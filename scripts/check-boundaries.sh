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

# Rule 1: domain imports only shared + std (+ its own context's domain).
# A domain file may not `use crate::<other-context>::` at all, and may not
# reach into any context's application/infrastructure layer, including its
# own.
for ctx in $contexts; do
    dir="src/$ctx/domain"
    [ -d "$dir" ] || continue
    while IFS= read -r match; do
        note_violation "$match (domain must import only shared + std)"
    done < <(grep -rnE '^\s*use crate::' "$dir" --include='*.rs' \
        | grep -vE 'use crate::shared(::|;| )' \
        | grep -E "use crate::($(echo "$contexts" | tr ' ' '|')|bootstrap)::" || true)
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
