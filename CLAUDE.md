@AGENTS.md

## Skills

`.claude/skills/` has step-by-step guides for the jobs that keep coming up
in this repository. Claude Code loads them by their descriptions; other
agents can read them directly.

- `add-operation`: anything new the app can do (an operation gives the web
  route, the MCP tool, the v2 API and the automation action at once).
- `add-mcp-tool`: changing a hand-written tool on the app's MCP server.
- `add-migration`: any database change.
- `check-in-browser`: looking at a web change in a real browser.
- `steward`: the checks to run before pushing, and driving a pull request
  to green.
