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
