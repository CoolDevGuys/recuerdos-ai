# Web UI

The daemon serves a browser UI at **`/ui`** on its API port, at
`http://127.0.0.1:7070/ui` by default. The UI is a static single-page app
compiled into the binary. It reads and writes memories through the same `/v1` API
as every other client, under the same API-key scopes.

> **Status:** phase P0 of [ai-specs/web-ui-plan.md](../ai-specs/web-ui-plan.md).
> You can sign in and browse a filterable list of your memories. Editing,
> the mental map and category management come in the later phases.

## Signing in

Paste an API key, issued with:

```bash
recuerdos-ai key issue --user <you> --scopes read,write
```

| Scope | What the UI offers |
|---|---|
| `read` | Browse memories |
| `write` (with `read`) | Browse and edit, once the editing phase lands |
| `write` only | Nothing to show: the UI says the key cannot read memories |
| `admin` | Everything |

The key is kept in the tab's `sessionStorage`. It is gone when the tab closes, and it is
never put in a URL or shared with other tabs. **Sign out** clears it.

With `[auth].mode = "none"`, the UI skips sign-in. It shows a banner as a
reminder that anyone who can reach the port acts as the `default` user.

## Turning it off

```toml
[ui]
enabled = false
```

With the UI off, `/ui` is not mounted and answers like any path that does not exist. The
API is unaffected.

## Building

The bundle lives in `web/` (React, TypeScript, Vite, Tailwind, pnpm) and is
built into `web/dist`, which `cargo build` embeds.

```bash
just web-build     # pnpm install + build → web/dist
cargo build --release --bin recuerdos-ai
```

- **Without `web/dist`**, the daemon still builds. `/ui` then answers `503`
  with the command above. This keeps the test suite, the eval and the dev
  container free of Node.
- **Release binaries and the Docker image** always build the bundle first.
- **`--no-default-features`** drops the `ui` feature, and with it the embed,
  entirely.

For UI development, run the daemon on `:7070` and start Vite's dev server.
Vite proxies `/v1` to the daemon:

```bash
just web-dev       # http://localhost:5173/ui/
just web-test      # type-check + unit tests
```

## Security

- **Static files only.** `/ui` serves the bundle's files, none of which
  carry data. Static files need no credential, and the rate limiter does not
  count them.
- **Strict CSP.**
  - `script-src 'self'`: no inline or third-party scripts.
  - `connect-src 'self'`: the page cannot send anything to another host.
  - `frame-ancestors 'none'`.
  - The response also carries `nosniff` and `no-referrer`.
- **Memory content is rendered as text, never as HTML.**
