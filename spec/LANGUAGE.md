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
Err None Ok Some ai_invoke checkpoint_get checkpoint_put config_string db_begin_read db_begin_write db_commit db_execute db_query db_rollback http_request json_decode json_encode log_error log_info object_delete object_get object_put print println queue_publish replay_ai replay_http secret state_delete state_get state_put
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

`?` is a token of its own: the postfix error-propagation operator (section
8.2). After an operand, the parser also uses it to recognize another
language's conditional operator (section 4.2).

Strings support these escapes:

```text
\"  \\  \n  \r  \t  \0  \u{hex-scalar}
```

Other escapes and invalid Unicode scalar values are lexical errors.

## 4. Syntactic grammar

```text
program         = item* ;

item            = let-declaration
                | let-else
                | function-declaration
                | webhook-declaration
                | expression, ";" ;

let-declaration = "let", identifier, (":", type)?, "=", expression, ";" ;

let-else        = "let", variant-pattern, (":", type)?, "=", expression,
                  "else", let-else-arm, ";" ;
let-else-arm    = variant-pattern, "=>", expression ;
variant-pattern = ("Some" | "Ok" | "Err"), "(", identifier, ")"
                | "None" ;

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
                | let-else
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
                  ("(", arguments?, ")" | ".", identifier | "?")* ;
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
                = "match", expression, "{",
                  match-arm, (",", match-arm)*, ","?, "}" ;
match-arm       = pattern, "=>", expression ;
pattern         = binder
                | "_"
                | ("Some" | "Ok" | "Err"), "(", pattern, ")"
                | "None"
                | "[", "]"
                | "[", (binder | "_"), ",", "..", (binder | "_"), "]" ;
binder          = identifier ;   (* other than "_" *)

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

Assignments and mutable bindings do not exist in edition 2026. Neither do
`return`, loops, classes, modules, `async`, exceptions, indexing, arrow
functions, the conditional operator, or `switch`; section 4.2 specifies how
the parser reports them. The two early exits are postfix `?` (section 8.2)
and let-else (section 8.3), which the grammar accepts wherever a `let`
declaration may appear and checking accepts only as a statement of a named
function's body.

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

- a parenthesized expression, a block, a list literal, and a record literal
- the argument list of a call, and each call, field-access, or `?` link of a
  postfix chain (`a.b(c)?.d` opens four levels for what follows `a`)
- the operand of a unary operator
- the condition of an `if`, the subject and the arm list of a `match`, and
  the arm of a let-else (section 8.3)
- the payload of a `Some`, `Ok`, or `Err` pattern; and the value of a `match`
  arm, once for each variant or list pattern its pattern holds inside a
  payload, since Core IR decides each of them with one more nested match
  (`Ok(Some(v)) => value` nests `value` one level deeper than
  `Ok(v) => value`)
- the argument list of `List<…>`, `Option<…>`, or `Result<…, …>`, and the
  field list of `Record { … }`

An `else if` continuation stays at the level of the `if` it continues, and a
chain of binary operators such as `a + b + c` stays at the level of its first
operand, so long flat chains are not nesting. They still nest in the
program's syntax tree, so a separate **structural depth** limit applies: the
nesting level of any point plus the number of `else if` continuations, binary
operators, and call, field-access, or `?` links above it in the tree may not
exceed **4,096**.

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
- `K1110`: a `?` after an operand that starts another language's
  conditional operator. Call a `:` a **branch end** for the `?` when it
  follows the `?` at the same bracket depth before the end of the file, a
  closing bracket opened before the `?`, or the first `;`, `,`, or `let` at
  that depth; no valid program has one, since a first branch contains none of
  those outside brackets. What follows the `?` decides:
  - A token that can only start an operand, as above (`ready ? 1 : 2`): the
    `?` is `K1110` when the tokens after it are an expression and then `:`,
    or start an expression that does not parse and a branch end follows
    (`ready ? items[0] : None`, `c ? return 1 : 2`). Otherwise it is an
    unexpected token (`K1001`) when that token is on the same line
    (`ready ? 1`), and a postfix `?` followed by a missing `;` or `,`, which
    the parser reports next, when it is on a later line.
  - `-`, `(`, `[`, or `{`, which may also continue a postfix `?`
    (`count? - 1`, `pick()?(1)`, `if ready? {`): the `?` is `K1110` exactly
    when a branch end follows (`sign ? -1 : 1`,
    `ok ? { status: 200 } : { status: 500 }`), and the tokens between are
    never parsed.
  - Any other token (`;`, `)`, `.`, `?`, a binary operator other than `-`):
    the `?` is the postfix operator.

  The parser examines the tokens after the `?` without consuming them, and a
  `?` where an operand should start is an unexpected token (`K1001`).
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
conditional operator there that ends at the same branch end is reported in
place of the outer one (`a ? b ? c : d : e` and `a ? -b ? c : d : e` report
`? c :`, while in `a ? (b ? c[0] : d) : e` the inner one ends inside the
brackets and the outer one is reported), and so is a nesting limit (`K1005`)
crossed in a first branch that the parser parses.

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
and parameters in one scope may use it. A program should bind `_` only for a
value it does not use. Reading `_` as a value, that is, an identifier
expression `_` that resolves to a binding, is valid in edition 2026 and refers
to the most recent `_` binding in scope, exactly as shadowing does, but
checking reports warning `K2003`; edition 2027 makes it an error. An
identifier expression `_` with no `_` binding in scope is an undefined name
(`K2001`). Warnings never change whether a program checks or runs
(`DIAGNOSTICS.md`).

In a pattern (sections 8.3 and 11), `_` is a wildcard: it matches any value
and binds nothing, and one pattern may hold it any number of times
(`[_, .._]`). Where a binder in its place would be visible (the arm of a
`match` or let-else, or the rest of the block after a let-else pattern), `_`
names no value at all: reading it there is an undefined name (`K2001`, with a
note), even when an enclosing scope binds `_`, until a later declaration
binds `_` again. A program written for 0.2, where a pattern's `_` was an
ordinary binder, therefore fails to check rather than silently reading a
different value.

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

A let-else (section 8.3) declares its pattern's binder exactly as `let`
declares its name: in the scope of the enclosing block, from the following
statement to the end of that block, under the same one-declaration rule. The
binder of its arm is visible only in the arm.

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

## 8.2 Error propagation

Postfix `?` unwraps a `Result` or an `Option` and returns its failure from the
enclosing function:

```krit
fn review(input: String) -> Result<String, String> {
    let model = config_string("agent.model")?;
    let answer = ai_invoke("reviewer", model + ": " + input)?;
    Ok("reviewed: " + answer)
}
```

`?` binds like a call or a field access: `load()?.body`, `pick()?(1)`, and
`nested??` chain, `-count?` negates `count?`, and `count? + 1` adds to it.

If `e` has type `Result<T, E>`, `e?` has type `T`; if `e` has type
`Option<T>`, `e?` has type `T`. `?` is valid in any expression position of the
body of a named function, including a `queue` or `schedule` entrypoint, whose
declared return type is `Result<U, E>` with the identical error type `E` (for
a `Result` operand) or `Option<U>` (for an `Option` operand); `U` need not be
`T`. There is no error conversion. The innermost enclosing function decides;
`?` never reaches through it to an outer one. Everywhere else `?` is `K3012`:

- at the top level;
- in an anonymous function `fn(...) { ... }`, even one with a declared
  `Result` or `Option` return type;
- in a `webhook` entrypoint, which returns `HttpResponse`: bind each
  fallible step with let-else (section 8.3), or move the steps into a helper
  that returns `Result<HttpResponse, String>`, use `?` there, and `match` the
  helper's result in the webhook;
- in a named function without a declared return type, or whose declared
  return type is neither a `Result` nor an `Option`.

A `Result` operand in a function that returns `Option`, an `Option` operand in
a function that returns `Result`, and a `Result` operand whose error type
differs from the declared one are `K3013`; `K3012` and `K3013` each carry one
note with the fix. An operand of any other type is `K3001`, and an operand
whose type is not yet known takes the declared family and error type.
Propagating rebuilds
`Err(error)` at the function's return type, so an error type that contains
`Secret` or `DatabaseTransaction` is rejected exactly as `Err(error)` is
(`K3009`, `K3010`).

`e` evaluates first. If it is `Ok(v)` or `Some(v)`, `e?` evaluates to `v`. If
it is `Err(x)` or `None`, the enclosing function returns `Err(x)` or `None`
immediately: nothing after the `?` in that function call evaluates, including
the remaining operands and arguments of the expressions around it, while
effects that already happened stay performed. In `f(println("a"), g()?)`,
`a` is printed, then `g()` is called, and when it fails `f` is never called.
`?` adds no effect and no capability requirement of its own.

Core IR lowers `e?` to a variant match on `e`. The `Ok` or `Some` arm yields
the payload; the `Err` or `None` arm rebuilds the failure at the function's
return type and ends with a verified function exit (`TYPES-AND-EFFECTS.md`,
"Error propagation"), which components lower to a WebAssembly `return`
(`WASM-SANDBOX.md`).

## 8.3 Let-else

A let-else binds the payload of one variant and makes the enclosing function
return when the value is the other variant:

```krit
fn failure(error: String) -> HttpResponse {
    record { status: 502, headers: [], body: error }
}

webhook fn handle(request: HttpRequest) -> HttpResponse {
    let Ok(repository) = config_string("reference.repository") else Err(error) => failure(error);
    let Ok(stored) = state_get("agent-work", request.path) else Err(error) => failure(error);
    let Some(summary) = stored else None => record { status: 404, headers: [], body: "" };
    record { status: 200, headers: [], body: repository + ": " + summary }
}
```

A let-else is `let PATTERN (: TYPE)? = value else ARM;`, where the arm is a
pattern, `=>`, and an expression, as in a `match`. Its two patterns are the
two variants of one built-in family, in either order: `Ok(p)` and `Err(q)`, or
`Some(p)` and `None`. A pattern's payload binder is an identifier or the
wildcard `_`, which binds nothing (section 7); `None` binds nothing either.
Unlike a `match` pattern, a let-else pattern does not nest (`K3014`, below).
A `Some`, `Ok`, or
`Err` written after
`let` without `(` is still a binding of that reserved name (`K2002`), and a
variant pattern without an `else` arm is `K1002`.

It is exactly

```text
match value { PATTERN => { rest of the block }, ARM }
```

where the rest of the block is every statement after the let-else and the
block's final expression, except that the pattern's binder is declared in
the enclosing block as a `let` binding is (section 7). The value evaluates
first. If it is the pattern's variant, the binder names its payload for the
rest of the block. If it is the arm's variant, the arm's binder names its
payload, the arm's expression evaluates, and the enclosing function returns
its value immediately: nothing after the let-else in that function call
evaluates, while effects that already happened stay performed.

The annotation, when present, is the type of the value, as it is in a `let`
declaration; the value must be an `Option` (for `Some` and `None`) or a
`Result` (for `Ok` and `Err`), otherwise checking reports `K3005`. Because the
arm's value is the function's return value, it must have the function's
return type, declared or inferred (`K3001` otherwise). A let-else adds the
effects and capability requirements of its value and its arm, and nothing of
its own.

`K3014` reports a let-else that cannot work this way, with one note naming
the fix:

- it is not a direct statement of the body block of a named function
  (including a `webhook`, `queue`, or `schedule` entrypoint): at the top level,
  in an anonymous function, or in a nested block such as a block expression,
  an `if` branch, or a `match` arm, it would leave an expression rather than
  return from a function;
- its two patterns are not the two variants of one family;
- a pattern nests another variant or list pattern in its payload
  (`let Ok(Some(x)) = ...`): bind the payload, then decide it with another
  let-else or a `match` (section 11), whose patterns nest;
- its value ends in `}`, such as an `if`, `match`, block, `record`, or
  anonymous function: in `let Ok(x) = if c { a } else { b } else Err(e) =>
  ...`, the first `else` would read as part of the let-else. Bind the value
  with `let` first, or wrap it in parentheses.

Unlike `?`, a let-else works in a webhook entrypoint, which returns
`HttpResponse`, and in any named function whatever its return type. An
`else` followed by another language's guard clause, `else { return value; }`,
is `K1112` (section 4.2), and any other block after `else` is `K1003`, with a
note showing the arm to write instead.

Core IR lowers a let-else to a variant match on the value in the enclosing
block, followed by the binding of its result. The pattern's arm yields the
payload (unit for `None`); the other arm binds the arm's binder, evaluates its
expression, and ends with a verified function exit (`TYPES-AND-EFFECTS.md`,
"Let-else"), which components lower to a WebAssembly `return`
(`WASM-SANDBOX.md`). A body of many let-else statements therefore nests no
deeper than one of them.

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

The `record` prefix distinguishes a record literal from a block. Fields
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

The only list decomposition construct in edition 2026 is matching:

```krit
match items {
    [] => 0,
    [head, ..tail] => head + sum(tail),
}
```

A `match` evaluates its subject once, then the value of the one arm whose
pattern matches the subject's value:

```krit
match state_get("agent-work", key) {
    Ok(Some(saved)) => saved,
    Ok(None) => "missing",
    Err(error) => "failed: " + error,
}
```

| Pattern | Matches | Binds |
|---|---|---|
| `name` | any value | `name`, to the value |
| `_` | any value | nothing |
| `Some(p)`, `Ok(p)`, `Err(p)` | that variant, when its payload matches `p` | what `p` binds |
| `None` | `None` | nothing |
| `[]` | the empty list | nothing |
| `[head, ..tail]` | a non-empty list | `head` to the first element, `tail` to the remaining immutable list |

A variant pattern's payload is a pattern, so patterns nest through the
built-in families: `Ok(Some(value))`, `Some(Err(_))`, `Ok([head, ..rest])`.
The head and tail of a list pattern are each a name or `_` (`K1003`
otherwise); to look inside the head, match it in the arm. A name binds the
value at its position for the arm's value only, and the head and tail cannot
be the same name (`K2002`). `_` binds nothing (section 7). There are no
literal patterns, guards, or record patterns: compare values with `if` and
`==`. Other tokens where a pattern starts, an unknown variant such as
`Foo(x)`, `None(x)`, a `Some`, `Ok`, or `Err` without its parenthesized
payload, and list patterns of any other shape are `K1003` syntax errors.

The arms of a match name every outcome exactly once. Checking enforces four
rules:

- **One family at each position.** The patterns that reach the same position
  of the value are all `Some` and `None`, all `Ok` and `Err`, or all `[]` and
  `[head, ..tail]` (`K1003`), and that family must be the static type at the
  position, the subject's type or a payload type (`K3005`).
- **No overlap.** No value matches two arms. An arm that only matches values
  that earlier arms already match is a duplicate or unreachable arm, and an
  arm that also matches what an earlier arm matches overlaps it: both are
  `K1003`. `Ok(Some(v)) => a, Ok(other) => b` is rejected with a note to name
  the remaining outcome instead: `Ok(None) => b`.
- **No catch-all arm.** An arm whose whole pattern is a name or `_` would
  stand for every outcome the other arms leave unnamed, so it is `K3005`, with
  a note naming the arms to write instead. A name or `_` inside a payload
  (`Err(_)`, `Some(value)`) names its variant and is allowed.
- **Exhaustive.** Every value of the subject's type matches some arm.
  Otherwise checking reports `K3005`, whose message lists every missing
  pattern verbatim, with `_` for whatever no arm distinguishes, such as
  ``non-exhaustive match; missing: `Ok(None)`, `Err(_)` ``.

Because arms never overlap, their order never changes what a match means, and
they may appear in any order, including `[head, ..tail]` before `[]`. Every
arm's value has the match's type. A match adds the effects and capability
requirements of its subject and of every arm's value, and nothing of its own.
`TYPES-AND-EFFECTS.md` ("Matching") gives the order of these checks.

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
- `?` outside a named function that returns its operand's family, or with a
  different error type (section 8.2)
- a let-else that is not a direct statement of a named function's body, whose
  patterns are not the two variants of one family, or whose value ends in `}`
  (section 8.3)
- `match` arms that mix families at a position, overlap, include a top-level
  catch-all, or leave an outcome unnamed (section 11)

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
diagnostic. Such a program also fails with `K3012` when a `?` outside every
function meets `Err` or `None`, and with `K3014` when a let-else outside every
function meets its arm's variant.

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
  Calls, field access, postfix `?`, unary operators, generic type brackets,
  and patterns remain compact (`load(id)?.body`, `-count? - 1`,
  `let Ok(value) =`, `Ok(Some(value)) =>`, `[_, ..rest] =>`). Match arms keep
  their source order.
- A let-else stays on one line when the rest of the statement, from `else`
  through `;`, fits on the line where `else` ends without wrapping:
  `let Ok(x) = load(id) else Err(e) => handle(e);`. Otherwise the line breaks
  after `else` and the arm continues on the next line, indented one level. An
  arm that holds a block, a `match`, or a comment always breaks. A value that
  wraps on its own keeps a fitting arm after its closing delimiter
  (`) else Err(e) => handle(e);`).
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
