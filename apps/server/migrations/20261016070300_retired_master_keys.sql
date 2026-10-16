-- Master keys secret-key-rotate has removed from the keyring. A server still
-- holding the old keyring refuses to seal a new secret under one of these,
-- instead of storing a value nobody can open.
CREATE TABLE secret_master_keys_retired (
    master_key_id text PRIMARY KEY,
    retired_at timestamptz NOT NULL DEFAULT now()
);
