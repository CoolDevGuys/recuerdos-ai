#!/usr/bin/env bash
# Cheap architecture test: greps for import/naming patterns that violate the
# layer rules in implementation-plan.md §2. Not a substitute for review, but
# catches the common mistakes fast in CI.
set -euo pipefail

cd "$(dirname "$0")/.."

fail=0
contexts="identity memories understanding providers consolidation"

note_violation() {
    echo "BOUNDARY VIOLATION: $1"
    fail=1
}

# Rule 1: a domain may import `shared`, std, and other contexts' *domain*
# — never anyone's application or infrastructure layer, and never
# bootstrap.
#
# Cross-context domain imports are the published language between
# contexts. Today that is exactly `identity::domain::UserContext`, which
# every repository contract takes so that reaching another user's data
# cannot compile. It has to live in `identity` for its constructors to
# stay `pub(in crate::identity)`; moving it to `shared` would force them
# public and throw the guarantee away. So the rule permits domain→domain
# and holds the line at the layers that actually carry framework and I/O
# dependencies.
for ctx in $contexts; do
    dir="src/$ctx/domain"
    [ -d "$dir" ] || continue
    while IFS= read -r match; do
        note_violation "$match (domain must not import application, infrastructure or bootstrap)"
    done < <(grep -rnE '^\s*use crate::' "$dir" --include='*.rs' \
        | grep -E "use crate::(bootstrap(::|;| )|($(echo "$contexts" | tr ' ' '|'))::(application|infrastructure)(::|;| ))" || true)
done

# Rule 2: application imports its own domain + shared + other contexts'
# application layer — never any context's infrastructure.
for ctx in $contexts; do
    dir="src/$ctx/application"
    [ -d "$dir" ] || continue
    while IFS= read -r match; do
        note_violation "$match (application must not import any infrastructure)"
    done < <(grep -rnE '^\s*use crate::' "$dir" --include='*.rs' \
        | grep -E '::infrastructure(::|;| )' || true)
done

# Rule 3: only bootstrap wires concrete infrastructure across contexts —
# infrastructure modules must not import another context's infrastructure.
for ctx in $contexts; do
    dir="src/$ctx/infrastructure"
    [ -d "$dir" ] || continue
    while IFS= read -r match; do
        note_violation "$match (infrastructure must not import another context's infrastructure)"
    done < <(grep -rnE '^\s*use crate::' "$dir" --include='*.rs' \
        | grep -E "use crate::($(echo "$contexts" | tr ' ' '|'))::infrastructure(::|;| )" \
        | grep -v "use crate::${ctx}::infrastructure" || true)
done

# Rule (isolation): only the identity context may mint a `UserContext`.
# rustc already enforces this — the constructors are
# `pub(in crate::identity)` — so this grep exists to catch someone
# "fixing" a compile error by widening that visibility instead of routing
# the call through authentication. See src/identity/domain/user_context.rs.
while IFS= read -r match; do
    note_violation "$match (only crate::identity may construct a UserContext)"
done < <(grep -rnE 'UserContext::(authenticated|unauthenticated)\b' src --include='*.rs' \
    | grep -v '^src/identity/' || true)

while IFS= read -r match; do
    note_violation "$match (UserContext constructors must stay pub(in crate::identity))"
done < <(grep -rnE '^\s*pub(\s|\()' src/identity/domain/user_context.rs \
    | grep -E 'fn (authenticated|unauthenticated)\b' \
    | grep -v 'pub(in crate::identity)' || true)

# Rule (naming): the suffixes *Port, *Service, *Manager, *Helper are banned
# on traits/structs — they invite logic dumping and say nothing about role.
while IFS= read -r match; do
    note_violation "$match (banned suffix: Port/Service/Manager/Helper)"
done < <(grep -rnE '^\s*(pub\s+)?(struct|trait)\s+\w+(Port|Service|Manager|Helper)\b' src \
    --include='*.rs' || true)

if [ "$fail" -ne 0 ]; then
    echo
    echo "check-boundaries.sh: violations found (see above)."
    exit 1
fi

echo "check-boundaries.sh: no violations found."
