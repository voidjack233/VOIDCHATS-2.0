-- Apply explicitly as the schema owner with the old backend stopped. Deploy
-- the backend that uses these names before accepting traffic again.
-- For a new database, apply 001/002 and their runtime grants before this file.
-- RENAME preserves rows, object identities, foreign keys, ownership and grants.
-- Existing accounts, column names and schema marker version 1 are unchanged.
BEGIN;
SET LOCAL lock_timeout = '5s';
SET LOCAL statement_timeout = '1min';
SET LOCAL search_path = pg_catalog;

DO $migration$
DECLARE
    old_names constant text[] := ARRAY[
        'void_sessions',
        'void_key_packages',
        'void_requests',
        'void_conversations',
        'void_blocks',
        'void_messages',
        'void_event_counters',
        'void_events',
        'void_messaging_schema'
    ];
    new_names constant text[] := ARRAY[
        'sessions',
        'key_packages',
        'message_requests',
        'conversations',
        'blocks',
        'messages',
        'sync_cursors',
        'sync_events',
        'messaging_schema'
    ];
    table_number integer;
    source_oid oid;
    target_name text;
    item record;
BEGIN
    -- Check the complete source and destination set before changing anything.
    -- to_regclass also detects conflicting destination views/indexes/sequences.
    FOR table_number IN 1..array_length(old_names, 1) LOOP
        source_oid := to_regclass(format('public.%I', old_names[table_number]));
        IF source_oid IS NULL OR NOT EXISTS (
            SELECT 1 FROM pg_class
            WHERE oid = source_oid AND relkind = 'r' AND relpersistence = 'p'
        ) THEN
            RAISE EXCEPTION 'Expected persistent table public.% is missing or has an unexpected type',
                old_names[table_number];
        END IF;
        IF to_regclass(format('public.%I', new_names[table_number])) IS NOT NULL THEN
            RAISE EXCEPTION 'Destination relation public.% already exists',
                new_names[table_number];
        END IF;
    END LOOP;

    -- Acquire every lock before the first rename. A timeout/error rolls back
    -- this whole transaction; no partly renamed schema can become visible.
    FOR table_number IN 1..array_length(old_names, 1) LOOP
        EXECUTE format('LOCK TABLE public.%I IN ACCESS EXCLUSIVE MODE',
            old_names[table_number]);
    END LOOP;
    FOR table_number IN 1..array_length(old_names, 1) LOOP
        EXECUTE format('ALTER TABLE public.%I RENAME TO %I',
            old_names[table_number], new_names[table_number]);
    END LOOP;

    -- ALTER TABLE RENAME alone leaves constraint/index names unchanged.
    -- Rename constraints first: PostgreSQL also renames a PRIMARY KEY/UNIQUE
    -- constraint's backing index, which must not be renamed a second time.
    FOR table_number IN 1..array_length(new_names, 1) LOOP
        source_oid := to_regclass(format('public.%I', new_names[table_number]));
        FOR item IN
            SELECT conname FROM pg_constraint
            WHERE conrelid = source_oid
              AND left(conname, length(old_names[table_number]) + 1)
                    = old_names[table_number] || '_'
            ORDER BY conname COLLATE "C"
        LOOP
            target_name := new_names[table_number]
                || substr(item.conname, length(old_names[table_number]) + 1);
            IF octet_length(target_name) > 63 THEN
                RAISE EXCEPTION 'Renamed constraint identifier would exceed PostgreSQL limit: %', target_name;
            END IF;
            EXECUTE format('ALTER TABLE public.%I RENAME CONSTRAINT %I TO %I',
                new_names[table_number], item.conname, target_name);
        END LOOP;

        FOR item IN
            SELECT index_class.relname FROM pg_index AS index_info
            JOIN pg_class AS index_class ON index_class.oid = index_info.indexrelid
            WHERE index_info.indrelid = source_oid
              AND left(index_class.relname, length(old_names[table_number]) + 1)
                    = old_names[table_number] || '_'
            ORDER BY index_class.relname COLLATE "C"
        LOOP
            target_name := new_names[table_number]
                || substr(item.relname, length(old_names[table_number]) + 1);
            IF octet_length(target_name) > 63 THEN
                RAISE EXCEPTION 'Renamed index identifier would exceed PostgreSQL limit: %', target_name;
            END IF;
            EXECUTE format('ALTER INDEX public.%I RENAME TO %I', item.relname, target_name);
        END LOOP;
    END LOOP;
END
$migration$;

COMMIT;
