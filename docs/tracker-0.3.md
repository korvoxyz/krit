# Krit 0.3 tracker

Requirements: [prd-0.3.md](prd-0.3.md). Status values: `todo`, `doing`,
`review`, `done`, `deferred`. Each item lands with tests, conformance cases
where applicable, and spec text.

Work runs in waves. Wave 1 tracks are independent and run in parallel
worktrees; wave 2 is the sequential language track built on wave 1; wave 3 is
documentation, examples, and the release gate.

## Wave 1 — safety, honest check (parallel tracks)

### Track P: platform

| ID | Item | Req | Status | Acceptance check | Notes |
|---|---|---|---|---|---|
| P1 | Wasmtime 47.0.4 → 48.0.5 LTS, MSRV 1.94 → 1.95, CI toolchain | R1.9 | review | workspace green; docs name 48 LTS | wasm-tools crates only if required |
| P2 | Artifact compatibility window (supported sets; golden 0.2.0 artifact) | R1.4 | done | 0.2.0 artifact runs on 0.3 | `validation.rs:503-548` |
| P3 | Secret→origin binding, K5006, permissions UNBOUND label | R1.5 | review | swapped-token probe refused | `network.rs`, manifest, host config |
| P4 | `[dependencies]` fail closed, K6003 | R1.3 | done | package case | `krit-package/src/lib.rs:160` |
| P5 | Host path hardening, `CURLOPT_PATH_AS_IS` | R1.6 | doing | dot/encoded-segment cases refused | `webhook.rs:115-140` |
| P6 | Papercuts: config `_`, prompt-pack version, User-Agent, SECURITY.md | R1.7 | done | tests/docs | |
| P7 | CI on Linux/macOS/Windows; Windows host-mode refusal | R1.8 | done | refusal test; ci.yml matrix | `host_config.rs:1668` |

### Track W: Wasm honesty and strings

| ID | Item | Req | Status | Acceptance check | Notes |
|---|---|---|---|---|---|
| W1 | `check --target component\|module`, all K7002 at spans, `build --diagnostic-format json`, policy-name fix | R2.1, R1.7 | done | check == build on every file | `support.rs`, `agent_compiler.rs` |
| W2 | Guest string runtime: `==`, `!=`, ordering, `+` | R2.2 | done | POST-routing webhook builds and invokes | |
| W3 | JSON unescape in Wasm `json_decode` to String | R2.3 | done | escaped model output case | `agent_compiler.rs:1200-1287` |

### Track F: front end

| ID | Item | Req | Status | Acceptance check | Notes |
|---|---|---|---|---|---|
| F1 | Depth guards K1005 (all stages), evaluator K4010 | R1.1 | doing | 2k nesting / 100k recursion | no exit 134 |
| F2 | `run` gated on analysis; move ill-typed run cases | R1.2 | doing | run refuses check failures | `main.rs:1176-1224` |
| F3 | Warnings channel (severity in human/JSON/LSP) | R4.3 | doing | warning shown, exit 0 | prerequisite for K2003/K9013 |
| F4 | Habit codes K1101–K1112, did-you-mean K2001, K3011 | R4.4 | doing | one case per code | hints in `notes` |

## Wave 2 — flat code and stdlib (sequential)

| ID | Item | Req | Status | Acceptance check | Depends |
|---|---|---|---|---|---|
| L1 | Postfix `?` with Core exit op, K3012/K3013, Wasm `return` | R3.1 | todo | order case; webhook helper builds | wave 1 |
| L2 | let-else, K3014 | R3.2 | todo | webhook builds | L1 |
| L3 | Nested built-in patterns, `_`, exhaustiveness, K2003 | R3.3 | todo | 3-arm nested match builds | F3 |
| L4 | Named construction with punning and defaults, K3016 | R3.4 | todo | examples drop `record` | |
| L5 | Entry lists, `()`, `///` (formatter + LSP hover) | R3.5 | todo | fmt idempotent | |
| L6 | Interpolation `\(…)`, `"""` text blocks, K3015 | R3.6 | todo | prompt webhook builds | W2 |
| L7 | Stdlib v0 (8 functions) + K9013 lint | R4.1, R4.2 | todo | evaluator/Wasm agreement | W2, F3 |
| L8 | Formatter polish: `+` chains, digit separators, 100 cols | R3.7 | todo | idempotence suite | L1–L7 |

## Wave 3 — docs, examples, release

| ID | Item | Req | Status | Acceptance check |
|---|---|---|---|---|
| D1 | Spec updates (LANGUAGE, DIAGNOSTICS, TYPES-AND-EFFECTS, WASM-SANDBOX, CAPABILITIES, PACKAGES) | R5.1 | todo | spec matches code |
| D2 | Examples rewritten; `scripts/metrics.py`; depth ≤ 3 | R5.2 | todo | metrics table |
| D3 | Conformance ≥ 40 run / 60 check / 8 format | R5.3 | todo | counts |
| D4 | Prompt pack 0.3 with checked examples | R4.5 | todo | examples check |
| D5 | `docs/roadmap.md`, RFC-0007 resource handles (draft), CHANGELOG, README, version 0.3.0 | R5.4 | todo | checklist |
| D6 | Release gate (PRD §5) and grounded-sketch re-measure | §5 | todo | all gates |

## Log

- 2026-10-11 — Baseline on `release/0.3` (`e1bff4f`): fmt and clippy clean,
  629 tests pass, 1 ignored. PRD and tracker created.
- 2026-10-11 — Wave 1 started in four parallel worktrees: runtime (P1, P3,
  P5, P6 User-Agent), package (P4, P2, P6 papercuts, P7), wasm (W1–W3),
  frontend (F1–F4). Each item: implement → adversarial review → fix.
- 2026-10-11 — Wave 1 results: 13 of 15 items implemented, 11 approved by
  adversarial review. Package track (P4, P2, P6 papercuts, P7) and wasm
  track (W1–W3) merged into `release/0.3`: fmt and clippy clean, 701 tests
  pass. Runtime track (P1, P3, P6 User-Agent approved; P5 closing one
  traversal finding) and frontend track (F1 closing two findings, F3 in
  review, F4 pending) resumed after a session-limit interruption.
