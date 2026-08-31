# Krit

Krit is an open, human-auditable programming language for software written
with AI and trusted by people.

```krit
fn sum(items) {
    match items {
        [] => 0,
        [head, ..tail] => head + sum(tail),
    }
}

println(sum([10, 20, 12]));
```

```text
42
```

Krit favors readable source, deterministic behavior, immutable values,
explicit authority, and machine-readable compiler facts. Natural language can
help generate Krit, but natural language is never executable Krit.

Krit and the Krit language are owned by Akshay Bhardwaj.

## Status

Krit 0.2 is an early Rust bootstrap implementing the normative dynamic core:

- UTF-8 source with precise byte and human positions
- familiar expressions, blocks, functions, calls, and operators
- immutable lexical bindings and closures
- checked 64-bit integer arithmetic
- booleans, strings, unit, and immutable lists
- recursive function declarations
- exhaustive empty/cons list matching
- deterministic human and JSON diagnostics
- implementation-neutral conformance cases
- strict package manifest validation

Static types and effects, capability enforcement, modules, dependency
resolution, bytecode, and build caching are specified directions, not yet
implemented features. Krit is not production-ready.

## Requirements

- Rust 1.94 or newer
- Cargo

Install Rust through [rustup](https://rustup.rs/) if it is not already
available.

## Build

```sh
cargo build --release --locked
./target/release/krit --version
```

Install the CLI from this checkout:

```sh
cargo install --path crates/krit-cli --locked
krit --version
```

## Use

Run a source file:

```sh
krit run examples/factorial.krit
krit run examples/lists.krit
```

Check syntax without executing:

```sh
krit check examples/factorial.krit
```

Validate a package manifest:

```sh
krit package check
```

Request JSON Lines diagnostics for tools and AI agents:

```sh
krit run --diagnostic-format json broken.krit
```

```json
{"schema":1,"severity":"error","code":"K2001","message":"undefined name `total`","file":"broken.krit","span":{"start":{"line":1,"column":9,"byte":8},"end":{"line":1,"column":14,"byte":13}},"labels":[],"notes":[]}
```

Show all commands:

```sh
krit --help
```

## Language tour

### Values and checked operators

```krit
let answer = 20 + 22;
let greeting = "Hello, " + "Krit!";
let ready = answer == 42 && true;

println(answer);
println(greeting);
println(ready);
```

Integers are signed 64-bit values. Overflow, division by zero, wrong value
kinds, unresolved names, and incorrect function arguments are errors rather
than silent conversions.

### Lexical functions

```krit
let offset = 40;
let add_offset = fn(value) {
    value + offset
};

println(add_offset(2));
```

Bindings are immutable and functions use lexical scope.

### Recursion

```krit
fn factorial(number) {
    if number == 0 {
        1
    } else {
        number * factorial(number - 1)
    }
}

println(factorial(6));
```

### Lists and exhaustive matching

```krit
fn length(items) {
    match items {
        [] => 0,
        [head, ..tail] => 1 + length(tail),
    }
}

println(length(["human", "and", "AI"]));
```

The two list shapes are visible and mandatory. Pattern names exist only in the
non-empty branch.

See [spec/LANGUAGE.md](spec/LANGUAGE.md) for complete normative syntax and
runtime semantics.

## Package baseline

`krit.pkg` is strict TOML:

```toml
schema = 1

[package]
name = "akshay/krit"
version = "0.2.0"
edition = "2026"
entry = "examples/factorial.krit"
license = "Apache-2.0"

[capabilities]
stdout = true
```

Unknown fields, malformed names, unsupported editions, invalid versions, and
unsafe entry paths fail closed. Dependency resolution and lockfile generation
will follow [spec/PACKAGES.md](spec/PACKAGES.md).

## Architecture and specifications

The specification is the semantic authority:

- [Language charter](spec/CHARTER.md)
- [Krit 0.2 language](spec/LANGUAGE.md)
- [Diagnostic contract](spec/DIAGNOSTICS.md)
- [Types and effects](spec/TYPES-AND-EFFECTS.md) — draft
- [Capabilities](spec/CAPABILITIES.md) — draft
- [Modules and packages](spec/PACKAGES.md) — draft
- [Rust technical design](docs/technical-design.md)
- [Performance methodology](docs/performance.md)
- [Conformance suite](conformance/README.md)

The Racket prototype is preserved only in Git history at tag
`racket-v0.1.0`. It is not an active implementation, runtime dependency,
semantic reference, CI requirement, or contributor tool.

## Develop

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --all-targets --locked
cargo build --workspace --release --locked
```

The conformance suite runs through the Rust tests. See
[CONTRIBUTING.md](CONTRIBUTING.md) for the language-change process.

## Direction

The accepted implementation path is:

1. Rust source, parser, diagnostics, and direct evaluator
2. name resolution plus static type/effect checking
3. explicit capability host boundary
4. typed Core IR with precise closure capture
5. compact register bytecode and validated artifacts
6. deterministic package resolution, lockfiles, content store, and cache
7. optional Cranelift or WebAssembly backends only when measurements justify
   them

Performance claims follow [docs/performance.md](docs/performance.md), not
implementation-language assumptions.

## License

Krit's specifications, implementation, and documentation are licensed under
the [Apache License 2.0](LICENSE). It is permissive and includes an explicit
patent grant. It does not place licensing requirements on programs written in
Krit.
