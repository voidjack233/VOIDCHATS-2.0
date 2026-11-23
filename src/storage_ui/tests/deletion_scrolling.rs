use super::*;
use i_slint_backend_testing::ElementHandle;

#[test]
fn deleting_the_visible_anchor_keeps_its_survivor_at_the_same_pixel() {
    if !in_test_process(
        "storage_ui::tests::deletion_scrolling::deleting_the_visible_anchor_keeps_its_survivor_at_the_same_pixel",
    ) {
        return;
    }
    i_slint_backend_testing::init_integration_test_with_system_time();
    fixture(false);
}

#[test]
#[ignore = "isolated native deletion scroll-position verification"]
fn native_deletion_preserves_visible_message_positions() {
    fixture(true);
}

fn bubble_y(ui: &AppWindow, body: &str) -> f32 {
    ElementHandle::find_by_element_id(ui, "MessageBubble::root")
        .find(|element| {
            element
                .accessible_label()
                .is_some_and(|label| label == body)
        })
        .expect("visible survivor must remain rendered")
        .absolute_position()
        .y
}

fn remove(db: &mut Database, chat: i64, row: StoredMessage) {
    db.delete_for_me(
        chat,
        &storage::message_events::MessageReference {
            author_void_id: row.author_id.into(),
            event_id: row.event_id.into(),
        },
    )
    .unwrap();
}

fn viewport(ui: &AppWindow) -> ElementHandle {
    ElementHandle::find_by_element_id(ui, "MessageTimeline::timeline")
        .next()
        .unwrap()
}

fn fixture(_native: bool) {
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
    for index in 0..100 {
        db.queue_outgoing(
            chat,
            &format!("Message {index}: {}", "Wrapped text ".repeat(index % 5 + 1)),
            1_700_000_000_000 + index as i64,
        )
        .unwrap();
    }
    let window = move |db: &Database| TimelineWindow {
        messages: db.timeline_before(chat, None, 60).unwrap(),
        has_older: true,
        has_newer: false,
    };
    let ui = AppWindow::new().unwrap();
    bind_timeline_positions(&ui);
    ui.set_setup_page(0);
    ui.set_storage_ready(true);
    ui.set_selected_conversation(chat.to_string().into());
    show_window(&ui, window(&db));
    ui.show().unwrap();
    ui.window().set_size(slint::PhysicalSize::new(1080, 680));
    let phase = std::rc::Rc::new(Cell::new(0));
    let seen = phase.clone();
    let weak = ui.as_weak();
    let timer = Timer::default();
    let mut survivor = StoredMessage::default();
    let mut expected_y = 0.0;
    let mut viewport_height = 0.0;
    let mut expected_bottom = 0.0;
    let mut settling = 2;
    timer.start(TimerMode::Repeated, Duration::from_millis(300), move || {
        let ui = weak.upgrade().unwrap();
        // The lightweight backend does not draw. Evaluate layout, then let
        // geometry notifications and the existing restoration timer run, just
        // as native frames do, before comparing rendered coordinates.
        let view = viewport(&ui);
        let _ = (view.absolute_position(), view.size());
        let history = ElementHandle::find_by_element_id(&ui, "MessageTimeline::history")
            .next()
            .unwrap();
        let _ = (history.absolute_position(), history.size());
        if settling > 0 {
            settling -= 1;
            return;
        }
        settling = 2;
        if (2..=5).contains(&seen.get()) {
            let actual = bubble_y(&ui, &survivor.body);
            assert!(
                (actual - expected_y).abs() < 1.5,
                "phase {} moved a surviving message: before={expected_y}, after={actual}",
                seen.get()
            );
            assert!(!ui.get_timeline_following());
        }
        match seen.get() {
            0 => {
                let anchor = ui.get_stored_messages().row_data(25).unwrap();
                ui.set_timeline_following(false);
                ui.set_timeline_anchor_key(anchor.key);
                ui.set_timeline_anchor_offset(-9.0);
                seen.set(1);
            }
            1 => {
                let anchor = ui.get_stored_messages().row_data(25).unwrap();
                survivor = ui.get_stored_messages().row_data(26).unwrap();
                expected_y = bubble_y(&ui, &survivor.body);
                remove(&mut db, chat, anchor);
                update_timeline(&ui, window(&db), false, true);
                seen.set(2);
            }
            2 => {
                remove(&mut db, chat, ui.get_stored_messages().row_data(0).unwrap());
                update_timeline(&ui, window(&db), false, true);
                seen.set(3);
            }
            3 => {
                let model = ui.get_stored_messages();
                let index = model
                    .iter()
                    .position(|row| row.key == survivor.key)
                    .unwrap();
                remove(&mut db, chat, model.row_data(index + 3).unwrap());
                update_timeline(&ui, window(&db), false, true);
                seen.set(4);
            }
            4 => {
                let model = ui.get_stored_messages();
                let index = model
                    .iter()
                    .position(|row| row.key == survivor.key)
                    .unwrap();
                survivor = model.row_data(index + 3).unwrap();
                expected_y = bubble_y(&ui, &survivor.body);
                for index in index..index + 3 {
                    remove(&mut db, chat, model.row_data(index).unwrap());
                }
                update_timeline(&ui, window(&db), false, true);
                seen.set(5);
            }
            5 => {
                ui.set_timeline_following(true);
                ui.set_timeline_anchor_key("".into());
                viewport_height = viewport(&ui).size().height;
                seen.set(6);
            }
            6 => {
                let model = ui.get_stored_messages();
                let last = model.row_data(model.row_count() - 1).unwrap();
                let bubble = ElementHandle::find_by_element_id(&ui, "MessageBubble::root")
                    .find(|e| e.accessible_label().is_some_and(|label| label == last.body))
                    .unwrap();
                expected_bottom = bubble.absolute_position().y + bubble.size().height;
                remove(&mut db, chat, last);
                ui.set_pending_controls(true);
                update_timeline(&ui, window(&db), false, true);
                seen.set(7);
            }
            _ => {
                assert!(ui.get_timeline_following());
                assert!(
                    (viewport(&ui).size().height - viewport_height).abs() < 0.5,
                    "queued deletion feedback must not shrink the message viewport"
                );
                let model = ui.get_stored_messages();
                let last = model.row_data(model.row_count() - 1).unwrap();
                let bubble = ElementHandle::find_by_element_id(&ui, "MessageBubble::root")
                    .find(|e| e.accessible_label().is_some_and(|label| label == last.body))
                    .unwrap();
                assert!(
                    (bubble.absolute_position().y + bubble.size().height - expected_bottom).abs()
                        < 1.5,
                    "following the present must stay pinned to the same bottom edge"
                );
                seen.set(8);
                ui.hide().unwrap();
                slint::quit_event_loop().unwrap();
            }
        }
    });
    slint::run_event_loop().unwrap();
    timer.stop();
    assert_eq!(phase.get(), 8);
}
