#lang racket/base

(require rackunit
         "../main.rkt")

(define (krit-eval source)
  (evaluate-string source "evaluator-test.krit"))

(module+ test
  (test-case "evaluates primitive values and arithmetic"
    (check-equal? (krit-eval "42") 42)
    (check-equal? (krit-eval "(+ 20 22)") 42)
    (check-equal? (krit-eval "(/ 9 2)") 4)
    (check-equal? (krit-eval "(modulo 9 4)") 1)
    (check-equal? (krit-eval "(++ \"K\" \"rit\")") "Krit"))

  (test-case "evaluates boolean expressions lazily"
    (check-equal? (krit-eval "(if (> 3 2) true false)") #t)
    (check-equal? (krit-eval "(and false missing)") #f)
    (check-equal? (krit-eval "(or true missing)") #t)
    (check-equal? (krit-eval "(not false)") #t))

  (test-case "uses lexical scope"
    (check-equal?
     (krit-eval
      "(let ([x 10]) (let ([add-x (fn (y) (+ x y))]) (add-x 5)))")
     15))

  (test-case "supports named recursive functions"
    (check-equal?
     (krit-eval
      #<<KRIT
((fn factorial (n)
   (if (= n 0)
       1
       (* n (factorial (- n 1)))))
 6)
KRIT
      )
     720))

  (test-case "supports recursive top-level definitions"
    (check-equal?
     (krit-eval
      #<<KRIT
(define countdown
  (fn (n)
    (if (= n 0)
        (list)
        (cons n (countdown (- n 1))))))
(countdown 3)
KRIT
      )
     '(3 2 1)))

  (test-case "supports immutable list operations"
    (check-equal? (krit-eval "(first (list 4 5 6))") 4)
    (check-equal? (krit-eval "(rest (list 4 5 6))") '(5 6))
    (check-equal? (krit-eval "(empty? (list))") #t)
    (check-equal? (krit-eval "(cons 1 (list 2 3))") '(1 2 3)))

  (test-case "matches empty and non-empty lists"
    (check-equal?
     (krit-eval
      "(match (list) [empty 10] [(cons head tail) head])")
     10)
    (check-equal?
     (krit-eval
      "(match (list 7 8) [empty 0] [(cons head tail) (+ head (first tail))])")
     15))

  (test-case "evaluates a recursive list program"
    (check-equal?
     (krit-eval
      #<<KRIT
(define sum
  (fn sum (items)
    (match items
      [empty 0]
      [(cons head tail) (+ head (sum tail))])))
(sum (list 10 20 12))
KRIT
      )
     42))

  (test-case "prints values"
    (define output (open-output-string))
    (define result
      (parameterize ([current-output-port output])
        (krit-eval "(println (++ \"Hello, \" \"Krit!\"))")))
    (check-equal? result "Hello, Krit!")
    (check-equal? (get-output-string output) "Hello, Krit!\n"))

  (test-case "reports runtime errors with source positions"
    (check-exn
     #rx"evaluator-test.krit:1:1: division by zero"
     (lambda () (krit-eval "(/ 4 0)")))
    (check-exn
     #rx"expected an integer, received string"
     (lambda () (krit-eval "(+ 1 \"two\")")))
    (check-exn
     #rx"undefined name: missing"
     (lambda () (krit-eval "missing")))
    (check-exn
     #rx"function expects 1 argument, received 2"
     (lambda () (krit-eval "((fn (x) x) 1 2)")))
    (check-exn
     #rx"first cannot be applied to an empty list"
     (lambda () (krit-eval "(first (list))")))
    (check-exn
     #rx"functions cannot be compared"
     (lambda ()
       (krit-eval "(= (list (fn (x) x)) (list (fn (x) x)))")))))
