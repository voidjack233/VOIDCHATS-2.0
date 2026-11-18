use super::*;

#[test]
fn quote_callback_restores_original_outside_the_loaded_window() {
    if !in_test_process(
        "storage_ui::tests::presentation::quote_callback_restores_original_outside_the_loaded_window",
    ) {
        return;
    }
    std::thread::spawn(|| {
        let root = TestDir::new();
        let key = ed25519_dalek::SigningKey::from_bytes(&[118; 32]);
        let public = key.verifying_key();
        let mut db = Database::open(
            &root.0,
            Owner {
                void_id: "41",
                public_key: public.as_bytes(),
            },
        )
        .unwrap();
        let chat = db.create_conversation("123", 100).unwrap();
        for i in 0..140 {
            db.save_message(&Message {
                id: format!("fixture-{i:03}"),
                conversation_id: chat,
                direction: Direction::Incoming,
                body: "History".into(),
                created_at: 110 + i,
                delivery_state: DeliveryState::Received,
            })
            .unwrap();
        }
        let first = db.timeline_before(chat, None, usize::MAX).unwrap()[0]
            .cursor
            .key();
        db.save_draft(chat, "Preserve draft").unwrap();
        db.close().unwrap();
        i_slint_backend_testing::init_integration_test_with_system_time();
        let ui = AppWindow::new().unwrap();
        ui.set_account_void_id("41".into());
        ui.set_setup_page(0);
        let path = root.0.clone();
        let worker = bind_fixture(&ui, move |_| (path.clone(), key.clone())).unwrap();
        worker.opener()(&ui);
        let weak = ui.as_weak();
        let sender = worker.sender.clone();
        let phase = std::rc::Rc::new(Cell::new(0));
        let observed = phase.clone();
        let first_key = first.clone();
        let started = Instant::now();
        let timer = Timer::default();
        timer.start(TimerMode::Repeated, Duration::from_millis(10), move || {
            let ui = weak.upgrade().unwrap();
            assert!(
                started.elapsed() < Duration::from_secs(12),
                "quote navigation timed out"
            );
            if observed.get() == 0 && ui.get_storage_ready() {
                ui.set_selected_conversation(chat.to_string().into());
                ui.set_storage_selection_token(1);
                sender
                    .send(crate::storage_ui::Command::Select(chat, 1, false, None))
                    .unwrap();
                observed.set(1);
            } else if observed.get() == 1
                && ui.get_stored_messages().row_count() > 0
                && !ui.get_conversation_loading()
            {
                assert_ne!(
                    ui.get_stored_messages().row_data(0).unwrap().key.as_str(),
                    first_key
                );
                ui.invoke_open_quote(first_key.clone().into());
                observed.set(2);
            } else if observed.get() == 2
                && !ui.get_timeline_loading()
                && ui.get_timeline_anchor_key() == first_key
            {
                assert!(!ui.get_timeline_following());
                assert!(ui.get_timeline_has_newer());
                assert!(ui.get_stored_messages().row_count() <= 120);
                assert_eq!(
                    ui.get_stored_messages().row_data(0).unwrap().key.as_str(),
                    first_key
                );
                observed.set(3);
                slint::quit_event_loop().unwrap();
            }
        });
        slint::run_event_loop().unwrap();
        timer.stop();
        worker.finish();
        assert_eq!(phase.get(), 3);
        let db = Database::open(
            &root.0,
            Owner {
                void_id: "41",
                public_key: public.as_bytes(),
            },
        )
        .unwrap();
        assert_eq!(db.draft(chat).unwrap().as_deref(), Some("Preserve draft"));
    })
    .join()
    .unwrap();
}

#[test]
#[ignore = "isolated native rendering; opens a fixture window, never a saved account"]
fn render_receiver_presentation_at_desktop_sizes() {
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
    db.save_message(&Message {
        id: "original".into(),
        conversation_id: chat,
        direction: Direction::Incoming,
        body: "A compact original message".into(),
        created_at: 110,
        delivery_state: DeliveryState::Received,
    })
    .unwrap();
    db.save_message(&Message {
        id: "reply".into(),
        conversation_id: chat,
        direction: Direction::Outgoing,
        body:
            "A longer reply that wraps naturally while keeping the quoted preview above the bubble."
                .into(),
        created_at: 120,
        delivery_state: DeliveryState::Delivered,
    })
    .unwrap();
    let mut messages = db.timeline_before(chat, None, 120).unwrap();
    messages[1].presentation = storage::presentation::Presentation {
        quote: "A compact original message".into(),
        author: "Contact".into(),
        target: messages[0].cursor.key(),
        reactions: vec![("👍".into(), 2, true)],
        ..Default::default()
    };
    let mut deleted = messages[0].clone();
    deleted.cursor = TimelineCursor::from_key("130:m:deleted-fixture").unwrap();
    deleted.presentation.deleted = true;
    deleted.body = "Must never display deleted content".into();
    messages.push(deleted);
    let mut missing = messages[1].clone();
    missing.cursor = TimelineCursor::from_key("140:m:missing-fixture").unwrap();
    missing.body = "Reply to an unavailable original".into();
    missing.presentation.quote = "Message unavailable".into();
    missing.presentation.target.clear();
    missing.presentation.reactions.clear();
    messages.push(missing);
    let ui = AppWindow::new().unwrap();
    ui.set_setup_page(0);
    ui.set_selected_conversation(chat.to_string().into());
    ui.set_storage_ready(true);
    show_window(
        &ui,
        TimelineWindow {
            messages,
            has_older: false,
            has_newer: false,
        },
    );
    ui.show().unwrap();
    let weak = ui.as_weak();
    let step = Cell::new(0);
    let timer = Timer::default();
    timer.start(TimerMode::Repeated, Duration::from_millis(400), move || {
        let ui = weak.upgrade().unwrap();
        let i = step.get();
        if i == 0 {
            ui.window().set_size(slint::PhysicalSize::new(850, 560));
        } else {
            let image = ui.window().take_snapshot().unwrap();
            assert!(image.width() >= 850 && image.height() >= 560);
            let path = root.0.join(format!("render-{i}.ppm"));
            let mut bytes = format!("P6\n{} {}\n255\n", image.width(), image.height()).into_bytes();
            for pixel in image.as_slice() {
                bytes.extend_from_slice(&[pixel.r, pixel.g, pixel.b]);
            }
            std::fs::create_dir_all("target/presentation-renders").unwrap();
            std::fs::write(format!("target/presentation-renders/render-{i}.ppm"), bytes).unwrap();
            let _ = path;
            if i == 1 {
                ui.window().set_size(slint::PhysicalSize::new(1280, 720));
            } else if i == 2 {
                ui.window().set_size(slint::PhysicalSize::new(1600, 900));
            } else {
                ui.hide().unwrap();
                slint::quit_event_loop().unwrap();
            }
        }
        step.set(i + 1);
    });
    slint::run_event_loop().unwrap();
}
