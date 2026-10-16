"""Unit tests: the client against a mock transport.

No daemon, no network. These cover the parts that are the SDK's own
behaviour rather than the server's — error mapping, request shaping,
polling — which is exactly where a client library goes wrong quietly.
The integration suite covers whether the two actually agree.
"""

from __future__ import annotations

import importlib.metadata
import json
from datetime import UTC, datetime
from typing import Any

import httpx
import pytest

import recuerdos_ai
from recuerdos_ai import (
    AuthenticationError,
    Client,
    JobFailedError,
    NotFoundError,
    PermissionError_,
    RecuerdosError,
    RerankSkipped,
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


def test_an_error_without_an_envelope_is_still_a_recuerdos_error() -> None:
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

    with pytest.raises(RecuerdosError) as caught:
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


def test_as_of_reaches_the_wire_for_time_travel_recall() -> None:
    bodies: list[Any] = []

    def handler(request: httpx.Request) -> httpx.Response:
        bodies.append(json.loads(request.content))
        return httpx.Response(200, json={"results": []})

    client_for(handler).search(
        "who was on call", as_of=datetime(2026, 1, 1, tzinfo=UTC)
    )

    assert bodies[0]["as_of"].startswith("2026-01-01")


def test_min_relevance_reaches_the_wire_and_is_omitted_when_unset() -> None:
    # 0.0 is a real value with a meaning ("show me the near-misses"), so it
    # must travel; the default must not, or the client would be silently
    # overriding the daemon's own floor.
    bodies: list[Any] = []

    def handler(request: httpx.Request) -> httpx.Response:
        bodies.append(json.loads(request.content))
        return httpx.Response(200, json={"results": []})

    ra = client_for(handler)
    ra.search("anything", min_relevance=0.0)
    ra.search("anything")

    assert bodies[0]["min_relevance"] == 0.0
    assert "min_relevance" not in bodies[1]


# --- models ----------------------------------------------------------


def test_a_search_hit_carries_why_it_matched() -> None:
    hit_body = {
        **MEMORY,
        "score": 0.0325,
        "matched": {"vector_rank": 1, "bm25_rank": 2, "graph_rank": 3},
    }
    ra = client_for(responding(200, {"results": [hit_body], "took_ms": 9}))

    hits = ra.search("imports")

    assert hits[0].content == "User prefers pnpm"
    assert hits[0].matched.vector_rank == 1
    assert hits[0].matched.bm25_rank == 2
    assert hits[0].matched.graph_rank == 3


def test_a_leg_that_did_not_match_reads_as_none_not_zero() -> None:
    # Rank 0 would be a real rank. Absent means the leg did not return it.
    hit_body = {**MEMORY, "score": 0.01, "matched": {"vector_rank": 1}}
    ra = client_for(responding(200, {"results": [hit_body]}))

    assert ra.search("imports")[0].matched.bm25_rank is None


def test_a_reranked_hit_carries_the_second_stage_fields() -> None:
    # ``[rerank].mode = "on"`` is the only server setting that emits
    # these fields; the score is on the model's scale, not ``relevance``.
    hit_body = {
        **MEMORY,
        "score": 0.03,
        "matched": {"vector_rank": 3, "rerank_rank": 1, "rerank_score": 0.97},
    }
    ra = client_for(responding(200, {"results": [hit_body]}))

    hit = ra.search("imports")[0]
    assert hit.matched.rerank_rank == 1
    assert hit.matched.rerank_score == pytest.approx(0.97)


def test_an_unreranked_result_reads_the_second_stage_as_none() -> None:
    # Default server config has no reranker: an older client (and this
    # SDK) sees exactly the fields it always had, and no null surprises.
    hit_body = {**MEMORY, "score": 0.01, "matched": {"vector_rank": 1}}
    ra = client_for(responding(200, {"results": [hit_body]}))

    hit = ra.search("imports")[0]
    assert hit.matched.rerank_rank is None
    assert hit.matched.rerank_score is None


# --- the second stage, and what else the response says --------------


def test_a_search_report_carries_what_is_not_about_any_hit() -> None:
    ra = client_for(
        responding(
            200,
            {
                "results": [{**MEMORY, "score": 0.9}],
                "took_ms": 27,
                "min_relevance": 0.25,
                "rerank": {"state": "applied", "judged": 10, "took_ms": 71},
            },
        )
    )

    report = ra.search_report("imports")

    assert report.took_ms == 27
    assert report.min_relevance == 0.25
    assert report.hits[0].content == "User prefers pnpm"
    assert report.rerank is not None
    assert report.rerank.state == "applied"
    # `state` is a discriminator, so the fields that belong to one state are
    # reachable without the caller narrowing anything.
    assert report.rerank.judged == 10


def test_a_second_stage_that_was_dropped_is_named_rather_than_guessed() -> None:
    # The reason the field exists: `mode = "on"` on a machine that cannot meet
    # its deadline used to be indistinguishable from `mode = "off"`.
    ra = client_for(
        responding(
            200,
            {
                "results": [],
                "took_ms": 258,
                "min_relevance": 0.25,
                "rerank": {
                    "state": "skipped",
                    "reason": "deadline",
                    "candidates": 10,
                    "took_ms": 251,
                },
            },
        )
    )

    status = ra.search_report("imports").rerank

    assert isinstance(status, RerankSkipped)
    assert status.reason == "deadline"
    assert status.candidates == 10


def test_no_second_stage_reads_as_absent_rather_than_as_failure() -> None:
    # `mode = "off"`, and every server older than 0.4.2: the key is not there.
    ra = client_for(
        responding(200, {"results": [], "took_ms": 4, "min_relevance": 0.25})
    )

    assert ra.search_report("imports").rerank is None


def test_every_rerank_state_the_server_can_report_parses() -> None:
    for payload in [
        {"state": "applied", "judged": 3, "took_ms": 12},
        {"state": "shadow", "judged": 6, "changed": True, "took_ms": 180},
        {"state": "not_needed", "candidates": 1},
        {"state": "skipped", "reason": "model", "candidates": 10, "took_ms": 4},
    ]:
        ra = client_for(responding(200, {"results": [], "rerank": payload}))

        status = ra.search_report("imports").rerank

        assert status is not None, payload
        assert status.state == payload["state"]


def test_search_still_returns_only_the_hits() -> None:
    # `search()` is `search_report().hits` now: a delegation, not a change of
    # shape for the callers who only ever wanted memories.
    ra = client_for(
        responding(200, {"results": [{**MEMORY, "score": 0.9}], "took_ms": 1})
    )

    assert [hit.content for hit in ra.search("imports")] == ["User prefers pnpm"]


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


# --- the package itself -----------------------------------------------


def test_the_version_a_caller_can_read_is_the_version_installed() -> None:
    # `recuerdos_ai.__version__` sat at 0.2.0 through two releases whose
    # packaging metadata said 0.3.0. Whatever a caller quotes when reporting a
    # bug has to be what they are actually running, so it is pinned to the
    # installed distribution instead of to a second copy of the truth that
    # nobody updates.
    assert recuerdos_ai.__version__ == importlib.metadata.version("recuerdos-ai")
