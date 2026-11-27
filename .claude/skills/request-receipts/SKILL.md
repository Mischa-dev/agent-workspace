---
name: request-receipts
description: Use when changing how a retried call is answered in Agent Workspace's server, such as apps/server/src/idempotency.rs, the request-id steps in apps/server/src/ops/dispatch.rs, an operation's IDEMPOTENCY or OWN_TRANSACTION, or a command that commits and then does more work.
---

# Check a change to request ids against every order of steps

A call that carries a request id (`clientRequestId`) makes one promise:
however it is retried, its write is made once. A break means a second page,
a second message or a second charge, and it only shows when a server stops
or an answer is lost at the wrong moment, which tests rarely reach.

`apps/server/src/idempotency/model.rs` checks that promise in every order
of steps: three calls with the same id, a server that may stop between any
two steps, and time passing. It is not a test of cases someone thought of.

## Run it

```bash
cargo test -p agent-workspace-server --lib idempotency::model
```

A few seconds once the crate is built. No database. CI runs it with the
other Rust tests.

## What it covers

| In the model | In the code |
| --- | --- |
| The decision when a call arrives | `idempotency::standing`, the real function, called by `reserve` and `finished` |
| "reserves the id and commits" | `idempotency::reserve_apart`, then the commit, in `ops/dispatch.rs::dispatch` (`apart`) |
| "reserves, writes and keeps its answer in one transaction" | `idempotency::reserve` on `call.tx()`, `O::run`, `idempotency::complete`, one commit |
| "the command commits its write" | `O::run` of an `OWN_TRANSACTION` operation (every `tool_op!`) |
| "gives its reservation back" | `idempotency::release` in `dispatch`'s `Err(error)` arm |
| "keeps its answer under the id" | `idempotency::complete` |
| "time passes" | `stalled` and `lapsed` in `row_for_key!`: 30 minutes (`STALLED_AFTER`) and the 24 hours a row is kept |
| An arrival is one step | The advisory lock `reserve` takes first |

Last confirmed against the code at commit `ac493904`. When any file above
has changed since then, read the change against this table before trusting
a pass, then move the commit here forward.

## What it found

- **Holds:** an id kept with the write is written once, and no caller sees
  it unanswered. An id kept apart is written once within its day, when its
  command fails only before its commit. A caller told done, or replayed,
  has its write. A call whose command refused can run again.
- **The past bug it replays:** before #358 an unanswered reservation lapsed
  after ten minutes (`reserve_briefly`). The model finds the double write in
  seven steps (`the_ten_minute_lapse_ran_a_lost_call_twice`).
- **Open:** the dispatcher gives a reservation back on any error, on the
  claim that a failed command made nothing. Nothing checks the claim. A
  command kept apart that commits and then returns an error is run again by
  a retry (`a_command_kept_apart_that_fails_after_its_commit_is_run_again`).
  `mcp/memory_tools.rs::page_write` is one: it commits, then reads the page
  back, and a failed read-back becomes the call's error.
- **By contract:** past the day an id is kept, a lost call can run again.

## Reading a failure

A failing test prints the shortest order of steps that breaks the promise,
in plain words, then the state it ends in:

```
  1. call 1 arrives
  2. call 1 reserves the id and commits
  3. call 1's command commits its write
  4. call 2 arrives
  5. time passes: TenMinutes
  6. call 2 reserves the id and commits
  7. call 2's command commits its write
  ends in World { row: …, writes: 2, calls: […] }
```

`writes: 2` is the harm: the same request made its change twice.

- If your change is to `standing`, `reserve`, `complete` or `release`, the
  path is a real bug in it. Fix the code, not the model.
- If your change is to the dispatcher's steps (a new transaction, a
  different order, a new place a call can end), the model no longer
  describes the code. Change the matching step in `model.rs`, run it, and
  update the table above.
- Never make a failing property pass by narrowing it or by removing a step
  the code still takes.

## What it does not cover

- A different request sent under a used id (`Mismatch`).
- What a replayed answer contains, and whether a replay checks the caller's
  access again.
- Commands that keep their own request id (`Idempotency::Command`): each
  reserves in its own transaction with its write, which is the "with the
  write" shape, but nothing here checks that each one does.
- The SQL itself. The model trusts that `stalled` and `lapsed` mean what
  the table says; `tests/integration/ops_edges.rs` and the operations'
  own tests run the real statements.
