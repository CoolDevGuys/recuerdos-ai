# REST API

**Status: Phase 0.** No auth yet, no memory endpoints yet — see
[implementation-plan.md](../implementation-plan.md) for what lands when.

## Available today

### `GET /healthz`

```json
{"status": "ok"}
```

### `GET /version`

```json
{"version": "0.1.0", "git_sha": "a1b2c3d4e5f6"}
```

## Coming in Phase 1

Every other route, an auth middleware (`Authenticated(UserContext)`
extractor, bearer API keys), and the standard error envelope:

```json
{"error": {"code": "unauthorized", "message": "invalid API key"}}
```

## Coming in Phase 2

`POST /v1/memories:direct`, `POST /v1/memories/search`,
`GET/PATCH/DELETE /v1/memories/{id}`, `GET /v1/memories/export`,
`GET /v1/audit`, plus a generated `openapi.json` at `/v1/openapi.json`.

## Coming in Phase 4

`POST /v1/memories` (async understanding pipeline, `202 {job_id}`),
`GET /v1/jobs/{id}`.

See [project-plan.md §9](../project-plan.md#9-api-design-rest--mcp) for
the full design.
