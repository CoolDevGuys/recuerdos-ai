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
from typing import Literal

from pydantic import BaseModel, ConfigDict, Field

__all__ = ["Memory", "MatchDetail", "SearchHit", "Job", "Distillation", "JobStatus"]

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


class SearchHit(Memory):
    """A memory, plus why it came back."""

    score: float = 0.0
    matched: MatchDetail = Field(default_factory=MatchDetail)


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
