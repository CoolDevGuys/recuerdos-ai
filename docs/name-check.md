# Name availability check

project-plan.md §16 asks for this before launch: "verify crate/repo/domain
availability and trademark conflicts for the final name (*Recuerdos AI* is
a working title)."

Checked **2026-07-20**. Registries change; re-run before you actually
claim anything.

## Package registries

| Registry | Name | State |
|---|---|---|
| crates.io | `recuerdos-ai` | available |
| crates.io | `record-agent` | available |
| PyPI | `recuerdos-ai` | available |
| TestPyPI | `recuerdos-ai` | available |
| npm | `recuerdos-ai` | available |

The PyPI one is the only one that currently matters — `Cargo.toml` sets
`publish = false`, so the crate name is reserved-by-nobody rather than
needed. Worth claiming both anyway: a squatted package name on the
registry your users install from is expensive to undo.

## Repository and image

| | |
|---|---|
| `github.com/CoolDevGuys/recuerdos-ai` | the canonical repo (currently private, so an unauthenticated check reads as 404) |
| `ghcr.io/CoolDevGuys/recuerdos-ai` | namespaced under the same account; available by construction |

Six other GitHub repos carry the name, all dormant — the largest has 2
stars, and none is a memory service:

```
luanne/recuerdos-ai (2)   passxxx/recordAgent (0)   SaraSen/Recuerdos AI (0)
mx504/recordAgent (1)    SaraSen/Recuerdos AIBackend (0)
```

No collision worth renaming over, but the name is not distinctive on
GitHub, which is worth knowing if discoverability matters later.

## Domains

| Domain | State |
|---|---|
| recuerdos-ai.com | **registered** |
| recuerdos-ai.dev | available |
| recuerdos-ai.io | available |
| recuerdos-ai.ai | available |
| recuerdos-ai.sh | available |

Checked over RDAP. `.com` being gone is the only real constraint; `.dev`
is the conventional choice for a developer tool and is free.

## Trademark

**Not cleared, and not clearable here.** A registry query is not a
trademark search: it says nothing about unregistered common-law marks,
about registrations in classes covering software or cloud services, or
about how similar is too similar.

Two dormant GitHub repos and a parked `.com` are weak signals of nothing
in particular. If Recuerdos AI ships under this name commercially, the
remaining step is a real search (USPTO TESS, EUIPO eSearch) and, if
anything turns up, an actual lawyer. Neither is something this document
substitutes for.

## Conclusion

Nothing here blocks shipping under **Recuerdos AI**. Every name the
project needs to claim is free; `.com` is taken, which argues for
`recuerdos-ai.dev` if a domain is ever wanted.
