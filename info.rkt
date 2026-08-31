#lang info

(define collection "krit")
(define deps '(("base" #:version "9.3")))
(define build-deps '("rackunit-lib"))
(define pkg-desc
  "A small functional language for learning interpreters and language design")
(define version "0.1")
(define pkg-authors '("akshay-bhardwaj"))
(define license 'Apache-2.0)
(define racket-minimum-version "9.3")
(define racket-launcher-names '("krit"))
(define racket-launcher-libraries '("launcher.rkt"))
(define raco-commands
  '(("krit" (submod krit/cli main) "run the Krit interpreter" #f)))
(define test-omit-paths '("launcher.rkt"))
