"""Integration tests against a running daemon.

These exist to catch the one class of bug the unit tests structurally
cannot: the SDK and the server disagreeing about the wire. A mock built
from the same reading of the API docs that produced the client will
agree with itself forever, whatever the server actually does.

Run with `just sdk-test`, which brings a daemon up first.
"""

from __future__ import annotations

import pytest

from recordagent import Client, NotFoundError

pytestmark = pytest.mark.integration


def test_the_daemon_is_reachable(ra: Client) -> None:
    assert ra.health()


def test_save_direct_then_get_round_trips_every_field(ra: Client, tag: str) -> None:
    saved = ra.save_direct(
        "User forbids barrel files and index.ts re-exports",
        category="preference.coding",
        tags=[tag, "typescript"],
        confidence=0.9,
        client="sdk-test",
    )

    assert saved.content == "User forbids barrel files and index.ts re-exports"
    assert saved.category == "preference.coding"
    assert tag in saved.tags
    assert abs(saved.confidence - 0.9) < 1e-6

    fetched = ra.get(saved.id)
    assert fetched == saved, "the model does not survive a round trip"


def test_the_core_loop_save_then_recall_by_meaning(ra: Client, tag: str) -> None:
    # The whole product in three lines: store something in one phrasing,
    # find it with another.
    ra.save_direct(
        "User prefers pnpm and never uses npm or yarn",
        category="preference.coding",
        tags=[tag],
    )

    hits = ra.search("which package manager should I use?", tags=[tag])

    assert hits, "semantic recall found nothing"
    assert "pnpm" in hits[0].content
    assert hits[0].score > 0


def test_recall_finds_an_exact_identifier(ra: Client, tag: str) -> None:
    # The keyword leg's reason for existing: a vector alone blurs
    # identifiers into their neighbourhood.
    ra.save_direct(
        "Prefer useQuery over manual fetch effects in this codebase",
        category="preference.coding",
        tags=[tag],
    )

    hits = ra.search("useQuery", tags=[tag])

    assert hits
    assert "useQuery" in hits[0].content
    assert hits[0].matched.bm25_rank is not None, "the keyword leg did not match"


def test_search_filters_narrow_rather_than_widen(ra: Client, tag: str) -> None:
    ra.save_direct("A coding preference", category="preference.coding", tags=[tag])
    ra.save_direct("A project fact", category="fact.project", tags=[tag])

    everything = ra.search("a", tags=[tag], limit=50)
    only_facts = ra.search("a", tags=[tag], categories=["fact.project"], limit=50)

    assert len(everything) >= 2
    assert all(hit.category == "fact.project" for hit in only_facts)


def test_a_memory_can_be_edited_and_forgotten(ra: Client, tag: str) -> None:
    saved = ra.save_direct("original wording", category="reference", tags=[tag])

    edited = ra.update(saved.id, content="revised wording", tags=[tag, "edited"])
    assert edited.content == "revised wording"
    assert edited.id == saved.id
    assert "edited" in edited.tags

    ra.forget(saved.id)

    with pytest.raises(NotFoundError):
        ra.get(saved.id)


def test_ingestion_returns_a_job_that_reaches_a_terminal_state(
    ra: Client, tag: str
) -> None:
    # Without a provider configured the daemon stores content verbatim,
    # but the job machinery — the 202, the id, the polling, the terminal
    # status — is identical either way. That contract is what this
    # asserts, and it is the part an SDK can get wrong.
    job = ra.save("The billing service exposes a /v2/invoices endpoint", tags=[tag])

    assert job.job_id
    assert job.status == "pending"
    assert job.memory_ids == [], "a 202 should not carry results yet"

    finished = ra.wait_for_job(job, timeout=60)
    assert finished.status == "succeeded"


def test_save_and_wait_returns_the_memories_it_produced(ra: Client, tag: str) -> None:
    memories = ra.save_and_wait("The deploy target is Hetzner", tags=[tag], timeout=60)

    assert memories, "ingestion produced nothing at all"
    assert all(m.id for m in memories)
    # Fetched through `get`, so this also proves the ids the job reported
    # are real and readable.
    assert any("Hetzner" in m.content for m in memories)


def test_a_missing_memory_is_not_found_rather_than_a_crash(ra: Client) -> None:
    with pytest.raises(NotFoundError):
        ra.get("019f7c5a-0000-7000-8000-00000000dead")


def test_a_malformed_id_is_rejected_as_a_client_mistake(ra: Client) -> None:
    from recordagent import ValidationError

    with pytest.raises(ValidationError):
        ra.get("not-a-uuid")


def test_the_profile_is_markdown_and_mentions_a_stored_memory(
    ra: Client, tag: str
) -> None:
    ra.save_direct(
        "User writes commit messages in the imperative mood",
        category="preference.coding",
        tags=[tag],
    )

    profile = ra.profile()

    assert profile.startswith("# Memory profile:")
    assert "imperative mood" in profile


def test_export_returns_both_formats(ra: Client, tag: str) -> None:
    ra.save_direct("An exportable memory", category="reference", tags=[tag])

    markdown = ra.export()
    as_json = ra.export(format="json")

    assert "An exportable memory" in markdown
    assert as_json.lstrip().startswith(("{", "["))


def test_distillation_without_a_provider_refuses_and_says_why(ra: Client) -> None:
    # The one endpoint that does not degrade to verbatim: a transcript
    # stored whole is unrecallable and costs a context window on every
    # match. The SDK has to surface the reason, not just a 400.
    from recordagent import ValidationError

    with pytest.raises(ValidationError) as caught:
        ra.distill_session("a long transcript of a session that went nowhere")

    assert "[understanding].provider" in str(caught.value)
