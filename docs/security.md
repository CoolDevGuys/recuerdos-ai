# Security & isolation

**Coming in Phase 6** (full write-up + threat model). The isolation
guarantee itself lands in Phase 1: every storage-trait method takes
`&UserContext`, which is uncallable without passing auth — see
[project-plan.md §11](../project-plan.md#11-multi-user-isolation--security).
