mod account;

use slint::{ModelRc, SharedString, VecModel};
use std::{cell::RefCell, rc::Rc};

slint::include_modules!();

struct DemoChat {
    id: String,
    name: String,
    initials: String,
    time: String,
    unread: String,
    messages: Vec<Message>,
    draft: String,
    is_request: bool,
}

fn message(text: &str, time: &str, outgoing: bool) -> Message {
    Message { text: text.into(), time: time.into(), outgoing }
}

fn refresh(ui: &AppWindow, chats: &[DemoChat], selected: usize) {
    let query = ui.get_search().trim().to_lowercase();
    let section = ui.get_section();
    let contacts = section == "Contacts";
    let requests = section == "Requests";
    let rows: Vec<Conversation> = chats.iter().enumerate().filter(|(_, chat)| {
        chat.is_request == requests &&
        (!contacts || !chat.messages.is_empty()) &&
        (chat.name.to_lowercase().contains(&query) || chat.id.contains(&query) ||
            (!contacts && chat.messages.last().is_some_and(|m| m.text.to_lowercase().contains(&query))))
    }).map(|(index, chat)| Conversation {
        name: chat.name.clone().into(), initials: chat.initials.clone().into(),
        preview: if contacts { format!("VOID-ID {}", chat.id).into() } else { chat.messages.last().map(|m| m.text.clone()).unwrap_or_else(|| "Write a message".into()) },
        time: if contacts { "".into() } else { chat.time.clone().into() },
        unread: if contacts { "".into() } else { chat.unread.clone().into() }, index: index as i32,
    }).collect();
    ui.set_row_count(rows.len() as i32);
    ui.set_conversations(ModelRc::new(VecModel::from(rows)));
    ui.set_selected_index(selected as i32);
    let displayed = if requests {
        chats.get(ui.get_request_index() as usize).filter(|chat| chat.is_request)
    } else {
        chats.get(selected)
    };
    if let Some(chat) = displayed {
        ui.set_selected_name(chat.name.clone().into());
        ui.set_selected_initials(chat.initials.clone().into());
        ui.set_messages(ModelRc::new(VecModel::from(chat.messages.clone())));
        ui.set_draft(chat.draft.clone().into());
        ui.set_message_count(chat.messages.len() as i32);
    }
}

fn main() -> Result<(), slint::PlatformError> {
    let ui = AppWindow::new()?;
    if account::bind(&ui).is_err() {
        ui.set_setup_busy(false);
        ui.set_setup_status("".into());
        ui.set_setup_can_retry(false);
        ui.set_setup_error("Could not start account setup. Close and reopen VOID.".into());
        ui.set_setup_page(5);
    }
    let chats = Rc::new(RefCell::new(vec![
        DemoChat { id: "101".into(), name: "Maya Chen".into(), initials: "MC".into(), time: "10:42".into(), unread: "2".into(), draft: String::new(), is_request: false, messages: vec![
            message("Hey! Have you had a chance to see the new concept?", "10:38", false),
            message("Yes — the direction is looking really good.", "10:40", true),
            message("Glad you like it! Let's catch up later today.", "10:42", false),
        ]},
        DemoChat { id: "102".into(), name: "Alex Morgan".into(), initials: "AM".into(), time: "Yesterday".into(), unread: "".into(), draft: String::new(), is_request: false, messages: vec![
            message("Thanks for sharing that with me.", "Yesterday", false), message("Of course! Talk soon.", "Yesterday", true),
        ]},
        DemoChat { id: "103".into(), name: "Jordan Lee".into(), initials: "JL".into(), time: "Monday".into(), unread: "1".into(), draft: String::new(), is_request: false, messages: vec![message("See you around!", "Monday", false)] },
        DemoChat {
            id: "201".into(),
            name: "Sam Rivera".into(),
            initials: "SR".into(),
            time: "11:08".into(),
            unread: "1".into(),
            draft: String::new(),
            is_request: true,
            messages: vec![message("Hi! Would you like to join our weekend design meetup?", "11:08", false)],
        },
        DemoChat {
            id: "202".into(),
            name: "Priya Shah".into(),
            initials: "PS".into(),
            time: "Yesterday".into(),
            unread: "1".into(),
            draft: String::new(),
            is_request: true,
            messages: vec![message("Hello! Could I ask you about the community workshop?", "Yesterday", false)],
        },
    ]));
    let selected = Rc::new(RefCell::new(0usize));
    refresh(&ui, &chats.borrow(), 0);
    {
        let weak = ui.as_weak(); let chats = chats.clone(); let selected = selected.clone();
        ui.on_select_conversation(move |index| {
            if let Some(ui) = weak.upgrade() {
                let index = index as usize;
                let mut data = chats.borrow_mut();
                let Some(chat) = data.get_mut(index) else { return; };
                chat.unread.clear();
                if chat.is_request {
                    ui.set_request_index(index as i32);
                    refresh(&ui, &data, *selected.borrow());
                    return;
                }
                *selected.borrow_mut() = index;
                if ui.get_section() != "Chats" {
                    ui.set_section("Chats".into());
                    ui.set_search("".into());
                }
                refresh(&ui, &data, index);
            }
        });
    }
    {
        let weak = ui.as_weak(); let chats = chats.clone(); let selected = selected.clone();
        ui.on_change_section(move |section: SharedString| {
            if let Some(ui) = weak.upgrade() {
                ui.set_section(section); ui.set_search("".into()); ui.set_new_id("".into()); ui.set_id_error("".into());
                refresh(&ui, &chats.borrow(), *selected.borrow());
            }
        });
    }
    {
        let weak = ui.as_weak(); let chats = chats.clone(); let selected = selected.clone();
        ui.on_search_changed(move |_query: SharedString| {
            if let Some(ui) = weak.upgrade() { refresh(&ui, &chats.borrow(), *selected.borrow()); }
        });
    }
    {
        let weak = ui.as_weak(); let chats = chats.clone(); let selected = selected.clone();
        ui.on_open_new_chat(move |id| {
            let id = id.trim();
            if let Some(ui) = weak.upgrade() {
                if !account::valid_void_id(id) { ui.set_id_error("Enter a valid VOID-ID (1–19 digits).".into()); return; }
                let mut data = chats.borrow_mut();
                let index = match data.iter().position(|chat| chat.id == id) {
                    Some(index) => index,
                    None => {
                        data.push(DemoChat {
                            id: id.into(),
                            name: id.into(),
                            initials: "ID".into(),
                            time: String::new(),
                            unread: String::new(),
                            draft: String::new(),
                            is_request: false,
                            messages: vec![],
                        });
                        data.len() - 1
                    }
                };
                data[index].unread.clear();
                if data[index].is_request {
                    ui.set_section("Requests".into());
                    ui.set_request_index(index as i32);
                    ui.set_search("".into());
                    refresh(&ui, &data, *selected.borrow());
                    return;
                }
                *selected.borrow_mut() = index;
                ui.set_section("Chats".into()); ui.set_search("".into()); ui.set_new_id("".into()); ui.set_id_error("".into()); ui.set_draft("".into());
                refresh(&ui, &data, index);
            }
        });
    }
    {
        let chats = chats.clone(); let selected = selected.clone();
        ui.on_draft_changed(move |text| { chats.borrow_mut()[*selected.borrow()].draft = text.to_string(); });
    }
    {
        let weak = ui.as_weak(); let chats = chats.clone(); let selected = selected.clone();
        ui.on_send_message(move |text| {
            let trimmed = text.trim(); if trimmed.is_empty() { return; }
            let index = *selected.borrow();
            let mut data = chats.borrow_mut();
            data[index].messages.push(message(trimmed, "Just now", true)); data[index].draft.clear(); data[index].time = "Just now".into();
            if let Some(ui) = weak.upgrade() { refresh(&ui, &data, index); }
        });
    }
    ui.run()
}
