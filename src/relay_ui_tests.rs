use super::{AppWindow, Connection, ConnectionStatus, bind_reconnect, report_relay_status};
use crate::account::RegisteredIdentity;
use ed25519_dalek::SigningKey;
use futures_util::StreamExt;
use slint::ComponentHandle;
use std::{
    cell::Cell,
    net::TcpListener,
    rc::Rc,
    sync::{Arc, atomic::{AtomicBool, Ordering}},
    thread,
    time::Duration,
};

#[test]
fn slint_keeps_running_while_the_local_relay_withholds_its_challenge() {
    let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    listener.set_nonblocking(true).unwrap();
    let endpoint = format!("ws://{}/ws", listener.local_addr().unwrap());
    let begun = Arc::new(AtomicBool::new(false));
    let release = Arc::new(tokio::sync::Notify::new());
    let server = {
        let begun = begun.clone();
        let release = release.clone();
        thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
            runtime.block_on(async {
                tokio::time::timeout(Duration::from_secs(6), async {
                    let listener = tokio::net::TcpListener::from_std(listener).unwrap();
                    let (socket, _) = listener.accept().await.unwrap();
                    let mut relay = tokio_tungstenite::accept_async(socket).await.unwrap();
                    let begin = relay.next().await.unwrap().unwrap().into_text().unwrap();
                    assert_eq!(serde_json::from_str::<serde_json::Value>(&begin).unwrap(),
                        serde_json::json!({"type": "auth_begin", "void_id": "123"}));
                    begun.store(true, Ordering::Release);
                    // Do not answer until the UI reports work done during the stalled handshake.
                    release.notified().await;
                }).await.unwrap();
            });
        })
    };

    i_slint_backend_testing::init_integration_test_with_system_time();
    let ui = AppWindow::new().unwrap();
    ui.set_account_void_id("123".into());
    ui.set_setup_page(0);
    let main_thread = thread::current().id();
    let relay = Connection::spawn(endpoint, move |id| {
        assert_ne!(thread::current().id(), main_thread);
        Ok(RegisteredIdentity { void_id: id.to_owned(), key: SigningKey::from_bytes(&[7; 32]) })
    }, report_relay_status(&ui)).unwrap();
    bind_reconnect(&ui, relay);

    let observed = Rc::new(Cell::new(false));
    let timer = slint::Timer::default();
    let weak = ui.as_weak();
    let saw_responsive_ui = observed.clone();
    let release_server = release.clone();
    timer.start(slint::TimerMode::Repeated, Duration::from_millis(10), move || {
        if begun.load(Ordering::Acquire) && !saw_responsive_ui.get()
            && let Some(ui) = weak.upgrade()
        {
            assert_eq!(ui.get_connection_status(), ConnectionStatus::Connecting.label());
            ui.set_section("Settings".into());
            saw_responsive_ui.set(true);
            release_server.notify_one();
            slint::quit_event_loop().unwrap();
        }
    });
    // The watchdog exits the loop if the expected UI callback never ran.
    let watchdog = slint::Timer::default();
    watchdog.start(slint::TimerMode::SingleShot, Duration::from_secs(4), || {
        slint::quit_event_loop().unwrap();
    });
    ui.invoke_reconnect();
    slint::run_event_loop_until_quit().unwrap();
    assert!(observed.get());
    assert_eq!(ui.get_section(), "Settings");
    server.join().unwrap();
}
