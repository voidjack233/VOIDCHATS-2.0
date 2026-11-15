CREATE TABLE signal_peer_bindings (
    peer_void_id TEXT NOT NULL,
    device_id INTEGER NOT NULL CHECK (device_id = 1),
    account_public_key BLOB NOT NULL CHECK (length(account_public_key) = 32),
    registration_id INTEGER NOT NULL CHECK (registration_id BETWEEN 1 AND 16380),
    account_signature BLOB NOT NULL CHECK (length(account_signature) = 64),
    PRIMARY KEY (peer_void_id, device_id),
    FOREIGN KEY (peer_void_id, device_id) REFERENCES signal_identities(peer_void_id, device_id)
) STRICT;
