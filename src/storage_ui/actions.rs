//! Message actions stay on the SQLCipher worker. No timeline keys are routed.
use super::*;
use storage::message_events::MessageReference;

pub(super) enum MessageAction {
    Reply(Option<MessageReference>, String),
    Reaction(MessageReference, String),
    Delete(MessageReference, bool),
}

pub(super) fn apply(
    db: &mut storage::Database,
    conversation: i64,
    action: MessageAction,
    remote: bool,
) -> storage::Result<()> {
    match action {
        MessageAction::Reply(target, body) => {
            if target.is_some() && !remote {
                return Err(storage::Error::MessagingUnavailable);
            }
            db.save_reply_draft(conversation, &body, target.as_ref())
        }
        MessageAction::Reaction(target, emoji) => {
            if !remote {
                return Err(storage::Error::MessagingUnavailable);
            }
            let own = db.local_void_id()?;
            let peer = db.conversation(conversation)?.peer_void_id;
            let active = !db
                .reaction_states(&peer, &target)?
                .iter()
                .any(|(actor, e, on, _)| actor == &own && e == &emoji && *on);
            db.queue_reaction(
                conversation,
                &storage::messaging::new_event_id()?,
                &target,
                &emoji,
                active,
                unix_millis(),
            )?;
            Ok(())
        }
        MessageAction::Delete(target, everyone) => {
            if everyone {
                // Save deletion intent and the local tombstone even offline.
                // The existing control dispatcher waits for messaging readiness.
                db.queue_delete_everyone(
                    conversation,
                    &storage::messaging::new_event_id()?,
                    &target,
                    unix_millis(),
                )?;
                Ok(())
            } else {
                db.delete_for_me(conversation, &target)
            }
        }
    }
}

pub(super) fn show_reply(weak: &Weak<AppWindow>, db: &storage::Database, id: i64, selection: i32) {
    let result = db
        .reply_draft(id)
        .and_then(|target| Ok((target, db.reply_draft_preview(id)?)));
    let _ = weak.upgrade_in_event_loop(move |ui| {
        if !is_current_timeline(&ui, id, selection) {
            return;
        }
        if let Ok((target, (author, preview))) = result {
            ui.set_reply_event_id(target.as_ref().map_or("", |r| r.event_id.as_str()).into());
            ui.set_reply_author_id(
                target
                    .as_ref()
                    .map_or("", |r| r.author_void_id.as_str())
                    .into(),
            );
            ui.set_reply_author(author.into());
            ui.set_reply_preview(preview.into());
        }
    });
}

/// One control per turn, bounded retries per connection, durable intent stays in
/// SQLCipher. User messages are never retransmitted by this scheduler.
#[derive(Default)]
pub(super) struct Dispatch {
    queue: VecDeque<String>,
    attempts: HashMap<String, u8>,
    pub due: Option<Instant>,
}
impl Dispatch {
    pub fn resume(&mut self, db: &storage::Database) -> storage::Result<bool> {
        let ids = db.pending_controls()?;
        let pending = !ids.is_empty();
        let live: HashSet<_> = ids.iter().cloned().collect();
        self.queue.retain(|id| live.contains(id));
        self.attempts.retain(|id, _| live.contains(id));
        if self.queue.is_empty() {
            self.due = None;
        }
        {
            for id in ids {
                if !self.queue.contains(&id) && self.attempts.get(&id).copied().unwrap_or(0) < 5 {
                    self.queue.push_back(id);
                }
            }
            if !self.queue.is_empty() && self.due.is_none() {
                self.due = Some(Instant::now() + Duration::from_millis(600));
            }
        }
        Ok(pending)
    }
    pub fn run(
        &mut self,
        db: &mut storage::Database,
        session: &Session,
    ) -> Result<(), messaging::Error> {
        self.due = None;
        let Some(id) = self.queue.pop_front() else {
            return Ok(());
        };
        let result = messaging::send_control(db, session, &id);
        let delay = if result.is_err() {
            let attempts = self.attempts.entry(id.clone()).or_default();
            *attempts += 1;
            if *attempts < 5 {
                self.queue.push_back(id);
            }
            Duration::from_secs((1u64 << *attempts).min(30))
        } else {
            self.attempts.remove(&id);
            Duration::from_millis(600)
        };
        if !self.queue.is_empty() {
            self.due = Some(Instant::now() + delay);
        }
        result
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use storage::{Database, Owner, tests::TestDir};

    #[test]
    fn dispatcher_replaces_stale_unencrypted_reactions_without_duplicate_jobs() {
        let root = TestDir::new();
        let mut db = Database::open(
            &root.0,
            Owner {
                void_id: "41",
                public_key: &[5; 32],
            },
        )
        .unwrap();
        let chat = db.create_conversation("123", 100).unwrap();
        let (message, _) = db.queue_outgoing(chat, "Original", 101).unwrap();
        let target = MessageReference {
            author_void_id: "41".into(),
            event_id: message,
        };
        let add = "00000000-0000-4000-8000-000000000001";
        let remove = "00000000-0000-4000-8000-000000000002";
        db.queue_reaction(chat, add, &target, "👍", true, 102)
            .unwrap();
        let mut dispatch = Dispatch::default();
        assert!(dispatch.resume(&db).unwrap());
        assert_eq!(dispatch.queue, VecDeque::from([add.to_owned()]));
        dispatch.attempts.insert(add.into(), 2);
        db.queue_reaction(chat, remove, &target, "👍", false, 103)
            .unwrap();
        assert!(dispatch.resume(&db).unwrap());
        assert_eq!(dispatch.queue, VecDeque::from([remove.to_owned()]));
        assert!(!dispatch.attempts.contains_key(add));
        assert!(dispatch.due.is_some());
        // A delayed repeat of the old submission must not resurrect its job.
        assert_eq!(
            db.queue_reaction(chat, add, &target, "👍", true, 102)
                .unwrap(),
            add
        );
        dispatch.resume(&db).unwrap();
        dispatch.resume(&db).unwrap();
        assert_eq!(dispatch.queue, VecDeque::from([remove.to_owned()]));
    }
}

pub(super) fn resume(weak: &Weak<AppWindow>, controls: &mut Dispatch, db: &storage::Database) {
    let result = controls.resume(db);
    let _ = weak.upgrade_in_event_loop(move |ui| match result {
        Ok(pending) => ui.set_pending_controls(pending),
        Err(error) => {
            ui.set_pending_controls(true);
            ui.set_message_action_error(error.open_message().into());
        }
    });
}

pub(super) fn bind(ui: &AppWindow, sender: Sender<Command>) {
    let weak = ui.as_weak();
    let send = sender.clone();
    ui.on_choose_reply(move |author, event| {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        submit(
            &ui,
            &send,
            MessageAction::Reply(
                Some(MessageReference {
                    author_void_id: author.into(),
                    event_id: event.into(),
                }),
                ui.get_stored_draft().into(),
            ),
            true,
        );
    });
    let weak = ui.as_weak();
    let send = sender.clone();
    ui.on_cancel_reply(move || {
        if let Some(ui) = weak.upgrade() {
            submit(
                &ui,
                &send,
                MessageAction::Reply(None, ui.get_stored_draft().into()),
                false,
            );
        }
    });
    let weak = ui.as_weak();
    let send = sender.clone();
    ui.on_toggle_reaction(move |author, event, emoji| {
        if let Some(ui) = weak.upgrade() {
            submit(
                &ui,
                &send,
                MessageAction::Reaction(
                    MessageReference {
                        author_void_id: author.into(),
                        event_id: event.into(),
                    },
                    emoji.into(),
                ),
                true,
            );
        }
    });
    let weak = ui.as_weak();
    let send = sender.clone();
    ui.on_delete_message(move |author, event, everyone| {
        if let Some(ui) = weak.upgrade() {
            submit(
                &ui,
                &send,
                MessageAction::Delete(
                    MessageReference {
                        author_void_id: author.into(),
                        event_id: event.into(),
                    },
                    everyone,
                ),
                false,
            );
        }
    });
    let weak = ui.as_weak();
    ui.on_retry_controls(move || {
        if let Some(ui) = weak.upgrade()
            && ui.get_remote_actions_supported()
            && !ui.get_update_required()
        {
            let _ = sender.send(Command::RetryControls);
        }
    });
}
fn submit(ui: &AppWindow, sender: &Sender<Command>, action: MessageAction, remote: bool) {
    if ui.get_update_required()
        || !ui.get_storage_ready()
        || ui.get_conversation_loading()
        || ui.get_action_busy()
        || (remote && !ui.get_remote_actions_supported())
    {
        return;
    }
    let Ok(id) = ui.get_selected_conversation().parse() else {
        return;
    };
    ui.set_action_busy(true);
    if sender
        .send(Command::MessageAction(
            id,
            ui.get_storage_selection_token(),
            action,
            ReadingPosition::capture(ui),
        ))
        .is_err()
    {
        ui.set_action_busy(false);
        worker_stopped(ui);
    }
}
