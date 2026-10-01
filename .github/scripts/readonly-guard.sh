#!/usr/bin/env bash
# The read-only contract's structural checks (docs/ARCHITECTURE.md §9), beyond what clippy's
# deny-list already enforces. Run from the repository root:
#   .github/scripts/readonly-guard.sh              the sources
#   .github/scripts/readonly-guard.sh <exe>        the sources and a release executable
set -euo pipefail

fail() {
    echo "read-only guard: $*" >&2
    exit 1
}

# 1. Exactly one reviewed device write per hardware source crate, and no exception anywhere else.
for dir in crates/*/; do
    name=$(basename "$dir")
    # Real attributes only, not docs that mention them.
    count=$( (grep -r --include='*.rs' -E '^[[:space:]]*#\[allow\(clippy::disallowed_methods\)\]' "$dir" || true) | wc -l)
    case "$name" in
        source-api | source-sim) want=0 ;; # the shared context and the simulator never write
        source-*) want=1 ;;                # a hardware source: its transport's single write
        *) want=0 ;;
    esac
    [ "$count" -eq "$want" ] || fail "$name has $count disallowed-method exceptions, expected $want"
done

# 2. Only the source layer may depend on hidapi directly (any target platform).
dependents=$(cargo tree --target all -i hidapi --depth 1 -e normal --prefix none | tail -n +2 | awk '{print $1}' | sort -u)
for d in $dependents; do
    case "$d" in
        meltalarm-source-api) ;;
        meltalarm-source-sim) fail "the simulator depends on hidapi" ;;
        meltalarm-source-*) ;;
        *) fail "$d depends on hidapi directly" ;;
    esac
done

# 3. A release executable: the real, elevated build, with no simulator inside.
if [ $# -ge 1 ]; then
    exe=$1
    if grep -q -a 'MELTALARM_SIM' "$exe"; then fail "$exe contains the simulator"; fi
    grep -q -a 'requireAdministrator' "$exe" || fail "$exe is not the real (elevated) build"
fi

echo "read-only guard: ok"
