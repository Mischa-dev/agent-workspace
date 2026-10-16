# Plan: tag an agent to hand it work

October 8, 2026. A draft for Mischa. It covers the Agent Workspace half of a
"Claude tag" (mention `@claude` in a thread and Claude Code picks the work up,
keeps you posted, and answers there) and the same thing for Codex. The runners
that start a Claude Code or Codex session when they are woken are out of
scope; this plan defines what they talk to.

## What already exists

Most of the plumbing is here (`replies.rs`, `mcp/replies.rs`, migration 0038):

- A person's @mention of an agent in a channel or thread creates a **reply
  request** in the message's own transaction.
- The agent is woken by an inbox event and, if its connection has one, a
  **signed webhook** (`inbox.available`, HMAC over `timestamp.body`, retried
  with backoff).
- The agent **claims** the request (a 15-minute lease, renewed by claiming
  again) and gets the last 30 messages as context.
- It answers with **one** `reply_send`, which posts the message and closes the
  request in the same transaction. `reply_release` hands it back with a reason.
- The conversation shows one status line per agent: waiting, "writing a
  reply…", passed, handed back, didn't answer in time (with Ask again).

That is built for chat replies that take a minute. Handing an agent real work
needs a few more things.

## What's missing for tagging

1. **Long work on one request.** A coding task takes many minutes and several
   messages. Today the first message closes the request, and the only sign of
   life is "writing a reply…".
2. **Follow-ups go to the same agent.** After you tag `@claude` in a thread,
   your next message there ("also fix the test") should reach Claude without
   tagging it again, and land in the same session.
3. **A session per thread.** The runner needs to know which Claude Code or
   Codex session belongs to a thread so it resumes it instead of starting cold,
   and people want a link to open it.
4. **Tagging in a channel opens a thread.** A top-level `@claude` message
   should get its answer in a thread started from that message, as in Slack.
5. **Stop.** A person needs a Stop button that the runner hears about.
6. **A wakeup the runner can act on.** The webhook says only "check your
   inbox". It should say what happened (asked, follow-up, stopped) and where,
   so a runner can route it without a round trip.
7. **Setup.** Adding "Claude" or "Codex" as an agent with a webhook should be
   one form, ending with the token and the runner settings to copy.

## The pieces, in build order

Pieces 1, 2 and 4 to 7 shipped together as roadmap workstream W5; the
protocol a runner implements is [docs/agent-runner.md](../agent-runner.md).
Piece 3 (sessions) is roadmap item A3, built with Launch (W7).

### 1. Progress and interim messages (built, W5)

- `reply_progress(requestId, generation, status)`: a one-line status (up to
  200 characters, like "Running the server tests") shown under the agent's
  name while it works. It renews the lease, so a runner that reports progress
  never expires mid-task.
- `reply_send` gains `final` (default `true`). With `final: false` the message
  is posted and the request stays claimed, so an agent can post a plan, a
  question, or a PR link and keep going.
- The status line reads "Claude is working · Running the server tests ·
  updated 2 min ago".
- Migration: `reply_requests.progress_text`, `progress_at`.

### 2. Follow-ups stick to the thread (built, W5)

- When a person writes in a thread where an agent was tagged and hasn't
  finished being asked (a pending or claimed request, or a reply in the last
  N messages), that agent is asked again without a new mention.
- A claimed request is never superseded by a follow-up. The follow-up becomes
  a new pending request queued behind it, and the wakeup is marked
  `followUp: true` with the claimed request's ID, so the runner feeds it into
  the running session.
- Tests: a follow-up reaches only agents already in the thread; it never
  reaches an agent who can't see the thread.

### 3. Agent sessions (roadmap A3, in W7)

- A small `agent_sessions` table: agent, thread, runner kind (`claude_code`,
  `codex`, other), an opaque external session reference, an optional `https`
  link, state, last activity. Separate from Agent Runs and provider sessions,
  as AGENTS.md requires.
- `session_bind(conversationId, externalRef, url?)` for the runner; the claim
  result and the wakeup carry the existing binding so the runner resumes it.
- The thread header shows "Claude · Open session".

### 4. Channel tags open a thread (built, W5)

- A tagged top-level channel message: at claim time the server starts a
  thread from the message (the same code as `thread_start`) and moves the
  request there, so the work and its follow-ups stay in one place. DMs and
  groups keep answering inline.
- Project channels and their threads show the agents' status lines too.
  Today only DMs, groups and their threads do (`ReplyStatusList`), so a tag
  in a project thread gives no sign the agent picked it up.

### 5. Stop (built, W5)

- A Stop button on a working agent's status line. The request becomes
  `cancelled`; the runner's next `reply_*` call gets a conflict that says it
  was stopped, and a `reply.cancelled` webhook goes out at once.

### 6. Typed wakeups (built, W5)

- The webhook body gains `event` (`reply.requested`, `reply.followup`,
  `reply.cancelled`), `conversationId`, `threadId`, `sessionRef` and the
  `generation` to use. Still no message text: the runner claims to read it, so
  every read goes through `conversation_visible_to`.
- `inbox.available` stays accepted by existing bridges (Nyx, Muse) for one
  release.

### 7. Setup and the runner contract (built, W5)

- On the Agents page, "Add Claude Code" and "Add Codex" presets: handle
  (`@claude`, `@codex`), webhook URL, and a result screen with the token,
  webhook secret and a settings block for the runner.
- `docs/agent-runner.md`: the protocol a runner implements (verify the
  signature, claim, bind the session, progress, interim and final replies,
  stop), so the Claude Code and Codex halves can be built against it.

## Codex

Decided October 8, 2026: Codex uses the same protocol. `@codex` wakes an
external Codex runner exactly as `@claude` wakes Claude Code, so there is one
code path. Routing `@codex` to the built-in Codex App Server worker
(`start_run_in`) can come later if it's wanted.

## Not now

- The Claude Code and Codex runners themselves.
- Permission prompts relayed from a session into the thread (approvals exist
  for Agent Runs; extend them once a runner needs it).
- Attaching files from a session.
