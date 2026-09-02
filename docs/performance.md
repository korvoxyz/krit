# Performance methodology

**Status:** Accepted baseline

Krit performance claims require reproducible measurements. Rust, WebAssembly,
component runtime selection, caching, and individual optimizations are
hypotheses until they improve representative workloads.

## Priorities

For AI-authored automation, optimize in this order:

1. warm command startup
2. cached script execution
3. incremental `check`
4. clean package build
5. predictable peak memory
6. evaluator/component throughput
7. dependency resolution and fetch

Network or model latency is measured separately from language overhead.

## Workloads

The baseline suite contains:

- empty program startup
- parse-only 1 KiB, 100 KiB, and 1 MiB sources
- name and type checking across module graphs
- integer arithmetic loop
- recursive and tail-recursive calls
- closure creation and invocation
- short and long immutable list processing
- string construction and output formatting
- clean package graph build
- one-leaf incremental rebuild
- no-change cached build
- lockfile resolution from warm metadata
- capability dispatch overhead
- SQLite state read and one-mutation FULL/NORMAL commits
- checkpoint read/commit and replay hit/miss overhead
- durable idempotency reservation/completion/replay across process restart

Inputs and expected checksums are versioned. Benchmarks do not print large
results during timing.

## Profiles

- `dev`: developer feedback, debug information enabled
- `release`: shipped defaults
- `bench`: release optimization plus benchmark instrumentation

Release configuration begins with:

```toml
[profile.release]
codegen-units = 1
lto = "thin"
opt-level = 3
panic = "abort"
strip = "symbols"
```

Each option remains only if measurements justify its build-time and debugging
cost.

## Environment record

Every published result records:

- UTC date
- Git commit
- compiler and language version
- Rust compiler and target
- OS and kernel
- CPU model, physical/logical core count
- memory
- power mode
- benchmark tool and version
- profile and feature flags
- cold/warm cache state

Results from different machines are not compared as regressions.

## Sampling

- Run one untimed warm-up for warm benchmarks.
- Run at least 30 measured samples.
- Report median, p95, minimum, and median absolute deviation.
- Record peak resident memory where the platform supports it.
- Keep compiler and runtime benchmarks separate.
- Preserve raw machine-readable samples.

Means alone and single runs are not acceptable evidence.

## Regression policy

CI correctness does not depend on noisy wall-clock thresholds. A dedicated,
stable benchmark runner compares results.

Initial alert thresholds:

- more than 10% median regression and more than 2 ms absolute for latency
- more than 10% throughput regression
- more than 15% peak-memory regression
- more than 10% clean or incremental build regression

An alert prompts investigation; it does not prove a regression. Accepted
regressions require a documented correctness, security, maintainability, or
feature tradeoff.

## Startup measurement

Startup cases distinguish:

- process launch only
- CLI argument parsing
- package discovery
- source parse/check
- cache lookup
- artifact load/validation
- first user instruction

The compiler exposes tracing in development builds so aggregate timing can be
attributed without changing normal user output.

Durable-state measurements separate SQLite open/integrity validation, indexed
read, busy wait, WAL commit/fsync, replay serialization, and component/runtime
overhead. FULL and NORMAL synchronization are reported separately. Replay-hit
latency is never compared directly with provider/network latency.

## Cache measurement

For every cache:

- compare cold, warm, and invalidated states
- verify a warm result equals a clean result
- measure lookup overhead when the artifact is absent
- measure storage size and garbage-collection cost
- test concurrent readers and writers

A cache that is incorrect, secret-dependent, host-path-dependent, or slower
than recomputation is removed.

## Baseline file

`benchmarks/baseline.json` stores the initial direct-CLI environment metadata
and raw results. `benchmarks/phase3-wasm-host.json` records the first measured
host baseline: a 1,393-byte factorial component, 4.523 ms median release
process startup, 7.744 ms median end-to-end factorial sandbox invocation, and
5.453 ms median artifact-permission inspection on the recorded Apple M3 Max
environment. These are local reference measurements, not cross-machine
targets or steady-state runtime throughput. The host file preserves every raw
sample and states that process startup, validation, JIT compilation,
instantiation, execution, and cleanup are included.

No Phase 6 latency baseline is published yet. Correctness tests cover bounded
local transaction/replay behavior without introducing noisy wall-clock CI
thresholds; representative 30-sample release measurements remain a dedicated
benchmark-runner task.
