-- C2.34: push that acts. Which transport a device's subscription uses (only
-- Web Push today; a native wrapper would add APNs or FCM without changing
-- the rest), and a receipt per push to a device, so the service worker can
-- say it was shown and tapped. Only the receipt's sha256 is kept.

ALTER TABLE push_subscriptions
    ADD COLUMN transport TEXT NOT NULL DEFAULT 'web_push'
        CHECK (transport IN ('web_push', 'apns', 'fcm'));

CREATE TABLE push_receipts (
    receipt_hash BYTEA PRIMARY KEY CHECK (octet_length(receipt_hash) = 32),
    workspace_id UUID NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    subscription_id UUID NOT NULL REFERENCES push_subscriptions(id) ON DELETE CASCADE,
    notification_id UUID NOT NULL REFERENCES notifications(id) ON DELETE CASCADE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    shown_at TIMESTAMPTZ,
    opened_at TIMESTAMPTZ
);

CREATE INDEX push_receipts_created ON push_receipts (created_at);
CREATE INDEX push_receipts_by_subscription ON push_receipts (subscription_id, created_at);
CREATE INDEX push_receipts_by_workspace ON push_receipts (workspace_id);
CREATE INDEX push_receipts_by_notification ON push_receipts (notification_id);
SELECT tenancy_protect('push_receipts', 'workspace_id');
