-- REVIEW ONLY. Do not apply with the unchanged e810561 backend: its schema
-- validator currently permits exactly public.accounts and public.messages.
-- This script is not executed by startup and has not been applied.
-- Adds public-key distribution only. Existing accounts/messages remain intact.
BEGIN;
SET LOCAL lock_timeout = '5s';
SET LOCAL statement_timeout = '1min';
SET LOCAL search_path = pg_catalog;

-- One Signal device per registered account for the first milestone.
-- JSON objects contain PUBLIC key_id/public_key/signature fields only;
-- strict upload field/size validation belongs to the authenticated handler.
-- Store no private keys, sessions, ratchets, plaintext or conversations here.
CREATE TABLE public.signal_prekey_inventory (
    void_id bigint PRIMARY KEY REFERENCES public.accounts(void_id),
    device_id smallint NOT NULL DEFAULT 1 CHECK (device_id = 1),
    registration_id integer NOT NULL CHECK (registration_id BETWEEN 1 AND 16380),
    identity_key bytea NOT NULL CHECK (octet_length(identity_key) = 33),
    account_signature bytea NOT NULL CHECK (octet_length(account_signature) = 64),
    signed_prekey jsonb NOT NULL CHECK (
        jsonb_typeof(signed_prekey) = 'object'
        AND octet_length(signed_prekey::text) <= 8192
    ),
    kyber_last_resort jsonb NOT NULL CHECK (
        jsonb_typeof(kyber_last_resort) = 'object'
        AND octet_length(kyber_last_resort::text) <= 8192
    ),
    one_time_prekeys jsonb NOT NULL DEFAULT '[]'::jsonb CHECK (
        jsonb_typeof(one_time_prekeys) = 'array'
        AND jsonb_array_length(one_time_prekeys) <= 64
        AND octet_length(one_time_prekeys::text) <= 32768
    ),
    one_time_kyber_prekeys jsonb NOT NULL DEFAULT '[]'::jsonb CHECK (
        jsonb_typeof(one_time_kyber_prekeys) = 'array'
        AND jsonb_array_length(one_time_kyber_prekeys) <= 64
        AND octet_length(one_time_kyber_prekeys::text) <= 262144
    ),
    -- Monotonic high-water marks survive removal of claimed public keys.
    -- The handler must reject old IDs rather than republishing consumed keys.
    max_ec_prekey_id integer NOT NULL DEFAULT 0
        CHECK (max_ec_prekey_id BETWEEN 0 AND 16777214),
    max_kyber_prekey_id integer NOT NULL DEFAULT 0
        CHECK (max_kyber_prekey_id BETWEEN 0 AND 16777214),
    publication_id uuid NOT NULL
        CHECK (publication_id <> '00000000-0000-0000-0000-000000000000'::uuid),
    publication_hash bytea NOT NULL CHECK (octet_length(publication_hash) = 32),
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    CHECK (updated_at >= created_at)
);

-- A lost response must return the SAME selected public bundle for this UUID.
-- Reuse of this UUID for another recipient is a conflict.
CREATE TABLE public.signal_prekey_claims (
    requester_void_id bigint NOT NULL REFERENCES public.accounts(void_id),
    request_id uuid NOT NULL
        CHECK (request_id <> '00000000-0000-0000-0000-000000000000'::uuid),
    recipient_void_id bigint NOT NULL REFERENCES public.accounts(void_id),
    device_id smallint NOT NULL DEFAULT 1 CHECK (device_id = 1),
    public_bundle jsonb NOT NULL CHECK (
        jsonb_typeof(public_bundle) = 'object'
        AND octet_length(public_bundle::text) <= 32768
    ),
    claimed_at timestamptz NOT NULL DEFAULT now(),
    claim_expires_at timestamptz NOT NULL,
    PRIMARY KEY (requester_void_id, request_id),
    CHECK (requester_void_id <> recipient_void_id),
    CHECK (claim_expires_at > claimed_at)
);

-- Backend requirements before activation:
-- * SELECT the recipient inventory FOR UPDATE.
-- * Recheck the existing claim after taking that lock.
-- * Remove at most one EC and one Kyber one-time key, retaining the fallback.
-- * INSERT the claim and UPDATE inventory in the SAME transaction.
-- * Retry the same UUID without consuming more keys.
-- * Expired claims remain terminal; do not delete their UUID history and
--   silently allocate a new bundle for a reused request.
-- * Bound publications/claims per authenticated account in the handler.
-- No roles, passwords, grants, account rows or message-queue rows change here.
-- Runtime grants are a separate review if the existing owner lacks access.
COMMIT;
