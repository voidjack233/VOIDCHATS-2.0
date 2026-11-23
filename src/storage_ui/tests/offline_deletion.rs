use super::*;
use i_slint_backend_testing::ElementHandle;

#[test]
fn offline_failed_messages_can_be_deleted_and_intent_recovers_after_restart() {
    if !in_test_process(
        "storage_ui::tests::offline_deletion::offline_failed_messages_can_be_deleted_and_intent_recovers_after_restart",
    ) {
        return;
    }
    i_slint_backend_testing::init_integration_test_with_system_time();
    fixture(false);
}

#[test]
#[ignore = "isolated native offline deletion dialogs; no accounts or network connections"]
fn render_offline_deletion_dialogs() {
    fixture(true);
}

fn button(ui: &AppWindow, label: &str) -> ElementHandle {
    // The timeline toolbar also has a "Delete message" action. Scope the
    // lookup to the open dialog so confirmation cannot merely reopen the menu.
    let dialog = ElementHandle::find_by_element_type_name(ui, "MessageActions")
        .next()
        .unwrap();
    let label = label.to_owned();
    dialog
        .query_descendants()
        .match_predicate(move |element| {
            element
                .accessible_label()
                .is_some_and(|value| value == label)
                && element.accessible_enabled().is_some()
        })
        .find_first()
        .expect("missing dialog action")
}

fn snapshot(ui: &AppWindow, name: &str) {
    let image = ui.window().take_snapshot().unwrap();
    let mut bytes = format!("P6\n{} {}\n255\n", image.width(), image.height()).into_bytes();
    for pixel in image.as_slice() {
        bytes.extend_from_slice(&[pixel.r, pixel.g, pixel.b]);
    }
    std::fs::create_dir_all("target/offline-deletion-renders").unwrap();
    std::fs::write(format!("target/offline-deletion-renders/{name}.ppm"), bytes).unwrap();
}

fn fixture(native: bool) {
    let root = TestDir::new();
    let key = ed25519_dalek::SigningKey::from_bytes(&[86; 32]);
    let public = key.verifying_key();
    let owner = Owner {
        void_id: "41",
        public_key: public.as_bytes(),
    };
    let mut db = Database::open(&root.0, owner).unwrap();
    let chat = db.create_conversation("123", 100).unwrap();
    let (local, _) = db
        .queue_outgoing(chat, "Never transmitted: remove locally", 110)
        .unwrap();
    let (remote, _) = db
        .queue_outgoing(chat, "Failed message: queue deletion", 120)
        .unwrap();
    for event in [&local, &remote] {
        db.set_send_state(event, storage::messaging::SendState::Failed)
            .unwrap();
    }
    db.save_draft(chat, "Keep this unsent draft").unwrap();
    db.close().unwrap();
    let ui = AppWindow::new().unwrap();
    ui.set_setup_page(0);
    ui.set_account_void_id("41".into());
    let path = root.0.clone();
    let saved_key = key.clone();
    let worker = bind_fixture(&ui, move |_| (path.clone(), saved_key.clone())).unwrap();
    worker.opener()(&ui);
    if native {
        ui.show().unwrap();
        ui.window().set_size(slint::PhysicalSize::new(1080, 680));
    }
    let phase = std::rc::Rc::new(Cell::new(0));
    let seen = phase.clone();
    let weak = ui.as_weak();
    let started = Instant::now();
    let timer = Timer::default();
    timer.start(TimerMode::Repeated, Duration::from_millis(100), move || {
        let ui = weak.upgrade().unwrap();
        assert!(
            started.elapsed() < Duration::from_secs(15),
            "offline deletion fixture timed out at {}: rows={}, busy={}, loading={}, pending={}, error={}",
            seen.get(), ui.get_stored_messages().row_count(), ui.get_action_busy(),
            ui.get_conversation_loading(), ui.get_pending_controls(), ui.get_message_action_error()
        );
        assert!(!ui.get_remote_actions_supported());
        match seen.get() {
            0 if ui.get_storage_ready() => {
                ui.invoke_select_stored_conversation(chat.to_string().into());
                seen.set(1);
            }
            1 if !ui.get_conversation_loading() && ui.get_stored_messages().row_count() == 2 => {
                let row = ui.get_stored_messages().row_data(0).unwrap();
                assert!(!row.retry_event.is_empty());
                ui.invoke_open_message_actions(row, "delete".into(), 680.0, 470.0);
                seen.set(2);
            }
            2 => {
                if native {
                    snapshot(&ui, "offline-menu");
                }
                assert_eq!(
                    button(&ui, "Delete for Everyone").accessible_enabled(),
                    Some(true)
                );
                button(&ui, "Delete for Me").invoke_accessible_default_action();
                seen.set(3);
            }
            3 if !ui.get_action_busy() && ui.get_stored_messages().row_count() == 1 => {
                assert!(!ui.get_pending_controls());
                let row = ui.get_stored_messages().row_data(0).unwrap();
                ui.invoke_open_message_actions(row, "delete".into(), 680.0, 470.0);
                seen.set(4);
            }
            4 => {
                button(&ui, "Delete for Everyone").invoke_accessible_default_action();
                if native {
                    ui.window().set_size(slint::PhysicalSize::new(850, 560));
                }
                seen.set(5);
            }
            5 => {
                if native {
                    snapshot(&ui, "offline-confirmation");
                }
                assert_eq!(
                    button(&ui, "Delete message").accessible_enabled(),
                    Some(true)
                );
                button(&ui, "Delete message").invoke_accessible_default_action();
                seen.set(6);
            }
            6 if !ui.get_action_busy() && ui.get_stored_messages().row_count() == 0 => {
                assert!(ui.get_pending_controls());
                assert!(ui.get_message_action_error().is_empty());
                assert_eq!(ui.get_stored_draft(), "Keep this unsent draft");
                if native {
                    snapshot(&ui, "queued-offline");
                }
                seen.set(7);
                slint::quit_event_loop().unwrap();
            }
            _ => (),
        }
    });
    slint::run_event_loop().unwrap();
    timer.stop();
    worker.finish();
    assert_eq!(phase.get(), 7);
    let public = key.verifying_key();
    let mut db = Database::open(
        &root.0,
        Owner {
            void_id: "41",
            public_key: public.as_bytes(),
        },
    )
    .unwrap();
    let pending = db.pending_controls().unwrap();
    assert_eq!(pending.len(), 1);
    let control = db.control(&pending[0]).unwrap();
    assert_eq!(control.target.event_id, remote);
    assert_eq!(control.kind, "delete_for_everyone");
    assert!(control.payload.is_none()); // Offline queuing does not touch the ratchet.
    assert!(db.outgoing_message_blocked(&local).unwrap());
    assert!(db.outgoing_message_blocked(&remote).unwrap());
    assert!(messaging::send(&mut db, None, &local).is_err());
    assert!(messaging::send(&mut db, None, &remote).is_err());
    assert_eq!(db.pending_controls().unwrap(), pending);
    assert!(actions::Dispatch::default().resume(&db).unwrap());
    db.close().unwrap();

    // Reopening without a connection still exposes the durable pending state.
    ui.set_storage_ready(false);
    ui.set_pending_controls(false);
    let path = root.0.clone();
    let worker = bind_fixture(&ui, move |_| (path.clone(), key.clone())).unwrap();
    worker.opener()(&ui);
    let weak = ui.as_weak();
    let started = Instant::now();
    timer.start(TimerMode::Repeated, Duration::from_millis(20), move || {
        assert!(started.elapsed() < Duration::from_secs(10));
        let ui = weak.upgrade().unwrap();
        if ui.get_storage_ready() {
            assert!(ui.get_pending_controls());
            assert!(!ui.get_remote_actions_supported());
            slint::quit_event_loop().unwrap();
        }
    });
    slint::run_event_loop().unwrap();
    timer.stop();
    worker.finish();
    if native {
        ui.hide().unwrap();
    }
}
