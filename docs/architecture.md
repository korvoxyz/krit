# Interpreter architecture

Krit is a tree-walking interpreter written in Racket 9.3. Its implementation
is split into small stages so each language concept can be studied
independently.

```text
Krit source
    |
    v
Racket data reader
    |
    v
Parser and source-located AST
    |
    v
Tree-walking evaluator
    |
    v
Krit value or diagnostic
```

## Modules

| File | Responsibility |
|---|---|
| `ast.rkt` | Transparent syntax structures, source locations, and free-variable analysis |
| `parser.rkt` | Safe S-expression reading, syntax validation, and AST construction |
| `evaluator.rkt` | Environments, closures, values, operations, and program evaluation |
| `errors.rkt` | Krit exception type and source-position formatting |
| `main.rkt` | Public library API combining parser and evaluator |
| `cli.rkt` | File runner, `--eval`, version output, and persistent REPL |
| `launcher.rkt` | Installed `krit` executable entry point |
| `tests/` | Parser, evaluator, free-variable, and CLI behavior tests |

## Parsing

Racket's `read-syntax` performs tokenization and balanced-delimiter handling.
The reader is configured to reject `#lang` and custom `#reader` extensions.
Krit never passes source to Racket's evaluator.

Each parsed expression stores a `source-location`. Parser and runtime errors
therefore use the location of the failing Krit construct rather than an
interpreter implementation stack trace.

## Environments and closures

An environment is a mutable table with an optional parent. Mutation is an
interpreter implementation detail used to support top-level definitions and
REPL redefinition; Krit programs have no mutation operation.

A closure stores:

- its optional recursive name
- its ordered parameters
- its body AST
- the lexical environment where it was created

A call creates a child environment, binds the recursive name and arguments,
then evaluates the body. This gives Krit lexical rather than dynamic scope.

## Free-variable analysis

`free-variables` in `ast.rkt` computes the unbound identifier set of any AST.
It correctly removes names introduced by `let`, function parameters, named
recursion, list patterns, and definitions.

The tree-walking evaluator currently retains a closure's environment object.
A future optimized evaluator can use the free-variable set to capture only
the needed bindings without changing the parser or language semantics.

## Lists and matching

Krit lists use Racket's immutable list representation internally. Runtime
checks prevent improper list tails from entering the language through `cons`.
List matching creates a child lexical environment for the pattern's head and
tail bindings.

## Public API

Requiring the installed `krit` collection exposes the AST, parser, evaluator,
and these convenient entry points:

```racket
(evaluate-string "(+ 20 22)")
(evaluate-port input-port "program.krit")
(parse-program-string "(fn (x) (+ x external))")
(free-variables expression)
```

Callers can pass a shared environment to `evaluate-string` or `evaluate-port`
when they need definitions to persist across evaluations.

## Intended evolution

The interpreter favors explicit code over metaprogramming so it remains useful
for teaching. New stages such as static analysis or bytecode execution should
be separate modules with behavior checked against the tree-walking evaluator.
