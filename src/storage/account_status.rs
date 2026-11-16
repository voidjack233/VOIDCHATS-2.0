use super::{Database, Error, Result};
use rusqlite::{Connection, params};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AccountStatus {
    Unknown,
    Active,
    Deleted,
}

/// An explicit peer status from an authenticated, correlated relay reply.
pub(crate) enum VerifiedPeerStatus {
    Active,
    Deleted,
}

impl Database {
    pub(crate) fn peers_to_refresh(&self, selected: Option<i64>) -> Result<Vec<String>> {
        let mut query = self.connection.prepare(
            "SELECT c.peer_void_id FROM conversations c
             LEFT JOIN peer_account_status account ON account.void_id = c.peer_void_id
             WHERE account.status IS NULL OR account.status <> 'deleted'
             ORDER BY (c.id = ?1) DESC, c.last_activity_at DESC, c.id",
        )?;
        Ok(query
            .query_map([selected], |row| row.get(0))?
            .collect::<rusqlite::Result<_>>()?)
    }

    /// Read the saved evidence for a VOID-ID. A missing row means Unknown.
    pub(crate) fn peer_account_status(&self, void_id: &str) -> Result<AccountStatus> {
        let status = self.connection.query_row(
            "SELECT (SELECT status FROM peer_account_status WHERE void_id = ?1)",
            [void_id],
            |row| row.get::<_, Option<String>>(0),
        )?;
        decode(status.as_deref())
    }

    pub(crate) fn conversation_account_status(&self, id: i64) -> Result<AccountStatus> {
        conversation_status(&self.connection, id)
    }

    /// The caller must authenticate and correlate the backend response before invoking this.
    pub(crate) fn record_verified_peer_status(
        &self,
        void_id: &str,
        status: VerifiedPeerStatus,
    ) -> Result<()> {
        if !crate::account::valid_void_id(void_id) {
            return Err(Error::InvalidAccountStatusPeer);
        }
        let status = match status {
            VerifiedPeerStatus::Active => "active",
            VerifiedPeerStatus::Deleted => "deleted",
        };
        self.connection.execute(
            "INSERT INTO peer_account_status (void_id, status)
             SELECT ?1, ?2 WHERE NOT EXISTS (SELECT 1 FROM local_account WHERE void_id = ?1)
             ON CONFLICT (void_id) DO UPDATE SET status = excluded.status
             WHERE peer_account_status.status <> 'deleted'",
            params![void_id, status],
        )?;
        Ok(())
    }
}

pub(super) fn conversation_status(connection: &Connection, id: i64) -> Result<AccountStatus> {
    let status = connection.query_row(
        "SELECT account.status FROM conversations c
         LEFT JOIN peer_account_status account ON account.void_id = c.peer_void_id
         WHERE c.id = ?1",
        [id],
        |row| row.get::<_, Option<String>>(0),
    )?;
    decode(status.as_deref())
}

fn decode(status: Option<&str>) -> Result<AccountStatus> {
    match status {
        None => Ok(AccountStatus::Unknown),
        Some("active") => Ok(AccountStatus::Active),
        Some("deleted") => Ok(AccountStatus::Deleted),
        Some(_) => Err(Error::Unreadable),
    }
}
