# Deploying main to the private host (alpha)

The live app on the Red Hat host was last deployed by hand, before the roadmap
workstreams (W0 to W17) merged. This is the one list of what a deploy of main
needs on top of the usual steps in `docs/self-hosted-red-hat.md`. Each merged
PR's description has the detail behind its line here.

## Before

1. **Back up.** About 40 migrations run in one go, and an application-only
   rollback isn't possible once they have (see "Rollback boundary" in
   `docs/self-hosted-red-hat.md`). Take an ordinary backup with the current
   release's `agent-workspace-host-maintenance backup` and `validate` it, as
   that guide's "Retained upgrade" section says.
2. **Disk and memory.** `deploy-release.sh` builds under a 10 GB memory cap
   with 4 cargo jobs (`AGENT_WORKSPACE_BUILD_MEMORY_MAX`, `CARGO_BUILD_JOBS`).
   Leave 15 GB free on the disk for the build and the new release.
3. **Reverse proxy.** Whiteboard images upload up to 4 MiB each. Raise any
   request body cap below 6 MiB for `/api/v1/whiteboards/` and `/api/v1/mcp`
   (agents send files there, base64, up to 4 MiB). Files on messages and tasks
   (W20) upload as the raw body to `/api/v1/conversations/*/files` and
   `/api/v1/projects/*/work-items/*/files`, up to the workspace's
   `files.max_upload_mb` setting (25 MB by default, 100 MB at most): give
   those paths a cap of 100 MiB, and a body timeout long enough for slow phone
   uploads.

## Deploy

Transfer the exact commit (credential-free, as the guide says), then:

```bash
cd /home/bot/deployments/agent-workspace
./deploy/redhat/deploy-release.sh
```

The script builds, installs the release and runs the new build's `preflight`
against the live configuration and database before switching. `preflight` only
reads. It reports:

- the keyring
- the codex and claude CLIs
- GitHub App setup
- the number of trusted webhook origins
- free disk
- the migrations it will run

It stops the deploy when:

- the keyring is configured but can't be read
- the disk is too full
- the database has migrations this build doesn't know
- a migration changed after it ran

Missing optional setup only gives a warning. After the switch, the script waits
for `/health/ready`, checks `/health/version` names the new commit, and turns
on the nightly backup timer if `/etc/agent-workspace/backup.env` exists.

Settings › General shows the running commit under "Server version".

## After: server settings (`sudoedit /etc/agent-workspace/agent-workspace.env`)

Restart the service after editing.

- **Secrets keyring (W1).** Two-factor, AI provider keys, push and webhook
  secrets all need it. Create it with the new release, as the service user,
  outside any Git work tree (etckeeper's `/etc` counts):
  `sudo /opt/agent-workspace/current/bin/agent-workspace-service-command secret-key-init --path <path>`,
  then set `AGENT_WORKSPACE_SECRETS_KEY_PATH=<path>`. Back the keyring up
  separately from the database (`docs/operations/secret-rotation.md`).
- **Crew wakeups.** Add `AGENT_WORKSPACE_WEBHOOK_TRUSTED_ORIGINS=<the crew's private origin>`
  so wakeup webhooks to private or Tailscale addresses are allowed. Outgoing
  webhooks (W14) to private targets need their origins listed there too.
- **Push (W9).** Optional `AGENT_WORKSPACE_PUSH_CONTACT=mailto:...`. Push needs
  the keyring, the app served over HTTPS, and outbound HTTPS to the push
  services (FCM, Mozilla, Apple, Windows). VAPID keys are made on first use.
- **Codex Cloud (W7).** A `codex` CLI on the service PATH that supports
  `codex cloud` (0.163 or later), and a Codex provider account signed in with
  ChatGPT. `AGENT_WORKSPACE_CODEX_CLOUD_POLL_MILLISECONDS` is optional.
- **Claude Code (W12).** The `claude` CLI (2.1.0 or later) on the service PATH,
  or `AGENT_WORKSPACE_CLAUDE_BINARY`. Sign each Claude provider account in
  once, with `CLAUDE_CONFIG_DIR=<account home>/.claude claude auth login`, or
  give it an API key. Cloud Claude runs need an Anthropic API key account and
  a Managed Agents environment id in the project's environment settings.
- **Own harnesses (W12).** Optional `AGENT_WORKSPACE_COMMAND_HARNESS_DIRS` for
  command harnesses.
- **GitHub (W8, W11).** Optional GitHub App settings. To import GitHub issues,
  give the App the Issues: read permission.
- **Email (W19).** Nothing goes in this file: an owner sets SMTP in the app
  (below). The SMTP password is kept in the keyring, so the keyring must exist,
  and the host needs outbound access to the mail server's port (587 for
  STARTTLS or 465 for TLS). Links in emails use the public origin, so
  `AGENT_WORKSPACE_PUBLIC_ORIGIN` must be the address people open.
- **Later, reminders and scheduling (W21).** Nothing goes in this file. The
  migration `20261017160000_later_and_scheduling.sql` runs with the rest.
  Reminders and scheduled messages are background jobs, so they fire only
  while the service runs. Their push notifications offer Done and Snooze
  through the updated service worker (`dist/sw.js`, part of the web build); an
  installed home-screen app picks it up the next time it opens, and on Android
  re-adding the app forces it if the old buttons stay.
- **Files and link previews (W20).** Nothing new goes in this file. Uploaded
  files live in `<data root>/files` (`/var/lib/agent-workspace/files`), which
  the server creates; the nightly backup now includes it (installed
  `agent-workspace-backup` and any `BACKUP_DATA_PATHS` in `backup.env` must
  list `files`). Uploads count toward `AGENT_WORKSPACE_DATA_QUOTA_BYTES` and
  the free-disk watermark like other data. Link previews fetch public pages
  from the server, so it needs outbound HTTP and HTTPS; private, loopback and
  Tailscale addresses are always refused. The CSP is unchanged: pictures and
  preview images are served from the app's own origin.

## After: in the app

1. Sign in, open Settings › General and check "Server version".
2. Settings › AI and models: add the Anthropic key and press Test connection.
   Set a monthly budget if you want one.
3. Settings › Provider accounts: connect the Claude and Codex accounts runs
   should use.
4. Project settings › Repositories: connect the GitHub App for each project.
5. On the phone: open the site, add it to the home screen, then turn on push in
   Settings › Notifications from the installed app.
6. Launch one task from its page and watch it on Runs.
7. Settings › Workspace › Email: enter the SMTP server, sender address and
   password, save, and press Send test email. Then confirm your own address in
   Settings › Profile; password resets and email notifications only go to
   confirmed addresses, and "Forgot your password?" appears on sign-in once
   email works.
8. Settings › Workspace › Server: check that the checks are green and the last
   backup shows up after the first nightly run.
9. In any channel, type `/remind me to check the deploy in 2 minutes`. The
   reminder should reach the inbox and the phone, with Done and Snooze on the
   push. Later (the bookmark in the rail) lists it.
10. Files (W20): Settings › Workspace has the upload size limit (25 MB), the
    workspace's file storage (10 GB) and link previews (on). Attach a picture
    to a message once to check uploads get through the proxy. On Android,
    remove and re-add the home-screen app so sharing pictures and files to it
    works (its share target changed).

## Nightly backups

Fill in `/etc/agent-workspace/backup.env` from
`deploy/redhat/backup.env.example` (off-host remote, age recipients, keyring
path), as `docs/operations/backups.md` describes. The next deploy enables the
timer; to start it now, run that guide's install step. Then run the restore
drill once.

Each run writes its outcome (start, finish, which steps worked; no paths or
remotes) to `/var/lib/agent-workspace/backup-status.json`, which the Server
page reads. Set `BACKUP_STATUS_FILE` in `backup.env` to put it elsewhere, but
it must stay in the app's data root for the page to see it.
