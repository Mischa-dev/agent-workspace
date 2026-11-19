# TLS and a public address

The server speaks plain HTTP and expects a proxy in front of it for TLS.
Over plain HTTP it serves `localhost` only: any other address must be
`https`, because sign-in cookies are marked secure.

1. Point a name at the machine and put a TLS proxy in front of port 3100:
   Caddy, nginx, Traefik, a tunnel, or your load balancer. The proxy must
   pass WebSocket upgrades through (live updates use `/api/v1/realtime`)
   and allow request bodies as large as your biggest upload.
2. Set `AW_PUBLIC_ORIGIN=https://<your name>` in `deploy/container/.env`
   (it becomes `AGENT_WORKSPACE_PUBLIC_ORIGIN`), and restart:
   `docker compose up -d`.
3. Keep the app's port on loopback (the Compose file publishes it on
   `127.0.0.1` only), so nothing reaches it except through the proxy.

A minimal Caddy site, for example:

```
workspace.example.com {
    reverse_proxy 127.0.0.1:3100
}
```

Caddy gets and renews the certificate itself and passes WebSockets through
as it is.

## Behind the proxy

- **Client addresses.** Sign-in and reset limits key on the caller's
  address. Name the header your proxy puts it in with
  `AGENT_WORKSPACE_CLIENT_ADDRESS_HEADER` (such as `x-real-ip`; see
  [configuration](configuration.md)), and only when every request comes
  through that proxy. Unset, no address is read from requests.
- **Other origins.** If the web app is served from somewhere else (a CDN),
  see [serving the web app from a static host](static-web-app.md).
- **Single sign-on** redirects back to `<public origin>/auth/...`, so the
  public origin must be the address people actually use.
