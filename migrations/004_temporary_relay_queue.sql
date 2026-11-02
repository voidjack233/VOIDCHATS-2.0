-- PREPARED FOR REVIEW ONLY. No startup path executes this migration.
-- Replaces the empty legacy messaging schema after migrations 001,002,003.
-- Stop the legacy backend first. Apply only to a separately approved database.
-- Any legacy user/session/message/block/package data causes a complete abort:
-- obtain a separate drain/export/import decision rather than dropping records.
BEGIN;
SET LOCAL lock_timeout = '5s';
SET LOCAL statement_timeout = '1min';
SET LOCAL search_path = pg_catalog;
SET LOCAL row_security = off;

DO $migration$
DECLARE
    table_name text;
    has_rows boolean;
    legacy_tables constant text[] := ARRAY[
        'sessions','key_packages','message_requests','conversations','blocks',
        'messages','sync_cursors','sync_events','messaging_schema'
    ];
BEGIN
    IF (SELECT array_agg(c.relname::text ORDER BY c.relname COLLATE "C")
        FROM pg_class c JOIN pg_namespace n ON n.oid=c.relnamespace
        WHERE n.nspname='public' AND c.relkind IN ('r','p'))
       IS DISTINCT FROM ARRAY[
           'accounts','blocks','conversations','key_packages','message_requests',
           'messages','messaging_schema','sessions','sync_cursors','sync_events'
       ]::text[] THEN
        RAISE EXCEPTION 'Unexpected public table inventory; review the migration target';
    END IF;
    -- Lock every source relation before checking emptiness; no legacy writer
    -- can add a row between a successful guard and the schema replacement.
    LOCK TABLE public.accounts IN ACCESS EXCLUSIVE MODE;
    FOREACH table_name IN ARRAY legacy_tables LOOP
        IF NOT EXISTS (
            SELECT 1 FROM pg_class
            WHERE oid=to_regclass(format('public.%I',table_name))
              AND relkind='r' AND relpersistence='p'
        ) THEN
            RAISE EXCEPTION 'Expected legacy persistent table public.% is missing',table_name;
        END IF;
        EXECUTE format('LOCK TABLE public.%I IN ACCESS EXCLUSIVE MODE',table_name);
    END LOOP;
    FOREACH table_name IN ARRAY legacy_tables LOOP
        IF table_name='messaging_schema' THEN CONTINUE; END IF;
        EXECUTE format('SELECT EXISTS(SELECT 1 FROM public.%I)',table_name) INTO has_rows;
        IF has_rows THEN
            RAISE EXCEPTION 'Legacy table public.% is not empty; approved drain/export/import is required',table_name;
        END IF;
    END LOOP;
    IF (SELECT count(*) FROM public.messaging_schema)<>1
       OR NOT EXISTS(SELECT 1 FROM public.messaging_schema WHERE version=1) THEN
        RAISE EXCEPTION 'Unexpected legacy messaging schema marker';
    END IF;
END
$migration$;

-- Existing identity/key values, account UUIDs and optional usernames survive.
ALTER TABLE public.accounts ALTER COLUMN username DROP NOT NULL;

-- Break the existing circular package/request dependency explicitly. No CASCADE.
ALTER TABLE public.key_packages DROP CONSTRAINT key_packages_request_fk;
DROP TABLE public.messages;
DROP TABLE public.conversations;
DROP TABLE public.message_requests;
DROP TABLE public.key_packages;
DROP TABLE public.blocks;
DROP TABLE public.sync_events;
DROP TABLE public.sync_cursors;
DROP TABLE public.sessions;
DROP TABLE public.messaging_schema;

-- Temporary ciphertext queue: no conversation/history/tombstone state.
CREATE TABLE public.messages (
    event_id uuid NOT NULL,
    sender_void_id bigint NOT NULL REFERENCES public.accounts(void_id),
    recipient_void_id bigint NOT NULL REFERENCES public.accounts(void_id),
    payload bytea NOT NULL,
    created_at timestamptz NOT NULL,
    expires_at timestamptz NOT NULL,
    PRIMARY KEY(sender_void_id,event_id),
    CONSTRAINT messages_payload_check CHECK(octet_length(payload) BETWEEN 1 AND 1048576),
    CONSTRAINT messages_expiry_check CHECK(
        expires_at=created_at+INTERVAL '720 hours'
    )
);
CREATE INDEX messages_recipient_delivery_idx
    ON public.messages(recipient_void_id,created_at,sender_void_id,event_id);
CREATE INDEX messages_expiry_idx
    ON public.messages(expires_at,sender_void_id,event_id);

-- New table grants are reviewed separately in 004_temporary_relay_privileges.sql.
-- Existing owner/superuser access needs no change; no role/password is modified.
COMMIT;
