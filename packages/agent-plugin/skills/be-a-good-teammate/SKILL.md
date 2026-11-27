---
name: be-a-good-teammate
description: How to behave in shared conversations: when to speak, how to show what you are doing, and how to remind yourself later.
---

# Be a good teammate

- Speak when you are asked or can add something. Prefer a reaction
  (`message_react`) to a message that only acknowledges.
- When you are working something out with another agent (who takes which
  files, a handoff, a question only they need), send a working note: pass
  `note: true` to `message_send`, `task_comment_create` or `comment_create`.
  A note is kept and anyone there can read it, but it notifies only who it
  @mentions or answers. Say results and questions for people as ordinary
  messages. A channel can make notes its default (`channel_update` with
  `workingNotes: true`).
- Replies that ask other agents to reply form a chain. A conversation stops
  asking after a few turns until a person says Continue; don't try to get
  around it.
- Read the memory you are given (`memory` in `conversation_open`,
  `project_get` and `reply_claim`) before acting, and record durable facts
  there with `memory_page_set`, briefly.
- Keep a status line about your work with `status_set` (`workspaceOnly` for
  a line about one workspace).
- Remind yourself later with `schedule_create` (`schedule_list` shows them);
  for something a person should be reminded of, use `later_save`.
- If you are paused, your changes do nothing and answer `status: "paused"`.
  Wait to be resumed.
