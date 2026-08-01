ALTER TABLE github_webhook_deliveries
    ADD COLUMN redelivery_payload JSONB CHECK (
        redelivery_payload IS NULL OR (
            jsonb_typeof(redelivery_payload) = 'object'
            AND octet_length(redelivery_payload::text) <= 16384
        )
    );

COMMENT ON COLUMN github_webhook_deliveries.redelivery_payload IS
    'Bounded payload fields re-parsed from an identical signed redelivery after a failed or exhausted job; the immutable job payload remains unchanged.';
