#lang racket/base

(require "ast.rkt")

(provide
 (struct-out exn:fail:krit)
 format-source-location
 raise-krit-error)

(struct exn:fail:krit exn:fail (location) #:transparent)

(define (source-name source)
  (cond
    [(path? source) (path->string source)]
    [source (format "~a" source)]
    [else "<unknown>"]))

(define (format-source-location location)
  (define source (source-name (source-location-source location)))
  (define line (or (source-location-line location) "?"))
  (define column
    (if (source-location-column location)
        (add1 (source-location-column location))
        "?"))
  (format "~a:~a:~a" source line column))

(define (raise-krit-error location message . arguments)
  (define detail (apply format message arguments))
  (raise
   (exn:fail:krit
    (format "~a: ~a" (format-source-location location) detail)
    (current-continuation-marks)
    location)))
