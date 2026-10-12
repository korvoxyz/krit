# Krit 0.3 "Honest and Flat" — product requirements

**Status:** Accepted for implementation (2026-10-11)
**Owner:** Akshay Bhardwaj
**Tracker:** [tracker-0.3.md](tracker-0.3.md)
**Long-range plan:** [roadmap.md](roadmap.md)

## 1. Why this release exists

Krit 0.2 has a strong authority model: literal-resource capabilities re-proven
against Wasm imports, opaque secrets, exact-origin egress, and transactional
durable state. But measured against real programs it fails its own promise:

- `run`, `check`, and `build` accept three different languages. Seven of eight
  realistic agent programs pass `krit check` and then fail `krit build` with
  K7002. `if request.method == "POST"` runs nowhere: `check` accepts it,
  `run` refuses webhooks, and `build` rejects string equality.
- Every fallible host call costs a two-arm `match`, so the reference agent
  nests 15 indentation levels deep in 79 lines.
- Deep recursion and deep nesting abort the process (exit 134) instead of
  producing a diagnostic.
- Any granted secret can be sent as the bearer token to any granted origin.
- Every release rejects artifacts built by the previous release.
- Common LLM habits (`return`, `for`, `.len()`, `null`) produce generic syntax
  errors with no hint.

0.3 makes Krit **honest** (what checks is what ships, failures are
diagnostics, authority is bound) and **flat** (the happy path reads top to
bottom at depth ≤ 3). It folds the master plan's 0.2.1 "stop the bleeding",
0.2.2 "honest check", and the flat-code and stdlib-v0 parts of its 0.3 into
one release. The deep refactors (NodeIds, multi-error sink, registry, Core
interpreter, durable protocol 1.1) become 0.4 "One semantics"; see §9.

## 2. Users and use cases

Primary: engineers who let AI write small bots and integrations and must
approve them — beachhead is self-hosted AI repo bots (GitHub issue/PR triage,
summaries, digests to chat). 0.3 does not yet deploy the full triage bot
(that needs `type`/`enum`/`json_parse`, 0.4), but it must make these
deployable today: outbox/retry workers, idempotent ingestion, RAG
pass-through with composed prompts, path routing on `request.method` and
`request.path`, and webhooks that compose strings for logs and responses.

## 3. Goals and non-goals

### Goals

1. **G1 Honest:** no process aborts; `run` refuses what `check` refuses;
   `check --target component` reports exactly the K7002 set `build` would,
   at real spans; string `==`, ordering, and `+` deploy.
2. **G2 Bound authority:** secrets are bound to the origins they may reach,
   enforced by the host; dependencies fail closed; dot-segment path
   injection is refused.
3. **G3 Flat:** `?`, let-else, nested patterns with `_`, named construction,
   entry lists, `()`, `///`, interpolation and text blocks — all additive in
   edition 2026, all deployable where the underlying types deploy.
4. **G4 Useful:** a first pure stdlib (`int_to_string`, URL builders, string
   predicates) that works in `run` and in Wasm.
5. **G5 Repairable:** habit codes and did-you-mean hints for the constructs
   models import from other languages; warnings channel.
6. **G6 Upgradeable:** 0.2.0 artifacts load on the 0.3 runtime; Wasmtime
   moves to the supported 48 LTS line.

### Non-goals (0.3)

User-defined `type`/`enum`, generics/let-polymorphism, modules, `json_parse`,
general Wasm layouts for user records and lists, Core interpreter, multi-error
reporting, durable protocol 1.1, `krit test`, `krit-action`, KritBench, MCP.
Resource-handle syntax (`uses { work: state("…") }`) is an open RFC
(RFC-0007), not a 0.3 feature.

## 4. Requirements

Each requirement lists its acceptance check. Codes come from the master plan's
single allocation (roadmap §9.2).

### M1 — Safety and honesty

| ID | Requirement | Acceptance |
|---|---|---|
| R1.1 | **Depth guards.** Parser, analyzer, Core lowering, formatter, and both Wasm compilers stop at a nesting limit (256 syntactic levels; an `else if` continuation does not add a level) with **K1005** and a span. The direct evaluator limits call depth (10,000 frames) with **K4010** and runs on a stack sized so the limit is always reached before a native overflow. | 2,000-deep nesting → K1005 in every stage; 100,000-deep recursion → K4010; no exit 134 anywhere; a 1,000-branch `else if` chain still checks. |
| R1.2 | **`run` is gated on analysis.** `krit run` performs the same analysis as `krit check` and refuses on any error with check's diagnostic and exit status. It builds no artifact, so it skips the build-target check; an entrypoint file that fails only that check is refused with K5003, as every entrypoint file is (spec/LANGUAGE.md section 14). Ill-typed runtime conformance cases move to `conformance/check`. | `run` refuses every `conformance/check` failure. |
| R1.3 | **Dependencies fail closed.** A manifest with a non-empty `[dependencies]` table is rejected with **K6003** by `package check`, `build`, `sandbox`, `permissions`. | New conformance/package case. |
| R1.4 | **Artifact compatibility window.** Load by membership in supported sets (metadata schema, policy version, `krit:runtime` WIT version, edition); the compiler version is recorded, not compared; effects are still re-derived from imports. Runtime N supports artifacts of the same and two previous minor toolchains, never newer. | A checked-in 0.2.0-built artifact loads and runs on 0.3. |
| R1.5 | **Secret→origin binding.** Manifest form `[secrets.<name>] origins = ["https://…"]` (`inject = "bearer"` default). The host refuses injection to an unbound origin with **K5006** before any bytes leave. The legacy flat `secrets = [...]` form still parses and is reported `UNBOUND (any granted origin)` by `krit permissions`. | The swapped-token probe (webhook-agent sending `github-token` to the message origin) is refused once bindings are declared. |
| R1.6 | **Path hardening.** The host rejects `.`/`..` path segments and percent-encoded `.`, `/`, `\`; outbound requests are sent exactly as validated (`CURLOPT_PATH_AS_IS`). | Conformance/runtime tests for each rejected form. |
| R1.7 | **Papercuts.** K7002 messages name the correct policy; config keys accept `_`; prompt-pack version single-sourced; host default `User-Agent`; `SECURITY.md` with a private disclosure process. | Tests/doc present. |
| R1.8 | **Platforms.** CI runs tests on Linux, macOS, Windows. On non-Unix hosts, `invoke`/`worker`/`schedule`/`serve` with durable state or secrets refuse to start (owner-only file checks are no-ops there). | CI matrix green; refusal test. |
| R1.9 | **Wasmtime 48.0.5 LTS**, MSRV 1.95, CI toolchain updated; golden digests re-baselined if they change. | Workspace green; docs updated. |

### M2 — Honest check and deployable strings

| ID | Requirement | Acceptance |
|---|---|---|
| R2.1 | **`krit check --target component\|module`** reports every K7002/K7001 the build would, each at the originating source span (none at `1:1`), by running full emission with output discarded. `krit build --diagnostic-format json`. The backend support checker returns all findings, not the first; dead helpers are eliminated before the residual-type check. | For every example, conformance, and probe file: `check --target` K7002 set == `build` K7002 set. |
| R2.2 | **Guest strings.** String `==`, `!=`, `<`, `<=`, `>`, `>=` (byte-lexicographic over UTF-8) and `+` (concatenation) lower to Wasm in entrypoint programs (and module programs where strings are supported). | `request.method == "POST"` routing and `"hi " + request.path` webhooks build, `invoke` correctly, and agree with `run` on a string case corpus. |
| R2.3 | **JSON string unescape in Wasm.** `json_decode` to `String` handles every JSON escape (`\n`, `\"`, `\\`, `\/`, `\b`, `\f`, `\r`, `\t`, `\uXXXX` incl. surrogate pairs) instead of trapping. | Escaped-model-output build case passes; lone surrogate is an error, not a trap without span. |

### M3 — Flat code (edition 2026, additive)

| ID | Requirement | Acceptance |
|---|---|---|
| R3.1 | **Postfix `?`.** Grammar: `call = primary ( "(" args? ")" \| "." ident \| "?" )*`. If `e: Result<T, E>` then `e?: T`; if `e: Option<T>` then `e?: T`. Valid only in a named `fn` (including `queue`/`schedule` entrypoints) whose declared return type is `Result<_, E>` with the *identical* `E`, or `Option<_>`; otherwise **K3012** (not allowed here, e.g. closures, webhook entrypoints, top level) or **K3013** (error type differs / family mismatch). Semantics: evaluate `e`; on `Err(x)`/`None` the function returns `Err(x)`/`None` immediately; effects already performed stay performed; evaluation is left to right. Core gains a verified function-exit operation; Wasm lowers it to `return`. | Conformance for order (`f(log_info("a", []), g()?)`), both families, K3012/K3013, Wasm build of a webhook helper using `?`. |
| R3.2 | **let-else.** `let PATTERN (: T)? = EXPR else ARM;` where `ARM` is `PATTERN => EXPR` and the two patterns are the complementary arms of one Option/Result family (`Ok`/`Err`, `Some`/`None`). The arm's value becomes the enclosing function's return value. Only a direct statement of a function body; the scrutinee may not end in `}`. Violations: **K3014**. Equivalent to `match EXPR { PATTERN => rest-of-block, ARM }`. | Conformance; webhook using let-else builds. |
| R3.3 | **Nested built-in patterns and `_`.** `pattern = binder \| "_" \| ("Ok"\|"Err"\|"Some") "(" pattern ")" \| "None" \| "[" "]" \| "[" (binder\|"_") "," ".." (binder\|"_") "]"`. Exhaustiveness is checked over the nested built-in families; missing combinations are reported verbatim (K3005), unreachable/duplicate arms are rejected, a top-level catch-all `_ =>` arm is rejected (every outcome is named). `_` binds nothing and may repeat; reading `_` as a value is warning **K2003** (error in edition 2027). | `Ok(Some(x)) / Ok(None) / Err(e)` matches check, run, and build. |
| R3.4 | **Named construction.** `TypeName { field: expr, punned, … }` for the built-in contract record types `HttpRequest`, `HttpResponse`, `HttpHeader`, `LogField`. Spec-fixed defaults: `HttpRequest { method, path` required, `query: "", headers: [], body: "" }`; `HttpResponse { status` required, `headers: [], body: "" }`; `HttpHeader`/`LogField` require `name`, `value`. `x` alone means `x: x`. Unknown, duplicate, or missing-required field: **K3016**. The value has exactly the type `record { … }` would. | `record` count in examples drops to 0 where named construction applies. |
| R3.5 | **Entry lists, unit literal, doc comments.** `["k": v, …]` means `[record { name: k, value: v }, …]` (no mixing with plain elements). `()` is the Unit value. `///` doc comments attach to the next item, are preserved by the formatter, and are shown by LSP hover. | Formatter idempotent on all new forms. |
| R3.6 | **Interpolation and text blocks.** `"…\(name.field)…"`: a hole is an identifier optionally followed by field accesses, of type `String` (**K3015** otherwise — use `int_to_string`); it desugars to concatenation. An interpolated string is never a capability literal (K3008 if used as a resource argument). `"""` text blocks: opening `"""` + line break, closing `"""` on its own line; the closing line's indentation is removed from every line; escapes and holes work; no trailing newline. | Conformance; prompt composition webhook builds. |
| R3.7 | **Formatter** has one canonical spelling for every new form; it breaks long `+` chains one operand per line, keeps digit separators, and keeps lines ≤ 100 columns where breakable. | `fmt --check` idempotence suite. |

### M4 — Stdlib v0 and AI authoring

| ID | Requirement | Acceptance |
|---|---|---|
| R4.1 | **Stdlib v0** (pure, no effects, deterministic, flat prefixed names): `int_to_string(Int) -> String`; `url_path_segment(String) -> Result<String, String>` (percent-encodes all but RFC 3986 unreserved; `Err` for text the host path rule would refuse — empty, `/`, `\`, ASCII control characters, `%` followed by a hex digit or `u`/`U`, or `.`/`..` after the host's `;?#` cut and space trim; the `Err` table in spec/LANGUAGE.md section 12.1 is normative — so untrusted text fails recoverably instead of trapping the invocation; every `Ok` output is accepted by the host path rule); `url_query(List<{name, value}>) -> String` (`k=v&…`, percent-encoded, no leading `?`); `string_byte_len(String) -> Int`; `string_is_empty(String) -> Bool`; `string_contains(String, String) -> Bool`; `string_starts_with(String, String) -> Bool`; `string_ends_with(String, String) -> Bool`. Each works in `check`, `run`, LSP, and Wasm. | Per-function conformance and evaluator/Wasm agreement. |
| R4.2 | **K9013 lint** (warning): a non-literal piece of an `HttpRequest` `path` or `query` (interpolation hole or `+` operand) that is not a call to `url_path_segment`, `url_query`, or `int_to_string`. | Lint fires on the injection probe, not on the examples. |
| R4.3 | **Warnings channel.** Diagnostics carry severity; `check`, `run`, `build`, LSP print warnings (human: `warning[K…]`; JSON: `"severity": "warning"`) without changing the exit status. | K2003/K9013 shown, exit 0. |
| R4.4 | **Habit codes** K1101–K1112 with a one-line fix hint in `notes`: `return`; `for`/`while`; assignment/`mut`/`var`/`const`; `struct`/`class`/`impl`/`trait`; `import`; `async`/`await`; `try`/`catch`/`throw`; `a[i]`; `(x) => e`; ternary; `switch`/`case`; `else { return … }`. **K2001** gains did-you-mean (and `null` → `None`); **K3011** method-call `s.len()` → stdlib function hint. | One conformance case per code. |
| R4.5 | **Prompt pack 0.3**: system prompt, prompt-pack, and `krit prompt` describe 0.3 syntax and stdlib with CI-checked examples. | Every embedded example checks. |

### M5 — Docs, examples, release

| ID | Requirement | Acceptance |
|---|---|---|
| R5.1 | Spec updated: LANGUAGE.md (grammar + desugarings), DIAGNOSTICS.md (new codes, K9 range, severities), TYPES-AND-EFFECTS.md, WASM-SANDBOX.md (strings, exit, compat), CAPABILITIES.md/PACKAGES.md (secret binding, K6003). | Spec matches implementation. |
| R5.2 | All examples rewritten in 0.3 style; every example has depth ≤ 3; webhook-agent ≤ 50 lines; `scripts/metrics.py` reports lines/depth/nested matches/`record` count. | Metrics table in CHANGELOG. |
| R5.3 | Conformance grows to ≥ 40 run, ≥ 60 check, ≥ 8 format cases. | Counted in CI. |
| R5.4 | `docs/roadmap.md` (master plan), `rfcs/0007-resource-handles.md` (draft), CHANGELOG, README, version 0.3.0. | Release checklist. |

## 5. Release gate (all must hold)

1. `cargo fmt --check`, `cargo clippy -D warnings`, `cargo test --workspace` green; release build.
2. Every example formats and checks; every example that is a deployable package (has a manifest, plus `factorial` via the root `krit.pkg`) builds; `examples/webhook-agent.krit` ≤ 50 lines, all examples depth ≤ 3. Exception, recorded 2026-10-12: `examples/lists.krit` is pure list processing that runs with `krit run`; Wasm list layouts and list matching are 0.4 scope (it did not build in 0.2 either), and a test pins it as the only unbuildable example.
3. A webhook routing on `request.method == "POST"` and composing strings with `+`/interpolation builds and runs through `krit invoke`.
4. 0.2.0 golden artifact runs on the 0.3 runtime.
5. Swapped-token probe refused with K5006.
6. 100k recursion → K4010; 2k nesting → K1005; no exit 134.
7. `run` refuses every check failure; `check --target` == `build` on every file.
8. Re-measure the 8 grounded use-case sketches: deployable count rises from 1 (target ≥ 4 using 0.3 features).
   **Measured 2026-10-12:** 5/8 pass check and build (02 Slack, 03 payment,
   05 ticket router, 06 flaky worker, 07 RAG), but 02, 03 and 05 only after
   scope reductions (signature check in a shim, auth via query token and
   host-SQL decoding, one-word model output). With full behavior, 2/8 (06,
   07), up from 1/8. The remaining blockers (typed decoding into records,
   record/list layouts, header lookup via list matching, HMAC) are 0.4
   scope. The gate is met only when reduced-scope rewrites count; that is the
   owner's release decision.
   The corpus is in [benchmarks/sketches](../benchmarks/sketches/README.md),
   and `cargo test -p krit-cli --test sketches` measures it again. The
   release-gate audits of `6a862f0` and `aa417b4` reproduced both figures.
   Counting the roadmap's corpus instead (03b in place of 02) gives the same
   2/8 and 5/8.
   **Owner decision on counting reduced scope:** not yet recorded.

## 6. Metrics

| Metric | 0.2.0 | 0.3 target |
|---|---|---|
| Realistic sketches that pass check *and* build | 1/8 | ≥ 4/8 (measured: 5/8 with reduced scope, 2/8 full behavior) |
| webhook-agent lines / max depth | 79 / 15 | ≤ 50 / ≤ 3 |
| Process aborts on adversarial depth | yes | none |
| Conformance run / check / format | 25 / 29 / 3 | ≥ 40 / ≥ 60 / ≥ 8 |
| Habit codes with hints | 0 | 12 |

## 7. Risks

- **Sugar before NodeIds.** The master plan puts the desugaring pass after the
  NodeId refactor because analysis facts are keyed by source span with
  uniqueness assertions. Mitigation: each feature either keeps unique spans
  (sub-spans of the source) or is represented natively; the 0.4 NodeId
  refactor absorbs it. Documented deviation.
- **Durable records across the upgrade.** Replay and inbound-idempotency
  identity still includes the artifact digest (protocol 1.1 is 0.4), so
  rebuilding with 0.3 orphans 0.2 replay records. Krit is pre-production;
  release notes state it.
- **MSRV bump** to 1.95 for Wasmtime 48 LTS.

## 8. Decisions specific to 0.3

1. Ship as one 0.3.0 release instead of 0.2.1 + 0.2.2 + 0.3 (owner request).
2. `?` uses a real Core exit operation, so it is valid in any expression
   position of an eligible function, not only on the statement spine.
3. Stdlib v0 adds five allocation-free string predicates beyond the plan's
   three functions; they reuse the guest string runtime and remove the
   grounded fit's #3 blocker for routing and validation.
4. `krit:runtime` WIT stays at 0.2.0: no host interface changes.
5. `url_path_segment` returns `Result` (the master plan sketched `String`):
   the 0.3 host path rule refuses dot segments behind `;?#`, padded dot
   segments, control-byte escapes, and double encodings, so a total function
   would turn untrusted text into an invocation trap. `Err` keeps it
   recoverable, and `?` makes it one character at the call site.

## 9. What moves to 0.4 "One semantics"

NodeId arena and module split; DiagnosticSink with recovery (multi-error) and
schema 2; registry v1; Core interpreter with FakeHost and deletion of
`evaluator.rs`; `krit test`; durable protocol 1.1 and `krit state migrate`;
compiler-proven secret pairs; `krit-action` v0; KritBench v0; fuzzing; the
language card; `krit fix`. Subsequent roadmap versions shift by one.
