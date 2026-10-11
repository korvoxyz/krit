# Krit conformance suite

These cases are implementation-neutral examples of normative Krit behavior.
An implementation is conforming only when it passes every case for the
language edition it claims to support.

## Case format

Each directory contains:

- `program.krit` — UTF-8 source
- `expect.status` — required decimal process status plus line feed
- `expect.stdout` — optional exact standard-output bytes; absent means empty
- `expect.diagnostics` — optional ordered error codes, one per line; absent
  means no errors
- `expect.warnings` — optional ordered warning codes, one per line; absent
  means no warnings
- `expect.notes` — optional decimal number of notes (fix hints) the error
  carries, plus line feed, for `conformance/check` cases; absent means one
  for a habit code (`K1101`-`K1112`), `K3011`, `K3012`, or `K3013` and none
  for any other error

`conformance/cases` contains runtime cases. The harness invokes the
equivalent of:

```text
krit run --diagnostic-format json program.krit
```

It compares status and standard output exactly. It decodes JSON Lines from
standard error and compares the ordered `code` fields of the diagnostics whose
`severity` is `error` with `expect.diagnostics` and of those whose `severity`
is `warning` with `expect.warnings`. Every diagnostic must be one or the
other. Additional diagnostic prose and labels may vary within
`spec/DIAGNOSTICS.md`.

`krit run` checks a program exactly as `krit check` does, apart from the
build-target check, before evaluating it (`spec/LANGUAGE.md` section 14), so
every runtime case must pass `krit check`, and the harness asserts that it
does. A runtime case therefore defines evaluation behavior, including
runtime errors such as overflow, division by zero, invalid JSON, and depth
limits. A program that checking rejects belongs in `conformance/check`.

`conformance/check` contains implementation-neutral static-analysis cases.
That harness invokes the equivalent of:

```text
krit check --diagnostic-format json program.krit
```

It compares status, error codes, and warning codes as the runtime harness does,
and requires that checking never execute program effects or emit program
output. The error must carry exactly the number of notes that `expect.notes`
gives, and every note must be one line (`spec/DIAGNOSTICS.md`, Fix hints);
the wording of notes is not compared. Files that declare a webhook, queue, or
schedule entrypoint are checked against the component target by default, so
`K7001`/`K7002` build-target diagnostics are part of their expected output, in
source order. `conformance/check/target` pins the current bounded backend: as
the backend grows, these cases move from failure to success. A front-end-only
harness treats a case whose expected codes are all `K7xxx` as passing
analysis. Because `krit run` performs check's front-end checking first, the
harness also requires that `krit run` refuse every case that fails it with the
same diagnostics and status and no program output; `krit run` performs no
target check, since direct execution builds no artifact.

`conformance/package` contains manifest-validation cases. Each case
directory holds `krit.pkg`, the entry it names, `expect.status`, and optional
`expect.diagnostics`. That harness runs from the case directory and invokes the
equivalent of:

```text
krit package check krit.pkg
```

It compares status and the ordered codes from the normative human diagnostic
lines (`file:line:column: severity[CODE]: message`) on standard error.

`conformance/build/compat` contains golden artifacts written by released
toolchains. Each case directory `compat/<toolchain>/<case>/` holds the
package exactly as it was built (`krit.pkg` and the entry it names), the
component `artifact.wasm` and its adjacent metadata `artifact.wasm.json`
exactly as that toolchain's `krit build --output artifact.wasm` wrote them,
`expect.status`, and optional `expect.stdout`. The harness copies the case to
a scratch directory, creates an owner-only parent directory for every durable
store `host.json` names, and invokes the equivalent of:

```text
krit sandbox --manifest krit.pkg --artifact artifact.wasm
```

or, when the case has `request.json`:

```text
krit invoke --manifest krit.pkg --artifact artifact.wasm --request request.json [--host-config host.json]
```

It compares status and standard output exactly and requires the recorded
compiler version to equal `<toolchain>`. These cases are frozen: they are
never rebuilt or reformatted, and their sources need not be canonical under a
later formatter. Cases with `host.json` run only on Unix hosts, where
owner-only state directories can be enforced.

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
| `core/string-ordering` | byte-lexicographic UTF-8 string ordering and string equality |
| `core/chains` | `else if` chains and left-associative operator chains, including short-circuiting |
| `core/else-if-chain` | a 300-branch `else if` chain adds no nesting levels |
| `scope/closures` | immutable lexical capture and shadowing |
| `scope/builtin-shadowing` | match binders shadow unreserved built-in names, as in checking |
| `scope/discard` | repeated `_` bindings in one scope; reading `_` evaluates the most recent one and warns (`K2003`) |
| `functions/recursion` | recursive declaration and calls |
| `functions/deep-recursion` | 9,000 nested calls stay within the call-depth limit |
| `lists/match` | list construction and exhaustive decomposition |
| `records/fields` | ordered record rendering and field access |
| `variants/option-match` | exhaustive Option matching |
| `variants/result-match` | exhaustive Result matching |
| `propagate/result`, `propagate/option` | postfix `?` unwraps `Ok` and `Some` and returns `Err` and `None` from the enclosing function |
| `propagate/order` | arguments evaluate left to right; effects before a `?` exit stay performed and nothing after it evaluates |
| `propagate/positions` | `?` in `if` conditions, `match` subjects, list and record elements, and nested blocks |
| `json/round-trip` | deterministic JSON conversion |
| `json/string-escapes` | every JSON string escape, surrogate pairs, and surrounding whitespace decode |
| `types/annotations` | parsed binding, parameter, return, and generic types |
| `errors/division-zero` | checked division |
| `errors/overflow` | checked signed 64-bit arithmetic |
| `errors/json-invalid` | invalid JSON input |
| `errors/json-lone-surrogate` | an unpaired surrogate escape is invalid JSON |
| `errors/json-invalid-escape` | an unknown escape is invalid JSON |
| `errors/call-depth` | runaway recursion fails with `K4010` instead of aborting |
| `check/valid/inference` | recursion, closures, empty lists, None, and effects infer together |
| `check/syntax/missing-semicolon` | missing required syntax |
| `check/syntax/habit-word-line-break` | a bound habit word ending a line before a missing `;` stays an ordinary name (`K1002`) |
| `check/name/*` | lexical undefined and duplicate name diagnostics, including duplicate record fields |
| `check/name/discard-*` | the discard name `_`: repeated bindings, reads warn with `K2003` without failing, warnings reported beside an error, and an unbound `_` is undefined |
| `check/name/did-you-mean`, `check/name/null` | undefined names (`K2001`) that carry a did-you-mean or `None` note |
| `check/habit/*` | one case per construct from another language (`K1101`-`K1112`): `return`, loops, assignment, `struct`, `import`, `await`, `throw`, indexing, arrow functions, the conditional operator, `switch`, and `else { return ... }`; plus a conditional operator whose first branch does not parse and one after a postfix `?` |
| `check/syntax/question-before-operand` | a `?` followed on its line by an operand, without a `:`, is an unexpected token (`K1001`) with one note |
| `check/propagate/*` | postfix `?` where it cannot return (`K3012`: top level, anonymous function, webhook, missing or non-`Result`/`Option` return type), with another family or error type (`K3013`), and with an opaque `Secret` error type (`K3009`) |
| `check/valid/propagate-queue` | a queue entrypoint returns its failures with `?` and builds |
| `check/valid/habit-words-as-names` | habit words stay ordinary identifiers outside foreign constructs |
| `check/type/*` | stable static operator, condition, field, call, arity, annotation, match, comparison, and JSON-encoding errors, including an infinite type between two solved bindings and a method call on a string (`K3011`) |
| `check/match/*` | unique, exhaustive, single-family Option and Result arms (`K1003`) |
| `check/webhook/*` | top-level uniqueness and exact typed webhook signatures |
| `check/capability/*` | literal-resource host capability contracts |
| `check/nesting/*` | syntactic, structural, and inferred-type nesting limits (`K1005`), including deep list literals, chains hidden by parentheses, types that grow after they are bound, and types that grow when two solved types unify |
| `check/jobs/*` | typed queue/schedule entrypoint uniqueness, signatures, and literal queue/schedule/bucket resources |
| `check/target/*` | entrypoint files report every build-target diagnostic at check time, after any front-end warnings; unused helpers are eliminated first; String routing and composition build; JSON string decoding builds; a webhook helper chaining config, secret, and HTTP calls with `?` builds |
| `check/secret/*` | opaque secret non-disclosure restrictions |
| `check/valid/webhook-contract` | transitive typed webhook/config/secret facts |
| `check/valid/queue-consumer` | typed `QueueJob` delivery and bounded object write |
| `check/valid/schedule-handler` | typed `ScheduleEvent` trigger facts and outcome |
| `package/dependencies/declared` | a declared version dependency fails closed with `K6003` |
| `package/dependencies/path-source` | a declared source-table dependency fails closed with `K6003` |
| `package/dependencies/empty-table` | an empty `[dependencies]` table is accepted |
| `build/compat/0.2.0/factorial` | a 0.2.0-built policy-1 module artifact runs in the sandbox |
| `build/compat/0.2.0/stateful-webhook` | a 0.2.0-built policy-2 durable-state webhook artifact invokes |
| `format/comments` | exact standalone and end-of-line comment preservation |
| `format/edition-2026` | canonical layout for all currently implemented syntax families |
| `format/webhook` | canonical typed webhook and ordered duplicate-header layout |
| `format/propagate` | postfix `?` stays attached to its operand and chains like a call |

## Adding a case

A conformance change must cite the normative specification section in its pull
request. Keep each program minimal and test one semantic distinction. Unit and
regression tests belong in Rust crates rather than this directory when they do
not define portable behavior.
