"""Wire types.

Pydantic models rather than dicts, for the reason typed clients usually
exist: ``hit.content`` fails at the point of the typo, ``hit["contnet"]``
fails wherever the value is finally used.

Every model allows unknown fields. A client pinned to an older SDK must
keep working against a newer server — the alternative is that adding a
response field is a breaking change for everyone who has not upgraded.
"""

from __future__ import annotations

from datetime import datetime
from typing import Annotated, Literal

from pydantic import BaseModel, ConfigDict, Field

__all__ = [
    "Memory",
    "MatchDetail",
    "SearchHit",
    "SearchReport",
    "RerankStatus",
    "RerankApplied",
    "RerankShadow",
    "RerankNotNeeded",
    "RerankSkipped",
    "Job",
    "Distillation",
    "JobStatus",
]

JobStatus = Literal["pending", "running", "succeeded", "failed"]


class _Model(BaseModel):
    model_config = ConfigDict(extra="allow", frozen=True)


class Memory(_Model):
    """One atomic thing worth remembering."""

    id: str
    content: str
    category: str
    tags: list[str] = Field(default_factory=list)
    confidence: float = 1.0
    created_at: datetime
    updated_at: datetime
    expires_at: datetime | None = None
    #: Set when a later memory replaced this one. Superseded memories are
    #: retained and excluded from ordinary recall.
    superseded_by: str | None = None

    def __str__(self) -> str:
        return f"[{self.category}] {self.content}"


class MatchDetail(_Model):
    """Which leg of hybrid search found a result, and at what rank.

    Present so a surprising ranking can be explained rather than merely
    distrusted. ``None`` means that leg did not return the memory at all.
    """

    vector_rank: int | None = None
    bm25_rank: int | None = None
    #: Rank in the graph leg, set only when an entity-graph hop reached this
    #: memory — the case where it answers a question it shares no words with.
    graph_rank: int | None = None
    #: 1-based rank in the optional second-stage reranker
    #: (``[rerank].mode = "on"``). ``None`` whenever the reranker is off,
    #: in shadow mode, or never reached this result.
    rerank_rank: int | None = None
    #: The reranker's own score for the memory, present exactly when
    #: ``rerank_rank`` is. Every model's output is squashed onto ``(0, 1)`` by
    #: a strictly increasing function, so **ranks are comparable across
    #: models while magnitudes are not**, and neither is a calibrated
    #: probability that the memory answers the query. A diagnostic, never a
    #: threshold: ``relevance`` keeps its meaning regardless.
    rerank_score: float | None = None


class SearchHit(Memory):
    """A memory, plus why it came back."""

    score: float = 0.0
    matched: MatchDetail = Field(default_factory=MatchDetail)


class RerankApplied(_Model):
    """The second stage judged the head, and its order is the one you got.

    ``judged`` is how many candidates it scored — the head, not the list you
    received, which is usually shorter.
    """

    state: Literal["applied"]
    judged: int
    took_ms: int


class RerankShadow(_Model):
    """It judged the head and logged what it *would* have served.

    ``mode = "shadow"`` exists to be measured, so ``changed`` is the useful
    bit: whether the top result would have moved. It did not — you are reading
    the pre-rerank order.
    """

    state: Literal["shadow"]
    judged: int
    changed: bool
    took_ms: int


class RerankNotNeeded(_Model):
    """Nothing to reorder: fewer than two candidates cleared the floor.

    A fact about the query and the store, not a malfunction. It is reported
    rather than left silent because a head of one is also what a too-high
    ``min_relevance`` looks like from here.
    """

    state: Literal["not_needed"]
    candidates: int


class RerankSkipped(_Model):
    """A configured stage that did not contribute to this search.

    The results above are the pre-rerank order — which is the intended
    behaviour, since the second stage may cost milliseconds and must never
    cost a request. It is visible because the alternative was ``mode = "on"``
    being indistinguishable from ``mode = "off"``: on a machine slower than
    the one that sized ``[rerank].timeout_ms``, every query can lose the stage.

    ``reason`` says what to do about it: ``deadline`` is raised (or
    ``[rerank].depth`` lowered), ``model`` is neither.
    """

    state: Literal["skipped"]
    reason: Literal["deadline", "model"]
    candidates: int
    took_ms: int


RerankStatus = Annotated[
    RerankApplied | RerankShadow | RerankNotNeeded | RerankSkipped,
    Field(discriminator="state"),
]


class SearchReport(_Model):
    """One search, with the facts the response carries above the results.

    ``search()`` returns the hits and drops these. That is the right default
    for a caller who only wants memories, and wrong for two others: anyone
    deciding whether an empty list means "nothing stored" or "nothing cleared
    the floor" needs ``min_relevance``, and anyone who enabled the reranker
    needs ``rerank`` to know whether it ran.
    """

    hits: list[SearchHit] = Field(default_factory=list)
    took_ms: int = 0
    #: The relevance floor that produced ``hits``. An empty list with a floor
    #: above ``0.0`` is "nothing cleared it", which is a different fact from
    #: "nothing matched" and is what a retry at ``min_relevance=0.0`` turns on.
    min_relevance: float = 0.0
    #: Absent (`None`) when the server has no second stage configured, which
    #: includes every server older than 0.4.2.
    rerank: RerankStatus | None = None


class Job(_Model):
    """An ingestion, which happens off the request path."""

    job_id: str
    status: JobStatus
    attempts: int = 0
    #: The reason for the previous attempt. Can be set while a job is
    #: still ``pending`` — "it worked eventually, but here is what went
    #: wrong" is the more useful record.
    error: str | None = None
    #: What the job produced. Empty on success is legitimate and common:
    #: most submitted text contains nothing worth remembering.
    memory_ids: list[str] = Field(default_factory=list)
    #: ``False`` when no provider is configured, so you can tell
    #: "extracted and reconciled" from "stored as sent".
    understanding: bool | None = None

    @property
    def is_terminal(self) -> bool:
        return self.status in ("succeeded", "failed")


class Distillation(_Model):
    """What a session left behind."""

    memory_ids: list[str] = Field(default_factory=list)
    #: How many memories survived the session. Zero is the ordinary
    #: outcome and is a success, not a failure.
    distilled: int = 0
