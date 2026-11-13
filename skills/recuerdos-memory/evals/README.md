# Testing the skill

These cases need a Recuerdos AI daemon, because what they test is how an agent uses
the memory tools. Each case folder holds:

- `prompt.md` — the user's message (and the run settings `claude plugin eval` reads);
- `case.yaml` — what a good result looks like;
- `graders/` — one checkable statement each;
- `memories.json` — what the store holds before the run, and the git remote of the
  folder the agent works in (`folder`), so the MCP shim detects the project;
- `files/` — the project files the agent starts with, if any.

Run each case against a **scratch** daemon, with a fresh user per run, never your own
store: the agent saves and replaces memories. The baseline is the same run without the
skill (for example with only a short memory note in AGENTS.md).
