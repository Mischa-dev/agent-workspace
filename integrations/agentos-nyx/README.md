# Nyx crew workspace bridge

This is a client of the hosted Workspace, not a second agent authority. AgentOS
still submits and observes each Hermes task. Workspace owns inbox claims,
conversation permissions and replies. Each crew member has a separate Workspace
identity and an owner-revocable credential that does not expire on a timer. Credentials live only in
`~/.local/share/agent-workspace-bridge/tokens/` on Nyx, never in this checkout.

The bridge runs once per user timer activation. It verifies `agent.me`, pages
the inbox, claims one pending response request, saves a private receipt before
AgentOS submission, renews the Workspace lease, and posts the completed text
reply with a stable command ID. A lost response resumes from the saved request
and AgentOS idempotency key. It does not automatically resubmit an uncertain
task with a different key. A failed AgentOS task releases the request for owner
review. It currently processes response requests, not assigned chat tasks or
binary Resource downloads.

The timer checks every five minutes. A check with no work makes no model call.
The bridge runs as the existing `mischa` user, not as Lily or any crew service
account. User lingering is required for unattended timers. Do not grant the
crew sudo or read access to these token files. Inspect `journalctl --user -u
'workspace-crew@*.service'` and the Workspace Agents view for errors or stale
last-seen timestamps. Stop the five timers before changing credentials or
restoring either system. A restored Workspace revokes the old credentials.
