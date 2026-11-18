-- PREPARED ONLY. Apply separately after approval and stopping all old backends.
-- Startup validates this schema but never executes migrations.
-- Existing ciphertext, routing, timestamps, ACKs and 30-day retention survive.
BEGIN;
SET LOCAL lock_timeout = '5s';
SET LOCAL statement_timeout = '1min';
SET LOCAL search_path = pg_catalog;
SET LOCAL row_security = off;

ALTER TABLE public.messages
    ADD COLUMN min_protocol_version integer NOT NULL DEFAULT 1,
    ADD CONSTRAINT messages_protocol_version_check CHECK(min_protocol_version >= 1);

-- The constant default makes all previously queued events protocol version 1.
-- No account, key, role, grant, ciphertext or existing queue index is changed.
COMMIT;
