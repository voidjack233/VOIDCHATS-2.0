-- Signal state is separate from dormant MLS state. Existing rows remain intact.
-- This migration is applied only by the application's protected database opener.
CREATE TABLE signal_local_identity (
    slot INTEGER PRIMARY KEY CHECK (slot = 1),
    key_pair BLOB NOT NULL CHECK (length(key_pair) > 0),
    registration_id INTEGER NOT NULL CHECK (registration_id BETWEEN 1 AND 16380),
    next_prekey_id INTEGER NOT NULL CHECK (next_prekey_id BETWEEN 1 AND 16777215),
    initial_prekeys_generated INTEGER NOT NULL DEFAULT 0 CHECK (initial_prekeys_generated IN (0,1)),
    signed_prekey_id INTEGER,
    kyber_last_resort_id INTEGER
);

CREATE TABLE signal_prekeys (
    kind TEXT NOT NULL CHECK (kind IN ('ec','signed','kyber')),
    id INTEGER NOT NULL CHECK (id BETWEEN 1 AND 16777214),
    record BLOB NOT NULL CHECK (length(record) > 0),
    last_resort INTEGER NOT NULL DEFAULT 0 CHECK (last_resort IN (0,1)),
    published INTEGER NOT NULL DEFAULT 0 CHECK (published IN (0,1)),
    PRIMARY KEY(kind,id),
    CHECK (last_resort = 0 OR kind = 'kyber')
);

CREATE TABLE signal_sessions (
    peer_void_id TEXT NOT NULL,
    device_id INTEGER NOT NULL CHECK (device_id = 1),
    record BLOB NOT NULL CHECK (length(record) > 0),
    PRIMARY KEY(peer_void_id,device_id)
);

-- A row is pinned only after the account Ed25519 binding has been verified.
-- This is protocol acceptance, not a user-visible verified/trusted contact flag.
CREATE TABLE signal_identities (
    peer_void_id TEXT NOT NULL,
    device_id INTEGER NOT NULL CHECK (device_id = 1),
    identity_key BLOB NOT NULL CHECK (length(identity_key) = 33),
    PRIMARY KEY(peer_void_id,device_id)
);

-- Last-resort Kyber keys remain available; their session base keys cannot repeat.
CREATE TABLE signal_kyber_uses (
    kyber_prekey_id INTEGER NOT NULL,
    signed_prekey_id INTEGER NOT NULL,
    base_key BLOB NOT NULL CHECK (length(base_key) = 33),
    PRIMARY KEY(kyber_prekey_id,signed_prekey_id,base_key)
);
