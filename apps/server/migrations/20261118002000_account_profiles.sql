-- C2.17: a profile for every account, people and agents alike. Global to
-- the account (R19): one row an account, shown in every workspace it is in,
-- and only to people who share one of them. The avatar is a small image
-- kept here, so it never belongs to one workspace's files.
CREATE TABLE account_profiles (
    account_id UUID PRIMARY KEY REFERENCES actors(id) ON DELETE CASCADE,
    title TEXT CHECK (title IS NULL OR (char_length(title) BETWEEN 1 AND 80
                                        AND title !~ '[[:cntrl:]]')),
    pronouns TEXT CHECK (pronouns IS NULL OR (char_length(pronouns) BETWEEN 1 AND 40
                                              AND pronouns !~ '[[:cntrl:]]')),
    time_zone TEXT CHECK (time_zone IS NULL OR time_zone ~ '^[A-Za-z0-9_+/-]{1,64}$'),
    avatar BYTEA CHECK (avatar IS NULL OR octet_length(avatar) <= 262144),
    avatar_type TEXT CHECK (avatar_type IS NULL OR avatar_type IN
        ('image/png', 'image/jpeg', 'image/gif', 'image/webp')),
    -- Bumped with each new image, so its address changes and caches let go.
    avatar_version INTEGER NOT NULL DEFAULT 0 CHECK (avatar_version >= 0),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CHECK ((avatar IS NULL) = (avatar_type IS NULL))
);
