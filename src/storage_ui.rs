mod actions;
mod message_time;
mod timeline;

use crate::{AppWindow, StoredContact, StoredConversation, StoredMessage, account, storage};
use crate::{
    messaging,
    relay::{Activity, Session},
    storage::account_status::AccountStatus,
};
use message_time::MessageTime;
use slint::{ComponentHandle, Model, ModelRc, VecModel, Weak};
use std::{
    collections::{HashMap, HashSet, VecDeque},
    io,
    sync::mpsc::{self, Sender},
    thread::{self, JoinHandle},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use storage::messaging::TimelineCursor;
use timeline::{History, ReadingPosition, TimelineWindow};

const PEER_STATUS_INTERVAL: Duration = Duration::from_millis(600);

enum Command {
    Open(String),
    Select(i64, i32, bool, Option<ReadingPosition>),
    Page(i64, i32, bool),
    Present(i64, i32),
    Restore(i64, i32, TimelineCursor),
    Quote(i64, i32, TimelineCursor),
    MarkRead(i64, i64),
    Create(String, i32, Option<ReadingPosition>),
    OpenContact(String, i32, Option<ReadingPosition>),
    RenameContact(String, String),
    SaveDraft(i64, String, i32),
    SendDraft(i64, String, i32),
    Resend(String),
    RetryMessaging,
    Relay(Activity),
    RefreshPeer(String),
    UploadReceipt,
    MessageAction(i64, i32, actions::MessageAction, Option<ReadingPosition>),
    DispatchControl,
    RetryControls,
    Close,
}

pub(crate) struct Worker {
    sender: Sender<Command>,
    thread: JoinHandle<()>,
}

impl Worker {
    pub(crate) fn relay_activity(&self) -> impl Fn(Activity) + Send + 'static {
        let sender = self.sender.clone();
        move |activity| {
            let _ = sender.send(Command::Relay(activity));
        }
    }

    pub fn opener(&self) -> impl Fn(&AppWindow) + Clone + Send + 'static {
        let sender = self.sender.clone();
        move |ui| {
            ui.set_storage_loading(true);
            ui.set_storage_error("".into());
            if ui.get_setup_page() == account::ENROLLMENT_PAGE {
                ui.set_setup_status("Securing local storage…".into());
            }
            if sender
                .send(Command::Open(ui.get_account_void_id().to_string()))
                .is_err()
            {
                worker_stopped(ui);
            }
        }
    }

    pub fn finish(self) {
        // All submitted draft edits are ahead of Close in the channel.
        let _ = self.sender.send(Command::Close);
        let _ = self.thread.join();
    }
}

fn unix_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

fn worker_stopped(ui: &AppWindow) {
    ui.set_storage_loading(false);
    ui.set_storage_ready(false);
    ui.set_storage_retryable(false);
    ui.set_storage_error("Local storage stopped. Close and reopen VOID.".into());
}

pub(crate) fn bind(ui: &AppWindow) -> io::Result<Worker> {
    bind_with(ui, account::open_enrollment)
}

fn bind_with(
    ui: &AppWindow,
    open: impl Fn(&str, &dyn Fn(&'static str)) -> storage::Result<(storage::Database, bool)>
    + Send
    + 'static,
) -> io::Result<Worker> {
    bind_timeline_positions(ui);
    let (sender, receiver) = mpsc::channel();
    let weak = ui.as_weak();
    let thread = thread::Builder::new()
        .name("void-storage".into())
        .spawn(move || {
            let mut database = None;
            let mut queued = None;
            let mut relay: Option<Session> = None;
            let mut messaging_ready = false;
            let mut history = History::default();
            let mut peer_refresh = VecDeque::new();
            let mut next_peer_refresh = Instant::now();
            let mut next_receipt = None;
            let mut receipt_not_before = Instant::now();
            let mut controls = actions::Dispatch::default();
            while let Some(mut command) = queued.take().or_else(|| {
                let peer_due = (!peer_refresh.is_empty()).then_some(next_peer_refresh);
                let Some(due) = peer_due
                    .into_iter()
                    .chain(next_receipt)
                    .chain(controls.due)
                    .min()
                else {
                    return receiver.recv().ok();
                };
                // Give queued sends, deliveries, and draft edits priority and
                // leave room for them in the relay's per-account command budget.
                match receiver.recv_timeout(due.saturating_duration_since(Instant::now())) {
                    Ok(command) => Some(command),
                    Err(mpsc::RecvTimeoutError::Timeout) if next_receipt == Some(due) => {
                        Some(Command::UploadReceipt)
                    }
                    Err(mpsc::RecvTimeoutError::Timeout) if controls.due == Some(due) => {
                        Some(Command::DispatchControl)
                    }
                    Err(mpsc::RecvTimeoutError::Timeout) => {
                        next_peer_refresh = Instant::now() + PEER_STATUS_INTERVAL;
                        peer_refresh.pop_front().map(Command::RefreshPeer)
                    }
                    Err(mpsc::RecvTimeoutError::Disconnected) => None,
                }
            }) {
                if let Command::SaveDraft(id, _, _) = command {
                    // Keep the latest edit in a burst without crossing a selection, create or close.
                    while let Ok(next) = receiver.try_recv() {
                        match next {
                            Command::SaveDraft(next_id, _, _) if next_id == id => command = next,
                            other => {
                                queued = Some(other);
                                break;
                            }
                        }
                    }
                }
                let refresh_receipts = matches!(
                    &command,
                    Command::Relay(_) | Command::RetryMessaging | Command::UploadReceipt
                );
                let selected = history.active.as_ref().map(|view| view.conversation);
                let opening_contact = matches!(&command, Command::OpenContact(..));
                match command {
                    Command::Open(id) => {
                        if database.is_some() {
                            continue;
                        }
                        let opened = open(&id, &|stage| {
                            let _ = weak.upgrade_in_event_loop(move |ui| {
                                ui.set_setup_status(stage.into());
                            });
                        })
                        .and_then(|(mut db, confirmed)| {
                            // Reuse the existing storage worker before loading the
                            // timeline. Recovery never decrypts Signal a second time.
                            db.recover_deferred_events()?;
                            load_lists(&db).map(|lists| (db, lists, confirmed))
                        });
                        match opened {
                            Ok((db, lists, confirmed)) => {
                                let pending_controls = db
                                    .pending_controls()
                                    .map(|ids| !ids.is_empty())
                                    .unwrap_or(true);
                                database = Some(db);
                                let _ = weak.upgrade_in_event_loop(move |ui| {
                                    lists.show(&ui);
                                    ui.set_pending_controls(pending_controls);
                                    ui.set_storage_loading(false);
                                    ui.set_storage_ready(true);
                                    if confirmed {
                                        finish_enrollment(&ui);
                                    }
                                    ui.invoke_storage_opened();
                                });
                            }
                            Err(error) => {
                                let _ = weak.upgrade_in_event_loop(move |ui| {
                                    ui.set_storage_loading(false);
                                    ui.set_storage_error(error.open_message().into());
                                });
                            }
                        }
                    }
                    Command::Select(id, selection, from_requests, previous) => {
                        if let Some(db) = &database {
                            history.remember(previous);
                            show_conversation(
                                &weak,
                                selection,
                                from_requests,
                                load_conversation(db, &mut history, id, selection),
                            );
                        }
                    }
                    Command::Page(id, selection, older) => {
                        if let (Some(db), Some(view)) = (&database, history.current(id, selection))
                        {
                            let result = view.page(db, older);
                            show_timeline_result(&weak, id, selection, result, false);
                        }
                    }
                    Command::Present(id, selection) => {
                        if let (Some(db), Some(view)) = (&database, history.current(id, selection))
                        {
                            let result = view.present(db);
                            show_timeline_result(&weak, id, selection, result, true);
                        }
                    }
                    Command::Restore(id, selection, cursor) => {
                        if let (Some(db), Some(view)) = (&database, history.current(id, selection))
                        {
                            let (result, to_present) = match view.restore(db, &cursor) {
                                Ok(Some(window)) => (Ok(window), false),
                                Ok(None) => (view.present(db), true),
                                Err(error) => (Err(error), false),
                            };
                            show_timeline_result(&weak, id, selection, result, to_present);
                        }
                    }
                    Command::Quote(id, selection, cursor) => {
                        if let (Some(db), Some(view)) = (&database, history.current(id, selection))
                        {
                            let result = view.restore(db, &cursor);
                            let key = cursor.key();
                            let _ = weak.upgrade_in_event_loop(move |ui| {
                                if !is_current_timeline(&ui, id, selection) {
                                    return;
                                }
                                ui.set_timeline_loading(false);
                                match result {
                                    Ok(Some(window)) => {
                                        ui.set_timeline_following(false);
                                        ui.set_timeline_anchor_key(key.into());
                                        ui.set_timeline_anchor_offset(16.0);
                                        show_window(&ui, window);
                                        advance_timeline_revision(&ui);
                                    }
                                    _ => ui.set_conversation_error(
                                        "Original message is unavailable.".into(),
                                    ),
                                }
                            });
                        }
                    }
                    Command::MarkRead(id, through) => {
                        if let Some(db) = &database
                            && db.mark_read(id, through).is_ok_and(|changed| changed)
                        {
                            let lists = load_lists(db);
                            let _ = weak.upgrade_in_event_loop(move |ui| {
                                if let Ok(lists) = lists {
                                    lists.show(&ui);
                                }
                            });
                        }
                    }
                    Command::Create(peer, selection, previous)
                    | Command::OpenContact(peer, selection, previous) => {
                        if let Some(db) = &database {
                            history.remember(previous);
                            let now = SystemTime::now()
                                .duration_since(UNIX_EPOCH)
                                .unwrap_or_default()
                                .as_millis() as i64;
                            let result = (if opening_contact && !db.contact(&peer).is_ok_and(|c| c.is_some()) {
                                Err(storage::Error::EventConflict)
                            } else { db.create_conversation(&peer, now) }).and_then(|id| {
                                Ok((
                                    load_lists(db)?,
                                    load_conversation(db, &mut history, id, selection)?,
                                ))
                            });
                            if result.is_err()
                                && let Some(view) = &mut history.active
                            {
                                view.selection = selection;
                            }
                            let _ = weak.upgrade_in_event_loop(move |ui| match result {
                                Ok((lists, detail)) => {
                                    lists.show(&ui);
                                    if ui.get_storage_selection_token() != selection {
                                        return;
                                    }
                                    ui.set_conversation_loading(false);
                                    if ui.get_creating_conversation() || opening_contact {
                                        ui.set_creating_conversation(false);
                                        ui.set_section(
                                            if detail.conversation.is_request {
                                                "Requests"
                                            } else {
                                                "Chats"
                                            }
                                            .into(),
                                        );
                                    }
                                    detail.show(&ui);
                                }
                                Err(_) if ui.get_storage_selection_token() == selection => {
                                    ui.set_conversation_loading(false);
                                    ui.set_conversation_error(
                                        "The conversation could not be opened. Try again.".into(),
                                    );
                                }
                                Err(_) => (),
                            });
                        }
                    }
                    Command::RenameContact(peer, name) => {
                        let error = if let Some(db) = &mut database {
                            match db.rename_contact(&peer, &name) {
                                Ok(true) => {
                                    refresh_messages(&weak, db, &mut history, None, None);
                                    if let Some(view) = &history.active
                                        && let Ok(conversation) = db.conversation(view.conversation)
                                    {
                                        let (id, selection) = (view.conversation, view.selection);
                                        let title = contact_title(conversation.display_name);
                                        let _ = weak.upgrade_in_event_loop(move |ui| {
                                            if is_current_timeline(&ui, id, selection) { ui.set_conversation_title(title); }
                                        });
                                    }
                                    ""
                                }
                                Ok(false) => "This contact is no longer available.",
                                Err(storage::Error::InvalidContactName) => "Use up to 100 characters without line breaks or control characters.",
                                Err(_) => "The name could not be saved. Try again.",
                            }
                        } else { "Local storage is unavailable." };
                        let _ = weak.upgrade_in_event_loop(move |ui| ui.invoke_contact_name_saved(peer.into(), error.into()));
                    }
                    Command::SaveDraft(id, body, revision) => {
                        if let Some(db) = &database {
                            let saved = db.save_draft(id, &body).is_ok();
                            let _ = weak.upgrade_in_event_loop(move |ui| {
                                ui.invoke_report_draft_saved(
                                    id.to_string().into(),
                                    revision,
                                    saved,
                                );
                            });
                        }
                    }
                    Command::Relay(Activity::Online(session)) if session.is_active() => {
                        if let Some(db) = &mut database {
                            messaging_ready = initialize_messaging(&weak, db, &session);
                            if messaging_ready {
                                if session.is_active() {
                                    actions::resume(&weak, &mut controls, db);
                                }
                                peer_refresh =
                                    db.peers_to_refresh(selected).unwrap_or_default().into();
                                next_peer_refresh = Instant::now();
                            }
                        }
                        relay = Some(session);
                    }
                    Command::Relay(Activity::Online(_)) => {}
                    Command::Relay(Activity::Offline(generation)) => {
                        if relay
                            .as_ref()
                            .is_some_and(|session| session.generation == generation)
                        {
                            relay = None;
                            messaging_ready = false;
                            peer_refresh.clear();
                            controls = actions::Dispatch::default();
                            let _ = weak
                                .upgrade_in_event_loop(|ui| ui.set_remote_actions_supported(false));
                        }
                    }
                    Command::RefreshPeer(peer) => {
                        if let (Some(db), Some(session)) = (&database, &relay)
                            && session.is_active()
                            && messaging::refresh_peer_status(db, session, &peer).is_ok()
                        {
                            show_selected_peer_status(&weak, db, selected);
                        }
                    }
                    Command::UploadReceipt => {
                        if let (Some(db), Some(session)) = (&mut database, &relay)
                            && messaging_ready
                            && session.is_active()
                        {
                            let _ = messaging::upload_due_receipt(db, session, unix_millis());
                        }
                        receipt_not_before = Instant::now() + Duration::from_secs(1);
                    }
                    Command::RetryMessaging => {
                        if let (Some(db), Some(session)) = (&mut database, &relay)
                            && !messaging_ready
                            && session.is_active()
                        {
                            messaging_ready = initialize_messaging(&weak, db, session);
                            if messaging_ready {
                                if session.is_active() {
                                    actions::resume(&weak, &mut controls, db);
                                }
                                peer_refresh =
                                    db.peers_to_refresh(selected).unwrap_or_default().into();
                                next_peer_refresh = Instant::now();
                            }
                        }
                    }
                    Command::Relay(Activity::Delivery {
                        generation,
                        sender,
                        event_id,
                        json,
                    }) => {
                        if let (Some(db), Some(session)) = (&mut database, &relay) {
                            // Drain only contiguous, already-queued deliveries. A saved
                            // message and its queued deletion reach the UI together;
                            // each event still commits and ACKs independently. Bound
                            // the batch so input commands cannot be starved.
                            let mut delivery = (generation, sender, event_id, json);
                            for index in 0..64 {
                                let (generation, sender, event_id, json) = delivery;
                                if session.generation == generation && session.is_active() {
                                    let result =
                                        messaging::receive(db, session, &sender, &event_id, &json);
                                    show_messaging_result(&weak, result);
                                }
                                if index == 63 {
                                    break;
                                }
                                match receiver.try_recv() {
                                    Ok(Command::Relay(Activity::Delivery {
                                        generation,
                                        sender,
                                        event_id,
                                        json,
                                    })) => {
                                        delivery = (generation, sender, event_id, json);
                                    }
                                    Ok(other) => {
                                        queued = Some(other);
                                        break;
                                    }
                                    Err(_) => break,
                                }
                            }
                            refresh_messages(&weak, db, &mut history, None, None);
                        }
                    }
                    Command::SendDraft(id, body, revision) => {
                        let result = if let Some(db) = &mut database {
                            let now = SystemTime::now()
                                .duration_since(UNIX_EPOCH)
                                .unwrap_or_default()
                                .as_millis() as i64;
                            let queued = db.queue_outgoing(id, &body, now);
                            match queued {
                                Ok((event_id, promoted)) => {
                                    let _ = weak.upgrade_in_event_loop(move |ui| {
                                        ui.invoke_draft_queued(
                                            id.to_string().into(),
                                            body.into(),
                                            revision,
                                        );
                                    });
                                    refresh_messages(
                                        &weak,
                                        db,
                                        &mut history,
                                        promoted.then_some(id),
                                        Some(id),
                                    );
                                    let result = messaging::send(
                                        db,
                                        relay.as_ref().filter(|_| messaging_ready),
                                        &event_id,
                                    );
                                    refresh_messages(&weak, db, &mut history, None, None);
                                    result
                                }
                                Err(error) => Err(error.into()),
                            }
                        } else {
                            Err(storage::Error::Unavailable.into())
                        };
                        finish_send(&weak, result);
                    }
                    Command::Resend(event_id) => {
                        let result = if let Some(db) = &mut database {
                            let result = messaging::send(
                                db,
                                relay.as_ref().filter(|_| messaging_ready),
                                &event_id,
                            );
                            refresh_messages(&weak, db, &mut history, None, None);
                            result
                        } else {
                            Err(storage::Error::Unavailable.into())
                        };
                        finish_send(&weak, result);
                    }
                    Command::MessageAction(id, selection, action, position) => {
                        history.remember(position);
                        if let Some(db) = &mut database {
                            // Preserve an already-submitted intent if this negotiated
                            // socket closes just before the worker handles it.
                            let remote = messaging_ready && relay.is_some();
                            let result = actions::apply(db, id, action, remote);
                            if result.is_ok() {
                                let pending = db
                                    .pending_controls()
                                    .map(|ids| !ids.is_empty())
                                    .unwrap_or(true);
                                let _ = weak.upgrade_in_event_loop(move |ui| {
                                    ui.set_pending_controls(pending)
                                });
                                refresh_messages(&weak, db, &mut history, None, None);
                                actions::show_reply(&weak, db, id, selection);
                                if remote {
                                    actions::resume(&weak, &mut controls, db);
                                }
                            }
                            let error = result.err().map_or("", |e| e.open_message());
                            let _ = weak.upgrade_in_event_loop(move |ui| {
                                ui.set_action_busy(false);
                                if is_current_timeline(&ui, id, selection)
                                    && (!error.is_empty() || !ui.get_pending_controls())
                                {
                                    ui.set_message_action_error(error.into());
                                }
                            });
                        } else {
                            let _ = weak.upgrade_in_event_loop(|ui| {
                                ui.set_action_busy(false);
                                ui.set_message_action_error("Local storage is unavailable.".into());
                            });
                        }
                    }
                    Command::RetryControls => {
                        controls = actions::Dispatch::default();
                        if messaging_ready
                            && relay.as_ref().is_some_and(Session::is_active)
                            && let Some(db) = &database
                        {
                            actions::resume(&weak, &mut controls, db);
                        }
                    }
                    Command::DispatchControl => {
                        if let (Some(db), Some(session)) = (&mut database, &relay)
                            && messaging_ready
                            && session.is_active()
                        {
                            let result = controls.run(db, session);
                            let pending = db
                                .pending_controls()
                                .map(|ids| !ids.is_empty())
                                .unwrap_or(true);
                            let error = result.err().map(|e| e.message());
                            let _ = weak.upgrade_in_event_loop(move |ui| {
                                ui.set_pending_controls(pending);
                                if let Some(error) = error {
                                    ui.set_message_action_error(error.into());
                                } else if !pending {
                                    ui.set_message_action_error("".into());
                                }
                            });
                        } else {
                            controls = actions::Dispatch::default();
                        }
                    }
                    Command::Close => break,
                }
                if refresh_receipts {
                    next_receipt = database
                        .as_ref()
                        .filter(|_| {
                            messaging_ready && relay.as_ref().is_some_and(Session::is_active)
                        })
                        .and_then(|db| {
                            let now = unix_millis();
                            match db.next_receipt_attempt(now) {
                                Ok(due) => due.map(|due| {
                                    (Instant::now()
                                        + Duration::from_millis(
                                            due.saturating_sub(now).max(0) as u64
                                        ))
                                    .max(receipt_not_before)
                                }),
                                Err(_) => Some(Instant::now() + Duration::from_secs(5)),
                            }
                        });
                }
            }
            if let Some(db) = database {
                let _ = db.close();
            }
        })?;

    let worker = Worker { sender, thread };
    ui.set_storage_retryable(true);
    let weak = ui.as_weak();
    let open = worker.opener();
    ui.on_retry_storage(move || {
        if let Some(ui) = weak.upgrade()
            && !ui.get_storage_loading()
            && !ui.get_storage_ready()
        {
            open(&ui);
        }
    });

    let weak = ui.as_weak();
    let sender = worker.sender.clone();
    ui.on_select_stored_conversation(move |id| {
        if let (Some(ui), Ok(id)) = (weak.upgrade(), id.parse()) {
            let previous = ReadingPosition::capture(&ui);
            let selection = begin_selection(&ui);
            let from_requests = ui.get_section() == "Requests";
            if sender
                .send(Command::Select(id, selection, from_requests, previous))
                .is_err()
            {
                worker_stopped(&ui);
            }
        }
    });

    bind_timeline(ui, worker.sender.clone());

    let sender = worker.sender.clone();
    ui.on_read_visible_conversation(move |id, through| {
        if let (Ok(id), Ok(through)) = (id.parse(), through.parse()) {
            let _ = sender.send(Command::MarkRead(id, through));
        }
    });

    let weak = ui.as_weak();
    let sender = worker.sender.clone();
    ui.on_create_stored_conversation(move |peer| {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        if !ui.get_storage_ready() || ui.get_conversation_loading() {
            return;
        }
        let peer = peer.trim();
        if !account::valid_void_id(peer) || peer == ui.get_account_void_id().as_str() {
            ui.set_conversation_error("Enter another person's VOID-ID.".into());
            return;
        }
        let previous = ReadingPosition::capture(&ui);
        let selection = begin_selection(&ui);
        if sender
            .send(Command::Create(peer.to_owned(), selection, previous))
            .is_err()
        {
            worker_stopped(&ui);
        }
    });

    bind_drafts(ui, worker.sender.clone());
    bind_contacts(ui, worker.sender.clone());
    bind_sending(ui, worker.sender.clone());
    actions::bind(ui, worker.sender.clone());
    let weak = ui.as_weak();
    let sender = worker.sender.clone();
    ui.on_retry_messaging(move || {
        if let Some(ui) = weak.upgrade()
            && ui.get_publication_retryable()
            && ui.get_connection_status() == "Connected"
        {
            ui.set_publication_retryable(false);
            ui.set_publishing_keys(true);
            if sender.send(Command::RetryMessaging).is_err() {
                ui.set_publishing_keys(false);
                worker_stopped(&ui);
            }
        }
    });
    Ok(worker)
}

struct PendingDraft {
    body: slint::SharedString,
    revision: i32,
    failed: bool,
}

fn bind_drafts(ui: &AppWindow, sender: Sender<Command>) {
    use std::{cell::RefCell, rc::Rc};
    let pending = Rc::new(RefCell::new(HashMap::<i64, PendingDraft>::new()));
    let weak = ui.as_weak();
    let edited = pending.clone();
    ui.on_save_stored_draft(move |body| {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        let Ok(id) = ui.get_selected_conversation().parse() else {
            return;
        };
        let revision = ui.get_storage_draft_token().wrapping_add(1);
        ui.set_storage_draft_token(revision);
        let failed = sender
            .send(Command::SaveDraft(id, body.to_string(), revision))
            .is_err();
        let mut drafts = edited.borrow_mut();
        drafts.insert(
            id,
            PendingDraft {
                body,
                revision,
                failed,
            },
        );
        ui.set_unsaved_draft_failure(drafts.values().any(|draft| draft.failed));
        draft_feedback(&ui, Some(failed));
    });

    let weak = ui.as_weak();
    let completed = pending.clone();
    ui.on_report_draft_saved(move |id, revision, saved| {
        let (Some(ui), Ok(number)) = (weak.upgrade(), id.parse()) else {
            return;
        };
        let mut drafts = completed.borrow_mut();
        let Some(draft) = drafts
            .get_mut(&number)
            .filter(|draft| draft.revision == revision)
        else {
            return;
        };
        if saved {
            drafts.remove(&number);
        } else {
            draft.failed = true;
        }
        if ui.get_selected_conversation() == id {
            draft_feedback(&ui, (!saved).then_some(true));
            if saved {
                ui.set_draft_status("Draft saved on this device".into());
            }
        }
        ui.set_unsaved_draft_failure(drafts.values().any(|draft| draft.failed));
    });

    let weak = ui.as_weak();
    let loaded = pending.clone();
    ui.on_load_stored_draft(move |id, body| {
        let (Some(ui), Ok(id)) = (weak.upgrade(), id.parse()) else {
            return;
        };
        let drafts = loaded.borrow();
        let pending = drafts.get(&id);
        ui.set_stored_draft(pending.map_or(body, |draft| draft.body.clone()));
        draft_feedback(&ui, pending.map(|draft| draft.failed));
    });

    let weak = ui.as_weak();
    ui.on_draft_queued(move |id, submitted, revision| {
        let (Some(ui), Ok(number)) = (weak.upgrade(), id.parse()) else {
            return;
        };
        let mut drafts = pending.borrow_mut();
        if drafts
            .get(&number)
            .is_some_and(|draft| draft.revision == revision)
        {
            drafts.remove(&number);
        }
        if ui.get_selected_conversation() == id
            && ui.get_storage_draft_token() == revision
            && ui.get_stored_draft() == submitted
        {
            ui.set_stored_draft("".into());
            ui.set_reply_event_id("".into());
            ui.set_reply_author_id("".into());
            ui.set_reply_author("".into());
            ui.set_reply_preview("".into());
            draft_feedback(&ui, None);
        }
        ui.set_unsaved_draft_failure(drafts.values().any(|draft| draft.failed));
    });
}

fn bind_sending(ui: &AppWindow, sender: Sender<Command>) {
    ui.on_has_message_content(|body| !body.trim().is_empty());
    let weak = ui.as_weak();
    let send = sender.clone();
    ui.on_send_message(move || {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        if ui.get_update_required()
            || !ui.get_storage_ready()
            || !ui.get_conversation_can_compose()
            || ui.get_conversation_loading()
            || ui.get_send_busy()
            || ui.get_stored_draft().trim().is_empty()
        {
            return;
        }
        let Ok(id) = ui.get_selected_conversation().parse() else {
            return;
        };
        ui.set_send_busy(true);
        if send
            .send(Command::SendDraft(
                id,
                ui.get_stored_draft().to_string(),
                ui.get_storage_draft_token(),
            ))
            .is_err()
        {
            worker_stopped(&ui);
            ui.set_send_busy(false);
        }
    });

    let weak = ui.as_weak();
    ui.on_resend_message(move |event_id| {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        if ui.get_update_required()
            || !ui.get_messaging_ready()
            || ui.get_send_busy()
            || event_id.is_empty()
            || ui.get_conversation_peer_deleted()
        {
            return;
        }
        ui.set_send_busy(true);
        if sender.send(Command::Resend(event_id.to_string())).is_err() {
            worker_stopped(&ui);
            ui.set_send_busy(false);
        }
    });
}

fn initialize_messaging(
    weak: &Weak<AppWindow>,
    db: &mut storage::Database,
    session: &Session,
) -> bool {
    let _ = weak.upgrade_in_event_loop(|ui| ui.set_publishing_keys(true));
    let result = messaging::publish(db, session);
    let ready = result.is_ok();
    let interrupted = matches!(
        &result,
        Err(messaging::Error::Relay(
            crate::relay::RequestError::Interrupted
        ))
    );
    let (notice, retryable) = result
        .as_ref()
        .err()
        .map_or(("", false), messaging::publication_failure);
    let session = session.clone();
    let _ = weak.upgrade_in_event_loop(move |ui| {
        ui.set_publishing_keys(false);
        ui.set_messaging_ready(ready && session.is_active());
        ui.set_remote_actions_supported(ready && session.is_active());
        if !interrupted || ui.get_publication_error().is_empty() {
            ui.set_publication_error(notice.into());
            ui.set_publication_retryable(retryable);
        }
        if ready {
            finish_enrollment(&ui);
        }
    });
    ready
}

fn finish_enrollment(ui: &AppWindow) {
    if ui.get_setup_page() == account::ENROLLMENT_PAGE
        && !ui.get_removal_busy()
        && ui.get_removal_error().is_empty()
    {
        ui.set_setup_status("Messaging ready".into());
        ui.set_section("Chats".into());
        ui.set_setup_page(0);
    }
}

fn show_messaging_result(weak: &Weak<AppWindow>, result: Result<(), messaging::Error>) {
    let message = result.as_ref().err().map_or("", messaging::Error::message);
    let _ = weak.upgrade_in_event_loop(move |ui| ui.set_messaging_notice(message.into()));
}

fn finish_send(weak: &Weak<AppWindow>, result: Result<(), messaging::Error>) {
    let message = result.as_ref().err().map_or("", messaging::Error::message);
    let _ = weak.upgrade_in_event_loop(move |ui| {
        ui.set_send_busy(false);
        ui.set_messaging_notice(message.into());
    });
}

fn bind_timeline(ui: &AppWindow, sender: Sender<Command>) {
    let weak = ui.as_weak();
    let page = sender.clone();
    ui.on_page_timeline(move |older| {
        if let Some(ui) = weak.upgrade() {
            request_timeline(&ui, &page, |id, selection| {
                Command::Page(id, selection, older)
            });
        }
    });
    let weak = ui.as_weak();
    let present = sender.clone();
    ui.on_jump_to_present(move || {
        if let Some(ui) = weak.upgrade() {
            request_timeline(&ui, &present, Command::Present);
        }
    });
    let weak = ui.as_weak();
    let quote = sender.clone();
    ui.on_open_quote(move |key| {
        if let (Some(ui), Some(cursor)) = (weak.upgrade(), TimelineCursor::from_key(&key)) {
            request_timeline(&ui, &quote, |id, selection| {
                Command::Quote(id, selection, cursor)
            });
        }
    });
    let weak = ui.as_weak();
    ui.on_restore_timeline(move |key| {
        if let (Some(ui), Some(cursor)) = (weak.upgrade(), TimelineCursor::from_key(&key)) {
            request_timeline(&ui, &sender, |id, selection| {
                Command::Restore(id, selection, cursor)
            });
        }
    });
}

fn request_timeline(
    ui: &AppWindow,
    sender: &Sender<Command>,
    command: impl FnOnce(i64, i32) -> Command,
) {
    if !ui.get_storage_ready() || ui.get_conversation_loading() || ui.get_timeline_loading() {
        return;
    }
    let Ok(id) = ui.get_selected_conversation().parse() else {
        return;
    };
    ui.set_timeline_loading(true);
    ui.set_conversation_error("".into());
    if sender
        .send(command(id, ui.get_storage_selection_token()))
        .is_err()
    {
        ui.set_timeline_loading(false);
        worker_stopped(ui);
    }
}

fn pin_to_present(ui: &AppWindow) {
    ui.set_timeline_following(true);
    ui.set_timeline_anchor_key("".into());
    ui.set_timeline_anchor_offset(0.0);
    advance_timeline_revision(ui);
}

fn advance_timeline_revision(ui: &AppWindow) {
    ui.set_timeline_revision(ui.get_timeline_revision().wrapping_add(1));
    ui.window().request_redraw();
}

fn is_current_timeline(ui: &AppWindow, id: i64, selection: i32) -> bool {
    ui.get_storage_selection_token() == selection
        && ui.get_selected_conversation().as_str() == id.to_string()
}

fn show_timeline_result(
    weak: &Weak<AppWindow>,
    id: i64,
    selection: i32,
    result: storage::Result<TimelineWindow>,
    to_present: bool,
) {
    let _ = weak.upgrade_in_event_loop(move |ui| {
        if !is_current_timeline(&ui, id, selection) {
            return;
        }
        ui.set_timeline_loading(false);
        match result {
            Ok(window) => update_timeline(&ui, window, to_present, false),
            Err(_) => {
                ui.set_conversation_error("Message history could not be loaded. Try again.".into())
            }
        }
    });
}

/// A bounded cache of settled row geometry, owned by this window's UI thread.
/// It has no per-scroll model writes and never contains message bodies.
fn bind_timeline_positions(ui: &AppWindow) {
    use std::{cell::RefCell, rc::Rc};
    let positions = Rc::new(RefCell::new(HashMap::<slint::SharedString, f32>::new()));
    let cache = positions.clone();
    ui.on_remember_timeline_position(move |key, y| {
        if !key.is_empty() && y.is_finite() && y >= 0.0 {
            cache.borrow_mut().insert(key, y);
        }
    });
    let cache = positions.clone();
    ui.on_timeline_position(move |key| cache.borrow().get(&key).copied().unwrap_or(-1.0));
    let weak = ui.as_weak();
    ui.on_prune_timeline_positions(move || {
        if let Some(ui) = weak.upgrade() {
            let keys: HashSet<_> = ui.get_stored_messages().iter().map(|row| row.key).collect();
            positions.borrow_mut().retain(|key, _| keys.contains(key));
        }
    });
}

fn update_timeline(
    ui: &AppWindow,
    window: TimelineWindow,
    to_present: bool,
    content_refresh: bool,
) {
    let mut restore_anchor = None;
    if !to_present {
        if window.has_newer && !ui.get_timeline_has_newer() && ui.get_timeline_following() {
            ui.invoke_jump_to_present();
            return;
        }
        let key = ui.get_timeline_anchor_key();
        if let Some(cursor) = TimelineCursor::from_key(&key)
            && !window.messages.iter().any(|row| row.cursor == cursor)
        {
            let previous = ui.get_stored_messages();
            if let Some(index) = previous
                .iter()
                .position(|row| row.key == key)
                .filter(|_| content_refresh)
            {
                // Choose a survivor, but preserve ITS measured screen position,
                // not the deleted row's offset. The latter causes a visible jump.
                let neighbor = previous
                    .iter()
                    .skip(index + 1)
                    .chain(
                        previous
                            .iter()
                            .take(index)
                            .collect::<Vec<_>>()
                            .into_iter()
                            .rev(),
                    )
                    .find(|row| window.messages.iter().any(|m| row.key == m.cursor.key()));
                let survivor = neighbor
                    .map(|row| row.key)
                    .or_else(|| window.messages.first().map(|m| m.cursor.key().into()))
                    .unwrap_or_default();
                restore_anchor = Some((survivor.clone(), measured_timeline_offset(ui, &survivor)));
            } else {
                // Scroll input may have moved beyond the overlap while a page was loading.
                ui.invoke_restore_timeline(key);
                return;
            }
        } else if content_refresh {
            // Normalize a surviving anchor against actual, pre-mutation layout
            // as well, including movement that happened while the worker ran.
            restore_anchor = Some((key.clone(), measured_timeline_offset(ui, &key)));
        }
    }
    show_window(ui, window);
    if let Some((key, offset)) = restore_anchor {
        // Apply against the reconciled rows, never the old delegate geometry.
        if let Some(offset) = offset {
            ui.set_timeline_anchor_offset(offset);
        }
        ui.set_timeline_anchor_key(key);
    }
    if to_present {
        pin_to_present(ui);
    }
}

fn measured_timeline_offset(ui: &AppWindow, key: &slint::SharedString) -> Option<f32> {
    let viewport = ui.get_timeline_viewport_y();
    let y = ui.invoke_timeline_position(key.clone());
    (y >= 0.0).then_some(y + viewport)
}

fn refresh_messages(
    weak: &Weak<AppWindow>,
    db: &storage::Database,
    history: &mut History,
    promoted: Option<i64>,
    force_present: Option<i64>,
) {
    let selected = history.active.as_ref().map(|view| view.conversation);
    if let Some(view) = &history.active {
        actions::show_reply(weak, db, view.conversation, view.selection);
    }
    show_selected_peer_status(weak, db, selected);
    let lists = load_lists(db);
    let timeline = history.active.as_mut().and_then(|view| {
        let id = view.conversation;
        let conversation = db.conversation(id).ok()?;
        let to_present = force_present == Some(id);
        let window = if to_present {
            view.present(db).ok()?
        } else {
            view.refresh(db).ok()?
        };
        Some((
            id,
            view.selection,
            window,
            conversation.unread_through,
            to_present,
        ))
    });
    let _ = weak.upgrade_in_event_loop(move |ui| {
        if let Ok(lists) = lists {
            lists.show(&ui);
        }
        if promoted.is_some_and(|id| ui.get_selected_conversation().as_str() == id.to_string()) {
            ui.set_conversation_is_request(false);
            if ui.get_section() == "Requests" && !ui.get_conversation_loading() {
                ui.set_section("Chats".into());
            }
        }
        if let Some((id, selection, window, unread_through, to_present)) = timeline
            && is_current_timeline(&ui, id, selection)
        {
            update_timeline(&ui, window, to_present, true);
            ui.set_conversation_read_through(read_through_label(unread_through).into());
        }
    });
}

fn show_selected_peer_status(
    weak: &Weak<AppWindow>,
    db: &storage::Database,
    selected: Option<i64>,
) {
    if let Some(id) = selected
        && let Ok(status) = db.conversation_account_status(id)
    {
        let id = id.to_string();
        let _ = weak.upgrade_in_event_loop(move |ui| {
            if ui.get_selected_conversation() == id {
                ui.set_conversation_peer_deleted(status == AccountStatus::Deleted);
            }
        });
    }
}

fn draft_feedback(ui: &AppWindow, pending: Option<bool>) {
    ui.set_draft_status(
        match pending {
            Some(true) => "Draft could not be saved",
            Some(false) => "Saving draft…",
            None => "Drafts stay on this device",
        }
        .into(),
    );
    ui.set_draft_save_failed(pending == Some(true));
}

fn begin_selection(ui: &AppWindow) -> i32 {
    let selection = ui.get_storage_selection_token().wrapping_add(1);
    ui.set_storage_selection_token(selection);
    ui.set_conversation_loading(true);
    ui.set_timeline_loading(false);
    ui.set_conversation_error("".into());
    selection
}

struct Lists {
    chats: Vec<ConversationRow>,
    requests: Vec<ConversationRow>,
    contacts: Vec<storage::Contact>,
}

struct ConversationRow {
    id: i64,
    display_name: Option<String>,
    preview: String,
    activity_at: i64,
    unread: bool,
}

impl Lists {
    fn show(self, ui: &AppWindow) {
        ui.set_chats_unread(self.chats.iter().any(|row| row.unread));
        ui.set_requests_unread(self.requests.iter().any(|row| row.unread));
        let today = chrono::Local::now().date_naive();
        ui.set_stored_chats(conversation_model(self.chats, today));
        ui.set_stored_requests(conversation_model(self.requests, today));
        let contacts: Vec<_> = self
            .contacts
            .into_iter()
            .map(|contact| StoredContact {
                title: contact_title(contact.display_name.clone()),
                name: contact.display_name.unwrap_or_default().into(),
                void_id: contact.void_id.into(),
            })
            .collect();
        ui.set_stored_contacts(ModelRc::new(VecModel::from(contacts)));
    }
}

fn load_lists(db: &storage::Database) -> storage::Result<Lists> {
    let chats = db.conversations(false)?;
    let requests = db.conversations(true)?;
    let mut previews = db.last_message_previews(chats.iter().chain(&requests).map(|row| row.id))?;
    Ok(Lists {
        chats: presentation_rows(chats, &mut previews),
        requests: presentation_rows(requests, &mut previews),
        contacts: db.contacts()?,
    })
}

fn presentation_rows(
    conversations: Vec<storage::Conversation>,
    previews: &mut HashMap<i64, storage::messaging::MessagePreview>,
) -> Vec<ConversationRow> {
    conversations
        .into_iter()
        .map(|row| {
            let (preview, activity_at) = match previews.remove(&row.id) {
                Some(message) => {
                    let mut preview = message
                        .body
                        .split_whitespace()
                        .collect::<Vec<_>>()
                        .join(" ");
                    if let Some((end, _)) = preview.char_indices().nth(140) {
                        preview.truncate(end);
                        preview.push('…');
                    }
                    (preview, row.last_activity_at.max(message.created_at))
                }
                None => (String::from("No messages yet"), row.created_at),
            };
            ConversationRow {
                id: row.id,
                display_name: row.display_name,
                preview,
                activity_at,
                unread: row.unread_through.is_some(),
            }
        })
        .collect()
}

fn bind_contacts(ui: &AppWindow, sender: Sender<Command>) {
    let weak = ui.as_weak();
    let open = sender.clone();
    ui.on_open_stored_contact(move |peer| {
        if let Some(ui) = weak.upgrade()
            && ui.get_storage_ready()
            && !ui.get_conversation_loading()
            && !ui.get_update_required()
        {
            let previous = ReadingPosition::capture(&ui);
            let selection = begin_selection(&ui);
            if open
                .send(Command::OpenContact(peer.into(), selection, previous))
                .is_err()
            {
                worker_stopped(&ui);
            }
        }
    });
    let weak = ui.as_weak();
    ui.on_save_contact_name(move |peer, name| {
        if let Some(ui) = weak.upgrade()
            && ui.get_storage_ready()
            && !ui.get_contact_save_busy()
            && !ui.get_update_required()
        {
            ui.set_contact_save_busy(true);
            ui.set_contact_action_error("".into());
            if sender
                .send(Command::RenameContact(peer.into(), name.into()))
                .is_err()
            {
                ui.set_contact_save_busy(false);
                ui.set_contact_action_error("Local storage stopped. Reopen VOID.".into());
            }
        }
    });
}

fn contact_title(saved: Option<String>) -> slint::SharedString {
    saved
        .filter(|name| !name.trim().is_empty())
        .unwrap_or_else(|| "Contact".into())
        .into()
}

fn conversation_model(
    list: Vec<ConversationRow>,
    today: chrono::NaiveDate,
) -> ModelRc<StoredConversation> {
    let rows: Vec<_> = list
        .into_iter()
        .map(|row| StoredConversation {
            id: row.id.to_string().into(),
            title: contact_title(row.display_name),
            preview: row.preview.into(),
            activity: MessageTime::local(row.activity_at)
                .map(|time| time.activity_label(today))
                .unwrap_or_default()
                .into(),
            unread: row.unread,
        })
        .collect();
    ModelRc::new(VecModel::from(rows))
}

struct Detail {
    conversation: storage::Conversation,
    account_status: AccountStatus,
    window: TimelineWindow,
    anchor: Option<timeline::Anchor>,
    draft: String,
    reply: Option<storage::message_events::MessageReference>,
    reply_preview: (String, String),
}

impl Detail {
    fn show(self, ui: &AppWindow) {
        let Self {
            conversation,
            account_status,
            window,
            anchor,
            draft,
            reply,
            reply_preview,
        } = self;
        ui.set_conversation_read_through("".into());
        ui.set_selected_conversation(conversation.id.to_string().into());
        ui.set_conversation_title(contact_title(conversation.display_name));
        ui.set_conversation_is_request(conversation.is_request);
        ui.set_conversation_peer_deleted(account_status == AccountStatus::Deleted);
        show_window(ui, window);
        if let Some(anchor) = anchor {
            anchor.restore(ui);
            advance_timeline_revision(ui);
        } else {
            pin_to_present(ui);
        }
        ui.set_conversation_read_through(read_through_label(conversation.unread_through).into());
        ui.invoke_load_stored_draft(ui.get_selected_conversation(), draft.into());
        ui.set_reply_event_id(reply.as_ref().map_or("", |r| r.event_id.as_str()).into());
        ui.set_reply_author_id(
            reply
                .as_ref()
                .map_or("", |r| r.author_void_id.as_str())
                .into(),
        );
        ui.set_reply_author(reply_preview.0.into());
        ui.set_reply_preview(reply_preview.1.into());
    }
}

fn load_conversation(
    db: &storage::Database,
    history: &mut History,
    id: i64,
    selection: i32,
) -> storage::Result<Detail> {
    let result = (|| {
        let conversation = db.conversation(id)?;
        let account_status = db.conversation_account_status(id)?;
        let draft = db.draft(id)?.unwrap_or_default();
        let (window, anchor) = history.open(db, id, selection)?;
        Ok(Detail {
            reply: db.reply_draft(id)?,
            reply_preview: db.reply_draft_preview(id)?,
            conversation,
            account_status,
            window,
            anchor,
            draft,
        })
    })();
    if result.is_err()
        && let Some(view) = &mut history.active
    {
        view.selection = selection;
    }
    result
}

pub(crate) fn track_local_calendar(ui: &AppWindow) -> slint::Timer {
    let weak = ui.as_weak();
    let mut last_days = None;
    let mut update = move || {
        if let Some(ui) = weak.upgrade() {
            let days = message_time::local_separator_days();
            if last_days.as_ref() != Some(&days) {
                ui.set_timeline_today(days.0.as_str().into());
                ui.set_timeline_yesterday(days.1.as_str().into());
                last_days = Some(days);
            }
        }
    };
    update();

    let timer = slint::Timer::default();
    timer.start(slint::TimerMode::Repeated, Duration::from_secs(1), update);
    timer
}

fn show_window(ui: &AppWindow, window: TimelineWindow) {
    let changed_edges = ui.get_timeline_has_older() != window.has_older
        || ui.get_timeline_has_newer() != window.has_newer;
    ui.set_timeline_has_older(window.has_older);
    ui.set_timeline_has_newer(window.has_newer);
    let mut previous_day = None;
    let messages: Vec<_> = window
        .messages
        .into_iter()
        .filter(|message| !message.presentation.deleted)
        .map(|message| {
            let time = MessageTime::local(message.created_at);
            let mut heading = String::new();
            if let Some(time) = &time
                && previous_day != Some(time.day())
            {
                previous_day = Some(time.day());
                heading = time.calendar_label();
            }
            StoredMessage {
                author_id: message
                    .reference
                    .as_ref()
                    .map_or("", |r| r.author_void_id.as_str())
                    .into(),
                event_id: message
                    .reference
                    .as_ref()
                    .map_or("", |r| r.event_id.as_str())
                    .into(),
                deleted: message.presentation.deleted,
                quote: message.presentation.quote.into(),
                quote_author: message.presentation.author.into(),
                quote_target: message.presentation.target.into(),
                reactions: ModelRc::new(VecModel::from(
                    message
                        .presentation
                        .reactions
                        .into_iter()
                        .map(|(emoji, count, mine)| crate::StoredReaction {
                            emoji: emoji.into(),
                            count,
                            mine,
                        })
                        .collect::<Vec<_>>(),
                )),
                key: message.cursor.key().into(),
                body: storage::messaging::without_edge_blank_lines(&message.body)
                    .unwrap_or_default()
                    .into(),
                day: heading.into(),
                time: time
                    .as_ref()
                    .map(MessageTime::clock_label)
                    .unwrap_or_default()
                    .into(),
                exact_time: time
                    .as_ref()
                    .map(MessageTime::exact_label)
                    .unwrap_or_default()
                    .into(),
                outgoing: message.outgoing,
                status: message.status.into(),
                retry_event: message.retry_event.into(),
            }
        })
        .collect();
    let changed = reconcile_messages(ui, messages);
    ui.invoke_prune_timeline_positions();
    if changed || changed_edges {
        advance_timeline_revision(ui);
    }
}

fn reconcile_messages(ui: &AppWindow, messages: Vec<StoredMessage>) -> bool {
    let model = ui.get_stored_messages();
    let Some(model) = model.as_any().downcast_ref::<VecModel<StoredMessage>>() else {
        ui.set_timeline_reflowing(true);
        ui.set_stored_messages(ModelRc::new(VecModel::from(messages)));
        return true;
    };
    // The same stable key keeps an outgoing bubble through commit, retry, and receipt.
    // Retained keys preserve their order across adjacent windows.
    let keys: HashSet<_> = messages.iter().map(|row| row.key.clone()).collect();
    let mut changed = false;
    for index in (0..model.row_count()).rev() {
        if model
            .row_data(index)
            .is_some_and(|row| !keys.contains(&row.key))
        {
            ui.set_timeline_reflowing(true);
            model.remove(index);
            changed = true;
        }
    }
    for (index, mut row) in messages.into_iter().enumerate() {
        if let Some(current) = model.row_data(index) {
            if current.reactions.iter().collect::<Vec<_>>()
                == row.reactions.iter().collect::<Vec<_>>()
            {
                row.reactions = current.reactions;
            }
        }
        match model.row_data(index) {
            Some(current) if current == row => continue,
            Some(current) if current.key == row.key => {
                ui.set_timeline_reflowing(true);
                model.set_row_data(index, row);
            }
            _ => {
                ui.set_timeline_reflowing(true);
                model.insert(index, row);
            }
        }
        changed = true;
    }
    changed
}

fn read_through_label(through: Option<i64>) -> String {
    through
        .map(|sequence| sequence.to_string())
        .unwrap_or_default()
}

fn show_conversation(
    weak: &Weak<AppWindow>,
    selection: i32,
    from_requests: bool,
    detail: storage::Result<Detail>,
) {
    let _ = weak.upgrade_in_event_loop(move |ui| {
        if ui.get_storage_selection_token() != selection {
            return;
        }
        ui.set_conversation_loading(false);
        match detail {
            Ok(detail) => {
                // A request selected while its reply was being queued may already be a chat.
                if from_requests
                    && ui.get_section() == "Requests"
                    && !detail.conversation.is_request
                {
                    ui.set_section("Chats".into());
                }
                detail.show(&ui);
            }
            Err(_) => ui.set_conversation_error(
                "The saved conversation could not be loaded. Try opening it again.".into(),
            ),
        }
    });
}

#[cfg(all(test, windows))]
pub(crate) mod tests {
    mod contacts;
    mod deletion_scrolling;
    mod history_scrolling;
    mod message_actions;
    mod offline_deletion;
    mod presentation;
    mod silent_deletion;
    mod update_required;
    use super::*;
    use crate::storage::{Database, DeliveryState, Direction, Message, Owner, tests::TestDir};
    use slint::{Model, Timer, TimerMode};
    use std::{cell::Cell, process::Command, time::Duration};

    pub(crate) fn bind_fixture(
        ui: &AppWindow,
        owner: impl Fn(&str) -> (std::path::PathBuf, ed25519_dalek::SigningKey) + Send + 'static,
    ) -> io::Result<Worker> {
        bind_with(ui, move |id, _| {
            let (root, key) = owner(id);
            let public = key.verifying_key();
            let mut db = Database::open(
                &root,
                Owner {
                    void_id: id,
                    public_key: public.as_bytes(),
                },
            )?;
            db.initialize_signal(id, &key)?;
            let confirmed = db.prepare_publication()?.is_none();
            db.recover_interrupted_sends()?;
            Ok((db, confirmed))
        })
    }

    #[test]
    fn conversation_previews_follow_the_visible_timeline_and_preserve_saved_names() {
        let temp = TestDir::new();
        let key = [8; 32];
        let owner = Owner {
            void_id: "41",
            public_key: &key,
        };
        let mut db = Database::open(&temp.0, owner).unwrap();
        let id = db.create_conversation("123", 100).unwrap();
        db.update_conversation(id, Some("Saved contact"), 100)
            .unwrap();
        db.save_message(&Message {
            id: "preview".into(),
            conversation_id: id,
            direction: Direction::Incoming,
            body: "Earlier message".into(),
            created_at: 200,
            delivery_state: DeliveryState::Received,
        })
        .unwrap();
        db.queue_outgoing(id, "Queued reply\nwith two lines", 300)
            .unwrap();
        let request = db.create_conversation("456", 400).unwrap();
        db.set_request(request, true).unwrap();

        let lists = load_lists(&db).unwrap();
        assert_eq!(
            lists.chats[0].display_name.as_deref(),
            Some("Saved contact")
        );
        assert_eq!(lists.chats[0].preview, "Queued reply with two lines");
        assert_eq!(lists.chats[0].activity_at, 300);
        assert_eq!(lists.requests[0].preview, "No messages yet");
        assert_eq!(
            contact_title(lists.requests.into_iter().next().unwrap().display_name),
            "Contact"
        );
    }

    pub(crate) fn in_test_process(test: &str) -> bool {
        const CHILD: &str = "VOID_STORAGE_UI_TEST_CHILD";
        if std::env::var_os(CHILD).is_none() {
            // Slint's threaded testing backend is process-global. Keep it separate from the relay UI test.
            let output = Command::new(std::env::current_exe().unwrap())
                .args(["--exact", test])
                .env(CHILD, "1")
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            return false;
        }
        true
    }

    #[test]
    fn restores_the_local_conversation_and_latest_draft_in_slint() {
        if !in_test_process(
            "storage_ui::tests::restores_the_local_conversation_and_latest_draft_in_slint",
        ) {
            return;
        }
        let account_key = ed25519_dalek::SigningKey::from_bytes(&[5; 32]);
        let public_key = account_key.verifying_key();
        let owner = Owner {
            void_id: "41",
            public_key: public_key.as_bytes(),
        };
        let temp = TestDir::new();
        let mut db = Database::open(&temp.0, owner).unwrap();
        db.initialize_signal(owner.void_id, &account_key).unwrap();
        let publication = db.prepare_publication().unwrap().unwrap();
        db.confirm_publication(&publication.id).unwrap();
        let id = db.create_conversation("123", 100).unwrap();
        let stored_body = "\nStored test message\n\nSecond paragraph\n\n\n";
        db.save_message(&Message {
            id: "test-1".into(),
            conversation_id: id,
            direction: Direction::Incoming,
            body: stored_body.into(),
            created_at: 200,
            delivery_state: DeliveryState::Received,
        })
        .unwrap();
        db.save_draft(id, "Earlier draft").unwrap();
        db.close().unwrap();

        i_slint_backend_testing::init_integration_test_with_system_time();
        let ui = AppWindow::new().unwrap();
        ui.set_account_void_id(owner.void_id.into());
        ui.set_setup_page(account::ENROLLMENT_PAGE);
        let this_thread = thread::current().id();
        let root = temp.0.clone();
        let worker = bind_fixture(&ui, move |id| {
            assert_ne!(this_thread, thread::current().id());
            assert_eq!(id, "41");
            (root.clone(), account_key.clone())
        })
        .unwrap();

        worker.opener()(&ui);
        let poll = Timer::default();
        let weak = ui.as_weak();
        poll.start(TimerMode::Repeated, Duration::from_millis(10), move || {
            let ui = weak.upgrade().unwrap();
            if ui.get_storage_ready()
                && ui.get_selected_conversation().is_empty()
                && !ui.get_conversation_loading()
            {
                assert_eq!(ui.get_setup_page(), 0);
                assert_eq!(ui.get_connection_status(), "Disconnected");
                assert_eq!(ui.get_stored_chats().row_count(), 1);
                assert!(ui.get_chats_unread());
                let row = ui.get_stored_chats().row_data(0).unwrap();
                assert!(row.unread);
                assert_eq!(row.title, "Contact");
                assert_eq!(row.preview, "Stored test message Second paragraph");
                ui.invoke_select_stored_conversation(id.to_string().into());
            } else if ui.get_selected_conversation() == id.to_string()
                && !ui.get_conversation_loading()
            {
                assert_eq!(
                    ui.get_stored_messages().row_data(0).unwrap().body,
                    "Stored test message\n\nSecond paragraph"
                );
                assert_eq!(ui.get_stored_draft(), "Earlier draft");
                if !ui.get_window_foreground() {
                    assert!(ui.get_chats_unread());
                    ui.set_window_foreground(true);
                    return;
                }
                if ui.get_chats_unread() {
                    return;
                }
                ui.invoke_save_stored_draft("Intermediate edit".into());
                ui.invoke_save_stored_draft("Latest edit".into());
                ui.invoke_create_stored_conversation("456".into());
            } else if !ui.get_selected_conversation().is_empty()
                && ui.get_selected_conversation() != id.to_string()
            {
                assert!(ui.get_stored_draft().is_empty());
                assert_eq!(ui.get_stored_chats().row_count(), 2);
                slint::quit_event_loop().unwrap();
            }
        });
        let watchdog = Timer::default();
        watchdog.start(TimerMode::SingleShot, Duration::from_secs(4), || {
            slint::quit_event_loop().unwrap()
        });
        slint::run_event_loop_until_quit().unwrap();
        poll.stop();
        assert_eq!(ui.get_conversation_title(), "Contact");
        assert!(ui.get_storage_error().is_empty());
        drop(ui);
        worker.finish();

        let db = Database::open(&temp.0, owner).unwrap();
        assert_eq!(db.messages(id).unwrap()[0].body, stored_body);
        assert_eq!(db.draft(id).unwrap().as_deref(), Some("Latest edit"));
        assert_eq!(db.conversations(false).unwrap().len(), 2);
    }

    #[test]
    fn replying_offline_promotes_the_selected_request_in_slint() {
        if !in_test_process(
            "storage_ui::tests::replying_offline_promotes_the_selected_request_in_slint",
        ) {
            return;
        }
        let account_key = ed25519_dalek::SigningKey::from_bytes(&[5; 32]);
        let public_key = account_key.verifying_key();
        let owner = Owner {
            void_id: "41",
            public_key: public_key.as_bytes(),
        };
        let temp = TestDir::new();
        let mut db = Database::open(&temp.0, owner).unwrap();
        let id = db.create_conversation("123", 100).unwrap();
        db.set_request(id, true).unwrap();
        db.save_message(&Message {
            id: "incoming-request".into(),
            conversation_id: id,
            direction: Direction::Incoming,
            body: "Incoming request".into(),
            created_at: 150,
            delivery_state: DeliveryState::Received,
        })
        .unwrap();
        db.save_draft(id, "First reply\n\n\n").unwrap();
        db.close().unwrap();

        i_slint_backend_testing::init_integration_test_with_system_time();
        let ui = AppWindow::new().unwrap();
        ui.set_account_void_id(owner.void_id.into());
        ui.set_setup_page(0);
        ui.set_section("Requests".into());
        let worker = bind_fixture(&ui, move |_| (temp.0.clone(), account_key.clone())).unwrap();
        worker.opener()(&ui);

        let poll = Timer::default();
        let weak = ui.as_weak();
        let stage = Cell::new(0);
        let id = id.to_string();
        poll.start(TimerMode::Repeated, Duration::from_millis(10), move || {
            let ui = weak.upgrade().unwrap();
            match stage.get() {
                0 if ui.get_storage_ready() => {
                    assert_eq!(ui.get_stored_requests().row_count(), 1);
                    ui.invoke_select_stored_conversation(id.clone().into());
                    stage.set(1);
                }
                1 if !ui.get_conversation_loading() => {
                    assert_eq!(ui.get_section(), "Requests");
                    assert!(ui.get_conversation_is_request());
                    assert_eq!(ui.get_stored_requests().row_count(), 1);
                    assert_eq!(ui.get_stored_draft(), "First reply\n\n\n");
                    ui.invoke_send_message();
                    stage.set(2);
                }
                2 if !ui.get_send_busy() => {
                    assert_eq!(ui.get_section(), "Chats");
                    assert_eq!(ui.get_selected_conversation().as_str(), id);
                    assert!(!ui.get_conversation_is_request());
                    assert_eq!(ui.get_stored_requests().row_count(), 0);
                    assert_eq!(ui.get_stored_chats().row_data(0).unwrap().id.as_str(), id);
                    assert_eq!(ui.get_stored_messages().row_count(), 2);
                    let reply = ui.get_stored_messages().row_data(1).unwrap();
                    assert_eq!(reply.body, "First reply");
                    assert_eq!(reply.status, "Send failed");
                    assert_eq!(ui.get_stored_draft(), "");
                    slint::quit_event_loop().unwrap();
                }
                _ => (),
            }
        });
        let watchdog = Timer::default();
        watchdog.start(TimerMode::SingleShot, Duration::from_secs(4), || {
            slint::quit_event_loop().unwrap();
        });
        slint::run_event_loop_until_quit().unwrap();
        poll.stop();
        assert_eq!(ui.get_section(), "Chats");
        assert!(!ui.get_send_busy());
        drop(ui);
        worker.finish();
    }

    #[test]
    fn verified_deletion_keeps_chats_and_requests_readable_in_slint() {
        if !in_test_process(
            "storage_ui::tests::verified_deletion_keeps_chats_and_requests_readable_in_slint",
        ) {
            return;
        }
        use crate::{
            relay::{
                Connection,
                tests::{next_attempt, receive_json, send_json, verify_account_proof},
            },
            report_relay_status,
            storage::{account_status::VerifiedPeerStatus, tests::relay_messaging::mock_relay},
        };
        use futures_util::StreamExt;
        use serde_json::json;
        use std::sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        };
        let key = ed25519_dalek::SigningKey::from_bytes(&[5; 32]);
        let public = key.verifying_key();
        let owner = Owner {
            void_id: "123",
            public_key: public.as_bytes(),
        };
        let temp = TestDir::new();
        let mut db = Database::open(&temp.0, owner).unwrap();
        let chat = db.create_conversation("234", 100).unwrap();
        let request = db.create_conversation("456", 100).unwrap();
        db.set_request(request, true).unwrap();
        db.record_verified_peer_status("234", VerifiedPeerStatus::Active)
            .unwrap();
        db.initialize_signal(owner.void_id, &key).unwrap();
        let publication = db.prepare_publication().unwrap().unwrap();
        db.confirm_publication(&publication.id).unwrap();
        for id in [chat, request] {
            db.save_message(&Message {
                id: format!("incoming-{id}"),
                conversation_id: id,
                direction: Direction::Incoming,
                body: "Saved incoming message".into(),
                created_at: 120,
                delivery_state: DeliveryState::Received,
            })
            .unwrap();
        }
        db.save_draft(chat, "Keep this draft").unwrap();
        let (event, _) = db
            .queue_outgoing(chat, "Existing outgoing message", 130)
            .unwrap();
        db.close().unwrap();

        let both_deleted = Arc::new(AtomicBool::new(false));
        let complete = both_deleted.clone();
        let (endpoint, mock) = mock_relay(move |listener| async move {
            for state in ["active", "deleted"] {
                let mut socket = next_attempt(&listener).await;
                verify_account_proof(&mut socket, &public, "123").await;
                crate::relay::tests::finish_authentication(&mut socket, "123").await;
                for peer in ["234", "456"] {
                    let request = receive_json(&mut socket).await;
                    assert_eq!(
                        request,
                        json!({"type":"account_status", "request_id":request["request_id"], "void_id":peer})
                    );
                    let mut response = json!({"type":"account_status", "request_id":request["request_id"], "void_id":peer, "state":state});
                    if state == "deleted" {
                        response["deleted_at"] = json!("2026-11-16T00:00:12.345678Z");
                    }
                    send_json(&mut socket, response).await;
                }
                if state == "deleted" {
                    complete.store(true, Ordering::Release);
                    // Keep the authenticated socket open for the UI assertions.
                    let _ = socket.next().await;
                }
            }
        });
        i_slint_backend_testing::init_integration_test_with_system_time();
        let ui = AppWindow::new().unwrap();
        ui.set_account_void_id(owner.void_id.into());
        ui.set_setup_page(0);
        let root = temp.0.clone();
        let relay_key = key.clone();
        let worker = bind_fixture(&ui, move |_| (root.clone(), key.clone())).unwrap();
        let (relay, relay_worker) = Connection::spawn_with_activity(
            endpoint,
            move |id| {
                Ok(account::RegisteredIdentity {
                    void_id: id.into(),
                    key: relay_key.clone(),
                })
            },
            report_relay_status(&ui),
            worker.relay_activity(),
        )
        .unwrap();
        worker.opener()(&ui);
        let weak = ui.as_weak();
        let step = Cell::new(0);
        let poll = Timer::default();
        let (chat, request) = (chat.to_string(), request.to_string());
        let event: slint::SharedString = event.into();
        poll.start(TimerMode::Repeated, Duration::from_millis(10), move || {
            let ui = weak.upgrade().unwrap();
            match step.get() {
                0 if ui.get_storage_ready() => {
                    ui.set_section("Requests".into());
                    ui.invoke_select_stored_conversation(request.clone().into());
                    step.set(1);
                }
                1 if ui.get_selected_conversation() == request
                    && !ui.get_conversation_loading() =>
                {
                    assert!(ui.get_conversation_can_compose());
                    assert!(ui.get_conversation_account_notice().is_empty());
                    ui.set_section("Chats".into());
                    ui.invoke_select_stored_conversation(chat.clone().into());
                    step.set(2);
                }
                2 if ui.get_selected_conversation() == chat && !ui.get_conversation_loading() => {
                    assert!(ui.get_conversation_can_compose());
                    assert!(ui.get_conversation_account_notice().is_empty());
                    assert!(relay.connect("123".into()));
                    step.set(3);
                }
                3 if ui.get_conversation_peer_deleted() => {
                    assert!(!ui.get_conversation_can_compose());
                    assert_eq!(
                        ui.get_conversation_account_notice(),
                        "This user account has been deleted."
                    );
                    assert_eq!(
                        ui.get_stored_messages().row_data(0).unwrap().body,
                        "Saved incoming message"
                    );
                    assert_eq!(ui.get_stored_messages().row_count(), 2);
                    assert_eq!(ui.get_stored_draft(), "Keep this draft");
                    ui.set_messaging_ready(true);
                    ui.invoke_send_message();
                    ui.invoke_resend_message(event.clone());
                    assert!(!ui.get_send_busy());
                    step.set(4);
                }
                4 if ui.get_connection_status() == "Connected"
                    && both_deleted.load(Ordering::Acquire) =>
                {
                    assert!(ui.get_conversation_peer_deleted());
                    ui.set_section("Requests".into());
                    ui.invoke_select_stored_conversation(request.clone().into());
                    step.set(5);
                }
                5 if ui.get_selected_conversation() == request
                    && !ui.get_conversation_loading() =>
                {
                    slint::quit_event_loop().unwrap();
                }
                _ => (),
            }
        });
        let watchdog = Timer::default();
        watchdog.start(TimerMode::SingleShot, Duration::from_secs(9), || {
            slint::quit_event_loop().unwrap();
        });
        slint::run_event_loop_until_quit().unwrap();
        poll.stop();
        assert_eq!(ui.get_section(), "Requests");
        assert!(ui.get_conversation_is_request());
        assert!(!ui.get_conversation_can_compose());
        assert_eq!(
            ui.get_conversation_account_notice(),
            "This user account has been deleted."
        );
        assert_eq!(ui.get_stored_messages().row_count(), 1);
        drop(poll);
        drop(ui);
        relay_worker.finish();
        worker.finish();
        mock.join().unwrap();
    }

    #[test]
    fn permanent_deletion_requires_exact_text_and_a_ready_account_worker() {
        if !in_test_process(
            "storage_ui::tests::permanent_deletion_requires_exact_text_and_a_ready_account_worker",
        ) {
            return;
        }
        i_slint_backend_testing::init_integration_test_with_system_time();
        let ui = AppWindow::new().unwrap();
        ui.set_account_void_id("41".into());
        ui.set_setup_page(0);
        ui.set_section("Settings".into());
        ui.set_stored_draft("Untouched draft".into());

        ui.invoke_open_permanent_deletion();
        assert!(ui.get_permanent_deletion_open());
        for phrase in ["", "delete", "Delete", "DELETE ", " DELETE", "DELETE\n"] {
            ui.set_permanent_deletion_phrase(phrase.into());
            assert!(!ui.get_permanent_deletion_confirmed());
            assert!(!ui.get_can_permanently_delete());
        }
        ui.set_permanent_deletion_phrase("DELETE".into());
        assert!(ui.get_permanent_deletion_confirmed());
        assert!(!ui.get_can_permanently_delete());
        ui.set_account_management_ready(true);
        assert!(ui.get_can_permanently_delete());
        ui.set_removal_busy(true);
        assert!(!ui.get_can_permanently_delete());
        ui.set_removal_busy(false);

        ui.invoke_cancel_permanent_deletion();
        assert!(!ui.get_permanent_deletion_open());
        assert!(ui.get_permanent_deletion_phrase().is_empty());
        assert_eq!(ui.get_account_void_id(), "41");
        assert_eq!(ui.get_stored_draft(), "Untouched draft");
        ui.invoke_open_permanent_deletion();
        assert!(!ui.get_permanent_deletion_confirmed());
    }

    #[test]
    fn setup_blocks_permanent_deletion_and_only_offers_local_removal_after_error() {
        if !in_test_process(
            "storage_ui::tests::setup_blocks_permanent_deletion_and_only_offers_local_removal_after_error",
        ) {
            return;
        }
        let temp = TestDir::new();
        let key = ed25519_dalek::SigningKey::from_bytes(&[5; 32]);
        let (db, _) = account::open_enrollment_fixture(&temp.0, "41", &key).unwrap();
        let conversation = db.create_conversation("123", 100).unwrap();
        db.save_draft(conversation, "Isolated draft").unwrap();
        db.close().unwrap();
        let file = temp.0.join("account.dpapi");
        let identity = std::fs::read(&file).unwrap();

        i_slint_backend_testing::init_integration_test_with_system_time();
        let ui = AppWindow::new().unwrap();
        ui.set_account_void_id("41".into());
        ui.set_account_management_ready(true);
        ui.set_setup_page(account::ENROLLMENT_PAGE);
        ui.set_storage_loading(false);
        ui.set_storage_ready(true);
        ui.set_connection_status("Connected".into());
        ui.set_publishing_keys(true);
        assert!(!ui.get_can_remove_enrollment_account());
        ui.invoke_open_local_removal();
        assert!(!ui.get_local_removal_open());
        let conflict = messaging::Error::Relay(crate::relay::RequestError::Rejected {
            command: "prekeys_publish",
            code: "prekeys_conflict".into(),
        });
        let (notice, retryable) = messaging::publication_failure(&conflict);
        ui.set_publishing_keys(false);
        ui.set_publication_error(notice.into());
        ui.set_publication_retryable(retryable);
        assert!(ui.get_can_remove_enrollment_account());
        assert!(!ui.get_enrollment_can_retry());
        ui.invoke_open_permanent_deletion();
        ui.set_permanent_deletion_phrase("DELETE".into());
        assert!(!ui.get_permanent_deletion_open());
        assert!(!ui.get_can_permanently_delete());

        ui.invoke_open_local_removal();
        assert!(ui.get_local_removal_open());
        for invalid in ["delete", "DELETE ", " DELETE", "DELETE\n"] {
            ui.set_local_removal_phrase(invalid.into());
            assert!(!ui.get_local_removal_confirmed());
        }
        ui.set_local_removal_phrase("DELETE".into());
        assert!(ui.get_local_removal_confirmed());
        assert!(!ui.get_permanent_deletion_confirmed());
        ui.invoke_cancel_local_removal();
        assert!(ui.get_local_removal_phrase().is_empty());
        assert_eq!(ui.get_setup_page(), account::ENROLLMENT_PAGE);
        assert_eq!(ui.get_publication_error(), notice);

        ui.set_connection_status("Reconnecting".into());
        assert!(!ui.get_can_remove_enrollment_account());
        ui.invoke_open_permanent_deletion();
        ui.set_permanent_deletion_phrase("DELETE".into());
        assert!(!ui.get_permanent_deletion_confirmed());
        assert!(!ui.get_can_permanently_delete());
        assert!(!ui.get_local_removal_confirmed());
        assert!(std::fs::read(&file).unwrap() == identity);
        let (db, _) = account::open_enrollment_fixture(&temp.0, "41", &key).unwrap();
        assert_eq!(
            db.draft(conversation).unwrap().as_deref(),
            Some("Isolated draft")
        );
    }

    #[test]
    fn registered_startup_publishes_automatically_and_keeps_the_rejection_until_success() {
        if !in_test_process(
            "storage_ui::tests::registered_startup_publishes_automatically_and_keeps_the_rejection_until_success",
        ) {
            return;
        }
        use crate::{
            bind_reconnect,
            relay::{
                Connection, ConnectionStatus,
                tests::{next_attempt, receive_json, send_json, verify_account_proof},
            },
            report_relay_status,
        };
        use ed25519_dalek::SigningKey;
        use futures_util::StreamExt;
        use serde_json::json;
        use std::sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        };
        use tokio::{net::TcpListener, sync::Notify, time::timeout};

        let account_key = SigningKey::from_bytes(&[7; 32]);
        let public_key = account_key.verifying_key();
        let owner = Owner {
            void_id: "123",
            public_key: public_key.as_bytes(),
        };
        let temp = TestDir::new();
        let mut db = Database::open(&temp.0, owner).unwrap();
        let conversation = db.create_conversation("456", 100).unwrap();
        let (event, _) = db
            .queue_outgoing(conversation, "Saved outgoing operation", 120)
            .unwrap();
        db.close().unwrap();

        let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        listener.set_nonblocking(true).unwrap();
        let endpoint = format!("ws://{}/ws", listener.local_addr().unwrap());
        let saw_error = Arc::new(Notify::new());
        let saw_reconnect = Arc::new(Notify::new());
        let saw_ready = Arc::new(Notify::new());
        let second_attempt = Arc::new(AtomicBool::new(false));
        let checked_no_resend = Arc::new(AtomicBool::new(false));
        let mock = {
            let (saw_error, saw_reconnect, saw_ready) =
                (saw_error.clone(), saw_reconnect.clone(), saw_ready.clone());
            let (second_attempt, checked_no_resend) =
                (second_attempt.clone(), checked_no_resend.clone());
            thread::spawn(move || {
                tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .unwrap()
                    .block_on(async {
                        timeout(Duration::from_secs(18), async {
                            let listener = TcpListener::from_std(listener).unwrap();
                            let mut first = next_attempt(&listener).await;
                            verify_account_proof(&mut first, &public_key, "123").await;
                            crate::relay::tests::finish_authentication(&mut first, "123").await;
                            let sent = receive_json(&mut first).await;
                            assert_eq!(sent["type"], "prekeys_publish");
                            send_json(
                                &mut first,
                                json!({"type":"prekeys_error",
                                "request_id":sent["publication_id"], "code":"prekeys_invalid"}),
                            )
                            .await;
                            saw_error.notified().await;
                            drop(first);

                            let mut second = next_attempt(&listener).await;
                            verify_account_proof(&mut second, &public_key, "123").await;
                            crate::relay::tests::finish_authentication(&mut second, "123").await;
                            assert!(receive_json(&mut second).await == sent);
                            second_attempt.store(true, Ordering::Release);
                            saw_reconnect.notified().await;
                            send_json(
                                &mut second,
                                json!({"type":"prekeys_error",
                                "request_id":sent["publication_id"], "code":"prekeys_capacity"}),
                            )
                            .await;
                            assert!(receive_json(&mut second).await == sent);
                            send_json(
                                &mut second,
                                json!({"type":"prekeys_published",
                                "publication_id":sent["publication_id"],
                                "ec_remaining":"20", "kyber_remaining":"20"}),
                            )
                            .await;
                            let status = receive_json(&mut second).await;
                            assert_eq!(status, json!({"type":"account_status", "request_id":status["request_id"], "void_id":"456"}));
                            send_json(&mut second, json!({"type":"account_status", "request_id":status["request_id"], "void_id":"456", "state":"active"})).await;
                            assert!(
                                timeout(Duration::from_millis(200), second.next())
                                    .await
                                    .is_err()
                            );
                            checked_no_resend.store(true, Ordering::Release);
                            saw_ready.notified().await;
                        })
                        .await
                        .unwrap();
                    });
            })
        };

        i_slint_backend_testing::init_integration_test_with_system_time();
        let ui = AppWindow::new().unwrap();
        ui.set_account_void_id(owner.void_id.into());
        ui.set_setup_page(account::ENROLLMENT_PAGE);
        let root = temp.0.clone();
        let worker = bind_fixture(&ui, move |_| (root.clone(), account_key.clone())).unwrap();
        let (relay, relay_worker) = Connection::spawn_with_activity(
            endpoint,
            |id| {
                Ok(account::RegisteredIdentity {
                    void_id: id.into(),
                    key: SigningKey::from_bytes(&[7; 32]),
                })
            },
            report_relay_status(&ui),
            worker.relay_activity(),
        )
        .unwrap();
        bind_reconnect(&ui, relay);
        assert!(!ui.get_can_reconnect());
        ui.set_messaging_notice("A saved send still requires attention.".into());
        worker.opener()(&ui);

        let weak = ui.as_weak();
        let stage = Cell::new(0);
        let poll = Timer::default();
        poll.start(TimerMode::Repeated, Duration::from_millis(10), move || {
            let ui = weak.upgrade().unwrap();
            if ui.get_connection_status() != ConnectionStatus::Connected.label() {
                return;
            }
            if stage.get() <= 1
                && ui
                    .get_publication_error()
                    .contains("public Signal keys as invalid")
            {
                assert!(ui.get_storage_ready());
                assert!(!ui.get_messaging_ready());
                assert_eq!(ui.get_setup_page(), account::ENROLLMENT_PAGE);
                assert!(!ui.get_enrollment_can_retry());
                assert!(!ui.get_can_reconnect());
                assert!(!ui.get_publication_retryable());
                if stage.get() == 0 {
                    stage.set(1);
                    saw_error.notify_one();
                }
                if second_attempt.load(Ordering::Acquire) {
                    stage.set(2);
                    saw_reconnect.notify_one();
                }
            }
            if stage.get() == 2 && ui.get_publication_error().contains("at capacity") {
                assert!(!ui.get_messaging_ready());
                assert!(!ui.get_can_reconnect());
                assert!(ui.get_publication_retryable());
                assert!(ui.get_enrollment_can_retry());
                ui.invoke_retry_messaging();
                stage.set(3);
            }
            if checked_no_resend.load(Ordering::Acquire) {
                assert_eq!(stage.get(), 3);
                assert!(ui.get_messaging_ready());
                assert_eq!(ui.get_setup_page(), 0);
                assert_eq!(ui.get_section(), "Chats");
                assert!(ui.get_publication_error().is_empty());
                assert!(!ui.get_can_reconnect());
                assert_eq!(
                    ui.get_messaging_notice(),
                    "A saved send still requires attention."
                );
                slint::quit_event_loop().unwrap();
            }
        });
        let watchdog = Timer::default();
        watchdog.start(TimerMode::SingleShot, Duration::from_secs(14), || {
            slint::quit_event_loop().unwrap()
        });
        slint::run_event_loop_until_quit().unwrap();
        poll.stop();
        assert!(ui.get_messaging_ready());
        saw_ready.notify_one();
        mock.join().unwrap();
        drop(ui);
        relay_worker.finish();
        worker.finish();
        let mut db = Database::open(&temp.0, owner).unwrap();
        assert!(db.prepare_publication().unwrap().is_none());
        assert_eq!(db.outgoing_operation(&event).unwrap().state, "failed");
    }
}
