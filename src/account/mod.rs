mod protocol;
mod storage;

pub(crate) use protocol::valid_void_id;

use crate::AppWindow;
use base64::{Engine, engine::general_purpose::STANDARD};
use ed25519_dalek::Signer;
use protocol::{AccountApi, Failure, HttpAccountApi, Registration, Rejection};
use slint::ComponentHandle;
use std::{sync::mpsc, thread, time::{Duration, Instant}};
use storage::{Identity, State, Store};

const UNRESOLVED: &str = "VOID may have created your account, but setup did not receive or save a confirmed VOID-ID. Keep this Windows profile’s VOID data. Account recovery is not available yet.";
const KEY_REGISTERED: &str = "This device key is already registered. Keep this Windows profile’s VOID data. Account recovery is not available yet.";

struct SetupError {
    message: String,
    retry: bool,
    wait: Option<u64>,
}

impl SetupError {
    fn retry(message: &str) -> Self {
        Self { message: message.into(), retry: true, wait: None }
    }

    fn blocked(message: &str) -> Self {
        Self { message: message.into(), retry: false, wait: None }
    }
}

enum Command {
    CheckStorage,
    CreateAccount,
}

#[derive(Default)]
struct Setup {
    pending_save: Option<([u8; 32], Registration)>,
    retry_after: Option<(Instant, Duration)>,
}

enum Startup {
    Welcome,
    Registered(String),
    Blocked(&'static str),
}

fn inspect(store: &Store) -> Result<Startup, SetupError> {
    let identity = store.load().map_err(|_| SetupError::retry(
        "Could not open the saved identity. Check that this is the same Windows user, then retry opening local storage."
    ))?;
    Ok(match identity.map(|id| id.state) {
        None | Some(State::Prepared) => Startup::Welcome,
        Some(State::Registered(account)) => Startup::Registered(account.void_id),
        Some(State::SubmissionPending) => Startup::Blocked(UNRESOLVED),
        Some(State::AlreadyRegistered) => Startup::Blocked(KEY_REGISTERED),
    })
}

impl Setup {
    fn run<A: AccountApi>(
        &mut self,
        store: &Store,
        api: impl FnOnce() -> Result<A, Failure>,
        stage: impl Fn(&'static str),
    ) -> Result<Registration, SetupError> {
        if let Some((start, wait)) = self.retry_after {
            let remaining = wait.saturating_sub(start.elapsed());
            if !remaining.is_zero() {
                return Err(SetupError::retry(&format!(
                    "Please wait {} seconds before trying again.", remaining.as_secs().saturating_add(u64::from(remaining.subsec_nanos() > 0))
                )));
            }
            self.retry_after = None;
        }

        stage("Opening protected local storage…");
        let _lock = store.lock().map_err(|_| SetupError::retry(
            "Could not open local account storage for writing. Close other VOID desktop windows, then try again."
        ))?;
        let existing = store.load().map_err(|_| SetupError::retry(
            "Could not read the saved device identity. Check this Windows profile and try again."
        ))?;

        let mut identity = match existing {
            Some(identity) => identity,
            None if self.pending_save.is_some() => return Err(SetupError::blocked(
                "The original identity file is missing. Keep any existing VOID data. Account recovery is not available yet."
            )),
            None => {
                stage("Creating your protected device identity…");
                let identity = Identity::generate().map_err(|_| SetupError::retry(
                    "Could not prepare a secure device identity. Please try again."
                ))?;
                store.save(&identity).map_err(|_| SetupError::retry(
                    "Could not save the protected identity. Check that local storage is writable and try again."
                ))?;
                identity
            }
        };

        match &identity.state {
            State::Registered(account) => return Ok(account.clone()),
            State::AlreadyRegistered => return Err(SetupError::blocked(KEY_REGISTERED)),
            State::SubmissionPending => {
                return if self.pending_save.is_some() {
                    self.save_registration(store, identity, stage)
                } else {
                    Err(SetupError::blocked(UNRESOLVED))
                };
            }
            State::Prepared if self.pending_save.is_some() => return Err(SetupError::blocked(UNRESOLVED)),
            State::Prepared => {}
        }

        stage("Requesting a setup challenge from VOID…");
        let api = api().map_err(|error| self.failure(error, false))?;
        let public_key = STANDARD.encode(identity.key.verifying_key().as_bytes());
        let challenge = match api.challenge(&public_key) {
            Ok(challenge) => challenge,
            Err(error) => return Err(self.handle_rejection(store, &mut identity, error, false)),
        };
        if challenge.is_expired() {
            return Err(self.failure(Failure::ExpiredChallenge, false));
        }
        stage("Confirming your device identity…");
        let signature = identity.key.sign(&challenge.message);
        if challenge.is_expired() {
            return Err(self.failure(Failure::ExpiredChallenge, false));
        }
        let signature = STANDARD.encode(signature.to_bytes());

        // A crash, timeout or malformed reply after this point must never cause a second POST.
        identity.state = State::SubmissionPending;
        store.save(&identity).map_err(|_| SetupError::retry(
            "Could not save setup progress locally. No account request was sent. Check local storage and try again."
        ))?;
        if challenge.is_expired() {
            return Err(self.handle_rejection(store, &mut identity, Failure::ExpiredChallenge, true));
        }
        stage("Creating your VOID account…");
        match api.register(&challenge.id, &signature) {
            Ok(registration) => {
                self.pending_save = Some((identity.key.verifying_key().to_bytes(), registration));
                self.save_registration(store, identity, stage)
            }
            Err(error) => Err(self.handle_rejection(store, &mut identity, error, true)),
        }
    }

    fn save_registration(
        &mut self,
        store: &Store,
        mut identity: Identity,
        stage: impl Fn(&'static str),
    ) -> Result<Registration, SetupError> {
        let (_, account) = self.pending_save.as_ref()
            .filter(|(key, _)| key == identity.key.verifying_key().as_bytes())
            .ok_or_else(|| SetupError::blocked(UNRESOLVED))?;
        stage("Saving your VOID-ID on this device…");
        identity.state = State::Registered(account.clone());
        store.save(&identity).map_err(|_| SetupError::retry(
            "VOID created your account, but this device could not save the VOID-ID. Keep this window open and retry saving."
        ))?;
        self.pending_save = None;
        match identity.state {
            State::Registered(account) => Ok(account),
            _ => unreachable!("registration state was saved"),
        }
    }

    fn handle_rejection(
        &mut self,
        store: &Store,
        identity: &mut Identity,
        error: Failure,
        submitted: bool,
    ) -> SetupError {
        if matches!(error, Failure::Rejected(Rejection::KeyRegistered, _)) {
            identity.state = State::AlreadyRegistered;
            return if store.save(identity).is_ok() {
                SetupError::blocked(KEY_REGISTERED)
            } else {
                SetupError::blocked(
                    "VOID says this key is already registered, but local storage could not save that status. Keep this Windows profile’s VOID data."
                )
            };
        }
        if submitted {
            if !matches!(error, Failure::Rejected(_, _) | Failure::ExpiredChallenge) {
                return SetupError::blocked(UNRESOLVED);
            }
            identity.state = State::Prepared;
            if store.save(identity).is_err() {
                return SetupError::blocked(
                    "The account request was rejected, but setup could not save that result locally. Keep this Windows profile’s VOID data."
                );
            }
        }
        self.failure(error, submitted)
    }

    fn failure(&mut self, error: Failure, submitted: bool) -> SetupError {
        let (message, wait) = match error {
            Failure::Network => ("Could not reach VOID. Check your connection and try again. Your device identity is saved.", None),
            Failure::Timeout => ("VOID did not respond in time. Check your connection and try again. Your device identity is saved.", None),
            Failure::InvalidResponse => ("VOID returned an unexpected setup response. Your device identity is saved.", None),
            Failure::InvalidChallenge => ("VOID returned an invalid setup challenge. Try again to request a fresh one.", None),
            Failure::ExpiredChallenge =>
                ("The setup challenge expired or was rejected. Try again to request a fresh one.", None),
            Failure::Rejected(Rejection::InvalidProof, wait) =>
                ("The setup challenge expired or was rejected. Try again to request a fresh one.", wait),
            Failure::Rejected(Rejection::InvalidInput, wait) => (
                if submitted { "VOID rejected the account request. Try again with the saved identity." }
                else { "VOID rejected the challenge request. Try again with the saved identity." }, wait
            ),
            Failure::Rejected(Rejection::RateLimited, wait) => ("Too many setup attempts. Please wait before trying again.", wait),
            Failure::Rejected(Rejection::CapacityReached, wait) => ("VOID is handling too many setup requests. Please wait and try again.", wait),
            Failure::Rejected(Rejection::KeyRegistered, _) => return SetupError::blocked(KEY_REGISTERED),
            Failure::Unavailable => ("VOID is temporarily unavailable. Your device identity is saved. Try again later.", None),
        };
        if let Some(seconds) = wait {
            self.retry_after = Some((Instant::now(), Duration::from_secs(seconds)));
        }
        SetupError { message: message.into(), retry: true, wait }
    }
}

pub(crate) fn bind(ui: &AppWindow) -> std::io::Result<()> {
    let (send, receive) = mpsc::channel();
    let weak = ui.as_weak();
    thread::Builder::new().name("void-account".into()).spawn(move || {
        let mut setup = Setup::default();
        let mut command = Command::CheckStorage;
        loop {
            let checking = matches!(command, Command::CheckStorage);
            let result = Store::for_user().map_err(|_| SetupError::retry(
                "The Windows local app data directory is unavailable. Retry opening local storage."
            )).and_then(|store| match command {
                Command::CheckStorage => inspect(&store),
                Command::CreateAccount => setup.run(&store, HttpAccountApi::new, |stage| {
                    let _ = weak.upgrade_in_event_loop(move |ui| ui.set_setup_status(stage.into()));
                }).map(|account| Startup::Registered(account.void_id)),
            });
            let save_only = setup.pending_save.is_some();
            let _ = weak.upgrade_in_event_loop(move |ui| finish(&ui, checking, save_only, result));
            match receive.recv() {
                Ok(next) => command = next,
                Err(_) => break,
            }
        }
    })?;

    let weak = ui.as_weak();
    ui.on_submit_setup(move || {
        let Some(ui) = weak.upgrade() else { return; };
        if ui.get_setup_busy() || !ui.get_setup_can_retry() || ui.get_setup_page() != 5 {
            return;
        }
        let command = if ui.get_setup_retry_storage() {
            Command::CheckStorage
        } else {
            Command::CreateAccount
        };
        ui.set_setup_busy(true);
        ui.set_setup_error("".into());
        ui.set_setup_status("Opening protected local storage…".into());
        if send.send(command).is_err() {
            finish(&ui, false, false, Err(SetupError::blocked("Account setup stopped. Close and reopen VOID.")));
        }
    });
    Ok(())
}

fn finish(ui: &AppWindow, checking: bool, save_only: bool, result: Result<Startup, SetupError>) {
    ui.set_setup_busy(false);
    ui.set_setup_status("".into());
    ui.set_setup_retry_storage(checking && result.is_err());
    ui.set_setup_can_retry(true);
    ui.set_setup_action("Create account".into());
    match result {
        Ok(Startup::Welcome) => {
            ui.set_setup_error("".into());
            ui.set_setup_page(1);
        }
        Ok(Startup::Registered(id)) => {
            ui.set_account_void_id(id.into());
            ui.set_setup_page(0);
        }
        Ok(Startup::Blocked(message)) => {
            ui.set_setup_error(message.into());
            ui.set_setup_can_retry(false);
            ui.set_setup_page(5);
        }
        Err(error) => {
            let message = match error.wait {
                Some(seconds) => format!("{} Try again in {seconds} seconds.", error.message),
                None => error.message,
            };
            ui.set_setup_error(message.into());
            ui.set_setup_can_retry(error.retry);
            ui.set_setup_action(if checking { "Retry opening local storage" }
                else if save_only { "Retry saving VOID-ID" } else { "Retry account setup" }.into());
            ui.set_setup_page(5);
        }
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use ed25519_dalek::{Signature, VerifyingKey};
    use protocol::Challenge;
    use std::{cell::Cell, fs::{File, OpenOptions}, os::windows::fs::OpenOptionsExt};
    use storage::tests::TestDir;

    struct LocalApi<'a> {
        store: &'a Store,
        expected_key: [u8; 32],
        submitted: &'a Cell<usize>,
        answer: Cell<Option<Result<Registration, Failure>>>,
        hold_record: Option<(&'a std::path::Path, &'a Cell<Option<File>>)>,
    }

    impl AccountApi for LocalApi<'_> {
        fn challenge(&self, public_key: &str) -> Result<Challenge, Failure> {
            assert_eq!(public_key, STANDARD.encode(self.expected_key));
            Ok(Challenge {
                id: "test-challenge".into(),
                message: b"validated test challenge".to_vec(),
                expires_at_ms: i64::MAX,
            })
        }

        fn register(&self, challenge_id: &str, signature: &str) -> Result<Registration, Failure> {
            assert_eq!(challenge_id, "test-challenge");
            assert!(matches!(self.store.load().unwrap().unwrap().state, State::SubmissionPending));
            VerifyingKey::from_bytes(&self.expected_key).unwrap().verify_strict(
                b"validated test challenge",
                &Signature::from_slice(&STANDARD.decode(signature).unwrap()).unwrap(),
            ).unwrap();
            self.submitted.set(self.submitted.get() + 1);
            if let Some((path, held)) = self.hold_record {
                let handle = OpenOptions::new().read(true).share_mode(0x3).open(path).unwrap();
                held.set(Some(handle));
            }
            self.answer.take().unwrap()
        }
    }

    #[test]
    fn explicit_rejection_reuses_key_and_lost_reply_stays_blocked_after_reopen() {
        let dir = TestDir::new();
        let store = dir.store();
        let lock = store.lock().unwrap();
        let identity = Identity::generate().unwrap();
        let expected_key = identity.key.verifying_key().to_bytes();
        store.save(&identity).unwrap();
        drop(lock);

        let submitted = Cell::new(0);
        let api = |answer| Ok(LocalApi {
            store: &store, expected_key, submitted: &submitted, answer: Cell::new(Some(answer)),
            hold_record: None,
        });
        let mut setup = Setup::default();
        let rejected = setup.run(&store, || api(Err(Failure::Rejected(Rejection::InvalidProof, None))), |_| {})
            .err().unwrap();
        assert!(rejected.retry);
        assert!(matches!(store.load().unwrap().unwrap().state, State::Prepared));
        let lost = setup.run(&store, || api(Err(Failure::Timeout)), |_| {}).err().unwrap();
        assert!(!lost.retry);
        assert_eq!(submitted.get(), 2);

        let reopened = dir.store();
        let saved = reopened.load().unwrap().unwrap();
        assert_eq!(saved.key.verifying_key().to_bytes(), expected_key);
        assert!(matches!(inspect(&reopened).ok().unwrap(), Startup::Blocked(UNRESOLVED)));
        let retry = Setup::default()
            .run::<LocalApi<'_>>(&reopened, || panic!("a pending registration must not reach the API"), |_| {})
            .err().unwrap();
        assert!(!retry.retry);
    }

    #[test]
    fn saving_a_confirmed_account_can_retry_without_another_submission() {
        let dir = TestDir::new();
        let store = dir.store();
        let lock = store.lock().unwrap();
        let identity = Identity::generate().unwrap();
        let expected_key = identity.key.verifying_key().to_bytes();
        store.save(&identity).unwrap();
        drop(lock);

        let submitted = Cell::new(0);
        let held = Cell::new(None);
        let record_path = dir.record_path();
        let mut setup = Setup::default();
        let error = setup.run(&store, || Ok(LocalApi {
            store: &store,
            expected_key,
            submitted: &submitted,
            answer: Cell::new(Some(Ok(Registration {
                void_id: "300".into(),
                created_at: "2026-11-14T00:00:00Z".into(),
            }))),
            hold_record: Some((&record_path, &held)),
        }), |_| {}).err().unwrap();
        assert!(error.retry);
        assert!(setup.pending_save.is_some());
        drop(held.take());

        let saved = setup.run::<LocalApi<'_>>(
            &store, || panic!("saving a confirmed registration must not reach the API"), |_| {},
        ).ok().unwrap();
        assert_eq!(submitted.get(), 1);
        assert_eq!(saved.void_id, "300");
        assert!(matches!(inspect(&store).ok().unwrap(), Startup::Registered(id) if id == "300"));
    }
}
