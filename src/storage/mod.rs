mod records;
#[cfg(all(test, windows))]
pub(crate) mod tests;

pub(crate) use records::{Contact, Conversation, Direction, Message};
#[allow(unused_imports)] // Storage API for future message delivery updates.
pub(crate) use records::DeliveryState;

use crate::local_data::{dpapi, move_file};
use rusqlite::{Connection, OpenFlags, OptionalExtension, TransactionBehavior, ffi, params};
use std::{
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    path::Path,
    time::Duration,
};
use zeroize::Zeroizing;

const DATABASE: &str = "messages.db";
const KEY_FILE: &str = "messages-key.dpapi";
const KEY_MAGIC: &[u8] = b"VOID-MESSAGE-DB-KEY\x01";
const MAX_KEY_FILE_SIZE: u64 = 16_384;
const MIGRATIONS: &[&str] = &[include_str!("schema_v1.sql")];

#[derive(Clone, Copy)]
pub(crate) struct Owner<'a> {
    pub void_id: &'a str,
    pub public_key: &'a [u8; 32],
}

#[derive(Debug)]
pub(crate) enum Error {
    Unavailable,
    Busy,
    KeyUnavailable,
    SqlCipherUnavailable,
    Unreadable,
    WrongAccount,
    UnsupportedSchema,
    Database,
}

impl Error {
    pub fn open_message(&self) -> &'static str {
        match self {
            Self::Unavailable => "Local storage is unavailable. Check that this Windows profile can access its VOID data.",
            Self::Busy => "Local storage is in use. Close other VOID windows and try again.",
            Self::KeyUnavailable => "The database key is missing or could not be unlocked. Keep your VOID files and use the original Windows profile and protected key.",
            Self::SqlCipherUnavailable => "This build could not enable SQLCipher. An SQLCipher-enabled build is required to open local storage.",
            Self::Unreadable => "The encrypted database could not be verified. Keep your VOID files and restore a matching backup if available.",
            Self::WrongAccount => "The encrypted database belongs to a different VOID account. Use the storage for the account registered on this device.",
            Self::UnsupportedSchema => "This build cannot open the local database version. Use a compatible VOID build.",
            Self::Database => "Local storage could not be opened. Check the VOID data directory and try again.",
        }
    }
}

impl From<io::Error> for Error {
    fn from(_: io::Error) -> Self { Self::Unavailable }
}

impl From<rusqlite::Error> for Error {
    fn from(error: rusqlite::Error) -> Self {
        match error.sqlite_error_code() {
            Some(rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked) => Self::Busy,
            Some(rusqlite::ErrorCode::DatabaseCorrupt | rusqlite::ErrorCode::NotADatabase) => Self::Unreadable,
            _ => Self::Database,
        }
    }
}

pub(crate) type Result<T> = std::result::Result<T, Error>;

pub(crate) struct Database {
    connection: Connection,
    _lock: File,
}

impl Database {
    pub(crate) fn open(root: &Path, owner: Owner<'_>) -> Result<Self> {
        let lock = OpenOptions::new().read(true).write(true).create(true).truncate(false)
            .open(root.join("messages.lock"))?;
        lock.try_lock().map_err(|error| match error {
            std::fs::TryLockError::WouldBlock => Error::Busy,
            std::fs::TryLockError::Error(_) => Error::Unavailable,
        })?;

        let directory = root.join("messages");
        if !entry_exists(&directory)? {
            // Publish the completed pair as a directory so a crash cannot expose a half-created store.
            // The separate staging directory is only used before the first successful publication.
            let staging = root.join("messages.initializing");
            match fs::create_dir(&staging) {
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => (),
                result => result?,
            }
            entry_exists(&staging)?;
            let key_path = staging.join(KEY_FILE);
            let db_path = staging.join(DATABASE);
            let has_database = entry_exists(&db_path)?;
            if !has_database {
                for sidecar in ["messages.db-journal", "messages.db-wal", "messages.db-shm"] {
                    if entry_exists(&staging.join(sidecar))? {
                        return Err(Error::Unreadable);
                    }
                }
            }
            let key = if entry_exists(&key_path)? {
                read_key(&key_path)?
            } else if has_database {
                return Err(Error::KeyUnavailable);
            } else {
                create_key(&key_path)?
            };
            let mut connection = open_keyed(&db_path, &key, !has_database)?;
            migrate(&mut connection, owner, true)?;
            connection.close().map_err(|(_, error)| Error::from(error))?;
            move_file(&staging, &directory, false)?;
        }

        let key = read_key(&directory.join(KEY_FILE))?;
        let mut connection = open_keyed(&directory.join(DATABASE), &key, false)?;
        migrate(&mut connection, owner, false)?;
        Ok(Self { connection, _lock: lock })
    }

    pub(crate) fn close(self) -> Result<()> {
        self.connection.close().map_err(|(_, error)| error.into())
    }
}

fn entry_exists(path: &Path) -> io::Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => Err(io::Error::other("Unexpected storage link")),
        Ok(_) => Ok(true),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error),
    }
}

fn read_key(path: &Path) -> Result<Zeroizing<[u8; 32]>> {
    if !entry_exists(path).map_err(|_| Error::KeyUnavailable)? {
        return Err(Error::KeyUnavailable);
    }
    let mut encrypted = Vec::new();
    File::open(path).and_then(|file| file.take(MAX_KEY_FILE_SIZE + 1).read_to_end(&mut encrypted))
        .map_err(|_| Error::KeyUnavailable)?;
    if encrypted.len() as u64 > MAX_KEY_FILE_SIZE {
        return Err(Error::KeyUnavailable);
    }
    let clear = dpapi(&encrypted, false).map_err(|_| Error::KeyUnavailable)?;
    let bytes = clear.strip_prefix(KEY_MAGIC).filter(|bytes| bytes.len() == 32)
        .ok_or(Error::KeyUnavailable)?;
    let mut key = Zeroizing::new([0; 32]);
    key.copy_from_slice(bytes);
    Ok(key)
}

fn create_key(path: &Path) -> Result<Zeroizing<[u8; 32]>> {
    let mut key = Zeroizing::new([0; 32]);
    getrandom::fill(key.as_mut()).map_err(|_| Error::Unavailable)?;
    let mut record = Zeroizing::new(Vec::with_capacity(KEY_MAGIC.len() + key.len()));
    record.extend_from_slice(KEY_MAGIC);
    record.extend_from_slice(key.as_ref());
    let protected = dpapi(&record, true).map_err(|_| Error::KeyUnavailable)?;
    let mut nonce = [0; 8];
    getrandom::fill(&mut nonce).map_err(|_| Error::Unavailable)?;
    let temp = path.with_extension(format!("{:016x}.tmp", u64::from_ne_bytes(nonce)));
    let mut file = OpenOptions::new().write(true).create_new(true).open(&temp)?;
    let written = file.write_all(&protected).and_then(|()| file.sync_all());
    drop(file);
    let result = written.and_then(|()| move_file(&temp, path, false));
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result?;
    Ok(key)
}

fn open_keyed(path: &Path, key: &[u8; 32], create: bool) -> Result<Connection> {
    if !create && !entry_exists(path)? {
        return Err(Error::Unreadable);
    }
    let mut flags = OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_PRIVATE_CACHE;
    if create {
        flags |= OpenFlags::SQLITE_OPEN_CREATE;
    }
    let connection = Connection::open_with_flags(path, flags)?;
    let cipher: String = connection.pragma_query_value(None, "cipher_version", |row| row.get(0))
        .map_err(|_| Error::SqlCipherUnavailable)?;
    if cipher.is_empty() {
        return Err(Error::SqlCipherUnavailable);
    }
    // Give SQLCipher a raw key through its C API so it never appears in SQL text or traces.
    let mut raw_key = Zeroizing::new([0; 67]);
    raw_key[0..2].copy_from_slice(b"x'");
    for (byte, hex) in key.iter().zip(raw_key[2..66].chunks_exact_mut(2)) {
        const HEX: &[u8] = b"0123456789abcdef";
        hex[0] = HEX[(byte >> 4) as usize];
        hex[1] = HEX[(byte & 15) as usize];
    }
    raw_key[66] = b'\'';
    connection.pragma_update(None, "cipher_log_level", "NONE")?;
    // SAFETY: The connection and the key buffer remain live for the call. SQLCipher copies the key.
    let result = unsafe { ffi::sqlite3_key(connection.handle(), raw_key.as_ptr().cast(), raw_key.len() as i32) };
    if result != ffi::SQLITE_OK {
        return Err(Error::Unreadable);
    }
    connection.pragma_update(None, "cipher_log_level", "NONE")?;
    let status: String = connection.pragma_query_value(None, "cipher_status", |row| row.get(0))
        .map_err(|_| Error::SqlCipherUnavailable)?;
    if status != "1" {
        return Err(Error::SqlCipherUnavailable);
    }
    connection.busy_timeout(Duration::from_secs(2))?;
    connection.execute_batch("PRAGMA temp_store = MEMORY; PRAGMA foreign_keys = ON; PRAGMA trusted_schema = OFF; PRAGMA synchronous = FULL;")?;
    if connection.pragma_query_value(None, "foreign_keys", |row| row.get::<_, i64>(0))? != 1 {
        return Err(Error::Database);
    }
    Ok(connection)
}

fn migrate(connection: &mut Connection, owner: Owner<'_>, initializing: bool) -> Result<()> {
    migrate_with_scripts(connection, owner, initializing, MIGRATIONS)
}

fn migrate_with_scripts(connection: &mut Connection, owner: Owner<'_>, initializing: bool, migrations: &[&str]) -> Result<()> {
    let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let version: i64 = tx.pragma_query_value(None, "user_version", |row| row.get(0))?;
    let version = usize::try_from(version).ok().filter(|&version| version <= migrations.len())
        .ok_or(Error::UnsupportedSchema)?;
    if version == 0 {
        let empty = tx.query_row("SELECT count(*) FROM sqlite_schema", [], |row| row.get::<_, i64>(0))? == 0;
        if !initializing || !empty {
            return Err(Error::Unreadable);
        }
    } else {
        let stored = tx.query_row("SELECT void_id, public_key FROM local_account WHERE singleton = 1", [], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, Vec<u8>>(1)?))
        }).optional().map_err(|_| Error::Unreadable)?;
        let Some((void_id, public_key)) = stored else { return Err(Error::Unreadable); };
        if void_id != owner.void_id || public_key != owner.public_key {
            return Err(Error::WrongAccount);
        }
        if tx.prepare("PRAGMA cipher_integrity_check")?.exists([])?
            || tx.pragma_query_value(None, "quick_check", |row| row.get::<_, String>(0))? != "ok"
            || tx.prepare("PRAGMA foreign_key_check")?.exists([])?
        {
            return Err(Error::Unreadable);
        }
    }
    for (index, sql) in migrations.iter().enumerate().skip(version) {
        tx.execute_batch(sql)?;
        if index == 0 {
            tx.execute("INSERT INTO local_account (singleton, void_id, public_key) VALUES (1, ?1, ?2)",
                params![owner.void_id, owner.public_key.as_slice()])?;
        }
        tx.pragma_update(None, "user_version", (index + 1) as i64)?;
    }
    tx.commit()?;
    Ok(())
}
