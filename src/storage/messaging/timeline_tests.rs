use super::*;
use crate::storage::{Direction, Message, Owner, events::Incoming, tests::TestDir};

const OWNER: Owner<'_> = Owner {
    void_id: "41",
    public_key: &[5; 32],
};

fn keys(messages: &[TimelineMessage]) -> Vec<String> {
    messages
        .iter()
        .map(|message| message.cursor.key())
        .collect()
}

#[test]
fn pages_are_contiguous_and_bounded_across_equal_timestamps_and_sources() {
    let root = TestDir::new();
    let mut db = Database::open(&root.0, OWNER).unwrap();
    let conversation = db.create_conversation("123", 100).unwrap();
    let other = db.create_conversation("456", 100).unwrap();
    let expected = db
        .write(|tx| {
            let mut keys = Vec::new();
            for n in 0..80 {
                let created_at = 120 + n / 4;
                let id = format!("123:{n:03}");
                tx.execute(
                    "INSERT INTO messages (id, conversation_id, direction, body, created_at, delivery_state)
                     VALUES (?1, ?2, 0, 'Saved', ?3, 0)",
                    params![id, conversation, created_at],
                )?;
                tx.execute(
                    "INSERT INTO outgoing_operations (event_id, conversation_id, body, created_at, state)
                     VALUES (?1, ?2, 'Pending', ?3, 'pending')",
                    params![id, conversation, created_at],
                )?;
                keys.push(format!("{created_at}:m:{id}"));
                keys.push(format!("{created_at}:o:{id}"));
            }
            tx.execute(
                "INSERT INTO messages (id, conversation_id, direction, body, created_at, delivery_state)
                 VALUES ('other', ?1, 0, 'Other chat', 999, 0)",
                [other],
            )?;
            Ok(keys)
        })
        .unwrap();

    let mut end = expected.len();
    let mut cursor = None;
    while end > 0 {
        let page = db
            .timeline_before(conversation, cursor.as_ref(), 9)
            .unwrap();
        let start = end.saturating_sub(9);
        assert_eq!(keys(&page), expected[start..end]);
        let first = &page[0].cursor;
        assert_eq!(TimelineCursor::from_key(&first.key()).as_ref(), Some(first));
        cursor = Some(first.clone());
        end = start;
    }
    assert!(
        db.timeline_before(conversation, cursor.as_ref(), 9)
            .unwrap()
            .is_empty()
    );

    // A page from saved reading position must include that row, while next omits it.
    // Both possible sources must handle equality correctly.
    for index in [44, 45, expected.len() - 1] {
        let cursor = TimelineCursor::from_key(&expected[index]).unwrap();
        assert_eq!(
            keys(&db.timeline_from(conversation, &cursor, 12).unwrap()),
            expected[index..(index + 12).min(expected.len())],
        );
        assert_eq!(
            keys(&db.timeline_after(conversation, &cursor, 12).unwrap()),
            expected[index + 1..(index + 13).min(expected.len())],
        );
    }
}

#[test]
fn an_outgoing_retry_and_verified_receipt_keep_one_timeline_identity() {
    let root = TestDir::new();
    let mut db = Database::open(&root.0, OWNER).unwrap();
    let conversation = db.create_conversation("123", 100).unwrap();
    let (event_id, _) = db.queue_outgoing(conversation, "Outgoing", 120).unwrap();
    let pending = db.timeline_before(conversation, None, 2).unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].status, "Pending");
    let cursor = pending[0].cursor.clone();

    db.set_send_state(&event_id, SendState::Failed).unwrap();
    let failed = db.timeline_from(conversation, &cursor, 2).unwrap();
    assert_eq!(failed.len(), 1);
    assert_eq!(failed[0].cursor, cursor);
    assert_eq!(failed[0].retry_event, event_id);

    db.commit_outgoing(conversation, &event_id, true, |_| {
        Ok((
            Message {
                id: format!("41:{event_id}"),
                conversation_id: conversation,
                direction: Direction::Outgoing,
                body: "Outgoing".into(),
                created_at: 120,
                delivery_state: DeliveryState::Pending,
            },
            vec![1, 2, 3],
        ))
    })
    .unwrap();
    db.set_send_state(&event_id, SendState::Sent).unwrap();
    let sent = db.timeline_from(conversation, &cursor, 2).unwrap();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].cursor, cursor);
    assert_eq!(sent[0].status, "Sent to relay");

    db.commit_signal_incoming("123", &new_event_id().unwrap(), &[7; 32], |_| {
        Ok(Incoming::Receipt {
            original_event_id: event_id.clone(),
        })
    })
    .unwrap();
    let delivered = db.timeline_before(conversation, None, 2).unwrap();
    assert_eq!(delivered.len(), 1);
    assert_eq!(delivered[0].cursor, cursor);
    assert_eq!(delivered[0].status, "Delivered");
    assert!(delivered[0].retry_event.is_empty());
}
