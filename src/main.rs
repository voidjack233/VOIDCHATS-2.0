mod account;
mod local_data;
mod relay;
mod storage;
mod storage_ui;

#[cfg(test)]
mod relay_ui_tests;

use relay::{Connection, ConnectionStatus};

slint::include_modules!();

fn connect_relay(ui: &AppWindow, relay: &Connection) {
    let void_id = ui.get_account_void_id();
    if ui.get_setup_page() != 0 || void_id.is_empty() {
        return;
    }
    let status = ui.get_connection_status();
    if status == ConnectionStatus::Connecting.label() || status == ConnectionStatus::Connected.label() {
        return;
    }
    if relay.connect(void_id.to_string()) {
        ui.set_connection_status(ConnectionStatus::Connecting.label().into());
    }
}

fn bind_reconnect(ui: &AppWindow, relay: Connection) {
    let weak = ui.as_weak();
    ui.on_reconnect(move || {
        if let Some(ui) = weak.upgrade() {
            connect_relay(&ui, &relay);
        }
    });
}

fn report_relay_status(ui: &AppWindow) -> impl Fn(ConnectionStatus) + Send + 'static {
    let weak = ui.as_weak();
    move |status| {
        let _ = weak.upgrade_in_event_loop(move |ui| ui.set_connection_status(status.label().into()));
    }
}

fn main() -> Result<(), slint::PlatformError> {
    let ui = AppWindow::new()?;
    let local_storage = storage_ui::bind(&ui).ok();
    let open_storage = local_storage.as_ref().map(|worker| worker.opener());
    let relay = Connection::start(report_relay_status(&ui)).ok();
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
        if let Some(relay) = &relay {
            connect_relay(ui, relay);
        }
    }).is_err() {
        ui.set_setup_busy(false);
        ui.set_setup_status("".into());
        ui.set_setup_can_retry(false);
        ui.set_setup_error("Could not start account setup. Close and reopen VOID.".into());
        ui.set_setup_page(5);
    }
    let result = ui.run();
    drop(ui);
    if let Some(worker) = local_storage {
        worker.finish();
    }
    result
}
