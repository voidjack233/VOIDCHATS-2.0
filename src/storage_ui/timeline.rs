use crate::{AppWindow, storage};
use std::collections::HashMap;
use storage::messaging::{TimelineCursor, TimelineMessage};

const RECENT_MESSAGES: usize = 60;
const PAGE_MESSAGES: usize = 40;
const MAX_MESSAGES: usize = 120;

pub(super) struct ReadingPosition {
    conversation: i64,
    anchor: Option<Anchor>,
}

#[derive(Clone)]
pub(super) struct Anchor {
    cursor: TimelineCursor,
    offset: f32,
}

impl ReadingPosition {
    pub(super) fn capture(ui: &AppWindow) -> Option<Self> {
        let conversation = ui.get_selected_conversation().parse().ok()?;
        let anchor = if ui.get_timeline_following() && !ui.get_timeline_has_newer() {
            None
        } else {
            Some(Anchor {
                cursor: TimelineCursor::from_key(&ui.get_timeline_anchor_key())?,
                offset: ui.get_timeline_anchor_offset(),
            })
        };
        Some(Self {
            conversation,
            anchor,
        })
    }
}

impl Anchor {
    pub(super) fn restore(&self, ui: &AppWindow) {
        ui.set_timeline_anchor_key(self.cursor.key().into());
        ui.set_timeline_anchor_offset(self.offset);
        ui.set_timeline_following(false);
    }
}

#[derive(Default)]
pub(super) struct History {
    positions: HashMap<i64, Anchor>,
    pub(super) active: Option<ActiveTimeline>,
}

impl History {
    pub(super) fn remember(&mut self, position: Option<ReadingPosition>) {
        if let Some(ReadingPosition {
            conversation,
            anchor,
        }) = position
        {
            if let Some(anchor) = anchor {
                self.positions.insert(conversation, anchor);
            } else {
                self.positions.remove(&conversation);
            }
        }
    }

    pub(super) fn open(
        &mut self,
        db: &storage::Database,
        conversation: i64,
        selection: i32,
    ) -> storage::Result<(TimelineWindow, Option<Anchor>)> {
        let mut anchor = self.positions.get(&conversation).cloned();
        let around = if let Some(saved) = &anchor {
            TimelineWindow::around(db, conversation, &saved.cursor)?
        } else {
            None
        };
        let window = match around {
            Some(window) => window,
            None => {
                anchor = None;
                TimelineWindow::latest(db, conversation)?
            }
        };
        self.active = Some(ActiveTimeline {
            conversation,
            selection,
            window: window.clone(),
        });
        Ok((window, anchor))
    }

    pub(super) fn current(
        &mut self,
        conversation: i64,
        selection: i32,
    ) -> Option<&mut ActiveTimeline> {
        self.active
            .as_mut()
            .filter(|view| view.conversation == conversation && view.selection == selection)
    }
}

pub(super) struct ActiveTimeline {
    pub(super) conversation: i64,
    pub(super) selection: i32,
    window: TimelineWindow,
}

impl ActiveTimeline {
    pub(super) fn page(
        &mut self,
        db: &storage::Database,
        older: bool,
    ) -> storage::Result<TimelineWindow> {
        self.window.page(db, self.conversation, older)?;
        Ok(self.window.clone())
    }

    pub(super) fn present(&mut self, db: &storage::Database) -> storage::Result<TimelineWindow> {
        self.window = TimelineWindow::latest(db, self.conversation)?;
        Ok(self.window.clone())
    }

    pub(super) fn restore(
        &mut self,
        db: &storage::Database,
        cursor: &TimelineCursor,
    ) -> storage::Result<Option<TimelineWindow>> {
        let Some(window) = TimelineWindow::around(db, self.conversation, cursor)? else {
            return Ok(None);
        };
        self.window = window.clone();
        Ok(Some(window))
    }

    pub(super) fn refresh(&mut self, db: &storage::Database) -> storage::Result<TimelineWindow> {
        self.window = self.window.refresh(db, self.conversation)?;
        Ok(self.window.clone())
    }
}

#[derive(Clone)]
pub(super) struct TimelineWindow {
    pub(super) messages: Vec<TimelineMessage>,
    pub(super) has_older: bool,
    pub(super) has_newer: bool,
}

impl TimelineWindow {
    fn latest(db: &storage::Database, id: i64) -> storage::Result<Self> {
        let mut messages = db.timeline_before(id, None, RECENT_MESSAGES + 1)?;
        let has_older = messages.len() > RECENT_MESSAGES;
        if has_older {
            messages.remove(0);
        }
        Ok(Self {
            messages,
            has_older,
            has_newer: false,
        })
    }

    fn around(
        db: &storage::Database,
        id: i64,
        cursor: &TimelineCursor,
    ) -> storage::Result<Option<Self>> {
        let mut after = db.timeline_from(id, cursor, RECENT_MESSAGES + 1)?;
        if after.first().is_none_or(|row| row.cursor != *cursor) {
            return Ok(None);
        }
        let has_newer = after.len() > RECENT_MESSAGES;
        after.truncate(RECENT_MESSAGES);
        let mut messages = db.timeline_before(id, Some(cursor), PAGE_MESSAGES + 1)?;
        let has_older = messages.len() > PAGE_MESSAGES;
        if has_older {
            messages.remove(0);
        }
        messages.append(&mut after);
        Ok(Some(Self {
            messages,
            has_older,
            has_newer,
        }))
    }

    fn page(&mut self, db: &storage::Database, id: i64, older: bool) -> storage::Result<()> {
        if older
            && self.has_older
            && let Some(first) = self.messages.first()
        {
            let mut page = db.timeline_before(id, Some(&first.cursor), PAGE_MESSAGES + 1)?;
            self.has_older = page.len() > PAGE_MESSAGES;
            if self.has_older {
                page.remove(0);
            }
            page.append(&mut self.messages);
            self.has_newer |= page.len() > MAX_MESSAGES;
            page.truncate(MAX_MESSAGES);
            self.messages = page;
        } else if !older
            && self.has_newer
            && let Some(last) = self.messages.last()
        {
            let mut page = db.timeline_after(id, &last.cursor, PAGE_MESSAGES + 1)?;
            self.has_newer = page.len() > PAGE_MESSAGES;
            page.truncate(PAGE_MESSAGES);
            self.messages.append(&mut page);
            let excess = self.messages.len().saturating_sub(MAX_MESSAGES);
            self.has_older |= excess > 0;
            self.messages.drain(..excess);
        }
        Ok(())
    }

    fn refresh(&self, db: &storage::Database, id: i64) -> storage::Result<Self> {
        let (Some(first), Some(last)) = (self.messages.first(), self.messages.last()) else {
            return Self::latest(db, id);
        };
        let mut messages = db.timeline_from(id, &first.cursor, MAX_MESSAGES + 1)?;
        // When reading history, update within the current range. Newer messages are
        // available through paging or Jump to Present without implying adjacency.
        let has_newer = messages.len() > MAX_MESSAGES
            || (self.has_newer && messages.last().is_some_and(|row| row.cursor > last.cursor));
        if self.has_newer {
            messages.retain(|row| row.cursor <= last.cursor);
        }
        messages.truncate(MAX_MESSAGES);
        let has_older = if self.has_older {
            true
        } else {
            // A newly received message may predate the first saved message we showed.
            let room = MAX_MESSAGES - messages.len();
            let mut preceding = db.timeline_before(id, Some(&first.cursor), room + 1)?;
            let have_more = preceding.len() > room;
            if have_more {
                preceding.remove(0);
            }
            preceding.append(&mut messages);
            messages = preceding;
            have_more
        };
        Ok(Self {
            messages,
            has_older,
            has_newer,
        })
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use storage::{Database, DeliveryState, Direction, Message, Owner, tests::TestDir};

    fn open(root: &TestDir) -> Database {
        Database::open(
            &root.0,
            Owner {
                void_id: "41",
                public_key: &[5; 32],
            },
        )
        .unwrap()
    }

    fn save(db: &mut Database, id: i64, timestamp: i64) {
        db.save_message(&Message {
            id: format!("{id}:{timestamp}"),
            conversation_id: id,
            body: timestamp.to_string(),
            created_at: timestamp,
            direction: Direction::Incoming,
            delivery_state: DeliveryState::Received,
        })
        .unwrap();
    }

    fn assert_range(window: &TimelineWindow, start: i64, end: i64) {
        assert!(window.messages.len() <= MAX_MESSAGES);
        assert!(
            window
                .messages
                .iter()
                .map(|row| row.created_at)
                .eq(start..=end)
        );
    }

    #[test]
    fn history_stays_bounded_contiguous_and_restores_a_session_position() {
        let root = TestDir::new();
        let mut db = open(&root);
        let id = db.create_conversation("123", 1).unwrap();
        let other = db.create_conversation("456", 1).unwrap();
        for time in 1..=320 {
            save(&mut db, id, time);
        }
        let mut history = History::default();
        let (latest, initial) = history.open(&db, id, 1).unwrap();
        assert_range(&latest, 261, 320);
        assert!(latest.has_older && !latest.has_newer && initial.is_none());

        let view = history.current(id, 1).unwrap();
        assert_range(&view.page(&db, true).unwrap(), 221, 320);
        let older = view.page(&db, true).unwrap();
        assert_range(&older, 181, 300);
        assert!(older.has_older && older.has_newer);

        save(&mut db, id, 321);
        let refreshed = view.refresh(&db).unwrap();
        assert_range(&refreshed, 181, 300);
        assert!(refreshed.has_newer);

        let anchor = Anchor {
            cursor: older.messages[8].cursor.clone(),
            offset: -12.0,
        };
        history.remember(Some(ReadingPosition {
            conversation: id,
            anchor: Some(anchor.clone()),
        }));
        history.open(&db, other, 2).unwrap();
        assert!(history.current(id, 1).is_none());
        let (restored, position) = history.open(&db, id, 3).unwrap();
        assert_range(&restored, 149, 248);
        let position = position.unwrap();
        assert_eq!(position.cursor, anchor.cursor);
        assert_eq!(position.offset, -12.0);

        let view = history.current(id, 3).unwrap();
        assert_range(&view.page(&db, false).unwrap(), 169, 288);
        let following = view.page(&db, false).unwrap();
        assert_range(&following, 202, 321);
        assert!(!following.has_newer);
        assert_range(&view.present(&db).unwrap(), 262, 321);

        history.remember(Some(ReadingPosition {
            conversation: id,
            anchor: None,
        }));
        assert!(history.open(&db, id, 4).unwrap().1.is_none());
    }

    #[test]
    fn refresh_reconciles_late_messages_and_keeps_the_loaded_edges_bounded() {
        let root = TestDir::new();
        let mut db = open(&root);
        let id = db.create_conversation("123", 1).unwrap();
        save(&mut db, id, 2);
        save(&mut db, id, 4);
        let window = TimelineWindow::latest(&db, id).unwrap();
        for time in [1, 3, 5] {
            save(&mut db, id, time);
        }
        let window = window.refresh(&db, id).unwrap();
        assert_range(&window, 1, 5);
        assert!(!window.has_older && !window.has_newer);

        for time in 6..=130 {
            save(&mut db, id, time);
        }
        let window = window.refresh(&db, id).unwrap();
        assert_range(&window, 1, 120);
        assert!(window.has_newer);
        assert_range(&TimelineWindow::latest(&db, id).unwrap(), 71, 130);
    }
}
