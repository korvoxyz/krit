# Krit

Krit is a small functional programming language for learning how interpreters
work. It has lexical closures, recursive functions, immutable lists, pattern
matching, and source-positioned errors, while keeping the implementation small
enough to read.

```krit
(define sum
  (fn sum (items)
    (match items
      [empty 0]
      [(cons head tail) (+ head (sum tail))])))

(println (sum (list 10 20 12)))
```

```text
42
```

Krit uses S-expression syntax and is implemented in Racket. Version 0.1.0 is
an educational interpreter, not a production application runtime.

## Requirements

- [Racket 9.3](https://download.racket-lang.org/) or newer

On macOS, Racket 9.3 is also available through Homebrew:

```sh
brew install --cask racket
```

Confirm the installed version:

```sh
racket --version
# Welcome to Racket v9.3 [cs].
```

## Install

From a clone of this repository:

```sh
raco pkg install --auto --name krit
raco krit --version
```

The package also generates a `krit` executable in Racket's user executable
directory. Add that directory to your shell path once:

```sh
RACKET_BIN="$(racket -e '(require setup/dirs) (display (find-user-console-bin-dir))')"
export PATH="$RACKET_BIN:$PATH"
krit --version
```

Add the `export` line, with the resolved directory, to your shell profile to
make it permanent. `raco krit` works without this path setup.

You can run the interpreter without installing the package:

```sh
racket cli.rkt examples/factorial.krit
```

To update a linked development installation after changing package metadata:

```sh
raco setup krit
```

## Use Krit

Run a source file using either installed command:

```sh
krit examples/factorial.krit
raco krit examples/factorial.krit
```

Evaluate an expression:

```sh
krit --eval '(+ 20 22)'
```

Start the REPL:

```sh
krit
```

```text
Krit 0.1.0 -- press Ctrl-D to exit
krit> (define double (fn (x) (* x 2)))
double defined
krit> (double 21)
42
```

Show all command-line options:

```sh
krit --help
```

## Language tour

Krit programs contain expressions and top-level definitions. A semicolon
starts a line comment.

### Values and operations

```krit
42
true
false
"hello"

(+ 20 22)
(>= 5 3)
(++ "Hello, " "Krit!")
(if true "yes" "no")
```

Krit has integers, booleans, strings, functions, and immutable lists. Integer
division truncates toward zero.

### Bindings and functions

`let` creates simultaneous lexical bindings:

```krit
(let ([x 20]
      [y 22])
  (+ x y))
```

Functions can accept any number of parameters and close over their lexical
environment:

```krit
(let ([x 10])
  ((fn (y) (+ x y)) 5))
```

Give a function a name when it needs to call itself:

```krit
(fn factorial (n)
  (if (= n 0)
      1
      (* n (factorial (- n 1)))))
```

### Immutable lists and matching

```krit
(list 1 2 3)
(cons 0 (list 1 2 3))
(first (list 1 2 3))
(rest (list 1 2 3))
(empty? (list))
```

`match` handles both possible list shapes:

```krit
(match items
  [empty 0]
  [(cons head tail) (+ head (sum tail))])
```

The bindings in the `cons` pattern are available only in that branch.

See [docs/language-reference.md](docs/language-reference.md) for the complete
syntax and semantics.

## Errors

Parser and runtime errors include the source file, line, and column:

```text
example.krit:3:7: expected an integer, received string
```

The parser reads data and builds Krit syntax directly; it never evaluates
source as Racket code.

## Develop

Compile the interpreter and run the test suite:

```sh
raco make ast.rkt errors.rkt parser.rkt evaluator.rkt main.rkt cli.rkt
raco test tests/parser-tests.rkt tests/evaluator-tests.rkt tests/cli-tests.rkt
```

Run the examples:

```sh
racket cli.rkt examples/factorial.krit
racket cli.rkt examples/lists.krit
```

The project is continuously tested with Racket 9.3. See
[docs/architecture.md](docs/architecture.md) for the interpreter design and
[CONTRIBUTING.md](CONTRIBUTING.md) before proposing changes.

## Project direction

Krit is intentionally focused on teaching and experimentation:

- **0.1:** parser, evaluator, lexical closures, recursion, immutable lists,
  matching, REPL, CLI, errors, tests, and examples
- **Next:** richer diagnostics, more match patterns, a small standard library,
  and optional static type checking
- **Later:** modules and a bytecode evaluator, if they improve the educational
  value without obscuring the implementation

Compatibility will be documented from version 1.0 onward. Before then, syntax
may evolve between minor releases.

## Origin

Krit began in 2013 as a small interpreter exercise based on the MUPL
educational language. The current implementation is a fresh language surface
and interpreter foundation, while the Git history preserves that origin.

## License

**Ownership:** Krit and the Krit language are owned by Akshay Bhardwaj.

Krit's implementation and documentation are licensed under the
[Apache License 2.0](LICENSE). The license is permissive and includes an
explicit patent grant. It does not place licensing requirements on programs
written in Krit.
