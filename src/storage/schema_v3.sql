CREATE TABLE signal_local_identity (
    slot INTEGER PRIMARY KEY CHECK (slot = 1),
    device_id INTEGER NOT NULL CHECK (device_id = 1),
    key_pair BLOB,
    registration_id INTEGER,
    binding_signature BLOB,
    next_prekey_id INTEGER NOT NULL CHECK (next_prekey_id BETWEEN 1 AND 16777215),
    signed_prekey_id INTEGER,
    kyber_last_resort_id INTEGER,
    CHECK (
        (key_pair IS NULL AND registration_id IS NULL AND binding_signature IS NULL
            AND next_prekey_id = 1 AND signed_prekey_id IS NULL AND kyber_last_resort_id IS NULL)
        OR
        (key_pair IS NOT NULL AND length(key_pair) > 0
            AND registration_id IS NOT NULL AND registration_id BETWEEN 1 AND 16380
            AND binding_signature IS NOT NULL AND length(binding_signature) = 64
            AND next_prekey_id >= 43
            AND signed_prekey_id IS NOT NULL AND signed_prekey_id BETWEEN 1 AND next_prekey_id - 1
            AND kyber_last_resort_id IS NOT NULL AND kyber_last_resort_id BETWEEN 1 AND next_prekey_id - 1
            AND signed_prekey_id <> kyber_last_resort_id)
    )
) STRICT;

-- Only the migration creates the uninitialized slot. An absent row on open is an error.
INSERT INTO signal_local_identity (slot, device_id, next_prekey_id) VALUES (1, 1, 1);

CREATE TABLE signal_prekeys (
    kind TEXT NOT NULL CHECK (kind IN ('ec', 'signed', 'kyber')),
    id INTEGER NOT NULL UNIQUE CHECK (id BETWEEN 1 AND 16777214),
    record BLOB NOT NULL CHECK (length(record) > 0),
    last_resort INTEGER NOT NULL DEFAULT 0 CHECK (last_resort IN (0, 1)),
    published INTEGER NOT NULL DEFAULT 0 CHECK (published IN (0, 1)),
    PRIMARY KEY (kind, id),
    CHECK (last_resort = 0 OR kind = 'kyber')
) STRICT;

CREATE UNIQUE INDEX signal_last_resort ON signal_prekeys(last_resort) WHERE last_resort = 1;

CREATE TABLE signal_sessions (
    peer_void_id TEXT NOT NULL,
    device_id INTEGER NOT NULL CHECK (device_id = 1),
    record BLOB NOT NULL CHECK (length(record) > 0),
    PRIMARY KEY (peer_void_id, device_id)
) STRICT;

CREATE TABLE signal_identities (
    peer_void_id TEXT NOT NULL,
    device_id INTEGER NOT NULL CHECK (device_id = 1),
    identity_key BLOB NOT NULL CHECK (length(identity_key) = 33),
    PRIMARY KEY (peer_void_id, device_id)
) STRICT;

CREATE TABLE signal_kyber_uses (
    kyber_prekey_id INTEGER NOT NULL CHECK (kyber_prekey_id BETWEEN 1 AND 16777214),
    signed_prekey_id INTEGER NOT NULL CHECK (signed_prekey_id BETWEEN 1 AND 16777214),
    base_key BLOB NOT NULL CHECK (length(base_key) = 33),
    PRIMARY KEY (kyber_prekey_id, signed_prekey_id, base_key)
) STRICT;
