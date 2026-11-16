CREATE TABLE peer_account_status (
    void_id TEXT PRIMARY KEY,
    status TEXT NOT NULL CHECK (status IN ('active', 'deleted'))
) STRICT;
