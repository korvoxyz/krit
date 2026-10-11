#!/usr/bin/env bash
# Remove Krit build output and intermediates to reclaim disk space.
#
# Dry run by default: prints what would be removed and how much space it
# frees. Pass --yes to delete. Everything removed here is regenerable with
# `cargo build`, `cargo test`, or `.github/scripts/package.py`.
#
# Usage: scripts/clean.sh [--yes] [CATEGORY...]
#
# Categories (default: debug intermediates dist scratch):
#   debug          target/debug (dev and test builds)
#   intermediates  target/release/{deps,build,incremental,.fingerprint,examples};
#                  keeps the target/release/krit binary
#   release        all of target/release, including the binary
#   dist           target/dist (packaged release archives)
#   scratch        everything else at the top of target/ (probe files,
#                  scratch and tmp directories left by earlier sessions)
#   examples       examples/target, examples/state, examples/data
#                  (local runtime state from running the examples)
#   worktrees      target/ of every other git worktree of this repository;
#                  do not use while builds are running in those worktrees
#   all            every category above

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd -P)"
if [[ ! -f "$ROOT/Cargo.toml" || ! -d "$ROOT/crates/krit" ]]; then
    echo "error: $ROOT does not look like the Krit repository" >&2
    exit 2
fi

apply=false
categories=()
for argument in "$@"; do
    case "$argument" in
        --yes) apply=true ;;
        -h | --help)
            awk 'NR > 1 && /^#/ { sub(/^# ?/, ""); print; next } NR > 1 { exit }' "$0"
            exit 0
            ;;
        debug | intermediates | release | dist | scratch | examples | worktrees) categories+=("$argument") ;;
        all) categories+=(debug release dist scratch examples worktrees) ;;
        *)
            echo "error: unknown argument '$argument' (see --help)" >&2
            exit 2
            ;;
    esac
done
if [[ ${#categories[@]} -eq 0 ]]; then
    categories=(debug intermediates dist scratch)
fi

has() {
    local wanted="$1" category
    for category in "${categories[@]}"; do
        [[ "$category" == "$wanted" ]] && return 0
    done
    return 1
}

targets=()
scratch=()
add() {
    [[ -e "$1" || -L "$1" ]] && targets+=("$1")
    return 0
}

target="$ROOT/target"
has debug && add "$target/debug"
if has release; then
    add "$target/release"
elif has intermediates; then
    for name in deps build incremental .fingerprint examples; do
        add "$target/release/$name"
    done
fi
has dist && add "$target/dist"
if has scratch && [[ -d "$target" ]]; then
    for entry in "$target"/* "$target"/.[!.]*; do
        case "$(basename "$entry")" in
            debug | release | dist | doc | CACHEDIR.TAG | .rustc_info.json | '*' | '.[!.]*') ;;
            *) scratch+=("$entry") ;;
        esac
    done
fi
if has examples; then
    for name in target state data; do
        add "$ROOT/examples/$name"
    done
fi
if has worktrees; then
    while IFS= read -r line; do
        worktree="${line#worktree }"
        [[ "$line" == worktree\ * && "$worktree" != "$ROOT" ]] && add "$worktree/target"
    done < <(git -C "$ROOT" worktree list --porcelain)
fi

if [[ ${#targets[@]} -eq 0 && ${#scratch[@]} -eq 0 ]]; then
    echo "Nothing to clean for: ${categories[*]}"
    exit 0
fi

total_kb=0
report() {
    local label="$1" kb
    shift
    kb=$(du -sk "$@" 2>/dev/null | awk '{ sum += $1 } END { print sum + 0 }')
    total_kb=$((total_kb + kb))
    printf '%10s  %s\n' "$(awk -v k="$kb" 'BEGIN { printf "%.1f MB", k / 1024 }')" "$label"
}
for path in ${targets[@]+"${targets[@]}"}; do
    report "${path#"$ROOT"/}" "$path"
done
if [[ ${#scratch[@]} -gt 0 ]]; then
    report "target/ (${#scratch[@]} stray probe and scratch entries)" "${scratch[@]}"
    targets+=("${scratch[@]}")
fi
total="$(awk -v k="$total_kb" 'BEGIN { if (k >= 1048576) printf "%.2f GB", k / 1048576; else printf "%.1f MB", k / 1024 }')"

if ! $apply; then
    echo "Dry run: $total would be freed. Re-run with --yes to delete."
    exit 0
fi

for path in "${targets[@]}"; do
    rm -rf -- "$path"
done
echo "Removed ${#targets[@]} paths, freed $total."
