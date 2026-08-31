#lang racket/base

(require racket/cmdline
         racket/port
         "main.rkt")

(provide krit-version main repl)

(define krit-version "0.1.0")

(define (report-error exception)
  (eprintf "krit: ~a\n" (exn-message exception))
  1)

(define (with-reported-errors action)
  (with-handlers ([exn:fail:krit? report-error]
                  [exn:fail:read? report-error]
                  [exn:fail:filesystem? report-error])
    (action)))

(define (execute-port input source environment print-result?)
  (define result (evaluate-port input source environment))
  (when (and print-result? (not (void? result)))
    (displayln (value->string result)))
  0)

(define (execute-source source-text)
  (with-reported-errors
   (lambda ()
     (call-with-input-string
      source-text
      (lambda (input)
        (execute-port
         input
         "<command-line>"
         (make-global-environment)
         #t))))))

(define (execute-file path)
  (with-reported-errors
   (lambda ()
     (call-with-input-file
      path
      (lambda (input)
        (execute-port input path (make-global-environment) #f))))))

(define (repl)
  (define environment (make-global-environment))
  (port-count-lines! (current-input-port))
  (displayln (format "Krit ~a -- press Ctrl-D to exit" krit-version))
  (let loop ()
    (display "krit> ")
    (flush-output)
    (define status
      (with-handlers ([exn:fail:krit?
                       (lambda (exception)
                         (report-error exception))]
                      [exn:fail:read?
                       (lambda (exception)
                         (report-error exception))])
        (parameterize ([read-accept-lang #f]
                       [read-accept-reader #f])
          (define syntax
            (read-syntax "<repl>" (current-input-port)))
          (cond
            [(eof-object? syntax)
             (newline)
             'done]
            [else
             (define form (parse-top-level-syntax syntax))
             (define result (evaluate-form form environment))
             (if (definition? form)
                 (displayln (format "~a defined" (definition-name form)))
                 (displayln (value->string result)))
             'continue]))))
    (cond
      [(eq? status 'done) 0]
      [else (loop)])))

(define (main [arguments (current-command-line-arguments)])
  (define source-text #f)
  (define show-version? #f)
  (define files null)
  (command-line
   #:program "krit"
   #:argv arguments
   #:once-each
   [("-e" "--eval") source
                    "Evaluate SOURCE and print its result"
                    (set! source-text source)]
   [("-v" "--version")
    "Print the Krit version"
    (set! show-version? #t)]
   #:args paths
   (set! files paths))
  (cond
    [show-version?
     (displayln (format "Krit ~a (Racket ~a)" krit-version (version)))
     0]
    [(and source-text (pair? files))
     (eprintf "krit: --eval cannot be combined with a file\n")
     2]
    [(> (length files) 1)
     (eprintf "krit: expected at most one source file\n")
     2]
    [source-text (execute-source source-text)]
    [(pair? files) (execute-file (car files))]
    [else (repl)]))

(module+ main
  (exit (main)))
