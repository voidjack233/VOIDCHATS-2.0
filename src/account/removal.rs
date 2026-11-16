use super::storage::{CONFIRMATION_FILE, Store, read_protected};
use crate::storage::{self, Owner};
use serde::{Deserialize, Serialize};
use std::{
    fs, io,
    path::{Path, PathBuf},
};

const JOURNAL: &str = "account-removal.dpapi";
const MAGIC: &[u8] = b"VOID-LOCAL-REMOVAL\x01";

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Removal {
    pub(super) void_id: String,
    pub(super) public_key: [u8; 32],
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) deletion: Option<Deletion>,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum Deletion {
    Pending,
    Confirmed { deleted_at: String },
}

#[derive(Debug)]
pub(super) enum Error {
    Busy,
    UnverifiedAccount,
    UnverifiedStorage,
    UnexpectedFiles,
    Unavailable,
    PendingDeletion,
}

impl Error {
    pub(super) fn message(&self) -> &'static str {
        match self {
            Self::Busy => "Close other VOID windows, then retry local removal.",
            Self::UnverifiedAccount => "VOID could not verify which local account to remove.",
            Self::UnverifiedStorage => {
                "VOID could not verify that the encrypted message database belongs to this account. Keep the original files."
            }
            Self::UnexpectedFiles => {
                "VOID found unexpected files or links in its account storage. Review them before retrying removal."
            }
            Self::Unavailable => {
                "VOID could not finish removing this device’s data. Check that the files are accessible, then retry."
            }
            Self::PendingDeletion => {
                "Finish permanent account deletion before removing this device’s data."
            }
        }
    }
}

impl From<io::Error> for Error {
    fn from(error: io::Error) -> Self {
        if error.kind() == io::ErrorKind::WouldBlock {
            Self::Busy
        } else {
            Self::Unavailable
        }
    }
}

type Result<T> = std::result::Result<T, Error>;

pub(super) fn pending(store: &Store) -> io::Result<bool> {
    metadata(&store.path().join(JOURNAL)).map(|entry| entry.is_some())
}

pub(super) fn lock_account(store: &Store) -> Result<fs::File> {
    let root = store.path();
    require_directory(root)?;
    require_directory(root.parent().ok_or(Error::UnexpectedFiles)?)?;
    for name in [
        "account.lock",
        "messages.lock",
        "account.dpapi",
        CONFIRMATION_FILE,
        JOURNAL,
    ] {
        regular_file(&root.join(name))?;
    }

    Ok(store.lock()?)
}

pub(super) fn read_journal(store: &Store) -> Result<Option<Removal>> {
    let path = store.path().join(JOURNAL);
    regular_file(&path)?;
    read_protected(&path)?
        .map(|bytes| {
            let bytes = bytes.strip_prefix(MAGIC).ok_or(Error::UnverifiedAccount)?;
            let target: Removal =
                serde_json::from_slice(bytes).map_err(|_| Error::UnverifiedAccount)?;
            if !super::valid_void_id(&target.void_id)
                || matches!(&target.deletion, Some(Deletion::Confirmed { deleted_at })
                    if !crate::relay::valid_utc_timestamp(deleted_at))
            {
                return Err(Error::UnverifiedAccount);
            }
            Ok(target)
        })
        .transpose()
}

pub(super) fn write_journal(store: &Store, target: &Removal) -> io::Result<()> {
    let mut bytes = MAGIC.to_vec();
    serde_json::to_writer(&mut bytes, target)?;
    store.write_protected(JOURNAL, &bytes)
}

/// With no explicit target, resume only a previously confirmed removal.
pub(super) fn remove(store: &Store, expected_id: Option<&str>) -> Result<bool> {
    if expected_id.is_none() && !pending(store)? {
        return Ok(false);
    }
    let root = store.path();
    let _account = lock_account(store)?;
    let journal = read_journal(store)?;
    if journal.is_none() && expected_id.is_none() {
        return Ok(false);
    }
    if matches!(
        &journal,
        Some(Removal {
            deletion: Some(Deletion::Pending),
            ..
        })
    ) {
        return Err(Error::PendingDeletion);
    }
    let _messages = storage::lock_messages(root).map_err(|error| match error {
        storage::Error::Busy => Error::Busy,
        _ => Error::Unavailable,
    })?;
    let identity = store
        .load_for_removal()
        .map_err(|_| Error::UnverifiedAccount)?;
    let existing = identity
        .map(|identity| -> Result<Removal> {
            let account = identity
                .state
                .registration()
                .ok_or(Error::UnverifiedAccount)?;
            Ok(Removal {
                void_id: account.void_id.clone(),
                public_key: identity.key.verifying_key().to_bytes(),
                deletion: None,
            })
        })
        .transpose()?;
    let resuming = journal.is_some();
    let target = journal
        .as_ref()
        .or(existing.as_ref())
        .ok_or(Error::UnverifiedAccount)?;
    if expected_id.is_some_and(|id| id != target.void_id)
        || existing.as_ref().is_some_and(|current| {
            current.void_id != target.void_id || current.public_key != target.public_key
        })
    {
        return Err(Error::UnverifiedAccount);
    }

    let owner = Owner {
        void_id: &target.void_id,
        public_key: &target.public_key,
    };
    store
        .verify_removal_confirmation(owner.void_id, owner.public_key)
        .map_err(|_| Error::UnverifiedAccount)?;
    let stores = [
        inspect_message_dir(&root.join("messages"), owner, false)?,
        inspect_message_dir(&root.join("messages.initializing"), owner, true)?,
    ];
    let temporary = fs::read_dir(root)?
        .filter_map(|entry| match entry {
            Ok(entry) if hex_name(&entry.file_name().to_string_lossy(), "account-") => {
                Some(Ok(entry.path()))
            }
            Ok(_) => None,
            Err(error) => Some(Err(error)),
        })
        .collect::<io::Result<Vec<_>>>()?;
    for file in &temporary {
        regular_file(file)?;
    }

    if !resuming {
        write_journal(store, target)?;
    }
    for (directory, files) in stores.into_iter().flatten() {
        // Keep the database key and any journals available until the database file is removed.
        remove_file(&directory.join("messages.db"))?;
        for file in files {
            remove_file(&file)?;
        }
        fs::remove_dir(&directory)?;
    }
    for file in temporary {
        remove_file(&file)?;
    }
    remove_file(&root.join(CONFIRMATION_FILE))?;
    remove_file(&root.join("account.dpapi"))?;
    remove_file(&root.join(JOURNAL))?;
    Ok(true)
}

fn inspect_message_dir(
    directory: &Path,
    owner: Owner<'_>,
    staging: bool,
) -> Result<Option<(PathBuf, Vec<PathBuf>)>> {
    if metadata(directory)?.is_none() {
        return Ok(None);
    }
    require_directory(directory)?;
    let mut files = Vec::new();
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let name = entry.file_name();
        let name = name.to_str().ok_or(Error::UnexpectedFiles)?;
        if !matches!(
            name,
            "messages.db"
                | "messages.db-wal"
                | "messages.db-shm"
                | "messages.db-journal"
                | "messages-key.dpapi"
        ) && !hex_name(name, "messages-key.")
        {
            return Err(Error::UnexpectedFiles);
        }
        regular_file(&entry.path())?;
        if name != "messages.db" {
            files.push(entry.path());
        }
    }
    if metadata(&directory.join("messages.db"))?.is_some() {
        storage::verify_removal_owner(directory, owner, staging)
            .map_err(|_| Error::UnverifiedStorage)?;
    }
    Ok(Some((directory.to_owned(), files)))
}

fn hex_name(name: &str, prefix: &str) -> bool {
    name.strip_prefix(prefix)
        .and_then(|name| name.strip_suffix(".tmp"))
        .is_some_and(|hex| hex.len() == 16 && hex.bytes().all(|byte| byte.is_ascii_hexdigit()))
}

fn metadata(path: &Path) -> io::Result<Option<fs::Metadata>> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => Ok(Some(metadata)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

fn is_link(metadata: &fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        // FILE_ATTRIBUTE_REPARSE_POINT includes directory junctions and symlinks.
        metadata.file_attributes() & 0x400 != 0
    }
    #[cfg(not(windows))]
    metadata.file_type().is_symlink()
}

fn require_directory(path: &Path) -> Result<()> {
    if !path.is_absolute()
        || !metadata(path)?.is_some_and(|entry| entry.is_dir() && !is_link(&entry))
    {
        return Err(Error::UnexpectedFiles);
    }
    Ok(())
}

fn regular_file(path: &Path) -> Result<()> {
    if metadata(path)?.is_some_and(|entry| !entry.is_file() || is_link(&entry)) {
        return Err(Error::UnexpectedFiles);
    }
    Ok(())
}

fn remove_file(path: &Path) -> Result<()> {
    regular_file(path)?;
    match fs::remove_file(path) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        result => Ok(result?),
    }
}
