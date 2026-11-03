-- REVIEW ONLY. Optional grants for an existing restricted runtime role.
-- No role or password is created or changed. The configured owner already has access.
-- psql --set runtime_role=<existing-role> --file <this-file>
BEGIN;
GRANT SELECT,INSERT ON public.signal_prekey_inventory,public.signal_prekey_claims
    TO :"runtime_role";
GRANT UPDATE(one_time_prekeys,one_time_kyber_prekeys,max_ec_prekey_id,
    max_kyber_prekey_id,publication_id,publication_hash,publication_history,updated_at)
    ON public.signal_prekey_inventory TO :"runtime_role";
GRANT UPDATE(recipient_void_id,public_bundle)
    ON public.signal_prekey_claims TO :"runtime_role";
COMMIT;
