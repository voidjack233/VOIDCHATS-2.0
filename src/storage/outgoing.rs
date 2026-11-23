//! Durable feature intents used by the protocol-gated message action worker.
use super::{
    Database, Error, Result,
    account_status::{self, AccountStatus},
    message_events::{self, MessageReference},
    messaging::new_event_id,
};
use rusqlite::{Connection, OptionalExtension, Transaction, params};

// Keep immutable submission records for idempotency. Only never-encrypted,
// never-attempted reactions may yield to a newer durable revision. The current
// reaction and control primary keys avoid scanning the full control history.
const SUPERSEDED_REACTION: &str = "o.kind='reaction' AND o.state='pending' AND o.payload IS NULL
    AND EXISTS (SELECT 1 FROM message_reactions r
        JOIN outgoing_controls n ON n.event_id=r.source_event_id
        WHERE r.peer_void_id=(SELECT peer_void_id FROM conversations WHERE id=o.conversation_id)
        AND r.original_author_void_id=o.author_void_id AND r.original_event_id=o.original_event_id
        AND r.reactor_void_id=(SELECT void_id FROM local_account) AND r.emoji=o.emoji
        AND r.revision>o.revision AND n.revision=r.revision AND n.kind='reaction'
        AND n.conversation_id=o.conversation_id AND n.author_void_id=o.author_void_id
        AND n.original_event_id=o.original_event_id AND n.emoji=o.emoji)";

pub(super) fn control_superseded(connection: &Connection, id: &str) -> Result<bool> {
    Ok(connection.query_row(
        &format!("SELECT EXISTS(SELECT 1 FROM outgoing_controls o WHERE o.event_id=?1 AND ({SUPERSEDED_REACTION}))"),
        [id], |row| row.get(0),
    )?)
}

#[derive(Clone)]
#[allow(dead_code)] // Persisted operation metadata; never logged.
pub(crate) struct Control {
    pub id: String,
    pub conversation: i64,
    pub peer: String,
    pub kind: String,
    pub target: MessageReference,
    pub emoji: Option<String>,
    pub active: Option<bool>,
    pub revision: Option<i64>,
    pub claim_id: String,
    pub payload: Option<Vec<u8>>,
    pub state: String,
}

pub(super) fn local(connection: &Connection) -> Result<String> {
    Ok(connection.query_row("SELECT void_id FROM local_account", [], |r| r.get(0))?)
}
pub(super) fn schema_ready(connection: &Connection) -> Result<bool> {
    Ok(connection.pragma_query_value(None, "user_version", |r| r.get::<_, i64>(0))? >= 12)
}
pub(super) fn validate_target(
    tx: &Transaction<'_>,
    conversation: i64,
    target: &MessageReference,
    require_visible: bool,
) -> Result<String> {
    let peer: String = tx.query_row(
        "SELECT peer_void_id FROM conversations WHERE id=?1",
        [conversation],
        |r| r.get(0),
    )?;
    let own = local(tx)?;
    if !message_events::reference_valid(target, &own, &peer, "") {
        return Err(Error::SignalEnvelope);
    }
    let known:bool=tx.query_row("SELECT EXISTS (
       SELECT 1 FROM inbox i JOIN messages m ON m.id=i.message_id WHERE i.sender_void_id=?2 AND i.event_id=?3 AND m.conversation_id=?1
       UNION ALL SELECT 1 FROM outbox b JOIN messages m ON m.id=b.message_id WHERE b.event_id=?3 AND ?2=?4 AND m.conversation_id=?1
       UNION ALL SELECT 1 FROM outgoing_operations WHERE event_id=?3 AND ?2=?4 AND conversation_id=?1)",params![conversation,target.author_void_id,target.event_id,own],|r|r.get(0))?;
    if !known {
        return Err(Error::SignalEnvelope);
    }
    if require_visible
        && (message_events::tombstoned(tx, &peer, target)? || hidden(tx, conversation, target)?)
    {
        return Err(Error::EventConflict);
    }
    Ok(peer)
}
fn hidden(c: &Connection, conversation: i64, target: &MessageReference) -> Result<bool> {
    Ok(c.query_row("SELECT EXISTS(SELECT 1 FROM local_message_hides WHERE conversation_id=?1 AND author_void_id=?2 AND event_id=?3)",params![conversation,target.author_void_id,target.event_id],|r|r.get(0))?)
}
pub(super) fn queued_reply(c: &Connection, id: &str) -> Result<Option<MessageReference>> {
    if !schema_ready(c)? {
        return Ok(None);
    }
    Ok(c.query_row(
        "SELECT author_void_id,original_event_id FROM outgoing_reply_refs WHERE event_id=?1",
        [id],
        |r| {
            Ok(MessageReference {
                author_void_id: r.get(0)?,
                event_id: r.get(1)?,
            })
        },
    )
    .optional()?)
}
fn unused_id(tx: &Transaction<'_>, id: &str) -> Result<()> {
    let used:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM outgoing_operations WHERE event_id=?1 UNION ALL SELECT 1 FROM outbox WHERE event_id=?1 UNION ALL SELECT 1 FROM receipt_outbox WHERE event_id=?1)",[id],|r|r.get(0))?;
    if used {
        return Err(Error::EventConflict);
    }
    Ok(())
}

#[allow(dead_code)] // Storage API includes explicit recovery/inspection operations.
impl Database {
    pub(crate) fn local_void_id(&self) -> Result<String> {
        local(&self.connection)
    }
    pub(crate) fn reply_draft(&self, conversation: i64) -> Result<Option<MessageReference>> {
        let target = self
            .connection
            .query_row(
                "SELECT author_void_id,event_id FROM draft_reply_refs WHERE conversation_id=?1",
                [conversation],
                |r| {
                    Ok(MessageReference {
                        author_void_id: r.get(0)?,
                        event_id: r.get(1)?,
                    })
                },
            )
            .optional()?;
        // Keep the draft text, but stop quoting a remotely deleted original.
        if let Some(target) = &target
            && self.message_deleted(&self.conversation(conversation)?.peer_void_id, target)?
        {
            return Ok(None);
        }
        Ok(target)
    }
    pub(crate) fn save_reply_draft(
        &mut self,
        conversation: i64,
        body: &str,
        reply: Option<&MessageReference>,
    ) -> Result<()> {
        self.write(|tx| {
            if let Some(target)=reply {validate_target(tx,conversation,target,true)?;}
            tx.execute("INSERT INTO drafts VALUES (?1,?2) ON CONFLICT(conversation_id) DO UPDATE SET body=excluded.body",params![conversation,body])?;
            tx.execute("DELETE FROM draft_reply_refs WHERE conversation_id=?1",[conversation])?;
            if let Some(target)=reply {tx.execute("INSERT INTO draft_reply_refs VALUES (?1,?2,?3)",params![conversation,target.author_void_id,target.event_id])?;}
            Ok(())
        })
    }
    pub(crate) fn outgoing_reply(&self, id: &str) -> Result<Option<MessageReference>> {
        queued_reply(&self.connection, id)
    }
    pub(crate) fn outgoing_message_blocked(&self, id: &str) -> Result<bool> {
        if !schema_ready(&self.connection)? {
            return Ok(false);
        }
        let own = local(&self.connection)?;
        Ok(self.connection.query_row("SELECT EXISTS (
          SELECT 1 FROM local_message_hides WHERE author_void_id=?2 AND event_id=?1
          UNION ALL SELECT 1 FROM message_tombstones WHERE original_author_void_id=?2 AND original_event_id=?1)",params![id,own],|r|r.get(0))?)
    }
    pub(crate) fn pending_controls(&self) -> Result<Vec<String>> {
        Ok(self.connection.prepare(&format!("SELECT o.event_id FROM outgoing_controls o WHERE o.state<>'accepted' AND NOT ({SUPERSEDED_REACTION}) ORDER BY o.created_at,o.event_id"))?.query_map([],|r|r.get(0))?.collect::<std::result::Result<Vec<_>,_>>()?)
    }
    pub(crate) fn control_superseded(&self, id: &str) -> Result<bool> {
        control_superseded(&self.connection, id)
    }
    pub(crate) fn control(&self, id: &str) -> Result<Control> {
        Ok(self.connection.query_row("SELECT o.event_id,o.conversation_id,c.peer_void_id,o.kind,o.author_void_id,o.original_event_id,o.emoji,o.active,o.revision,o.claim_id,o.payload,o.state FROM outgoing_controls o JOIN conversations c ON c.id=o.conversation_id WHERE o.event_id=?1",[id],|r|Ok(Control{id:r.get(0)?,conversation:r.get(1)?,peer:r.get(2)?,kind:r.get(3)?,target:MessageReference{author_void_id:r.get(4)?,event_id:r.get(5)?},emoji:r.get(6)?,active:r.get(7)?,revision:r.get(8)?,claim_id:r.get(9)?,payload:r.get(10)?,state:r.get(11)?}))?)
    }
    pub(crate) fn set_control_state(&self, id: &str, state: &str) -> Result<()> {
        if !["unconfirmed", "failed", "accepted"].contains(&state) {
            return Err(Error::EventConflict);
        }
        if self.connection.execute(
            "UPDATE outgoing_controls SET state=?2 WHERE event_id=?1 AND state<>'accepted'",
            params![id, state],
        )? != 1
        {
            return Err(Error::EventConflict);
        }
        Ok(())
    }
    pub(crate) fn renew_control_claim(&self, id: &str) -> Result<()> {
        self.connection.execute(
            "UPDATE outgoing_controls SET claim_id=?2 WHERE event_id=?1 AND payload IS NULL",
            params![id, new_event_id()?],
        )?;
        Ok(())
    }
    /// Caller retains submission UUID across retries. Reusing it for a different
    /// intent fails; repeating an already-intended state does not allocate a revision.
    pub(crate) fn queue_reaction(
        &mut self,
        conversation: i64,
        submission: &str,
        target: &MessageReference,
        emoji: &str,
        active: bool,
        created_at: i64,
    ) -> Result<String> {
        if !super::signal::valid_event_id(submission)
            || !message_events::valid_reaction_token(emoji)
            || created_at < 0
        {
            return Err(Error::SignalEnvelope);
        }
        self.write(|tx| {
            let prior:Option<(i64,String,String,String,Option<String>,Option<bool>)>=tx.query_row("SELECT conversation_id,kind,author_void_id,original_event_id,emoji,active FROM outgoing_controls WHERE event_id=?1",[submission],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?))).optional()?;
            if let Some((c,k,a,e,em,on))=prior {return if c==conversation && k=="reaction" && a==target.author_void_id && e==target.event_id && em.as_deref()==Some(emoji) && on==Some(active) {Ok(submission.into())} else {Err(Error::EventConflict)};}
            unused_id(tx,submission)?;
            let peer=validate_target(tx,conversation,target,true)?;
            if account_status::conversation_status(tx,conversation)?==AccountStatus::Deleted {return Err(Error::PeerDeleted);}
            let own=local(tx)?;
            let current:Option<(bool,i64,String)>=tx.query_row("SELECT active,revision,source_event_id FROM message_reactions WHERE peer_void_id=?1 AND original_author_void_id=?2 AND original_event_id=?3 AND reactor_void_id=?4 AND emoji=?5",params![peer,target.author_void_id,target.event_id,own,emoji],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional()?;
            if let Some((on,_,id))=&current && *on==active && tx.query_row("SELECT EXISTS(SELECT 1 FROM outgoing_controls WHERE event_id=?1)",[id],|r|r.get::<_,bool>(0))? {return Ok(id.clone());}
            if !active && current.is_none() {return Err(Error::EventConflict);}
            let exists:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM message_reactions WHERE peer_void_id=?1 AND original_author_void_id=?2 AND original_event_id=?3 AND emoji=?4 AND active=1)",params![peer,target.author_void_id,target.event_id,emoji],|r|r.get(0))?;
            let count:i64=tx.query_row("SELECT count(DISTINCT emoji) FROM message_reactions WHERE peer_void_id=?1 AND original_author_void_id=?2 AND original_event_id=?3 AND active=1",params![peer,target.author_void_id,target.event_id],|r|r.get(0))?;
            if active && !exists && count>=10 {return Err(Error::ReactionLimit);}
            let revision=current.map_or(Some(1),|(_,rev,_)|rev.checked_add(1)).ok_or(Error::EventConflict)?;
            tx.execute("INSERT INTO outgoing_controls(event_id,conversation_id,kind,author_void_id,original_event_id,emoji,active,revision,created_at,claim_id) VALUES (?1,?2,'reaction',?3,?4,?5,?6,?7,?8,?9)",params![submission,conversation,target.author_void_id,target.event_id,emoji,active,revision,created_at,new_event_id()?])?;
            tx.execute("INSERT INTO message_reactions VALUES (?1,?2,?3,?4,?5,?6,?7,?8) ON CONFLICT(peer_void_id,original_author_void_id,original_event_id,reactor_void_id,emoji) DO UPDATE SET active=excluded.active,revision=excluded.revision,source_event_id=excluded.source_event_id",params![peer,target.author_void_id,target.event_id,own,emoji,active,revision,submission])?;
            Ok(submission.into())
        })
    }
    pub(crate) fn delete_for_me(
        &mut self,
        conversation: i64,
        target: &MessageReference,
    ) -> Result<()> {
        self.write(|tx| {
            validate_target(tx,conversation,target,false)?;
            let own=local(tx)?;
            let message:Option<String>=tx.query_row("SELECT message_id FROM inbox WHERE sender_void_id=?1 AND event_id=?2 UNION ALL SELECT message_id FROM outbox WHERE event_id=?2 AND ?1=?3",params![target.author_void_id,target.event_id,own],|r|r.get(0)).optional()?;
            tx.execute("INSERT INTO local_message_hides VALUES (?1,?2,?3,?4) ON CONFLICT DO NOTHING",params![conversation,target.author_void_id,target.event_id,message])?;
            tx.execute("DELETE FROM unread_messages WHERE message_id=?1",[message])?;Ok(())
        })
    }
    pub(crate) fn queue_delete_everyone(
        &mut self,
        conversation: i64,
        submission: &str,
        target: &MessageReference,
        created_at: i64,
    ) -> Result<String> {
        if !super::signal::valid_event_id(submission) || created_at < 0 {
            return Err(Error::SignalEnvelope);
        }
        self.write(|tx| {
            let own=local(tx)?;
            if target.author_void_id!=own {return Err(Error::SignalPeer);}
            let peer=validate_target(tx,conversation,target,false)?;
            let prior:Option<(String,i64,String,String)>=tx.query_row("SELECT kind,conversation_id,author_void_id,original_event_id FROM outgoing_controls WHERE event_id=?1",[submission],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).optional()?;
            if let Some((kind,c,a,e))=prior {return if kind=="delete_for_everyone" && c==conversation && a==own && e==target.event_id {Ok(submission.into())} else {Err(Error::EventConflict)};}
            unused_id(tx,submission)?;
            let prior:Option<String>=tx.query_row("SELECT event_id FROM outgoing_controls WHERE conversation_id=?1 AND kind='delete_for_everyone' AND author_void_id=?2 AND original_event_id=?3",params![conversation,own,target.event_id],|r|r.get(0)).optional()?;
            if let Some(id)=prior {return Ok(id);}
            tx.execute("INSERT INTO outgoing_controls(event_id,conversation_id,kind,author_void_id,original_event_id,created_at,claim_id) VALUES (?1,?2,'delete_for_everyone',?3,?4,?5,?6)",params![submission,conversation,own,target.event_id,created_at,new_event_id()?])?;
            tx.execute("INSERT INTO message_tombstones VALUES (?1,?2,?3,?4) ON CONFLICT DO NOTHING",params![peer,own,target.event_id,submission])?;
            tx.execute("UPDATE messages SET body='' WHERE id=(SELECT message_id FROM outbox WHERE event_id=?1)",[&target.event_id])?;
            tx.execute("UPDATE outgoing_operations SET body='' WHERE event_id=?1",[&target.event_id])?;
            tx.execute("UPDATE message_reactions SET active=0 WHERE peer_void_id=?1 AND original_author_void_id=?2 AND original_event_id=?3",params![peer,own,target.event_id])?;
            Ok(submission.into())
        })
    }
}
