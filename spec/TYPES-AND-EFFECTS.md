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
performs the same checking first and evaluates only a program that passes it,
reporting check's diagnostic and exit status otherwise (`LANGUAGE.md`
section 14). Ordinary public declarations,
source-level generic declarations, and effect annotation syntax are still
future work. A webhook declaration is the one implemented public boundary and
requires the exact annotated `(HttpRequest) -> HttpResponse` signature.

The bootstrap checker uses deterministic constraints and unification. It
supports recursive functions, closures, empty lists, `None`, structural
records, and matches over nested Option, Result, and list patterns. An unresolved variable may remain generic;
a known contradiction is always rejected. Full Hindley-Milner generalization
across independently polymorphic uses is still draft.

## Error propagation

Postfix `?` (`LANGUAGE.md` section 8.2) is typed against the innermost
enclosing function body. Analysis records, for each body, whether `?` may
return from it: a named function or `queue`/`schedule` entrypoint whose
declared return type is `Result<_, E>` or `Option<_>` may be exited; an
anonymous function, a webhook entrypoint, a named function without a declared
return type or with another declared return type, and the top level may not
(`K3012`). The decision uses the declared annotation, never an inferred
return type, so a function's signature alone shows whether `?` can exit its
body early. A let-else, whose arm supplies the returned value itself, may
exit any named function's body ("Let-else" below).

For `e?` in a body that may be exited:

| Declared return | Type of `e` | Type of `e?` | Otherwise |
|---|---|---|---|
| `Result<U, E>` | `Result<T, E>` (same `E`) | `T` | `K3013` for another `E` or an `Option` |
| `Option<U>` | `Option<T>` | `T` | `K3013` for a `Result` |

An operand of an unresolved type is unified with `Result<T, E>` or
`Option<T>` for a fresh `T`; any other operand type is `K3001`. The error
types are unified, not converted, so `E` is identical after inference. Every
diagnostic is reported at the `?` token, after the operand's own errors. `?`
adds no effect and no capability requirement: the facts of `e?` carry exactly
the effects and requirements of `e`.

Core IR represents the early return natively. `e?` lowers to a variant match
on `e`; the success arm binds the payload and yields it, and the failure arm
binds the error (for `Result`), rebuilds `Err(error)` or `None` at the
function's declared return type, and ends with an `exit` operation. Those
arm bindings are Core-only match bindings (debug names `$value` and `$error`)
numbered after every analysis symbol. Core verification requires an `exit`'s
value to have the enclosing function's result type, the `exit` to be the last
operation of its block and that block's result, and no `exit` in module
initialization; the exit's own type is whatever its block requires, since
control never continues past it.

## Let-else

A let-else (`LANGUAGE.md` section 8.3), `let P (: A)? = e else Q => r;`, is
typed against the innermost enclosing function body, like `?`, but it may
appear only as a direct statement of that body's block, and only when the
body is a named function's (including a `webhook`, `queue`, or `schedule`
entrypoint). Analysis records, for each body, its block and its return type;
a let-else anywhere else is `K3014` at its `let` and pattern: at the top
level, in an anonymous function's body, or in any nested block. The patterns
`P` and `Q` must be the two variants of one family (`K3014` at the arm
otherwise).

The checks run in source order:

1. `P`'s binder must be free in the body's scope (`K2002`), as for `let`;
   the discard name `_` is exempt, as it is for `let`.
2. `e` is analyzed; an annotation `A` is the type of `e` and is unified with
   it (`K3001`).
3. The type of `e` must be `Option<T>` for `Some`/`None` or `Result<T, E>` for
   `Ok`/`Err`, for fresh `T` and `E` when it is unresolved (`K3005` for any
   other type).
4. `Q`'s binder (the `T` of `Some`, or the `T` or `E` of `Ok` or `Err`) is
   declared in a scope of the arm alone, as a match symbol at the arm, and `r`
   is analyzed there. A binder `_` is declared the same way, as the discard
   name: reading `_` in `r` refers to it, with warning `K2003`.
5. The type of `r` is unified with the function's return type: the declared
   one, or the type variable that stands for an undeclared one (`K3001`).
6. `P`'s binder is declared in the body's scope with its payload type, as a
   `let` binding at the let-else statement, visible to the following
   statements and the body's final expression. A binder `_` is declared the
   same way, as the discard name, exactly like `let _ = ...`: reading `_`
   afterwards refers to it, with warning `K2003`.

The statement's effects and requirements are those of `e` and `r`; let-else
adds none of its own. Nothing is rebuilt, so no structural constraint applies
to the payload types: binding an opaque `Secret` or `DatabaseTransaction`
payload is allowed exactly as matching it is.

Core IR lowers a let-else to a variant match on `e` in the enclosing block,
followed by `bind` of the match's result to `P`'s binder (or `discard` of the
unit result of a `None` pattern). The arm of `P` binds the payload through a
Core-only match binding (debug name `$value`) and yields it; the arm of `Q`
binds `Q`'s source binder, evaluates `r`, and ends with an `exit` of its
value, verified like every `exit` to have the function's result type. A
binder `_` in either pattern is a source binder like any other: `P`'s `_`
gets the `bind` (debug name `_`), and `Q`'s arm binds the payload to `Q`'s
`_`. The match has `P`'s payload type, so successive let-else statements
stay flat in the body block.

## Matching

A `match` (`LANGUAGE.md` section 11) is checked in this order, stopping at
the first error:

1. The subject is analyzed.
2. The arms' patterns are checked against each other, without types. Each
   pattern is described by its *path*: the variant and list constructors it
   requires, from the outside in (`Ok(Some(v))` is `Ok`, `Some`; a name or
   `_` ends a path, and `None`, `[]`, and `[head, ..tail]` require nothing
   below them). A pattern matches exactly the values whose constructors
   start with its path, so two patterns overlap exactly when one path is a
   prefix of the other. In arm order, skipping top-level catch-alls, walking
   each path from the outside in: the constructors that follow the same
   prefix must be of one family (`K1003`); an earlier arm's path may not be
   a prefix of this one's (`K1003`: a duplicate arm when the paths are equal,
   an unreachable one otherwise); and this path may not be a proper prefix of
   earlier arms' paths (`K1003`: an overlapping arm, or an unreachable one
   when those arms already cover every value it matches).
3. An arm whose whole pattern is a name or `_` is `K3005`.
4. Each arm's pattern is typed against the subject's type, walking its path:
   a variant or list pattern requires its family (`Option<T>`,
   `Result<T, E>`, or `List<T>`) at its position, unifying an unresolved type
   with that family over fresh payload types (`K3005` at the match for the
   subject, or at the nested pattern for a payload). The position below
   `Some`, `Ok`, or `Err` has type `T`, `T`, or `E`; a list pattern's head
   has type `T` and its tail `List<T>`.
5. The paths must be exhaustive: at every proper prefix of a path, each
   constructor of the next position's family continues some path. Otherwise
   the match is `K3005`, listing each missing prefix-plus-constructor as a
   pattern with `_` below it.
6. Each arm's value is analyzed in a scope of the arm that declares its
   pattern's names, as match symbols at the names' own spans, with the types
   of step 4. A `_` is declared the same way, as the discard name, at its own
   span (steps 2, 3, and 5 treat it exactly like a name), so reading `_` in
   the arm refers to the most recent `_` (in `[_, .._]`, the tail), with
   warning `K2003`. The arms' types are unified into the match's type
   (`K3001`).

Because the paths are pairwise disjoint and exhaustive, every value matches
exactly one arm, and arm order has no meaning. The match's effects and
requirements are those of its subject and all of its arms.

Core IR decides a match with nested variant and list matches over the
decision tree the paths form. At each position, the Core match has one arm
per constructor, in the order the source first uses it. That arm binds the
payload to the arm's name or `_` when one arm matches the payload with a name
or `_`, and otherwise to a Core-only binding (debug name `$payload`) that a
nested match on that binding decides. A list arm binds its head and tail to
their names or `_`. Every arm's value is lowered exactly once, one Core block
deeper for each pattern nested in a payload, which is why the parser counts
those patterns as nesting levels of the arm's value (`LANGUAGE.md` section
4.1). Core verification applies its ordinary two-arm rules to each nested
match.

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
typed Core IR. For files that declare a webhook, queue, or schedule
entrypoint, or when `--target component|module` is given, it then runs the
WebAssembly target check described in `WASM-SANDBOX.md`, which reports every
`K7001` residual type and `K7002` unsupported construct that `krit build`
would, after eliminating functions the selected entrypoint cannot reach.
`krit explain FILE` renders module-init effects, top-level binding types,
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
- `Some`, `Ok`, or `Err` construction, including the `Err` that propagating an
  error with `?` rebuilds
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

## Open decisions

- Generic exported functions
- Generic user-defined record and variant declarations
- Whether effect rows are written inline or in a `requires` clause
- Exhaustiveness rules for future user-defined variants
- Stable ABI representation of exported types
- Let-generalization and effect polymorphism
