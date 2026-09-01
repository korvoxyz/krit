# Krit performance baselines

`baseline.json` records the first Rust 0.2 release-binary measurements. It is a
reference point, not a performance claim or cross-machine target.

The baseline follows `docs/performance.md`:

- release profile
- five warm-up executions per command
- 50 measured child-process executions
- monotonic nanosecond clock
- standard output and error discarded during timing
- raw millisecond samples retained
- minimum, median, p95, and median absolute deviation reported

Run future comparisons on the same dedicated machine and power profile.
Record a new file rather than overwriting historical measurements.

`phase3-wasm-host.json` adds the first policy-1 host measurements. Reproduce
the artifact and release binary with:

```sh
cargo build -p krit-cli --release --locked
./target/release/krit build
```

It records 5 warm-ups and 30 fresh child-process samples per command with
Python 3 `time.perf_counter_ns`, discarding stdout/stderr. The sandbox number
is deliberately end to end: process startup, bounded artifact/metadata load,
validation, Cranelift component compilation, fresh Store/instance creation,
isolated execution-thread creation, execution, deadline-worker join, and
shutdown. It is not an in-process throughput result. The interactive machine's
background load and power mode were not controlled, so the raw samples and
median/MAD matter more than small differences.
