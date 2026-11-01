-- REVIEW ONLY. Run explicitly with the existing role selected by the owner:
-- psql "$APPROVED_DATABASE_URL" -v ON_ERROR_STOP=1 -v runtime_role=EXISTING_ROLE -f migrations/001_messaging_privileges.sql
-- This changes grants on additive tables only; no role or password is created
-- or modified. Omit execution when the existing owner already has privileges.
BEGIN;
GRANT SELECT ON public.void_messaging_schema TO :"runtime_role";
GRANT SELECT(void_id,username,auth_public_key) ON public.accounts TO :"runtime_role";
GRANT SELECT,INSERT,UPDATE ON
 public.void_key_packages,public.void_requests,public.void_conversations,
 public.void_blocks,public.void_messages,public.void_event_counters,
 public.void_events TO :"runtime_role";
COMMIT;
