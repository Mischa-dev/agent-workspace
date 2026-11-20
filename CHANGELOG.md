# Changelog

Releases are numbered `0.N.0` until the public release. Each section says
what an operator needs to know: what changed, and anything to do by hand
before or after upgrading (a setting renamed, a step before stopping the old
server). Read the section for every release between yours and the new one.

## 0.1.0 (not released yet)

The first numbered release: the rebuilt foundations.

- **Upgrading from an earlier build** needs the steps on the deploy notes
  ("At the deploy"): let the old server drain before stopping it, then
  start the new one, which converts existing rows once as it migrates.
- Several `work` processes may start at once: one takes the
  `server.startup` lease and migrates, the others wait
  (`AGENT_WORKSPACE_STARTUP_WAIT_SECONDS`, default 600) and then start.
- Update check: set `AGENT_WORKSPACE_UPDATE_CHECK_URL` to the release feed
  of the source you install from, and the Server page and `preflight` say
  when a newer release is out. It sends nothing but the running version in
  its user agent; `AGENT_WORKSPACE_UPDATE_CHECK=off` turns it off.
- Files in a DM or group never go to a workspace's own bucket; they stay in
  the server's store.
