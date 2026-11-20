# Running your own Agent Workspace

Everything here is for someone running the server for themselves or their
team. One machine with Docker or Podman is enough; nothing it does needs a
service outside it (the CI checks that, see "Offline" below).

1. [Install](install.md): the container stack, first run in the browser.
2. [Configuration](configuration.md): every `AGENT_WORKSPACE_*` setting.
3. [TLS and a public address](tls.md): serving it beyond localhost.
4. [Backups and restore](backups.md).
5. [Upgrades](upgrades.md): moving to a new release, and back.
6. [Moving a workspace](moving-a-workspace.md): an archive out of one
   server and into another.
7. [Small machines](small-machines.md): what it needs, and how to run it
   on less.
8. [The container image](container.md): what runs, and building it.
9. [Serving the web app from a static host](static-web-app.md).

For hosting at larger scale, the shapes and what each costs idle are in
[`docs/hosting/shapes.md`](../hosting/shapes.md). The Red Hat guide,
[`docs/self-hosted-red-hat.md`](../self-hosted-red-hat.md), is one worked
example of running it without containers.

## Offline

A self-hosted server needs no outside service: no sign-in provider, code
host, mail server, object store, model provider or metrics collector. Each
is optional and off until configured. Three checks hold this:

- `tests/integration/offline.rs` starts the server with only its database
  and a data folder and goes through first run, a message, a task, a page,
  an upload and a whole-workspace export.
- `scripts/check-web-offline.mjs` (run with the web asset checks) fails if
  the built web app loads a script, style, font or image from another
  origin.
- The image smoke step (`tools/ci/image-smoke.sh`) starts the Compose stack
  a second time on a network with no way out and waits for it to be ready.

Things that reach out only do so once someone sets them up: link previews
fetch the pages people post, webhooks and agent wake-ups go where a
workspace points them, and model providers are the ones a workspace adds.
