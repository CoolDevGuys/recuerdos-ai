"""The whole loop in one file: store, recall, profile.

    python examples/quickstart.py

Needs a daemon. The fastest way to get one:

    docker run -p 7070:7070 -e RECUERDOS_AI_AUTH__MODE=none \\
      ghcr.io/cooldevguys/recuerdos-ai

With authentication on (the default), issue a key and export it:

    recuerdos-ai user add alex
    recuerdos-ai key issue --user alex --scopes read,write
    export RECUERDOS_AI_API_KEY=ra_live_...
"""

from __future__ import annotations

import os
import sys

from recuerdos_ai import Client, RecuerdosError


def main() -> int:
    ra = Client(
        base_url=os.environ.get("RECUERDOS_AI_URL", "http://localhost:7070"),
        api_key=os.environ.get("RECUERDOS_AI_API_KEY"),
    )

    if not ra.health():
        print(f"no daemon at {ra.base_url} — is it running?", file=sys.stderr)
        return 1

    # --- 1. Store something you already know is worth remembering ----
    #
    # `save_direct` writes it verbatim. No model, no waiting — but also
    # no chance to notice that it contradicts something already stored.
    ra.save_direct(
        "User forbids barrel files / index.ts re-exports",
        category="preference.coding",
        tags=["typescript", "imports"],
    )
    print("stored a preference directly")

    # --- 2. Hand over raw text and let the service decide ------------
    #
    # One sentence, two unrelated facts. Extraction splits them into
    # separately-recallable memories; `:direct` would have stored the
    # whole thing as one blob that matches neither question well.
    memories = ra.save_and_wait(
        "btw we moved the backend to Hetzner, fly.io got too expensive. "
        "Also always write table-driven tests in Go",
        client="quickstart",
    )
    print(f"ingestion produced {len(memories)} memor(ies):")
    for memory in memories:
        print(f"  - [{memory.category}] {memory.content}")

    # An empty result here is not a bug. With no provider configured the
    # daemon stores content as sent; with one, "nothing durable in this"
    # is a legitimate and common answer.

    # --- 3. Ask the question you actually have -----------------------
    print("\nrecall for 'how should I structure my imports?':")
    for hit in ra.search("how should I structure my imports?", limit=3):
        legs = []
        if hit.matched.vector_rank:
            legs.append(f"vector #{hit.matched.vector_rank}")
        if hit.matched.bm25_rank:
            legs.append(f"keyword #{hit.matched.bm25_rank}")
        print(f"  {hit.score:.4f}  {hit.content}")
        print(f"          found by: {', '.join(legs) or 'nothing'}")

    # --- 4. What an agent reads before its first turn ----------------
    print("\n--- memory://profile ---")
    print(ra.profile())

    ra.close()
    return 0


if __name__ == "__main__":
    try:
        sys.exit(main())
    except RecuerdosError as error:
        print(f"error: {error}", file=sys.stderr)
        sys.exit(1)
