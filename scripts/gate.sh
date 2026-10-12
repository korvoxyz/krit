#!/usr/bin/env bash
# Run the checks that CI runs, locally, with CI's toolchain and flags.
#
# Mirrors .github/workflows/ci.yml step for step: the "Rust 1.95 quality" job
# (formatting, the example metrics, Clippy, the workspace tests and
# conformance, and the documentation tests) and the "Supply chain" job
# (cargo deny and cargo audit, at the versions CI installs). Cargo runs with
# +TOOLCHAIN and --locked, and in CI's environment: CARGO_INCREMENTAL=0,
# CARGO_TERM_COLOR=never, RUST_BACKTRACE=1, and no RUSTFLAGS or RUSTDOCFLAGS.
#
# Usage: scripts/gate.sh [--toolchain X] [--skip-supply] [--target-dir DIR]
#
#   --toolchain X     rustup toolchain to run (default: 1.95.0, CI's quality
#                     toolchain); `stable` repeats the current-stable tests
#   --skip-supply     skip cargo deny and cargo audit, which fetch advisory
#                     data over the network; a pass is then partial
#   --target-dir DIR  Cargo target directory (default: target/gate-X), kept
#                     apart from the everyday build cache
#   -h, --help        print this help
#
# Every step runs even when an earlier one fails. The script prints the
# toolchain, OS, and commit it ran on, a result line per step, and a final
# PASS or FAIL line that names the toolchain, OS, and commit again. It exits
# 0 on PASS, 1 on FAIL, and 2 on a usage or setup error.

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd -P)"
if [[ ! -f "$ROOT/Cargo.toml" || ! -d "$ROOT/crates/krit" ]]; then
    echo "error: $ROOT does not look like the Krit repository" >&2
    exit 2
fi

# The versions that ci.yml's supply-chain job installs; keep them in step.
CARGO_DENY_VERSION=0.20.2
CARGO_AUDIT_VERSION=0.22.2

usage_error() {
    echo "error: $1 (see --help)" >&2
    exit 2
}

toolchain=1.95.0
supply=true
target_dir=
while [[ $# -gt 0 ]]; do
    case "$1" in
        --toolchain)
            [[ $# -ge 2 ]] || usage_error "--toolchain needs a value"
            toolchain="$2"
            shift 2
            ;;
        --toolchain=*)
            toolchain="${1#*=}"
            shift
            ;;
        --target-dir)
            [[ $# -ge 2 ]] || usage_error "--target-dir needs a value"
            target_dir="$2"
            shift 2
            ;;
        --target-dir=*)
            target_dir="${1#*=}"
            shift
            ;;
        --skip-supply)
            supply=false
            shift
            ;;
        -h | --help)
            awk 'NR > 1 && /^#/ { sub(/^# ?/, ""); print; next } NR > 1 { exit }' "$0"
            exit 0
            ;;
        *) usage_error "unknown argument '$1'" ;;
    esac
done

toolchain="${toolchain#+}"
if [[ ! "$toolchain" =~ ^[A-Za-z0-9][A-Za-z0-9._-]*$ ]]; then
    usage_error "'$toolchain' is not a rustup toolchain name"
fi
if [[ -z "$target_dir" ]]; then
    target_dir="$ROOT/target/gate-$toolchain"
elif [[ "$target_dir" != /* && "$target_dir" != [A-Za-z]:* ]]; then
    # Relative to where the script was started, as Cargo would read it.
    target_dir="$PWD/$target_dir"
fi

cd "$ROOT"
export CARGO_TARGET_DIR="$target_dir"
export CARGO_INCREMENTAL=0 CARGO_TERM_COLOR=never RUST_BACKTRACE=1
unset RUSTFLAGS RUSTDOCFLAGS CARGO_ENCODED_RUSTFLAGS CARGO_ENCODED_RUSTDOCFLAGS \
    CARGO_BUILD_RUSTFLAGS CARGO_BUILD_RUSTDOCFLAGS
# A missing toolchain is a setup error, not something to download unasked.
export RUSTUP_AUTO_INSTALL=0

if ! rustc_version="$(rustc +"$toolchain" --version 2>/dev/null)"; then
    echo "error: Rust toolchain '$toolchain' is not installed; install it with" >&2
    echo "  rustup toolchain install $toolchain --profile minimal --component clippy,rustfmt" >&2
    exit 2
fi
os="$(uname -srm)"
if commit="$(git rev-parse --short=12 HEAD 2>/dev/null)"; then
    if [[ -n "$(git status --porcelain 2>/dev/null || true)" ]]; then
        commit="$commit with uncommitted changes"
    fi
else
    commit="unknown commit"
fi

echo "gate: toolchain  $rustc_version (+$toolchain)"
echo "gate: os         $os"
echo "gate: commit     $commit"
echo "gate: target dir ${target_dir#"$ROOT"/}"

names=()
results=()
failed=0

# Print a command, then run it.
run() {
    printf '$ %s\n' "$*"
    "$@"
}

step() {
    local name="$1" start status
    shift
    printf '\ngate: ==> %s\n' "$name"
    start=$SECONDS
    if "$@"; then
        status=ok
    else
        status=FAILED
        failed=$((failed + 1))
    fi
    names+=("$name")
    results+=("$(printf '%-7s %5ss' "$status" $((SECONDS - start)))")
    printf 'gate: <== %s: %s\n' "$name" "$status"
}

skip() {
    names+=("$1")
    results+=("skipped")
}

example_metrics() {
    run python3 -I scripts/metrics.py --check-depth 3 examples &&
        run python3 -I scripts/metrics.py --check-lines 50 examples/webhook-agent.krit
}

# Run `cargo TOOL ARGS...` after checking that TOOL is the version CI runs.
supply_tool() {
    local tool="$1" pinned="$2" version
    shift 2
    if ! version="$(cargo +"$toolchain" "$tool" --version 2>/dev/null)"; then
        echo "error: cargo-$tool is not installed; install CI's version with" >&2
        echo "  cargo install --locked --version $pinned cargo-$tool" >&2
        return 1
    fi
    version="${version##* }"
    if [[ "$version" != "$pinned" ]]; then
        echo "error: cargo-$tool $version is installed, but CI runs $pinned; install it with" >&2
        echo "  cargo install --locked --version $pinned cargo-$tool" >&2
        return 1
    fi
    echo "cargo-$tool $version"
    run cargo +"$toolchain" "$tool" "$@"
}

step "Check formatting" run cargo +"$toolchain" fmt --all --check
step "Check example metrics" example_metrics
step "Run Clippy" run cargo +"$toolchain" clippy --workspace --all-targets --locked -- -D warnings
step "Run tests and conformance" run cargo +"$toolchain" test --workspace --all-targets --locked
step "Run documentation tests" run cargo +"$toolchain" test --workspace --doc --locked
if $supply; then
    step "Check dependencies with cargo deny" supply_tool deny "$CARGO_DENY_VERSION" --locked check
    step "Audit dependencies with cargo audit" supply_tool audit "$CARGO_AUDIT_VERSION"
else
    skip "Check dependencies with cargo deny"
    skip "Audit dependencies with cargo audit"
fi

echo
echo "gate: summary"
for i in "${!names[@]}"; do
    printf 'gate:   %-38s %s\n' "${names[$i]}" "${results[$i]}"
done
where="$rustc_version | $os | $commit"
if [[ $failed -gt 0 ]]; then
    echo "gate: FAIL ($failed of ${#names[@]} steps failed) | $where"
    exit 1
elif $supply; then
    echo "gate: PASS | $where"
else
    echo "gate: PASS without the supply-chain checks (--skip-supply) | $where"
fi
