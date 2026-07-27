"""Memories as LangChain ``Document``s.

    pip install "recuerdos-ai[langchain]"
    python examples/langchain_retriever.py

Needs a daemon but no language model — retrieval is the local embedding
model plus BM25, so this runs offline and costs nothing. That also makes
it the example CI executes on every push.

For the agent-side pattern — memory read before the turn and written
after it — see ``langgraph_memory.py``.
"""

from __future__ import annotations

import os
import sys
import uuid

from recuerdos_ai import Client, RecuerdosError
from recuerdos_ai.langchain import RecuerdosRetriever

SEED = [
    ("User prefers pnpm and never uses npm or yarn", "preference.coding"),
    ("User forbids barrel files and index.ts re-exports", "preference.coding"),
    ("SQLite was chosen over Postgres because of installer size", "decision"),
    ("The backend runs on Hetzner, migrated from Fly.io on cost", "fact.project"),
    ("User is vegetarian", "preference.personal"),
]


def rank(value: int | None) -> str:
    """Renders one search leg's rank.

    ``None`` means that leg did not return the memory at all — the
    interesting case, since it is why hybrid search finds things neither
    leg would find alone.
    """
    return f"#{value}" if value is not None else "—"


def main() -> int:
    ra = Client(
        base_url=os.environ.get("RECUERDOS_AI_URL", "http://localhost:7070"),
        api_key=os.environ.get("RECUERDOS_AI_API_KEY"),
    )
    if not ra.health():
        print(f"no daemon at {ra.base_url} — is it running?", file=sys.stderr)
        return 1

    # Tagged per run so re-running this does not search a store the last
    # run polluted.
    run = f"lc-{uuid.uuid4().hex[:8]}"
    for content, category in SEED:
        ra.save_direct(content, category=category, tags=[run])
    print(f"seeded {len(SEED)} memories (tag {run})")

    retriever = RecuerdosRetriever(client=ra, limit=2, tags=[run])

    # None of these queries share vocabulary with the memory that should
    # answer them. That gap is the whole reason this is a memory service
    # and not a grep.
    for question in [
        "which package manager should I use here?",
        "why did we pick this database?",
        "where does this thing deploy?",
    ]:
        print(f"\n> {question}")
        for document in retriever.invoke(question):
            meta = document.metadata
            print(f"  [{meta['category']}] {document.page_content}")
            print(
                f"      score {meta['score']:.4f}  "
                f"vector {rank(meta['vector_rank'])}  "
                f"keyword {rank(meta['bm25_rank'])}"
            )

    # Restricting to a category is how you keep a coding agent's prompt
    # free of a user's dietary requirements.
    coding_only = RecuerdosRetriever(
        client=ra, limit=3, tags=[run], categories=["preference.coding"]
    )
    print("\n> conventions (preference.coding only)")
    for document in coding_only.invoke("what conventions does this user hold?"):
        print(f"  {document.page_content}")

    ra.close()
    return 0


if __name__ == "__main__":
    try:
        sys.exit(main())
    except RecuerdosError as error:
        print(f"error: {error}", file=sys.stderr)
        sys.exit(1)
