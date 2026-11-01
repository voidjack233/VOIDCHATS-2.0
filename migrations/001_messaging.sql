-- REVIEW ONLY. Apply explicitly to an approved database; server startup never
-- executes this file. No existing account objects, credentials or rows change.
-- Requirements: public.accounts(void_id bigint UNIQUE), PostgreSQL 14+.
BEGIN;

CREATE TABLE public.void_messaging_schema (
 version smallint PRIMARY KEY CHECK (version = 1)
);

CREATE TABLE public.void_key_packages (
 key_package_id uuid PRIMARY KEY,
 owner_void_id bigint NOT NULL REFERENCES public.accounts(void_id),
 fingerprint text NOT NULL CHECK (fingerprint ~ '^[0-9a-f]{64}$'),
 package_base64 text NOT NULL CHECK (octet_length(package_base64) BETWEEN 4 AND 1398104),
 expires_at timestamptz NOT NULL,
 claimed_by bigint REFERENCES public.accounts(void_id),
 claim_id uuid,
 request_id uuid,
 UNIQUE(owner_void_id,fingerprint),
 UNIQUE(claimed_by,claim_id),
 CHECK ((claimed_by IS NULL) = (claim_id IS NULL)),
 CHECK (claimed_by IS NULL OR claimed_by <> owner_void_id),
 CHECK (request_id IS NULL OR claimed_by IS NOT NULL)
);
CREATE INDEX void_key_packages_available ON public.void_key_packages(owner_void_id,expires_at,key_package_id) WHERE claimed_by IS NULL;

-- record is canonical JSON encoded as bytea, containing public metadata and
-- opaque base64 MLS bytes. Explicit indexed columns enforce ownership, pair,
-- idempotency and sequence constraints without scanning/decrypting records.
-- Exact encoded-record lengths also make byte quotas deterministic.
CREATE TABLE public.void_requests (
 request_id uuid PRIMARY KEY,
 conversation_id uuid NOT NULL UNIQUE,
 sender_void_id bigint NOT NULL REFERENCES public.accounts(void_id),
 recipient_void_id bigint NOT NULL REFERENCES public.accounts(void_id),
 client_request_id uuid NOT NULL,
 key_package_id uuid NOT NULL UNIQUE REFERENCES public.void_key_packages(key_package_id),
 status text NOT NULL CHECK(status IN ('pending','accepted','declined','cancelled')),
 pair_a bigint NOT NULL REFERENCES public.accounts(void_id),
 pair_b bigint NOT NULL REFERENCES public.accounts(void_id),
 record bytea NOT NULL CHECK (octet_length(record) BETWEEN 2 AND 1500000),
 UNIQUE(sender_void_id,client_request_id),
 CHECK(sender_void_id<>recipient_void_id),
 CHECK(pair_a=least(sender_void_id,recipient_void_id) AND pair_b=greatest(sender_void_id,recipient_void_id))
);
CREATE UNIQUE INDEX void_requests_active_pair ON public.void_requests(pair_a,pair_b) WHERE status IN ('pending','accepted');
CREATE INDEX void_requests_sender_page ON public.void_requests(sender_void_id,request_id);
CREATE INDEX void_requests_recipient_page ON public.void_requests(recipient_void_id,request_id);
ALTER TABLE public.void_key_packages ADD CONSTRAINT void_key_packages_request_fk FOREIGN KEY(request_id) REFERENCES public.void_requests(request_id);

CREATE TABLE public.void_conversations (
 conversation_id uuid PRIMARY KEY,
 request_id uuid NOT NULL UNIQUE REFERENCES public.void_requests(request_id),
 sender_void_id bigint NOT NULL REFERENCES public.accounts(void_id),
 recipient_void_id bigint NOT NULL REFERENCES public.accounts(void_id),
 last_sequence bigint NOT NULL DEFAULT 0 CHECK(last_sequence>=0),
 record bytea NOT NULL CHECK(octet_length(record) BETWEEN 2 AND 8192),
 CHECK(sender_void_id<>recipient_void_id)
);
CREATE INDEX void_conversations_sender_page ON public.void_conversations(sender_void_id,conversation_id);
CREATE INDEX void_conversations_recipient_page ON public.void_conversations(recipient_void_id,conversation_id);

CREATE TABLE public.void_blocks (
 blocker_void_id bigint NOT NULL REFERENCES public.accounts(void_id),
 target_void_id bigint NOT NULL REFERENCES public.accounts(void_id),
 created_at timestamptz NOT NULL,
 PRIMARY KEY(blocker_void_id,target_void_id),
 CHECK(blocker_void_id<>target_void_id)
);
CREATE INDEX void_blocks_target ON public.void_blocks(target_void_id,blocker_void_id);

CREATE TABLE public.void_messages (
 message_id uuid PRIMARY KEY,
 conversation_id uuid NOT NULL REFERENCES public.void_conversations(conversation_id),
 sender_void_id bigint NOT NULL REFERENCES public.accounts(void_id),
 recipient_void_id bigint NOT NULL REFERENCES public.accounts(void_id),
 client_message_id uuid NOT NULL,
 sequence bigint NOT NULL CHECK(sequence>0),
 ciphertext_bytes integer NOT NULL CHECK(ciphertext_bytes BETWEEN 4 AND 1398104),
 record bytea NOT NULL CHECK(octet_length(record) BETWEEN 2 AND 1500000),
 UNIQUE(sender_void_id,client_message_id),
 UNIQUE(conversation_id,sequence),
 CHECK(sender_void_id<>recipient_void_id)
);

CREATE TABLE public.void_event_counters (
 owner_void_id bigint PRIMARY KEY REFERENCES public.accounts(void_id),
 last_cursor bigint NOT NULL DEFAULT 0 CHECK(last_cursor>=0),
 receipt_reservations integer NOT NULL DEFAULT 0 CHECK(receipt_reservations BETWEEN 0 AND 1000)
);

CREATE TABLE public.void_events (
 event_id uuid PRIMARY KEY,
 owner_void_id bigint NOT NULL REFERENCES public.accounts(void_id),
 cursor bigint NOT NULL CHECK(cursor>0),
 record bytea NOT NULL CHECK(octet_length(record) BETWEEN 2 AND 1600000),
 outcome text CHECK(outcome IN ('applied','rejected','cancelled')),
 UNIQUE(owner_void_id,cursor)
);
CREATE INDEX void_events_pending ON public.void_events(owner_void_id,cursor) WHERE outcome IS NULL;
INSERT INTO public.void_messaging_schema(version) VALUES(1);

-- Runtime grants (replace void_runtime with the existing runtime role AFTER
-- review; uncomment only the grants for the chosen role):
-- GRANT SELECT ON public.void_messaging_schema TO void_runtime;
-- GRANT SELECT(void_id,username,auth_public_key) ON public.accounts TO void_runtime;
-- GRANT SELECT,INSERT,UPDATE ON public.void_key_packages,public.void_requests,
-- public.void_conversations,public.void_blocks,public.void_messages,
-- public.void_event_counters,public.void_events TO void_runtime;
-- No DELETE or sequence privileges are required. Rows are retained for stable
-- retries; future cleanup must preserve fingerprints, client-ID bindings, ACK
-- outcomes and unacknowledged delivery. Quotas return backpressure, never evict.
COMMIT;
