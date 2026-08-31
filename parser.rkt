#lang racket/base

(require racket/list
         racket/port
         racket/set
         "ast.rkt"
         "errors.rkt")

(provide
 read-program
 parse-program-string
 parse-top-level-syntax
 parse-expression)

(define operator-arities
  (hasheq
   '+ 2
   '- 2
   '* 2
   '/ 2
   'modulo 2
   '= 2
   '< 2
   '<= 2
   '> 2
   '>= 2
   '++ 2
   'and 2
   'or 2
   'not 1
   'print 1
   'println 1))

(define reserved-identifiers
  (for/fold ([names
              (seteq
               'define 'if 'let 'fn 'list 'cons 'first 'rest 'empty?
               'match 'empty 'true 'false)])
            ([operator (in-hash-keys operator-arities)])
    (set-add names operator)))

(define (syntax-location syntax)
  (source-location
   (syntax-source syntax)
   (syntax-line syntax)
   (syntax-column syntax)
   (syntax-position syntax)
   (syntax-span syntax)))

(define (syntax-elements syntax description)
  (define elements (syntax->list syntax))
  (unless elements
    (raise-krit-error
     (syntax-location syntax)
     "expected ~a, received an improper list"
     description))
  elements)

(define (expect-length syntax elements expected description)
  (unless (= (length elements) expected)
    (raise-krit-error
     (syntax-location syntax)
     "~a expects ~a form~a, received ~a"
     description
     expected
     (if (= expected 1) "" "s")
     (length elements))))

(define (parse-identifier syntax role)
  (define name (syntax-e syntax))
  (unless (symbol? name)
    (raise-krit-error
     (syntax-location syntax)
     "expected an identifier for ~a"
     role))
  (when (set-member? reserved-identifiers name)
    (raise-krit-error
     (syntax-location syntax)
     "~a is reserved and cannot be used as ~a"
     name
     role))
  name)

(define (reject-duplicates names syntax role)
  (define duplicate (check-duplicates names eq?))
  (when duplicate
    (raise-krit-error
     (syntax-location syntax)
     "duplicate ~a: ~a"
     role
     duplicate)))

(define (parse-parameters syntax)
  (define parameter-syntaxes
    (syntax-elements syntax "a parameter list"))
  (define parameters
    (for/list ([parameter (in-list parameter-syntaxes)])
      (parse-identifier parameter "a parameter")))
  (reject-duplicates parameters syntax "parameter")
  parameters)

(define (parse-binding syntax)
  (define parts (syntax-elements syntax "a let binding"))
  (expect-length syntax parts 2 "a let binding")
  (binding
   (parse-identifier (first parts) "a let binding")
   (parse-expression (second parts))))

(define (parse-let syntax elements)
  (expect-length syntax elements 3 "let")
  (define binding-syntaxes
    (syntax-elements (second elements) "a list of let bindings"))
  (define bindings (map parse-binding binding-syntaxes))
  (reject-duplicates
   (map binding-name bindings)
   (second elements)
   "let binding")
  (let-expression
   bindings
   (parse-expression (third elements))
   (syntax-location syntax)))

(define (parse-function syntax elements)
  (unless (or (= (length elements) 3)
              (= (length elements) 4))
    (raise-krit-error
     (syntax-location syntax)
     "fn expects (fn (parameters ...) body) or (fn name (parameters ...) body)"))
  (define named? (= (length elements) 4))
  (define name
    (and named?
         (parse-identifier (second elements) "a function name")))
  (define parameters
    (parse-parameters
     (if named?
         (third elements)
         (second elements))))
  (define body
    (parse-expression
     (if named?
         (fourth elements)
         (third elements))))
  (function-expression name parameters body (syntax-location syntax)))

(define (parse-match syntax elements)
  (expect-length syntax elements 4 "match")
  (define empty-clause
    (syntax-elements (third elements) "an empty match clause"))
  (define cons-clause
    (syntax-elements (fourth elements) "a cons match clause"))
  (expect-length (third elements) empty-clause 2 "an empty match clause")
  (expect-length (fourth elements) cons-clause 2 "a cons match clause")
  (unless (eq? (syntax-e (first empty-clause)) 'empty)
    (raise-krit-error
     (syntax-location (first empty-clause))
     "the first match pattern must be empty"))
  (define cons-pattern
    (syntax-elements (first cons-clause) "a (cons head tail) pattern"))
  (expect-length (first cons-clause) cons-pattern 3 "a cons pattern")
  (unless (eq? (syntax-e (first cons-pattern)) 'cons)
    (raise-krit-error
     (syntax-location (first cons-pattern))
     "the second match pattern must be (cons head tail)"))
  (define head-name
    (parse-identifier (second cons-pattern) "a match binding"))
  (define tail-name
    (parse-identifier (third cons-pattern) "a match binding"))
  (when (eq? head-name tail-name)
    (raise-krit-error
     (syntax-location (third cons-pattern))
     "match bindings must have different names"))
  (list-match
   (parse-expression (second elements))
   (parse-expression (second empty-clause))
   head-name
   tail-name
   (parse-expression (second cons-clause))
   (syntax-location syntax)))

(define (parse-operation syntax elements operator)
  (define expected (hash-ref operator-arities operator))
  (define operands (rest elements))
  (unless (= (length operands) expected)
    (raise-krit-error
     (syntax-location syntax)
     "~a expects ~a argument~a, received ~a"
     operator
     expected
     (if (= expected 1) "" "s")
     (length operands)))
  (operation
   operator
   (map parse-expression operands)
   (syntax-location syntax)))

(define (parse-list-form syntax elements)
  (when (null? elements)
    (raise-krit-error
     (syntax-location syntax)
     "an empty form is not an expression; use (list) for an empty list"))
  (define head (syntax-e (first elements)))
  (cond
    [(eq? head 'if)
     (expect-length syntax elements 4 "if")
     (conditional
      (parse-expression (second elements))
      (parse-expression (third elements))
      (parse-expression (fourth elements))
      (syntax-location syntax))]
    [(eq? head 'let) (parse-let syntax elements)]
    [(eq? head 'fn) (parse-function syntax elements)]
    [(eq? head 'list)
     (list-expression
      (map parse-expression (rest elements))
      (syntax-location syntax))]
    [(eq? head 'cons)
     (expect-length syntax elements 3 "cons")
     (cons-expression
      (parse-expression (second elements))
      (parse-expression (third elements))
      (syntax-location syntax))]
    [(eq? head 'first)
     (expect-length syntax elements 2 "first")
     (first-expression
      (parse-expression (second elements))
      (syntax-location syntax))]
    [(eq? head 'rest)
     (expect-length syntax elements 2 "rest")
     (rest-expression
      (parse-expression (second elements))
      (syntax-location syntax))]
    [(eq? head 'empty?)
     (expect-length syntax elements 2 "empty?")
     (empty-predicate
      (parse-expression (second elements))
      (syntax-location syntax))]
    [(eq? head 'match) (parse-match syntax elements)]
    [(eq? head 'define)
     (raise-krit-error
      (syntax-location syntax)
      "define is only allowed at the top level")]
    [(hash-has-key? operator-arities head)
     (parse-operation syntax elements head)]
    [else
     (application
      (parse-expression (first elements))
      (map parse-expression (rest elements))
      (syntax-location syntax))]))

(define (parse-expression syntax)
  (define value (syntax-e syntax))
  (cond
    [(exact-integer? value)
     (literal value (syntax-location syntax))]
    [(boolean? value)
     (literal value (syntax-location syntax))]
    [(string? value)
     (literal value (syntax-location syntax))]
    [(symbol? value)
     (case value
       [(true) (literal #t (syntax-location syntax))]
       [(false) (literal #f (syntax-location syntax))]
       [else (variable value (syntax-location syntax))])]
    [(pair? value)
     (parse-list-form
      syntax
      (syntax-elements syntax "an expression"))]
    [(null? value)
     (raise-krit-error
      (syntax-location syntax)
      "an empty form is not an expression; use (list) for an empty list")]
    [else
     (raise-krit-error
      (syntax-location syntax)
      "unsupported literal: ~v"
      value)]))

(define (parse-definition syntax elements)
  (expect-length syntax elements 3 "define")
  (definition
   (parse-identifier (second elements) "a definition")
   (parse-expression (third elements))
   (syntax-location syntax)))

(define (parse-top-level-syntax syntax)
  (define value (syntax-e syntax))
  (if (pair? value)
      (let ([elements (syntax-elements syntax "a top-level form")])
        (if (and (pair? elements)
                 (eq? (syntax-e (first elements)) 'define))
            (parse-definition syntax elements)
            (parse-expression syntax)))
      (parse-expression syntax)))

(define (read-program input [source "<input>"])
  (port-count-lines! input)
  (parameterize ([read-accept-lang #f]
                 [read-accept-reader #f])
    (let loop ([forms null])
      (define syntax (read-syntax source input))
      (if (eof-object? syntax)
          (reverse forms)
          (loop (cons (parse-top-level-syntax syntax) forms))))))

(define (parse-program-string source-text [source "<string>"])
  (call-with-input-string
   source-text
   (lambda (input)
     (read-program input source))))
