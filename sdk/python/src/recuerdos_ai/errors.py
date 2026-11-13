"""Typed exceptions, mapped from the API's error envelope.

Every Recuerdos AI error has the shape ``{"error": {"code": …, "message":
…}}``, and the ``code`` is the stable part — the message is for the human
reading the log. So the mapping keys off the code rather than the status,
and callers branch on an exception type rather than on string matching.
"""

from __future__ import annotations

from typing import Any

__all__ = [
    "RecuerdosError",
    "ValidationError",
    "RejectedError",
    "AuthenticationError",
    "PermissionError_",
    "NotFoundError",
    "ConflictError",
    "ServerError",
    "TimeoutError_",
    "JobFailedError",
]


class RecuerdosError(Exception):
    """Base class. Catch this to catch everything the SDK raises."""

    def __init__(
        self,
        message: str,
        *,
        code: str | None = None,
        status: int | None = None,
        request_id: str | None = None,
        details: dict[str, Any] | None = None,
    ) -> None:
        super().__init__(message)
        self.message = message
        self.code = code
        self.status = status
        #: Machine-readable context the server sent with the error, when it
        #: sent any: for a ``conflict``, the stored memories it contradicts.
        self.details = details
        #: The ``x-request-id`` header. Quote it when reporting a problem:
        #: it ties this exception to the server log line that has the real
        #: cause, which for a 500 is the only place the cause exists.
        self.request_id = request_id

    def __str__(self) -> str:
        if self.request_id:
            return f"{self.message} (request id {self.request_id})"
        return self.message


class ValidationError(RecuerdosError):
    """400 — the request was malformed. Retrying it unchanged will not help."""


class RejectedError(RecuerdosError):
    """422 — a memory rule refused the save (``docs/memory-model.md``).

    ``code`` says which: ``missing_field``, ``invalid_value``,
    ``kind_target_mismatch``, ``unknown_project``, ``no_project``,
    ``needs_confirmation``, ``scope_contradiction``, ``secret``,
    ``unknown_topic`` or ``conflict``. The message says how to fix it — often,
    by asking the user first.

    A ``conflict`` names the stored memories the save contradicts in
    :attr:`conflicts`. Ask the user which holds, then save again with
    ``replaces=`` (they changed their mind), fix the stored one with
    :meth:`Client.update`, or save with ``coexists_with=`` (both hold).
    """

    @property
    def conflicts(self) -> list[dict[str, Any]]:
        """For a ``conflict``: each stored memory's ``id``, ``content``,
        ``kind``, ``applies_to`` and the judge's ``reason``. Empty otherwise."""
        listed = (self.details or {}).get("conflicts")
        return list(listed) if isinstance(listed, list) else []

    @property
    def judged(self) -> bool:
        """For a ``conflict``: whether a language model judged it, or the
        server only found a near-repeat without one."""
        return bool((self.details or {}).get("judged", False))


class AuthenticationError(RecuerdosError):
    """401 — no usable credential.

    The API deliberately returns the same error for a missing, malformed,
    unknown, wrong-secret or revoked key, so this exception cannot tell
    you which it was either.
    """


# Trailing underscore: `PermissionError` is a builtin, and shadowing it in
# a library that users `from recuerdos_ai import *` would be hostile.
class PermissionError_(RecuerdosError):
    """403 — the key was valid but lacks the scope this route needs.

    Note that ``write`` does not imply ``read``: a key that can save
    memories may legitimately be unable to search them.
    """


class NotFoundError(RecuerdosError):
    """404 — no such resource.

    Also what you get for a memory belonging to another user. The two are
    indistinguishable on purpose, so this exception cannot distinguish
    them either.
    """


class ConflictError(RecuerdosError):
    """409 — violates a uniqueness rule."""


class ServerError(RecuerdosError):
    """5xx — a server fault.

    The message is always the literal ``"internal error"``; the detail is
    in the server log under ``request_id``.
    """


class TimeoutError_(RecuerdosError):
    """A local timeout: the client gave up waiting.

    Distinct from a server error because the work may still be running.
    After :meth:`Client.wait_for_job` times out, the job usually has not
    failed — it is still being processed, and polling again is reasonable.
    """


class JobFailedError(RecuerdosError):
    """An ingestion job reached ``failed`` — out of retry attempts."""

    def __init__(self, message: str, *, job_id: str, attempts: int) -> None:
        super().__init__(message, code="job_failed")
        self.job_id = job_id
        self.attempts = attempts


_BY_CODE: dict[str, type[RecuerdosError]] = {
    "validation_failed": ValidationError,
    "unauthorized": AuthenticationError,
    "forbidden": PermissionError_,
    "not_found": NotFoundError,
    "conflict": ConflictError,
    "internal": ServerError,
}

_BY_STATUS: dict[int, type[RecuerdosError]] = {
    400: ValidationError,
    401: AuthenticationError,
    403: PermissionError_,
    404: NotFoundError,
    409: ConflictError,
    422: RejectedError,
}


def from_response(status: int, body: Any, request_id: str | None) -> RecuerdosError:
    """Builds the exception for an error response.

    Falls back through code, then status, then the base class. A proxy
    that returns an HTML 502 has no envelope at all, and that has to
    surface as a RecuerdosError rather than a ``KeyError`` from inside
    the SDK.
    """
    code: str | None = None
    message = ""
    details: dict[str, Any] | None = None

    if isinstance(body, dict):
        envelope = body.get("error")
        if isinstance(envelope, dict):
            raw_code = envelope.get("code")
            code = raw_code if isinstance(raw_code, str) else None
            raw_message = envelope.get("message")
            message = raw_message if isinstance(raw_message, str) else ""
            raw_details = envelope.get("details")
            details = raw_details if isinstance(raw_details, dict) else None

    if not message:
        message = f"HTTP {status}"

    # A memory rule's refusal is always a 422, whatever its code: its
    # ``conflict`` is a contradiction with a stored memory, not the 409
    # uniqueness error that shares the word.
    kind = (
        RejectedError
        if status == 422
        else _BY_CODE.get(code or "") or _BY_STATUS.get(status)
    )
    if kind is None:
        kind = ServerError if status >= 500 else RecuerdosError

    return kind(
        message, code=code, status=status, request_id=request_id, details=details
    )
