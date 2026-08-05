# Private-host performance budgets

M8 treats performance as an exact-commit private-host acceptance gate, not a
public service-level objective. Provider and model latency is always reported
separately from Agent Workspace admission work.

## Budgets

| Surface | Budget |
| --- | --- |
| Production JavaScript | at most 500 KiB raw and 160 KiB gzip in aggregate |
| Production CSS | at most 60 KiB raw in aggregate |
| Ordinary Project, Conversation, timeline, and Work Item reads | loopback p95 below 250 ms |
| Ordinary non-provider write | loopback p95 below 500 ms |
| Authenticated realtime connect through first authorized invalidation | below 2 seconds |
| Startup to readiness with no migration/recovery work | below 10 seconds |
| Controlled service shutdown | below the 20 second systemd limit |

The HTTP percentile gate uses five sequential warmups followed by exactly 25
sequential samples. p95 is the nearest-rank sample after sorting. Responses are
read and parsed within the existing 1 MiB bound; response bodies, credentials,
cookies, and request content are never included in the report. A result above
half of its budget requires a query-count and `EXPLAIN (ANALYZE, BUFFERS,
FORMAT JSON)` investigation against the same fixture before a cache is
considered.

## CI asset gate

Build first, then run the deterministic budget checker:

```bash
corepack pnpm build
corepack pnpm check:assets
```

The JSON report identifies the exact Git commit, lists every emitted file,
records raw and gzip-level-9 bytes, and fails when any aggregate budget is
exceeded. Gzip is calculated independently for each emitted asset. CI also
runs the percentile, loopback-boundary, cookie-parser, and asset-fixture tests
through `corepack pnpm check:performance-tools`.

## Red Hat HTTP measurement

Run only against the SSH-forwarded or host-local loopback listener. Create an
owner-only file containing the existing test User password; never put the
password in argv, an environment variable, shell history, or the report.

```bash
install -m 0600 /dev/null /home/bot/.agent-workspace-measure-password
read -r -s -p 'Measurement password: ' measurement_password
printf '%s\n' "${measurement_password}" > /home/bot/.agent-workspace-measure-password
unset measurement_password

cd /home/bot/deployments/agent-workspace
/opt/agent-workspace/toolchains/node-v24.18.1-linux-arm64/bin/node \
  scripts/measure-private-host.mjs \
  --base-url http://127.0.0.1:3100 \
  --login-name '<test-login-name>' \
  --password-file /home/bot/.agent-workspace-measure-password
```

The command accepts only an explicit HTTP loopback origin, validates the
password file as a bounded owner-only regular file, creates one ordinary
browser session, samples Project list/read, Conversation sidebar, Work Item
list, one Thread timeline when present, and Project-switch write, then logs
out. The report records only commit, architecture/platform/kernel, safe fixture
cardinalities, sampling rules, min/median/p95/max, budgets, and bounded failure
categories. Remove the exact password file immediately after the run.

Use a retained-scale drill database rather than inflating the active private
Project. Before acceptance, the fixture must contain at least 10 Projects, 100
Channels, 500 Threads, 10,000 Messages, 2,000 Work Items, and 250 completed
Agent Runs. Record those safe cardinalities beside the command output. If the
fixture does not meet the minimum, the report is diagnostic only and cannot
satisfy M8.

## Lifecycle, realtime, and run admission

With no non-terminal run and after a valid backup, measure the active unit:

```bash
sudo /opt/agent-workspace/current/bin/measure-service-lifecycle.sh
```

The checked-in command validates the exact immutable release, measures stop
and ready time using the supported unit and loopback health endpoint, restores
the service on every exit path, emits bounded JSON, and fails at 20 seconds for
stop or 10 seconds for readiness. It does not run migrations, seed, or modify
configuration.

In Chrome on the same exact commit, measure realtime from WebSocket creation to
the first authorized invalidation caused by a second session. Require less
than two seconds and record request IDs and durations only. For hosted-run
admission, record database reservation plus owned mirror/worktree preparation
separately from provider `thread/start` and model latency. A dirty or ambiguous
worktree, provider authentication, or model response cannot be relabeled as an
application-latency regression.

After capturing evidence, remove the owner-only password file, disposable
fixture database and data roots, transient measurement session, and any trace
or plan file. Do not add a cache until a retained-scale measurement identifies
a repeatable hot path and the cache design preserves Project/Actor policy,
bounded memory, invalidation, and restart correctness.
