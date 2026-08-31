# Static types and effects

**Status:** Draft  
**Target:** Krit 0.3

## Goals

The type/effect system must make AI-generated code safer to review without
requiring verbose annotations inside every function.

- Infer local types.
- Require explicit public package boundaries.
- Reject implicit `Any`.
- Track side effects separately from value types.
- Produce concise explanations and machine-readable facts.
- Keep checking deterministic and incremental.

## Value types

Proposed built-in types:

```text
Int
Bool
String
Unit
List<T>
Option<T>
Result<T, E>
Record { field: Type, ... }
fn(A, B) -> C
```

Type variables use lowercase identifiers in compiler output, such as `a`.
Source-level generic declaration syntax is not yet accepted.

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

The parser accepts value annotations on let bindings, function parameters,
and function return values:

```krit
let fallback: Option<String> = None;

fn total(items: List<Int>) -> Int {
    // ...
}
```

The exact implemented annotation grammar is normative in `LANGUAGE.md`.
Annotations are stored in the AST but remain dynamically inert until the
static checker lands. Public declarations and effect annotation syntax are
still future work.

## Effects

A value type describes what an expression returns. An effect row describes
what observable operations it may perform.

Proposed effects:

```text
io.stdout
io.stdin
fs.read
fs.write
net.connect
process.spawn
env.read
clock.read
random.read
secret.read
ai.invoke
```

Pure functions have an empty effect row. Calling a function adds its effects
to the caller. Branch effects are the union of effects reachable through
either branch.

Effect polymorphism may be added for higher-order functions, but the first
checker can require a concrete effect row.

## Effects versus capabilities

Effects and capabilities answer different questions:

- **Effect:** what an expression may attempt.
- **Capability:** what the host permits this execution to do.

A program can type-check while lacking a runtime grant. `krit check` reports
the effect set; `krit permissions` compares that set with package and host
grants.

A dependency may declare required effects but cannot grant capabilities.

## Inference algorithm

The intended baseline is constraint-based Hindley-Milner inference extended
with explicit effect rows and the value restriction if effectful bindings are
generalized.

Implementation requirements:

- unification must be deterministic
- type variable numbering must be stable
- errors should identify the originating constraints
- checking one changed module should not require unrelated modules
- exported signatures form module cache boundaries

## Error quality

A type error should state:

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
