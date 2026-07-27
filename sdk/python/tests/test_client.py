"""Unit tests: the client against a mock transport.

No daemon, no network. These cover the parts that are the SDK's own
behaviour rather than the server's — error mapping, request shaping,
polling — which is exactly where a client library goes wrong quietly.
The integration suite covers whether the two actually agree.
"""

from __future__ import annotations

import json
from datetime import UTC, datetime
from typing import Any

import httpx
import pytest

from recordagent import (
    AuthenticationError,
    Client,
    JobFailedError,
    NotFoundError,
    PermissionError_,
    RecordAgentError,
    ServerError,
    TimeoutError_,
    ValidationError,
)

MEMORY = {
    "id": "019f7c5a-0000-7000-8000-000000000001",
    "content": "User prefers pnpm",
    "category": "preference.coding",
    "tags": ["tooling"],
    "confidence": 1.0,
    "created_at": "2026-07-19T08:18:19Z",
    "updated_at": "2026-07-19T08:18:19Z",
}


def client_for(handler: Any, **kwargs: Any) -> Client:
    transport = httpx.MockTransport(handler)
    return Client(
        base_url="http://daemon:7070",
        api_key="ra_live_test",
        client=httpx.Client(transport=transport),
        **kwargs,
    )


def responding(status: int, body: Any, headers: dict[str, str] | None = None) -> Any:
    def handler(request: httpx.Request) -> httpx.Response:
        return httpx.Response(status, json=body, headers=headers or {})

    return handler


def error_body(code: str, message: str) -> dict[str, Any]:
    return {"error": {"code": code, "message": message}}


# --- error mapping ---------------------------------------------------


@pytest.mark.parametrize(
    ("status", "code", "expected"),
    [
        (400, "validation_failed", ValidationError),
        (401, "unauthorized", AuthenticationError),
        (403, "forbidden", PermissionError_),
        (404, "not_found", NotFoundError),
        (500, "internal", ServerError),
    ],
)
def test_each_error_code_maps_to_its_own_exception(
    status: int, code: str, expected: type[Exception]
) -> None:
    # Callers branch on type. If two codes collapsed to one exception,
    # "retry this" and "fix your request" would be indistinguishable.
    ra = client_for(responding(status, error_body(code, "nope")))

    with pytest.raises(expected):
        ra.get("some-id")


def test_an_error_without_an_envelope_is_still_a_recordagent_error() -> None:
    # A proxy returning an HTML 502 has no envelope. That has to surface
    # as a library error, not a KeyError from inside the SDK.
    def handler(request: httpx.Request) -> httpx.Response:
        return httpx.Response(502, text="<html>Bad Gateway</html>")

    ra = client_for(handler)

    with pytest.raises(ServerError) as caught:
        ra.get("some-id")
    assert "502" in str(caught.value)


def test_the_request_id_is_carried_onto_the_exception() -> None:
    # For a 500 the message is always "internal error" and the real cause
    # is only in the server log, findable by this id.
    ra = client_for(
        responding(
            500,
            error_body("internal", "internal error"),
            {"x-request-id": "req-abc123"},
        )
    )

    with pytest.raises(ServerError) as caught:
        ra.get("some-id")

    assert caught.value.request_id == "req-abc123"
    assert "req-abc123" in str(caught.value)


def test_an_unreachable_daemon_says_so_in_plain_words() -> None:
    def handler(request: httpx.Request) -> httpx.Response:
        raise httpx.ConnectError("connection refused")

    ra = client_for(handler)

    with pytest.raises(RecordAgentError) as caught:
        ra.search("anything")

    assert "Is it running?" in str(caught.value)
    assert "http://daemon:7070" in str(caught.value)


# --- request shaping -------------------------------------------------


def test_the_api_key_is_sent_as_a_bearer_token() -> None:
    seen: dict[str, str] = {}

    def handler(request: httpx.Request) -> httpx.Response:
        seen.update(request.headers)
        return httpx.Response(200, json={"results": []})

    client_for(handler).search("anything")

    assert seen["authorization"] == "Bearer ra_live_test"


def test_no_key_means_no_authorization_header() -> None:
    # `[auth].mode = "none"` is a supported deployment. Sending
    # `Bearer None` would be worse than sending nothing.
    seen: list[httpx.Request] = []

    def handler(request: httpx.Request) -> httpx.Response:
        seen.append(request)
        return httpx.Response(200, json={"results": []})

    Client(
        base_url="http://daemon:7070",
        client=httpx.Client(transport=httpx.MockTransport(handler)),
    ).search("anything")

    assert "authorization" not in seen[0].headers


def test_optional_fields_are_omitted_rather_than_sent_as_null() -> None:
    # The server treats an absent field and an explicit null differently
    # on PATCH, so the client must not blur them anywhere.
    bodies: list[Any] = []

    def handler(request: httpx.Request) -> httpx.Response:
        bodies.append(json.loads(request.content))
        return httpx.Response(201, json=MEMORY)

    client_for(handler).save_direct("User prefers pnpm")

    assert bodies[0] == {"content": "User prefers pnpm"}


def test_clearing_an_expiry_is_distinct_from_leaving_it_alone() -> None:
    # JSON cannot express the difference between "absent" and "null", so
    # the SDK gives it two different call shapes.
    bodies: list[Any] = []

    def handler(request: httpx.Request) -> httpx.Response:
        bodies.append(json.loads(request.content))
        return httpx.Response(200, json=MEMORY)

    ra = client_for(handler)
    ra.update("an-id", content="revised")
    ra.update("an-id", clear_expiry=True)

    assert "expires_at" not in bodies[0], "leaving it alone sent a null"
    assert bodies[1]["expires_at"] is None, "clearing it did not send a null"


def test_saving_defaults_to_not_waiting() -> None:
    # `wait=True` holds a request open for a model call. It has to be a
    # deliberate choice, never the default.
    bodies: list[Any] = []

    def handler(request: httpx.Request) -> httpx.Response:
        bodies.append(json.loads(request.content))
        return httpx.Response(202, json={"job_id": "j-1", "status": "pending"})

    client_for(handler).save("we moved to Hetzner")

    assert bodies[0]["wait"] is False


def test_search_filters_reach_the_wire() -> None:
    bodies: list[Any] = []

    def handler(request: httpx.Request) -> httpx.Response:
        bodies.append(json.loads(request.content))
        return httpx.Response(200, json={"results": []})

    client_for(handler).search(
        "imports",
        limit=3,
        categories=["preference.coding"],
        tags=["typescript"],
        since=datetime(2026, 1, 1, tzinfo=UTC),
        include_superseded=True,
    )

    assert bodies[0]["limit"] == 3
    assert bodies[0]["categories"] == ["preference.coding"]
    assert bodies[0]["tags"] == ["typescript"]
    assert bodies[0]["include_superseded"] is True
    assert bodies[0]["since"].startswith("2026-01-01")


# --- models ----------------------------------------------------------


def test_a_search_hit_carries_why_it_matched() -> None:
    hit_body = {
        **MEMORY,
        "score": 0.0325,
        "matched": {"vector_rank": 1, "bm25_rank": 2},
    }
    ra = client_for(responding(200, {"results": [hit_body], "took_ms": 9}))

    hits = ra.search("imports")

    assert hits[0].content == "User prefers pnpm"
    assert hits[0].matched.vector_rank == 1
    assert hits[0].matched.bm25_rank == 2


def test_a_leg_that_did_not_match_reads_as_none_not_zero() -> None:
    # Rank 0 would be a real rank. Absent means the leg did not return it.
    hit_body = {**MEMORY, "score": 0.01, "matched": {"vector_rank": 1}}
    ra = client_for(responding(200, {"results": [hit_body]}))

    assert ra.search("imports")[0].matched.bm25_rank is None


def test_unknown_response_fields_are_tolerated() -> None:
    # A client pinned to an older SDK must keep working against a newer
    # server, or adding a response field breaks everyone who has not
    # upgraded.
    ra = client_for(responding(200, {**MEMORY, "a_field_from_the_future": 42}))

    assert ra.get("an-id").content == "User prefers pnpm"


def test_a_deletion_returning_no_body_does_not_break_parsing() -> None:
    def handler(request: httpx.Request) -> httpx.Response:
        return httpx.Response(204)

    client_for(handler).forget("an-id")  # must not raise


# --- jobs ------------------------------------------------------------


def test_waiting_polls_until_the_job_finishes() -> None:
    states = iter(
        [
            {"job_id": "j-1", "status": "pending", "attempts": 0},
            {"job_id": "j-1", "status": "running", "attempts": 1},
            {
                "job_id": "j-1",
                "status": "succeeded",
                "attempts": 1,
                "memory_ids": ["m-1"],
            },
        ]
    )

    def handler(request: httpx.Request) -> httpx.Response:
        return httpx.Response(200, json=next(states))

    job = client_for(handler).wait_for_job("j-1", timeout=5)

    assert job.status == "succeeded"
    assert job.memory_ids == ["m-1"]


def test_a_dead_lettered_job_raises_with_its_reason() -> None:
    ra = client_for(
        responding(
            200,
            {
                "job_id": "j-1",
                "status": "failed",
                "attempts": 3,
                "error": "provider returned 401",
            },
        )
    )

    with pytest.raises(JobFailedError) as caught:
        ra.wait_for_job("j-1", timeout=5)

    assert caught.value.attempts == 3
    assert "provider returned 401" in str(caught.value)


def test_a_failed_job_can_be_inspected_instead_of_raised() -> None:
    ra = client_for(
        responding(
            200, {"job_id": "j-1", "status": "failed", "attempts": 3, "error": "boom"}
        )
    )

    job = ra.wait_for_job("j-1", timeout=5, raise_on_failure=False)

    assert job.status == "failed"
    assert job.error == "boom"


def test_a_timeout_says_the_work_is_probably_still_running() -> None:
    # The distinction that matters: a timeout is the client giving up,
    # not the job failing. Resubmitting would duplicate the work.
    ra = client_for(responding(200, {"job_id": "j-1", "status": "pending"}))

    with pytest.raises(TimeoutError_) as caught:
        ra.wait_for_job("j-1", timeout=0.01)

    assert "still running" in str(caught.value)


def test_an_empty_result_is_a_success_not_a_failure() -> None:
    # "Nothing here was worth remembering" is the most common outcome of
    # ingesting real text.
    ra = client_for(
        responding(200, {"job_id": "j-1", "status": "succeeded", "memory_ids": []})
    )

    job = ra.wait_for_job("j-1", timeout=5)

    assert job.status == "succeeded"
    assert job.memory_ids == []


# --- lifecycle -------------------------------------------------------


def test_a_supplied_http_client_is_not_closed_by_us() -> None:
    # Its lifetime belongs to whoever created it; closing a shared pool
    # out from under an application would be a nasty surprise.
    http = httpx.Client(transport=httpx.MockTransport(responding(200, {})))

    with Client(base_url="http://daemon:7070", client=http):
        pass

    assert not http.is_closed


def test_a_client_we_created_is_closed_on_exit() -> None:
    ra = Client(base_url="http://daemon:7070")
    with ra:
        pass

    assert ra._http.is_closed


def test_a_trailing_slash_in_the_base_url_does_not_double_up() -> None:
    seen: list[str] = []

    def handler(request: httpx.Request) -> httpx.Response:
        seen.append(str(request.url))
        return httpx.Response(200, json={"results": []})

    Client(
        base_url="http://daemon:7070/",
        client=httpx.Client(transport=httpx.MockTransport(handler)),
    ).search("anything")

    assert seen[0] == "http://daemon:7070/v1/memories/search"
