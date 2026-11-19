use super::*;
use crate::{
    relay::{ApplicationProtocolVersion, ConnectionStatus},
    report_relay_status,
};

#[test]
fn update_gate_blocks_sends_preserves_storage_and_latches_until_close() {
    if !in_test_process(
        "storage_ui::tests::update_required::update_gate_blocks_sends_preserves_storage_and_latches_until_close",
    ) {
        return;
    }
    let root = TestDir::new();
    let key = ed25519_dalek::SigningKey::from_bytes(&[119; 32]);
    let public = key.verifying_key();
    let mut db = Database::open(
        &root.0,
        Owner {
            void_id: "41",
            public_key: public.as_bytes(),
        },
    )
    .unwrap();
    db.initialize_signal("41", &key).unwrap();
    let publication = db.prepare_publication().unwrap().unwrap();
    db.confirm_publication(&publication.id).unwrap();
    let chat = db.create_conversation("123", 100).unwrap();
    db.save_message(&Message {
        id: "preserved".into(),
        conversation_id: chat,
        direction: Direction::Incoming,
        body: "Saved history".into(),
        created_at: 110,
        delivery_state: DeliveryState::Received,
    })
    .unwrap();
    db.save_draft(chat, "Unsent draft").unwrap();
    db.close().unwrap();
    let protected = std::fs::read(root.0.join("messages/messages-key.dpapi")).unwrap();
    i_slint_backend_testing::init_integration_test_with_system_time();
    let ui = AppWindow::new().unwrap();
    ui.set_account_void_id("41".into());
    ui.set_setup_page(0);
    let path = root.0.clone();
    let worker = bind_fixture(&ui, move |_| (path.clone(), key.clone())).unwrap();
    worker.opener()(&ui);
    let report = report_relay_status(&ui);
    let weak = ui.as_weak();
    let phase = std::rc::Rc::new(Cell::new(0));
    let seen = phase.clone();
    let closed = std::rc::Rc::new(Cell::new(false));
    let close_flag = closed.clone();
    ui.on_close_for_update(move || close_flag.set(true));
    let started = Instant::now();
    let timer = Timer::default();
    timer.start(TimerMode::Repeated, Duration::from_millis(20), move || {
        let ui = weak.upgrade().unwrap();
        assert!(started.elapsed() < Duration::from_secs(10));
        if seen.get() == 0 && ui.get_storage_ready() {
            report(ConnectionStatus::UpdateRequired(
                ApplicationProtocolVersion::V2,
            ));
            seen.set(1);
        } else if seen.get() == 1 && ui.get_update_required() {
            assert_eq!(ui.get_required_protocol_version(), "2");
            assert!(!ui.get_can_reconnect());
            assert!(!ui.get_messaging_ready());
            ui.set_selected_conversation(chat.to_string().into());
            ui.set_stored_draft("Should not queue".into());
            ui.invoke_send_message();
            ui.invoke_resend_message("unavailable-event".into());
            report(ConnectionStatus::Connected);
            seen.set(2);
        } else if seen.get() == 2 {
            assert!(ui.get_update_required());
            assert_eq!(ui.get_connection_status(), "Update required");
            ui.invoke_close_for_update();
            seen.set(3);
            slint::quit_event_loop().unwrap();
        }
    });
    slint::run_event_loop().unwrap();
    timer.stop();
    worker.finish();
    assert!(closed.get());
    assert_eq!(phase.get(), 3);
    let mut db = Database::open(
        &root.0,
        Owner {
            void_id: "41",
            public_key: public.as_bytes(),
        },
    )
    .unwrap();
    assert_eq!(db.messages(chat).unwrap().len(), 1);
    assert_eq!(db.conversation_timeline(chat).unwrap().len(), 1);
    assert_eq!(db.messages(chat).unwrap()[0].body, "Saved history");
    assert_eq!(db.draft(chat).unwrap().as_deref(), Some("Unsent draft"));
    assert!(
        db.prepare_publication().unwrap().is_none(),
        "Signal enrollment must remain unchanged"
    );
    assert!(std::fs::read(root.0.join("messages/messages-key.dpapi")).unwrap() == protected);
}

#[test]
#[ignore = "native update screen fixture; no accounts or network"]
fn render_update_required_screen() {
    let ui = AppWindow::new().unwrap();
    ui.set_setup_page(0);
    ui.set_required_protocol_version("2".into());
    ui.on_close_for_update(|| {
        slint::quit_event_loop().unwrap();
    });
    ui.show().unwrap();
    let weak = ui.as_weak();
    let step = Cell::new(0);
    let timer = Timer::default();
    timer.start(TimerMode::Repeated, Duration::from_millis(350), move || {
        let ui = weak.upgrade().unwrap();
        let i = step.get();
        if i == 0 {
            ui.window().set_size(slint::PhysicalSize::new(850, 560));
        } else {
            let pixels = ui.window().take_snapshot().unwrap();
            let mut bytes =
                format!("P6\n{} {}\n255\n", pixels.width(), pixels.height()).into_bytes();
            for pixel in pixels.as_slice() {
                bytes.extend_from_slice(&[pixel.r, pixel.g, pixel.b]);
            }
            std::fs::create_dir_all("target/update-required-renders").unwrap();
            std::fs::write(format!("target/update-required-renders/{i}.ppm"), bytes).unwrap();
            if i == 1 {
                ui.window().set_size(slint::PhysicalSize::new(1600, 900));
            } else {
                ui.hide().unwrap();
                ui.invoke_close_for_update();
            }
        }
        step.set(i + 1);
    });
    slint::run_event_loop().unwrap();
}
