-- REVIEW ONLY. Existing runtime role; no role, credential, or cluster changes.
-- psql --set runtime_role=<existing-role> --file <this-file>
BEGIN;
GRANT SELECT(deleted_at), UPDATE(deleted_at) ON public.accounts TO :"runtime_role";
GRANT DELETE ON public.signal_prekey_inventory, public.signal_prekey_claims TO :"runtime_role";
COMMIT;
