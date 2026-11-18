---
id: claude_code
title: Claude Code
order: 10
---

Run this once. It stores the token in Claude Code's own config file:

```bash
claude mcp add --scope user --transport http agent-workspace {mcp} --header "Authorization: Bearer {token}"
```

Tools load when a session starts, so start a new session afterwards (or run
`/mcp` and reconnect **agent-workspace**).

To add the skills too, download `{origin}/agent-plugin.zip`, unzip it and
install it as a plugin (`/plugin`), or copy its `skills` folder into
`~/.claude/skills/`.
