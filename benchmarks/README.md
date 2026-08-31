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
