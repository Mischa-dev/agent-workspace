# Delivery fixtures

Examples of everything the server sends an agent, for SDKs and receivers in
any language to test against. The Rust crate `crates/delivery-contract`
checks these files and checks a real server against them; the server's own
tests check that what it sends matches them.

| File | What it shows |
| --- | --- |
| `envelope.json` | One event, as every transport carries it. |
| `poll-page.json` | `GET /api/v1/events` (and the `event_list` operation). |
| `catalogue-entry.json` | One entry of `GET /api/v1/events/catalogue`. |
| `stream.txt` | The same address with `Accept: text/event-stream`. |
| `webhook-standard.json` | A webhook push. |

The webhook fixture holds the `secret` that signed it, the `headers` sent and
the raw `body`, so a verifier can be tested on them as they are. The secret is
the published Standard Webhooks test secret, not a real one.

## The rules a client can rely on

- An event carries ids, kinds and field names, never what someone wrote.
  Fetch text live (`conversation_read`, `task_open`, ...) with your own
  access.
- A shape may grow: new keys can appear anywhere, so ignore keys you don't
  know. Keys shown here don't go away or change type.
- `actor` and `requestedBy` may be `null`; `data` depends on `type` (see the
  catalogue); `self: true` marks an event caused by the reader.
- `cursor` is opaque. Pass it back as `cursor`, or as `Last-Event-ID` on a
  stream, to read on from just after that event. On a stream each event's
  SSE `id` is its cursor and its SSE `event` is its `type`.
- `id` (`evt_<uuid>`) is the same on every transport and every retry: use it
  to drop a repeat.

## Verifying a push

Standard Webhooks: `webhook-signature` is one or
more space-separated `v1,<base64>` values, each an HMAC-SHA256 of
`{webhook-id}.{webhook-timestamp}.{body}` keyed by the base64-decoded secret
after `whsec_`. Accept the push if any one matches.

Refuse a timestamp more than five minutes from your clock.

## Running the suite against a server

```text
AW_TOKEN=<an agent's token> cargo run -p agent-workspace-delivery-contract -- \
  server --origin <server address> --workspace <workspace id or slug>
```

With `--workspace` it sets itself a one-time follow-up a minute ahead and
waits for it, to check long-poll, stream and resume with a real event.
