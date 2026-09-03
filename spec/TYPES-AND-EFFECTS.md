# Static types and effects

**Status:** Implemented through bounded Phase 4 host contracts
**Target:** Krit 0.2 bootstrap

## Goals

The type/effect system must make AI-generated code safer to review without
requiring verbose annotations inside every function.

- Infer local types.
- Require explicit public package boundaries (future module work).
- Reject implicit `Any`.
- Track side effects separately from value types.
- Produce concise explanations and machine-readable facts.
- Keep checking deterministic; incremental module checking is future work.

## Value types

Implemented built-in and inferred types:

```text
Int
Bool
String
Unit
List<T>
Option<T>
Result<T, E>
Record { field: Type, ... }
HttpHeader
HttpRequest
HttpResponse
LogField
Secret
fn(A, B) -> C
```

The public Rust analysis API represents unresolved inference variables as
`Type::Variable` and renders them as stable lowercase names such as `'a`.
Source-level generic declaration syntax is not yet accepted. `HttpHeader`,
`HttpRequest`, and `HttpResponse` are stable built-in aliases for the closed
record structures in `LANGUAGE.md`; they retain their nominal names at public
entrypoint and Core boundaries while participating in structural record
checking. `Secret` is a distinct opaque type, never a `String` alias.

There are no implicit numeric, string, boolean, or collection conversions.
Union types, subtyping, null, exceptions, and user-defined mutable objects are
out of scope for the first static checker.

## Inference boundary

Local bindings and private functions may omit annotations:

```krit
fn double(value) {
    value + value
}
```

The parser and checker accept value annotations on let bindings, function
parameters, and function return values:

```krit
let fallback: Option<String> = None;

fn total(items: List<Int>) -> Int {
    // ...
}
```

The exact implemented annotation grammar is normative in `LANGUAGE.md`.
`krit check` enforces annotations without evaluating source. `krit run`
continues to use the direct dynamic evaluator in this milestone, so runtime
conformance diagnostics remain unchanged. Ordinary public declarations,
source-level generic declarations, and effect annotation syntax are still
future work. A webhook declaration is the one implemented public boundary and
requires the exact annotated `(HttpRequest) -> HttpResponse` signature.

The bootstrap checker uses deterministic constraints and unification. It
supports recursive functions, closures, empty lists, `None`, structural
records, and Option/Result matches. An unresolved variable may remain generic;
a known contradiction is always rejected. Full Hindley-Milner generalization
across independently polymorphic uses is still draft.

## Effects

A value type describes what an expression returns. An effect row describes
what observable operations it may perform.

The representation is extensible to these effects:

```text
io.stdout
io.stdin
config.read
fs.read
fs.write
http.request
net.connect
process.spawn
env.read
clock.read
random.read
secret.read
ai.invoke
queue.publish
queue.consume
schedule.trigger
object.read
object.write
database.read
database.write
cache.read
cache.write
search.query
search.vector
```

Implemented analysis recognizes `io.stdout`, `config.read`, `secret.read`,
`http.request`, `ai.invoke`, `observe.log`, `state.transaction`,
`queue.publish`, `queue.consume`, `schedule.trigger`, `object.read`,
`object.write`, `database.read`, `database.write`, `cache.read`, `cache.write`,
`search.query`, and `search.vector`. Pure functions have an
empty
effect set. Calling a function adds its effects to the caller,
including recursive and higher-order propagation. Branch and match effects
are conservative unions. JSON conversion is pure.

The Rust API returns effects in sorted deterministic order through
`Analysis::effects`; function types expose their inferred latent effects.
`Analysis` separately exposes sorted, deduplicated literal-resource
requirements through `Analysis::requirements`; function, expression, and
block facts expose the same transitive requirement summaries. A requirement
is the ordered pair `(capability, resource)`, currently
`config.read`/configuration-key, `secret.read`/secret-name,
`http.request`/exact-origin, `ai.invoke`/adapter-name,
`state.transaction`/store-name, `queue.publish`/queue-name,
`queue.consume`/queue-name, `schedule.trigger`/schedule-name, or
`object.read`/`object.write`/bucket-name, or
`database.read`/`database.write`/database-name,
`cache.read`/`cache.write`/namespace-name, or
`search.query`/`search.vector`/index-name. Replay operations carry the
state effect and both the store and exact external HTTP/AI requirements.
Database query, execute, commit, and rollback carry no effect of their own:
their authority is the opaque transaction handle they receive.
`queue.consume` and `schedule.trigger` come from the entrypoint declaration
rather than a call. Coarse effects
never erase or replace these resource identities. `Analysis` also exposes
normalized symbol facts and resolved symbol or built-in identities. Core
lowering consumes those facts and does not run an independent inference
algorithm.

Core name resolution and type inference do not imply that every type has a
concrete backend layout. Valid generic Core may retain constrained
`Type::Variable` values, and open structural record constraints may retain
required-field information without defining a closed representation. A Wasm
artifact stage must specialize or monomorphize those boundaries, or emit a
stable source diagnostic, before layout selection and code emission.

`krit check` preserves its existing success line while lowering and verifying
typed Core IR. `krit explain FILE` renders module-init effects, top-level binding types,
stable Core facts, and a compact webhook contract when present.
`krit explain --json FILE` keeps schema 1 compatibility and adds a versioned
entrypoint-contract fact containing source name, kind, normalized signature,
sorted effects, sorted capability requirements, and exact draft-2020-12
request/response JSON Schemas.

## Effects versus capabilities

Effects and capabilities answer different questions:

- **Effect:** what an expression may attempt.
- **Capability:** what the host permits this execution to do.

A program can type-check while lacking a runtime grant. The analyzer reports
the inferred effect and requirement sets to compiler clients. Source-only
checking and explanation do not need a manifest. Package build orchestration
rejects a missing matching literal resource before backend emission.
`krit permissions` without an artifact reports manifest requests. With
`--artifact PATH`, it validates the component and compares the artifact's
exact required effects/imports with the local manifest; deployment grants
remain `not-evaluated`.

A dependency may declare required effects but cannot grant capabilities.

## Opaque-secret restrictions

The checker rejects `Secret` anywhere an operation could reveal, duplicate,
serialize, or turn the handle into ordinary application data:

- `print` and `println`
- equality or inequality
- `json_encode`
- list and record construction
- `Some`, `Ok`, or `Err` construction
- structured logging fields

`Result<Secret, String>` produced by `secret("literal")` is an explicit host
operation result, not permission to construct arbitrary secret-containing
data. Binding or pattern-matching the opaque handle is allowed so a future
approved connector can consume it without revealing its contents.

## Inference algorithm

The implemented baseline is deterministic constraint solving with
monomorphic inference variables, structural record constraints, and effect
dependency propagation. Hindley-Milner generalization and a value restriction
remain future extensions.

Implementation requirements:

- unification must be deterministic
- type variable numbering must be stable
- errors should identify the originating constraints
- checking one changed module should not require unrelated modules (draft)
- exported signatures form module cache boundaries (draft)

## Error quality

The bootstrap errors state the operation, expected type, inferred type, and a
stable primary span where applicable. Multiple labels and full inference
traces remain draft. The intended complete error should state:

1. the operation being checked
2. the expected type
3. the inferred type
4. both relevant source spans when they differ
5. the shortest useful inference explanation

The compiler must not emit a cascade of speculative errors after a binding's
type becomes unknown.

## Open decisions

- Generic exported functions
- Generic user-defined record and variant declarations
- Whether effect rows are written inline or in a `requires` clause
- Exhaustiveness rules for future user-defined variants
- Stable ABI representation of exported types
- Let-generalization and effect polymorphism
