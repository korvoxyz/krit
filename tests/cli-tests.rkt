#lang racket/base

(require rackunit
         "../cli.rkt")

(define (run-cli . arguments)
  (define output (open-output-string))
  (define error-output (open-output-string))
  (define status
    (parameterize ([current-output-port output]
                   [current-error-port error-output])
      (main (list->vector arguments))))
  (values status
          (get-output-string output)
          (get-output-string error-output)))

(module+ test
  (test-case "evaluates command-line source"
    (define-values (status output error-output)
      (run-cli "--eval" "(+ 40 2)"))
    (check-equal? status 0)
    (check-equal? output "42\n")
    (check-equal? error-output ""))

  (test-case "reports command-line evaluation errors"
    (define-values (status output error-output)
      (run-cli "--eval" "(/ 1 0)"))
    (check-equal? status 1)
    (check-equal? output "")
    (check-regexp-match
     #rx"<command-line>:1:1: division by zero"
     error-output))

  (test-case "prints version information"
    (define-values (status output error-output)
      (run-cli "--version"))
    (check-equal? status 0)
    (check-regexp-match #rx"^Krit 0[.]1[.]0 \\(Racket 9[.]3\\)" output)
    (check-equal? error-output ""))

  (test-case "rejects conflicting inputs"
    (define-values (status output error-output)
      (run-cli "--eval" "1" "program.krit"))
    (check-equal? status 2)
    (check-equal? output "")
    (check-regexp-match #rx"cannot be combined" error-output)))
