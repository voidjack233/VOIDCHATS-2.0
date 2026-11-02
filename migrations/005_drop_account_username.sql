-- Remove the approved username field; keep all account identities and keys.
-- RESTRICT aborts if an external object depends on this column.
BEGIN;
SET LOCAL lock_timeout = '5s';
SET LOCAL statement_timeout = '1min';

ALTER TABLE public.accounts DROP COLUMN username RESTRICT;

COMMIT;
