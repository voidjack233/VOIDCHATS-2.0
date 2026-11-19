//! Exercise the production build policy using loopback sockets and synthetic keys.
use super::*;
use crate::relay::{ApplicationProtocolVersion as Version, SUPPORTED_APPLICATION_PROTOCOL};

#[test]
#[ignore = "requires explicitly provisioned disposable Go 9eb1319 and synthetic accounts"]
fn actual_go_minimum_enforcement_matches_the_release_profile() {
    use std::sync::mpsc;
    assert_eq!(std::env::var("VOID_TEST_DISPOSABLE").as_deref(), Ok("1"));
    assert_eq!(
        std::env::var("VOID_TEST_BACKEND_COMMIT").as_deref(),
        Ok("9eb131933a3c9e95cc8b5dba1bebae6400691419")
    );
    let endpoint = std::env::var("VOID_TEST_RELAY_URL").unwrap();
    let minimum = Version::parse(&std::env::var("VOID_TEST_MIN_PROTOCOL").unwrap()).unwrap();
    let who = || {
        Ok(SessionIdentity {
            void_id: "101".into(),
            key: SigningKey::from_bytes(&[121; 32]),
        })
    };
    for legacy in [true, false] {
        let (send, states) = mpsc::channel();
        let (events, activity) = mpsc::channel();
        let report = move |s| {
            let _ = send.send(s);
        };
        let report_activity = move |a| {
            let _ = events.send(a);
        };
        let version = if legacy {
            Version::V1
        } else {
            SUPPORTED_APPLICATION_PROTOCOL
        };
        let (relay, worker) = if legacy {
            Connection::spawn_with_test_protocol(
                endpoint.clone(),
                AUDIENCE.into(),
                version,
                move |_| who(),
                report,
                report_activity,
            )
        } else {
            Connection::spawn_for_release_test(
                endpoint.clone(),
                AUDIENCE.into(),
                move |_| who(),
                report,
                report_activity,
            )
        }
        .unwrap();
        assert!(relay.connect("101".into()));
        assert_eq!(
            states.recv_timeout(Duration::from_secs(15)).unwrap(),
            ConnectionStatus::Connecting
        );
        let next = states.recv_timeout(Duration::from_secs(15)).unwrap();
        if version < minimum {
            assert_eq!(next, ConnectionStatus::UpdateRequired(minimum));
            assert!(matches!(
                activity.recv_timeout(WAIT).unwrap(),
                Activity::Offline(_)
            ));
            assert!(!relay.connect("101".into()));
        } else {
            assert_eq!(next, ConnectionStatus::Connected);
            let Activity::Online(session) = activity.recv_timeout(WAIT).unwrap() else {
                panic!("missing session");
            };
            assert_eq!(
                session.feature_operations_supported(),
                version == Version::V2
            );
            let result = session
                .request(Request::Lookup {
                    void_id: "202".into(),
                })
                .unwrap();
            assert!(matches!(result,crate::relay::Reply::Person{void_id,..} if void_id=="202"));
            if version == Version::V1 {
                assert!(
                    matches!(session.request(Request::SendVersioned{event_id:"00000000-0000-4000-8000-000000000001".into(),to_void_id:"202".into(),payload_base64:"AA==".into(),min_protocol_version:Version::V2}),Err(RequestError::Rejected{code,..}) if code=="protocol_release_not_enabled")
                );
            }
        }
        worker.finish();
    }
}

async fn begin(socket: &mut Socket, version: Version) {
    verify_proof(socket, &identity().key.verifying_key()).await;
    send_json(socket, json!({"type":"auth_ok","void_id":VOID_ID})).await;
    assert_eq!(
        receive_json(socket).await,
        json!({"type":"protocol","version":version})
    );
}

#[test]
fn coordinated_build_is_explicit_and_defaults_remain_v1() {
    assert_eq!(
        SUPPORTED_APPLICATION_PROTOCOL,
        if cfg!(feature = "coordinated-v2-release") {
            Version::V2
        } else {
            Version::V1
        }
    );
    // Both backend minimums return this same response. There is deliberately no
    // parser field, capability probe or inferred deployment attestation.
    assert!(matches!(
        crate::relay::protocol::decode(r#"{"type":"protocol_ok","version":"2"}"#),
        Some(crate::relay::protocol::ServerFrame::ProtocolOk(Version::V2))
    ));
}

#[tokio::test]
async fn release_policy_is_negotiated_again_and_guards_the_transport() {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let endpoint = format!("ws://{}/ws", listener.local_addr().unwrap());
    let (send, mut activity) = tokio::sync::mpsc::unbounded_channel();
    let (relay, worker) = Connection::spawn_for_release_test(
        endpoint,
        AUDIENCE.into(),
        |_| Ok(identity()),
        |_| {},
        move |event| {
            let _ = send.send(event);
        },
    )
    .unwrap();
    assert!(relay.connect(VOID_ID.into()));
    let mut prior = None::<crate::relay::Session>;
    for _ in 0..2 {
        let mut socket = next_attempt(&listener).await;
        begin(&mut socket, SUPPORTED_APPLICATION_PROTOCOL).await;
        assert!(
            activity.try_recv().is_err(),
            "session escaped before protocol_ok"
        );
        send_json(
            &mut socket,
            json!({"type":"protocol_ok","version":SUPPORTED_APPLICATION_PROTOCOL}),
        )
        .await;
        let Some(Activity::Online(session)) = timeout(WAIT, activity.recv()).await.unwrap() else {
            panic!("missing session");
        };
        assert_eq!(
            session.feature_operations_supported(),
            cfg!(feature = "coordinated-v2-release")
        );
        if let Some(old) = &prior {
            assert_ne!(old.generation, session.generation);
            assert!(!old.feature_sends_allowed());
        }
        let own = session.clone();
        let attempt = tokio::task::spawn_blocking(move || {
            own.request(Request::SendVersioned {
                event_id: "00000000-0000-4000-8000-000000000001".into(),
                to_void_id: "456".into(),
                payload_base64: "AA==".into(),
                min_protocol_version: Version::V2,
            })
        });
        if cfg!(feature = "coordinated-v2-release") {
            let frame = receive_json(&mut socket).await;
            assert_eq!(frame["type"], "send_versioned");
            assert_eq!(frame["min_protocol_version"], "2");
            send_json(&mut socket,json!({"type":"accepted","event_id":frame["event_id"],"created_at":expiry(0),"expires_at":expiry(60)})).await;
            assert!(matches!(
                attempt.await.unwrap(),
                Ok(crate::relay::Reply::Accepted { .. })
            ));
        } else {
            assert!(
                matches!(attempt.await.unwrap(),Err(RequestError::Rejected{code,..}) if code=="protocol_release_not_enabled")
            );
            assert!(
                timeout(Duration::from_millis(100), socket.next())
                    .await
                    .is_err(),
                "uncoordinated build transmitted V2"
            );
        }
        // Ordinary messages retain the legacy transport even on the V2 profile.
        let own = session.clone();
        let ordinary = tokio::task::spawn_blocking(move || {
            own.request(Request::Send {
                event_id: "00000000-0000-4000-8000-000000000002".into(),
                to_void_id: "456".into(),
                payload_base64: "AA==".into(),
            })
        });
        let frame = receive_json(&mut socket).await;
        assert_eq!(frame["type"], "send");
        assert!(frame.get("min_protocol_version").is_none());
        send_json(&mut socket,json!({"type":"accepted","event_id":frame["event_id"],"created_at":expiry(0),"expires_at":expiry(60)})).await;
        assert!(ordinary.await.unwrap().is_ok());
        // The next connection must repeat auth + protocol negotiation.
        assert!(relay.connect(VOID_ID.into()));
        assert!(matches!(
            timeout(WAIT, activity.recv()).await.unwrap(),
            Some(Activity::Offline(_))
        ));
        prior = Some(session);
    }
    worker.finish();
}

#[tokio::test]
async fn minimum_above_the_release_stops_reconnect_and_never_exposes_messaging() {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let endpoint = format!("ws://{}/ws", listener.local_addr().unwrap());
    let (send, mut states) = tokio::sync::mpsc::unbounded_channel();
    let (events, mut activity) = tokio::sync::mpsc::unbounded_channel();
    let (relay, worker) = Connection::spawn_for_release_test(
        endpoint,
        AUDIENCE.into(),
        |_| Ok(identity()),
        move |s| {
            let _ = send.send(s);
        },
        move |a| {
            let _ = events.send(a);
        },
    )
    .unwrap();
    assert!(relay.connect(VOID_ID.into()));
    let mut socket = next_attempt(&listener).await;
    begin(&mut socket, SUPPORTED_APPLICATION_PROTOCOL).await;
    let required = Version::parse(if cfg!(feature = "coordinated-v2-release") {
        "3"
    } else {
        "2"
    })
    .unwrap();
    send_json(
        &mut socket,
        json!({"type":"error","code":"update_required","min_protocol_version":required}),
    )
    .await;
    assert_status(&mut states, ConnectionStatus::Connecting).await;
    assert_status(&mut states, ConnectionStatus::UpdateRequired(required)).await;
    assert!(matches!(
        timeout(WAIT, activity.recv()).await.unwrap(),
        Some(Activity::Offline(_))
    ));
    assert!(activity.recv().await.is_none());
    assert!(!relay.connect(VOID_ID.into()));
    assert!(
        timeout(Duration::from_millis(2300), listener.accept())
            .await
            .is_err()
    );
    worker.finish();
}

#[tokio::test]
async fn v2_success_does_not_reveal_whether_the_deployment_minimum_is_one_or_two() {
    for _simulated_minimum in [1, 2] {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let endpoint = format!("ws://{}/ws", listener.local_addr().unwrap());
        let (send, mut activity) = tokio::sync::mpsc::unbounded_channel();
        let (relay, worker) = Connection::spawn_with_test_protocol(
            endpoint,
            AUDIENCE.into(),
            Version::V2,
            |_| Ok(identity()),
            |_| {},
            move |a| {
                let _ = send.send(a);
            },
        )
        .unwrap();
        assert!(relay.connect(VOID_ID.into()));
        let mut socket = next_attempt(&listener).await;
        begin(&mut socket, Version::V2).await;
        send_json(&mut socket, json!({"type":"protocol_ok","version":"2"})).await;
        let Some(Activity::Online(session)) = timeout(WAIT, activity.recv()).await.unwrap() else {
            panic!("missing session");
        };
        assert!(session.feature_sends_allowed());
        worker.finish();
    }
}
