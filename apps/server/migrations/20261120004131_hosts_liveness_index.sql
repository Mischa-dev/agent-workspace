-- The host sweep marks reporting hosts offline when they go quiet, every
-- thirty seconds. Every workspace has a built-in host, so without an index
-- for the hosts it can change, each pass read the whole table: a row per
-- workspace (H1.42, nothing runs per workspace).
CREATE INDEX hosts_reporting_liveness ON hosts (last_seen_at)
    WHERE status = 'active' AND NOT builtin AND connection <> 'a2a';
