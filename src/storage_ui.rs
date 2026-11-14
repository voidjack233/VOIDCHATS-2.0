use crate::{AppWindow, StoredContact, StoredConversation, StoredMessage, account, local_data, storage};
use slint::{ComponentHandle, ModelRc, VecModel, Weak};
use std::{
    collections::HashMap,
    io,
    path::PathBuf,
    sync::mpsc::{self, Sender},
    thread::{self, JoinHandle},
    time::{SystemTime, UNIX_EPOCH},
};

enum Command {
    Open(String),
    Select(i64, i32),
    Create(String, i32),
    SaveDraft(i64, String, i32),
    Close,
}

pub(crate) struct Worker {
    sender: Sender<Command>,
    thread: JoinHandle<()>,
}

impl Worker {
    pub fn opener(&self) -> impl Fn(&AppWindow) + Clone + Send + 'static {
        let sender = self.sender.clone();
        move |ui| {
            ui.set_storage_loading(true);
            ui.set_storage_error("".into());
            if sender.send(Command::Open(ui.get_account_void_id().to_string())).is_err() {
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

fn worker_stopped(ui: &AppWindow) {
    ui.set_storage_loading(false);
    ui.set_storage_ready(false);
    ui.set_storage_error("Local storage stopped. Close and reopen VOID.".into());
}

pub(crate) fn bind(ui: &AppWindow) -> io::Result<Worker> {
    bind_with(ui, |id| {
        let account = account::load_registered_identity(id).map_err(|_| storage::Error::WrongAccount)?;
        Ok((local_data::user_data_dir()?, account.key.verifying_key().to_bytes()))
    })
}

fn bind_with(
    ui: &AppWindow,
    owner: impl Fn(&str) -> storage::Result<(PathBuf, [u8; 32])> + Send + 'static,
) -> io::Result<Worker> {
    let (sender, receiver) = mpsc::channel();
    let weak = ui.as_weak();
    let thread = thread::Builder::new().name("void-storage".into()).spawn(move || {
        let mut database = None;
        let mut queued = None;
        while let Some(mut command) = queued.take().or_else(|| receiver.recv().ok()) {
            if let Command::SaveDraft(id, _, _) = command {
                // Keep the latest edit in a burst without crossing a selection, create or close.
                while let Ok(next) = receiver.try_recv() {
                    match next {
                        Command::SaveDraft(next_id, _, _) if next_id == id => command = next,
                        other => { queued = Some(other); break; }
                    }
                }
            }
            match command {
                Command::Open(id) => {
                    if database.is_some() { continue; }
                    let opened = owner(&id).and_then(|(root, public_key)| {
                        storage::Database::open(&root, storage::Owner { void_id: &id, public_key: &public_key })
                    }).and_then(|db| load_lists(&db).map(|lists| (db, lists)));
                    match opened {
                        Ok((db, lists)) => {
                            database = Some(db);
                            let _ = weak.upgrade_in_event_loop(move |ui| {
                                lists.show(&ui);
                                ui.set_storage_loading(false);
                                ui.set_storage_ready(true);
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
                Command::Select(id, selection) => {
                    if let Some(db) = &database {
                        show_conversation(&weak, selection, load_conversation(db, id));
                    }
                }
                Command::Create(peer, selection) => {
                    if let Some(db) = &database {
                        let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis() as i64;
                        let result = db.create_conversation(&peer, now).and_then(|id| {
                            Ok((load_lists(db)?, load_conversation(db, id)?))
                        });
                        let _ = weak.upgrade_in_event_loop(move |ui| {
                            match result {
                                Ok((lists, detail)) => {
                                    lists.show(&ui);
                                    if ui.get_storage_selection_token() != selection { return; }
                                    ui.set_conversation_loading(false);
                                    ui.set_creating_conversation(false);
                                    ui.set_section(if detail.conversation.is_request { "Requests" } else { "Chats" }.into());
                                    detail.show(&ui);
                                }
                                Err(_) if ui.get_storage_selection_token() == selection => {
                                    ui.set_conversation_loading(false);
                                    ui.set_conversation_error("The conversation could not be opened. Try again.".into());
                                }
                                Err(_) => (),
                            }
                        });
                    }
                }
                Command::SaveDraft(id, body, revision) => {
                    if let Some(db) = &database {
                        let saved = db.save_draft(id, &body).is_ok();
                        let _ = weak.upgrade_in_event_loop(move |ui| {
                            ui.invoke_report_draft_saved(id.to_string().into(), revision, saved);
                        });
                    }
                }
                Command::Close => break,
            }
        }
        if let Some(db) = database {
            let _ = db.close();
        }
    })?;

    let worker = Worker { sender, thread };
    let weak = ui.as_weak();
    let open = worker.opener();
    ui.on_retry_storage(move || {
        if let Some(ui) = weak.upgrade() && !ui.get_storage_loading() && !ui.get_storage_ready() {
            open(&ui);
        }
    });

    let weak = ui.as_weak();
    let sender = worker.sender.clone();
    ui.on_select_stored_conversation(move |id| {
        if let (Some(ui), Ok(id)) = (weak.upgrade(), id.parse()) {
            let selection = begin_selection(&ui);
            if sender.send(Command::Select(id, selection)).is_err() { worker_stopped(&ui); }
        }
    });

    let weak = ui.as_weak();
    let sender = worker.sender.clone();
    ui.on_create_stored_conversation(move |peer| {
        let Some(ui) = weak.upgrade() else { return; };
        if !ui.get_storage_ready() || ui.get_conversation_loading() { return; }
        let peer = peer.trim();
        if !account::valid_void_id(peer) || peer == ui.get_account_void_id().as_str() {
            ui.set_conversation_error("Enter another person's VOID-ID.".into());
            return;
        }
        let selection = begin_selection(&ui);
        if sender.send(Command::Create(peer.to_owned(), selection)).is_err() { worker_stopped(&ui); }
    });

    bind_drafts(ui, worker.sender.clone());
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
        let Some(ui) = weak.upgrade() else { return; };
        let Ok(id) = ui.get_selected_conversation().parse() else { return; };
        let revision = ui.get_storage_draft_token().wrapping_add(1);
        ui.set_storage_draft_token(revision);
        let failed = sender.send(Command::SaveDraft(id, body.to_string(), revision)).is_err();
        let mut drafts = edited.borrow_mut();
        drafts.insert(id, PendingDraft { body, revision, failed });
        ui.set_unsaved_draft_failure(drafts.values().any(|draft| draft.failed));
        draft_feedback(&ui, Some(failed));
    });

    let weak = ui.as_weak();
    let completed = pending.clone();
    ui.on_report_draft_saved(move |id, revision, saved| {
        let (Some(ui), Ok(number)) = (weak.upgrade(), id.parse()) else { return; };
        let mut drafts = completed.borrow_mut();
        let Some(draft) = drafts.get_mut(&number).filter(|draft| draft.revision == revision) else { return; };
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
    ui.on_load_stored_draft(move |id, body| {
        let (Some(ui), Ok(id)) = (weak.upgrade(), id.parse()) else { return; };
        let drafts = pending.borrow();
        let pending = drafts.get(&id);
        ui.set_stored_draft(pending.map_or(body, |draft| draft.body.clone()));
        draft_feedback(&ui, pending.map(|draft| draft.failed));
    });
}

fn draft_feedback(ui: &AppWindow, pending: Option<bool>) {
    ui.set_draft_status(match pending {
        Some(true) => "Draft could not be saved",
        Some(false) => "Saving draft…",
        None => "Drafts stay on this device",
    }.into());
    ui.set_draft_save_failed(pending == Some(true));
}

fn begin_selection(ui: &AppWindow) -> i32 {
    let selection = ui.get_storage_selection_token().wrapping_add(1);
    ui.set_storage_selection_token(selection);
    ui.set_conversation_loading(true);
    ui.set_conversation_error("".into());
    selection
}

struct Lists {
    chats: Vec<storage::Conversation>,
    requests: Vec<storage::Conversation>,
    contacts: Vec<storage::Contact>,
}

impl Lists {
    fn show(self, ui: &AppWindow) {
        ui.set_stored_chats(conversation_model(self.chats));
        ui.set_stored_requests(conversation_model(self.requests));
        let contacts: Vec<_> = self.contacts.into_iter().map(|contact| StoredContact {
            title: contact.display_name.unwrap_or_else(|| format!("VOID-ID {}", contact.void_id)).into(),
            void_id: contact.void_id.into(),
        }).collect();
        ui.set_stored_contacts(ModelRc::new(VecModel::from(contacts)));
    }
}

fn load_lists(db: &storage::Database) -> storage::Result<Lists> {
    Ok(Lists { chats: db.conversations(false)?, requests: db.conversations(true)?, contacts: db.contacts()? })
}

fn conversation_model(list: Vec<storage::Conversation>) -> ModelRc<StoredConversation> {
    let rows: Vec<_> = list.into_iter().map(|row| {
        let time = display_time(row.last_activity_at);
        let activity = if row.last_activity_at == row.created_at {
            format!("Created {time}")
        } else {
            time
        };
        StoredConversation {
            id: row.id.to_string().into(),
            title: row.display_name.unwrap_or_else(|| format!("VOID-ID {}", row.peer_void_id)).into(),
            activity: activity.into(),
        }
    }).collect();
    ModelRc::new(VecModel::from(rows))
}

struct Detail {
    conversation: storage::Conversation,
    messages: Vec<storage::Message>,
    draft: String,
}

impl Detail {
    fn show(self, ui: &AppWindow) {
        let Self { conversation, messages, draft } = self;
        ui.set_selected_conversation(conversation.id.to_string().into());
        ui.set_conversation_title(conversation.display_name.unwrap_or_else(|| format!("VOID-ID {}", conversation.peer_void_id)).into());
        ui.set_conversation_is_request(conversation.is_request);
        let messages: Vec<_> = messages.into_iter().map(|message| StoredMessage {
            body: message.body.into(),
            time: display_time(message.created_at).into(),
            outgoing: message.direction == storage::Direction::Outgoing,
        }).collect();
        ui.set_stored_messages(ModelRc::new(VecModel::from(messages)));
        ui.invoke_load_stored_draft(ui.get_selected_conversation(), draft.into());
    }
}

fn load_conversation(db: &storage::Database, id: i64) -> storage::Result<Detail> {
    Ok(Detail { conversation: db.conversation(id)?, messages: db.messages(id)?, draft: db.draft(id)?.unwrap_or_default() })
}

fn show_conversation(weak: &Weak<AppWindow>, selection: i32, detail: storage::Result<Detail>) {
    let _ = weak.upgrade_in_event_loop(move |ui| {
        if ui.get_storage_selection_token() != selection { return; }
        ui.set_conversation_loading(false);
        match detail {
            Ok(detail) => detail.show(&ui),
            Err(_) => ui.set_conversation_error("The saved conversation could not be loaded. Try opening it again.".into()),
        }
    });
}

fn display_time(timestamp: i64) -> String {
    chrono::DateTime::from_timestamp_millis(timestamp)
        .map(|date| date.format("%b %-d, %Y · %H:%M UTC").to_string()).unwrap_or_default()
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use crate::storage::{Database, DeliveryState, Direction, Message, Owner, tests::TestDir};
    use slint::{Model, Timer, TimerMode};
    use std::{process::Command, time::Duration};

    #[test]
    fn restores_the_local_conversation_and_latest_draft_in_slint() {
        const CHILD: &str = "VOID_STORAGE_UI_TEST_CHILD";
        if std::env::var_os(CHILD).is_none() {
            // Slint's threaded testing backend is process-global. Keep it separate from the relay UI test.
            let output = Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "storage_ui::tests::restores_the_local_conversation_and_latest_draft_in_slint"])
                .env(CHILD, "1").output().unwrap();
            assert!(output.status.success(), "{}\n{}",
                String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
            return;
        }

        const OWNER: Owner<'_> = Owner { void_id: "41", public_key: &[5; 32] };
        let temp = TestDir::new();
        let mut db = Database::open(&temp.0, OWNER).unwrap();
        let id = db.create_conversation("123", 100).unwrap();
        db.save_message(&Message { id: "test-1".into(), conversation_id: id,
            direction: Direction::Incoming, body: "Stored test message".into(), created_at: 200,
            delivery_state: DeliveryState::Received }).unwrap();
        db.save_draft(id, "Earlier draft").unwrap();
        db.close().unwrap();

        i_slint_backend_testing::init_integration_test_with_system_time();
        let ui = AppWindow::new().unwrap();
        ui.set_account_void_id(OWNER.void_id.into());
        ui.set_setup_page(0);
        let this_thread = thread::current().id();
        let root = temp.0.clone();
        let worker = bind_with(&ui, move |id| {
            assert_ne!(this_thread, thread::current().id());
            assert_eq!(id, OWNER.void_id);
            Ok((root.clone(), *OWNER.public_key))
        }).unwrap();

        worker.opener()(&ui);
        let poll = Timer::default();
        let weak = ui.as_weak();
        poll.start(TimerMode::Repeated, Duration::from_millis(10), move || {
            let ui = weak.upgrade().unwrap();
            if ui.get_storage_ready() && ui.get_selected_conversation().is_empty() && !ui.get_conversation_loading() {
                assert_eq!(ui.get_stored_chats().row_count(), 1);
                ui.invoke_select_stored_conversation(id.to_string().into());
            } else if ui.get_selected_conversation() == id.to_string() && !ui.get_conversation_loading() {
                assert_eq!(ui.get_stored_messages().row_data(0).unwrap().body, "Stored test message");
                assert_eq!(ui.get_stored_draft(), "Earlier draft");
                ui.invoke_save_stored_draft("Intermediate edit".into());
                ui.invoke_save_stored_draft("Latest edit".into());
                ui.invoke_create_stored_conversation("456".into());
            } else if ui.get_conversation_title() == "VOID-ID 456" {
                assert!(ui.get_stored_draft().is_empty());
                assert_eq!(ui.get_stored_chats().row_count(), 2);
                slint::quit_event_loop().unwrap();
            }
        });
        let watchdog = Timer::default();
        watchdog.start(TimerMode::SingleShot, Duration::from_secs(4), || slint::quit_event_loop().unwrap());
        slint::run_event_loop_until_quit().unwrap();
        poll.stop();
        assert_eq!(ui.get_conversation_title(), "VOID-ID 456");
        assert!(ui.get_storage_error().is_empty());
        drop(ui);
        worker.finish();

        let db = Database::open(&temp.0, OWNER).unwrap();
        assert_eq!(db.draft(id).unwrap().as_deref(), Some("Latest edit"));
        assert_eq!(db.conversations(false).unwrap().len(), 2);
    }
}
