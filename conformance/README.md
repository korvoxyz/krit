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

`conformance/cases` contains runtime cases. The harness invokes the
equivalent of:

```text
krit run --diagnostic-format json program.krit
```

It compares status and standard output exactly. It decodes JSON Lines from
standard error and compares the ordered `code` fields. Additional diagnostic
prose and labels may vary within `spec/DIAGNOSTICS.md`.

`krit run` checks a program exactly as `krit check` does before evaluating it
(`spec/LANGUAGE.md` section 14), so every runtime case must pass `krit check`,
and the harness asserts that it does. A runtime case therefore defines
evaluation behavior, including runtime errors such as overflow, division by
zero, invalid JSON, and depth limits. A program that checking rejects belongs
in `conformance/check`.

`conformance/check` contains implementation-neutral static-analysis cases.
That harness invokes the equivalent of:

```text
krit check --diagnostic-format json program.krit
```

It compares status and diagnostic codes and requires that checking never
execute program effects or emit program output. Because `krit run` checks
first, the harness also requires that `krit run` refuse every failing check
case with the same diagnostics and status and no program output.

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
| `core/chains` | `else if` chains and left-associative operator chains, including short-circuiting |
| `core/else-if-chain` | a 300-branch `else if` chain adds no nesting levels |
| `scope/closures` | immutable lexical capture and shadowing |
| `scope/builtin-shadowing` | match binders shadow unreserved built-in names, as in checking |
| `functions/recursion` | recursive declaration and calls |
| `functions/deep-recursion` | 9,000 nested calls stay within the call-depth limit |
| `lists/match` | list construction and exhaustive decomposition |
| `records/fields` | ordered record rendering and field access |
| `variants/option-match` | exhaustive Option matching |
| `variants/result-match` | exhaustive Result matching |
| `json/round-trip` | deterministic JSON conversion |
| `types/annotations` | parsed binding, parameter, return, and generic types |
| `errors/division-zero` | checked division |
| `errors/overflow` | checked signed 64-bit arithmetic |
| `errors/json-invalid` | invalid JSON input |
| `errors/call-depth` | runaway recursion fails with `K4010` instead of aborting |
| `check/valid/inference` | recursion, closures, empty lists, None, and effects infer together |
| `check/syntax/missing-semicolon` | missing required syntax |
| `check/name/*` | lexical undefined and duplicate name diagnostics, including duplicate record fields |
| `check/type/*` | stable static operator, condition, field, call, arity, annotation, match, comparison, and JSON-encoding errors |
| `check/match/*` | unique, exhaustive, single-family Option and Result arms (`K1003`) |
| `check/webhook/*` | top-level uniqueness and exact typed webhook signatures |
| `check/capability/*` | literal-resource host capability contracts |
| `check/nesting/*` | syntactic, structural, and inferred-type nesting limits (`K1005`), including deep list literals, chains hidden by parentheses, and types that grow after they are bound |
| `check/jobs/*` | typed queue/schedule entrypoint uniqueness, signatures, and literal queue/schedule/bucket resources |
| `check/secret/*` | opaque secret non-disclosure restrictions |
| `check/valid/webhook-contract` | transitive typed webhook/config/secret facts |
| `check/valid/queue-consumer` | typed `QueueJob` delivery and bounded object write |
| `check/valid/schedule-handler` | typed `ScheduleEvent` trigger facts and outcome |
| `format/comments` | exact standalone and end-of-line comment preservation |
| `format/edition-2026` | canonical layout for all currently implemented syntax families |
| `format/webhook` | canonical typed webhook and ordered duplicate-header layout |

## Adding a case

A conformance change must cite the normative specification section in its pull
request. Keep each program minimal and test one semantic distinction. Unit and
regression tests belong in Rust crates rather than this directory when they do
not define portable behavior.
