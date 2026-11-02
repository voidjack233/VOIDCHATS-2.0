-- REVIEW ONLY. Only needed for the owner's existing restricted runtime role.
-- Apply separately with psql -v runtime_role=EXISTING_ROLE after queue migration.
-- No role/password/database/schema privilege or account DELETE grant is created.
BEGIN;
GRANT SELECT,INSERT,DELETE ON TABLE public.messages TO :"runtime_role";
-- Existing registration/authentication account privileges remain unchanged.
COMMIT;
