-- REVIEW ONLY. Apply explicitly as schema owner before enabling HTTP sessions.
-- This migration adds one table; existing accounts and key bindings are intact.
BEGIN;

CREATE TABLE public.void_sessions (
    token_hash bytea PRIMARY KEY CHECK (octet_length(token_hash) = 32),
    void_id bigint NOT NULL REFERENCES public.accounts (void_id),
    created_at timestamptz NOT NULL,
    expires_at timestamptz NOT NULL,
    CONSTRAINT void_sessions_expiry_check CHECK (expires_at > created_at)
);

CREATE INDEX void_sessions_account_expiry_idx ON public.void_sessions (void_id, expires_at);

COMMIT;
