-- Connected modules' manifests are read again once a day (M1.12): the
-- sweep looks for live installs not read since.
CREATE INDEX module_apps_due_refetch
    ON module_apps (manifest_fetched_at)
    WHERE revoked_at IS NULL;
