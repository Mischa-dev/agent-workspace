-- Agents doing long work on one reply request (a tagged coding task, say)
-- report a one-line status while they work. Reporting renews the claim's
-- lease. A new claim of a pending request starts with no status.
ALTER TABLE reply_requests
    ADD COLUMN progress_text TEXT CHECK (
        progress_text IS NULL OR char_length(progress_text) BETWEEN 1 AND 200
    ),
    ADD COLUMN progress_at TIMESTAMPTZ,
    ADD CONSTRAINT reply_requests_progress_pair
        CHECK ((progress_text IS NULL) = (progress_at IS NULL));
