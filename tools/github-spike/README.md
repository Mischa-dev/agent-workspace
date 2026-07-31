# GitHub delivery spike

This crate implements the reusable M0 GitHub App boundary and a webhook-only
listener on `127.0.0.1:3101`.

- App JWT lifetime is nine minutes with a 60-second issued-at skew.
- Each repository-scoped installation token is requested for one child
  operation and discarded afterward.
- Credentialed Git receives the opaque token only through a temporary
  `GIT_ASKPASS` helper and a child-only environment.
- System Git configuration, inherited credential helpers, prompts, and
  repository hooks are disabled.
- Webhooks require `X-Hub-Signature-256`, `X-GitHub-Delivery`, an allowed event,
  and the exact spike repository identity. Duplicate deliveries are recognized.

Private keys and webhook secrets must remain outside the repository with
owner-restricted permissions.

Run the disposable live delivery proof with the App installed only on the spike
repository:

```sh
AGENT_WORKSPACE_GITHUB_APP_ID=... \
AGENT_WORKSPACE_GITHUB_INSTALLATION_ID=... \
AGENT_WORKSPACE_GITHUB_PRIVATE_KEY_PATH=... \
cargo run -p github-spike --bin live
```

The command requests a fresh repository-scoped installation token for each
remote child operation, clones the spike repository, pushes a unique branch,
opens and reads checks for a pull request, then closes the pull request and
deletes the branch. Its JSON output contains no credential material.

The temporary webhook configuration is updated with the App JWT and secret
files are read by path so neither credential appears in process arguments:

```sh
AGENT_WORKSPACE_GITHUB_APP_ID=... \
AGENT_WORKSPACE_GITHUB_PRIVATE_KEY_PATH=... \
AGENT_WORKSPACE_GITHUB_WEBHOOK_URL=https://example.trycloudflare.com/github/webhooks \
AGENT_WORKSPACE_GITHUB_WEBHOOK_SECRET_PATH=... \
cargo run -p github-spike --bin configure-webhook
```
