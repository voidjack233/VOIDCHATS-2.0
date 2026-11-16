#![cfg_attr(all(windows, not(test)), windows_subsystem = "windows")]

mod account;
mod local_data;
mod messaging;
mod relay;
mod storage;
mod storage_ui;

#[cfg(test)]
mod relay_ui_tests;

use relay::{Connection, ConnectionStatus};
use std::{cell::RefCell, rc::Rc};

slint::include_modules!();

fn connect_relay(ui: &AppWindow, relay: &Connection) {
    let void_id = ui.get_account_void_id();
    if !matches!(ui.get_setup_page(), 0 | account::ENROLLMENT_PAGE)
        || !ui.get_storage_ready()
        || !account::valid_void_id(&void_id)
        || ui.get_removal_busy()
        || !ui.get_removal_error().is_empty()
    {
        return;
    }
    let status = ui.get_connection_status();
    if status == ConnectionStatus::Connecting.label()
        || status == ConnectionStatus::Connected.label()
    {
        return;
    }
    if relay.connect(void_id.to_string()) {
        ui.set_connection_status(ConnectionStatus::Connecting.label().into());
    }
}

struct MessagingWorkers {
    relay: Option<relay::Worker>,
    storage: Option<storage_ui::Worker>,
}

impl MessagingWorkers {
    fn start(ui: &AppWindow) -> Self {
        let storage = storage_ui::bind(ui).ok();
        let activity = storage.as_ref().map(|worker| worker.relay_activity());
        let relay = match Connection::start(report_relay_status(ui), move |event| {
            if let Some(report) = &activity {
                report(event);
            }
        }) {
            Ok((relay, worker)) => {
                bind_reconnect(ui, relay);
                Some(worker)
            }
            Err(_) => {
                ui.set_publication_error(
                    "The relay worker could not start. Close and reopen VOID.".into(),
                );
                None
            }
        };
        if let Some(storage) = &storage {
            storage.opener()(ui);
        } else {
            ui.set_storage_retryable(false);
            ui.set_storage_loading(false);
            ui.set_storage_error("Local storage could not start. Close and reopen VOID.".into());
        }
        Self { relay, storage }
    }

    fn finish(self) {
        if let Some(worker) = self.relay {
            worker.finish();
        }
        if let Some(worker) = self.storage {
            worker.finish();
        }
    }
}

fn bind_reconnect(ui: &AppWindow, relay: Connection) {
    let weak = ui.as_weak();
    let reconnect = relay.clone();
    ui.on_reconnect(move || {
        if let Some(ui) = weak.upgrade() {
            connect_relay(&ui, &reconnect);
        }
    });
    let weak = ui.as_weak();
    ui.on_storage_opened(move || {
        if let Some(ui) = weak.upgrade() {
            connect_relay(&ui, &relay);
        }
    });
}

fn report_relay_status(ui: &AppWindow) -> impl Fn(ConnectionStatus) + Send + 'static {
    let weak = ui.as_weak();
    move |status| {
        let _ = weak.upgrade_in_event_loop(move |ui| {
            if status != ConnectionStatus::Connected {
                ui.set_messaging_ready(false);
            }
            ui.set_connection_status(status.label().into());
        });
    }
}

fn bind_account_removal(
    ui: &AppWindow,
    messaging: Rc<RefCell<Option<MessagingWorkers>>>,
    worker: &account::Worker,
) {
    ui.set_account_management_ready(true);
    let remove = worker.remover();
    let runtime = messaging;
    let weak = ui.as_weak();
    ui.on_request_account_removal(move |permanent| {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        if ui.get_removal_busy() || !ui.get_account_management_ready() {
            return;
        }
        let continuing = !ui.get_removal_error().is_empty();
        let id = ui.get_account_void_id();
        let expected_id = account::valid_void_id(&id).then(|| id.to_string());
        let confirmed = if permanent {
            ui.get_can_permanently_delete()
        } else {
            ui.get_local_removal_confirmed()
        };
        if (continuing && permanent != ui.get_removal_permanent())
            || (!continuing && (!confirmed || expected_id.is_none()))
        {
            return;
        }
        ui.set_removal_permanent(permanent);
        ui.set_removal_status(
            if permanent {
                "Preparing account deletion…"
            } else {
                "Removing this device’s VOID data…"
            }
            .into(),
        );
        ui.set_removal_busy(true);
        ui.set_removal_error("".into());
        ui.set_removal_can_go_back(false);
        ui.invoke_cancel_local_removal();
        ui.invoke_cancel_permanent_deletion();
        let messaging = runtime.borrow_mut().take();
        if !remove(
            expected_id,
            permanent,
            Box::new(move || {
                if let Some(workers) = messaging {
                    workers.finish();
                }
            }),
        ) {
            ui.set_removal_busy(false);
            ui.set_removal_error("Local account management stopped. Close and reopen VOID.".into());
        }
    });
    let weak = ui.as_weak();
    ui.on_leave_failed_removal(move || {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        if !ui.get_removal_can_go_back() || ui.get_removal_busy() {
            return;
        }
        ui.set_removal_can_go_back(false);
        ui.set_removal_error("".into());
        ui.set_removal_permanent(false);
        ui.set_storage_ready(false);
        ui.set_messaging_ready(false);
        ui.set_publishing_keys(false);
        ui.set_publication_error("".into());
        ui.set_publication_retryable(false);
        ui.set_connection_status(ConnectionStatus::Disconnected.label().into());
        ui.invoke_account_registered();
    });
}

fn main() -> Result<(), slint::PlatformError> {
    let mut args = std::env::args_os().skip(1);
    if args
        .next()
        .is_some_and(|argument| argument == "--check-storage")
    {
        #[cfg(windows)]
        {
            use windows_sys::Win32::System::Console::{ATTACH_PARENT_PROCESS, AttachConsole};
            // Only the command-line diagnostic needs the parent's console.
            unsafe { AttachConsole(ATTACH_PARENT_PROCESS) };
        }
        let Some(void_id) = args
            .next()
            .and_then(|id| id.into_string().ok())
            .filter(|id| account::valid_void_id(id) && args.next().is_none())
        else {
            eprintln!("Usage: voidchats-native --check-storage <VOID-ID>");
            std::process::exit(2);
        };
        let report = account::load_registered_identity(&void_id)
            .map_err(|_| storage::Error::WrongAccount)
            .and_then(|identity| {
                storage::check_existing(
                    &local_data::user_data_dir()?,
                    storage::Owner {
                        void_id: &identity.void_id,
                        public_key: &identity.key.verifying_key().to_bytes(),
                    },
                )
            });
        match report {
            Ok(report) => {
                for (name, passed) in [
                    ("SQLCipher", report.cipher),
                    ("SQLite", report.sqlite),
                    ("Foreign keys", report.foreign_keys),
                ] {
                    println!("{name}: {}", if passed { "passed" } else { "did not pass" });
                }
                if !(report.cipher && report.sqlite && report.foreign_keys) {
                    std::process::exit(1);
                }
            }
            Err(error) => {
                eprintln!("{}", error.open_message());
                std::process::exit(1);
            }
        }
        return Ok(());
    }
    let ui = AppWindow::new()?;
    let messaging = Rc::new(RefCell::new(None::<MessagingWorkers>));
    let runtime = messaging.clone();
    let weak = ui.as_weak();
    ui.on_account_registered(move || {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        let mut runtime = runtime.borrow_mut();
        if runtime.is_none() {
            *runtime = Some(MessagingWorkers::start(&ui));
        }
    });
    let account = match account::bind(&ui) {
        Ok(worker) => {
            bind_account_removal(&ui, messaging.clone(), &worker);
            Some(worker)
        }
        Err(_) => {
            ui.set_setup_busy(false);
            ui.set_setup_status("".into());
            ui.set_setup_can_retry(false);
            ui.set_setup_error("Could not start account setup. Close and reopen VOID.".into());
            ui.set_setup_page(5);
            None
        }
    };
    #[cfg(windows)]
    let focus_timer = {
        use slint::ComponentHandle;
        use windows_sys::Win32::UI::Input::KeyboardAndMouse::GetActiveWindow;

        let timer = slint::Timer::default();
        let weak = ui.as_weak();
        // The Slint UI thread owns only this window. GetActiveWindow only
        // checks the calling thread, including when another VOID is in front.
        timer.start(
            slint::TimerMode::Repeated,
            std::time::Duration::from_millis(150),
            move || {
                if let Some(ui) = weak.upgrade() {
                    ui.set_window_foreground(!unsafe { GetActiveWindow() }.is_null());
                }
            },
        );
        timer
    };
    let calendar_timer = storage_ui::track_local_calendar(&ui);
    let result = ui.run();
    calendar_timer.stop();
    #[cfg(windows)]
    focus_timer.stop();
    drop(ui);
    if let Some(workers) = messaging.borrow_mut().take() {
        workers.finish();
    }
    if let Some(worker) = account {
        worker.finish();
    }
    result
}
