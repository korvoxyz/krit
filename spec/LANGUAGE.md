# Krit 0.2 language specification

**Status:** Normative  
**Edition:** 2026

## 1. Scope

This document defines the syntax and runtime behavior required by the initial
Rust implementation. The bootstrap static checker is specified in
`TYPES-AND-EFFECTS.md`, and the evaluator-independent typed Core form is
described in `docs/technical-design.md`. This milestone also defines
typed webhook entrypoints, configuration reads, opaque secret acquisition,
exact-origin outbound HTTP, provider-neutral AI invocation, and structured
logging. The direct evaluator remains intentionally hostless; these operations
execute only through the bounded Component Model runtime. General component
generation remains narrower than the full dynamic language.

The historical Racket S-expression syntax is not accepted by Krit 0.2.

## 2. Source text

A source file is UTF-8 and conventionally ends in `.krit`.

- Keywords and identifiers are case-sensitive.
- Version 0.2 identifiers use ASCII letters, digits, and `_`.
- An identifier starts with a letter or `_`.
- `//` begins a comment ending at the next line break.
- Whitespace has no semantic meaning except token separation.
- Tabs are permitted in input; the canonical formatter emits spaces.
- Source positions are one-based Unicode scalar-value line and column numbers.

Reserved keywords:

```text
else false fn if let match record true webhook
```

Reserved built-in names:

```text
Err None Ok Some ai_invoke config_string http_request json_decode json_encode log_error log_info print println secret
```

A binding cannot use a keyword or reserved built-in name.

## 3. Lexical grammar

The grammar uses EBNF. `*` means zero or more, `?` means optional, and quoted
text is literal.

```text
identifier  = (letter | "_"), (letter | digit | "_")* ;
integer     = digit, (digit | "_")* ;
string      = '"', string-character*, '"' ;
comment     = "//", non-line-break* ;
```

Integer separators cannot be leading, trailing, or repeated. Integers are
signed through unary `-`; the sign is not part of the token.

Strings support these escapes:

```text
\"  \\  \n  \r  \t  \0  \u{hex-scalar}
```

Other escapes and invalid Unicode scalar values are lexical errors.

## 4. Syntactic grammar

```text
program         = item* ;

item            = let-declaration
                | function-declaration
                | webhook-declaration
                | expression, ";" ;

let-declaration = "let", identifier, (":", type)?, "=", expression, ";" ;

function-declaration
                = "fn", identifier, "(", parameters?, ")",
                  ("->", type)?, block ;

webhook-declaration
                = "webhook", "fn", identifier, "(", parameters?, ")",
                  ("->", type)?, block ;

parameters      = parameter, (",", parameter)*, ","? ;
parameter       = identifier, (":", type)? ;

block           = "{", statement*, expression?, "}" ;

statement       = let-declaration
                | function-declaration
                | expression, ";" ;

expression      = assignment-free-expression ;

assignment-free-expression
                = logical-or ;
logical-or      = logical-and, ("||", logical-and)* ;
logical-and     = equality, ("&&", equality)* ;
equality        = comparison, (("==" | "!="), comparison)* ;
comparison      = term, (("<" | "<=" | ">" | ">="), term)* ;
term            = factor, (("+" | "-"), factor)* ;
factor          = unary, (("*" | "/" | "%"), unary)* ;
unary           = ("!" | "-"), unary | call ;
call            = primary,
                  ("(", arguments?, ")" | ".", identifier)* ;
arguments       = expression, (",", expression)*, ","? ;

primary         = integer
                | string
                | "true"
                | "false"
                | identifier
                | list
                | record
                | block
                | if-expression
                | match-expression
                | function-expression
                | "(", expression, ")" ;

list            = "[", arguments?, "]" ;
record          = "record", "{", record-fields?, "}" ;
record-fields   = record-field, (",", record-field)*, ","? ;
record-field    = identifier, ":", expression ;

if-expression   = "if", expression, block, "else",
                  (block | if-expression) ;

function-expression
                = "fn", "(", parameters?, ")", ("->", type)?, block ;

match-expression
                = list-match | option-match | result-match ;
list-match      = "match", expression, "{",
                  "[", "]", "=>", expression, ",",
                  "[", identifier, ",", "..", identifier, "]",
                  "=>", expression, ","?, "}" ;
option-match    = "match", expression, "{",
                  option-arm, ",", option-arm, ","?, "}" ;
option-arm      = "Some", "(", identifier, ")", "=>", expression
                | "None", "=>", expression ;
result-match    = "match", expression, "{",
                  result-arm, ",", result-arm, ","?, "}" ;
result-arm      = "Ok", "(", identifier, ")", "=>", expression
                | "Err", "(", identifier, ")", "=>", expression ;

type            = "Int" | "Bool" | "String" | "Unit"
                | "HttpHeader" | "HttpRequest" | "HttpResponse"
                | "LogField" | "Secret"
                | "List", "<", type, ">"
                | "Option", "<", type, ">"
                | "Result", "<", type, ",", type, ">"
                | "Record", "{", record-type-fields?, "}" ;
record-type-fields
                = record-type-field, (",", record-type-field)*, ","? ;
record-type-field
                = identifier, ":", type ;
```

Assignments and mutable bindings do not exist in edition 2026.

A `webhook` declaration is valid only as a direct program item. A source
module contains zero or one webhook. Nested or duplicate webhook declarations
are errors. The parser retains its written parameter and result annotations;
the static checker requires exactly:

```krit
webhook fn name(request: HttpRequest) -> HttpResponse {
    // ...
}
```

The source name is a compiler fact. The component host exports the declaration
through the canonical `krit:runtime/webhook@0.2.0` interface's `handle`
operation; the name does not create an ambient network listener.

## 5. Program and block values

Items execute in source order.

- A declaration evaluates to unit.
- An expression followed by `;` evaluates for effects and then produces unit.
- A block's final expression without `;` is the block value.
- An empty block or a block ending in a statement produces unit.
- A file's value is not printed automatically.

The unit value is written by the REPL as `()`. There is no unit literal in
edition 2026; unit is produced by statements and empty blocks.

## 6. Values

The normative runtime value kinds are:

- signed 64-bit integer
- boolean
- UTF-8 string
- unit
- immutable list
- immutable record
- built-in `Option` variant (`Some` or `None`)
- built-in `Result` variant (`Ok` or `Err`)
- opaque host secret handle
- function closure

Integer overflow is a runtime error. Division truncates toward zero. Division
or remainder by zero is a runtime error.

Lists may contain values of different kinds in the dynamic baseline. The
static type system will require a common element type.

Records preserve their source field order for rendering. A record literal or
record type cannot repeat a field name. Record equality is structural by field
name and value, independent of field order.

`Some(value)`, `None`, `Ok(value)`, and `Err(value)` are ordinary immutable
runtime values. An opaque `Secret` is the exception to ordinary structural
composition: it cannot be printed, compared, JSON-encoded, or placed in a
list, record, or user-constructed `Result`. The sole `Option` exception is a
direct `Some(secret)` bearer argument to `http_request`; no other
`Option<Secret>` storage is accepted. The host injects the credential without
exposing bytes to guest code.

## 7. Bindings and scope

`let` evaluates its expression before introducing the new immutable binding.
The binding extends from the following item in the current scope to the end of
that scope.

A lexical scope cannot declare the same name more than once. This applies
across `let` and function declarations in that scope. A nested scope may
shadow a name from an outer scope. A binding cannot be read before its
declaration. `krit check` enforces these rules as a deliberate readability
constraint for both human- and AI-authored code.

Function declarations are visible from their declaration to the end of their
containing scope and may call themselves recursively. Mutually recursive
declarations are not supported in the dynamic baseline.

Webhook declarations introduce a function binding with the same lexical
visibility and recursion rules as an ordinary function declaration. The
`webhook` modifier additionally marks that function as the source module's
single exported host entrypoint.

Krit uses lexical scope. A function expression captures bindings from the
environment where it is created, not where it is called.

Duplicate parameters are a compile error.

## 8. Functions and calls

```krit
fn add(left, right) {
    left + right
}

let add_one = fn(value) {
    value + 1
};
```

Arguments evaluate from left to right. The argument count must equal the
parameter count. Calling a non-function is a runtime error.

Function values and opaque `Secret` values cannot be compared, including when
nested in an ordinary structural value.

Annotations may be written on let bindings, parameters, and function return
values:

```krit
let request: Record { path: String, retry: Option<Int> } =
    record { path: "/events", retry: Some(2) };

fn attempt(value: Int) -> Result<Int, String> {
    Ok(value)
}
```

Annotations are parsed and retained by the compiler. `krit check` enforces
them through static analysis without executing the program. `krit run`
remains the transitional direct dynamic evaluator and preserves runtime
conformance behavior, but preflights valid webhook/configuration/secret
contracts so unavailable hosts fail explicitly rather than fabricating a
value. Static checking is specified in `TYPES-AND-EFFECTS.md`.

## 8.1 Webhook contract types

The edition-2026 built-in names below are fixed aliases with stable public
names and closed structural shapes:

```text
HttpHeader   = Record { name: String, value: String }
HttpRequest  = Record {
    method: String,
    path: String,
    query: String,
    headers: List<HttpHeader>,
    body: String,
}
HttpResponse = Record {
    status: Int,
    headers: List<HttpHeader>,
    body: String,
}
```

Header order is preserved and duplicate header names are representable.
`HttpResponse` is exact: a response with a missing or additional field is a
type error. Status-range validation belongs to the component HTTP host, not the
static type checker. These built-ins do not add general user-defined type
alias syntax.

## 9. Operators

Operators evaluate left to right.

| Operator | Operands | Result |
|---|---|---|
| unary `-` | integer | checked negation |
| `+` | two integers | checked addition |
| `+` | two strings | concatenation |
| `-`, `*` | two integers | checked arithmetic |
| `/`, `%` | two integers | checked division or remainder |
| `<`, `<=`, `>`, `>=` | two integers | boolean |
| `==`, `!=` | comparable values | boolean |
| `!` | boolean | boolean |
| `&&`, `||` | booleans | boolean |

`&&` and `||` short-circuit. All other binary operators evaluate both
operands.

Equality is structural for integers, booleans, strings, unit, lists, records,
`Option`, and `Result`. Comparing a function directly or inside any composite
value is a runtime error.

No implicit conversions occur. In particular, integers and strings are not
automatically converted for `+`, and non-booleans are not truthy.

## 10. Conditionals

```krit
if condition {
    consequent
} else {
    alternative
}
```

The condition must be boolean. Only the selected branch evaluates.
`else if` is accepted through the grammar above. Both branches should
eventually have the same static type.

## 11. Records, lists, and matching

Record literal and field access:

```krit
let response = record { status: 200, body: "ready" };
println(response.status);
```

The `record` prefix distinguishes a record literal from a block. Fields
evaluate from left to right. Accessing a field requires a record containing
that field.

List literal:

```krit
[1, 2, 3]
```

The only list decomposition construct in edition 2026 is exhaustive matching:

```krit
match items {
    [] => 0,
    [head, ..tail] => head + sum(tail),
}
```

The subject evaluates once and must be a list.

- The `[]` arm is selected for an empty list.
- The `[head, ..tail]` arm is selected for a non-empty list.
- `head` receives the first element.
- `tail` receives the remaining immutable list.
- Pattern bindings exist only in the non-empty arm.

The two pattern names must differ. Both arms are mandatory and their order is
fixed, making the match visibly exhaustive.

`Option` and `Result` matches contain exactly the two variants of one family.
Their arms may appear in either order:

```krit
match possible_name {
    Some(name) => name,
    None => "anonymous",
}

match connector_result {
    Err(message) => message,
    Ok(response) => response.body,
}
```

`Some`, `Ok`, and `Err` bind their single payload. `None` binds nothing.
Missing, duplicate, unknown, or mixed-family arms are `K1003` syntax errors.
The subject evaluates once and must belong to the matched variant family.

## 12. Built-in output

```krit
print(value);
println(value);
```

`print` writes without a line break. `println` appends one line feed. Both
return unit. Arguments evaluate before output.

Output rendering is deterministic:

- booleans: `true` or `false`
- integers: base ten without separators
- strings: raw UTF-8 contents
- unit: `()`
- lists: `[item, item]`; nested strings use quoted escaped syntax
- records: `record { field: value }` in source field order
- variants: `Some(value)`, `None`, `Ok(value)`, or `Err(value)`
- functions: `<function>` or `<function name>`

Output is the only host effect executable by the direct evaluator. It is
modeled as `io.stdout`. The component runtime additionally provides bounded
`config.read`, `secret.read`, `http.request`, `ai.invoke`, and `observe.log`
interfaces to typed webhook artifacts.

## 13. JSON conversion

`json_encode(value)` returns a compact deterministic JSON string.
`json_decode(string)` parses JSON into dynamic Krit values.

| Krit | JSON |
|---|---|
| integer, boolean, string | corresponding JSON scalar |
| unit | `null` |
| list | array |
| record | object |
| `Some(value)` | `{"Some":value}` |
| `None` | `{"None":null}` |
| `Ok(value)` | `{"Ok":value}` |
| `Err(value)` | `{"Err":value}` |

Encoded object keys are sorted lexicographically so output is deterministic.
JSON objects decode as records in lexicographic field order, except an object
with exactly one `Some`, `None`, `Ok`, or `Err` key is decoded as that variant.
Consequently, those four single-key shapes are reserved as JSON variant tags.

JSON numbers must be signed 64-bit integers; floating-point and out-of-range
numbers are rejected. Encoding a function directly or inside another value is
`K4008`. `Secret` is rejected statically and can never reach JSON conversion.
Invalid JSON or JSON without a Krit representation is `K4009`.

The policy-2 component backend implements one additional fail-closed
specialization: when the inferred result is `String`, an unescaped JSON string
is validated and decoded without importing a host interface. Escapes and all
other component JSON shapes remain `K7002` or a bounded guest validation trap;
there is no evaluator fallback.

## 13.1 Configuration, secret, HTTP, AI, and logging host contracts

```krit
config_string("agent.model") // Result<String, String>
secret("github-token")       // Result<Secret, String>
http_request(
    "https://api.example.com",
    request,
    Some(token),
)                            // Result<HttpResponse, String>
ai_invoke("reviewer", input) // Result<String, String>
log_info("review.started", fields) // Result<Unit, String>
log_error("review.failed", fields) // Result<Unit, String>
```

Config and secret operations require exactly one direct string-literal
resource argument. `http_request` requires a direct normalized exact-origin
literal, an `HttpRequest`, and directly `None` or `Some(secret)`. Indirect host
operation use and computed resources are rejected because capability
requirements would not be statically knowable. A configuration read has
effect `config.read` and
requirement pair `("config.read", "agent.model")`. Secret acquisition has
effect `secret.read`; outbound HTTP has `http.request` and the exact origin.

`ai_invoke` requires a direct canonical adapter-name literal and adds
`ai.invoke("adapter")`. It returns bounded raw UTF-8 model text. The text is
nondeterministic and untrusted; source must explicitly validate or parse it
before structured use. The host never executes model output.

`LogField` is the closed alias `Record { name: String, value: String }`.
`log_info` and `log_error` require a direct canonical event literal and an
ordered `List<LogField>`, add `observe.log`, and return a fallible unit result.
No `Secret` can enter a log field. Host-side validation, redaction, buffering,
and publication are defined in `AI-OBSERVABILITY.md`.

The source checker does not require a manifest. Package build orchestration
checks requirements against the schema-1 manifest. The direct evaluator emits
`K5003` because these host operations are unavailable there; it never
substitutes an empty string, environment variable, network response, or secret
bytes. `krit invoke` and `krit serve` execute the separately built component
with explicit immutable host inputs.

## 14. Evaluation failure

The following are errors rather than implementation-defined behavior:

- malformed UTF-8 or tokens
- malformed syntax
- unresolved names
- integer literal outside signed 64-bit range
- integer overflow
- wrong operand, condition, subject, or callee kind
- incorrect argument count
- division or remainder by zero
- function comparison
- missing record field
- JSON encoding of a function
- non-literal configuration, secret, HTTP-origin, AI-adapter, or log-event
  resource
- printing, comparing, encoding, or structurally storing an opaque secret
- unavailable webhook, configuration, secret, HTTP, AI, or logging direct-run
  host contract
- invalid or unsupported JSON

Evaluation stops at the first error. Errors follow `DIAGNOSTICS.md`.

## 15. Determinism

For the same source, compiler version, edition, arguments, standard input, and
explicit capability responses, a conforming implementation produces the same
value, output bytes, and diagnostic code.

Hash iteration, memory addresses, host paths, timestamps, and stack traces
must not affect user-visible output.

## 16. Canonical source format

`krit fmt` defines the canonical textual layout for implemented edition-2026
syntax. Formatting changes trivia, equivalent literal spellings, and
grammar-permitted trailing commas, not expression order or program semantics.

- Indentation is four ASCII spaces. Formatter-produced structural whitespace
  contains no tabs or trailing spaces.
- Output uses LF line endings and exactly one final line feed.
- Binary operators, `=`, `=>`, `->`, and type/value colons use stable spacing.
  Calls, field access, unary operators, and generic type brackets remain
  compact.
- Non-empty blocks and every match are multiline. Short lists, records,
  parameter lists, argument lists, and record types remain on one line when
  readable.
- Multiline lists, records, parameter lists, argument lists, record types,
  and matches have one item per line and a trailing comma. Single-line forms
  omit a trailing comma.
- Top-level declarations are separated from executable expression statements
  by one blank line.
- Integer separators are removed. Strings use direct printable Unicode plus
  `\"`, `\\`, `\n`, `\r`, `\t`, `\0`, or lowercase `\u{...}` escapes for
  required control characters.
- Parentheses from the input are retained, including every pair needed to
  preserve evaluation order.
- Every `//` comment and its text is retained in source order. Standalone
  comments remain standalone, and end-of-line comments remain after code so
  they cannot comment out a following token. Whitespace inside comment text is
  not rewritten.

The formatter uses a soft 100-column target. It wraps delimiter-based
collections and signatures deterministically; an indivisible string, comment,
or operator chain may exceed the target. Correctness, comment preservation,
and idempotence take priority over filling every line optimally.
