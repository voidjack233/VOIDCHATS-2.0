-- REVIEW ONLY. Run using psql -v runtime_role=YOUR_EXISTING_RUNTIME_ROLE.
-- Keep the current login/credentials. Do not grant schema administration.
BEGIN;
GRANT SELECT, INSERT, DELETE ON TABLE public.void_sessions TO :"runtime_role";
-- Already required by returning-user authentication and discovery:
GRANT SELECT (void_id, auth_public_key) ON TABLE public.accounts TO :"runtime_role";
COMMIT;
