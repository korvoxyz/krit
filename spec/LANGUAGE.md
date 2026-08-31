# Krit 0.2 language specification

**Status:** Normative  
**Edition:** 2026

## 1. Scope

This document defines the syntax and runtime behavior required by the initial
Rust implementation. Static type/effect checking, modules, packages, and
external capabilities are specified separately and remain draft until their
documents become normative.

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
else false fn if let match true
```

Reserved built-in names:

```text
print println
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
                | expression, ";" ;

let-declaration = "let", identifier, "=", expression, ";" ;

function-declaration
                = "fn", identifier, "(", parameters?, ")", block ;

parameters      = identifier, (",", identifier)*, ","? ;

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
call            = primary, ("(", arguments?, ")")* ;
arguments       = expression, (",", expression)*, ","? ;

primary         = integer
                | string
                | "true"
                | "false"
                | identifier
                | list
                | block
                | if-expression
                | match-expression
                | function-expression
                | "(", expression, ")" ;

list            = "[", arguments?, "]" ;

if-expression   = "if", expression, block, "else",
                  (block | if-expression) ;

function-expression
                = "fn", "(", parameters?, ")", block ;

match-expression
                = "match", expression, "{",
                  "[", "]", "=>", expression, ",",
                  "[", identifier, ",", "..", identifier, "]",
                  "=>", expression, ","?,
                  "}" ;
```

Assignments and mutable bindings do not exist in edition 2026.

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
- function closure

Integer overflow is a runtime error. Division truncates toward zero. Division
or remainder by zero is a runtime error.

Lists may contain values of different kinds in the dynamic baseline. The
static type system will require a common element type.

## 7. Bindings and scope

`let` evaluates its expression before introducing the new immutable binding.
The binding extends from the following item in the current scope to the end of
that scope.

An inner binding may shadow an outer binding. A binding cannot be read before
its declaration.

Function declarations are visible from their declaration to the end of their
containing scope and may call themselves recursively. Mutually recursive
declarations are not supported in the dynamic baseline.

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

Function values cannot be compared.

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

Equality is structural for integers, booleans, strings, unit, and lists.
Comparing a function directly or inside a list is a runtime error.

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

## 11. Lists and matching

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
- functions: `<function>` or `<function name>`

Output is the only effect in the normative 0.2 runtime. Package execution will
model it as the `io.stdout` capability when the capability specification
becomes normative.

## 13. Evaluation failure

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

Evaluation stops at the first error. Errors follow `DIAGNOSTICS.md`.

## 14. Determinism

For the same source, compiler version, edition, arguments, standard input, and
explicit capability responses, a conforming implementation produces the same
value, output bytes, and diagnostic code.

Hash iteration, memory addresses, host paths, timestamps, and stack traces
must not affect user-visible output.
