# Krit conformance suite

These cases are implementation-neutral examples of normative Krit behavior.
An implementation is conforming only when it passes every case for the
language edition it claims to support.

## Case format

Each directory contains:

- `program.krit` — UTF-8 source
- `expect.status` — required decimal process status plus line feed
- `expect.stdout` — optional exact standard-output bytes; absent means empty
- `expect.diagnostics` — optional ordered diagnostic codes, one per line;
  absent means no diagnostics

The harness invokes the equivalent of:

```text
krit run --diagnostic-format json program.krit
```

It compares status and standard output exactly. It decodes JSON Lines from
standard error and compares the ordered `code` fields. Additional diagnostic
prose and labels may vary within `spec/DIAGNOSTICS.md`.

`conformance/check` contains implementation-neutral static-analysis cases.
That harness invokes the equivalent of:

```text
krit check --diagnostic-format json program.krit
```

It compares status and diagnostic codes and requires that checking never
execute program effects or emit program output.

`conformance/format` contains formatter input/output pairs. Each
`input.krit` must parse, and formatting must exactly equal
`formatted.krit`. Formatting the expected output again must produce identical
bytes. These fixtures exercise the normative canonical style without making
non-canonical inputs runtime conformance programs.

No case depends on a host path, locale, time zone, environment variable,
network service, or nondeterministic value.

## Current cases

| Case | Contract |
|---|---|
| `core/arithmetic` | checked integer operators and precedence |
| `core/booleans` | strict booleans and short-circuiting |
| `core/strings` | UTF-8 strings and concatenation |
| `scope/closures` | immutable lexical capture and shadowing |
| `functions/recursion` | recursive declaration and calls |
| `lists/match` | list construction and exhaustive decomposition |
| `records/fields` | ordered record rendering and field access |
| `variants/option-match` | exhaustive Option matching |
| `variants/result-match` | exhaustive Result matching |
| `json/round-trip` | deterministic JSON conversion |
| `types/annotations` | parsed binding, parameter, return, and generic types |
| `errors/syntax` | missing required syntax |
| `errors/undefined-name` | lexical name resolution |
| `errors/wrong-kind` | runtime kind checks |
| `errors/arity` | exact function arity |
| `errors/division-zero` | checked division |
| `errors/overflow` | checked signed 64-bit arithmetic |
| `errors/function-comparison` | non-comparable functions, including in lists |
| `errors/duplicate-record-field` | unique record field names |
| `errors/duplicate-variant-arm` | unique Option and Result arms |
| `errors/incomplete-option-match` | exhaustive Option arms |
| `errors/mixed-variant-match` | one variant family per match |
| `errors/json-function` | functions cannot be JSON encoded |
| `errors/json-invalid` | invalid JSON input |
| `errors/missing-record-field` | field access requires an existing field |
| `check/valid/inference` | recursion, closures, empty lists, None, and effects infer together |
| `check/name/*` | lexical undefined and duplicate name diagnostics |
| `check/type/*` | stable static operator, condition, field, call, annotation, and match errors |
| `format/comments` | exact standalone and end-of-line comment preservation |
| `format/edition-2026` | canonical layout for all currently implemented syntax families |

## Adding a case

A conformance change must cite the normative specification section in its pull
request. Keep each program minimal and test one semantic distinction. Unit and
regression tests belong in Rust crates rather than this directory when they do
not define portable behavior.
