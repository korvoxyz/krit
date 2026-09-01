# Krit Rust technical design

**Status:** Accepted; policy-1 artifact backend and bounded host implemented
**Owner:** Akshay Bhardwaj

## Decision

Krit's active implementation is Rust-only. The previous Racket implementation
is archived at Git tag `racket-v0.1.0` and is not a runtime dependency,
semantic authority, CI job, or contributor requirement.

The specification and conformance suite define the language.

## System shape

```text
UTF-8 source
    |
    v
source database -> lexer -> parser -> AST
                                |
                                v
                     name and module resolution
                                |
                                v
                       type + effect checking
                                |
                                v
                          typed HIR
                                |
                                v
                  normalized Core IR (ANF)
                       /                \
                      v                  v
             Wasm component         explanation data
                      |
                      v
            validator + component host
                      |
                      v
             capability interfaces
```

The Rust bootstrap implements source, lexer, parser, AST, diagnostics, name
resolution, type/effect analysis, a verified typed Core IR, a direct evaluator
that establishes runtime semantics, and `krit-wasm`, a strict Core-to-component
artifact backend, plus `krit-runtime`, a bounded component host. `krit build`
emits validated artifacts, `krit sandbox` executes only those artifacts, and
`krit run` remains the broader direct evaluator.

## Workspace

```text
crates/
  krit-source/       source files, spans, line maps
  krit-syntax/       tokens, lexer, AST, parser
  krit-diagnostics/  codes, rendering, JSON schema
  krit-semantics/    names, types, effects, HIR
  krit-ir/           normalized Core IR
  krit-wasm/         Core IR to WebAssembly component lowering
  krit-runtime/      component runtime, limits, and capability handles
  krit-package/      manifests, lockfiles, resolver, store
  krit-cli/          user command and orchestration
  krit-lsp/          deterministic compiler facts and editor operations
  krit-assist/       optional provider-neutral LLM edit suggestions
  krit-conformance/  implementation-neutral suite runner
```

The bootstrap may combine crates when an interface has not stabilized.
Splitting requires a measurable build, ownership, or dependency benefit.

Dependency direction is one-way down the list of compiler stages. The runtime
does not depend on parser internals. Package code orchestrates compiler inputs
but does not own language semantics.

## Rust baseline

- Stable Rust only
- Minimum supported Rust version recorded in `Cargo.toml`
- Rust 2024 edition
- `unsafe` forbidden by default
- Dependencies minimized and locked
- Release binaries use link-time optimization and stripped symbols
- Reproducible source archives include `Cargo.lock`

The initial minimum supported version is Rust 1.94 because that toolchain is
available in the development baseline. CI should also test current stable
Rust. Raising the minimum requires a changelog entry.

## Source and syntax

Source files are immutable byte buffers validated as UTF-8. Spans are
half-open byte ranges associated with a source identifier. Line/column
conversion is lazy so lexing and parsing do not repeatedly scan text.

Parser tokens retain spans and normalized literal values. The lexer also
exposes formatter-only line-comment trivia with exact comment text, source
order, and standalone/end-of-line placement. Identifier interning begins only
when profiling shows a memory or comparison benefit.

The parser is handwritten recursive descent with Pratt/precedence parsing for
expressions. Error recovery synchronizes at `;`, `}`, and declaration
keywords. The first implementation may stop after one error, but its
diagnostic code and primary span must match the specification.

The canonical formatter validates with the normal parser, then renders the
parsed token stream with AST-derived block, type, and statement boundaries.
It retains grouping parentheses and comments rather than reconstructing them
from the lossy semantic AST. It only inserts or removes grammar-permitted
trailing commas. Delimiter groups provide deterministic wrapping with a soft
100-column target. This hybrid keeps formatting semantic-safe while avoiding
a second language parser.

## Semantic stages

### Name resolution

The analyzer resolves lexical names over the spanned AST, reports duplicate
declarations, and assigns deterministic symbol IDs to lets, named functions,
parameters, and match bindings. Typed expression facts record either a symbol
ID or an explicit built-in identity; executable Core references never perform
source-name lookup. Original names survive only as optional debug metadata.

### Types and effects

The checker emits public analysis facts from the AST: inferred binding,
symbol, expression, and block types; resolved name facts; source spans; and
sorted effects. Core lowering consumes these normalized facts instead of
re-running inference. Types retain shared `Arc` substructure so repeated Core
boundaries do not expand the inference DAG. A normalized inference type can
still contain a constrained parametric variable; name resolution does not
imply a concrete backend layout. Public signature cache boundaries remain
future module work.

### Core IR

The bootstrap uses one resolved, typed Core IR rather than a second HIR whose
boundary would duplicate the small current analyzer. Core IR is
expression-oriented and in administrative normal form:

- every executable operand is a `ValueId`, never a source variable name
- bindings, functions, values, parameters, captures, closures, blocks, and
  match inputs have deterministic typed IDs
- calls evaluate the callee and arguments left to right; lists and records
  evaluate elements and fields left to right
- short-circuit operators, blocks, conditionals, list matches, and
  Option/Result matches contain explicit nested blocks
- recursive functions have an explicit self value and lexical closures have
  explicit typed capture parameters and closure arguments
- built-ins identify stdout host effects, pure variant constructors, and pure
  JSON conversions separately
- every operation, block, function boundary, and module-init entrypoint exposes
  a normalized inferred type and sorted conservative effects
- source spans survive lowering without becoming executable names

Standalone source lowers to the stable synthetic `module-init` entrypoint.
Future package exports can add entrypoint kinds without inventing agent APIs.

Built-in identity defines intrinsic behavior. Constructing any built-in
function value is pure; stdout built-ins require `io.stdout` when called, while
constructors and JSON conversions have no intrinsic effect. Bidirectional
inference may conservatively add effects to a built-in's function type, and a
later Core call carries that inferred superset without making built-in value
creation effectful.

The Core verifier rejects duplicate or out-of-range IDs, unavailable uses,
leaked branch values, inconsistent branch and match results, invalid call
signatures or arity, mismatched operation types, malformed captures, and
understated effect summaries. `krit check` and `krit explain` lower and verify
successfully analyzed source; a failure after valid analysis is an internal
compiler error.

Residual `Type::Variable` values at Core boundaries are intentional
parametric types, not malformed Core and not yet Wasm-ready layouts. The
verifier accepts them where an operation records the relevant constraint, such
as addition or equality. Before layout selection or emission, the Wasm
artifact stage must monomorphize/specialize them or issue a stable diagnostic
at source level. Open structural record requirements similarly state required
fields without selecting a closed physical record layout.

`CoreModule::render_text` is deterministic and is covered by checked golden
files. `krit explain [--json] FILE` exposes module-init effects, top-level
types, and the same Core facts. Its JSON schema is versioned independently and
serialized through `serde_json`.

Optimization never changes overflow, effect order, capability checks,
diagnostic category, or deterministic rendering.

Initial optimizations:

- constant folding with checked arithmetic
- dead pure binding elimination
- unreachable branch elimination
- precise closure capture
- tail-call marking

## WebAssembly component target

The implemented policy-1 artifact is a small WebAssembly component that
selects one of two checked-in `krit:runtime@0.2.0` worlds from checked effects.
`krit:runtime/pure-program@0.2.0` exports `run: func()` with no imports.
`krit:runtime/program@0.2.0` exports the same function and imports the typed
`krit:runtime/stdout@0.2.0` interface. Unused manifest grants do not widen the
selected world. HTTP, webhook, schedule, queue, and agent-tool entry points
remain future worlds.

Krit does not grant a general WASI environment. Files, sockets, processes,
environment variables, clocks, randomness, secrets, state, and AI calls are
host resources available only through explicit component imports and
unforgeable handles.

The backend uses `i64` for `Int`, `i32` for `Bool`, zero-width `Unit`, and
bounded-table `i32` slots for non-capturing functions. It supports recursive
and higher-order calls, Core blocks/conditionals, checked integer operations,
primitive comparisons, and scalar stdout. It rejects residual types, lexical
captures, strings, composites, variants, matching, JSON conversion, and
unknown built-ins before emission.

An adjacent metadata document records:

- WebAssembly Component Model version
- compiler build identifier
- language edition
- target world and sorted imports/effects
- package-relative source entry and package identity
- build profile and validation-policy version
- exact final-byte BLAKE3 checksum and byte size

Core and component bytes are validated with an explicit fail-closed feature
and import policy. WIT canonical ABI names and signatures are derived from the
selected world in the parsed checked-in package. Validation derives policy
effects and world selection from the exact component and core import surfaces,
then requires embedded and adjacent metadata to match. Components contain
bounded standard/custom metadata without source text or machine paths.

The reference host uses Wasmtime with only the component-model, Cranelift,
runtime, and std features. One Engine is reusable, while every invocation gets
a fresh Store, StoreLimits, host state, component instance, fuel budget, epoch
deadline, and output buffer. The linker provides either no imports or exactly
the checked stdout WIT interface; it never adds WASI.

Validation, digest checking, authorization, input-size checks, and static
component resource inspection precede Wasmtime compilation. Component
compilation occurs outside the one-second default execution deadline. Epoch
scheduling is serialized per Runtime because increments affect all Stores
using an Engine; each cancellable deadline worker is joined before returning.
Output remains buffered until success, so failed invocations publish no
partial stdout.

Wasmtime 47 is a short-supported non-LTS line. `Cargo.toml` accepts security
patches compatible with 47.0.4, and `Cargo.lock` records the exact tested
patch. The project plans an audited move to Wasmtime 48 LTS before 47 support
ends on 2026-09-20. Runtime-major and MSRV changes require explicit CI,
documentation, and changelog updates. Untrusted multi-tenant execution still
adds a restricted OS process or container. `spec/WASM-SANDBOX.md` defines the
exact limits and security contract.

A custom bytecode VM and native backend are deferred. They add a second
security and artifact surface without helping prove the initial agent product.

## Runtime values and memory

The bootstrap evaluator uses safe Rust enums and reference-counted immutable
objects. The component backend uses explicit canonical interface types for
records, variants, strings, lists, options, and results. Host pointers and
credentials never become guest values.

The component's internal value and memory layout is selected through
measurements. No custom unsafe representation will be introduced without:

- a safe reference implementation
- Miri and sanitizer coverage
- fuzzed component and canonical-ABI validation
- representative memory and speed measurements
- a documented invariant review

Lists should use persistent vector/list structures chosen from workload data,
not assumed linked lists.

## Capabilities

Core evaluation cannot call operating-system APIs directly. Effect
instructions call a `Host` interface containing opaque capability handles.
The current policy-1 CLI constructs only the exact stdout grant set from the
local manifest. Future deployment hosts will additionally intersect user or
deployment policy and OS sandbox support; artifact permission reports label
that layer `not-evaluated` today.

The runtime never inherits the entire process environment. Capability data is
not serializable into components or cache artifacts.

## Package and build system

The package resolver produces a deterministic graph before compiling source.
Source fetching, checksum verification, resolution, compilation, and
execution are separate phases with separate errors.

Each module compiles against imported public-interface hashes. Private changes
do not invalidate downstream modules when the public interface is unchanged.

Build writes use temporary files followed by atomic rename. Locks coordinate
concurrent builds, and stale locks are recoverable without deleting valid
artifacts.

## CLI

One `krit` binary owns compiler, runtime, formatter, package, and explanation
commands. Subcommands share source loading, diagnostics, package discovery,
and cache configuration.

Human output goes to standard output; diagnostics and progress go to standard
error. `--json` or command-specific JSON options emit stable schemas with no
decorative text.

`krit sandbox [--manifest PATH] [--artifact PATH]` loads only an existing
component and adjacent schema-1 metadata, authorizes it against the manifest,
and writes buffered guest output after success. It never builds or invokes
the direct evaluator. `krit permissions --artifact PATH [--json] [MANIFEST]`
uses the same bounded loader and runtime validation, prints the complete
effective report even when denied, and exits 4 for local authorization denial.

`krit fmt [--check] FILE...` validates and formats files in argument order.
Normal mode reads and formats the complete batch before creating
same-directory staged files, preserves file permissions, and atomically
renames each staged result. A read or parse failure therefore leaves every
requested source untouched. Check mode never writes and reports `K8001` for
each non-canonical source.

Editor integration is separate from compilation. `krit-lsp` exposes
deterministic syntax, type, effect, formatting, and permission facts.
`krit-assist` may ask a configured model for small structured edits, but those
edits remain provisional until accepted and rechecked. Builds and deployments
never require an LLM.

## Testing

Required layers:

- lexer/parser unit tests
- semantic and runtime unit tests
- implementation-neutral conformance cases
- CLI integration tests
- package resolver/store tests with local fixtures
- malformed source and WebAssembly component fuzzing
- deterministic output tests
- formatter fixtures, comment preservation, idempotence, parse/analyze
  round-trips, and atomic batch CLI tests
- cache clean/hit equivalence tests
- capability denial tests
- differential direct-evaluator/component tests during backend development
- capability import, resource exhaustion, and instance-reset tests
- guided-authoring privacy and permission-bypass tests

The conformance suite may be implemented in Rust, but case inputs and expected
outputs must remain readable by independent implementations.

## Versioning

Language edition, compiler version, package schema, lockfile schema,
diagnostic schema, component interface version, authoring protocol, and JSON
command schemas are independent versions. A single compiler release records
all supported versions.

Unknown future versions are errors. Compatibility adapters are explicit.

## Rejected baseline alternatives

### Keep Racket as production runtime

Rejected because standalone distribution, startup control, low-level runtime
work, sandbox integration, and contributor reach matter more than rapid
language prototyping at this stage.

### Keep Racket as active reference implementation

Rejected because two active implementations create semantic drift and
duplicate maintenance. Git history preserves the prototype when historical
inspection is useful.

### Start with LLVM

Rejected because it increases compiler complexity and build latency before
workloads justify native code generation.

### Build a custom bytecode VM first

Rejected because the product requires a portable sandbox boundary more than a
second runtime. WebAssembly provides validation, mature isolation machinery,
and typed component interfaces while Krit validates its domain.

### Let an LLM automatically rewrite source

Rejected because invisible semantic changes undermine review and trust. Models
may propose visible edits; the formatter and checker remain deterministic.

### Execute natural language

Rejected because ambiguity defeats deterministic checking, source-level
review, and capability enforcement.

### Allow package install scripts

Rejected because installation-time execution introduces unnecessary
supply-chain authority.
