"""Python client for RecordAgent — long-term memory for AI agents.

```python
from recordagent import Client

ra = Client(base_url="http://localhost:7070", api_key="ra_live_…")
ra.save("We moved the backend to Hetzner; fly.io got too expensive")

for hit in ra.search("where do we deploy?"):
    print(hit.content)
```

`recordagent.langchain` holds the LangChain retriever. It is a separate
module so that ``import recordagent`` never pulls LangChain in.
"""

from .client import DEFAULT_BASE_URL, Client
from .errors import (
    AuthenticationError,
    ConflictError,
    JobFailedError,
    NotFoundError,
    PermissionError_,
    RecordAgentError,
    ServerError,
    TimeoutError_,
    ValidationError,
)
from .models import Distillation, Job, JobStatus, MatchDetail, Memory, SearchHit

__version__ = "0.1.0"

__all__ = [
    "DEFAULT_BASE_URL",
    "AuthenticationError",
    "Client",
    "ConflictError",
    "Distillation",
    "Job",
    "JobFailedError",
    "JobStatus",
    "MatchDetail",
    "Memory",
    "NotFoundError",
    "PermissionError_",
    "RecordAgentError",
    "SearchHit",
    "ServerError",
    "TimeoutError_",
    "ValidationError",
    "__version__",
]
