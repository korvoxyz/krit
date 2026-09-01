# Krit

Krit is an open, human-auditable programming language for small sandboxed
agents, bots, and integration APIs written with AI and trusted by people.

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
- booleans, strings, unit, immutable lists, and ordered records
- built-in `Option` and `Result` values with exhaustive matching
- value annotations enforced by `krit check` and deterministic JSON conversion
- lexical name resolution and deterministic static type inference/checking
- sorted `io.stdout` effect inference with function and call propagation
- name-resolved, inferred typed Core IR with deterministic IDs and explicit
  evaluation order
- verified closures, recursive self bindings, captures, branches, and matches
- stable human and schema-1 JSON compiler explanations
- deterministic validator-accepted WebAssembly Component Model artifacts for
  the initial layout-concrete Core subset
- effect-selected `krit:runtime@0.2.0` pure and stdout WIT worlds
- explicit fail-closed Wasm feature/import policy, schema-1 adjacent metadata,
  and exact-byte BLAKE3 digests
- recursive function declarations
- exhaustive empty/cons list matching
- deterministic comment-preserving canonical source formatting
- deterministic human and JSON diagnostics
- implementation-neutral conformance cases
- strict package manifest validation

Type/effect generalization beyond the bootstrap checker, composite Wasm
layouts, the component runtime host, sandbox resource enforcement, agent APIs,
guided authoring, modules, dependency resolution, and build caching are
specified directions, not yet implemented features. Krit is not
production-ready.

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

Check syntax, lexical names, types, matches, and inferred effects without
executing. A successful check also lowers and verifies typed Core IR:

```sh
krit check examples/factorial.krit
```

Inspect stable compiler facts and the resolved Core program:

```sh
krit explain examples/factorial.krit
krit explain --json examples/factorial.krit
```

The explanation shows the synthetic `module-init` entrypoint, its inferred
effects, top-level binding/function types, and deterministic typed Core IR.
Core executable references use numeric IDs; source names appear only as debug
metadata. Explanation JSON schema 1 is serialized with `serde_json` and does
not include absolute compiler or cache paths.

Core names are resolved and its types are normalized inference results, but
not every Core type is a concrete storage layout. Constrained parametric type
variables may remain in otherwise valid generic Core. The WebAssembly artifact
stage must specialize such variables, or report a stable source diagnostic,
before choosing layouts or emitting code. Open structural record requirements
likewise describe required fields rather than a final closed Wasm record
layout.

Format one or more files after validating the complete batch:

```sh
krit fmt examples/factorial.krit examples/lists.krit
krit fmt --check examples/factorial.krit examples/lists.krit
```

`krit fmt` preserves every `//` comment, emits four-space indentation and LF
line endings, and leaves all requested files untouched if any file cannot be
read or parsed. `--check` writes nothing and returns status `1` when a file is
not canonical.

Validate a package manifest:

```sh
krit package check
```

Inspect every capability requested by the package:

```sh
krit permissions
krit permissions --json
```

Phase 1 validates configuration keys, opaque secret names, and exact
outbound HTTP origins in `krit.pkg`. It reports requested authority only;
deployment grants and sandbox enforcement arrive with the WebAssembly host.

Build the package's validated WebAssembly component:

```sh
krit build
krit build --manifest path/to/krit.pkg --output dist/program.wasm
```

The default output is `target/krit/krit.wasm` for this repository, with
metadata at `target/krit/krit.wasm.json`. Metadata schema 1 includes the exact
`blake3:<hex>` digest and byte size, package-relative entry, WIT world, sorted
effects/imports, and policy version. The digest covers the final component
bytes after bounded embedded metadata is attached. Pure programs select the
zero-import `pure-program` world; programs with the checked `io.stdout` effect
select `program` and its stdout interface. Unused manifest grants do not widen
artifact imports, and validation derives the world and effects from the actual
component/core import surface before accepting metadata claims.

Artifact policy 1 supports `Int`, `Bool`, `Unit`, recursive and higher-order
non-capturing functions, blocks, conditionals/short circuit, checked integer
operators, primitive comparisons, and scalar `print`/`println`. Strings,
lists, records, Option/Result, JSON, lexical captures, and unresolved
parametric layouts fail with stable `K7001`/`K7002` diagnostics. `krit build`
never falls back to direct interpretation. No command instantiates the
component yet; `krit run` remains the direct evaluator.

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

Generate Krit with Claude, ChatGPT, Gemini, or a local model using the exact
provider-neutral instruction shipped with this compiler:

```sh
krit prompt
```

See [ai/README.md](ai/README.md) for the generation and diagnostic-repair
workflow. Prompt material contains only currently implemented syntax so models
cannot confuse draft agent APIs with compilable Krit 0.2 code.

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
target = "wasm-component"

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
- [Agent application model](spec/AGENT-APPLICATIONS.md) — draft
- [Types and effects](spec/TYPES-AND-EFFECTS.md) — implemented baseline
- [Capabilities](spec/CAPABILITIES.md) — draft
- [Modules and packages](spec/PACKAGES.md) — draft
- [WebAssembly sandbox](spec/WASM-SANDBOX.md) — artifact baseline implemented;
  host draft
- [Guided AI authoring](spec/GUIDED-AUTHORING.md) — draft
- [Narrow product MVP](docs/mvp.md)
- [Agent platform roadmap](docs/agent-roadmap.md)
- [Rust technical design](docs/technical-design.md)
- [Performance methodology](docs/performance.md)
- [Initial measured baseline](benchmarks/baseline.json)
- [Conformance suite](conformance/README.md)

The Racket prototype is preserved only in Git history at tag
`racket-v0.1.0`. It is not an active implementation, runtime dependency,
semantic reference, CI requirement, or contributor tool.

## Develop

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --all-targets --locked
cargo build --workspace --release --locked
```

The conformance suite runs through the Rust tests. See
[CONTRIBUTING.md](CONTRIBUTING.md) for the language-change process.

## Direction

The accepted implementation path is:

1. Rust source, parser, diagnostics, and direct evaluator
2. readable records, built-in `Option` and `Result`, parsed annotations, and
   dynamic JSON conversion
3. name resolution, static type/effect checking, and canonical formatting
   (complete)
4. typed verified Core IR and deterministic explanations (complete)
5. Core layout diagnostics and validated WebAssembly component artifacts
   (complete for policy 1), followed by one bounded host
6. HTTP/webhook interfaces with outbound HTTP, secrets, and AI calls
7. optional provider-neutral inline prediction with visible checked edits
8. broader connectors and packaging only after the reference agent succeeds

Performance claims follow [docs/performance.md](docs/performance.md), not
implementation-language assumptions.

## License

Krit's specifications, implementation, and documentation are licensed under
the [Apache License 2.0](LICENSE). It is permissive and includes an explicit
patent grant. It does not place licensing requirements on programs written in
Krit.
