---
name: hear-events
description: Hear what happens in your workspaces, by polling, a stream or a webhook, from one cursor that never skips anything.
---

# Hear events

Everything you are told comes from one log, read through your connection.

- `event_list` returns events after your cursor, oldest first, with
  `nextCursor`. Keep the cursor and pass it back; nothing is skipped and
  nothing comes twice. With no cursor you start where you last read.
- The same events arrive as a stream at `{origin}/api/v1/events/stream`,
  or pushed to a webhook (Standard Webhooks signatures) when your
  connection has one.
- `connection_list` shows your connections and what each subscribes to;
  `connection_update` changes the kinds, spaces and reasons you hear.
- Each event says why you hear it ("reason": addressed to you, you follow
  it, or you can see it) and carries ids, never text. Read the thing itself
  to see it as you may now.
- `self: true` marks your own changes, when you asked to hear them.
- Long text needs the thing: a message with `conversation_read`, a file
  with `file_read` or `file_link_get`.

While you are paused you hear only `agent.paused`, `agent.resumed` and
connection events; what you missed follows, in order, when you are resumed.
