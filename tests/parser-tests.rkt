#lang racket/base

(require rackunit
         racket/list
         racket/set
         "../main.rkt")

(module+ test
  (test-case "parses literals and operations"
    (define forms
      (parse-program-string
       "42\ntrue\n\"hello\"\n(+ 1 2)"
       "parser-test.krit"))
    (check-equal? (length forms) 4)
    (check-equal? (literal-value (first forms)) 42)
    (check-equal? (literal-value (second forms)) #t)
    (check-equal? (literal-value (third forms)) "hello")
    (check-equal? (operation-name (fourth forms)) '+)
    (check-equal?
     (source-location-source
      (operation-location (fourth forms)))
     "parser-test.krit"))

  (test-case "parses definitions, functions, and matching"
    (define forms
      (parse-program-string
       #<<KRIT
(define sum
  (fn sum (items)
    (match items
      [empty 0]
      [(cons head tail) (+ head (sum tail))])))
KRIT
       "sum.krit"))
    (check-equal? (length forms) 1)
    (define form (first forms))
    (check-true (definition? form))
    (check-equal? (definition-name form) 'sum)
    (check-true (function-expression? (definition-value form)))
    (check-true
     (list-match?
      (function-expression-body (definition-value form)))))

  (test-case "computes free variables"
    (define expression
      (first
       (parse-program-string
        "(fn (x) (let ([y external]) (+ x y)))")))
    (check-equal? (free-variables expression) (seteq 'external)))

  (test-case "rejects duplicate parameters"
    (check-exn
     #rx"duplicate parameter: x"
     (lambda ()
       (parse-program-string "(fn (x x) x)" "duplicate.krit"))))

  (test-case "reports source positions"
    (check-exn
     #rx"broken.krit:2:1: if expects 4 forms, received 3"
     (lambda ()
       (parse-program-string "1\n(if true 2)" "broken.krit")))))
