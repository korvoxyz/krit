#lang racket/base

(require racket/match
         racket/set)

(provide
 (struct-out source-location)
 (struct-out literal)
 (struct-out variable)
 (struct-out operation)
 (struct-out conditional)
 (struct-out binding)
 (struct-out let-expression)
 (struct-out function-expression)
 (struct-out application)
 (struct-out list-expression)
 (struct-out cons-expression)
 (struct-out first-expression)
 (struct-out rest-expression)
 (struct-out empty-predicate)
 (struct-out list-match)
 (struct-out definition)
 expression-location
 free-variables)

(struct source-location (source line column position span) #:transparent)

(struct literal (value location) #:transparent)
(struct variable (name location) #:transparent)
(struct operation (name operands location) #:transparent)
(struct conditional (test consequent alternative location) #:transparent)
(struct binding (name value) #:transparent)
(struct let-expression (bindings body location) #:transparent)
(struct function-expression (name parameters body location) #:transparent)
(struct application (callee arguments location) #:transparent)
(struct list-expression (elements location) #:transparent)
(struct cons-expression (head tail location) #:transparent)
(struct first-expression (list location) #:transparent)
(struct rest-expression (list location) #:transparent)
(struct empty-predicate (list location) #:transparent)
(struct list-match
  (subject empty-case head-name tail-name cons-case location)
  #:transparent)
(struct definition (name value location) #:transparent)

(define (expression-location expression)
  (match expression
    [(literal _ location) location]
    [(variable _ location) location]
    [(operation _ _ location) location]
    [(conditional _ _ _ location) location]
    [(let-expression _ _ location) location]
    [(function-expression _ _ _ location) location]
    [(application _ _ location) location]
    [(list-expression _ location) location]
    [(cons-expression _ _ location) location]
    [(first-expression _ location) location]
    [(rest-expression _ location) location]
    [(empty-predicate _ location) location]
    [(list-match _ _ _ _ _ location) location]
    [(definition _ _ location) location]))

(define (free-variables expression)
  (define (union-all expressions)
    (for/fold ([names (seteq)])
              ([item (in-list expressions)])
      (set-union names (free-variables item))))

  (match expression
    [(literal _ _) (seteq)]
    [(variable name _) (seteq name)]
    [(operation _ operands _) (union-all operands)]
    [(conditional test consequent alternative _)
     (union-all (list test consequent alternative))]
    [(let-expression bindings body _)
     (define names
       (for/seteq ([item (in-list bindings)])
         (binding-name item)))
     (set-union
      (union-all (map binding-value bindings))
      (set-subtract (free-variables body) names))]
    [(function-expression name parameters body _)
     (define bound
       (list->seteq
        (if name
            (cons name parameters)
            parameters)))
     (set-subtract (free-variables body) bound)]
    [(application callee arguments _)
     (union-all (cons callee arguments))]
    [(list-expression elements _) (union-all elements)]
    [(cons-expression head tail _)
     (union-all (list head tail))]
    [(first-expression list _) (free-variables list)]
    [(rest-expression list _) (free-variables list)]
    [(empty-predicate list _) (free-variables list)]
    [(list-match subject empty-case head-name tail-name cons-case _)
     (set-union
      (free-variables subject)
      (free-variables empty-case)
      (set-subtract
       (free-variables cons-case)
       (seteq head-name tail-name)))]
    [(definition name value _)
     (set-remove (free-variables value) name)]))
