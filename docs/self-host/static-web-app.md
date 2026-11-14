# Serving the web app from a static host

The web app is static files: `corepack pnpm --dir apps/web build` writes
`apps/web/dist`, with no origin built in. The server serves those files
itself by default (`AGENT_WORKSPACE_WEB_ROOT`), which is all a self-hoster
needs. They can also be served by any static host or CDN, with the server
answering only the API.

## How the app finds the API

At start the app reads `/client-config` from the origin it was loaded from.

- Served by the server, that is the server's own answer, and the app keeps
  to the origin it was loaded from.
- On a static host, put a file named `client-config` (no extension) next to
  `index.html`:

  ```json
  {"apiOrigin": "https://api.example.com"}
  ```

  The app then reads `https://api.example.com/client-config` for the rest:
  the live origin, the sign-in methods and the server's release.

With no answer at all (offline, or an older server), the app keeps to its
own origin.

## What the server needs

- **The same site.** The session cookies are `SameSite=Strict`, so the app
  and the API must be on one site, such as `app.example.com` and
  `api.example.com`. Different sites can't share a signed-in session.
- `AGENT_WORKSPACE_PUBLIC_ORIGIN`: the API's origin, `https://api.example.com`.
- `AGENT_WORKSPACE_ALLOWED_ORIGINS`: every origin the app is served from,
  such as `https://app.example.com,https://api.example.com`. Only these get
  CORS headers, and writes from any other origin are refused.
- `AGENT_WORKSPACE_LIVE_ORIGIN`, only when a separate live service answers
  the live connection.

The app reads the request token it sends with every write from
`/api/v1/auth/csrf`, since a page on another origin can't read the API's
cookie.

## What the static host needs

- Serve `index.html` for every path that isn't a file (the app routes in
  the browser), and the `client-config` file above.
- A content security policy at least as strict as the server's own, with
  the API and live origins added to `connect-src` and the API origin to
  `img-src`, for example:
  `default-src 'self'; script-src 'self'; style-src 'self'; img-src 'self' data: blob: https://api.example.com; connect-src 'self' https://api.example.com wss://api.example.com; font-src 'self'; worker-src 'self'; object-src 'none'; base-uri 'none'; frame-ancestors 'none'`.
- Cache `assets/` for a long time (their names carry a hash) and
  `index.html`, `sw.js` and `client-config` for a short one.

## Not yet

- Sign-in through WorkOS or Google finishes on the server's origin, so it
  brings people back to the server's copy of the app rather than the static
  host's. Password sign-in works from either.
- Links in mail and notifications are built from the public origin, so they
  open the server's copy of the app.
- Answering a notification from its buttons posts to the origin the app was
  installed from.
