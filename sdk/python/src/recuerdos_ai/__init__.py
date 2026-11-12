"""Python client for Recuerdos AI — long-term memory for AI agents.

```python
from recuerdos_ai import Client

ra = Client(base_url="http://localhost:7070", api_key="ra_live_…")
ra.save("We moved the backend to Hetzner; fly.io got too expensive")

for hit in ra.search("where do we deploy?"):
    print(hit.content)
```

`recuerdos_ai.langchain` holds the LangChain retriever. It is a separate
module so that ``import recuerdos_ai`` never pulls LangChain in.
"""

from .client import DEFAULT_BASE_URL, Client
from .errors import (
    AuthenticationError,
    ConflictError,
    JobFailedError,
    NotFoundError,
    PermissionError_,
    RecuerdosError,
    RejectedError,
    ServerError,
    TimeoutError_,
    ValidationError,
)
from .models import (
    AppliesTo,
    BulkResult,
    Distillation,
    Job,
    JobStatus,
    MatchDetail,
    Memory,
    Project,
    ProjectChange,
    RerankApplied,
    RerankNotNeeded,
    RerankShadow,
    RerankSkipped,
    RerankStatus,
    SavedMemory,
    SearchHit,
    SearchReport,
    Suggestion,
    Topic,
    UpdatedMemory,
)

__version__ = "0.5.0"

__all__ = [
    "DEFAULT_BASE_URL",
    "AppliesTo",
    "AuthenticationError",
    "BulkResult",
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
    "Project",
    "ProjectChange",
    "RecuerdosError",
    "RejectedError",
    "RerankApplied",
    "RerankNotNeeded",
    "RerankSkipped",
    "RerankShadow",
    "RerankStatus",
    "SavedMemory",
    "UpdatedMemory",
    "SearchHit",
    "SearchReport",
    "ServerError",
    "Suggestion",
    "Topic",
    "TimeoutError_",
    "ValidationError",
    "__version__",
]
