-- PREPARED FOR OWNER REVIEW. Apply only with the matching prekey backend.
-- Startup validates schema and never applies this migration.
-- This script is not executed by startup and has not been applied.
-- Adds public-key distribution only. Existing accounts/messages remain intact.
BEGIN;
SET LOCAL lock_timeout = '5s';
SET LOCAL statement_timeout = '1min';
SET LOCAL search_path = pg_catalog;

-- One Signal device per registered account for the first milestone.
-- JSON objects contain PUBLIC key_id/public_key/signature fields only;
-- Go strictly validates public fields, sizes and the Ed25519 account binding.
-- Devices verify Signal prekey signatures with official libsignal.
-- EC keys are 33 bytes; Kyber public keys are bounded to 2048 bytes.
-- This keeps every selected bundle below the 32768-byte claim limit.
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
    -- At most 64 publication IDs per account, enforced by the locked Go update.
    -- Remember every accepted ID/hash so an old retry cannot republish a key.
    publication_history jsonb NOT NULL CHECK (
        jsonb_typeof(publication_history) = 'object'
        AND octet_length(publication_history::text) <= 8192
        AND publication_history ? publication_id::text
        AND publication_history ->> publication_id::text = encode(publication_hash, 'hex')
    ),
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
    recipient_void_id bigint REFERENCES public.accounts(void_id),
    device_id smallint NOT NULL DEFAULT 1 CHECK (device_id = 1),
    public_bundle jsonb,
    -- Expiry scrubs the bundle and recipient relationship. Only the bounded
    -- requester/UUID tombstone remains, so an expired retry stays terminal.
    CHECK (
        (public_bundle IS NULL AND recipient_void_id IS NULL)
        OR (public_bundle IS NOT NULL AND recipient_void_id IS NOT NULL
            AND jsonb_typeof(public_bundle) = 'object'
            AND octet_length(public_bundle::text) <= 32768)
    ),
    claimed_at timestamptz NOT NULL DEFAULT now(),
    claim_expires_at timestamptz NOT NULL,
    PRIMARY KEY (requester_void_id, request_id),
    CHECK (requester_void_id <> recipient_void_id),
    CHECK (claim_expires_at = claimed_at + INTERVAL '24 hours')
);

CREATE INDEX signal_prekey_claims_expiry_idx
    ON public.signal_prekey_claims(claim_expires_at,requester_void_id,request_id)
    WHERE public_bundle IS NOT NULL;

-- Backend requirements before activation:
-- * SELECT the recipient inventory FOR UPDATE.
-- * Recheck the existing claim after taking that lock.
-- * Remove at most one EC and one Kyber one-time key, retaining the fallback.
-- * INSERT the claim and UPDATE inventory in the SAME transaction.
-- * Retry the same UUID without consuming more keys.
-- * Go assigns claimed_at and a fixed 24-hour expiry; expiry scrubs both public
--   bundle and recipient FK, retaining only terminal requester/UUID history.
-- * Hard lifetime caps (including tombstones): 1024 claims per requester and
--   100000 claims globally. At most 10000 inventories and 64 publications per
--   account. Transactional prekey-only quota locks prevent concurrent overshoot.
-- * Account/Signal identity, registration and signed/fallback keys are immutable
--   in this milestone; automatic rotation/recovery is outside this contract.
-- * Strict authenticated command rates supplement these persistent caps.
-- No roles, passwords, grants, account rows or message-queue rows change here.
-- Runtime grants are a separate review if the existing owner lacks access.
COMMIT;
