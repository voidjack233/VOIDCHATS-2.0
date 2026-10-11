CREATE TABLE accounts (
    void_id text COLLATE "C" NOT NULL,
    display_name text NOT NULL,
    identity_public_key text NOT NULL,
    created_at timestamptz NOT NULL,
    CONSTRAINT accounts_void_id_key UNIQUE (void_id),
    CONSTRAINT accounts_void_id_length_check CHECK (octet_length(void_id) <= 512)
);
