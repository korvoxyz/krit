# Krit diagnostic contract

**Status:** Normative  
**Schema:** 1

Diagnostics are part of Krit's human and AI tooling interface. Their codes and
structured fields are stable within an edition; prose may improve without
breaking compatibility.

## Severity

Every diagnostic a command reports has a severity:

| Severity | Meaning |
|---|---|
| `error` | The program or input is rejected and the command fails |
| `warning` | The program is accepted; the diagnostic reports something the author should change |

A warning never changes a command's outcome or exit status: a program whose
only diagnostics are warnings checks, runs, builds, and is explained exactly as
it would without them. `krit check`, `krit run`, `krit build`, and
`krit explain` report every warning that analysis finds, ordered by source
span and then code, on standard error in the selected diagnostic format. When
analysis also rejects the program, the warnings found before the error are
reported first and the error last. The language server publishes warnings with
LSP `DiagnosticSeverity.Warning` and errors with `DiagnosticSeverity.Error`,
and `krit/compilerFacts` diagnostics carry the same `severity` strings as the
JSON Lines format. A published LSP diagnostic's message is the diagnostic's
message followed by each of its notes on its own line as `help: <note>`, as in
the human format; `krit/compilerFacts` diagnostics carry the message and a
`notes` array as the JSON Lines format does. Both are bounded as described
next.

The language server bounds what it publishes for one document, so that the
diagnostics of a document within its size limit fit in a protocol frame. It
keeps the first 100 warnings in the order above, and stops before the first
warning that would take their combined published message text, notes
included, past 64 KiB. When it leaves warnings out, it publishes one `K8003`
notice after the warnings it kept: the notice sits at the span of the first
warning left out, has LSP `DiagnosticSeverity.Information`, and states how
many warnings were left out. The error that rejects the document, if any, is
always published, last; its message is cut to 64 KiB and each of its notes to
4 KiB, at character boundaries, ending with a marker saying the text was cut.
`krit/compilerFacts` returns the same list, with severity `info` for the
notice. Should a document's diagnostics still not fit in a protocol frame, the
server publishes a single `K8002` error for the document in their place and
keeps running. The commands above bound neither warnings nor messages.

The schema reserves `info` for informational diagnostics. Only the language
server's `K8003` notice uses it; commands never report an `info` diagnostic on
standard error.

## Human format

The default format is:

```text
path/to/file.krit:4:11: error[K2001]: undefined name `totl`
help: did you mean `total`?
path/to/file.krit:7:9: warning[K2003]: `_` is a discard; reading it will be an error in edition 2027
```

The severity word comes first, followed by the code in brackets; warnings use
the same location style as errors. Each of the diagnostic's notes follows on
its own line, in order, as `help: <note>`. An implementation may add source
excerpts after these lines. It must not expose a host-language stack trace
unless explicitly requested with a developer option.

Paths should be relative to the package root. Standalone files use the path
provided by the user. Command-line expressions use `<command-line>` and the
REPL uses `<repl>`.

## JSON Lines format

`--diagnostic-format json` emits one compact JSON object per line to standard
error:

```json
{"schema":1,"severity":"error","code":"K2001","message":"undefined name `total`","file":"src/main.krit","span":{"start":{"line":4,"column":11,"byte":57},"end":{"line":4,"column":16,"byte":62}},"labels":[],"notes":[]}
```

Required top-level fields:

| Field | Type | Meaning |
|---|---|---|
| `schema` | integer | Diagnostic schema version, currently `1` |
| `severity` | string | `error`, `warning`, or `info` |
| `code` | string | Stable Krit diagnostic code |
| `message` | string | Concise UTF-8 explanation |
| `file` | string | Package-relative or synthetic source name |
| `span` | object or null | Primary source range |
| `labels` | array | Additional labeled source ranges |
| `notes` | array of strings | One-line repair hints, in order, that have no precise source range (see [Fix hints](#fix-hints)) |

Line and column values are one-based. Byte offsets are zero-based UTF-8 byte
offsets. End positions are exclusive.

Unknown fields may be added within schema 1. Consumers must ignore unknown
fields and must reject an unknown schema number.

## Code ranges

| Range | Category |
|---|---|
| `K0001`-`K0999` | Source decoding and lexical errors |
| `K1000`-`K1999` | Syntax errors |
| `K2000`-`K2999` | Name and module resolution |
| `K3000`-`K3999` | Static type and effect errors |
| `K4000`-`K4999` | Runtime errors |
| `K5000`-`K5999` | Capability violations |
| `K6000`-`K6999` | Package, manifest, and lockfile errors |
| `K7000`-`K7999` | Build and cache errors |
| `K8000`-`K8999` | Formatter and tooling errors |
| `K9000`-`K9999` | Lints: warnings by default |

The table below gives every code's severity: codes outside the lint range are
errors unless it says otherwise (`K2003` is a warning and `K8003` is the
language server's `info` notice), and that severity is part of the code's
contract within an edition. Lints report readability, safety, and authority hazards in programs
that are otherwise valid. A lint is a warning by default; a project profile or
command mode may configure a different severity for a specific lint, and that
configuration is then part of the profile's or mode's contract. No profile or
mode configures lint severity yet, so every lint is currently a warning.

## Codes

Every code the implementation can report appears exactly once below. The
severity is part of each code's contract within an edition.

| Code | Severity | Meaning |
|---|---|---|
| `K0001` | error | Invalid source character, malformed integer (repeated or trailing `_` separator, or digits touching a letter) |
| `K0002` | error | Unterminated string or text block |
| `K0003` | error | Invalid string escape, including `\` at the end of a text-block line |
| `K0004` | error | Text block layout is invalid: text after the opening `"""` on its line, a content line that does not begin with the closing delimiter's indentation, or `"""` inside a content line |
| `K1001` | error | Unexpected token, including a `?` followed on its line by an operand that does not form a conditional operator (`K1110`), a named construction written directly as an `if` condition or `match` subject, a list literal that mixes `key: value` entries with plain elements, and `[:]` |
| `K1002` | error | Expected token not found |
| `K1003` | error | Invalid match pattern or arm list (including a `match` without arms and a block after a let-else's `else`), or `match` arms that mix families at one position, repeat an arm, or overlap (including an unreachable arm) |
| `K1004` | error | A `webhook`, `queue`, or `schedule` entrypoint declaration inside a block: entrypoints are top-level items |
| `K1005` | error | Nesting depth limit exceeded: syntax nests more than 256 levels, nesting plus the `else if` continuations, operators, and call, field-access, or `?` links above a point exceeds 4,096, or an inferred type would nest more than 256 levels |
| `K1101` | error | `return`: a block's value is its final expression |
| `K1102` | error | `for`, `while`, `loop`, or `do` loop |
| `K1103` | error | Assignment, compound assignment (`+=`, `-=`, `*=`, `/=`, `%=`), `++` or `--`, `mut`, `var`, or `const` |
| `K1104` | error | `struct`, `class`, `impl`, `trait`, or `interface` declaration |
| `K1105` | error | `import`, `use`, `require`, or `from`: modules are not available |
| `K1106` | error | `async` or `await` |
| `K1107` | error | `try`, `catch`, `throw`, `raise`, `except`, or `finally` |
| `K1108` | error | Indexing `a[i]` |
| `K1109` | error | Arrow function `(x) => e` or `x => e` |
| `K1110` | error | Conditional operator `c ? a : b` |
| `K1111` | error | `switch`, `case`, or `default` |
| `K1112` | error | `return` opening an `else` block: an early exit |
| `K2001` | error | Undefined name; a note suggests the closest visible name, `None` for a spelling of null, or the fix for a construct from another language |
| `K2002` | error | Duplicate declaration, parameter, record field, record type field, or list-pattern binding (the discard name `_` is exempt); a `let`, function, or parameter that reuses a built-in name; or a second entrypoint in one module |
| `K2003` | warning | The discard name `_` is read as a value, including a pattern's `_`; this becomes an error in edition 2027 |
| `K3001` | error | Static type mismatch or invalid typed operator |
| `K3002` | error | Statically known non-function call target |
| `K3003` | error | Static function argument count mismatch |
| `K3004` | error | Invalid record field access |
| `K3005` | error | Match subject or pattern of the wrong type, a catch-all `match` arm, or a non-exhaustive `match` (the message lists every missing pattern) |
| `K3006` | error | Type is not comparable or JSON-encodable |
| `K3007` | error | Entrypoint signature is not exactly `(HttpRequest) -> HttpResponse` for a webhook, or `(QueueJob) -> Result<String, String>` or `(ScheduleEvent) -> Result<String, String>` for a queue or schedule handler |
| `K3008` | error | Host capability resource or entrypoint name is not a direct, valid canonical string literal |
| `K3009` | error | Opaque secret is used by a revealing or structural operation |
| `K3010` | error | Opaque database transaction handle is used outside a database operation |
| `K3011` | error | Method call `value.name(...)` on a value that has no field `name`: Krit values have no methods |
| `K3012` | error | Postfix `?` where it cannot return: at the top level, in an anonymous function or a webhook entrypoint, or in a named function whose declared return type is missing or is neither a `Result` nor an `Option` |
| `K3013` | error | Postfix `?` whose operand's family or error type differs from the enclosing function's declared `Result<_, E>` or `Option<_>` |
| `K3014` | error | let-else that is not a direct statement of a named function's body, whose two patterns are not the two variants of one family, whose pattern nests another pattern, or whose value ends in `}` |
| `K3015` | error | Invalid interpolation hole: its content is not a name followed by field accesses (reported while the source is read), or its value is not a `String` |
| `K3016` | error | Named construction of a contract record writes a field the contract does not have, writes a field more than once, or omits a required field |
| `K4001` | error | Generic guest trap, wrong runtime value kind, or HTTP request/response the host refuses (including a request path that breaks the rule in `WEBHOOK-CONTRACTS.md`, "Request paths and queries") |
| `K4002` | error | Calling a non-function in direct evaluation (checking reports `K3002` first) |
| `K4003` | error | Function argument count mismatch in direct evaluation (checking reports `K3003` first) |
| `K4004` | error | Division or remainder by zero |
| `K4005` | error | Integer overflow, or an integer literal outside the signed 64-bit range |
| `K4006` | error | Function comparison in direct evaluation (checking reports `K3006` first) |
| `K4007` | error | Output operation failed |
| `K4008` | error | Value cannot be encoded as JSON in direct evaluation (checking reports `K3006` first) |
| `K4009` | error | Invalid or unsupported JSON input, in direct evaluation or a component's `json_decode` into `String` |
| `K4010` | error | Direct-evaluation depth limit exceeded: more than 10,000 active function calls, 100,000 nested evaluations, or 10,000 levels of list, record, and variant value nesting |
| `K5001` | error | Required capability not granted by the manifest, or a host configuration that a non-Unix host cannot protect (secrets, durable state, jobs, or databases) |
| `K5002` | error | Artifact import/world does not match the authorized host interface |
| `K5003` | error | `krit run` refuses an entrypoint or host operation: direct source execution has no host other than standard output |
| `K5006` | error | Secret would be injected into an origin its manifest binding does not list |
| `K5101` | error | Wasm fuel budget exhausted |
| `K5102` | error | Wasm wall-clock deadline exceeded |
| `K5103` | error | Wasm memory, table, instance, stack, or host resource limit exceeded |
| `K5104` | error | Host-call limit exceeded |
| `K5105` | error | Buffered output-byte limit exceeded |
| `K5106` | error | Embedding cancellation requested before guest execution |
| `K5201` | error | Durable state database, schema, corruption, migration, or I/O failure |
| `K5202` | error | Durable state transaction conflict, configured state/queue/schedule/object limit exceeded, or an unrepresentable delivery instant |
| `K5203` | error | Replay operation is unsafe, in progress, expired incorrectly, or conflicts with recorded input |
| `K5204` | error | Durable inbound idempotency reservation or completion failed |
| `K5205` | error | Durable delivery lease, acknowledgement, or outcome detail is invalid |
| `K5301` | error | Application database file, schema, catalog, or connection is invalid |
| `K5302` | error | Database transaction lifecycle, handle, or lock-safety rule was violated |
| `K5303` | error | Configured database count, byte, row, time, or busy bound was exceeded |
| `K5401` | error | Cache namespace is not configured, or refuses the requested operation |
| `K5402` | error | Configured cache key, value, entry, byte, or time-to-live bound was exceeded |
| `K5403` | error | Cache backend is unavailable |
| `K5404` | error | Search connector is missing, misconfigured, or returned invalid output |
| `K6001` | error | Invalid package manifest |
| `K6003` | error | Manifest declares dependencies, but dependency resolution is not supported |
| `K7001` | error | Residual or unresolved type requires specialization before layout |
| `K7002` | error | Core semantics or concrete layout is unsupported by the artifact target |
| `K7003` | error | WebAssembly emission, validation, metadata attachment, artifact or input-file loading, host configuration, or output failed |
| `K7004` | error | Artifact metadata, byte size, or content digest mismatch, or adjacent metadata outside the runtime's supported compatibility sets or toolchain window |
| `K8001` | error | Source is not canonically formatted in formatter check mode |
| `K8002` | error | Language-server input, open-document state, or a document's published diagnostics exceed a bounded limit |
| `K8003` | info | The language server left a document's remaining warnings out of its published diagnostics at the bounded limit |
| `K8101` | error | Authoring assistance is disabled or provider configuration is invalid |
| `K8102` | error | Authoring context, range, path, ignore, redaction, or privacy validation failed |
| `K8103` | error | Authoring provider transport or strict response decoding failed |
| `K8104` | error | Authoring request, response, edit, proposal, digest, or stale precondition is invalid |
| `K8105` | error | Proposed source failed canonical formatting or compiler checking |
| `K8106` | error | Required authority expansion lacks exact review approval or a manifest grant |
| `K8107` | error | Proposal or accepted-source atomic I/O failed |
| `K9013` | warning | A piece of an `HttpRequest` `path` or `query` built by interpolation or `+` is not a literal, a direct call of `url_path_segment`, `url_query`, or `int_to_string`, a postfix `?` on a `url_path_segment` result, or a name bound to such a result |

### Reserved codes

These codes are allocated but no 0.3 command reports them. Codes planned for
later releases are allocated in `docs/roadmap.md` (section 9.2) and are not
listed until they ship.

| Code | Reserved for |
|---|---|
| `K6002` | Lockfile is stale or invalid (lockfiles arrive with dependency resolution, `PACKAGES.md`) |

## Build-target diagnostics

`K7001` and `K7002` are reported by `krit build` and by `krit check` when it
runs a target check (`--target component|module`, or by default for files
that declare a webhook, queue, or schedule entrypoint). Both commands run the
same backend pipeline and report the same set. The backend runs only on a
program that passes front-end checking, so its diagnostics follow any
warnings that analysis reported (see [Severity](#severity)). Unlike front-end
analysis, which stops at its first error, a failing target check reports every
residual type and unsupported construct in the code that the selected
entrypoint can reach. Before that support check, the backend refuses Core
nested deeper than its compilers accept with a single `K1005`
(`WASM-SANDBOX.md`, "Support checking"). Each diagnostic caused by a source
construct carries the span of the originating operation (or of the enclosing
parameter, block, or declaration) and is never reported at a placeholder
`1:1` position. They are emitted in source order (span start, span end, then
code) with at most one diagnostic per code and source position. `K7002`
messages name the layout policy of the program kind: "WebAssembly policy 1"
for module programs and "the bounded webhook ABI (WebAssembly policy 2)" for
entrypoint programs. Diagnostics about a file whose text is unavailable, such
as an invalid manifest (`K6001`), use the file's first position.

## Fix hints

A diagnostic's notes are one-line repair hints. Their wording is prose and
may improve, but which diagnostics carry one is part of each code's contract:

- `K1101`-`K1112`, `K3011`, `K3012`, `K3013`, and `K3014` always carry
  exactly one note with the fix.
- `K2001` carries one note when one of the rules below applies.
- `K3002` for a call of `None` carries one note: `None` is a value, written
  without parentheses.
- `K1001` for a `?` followed on its line by an operand carries one note:
  postfix `?` takes no operand, and `if` chooses between two values.
- `K1002` for a `let` with a variant pattern and no `else` arm, and `K1003`
  for a block after a let-else's `else`, each carry one note showing the
  let-else arm to write.
- `K1003` for a token that cannot start a match pattern carries one note
  listing the pattern forms; `K1003` for `match` arms that mix families,
  repeat, or overlap, and `K3005` for a catch-all arm or a non-exhaustive
  `match`, each carry one note with the fix (see [Matches](#matches)).
- `K1001` for a named construction written directly as an `if` condition or
  a `match` subject carries one note: wrap the construction in parentheses.
- `K1001` for a list literal that mixes `key: value` entries with plain
  elements carries one note: write every element as an entry, or none. It is
  reported at the first element written in the other form than the list's
  first element (an entry's key and `:`, or a plain element).
- `K1001` for `[:]` carries one note: the empty list is `[]`, for entry lists
  too.
- `K1002` for `()` written as a type (`fn f() -> () { ... }`) carries one
  note: the unit type is spelled `Unit`, and `()` is the unit value.
- `K0004` always carries exactly one note: put the text on the next line,
  indent the line at least as far as the closing `"""`, or move the `"""` to
  a line of its own (or escape it as `\"""`).
- `K0002` for an unterminated text block carries one note: close it with
  `"""` on a line of its own. An unterminated one-line string carries none.
  A text block that no line closes but that has `"""` inside a content line
  is reported as `K0004` at that `"""`, which most likely meant to close it.
- `K3015` always carries exactly one note. It is reported at the hole, from
  its `\` to its `)` (at the `\(` alone for a hole whose line ends before its
  `)`). A hole that is not a name path is reported while the source is read,
  before any syntax error later in the file, with a note to bind the value to
  a name first (or to remove the whitespace, or to name a value); a hole whose
  value is not a `String` names the hole's path and type, and its note says
  how to obtain a `String`: for an `Int`, a `let` that converts it with
  `int_to_string` (`LANGUAGE.md` section 12.1).

```text
main.krit:2:16: error[K3015]: interpolation hole `count` has type `Int`, but a hole must be a `String`
help: convert it first, as in `let count_text = int_to_string(count);`, then write `\(count_text)`
```

- `K9013` always carries exactly one note: for a path, to encode text with
  `url_path_segment` and numbers with `int_to_string`, binding each result
  to a name; for a query, to build it with `url_query`.
- `K3016` always carries exactly one note. For an unknown field it lists the
  contract's fields, preceded by `did you mean `name`?` when one of them is
  close by the rule for undefined names below; for a repeated field it lists
  the contract's fields; for missing fields it names the contract's required
  fields and the default of each other field:

```text
main.krit:1:15: error[K3016]: `HttpRequest { ... }` is missing required field `path`
help: `HttpRequest` requires `method` and `path`; `query` defaults to `""`, `headers` defaults to `[]`, and `body` defaults to `""`
```

### Constructs from other languages

Models and people write constructs from other languages out of habit. Krit
reports each with its own syntax code, under the rules of `LANGUAGE.md`
section 4.2, instead of a generic `K1001` or `K1002`:

| Code | Example | Fix in the note |
|---|---|---|
| `K1101` | `return total;` | a block's value is its final expression; for early exit, use postfix `?` on a `Result` or `Option`, or let-else |
| `K1102` | `for item in items { ... }` | write a recursive function over `[]` and `[head, ..tail]`; list functions arrive in Krit 0.4 |
| `K1103` | `count = count + 1;`, `x += 1`, `let mut x` | values are immutable; bind each new value with `let` under a new name |
| `K1104` | `struct Point { x: Int }` | use a `record { field: value }` value and plain functions |
| `K1105` | `import json;` | modules are not available yet; built-in functions need no import |
| `K1106` | `await fetch()` | host calls finish before they return; call them directly |
| `K1107` | `throw "empty";`, `try { ... }` | return `Ok(value)` or `Err(error)`, then handle the `Result` with `match` or pass its error on with postfix `?` |
| `K1108` | `items[0]` | match the list with `[]` and `[head, ..tail]`; `list_get` arrives in Krit 0.4 |
| `K1109` | `(x) => x + 1` | write `fn(x) { x + 1 }` |
| `K1110` | `ready ? 1 : 2` | `if` is an expression: `if ready { 1 } else { 2 }` |
| `K1111` | `switch code { case 200: ... }` | use `match`, or an `if ... else if ... else` chain |
| `K1112` | `else { return 0; }` | exit early with let-else, whose arm names the other variant: `let Some(x) = value else None => fallback;` |

```text
main.krit:2:5: error[K1101]: `return` is not Krit syntax
help: a block's value is its final expression, written without `return` or `;`; for early exit, use postfix `?` on a `Result` or `Option`, or let-else: `let Some(x) = value else None => fallback;`
```

### Undefined names

`K2001` carries the first of these notes that applies:

1. For `null`, `NULL`, `Null`, `nil`, `Nil`, `NIL`, `undefined`, or
   `nullptr`: write `None` for an absent `Option` value.
2. For `return`, `await`, `throw`, `raise`, `require`, or `import`, which
   parse as names where they could still be one (`return;`, `return -1;`,
   `require("json")`): the fix of the construct the word comes from.
3. `did you mean `name`?`, naming the closest visible name: a binding in
   scope at the reference, a built-in name, `true`, or `false`. A name
   qualifies when it is at most two edits from the undefined name (inserting,
   deleting, or substituting one character, or swapping two adjacent ones) and
   fewer edits than the undefined name has characters. Fewer edits win, then
   the bytewise smallest name, so the suggestion never depends on declaration
   order.

```text
main.krit:2:9: error[K2001]: undefined name `null`
help: Krit has no `null`; write `None` for an absent `Option` value
```

### Matches

The arms of a `match` name every outcome exactly once (`LANGUAGE.md`
section 11). Checking reports, in this order: within the arms that are not
catch-alls and in arm order, a pattern of another family than an earlier
arm's at the same position, at that pattern (`K1003`), and an arm that
repeats or overlaps an earlier one, at its pattern (`K1003`); then a
top-level catch-all arm, a name or `_`, at its pattern (`K3005`); then a
pattern whose family is not the type at its position (`K3005`); and last a
non-exhaustive match, at the match (`K3005`). A missing pattern is written
exactly as it would be in source, with `_` for every part no arm
distinguishes, in a fixed order: depth first, with `Some` before `None`,
`Ok` before `Err`, and `[]` before `[head, ..tail]` at each position.

| Diagnostic | Note |
|---|---|
| non-exhaustive match | the arm to add for each missing pattern |
| catch-all arm | the arms to write in its place, or that the other arms already name every outcome |
| overlapping arm (it also matches what an earlier, narrower arm matches) | the arms for the outcomes it would catch |
| duplicate or unreachable arm | remove it |
| mixed families | the families one position takes |

```text
main.krit:2:5: error[K3005]: non-exhaustive match; missing: `Ok(None)`
help: add an arm for each missing pattern: `Ok(None) => ...`
main.krit:4:9: error[K3005]: a match arm cannot match every value: `_` would stand for outcomes it does not name
help: name every outcome: replace `_ => ...` with `Ok(None) => ...` and `Err(_) => ...`
main.krit:4:9: error[K1003]: match arm `Ok(other)` overlaps the earlier arm `Ok(Some(value))`: arms may not overlap
help: name each remaining outcome instead: replace `Ok(other) => ...` with `Ok(None) => ...`
```

These rules treat `_` in a pattern as a wildcard that names no outcome:
`Err(_)` names `Err`, and `_ =>` is a catch-all. In edition 2026 a pattern's
`_` still binds the value at its position as a discard binding, so reading
`_` in its arm (or after a let-else pattern `_`) is warning `K2003` and reads
that value, exactly as reading a `let` or parameter `_` does.

### Method calls

Krit values have no methods. `value.name(arguments)` calls the function
stored in field `name` of the record `value`; when `value` has no field
`name`, because it is not a record or is a record without that field,
checking reports `K3011` instead of the field-access error `K3004`. A field
access that is not called still reports `K3004`. The note names the
standard-library function that does the job when there is one:

| Receiver | Methods | Note names |
|---|---|---|
| `String` | `len`, `length`, `size`, `count`, `byte_len` | `string_byte_len(s)` (counts UTF-8 bytes) |
| `String` | `is_empty`, `isEmpty`, `empty` | `string_is_empty(s)` |
| `String` | `contains`, `includes` | `string_contains(s, part)` |
| `String` | `starts_with`, `startsWith`, `startswith`, `hasPrefix` | `string_starts_with(s, prefix)` |
| `String` | `ends_with`, `endsWith`, `endswith`, `hasSuffix` | `string_ends_with(s, suffix)` |
| `String` | any other | the five string functions |
| `Int` | `to_string`, `toString`, `string`, `str` | `int_to_string(n)` |
| `List` | any | list patterns; list functions arrive in Krit 0.4 |
| `Option`, `Result` | any | `match`, postfix `?`, or let-else |
| other | any | passing the value to a function |

The note spells the receiver as written when it is a name or a short field
path (`string_byte_len(request.body)`), and as `value` otherwise. The string
and integer functions it names are the standard library (`LANGUAGE.md`
section 12.1).

```text
main.krit:2:9: error[K3011]: cannot call `.len()` on a value of type `String`: Krit values have no methods
help: call `string_byte_len(name)` instead; it counts UTF-8 bytes
```

### Request path and query lints

`K9013` reports text that reaches the path or query of an outbound
`HttpRequest` without percent-encoding, where untrusted text could add `/`,
`..`, `?`, `#`, or an escape and so address a different resource than the
program meant. In a named construction or a record literal typed as
`HttpRequest`, a `path` or `query` value written as an interpolated string or
a `+` chain is split into pieces: each hole and each operand. A piece that is
a literal, a direct call of `url_path_segment`, `url_query`, or
`int_to_string`, a postfix `?` on a direct `url_path_segment` call or on a
name bound to one, or a name bound to the result of one (by `let x = <call>;`,
by `let x = <call or name>?;`, or as the `Ok` binder of a match arm or a
let-else on a direct `url_path_segment` call or a name bound to one) is
encoded; every other piece is reported at its hole or operand. A literal-only path, and a value that is not split, such as
`request.path` forwarded unchanged, never warn (`TYPES-AND-EFFECTS.md`,
Lints).

```text
main.krit:2:51: warning[K9013]: this value is inserted into an `HttpRequest` path without percent-encoding
help: encode untrusted text with `url_path_segment` and numbers with `int_to_string`, binding each result to a name, so the value cannot add `/`, `..`, `?`, or `#` to the path
```

### Error propagation

`K3012` and `K3013` are reported at the `?` token (`LANGUAGE.md` section
8.2). The `K3012` note names what to change for the place the `?` is in: at
the top level or in an anonymous function, `match` the value or move the code
into a named function that returns `Result` or `Option`; in a webhook, bind
each fallible step with let-else, or move the steps into a helper that
returns `Result<HttpResponse, String>` and use `?` there; in a named
function, declare a `Result` or `Option` return type. The `K3013` note
explains that `?` passes the failure on unchanged, so the family and the
error type must match the declared return type, and suggests `match` to
convert.

```text
main.krit:2:45: error[K3012]: `?` is not allowed in a webhook entrypoint, which returns `HttpResponse`
help: bind each fallible step with let-else here, as in `let Ok(value) = step else Err(error) => response;`, or move the steps into a helper `fn` that returns `Result<HttpResponse, String>` and use `?` there
```

`K3014` (`LANGUAGE.md` section 8.3) is reported at the let-else's `let` and
pattern when it is not a direct statement of a named function's body, with a
note to `match` the value there instead (or, in an anonymous function, to
move the code into a named `fn`); at its arm when the two patterns are not
the two variants of one family, with a note naming the arm to write; at a
pattern nested in either pattern's payload, such as the `Some` of
`let Ok(Some(x)) = ...`, with a note to bind the payload and decide it with
another let-else or a `match`; and at its value when the value ends in `}`,
with a note to bind the value first or wrap it in parentheses.

```text
main.krit:3:9: error[K3014]: let-else must be a statement of the function body itself, not of a nested block
help: let-else returns from the enclosing function, so it may only appear at the top level of a named function's body; handle the value with `match` here
```

## Exit statuses

| Status | Meaning |
|---|---|
| `0` | Command completed successfully |
| `1` | Source, compile, runtime, test, or conformance failure |
| `2` | Invalid CLI usage |
| `3` | Package resolution, lockfile, registry, or build-plan failure |
| `4` | Capability, unavailable-host, import, or secret-binding authorization denial (`K5001`, `K5002`, `K5003`, or `K5006`); artifact-aware permission reports are still printed |
| `101` | Internal compiler error |

Warnings never affect the exit status.

An internal compiler error is always a Krit bug. It must include a stable crash
identifier but must not include source content, environment variables, or
secrets in telemetry without explicit consent.

`krit run` checks source exactly as `krit check` does before evaluating it,
except that it never runs a build-target check (see
[Build-target diagnostics](#build-target-diagnostics)): direct execution builds
no artifact. For a program that fails checking it emits check's diagnostic and
exits with check's status, so it reports runtime codes (`K4000`-`K4999`) only
for programs that pass checking.

## Determinism and privacy

Diagnostics must not include:

- memory addresses
- nondeterministic hash ordering
- absolute cache paths
- secret values
- environment variable values
- capability contents
- model prompts or responses unless explicitly requested

The same failing program should produce the same diagnostic code and primary
span across supported platforms.
