# REST API

**Status: Phase 1.** Authentication is live; memory endpoints arrive in
Phase 2. See [implementation-plan.md](../implementation-plan.md) for what
lands when.

## Authentication

Every route under `/v1` requires an API key as a bearer token:

```bash
curl -H "Authorization: Bearer ra_live_b99f884a…" localhost:7070/v1/ping
```

Keys are issued from the CLI (`recordagent key issue`) — there is no
endpoint that mints them, by design. See
[configuration.md](configuration.md#authentication) for the
`[auth].mode = "none"` single-user escape hatch.

The scheme name is case-insensitive (`Bearer`, `bearer`). A missing,
malformed, unknown, wrong-secret or revoked key all produce the identical
401 — which of those it was is not information the API hands out.

### Scopes

| Scope | Grants |
|---|---|
| `read` | Reading memories (Phase 2) |
| `write` | Creating and modifying memories (Phase 2) |
| `admin` | Everything |

`write` does **not** imply `read`: a write-only ingestion key — an agent
that saves memories but can never read them back — is a useful thing to be
able to hand out. A key missing the scope a route needs gets `403`, not
`401`: the credential was valid, the permission wasn't.

## Error envelope

Every error, from every route, has this shape:

```json
{"error": {"code": "unauthorized", "message": "invalid API key"}}
```

Branch on `code`; `message` is for the human reading the log.

| `code` | HTTP | Meaning |
|---|---|---|
| `validation_failed` | 400 | The request was malformed |
| `unauthorized` | 401 | No usable credential (`WWW-Authenticate: Bearer`) |
| `forbidden` | 403 | Valid key, missing scope |
| `not_found` | 404 | No such resource |
| `conflict` | 409 | Violates a uniqueness rule |
| `internal` | 500 | Server fault |

`internal` responses always carry the literal message `"internal error"`.
Details (paths, SQL, driver text) go to the server log with the request id
— they are not for the client.

## Routes

### `GET /healthz`

Unauthenticated by design: a health check that needs a credential is
useless to a load balancer or Docker's `HEALTHCHECK`.

```json
{"status": "ok"}
```

### `GET /version`

Unauthenticated.

```json
{"version": "0.1.0", "git_sha": "a1b2c3d4e5f6"}
```

### `GET /v1/ping`

**Temporary.** Requires any valid key. Exists to prove authentication
end-to-end until Phase 2 adds real routes, then it is removed.

```bash
curl -H "Authorization: Bearer $KEY" localhost:7070/v1/ping
```

```json
{"user": "alex", "scopes": ["read", "write"]}
```

## Request ids

Every response carries `x-request-id` (generated if you don't send one).
Include it when reporting a problem: it ties the response to the server
log line that has the real error.

## Coming in Phase 2

`POST /v1/memories:direct`, `POST /v1/memories/search`,
`GET/PATCH/DELETE /v1/memories/{id}`, `GET /v1/memories/export`,
`GET /v1/audit`, plus a generated `openapi.json` at `/v1/openapi.json`.

## Coming in Phase 4

`POST /v1/memories` (async understanding pipeline, `202 {job_id}`),
`GET /v1/jobs/{id}`.

See [project-plan.md §9](../project-plan.md#9-api-design-rest--mcp) for the
full design.
