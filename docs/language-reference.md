# Krit language reference

This document describes Krit 0.1.0.

## Source files

Krit source files conventionally use the `.krit` extension. A file is a
sequence of top-level definitions and expressions. Whitespace separates
tokens, parentheses and square brackets group forms, and `;` starts a comment
that continues to the end of the line.

Krit is case-sensitive. `name`, `Name`, and `NAME` are different identifiers.

## Values

| Kind | Examples | Notes |
|---|---|---|
| Integer | `0`, `42`, `-7` | Exact integers |
| Boolean | `true`, `false` | Racket-style `#t` and `#f` are also accepted |
| String | `"hello"` | Immutable text |
| Function | `(fn (x) (+ x 1))` | Lexical closure |
| List | `(list 1 2 3)` | Immutable, may contain mixed value kinds |

## Definitions and identifiers

A top-level definition evaluates its value and binds it in the file or REPL
environment:

```krit
(define answer 42)
```

Redefining a top-level name replaces its value for later forms. `define` is
not valid inside an expression; use `let` for local bindings.

The following identifiers are reserved:

```text
define if let fn list cons first rest empty? match empty true false
+ - * / modulo = < <= > >= ++ and or not print println
```

## Arithmetic and comparison

All arithmetic operators accept exactly two integers:

| Form | Result |
|---|---|
| `(+ left right)` | Addition |
| `(- left right)` | Subtraction |
| `(* left right)` | Multiplication |
| `(/ left right)` | Integer quotient, truncated toward zero |
| `(modulo left right)` | Integer modulo |

Division or modulo by zero is an error.

Ordering operators accept exactly two integers and return a boolean:

```krit
(< left right)
(<= left right)
(> left right)
(>= left right)
```

`(= left right)` compares integers, booleans, strings, and lists structurally.
Functions cannot be compared.

## Booleans and conditionals

```krit
(if condition consequent alternative)
(and left right)
(or left right)
(not value)
```

Conditions must be booleans; Krit does not treat other values as true or
false. `and` and `or` short-circuit, so they evaluate their right operand only
when needed.

## Strings and output

`(++ left right)` concatenates two strings.

```krit
(print value)
(println value)
```

`print` writes a value without a trailing newline. `println` appends a newline.
Both return the value they printed. Strings are printed without quote marks;
other values use their REPL representation.

## Local bindings

```krit
(let ([name value]
      [other-name other-value])
  body)
```

The binding values are evaluated in the surrounding environment before any
new name is introduced. The bindings are therefore simultaneous:

```krit
(define x 1)
(let ([x 2]
      [y x])
  y)
; => 1
```

Duplicate names in one `let` are rejected.

## Functions and calls

Anonymous function:

```krit
(fn (parameter ...) body)
```

Named recursive function:

```krit
(fn function-name (parameter ...) body)
```

Call:

```krit
(function-expression argument ...)
```

Arguments are evaluated from left to right. The argument count must exactly
match the parameter count. Functions use lexical scope: a function retains
the environment in which it was created.

Named functions bind their own name while their body runs. Top-level
definitions also support recursion because closures retain the shared
top-level environment.

## Lists

```krit
(list element ...)
(cons head list)
(first list)
(rest list)
(empty? list)
```

`list` constructs an immutable list. The second argument to `cons` must be a
list. `first` and `rest` report an error for an empty list. `empty?` returns a
boolean.

## Pattern matching

Krit 0.1 supports exhaustive matching over lists:

```krit
(match list-expression
  [empty empty-result]
  [(cons head-name tail-name) non-empty-result])
```

The subject must be a list. An empty list evaluates `empty-result`. A non-empty
list binds its first item to `head-name`, its remaining list to `tail-name`,
and evaluates `non-empty-result`.

The clauses and their order are currently fixed. More general patterns are a
future language feature.

## Evaluation order

Krit evaluates eagerly and from left to right, except for:

- `if`, which evaluates only the selected branch
- `and` and `or`, which short-circuit
- `match`, which evaluates only the selected clause
- function bodies, which run only when called

## Errors

Syntax and runtime failures stop the current file or command-line evaluation.
Messages identify the source, line, and column:

```text
program.krit:4:11: undefined name: total
```

The REPL reports an error and continues with the same environment.
