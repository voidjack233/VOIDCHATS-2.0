use super::*;
use storage::message_events::MessageReference;

struct NativeFixture {
    ui: AppWindow,
    storage: Option<Worker>,
    relay: Option<crate::relay::Worker>,
    connection: crate::relay::Connection,
    chat: i64,
}
impl NativeFixture {
    fn new(
        path: &std::path::Path,
        id: &'static str,
        peer: &str,
        seed: u8,
        endpoint: &str,
        audience: &str,
    ) -> Self {
        let key = ed25519_dalek::SigningKey::from_bytes(&[seed; 32]);
        let mut db = Database::open(
            path,
            Owner {
                void_id: id,
                public_key: key.verifying_key().as_bytes(),
            },
        )
        .unwrap();
        db.initialize_signal(id, &key).unwrap();
        let chat = db.create_conversation(peer, unix_millis()).unwrap();
        db.close().unwrap();
        let ui = AppWindow::new().unwrap();
        ui.set_account_void_id(id.into());
        ui.set_setup_page(0);
        let root = path.to_owned();
        let storage = bind_fixture(&ui, move |_| (root.clone(), key.clone())).unwrap();
        let activity = storage.relay_activity();
        let (connection, relay) = crate::relay::Connection::spawn_with_test_protocol(
            endpoint.into(),
            audience.into(),
            crate::relay::ApplicationProtocolVersion::V2,
            move |_| {
                Ok(account::RegisteredIdentity {
                    void_id: id.into(),
                    key: ed25519_dalek::SigningKey::from_bytes(&[seed; 32]),
                })
            },
            crate::report_relay_status(&ui),
            activity,
        )
        .unwrap();
        storage.opener()(&ui);
        Self {
            ui,
            storage: Some(storage),
            relay: Some(relay),
            connection,
            chat,
        }
    }
    fn stop(&mut self) {
        if let Some(relay) = self.relay.take() {
            relay.finish();
        }
        if let Some(storage) = self.storage.take() {
            storage.finish();
        }
    }
}
impl Drop for NativeFixture {
    fn drop(&mut self) {
        self.stop();
    }
}

#[test]
#[ignore = "requires explicitly provisioned loopback Go relay and synthetic accounts 101/202"]
fn native_callbacks_exchange_features_offline_and_restore_reply_drafts() {
    assert_eq!(std::env::var("VOID_TEST_DISPOSABLE").as_deref(), Ok("1"));
    assert_eq!(
        std::env::var("VOID_TEST_BACKEND_COMMIT").as_deref(),
        Ok("2faffd27f12d1d02ec28e4be298610d4fee50856")
    );
    let endpoint = std::env::var("VOID_TEST_RELAY_URL").unwrap();
    let audience = std::env::var("VOID_TEST_RELAY_AUDIENCE").unwrap();
    i_slint_backend_testing::init_integration_test_with_system_time();
    let alice_root = TestDir::new();
    let bob_root = TestDir::new();
    let a = std::rc::Rc::new(std::cell::RefCell::new(NativeFixture::new(
        &alice_root.0,
        "101",
        "202",
        121,
        &endpoint,
        &audience,
    )));
    let b = std::rc::Rc::new(std::cell::RefCell::new(NativeFixture::new(
        &bob_root.0,
        "202",
        "101",
        122,
        &endpoint,
        &audience,
    )));
    let aa = a.clone();
    let bb = b.clone();
    let apath = alice_root.0.clone();
    let bpath = bob_root.0.clone();
    let phase = std::rc::Rc::new(Cell::new(0));
    let observed = phase.clone();
    let mut target = None::<(slint::SharedString, slint::SharedString)>;
    let started = Instant::now();
    let timer = Timer::default();
    timer.start(TimerMode::Repeated, Duration::from_millis(40), move || {
        assert!(
            started.elapsed() < Duration::from_secs(90),
            "native V2 fixture timed out in phase {}: A={}, B={}",
            observed.get(),
            aa.borrow().ui.get_message_action_error(),
            bb.borrow().ui.get_message_action_error()
        );
        let ui = aa.borrow().ui.as_weak().upgrade().unwrap();
        let ub = bb.borrow().ui.as_weak().upgrade().unwrap();
        let row = ui.get_stored_messages().row_data(0);
        let brow = ub.get_stored_messages().row_data(0);
        match observed.get() {
            0 if ui.get_storage_ready() && ub.get_storage_ready() => {
                aa.borrow().connection.connect("101".into());
                bb.borrow().connection.connect("202".into());
                observed.set(1);
            }
            1 if ui.get_remote_actions_supported() && ub.get_remote_actions_supported() => {
                ui.invoke_select_stored_conversation(aa.borrow().chat.to_string().into());
                ub.invoke_select_stored_conversation(bb.borrow().chat.to_string().into());
                observed.set(2);
            }
            2 if !ui.get_conversation_loading()
                && !ub.get_conversation_loading()
                && !ui.get_selected_conversation().is_empty() =>
            {
                ui.set_stored_draft("Original from native UI".into());
                ui.invoke_save_stored_draft(ui.get_stored_draft());
                ui.invoke_send_message();
                observed.set(3);
            }
            3 if brow.is_some() && row.as_ref().is_some_and(|r| r.status == "Delivered") => {
                let r = brow.unwrap();
                target = Some((r.author_id.clone(), r.event_id.clone()));
                ub.invoke_choose_reply(r.author_id, r.event_id);
                observed.set(4);
            }
            4 if !ub.get_reply_event_id().is_empty() && !ub.get_action_busy() => {
                assert_eq!(ub.get_reply_author(), "Contact");
                ub.set_stored_draft("Reply from native UI".into());
                ub.invoke_save_stored_draft(ub.get_stored_draft());
                ub.invoke_send_message();
                observed.set(5);
            }
            5 if ui.get_stored_messages().row_count() == 2
                && ub
                    .get_stored_messages()
                    .row_data(1)
                    .is_some_and(|r| r.status == "Delivered") =>
            {
                assert_eq!(
                    ui.get_stored_messages().row_data(1).unwrap().quote,
                    "Original from native UI"
                );
                let (author, event) = target.clone().unwrap();
                ub.invoke_toggle_reaction(author, event, "👍".into());
                observed.set(6);
            }
            6 if row.as_ref().is_some_and(|r| r.reactions.row_count() == 1)
                && brow
                    .as_ref()
                    .is_some_and(|r| r.reactions.row_data(0).is_some_and(|r| r.mine))
                && !ub.get_pending_controls() =>
            {
                let (author, event) = target.clone().unwrap();
                ub.invoke_toggle_reaction(author, event, "👍".into());
                observed.set(7);
            }
            7 if row.as_ref().is_some_and(|r| r.reactions.row_count() == 0)
                && brow.as_ref().is_some_and(|r| r.reactions.row_count() == 0)
                && !ub.get_action_busy() =>
            {
                let (author, event) = target.clone().unwrap();
                ui.invoke_choose_reply(author, event);
                observed.set(8);
            }
            8 if !ui.get_reply_event_id().is_empty() && !ui.get_action_busy() => {
                ui.set_stored_draft("Draft survives restart".into());
                ui.invoke_save_stored_draft(ui.get_stored_draft());
                // Close is queued behind this local intent, before its 600 ms
                // dispatch turn. The new worker must recover it on V2 startup.
                let (author, event) = target.clone().unwrap();
                ui.invoke_toggle_reaction(author, event, "🎉".into());
                aa.borrow_mut().stop();
                *aa.borrow_mut() =
                    NativeFixture::new(&apath, "101", "202", 121, &endpoint, &audience);
                observed.set(9);
            }
            9 if ui.get_storage_ready() => {
                ui.invoke_select_stored_conversation(aa.borrow().chat.to_string().into());
                aa.borrow().connection.connect("101".into());
                observed.set(10);
            }
            10 if ui.get_remote_actions_supported()
                && ui.get_stored_draft() == "Draft survives restart"
                && !ui.get_reply_event_id().is_empty()
                && brow.as_ref().is_some_and(|r| r.reactions.row_count() == 1)
                && !ui.get_pending_controls() =>
            {
                bb.borrow_mut().stop();
                ui.invoke_send_message();
                observed.set(17);
            }
            17 if !ui.get_send_busy() && ui.get_stored_messages().row_count() == 3 => {
                let (author, event) = target.clone().unwrap();
                ui.invoke_toggle_reaction(author, event, "🔥".into());
                observed.set(11);
            }
            11 if !ui.get_action_busy()
                && !ui.get_pending_controls()
                && row.as_ref().is_some_and(|r| r.reactions.row_count() == 2) =>
            {
                let (author, event) = target.clone().unwrap();
                ui.invoke_delete_message(author, event, true);
                observed.set(12);
            }
            12 if !ui.get_action_busy()
                && !ui.get_pending_controls()
                && row.as_ref().is_some_and(|r| r.deleted) =>
            {
                *bb.borrow_mut() =
                    NativeFixture::new(&bpath, "202", "101", 122, &endpoint, &audience);
                observed.set(13);
            }
            13 if ub.get_storage_ready() => {
                ub.invoke_select_stored_conversation(bb.borrow().chat.to_string().into());
                bb.borrow().connection.connect("202".into());
                observed.set(14);
            }
            14 if brow.as_ref().is_some_and(|r| r.deleted)
                && ub.get_stored_messages().row_count() == 3
                && ui
                    .get_stored_messages()
                    .row_data(2)
                    .is_some_and(|r| r.status == "Delivered")
                && ub.get_remote_actions_supported() =>
            {
                let reply = ub.get_stored_messages().row_data(1).unwrap();
                assert_eq!(reply.quote, "Message deleted");
                ub.invoke_delete_message(reply.author_id, reply.event_id, false);
                observed.set(15);
            }
            15 if !ub.get_action_busy() && ub.get_stored_messages().row_count() == 2 => {
                observed.set(16);
                slint::quit_event_loop().unwrap();
            }
            _ => (),
        }
    });
    slint::run_event_loop().unwrap();
    timer.stop();
    a.borrow_mut().stop();
    b.borrow_mut().stop();
    assert_eq!(phase.get(), 16);
    let db = Database::open(
        &alice_root.0,
        Owner {
            void_id: "101",
            public_key: ed25519_dalek::SigningKey::from_bytes(&[121; 32])
                .verifying_key()
                .as_bytes(),
        },
    )
    .unwrap();
    assert!(db.draft(a.borrow().chat).unwrap().is_none());
    assert!(db.reply_draft(a.borrow().chat).unwrap().is_none());
    assert!(db.pending_controls().unwrap().is_empty());
}

#[test]
fn v1_blocks_remote_actions_while_local_delete_and_reply_cancellation_work() {
    if !in_test_process(
        "storage_ui::tests::message_actions::v1_blocks_remote_actions_while_local_delete_and_reply_cancellation_work",
    ) {
        return;
    }
    let root = TestDir::new();
    let key = ed25519_dalek::SigningKey::from_bytes(&[81; 32]);
    let mut db = Database::open(
        &root.0,
        Owner {
            void_id: "41",
            public_key: key.verifying_key().as_bytes(),
        },
    )
    .unwrap();
    let chat = db.create_conversation("123", 100).unwrap();
    let event = "00000000-0000-4000-8000-000000000001";
    db.commit_incoming("123", event, &[8; 32], |_| {
        Ok(Message {
            id: "original".into(),
            conversation_id: chat,
            direction: Direction::Incoming,
            body: "Saved original".into(),
            created_at: 110,
            delivery_state: DeliveryState::Received,
        })
    })
    .unwrap();
    db.commit_incoming(
        "123",
        "00000000-0000-4000-8000-000000000002",
        &[9; 32],
        |_| {
            Ok(Message {
                id: "next".into(),
                conversation_id: chat,
                direction: Direction::Incoming,
                body: "Next message".into(),
                created_at: 120,
                delivery_state: DeliveryState::Received,
            })
        },
    )
    .unwrap();
    db.save_reply_draft(
        chat,
        "Retain this draft",
        Some(&MessageReference {
            author_void_id: "123".into(),
            event_id: event.into(),
        }),
    )
    .unwrap();
    db.close().unwrap();
    i_slint_backend_testing::init_integration_test_with_system_time();
    let ui = AppWindow::new().unwrap();
    ui.set_account_void_id("41".into());
    ui.set_setup_page(0);
    let path = root.0.clone();
    let saved_key = key.clone();
    let worker = bind_fixture(&ui, move |_| (path.clone(), saved_key.clone())).unwrap();
    worker.opener()(&ui);
    let phase = std::rc::Rc::new(Cell::new(0));
    let observed = phase.clone();
    let weak = ui.as_weak();
    let started = Instant::now();
    let timer = Timer::default();
    timer.start(TimerMode::Repeated, Duration::from_millis(15), move || {
        let ui = weak.upgrade().unwrap();
        assert!(started.elapsed() < Duration::from_secs(12));
        match observed.get() {
            0 if ui.get_storage_ready() => {
                ui.invoke_select_stored_conversation(chat.to_string().into());
                observed.set(1);
            }
            1 if !ui.get_conversation_loading() && ui.get_reply_event_id() == event => {
                assert_eq!(ui.get_reply_author(), "Contact");
                assert_eq!(ui.get_reply_preview(), "Saved original");
                assert_eq!(
                    ui.get_stored_messages().row_data(0).unwrap().event_id,
                    event
                );
                ui.invoke_send_message(); // V1 cannot consume a restored reply draft.
                ui.invoke_toggle_reaction("123".into(), event.into(), "👍".into());
                ui.invoke_delete_message("123".into(), event.into(), true);
                assert!(!ui.get_send_busy());
                assert!(!ui.get_action_busy());
                ui.invoke_cancel_reply();
                observed.set(2);
            }
            2 if !ui.get_action_busy() && ui.get_reply_event_id().is_empty() => {
                assert_eq!(ui.get_stored_draft(), "Retain this draft");
                // Even a stale/manipulated UI flag cannot bypass the worker gate.
                ui.set_remote_actions_supported(true);
                ui.invoke_toggle_reaction("123".into(), event.into(), "👍".into());
                observed.set(3);
            }
            3 if !ui.get_action_busy() && !ui.get_message_action_error().is_empty() => {
                ui.set_remote_actions_supported(false);
                ui.set_timeline_following(false);
                ui.set_timeline_anchor_key("110:m:original".into());
                ui.set_timeline_anchor_offset(16.0);
                ui.invoke_delete_message("123".into(), event.into(), false);
                observed.set(4);
            }
            4 if !ui.get_action_busy() && ui.get_stored_messages().row_count() == 1 => {
                assert_eq!(ui.get_timeline_anchor_key(), "120:m:next");
                assert_eq!(ui.get_timeline_anchor_offset(), 16.0);
                assert!(!ui.get_timeline_following());
                observed.set(5);
                slint::quit_event_loop().unwrap();
            }
            _ => (),
        }
    });
    slint::run_event_loop().unwrap();
    timer.stop();
    worker.finish();
    assert_eq!(phase.get(), 5);
    let db = Database::open(
        &root.0,
        Owner {
            void_id: "41",
            public_key: key.verifying_key().as_bytes(),
        },
    )
    .unwrap();
    assert!(db.reply_draft(chat).unwrap().is_none());
    assert_eq!(
        db.draft(chat).unwrap().as_deref(),
        Some("Retain this draft")
    );
    assert!(db.pending_controls().unwrap().is_empty());
    assert_eq!(db.timeline_before(chat, None, 120).unwrap().len(), 1);
    assert_eq!(db.messages(chat).unwrap().len(), 2);
}

#[test]
#[ignore = "native rendering of isolated message action fixtures; no saved accounts"]
fn render_message_actions_at_desktop_sizes() {
    let ui = AppWindow::new().unwrap();
    ui.set_setup_page(0);
    ui.set_storage_ready(true);
    ui.set_selected_conversation("1".into());
    ui.set_conversation_title("Contact".into());
    ui.set_remote_actions_supported(true);
    ui.set_reply_event_id("00000000-0000-4000-8000-000000000001".into());
    ui.set_reply_author("Contact".into());
    ui.set_reply_preview("A compact original message with a meaningful preview".into());
    let message = StoredMessage {
        author_id: "123".into(),
        event_id: "00000000-0000-4000-8000-000000000001".into(),
        key: "110:m:original".into(),
        body: "A compact message".into(),
        time: "19:38".into(),
        outgoing: true,
        status: "Delivered".into(),
        reactions: ModelRc::new(VecModel::from(vec![crate::StoredReaction {
            emoji: "👍".into(),
            count: 2,
            mine: true,
        }])),
        ..Default::default()
    };
    ui.set_stored_messages(ModelRc::new(VecModel::from(vec![message.clone()])));
    ui.show().unwrap();
    let weak = ui.as_weak();
    let step = Cell::new(0);
    let timer = Timer::default();
    timer.start(TimerMode::Repeated, Duration::from_millis(500), move || {
        let ui = weak.upgrade().unwrap();
        let i = step.get();
        if i > 0 {
            let image = ui.window().take_snapshot().unwrap();
            let mut bytes = format!("P6\n{} {}\n255\n", image.width(), image.height()).into_bytes();
            for p in image.as_slice() {
                bytes.extend_from_slice(&[p.r, p.g, p.b]);
            }
            std::fs::create_dir_all("target/message-action-renders").unwrap();
            std::fs::write(format!("target/message-action-renders/{i}.ppm"), bytes).unwrap();
        }
        let sizes = [(850, 560), (1080, 680), (1280, 720), (1600, 900)];
        if i >= 15 {
            ui.hide().unwrap();
            slint::quit_event_loop().unwrap();
        } else if i == 8 {
            ui.window().set_size(slint::PhysicalSize::new(850, 560));
            ui.invoke_open_message_actions(message.clone(), "confirm".into(), 550.0, 360.0);
        } else if i == 9 {
            ui.set_remote_actions_supported(false);
        } else if i == 10 {
            ui.invoke_open_message_actions(message.clone(), "actions".into(), 550.0, 360.0);
        } else if i == 11 {
            ui.window()
                .dispatch_event(slint::platform::WindowEvent::KeyPressed {
                    text: "\u{001b}".into(),
                });
            ui.window()
                .dispatch_event(slint::platform::WindowEvent::KeyReleased {
                    text: "\u{001b}".into(),
                });
            ui.window().set_size(slint::PhysicalSize::new(1280, 720));
        } else if i == 12 {
            ui.set_remote_actions_supported(true);
            ui.window()
                .dispatch_event(slint::platform::WindowEvent::PointerMoved {
                    position: slint::LogicalPosition::new(1240.0, 508.0),
                });
        } else if i == 13 {
            ui.window()
                .dispatch_event(slint::platform::WindowEvent::PointerMoved {
                    position: slint::LogicalPosition::new(1030.0, 508.0),
                });
        } else if i == 14 {
            ui.window()
                .dispatch_event(slint::platform::WindowEvent::PointerExited);
        } else {
            let (w, h) = sizes[i as usize / 2];
            ui.window().set_size(slint::PhysicalSize::new(w, h));
            ui.invoke_open_message_actions(
                message.clone(),
                if i % 2 == 0 { "react" } else { "delete" }.into(),
                w as f32 * 0.65,
                h as f32 * 0.65,
            );
        }
        step.set(i + 1);
    });
    slint::run_event_loop().unwrap();
}
