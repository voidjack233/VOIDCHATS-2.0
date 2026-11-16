use super::{
    RegisteredIdentity,
    removal::{self, Deletion, Removal},
    storage::Store,
};
use crate::relay::deletion::{self, Error};
use std::io;

pub(super) fn confirm(
    store: &Store,
    expected_id: Option<&str>,
    progress: &dyn Fn(&'static str),
) -> Result<(), &'static str> {
    confirm_with(store, expected_id, progress, |identity, prepare| {
        deletion::delete(identity, prepare)
    })
}

pub(super) fn confirm_with(
    store: &Store,
    expected_id: Option<&str>,
    progress: &dyn Fn(&'static str),
    exchange: impl FnOnce(
        &RegisteredIdentity,
        &mut dyn FnMut() -> io::Result<()>,
    ) -> Result<String, Error>,
) -> Result<(), &'static str> {
    let _lock = removal::lock_account(store).map_err(|error| error.message())?;
    let journal = removal::read_journal(store).map_err(|error| error.message())?;
    if let Some(target) = &journal {
        if expected_id.is_some_and(|id| target.void_id != id) {
            return Err(removal::Error::UnverifiedAccount.message());
        }
        match target.deletion {
            Some(Deletion::Confirmed { .. }) => return Ok(()),
            Some(Deletion::Pending) => {}
            None => return Err("Finish removing the account from this device first."),
        }
    }

    let identity = store
        .load_for_removal()
        .map_err(|_| removal::Error::UnverifiedAccount.message())?
        .ok_or_else(|| removal::Error::UnverifiedAccount.message())?;
    let account = identity
        .state
        .registration()
        .ok_or_else(|| removal::Error::UnverifiedAccount.message())?;
    let mut target = Removal {
        void_id: account.void_id.clone(),
        public_key: identity.key.verifying_key().to_bytes(),
        deletion: Some(Deletion::Pending),
    };
    if expected_id.is_some_and(|id| target.void_id != id)
        || journal.as_ref().is_some_and(|saved| {
            saved.void_id != target.void_id || saved.public_key != target.public_key
        })
        || (expected_id.is_none() && journal.is_none())
    {
        return Err(removal::Error::UnverifiedAccount.message());
    }
    let identity = RegisteredIdentity {
        void_id: target.void_id.clone(),
        key: identity.key,
    };
    let mut pending = journal.is_some();
    progress(if pending {
        "Resuming account deletion…"
    } else {
        "Connecting to confirm account deletion…"
    });
    let deleted_at = exchange(&identity, &mut || {
        if !pending {
            removal::write_journal(store, &target)?;
            pending = true;
        }
        progress("Waiting for the relay to confirm deletion…");
        Ok(())
    })
    .map_err(|error| error.message(pending))?;
    target.deletion = Some(Deletion::Confirmed { deleted_at });
    removal::write_journal(store, &target).map_err(|_| {
        "The relay confirmed deletion, but VOID could not save that confirmation. Check this device’s account files, then retry."
    })
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use crate::{
        account::{API_BASE, Startup, open_enrollment_fixture, remove_permanently, resume_setup},
        relay::tests::{Socket, expiry, receive_json, send_json},
        storage::tests::TestDir,
    };
    use base64::{
        Engine,
        engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD},
    };
    use ed25519_dalek::{Signature, SigningKey, VerifyingKey};
    use futures_util::StreamExt;
    use serde_json::json;
    use std::{fs, os::windows::fs::OpenOptionsExt, sync::mpsc, time::Duration};
    use tokio::{net::TcpListener, time::timeout};
    use tokio_tungstenite::{accept_async, tungstenite::Message};

    const ID: &str = "789";
    const DELETED_AT: &str = "2026-11-16T00:00:12.345678Z";

    async fn accept_delete(listener: &TcpListener) -> Socket {
        let (tcp, _) = listener.accept().await.unwrap();
        let mut socket = accept_async(tcp).await.unwrap();
        assert_eq!(
            receive_json(&mut socket).await,
            json!({"type": "account_delete_begin", "void_id": ID})
        );
        socket
    }

    async fn challenge(
        socket: &mut Socket,
        key: &VerifyingKey,
        token: u8,
        seconds_until_expiry: i64,
    ) -> (String, String) {
        let expires_at = expiry(seconds_until_expiry);
        let id = URL_SAFE_NO_PAD.encode([token; 32]);
        let nonce = URL_SAFE_NO_PAD.encode([88; 32]);
        let message = format!(
            "VOID:delete-account:v1\n{id}\n{ID}\n{}\n{expires_at}\n{}\nDELETE\n{nonce}\n",
            STANDARD.encode(key.as_bytes()),
            STANDARD.encode(API_BASE),
        );
        send_json(
            socket,
            json!({"type": "account_delete_challenge", "challenge_id": id, "message": STANDARD.encode(&message), "expires_at": expires_at}),
        )
        .await;
        (id, message)
    }

    #[test]
    fn blocked_setup_deletes_only_after_explicit_confirmation_and_returns_to_welcome() {
        use crate::{
            AppWindow, bind_account_removal, storage::tests::relay_messaging::mock_relay,
            storage_ui::tests::in_test_process,
        };
        use slint::{ComponentHandle, Timer, TimerMode};
        use std::{cell::Cell, rc::Rc};

        if !in_test_process(
            "account::deletion::tests::blocked_setup_deletes_only_after_explicit_confirmation_and_returns_to_welcome",
        ) {
            return;
        }
        let dir = TestDir::new();
        let key = SigningKey::from_bytes(&[15; 32]);
        let (db, _) = open_enrollment_fixture(&dir.0, ID, &key).unwrap();
        db.close().unwrap();
        let public = key.verifying_key();
        let (allow_ack, wait_for_ui) = tokio::sync::oneshot::channel();
        let (endpoint, mock) = mock_relay(move |listener| async move {
            let mut socket = accept_delete(&listener).await;
            // The relay's clock is six seconds ahead of the client.
            let (id, message) = challenge(&mut socket, &public, 3, 36).await;
            let proof = receive_json(&mut socket).await;
            assert_eq!(proof["challenge_id"], id);
            assert_eq!(proof["confirmation"], "DELETE");
            let signature = Signature::from_slice(
                &STANDARD
                    .decode(proof["signature"].as_str().unwrap())
                    .unwrap(),
            )
            .unwrap();
            public
                .verify_strict(message.as_bytes(), &signature)
                .unwrap();
            wait_for_ui.await.unwrap();
            send_json(
                &mut socket,
                json!({"type": "account_deleted", "void_id": ID, "deleted_at": DELETED_AT}),
            )
            .await;
        });

        i_slint_backend_testing::init_integration_test_with_system_time();
        let ui = AppWindow::new().unwrap();
        let weak = ui.as_weak();
        ui.on_account_registered(move || {
            let ui = weak.upgrade().unwrap();
            let error = crate::messaging::Error::Relay(crate::relay::RequestError::Rejected {
                command: "prekeys_publish",
                code: "prekeys_conflict".into(),
            });
            ui.set_storage_loading(false);
            ui.set_storage_ready(true);
            ui.set_publication_error(crate::messaging::publication_failure(&error).0.into());
        });
        let root = dir.0.clone();
        let worker = super::super::bind_with(
            &ui,
            move || Ok(Store::from_test_path(root.clone())),
            move |store, expected, progress| {
                confirm_with(store, expected, progress, |identity, prepare| {
                    deletion::delete_at(&endpoint, identity, prepare)
                })
            },
        )
        .unwrap();
        bind_account_removal(&ui, Rc::default(), &worker);
        let record = dir.0.join("account.dpapi");
        let fixture_record = record.clone();
        let weak = ui.as_weak();
        let stage = Cell::new(0);
        let allow_ack = Cell::new(Some(allow_ack));
        let poll = Timer::default();
        poll.start(TimerMode::Repeated, Duration::from_millis(10), move || {
            let ui = weak.upgrade().unwrap();
            match stage.get() {
                0 if ui.get_can_manage_enrollment_account() => {
                    assert!(!ui.get_publication_error().is_empty());
                    ui.invoke_open_permanent_deletion();
                    ui.set_permanent_deletion_phrase("DELETE".into());
                    ui.invoke_cancel_permanent_deletion();
                    ui.invoke_request_account_removal(true);
                    assert!(!ui.get_removal_busy());
                    ui.invoke_open_permanent_deletion();
                    ui.set_permanent_deletion_phrase("DELETE ".into());
                    ui.invoke_request_account_removal(true);
                    assert!(!ui.get_removal_busy());
                    ui.set_permanent_deletion_phrase("DELETE".into());
                    assert!(ui.get_can_permanently_delete());
                    ui.invoke_request_account_removal(true);
                    assert!(ui.get_removal_busy());
                    stage.set(1);
                }
                1 if ui.get_removal_status().starts_with("Waiting for the relay") => {
                    assert!(fixture_record.exists());
                    assert!(!ui.get_can_permanently_delete());
                    ui.invoke_request_account_removal(true);
                    allow_ack.take().unwrap().send(()).unwrap();
                    stage.set(2);
                }
                2 if ui.get_setup_page() == 1 => slint::quit_event_loop().unwrap(),
                _ => {}
            }
        });
        let watchdog = Timer::default();
        watchdog.start(TimerMode::SingleShot, Duration::from_secs(8), || {
            slint::quit_event_loop().unwrap()
        });
        slint::run_event_loop_until_quit().unwrap();
        poll.stop();
        assert_eq!(ui.get_setup_page(), 1);
        assert!(ui.get_account_void_id().is_empty());
        assert!(!ui.get_removal_busy());
        assert!(!record.exists());
        assert!(!dir.0.join("messages").exists());
        mock.join().unwrap();
        worker.finish();
    }

    #[test]
    fn resumes_lost_acknowledgment_then_resumes_confirmed_cleanup_without_the_relay() {
        let dir = TestDir::new();
        let key = SigningKey::from_bytes(&[13; 32]);
        let (db, _) = open_enrollment_fixture(&dir.0, ID, &key).unwrap();
        let conversation = db.create_conversation("456", 100).unwrap();
        db.save_draft(conversation, "Fixture draft").unwrap();
        db.close().unwrap();
        let root = dir.0.clone();
        let public = key.verifying_key();
        let wrong_key = SigningKey::from_bytes(&[2; 32]).verifying_key();
        let (address, endpoint) = mpsc::channel();
        let relay = std::thread::spawn(move || {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap()
                .block_on(async {
                    timeout(Duration::from_secs(12), async {
                        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
                        address.send(format!("ws://{}/ws", listener.local_addr().unwrap())).unwrap();
                        let mut socket = accept_delete(&listener).await;
                        challenge(&mut socket, &wrong_key, 1, 20).await;
                        assert!(!matches!(socket.next().await, Some(Ok(Message::Text(_)))));
                        drop(socket);
                        for token in [2, 3] {
                            let mut socket = accept_delete(&listener).await;
                            let (id, message) = challenge(&mut socket, &public, token, 20).await;
                            let proof = receive_json(&mut socket).await;
                            let signature = proof["signature"].as_str().unwrap();
                            assert_eq!(
                                proof,
                                json!({"type": "account_delete_confirm", "challenge_id": id, "confirmation": "DELETE", "signature": signature})
                            );
                            let signature = Signature::from_slice(&STANDARD.decode(signature).unwrap()).unwrap();
                            public.verify_strict(message.as_bytes(), &signature).unwrap();
                            let store = Store::from_test_path(root.clone());
                            let journal = removal::read_journal(&store).unwrap().unwrap();
                            assert!(matches!(journal.deletion, Some(Deletion::Pending)));
                            assert_eq!(journal.public_key, public.to_bytes());
                            assert!(root.join("messages/messages.db").is_file());
                            if token == 3 {
                                send_json(&mut socket, json!({"type": "account_deleted", "void_id": ID, "deleted_at": DELETED_AT})).await;
                            }
                        }
                    }).await.unwrap();
                });
        });
        let endpoint = endpoint.recv_timeout(Duration::from_secs(3)).unwrap();
        let exchange = |store: &Store, expected: Option<&str>, progress: &dyn Fn(&'static str)| {
            confirm_with(store, expected, progress, |identity, prepare| {
                deletion::delete_at(&endpoint, identity, prepare)
            })
        };
        let store = Store::from_test_path(dir.0.clone());
        let account_path = dir.0.join("account.dpapi");
        let account_before = fs::read(&account_path).unwrap();
        assert!(matches!(
            remove_permanently(&store, Some(ID), &|_| {}, exchange).ok(),
            Some(Startup::RemovalIncomplete {
                can_go_back: true,
                permanent: true,
                ..
            })
        ));
        assert!(!removal::pending(&store).unwrap());
        assert!(store.load().unwrap().is_some());
        assert!(matches!(
            remove_permanently(&store, Some(ID), &|_| {}, exchange).ok(),
            Some(Startup::RemovalIncomplete {
                can_go_back: false,
                permanent: true,
                ..
            })
        ));
        assert!(store.load().is_err());
        assert!(matches!(
            removal::remove(&store, Some(ID)),
            Err(removal::Error::PendingDeletion)
        ));
        assert_eq!(fs::read(&account_path).unwrap(), account_before);

        // Simulate a restarted client receiving the idempotent acknowledgment,
        // then hitting a file lock before it can finish local cleanup.
        let held = fs::OpenOptions::new()
            .read(true)
            .share_mode(0x3)
            .open(&account_path)
            .unwrap();
        let reopened = Store::from_test_path(dir.0.clone());
        assert!(matches!(
            resume_setup(&reopened, &|_| {}, exchange).ok(),
            Some(Startup::RemovalIncomplete {
                can_go_back: false,
                permanent: true,
                ..
            })
        ));
        let journal = removal::read_journal(&reopened).unwrap().unwrap();
        assert!(
            matches!(journal.deletion, Some(Deletion::Confirmed { deleted_at }) if deleted_at == DELETED_AT)
        );
        relay.join().unwrap();
        assert!(!dir.0.join("messages").exists());
        assert!(account_path.exists());
        assert!(reopened.load().is_err());
        drop(held);

        let reopened = Store::from_test_path(dir.0.clone());
        let offline = |store: &Store, expected: Option<&str>, progress: &dyn Fn(&'static str)| {
            confirm_with(store, expected, progress, |_, _| {
                panic!("confirmed deletion contacted the relay")
            })
        };
        assert!(matches!(
            resume_setup(&reopened, &|_| {}, offline).ok(),
            Some(Startup::Removed)
        ));
        assert!(!account_path.exists());
        assert!(!removal::pending(&reopened).unwrap());
        assert!(matches!(
            resume_setup(&reopened, &|_| {}, offline).ok(),
            Some(Startup::Welcome)
        ));
    }
}
