//! Read-only presentation metadata, resolved independently of the loaded window.
use super::{Database, Result, message_events::MessageReference};
use rusqlite::{OptionalExtension, params};

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use crate::storage::{DeliveryState, Direction, Message, Owner, tests::TestDir};

    #[test]
    fn reaction_counts_and_local_highlight_refresh_without_leaking_tombstoned_content() {
        let root = TestDir::new();
        let mut db = Database::open(
            &root.0,
            Owner {
                void_id: "41",
                public_key: &[5; 32],
            },
        )
        .unwrap();
        let chat = db.create_conversation("123", 100).unwrap();
        let id = "00000000-0000-4000-8000-000000000001";
        db.commit_incoming("123", id, &[7; 32], |_| {
            Ok(Message {
                id: "opaque-original-id".into(),
                conversation_id: chat,
                direction: Direction::Incoming,
                body: "Never show after deletion".into(),
                created_at: 110,
                delivery_state: DeliveryState::Received,
            })
        })
        .unwrap();
        db.connection.execute_batch("INSERT INTO message_reactions VALUES
          ('123','123','00000000-0000-4000-8000-000000000001','41','👍',1,2,'00000000-0000-4000-8000-000000000002'),
          ('123','123','00000000-0000-4000-8000-000000000001','123','👍',1,1,'00000000-0000-4000-8000-000000000003');").unwrap();
        let key = db.timeline_before(chat, None, 1).unwrap()[0].cursor.key();
        assert_eq!(
            db.message_presentation(chat, &key).unwrap().reactions,
            vec![("👍".into(), 2, true)]
        );
        db.connection
            .execute(
                "UPDATE message_reactions SET active=0,revision=3 WHERE reactor_void_id='41'",
                [],
            )
            .unwrap();
        assert_eq!(
            db.message_presentation(chat, &key).unwrap().reactions,
            vec![("👍".into(), 1, false)]
        );
        db.connection.execute("INSERT INTO message_tombstones VALUES ('123','123',?1,'00000000-0000-4000-8000-000000000004')",[id]).unwrap();
        let message = db.timeline_before(chat, None, 1).unwrap().remove(0);
        assert_eq!(message.body, "Message deleted");
        assert!(message.presentation.reactions.is_empty());
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Presentation {
    pub deleted: bool,
    pub quote: String,
    pub author: String,
    pub target: String,
    pub reactions: Vec<(String, i32, bool)>,
}

impl Database {
    pub(crate) fn message_reference(
        &self,
        conversation: i64,
        key: &str,
    ) -> Result<Option<MessageReference>> {
        let Some(cursor) = super::messaging::TimelineCursor::from_key(key) else {
            return Ok(None);
        };
        let mut parts = key.splitn(3, ':');
        parts.next();
        let source = parts.next().unwrap_or_default();
        let id = parts.next().unwrap_or_default();
        let reference = if source == "o" {
            self.connection.query_row("SELECT (SELECT void_id FROM local_account), event_id FROM outgoing_operations WHERE event_id=?1 AND conversation_id=?2",params![id,conversation],|r|Ok(MessageReference{author_void_id:r.get(0)?,event_id:r.get(1)?})).optional()?
        } else {
            self.connection.query_row("SELECT i.sender_void_id,i.event_id FROM inbox i JOIN messages m ON m.id=i.message_id WHERE m.id=?1 AND m.conversation_id=?2 UNION ALL SELECT (SELECT void_id FROM local_account),b.event_id FROM outbox b JOIN messages m ON m.id=b.message_id WHERE m.id=?1 AND m.conversation_id=?2",params![id,conversation],|r|Ok(MessageReference{author_void_id:r.get(0)?,event_id:r.get(1)?})).optional()?
        };
        Ok(reference.filter(|_| cursor.key() == key))
    }

    pub(crate) fn reply_draft_preview(&self, conversation: i64) -> Result<(String, String)> {
        let Some(target) = self.reply_draft(conversation)? else {
            return Ok((String::new(), String::new()));
        };
        let author = if target.author_void_id == super::outgoing::local(&self.connection)? {
            "You".into()
        } else {
            self.conversation(conversation)?
                .display_name
                .filter(|v| !v.trim().is_empty())
                .unwrap_or_else(|| "Contact".into())
        };
        let peer = self.conversation(conversation)?.peer_void_id;
        let text = if self.message_deleted(&peer, &target)? {
            "Message deleted".into()
        } else if let Some(key) = self.reference_cursor(conversation, &target)? {
            let mut parts = key.splitn(3, ':');
            parts.next();
            let source = parts.next().unwrap_or_default();
            let id = parts.next().unwrap_or_default();
            self.connection
                .query_row(
                    if source == "o" {
                        "SELECT body FROM outgoing_operations WHERE event_id=?1"
                    } else {
                        "SELECT body FROM messages WHERE id=?1"
                    },
                    [id],
                    |r| r.get::<_, String>(0),
                )?
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
                .chars()
                .take(120)
                .collect()
        } else {
            "Message unavailable".into()
        };
        Ok((author, text))
    }
    pub(crate) fn reference_cursor(
        &self,
        conversation: i64,
        target: &MessageReference,
    ) -> Result<Option<String>> {
        if super::outgoing::schema_ready(&self.connection)? && self.connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM local_message_hides WHERE conversation_id=?1 AND author_void_id=?2 AND event_id=?3)",
            params![conversation,target.author_void_id,target.event_id],|r|r.get::<_,bool>(0))? { return Ok(None); }
        let local: String =
            self.connection
                .query_row("SELECT void_id FROM local_account", [], |r| r.get(0))?;
        Ok(self
            .connection
            .query_row(
                "SELECT created_at || ':' || source || ':' || sort_id FROM (
             SELECT o.created_at, 'o' source, o.event_id sort_id FROM outgoing_operations o
             WHERE o.conversation_id=?1 AND o.event_id=?3 AND ?2=?4
             UNION ALL
             SELECT m.created_at, 'm', m.id FROM messages m WHERE m.conversation_id=?1 AND m.id IN (
               SELECT message_id FROM inbox WHERE sender_void_id=?2 AND event_id=?3
               UNION ALL SELECT message_id FROM outbox WHERE event_id=?3 AND ?2=?4)
             ) LIMIT 1",
                params![conversation, target.author_void_id, target.event_id, local],
                |r| r.get(0),
            )
            .optional()?)
    }

    pub(crate) fn message_presentation(
        &self,
        conversation: i64,
        key: &str,
    ) -> Result<Presentation> {
        let mut result = Presentation::default();
        let mut parts = key.splitn(3, ':');
        parts.next();
        let source = parts.next().unwrap_or_default();
        let id = parts.next().unwrap_or_default();
        let peer = self.conversation(conversation)?;
        let local: String =
            self.connection
                .query_row("SELECT void_id FROM local_account", [], |r| r.get(0))?;
        let message_id: Option<String> = if source == "o" {
            self.connection
                .query_row(
                    "SELECT message_id FROM outbox WHERE event_id=?1",
                    [id],
                    |r| r.get(0),
                )
                .optional()?
        } else {
            Some(id.into())
        };
        let reference: Option<MessageReference> = if source == "o" {
            Some(MessageReference {
                author_void_id: local.clone(),
                event_id: id.into(),
            })
        } else {
            self.connection.query_row("SELECT sender_void_id,event_id FROM inbox WHERE message_id=?1 UNION ALL SELECT ?2,event_id FROM outbox WHERE message_id=?1",params![id,local],|r|Ok(MessageReference{author_void_id:r.get(0)?,event_id:r.get(1)?})).optional()?
        };
        if let Some(target) = &reference {
            result.deleted = self.message_deleted(&peer.peer_void_id, target)?;
        }
        if result.deleted {
            return Ok(result);
        }
        let reply = if source == "o" {
            self.outgoing_reply(id)?
        } else {
            None
        }
        .or(match message_id {
            Some(id) => self.reply_reference(&id)?,
            None => None,
        });
        if let Some(target) = reply {
            result.author = if target.author_void_id == local {
                "You".into()
            } else {
                peer.display_name
                    .clone()
                    .filter(|s| !s.trim().is_empty())
                    .unwrap_or_else(|| "Contact".into())
            };
            result.target = self
                .reference_cursor(conversation, &target)?
                .unwrap_or_default();
            result.quote = if self.message_deleted(&peer.peer_void_id, &target)? {
                "Message deleted".into()
            } else if result.target.is_empty() {
                "Message unavailable".into()
            } else {
                let mut parts = result.target.splitn(3, ':');
                parts.next();
                let kind = parts.next().unwrap_or_default();
                let id = parts.next().unwrap_or_default();
                let body: String = if kind == "o" {
                    self.connection.query_row(
                        "SELECT body FROM outgoing_operations WHERE event_id=?1",
                        [id],
                        |r| r.get(0),
                    )?
                } else {
                    self.connection.query_row(
                        "SELECT body FROM messages WHERE id=?1",
                        [id],
                        |r| r.get(0),
                    )?
                };
                body.split_whitespace()
                    .collect::<Vec<_>>()
                    .join(" ")
                    .chars()
                    .take(120)
                    .collect()
            };
        }
        if let Some(target) = reference {
            let mut query=self.connection.prepare("SELECT emoji,count(*),max(reactor_void_id=?4) FROM message_reactions WHERE peer_void_id=?1 AND original_author_void_id=?2 AND original_event_id=?3 AND active=1 GROUP BY emoji ORDER BY emoji")?;
            result.reactions = query
                .query_map(
                    params![
                        peer.peer_void_id,
                        target.author_void_id,
                        target.event_id,
                        local
                    ],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                )?
                .collect::<std::result::Result<Vec<_>, _>>()?;
        }
        Ok(result)
    }
}
