# Hermes connector fixtures

These fixtures record only the bounded M9 compatibility contract for Hermes
Agent 0.20.0. They must never contain authentication material, prompts,
Message bodies, provider output or reasoning, repository content, absolute
worktree paths, browser state, or raw database values.

`dependency-freeze.txt` is the sorted `uv pip freeze` result from the isolated
Red Hat environment after installing Hermes's pinned `mcp` extra for
Streamable HTTP. Its editable source path is normalized to
`file://<isolated-source>` so the fixture is stable and does not make the
installation path part of the dependency identity.

The final `live-evidence.json` records protocol/tool identities, safe outcome
categories, lifecycle booleans, and opaque fixture labels only. IDs and cursors
are replaced with stable placeholders before commit.

`tool-allowlist.txt` is the exact set enabled in the isolated Hermes profile.
It intentionally omits `work_item_link_pull_request`: M9 permits bounded reads
of already linked pull requests but keeps GitHub publication and integration
management outside the external connector boundary.

Run `python3 tools/hermes-connector/verify-fixtures.py` to check the pinned
dependency checksum, exact sorted allowlist, MCP method presence, live-evidence
shape, and the fixture secret-redaction rules. CI runs the same verifier.
