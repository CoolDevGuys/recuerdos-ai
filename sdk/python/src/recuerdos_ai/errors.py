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
    ) -> None:
        super().__init__(message)
        self.message = message
        self.code = code
        self.status = status
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

    if isinstance(body, dict):
        envelope = body.get("error")
        if isinstance(envelope, dict):
            raw_code = envelope.get("code")
            code = raw_code if isinstance(raw_code, str) else None
            raw_message = envelope.get("message")
            message = raw_message if isinstance(raw_message, str) else ""

    if not message:
        message = f"HTTP {status}"

    kind = _BY_CODE.get(code or "") or _BY_STATUS.get(status)
    if kind is None:
        kind = ServerError if status >= 500 else RecuerdosError

    return kind(message, code=code, status=status, request_id=request_id)
