mod deletion;
mod enrollment;
mod protocol;
mod removal;
mod storage;

pub(crate) use enrollment::open as open_enrollment;
#[cfg(all(test, windows))]
pub(crate) use enrollment::open_fixture as open_enrollment_fixture;
pub(crate) use protocol::{API_BASE, timestamp_ms, valid_token, valid_void_id};

use crate::AppWindow;
use base64::{Engine, engine::general_purpose::STANDARD};
use ed25519_dalek::Signer;
use protocol::{AccountApi, Failure, HttpAccountApi, Registration, Rejection, valid_registration};
use slint::ComponentHandle;
use std::{
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};
use storage::{Identity, State, Store};

pub(crate) const ENROLLMENT_PAGE: i32 = 6;

const UNRESOLVED: &str = "VOID may have created your account, but setup did not receive or save a confirmed VOID-ID. Keep this Windows profile’s VOID data. Account recovery is not available yet.";
const KEY_REGISTERED: &str = "This device key is already registered. Keep this Windows profile’s VOID data. Account recovery is not available yet.";
const INVALID_CONFIRMATION: &str = "The saved registration confirmation could not be verified against this device’s identity and setup request. Keep this Windows profile’s VOID data.";
const STORAGE_LOCK: &str =
    "Could not open local account storage for writing. Close other VOID windows, then try again.";

struct SetupError {
    message: String,
    retry: bool,
    wait: Option<u64>,
    saving: bool,
}

impl SetupError {
    fn retry(message: &str) -> Self {
        Self {
            message: message.into(),
            retry: true,
            wait: None,
            saving: false,
        }
    }

    fn blocked(message: &str) -> Self {
        Self {
            message: message.into(),
            retry: false,
            wait: None,
            saving: false,
        }
    }

    fn retry_saving(message: &str) -> Self {
        Self {
            saving: true,
            ..Self::retry(message)
        }
    }
}

enum Command {
    CheckStorage,
    CreateAccount,
    Remove {
        expected_id: Option<String>,
        permanent: bool,
        shutdown: Box<dyn FnOnce() + Send>,
    },
    Close,
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
    Removed,
    RemovalIncomplete {
        message: &'static str,
        can_go_back: bool,
        permanent: bool,
    },
}

fn inspect(store: &Store) -> Result<Startup, SetupError> {
    let load = || {
        store.load().map_err(|_| SetupError::retry(
        "Could not open the saved identity. Check that this is the same Windows user, then retry opening local storage."
    ))
    };
    let mut identity = load()?;
    let mut lock = None;
    if identity
        .as_ref()
        .is_some_and(|id| id.state.submission_pending())
    {
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
        Some(Identity {
            state: State::Registered(account) | State::RegisteredPendingStorage(account),
            ..
        }) => Ok(Startup::Registered(account.void_id)),
        Some(Identity {
            state: State::AlreadyRegistered,
            ..
        }) => Ok(Startup::Blocked(KEY_REGISTERED)),
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
            "Could not check for a saved registration confirmation. Check local storage and try again.",
        )),
    }
}

fn load_confirmation(
    store: &Store,
    identity: &Identity,
) -> Result<Option<Registration>, SetupError> {
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
    identity.state = State::RegisteredPendingStorage(account.clone());
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
                    "Please wait {} seconds before trying again.",
                    remaining
                        .as_secs()
                        .saturating_add(u64::from(remaining.subsec_nanos() > 0))
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
            None if self.pending_save.is_some() => {
                return Err(SetupError::blocked(
                    "The original identity file is missing. Keep any existing VOID data. Account recovery is not available yet.",
                ));
            }
            None => {
                reject_unexpected_confirmation(store)?;
                stage("Creating your protected device identity…");
                let identity = Identity::generate().map_err(|_| {
                    SetupError::retry(
                        "Could not prepare a secure device identity. Please try again.",
                    )
                })?;
                store.save(&identity).map_err(|_| SetupError::retry(
                    "Could not save the protected identity. Check that local storage is writable and try again."
                ))?;
                identity
            }
        };

        match &identity.state {
            State::Registered(account) | State::RegisteredPendingStorage(account) => {
                return Ok(account.clone());
            }
            State::AlreadyRegistered => return Err(SetupError::blocked(KEY_REGISTERED)),
            State::SubmissionPending | State::SubmissionPendingFor { .. } => {
                return self.save_registration(store, identity, stage);
            }
            State::Prepared if self.pending_save.is_some() => {
                return Err(SetupError::blocked(UNRESOLVED));
            }
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
        identity.state = State::SubmissionPendingFor {
            challenge_id: challenge.id.clone(),
        };
        store.save(&identity).map_err(|_| SetupError::retry(
            "Could not save setup progress locally. No account request was sent. Check local storage and try again."
        ))?;
        if challenge.is_expired() {
            return Err(self.handle_rejection(
                store,
                &mut identity,
                Failure::ExpiredChallenge,
                true,
            ));
        }
        stage("Registering your VOID account…");
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
                let pending = self
                    .pending_save
                    .as_ref()
                    .filter(|pending| {
                        pending.public_key == identity.key.verifying_key().to_bytes()
                            && Some(pending.challenge_id.as_str()) == identity.state.challenge_id()
                    })
                    .ok_or_else(|| SetupError::blocked(UNRESOLVED))?;

                stage("Saving your VOID-ID on this device…");
                store.save_confirmation(&identity, &pending.account).map_err(|_| SetupError::retry_saving(
                    "VOID confirmed your account, but its confirmation could not be saved locally. Keep this window open and retry saving."
                ))?;
                pending.account.clone()
            }
        };
        stage("Saving your registered identity…");
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
                    "VOID says this key is already registered, but local storage could not save that status. Keep this Windows profile’s VOID data.",
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
                    "The account request was rejected, but setup could not save that result locally. Keep this Windows profile’s VOID data.",
                );
            }
        }
        self.failure(error, submitted)
    }

    fn failure(&mut self, error: Failure, submitted: bool) -> SetupError {
        let (message, wait) = match error {
            Failure::Network => (
                "Could not reach VOID. Check your connection and try again. Your device identity is saved.",
                None,
            ),
            Failure::Timeout => (
                "VOID did not respond in time. Check your connection and try again. Your device identity is saved.",
                None,
            ),
            Failure::InvalidResponse => (
                "VOID returned an unexpected setup response. Your device identity is saved.",
                None,
            ),
            Failure::InvalidChallenge => (
                "VOID returned an invalid setup challenge. Try again to request a fresh one.",
                None,
            ),
            Failure::ExpiredChallenge => (
                "The setup challenge expired or was rejected. Try again to request a fresh one.",
                None,
            ),
            Failure::Rejected(Rejection::InvalidProof, wait) => (
                "The setup challenge expired or was rejected. Try again to request a fresh one.",
                wait,
            ),
            Failure::Rejected(Rejection::InvalidInput, wait) => (
                if submitted {
                    "VOID rejected the account request. Try again with the saved identity."
                } else {
                    "VOID rejected the challenge request. Try again with the saved identity."
                },
                wait,
            ),
            Failure::Rejected(Rejection::RateLimited, wait) => (
                "Too many setup attempts. Please wait before trying again.",
                wait,
            ),
            Failure::Rejected(Rejection::CapacityReached, wait) => (
                "VOID is handling too many setup requests. Please wait and try again.",
                wait,
            ),
            Failure::Rejected(Rejection::KeyRegistered, _) => {
                return SetupError::blocked(KEY_REGISTERED);
            }
            Failure::Unavailable => (
                "VOID is temporarily unavailable. Your device identity is saved. Try again later.",
                None,
            ),
        };
        if let Some(seconds) = wait {
            self.retry_after = Some((Instant::now(), Duration::from_secs(seconds)));
        }
        SetupError {
            message: message.into(),
            retry: true,
            wait,
            saving: false,
        }
    }
}

pub(crate) struct RegisteredIdentity {
    pub(crate) void_id: String,
    pub(crate) key: ed25519_dalek::SigningKey,
}

pub(crate) fn load_registered_identity(expected_id: &str) -> std::io::Result<RegisteredIdentity> {
    let identity = Store::for_user()?.load()?;
    match identity {
        Some(Identity {
            key,
            state: State::Registered(account) | State::RegisteredPendingStorage(account),
        }) if account.void_id == expected_id => Ok(RegisteredIdentity {
            void_id: account.void_id,
            key,
        }),
        _ => Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "Registered identity unavailable",
        )),
    }
}

pub(crate) struct Worker {
    sender: mpsc::Sender<Command>,
    thread: thread::JoinHandle<()>,
}

impl Worker {
    pub(crate) fn remover(
        &self,
    ) -> impl Fn(Option<String>, bool, Box<dyn FnOnce() + Send>) -> bool + 'static {
        let sender = self.sender.clone();
        move |expected_id, permanent, shutdown| {
            sender
                .send(Command::Remove {
                    expected_id,
                    permanent,
                    shutdown,
                })
                .is_ok()
        }
    }

    pub(crate) fn finish(self) {
        let _ = self.sender.send(Command::Close);
        let _ = self.thread.join();
    }
}

pub(crate) fn bind(ui: &AppWindow) -> std::io::Result<Worker> {
    bind_with(ui, Store::for_user, deletion::confirm)
}

fn bind_with(
    ui: &AppWindow,
    open_store: impl Fn() -> std::io::Result<Store> + Send + 'static,
    confirm: impl Fn(&Store, Option<&str>, &dyn Fn(&'static str)) -> Result<(), &'static str>
    + Send
    + 'static,
) -> std::io::Result<Worker> {
    let (send, receive) = mpsc::channel();
    let weak = ui.as_weak();
    let thread = thread::Builder::new()
        .name("void-account".into())
        .spawn(move || {
            let mut setup = Setup::default();
            let mut command = Command::CheckStorage;
            loop {
                let checking = matches!(command, Command::CheckStorage);
                let update_removal = |permanent, stage: &'static str| {
                    let _ = weak.upgrade_in_event_loop(move |ui| {
                        ui.set_removal_permanent(permanent);
                        ui.set_removal_busy(true);
                        ui.set_removal_status(stage.into());
                    });
                };
                let result = match command {
                    Command::Close => break,
                    Command::Remove { expected_id, permanent, shutdown } => {
                        shutdown();
                        setup = Setup::default();
                        match open_store() {
                            Ok(store) if permanent => remove_permanently(
                                &store, expected_id.as_deref(), &|stage| update_removal(true, stage),
                                &confirm,
                            ),
                            Ok(store) => resume_or_remove(&store, expected_id.as_deref()),
                            Err(_) => Ok(Startup::RemovalIncomplete {
                                message: removal::Error::Unavailable.message(), can_go_back: true,
                                permanent,
                            }),
                        }
                    }
                    Command::CheckStorage | Command::CreateAccount => open_store().map_err(|_| SetupError::retry(
                        "The Windows local app data directory is unavailable. Retry opening local storage."
                    )).and_then(|store| {
                        if checking {
                            resume_setup(&store, &|stage| update_removal(true, stage), &confirm)
                        } else {
                            setup.run(&store, HttpAccountApi::new, |stage| {
                                let _ = weak.upgrade_in_event_loop(move |ui| ui.set_setup_status(stage.into()));
                            }).map(|account| Startup::Registered(account.void_id))
                        }
                    }),
                };
                let save_only = setup.pending_save.is_some();
                let _ = weak.upgrade_in_event_loop(move |ui| {
                    let registered = matches!(result, Ok(Startup::Registered(_)));
                    finish(&ui, checking, save_only, result);
                    if registered {
                        ui.invoke_account_registered();
                    }
                });
                match receive.recv() {
                    Ok(next) => command = next,
                    Err(_) => break,
                }
            }
        })?;

    let weak = ui.as_weak();
    let sender = send.clone();
    ui.on_submit_setup(move || {
        let Some(ui) = weak.upgrade() else {
            return;
        };
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
        if sender.send(command).is_err() {
            finish(
                &ui,
                false,
                false,
                Err(SetupError::blocked(
                    "Account setup stopped. Close and reopen VOID.",
                )),
            );
        }
    });
    Ok(Worker {
        sender: send,
        thread,
    })
}

fn resume_or_remove(store: &Store, expected_id: Option<&str>) -> Result<Startup, SetupError> {
    match removal::remove(store, expected_id) {
        Ok(true) => Ok(Startup::Removed),
        Ok(false) => inspect(store),
        Err(error) => Ok(Startup::RemovalIncomplete {
            message: error.message(),
            can_go_back: removal::pending(store).is_ok_and(|pending| !pending),
            permanent: matches!(error, removal::Error::PendingDeletion),
        }),
    }
}

fn resume_setup(
    store: &Store,
    progress: &impl Fn(&'static str),
    confirm: impl FnOnce(&Store, Option<&str>, &dyn Fn(&'static str)) -> Result<(), &'static str>,
) -> Result<Startup, SetupError> {
    match removal::read_journal(store) {
        Ok(Some(journal)) if journal.deletion.is_some() => {
            remove_permanently(store, None, progress, confirm)
        }
        _ => resume_or_remove(store, None),
    }
}

fn remove_permanently(
    store: &Store,
    expected_id: Option<&str>,
    progress: &impl Fn(&'static str),
    confirm: impl FnOnce(&Store, Option<&str>, &dyn Fn(&'static str)) -> Result<(), &'static str>,
) -> Result<Startup, SetupError> {
    if let Err(message) = confirm(store, expected_id, progress) {
        return Ok(Startup::RemovalIncomplete {
            message,
            can_go_back: removal::pending(store).is_ok_and(|pending| !pending),
            permanent: true,
        });
    }
    progress("Deletion confirmed. Removing this device’s VOID data…");
    match removal::remove(store, None) {
        Ok(_) => Ok(Startup::Removed),
        Err(error) => Ok(Startup::RemovalIncomplete {
            message: match error {
                removal::Error::Busy => {
                    "Deletion was confirmed. Close other VOID windows, then retry cleanup."
                }
                removal::Error::UnverifiedStorage => {
                    "Deletion was confirmed. VOID could not verify ownership of the encrypted database. Keep the original files."
                }
                removal::Error::UnexpectedFiles => {
                    "Deletion was confirmed. VOID found unexpected files or links. Review them, then retry cleanup."
                }
                _ => {
                    "Deletion was confirmed. Check that this device’s account files are accessible, then retry cleanup."
                }
            },
            can_go_back: false,
            permanent: true,
        }),
    }
}

fn finish(ui: &AppWindow, checking: bool, save_only: bool, result: Result<Startup, SetupError>) {
    ui.set_removal_busy(false);
    ui.set_removal_error("".into());
    ui.set_removal_can_go_back(false);
    ui.set_removal_status("".into());
    ui.set_removal_permanent(false);
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
        Ok(Startup::Removed) => {
            ui.set_account_void_id("".into());
            ui.set_setup_error("".into());
            ui.set_storage_ready(false);
            ui.set_storage_retryable(false);
            ui.set_storage_loading(true);
            ui.set_storage_error("".into());
            ui.set_messaging_ready(false);
            ui.set_publishing_keys(false);
            ui.set_messaging_notice("".into());
            ui.set_publication_error("".into());
            ui.set_publication_retryable(false);
            ui.set_connection_status("Disconnected".into());
            ui.set_stored_chats(Default::default());
            ui.set_stored_requests(Default::default());
            ui.set_chats_unread(false);
            ui.set_requests_unread(false);
            ui.set_stored_contacts(Default::default());
            ui.set_stored_messages(Default::default());
            ui.set_selected_conversation("".into());
            ui.set_conversation_title("".into());
            ui.set_conversation_read_through("".into());
            ui.set_conversation_is_request(false);
            ui.set_conversation_peer_deleted(false);
            ui.set_conversation_loading(false);
            ui.set_creating_conversation(false);
            ui.set_conversation_error("".into());
            ui.set_stored_draft("".into());
            ui.set_draft_status("".into());
            ui.set_draft_save_failed(false);
            ui.set_unsaved_draft_failure(false);
            ui.set_send_busy(false);
            ui.set_section("Chats".into());
            ui.invoke_cancel_local_removal();
            ui.invoke_cancel_permanent_deletion();
            ui.set_setup_page(1);
        }
        Ok(Startup::RemovalIncomplete {
            message,
            can_go_back,
            permanent,
        }) => {
            ui.set_removal_permanent(permanent);
            ui.set_removal_error(message.into());
            ui.set_removal_can_go_back(can_go_back && !ui.get_account_void_id().is_empty());
        }
        Ok(Startup::Registered(id)) => {
            ui.set_account_void_id(id.into());
            ui.set_setup_error("".into());
            ui.set_setup_page(ENROLLMENT_PAGE);
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
            ui.set_setup_action(
                if error.saving || save_only {
                    "Retry saving VOID-ID"
                } else if checking {
                    "Retry opening local storage"
                } else {
                    "Retry account setup"
                }
                .into(),
            );
            ui.set_setup_page(5);
        }
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use ed25519_dalek::{Signature, VerifyingKey};
    use protocol::{Challenge, valid_token};
    use std::{
        cell::Cell,
        fs::{self, File, OpenOptions},
        os::windows::fs::OpenOptionsExt,
        process::Command,
    };
    use storage::tests::TestDir;

    const CHALLENGE_ID: &str = "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";

    #[test]
    fn interrupted_local_removal_resumes_before_allowing_fresh_registration() {
        let dir = crate::storage::tests::TestDir::new();
        let store = Store::from_test_path(dir.0.clone());
        let original = ed25519_dalek::SigningKey::from_bytes(&[7; 32]);
        let (db, _) = enrollment::open_fixture(&dir.0, "123", &original).unwrap();
        let conversation = db.create_conversation("456", 100).unwrap();
        db.save_draft(conversation, "Isolated draft").unwrap();
        let unrelated = dir.0.join("keep.txt");
        fs::write(&unrelated, b"unrelated").unwrap();
        let account = dir.0.join("account.dpapi");
        assert!(matches!(
            removal::remove(&store, Some("123")),
            Err(removal::Error::Busy)
        ));
        assert!(!removal::pending(&store).unwrap());
        db.close().unwrap();

        let held = OpenOptions::new()
            .read(true)
            .share_mode(0x3)
            .open(&account)
            .unwrap();
        assert!(matches!(
            resume_or_remove(&store, Some("123")).ok(),
            Some(Startup::RemovalIncomplete {
                can_go_back: false,
                ..
            })
        ));
        assert!(removal::pending(&store).unwrap());
        assert!(store.load().is_err());
        assert!(!dir.0.join("messages").exists());
        assert!(account.exists());
        drop(held);

        let reopened = Store::from_test_path(dir.0.clone());
        assert!(matches!(
            resume_or_remove(&reopened, None).ok(),
            Some(Startup::Removed)
        ));
        assert!(!removal::pending(&reopened).unwrap());
        assert!(matches!(
            resume_or_remove(&reopened, None).ok(),
            Some(Startup::Welcome)
        ));
        assert_eq!(fs::read(unrelated).unwrap(), b"unrelated");
        assert!(!account.exists());

        let submitted = Cell::new(0);
        let result = Setup::default()
            .run(
                &reopened,
                || {
                    let identity = reopened.load().unwrap().unwrap();
                    let expected_key = identity.key.verifying_key().to_bytes();
                    assert_ne!(expected_key, original.verifying_key().to_bytes());
                    Ok(LocalApi {
                        store: &reopened,
                        expected_key,
                        submitted: &submitted,
                        answer: Cell::new(Some(Ok(Registration {
                            void_id: "789".into(),
                            created_at: "2026-11-16T00:00:00Z".into(),
                        }))),
                        hold_record: None,
                    })
                },
                |_| {},
            )
            .ok()
            .unwrap();
        assert_eq!(result.void_id, "789");
        assert_eq!(submitted.get(), 1);
        let (db, published) = enrollment::open_store(&reopened, "789", &|_| {}).unwrap();
        assert!(!published);
        assert!(db.conversations(false).unwrap().is_empty());
    }

    #[test]
    fn removal_preserves_a_database_owned_by_another_account() {
        let dir = crate::storage::tests::TestDir::new();
        let store = Store::from_test_path(dir.0.clone());
        let local = ed25519_dalek::SigningKey::from_bytes(&[7; 32]);
        let other = ed25519_dalek::SigningKey::from_bytes(&[8; 32]).verifying_key();
        let lock = store.lock().unwrap();
        store
            .save(&Identity {
                key: local,
                state: State::Registered(Registration {
                    void_id: "123".into(),
                    created_at: "2026-11-16T00:00:00Z".into(),
                }),
            })
            .unwrap();
        drop(lock);
        let owner = crate::storage::Owner {
            void_id: "456",
            public_key: other.as_bytes(),
        };
        let db = crate::storage::Database::open(&dir.0, owner).unwrap();
        let conversation = db.create_conversation("789", 100).unwrap();
        db.save_draft(conversation, "Other account’s draft")
            .unwrap();
        db.close().unwrap();
        let account_path = dir.0.join("account.dpapi");
        let before = fs::read(&account_path).unwrap();
        assert!(matches!(
            removal::remove(&store, Some("123")),
            Err(removal::Error::UnverifiedStorage)
        ));
        assert!(!removal::pending(&store).unwrap());
        assert!(fs::read(account_path).unwrap() == before);
        let db = crate::storage::Database::open_existing(&dir.0, owner).unwrap();
        assert_eq!(
            db.draft(conversation).unwrap().as_deref(),
            Some("Other account’s draft")
        );
    }

    #[test]
    fn signal_setup_preserves_the_protected_account_record() {
        let dir = TestDir::new();
        let store = dir.store();
        let lock = store.lock().unwrap();
        let mut identity = Identity::generate().unwrap();
        identity.state = State::Registered(Registration {
            void_id: "123".into(),
            created_at: "2026-01-02T03:04:05Z".into(),
        });
        store.save(&identity).unwrap();
        drop(lock);

        let record = dir.record_path();
        let protected = fs::read(&record).unwrap();
        let public_key = identity.key.verifying_key();
        let mut db = crate::storage::Database::open(
            record.parent().unwrap(),
            crate::storage::Owner {
                void_id: "123",
                public_key: public_key.as_bytes(),
            },
        )
        .unwrap();
        db.initialize_signal("123", &store.load().unwrap().unwrap().key)
            .unwrap();
        db.close().unwrap();

        assert!(fs::read(&record).unwrap() == protected);
        let loaded = store.load().unwrap().unwrap();
        assert!(matches!(loaded.state, State::Registered(account) if account.void_id == "123"));
        let challenge = b"local account proof test";
        assert!(
            public_key
                .verify_strict(challenge, &loaded.key.sign(challenge))
                .is_ok()
        );
    }

    #[test]
    fn saved_registration_resumes_signal_enrollment_after_a_lost_acknowledgement() {
        use crate::{
            messaging,
            relay::{
                Activity, Connection,
                tests::{next_attempt, receive_json, send_json, verify_account_proof},
            },
        };
        use serde_json::json;
        use tokio::{net::TcpListener, time::timeout};

        let dir = crate::storage::tests::TestDir::new();
        let store = Store::from_test_path(dir.0.clone());
        let identity = Identity::generate().unwrap();
        let key = identity.key.clone();
        let verifying_key = identity.key.verifying_key();
        let lock = store.lock().unwrap();
        store.save(&identity).unwrap();
        drop(lock);

        let submitted = Cell::new(0);
        let mut setup = Setup::default();
        let registration = setup
            .run(
                &store,
                || {
                    Ok(LocalApi {
                        store: &store,
                        expected_key: verifying_key.to_bytes(),
                        submitted: &submitted,
                        answer: Cell::new(Some(Ok(Registration {
                            void_id: "123".into(),
                            created_at: "2026-11-16T00:00:00Z".into(),
                        }))),
                        hold_record: None,
                    })
                },
                |_| {},
            )
            .ok()
            .unwrap();
        assert_eq!(registration.void_id, "123");
        assert!(matches!(
            store.load().unwrap().unwrap().state,
            State::RegisteredPendingStorage(_)
        ));
        let account_path = dir.0.join("account.dpapi");
        let held = OpenOptions::new()
            .read(true)
            .share_mode(0x3)
            .open(&account_path)
            .unwrap();
        assert!(matches!(
            enrollment::open_store(&store, "123", &|_| {}),
            Err(crate::storage::Error::EnrollmentCheckpoint)
        ));
        assert!(matches!(
            store.load().unwrap().unwrap().state,
            State::RegisteredPendingStorage(_)
        ));
        let mut prepared = crate::storage::Database::open_existing(
            &dir.0,
            crate::storage::Owner {
                void_id: "123",
                public_key: verifying_key.as_bytes(),
            },
        )
        .unwrap();
        let initial = prepared.prepare_publication().unwrap().unwrap();
        prepared.close().unwrap();
        drop(held);

        let (mut db, confirmed) = enrollment::open_store(&store, "123", &|_| {}).unwrap();
        assert!(!confirmed);
        let saved = db.prepare_publication().unwrap().unwrap();
        assert!(saved.id == initial.id && saved.payload_base64 == initial.payload_base64);
        let encrypted_account = fs::read(dir.0.join("account.dpapi")).unwrap();

        let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        listener.set_nonblocking(true).unwrap();
        let endpoint = format!("ws://{}/ws", listener.local_addr().unwrap());
        let server = thread::spawn(move || {
            tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap()
                .block_on(async {
                    timeout(Duration::from_secs(15), async {
                        let listener = TcpListener::from_std(listener).unwrap();
                        let mut before_restart = next_attempt(&listener).await;
                        verify_account_proof(&mut before_restart, &verifying_key, "123").await;
                        send_json(&mut before_restart, json!({"type":"auth_ok", "void_id":"123"})).await;
                        let first = receive_json(&mut before_restart).await;
                        assert_eq!(first["type"], "prekeys_publish");
                        drop(before_restart); // The relay stored the publication and lost the acknowledgement.

                        let mut after_restart = next_attempt(&listener).await;
                        verify_account_proof(&mut after_restart, &verifying_key, "123").await;
                        send_json(&mut after_restart, json!({"type":"auth_ok", "void_id":"123"})).await;
                        assert!(receive_json(&mut after_restart).await == first);
                        send_json(&mut after_restart, json!({"type":"prekeys_published",
                            "publication_id":first["publication_id"], "ec_remaining":"20", "kyber_remaining":"20"})).await;
                    }).await.unwrap();
                });
        });

        let connect = || {
            let (tx, rx) = mpsc::channel();
            let key = key.clone();
            let (relay, worker) = Connection::spawn_with_activity(
                endpoint.clone(),
                move |id| {
                    Ok(RegisteredIdentity {
                        void_id: id.into(),
                        key: key.clone(),
                    })
                },
                |_| {},
                move |activity| {
                    let _ = tx.send(activity);
                },
            )
            .unwrap();
            assert!(relay.connect("123".into()));
            let Activity::Online(session) = rx.recv_timeout(Duration::from_secs(6)).unwrap() else {
                panic!("mock relay did not authenticate the registration");
            };
            (relay, worker, session)
        };
        let (relay, worker, session) = connect();
        assert!(matches!(
            messaging::publish(&mut db, &session),
            Err(messaging::Error::Relay(
                crate::relay::RequestError::Interrupted
            ))
        ));
        worker.finish();
        drop(relay);
        db.close().unwrap();
        drop(setup);

        assert!(matches!(inspect(&store).ok(), Some(Startup::Registered(id)) if id == "123"));
        let registration = Setup::default()
            .run::<LocalApi<'_>>(
                &store,
                || panic!("resumed enrollment must not register again"),
                |_| {},
            )
            .ok()
            .unwrap();
        assert_eq!(registration.void_id, "123");
        let (mut db, confirmed) = enrollment::open_store(&store, "123", &|_| {}).unwrap();
        assert!(!confirmed);
        let saved = db.prepare_publication().unwrap().unwrap();
        assert!(saved.id == initial.id && saved.payload_base64 == initial.payload_base64);
        let (relay, worker, session) = connect();
        messaging::publish(&mut db, &session).unwrap();
        db.close().unwrap();
        worker.finish();
        drop(relay);
        server.join().unwrap();

        let (db, confirmed) = enrollment::open_store(&store, "123", &|_| {}).unwrap();
        assert!(confirmed);
        db.close().unwrap();
        assert_eq!(submitted.get(), 1);
        assert!(fs::read(dir.0.join("account.dpapi")).unwrap() == encrypted_account);

        let missing = crate::storage::tests::TestDir::new();
        fs::write(missing.0.join("account.dpapi"), &encrypted_account).unwrap();
        let missing = Store::from_test_path(missing.0.clone());
        assert!(matches!(
            enrollment::open_store(&missing, "123", &|_| {}),
            Err(crate::storage::Error::MissingSignalStore)
        ));
        assert!(!missing.path().join("messages").exists());
    }

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
            VerifyingKey::from_bytes(&self.expected_key)
                .unwrap()
                .verify_strict(
                    b"validated test challenge",
                    &Signature::from_slice(&STANDARD.decode(signature).unwrap()).unwrap(),
                )
                .unwrap();
            self.submitted.set(self.submitted.get() + 1);
            if let Some((path, held)) = self.hold_record {
                let handle = OpenOptions::new()
                    .read(true)
                    .share_mode(0x3)
                    .open(path)
                    .unwrap();
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
        let api = |answer| {
            Ok(LocalApi {
                store: &store,
                expected_key,
                submitted: &submitted,
                answer: Cell::new(Some(answer)),
                hold_record: None,
            })
        };
        let mut setup = Setup::default();
        let rejected = setup
            .run(
                &store,
                || api(Err(Failure::Rejected(Rejection::InvalidProof, None))),
                |_| {},
            )
            .err()
            .unwrap();
        assert!(rejected.retry);
        assert!(matches!(
            store.load().unwrap().unwrap().state,
            State::Prepared
        ));
        let lost = setup
            .run(&store, || api(Err(Failure::Timeout)), |_| {})
            .err()
            .unwrap();
        assert!(!lost.retry);
        assert_eq!(submitted.get(), 2);

        let reopened = dir.store();
        let saved = reopened.load().unwrap().unwrap();
        assert_eq!(saved.key.verifying_key().to_bytes(), expected_key);
        assert!(matches!(
            inspect(&reopened).ok().unwrap(),
            Startup::Blocked(UNRESOLVED)
        ));
        let retry = Setup::default()
            .run::<LocalApi<'_>>(
                &reopened,
                || panic!("a pending registration must not reach the API"),
                |_| {},
            )
            .err()
            .unwrap();
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
            assert_eq!(
                STANDARD.encode(identity.key.verifying_key().as_bytes()),
                std::env::var(PUBLIC_KEY).unwrap()
            );
            assert!(
                matches!(identity.state, State::RegisteredPendingStorage(account)
                if account.void_id == "300" && account.created_at == "2026-11-14T00:00:00Z")
            );
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
        let error = setup
            .run(
                &store,
                || {
                    Ok(LocalApi {
                        store: &store,
                        expected_key,
                        submitted: &submitted,
                        answer: Cell::new(Some(Ok(Registration {
                            void_id: "300".into(),
                            created_at: "2026-11-14T00:00:00Z".into(),
                        }))),
                        hold_record: Some((&record_path, &held)),
                    })
                },
                |_| {},
            )
            .err()
            .unwrap();
        assert!(error.retry && error.saving);
        assert!(store.has_confirmation().unwrap());
        let pending = store.load().unwrap().unwrap();
        assert_eq!(pending.state.challenge_id(), Some(CHALLENGE_ID));
        assert_eq!(
            store.load_confirmation(&pending).unwrap().unwrap().void_id,
            "300"
        );

        // Lose all in-memory state and retry while the final file is still locked.
        drop(setup);
        let still_locked = inspect(&store).err().unwrap();
        assert!(still_locked.retry && still_locked.saving);
        assert!(store.has_confirmation().unwrap());
        drop(held.take());

        let output = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "account::tests::confirmed_registration_finishes_after_process_restart",
            ])
            .env(DIRECTORY, dir.path())
            .env(PUBLIC_KEY, STANDARD.encode(expected_key))
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
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
        identity.state = State::SubmissionPendingFor {
            challenge_id: CHALLENGE_ID.into(),
        };
        store.save(&identity).unwrap();
        store
            .save_confirmation(
                &identity,
                &Registration {
                    void_id: "300".into(),
                    created_at: "2026-11-14T00:00:00Z".into(),
                },
            )
            .unwrap();
        let different_challenge = format!("B{}", &CHALLENGE_ID[1..]);
        assert!(valid_token(&different_challenge));
        identity.state = State::SubmissionPendingFor {
            challenge_id: different_challenge,
        };
        store.save(&identity).unwrap();
        drop(lock);

        let assert_blocked = || {
            assert!(!inspect(&store).err().unwrap().retry);
            let result = Setup::default().run::<LocalApi<'_>>(
                &store,
                || panic!("unverified confirmation must not reach the API"),
                |_| {},
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
