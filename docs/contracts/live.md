# The live contract

The server's live role holds the connections that hear things as they
happen. It is a small published contract, so a separate live service (one
that sleeps idle connections, or runs at the edge) can take its place
without the server changing. The server runs its own implementation in
process by default.

There are two sides:

- The **database side** (`api` and `work`) decides what each connection may
  hear, signs that into a ticket, and publishes every frame with the
  audiences it is for.
- The **live side** holds connections. It needs no database: it checks
  tickets with a public key and routes each frame to the connections whose
  ticket names one of its audiences.

The conformance suite (`apps/server/tests/integration/live_contract.rs`)
checks an implementation. It runs in CI against the in-process live role and
against a separate `live`-only instance. Set
`AGENT_WORKSPACE_LIVE_CONTRACT_URL` to run it against another service.

## Audiences

An audience is written `<kind>:<id>`:

| Audience | Who is in it |
| --- | --- |
| `workspace:<id>` | Everyone in the workspace |
| `space:<id>` | Everyone who reads the space |
| `conversation:<id>` | The members of a restricted conversation (a DM, a group or a private channel), by its root |
| `account:<id>` | One account |
| `document:<kind>.<id>` | The connections watching a page, sheet or whiteboard (see Document rooms) |

## Tickets

`GET /api/v2/live/ticket[?after=N]`, signed in, answers:

```json
{
  "ticket": "<payload>.<signature>",
  "url": "wss://workspace.example.com/live/v1/socket",
  "position": "18342",
  "expiresAt": 1763395200
}
```

The payload and the signature are both base64url without padding. The
signature is Ed25519 over the payload's base64url text. The payload is JSON:

```json
{
  "v": 1,
  "kid": "k1",
  "account": "0193…",
  "session": "0193…",
  "kind": "web",
  "audiences": ["workspace:0193…", "space:0193…", "conversation:0193…", "account:0193…"],
  "from": "18342",
  "expires": 1763395200
}
```

- `from` is a log position, written as a string because positions can pass
  JavaScript's safe integers. Durable frames after it come over the
  connection.
- `expires` is in Unix seconds. A ticket lasts five minutes, and the client
  fetches a new one before then.
- `kid` names the signing key. The keys are at `GET /api/v2/live/key`
  (public): `{"keys": [{"kid": "k1", "alg": "Ed25519", "key": "<32 bytes, base64url>"}]}`.

A live service must refuse a ticket that fails its signature, names an
unknown key, has expired, is not version 1, or is over 48 KiB.

## The connection

The client opens `GET /live/v1/socket` as a WebSocket and sends, within five
seconds:

```json
{"type": "hello", "ticket": "<ticket>"}
```

The live side answers with one of these:

- An error, then a close with code 4401, when the ticket is refused:
  `{"type": "error", "reason": "ticket"}`.
- A resync, then a close with 1012, when it can't vouch for every durable
  frame after the ticket's `from`. The client resumes through `api` first:
  `{"type": "resync", "reason": "resume_through_api", "position": "18342"}`.
- Otherwise, the durable frames it holds after `from` that the ticket may
  hear, then `{"type": "ready", "contract": "live.1", "position": "18350"}`.

After `ready`, the connection hears:

- Durable frames, in position order and each at most once. These are
  protocol 2's `event` frames: `{"type": "event", "position": "18351", "event": {…}}`.
  A frame whose position is at or below what the connection has already
  passed is never sent.
- Ephemeral frames (presence, typing, streamed text), which may be lost.
- `{"type": "ticket.stale"}` when what it may hear may have changed. The
  client resumes through `api` with its position, then sends the new ticket.
  A connection that hasn't sent one 15 seconds later is closed with 4401.
- `{"type": "ticket.expired"}`, then a close with 4401, when the ticket runs
  out.
- `{"type": "reconnect", "afterMs": 1000, "reason": "server_draining"}`, then
  a close with 1012, when the live side is stopping.
- `{"type": "resync", "reason": "too_far_behind", "position": "…"}`, then a
  close with 1012, when the connection fell too far behind.

The client sends a new ticket on the open connection with
`{"type": "ticket", "ticket": "<ticket>"}`. It must be for the same account
and session, or the connection closes with 4401. Its audiences replace the
old ones from the next frame on, and the live side answers
`{"type": "ticket.accepted", "position": "…"}`.

## Document rooms

A ticket can name rooms: `document:page.<id>`, `document:sheet.<id>` or
`document:whiteboard.<id>`. The client asks for them when it fetches a
ticket or resumes, with `documents=page.<id>,sheet.<id>` (at most 20). The
server checks that the caller may see each one now, and the answer's
`documents` lists the rooms the ticket names. A document's changes and its
watchers' cursors are published to its room. When access changes, the
ticket goes stale, and the new ticket leaves out the rooms the caller lost.
Each watcher ignores cursor frames whose `connection` is its own.

When a session signs out, its connections close with 4401.

## Resume through `api`

`GET /api/v2/live/resume?after=N`, signed in, answers with what the caller
missed after `N`, checked against what they may hear now, and a ticket that
starts where that ends:

```json
{
  "events": [{"type": "event", "position": "18343", "event": {}}],
  "position": "18350",
  "more": false,
  "resync": false,
  "ticket": "…",
  "url": "…",
  "expiresAt": 1763395200
}
```

- When `more` is true, the client asks again from `position`.
- When `resync` is true, `N` is too far back to replay (more than 5,000
  positions). The client refetches what it shows, then follows from
  `position`.

The live side keeps no history. It keeps a short window of recent durable
frames, so a ticket that starts a little behind the newest frame catches up
without a gap.

## Publishing

In process, publishing is a function call. A separate live service takes
`POST /live/v1/publish`. The body is JSON:

```json
{
  "sentAt": 1763394900,
  "items": [
    {"type": "frame", "audiences": ["space:0193…"], "position": "18351", "frame": "{\"type\":\"event\",…}"},
    {"type": "frame", "audiences": ["conversation:0193…"], "exceptAccount": "0193…", "frame": "{\"type\":\"ephemeral\",\"kind\":\"typing\",…}"},
    {"type": "ticket.stale", "audiences": ["workspace:0193…"]},
    {"type": "revoke", "session": "0193…"}
  ]
}
```

- The `x-live-signature` header is `<kid>.<signature>`: Ed25519 over the
  body's bytes, with the same keys as tickets.
- A publish whose signature fails, or whose `sentAt` is more than 60 seconds
  from now, is refused with 401.
- `frame` is sent to connections as it is.
- `position` marks a durable frame. Durable frames arrive in position order,
  and a repeat (a publish sent twice) is dropped.
- `exceptAccount` keeps a frame from the account's own connections.

The server sends durable frames from one `work` process holding the
`live.publisher` lease. Its cursor is stored, so a new holder carries on
without a gap. Each `api` process sends the ephemeral frames it makes. Set
`AGENT_WORKSPACE_LIVE_URL` on every process to the live service's address.
A `live` process of this server then takes its frames only from publishes.
