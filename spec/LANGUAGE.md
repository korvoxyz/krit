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
logging, transactional durable string state, checkpoints, and explicit
HTTP/AI replay. The direct evaluator remains intentionally hostless; these
operations execute only through the bounded Component Model runtime. General
component generation remains narrower than the full checked language.

The historical Racket S-expression syntax is not accepted by Krit 0.2.

## 2. Source text

A source file is UTF-8 and conventionally ends in `.krit`.

- Keywords and identifiers are case-sensitive.
- Version 0.2 identifiers use ASCII letters, digits, and `_`.
- An identifier starts with a letter or `_`.
- `//` begins a comment ending at the next line break. A comment that begins
  with exactly three slashes, `///`, is a doc-comment line (section 2.1);
  four or more slashes begin an ordinary comment.
- Whitespace has no semantic meaning except token separation.
- Tabs are permitted in input; the canonical formatter emits spaces.
- Source positions are one-based Unicode scalar-value line and column numbers.

Reserved keywords:

```text
else false fn if let match record true webhook
```

Reserved built-in names:

```text
Err None Ok Some ai_invoke checkpoint_get checkpoint_put config_string db_begin_read db_begin_write db_commit db_execute db_query db_rollback http_request json_decode json_encode log_error log_info object_delete object_get object_put print println queue_publish replay_ai replay_http secret state_delete state_get state_put
```

A binding cannot use a keyword or reserved built-in name.

## 2.1 Doc comments

The **doc comment** of an item is the run of `///` lines directly before it.
Items are function declarations, nested ones included, and `webhook`,
`queue`, and `schedule` entrypoint declarations.

```krit
/// Labels one issue.
///
/// Fails when the model's answer is not a known label.
fn label(issue: String) -> Result<String, String> {
    // ...
}
```

- Reading upward from the item's first token, the run takes every `///` line
  and skips blank lines; it ends at the first other line. Blank lines are
  whitespace, so they neither split a doc comment nor separate it from its
  item. Any other comment, including a `////` banner, ends the run, so `///`
  lines above it document nothing.
- A line's text is what follows its `///`, without one leading space and
  without trailing whitespace. The doc comment's text is its lines' texts
  joined by line feeds, so a `///` line with no text adds an empty line.
- A `///` comment written after code on the same line, or one that no item
  follows (before a `let`, an expression statement, a `}`, or the end of the
  file), is an ordinary comment. It is never an error.

Doc comments do not affect checking, evaluation, or building. The formatter
preserves them (section 16), `krit explain` reports the doc comment of every
top-level function and entrypoint, and the language server shows a function's
doc comment when hovering over its declaration or a use of it.

## 3. Lexical grammar

The grammar uses EBNF. `*` means zero or more, `?` means optional, and quoted
text is literal.

```text
identifier  = (letter | "_"), (letter | digit | "_")* ;
integer     = digit, (digit | "_")* ;
string      = '"', string-character*, '"' ;
comment     = "//", non-line-break* ;
doc-comment = "///", (non-slash-non-line-break, non-line-break*)? ;
```

A `doc-comment` is a `comment` too: both are trivia, which the syntactic grammar
does not mention.

Integer separators cannot be leading, trailing, or repeated. Integers are
signed through unary `-`; the sign is not part of the token.

`?` is a token of its own. Edition 2026 gives it no meaning, so it appears in
no valid program; the parser uses it only to recognize another language's
conditional operator (section 4.2).

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
                | construction
                | block
                | if-expression
                | match-expression
                | function-expression
                | "(", ")"
                | "(", expression, ")" ;

list            = "[", (arguments | entries)?, "]" ;
entries         = entry, (",", entry)*, ","? ;
entry           = expression, ":", expression ;
record          = "record", "{", record-fields?, "}" ;
record-fields   = record-field, (",", record-field)*, ","? ;
record-field    = identifier, ":", expression ;

construction    = contract-name, "{", construction-fields?, "}" ;
contract-name   = "HttpRequest" | "HttpResponse" | "HttpHeader" | "LogField" ;
construction-fields
                = construction-field, (",", construction-field)*, ","? ;
construction-field
                = identifier, (":", expression)? ;

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

`()` is the unit value (section 5). A list whose first element is an
`entry` is an entry list (section 11); every element after it must be an
entry too, and a list whose first element is a plain expression cannot
contain one. Mixing the two forms is `K1001` at the first element written the
other way, with a note. `[]` is the empty list, entry lists included, and
`[:]` is `K1001`. In a type the unit type is spelled `Unit`: `-> ()` is
`K1002`, with a note.

A `contract-name` is an ordinary identifier, not a keyword; only these four
names followed by `{` start a `construction` (section 8.2), and any other
identifier followed by `{` keeps its meaning. In the condition of an `if` or
`else if` and in the subject of a `match`, a contract name followed by `{` is
not a construction either: the `{` opens the `if` block or the match arms, as
it does after any other name. A construction there must be written inside
parentheses, a list, a call's argument list, a block, a record or construction
field, or a match arm: `if (HttpResponse { status: 200 }).status == 200 { ... }`.
When the tokens after that `{` are a name followed by `:` or `,`, which never
start a block, the parser reports `K1001` with a note to add the parentheses;
`if HttpResponse { x } else { y }` remains a condition named `HttpResponse`.

Assignments and mutable bindings do not exist in edition 2026. Neither do
`return`, loops, classes, modules, `async`, exceptions, indexing, arrow
functions, the conditional operator, or `switch`; section 4.2 specifies how
the parser reports them.

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

## 4.1 Nesting limits

A source file may nest syntax at most **256 levels** deep. Each of the
following opens one level for what it contains:

- a parenthesized expression, a block, a list literal (an entry list
  included), a record literal, and a named construction
- the argument list of a call, and each call or field-access link of a
  postfix chain (`a.b(c).d` opens three levels for what follows `a`)
- the operand of a unary operator
- the condition of an `if`, and the subject and the arm list of a `match`
- the argument list of `List<…>`, `Option<…>`, or `Result<…, …>`, and the
  field list of `Record { … }`

An `else if` continuation stays at the level of the `if` it continues, and a
chain of binary operators such as `a + b + c` stays at the level of its first
operand, so long flat chains are not nesting. They still nest in the
program's syntax tree, so a separate **structural depth** limit applies: the
nesting level of any point plus the number of `else if` continuations, binary
operators, and call or field-access links above it in the tree may not exceed
**4,096**.

Operator and postfix chains nest to the left: their first operand lies below
every operator or link of the chain. Parentheses group without changing that,
so in `(a + b) + c` the operand `a` has nesting level 1 and two operators
above it, a structural depth of 3, and wrapping a chain in parentheses cannot
hide its length from an enclosing chain. A 1,000-branch `else if` chain and a
4,000-operator sum are valid; a 2,100-operator sum whose first operand is a
parenthesized 2,100-operator sum is not.

Inferred types are bounded too. Inference solves for an unknown type at every
binding, parameter, function result, and list element, and rejects any step
that would make one of them stand for a type nesting more than 256 type
constructors deep. That covers a binding deeper than the one before it, such
as the 257th binding of a chain of `let next = [previous];` declarations, and
a type inferred earlier that grows later: after `let e = []; let top = [[e]];`,
the type of `top` deepens whenever a later expression resolves the element
type of `e` to a deeper list, including by comparing `e` with another binding
whose type is already known to be deeper. Types spelled out by nested literals
and annotations are bounded by the nesting limit, so every static type stays
within a fixed depth.

Exceeding any of these limits is error `K1005` at the construct, operator, or
inference step that crosses it. Parsing, static checking, Core lowering and
verification, formatting, and component generation all enforce the same
bounds, so no accepted program can exhaust the compiler's native stack. The
later stages also count their own recursion, to reject syntax trees built
without the parser; every tree within the limits above stays inside those
internal bounds, so a source program only ever sees the limits stated here.

## 4.2 Constructs from other languages

Constructs that other languages have and Krit does not are reported with
their own codes (`K1101`-`K1112`), each with a one-line fix in the
diagnostic's notes (`DIAGNOSTICS.md`), instead of a generic syntax error.
Their words are not keywords: each remains an ordinary identifier, and
`let for = 1;`, `use(value)`, and `record { from: a, default: b }` are valid.
The parser reports a construct only where its tokens cannot be part of any
valid program.

A **habit word** is one of these identifiers:

| Code | Construct | Words |
|---|---|---|
| `K1101` | `return` | `return` |
| `K1102` | loops | `for` `while` `loop` `do` |
| `K1103` | mutable bindings | `mut` `var` `const` |
| `K1104` | type declarations | `struct` `class` `impl` `trait` `interface` |
| `K1105` | imports | `import` `use` `require` `from` |
| `K1106` | async functions | `async` `await` |
| `K1107` | exceptions | `try` `catch` `throw` `raise` `except` `finally` |
| `K1111` | `switch` | `switch` `case` `default` |

A habit word reports its construct's code when:

- it is an operand followed, on the same line, by a token that can only start
  an operand, which never follows a complete expression: an identifier,
  integer, string, `true`, `false`, `record`, `if`, `match`, `fn`, `!`, or a
  `[` with whitespace between it and the word (`return value`,
  `for item in items`, `const limit = 3`, `class Point`, `async fn`,
  `throw "empty"`, `return [a, b]`). When a line break comes between them,
  the word stays an ordinary name and the program reports the generic error
  of the `;` or `,` that is missing (`let x = from` followed by `println(x);`
  on the next line is `K1002`);
- it is an operand followed by a parenthesized group and then `=>`
  (`async (x) => e`);
- it starts a statement and is followed by `{`, by `:`, or by a parenthesized
  group and then `{` (`try {`, `loop {`, `default:`,
  `while (ready) {`, `for (let i = 0; i < n; i++) {`); or
- it is `mut`, written where a binding name is expected and followed by
  another name (`let mut x`, `fn f(mut x)`, `Some(mut x)`), which reports
  `K1103`.

These constructs are reported wherever they occur:

- `K1103`: an expression statement, a block's final expression, or a `let`
  value followed by `=` (`count = count + 1;`, `record.field = value;`); and
  `+=`, `-=`, `*=`, `/=`, `%=`, `++`, or `--` written as two touching tokens
  where an operand is missing (`x += 1`, `x++`, `x--`).
- `K1108`: a `[` touching the end of an operand in a postfix chain
  (`items[0]`, `load()[0]`, `[a, b][0]`). With whitespace before the `[`
  (`items [0]`), the program is still invalid but is not reported as
  indexing.
- `K1109`: an identifier or a parenthesized group followed by `=>`
  (`x => e`, `(x, y) => e`, `(x: Int) => e`, `() => e`). No expression is
  ever followed by `=>`; only match patterns are.
- `K1110`: a `?` after an operand, followed by an expression and then `:`
  (`ready ? 1 : 2`). When the tokens after the `?` start an expression that
  does not parse, the `?` is still `K1110` if a `:` follows it at the same
  bracket depth before the end of the file, a closing bracket opened before
  the `?`, or the first `;`, `,`, or `let` at that depth
  (`ready ? items[0] : None`, `ok ? { status: 200 } : { status: 500 }`). The
  parser examines the tokens after the `?` without consuming them. A `?` in
  any other position is an unexpected token (`K1001`).
- `K1112`: `else` followed by `return` and a token that can only start an
  operand, as above, either as the first tokens of its block or directly
  (`else { return 0; }`, `else return 0;`).

To find the end of a parenthesized group, the parser matches parentheses and
looks no further than the first `;` when it expects `=>` after the group, or
the first `{` or `}` when it expects `{`.

Where a habit word could still be an ordinary name (`return;`,
`return -1;`, `await(job)`), the program parses, and if the name is not bound
checking reports `K2001` with the construct's fix. Parsing stops at the first
error, so the first construct reached in source order is the one reported,
with two exceptions inside the first branch of a conditional operator: a
conditional operator there is reported in place of the outer one
(`a ? b ? c : d : e` reports `? c :`), and so is a nesting limit (`K1005`)
crossed there.

## 5. Program and block values

Items execute in source order.

- A declaration evaluates to unit.
- An expression followed by `;` evaluates for effects and then produces unit.
- A block's final expression without `;` is the block value.
- An empty block or a block ending in a statement produces unit.
- A file's value is not printed automatically.

The unit value is written `()`, in source and when printed. Statements and
empty blocks produce it too. `()` evaluates to unit with no effect and is
valid wherever a value is (`Ok(())`, `if ready { () } else { log() }`). It is
a value, not a type: the unit type is spelled `Unit`.

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

The elements of a list share one static type (`TYPES-AND-EFFECTS.md`).

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
across `let` and function declarations in that scope. The discard name `_` is
the only exception (below). A nested scope may
shadow a name from an outer scope. A binding cannot be read before its
declaration. `krit check` enforces these rules as a deliberate readability
constraint for both human- and AI-authored code, and `krit run` enforces them
by checking first (section 14).

The identifier `_` alone is the discard name. It is exempt from the
one-declaration rule: any number of `let` bindings, function declarations,
parameters, and match binders in one scope may use it, including both names of
one `[_, .._]` list pattern. A program should bind `_` only for a value it
does not use. Reading `_` as a value, that is, an identifier expression `_`
that resolves to a binding, is valid in edition 2026 and refers to the most
recent `_` binding in scope, exactly as shadowing does, but checking reports
warning `K2003`; edition 2027 makes it an error. An identifier expression `_`
with no `_` binding in scope is an undefined name (`K2001`). Warnings never
change whether a program checks or runs (`DIAGNOSTICS.md`).

A lexical binding shadows a built-in name of the same spelling for the
binding's whole scope, in checking and evaluation alike. `let`, function, and
parameter names cannot reuse a built-in name (`K2002`). A match binder can
reuse a built-in name that the parser does not reserve, such as `state_get`,
and within its arm the name refers to the binder.

Function declarations are visible from their declaration to the end of their
containing scope and may call themselves recursively. Mutually recursive
declarations are not supported in the dynamic baseline.

Webhook declarations introduce a function binding with the same lexical
visibility and recursion rules as an ordinary function declaration. The
`webhook` modifier additionally marks that function as the source module's
single exported host entrypoint.

Krit uses lexical scope. A function expression captures bindings from the
environment where it is created, not where it is called.

Duplicate parameters are a compile error, except for the discard name `_`.

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
parameter count, and only a function can be called; checking reports either
violation before evaluation (section 14).

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
performs the same checking before it evaluates anything and refuses a
program that fails it (section 14), so annotations hold in direct evaluation
too. It then refuses checked programs that need an entrypoint or host
contract so unavailable hosts fail explicitly rather than fabricating a
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

## 8.2 Named construction

The contract records `HttpRequest`, `HttpResponse`, `HttpHeader`, and
`LogField` (section 13.1) can be built by name:

```krit
let fetch = HttpRequest { method: "GET", path: "/issues/1" };
let response = HttpResponse { status: 200, body };
```

A field written as its name alone is punned: `body` means `body: body`, a
reference to the binding named `body`. Fields may be written in any order, and
a trailing comma is allowed. A construction may omit a field that has a
spec-fixed default:

| Contract | Required fields | Defaults of the other fields |
|---|---|---|
| `HttpRequest` | `method`, `path` | `query: ""`, `headers: []`, `body: ""` |
| `HttpResponse` | `status` | `headers: []`, `body: ""` |
| `HttpHeader` | `name`, `value` | none |
| `LogField` | `name`, `value` | none |

Writing a field the contract does not have, writing a field more than once, or
omitting a required field is error `K3016`. The field names are checked before
any field value: an unknown or repeated field is reported at its name, and
missing required fields at the type name. Each written value must have its
contract field's type (`K3001` otherwise; `TYPES-AND-EFFECTS.md`).

The value is exactly the record that `record { ... }` with every contract
field would build. Its static type is the contract's closed structural record
type (section 8.1), so it flows anywhere such a record does, including
`http_request`, webhook results, and log field lists, and it is equal to that
record. Because the type is structural, `HttpHeader { ... }` and
`LogField { ... }` have the same type. The written values evaluate from left
to right in source order. The record lists its fields in the contract's
declared order (sections 8.1 and 13.1, and the table above), whatever order
they are written in, and renders in that order
(`record { status: 200, headers: [], body: "ready" }`). Defaults have no
source text and perform no evaluation or effect; the compiler supplies them
directly, without any syntax node. `record { ... }` remains valid for every
record, including these four.

## 9. Operators

Operators evaluate left to right.

| Operator | Operands | Result |
|---|---|---|
| unary `-` | integer | checked negation |
| `+` | two integers | checked addition |
| `+` | two strings | concatenation |
| `-`, `*` | two integers | checked arithmetic |
| `/`, `%` | two integers | checked division or remainder |
| `<`, `<=`, `>`, `>=` | two integers | boolean (numeric order) |
| `<`, `<=`, `>`, `>=` | two strings | boolean (byte-lexicographic order) |
| `==`, `!=` | comparable values | boolean |
| `!` | boolean | boolean |
| `&&`, `||` | booleans | boolean |

`&&` and `||` short-circuit. All other binary operators evaluate both
operands.

Equality is structural for integers, booleans, strings, unit, lists, records,
`Option`, and `Result`. Comparing a function directly or inside any composite
value is a runtime error.

Strings are ordered byte-lexicographically over their UTF-8 encoding: the
first differing byte, compared as an unsigned value, decides, and a proper
prefix orders before every longer string it begins. The order therefore
follows Unicode scalar values, not UTF-16 code units, locale collation, or
normalization (`"e\u{301}"` and `"é"` are different strings and
`"e\u{301}" < "é"`). Ordering any other operand kind, or mixing kinds, is
rejected statically (`K3001`, or `K3009`/`K3010` for opaque values) and is a
`K4001` runtime error in direct execution. String equality is byte equality,
so `a == b` exactly when neither `a < b` nor `b < a`.

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
`else if` is accepted through the grammar above; a continuation does not add
a nesting level (section 4.1). Both branches should eventually have the same
static type.

## 11. Records, lists, and matching

Record literal and field access:

```krit
let response = record { status: 200, body: "ready" };
println(response.status);
```

The `record` prefix distinguishes a record literal from a block; a named
construction (section 8.2) starts with a contract name instead. Fields
evaluate from left to right. Accessing a field requires a record containing
that field.

Values have no methods. `value.name(arguments)` reads field `name` of the
record `value` and calls the function stored there. When `value` has no such
field, because it is not a record or is a record without that field, checking
reports the method call (`K3011`) with the standard-library function to use
instead when there is one, such as `string_byte_len(s)` for `s.len()`.

List literal:

```krit
[1, 2, 3]
```

An **entry list** writes a list of name/value records, such as HTTP headers
and log fields:

```krit
["accept": "application/json", "x-request-id": id]
```

`["k1": v1, "k2": v2]` means
`[record { name: k1, value: v1 }, record { name: k2, value: v2 }]`. Keys and
values are arbitrary expressions of type `String` (`K3001` at the key or
value otherwise, `TYPES-AND-EFFECTS.md`) and evaluate from left to right in
source order: `k1`, `v1`, `k2`, `v2`. Each record lists `name` and then
`value`. The list's type is `List<Record { name: String, value: String }>`,
the closed structural record that `HttpHeader` and `LogField` name (sections
8.1 and 13.1), so an entry list is a header list for `HttpRequest` and
`HttpResponse` and a field list for `log_info` and `log_error`, and it is
equal to the spelled-out list. A trailing comma is allowed. An entry list has
at least one entry; `[]` is the empty list of every element type. One literal
cannot mix entries with plain elements (section 4).

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

The two pattern names must differ unless both are the discard name `_`
(section 7). Both arms are mandatory and their order is fixed, making the
match visibly exhaustive.

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
interfaces plus `state.transaction`, `queue.publish`, `object.read`,
`object.write`, `database.read`, `database.write`, `cache.read`, `cache.write`,
`search.query`, and `search.vector` to typed webhook, queue,
and schedule artifacts.

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

Decoded JSON text may be surrounded by JSON whitespace (space, tab, line
feed, carriage return). Strings decode every RFC 8259 escape: `\"`, `\\`,
`\/`, `\b`, `\f`, `\n`, `\r`, `\t`, and `\uXXXX`. A `\u` surrogate escape
must be a high surrogate immediately followed by a low surrogate, and the
pair decodes to one scalar value. A lone surrogate is invalid JSON (`K4009`),
because Krit strings hold only Unicode scalar values, and so are any other
escape and an unescaped control character below U+0020.

The policy-2 component backend implements one additional fail-closed
specialization: when the inferred result is `String`, a JSON string with any
of these escapes is validated and decoded in the guest without importing a
host interface. Text the evaluator rejects is `K4009` there too; so is valid
JSON that is not a string, which the evaluator would return unchecked. All
other component JSON shapes remain `K7002`; there is no evaluator fallback
(see `WASM-SANDBOX.md`, "Guest JSON string decoding").

## 13.1 Configuration, secret, HTTP, AI, logging, durable-state, queue, and object host contracts

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
state_get("agent-work", key) // Result<Option<String>, String>
state_put("agent-work", key, value) // Result<Unit, String>
state_delete("agent-work", key) // Result<Unit, String>
checkpoint_get("agent-work", "posted-message") // Result<Option<String>, String>
checkpoint_put("agent-work", "posted-message", value) // Result<Unit, String>
replay_http("agent-work", "fetch", "https://api.example.com", request)
// Result<HttpResponse, String>
replay_ai("agent-work", "summarize", "reviewer", input)
// Result<String, String>
queue_publish("render-jobs", body) // Result<String, String>
object_get("render-output", key) // Result<Option<String>, String>
object_put("render-output", key, value) // Result<Unit, String>
object_delete("render-output", key) // Result<Unit, String>
db_begin_read("catalog") // Result<DatabaseTransaction, String>
db_begin_write("catalog") // Result<DatabaseTransaction, String>
db_query(transaction, "count-visits", []) // Result<String, String>
db_execute(transaction, "record-visit", [path]) // Result<Int, String>
db_commit(transaction) // Result<Unit, String>
db_rollback(transaction) // Result<Unit, String>
```

Config and secret operations require exactly one direct string-literal
resource argument in canonical-name form (see `CAPABILITIES.md`); a
configuration key may additionally contain `_`, as in `"agent.timeout_ms"`,
while a secret name may not. A malformed name is `K3008`. `http_request`
requires a direct normalized exact-origin literal, an `HttpRequest`, and
directly `None` or `Some(secret)`. Indirect host
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

State/checkpoint/replay store names, checkpoint names, replay operation names,
HTTP origins, and AI adapters are direct canonical literals. State keys and
values are bounded ordinary strings. State and checkpoint mutations commit
only after successful invocation completion. Replay records completed bounded
external results independently so a later retry can resume without repeating
the recorded call. `replay_http` is anonymous and accepts only safe methods or
one valid ordinary `Idempotency-Key`; authenticated replay HTTP is unavailable.
`Secret` cannot enter any state, checkpoint, or replay value. The complete
crash and durability semantics are normative in `DURABLE-STATE.md`.

Queue names and object bucket names are also direct canonical literals; job
bodies and object keys are bounded ordinary strings. `queue_publish` adds
`queue.publish("queue")` and returns the durable job identity. `object_get`
adds `object.read("bucket")`; `object_put` and `object_delete` add
`object.write("bucket")`. Object and queue mutations stage in the invocation
transaction and commit only at the successful outcome boundary. `Secret` cannot
enter a job body, an object key, or an object value. Guest-visible object
listing is not part of protocol 1. The complete queue, schedule, and object
semantics are normative in `JOBS-AND-STORAGE.md`.

Database and statement names are direct canonical literals; parameters are a
bounded `List<String>` the host binds by declared type. `DatabaseTransaction` is
opaque exactly as `Secret` is: it may appear only as the first argument of
`db_query`, `db_execute`, `db_commit`, or `db_rollback`, and printing,
comparing, encoding, logging, or storing it is `K3010`. A transaction must be
explicitly committed or rolled back; an invocation that ends with one open fails
closed. Query results are bounded deterministic JSON text because protocol 1 has
no richer typed row value. The complete transaction, catalog, and crash
semantics are normative in `DATABASE.md`.

## 13.2 Typed delivery entrypoints

A module declares at most one `webhook`, `queue`, or `schedule` entrypoint:

```krit
queue "render-jobs" fn handle(job: QueueJob) -> Result<String, String> { Ok(job.id) }

schedule "hourly-sweep" fn handle(event: ScheduleEvent) -> Result<String, String> {
    Ok(event.id)
}
```

`queue` and `schedule` are contextual keywords: they introduce an entrypoint
only when directly followed by a canonical name literal and `fn`, so ordinary
bindings, parameters, and record fields may still be spelled `queue` or
`schedule`. The declared name is the exact resource of the
`queue.consume("queue")` or `schedule.trigger("schedule")` requirement.

`QueueJob` is the closed contract
`Record { queue: String, id: String, body: String, attempt: Int, maxAttempts: Int }`.
`ScheduleEvent` is the closed contract `Record { schedule: String, id: String,
scheduledAtMillis: Int, firedAtMillis: Int, attempt: Int, maxAttempts: Int }`.
Both instants are host-supplied UTC epoch milliseconds; guests have no clock.

`Ok(detail)` acknowledges the delivery and commits every staged mutation with
the acknowledgement in one transaction. `Err(detail)` commits nothing and lets
the host retry or dead-letter under its configured policy. `detail` is bounded
and is never interpreted as control flow.

The source checker does not require a manifest. Package build orchestration
checks requirements against the schema-1 manifest. The direct evaluator emits
`K5003` because these host operations are unavailable there; it never
substitutes an empty string, environment variable, network response, or secret
bytes. `krit invoke` and `krit serve` execute the separately built component
with explicit immutable host inputs.

## 14. Evaluation failure

`krit run` first checks the source exactly as `krit check` does: lexing,
parsing, name resolution, type, effect, and capability analysis, and Core
verification. It never runs the WebAssembly target check that `krit check`
adds for `--target` or for entrypoint files (`DIAGNOSTICS.md`, Build-target
diagnostics), because direct evaluation builds no artifact. A program that
fails checking is never evaluated: `run` reports the same diagnostic with the
same exit status as `check` and writes no program output. Warnings that checking reports, such as `K2003`, are
printed exactly as `check` prints them and never prevent evaluation.

A program that passes checking is still refused before evaluation, with
`K5003` and exit status 4, when it declares a webhook, queue, or schedule
entrypoint or requires any host capability, because direct evaluation
provides no entrypoint invocation and no host other than standard output.

The following are errors rather than implementation-defined behavior.
Checking reports these, so a program containing one is never evaluated:

- malformed UTF-8 or tokens
- malformed syntax
- unresolved names
- wrong operand, condition, subject, or callee kind
- incorrect argument count
- function comparison
- missing record field
- JSON encoding of a function
- non-literal configuration, secret, HTTP-origin, AI-adapter, log-event,
  state-store, checkpoint, or replay-operation resource
- printing, comparing, encoding, or structurally storing an opaque secret

Evaluation reports these:

- integer literal outside signed 64-bit range
- integer overflow
- division or remainder by zero
- invalid or unsupported JSON
- a value decoded by `json_decode` that does not have the kind or fields its
  use requires (`K4001`), because decoding does not validate the inferred type
- more than 10,000 active function calls, more than 100,000 nested
  expression and block evaluations, or a list, record, or variant value
  nesting more than 10,000 levels, in direct evaluation (`K4010`)

Evaluation stops at the first error. Errors follow `DIAGNOSTICS.md`. The
evaluator keeps its own kind, callee, arity, comparison, and encoding checks
(`K4001`, `K4002`, `K4003`, `K4006`, `K4008`). In a checked program only a
decoded JSON value can fail one of them; otherwise they make a program
evaluated without checking, which `krit run` never does, fail with a
diagnostic.

The direct-evaluation limits count active frames, not total work: sequential
calls that each recurse 9,000 levels deep are valid. A typical recursive
function evaluates three to six nested expressions per call, so it reaches the
call limit first; a function body nested unusually deeply reaches the
evaluation limit first. The value limit applies when a list, record, or
variant is built. A checked program's values nest no deeper than their static
types (which section 4.1 bounds far below 10,000 levels) plus the JSON
decoder's own small nesting bound, so `krit run` never reaches the value
limit; it bounds programs evaluated without checking, such as a function that
wraps its argument in one more list on every call. Printing, comparing,
JSON-encoding, and releasing a value therefore recurse at most 10,000 levels,
and releasing a long chain of closures that each capture the previous one
does not recurse once per closure.

`krit run` evaluates on a dedicated thread with a 1 GiB stack reservation.
The most stack measured at these limits is about 120 MiB in optimized builds
and 500 MiB in unoptimized builds, so `K4010` is reported before the native
stack is exhausted.

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
- A named construction is laid out like a record literal: on one line when it
  fits (`HttpResponse { status: 200, body }`), otherwise one field per line
  with a trailing comma. A field whose value is exactly the identifier with
  the field's own name is written punned (`body`, from `body` or
  `body: body`); every other field is written `field: value`, including
  `body: (body)`. A field keeps its `name: name` spelling when a comment
  stands between the name and the value, so no comment moves. Record literals
  are never punned.
- An entry list is laid out like a list: on one line when it fits
  (`["x-a": a, "x-b": b]`), otherwise one entry per line with a trailing
  comma. Each entry is written `key: value`, with no space before the `:` and
  one after it.
- The unit value is written `()`, with nothing between its parentheses.
- Each line of a doc comment (section 2.1) is written with exactly one space
  between its `///` and its text, or as `///` alone when the line has no
  text; the text after that one space, further indentation and trailing
  whitespace included, is kept as written (`///Adds.` becomes `/// Adds.`,
  `///   code` keeps its indentation). A `///` comment that documents
  nothing keeps its exact text, as every other comment does. As before any
  comment, blank lines between a doc comment and its item are removed, which
  never changes the doc comment.
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
  not rewritten, apart from the one space after a doc comment's `///` (above).
  Formatting therefore never attaches a doc comment to an item or detaches
  one from it.

The formatter uses a soft 100-column target. It wraps delimiter-based
collections and signatures deterministically; an indivisible string, comment,
or operator chain may exceed the target. Correctness, comment preservation,
and idempotence take priority over filling every line optimally.
