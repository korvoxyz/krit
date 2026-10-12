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
`HttpRequest`, `HttpResponse`, and `LogField` are stable built-in aliases for
the closed record structures in `LANGUAGE.md`; they retain their nominal names
at public entrypoint and Core boundaries while participating in structural
record checking. `Secret` is a distinct opaque type, never a `String` alias.

A named construction such as `HttpResponse { status: 200, body }`
(`LANGUAGE.md` section 8.2) has its contract's closed structural record type,
`Record { body: String, headers: List<HttpHeader>, status: Int }` here, which
is the type `record { ... }` with every contract field has once it meets the
contract. The checker first checks the written field names (`K3016`), then
analyzes each written value in source order and unifies it with its contract
field's type, reporting a mismatch as `K3001` at the value (`` `HttpResponse`
field `status` type mismatch ``). An omitted field contributes its contract
field's concrete type (`String` or `List<HttpHeader>`) and no inference
variable, effect, or requirement. The construction's effects and literal
requirements are the union of its written values'. Core lowering emits the
same record operation the full `record { ... }` would, with one pure literal
operation per omitted field at the type name's span.

The unit value `()` has type `Unit` and no effect. An entry list
`["k": v, ...]` (`LANGUAGE.md` section 11) has type
`List<Record { name: String, value: String }>`, the type
`[record { name: k, value: v }, ...]` has once its fields are `String`, so it
unifies with `List<HttpHeader>` and `List<LogField>`. The checker analyzes
each key and then its value in source order and unifies each with `String`,
reporting a mismatch as `K3001` at that key or value (`entry key type
mismatch`, `entry value type mismatch`). The entry list's effects and literal
requirements are the union of its keys' and values'. Core lowering emits the
operations of the spelled-out list: each key and then its value, one record
operation per entry at the entry's span (from the key to the value), and the
list operation. Doc comments (`LANGUAGE.md` section 2.1) take no part in
checking.

An interpolated string or text block (`LANGUAGE.md` section 9.1) has type
`String`. The checker analyzes each hole's value in source order, a name with
optional field accesses, records a fact for it at its own span inside the
string, and unifies its type with `String`; a mismatch is `K3015` at the hole
(`` interpolation hole `count` has type `Int`, but a hole must be a `String` ``)
with one note, never `K3001`. So a hole fixes an unannotated parameter or an
open record field to `String`, as `+` with a `String` operand would, and a
later use at another type is the ordinary `K3001` there. Holes add no effect
and no requirement. Core lowering is left-to-right concatenation with the
existing operations: one string literal operation per text part at that
text's span, each hole's value, and one `add` per part after the first,
spanning the string up to that part; a string with no parts is the empty
string literal, and one that is a single hole is that hole's value. No
operation or Core form is specific to interpolation, so every backend that
lowers string literals and string `+` lowers it.

The standard-library functions (`LANGUAGE.md` section 12.1) have the fixed
function types listed there and no effect or literal requirement. Each use of
one gets a fresh function type, so a use checks its arguments exactly as a
call of any function does: a mismatch is `K3001` at the argument, a wrong
count `K3003`. The `url_query` parameter is the closed structural
`List<Record { name: String, value: String }>`, so an entry list, a
`List<HttpHeader>`, and a `List<LogField>` all unify with it. Core lowering
emits an ordinary built-in operation and call, which the verifier checks
against the same signature (up to the structural equivalence of the contract
records), with no requirement.

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
performs the same checking first and evaluates only a program that passes it,
reporting check's diagnostic and exit status otherwise (`LANGUAGE.md`
section 14). Ordinary public declarations,
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
are conservative unions. JSON conversion and the standard library are pure.

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
typed Core IR. For files that declare a webhook, queue, or schedule
entrypoint, or when `--target component|module` is given, it then runs the
WebAssembly target check described in `WASM-SANDBOX.md`, which reports every
`K7001` residual type and `K7002` unsupported construct that `krit build`
would, after eliminating functions the selected entrypoint cannot reach.
`krit explain FILE` renders module-init effects, top-level binding types,
stable Core facts, and a compact webhook contract when present; each
top-level function or entrypoint binding with a doc comment (`LANGUAGE.md`
section 2.1) is followed by its doc comment's lines, each written `/// text`.
`krit explain --json FILE` keeps schema 1 compatibility and adds a versioned
entrypoint-contract fact containing source name, kind, normalized signature,
sorted effects, sorted capability requirements, and exact draft-2020-12
request/response JSON Schemas. A binding or entrypoint fact whose declaration
has a doc comment carries its text in an optional `doc` string field; the
field is absent otherwise.

## Effects versus capabilities

Effects and capabilities answer different questions:

- **Effect:** what an expression may attempt.
- **Capability:** what the host permits this execution to do.

Literal requirements come only from direct string literals, which are
one-line strings without holes. An interpolated string or text block in a
resource position is `K3008`, even when it has no holes, so every resource is
spelled out where it is used.

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
- no inference variable may stand for a type nesting more than 256 type
  constructors deep (`K1005`, `LANGUAGE.md` section 4.1); solving a variable
  rechecks every type that contains it, and unifying two solved variables
  rechecks every type that contains either, so a type bound earlier cannot
  grow past the limit when a variable inside it is solved later, and every
  later walk over types stays bounded
- unifying two solved variables fails as an infinite type (`K3001`) before
  either solution changes when one solution mentions the other variable, so
  a rejected unification never leaves a type that contains itself
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

A call whose callee is a field access, `value.name(...)`, calls the function
stored in that record field. When `value`'s type has no field `name`, because
it is not a record type or is a closed record type without that field, the
error is a method call (`K3011`) rather than a field-access error (`K3004`),
and its note names the standard-library function to use instead where there
is one (`DIAGNOSTICS.md`, Fix hints). An undefined name (`K2001`) carries a
did-you-mean note chosen from the names visible at the reference.

## Lints

After analysis accepts a program, lints (`K9000`-`K9999`, `DIAGNOSTICS.md`)
examine its syntax with the analysis facts: resolved names and inferred
types. They report warnings only, never change the analysis, and do not run
when analysis rejects the program.

`K9013` guards the path and query of an outbound request. It examines the
`path` and `query` values of every `HttpRequest` the program builds, written
as a named construction or as a record literal whose analyzed type is the
`HttpRequest` record (its five fields, each with its contract type or a type
still unknown, as `headers: []` has). Only a value that is an interpolated
string or a `+` chain is split into pieces: each hole (also one in a string
inside the chain) and each other operand of the chain. A literal piece is
safe, and so is a direct call of `url_path_segment`, `url_query`, or
`int_to_string` whose callee resolves to the built-in, or a name that resolves
to a binding of one of these results: a `let` whose value is such a call, or
the `Ok` binder of a match whose subject is a direct call of
`url_path_segment` or a name bound to one. Bindings are immutable, so such a
name always holds the encoded value, in the function that binds it or one
nested inside it. Every other piece is `K9013` at its hole or operand. A value
that is a literal, a name, a field access, or any other expression is not
split and never warns, so forwarding `request.path` unchanged is allowed.

## Open decisions

- Generic exported functions
- Generic user-defined record and variant declarations
- Whether effect rows are written inline or in a `requires` clause
- Exhaustiveness rules for future user-defined variants
- Stable ABI representation of exported types
- Let-generalization and effect polymorphism
