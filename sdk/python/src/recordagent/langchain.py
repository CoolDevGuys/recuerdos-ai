"""LangChain integration: a retriever over a user's memories.

Optional. ``langchain-core`` is an extra, and importing this module
without it raises a message that says what to install rather than an
opaque ``ModuleNotFoundError`` from three frames down.

    pip install "recordagent[langchain]"

# Retriever, or tool?

Both, and they answer different questions.

A **retriever** fetches memories for every turn, unconditionally. Right
when the memories are context the model should always have — the user's
conventions in a coding agent.

A **tool** lets the model decide when to look. Right when memory is one
source among several, and most turns do not need it. For that, wrap
:meth:`Client.search` in whatever tool decorator your framework uses;
there is nothing RecordAgent-specific about it, so this module does not
ship a second wrapper you would have to learn.
"""

from __future__ import annotations

from typing import TYPE_CHECKING, Any

from .client import Client

if TYPE_CHECKING:  # pragma: no cover
    from langchain_core.callbacks import CallbackManagerForRetrieverRun
    from langchain_core.documents import Document

try:
    from langchain_core.documents import Document
    from langchain_core.retrievers import BaseRetriever
except ModuleNotFoundError as error:  # pragma: no cover
    raise ModuleNotFoundError(
        "recordagent.langchain needs langchain-core. "
        'Install it with: pip install "recordagent[langchain]"'
    ) from error

__all__ = ["RecordAgentRetriever"]


class RecordAgentRetriever(BaseRetriever):
    """Retrieves a user's memories as LangChain ``Document``s.

    ```python
    from recordagent import Client
    from recordagent.langchain import RecordAgentRetriever

    retriever = RecordAgentRetriever(
        client=Client(api_key="ra_live_…"),
        limit=5,
        categories=["preference.coding", "decision"],
    )
    docs = retriever.invoke("how should imports be structured?")
    ```

    Each document's ``page_content`` is the memory text and its
    ``metadata`` carries the id, category, tags, score and which search
    leg matched — so a chain can cite a memory, filter on category, or
    explain a surprising retrieval.
    """

    client: Client
    """The RecordAgent client. Its API key determines whose memories these are."""

    limit: int = 5
    """How many memories to retrieve. Kept small by default: these go into
    every prompt, and ten mediocre memories crowd out three good ones."""

    categories: list[str] = []
    """Restrict to these categories. Empty means all."""

    tags: list[str] = []
    """Require every one of these tags."""

    # `Client` is a plain class, not a pydantic model, and LangChain's
    # base is a pydantic model — so it has to be told to allow it.
    model_config = {"arbitrary_types_allowed": True}

    def _get_relevant_documents(
        self,
        query: str,
        *,
        run_manager: CallbackManagerForRetrieverRun | None = None,
        **kwargs: Any,
    ) -> list[Document]:
        hits = self.client.search(
            query,
            limit=self.limit,
            categories=self.categories or None,
            tags=self.tags or None,
        )

        return [
            Document(
                page_content=hit.content,
                metadata={
                    "id": hit.id,
                    "category": hit.category,
                    "tags": hit.tags,
                    "confidence": hit.confidence,
                    "score": hit.score,
                    "created_at": hit.created_at.isoformat(),
                    "vector_rank": hit.matched.vector_rank,
                    "bm25_rank": hit.matched.bm25_rank,
                    "source": "recordagent",
                },
            )
            for hit in hits
        ]
