use super::storage::{State, Store};
use crate::storage::{Database, Error, Owner, Result};

pub(crate) fn open(expected_id: &str, stage: &dyn Fn(&'static str)) -> Result<(Database, bool)> {
    open_store(&Store::for_user()?, expected_id, stage)
}

pub(super) fn open_store(
    store: &Store,
    expected_id: &str,
    stage: &dyn Fn(&'static str),
) -> Result<(Database, bool)> {
    let _lock = store.lock().map_err(|error| match error.kind() {
        std::io::ErrorKind::WouldBlock => Error::Busy,
        _ => Error::Unavailable,
    })?;
    let mut identity = store
        .load()
        .map_err(|_| Error::WrongAccount)?
        .ok_or(Error::WrongAccount)?;
    let account = identity
        .state
        .registration()
        .filter(|account| account.void_id == expected_id)
        .ok_or(Error::WrongAccount)?;
    let first_store = matches!(identity.state, State::RegisteredPendingStorage(_));
    let public_key = identity.key.verifying_key();
    let owner = Owner {
        void_id: expected_id,
        public_key: public_key.as_bytes(),
    };
    let mut db = if first_store {
        Database::open(store.path(), owner)?
    } else {
        Database::open_existing(store.path(), owner)?
    };

    stage("Initializing encryption…");
    db.initialize_signal(expected_id, &identity.key)?;
    let confirmed = db.prepare_publication()?.is_none();
    db.recover_interrupted_sends()?;

    // Seal the first-store permission before handing a prepared publication to the relay.
    // A crash before this write can reuse the same local keys. After it, a missing store fails closed.
    if first_store {
        identity.state = State::Registered(account.clone());
        store
            .save(&identity)
            .map_err(|_| Error::EnrollmentCheckpoint)?;
    }
    Ok((db, confirmed))
}

#[cfg(all(test, windows))]
pub(crate) fn open_fixture(
    root: &std::path::Path,
    id: &str,
    key: &ed25519_dalek::SigningKey,
) -> Result<(Database, bool)> {
    use super::{Identity, Registration};
    let store = Store::from_test_path(root.to_owned());
    {
        let _lock = store.lock()?;
        if store.load()?.is_none() {
            store.save(&Identity {
                key: key.clone(),
                state: State::RegisteredPendingStorage(Registration {
                    void_id: id.into(),
                    created_at: "2026-11-16T00:00:00Z".into(),
                }),
            })?;
        }
    }
    open_store(&store, id, &|_| {})
}
