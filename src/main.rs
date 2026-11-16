mod account;
mod local_data;
mod messaging;
mod relay;
mod storage;
mod storage_ui;

#[cfg(test)]
mod relay_ui_tests;

use relay::{Connection, ConnectionStatus};

slint::include_modules!();

fn connect_relay(ui: &AppWindow, relay: &Connection) {
    let void_id = ui.get_account_void_id();
    if !matches!(ui.get_setup_page(), 0 | account::ENROLLMENT_PAGE)
        || !ui.get_storage_ready()
        || !account::valid_void_id(&void_id)
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

fn main() -> Result<(), slint::PlatformError> {
    let mut args = std::env::args_os().skip(1);
    if args
        .next()
        .is_some_and(|argument| argument == "--check-storage")
    {
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
    let local_storage = storage_ui::bind(&ui).ok();
    let open_storage = local_storage.as_ref().map(|worker| worker.opener());
    let activity = local_storage.as_ref().map(|worker| worker.relay_activity());
    let (relay, relay_worker) = match Connection::start(report_relay_status(&ui), move |event| {
        if let Some(report) = &activity {
            report(event);
        }
    }) {
        Ok((relay, worker)) => (Some(relay), Some(worker)),
        Err(_) => {
            ui.set_publication_error(
                "The relay worker could not start. Close and reopen VOID.".into(),
            );
            (None, None)
        }
    };
    if let Some(relay) = relay.clone() {
        bind_reconnect(&ui, relay);
    }
    if account::bind(&ui, move |ui| {
        if let Some(open) = &open_storage {
            open(ui);
        } else {
            ui.set_storage_loading(false);
            ui.set_storage_error("Local storage could not start. Close and reopen VOID.".into());
        }
    })
    .is_err()
    {
        ui.set_setup_busy(false);
        ui.set_setup_status("".into());
        ui.set_setup_can_retry(false);
        ui.set_setup_error("Could not start account setup. Close and reopen VOID.".into());
        ui.set_setup_page(5);
    }
    let result = ui.run();
    drop(ui);
    if let Some(worker) = relay_worker {
        worker.finish();
    }
    if let Some(worker) = local_storage {
        worker.finish();
    }
    result
}
