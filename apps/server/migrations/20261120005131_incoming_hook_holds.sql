-- Each incoming hook counts the delivery bodies it holds, so a hook can be
-- refused once it holds its share (answered 429 before anything is
-- stored), and the bytes count against the workspace's storage
-- (`storage_bytes`). The receiver adds to the counts as it stores a body;
-- retention takes off what it clears.
ALTER TABLE incoming_hooks
    ADD COLUMN held_deliveries INTEGER NOT NULL DEFAULT 0 CHECK (held_deliveries >= 0),
    ADD COLUMN held_bytes BIGINT NOT NULL DEFAULT 0 CHECK (held_bytes >= 0);

UPDATE incoming_hooks hook
SET held_deliveries = held.deliveries, held_bytes = held.bytes
FROM (
    SELECT hook_id, count(*)::integer AS deliveries, sum(octet_length(body))::bigint AS bytes
    FROM incoming_hook_deliveries
    WHERE body IS NOT NULL
    GROUP BY hook_id
) held
WHERE held.hook_id = hook.id;
