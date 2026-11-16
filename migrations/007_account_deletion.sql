-- REVIEW ONLY. Do not apply without owner approval and a coordinated backend stop.
-- No account is deleted by this migration. NULL means Active; non-NULL is Deleted.
BEGIN;
ALTER TABLE public.accounts ADD COLUMN deleted_at timestamptz;

-- Defense in depth: the ID/key binding and tombstone cannot be erased or revived
-- by ordinary SQL, including accidental runtime UPDATE/DELETE/TRUNCATE operations.
CREATE FUNCTION public.preserve_account_identity() RETURNS trigger
LANGUAGE plpgsql SET search_path = pg_catalog AS $$
BEGIN
    IF TG_OP IN ('DELETE', 'TRUNCATE') THEN
        RAISE EXCEPTION 'account identities are permanent';
    END IF;
    IF NEW.id IS DISTINCT FROM OLD.id
       OR NEW.void_id IS DISTINCT FROM OLD.void_id
       OR NEW.auth_public_key IS DISTINCT FROM OLD.auth_public_key
       OR NEW.created_at IS DISTINCT FROM OLD.created_at
       OR (OLD.deleted_at IS NOT NULL AND NEW.deleted_at IS DISTINCT FROM OLD.deleted_at) THEN
        RAISE EXCEPTION 'account identity or tombstone is immutable';
    END IF;
    IF OLD.deleted_at IS NULL AND NEW.deleted_at IS NOT NULL THEN
        NEW.deleted_at := clock_timestamp();
    END IF;
    RETURN NEW;
END;
$$;
CREATE TRIGGER accounts_preserve_identity BEFORE UPDATE OR DELETE ON public.accounts
    FOR EACH ROW EXECUTE FUNCTION public.preserve_account_identity();
CREATE TRIGGER accounts_preserve_tombstones BEFORE TRUNCATE ON public.accounts
    FOR EACH STATEMENT EXECUTE FUNCTION public.preserve_account_identity();
COMMIT;
