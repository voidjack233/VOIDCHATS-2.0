-- Recovery is local plaintext processing. Invalid records are retained for
-- diagnosis but never retried as though they were authenticated supported input.
ALTER TABLE deferred_signal_events ADD COLUMN recovery_state TEXT NOT NULL DEFAULT 'pending'
    CHECK (recovery_state IN ('pending', 'quarantined'));
ALTER TABLE deferred_signal_events ADD COLUMN recovery_error TEXT;
CREATE INDEX deferred_recovery_order ON deferred_signal_events(recovery_state, received_at, sender_void_id, event_id);
