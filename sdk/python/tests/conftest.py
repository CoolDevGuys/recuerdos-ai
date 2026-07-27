"""Shared fixtures.

Integration tests talk to a real daemon rather than a mock, because the
thing worth testing is whether the SDK and the server actually agree —
and a mock built from the same reading of the docs that produced the
client would agree with itself no matter what the server does.

Point them at a daemon with ``RECUERDOS_AI_TEST_URL``; without it they
skip, so a plain ``pytest`` on a laptop with no Docker still passes.
``just sdk-test`` brings one up.
"""

from __future__ import annotations

import os
import uuid

import pytest

from recuerdos_ai import Client


@pytest.fixture(scope="session")
def base_url() -> str:
    url = os.environ.get("RECUERDOS_AI_TEST_URL")
    if not url:
        pytest.skip("set RECUERDOS_AI_TEST_URL to run integration tests")
    return url


@pytest.fixture
def ra(base_url: str) -> Client:
    """A client against the test daemon.

    The daemon runs with ``[auth].mode = "none"``, so no key is needed:
    minting one requires CLI access to the daemon's own data directory,
    which would mean either a privileged back door or a much more
    elaborate fixture. What a key changes on the wire — the header, and
    the errors for a missing or insufficient one — is covered by the unit
    tests, where it can be asserted exactly.
    """
    with Client(base_url=base_url, api_key=None) as client:
        yield client


@pytest.fixture
def tag() -> str:
    """A tag unique to one test.

    The daemon is shared across the session and its store persists, so
    tests filter on this rather than assuming an empty store — otherwise
    they pass alone and fail together, which is the worst way to find
    out.
    """
    return f"t{uuid.uuid4().hex[:12]}"
