# Krit 0.3 language specification

**Status:** Normative  
**Edition:** 2026  
**Language version:** 0.3

## 1. Scope

This document defines the source syntax and the meaning of Krit programs in
edition 2026 as implemented by Krit 0.3. It is normative together with:

- `TYPES-AND-EFFECTS.md`: static types, effects, capability requirements,
  and the typed Core form that checking produces;
- `DIAGNOSTICS.md`: every diagnostic code, its severity, and its fix hints;
- `WASM-SANDBOX.md`: which checked programs build into WebAssembly
  components, and how those components run;
- `WEBHOOK-CONTRACTS.md`, `AI-OBSERVABILITY.md`, `DURABLE-STATE.md`,
  `JOBS-AND-STORAGE.md`, `DATABASE.md`, and `CACHE-AND-SEARCH.md`: the host
  operations and entrypoint contracts that section 13 summarizes.

One program has one meaning. `krit check` decides whether a program is valid,
and every other command accepts exactly the programs it accepts: `krit run`
checks first and refuses what checking refuses (section 14), and
`krit check --target component` reports exactly the build diagnostics that
`krit build` would (`DIAGNOSTICS.md`, Build-target diagnostics). The direct
evaluator provides no host other than standard output, so host operations and
entrypoints run only through a validated component. Component generation
supports a documented subset of the checked language and refuses the rest
with a diagnostic, never with a different meaning: where both can run a
program, they agree on its result, apart from the fail-closed JSON decoding
specialization in section 13.

Edition 2026 includes, since Krit 0.3, early exits with postfix `?` and
let-else, nested patterns with `_`, named construction of the contract
records, entry lists, the unit value `()`, doc comments, string interpolation,
text blocks, and the first standard library. Each is defined in terms of the
core language (section 4.3).

The historical Racket S-expression syntax is not Krit.

## 2. Source text

A source file is UTF-8 and conventionally ends in `.krit`.

- Keywords and identifiers are case-sensitive.
- Identifiers use ASCII letters, digits, and `_`.
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
Err None Ok Some ai_invoke cache_delete cache_get cache_put checkpoint_get checkpoint_put config_string db_begin_read db_begin_write db_commit db_execute db_query db_rollback http_request int_to_string json_decode json_encode log_error log_info object_delete object_get object_put print println queue_publish replay_ai replay_http search_query secret state_delete state_get state_put string_byte_len string_contains string_ends_with string_is_empty string_starts_with url_path_segment url_query vector_search
```

A binding cannot use a keyword or reserved built-in name.

Some identifiers have a meaning only in one position and stay ordinary names
everywhere else:

- `queue` and `schedule` start an entrypoint declaration when a string
  literal and `fn` follow them (section 13.2);
- `HttpRequest`, `HttpResponse`, `HttpHeader`, and `LogField` start a named
  construction when `{` follows them (section 8.4);
- type names such as `Int`, `String`, `List`, and `Result` are recognized
  only where a type is expected (section 4);
- the words of constructs from other languages, such as `return` and `for`,
  are reported with their own codes only where no valid program could use
  them (section 4.2).

### 2.1 Doc comments

The **doc comment** of an item is the run of `///` lines directly before it.
Items are function declarations, nested ones included, and `webhook`,
`queue`, and `schedule` entrypoint declarations.

```krit
/// Labels one issue.
///
/// Fails when the title is empty.
fn label(title: String) -> Result<String, String> {
    if string_is_empty(title) {
        Err("the issue has no title")
    } else {
        Ok("triage: " + title)
    }
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

Both grammars use EBNF. `,` separates the parts of a sequence, `|`
separates alternatives, `*` means zero or more, `?` means optional, quoted
text is literal, `(* ... *)` is a comment, and `? ... ?` describes a set of
characters in prose.

Source text is a sequence of tokens separated by whitespace and comments.
The lexer always reads the longest token that can start at a position, so
`==` is one token, not two `=`, and `..` is not two `.`.

```text
token       = keyword | identifier | integer | string | text-block
            | punctuator ;

letter      = "A" | ... | "Z" | "a" | ... | "z" ;
digit       = "0" | ... | "9" ;
hex-digit   = digit | "A" | ... | "F" | "a" | ... | "f" ;
line-break  = "\n" | "\r\n" | "\r" ;
whitespace  = ? any Unicode White_Space character ? ;

keyword     = "else" | "false" | "fn" | "if" | "let" | "match"
            | "record" | "true" | "webhook" ;
identifier  = (letter | "_"), (letter | digit | "_")* ;
                                    (* other than a keyword *)
integer     = digit, ("_"?, digit)* ;

string      = '"', (string-character | escape | hole)*, '"' ;
string-character
            = ? any character other than '"', "\", or a line break ? ;
escape      = "\", ( '"' | "\" | "n" | "r" | "t" | "0"
                   | "u{", hex-digit, hex-digit?, hex-digit?, hex-digit?,
                     hex-digit?, hex-digit?, "}" ) ;
hole        = "\(", identifier, (".", identifier)*, ")" ;

text-block  = '"""', line-break,
              (content-line, line-break)*,
              indentation, '"""' ;
content-line
            = (text-block-character | escape | hole)* ;
text-block-character
            = ? any character other than "\" or a line break ? ;
indentation = (" " | "\t")* ;

comment     = "//", comment-character* ;
doc-comment = "///", (? any character other than "/" or a line break ?,
                      comment-character*)? ;
comment-character
            = ? any character other than a line break ? ;

punctuator  = "(" | ")" | "{" | "}" | "[" | "]" | "," | ":" | ";"
            | "." | ".." | "?" | "=" | "=>" | "->"
            | "==" | "!=" | "<" | "<=" | ">" | ">=" | "!"
            | "+" | "-" | "*" | "/" | "%" | "&&" | "||" ;
```

A `doc-comment` is a `comment` too: both are trivia, which the syntactic grammar
does not mention. Any other character outside a string, text block, or
comment, including a lone `&` or `|`, is `K0001`.

A `hole` inside a string or text block is written without whitespace, and its
identifiers are not keywords. A string that contains at least one hole is an
**interpolated string**; section 9.1 gives its meaning. `\(` followed by
anything other than an identifier path and `)` is `K3015` at the hole, which
the lexer reports while it reads the source, with one note:
`\(f(x))`, `\(a + b)`, `\("x")`, `\(1)`, `\(true)`, `\( name )`, `\()`, and
a hole whose line ends before its `)`. A quoted string inside a hole is
skipped while looking for the hole's `)`, so `\(f("x"))` is one invalid hole.
A string without holes is an ordinary string literal, and `\\(` is a
backslash followed by `(`, never a hole.

A **text block** starts with `"""` followed immediately by a line break; any
other text after the opening `"""`, even a space, is `K0004`. It ends at the
first line whose first characters other than spaces and tabs are `"""`; that
line's spaces and tabs are the block's **indentation**, and the closing
`"""` may be followed by more tokens on its line (`""")`). The lines between
the two delimiters are its content lines:

- Every content line must begin with the indentation, character for
  character, which is removed from it. A line that does not is `K0004` at its
  start, unless it holds only spaces and tabs, which makes it an empty line.
  Spaces and tabs after the indentation are kept.
- The content lines are joined by `\n`, whatever line breaks the source
  uses, so a file with CRLF line endings means exactly what it means with LF
  line endings. The line break before the closing delimiter is not part of
  the text, so a block with no content lines is the empty string.
- Escapes and holes work in content lines as in strings, within one line: a
  line cannot end with `\` (`K0003`). A `"` or `""` needs no escape, but
  `"""` anywhere in a content line is `K0004` (write `\"""` for three quote
  characters).
- A text block that no line closes is `K0002`, or `K0004` at the first
  `"""` inside one of its lines when there is one, since that `"""` was most
  likely meant to close it.

```krit
fn prompt(title: String, body: String) -> String {
    """
    Classify this issue as "bug" or "feature".
      Title: \(title)

    \(body)
    """
}
```

The value of `prompt("Crash", "It fails.")` is
`Classify this issue as "bug" or "feature".\n  Title: Crash\n\nIt fails.`.

Integer separators cannot be leading, trailing, or repeated. Integers are
signed through unary `-`; the sign is not part of the token.

`?` is a token of its own: the postfix error-propagation operator (section
8.2). After an operand, the parser also uses it to recognize another
language's conditional operator (section 4.2).

Strings and text blocks support the escapes of `escape`: `\"`, `\\`, `\n`,
`\r`, `\t`, `\0`, and `\u{...}` with one to six hex digits naming a Unicode
scalar value. `\(` starts a hole instead. Other escapes and invalid Unicode
scalar values are lexical errors (`K0003`). A one-line string cannot contain
a line break (`K0002`); a text block holds several lines.

## 4. Syntactic grammar

```text
program         = item* ;
item            = statement
                | webhook-declaration
                | delivery-declaration ;

statement       = let-declaration
                | let-else
                | function-declaration
                | expression, ";" ;
block           = "{", statement*, expression?, "}" ;

let-declaration = "let", identifier, (":", type)?, "=", expression, ";" ;
let-else        = "let", variant-pattern, (":", type)?, "=", expression,
                  "else", variant-pattern, "=>", expression, ";" ;
variant-pattern = ("Some" | "Ok" | "Err"), "(", identifier, ")"
                | "None" ;

function-declaration
                = "fn", identifier, signature, block ;
webhook-declaration
                = "webhook", "fn", identifier, signature, block ;
delivery-declaration
                = ("queue" | "schedule"), direct-string, "fn", identifier,
                  signature, block ;
direct-string   = string ;                   (* without holes *)
signature       = "(", parameters?, ")", ("->", type)? ;
parameters      = parameter, (",", parameter)*, ","? ;
parameter       = identifier, (":", type)? ;

expression      = logical-or ;
logical-or      = logical-and, ("||", logical-and)* ;
logical-and     = equality, ("&&", equality)* ;
equality        = comparison, (("==" | "!="), comparison)* ;
comparison      = term, (("<" | "<=" | ">" | ">="), term)* ;
term            = factor, (("+" | "-"), factor)* ;
factor          = unary, (("*" | "/" | "%"), unary)* ;
unary           = ("!" | "-"), unary
                | call ;
call            = primary, ("(", arguments?, ")" | ".", identifier | "?")* ;
arguments       = expression, (",", expression)*, ","? ;

primary         = integer
                | string
                | text-block
                | "true"
                | "false"
                | "(", ")"
                | "(", expression, ")"
                | identifier
                | list
                | record
                | construction
                | block
                | if-expression
                | match-expression
                | function-expression ;

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

if-expression   = "if", expression, block, "else", (block | if-expression) ;
function-expression
                = "fn", signature, block ;
match-expression
                = "match", expression, "{", match-arm, (",", match-arm)*,
                  ","?, "}" ;
match-arm       = pattern, "=>", expression ;
pattern         = binder
                | "_"
                | ("Some" | "Ok" | "Err"), "(", pattern, ")"
                | "None"
                | "[", "]"
                | "[", (binder | "_"), ",", "..", (binder | "_"), "]" ;
binder          = identifier ;               (* other than "_" *)

type            = "Int" | "Bool" | "String" | "Unit" | "Secret"
                | "HttpHeader" | "HttpRequest" | "HttpResponse" | "LogField"
                | "QueueJob" | "ScheduleEvent" | "DatabaseTransaction"
                | "List", "<", type, ">"
                | "Option", "<", type, ">"
                | "Result", "<", type, ",", type, ">"
                | "Record", "{", record-type-fields?, "}" ;
record-type-fields
                = record-type-field, (",", record-type-field)*, ","? ;
record-type-field
                = identifier, ":", type ;
```

The grammar is complete: every program `krit check` accepts is derived from
`program`, and the rules below only restrict which derivations are valid or
decide between two readings of the same tokens.

- **Statements end with `;`.** Every expression statement ends with `;`,
  including an `if`, `match`, or block written for its effects
  (`if ready { log() } else { () };`). Only the final expression of a block,
  which is the block's value, has none. A program has no final expression:
  every top-level expression is a statement.
- **Precedence.** From loosest to tightest: `||`; `&&`; `==` and `!=`; `<`,
  `<=`, `>`, and `>=`; `+` and `-`; `*`, `/`, and `%`; unary `!` and `-`;
  then the postfix call, field-access, and `?` links of `call`. Every binary
  operator associates to the left, so `a - b - c` is `(a - b) - c`, and
  `a < b < c` parses as `(a < b) < c`, which checking rejects because it
  orders a `Bool` (`K3001`).
- **`fn` and `let`.** In statement position, `fn` followed by a name is a
  function declaration and `fn` followed by `(` is a function expression. A
  `let` followed by `None`, or by `Some`, `Ok`, or `Err` and then `(`, is a
  let-else; any other `let` is a let declaration.
- **Names.** `identifier` excludes the keywords of section 2; `queue`,
  `schedule`, the contract names, and the type names are identifiers that
  have a meaning only in the positions this grammar gives them (section 2).
  Any other identifier where a type is expected is `K1002`.
- **Entrypoints.** A `webhook` or delivery (`queue`, `schedule`) declaration
  is valid only as a program item: inside a block it is `K1004`. A module
  declares at most one entrypoint of any kind (`K2002`). `queue` and
  `schedule` start a delivery declaration only when a string and `fn` follow
  them; that string must be a direct string literal, and an interpolated
  string or text block there is `K1002` (section 13.2).
- **let-else patterns.** The payload of a let-else pattern is a name or `_`.
  A variant or list pattern there is `K3014`, a let-else without its `else`
  arm is `K1002`, and a block after its `else` is `K1003`, each with a note
  (section 8.3).
- **Match arms.** A `match` has at least one arm (`K1003`), and arms are
  separated by commas even when an arm's value is a block.
- **Conditionals.** Every `if` has an `else`; an `if` without one is
  `K1002`. `else if` continues a chain (section 10).

`()` is the unit value (section 5). A list whose first element is an
`entry` is an entry list (section 11); every element after it must be an
entry too, and a list whose first element is a plain expression cannot
contain one. Mixing the two forms is `K1001` at the first element written the
other way, with a note. `[]` is the empty list, entry lists included, and
`[:]` is `K1001`. In a type the unit type is spelled `Unit`: `-> ()` is
`K1002`, with a note.

A `contract-name` is an ordinary identifier, not a keyword; only these four
names followed by `{` start a `construction` (section 8.4), and any other
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
the parser reports them. The early exits are postfix `?` (section 8.2) and
let-else (section 8.3): the grammar accepts a let-else wherever a `let`
declaration may appear, and checking accepts it only as a direct statement of
a named function's body.

A source module contains zero or one webhook. The parser retains its written
parameter and result annotations; the static checker requires exactly one
`HttpRequest` parameter and an `HttpResponse` result (`K3007` otherwise):

```krit
webhook fn handle(request: HttpRequest) -> HttpResponse {
    HttpResponse { status: 200, body: request.path }
}
```

The source name is a compiler fact. The component host exports the declaration
through the canonical `krit:runtime/webhook@0.2.0` interface's `handle`
operation; the name does not create an ambient network listener.

### 4.1 Nesting limits

A source file may nest syntax at most **256 levels** deep. Each of the
following opens one level for what it contains:

- a parenthesized expression, a block, a list literal (an entry list
  included), a record literal, a named construction, and an interpolated
  string or text block, for its holes
- the argument list of a call, and each call, field-access, or `?` link of a
  postfix chain (`a.b(c)?.d` opens four levels for what follows `a`),
  including the field accesses in a hole (`"\(a.b.c)"` opens three levels
  for `a`: the string and two links)
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

### 4.2 Constructs from other languages

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
  that depth, and is not the `:` of an entry: directly inside a list
  literal's `[`, the first `:` after the `[` or after a `,` separates an
  entry's key from its value (section 11), so `[pick()?(k): v]` is an entry
  list. No valid program has a branch end, since a first branch contains
  none of those outside brackets. What follows the `?` decides:
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

### 4.3 Sugar and its meaning

Edition 2026 has a small core: literals, names, operators, calls, field
access, `let`, functions, blocks, `if`, `match` over the built-in families,
lists, and records. Every other form is sugar, defined by the translation into
the core in the table below. A translation gives the form its meaning: the
sugared program evaluates exactly as its translation does, in the same order
and with the same effects, and checking gives it the same types, effects, and
capability requirements. Translations are not textual rewrites. Diagnostics
refer to the source as written, the formatter keeps the sugar, and an
implementation may represent a form natively when the meaning is the same;
Krit's Core IR, for example, represents the early exit of `?` and let-else
with a verified `exit` operation (`TYPES-AND-EFFECTS.md`).

Two pseudo-forms appear only in translations: `return v` ends the call of the
innermost enclosing named function with the value `v`, and `rest` stands for
the statements and final expression that follow a statement in its block.
Names that start with `$` are fresh names the program can neither write nor
see (Core IR uses the same debug names); `P` and `Q` stand for patterns,
`a` and `b` for the program's own binders, and the other letters for
expressions.

| Sugar | Meaning | Section |
|---|---|---|
| `e?`, where `e` is a `Result` | `match e { Ok($value) => $value, Err($error) => return Err($error) }` | 8.2 |
| `e?`, where `e` is an `Option` | `match e { Some($value) => $value, None => return None }` | 8.2 |
| `let P = e else Q => r; rest` | `match e { P => { rest }, Q => return r }`, with `P`'s binder declared in the enclosing block as a `let` declares its name | 8.3 |
| `match e { Ok(Some(a)) => x, Ok(None) => y, Err(b) => z }` | `match e { Ok($payload) => match $payload { Some(a) => x, None => y }, Err(b) => z }`: one match per position of a nested pattern | 11 |
| `HttpResponse { status: s, body }` | `record { status: s, headers: [], body: body }`: every field of the contract record in its declared order, with the spec-fixed default for each field not written; written values evaluate in source order | 8.4 |
| `["k": v, "l": w]` | `[record { name: "k", value: v }, record { name: "l", value: w }]` | 11 |
| `"a\(x.y)b"` | `"a" + x.y + "b"` | 9.1 |
| a text block | the string of its content lines, dedented and joined by `\n`, with its holes translated as in an interpolated string | 3, 9.1 |
| `if a { x } else if b { y } else { z }` | `if a { x } else { if b { y } else { z } }`, without the extra nesting level | 4.1, 10 |

In a pattern, `_` names no outcome, so a match stays exhaustive and disjoint
(section 11); in edition 2026 it still binds a discard (section 7). The unit
value `()` is a literal of the core, not sugar, and a doc comment `///` is
trivia with no meaning in evaluation (section 2.1).

## 5. Program and block values

Items execute in source order.

- A declaration evaluates to unit.
- An expression followed by `;` evaluates for effects and then produces unit.
- A block's final expression without `;` is the block value.
- An empty block or a block ending in a statement produces unit.
- A file has no value and prints nothing by itself; output comes only from
  `print` and `println` (section 12).
- A `?` or let-else exit ends the enclosing function call early (sections
  8.2 and 8.3); the statements after it in that call do not run.

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
- opaque host secret handle (`Secret`)
- opaque host database transaction handle (`DatabaseTransaction`)
- function closure

Integer overflow is a runtime error (`K4005`). Division truncates toward
zero. Division or remainder by zero is a runtime error (`K4004`).

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
exposing bytes to guest code, and only into an origin the manifest binds the
secret to (`CAPABILITIES.md`, "Secret binding"). A `DatabaseTransaction` is
opaque in the same way and may appear only as the first argument of a
database operation (section 13.1).

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
parameters, and pattern binders in one scope may use it, including both names
of one `[_, .._]` list pattern. A program should bind `_` only for a value it
does not use. Reading `_` as a value, that is, an identifier expression `_`
that resolves to a binding, is valid in edition 2026 and refers to the most
recent `_` binding in scope, exactly as shadowing does, but checking reports
warning `K2003`; edition 2027 makes it an error. An identifier expression `_`
with no `_` binding in scope is an undefined name (`K2001`). Warnings never
change whether a program checks or runs (`DIAGNOSTICS.md`).

In a pattern (sections 8.3 and 11), `_` is also a wildcard: the rules that
keep a `match`'s arms disjoint and exhaustive treat it as matching any value
without naming an outcome, so `Err(_)` names `Err` and `_ =>` is a catch-all.
In edition 2026 a pattern's `_` still binds the value at its position as a
discard binding, so reading `_` in the arm of a `match` or
let-else, or after a let-else pattern, refers to the pattern's own `_` (in
`[_, .._]`, the later one, the tail) with warning `K2003`. Edition 2027 makes
every read of `_` an error, which leaves a pattern's `_` a pure wildcard.

A lexical binding shadows a built-in name of the same spelling for the
binding's whole scope, in checking and evaluation alike. `let`, function, and
parameter names cannot reuse a built-in name (`K2002`). A match binder can
reuse a built-in name that the parser does not reserve, such as `state_get`,
and within its arm the name refers to the binder.

Function declarations are visible from their declaration to the end of their
containing scope and may call themselves recursively. A function is not
visible before its declaration, so two functions cannot call each other: the
earlier one's reference to the later one is an undefined name (`K2001`).

Entrypoint declarations (`webhook`, `queue`, and `schedule`) introduce a
function binding with the same lexical visibility and recursion rules as an
ordinary function declaration. The modifier additionally marks that function
as the source module's single exported host entrypoint.

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

A call evaluates its callee, then its arguments from left to right, then the
function's body with each parameter bound to its argument. The argument count
must equal the parameter count (`K3003`), and only a function can be called
(`K3002`); checking reports either violation before evaluation (section 14).
A function's value is its body block's value, unless a `?` or let-else in
that body exits early (sections 8.2 and 8.3).

Function values and the opaque `Secret` and `DatabaseTransaction` values
cannot be compared, including when nested in an ordinary structural value
(`K3006`, `K3009`, `K3010`).

Annotations may be written on let bindings, parameters, and function return
values:

```krit
let request: Record { path: String, retry: Option<Int> } = record {
    path: "/events",
    retry: Some(2),
};

fn attempt(value: Int) -> Result<Int, String> {
    Ok(value)
}
```

Annotations are parsed and retained by the compiler. `krit check` enforces
them through static analysis without executing the program. `krit run`
performs the same checking before it evaluates anything and refuses a
program that fails it (section 14), so annotations hold in direct evaluation
too. It then refuses checked programs that need an entrypoint or host
contract (`K5003`), so unavailable hosts fail explicitly rather than
fabricating a value. Static checking is specified in `TYPES-AND-EFFECTS.md`.

### 8.1 Webhook contract types

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
alias syntax. `LogField` (section 13.1) has the same shape as `HttpHeader`,
and the delivery contracts `QueueJob` and `ScheduleEvent` are defined in
section 13.2.

### 8.2 Error propagation

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

### 8.3 Let-else

A let-else binds the payload of one variant and makes the enclosing function
return when the value is the other variant:

```krit
fn failure(error: String) -> HttpResponse {
    HttpResponse { status: 502, body: error }
}

webhook fn handle(request: HttpRequest) -> HttpResponse {
    let Ok(repository) = config_string("reference.repository") else Err(error) => failure(error);
    let Ok(stored) = state_get("agent-work", request.path) else Err(error) => failure(error);
    let Some(summary) = stored else None => HttpResponse { status: 404 };
    HttpResponse { status: 200, body: "\(repository): \(summary)" }
}
```

A let-else is `let PATTERN (: TYPE)? = value else ARM;`, where the arm is a
pattern, `=>`, and an expression, as in a `match`. Its two patterns are the
two variants of one built-in family, in either order: `Ok(p)` and `Err(q)`, or
`Some(p)` and `None`. A pattern's payload binder is an identifier or the
wildcard `_`, a discard binding that the program should not read (section 7:
reading it is warning `K2003`); `None` binds nothing. Unlike a `match`
pattern, a let-else pattern does not nest (`K3014`, below). A `Some`, `Ok`, or
`Err` written after `let` without `(` is still a binding of that reserved name
(`K2002`), and a variant pattern without an `else` arm is `K1002`.

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
- its value ends in `}`, such as an `if`, `match`, block, `record`, named
  construction, or anonymous function: in `let Ok(x) = if c { a } else { b }
  else Err(e) => ...`, the first `else` would read as part of the let-else.
  Bind the value with `let` first, or wrap it in parentheses.

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

### 8.4 Named construction

The contract records `HttpRequest`, `HttpResponse`, `HttpHeader`, and
`LogField` (section 13.1) can be built by name:

```krit
fn ready(body: String) -> HttpResponse {
    HttpResponse { status: 200, body }
}

let fetch = HttpRequest { method: "GET", path: "/issues/1" };

println(fetch.query == "");
println(ready("done").headers == []);
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

A binary operator evaluates its left operand and then its right one, except
that `&&` and `||` short-circuit. Precedence and associativity follow the
grammar (section 4).

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

`&&` evaluates its right operand only when the left one is `true`, and `||`
only when it is `false`. All other binary operators evaluate both operands.

Equality is structural for integers, booleans, strings, unit, lists, records,
`Option`, and `Result`. Comparing a function, a `Secret`, or a
`DatabaseTransaction`, directly or inside a composite value, is rejected
statically (`K3006`, `K3009`, `K3010`). Every operator of this table lowers to
WebAssembly for the operand kinds that have a component layout, including
string `+`, `==`, `!=`, and ordering; equality of lists, records, options,
and results does not build yet (`WASM-SANDBOX.md`, "Support checking").

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

### 9.1 Interpolated strings and text blocks

An interpolated string or a text block (section 3) is an expression of type
`String`: the concatenation of its text and its holes from left to right.
`"\(method) \(request.path)"` means `method + " " + request.path`, and a
text block with holes means the same concatenation of its dedented lines.

- Each hole names a value in scope, optionally followed by field accesses,
  and that value must be a `String`. Any other type is `K3015` at the hole,
  with one note; for an `Int`, the note shows how to convert it with
  `int_to_string` (section 12.1) into a name first. There are no implicit
  conversions (section 9).
- A hole that names an undefined value is `K2001` at the name, and reading
  `_` in a hole is the `K2003` warning, as anywhere else.
- Holes are names, so evaluating one has no effect and the parts' order is
  never observable.
- An interpolated string or text block is never a direct string literal, so
  it is never a capability resource (section 13.1): `config_string("agent.\(key)")`
  and an adapter name written as a text block are both `K3008`. A text block
  without holes is not a literal either.

## 10. Conditionals

```krit
fn sign(value: Int) -> String {
    if value < 0 {
        "negative"
    } else if value == 0 {
        "zero"
    } else {
        "positive"
    }
}

println(sign(-3));
```

The condition must be a `Bool` (`K3001`); nothing is truthy. Only the selected
branch evaluates. Both branches must have the same type, which is the type of
the `if` (`K3001` otherwise). Every `if` has an `else`. `else if` continues a
chain: it means an `else` block holding the next `if`, and a continuation does
not add a nesting level (section 4.1), so a long chain stays flat.

## 11. Records, lists, and matching

Record literal and field access:

```krit
let response = record { status: 200, body: "ready" };

println(response.status);
```

The `record` prefix distinguishes a record literal from a block; a named
construction (section 8.4) starts with a contract name instead. Fields
evaluate from left to right. Accessing a field requires a record containing
that field.

Values have no methods. `value.name(arguments)` reads field `name` of the
record `value` and calls the function stored there. When `value` has no such
field, because it is not a record or is a record without that field, checking
reports the method call (`K3011`) with the standard-library function
(section 12.1) to use instead when there is one, such as `string_byte_len(s)`
for `s.len()`.

A list literal evaluates its elements from left to right:

```krit
println([1, 2, 3]);
```

An **entry list** writes a list of name/value records, such as HTTP headers
and log fields:

```krit
let id = "req-1";
let headers = ["accept": "application/json", "x-request-id": id];
let spelled = [
    HttpHeader { name: "accept", value: "application/json" },
    HttpHeader { name: "x-request-id", value: id },
];

println(headers == spelled);
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

The only list decomposition construct in edition 2026 is matching:

```krit
fn sum(items: List<Int>) -> Int {
    match items {
        [] => 0,
        [head, ..tail] => head + sum(tail),
    }
}

println(sum([10, 20, 12]));
```

A `match` evaluates its subject once, then the value of the one arm whose
pattern matches the subject's value:

```krit
fn saved_summary(key: String) -> String {
    match state_get("agent-work", key) {
        Ok(Some(saved)) => saved,
        Ok(None) => "missing",
        Err(error) => "failed: " + error,
    }
}
```

| Pattern | Matches | Binds |
|---|---|---|
| `name` | any value | `name`, to the value |
| `_` | any value | nothing to read: a discard binding (section 7) |
| `Some(p)`, `Ok(p)`, `Err(p)` | that variant, when its payload matches `p` | what `p` binds |
| `None` | `None` | nothing |
| `[]` | the empty list | nothing |
| `[head, ..tail]` | a non-empty list | `head` to the first element, `tail` to the remaining immutable list |

A variant pattern's payload is a pattern, so patterns nest through the
built-in families: `Ok(Some(value))`, `Some(Err(_))`, `Ok([head, ..rest])`.
The head and tail of a list pattern are each a name or `_` (`K1003`
otherwise); to look inside the head, match it in the arm. A name binds the
value at its position for the arm's value only, and the head and tail cannot
be the same name (`K2002`). `_` is a discard binding, which the program
should not read: reading it is warning `K2003` (section 7). There are no
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

## 12. Built-in functions and output

Every reserved built-in name (section 2) is one of:

- a variant constructor, `Some`, `Ok`, and `Err`, or the value `None`;
- an output function, `print` or `println` (this section);
- a standard-library function (section 12.1);
- a JSON conversion, `json_encode` or `json_decode` (section 13);
- a host operation, such as `config_string` or `http_request`
  (section 13.1).

Each built-in is a value: it can be bound and passed like any function, and
it takes part in checking with a fixed type. Host operations additionally
require direct literal resources (section 13.1), and component targets
require every built-in other than `print` and `println` to be called
directly (`WASM-SANDBOX.md`).

`print` writes its argument without a line break, and `println` writes it
followed by one line feed. Both return unit, and the argument evaluates
before anything is written:

```krit
let value = 42;

print(value);
println(value);
```

Output rendering is deterministic:

- booleans: `true` or `false`
- integers: base ten without separators
- strings: raw UTF-8 contents at the top level; a string inside a list,
  record, or variant is written quoted, with escapes
- unit: `()`
- lists: `[item, item]`
- records: `record { field: value }` in the record's field order
- variants: `Some(value)`, `None`, `Ok(value)`, or `Err(value)`
- functions: `<function>` or `<function name>`

Output is the only host effect the direct evaluator can perform. It is
modeled as `io.stdout`. The component runtime additionally provides the
bounded `config.read`, `secret.read`, `http.request`, `ai.invoke`, and
`observe.log` interfaces plus `state.transaction`, `queue.publish`,
`object.read`, `object.write`, `database.read`, `database.write`,
`cache.read`, `cache.write`, `search.query`, and `search.vector` to typed
webhook, queue, and schedule artifacts (section 13.1).

### 12.1 Standard library

The standard library is a fixed set of built-in functions, first shipped in
Krit 0.3 (standard library v0). Each is pure: it has no effect, needs no
capability, and returns the same result for the same arguments in every
implementation and target, in `krit run`, the language server, and every
component. Names are flat, prefixed by the kind of value they work on:

| Function | Type |
|---|---|
| `int_to_string` | `fn(Int) -> String` |
| `url_path_segment` | `fn(String) -> Result<String, String>` |
| `url_query` | `fn(List<Record { name: String, value: String }>) -> String` |
| `string_byte_len` | `fn(String) -> Int` |
| `string_is_empty` | `fn(String) -> Bool` |
| `string_contains` | `fn(String, String) -> Bool` |
| `string_starts_with` | `fn(String, String) -> Bool` |
| `string_ends_with` | `fn(String, String) -> Bool` |

They are built-in names (section 2): a `let`, function, or parameter cannot
reuse one (`K2002`), and a match binder may shadow one (section 7). Like
every built-in, each is a function value that can be bound and passed;
component targets require a direct call (`WASM-SANDBOX.md`). Checking
reports a wrong argument type as `K3001` and a wrong argument count as
`K3003`. In a checked program no standard-library call ever fails.

- `int_to_string(n)` writes `n` in base ten exactly as `print` does: ASCII
  digits without separators or leading zeros, preceded by `-` when `n` is
  negative and by nothing otherwise. `int_to_string(-9223372036854775808)`
  is `"-9223372036854775808"`.
- `string_byte_len(s)` is the number of bytes of `s`'s UTF-8 encoding
  (`string_byte_len("é")` is `2`), and `string_is_empty(s)` is
  `string_byte_len(s) == 0`.
- `string_contains(s, part)`, `string_starts_with(s, part)`, and
  `string_ends_with(s, part)` are true when the UTF-8 bytes of `part` occur
  contiguously in those of `s`: anywhere, at its start, or at its end. The
  empty `part` occurs in every string, and a `part` longer than `s` in none.
  Comparison is byte equality, with no case folding or Unicode
  normalization (`string_ends_with("e\u{301}", "é")` is `false`). Both
  strings are valid UTF-8, so a match always begins and ends at character
  boundaries of `s`.
- `url_path_segment(text)` percent-encodes `text` as one URL path segment:
  every byte that is not an RFC 3986 unreserved character (`A`-`Z`, `a`-`z`,
  `0`-`9`, `-`, `.`, `_`, `~`) is written `%HH` with uppercase hex digits,
  each byte of a multibyte character on its own (`"octo cat"` gives
  `"octo%20cat"`, `"café"` gives `"caf%C3%A9"`). It returns `Err` with the
  first of these messages that applies, and `Ok` with the encoded segment
  otherwise:

  | Text | `Err` message |
  |---|---|
  | empty | `path segment is empty` |
  | contains `/`, `\`, or an ASCII control character (U+0000-U+001F, U+007F) | ``path segment contains `/`, `\`, or an ASCII control character`` |
  | contains `%` followed by a hex digit, `u`, or `U` | ``path segment contains `%` followed by a hex digit, `u`, or `U` `` |
  | its part before the first `;`, `?`, or `#`, with spaces trimmed from both ends, is `.` or `..` | ``path segment reads as `.` or `..` `` |

  These are exactly the texts whose encoding the host path rule refuses
  (`WEBHOOK-CONTRACTS.md`, "Request paths and queries"), plus the empty
  text, whose segment would vanish and leave `//` in a path. So every `Ok`
  segment is accepted by the host wherever it stands between two `/`, and
  decoding it once gives back `text`; untrusted text fails recoverably,
  as a value the program handles, instead of making the host refuse the
  request and trap the invocation. Messages never contain the text. Forms
  the host rule does not model, such as a fullwidth `．．`, are encoded and
  accepted.
- `url_query(entries)` builds a query string: `name=value` for each entry
  in list order, joined by `&`, with every name and value percent-encoded
  exactly as `url_path_segment` encodes (a space is `%20`, never `+`), no
  leading `?`, and `""` for the empty list. Entries may be written as an
  entry list (`url_query(["q": term, "page": "2"])`, section 11) or be any
  list of `HttpHeader` or `LogField` records. The result holds only
  unreserved characters, `%`, hex digits, `=`, and `&`, so the host query
  rule accepts every result and `url_query` cannot fail.

```krit
fn issue_request(owner: String, number: Int) -> Result<HttpRequest, String> {
    let segment = url_path_segment(owner)?;
    let issue = int_to_string(number);
    Ok(
        HttpRequest {
            method: "GET",
            path: "/repos/\(segment)/issues/\(issue)",
            query: url_query(["state": "open"]),
        },
    )
}
```

A `webhook` entrypoint, where `?` cannot return (section 8.2), binds the
segment with let-else instead (section 8.3):
`let Ok(segment) = url_path_segment(owner) else Err(reason) => HttpResponse { status: 400, body: reason };`.
A path or query built from text that none of these functions encoded is
warning `K9013` (`DIAGNOSTICS.md`); a segment taken from a
`url_path_segment` result with `?`, let-else, or a `match` counts as
encoded.

There are no list, option, map, or JSON functions in the standard library
yet; `docs/roadmap.md` (section 5.6) plans them. Lists are taken apart with
list patterns (section 11) and recursion.

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
numbers are rejected. Encoding a function, directly or inside another value,
is rejected statically (`K3006`), and so is encoding a `Secret` (`K3009`) or a
`DatabaseTransaction` (`K3010`); in a checked program no value reaches
`json_encode` that it cannot encode. Invalid JSON or JSON without a Krit
representation is `K4009`.

`json_decode` is not schema-directed: it returns whatever value the JSON
text holds, and its static type is inferred from how the result is used. A
decoded value that lacks the kind or fields its use requires fails at that
use with `K4001` (section 14).

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

### 13.1 Host operations

Host operations are the only way a program reaches anything outside itself.
Each is fallible and returns a `Result`, whose `Err` the program must handle
or pass on with `?`:

```text
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
cache_get("lookups", key) // Result<Option<String>, String>
cache_put("lookups", key, value, ttl_seconds) // Result<Unit, String>
cache_delete("lookups", key) // Result<Unit, String>
search_query("docs", query, limit) // Result<String, String>
vector_search("embeddings", vector, limit) // Result<String, String>
```

A direct string literal is a one-line string without holes (section 3);
interpolated strings and text blocks never count as one, so a resource must
be written out in full. Config and secret operations require exactly one
direct string-literal resource argument in canonical-name form (see
`CAPABILITIES.md`); a
configuration key may additionally contain `_`, as in `"agent.timeout_ms"`,
while a secret name may not. A malformed name is `K3008`. `http_request`
requires a direct normalized exact-origin literal, an `HttpRequest`, and
directly `None` or `Some(secret)`. Indirect host
operation use and computed resources are rejected because capability
requirements would not be statically knowable. A configuration read has
effect `config.read` and
requirement pair `("config.read", "agent.model")`. Secret acquisition has
effect `secret.read`; outbound HTTP has `http.request` and the exact origin.
Checking does not relate a secret to the origin it is sent to: the host
injects a secret only into an origin its manifest binding lists, and refuses
any other pairing with `K5006` before a byte is sent (`CAPABILITIES.md`,
"Secret binding").

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

Cache namespaces and search index names are direct canonical literals as
well. `cache_get` adds `cache.read("namespace")`, `cache_put` and
`cache_delete` add `cache.write("namespace")`, and `search_query` and
`vector_search` add `search.query("index")` and `search.vector("index")`.
Both services are optional: a hit, a miss, and an outage are distinct values
the program handles, so no result may depend on the cache being available.
The complete contract is normative in `CACHE-AND-SEARCH.md`.

### 13.2 Typed delivery entrypoints

A module declares at most one `webhook`, `queue`, or `schedule` entrypoint.
A queue consumer handles one durable job:

```krit
queue "render-jobs" fn handle(job: QueueJob) -> Result<String, String> {
    Ok(job.id)
}
```

A schedule handler handles one host-owned trigger:

```krit
schedule "hourly-sweep" fn handle(event: ScheduleEvent) -> Result<String, String> {
    Ok(event.id)
}
```

`queue` and `schedule` are contextual keywords: they introduce an entrypoint
only when directly followed by a canonical name literal and `fn`, so ordinary
bindings, parameters, and record fields may still be spelled `queue` or
`schedule`. An interpolated string or text block followed by `fn` in the
name's place is read as a declaration too, and is `K1002` because the name is
not a direct literal. The declared name is the exact resource of the
`queue.consume("queue")` or `schedule.trigger("schedule")` requirement.

The signatures are exact: `(QueueJob) -> Result<String, String>` and
`(ScheduleEvent) -> Result<String, String>` (`K3007` otherwise). `?` and
let-else work in both bodies (sections 8.2 and 8.3). `QueueJob` is the closed
contract
`Record { queue: String, id: String, body: String, attempt: Int, maxAttempts: Int }`.
`ScheduleEvent` is the closed contract `Record { schedule: String, id: String,
scheduledAtMillis: Int, firedAtMillis: Int, attempt: Int, maxAttempts: Int }`.
Both instants are host-supplied UTC epoch milliseconds; guests have no clock.

`Ok(detail)` acknowledges the delivery and commits every staged mutation with
the acknowledgement in one transaction. `Err(detail)` commits nothing and lets
the host retry or dead-letter under its configured policy. `detail` is bounded
and is never interpreted as control flow.

The source checker does not require a manifest. Package build orchestration
checks requirements against the schema-1 manifest (`K5001` for a missing
grant). `krit run` refuses every program that declares an entrypoint or uses
a host operation with `K5003` before evaluating anything (section 14); it
never substitutes an empty string, environment variable, network response, or
secret bytes. `krit invoke`, `krit serve`, `krit worker`, and `krit schedule`
execute the separately built component with explicit immutable host inputs.

## 14. Evaluation failure

`krit run` first checks the source exactly as `krit check` does: lexing,
parsing, name resolution, type, effect, and capability analysis, and Core
verification. It never runs the WebAssembly target check that `krit check`
adds for `--target` or for entrypoint files (`DIAGNOSTICS.md`, Build-target
diagnostics), because direct evaluation builds no artifact. A program that
fails checking is never evaluated: `run` reports the same diagnostic with the
same exit status as `check` and writes no program output. Warnings that
checking reports, such as `K2003` and `K9013`, are printed exactly as `check`
prints them and never prevent evaluation.

A program that passes checking is still refused before evaluation, with
`K5003` and exit status 4, when it declares a webhook, queue, or schedule
entrypoint or requires any host capability, because direct evaluation
provides no entrypoint invocation and no host other than standard output.

The following are errors rather than implementation-defined behavior.
Checking reports these, so a program containing one is never evaluated:

- malformed UTF-8 or tokens, and invalid strings, holes, or text blocks
- malformed syntax, including constructs from other languages (section 4.2)
- syntax or inferred types nested past the limits of section 4.1
- unresolved or duplicate names
- wrong operand, condition, subject, callee, or annotation type
- incorrect argument count
- comparison of a function or an opaque value
- missing record field, and unknown, repeated, or missing named-construction
  fields (section 8.4)
- an interpolation hole that is not a `String` (section 9.1)
- JSON encoding of a function or an opaque value
- a host-operation resource that is not a direct canonical literal: a
  configuration key, secret, HTTP origin, AI adapter, log event, store,
  checkpoint, replay operation, queue, bucket, database, statement, cache
  namespace, or search index
- printing, comparing, encoding, or structurally storing an opaque `Secret`
  or `DatabaseTransaction`
- an entrypoint that is nested, repeated, or has the wrong signature
- `?` outside a named function that returns its operand's family, or with a
  different error type (section 8.2)
- a let-else that is not a direct statement of a named function's body, whose
  patterns are not the two variants of one family, or whose value ends in `}`
  (section 8.3)
- `match` arms that mix families at a position, overlap, include a top-level
  catch-all, or leave an outcome unnamed (section 11)

Evaluation reports these (standard-library calls in a checked program
never fail, section 12.1):

- integer literal outside the signed 64-bit range (`K4005`; a literal too
  long for the parser's 128-bit reading is `K4005` already when parsing)
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

For the same source, compiler version, edition, command-line arguments, and
host inputs (host configuration, the delivered request, job, or trigger, and
every host response, such as an HTTP response or model output), a conforming
implementation produces the same value, output bytes, diagnostics, and build
artifacts:

- Evaluation order is fully specified. Callees, arguments, operands, list
  elements, entries, and record and construction fields evaluate from left to
  right in source order; only the selected branch of an `if` or `match`, and
  the needed operands of `&&` and `||`, evaluate; and a `?` or let-else exit
  stops evaluation at a defined point (sections 8.2 and 8.3).
- A program has no ambient inputs: no clock, randomness, environment
  variables, files, or standard input. Time reaches a program only as
  host-supplied data, such as the instants of a `ScheduleEvent`.
- Model output and other host responses are outside data. They may differ
  between invocations, and the program must validate them; durable replay
  lets a retry see the result an earlier attempt recorded
  (`DURABLE-STATE.md`).
- Diagnostics have deterministic codes, severities, primary spans, messages,
  and order (`DIAGNOSTICS.md`), and JSON output sorts object keys where this
  specification says so (section 13).
- `krit build` writes byte-identical components and metadata for the same
  package and compiler (`WASM-SANDBOX.md`).

Hash iteration, memory addresses, host paths, timestamps, and stack traces
must not affect user-visible output.

## 16. Canonical source format

`krit fmt` defines the canonical textual layout for implemented edition-2026
syntax. Formatting changes trivia, equivalent literal spellings, and
grammar-permitted trailing commas, not expression order or program semantics.

- Indentation is four ASCII spaces. Formatter-produced structural whitespace
  contains no tabs or trailing spaces; the content of a text block, which the
  formatter keeps, may.
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
  required control characters. The text of an interpolated string is written
  the same way, and each hole exactly as written (`\(request.path)`).
- A text block keeps its delimiters and its content lines exactly as written
  after their indentation, holes, escapes, quotes, tabs, and trailing
  whitespace included; only the indentation changes. The content lines and
  the closing `"""` are indented one level deeper than the line the block
  opens on, or at the opening `"""`'s own indentation when it starts its
  line, and an empty content line stays empty. Within its line a text block
  is as wide as its opening `"""`, so an argument list or construction that
  holds one can stay on one line around it:

  ```krit
  let reply = ai_invoke("reviewer", """
      Classify: \(request.path)
      """);
  ```
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
