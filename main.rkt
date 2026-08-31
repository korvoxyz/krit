#lang racket/base

(require racket/port
         "ast.rkt"
         "errors.rkt"
         "evaluator.rkt"
         "parser.rkt")

(provide
 (all-from-out "ast.rkt")
 (all-from-out "errors.rkt")
 (all-from-out "evaluator.rkt")
 (all-from-out "parser.rkt")
 evaluate-port
 evaluate-string)

(define (evaluate-port
         input
         [source "<input>"]
         [environment (make-global-environment)])
  (evaluate-program (read-program input source) environment))

(define (evaluate-string
         source-text
         [source "<string>"]
         [environment (make-global-environment)])
  (call-with-input-string
   source-text
   (lambda (input)
     (evaluate-port input source environment))))
