mod protocol;
mod storage;

pub(crate) use protocol::{API_BASE, timestamp_ms, valid_token, valid_void_id};

use crate::AppWindow;
use base64::{Engine, engine::general_purpose::STANDARD};
use ed25519_dalek::Signer;
use protocol::{AccountApi, Failure, HttpAccountApi, Registration, Rejection, valid_registration};
use slint::ComponentHandle;
use std::{sync::mpsc, thread, time::{Duration, Instant}};
use storage::{Identity, State, Store};

const UNRESOLVED: &str = "VOID may have created your account, but setup did not receive or save a confirmed VOID-ID. Keep this Windows profile’s VOID data. Account recovery is not available yet.";
const KEY_REGISTERED: &str = "This device key is already registered. Keep this Windows profile’s VOID data. Account recovery is not available yet.";
const INVALID_CONFIRMATION: &str = "The saved registration confirmation could not be verified against this device’s identity and setup request. Keep this Windows profile’s VOID data.";
const STORAGE_LOCK: &str = "Could not open local account storage for writing. Close other VOID windows, then try again.";

struct SetupError {
    message: String,
    retry: bool,
    wait: Option<u64>,
    saving: bool,
}

impl SetupError {
    fn retry(message: &str) -> Self {
        Self { message: message.into(), retry: true, wait: None, saving: false }
    }

    fn blocked(message: &str) -> Self {
        Self { message: message.into(), retry: false, wait: None, saving: false }
    }

    fn retry_saving(message: &str) -> Self {
        Self { saving: true, ..Self::retry(message) }
    }
}

enum Command {
    CheckStorage,
    CreateAccount,
}

#[derive(Default)]
struct Setup {
    pending_save: Option<PendingSave>,
    retry_after: Option<(Instant, Duration)>,
}

struct PendingSave {
    public_key: [u8; 32],
    challenge_id: String,
    account: Registration,
}

enum Startup {
    Welcome,
    Registered(String),
    Blocked(&'static str),
}

fn inspect(store: &Store) -> Result<Startup, SetupError> {
    let load = || store.load().map_err(|_| SetupError::retry(
        "Could not open the saved identity. Check that this is the same Windows user, then retry opening local storage."
    ));
    let mut identity = load()?;
    let mut lock = None;
    if identity.as_ref().is_some_and(|id| id.state.submission_pending()) {
        // Another process may still be waiting for the server. Re-read once we own the lock.
        lock = Some(store.lock().map_err(|_| SetupError::retry(STORAGE_LOCK))?);
        identity = load()?;
    }
    let result = match identity {
        Some(identity) if identity.state.submission_pending() => {
            match load_confirmation(store, &identity)? {
                Some(account) => finalize_registration(store, identity, account)
                    .map(|account| Startup::Registered(account.void_id)),
                None => Ok(Startup::Blocked(UNRESOLVED)),
            }
        }
        Some(Identity { state: State::Registered(account), .. }) => Ok(Startup::Registered(account.void_id)),
        Some(Identity { state: State::AlreadyRegistered, .. }) => Ok(Startup::Blocked(KEY_REGISTERED)),
        _ => {
            reject_unexpected_confirmation(store)?;
            Ok(Startup::Welcome)
        }
    };
    drop(lock);
    result
}

fn reject_unexpected_confirmation(store: &Store) -> Result<(), SetupError> {
    match store.has_confirmation() {
        Ok(false) => Ok(()),
        Ok(true) => Err(SetupError::blocked(INVALID_CONFIRMATION)),
        Err(_) => Err(SetupError::retry(
            "Could not check for a saved registration confirmation. Check local storage and try again."
        )),
    }
}

fn load_confirmation(store: &Store, identity: &Identity) -> Result<Option<Registration>, SetupError> {
    store.load_confirmation(identity).map_err(|error| {
        if error.kind() == std::io::ErrorKind::InvalidData {
            SetupError::blocked(INVALID_CONFIRMATION)
        } else {
            SetupError::retry_saving(
                "Could not read the protected registration confirmation. Check local storage and try again."
            )
        }
    })
}

fn finalize_registration(
    store: &Store,
    mut identity: Identity,
    account: Registration,
) -> Result<Registration, SetupError> {
    identity.state = State::Registered(account.clone());
    store.save(&identity).map_err(|_| SetupError::retry_saving(
        "Your registration confirmation is saved, but VOID could not finish saving your account. Try again now or reopen VOID to finish."
    ))?;
    store.clear_confirmation();
    Ok(account)
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
        let _lock = store.lock().map_err(|_| SetupError::retry(STORAGE_LOCK))?;
        let existing = store.load().map_err(|_| SetupError::retry(
            "Could not read the saved device identity. Check this Windows profile and try again."
        ))?;

        let mut identity = match existing {
            Some(identity) => identity,
            None if self.pending_save.is_some() => return Err(SetupError::blocked(
                "The original identity file is missing. Keep any existing VOID data. Account recovery is not available yet."
            )),
            None => {
                reject_unexpected_confirmation(store)?;
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
            State::SubmissionPending | State::SubmissionPendingFor { .. } => {
                return self.save_registration(store, identity, stage);
            }
            State::Prepared if self.pending_save.is_some() => return Err(SetupError::blocked(UNRESOLVED)),
            State::Prepared => reject_unexpected_confirmation(store)?,
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
        identity.state = State::SubmissionPendingFor { challenge_id: challenge.id.clone() };
        store.save(&identity).map_err(|_| SetupError::retry(
            "Could not save setup progress locally. No account request was sent. Check local storage and try again."
        ))?;
        if challenge.is_expired() {
            return Err(self.handle_rejection(store, &mut identity, Failure::ExpiredChallenge, true));
        }
        stage("Creating your VOID account…");
        match api.register(&challenge.id, &signature) {
            Ok(account) if valid_registration(&account) => {
                self.pending_save = Some(PendingSave {
                    public_key: identity.key.verifying_key().to_bytes(),
                    challenge_id: challenge.id,
                    account,
                });
                self.save_registration(store, identity, stage)
            }
            Ok(_) => Err(SetupError::blocked(UNRESOLVED)),
            Err(error) => Err(self.handle_rejection(store, &mut identity, error, true)),
        }
    }

    fn save_registration(
        &mut self,
        store: &Store,
        identity: Identity,
        stage: impl Fn(&'static str),
    ) -> Result<Registration, SetupError> {
        let account = match load_confirmation(store, &identity)? {
            Some(account) => account,
            None => {
                let pending = self.pending_save.as_ref().filter(|pending|
                    pending.public_key == identity.key.verifying_key().to_bytes()
                        && Some(pending.challenge_id.as_str()) == identity.state.challenge_id()
                ).ok_or_else(|| SetupError::blocked(UNRESOLVED))?;

                stage("Saving your VOID-ID on this device…");
                store.save_confirmation(&identity, &pending.account).map_err(|_| SetupError::retry_saving(
                    "VOID confirmed your account, but its confirmation could not be saved locally. Keep this window open and retry saving."
                ))?;
                pending.account.clone()
            }
        };
        stage("Finishing account setup on this device…");
        let account = finalize_registration(store, identity, account)?;
        self.pending_save = None;
        Ok(account)
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
        SetupError { message: message.into(), retry: true, wait, saving: false }
    }
}

pub(crate) struct RegisteredIdentity {
    pub(crate) void_id: String,
    pub(crate) key: ed25519_dalek::SigningKey,
}

pub(crate) fn load_registered_identity(expected_id: &str) -> std::io::Result<RegisteredIdentity> {
    let identity = Store::for_user()?.load()?;
    match identity {
        Some(Identity { key, state: State::Registered(account) }) if account.void_id == expected_id => {
            Ok(RegisteredIdentity { void_id: account.void_id, key })
        }
        _ => Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "Registered identity unavailable")),
    }
}

pub(crate) fn bind(
    ui: &AppWindow,
    on_registered: impl Fn(&AppWindow) + Clone + Send + 'static,
) -> std::io::Result<()> {
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
            let on_registered = on_registered.clone();
            let _ = weak.upgrade_in_event_loop(move |ui| {
                let registered = matches!(result, Ok(Startup::Registered(_)));
                finish(&ui, checking, save_only, result);
                if registered {
                    on_registered(&ui);
                }
            });
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
            ui.set_setup_action(if error.saving || save_only { "Retry saving VOID-ID" }
                else if checking { "Retry opening local storage" } else { "Retry account setup" }.into());
            ui.set_setup_page(5);
        }
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use ed25519_dalek::{Signature, VerifyingKey};
    use protocol::{Challenge, valid_token};
    use std::{cell::Cell, fs::{self, File, OpenOptions}, os::windows::fs::OpenOptionsExt, process::Command};
    use storage::tests::TestDir;

    const CHALLENGE_ID: &str = "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";

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
                id: CHALLENGE_ID.into(),
                message: b"validated test challenge".to_vec(),
                expires_at_ms: i64::MAX,
            })
        }

        fn register(&self, challenge_id: &str, signature: &str) -> Result<Registration, Failure> {
            assert_eq!(challenge_id, CHALLENGE_ID);
            let identity = self.store.load().unwrap().unwrap();
            assert_eq!(identity.state.challenge_id(), Some(challenge_id));
            assert!(!self.store.has_confirmation().unwrap());
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
    fn confirmed_registration_finishes_after_process_restart() {
        const DIRECTORY: &str = "VOID_NATIVE_TEST_RECOVERY_DIR";
        const PUBLIC_KEY: &str = "VOID_NATIVE_TEST_RECOVERY_KEY";
        if let Some(path) = std::env::var_os(DIRECTORY) {
            let store = Store::from_test_path(path.into());
            assert!(matches!(inspect(&store).ok(), Some(Startup::Registered(id)) if id == "300"));
            let identity = store.load().unwrap().unwrap();
            assert_eq!(STANDARD.encode(identity.key.verifying_key().as_bytes()), std::env::var(PUBLIC_KEY).unwrap());
            assert!(matches!(identity.state, State::Registered(account)
                if account.void_id == "300" && account.created_at == "2026-11-14T00:00:00Z"));
            assert!(!store.has_confirmation().unwrap());
            return;
        }

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
        assert!(error.retry && error.saving);
        assert!(store.has_confirmation().unwrap());
        let pending = store.load().unwrap().unwrap();
        assert_eq!(pending.state.challenge_id(), Some(CHALLENGE_ID));
        assert_eq!(store.load_confirmation(&pending).unwrap().unwrap().void_id, "300");

        // Lose all in-memory state and retry while the final file is still locked.
        drop(setup);
        let still_locked = inspect(&store).err().unwrap();
        assert!(still_locked.retry && still_locked.saving);
        assert!(store.has_confirmation().unwrap());
        drop(held.take());

        let output = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "account::tests::confirmed_registration_finishes_after_process_restart"])
            .env(DIRECTORY, dir.path())
            .env(PUBLIC_KEY, STANDARD.encode(expected_key))
            .output().unwrap();
        assert!(output.status.success(), "{}{}", String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr));
        assert!(String::from_utf8_lossy(&output.stdout).contains("1 passed"));
        assert_eq!(submitted.get(), 1);
        assert!(matches!(inspect(&store).ok().unwrap(), Startup::Registered(id) if id == "300"));
    }

    #[test]
    fn mismatched_and_orphaned_confirmations_never_reach_the_api() {
        let dir = TestDir::new();
        let store = dir.store();
        let lock = store.lock().unwrap();
        let mut identity = Identity::generate().unwrap();
        identity.state = State::SubmissionPendingFor { challenge_id: CHALLENGE_ID.into() };
        store.save(&identity).unwrap();
        store.save_confirmation(&identity, &Registration {
            void_id: "300".into(),
            created_at: "2026-11-14T00:00:00Z".into(),
        }).unwrap();
        let different_challenge = format!("B{}", &CHALLENGE_ID[1..]);
        assert!(valid_token(&different_challenge));
        identity.state = State::SubmissionPendingFor { challenge_id: different_challenge };
        store.save(&identity).unwrap();
        drop(lock);

        let assert_blocked = || {
            assert!(!inspect(&store).err().unwrap().retry);
            let result = Setup::default().run::<LocalApi<'_>>(
                &store, || panic!("unverified confirmation must not reach the API"), |_| {},
            );
            assert!(!result.err().unwrap().retry);
        };
        assert_blocked();
        fs::remove_file(dir.record_path()).unwrap();
        assert_blocked();
        assert!(store.load().unwrap().is_none());
        assert!(store.has_confirmation().unwrap());
    }
}
