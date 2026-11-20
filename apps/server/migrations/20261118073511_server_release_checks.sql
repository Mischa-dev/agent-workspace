-- H1.15: the newest release a feed the operator configured names, checked
-- once a day, so the Server page and `preflight` can say when there is one.
-- One row for the server; nothing in it is a workspace's.
CREATE TABLE server_release_checks (
    id SMALLINT PRIMARY KEY CHECK (id = 1),
    latest TEXT NOT NULL CHECK (latest ~ '^[0-9]{1,6}\.[0-9]{1,6}\.[0-9]{1,6}$'),
    notes_url TEXT CHECK (notes_url IS NULL OR (char_length(notes_url) <= 500 AND notes_url ~ '^https://')),
    checked_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
