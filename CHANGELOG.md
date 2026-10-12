# Changelog

All notable Krit changes are documented here.

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).
Krit is pre-1.0 and does not yet promise stable syntax between minor releases.

## [Unreleased]

## [0.3.0] - 2026-10-12

"Honest and flat." What checks is what ships, failures are diagnostics instead
of crashes, secrets are bound to the origins they may reach, and the happy path
reads top to bottom. This release folds the planned 0.2.1 and 0.2.2 releases
into 0.3; scope and gates are in [docs/prd-0.3.md](docs/prd-0.3.md).

### Highlights

- **What checks is what ships.** For a file with a webhook, queue, or schedule
  entrypoint, `krit check` runs the full WebAssembly build with the output
  discarded and reports every `K7001`/`K7002` that `krit build` would, each at
  its source span. `krit run` refuses every program that `krit check` refuses.
- **Flat code.** Postfix `?`, let-else, nested patterns with `_`, named
  construction, entry lists, `()`, `///` doc comments, string interpolation,
  and `"""` text blocks, all additive in edition 2026. The reference webhook
  agent goes from 79 lines nested 15 levels deep to 47 lines at depth 3.
- **Secrets are bound to origins.** A manifest lists the origins each secret
  may reach (`[secrets.github-token] origins = ["https://api.github.com"]`),
  and the host refuses to send it anywhere else (`K5006`) before a byte
  leaves.
- **Strings deploy.** String `==`, `!=`, ordering, `+`, interpolation, text
  blocks, and the new standard library build into components, so routing on
  `request.method == "POST"` and composed responses run through `krit invoke`.
- **No crashes on deep input.** Deep nesting reports `K1005` and runaway
  recursion reports `K4010` in every stage, instead of aborting the process
  with exit status 134.
- **Hints for LLM habits.** `return`, loops, assignment, `class`, `import`,
  `await`, `throw`, indexing, arrow functions, `?:`, `switch`, and
  `else { return … }` each get their own code (`K1101`-`K1112`) and a one-line
  fix. Undefined names get a did-you-mean, and a new warnings channel reports
  problems without failing the command.
- **Wasmtime 48 LTS.** The sandbox host moves to Wasmtime 48.0.5, supported
  upstream until 2028-08-20, and 0.2.0 artifacts keep loading through a
  published compatibility window.

### Language

- Postfix `?` returns `Err`/`None` early from a named function, including
  queue and schedule entrypoints, whose declared return type is `Result<_, E>`
  with the identical `E`, or `Option<_>`. Effects already performed stay
  performed, and evaluation stays left to right. `K3012` reports a `?` that
  cannot return (top level, closures, webhook entrypoints); `K3013` reports a
  different error type or family
- let-else: `let Ok(value) = step else Err(error) => fallback;` binds one
  variant's payload, and otherwise returns the arm's value from the enclosing
  function. It also works in webhook entrypoints, where `?` does not; misuse
  is `K3014`
- Nested built-in patterns such as `Ok(Some(x))`, `Ok(None)`, and `Err(e)`,
  with exhaustiveness checked over the nested families. Missing combinations
  are listed verbatim (`K3005`), unreachable and duplicate arms are rejected,
  and so is a top-level catch-all `_ =>` arm, so every outcome is named
- `_` binds nothing and may repeat in one scope. Reading `_` as a value is
  warning `K2003`, an error from edition 2027
- Named construction of the built-in contract records `HttpRequest`,
  `HttpResponse`, `HttpHeader`, and `LogField`, with punning (`body` means
  `body: body`) and fixed defaults (`HttpResponse { status: 200 }`). Unknown,
  repeated, or missing required fields are `K3016`
- Entry lists: `["accept": "text/plain"]` is a list of `record { name, value }`
  for HTTP headers and log fields
- The unit value `()` is an expression, so `Ok(())` works; the type is still
  spelled `Unit`
- `///` doc comments on functions and entrypoints, kept by `krit fmt` and
  shown by `krit explain` and language-server hover
- String interpolation such as `"\(request.method) \(request.path)"`. A hole
  holds a name and field accesses of type `String`; anything else is `K3015`,
  with a hint to use `int_to_string`
- `"""` text blocks with indentation removal, escapes, and holes; layout errors
  are `K0004`. Interpolated strings and text blocks are never capability
  resource literals (`K3008`)
- `<`, `<=`, `>`, and `>=` compare two strings byte by byte over UTF-8, with
  the same result in `check`, `run`, and built components
- Configuration keys may contain `_` (`config_string("agent.timeout_ms")`),
  with the same placement rules as `-`
- The habit words are not reserved: `let for = 1;` still works
- `spec/LANGUAGE.md` is the Krit 0.3 specification, with one complete,
  parser-verified grammar and each sugar defined by translation into the core
  language (section 4.3)

### Standard library

- First standard library (v0), pure and deterministic, available in `check`,
  `run`, the language server, and webhook, queue, and schedule components:
  `int_to_string`, `url_path_segment`, `url_query`, `string_byte_len`,
  `string_is_empty`, `string_contains`, `string_starts_with`, and
  `string_ends_with`
- `url_path_segment` returns `Result<String, String>`. It percent-encodes
  everything except RFC 3986 unreserved characters and returns `Err` for text
  the host path rule would refuse, so untrusted input fails as a value instead
  of trapping the invocation
- `url_query` builds `k=v&…` from an entry list, percent-encoded, with no
  leading `?`

### Diagnostics

- Diagnostics carry a severity. Warnings print as `warning[K…]` in human
  output and with `"severity": "warning"` in JSON Lines, never change the exit
  status, and are published by the language server with Warning severity
- Errors can carry one-line fix hints, printed on `help:` lines, returned in
  the JSON `notes` array, and shown by the language server
- New codes:
  - `K0004` invalid text block layout
  - `K1005` nesting depth limit (256 levels, 4,096 structural)
  - `K1101`-`K1112` constructs from other languages, each with a fix hint
  - `K2003` (warning) the discard name `_` read as a value
  - `K3011` method call on a value (was `K3004`), naming the standard-library
    function to use, such as `string_byte_len(s)`
  - `K3012` and `K3013` postfix `?` where it cannot return, or with a
    different error type or family
  - `K3014` invalid let-else
  - `K3015` invalid interpolation hole
  - `K3016` unknown, repeated, or missing field in a named construction
  - `K4010` direct-evaluation depth limit
  - `K5006` secret sent to an origin outside its binding
  - `K6003` manifest declares dependencies
  - `K8003` (info) the language server left warnings out at its limit
  - `K9013` (warning) an `HttpRequest` path or query piece that
    `url_path_segment`, `url_query`, or `int_to_string` did not encode
- `K9000`-`K9999` is reserved for lints, which are warnings by default
- Undefined names (`K2001`) suggest the closest name in scope or built-in;
  `null`, `nil`, and `undefined` suggest `None`
- `K7002` messages name the right policy: "WebAssembly policy 1" for module
  programs and "the bounded webhook ABI (WebAssembly policy 2)" for entrypoint
  programs
- Manifest diagnostics report their specific code instead of always `K6001`
- `spec/DIAGNOSTICS.md` lists every emitted code exactly once with its severity

### Compiler and Wasm backend

- The backend support check returns every finding, sorted by source position,
  instead of only the first. Composite equality and string operators are
  reported at their span instead of `1:1`
- Unused helpers are eliminated before layout checks, so an unused generic
  helper no longer fails a build with `K7001`
- A guest string runtime lowers String `==`, `!=`, ordering, `+`,
  interpolation, text blocks, and the standard library into webhook, queue,
  and schedule components
- `?` and let-else lower to a verified Core function-exit operation and to Wasm
  `return`, so a run of let-else statements does not nest deeper
- Component `json_decode` into `String` decodes every JSON escape, including
  `\uXXXX` surrogate pairs, and accepts surrounding whitespace, so model output
  such as `"line one\nline two"` no longer traps. Invalid JSON is `K4009`, as
  in `krit run`, and the input is never echoed
- Guest code that runs out of memory, for example through runaway string
  concatenation, reports `K5103` (resource limit) instead of a generic `K4001`
  trap
- Depth guards in the parser, analyzer, Core lowering, formatter, and both Wasm
  compilers. Inferred types nested more than 256 levels fail fast with `K1005`,
  and a 1,000-branch `else if` chain still checks, runs, formats, and builds
- Typing a `match` costs one unification per pattern position, so wide nested
  matches check in roughly linear time
- Fixed: unifying two bindings whose types contain each other reports `K3001`
  instead of overflowing the stack, and comparing a contract-typed value with
  an equivalent record no longer crashes the compiler with `KICE0001`

### Runtime and security

- Secret→origin binding: `[secrets.<name>] origins = ["https://…"]` in the
  manifest, with `inject = "bearer"` optional and the only accepted value. The
  host refuses injection to any other origin with `K5006` before anything is
  resolved or sent. AI adapter and search connector credentials in host
  configuration may narrow a binding but never widen it
- Request path hardening for inbound webhooks, `http_request`, durable
  `replay_http`, AI adapter paths, and search connector paths. The host refuses
  with `K4001`: `.` and `..` segments, including ones hidden behind `;`
  parameters, an escaped `?` or `#`, or escaped spaces; percent-encoded `.`,
  `/`, and `\`; double encodings; escaped ASCII control bytes; overlong UTF-8
  escapes; malformed escapes; and raw spaces. Outbound requests are sent
  exactly as validated (`CURLOPT_PATH_AS_IS`)
- Outbound host requests send `User-Agent: krit/<version>` unless the program
  sets one; an empty value turns the header off
- The CLI runs on a 256 MiB stack thread and `krit run` evaluates on a 1 GiB
  stack thread, both virtual reservations, so the depth limits are always
  reached before a native stack overflow
- On Windows and other non-Unix hosts, `invoke`, `worker`, `schedule`, and
  `serve` refuse host configurations that grant secrets, durable state, jobs,
  or application databases (`K5001`, exit status 4), because owner-only file
  checks exist only on Unix
- Wasmtime 48.0.5 LTS. Fuel, deadline, memory, stack, and host-call limits
  behave as before, and bulk memory and table operators are still rejected by
  validation
- `SECURITY.md` documents supported versions and private disclosure through
  GitHub private vulnerability reporting

### Tooling

- `krit check --target component|module|none` runs or skips the full build
  pipeline with its output discarded. Entrypoint files are checked against the
  component target by default
- `krit build --diagnostic-format human|json`
- Warnings and fix hints in `krit check`, `run`, `build`, `explain`, and the
  language server. The language server publishes at most 100 warnings per
  document followed by a `K8003` notice, and truncates oversized messages
  instead of exiting
- `krit permissions`, in human and `--json` form, with or without `--artifact`,
  shows each secret's binding
- `krit fmt` has one canonical spelling for every new form. It breaks long
  operator chains before each operator, `+` string building included, keeps
  integer digit separators (`1_000_000`), and keeps lines within 100 columns
  wherever a break point exists. A comment inside a chain keeps its place
- `krit assist` accepts candidates whose only diagnostics are warnings, and
  advertises the prompt-pack version read from `ai/prompt-pack.json`
- Prompt pack 0.3.0: `krit prompt` prints `KRIT-0.3-SYSTEM.md`, which teaches
  idiomatic 0.3, the standard library, the bounded component subset, and a
  "Krit is not Rust, TypeScript, or Python" table, in about 6,600 tokens
- Every Krit example in `README.md`, `spec/`, the AI material, and `rfcs/` is
  checked in CI with the real compiler
- `scripts/metrics.py` reports lines, depth, nested matches, duplicated `Err`
  arms, `record` uses, longest line, and authority literals (`--json`,
  `--check-depth`, `--check-lines`). CI fails if an example is nested deeper
  than 3 levels or `webhook-agent` grows past 50 lines
- `scripts/clean.sh` reclaims build output; it is a dry run unless given
  `--yes`
- CI runs the test suite on Linux, macOS, and Windows (Windows is advisory
  until its first green run), with Rust 1.95.0
- Examples are rewritten in idiomatic 0.3, all at depth 3 or less, with
  unchanged capabilities and permissions. Eight of nine build to a component;
  `examples/lists.krit` runs only with `krit run` until list layouts land
- Conformance gains package and build-compatibility suites; every runtime case
  must pass checking, cases can declare `expect.warnings`, and a test enforces
  a floor of 40 run, 60 check, and 8 format cases
- Added `docs/roadmap.md` (the plan to 1.0), an RFC template, and draft
  RFC-0007 on resource handles
- The grounded use-case sketches behind release gate 8 are kept in
  `benchmarks/sketches/`, and a test measures them again on every run, so the
  sketch figures below can be reproduced

### Compatibility and upgrade notes

- The minimum supported Rust version is 1.95, which Wasmtime 48 requires
- Artifact compatibility window: a runtime loads an artifact when its metadata
  schema, validation policy, `krit:runtime` WIT version, and edition are in
  the runtime's supported sets and its recorded toolchain is the runtime's own
  minor version or one of the two before it, never newer. 0.2.0 artifacts load
  and run on 0.3 (golden artifacts in `conformance/build/compat/0.2.0`).
  Newer, other-major, and unparsable versions are refused with `K7004`
  (adjacent metadata) or `K7003` (component metadata). The `krit:runtime` WIT
  package stays at 0.2.0; 0.3 artifacts record compiler and language version
  0.3.0
- Secret bindings: the legacy flat `[capabilities] secrets = [...]` list is
  still accepted, but `krit permissions` reports those secrets as
  `UNBOUND (any granted origin)`, and the host may send them to any granted
  origin. Declare `[secrets.<name>] origins` to get host enforcement
- A non-empty `[dependencies]` table is now rejected with `K6003` by
  `package check`, `build`, `sandbox`, `permissions`, `invoke`, `worker`,
  `schedule`, and `serve`. 0.2 ignored it silently; an empty table is still
  accepted
- `krit run` now refuses programs that fail checking, with check's diagnostic
  and exit status and no partial output; 0.2 evaluated some of them. The one
  exception is an entrypoint file that fails only the build-target check
  (`K7001`/`K7002`): `run` performs no target check because it builds no
  artifact, and refuses such a file with `K5003` and exit status 4, as it
  refuses every entrypoint file
- Reading `_` as a value still works in edition 2026 but warns (`K2003`)
- Method calls such as `s.len()` now report `K3011` instead of `K3004`
- Request paths that 0.2 accepted, such as dot segments, encoded separators,
  and raw spaces, are now refused with `K4001`
- Durable state: replay and inbound idempotency identity still includes the
  artifact digest, and a 0.3 build records a new compiler version, so
  rebuilding a 0.2 program with 0.3 orphans its replay and idempotency
  records. Durable protocol 1.1, planned for 0.4, keys that identity by
  package and declared namespace instead
- Windows host mode refuses host configurations that grant secrets or durable
  state (see Runtime and security)
- Fuel for instantiation, unchanged from 0.2 but now documented and pinned by
  tests: instantiation draws on the invocation budget. On hosts without
  copy-on-write memory images (macOS, Windows), copying a module's data image
  costs one unit per page-padded byte (16,384 units for one page on Apple
  silicon); on Linux it costs nothing. Fuel used by artifacts with linear
  memory is therefore host-dependent, so size narrowed fuel budgets for the
  most expensive host
- `krit assist` requests carry language version 0.3.0 and prompt-pack version
  0.3.0

### Known limitations

- Krit is pre-1.0 and not production-ready
- User-defined types (`type`, `enum`), generics, modules, and `json_parse` are
  not part of 0.3; they arrive in 0.4 and later. See
  [docs/roadmap.md](docs/roadmap.md)
- Components have no layouts for user records or general lists: list matching
  (including header lookup), `Some` outside the `http_request` bearer argument,
  and helpers returning `Result<HttpRequest, String>` or `Result<Bool, String>`
  are `K7002`. Typed JSON decoding and HMAC are not available yet
- The front end stops at its first error (backend findings are already
  reported together); multi-error reporting is planned for 0.4
- `K9013` inspects interpolation holes and `+` operands within one function. It
  does not flag an untrusted value used as a whole path or query

### Metrics

| Metric | 0.2.0 | 0.3.0 |
| --- | --- | --- |
| Workspace tests (`cargo test --workspace --all-targets`) | 629 | 1,021 |
| Conformance run / check / format cases | 25 / 29 / 3 | 50 / 174 / 18 |
| Conformance package / build-compatibility cases | 0 / 0 | 6 / 2 |
| `webhook-agent.krit` lines / maximum depth | 79 / 15 | 47 / 3 |
| Maximum depth across examples | 15 | 3 |
| Nested matches / `record` keywords across examples | 18 / 31 | 0 / 0 |
| Grounded sketches that check and build, full behavior | 1/8 | 2/8 |
| Grounded sketches that check and build, reduced scope counted | 2/8 | 5/8 |
| Habit codes with fix hints | 0 | 12 |
| Process aborts on adversarial depth | yes | none |

The grounded sketches are eight realistic agent programs (issue triage, chat
summaries, a payment webhook, a ticket router, a retrying worker, RAG answers,
and more) used to measure fit, as described in the release gate of
[docs/prd-0.3.md](docs/prd-0.3.md) and kept in
[benchmarks/sketches](benchmarks/sketches/README.md). In 0.2.0 a reduced-scope
payment sketch also built but was not counted, so the like-for-like comparison
is 1/8 to 2/8 with full behavior and 2/8 to 5/8 when reduced-scope rewrites
count. Three of the five that build in 0.3 (chat summaries, the payment
webhook, and the ticket router) do so only with reduced scope; full behavior
waits on typed decoding, record and list layouts, header lookup, and HMAC.

## [0.2.0] - 2026-09-05

First tagged Rust release, including the bootstrap begun on 2026-09-01 and the
completed bounded agent roadmap.

### Added

- Downloadable native CLI archives for Linux x86-64/ARM64, macOS Intel/Apple
  Silicon, and Windows x86-64, with checksums, runnable examples, build identity,
  and dependency license notices
- Reusable quality-gated CI that exercises extracted native packages and retains
  downloadable workflow artifacts
- Tagged-release publication to GitHub Releases and GitHub Packages, including
  non-root Linux AMD64/ARM64 containers and a multi-architecture GHCR index
- Pinned Actions dependencies, narrow publishing permissions, tag/version
  consistency checks, and explicit recovery for unpublished release drafts
- Normative language charter, readable syntax, runtime semantics, and
  diagnostic contract
- Draft type/effect, capability, module, and package specifications
- Rust technical design and reproducible performance methodology
- Rust workspace with source mapping, lexer, parser, direct evaluator, CLI,
  and strict manifest validation
- Stable human and JSON Lines diagnostics
- Checked signed 64-bit arithmetic
- Immutable lexical closures, recursion, lists, and exhaustive list matching
- Implementation-neutral conformance fixture format and cases
- Rust-only formatting, linting, testing, release build, and CLI CI
- Updated the provider-neutral generation prompt to version 0.2.15 for the
  optional cache and search capabilities
- Bounded namespaced TTL cache with `cache_get`, `cache_put`, and `cache_delete`
  where a hit, a miss, and an outage are three distinct guest-visible values, so
  correctness never depends on cache availability
- Process-local LRU cache backend with per-namespace and whole-cache entry and
  byte budgets, exact replacement accounting, strict inclusive expiry, bounded
  opportunistic cleanup, and no background thread
- Provider-neutral `search_query` and `vector_search` connectors with a strict
  generic `http-json` transport, a deterministic local transport, host-side
  vector dimension validation, and host re-encoded bounded results
- `cache.read`, `cache.write`, `search.query`, and `search.vector` effects with
  four separate least-authority WIT interfaces, so a reader never imports a
  write surface
- Host config schema 6 for cache namespaces and search connectors, validated
  entirely before any durable store, application database, or cache is created
- `cacheNamespaces`, `readOnlyCacheNamespaces`, `searchIndexes`, and
  `vectorIndexes` manifest capabilities with permission, explain, LSP, and
  assist redaction coverage
- Reference `examples/cached-search.krit` proving miss, search, store, and hit
  on a shared host, and identical answers with the cache disabled
- Complete search connector authority: the exact `http.request` origin and the
  `secret.read` credential are validated at setup and rechecked at dispatch, and
  a credentialed connector requires default-deny `http.bearer` approval before
  every attempt including every retry
- Mandatory HTTPS and shared origin-form path rules for `http-json` connectors
- Bounded read-only retry for search, marked internally rather than by inventing
  or transmitting an `Idempotency-Key`
- Search connector retry and rate selection identical to guest HTTP, with an
  exact-origin override and one shared per-origin rate bucket, while a rate
  denial still names only the connector's index
- Rejection of duplicate cache namespace and search connector keys in schema-6
  configuration, during phase-one validation
- Host-only cache seeding so a read-only namespace can hold reference data while
  guest write authority stays refused
- Exact global least-recently-used eviction under whole-cache pressure, instead
  of penalising the namespace being written
- Monotonic cache expiry that never reads a wall clock, so a clock jump cannot
  extend a time to live and a clock fault cannot reach guest code
- Numerically stable vector similarity that cannot overflow for finite input and
  never encodes a non-finite score
- Guided-authoring redaction of cache keys, cached values, search queries, and
  encoded vectors, propagating conservatively through nested expressions and
  immutable `let` aliases, in both parsed and malformed source
- Pre-execution rate-resource counting that includes host-owned search connector
  origins, so LRU replacement cannot reset a live rate counter
- Approval denials that name a connector's index rather than its endpoint, on
  the first attempt and on every retry
- Endpoint-neutral outbound HTTP failure messages: the driver's error text,
  which can carry a host, port, resolved address, proxy, URL, or certificate
  detail, is replaced by a fixed category while retry classification is kept
- Tolerant guided-authoring redaction that follows `let` alias chains and closes
  a binding at a statement-terminating newline as well as a semicolon
- Lexical binding scopes in the tolerant redaction fallback, restoring shadowed
  outer bindings after a block and installing parameters as untraceable shadows
- Whole-value redaction when a tolerant binding exceeds its exact-tracking
  bound, so an oversized value is over-redacted rather than partly exposed
- Draft agent application and guided AI authoring specifications plus the
  WebAssembly sandbox design now backed by an artifact-policy baseline
- Narrow reference-agent MVP and explicit deferred scope
- Phased agent service roadmap from authority planning through durable state,
  queues, databases, and optional caches
- Strict configuration, secret, and outbound HTTP capability declarations
- Human and JSON `krit permissions` output for pre-deployment authority review
- Versioned provider-neutral Krit 0.2 generation prompt and usage contract
- Readable records with deterministic rendering and field access
- Built-in `Option` and `Result` values with exhaustive variant matching
- Parsed type annotations for bindings, parameters, returns, lists, records,
  options, and results
- Deterministic `json_encode` and dynamic `json_decode` built-ins
- Stable `K4008` and `K4009` JSON diagnostics
- Lexical name resolution with duplicate declaration checks
- Deterministic static inference and checking for primitive, list, record,
  Option, Result, and function types
- Defensive exhaustive-family checking and stable `K3001` through `K3006`
  diagnostics
- Sorted `io.stdout` effect inference with recursive and higher-order call
  propagation
- Public `krit::analyze` API with inferred binding types, source spans, and
  effects
- Implementation-neutral static `check` conformance fixtures
- Public `krit::format_source` API with deterministic edition-2026 formatting
- Lossless standalone and end-of-line `//` comment preservation
- `krit fmt [--check] FILE...` with batch validation, atomic same-directory
  replacement, and stable `K8001` check diagnostics
- Formatter fixtures and repository-wide parse, analysis, and idempotence
  coverage
- Resolved typed Core IR with deterministic binding, value, function,
  parameter, capture, closure, block, and match-binding IDs
- Public normalized symbol, expression, block, and resolved-name analysis facts
  used directly by Core lowering
- Explicit left-to-right ANF evaluation order, nested control-flow blocks,
  recursive self values, and lexical closure capture arguments
- Explicit Core identities for stdout built-ins, pure Option/Result
  constructors, and pure JSON conversion
- Synthetic typed `module-init` entrypoint with inferred effects
- Core verification for ID ranges, dominance, branch and call types, captures,
  operation types, and conservative effects
- Stable `CoreModule::render_text` output with golden and repository-corpus
  lowering coverage
- `krit explain [--json] FILE` with human compiler facts and deterministic
  schema-1 JSON serialized through `serde_json`
- Dedicated `krit-wasm` artifact crate consuming verified Core without
  reparsing or inference
- Checked-in `krit:runtime@0.2.0` WIT package with effect-selected
  `pure-program` and stdout `program` worlds
- Deterministic core Wasm and Component Model emission for Int, Bool, Unit,
  checked operators, control flow, recursion, and non-capturing higher-order
  calls
- Fail-closed K7001/K7002 backend diagnostics for residual types, composites,
  JSON, lexical captures, unsupported output, and unlowered operations
- Explicit core/component feature and import validation with no WASI, start,
  memory, threads, GC, exceptions, or component async, plus effects and world
  derived from the validated import surface rather than metadata claims
- Bounded embedded producers/custom metadata plus deterministic adjacent
  schema-1 metadata and exact final-byte BLAKE3 digests
- `krit build [--manifest PATH] [--output PATH]` with safe package entry
  resolution, capability checking, deterministic defaults, and rollback-safe
  output replacement
- Public artifact inspection, policy validation, and digest verification APIs
- `krit-runtime`, a reusable-engine Wasmtime component host with a fresh Store
  and instance per invocation, exact pure/stdout WIT linking, no WASI, bounded
  precompile inputs, StoreLimits, stack, fuel, serialized epoch deadlines,
  host-call limits, and rollback-safe buffered output
- `krit sandbox [--manifest PATH] [--artifact PATH]` with no automatic build or
  evaluator fallback and stable authorization/runtime exit statuses
- Artifact-aware human and JSON `krit permissions --artifact PATH` reports
  with complete denied output and deployment status kept `not-evaluated`
- Stable host diagnostics `K5002` and `K5101` through `K5105`
- Genuine Wasm integer-overflow traps for compiler checked arithmetic so an
  adversarial `unreachable` remains generic `K4001`
- Explicit top-level `webhook fn` declarations with stable duplicate,
  placement, and exact-signature diagnostics
- Fixed `HttpHeader`, `HttpRequest`, and `HttpResponse` built-in contract
  aliases with exact closed response checking and ordered duplicate headers
- Deterministic draft-2020-12 webhook request/response JSON Schemas in
  versioned human and schema-1 JSON explanation facts
- `config_string` and `secret` host-operation identities with direct
  string-literal resource enforcement, sorted `config.read`/`secret.read`
  effects, and separate resource-specific capability requirements
- Opaque `Secret` analysis/Core type and static rejection of printing,
  comparison, JSON encoding, and ordinary structural storage
- Stable `K1004`, `K3007` through `K3009`, and `K5003` diagnostics
- Checked-in HTTP, config, WIT-resource secret, and typed webhook contracts
- Conformance, formatter, Core golden, explain-schema, WIT parseability,
  direct-run denial, build fail-closed, and unchanged policy-1 artifact tests
- Direct normalized-origin `http_request(origin, request, bearer)` with exact
  `http.request` requirements and bearer-only opaque `Secret` consumption
- Shared URL-based normalized origin parsing for source and manifest checks
- Finite effect-selected webhook worlds, including a separate anonymous HTTP
  surface that does not implicitly import secret acquisition
- Bounded policy-2 canonical ABI lowering for strings, fixed HTTP records,
  header lists, selected Result/Option layouts and matching, and static helper
  references while preserving policy-1 scalar artifact bytes
- Schema-1 embedded and adjacent exact resource requirements revalidated
  against component-derived effects/imports
- Typed `krit-runtime` webhook invocation with fresh Stores, instances,
  resource tables, handles, and rollback-safe output/response publication
- Explicit immutable config values and host-owned zeroizing secret storage
- Exact-origin ordered-header outbound HTTP/TLS via statically linked,
  rustls-backed libcurl with native platform trust roots,
  environment proxies disabled, redirects denied, DNS results pinned per
  request, public-address policy, independent timeouts, and body/header/call
  limits
- Strict host config JSON with relative secret-file references, bounded reads,
  no inline/environment values, owner-only Unix permission enforcement, and
  no-follow descriptor opens
- `krit invoke --request FILE` and loopback `krit serve [--once]` over existing
  artifacts only, using `tiny_http` rather than a handwritten parser
- Auditable `examples/webhook-agent.krit` plus manifest, host config, and
  request fixtures containing no credential
- Provider-neutral fallible `ai_invoke("adapter", input)` source/Core
  contract with exact `ai.invoke` requirements and typed Component Model AI
  imports
- Manifest schema-1 `ai` and `logs` requests plus exact least-authority
  AI-only, log-only, and combined finite webhook worlds
- Deterministic host-side `http-json` AI adapter with strict request/response
  mapping, bounded input/output/timeout, optional opaque bearer secret, and no
  provider SDK dependency
- Typed ordered `LogField` values and fallible `log_info`/`log_error`
  operations with invocation-local sequencing, atomic validation, bounded
  buffering, key-based and exact-secret-value redaction, and separate result
  events
- Success/failure structured-log JSON Lines publication on CLI stderr without
  changing invoke response stdout or served HTTP bodies
- Reusable stateful `AgentHost`, embedding `CancellationHandle`, and
  `ApprovalPolicy` APIs
- Bounded transport retries for connection/timeouts and 429/502/503/504,
  restricted to GET/HEAD or explicit valid idempotency keys, with capped
  deterministic backoff and Retry-After handling
- Finite per-resource fixed-window AI/HTTP rates with bounded LRU tracking and
  visible guest errors
- Entry- and byte-bounded process-local TTL/LRU inbound idempotency replay,
  exact request digest conflicts, response replay without guest execution,
  and exclusion of failed/trapped invocations
- Default-deny AI and bearer-HTTP approval checks before secrets/network and
  before every retry, with approval-required artifact permission facts
- Strict host config schema 2 for adapters, approvals, retries, rates, and
  idempotency; schema 1 remains readable but bearer operations now require
  migration to explicit schema-2 approval
- Stable pre-execution cancellation diagnostic `K5106`, libcurl progress
  cancellation/deadline aborts, bounded DNS workers, and cancellation-aware
  backoff
- Bounded policy-2 unescaped JSON-string decoding for explicit reference
  model-output validation; all general JSON component shapes remain
  fail-closed
- Local-only reference integration coverage proving GitHub-like -> neutral AI
  -> messaging-like order, retry/auth boundaries, output/log/stats facts, and
  exact permissions without Internet access or real credentials
- `krit-lsp`, a synchronous stdio language server with full-document sync,
  UTF-16 position handling, stable compiler diagnostics, canonical formatting,
  a deterministic format code action, hover, completion, and document symbols
- Schema-1 `krit/compilerFacts` responses for module/entrypoint effects,
  literal-resource requirements, resolved symbols and expressions, inferred
  and declared types, package metadata, requested/required permission status,
  reference status, and canonical edits
- Package-aware configuration, secret, exact-origin HTTP, and AI adapter
  completion plus compiler-owned built-in signatures and built-in record field
  facts
- Bounded 16 MiB protocol frames, 1 MiB documents, 128-document state,
  256 KiB validated manifests, recursive type rendering, and bounded
  completion/symbol/compiler-fact output with fail-closed malformed handling
- In-memory protocol and real `krit lsp` stdio coverage for malformed
  requests/documents, UTF-16 ranges, deterministic facts, formatting
  idempotence, no-execution behavior, framing purity, and graceful shutdown
- `krit-assist`, an isolated authoring layer consuming the public bounded
  language-server compiler-facts API with no dependency leakage into compiler,
  package, Wasm, runtime, or LSP layers
- Strict provider-neutral authoring protocol 1 request, response, proposal,
  review, and acceptance schemas for one digest-preconditioned package entry
- Disabled-by-default explicit provider configuration and one generic bounded
  HTTPS/loopback HTTP JSON adapter with host-managed optional environment
  credentials, disabled inherited proxies/redirects, and no branded SDK
- `krit assist inspect|suggest|review|accept` with exact pre-provider context
  inspection, JSON Lines output, untrusted proposal artifacts, explicit
  `--reviewed`, and exact added-permission approval
- Canonical package containment, `.kritignore`, generated/non-Krit exclusion,
  descriptor-relative no-follow source reads, bounded UTF-16 source ranges,
  source/type/diagnostic resource redaction, host-local real digests, and
  strict stale/overlap/path/symlink/size validation
- Deterministic canonical unified diffs and before/after diagnostics, types,
  effects, requested/required/granted permissions, usage, missing grants, and
  authority deltas
- Canonical format/parse/analyze/Core validation plus permission-preserving
  atomic exchange single-source acceptance with pre-write staged permissions,
  displaced-source digest validation, stale rollback, and fail-closed
  acceptance on platforms without the audited exchange primitive
- Fake provider, loopback HTTP provider, malicious response/context, prompt
  injection, redaction, stale/atomic failure, permission escalation, semantic
  cleanup, AI-off, and reference webhook guided-edit coverage
- Normative durable-state/replay specification with explicit local crash,
  transaction, checkpoint, migration, filesystem, retention, and exactly-once
  limitations
- `krit-state`, a bundled-SQLite schema-1 transactional store using WAL,
  configurable FULL/NORMAL synchronization, application/schema identity,
  integrity validation, bounded page count, busy timeout, revision conflicts,
  replay leases/results, and durable idempotency records
- Edition-2026 `state_get`, `state_put`, `state_delete`, `checkpoint_get`,
  `checkpoint_put`, `replay_http`, and `replay_ai` built-ins with direct
  canonical store/checkpoint/operation/resource facts
- Manifest `state` grants, `state.transaction` analysis/Core/metadata facts,
  policy-2 state artifact validation, typed `krit:runtime/state@0.2.0` WIT,
  finite exact state world selection, and bounded Wasm lowering
- Invocation-local state/checkpoint overlays with revision-checked commit only
  after successful guest completion and rollback on traps, cancellation,
  deadlines, invalid responses, conflicts, and host failures
- Durable completed HTTP/AI replay with exact artifact/operation/input identity,
  current grant and approval rechecks, stable AI idempotency keys, safe/keyed
  HTTP enforcement, leases, expiry, LRU, and byte bounds
- Optional durable inbound `Idempotency-Key` reservations/responses scoped by
  artifact identity and credential-sensitive request digests, while schema
  1/2 and unconfigured schema 3 preserve process-local behavior
- Strict schema-3 host config with manifest-narrowed named stores, no default
  path, bounded durability/SQLite/replay policy, owner-only directories/files,
  symlink denial, and corrupt/newer/foreign database rejection
- Stateful checkpoint example plus compiler, WIT, artifact, runtime,
  restart, killed-writer recovery, cancellation, replay, idempotency,
  filesystem, CLI, and legacy factorial-byte regression coverage
- Normative durable queue, scheduled-trigger, and object-storage specification
  with lifecycle state machines, ordering, lease, retry, dead-letter, catch-up,
  atomicity, crash-window, filesystem, limit, and non-goal contracts
- Store schema 2 with strict `queue_jobs`, `queue_dead`, `schedule_fires`,
  `schedule_cursors`, and `objects` tables, indexed reservation and cleanup
  queries, and a deterministic in-place schema-1 migration that preserves data
  and still rejects foreign, newer, extra, or malformed schemas
- Contextual `queue "name" fn` and `schedule "name" fn` entrypoints with the
  fixed `QueueJob` and `ScheduleEvent` contracts and `Result<String, String>`
  outcomes, plus `queue_publish`, `object_get`, `object_put`, and
  `object_delete` built-ins with direct canonical resource facts
- Separate `queue.publish`, `queue.consume`, `schedule.trigger`, `object.read`,
  and `object.write` effects across parser, analyzer, Core, manifest
  (`queues`, `consumes`, `schedules`, `buckets`, `readOnlyBuckets`),
  permissions, explain, LSP completion, and assist redaction
- Typed `krit:runtime/queue`, `objects-read`, `objects-write`, `job`, and
  `schedule` WIT interfaces, deterministic least-authority world generation for
  every import mask, and validation that re-derives every effect from the real
  component imports and export
- Durable queue publish, reservation, acknowledgement, retry with capped
  backoff, attempt caps, dead letters, and owner leases whose expiry recovers
  interrupted work without holding a database lock across guest execution
- Host-owned scheduled triggers with UTC epoch occurrences, durable
  `(schedule, due)` fire identities, bounded catch-up with explicit misfire
  skipping, and shared retry/dead-letter handling
- Capability-scoped object buckets with bounded object count, key, value, and
  total bytes, replacement accounting, and deterministic host-side listing
- One-transaction outcome commits that bind staged state, checkpoints, object
  writes, queue publishes, and the delivery acknowledgement together
- `krit worker --queue NAME` and `krit schedule --schedule NAME` with `--once`,
  bounded `--max-deliveries`, explicit `--now` wall time, and schema-1 JSON
  delivery reports
- Strict host config schema 4 that binds manifest-granted queues, schedules,
  and buckets to already-configured owner-only stores and can only narrow them
- Enqueue, worker, and scheduled-trigger examples plus store, compiler,
  artifact, runtime, and real-process CLI coverage for FIFO delivery,
  concurrent reservation, lease recovery, retry-then-success, dead-letter
  exhaustion, schedule fire recovery, object bounds, schema migration, and
  non-repeated completed external effects

### Fixed

- Simplified the database import/effect consistency condition without changing
  behavior, fixing the minimum-toolchain Clippy failure in GitHub Actions
- Removed platform-dependent port-order assumptions from reference-agent
  approval expectations and made the backoff cancellation fixture use a
  retry-eligible request
- Enforced LF checkouts and generated release metadata on every platform so
  Windows archives preserve canonical source formatting
- Reserve queue jobs and schedule fires only after acquiring the Runtime's
  execution scheduler, retain ownership through acknowledgement, and recheck
  cancellation after scheduler and schedule-materialization waits
- Advance delivery lease/outcome timestamps from host time using monotonic
  elapsed time, preserve the exact schedule tick cutoff, and refuse guest work
  when a reservation no longer covers its execution deadline
- Release delivery leases after failed outcome commits or malformed job bodies
  instead of leaving work unavailable until lease expiry
- Reject HTTP/AI replay inside an application-database transaction before
  approval or durable access; release replay leases after provider failures,
  cancellation, approval denial, host traps, and completion errors
- Preserve the original failure when durable reservation cleanup also fails
- Revalidate cached inbound responses and durable AI output against current
  runtime/adapter limits, including inclusive byte and header boundaries
- Enforce durable-state and job/storage protocol maxima in public lower-layer
  APIs, with the CLI sharing the same validation
- Reject duplicate keys across every host-configuration map and supported
  schema before resource initialization
- Apply direct-run rejection and editor/compiler facts consistently to webhook,
  queue, and schedule deployment entrypoints
- Distinguish implemented phases from planned package/build infrastructure and
  still-unmeasured performance and product-validation goals in the documentation

### Changed

- Replaced the active Racket implementation with a Rust-only bootstrap
- Replaced prototype S-expressions with Krit's edition-2026 readable syntax
- Made specifications and conformance cases the semantic authority
- Selected WebAssembly components instead of custom bytecode as the first
  deployment target
- Defined LLM assistance as optional visible edits gated by deterministic
  formatting, checks, and permission analysis
- Updated the provider-neutral generation prompt to version 0.2.2 for readable
  data and static checking, to version 0.2.3 for canonical formatting, and to
  version 0.2.4 for the checked explanation workflow
- Made `krit check` perform semantic analysis without executing source while
  preserving its success output, then lower and verify typed Core IR
- Marked the readable-agent-data phase complete
- Completed Phase 3 for the policy-1 scalar/stdout subset: Core IR, component
  artifacts, bounded sandbox hosting, differential execution, and effective
  local permission inspection. Full-language layouts and agent interfaces
  remain later work.
- Updated the provider-neutral generation prompt to version 0.2.5 for the
  checked artifact-build workflow and strict backend subset
- Updated the provider-neutral generation workflow to version 0.2.6 with
  explicit sandbox execution and artifact permission-review commands
- Changed the Wasmtime requirement from an exact 47.0.4 pin to compatible
  47.0.4 while retaining the tested patch in `Cargo.lock`; documented the
  short non-LTS support window and planned audited migration to 48 LTS
- Made direct `krit run` fail with `K5003` when source requires an unavailable
  webhook/config/secret host, without changing existing dynamic conformance
- Marked `phase4-http-runtime` complete while keeping Phase 4 in progress for
  AI, observability, and reliability work
- Updated the provider-neutral generation prompt to version 0.2.7 for typed
  webhook, configuration, opaque-secret, and fail-closed runtime contracts
- Updated the provider-neutral generation prompt to version 0.2.8 for the
  buildable bounded webhook, explicit host-input, invoke, and serve workflow
- Completed `phase4-ai-observability` and the Phase 4 stateless reference-agent
  gate; Phase 5 LSP/guided authoring and Phase 6 durable state remain
  unstarted
- Updated the provider-neutral generation prompt to version 0.2.9 for neutral
  AI calls, structured logs, explicit model-output validation, and bounded
  reliability/approval policy
- Completed `phase5-language-server` while keeping provider-neutral prediction,
  accepted-suggestion checking, and semantic cleanup in the later
  `phase5-guided-assistance` milestone
- Updated the provider-neutral generation prompt to version 0.2.10 for the
  offline language-server compiler-facts and editor workflow
- Completed `phase5-guided-assistance` and the Phase 5 gate without adding an
  LLM dependency to source checking, builds, runtime execution, or deployment
- Updated the provider-neutral generation prompt to version 0.2.11 for strict
  authoring protocol 1, review-gated edits, and permission-delta approval
- Completed `phase6-state` for durable local single-host coordination while
  retaining honest provider crash windows and no distributed exactly-once
  claim
- Updated the provider-neutral generation prompt to version 0.2.12 for durable
  state, checkpoints, replay, and schema-3 host configuration
- Completed `phase6-jobs-storage` and the Phase 6 durable-execution gate:
  interrupted worker deliveries resume safely without losing committed state or
  repeating completed external side effects, while distributed queues, brokers,
  cron expressions, and provider-side exactly once remain unclaimed
- Added `K5205` for invalid durable delivery leases, acknowledgements, and
  outcome details
- Updated the provider-neutral generation prompt to version 0.2.13 for durable
  queues, scheduled triggers, bounded object storage, and schema-4 host
  configuration
- Completed `phase7-database` for the local SQLite connector while explicitly
  refusing to claim atomicity between an application database and Krit state
- Updated the provider-neutral generation prompt to version 0.2.14 for
  catalogued parameterized database transactions and schema-5 host
  configuration
- Committed terminal queue and schedule transitions when a reservation reaches
  its scan bound, so a depth-one or single-attempt resource dead-letters and
  stays usable instead of wedging
- Required every configured queue and schedule lease to cover the runtime
  execution deadline plus the backing store's busy timeout
- Validated the complete schema-1 object set inside the exclusive migration
  transaction before any DDL and revalidated the finished schema before commit,
  so a rejected migration leaves the database byte-for-byte unchanged
- Installed the database page ceiling before initialization or migration,
  re-verified the materialized page count against the byte budget, and raised
  the minimum configurable database budget to a truthful 1 MiB
- Validated every schema-4 job definition, grant, limit, and store reference
  before any database is created, opened, or migrated
- Decoupled queue publication from the state revision so independent publishers
  never conflict, while combined state outcomes still advance it exactly once
- Charged staged queue depth per queue so an atomic fan-out to several queues
  commits
- Rejected unrepresentable schedule and queue instants before any cursor, fire,
  or job row moves
- Replaced case-insensitive `LIKE` object-prefix matching with exact
  case-sensitive binary matching in which `%` and `_` are ordinary characters
- Kept `krit worker --json` and `krit schedule --json` standard output a single
  machine-readable report by moving bounded guest output into an explicit
  `outputs` array
- Normative capability-scoped database specification with authority, typed API,
  transaction lifecycle, isolation, statement catalog, encoding, atomicity,
  filesystem, privacy, limit, and non-goal contracts
- `krit-database`, an isolated bundled-SQLite connector that shares no schema or
  migration logic with the durable-state store and adds no dependency
- Opaque non-serializable `DatabaseTransaction` handles enforced like `Secret`,
  with `K3010` for any revealing, structural, or out-of-position use
- Edition-2026 `db_begin_read`, `db_begin_write`, `db_query`, `db_execute`,
  `db_commit`, and `db_rollback` built-ins with direct canonical database and
  statement literals and bounded `List<String>` parameters
- Bounded `List<String>` values in the webhook ABI so database parameters are a
  typed list rather than encoded text
- Host-owned statement catalog validated against the live schema: exactly one
  statement, ordinal placeholders, declared parameter types and result columns,
  read-only queries versus mutating executes, and rejected `PRAGMA`, `ATTACH`,
  `DETACH`, `VACUUM`, `ANALYZE`, `REINDEX`, `EXPLAIN`, transaction-control, and
  schema statements
- Deterministic bounded JSON result encoding restricted to INTEGER, TEXT, and
  NULL, with REAL, BLOB, and non-UTF-8 values failing closed
- Separate `database.read` and `database.write` manifest grants (`databases`,
  `readOnlyDatabases`), effects, Core requirements, permissions, explain facts,
  LSP completion, and assist redaction
- Typed `krit:runtime/database@0.2.0` interface, least-authority world masks,
  and artifact validation that rejects disagreeing database imports and effects
- Runtime transaction safety: one open transaction per invocation, refused
  external HTTP and AI effects while open, bounded transaction wall time below
  the invocation deadline, completed-handle reuse rejection, and fail-closed
  rollback when an invocation ends with a transaction open
- Strict host config schema 5 that validates every database definition, grant,
  limit, and statement before opening any file, requires an existing owner-only
  database, and never creates, migrates, or resets an application schema
- Reference database webhook example with an operator-owned schema script, and
  store, compiler, artifact, runtime, and real-process CLI coverage including
  injection-as-data, handle misuse, catalog rejection, limit, concurrency, and
  restart cases
- Auditable database facts in `explain` and the LSP durable-fact stream, with an
  optional `store` so a handle-taking operation reports its statement name
  instead of a database name it cannot prove
- Guaranteed database transaction cleanup on every invocation exit path, a
  `Drop` fail-safe, connection poisoning when cleanup cannot restore a known
  state, and a `databaseTransactionsAbandoned` statistic
- SQLite progress-handler interruption bounded by the transaction deadline, the
  invocation deadline, and embedding cancellation, with busy waiting clamped to
  the remaining time
- Incremental result encoding that checks each JSON escape against the byte
  budget before copying, keeping host memory bounded at the hard maxima
- Comment- and quote-aware SQL statement classification, so a leading comment
  can no longer disguise a forbidden `PRAGMA`, `ATTACH`, `DROP`, or transaction
  control statement
- Refusal of write-ahead-logging application databases, a bounded rollback
  journal, and a disk budget that accounts for `-journal`, `-wal`, and `-shm`
- Strictly ordered host config loading: all validation, including host policy,
  runs before any durable store or application database is created or opened
- Rejection of any durable state store and application database that resolve to
  the same file by canonical path, or by device and inode on Unix

### Removed

- Racket runtime, package metadata, tests, installation instructions, and CI

## [0.1.0] - 2026-08-31

### Added

- Racket-based educational interpreter, CLI, tests, examples, and
  documentation

The immutable historical baseline is tagged `racket-v0.1.0`.
