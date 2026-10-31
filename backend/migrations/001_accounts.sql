-- Phase 1: public account routing identity only. No readable chats or private keys.
CREATE TABLE IF NOT EXISTS accounts (
    void_id VARCHAR(20) PRIMARY KEY
        CHECK (void_id ~ '^[0-9]{16,20}$'),
    username TEXT NOT NULL
        CHECK (char_length(username) BETWEEN 1 AND 32)
        CHECK (octet_length(username) <= 128),
    auth_public_key BYTEA NOT NULL UNIQUE
        CHECK (octet_length(auth_public_key) = 32),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
