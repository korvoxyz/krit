# Krit conformance suite

These cases are implementation-neutral examples of normative Krit behavior.
An implementation is conforming only when it passes every case for the
language edition it claims to support.

## Case format

The suite has five parts: runtime cases (`conformance/cases`), static cases
(`conformance/check`), format cases (`conformance/format`), package cases
(`conformance/package`), and build compatibility cases
(`conformance/build/compat`). Each runtime and static case directory
contains:

- `program.krit` — UTF-8 source
- `expect.status` — required decimal process status plus line feed
- `expect.stdout` — optional exact standard-output bytes; absent means empty
- `expect.diagnostics` — optional ordered error codes, one per line; absent
  means no errors
- `expect.warnings` — optional ordered warning codes, one per line; absent
  means no warnings
- `expect.notes` — optional decimal number of notes (fix hints) the error
  carries, plus line feed, for `conformance/check` cases; absent means one
  for a habit code (`K1101`-`K1112`), `K3011`, `K3012`, `K3013`, or `K3014`
  and none for any other error

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

## Harness implementations

`cargo test --workspace` runs every harness above:

| Test | Harness |
|---|---|
| `crates/krit/tests/conformance.rs` | runtime cases through the library, each checked first; and `krit run`'s refusal of every failing static case |
| `crates/krit/tests/check_conformance.rs` | static cases through the front end (a case whose expected codes are all `K7xxx` passes analysis there), and every runtime case and example passing analysis |
| `crates/krit-cli/tests/cli.rs` | runtime and static cases through the `krit` binary, in the JSON Lines format, with note counts |
| `crates/krit-cli/tests/target_agreement.rs` | static cases through `krit check` with the default component target, and `krit check --target` reporting exactly the `K7001`/`K7002` set (codes and spans) of `krit build` for every example, conformance program, and target probe |
| `crates/krit-cli/tests/package_conformance.rs` | package cases |
| `crates/krit-cli/tests/compat_conformance.rs`, `crates/krit-runtime/tests/compat.rs`, `crates/krit-wasm/tests/compat.rs` | build compatibility cases through the CLI, the runtime, and artifact validation |
| `crates/krit/tests/formatting.rs` | format cases, and canonical, idempotent formatting of every other Krit source in the repository except the frozen compatibility sources |

## Current cases

Each table lists case directories relative to its suite; `*` stands for a
family of cases with one contract.

### Runtime cases (`conformance/cases`)

Evaluation behavior, run with `krit run`.

| Case | Contract |
|---|---|
| `core/arithmetic` | checked integer operators and precedence |
| `core/booleans` | strict booleans and short-circuiting |
| `core/strings` | UTF-8 strings and concatenation |
| `core/string-ordering` | byte-lexicographic UTF-8 string ordering and string equality |
| `core/interpolation` | interpolated strings: holes with field paths, adjacent holes, escapes beside holes, empty parts, a hole that is the whole string, `\\(` as text, scoping, and equality with the `+` chain |
| `core/text-blocks` | text blocks: the closing delimiter's indentation is removed, deeper indentation, blank lines, quotes, escapes, and holes are kept, no final line break, and the empty block |
| `core/chains` | `else if` chains and left-associative operator chains, including short-circuiting |
| `core/else-if-chain` | a 300-branch `else if` chain adds no nesting levels |
| `core/unit` | the unit value `()`: printing, equality, `Ok(())`, and `Unit` results |
| `scope/closures` | immutable lexical capture and shadowing |
| `scope/builtin-shadowing` | match binders shadow unreserved built-in names, as in checking |
| `scope/discard` | repeated `_` bindings in one scope; reading `_` evaluates the most recent one and warns (`K2003`) |
| `scope/discard-pattern` | reading a pattern's `_` (a nested variant payload, a let-else pattern) evaluates the value at its position rather than an outer `_`, and warns (`K2003`) |
| `functions/recursion` | recursive declaration and calls |
| `functions/deep-recursion` | 9,000 nested calls stay within the call-depth limit |
| `functions/doc-comments` | doc comments, banners, and `///` lines that document nothing change no behavior |
| `lists/match` | list construction and exhaustive decomposition |
| `lists/patterns` | list patterns with `_` head or tail in either arm order, and list patterns nested in `Ok` |
| `lists/entry-lists` | entry lists: keys and values evaluated in source order, equality with the spelled-out records, header and log-field lists, trailing commas, and JSON encoding |
| `records/fields` | ordered record rendering and field access |
| `records/named-construction` | named construction of the four contract records: punning, spec-fixed defaults, contract field order, source-order evaluation, and equality with the full `record { ... }` |
| `variants/option-match` | exhaustive Option matching |
| `variants/result-match` | exhaustive Result matching |
| `variants/nested-patterns` | nested `Ok(Some(v))`/`Ok(None)`/`Err(e)`, `Some(Err(_))`, and three-level Option patterns, in any arm order |
| `propagate/result`, `propagate/option` | postfix `?` unwraps `Ok` and `Some` and returns `Err` and `None` from the enclosing function |
| `propagate/order` | arguments evaluate left to right; effects before a `?` exit stay performed and nothing after it evaluates |
| `propagate/positions` | `?` in `if` conditions, `match` subjects, list and record elements, and nested blocks |
| `propagate/constructions` | `?` inside named-construction fields, entry-list keys and values, and on `url_path_segment` results in a documented helper; a failing `Unit` step skips the rest of the construction |
| `let-else/result`, `let-else/option` | let-else binds the payload of either variant for the rest of the body and otherwise returns its arm's value from the function; `_` and `None` bind nothing |
| `let-else/order` | the value evaluates before the arm; effects before the exit stay performed and nothing after it evaluates; consecutive let-else statements |
| `let-else/responses` | let-else arms that build `HttpResponse { ... }` with holes, after a `url_path_segment` step; nested patterns over a `Result<Option<String>, String>` whose arms are constructions with entry lists, holes, and a text block |
| `json/round-trip` | deterministic JSON conversion |
| `json/string-escapes` | every JSON string escape, surrogate pairs, and surrounding whitespace decode |
| `types/annotations` | parsed binding, parameter, return, and generic types |
| `stdlib/int-to-string` | `int_to_string` in base ten: zero, negative values, and both `Int` bounds |
| `stdlib/url-path-segment` | `url_path_segment`: RFC 3986 unreserved bytes kept, every other byte escaped in uppercase hex, and each refusal of the host path rule with its message, in order |
| `stdlib/url-query` | `url_query`: encoded `name=value` pairs joined by `&` from entry, header, and log-field lists; the empty list and empty names |
| `stdlib/string-byte-len`, `stdlib/string-is-empty` | UTF-8 byte length and emptiness |
| `stdlib/string-contains`, `stdlib/string-starts-with`, `stdlib/string-ends-with` | byte-wise searches: the empty part, a part longer than the text, multibyte text, and no case folding or normalization |
| `errors/division-zero` | checked division |
| `errors/overflow` | checked signed 64-bit arithmetic |
| `errors/json-invalid` | invalid JSON input |
| `errors/json-lone-surrogate` | an unpaired surrogate escape is invalid JSON |
| `errors/json-invalid-escape` | an unknown escape is invalid JSON |
| `errors/call-depth` | runaway recursion fails with `K4010` instead of aborting |

### Static cases (`conformance/check`)

Checking behavior, run with `krit check` (and refused identically by
`krit run`).

| Case | Contract |
|---|---|
| `valid/inference` | recursion, closures, empty lists, None, and effects infer together |
| `valid/returned-record-field-type`, `valid/returned-record-presence` | a record returned through a helper that reads one field keeps its other fields and their types for the caller |
| `syntax/missing-semicolon` | missing required syntax |
| `syntax/habit-word-line-break` | a bound habit word ending a line before a missing `;` stays an ordinary name (`K1002`) |
| `syntax/construction-in-condition` | a named construction written directly as an `if` condition is `K1001` with a note to add parentheses |
| `syntax/entry-list-mixed-*`, `syntax/empty-entry-list` | a list literal that mixes `key: value` entries with plain elements, and `[:]`, are `K1001` with one note |
| `syntax/unit-type` | `()` written as a type is `K1002` with a note to write `Unit` |
| `syntax/hole-*` | an interpolation hole that is not a name path (a call, an operator, a literal, a postfix `?`, whitespace, an empty or unclosed hole) is `K3015` at the hole with one note |
| `syntax/text-block-*` | a text block with text after its opening `"""`, a line indented less than its closing `"""`, or a closing `"""` inside a line is `K0004`; an unclosed one is `K0002`; each with one note |
| `name/*` | lexical undefined and duplicate name diagnostics, including duplicate record fields and a binding that reuses a standard-library name |
| `name/discard-*` | the discard name `_`: repeated bindings, reads warn with `K2003` without failing, warnings reported beside an error, an unbound `_` is undefined, and reading a pattern's `_` refers to the value at its position, beside an outer `_`, with `K2003` |
| `name/did-you-mean`, `name/null` | undefined names (`K2001`) that carry a did-you-mean or `None` note |
| `name/hole-undefined`, `name/discard-hole` | a hole's name resolves like any other: an undefined one is `K2001` with a did-you-mean note, and reading `_` warns (`K2003`) |
| `habit/*` | one case per construct from another language (`K1101`-`K1112`): `return`, loops, assignment, `struct`, `import`, `await`, `throw`, indexing, arrow functions, the conditional operator, `switch`, and `else { return ... }`; plus a conditional operator whose first branch does not parse and one after a postfix `?` |
| `syntax/question-before-operand` | a `?` followed on its line by an operand, without a `:`, is an unexpected token (`K1001`) with one note |
| `propagate/*` | postfix `?` where it cannot return (`K3012`: top level, anonymous function, webhook, missing or non-`Result`/`Option` return type), with another family or error type (`K3013`), and with an opaque `Secret` error type (`K3009`) |
| `valid/propagate-queue` | a queue entrypoint returns its failures with `?` and builds |
| `let-else/*` | let-else at the top level, in a nested block or branch, or in an anonymous function, with patterns that are not the two variants of one family, with a nested pattern, or with a value ending in `}`, a named construction included (`K3014`); a missing `else` arm (`K1002`) or a block arm (`K1003`), each with one note; an arm of another type (`K3001`), a value of another family (`K3005`), a binder declared twice (`K2002`), and a binder read in the arm (`K2001`) |
| `valid/let-else-queue` | a queue entrypoint binds an object read and returns early with let-else, including a `None` pattern, and builds |
| `valid/habit-words-as-names` | habit words stay ordinary identifiers outside foreign constructs |
| `type/*` | stable static operator, condition, field, call, arity, annotation, match, comparison, and JSON-encoding errors, including an infinite type between two solved bindings, a method call on a string (`K3011`), and a `url_query` argument that is not a list of name/value records |
| `lint/*` | `K9013`: an unencoded hole or `+` operand of an `HttpRequest` path or query warns, in a named construction or a record literal, without failing the check or the build; pieces from `url_path_segment`, `url_query`, or `int_to_string` (direct or bound), `url_path_segment` results taken apart with `?` or let-else, literal paths, and forwarded values do not warn, while a `?` on another function's result, a let-else `Err` binder, and a nested `Ok(Some(_))` binder do |
| `type/construction-*` | named construction with an unknown, repeated, or missing required field (`K3016`, one note) or a value of the wrong type (`K3001`) |
| `valid/named-construction` | named constructions flow wherever contract records do: helper results, list annotations, parenthesized conditions, and equality with records |
| `type/entry-list-*` | entry-list keys and values must be `String`, and an entry list is not a list of another element type (`K3001`) |
| `type/hole-*` | a hole whose value is an `Int` (named or a field) or an `Option` is `K3015` at the hole with one note |
| `valid/entry-lists-unit-docs` | entry lists flow into `List<HttpHeader>` and `List<LogField>`, constructions, and equality with records; the unit value; documented helpers |
| `valid/doc-comments` | doc comments attach to declarations and entrypoints; banners, runs interrupted by an ordinary comment, and dangling `///` lines are ordinary comments |
| `match/*` | match arms with one family per position, no duplicate, unreachable, or overlapping arm (`K1003`, each with one note), a malformed list pattern (`K1003`), no top-level catch-all `_` or name (`K3005`, one note), patterns of the payload's type (`K3005`), and exhaustiveness with every missing pattern listed (`K3005`, one note) |
| `valid/nested-patterns` | nested patterns with `_` payloads and list patterns, in any arm order, check |
| `valid/entry-key-propagate` | an entry key that ends in a postfix `?`, also before a call, is an entry, not a conditional operator |
| `webhook/*` | top-level uniqueness and exact typed webhook signatures |
| `capability/*` | literal-resource host capability contracts |
| `capability/interpolated-resource`, `capability/text-block-resource` | an interpolated string or a text block is never a resource literal (`K3008`) |
| `nesting/*` | syntactic, structural, and inferred-type nesting limits (`K1005`), including deep list literals, chains hidden by parentheses, types that grow after they are bound, and types that grow when two solved types unify |
| `jobs/*` | typed queue/schedule entrypoint uniqueness, signatures, and literal queue/schedule/bucket resources |
| `target/*` | entrypoint files report every build-target diagnostic at check time, after any front-end warnings; unused helpers are eliminated first; String routing and composition build; JSON string decoding builds; a webhook helper chaining config, secret, and HTTP calls with `?` builds; a webhook binding config, secret, HTTP, and AI results with let-else in its own body builds, and so does one committing or rolling back a database transaction; a webhook deciding `state_get`'s `Result<Option<String>, String>` with nested patterns builds, and so does one reading a pattern's `_` (with `K2003`); a webhook built from named constructions builds; a webhook writing headers and log fields as entry lists and returning `Ok(())` from a helper builds; a webhook composing its model prompt from a text block with holes builds; a webhook calling every standard-library function builds; a webhook mixing let-else, `?` inside constructions, nested patterns over `state_get`, entry lists, holes, a text block, and doc comments builds |
| `secret/*` | opaque secret non-disclosure restrictions |
| `valid/webhook-contract` | transitive typed webhook/config/secret facts |
| `valid/queue-consumer` | typed `QueueJob` delivery and bounded object write |
| `valid/schedule-handler` | typed `ScheduleEvent` trigger facts and outcome |

### Format cases (`conformance/format`)

Canonical layout, run with `krit fmt`.

| Case | Contract |
|---|---|
| `comments` | exact standalone and end-of-line comment preservation |
| `edition-2026` | canonical layout for all currently implemented syntax families |
| `webhook` | canonical typed webhook and ordered duplicate-header layout |
| `propagate` | postfix `?` stays attached to its operand and chains like a call |
| `let-else` | a let-else stays on one line when it fits, otherwise breaks after `else` with its arm indented one level; a block or comment in the arm always breaks |
| `patterns` | nested and list patterns stay compact, keep their arm order, and nested list patterns are never list literals |
| `named-construction` | canonical named construction: one line when it fits, one field per line otherwise, and punning exactly when a value is the field's own name |
| `entry-lists` | canonical entry lists: one line when they fit, one entry per line with a trailing comma otherwise |
| `unit-literal` | the unit value is written `()` |
| `interpolation` | interpolated strings keep their holes exactly and write their text as any string |
| `text-blocks` | text blocks keep their content and are re-indented: at the opening `"""` when it starts its line, one level deeper than its line otherwise |
| `doc-comments` | attached doc comments get exactly one space after `///`; banners, end-of-line and dangling `///` comments keep their text |
| `cross-features` | documented helpers with `?`, constructions holding `?` and entry lists, let-else arms with punned fields and text blocks, nested patterns with holes, and `()` together are canonical and idempotent |

### Package cases (`conformance/package`)

Manifest validation, run with `krit package check`.

| Case | Contract |
|---|---|
| `dependencies/declared` | a declared version dependency fails closed with `K6003` |
| `dependencies/path-source` | a declared source-table dependency fails closed with `K6003` |
| `dependencies/empty-table` | an empty `[dependencies]` table is accepted |
| `config/*` | configuration keys may contain `_` after their first character; a configuration key that starts with `_`, and a secret name containing `_`, are `K6001` |

### Build compatibility cases (`conformance/build/compat`)

Frozen artifacts from released toolchains, run with `krit sandbox` or
`krit invoke`.

| Case | Contract |
|---|---|
| `0.2.0/factorial` | a 0.2.0-built policy-1 module artifact runs in the sandbox |
| `0.2.0/stateful-webhook` | a 0.2.0-built policy-2 durable-state webhook artifact invokes |

## Adding a case

A conformance change must cite the normative specification section in its pull
request. Keep each program minimal and test one semantic distinction. Unit and
regression tests belong in Rust crates rather than this directory when they do
not define portable behavior.
