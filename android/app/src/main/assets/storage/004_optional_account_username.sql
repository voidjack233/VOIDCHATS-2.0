-- Preserve applied migration checksums and all existing account metadata.
-- New account initialization does not create or require a username.
CREATE TABLE account_registration_without_username_requirement (
    slot INTEGER PRIMARY KEY CHECK (slot = 1),
    username TEXT CHECK (username IS NULL OR length(username) > 0),
    api_url TEXT NOT NULL CHECK (length(api_url) > 0),
    proof_signed INTEGER NOT NULL DEFAULT 0 CHECK (proof_signed IN (0, 1)),
    created_at TEXT
);

INSERT INTO account_registration_without_username_requirement(slot, username, api_url, proof_signed, created_at)
SELECT slot, username, api_url, proof_signed, created_at FROM account_registration;

DROP TABLE account_registration;
ALTER TABLE account_registration_without_username_requirement RENAME TO account_registration;
