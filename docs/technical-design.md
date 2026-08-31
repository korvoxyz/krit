# Krit Rust technical design

**Status:** Accepted baseline  
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

The first Rust milestone implements source, lexer, parser, AST, diagnostics,
name resolution, and a direct evaluator to establish semantics quickly. The
first deployable backend is a WebAssembly component. It becomes the default
only after differential tests prove equivalent behavior. The direct evaluator
remains a development oracle, not a deployment runtime.

## Workspace

```text
crates/
  krit-source/       source files, spans, line maps
  krit-syntax/       tokens, lexer, AST, parser
  krit-diagnostics/  codes, rendering, JSON schema
  krit-semantics/    names, types, effects, HIR
  krit-ir/           normalized Core IR
  krit-wasm/         Core IR to WebAssembly component lowering
  krit-host/         component runtime, limits, and capability handles
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

Tokens retain spans but not copied lexeme strings. Identifier interning begins
only when profiling shows a memory or comparison benefit.

The parser is handwritten recursive descent with Pratt/precedence parsing for
expressions. Error recovery synchronizes at `;`, `}`, and declaration
keywords. The first implementation may stop after one error, but its
diagnostic code and primary span must match the specification.

## Semantic stages

### Name resolution

Names become stable symbol IDs before type checking. Resolution stores the
definition span and lexical scope. Built-ins occupy an explicit prelude scope;
they are not magic string comparisons after resolution.

### Types and effects

The checker operates on resolved HIR and emits typed HIR. Public signatures are
module cache boundaries. Inference variables and diagnostic ordering are
deterministic.

### Core IR

Core IR is typed, expression-oriented, and in administrative normal form:

- operands are variables or constants
- evaluation order is explicit
- closure capture sets are explicit
- branches and list matching are explicit control flow
- effectful operations are distinct instructions
- source spans survive lowering

Optimization never changes overflow, effect order, capability checks,
diagnostic category, or deterministic rendering.

Initial optimizations:

- constant folding with checked arithmetic
- dead pure binding elimination
- unreachable branch elimination
- precise closure capture
- tail-call marking

## WebAssembly component target

The primary deployable artifact is a small WebAssembly component. It exports
typed HTTP, webhook, schedule, queue, or agent-tool entry points and imports
only narrow host interfaces authorized for the package.

Krit does not grant a general WASI environment. Files, sockets, processes,
environment variables, clocks, randomness, secrets, state, and AI calls are
host resources available only through explicit component imports and
unforgeable handles.

An artifact records:

- WebAssembly Component Model version
- compiler build identifier
- language edition
- public interface and inferred effect hashes
- source-map and explanation metadata selected by profile
- artifact checksum

Unknown features or interfaces fail closed. Every component is validated
before storage and instantiation.

Wasmtime is the initial reference host candidate. The host enforces fuel or
epoch interruption, memory, stack, host-call, output, and wall-time limits.
Untrusted multi-tenant execution adds a restricted OS process or container.
`spec/WASM-SANDBOX.md` defines the security contract.

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
The CLI constructs a host from manifest declarations, user policy, and OS
sandbox support.

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
