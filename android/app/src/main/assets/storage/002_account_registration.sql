-- Public registration metadata; the existing MLS signing key stays native.
CREATE TABLE account_registration (
    slot INTEGER PRIMARY KEY CHECK (slot = 1),
    username TEXT NOT NULL CHECK (length(username) > 0),
    api_url TEXT NOT NULL CHECK (length(api_url) > 0),
    proof_signed INTEGER NOT NULL DEFAULT 0 CHECK (proof_signed IN (0, 1)),
    created_at TEXT
);

CREATE TRIGGER account_binding_immutable
BEFORE UPDATE OF account_void_id ON local_identity
WHEN OLD.account_void_id IS NOT NULL AND NEW.account_void_id IS NOT OLD.account_void_id
BEGIN
    SELECT RAISE(ABORT, 'account_binding_immutable');
END;
