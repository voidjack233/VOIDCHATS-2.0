use super::*;
use i_slint_backend_testing::ElementHandle;
use slint::platform::{PointerEventButton, WindowEvent};

#[test]
fn contact_menu_editing_and_opening_preserve_conversations_and_drafts() {
    if !in_test_process(
        "storage_ui::tests::contacts::contact_menu_editing_and_opening_preserve_conversations_and_drafts",
    ) {
        return;
    }
    i_slint_backend_testing::init_integration_test_with_system_time();
    fixture(false);
}

#[test]
#[ignore = "isolated native Contacts, contextual menu and local name editor"]
fn render_contacts_at_desktop_sizes() {
    fixture(true);
}

fn control(ui: &AppWindow, label: &str) -> ElementHandle {
    ElementHandle::find_by_accessible_label(ui, label)
        .next()
        .unwrap_or_else(|| panic!("missing control: {label}"))
}
fn click(ui: &AppWindow, label: &str) {
    let element = control(ui, label);
    let origin = element.absolute_position();
    let size = element.size();
    let position =
        slint::LogicalPosition::new(origin.x + size.width / 2.0, origin.y + size.height / 2.0);
    ui.window()
        .dispatch_event(WindowEvent::PointerMoved { position });
    ui.window().dispatch_event(WindowEvent::PointerPressed {
        position,
        button: PointerEventButton::Left,
    });
    ui.window().dispatch_event(WindowEvent::PointerReleased {
        position,
        button: PointerEventButton::Left,
    });
}
fn snapshot(ui: &AppWindow, native: bool, name: &str) {
    if !native {
        return;
    }
    let image = ui.window().take_snapshot().unwrap();
    let mut bytes = format!("P6\n{} {}\n255\n", image.width(), image.height()).into_bytes();
    for p in image.as_slice() {
        bytes.extend_from_slice(&[p.r, p.g, p.b]);
    }
    std::fs::create_dir_all("target/contacts-review").unwrap();
    std::fs::write(format!("target/contacts-review/{name}.ppm"), bytes).unwrap();
}

fn fixture(native: bool) {
    let root = TestDir::new();
    let key = ed25519_dalek::SigningKey::from_bytes(&[87; 32]);
    let public = key.verifying_key();
    let owner = Owner {
        void_id: "41",
        public_key: public.as_bytes(),
    };
    let mut db = Database::open(&root.0, owner).unwrap();
    let chat = db.create_conversation("123", 100).unwrap();
    let other = db.create_conversation("456", 100).unwrap();
    for id in [chat, other] {
        db.commit_incoming(
            if id == chat { "123" } else { "456" },
            &format!("incoming-{id}"),
            &[8; 32],
            |_| {
                Ok(Message {
                    id: format!("fixture-{id}"),
                    conversation_id: id,
                    direction: Direction::Incoming,
                    body: "Fixture incoming message".into(),
                    created_at: 110,
                    delivery_state: DeliveryState::Received,
                })
            },
        )
        .unwrap();
        db.queue_outgoing(id, "Fixture outgoing reply", 120)
            .unwrap();
    }
    db.rename_contact("456", &format!("Alex {}", "long name ".repeat(8)))
        .unwrap();
    db.save_draft(chat, "Unsent draft kept").unwrap();
    db.close().unwrap();
    let ui = AppWindow::new().unwrap();
    ui.set_setup_page(0);
    ui.set_account_void_id("41".into());
    let path = root.0.clone();
    let saved_key = key.clone();
    let worker = bind_fixture(&ui, move |_| (path.clone(), saved_key.clone())).unwrap();
    worker.opener()(&ui);
    ui.show().unwrap();
    ui.window().set_size(slint::PhysicalSize::new(850, 560));
    let phase = std::rc::Rc::new(Cell::new(0));
    let seen = phase.clone();
    let weak = ui.as_weak();
    let started = Instant::now();
    let timer = Timer::default();
    timer.start(TimerMode::Repeated, Duration::from_millis(200), move || {
        let ui = weak.upgrade().unwrap();
        assert!(
            started.elapsed() < Duration::from_secs(20),
            "Contacts phase {} timed out: {}",
            seen.get(),
            ui.get_contact_action_error()
        );
        match seen.get() {
            0 if ui.get_storage_ready() => {
                ui.set_section("Contacts".into());
                seen.set(1);
            }
            1 => {
                assert_eq!(ui.get_stored_contacts().row_count(), 2);
                assert_eq!(
                    ElementHandle::find_by_element_type_name(&ui, "ContactRow").count(),
                    2
                );
                snapshot(&ui, native, "01-contacts-small");
                click(&ui, "Contact options for Contact");
                seen.set(2);
            }
            2 => {
                assert!(
                    ui.get_selected_conversation().is_empty(),
                    "ellipsis must not open a chat"
                );
                assert_eq!(ui.get_section(), "Contacts");
                snapshot(&ui, native, "02-contact-menu");
                click(&ui, "Edit name");
                seen.set(3);
            }
            3 => {
                control(&ui, "Local contact name").set_accessible_value("Cancelled nickname");
                snapshot(&ui, native, "03-name-editor");
                click(&ui, "Cancel");
                seen.set(4);
            }
            4 => {
                assert!(
                    ui.get_stored_contacts()
                        .iter()
                        .any(|c| c.title == "Contact")
                );
                click(&ui, "Contact options for Contact");
                seen.set(5);
            }
            5 => {
                click(&ui, "Edit name");
                seen.set(6);
            }
            6 => {
                assert!(
                    control(&ui, "Local contact name")
                        .accessible_value()
                        .unwrap()
                        .is_empty(),
                    "Cancel must not save the nickname"
                );
                control(&ui, "Local contact name").set_accessible_value("Maya Chen");
                click(&ui, "Save");
                seen.set(7);
            }
            7 if !ui.get_contact_save_busy()
                && ui
                    .get_stored_contacts()
                    .iter()
                    .any(|c| c.title == "Maya Chen") =>
            {
                assert_eq!(ui.get_section(), "Contacts");
                assert!(ui.get_selected_conversation().is_empty());
                ui.window().set_size(slint::PhysicalSize::new(1600, 900));
                seen.set(8);
            }
            8 => {
                snapshot(&ui, native, "04-contacts-large");
                click(&ui, "Open chat with Maya Chen");
                seen.set(9);
            }
            9 if !ui.get_conversation_loading()
                && ui.get_selected_conversation() == chat.to_string() =>
            {
                assert_eq!(ui.get_section(), "Chats");
                assert_eq!(ui.get_conversation_title(), "Maya Chen");
                assert_eq!(ui.get_stored_draft(), "Unsent draft kept");
                assert_eq!(ui.get_stored_messages().row_count(), 2);
                ui.set_section("Contacts".into());
                seen.set(10);
            }
            10 => {
                click(&ui, "Contact options for Maya Chen");
                seen.set(11);
            }
            11 => {
                click(&ui, "Edit name");
                seen.set(12);
            }
            12 => {
                control(&ui, "Local contact name").set_accessible_value("Maya Local");
                click(&ui, "Save");
                seen.set(13);
            }
            13 if !ui.get_contact_save_busy() && ui.get_conversation_title() == "Maya Local" => {
                ui.set_section("Chats".into());
                ui.window().set_size(slint::PhysicalSize::new(1080, 680));
                seen.set(14);
            }
            14 => {
                assert_eq!(ui.get_stored_draft(), "Unsent draft kept");
                assert_eq!(ui.get_stored_messages().row_count(), 2);
                snapshot(&ui, native, "05-associated-chat");
                seen.set(15);
                ui.hide().unwrap();
                slint::quit_event_loop().unwrap();
            }
            _ => (),
        }
    });
    slint::run_event_loop().unwrap();
    timer.stop();
    worker.finish();
    assert_eq!(phase.get(), 15);
    let db = Database::open(&root.0, owner).unwrap();
    assert_eq!(db.contacts().unwrap().len(), 2);
    assert_eq!(
        db.contact("123").unwrap().unwrap().display_name.as_deref(),
        Some("Maya Local")
    );
    assert_eq!(
        db.conversation(chat).unwrap().display_name.as_deref(),
        Some("Maya Local")
    );
    assert_eq!(
        db.draft(chat).unwrap().as_deref(),
        Some("Unsent draft kept")
    );
    assert_eq!(db.conversation_timeline(chat).unwrap().len(), 2);
}
