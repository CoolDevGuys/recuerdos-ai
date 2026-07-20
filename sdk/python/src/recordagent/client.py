"""The RecordAgent client.

Thin on purpose: one method per endpoint, typed models in and out, and no
caching, batching or retry cleverness of its own. The service already
decides what to remember; an SDK that also had opinions about it would be
a second place for those opinions to live.

# Sync only

There is no async client, deliberately, for now. Every call here is a
single request against a service on localhost, and the work that actually
takes seconds — extraction, reconciliation — already happens off the
request path behind a job id. Adding a parallel async surface would
double the API for latency that mostly is not there. If you need it
inside an async framework, wrap a call in ``asyncio.to_thread``.
"""

from __future__ import annotations

import time
from datetime import datetime
from types import TracebackType
from typing import Any

import httpx

from .errors import JobFailedError, RecordAgentError, TimeoutError_, from_response
from .models import Distillation, Job, Memory, SearchHit

__all__ = ["Client"]

DEFAULT_BASE_URL = "http://127.0.0.1:7070"

#: Long enough for a slow local model behind ``wait=True``, short enough
#: that a wedged daemon surfaces as an error rather than a hang.
DEFAULT_TIMEOUT = 30.0

#: How often :meth:`Client.wait_for_job` asks. Ingestion takes seconds, so
#: polling faster spends requests without learning anything sooner.
_POLL_INTERVAL = 0.25


class Client:
    """A RecordAgent client, scoped to one API key.

    ```python
    from recordagent import Client

    ra = Client(base_url="http://localhost:7070", api_key="ra_live_…")
    ra.save("We moved the backend to Hetzner; fly.io got too expensive")
    for hit in ra.search("where do we deploy?"):
        print(hit.content)
    ```

    The key determines the user: there is no user parameter anywhere in
    this class, because a key *is* the user. Two users means two clients.
    """

    def __init__(
        self,
        base_url: str = DEFAULT_BASE_URL,
        api_key: str | None = None,
        *,
        timeout: float = DEFAULT_TIMEOUT,
        client: httpx.Client | None = None,
    ) -> None:
        """
        Args:
            base_url: Where the daemon is.
            api_key: A key from ``recordagent key issue``. Optional only
                because a daemon running with ``[auth].mode = "none"``
                accepts unauthenticated requests.
            timeout: Per-request timeout in seconds.
            client: Bring your own ``httpx.Client`` — for a custom
                transport, proxy or connection pool. Its lifetime becomes
                yours; this class will not close it.
        """
        self.base_url = base_url.rstrip("/")
        self._api_key = api_key
        self._owns_client = client is None
        self._http = client or httpx.Client(timeout=timeout)

    # --- lifecycle ---------------------------------------------------

    def close(self) -> None:
        """Closes the underlying connection pool, unless you supplied it."""
        if self._owns_client:
            self._http.close()

    def __enter__(self) -> Client:
        return self

    def __exit__(
        self,
        exc_type: type[BaseException] | None,
        exc: BaseException | None,
        tb: TracebackType | None,
    ) -> None:
        self.close()

    # --- writing -----------------------------------------------------

    def save(
        self,
        content: str,
        *,
        category: str | None = None,
        tags: list[str] | None = None,
        client: str | None = None,
        session_id: str | None = None,
        wait: bool = False,
    ) -> Job:
        """Submits raw content for understanding.

        This is the endpoint that makes RecordAgent more than a store:
        the content is split into atomic memories, labelled, and checked
        against what is already known, so a contradiction supersedes what
        it replaces instead of piling up beside it.

        Returns a :class:`Job`, because the work is a model call that
        takes seconds. The returned job is ``pending`` and its
        ``memory_ids`` are empty — poll with :meth:`wait_for_job`.

        Args:
            content: Raw text of any length. A sentence, a paragraph, a
                session summary.
            category: A hint, not an instruction. Extraction may find
                several memories that do not all share it.
            tags: Applied to everything extracted from this content.
            client: Recorded as the source, for the audit trail.
            session_id: Recorded alongside ``client``.
            wait: Run the pipeline inline and return a terminal job with
                ``memory_ids`` populated. Convenient in a script; a poor
                idea in anything holding a request open, which is why it
                is not the default.

        Raises:
            ValidationError: The content was empty.
            PermissionError_: The key lacks ``write``.
        """
        body: dict[str, Any] = {"content": content, "wait": wait}
        if category is not None:
            body["category"] = category
        if tags:
            body["tags"] = tags
        if client is not None:
            body["client"] = client
        if session_id is not None:
            body["session_id"] = session_id

        return Job.model_validate(self._request("POST", "/v1/memories", json=body))

    def save_direct(
        self,
        content: str,
        *,
        category: str | None = None,
        tags: list[str] | None = None,
        confidence: float | None = None,
        client: str | None = None,
        session_id: str | None = None,
        expires_at: datetime | None = None,
    ) -> Memory:
        """Stores one memory exactly as given, with no pipeline.

        Use this when you have already decided what the memory should
        say. It is synchronous, cheap, and calls no model — but it also
        cannot supersede a contradiction, because nothing read the
        content. When in doubt, prefer :meth:`save`.

        Args:
            content: 1–4000 characters.
            category: Defaults to ``fact.project`` server-side.
            tags: Lowercased and de-duplicated; at most 32.
            confidence: ``0.0``–``1.0``. Clamped rather than rejected.
            expires_at: Must be in the future. The nightly job retires
                the memory once it passes.
        """
        body: dict[str, Any] = {"content": content}
        if category is not None:
            body["category"] = category
        if tags:
            body["tags"] = tags
        if confidence is not None:
            body["confidence"] = confidence
        if client is not None:
            body["client"] = client
        if session_id is not None:
            body["session_id"] = session_id
        if expires_at is not None:
            body["expires_at"] = expires_at.isoformat()

        return Memory.model_validate(
            self._request("POST", "/v1/memories:direct", json=body)
        )

    def distill_session(
        self,
        content: str,
        *,
        session_id: str | None = None,
        client: str | None = None,
        tags: list[str] | None = None,
    ) -> Distillation:
        """Reduces a finished session to the few things that outlive it.

        Pass what actually happened — a transcript, or a summary of one —
        rather than pre-filtering to the "important" parts. Everything
        about the task itself is discarded on purpose; what survives is
        conventions established, decisions and their reasons, and root
        causes worth not rediscovering.

        ``distilled == 0`` is the ordinary outcome. Most sessions produce
        nothing that stays true after they end, and that is a success.

        Unlike everything else here, this needs a configured provider:
        without one it raises :class:`ValidationError` rather than
        storing your transcript as a single unrecallable memory.

        Args:
            content: The session. At most 200 000 characters.
        """
        body: dict[str, Any] = {"content": content}
        if session_id is not None:
            body["session_id"] = session_id
        if client is not None:
            body["client"] = client
        if tags:
            body["tags"] = tags

        return Distillation.model_validate(
            self._request("POST", "/v1/sessions/distill", json=body)
        )

    # --- reading -----------------------------------------------------

    def search(
        self,
        query: str,
        *,
        limit: int | None = None,
        categories: list[str] | None = None,
        tags: list[str] | None = None,
        since: datetime | None = None,
        include_superseded: bool = False,
    ) -> list[SearchHit]:
        """Hybrid recall: semantic and keyword, fused by reciprocal rank.

        Ask the question you actually have — "which package manager does
        the user prefer" — rather than keywords. Exact identifiers work
        too (``useQuery``, a ticket id), because the keyword leg matches
        literal tokens the vector leg blurs.

        An empty result means nothing is stored on the subject, not that
        the user has no opinion.

        Args:
            query: 1–1000 characters.
            limit: Capped at 50 server-side.
            categories: OR-ed. Empty means all.
            tags: **AND**-ed — a memory must carry every one.
            since: Excludes memories created before it.
            include_superseded: Include memories a later one replaced.
        """
        body: dict[str, Any] = {"query": query}
        if limit is not None:
            body["limit"] = limit
        if categories:
            body["categories"] = categories
        if tags:
            body["tags"] = tags
        if since is not None:
            body["since"] = since.isoformat()
        if include_superseded:
            body["include_superseded"] = True

        payload = self._request("POST", "/v1/memories/search", json=body)
        results = payload.get("results", []) if isinstance(payload, dict) else []
        return [SearchHit.model_validate(hit) for hit in results]

    def get(self, memory_id: str) -> Memory:
        """One memory by id.

        Raises:
            NotFoundError: No such memory — or it belongs to someone
                else. The two are indistinguishable by design.
        """
        return Memory.model_validate(self._request("GET", f"/v1/memories/{memory_id}"))

    def profile(self) -> str:
        """The user's standing profile, as markdown.

        Read this at the start of a session. Recall answers a question,
        and an agent that has not asked one yet still needs to know the
        conventions it is expected to follow.

        Roughly 1500 tokens. Written by a model and cached when a
        provider is configured; assembled from the memories directly when
        not — the shape is the same either way.
        """
        return self._request_text("GET", "/v1/profile")

    def export(
        self, *, format: str = "markdown", include_inactive: bool = False
    ) -> str:
        """Every memory, as markdown or JSON text.

        Args:
            format: ``markdown`` (grouped by category, greppable) or
                ``json``.
            include_inactive: Include superseded and deleted memories.
        """
        return self._request_text(
            "GET",
            "/v1/memories/export",
            params={
                "format": format,
                "include_inactive": "true" if include_inactive else "false",
            },
        )

    # --- modifying ---------------------------------------------------

    def update(
        self,
        memory_id: str,
        *,
        content: str | None = None,
        category: str | None = None,
        tags: list[str] | None = None,
        expires_at: datetime | None = None,
        clear_expiry: bool = False,
    ) -> Memory:
        """Edits a memory in place. Omitted fields are left alone.

        Editing ``content`` re-embeds and re-indexes it.

        Args:
            clear_expiry: Removes an existing expiry. Distinct from
                passing ``expires_at=None``, which means "leave it
                alone" — JSON cannot express the difference between an
                absent field and an explicit null, so this does.
        """
        body: dict[str, Any] = {}
        if content is not None:
            body["content"] = content
        if category is not None:
            body["category"] = category
        if tags is not None:
            body["tags"] = tags
        if clear_expiry:
            body["expires_at"] = None
        elif expires_at is not None:
            body["expires_at"] = expires_at.isoformat()

        return Memory.model_validate(
            self._request("PATCH", f"/v1/memories/{memory_id}", json=body)
        )

    def forget(self, memory_id: str) -> None:
        """Deletes a memory.

        A soft delete: it stops being recalled, and the audit trail keeps
        what happened to it. Only do this when the user asked — they
        cannot see what you removed, and a memory deleted by mistake is
        gone from every future session. If a memory is merely out of
        date, :meth:`save` the correction instead and let reconciliation
        supersede it.
        """
        self._request("DELETE", f"/v1/memories/{memory_id}")

    # --- jobs --------------------------------------------------------

    def job(self, job_id: str) -> Job:
        """The current state of an ingestion."""
        return Job.model_validate(self._request("GET", f"/v1/jobs/{job_id}"))

    def wait_for_job(
        self,
        job: Job | str,
        *,
        timeout: float = 60.0,
        raise_on_failure: bool = True,
    ) -> Job:
        """Polls until the job finishes.

        Args:
            job: A :class:`Job` or a job id.
            timeout: Seconds before giving up. On expiry the work is
                usually still running, not failed — hence a distinct
                :class:`TimeoutError_` rather than a job failure.
            raise_on_failure: Raise :class:`JobFailedError` when the job
                dead-letters. Set ``False`` to inspect ``job.error``
                yourself.

        Raises:
            TimeoutError_: The deadline passed with the job unfinished.
            JobFailedError: The job ran out of attempts.
        """
        job_id = job.job_id if isinstance(job, Job) else job
        deadline = time.monotonic() + timeout

        while True:
            current = self.job(job_id)
            if current.is_terminal:
                if current.status == "failed" and raise_on_failure:
                    raise JobFailedError(
                        current.error or "ingestion failed",
                        job_id=job_id,
                        attempts=current.attempts,
                    )
                return current

            if time.monotonic() >= deadline:
                raise TimeoutError_(
                    f"job {job_id} was still {current.status} after {timeout:g}s; "
                    "it is probably still running — poll again rather than "
                    "resubmitting",
                    code="timeout",
                )

            time.sleep(_POLL_INTERVAL)

    def save_and_wait(self, content: str, **kwargs: Any) -> list[Memory]:
        """:meth:`save`, then :meth:`wait_for_job`, then fetch the results.

        The convenience wrapper for scripts and notebooks, where a job id
        is a nuisance. Returns the memories the content produced —
        possibly none, which is the correct outcome for text with nothing
        durable in it.
        """
        timeout = kwargs.pop("timeout", 60.0)
        finished = self.wait_for_job(self.save(content, **kwargs), timeout=timeout)
        return [self.get(memory_id) for memory_id in finished.memory_ids]

    # --- plumbing ----------------------------------------------------

    def health(self) -> bool:
        """Whether the daemon is up. Needs no credential."""
        try:
            response = self._http.get(f"{self.base_url}/healthz")
        except httpx.HTTPError:
            return False
        return response.status_code == 200

    def _headers(self) -> dict[str, str]:
        if self._api_key is None:
            return {}
        return {"Authorization": f"Bearer {self._api_key}"}

    def _send(
        self,
        method: str,
        path: str,
        *,
        json: Any = None,
        params: dict[str, str] | None = None,
    ) -> httpx.Response:
        try:
            response = self._http.request(
                method,
                f"{self.base_url}{path}",
                json=json,
                params=params,
                headers=self._headers(),
            )
        except httpx.HTTPError as error:
            # The overwhelmingly likely cause, so say it rather than
            # making someone decode a connection error.
            raise RecordAgentError(
                f"could not reach the RecordAgent daemon at {self.base_url}: {error}. "
                "Is it running?"
            ) from error

        if response.status_code >= 400:
            try:
                body = response.json()
            except ValueError:
                body = None
            raise from_response(
                response.status_code, body, response.headers.get("x-request-id")
            )

        return response

    def _request(
        self,
        method: str,
        path: str,
        *,
        json: Any = None,
        params: dict[str, str] | None = None,
    ) -> Any:
        response = self._send(method, path, json=json, params=params)
        if response.status_code == 204 or not response.content:
            return {}
        return response.json()

    def _request_text(
        self, method: str, path: str, *, params: dict[str, str] | None = None
    ) -> str:
        return self._send(method, path, params=params).text
