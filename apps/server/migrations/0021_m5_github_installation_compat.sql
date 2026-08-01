-- The GitHub installation response enumerates explicit App subscriptions, not
-- the automatically delivered installation/access webhook types. Keep the
-- already-applied 0020 migration immutable and correct the retained boundary
-- additively after live compatibility evidence invalidated that assumption.

ALTER TABLE github_installations
    DROP CONSTRAINT github_installations_subscribed_events_check,
    ADD CONSTRAINT github_installations_subscribed_events_check CHECK (
        cardinality(subscribed_events) BETWEEN 5 AND 6
        AND subscribed_events <@ ARRAY[
            'push', 'pull_request', 'pull_request_review', 'check_suite',
            'check_run', 'repository'
        ]::TEXT[]
    );
