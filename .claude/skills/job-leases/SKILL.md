---
name: job-leases
description: Use when changing how a background job's lease is claimed, confirmed, renewed or given up in Agent Workspace's server (apps/server/src/jobs.rs: claim, confirm_lease, renew_lease, extend_lease, lease_still_held, the lease length or the renewal timing).
---

# Check a change to job leases against every order of steps

A job's lease makes one promise: a handler never runs at or after the end
of its lease, so two workers never run the same job at once. A break means
a webhook sent twice or two launches of one run, and it only shows when a
claim arrives late or the database is unreachable for part of a lease,
which tests rarely reach.

`apps/server/src/jobs/model.rs` checks that promise in every order of
steps: one job, two workers, claims that arrive late, renewals that are
quick or slow and extend the lease, are refused, fail, or extend it and
lose their answer, and time passing for more than two leases.

## Run it

```bash
cargo test -p agent-workspace-server --lib jobs::model
```

A few seconds once the crate is built. No database. CI runs it with the
other Rust tests.

## What it covers

| In the model | In the code (`apps/server/src/jobs.rs`) |
| --- | --- |
| Whether a worker keeps running after a failed renewal | `lease_still_held`, the real function |
| How long a worker waits, and how long a renewal may take | `renewal_wait` and `renewal_patience`, the real functions |
| "claims the job" | `claim`: a queued job, or a running one whose `lease_expires_at < now()` |
| "starts the handler" | `confirm_lease` in `execute`, which renews before any effect |
| "sends a renewal (answered N sixths later)" | `extend_lease`, which answers within `renewal_patience` |
| "renews its lease" / "is told the lease is no longer its own" | `extend_lease` answering `Ok(true)` / `Ok(false)` |
| "renewal can't reach the database" / "extends the lease but its answer is lost" | `extend_lease` answering `Err(())`, with the row unchanged or extended |
| "stops the handler" | `renew_lease` returning, which ends the `select!` in `execute` |

Last confirmed against the code at commit `d0b13f8b`. When `jobs.rs` has
changed since then, read the change against this table before trusting a
pass, then move the commit here forward.

It assumes what the code assumes: a worker's clock and the database's run
at the same rate, a sleeping worker wakes on time, and dropping a handler
stops it at once. It does not cover what a handler does, `finish` (which
only counts while the lease token is still the job's), or the cluster
leases in `cluster/leases.rs`.

## What it found

- **Holds:** a handler never runs at or after the end of its lease, a job
  whose worker stopped is taken over, and one failed renewal doesn't stop
  a handler.
- **The past bugs it replays:** a claim that reached its worker late
  started a second handler (fixed in 2c013368 by `confirm_lease`), and a
  worker whose renewals kept failing ran beside its successor (fixed in
  aace9b02 by `lease_still_held`).

## Reading a failure

A failing test prints the shortest order of steps that breaks the promise,
then the state it ends in. Time is counted in sixths of a lease:

```
  1. worker 1 claims the job
  2. worker 1 starts the handler
  …
  7. worker 1's renewal can't reach the database: it keeps the handler running and will try again
  …
  10. worker 1 sends a renewal (answered 1 sixths later)
  11. time passes: 6 sixths of a lease
  ends in World { now: 6, row: Some(Row { token: 1, expires_at: 6 }), workers: [Renewing { … }, Idle] }
```

(That one is `lease_still_held` without the time a renewal may take.)

A worker that is `Running` or `Renewing` when `now` has reached its row's
`expires_at`, or whose row carries another token, is the harm: from that
moment another worker may start the same job.

- If your change is to `lease_still_held`, `renewal_wait`,
  `renewal_patience` or the lease length, the path is a real bug in it. Fix
  the code, not the model.
- If your change is to the steps (a new place a handler starts, a renewal
  that works differently), the model no longer describes the code. Change
  the matching step in `model.rs`, run it, and update the table above.
- Never make a failing property pass by narrowing it or by removing a step
  the code still takes.
