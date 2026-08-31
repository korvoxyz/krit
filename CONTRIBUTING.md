# Contributing to Krit

Krit is an educational language. Changes should keep the implementation
readable, the semantics coherent, and errors useful to someone learning how
an interpreter works.

## Development setup

Install Racket 9.3 or newer, then link the package:

```sh
raco pkg install --auto --name krit
```

Compile and test before submitting a change:

```sh
raco make ast.rkt errors.rkt parser.rkt evaluator.rkt main.rkt cli.rkt
raco test tests/parser-tests.rkt tests/evaluator-tests.rkt tests/cli-tests.rkt
raco krit examples/factorial.krit
raco krit examples/lists.krit
```

## Changing the language

A syntax or semantics change should include:

1. Parser and evaluator tests for success and failure cases.
2. Updates to `docs/language-reference.md`.
3. An example when the feature is easier to understand through a complete
   program.
4. A `CHANGELOG.md` entry when the change affects users.

Keep error handling precise. Syntax errors should be rejected by the parser,
and runtime type errors should report the source location of the failing
construct.

## License

By contributing, you agree that your contributions are licensed under the
Apache License 2.0.
