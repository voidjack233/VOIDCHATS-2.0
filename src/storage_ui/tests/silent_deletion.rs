//! Silent removal uses the real SQLCipher visibility queries and Slint model.
use super::*;
use crate::storage::message_events::MessageReference;

#[test]
fn deleted_anchor_and_reply_disappear_without_empty_timeline_rows() {
    if !in_test_process(
        "storage_ui::tests::silent_deletion::deleted_anchor_and_reply_disappear_without_empty_timeline_rows",
    ) {
        return;
    }
    i_slint_backend_testing::init_integration_test_with_system_time();
    fixture(false);
}

#[test]
#[ignore = "isolated native snapshots; no saved accounts or relay connections"]
fn render_silent_deletion_at_desktop_sizes() {
    fixture(true);
}

fn fixture(native: bool) {
    let root = TestDir::new();
    let owner = Owner {
        void_id: "41",
        public_key: &[5; 32],
    };
    let mut db = Database::open(&root.0, owner).unwrap();
    let chat = db.create_conversation("123", 100).unwrap();
    let (first, _) = db
        .queue_outgoing(chat, "Hello!", 1_700_000_000_000)
        .unwrap();
    let (removed, _) = db
        .queue_outgoing(chat, "This message will disappear", 1_700_100_000_000)
        .unwrap();
    let target = MessageReference {
        author_void_id: "41".into(),
        event_id: removed.clone(),
    };
    db.save_reply_draft(chat, "How are you?", Some(&target))
        .unwrap();
    let (last, _) = db
        .queue_outgoing(chat, "How are you?", 1_700_200_000_000)
        .unwrap();
    db.save_reply_draft(chat, "Keep my unsent draft", Some(&target))
        .unwrap();
    let window = |db: &Database| TimelineWindow {
        messages: db.timeline_before(chat, None, 120).unwrap(),
        has_older: false,
        has_newer: false,
    };
    let ui = AppWindow::new().unwrap();
    ui.set_setup_page(0);
    ui.set_storage_ready(true);
    ui.set_selected_conversation(chat.to_string().into());
    ui.set_conversation_title("Isolated deletion fixture".into());
    show_window(&ui, window(&db));
    assert_eq!(ui.get_stored_messages().row_count(), 3);
    let old = ui.get_stored_messages().row_data(1).unwrap().key;
    ui.set_timeline_following(false);
    ui.set_timeline_anchor_key(old);
    ui.set_timeline_anchor_offset(8.0);
    db.queue_delete_everyone(
        chat,
        "00000000-0000-4000-8000-000000009999",
        &target,
        1_700_300_000_000,
    )
    .unwrap();
    update_timeline(&ui, window(&db), false, true);
    let model = ui.get_stored_messages();
    assert_eq!(model.row_count(), 2);
    assert_eq!(model.row_data(0).unwrap().event_id, first);
    let last_row = model.row_data(1).unwrap();
    assert_eq!(last_row.event_id, last);
    assert!(last_row.quote.is_empty());
    assert_eq!(ui.get_timeline_anchor_key(), last_row.key);
    assert_eq!(ui.get_timeline_anchor_offset(), 8.0);
    assert!(
        model
            .iter()
            .all(|r| !r.deleted && r.body != "This message will disappear")
    );
    assert_eq!(model.iter().filter(|r| !r.day.is_empty()).count(), 2);
    assert_eq!(
        db.draft(chat).unwrap().as_deref(),
        Some("Keep my unsent draft")
    );
    assert!(db.reply_draft(chat).unwrap().is_none());
    assert!(db.reference_cursor(chat, &target).unwrap().is_none());
    db.close().unwrap();
    let mut db = Database::open(&root.0, owner).unwrap();
    assert_eq!(db.timeline_before(chat, None, 120).unwrap().len(), 2);
    assert!(db.reply_draft(chat).unwrap().is_none());

    if native {
        ui.set_timeline_following(true);
        ui.show().unwrap();
        ui.window().set_size(slint::PhysicalSize::new(850, 560));
        let weak = ui.as_weak();
        let step = Cell::new(0);
        let timer = Timer::default();
        timer.start(TimerMode::Repeated, Duration::from_millis(400), move || {
            let ui = weak.upgrade().unwrap();
            let i = step.get();
            let image = ui.window().take_snapshot().unwrap();
            let mut bytes = format!("P6\n{} {}\n255\n", image.width(), image.height()).into_bytes();
            for pixel in image.as_slice() {
                bytes.extend_from_slice(&[pixel.r, pixel.g, pixel.b]);
            }
            std::fs::create_dir_all("target/silent-deletion-renders").unwrap();
            std::fs::write(format!("target/silent-deletion-renders/{i}.ppm"), bytes).unwrap();
            match i {
                0 => ui.window().set_size(slint::PhysicalSize::new(1080, 680)),
                1 => ui.window().set_size(slint::PhysicalSize::new(1600, 900)),
                _ => {
                    ui.hide().unwrap();
                    slint::quit_event_loop().unwrap();
                }
            }
            step.set(i + 1);
        });
        slint::run_event_loop().unwrap();
        timer.stop();
    }
    db.queue_delete_everyone(
        chat,
        "00000000-0000-4000-8000-000000009998",
        &MessageReference {
            author_void_id: "41".into(),
            event_id: last,
        },
        1_700_300_000_001,
    )
    .unwrap();
    assert_eq!(
        db.last_message_previews([chat]).unwrap()[&chat].body,
        "Hello!"
    );
    db.delete_for_me(
        chat,
        &MessageReference {
            author_void_id: "41".into(),
            event_id: first,
        },
    )
    .unwrap();
    update_timeline(&ui, window(&db), false, true);
    assert_eq!(ui.get_stored_messages().row_count(), 0);
    assert!(db.last_message_previews([chat]).unwrap().is_empty());
}
