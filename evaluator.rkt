#lang racket/base

(require racket/list
         racket/match
         racket/string
         "ast.rkt"
         "errors.rkt")

(provide
 (struct-out environment)
 (struct-out closure)
 make-global-environment
 evaluate-expression
 evaluate-form
 evaluate-program
 value->string)

(struct environment (values parent) #:transparent)
(struct closure (name parameters body environment) #:transparent)

(define (make-global-environment)
  (environment (make-hasheq) #f))

(define (make-child-environment parent)
  (environment (make-hasheq) parent))

(define (bind! environment name value)
  (hash-set! (environment-values environment) name value))

(define (lookup environment name location)
  (cond
    [(hash-has-key? (environment-values environment) name)
     (hash-ref (environment-values environment) name)]
    [(environment-parent environment)
     (lookup (environment-parent environment) name location)]
    [else
     (raise-krit-error location "undefined name: ~a" name)]))

(define (value-type value)
  (cond
    [(exact-integer? value) "integer"]
    [(boolean? value) "boolean"]
    [(string? value) "string"]
    [(list? value) "list"]
    [(closure? value) "function"]
    [else "value"]))

(define (require-type predicate expected value location)
  (unless (predicate value)
    (raise-krit-error
     location
     "expected ~a, received ~a"
     expected
     (value-type value)))
  value)

(define (require-integer value location)
  (require-type exact-integer? "an integer" value location))

(define (require-boolean value location)
  (require-type boolean? "a boolean" value location))

(define (require-list value location)
  (require-type list? "a list" value location))

(define (require-string value location)
  (require-type string? "a string" value location))

(define (value->string value)
  (cond
    [(eq? value #t) "true"]
    [(eq? value #f) "false"]
    [(string? value) (format "~s" value)]
    [(list? value)
     (if (null? value)
         "(list)"
         (format
          "(list ~a)"
          (string-join (map value->string value) " ")))]
    [(closure? value)
     (if (closure-name value)
         (format "<function ~a>" (closure-name value))
         "<function>")]
    [(void? value) "<void>"]
    [else (format "~a" value)]))

(define (print-value value)
  (display
   (if (string? value)
       value
       (value->string value))))

(define (evaluate-arguments operands environment)
  (for/list ([operand (in-list operands)])
    (evaluate-expression operand environment)))

(define (contains-function? value)
  (or (closure? value)
      (and (list? value)
           (ormap contains-function? value))))

(define (evaluate-equality left right location)
  (when (or (contains-function? left)
            (contains-function? right))
    (raise-krit-error location "functions cannot be compared"))
  (equal? left right))

(define (evaluate-operation expression environment)
  (define name (operation-name expression))
  (define operands (operation-operands expression))
  (define location (operation-location expression))
  (case name
    [(and)
     (define left
       (require-boolean
        (evaluate-expression (first operands) environment)
        location))
     (if left
         (require-boolean
          (evaluate-expression (second operands) environment)
          location)
         #f)]
    [(or)
     (define left
       (require-boolean
        (evaluate-expression (first operands) environment)
        location))
     (if left
         #t
         (require-boolean
          (evaluate-expression (second operands) environment)
          location))]
    [(not)
     (not
      (require-boolean
       (evaluate-expression (first operands) environment)
       location))]
    [(print println)
     (define value
       (evaluate-expression (first operands) environment))
     (print-value value)
     (when (eq? name 'println)
       (newline))
     value]
    [else
     (define values (evaluate-arguments operands environment))
     (define left (first values))
     (define right (second values))
     (case name
       [(+)
        (+ (require-integer left location)
           (require-integer right location))]
       [(-)
        (- (require-integer left location)
           (require-integer right location))]
       [(*)
        (* (require-integer left location)
           (require-integer right location))]
       [(/)
        (define divisor (require-integer right location))
        (when (zero? divisor)
          (raise-krit-error location "division by zero"))
        (quotient (require-integer left location) divisor)]
       [(modulo)
        (define divisor (require-integer right location))
        (when (zero? divisor)
          (raise-krit-error location "modulo by zero"))
        (modulo (require-integer left location) divisor)]
       [(=) (evaluate-equality left right location)]
       [(<)
        (< (require-integer left location)
           (require-integer right location))]
       [(<=)
        (<= (require-integer left location)
            (require-integer right location))]
       [(>)
        (> (require-integer left location)
           (require-integer right location))]
       [(>=)
        (>= (require-integer left location)
            (require-integer right location))]
       [(++)
        (string-append
         (require-string left location)
         (require-string right location))])]))

(define (evaluate-application expression environment)
  (define function
    (evaluate-expression (application-callee expression) environment))
  (unless (closure? function)
    (raise-krit-error
     (application-location expression)
     "expected a function, received ~a"
     (value-type function)))
  (define arguments
    (evaluate-arguments (application-arguments expression) environment))
  (define parameters (closure-parameters function))
  (unless (= (length arguments) (length parameters))
    (raise-krit-error
     (application-location expression)
     "function expects ~a argument~a, received ~a"
     (length parameters)
     (if (= (length parameters) 1) "" "s")
     (length arguments)))
  (define call-environment
    (make-child-environment (closure-environment function)))
  (when (closure-name function)
    (bind! call-environment (closure-name function) function))
  (for ([parameter (in-list parameters)]
        [argument (in-list arguments)])
    (bind! call-environment parameter argument))
  (evaluate-expression (closure-body function) call-environment))

(define (evaluate-expression expression environment)
  (match expression
    [(literal value _) value]
    [(variable name location)
     (lookup environment name location)]
    [(operation _ _ _)
     (evaluate-operation expression environment)]
    [(conditional test consequent alternative location)
     (if (require-boolean
          (evaluate-expression test environment)
          location)
         (evaluate-expression consequent environment)
         (evaluate-expression alternative environment))]
    [(let-expression bindings body _)
     (define values
       (for/list ([item (in-list bindings)])
         (evaluate-expression (binding-value item) environment)))
     (define local-environment (make-child-environment environment))
     (for ([item (in-list bindings)]
           [value (in-list values)])
       (bind! local-environment (binding-name item) value))
     (evaluate-expression body local-environment)]
    [(function-expression name parameters body _)
     (closure name parameters body environment)]
    [(application _ _ _)
     (evaluate-application expression environment)]
    [(list-expression elements _)
     (evaluate-arguments elements environment)]
    [(cons-expression head tail location)
     (cons
      (evaluate-expression head environment)
      (require-list
       (evaluate-expression tail environment)
       location))]
    [(first-expression list location)
     (define value
       (require-list
        (evaluate-expression list environment)
        location))
     (when (null? value)
       (raise-krit-error location "first cannot be applied to an empty list"))
     (first value)]
    [(rest-expression list location)
     (define value
       (require-list
        (evaluate-expression list environment)
        location))
     (when (null? value)
       (raise-krit-error location "rest cannot be applied to an empty list"))
     (rest value)]
    [(empty-predicate list location)
     (null?
      (require-list
       (evaluate-expression list environment)
       location))]
    [(list-match subject empty-case head-name tail-name cons-case location)
     (define value
       (require-list
        (evaluate-expression subject environment)
        location))
     (if (null? value)
         (evaluate-expression empty-case environment)
         (let ([match-environment (make-child-environment environment)])
           (bind! match-environment head-name (first value))
           (bind! match-environment tail-name (rest value))
           (evaluate-expression cons-case match-environment)))]))

(define (evaluate-form form environment)
  (match form
    [(definition name value _)
     (define result (evaluate-expression value environment))
     (bind! environment name result)
     (void)]
    [_ (evaluate-expression form environment)]))

(define (evaluate-program forms [environment (make-global-environment)])
  (for/fold ([result (void)])
            ([form (in-list forms)])
    (evaluate-form form environment)))
