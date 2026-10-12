use std::collections::{BTreeMap, HashSet};

use crate::{
    Diagnostic, Source, Span,
    ast::{
        BinaryOperator, Block, Expression, ExpressionKind, MatchArm, Parameter, Pattern,
        PatternKind, Program, RecordField, RecordTypeField, Statement, StatementKind,
        TypeAnnotation, TypeKind, UnaryOperator, ValueLiteral, VariantArm, VariantName,
        VariantPattern,
    },
    habits::Habit,
    limits::{MAX_NESTING_DEPTH, MAX_STRUCTURAL_DEPTH, nesting_error, structural_error},
    token::{Token, TokenKind},
};

/// Parses `tokens`, which the lexer produced from `source`.
///
/// The source text tells the parser where lines break between tokens, which
/// decides whether a word from another language starts a foreign construct.
pub fn parse(source: &Source, tokens: Vec<Token>) -> Result<Program, Diagnostic> {
    Parser::new(source.text(), tokens).program()
}

struct Parser<'source> {
    /// The text the tokens' spans index.
    text: &'source str,
    tokens: Vec<Token>,
    cursor: usize,
    /// Syntactic nesting levels currently open (bounded by
    /// [`MAX_NESTING_DEPTH`]).
    depth: usize,
    /// Enclosing `else if` continuations; added to `depth` to give the
    /// structural depth of the current point.
    chain_depth: usize,
    /// The deepest structural depth reached since the innermost enclosing
    /// operator or postfix chain started measuring its current operand.
    ///
    /// Those chains nest to the left, so an operand's final depth is known
    /// only once the chain ends; they measure each operand's height with this
    /// mark and check the depth of the tree they build (bounded by
    /// [`MAX_STRUCTURAL_DEPTH`]).
    peak: usize,
    /// Conditional-operator checks currently parsing a first branch
    /// speculatively ([`Self::ternary_after`]).
    speculations: usize,
    /// For each `?` token followed by a `:` that could end a conditional
    /// operator's first branch, the `?` to report and that `:`
    /// ([`ternary_colons`]).
    ternary_colons: BTreeMap<usize, (Span, Span)>,
}

impl<'source> Parser<'source> {
    fn new(text: &'source str, tokens: Vec<Token>) -> Self {
        let ternary_colons = ternary_colons(&tokens);
        Self {
            text,
            tokens,
            cursor: 0,
            depth: 0,
            chain_depth: 0,
            peak: 0,
            speculations: 0,
            ternary_colons,
        }
    }

    fn program(mut self) -> Result<Program, Diagnostic> {
        let mut statements = Vec::new();
        let mut entrypoint = None;
        while !self.check(&TokenKind::Eof) {
            let statement = if self.check(&TokenKind::Webhook) {
                self.webhook_declaration()?
            } else if self.entrypoint_keyword().is_some() {
                self.resource_entrypoint_declaration()?
            } else {
                self.statement()?
            };
            if matches!(
                statement.kind,
                StatementKind::Webhook { .. }
                    | StatementKind::QueueConsumer { .. }
                    | StatementKind::ScheduleHandler { .. }
            ) {
                if entrypoint.is_some() {
                    return Err(Diagnostic::new(
                        "K2002",
                        "a source module may declare at most one `webhook`, `queue`, or `schedule` entrypoint",
                        statement.span,
                    ));
                }
                entrypoint = Some(statement.span);
            }
            statements.push(statement);
        }
        Ok(Program { statements })
    }

    fn statement(&mut self) -> Result<Statement, Diagnostic> {
        if let Some(keyword) = self.nested_entrypoint_keyword() {
            Err(Diagnostic::new(
                "K1004",
                format!("`{keyword}` declarations are only allowed at the top level"),
                self.current().span,
            ))
        } else if self.check(&TokenKind::Let) {
            self.let_declaration()
        } else if self.check(&TokenKind::Fn)
            && matches!(
                self.tokens.get(self.cursor + 1).map(|token| &token.kind),
                Some(TokenKind::Identifier(_))
            )
        {
            self.function_declaration()
        } else {
            if let Some(habit) = self.statement_habit() {
                return Err(habit);
            }
            let expression = self.expression()?;
            if let Some(assignment) = self.assignment_after(&expression) {
                return Err(assignment);
            }
            let semicolon = self.expect(TokenKind::Semicolon)?;
            let span = expression.span.join(semicolon.span);
            Ok(Statement {
                kind: StatementKind::Expression(expression),
                span,
            })
        }
    }

    fn let_declaration(&mut self) -> Result<Statement, Diagnostic> {
        let start = self.expect(TokenKind::Let)?.span;
        if self.at_variant_pattern() {
            return self.let_else(start);
        }
        let (name, _) = self.binding_name()?;
        let annotation = if self.consume(&TokenKind::Colon) {
            Some(self.type_annotation()?)
        } else {
            None
        };
        self.expect(TokenKind::Equal)?;
        let value = self.expression()?;
        if let Some(assignment) = self.assignment_after(&value) {
            return Err(assignment);
        }
        let end = self.expect(TokenKind::Semicolon)?.span;
        Ok(Statement {
            kind: StatementKind::Let {
                name,
                annotation,
                value,
            },
            span: start.join(end),
        })
    }

    /// Whether a variant pattern starts at the current token: `None`, or
    /// `Some`, `Ok`, or `Err` followed by `(`.
    ///
    /// The four names are reserved, so no `let` binding starts this way.
    fn at_variant_pattern(&self) -> bool {
        let TokenKind::Identifier(name) = &self.current().kind else {
            return false;
        };
        match name.as_str() {
            "None" => true,
            "Some" | "Ok" | "Err" => matches!(
                self.tokens.get(self.cursor + 1).map(|token| &token.kind),
                Some(TokenKind::LeftParen)
            ),
            _ => false,
        }
    }

    /// Parses the rest of a let-else after its `let` at `start`:
    /// `PATTERN (: TYPE)? = value else ARM;`.
    ///
    /// The value may not end in `}` (`K3014`): `if c { a } else { b } else
    /// ...` would read as one `if` with two `else` keywords. The arm opens one
    /// nesting level, as the arms of a `match` do.
    fn let_else(&mut self, start: Span) -> Result<Statement, Diagnostic> {
        let pattern = self.variant_pattern()?;
        let annotation = if self.consume(&TokenKind::Colon) {
            Some(self.type_annotation()?)
        } else {
            None
        };
        self.expect(TokenKind::Equal)?;
        let value = self.expression()?;
        if let Some(assignment) = self.assignment_after(&value) {
            return Err(assignment);
        }
        let complement = pattern.variant.complement().example_pattern();
        if !self.check(&TokenKind::Else) {
            let written = match &pattern.binding {
                Some(binding) => format!("{}({binding})", pattern.variant.as_str()),
                None => pattern.variant.as_str().to_owned(),
            };
            return Err(Diagnostic::new(
                "K1002",
                format!(
                    "expected `else` after the value of `let {written}`, found {}",
                    self.current().kind.description()
                ),
                self.current().span,
            )
            .with_note(format!(
                "a `let` with a variant pattern names what happens otherwise: \
                 `let {written} = value else {complement} => fallback;`"
            )));
        }
        if self.previous().kind == TokenKind::RightBrace {
            return Err(Diagnostic::new(
                "K3014",
                "the value of a let-else cannot end in `}`",
                value.span,
            )
            .with_note(
                "bind the value with `let` first and use its name here, or wrap the value in \
                 parentheses, so its `}` cannot be read as part of the `else`",
            ));
        }
        let else_span = self.advance().span;
        if let Some(habit) = self.else_return(else_span) {
            return Err(habit);
        }
        if self.check(&TokenKind::LeftBrace) {
            return Err(Diagnostic::new(
                "K1003",
                "expected a let-else arm after `else`, found `{`",
                self.current().span,
            )
            .with_note(format!(
                "a let-else names its other variant like a `match` arm: \
                 `else {complement} => fallback;`"
            )));
        }
        let arm = self.nested(else_span, |parser| {
            let arm_pattern = parser.variant_pattern()?;
            parser.expect(TokenKind::FatArrow)?;
            let value = parser.expression()?;
            Ok(VariantArm {
                variant: arm_pattern.variant,
                binding: arm_pattern.binding,
                span: arm_pattern.span.join(value.span),
                value,
            })
        })?;
        let end = self.expect(TokenKind::Semicolon)?.span;
        Ok(Statement {
            kind: StatementKind::LetElse {
                pattern,
                annotation,
                value,
                else_span,
                arm: Box::new(arm),
            },
            span: start.join(end),
        })
    }

    fn function_declaration(&mut self) -> Result<Statement, Diagnostic> {
        let start = self.expect(TokenKind::Fn)?.span;
        let (name, _) = self.binding_name()?;
        let parameters = self.parameters()?;
        let return_type = if self.consume(&TokenKind::ThinArrow) {
            Some(self.type_annotation()?)
        } else {
            None
        };
        let body = self.block()?;
        let span = start.join(body.span);
        Ok(Statement {
            kind: StatementKind::Function {
                name,
                parameters,
                return_type,
                body,
            },
            span,
        })
    }

    fn webhook_declaration(&mut self) -> Result<Statement, Diagnostic> {
        let start = self.expect(TokenKind::Webhook)?.span;
        self.expect(TokenKind::Fn)?;
        let (name, _) = self.binding_name()?;
        let parameters = self.parameters()?;
        let return_type = if self.consume(&TokenKind::ThinArrow) {
            Some(self.type_annotation()?)
        } else {
            None
        };
        let body = self.block()?;
        let span = start.join(body.span);
        Ok(Statement {
            kind: StatementKind::Webhook {
                name,
                parameters,
                return_type,
                body,
            },
            span,
        })
    }

    /// Parses `queue "name" fn ...` or `schedule "name" fn ...`.
    ///
    /// `queue` and `schedule` are contextual: they introduce an entrypoint only
    /// when followed by a direct name literal and `fn`, so ordinary bindings and
    /// record fields keep those spellings.
    fn resource_entrypoint_declaration(&mut self) -> Result<Statement, Diagnostic> {
        let keyword = self
            .entrypoint_keyword()
            .ok_or_else(|| self.expected("`queue` or `schedule`"))?;
        let start = self.advance().span;
        let token = self.advance().clone();
        let TokenKind::String(resource) = token.kind else {
            return Err(Diagnostic::new(
                "K1002",
                format!(
                    "`{keyword}` declarations require a direct canonical `{keyword}` name literal"
                ),
                token.span,
            ));
        };
        let resource_span = token.span;
        self.expect(TokenKind::Fn)?;
        let (name, _) = self.binding_name()?;
        let parameters = self.parameters()?;
        let return_type = if self.consume(&TokenKind::ThinArrow) {
            Some(self.type_annotation()?)
        } else {
            None
        };
        let body = self.block()?;
        let span = start.join(body.span);
        let kind = if keyword == "queue" {
            StatementKind::QueueConsumer {
                queue: resource,
                queue_span: resource_span,
                name,
                parameters,
                return_type,
                body,
            }
        } else {
            StatementKind::ScheduleHandler {
                schedule: resource,
                schedule_span: resource_span,
                name,
                parameters,
                return_type,
                body,
            }
        };
        Ok(Statement { kind, span })
    }

    fn entrypoint_keyword(&self) -> Option<&'static str> {
        let TokenKind::Identifier(name) = &self.current().kind else {
            return None;
        };
        let keyword = match name.as_str() {
            "queue" => "queue",
            "schedule" => "schedule",
            _ => return None,
        };
        let follows_literal = matches!(
            self.tokens.get(self.cursor + 1).map(|token| &token.kind),
            Some(TokenKind::String(_))
        );
        let follows_fn = matches!(
            self.tokens.get(self.cursor + 2).map(|token| &token.kind),
            Some(TokenKind::Fn)
        );
        (follows_literal && follows_fn).then_some(keyword)
    }

    fn nested_entrypoint_keyword(&self) -> Option<&'static str> {
        if self.check(&TokenKind::Webhook) {
            return Some("webhook");
        }
        self.entrypoint_keyword()
    }

    fn parameters(&mut self) -> Result<Vec<Parameter>, Diagnostic> {
        self.expect(TokenKind::LeftParen)?;
        let mut parameters = Vec::new();
        let mut seen = HashSet::new();

        if !self.check(&TokenKind::RightParen) {
            loop {
                let (name, span) = self.binding_name()?;
                // The discard name `_` may name any number of parameters.
                if name != "_" && !seen.insert(name.clone()) {
                    return Err(Diagnostic::new(
                        "K2002",
                        format!("duplicate parameter `{name}`"),
                        span,
                    ));
                }
                let annotation = if self.consume(&TokenKind::Colon) {
                    Some(self.type_annotation()?)
                } else {
                    None
                };
                parameters.push(Parameter {
                    name,
                    annotation,
                    span,
                });

                if !self.consume(&TokenKind::Comma) {
                    break;
                }
                if self.check(&TokenKind::RightParen) {
                    break;
                }
            }
        }

        self.expect(TokenKind::RightParen)?;
        Ok(parameters)
    }

    fn block(&mut self) -> Result<Block, Diagnostic> {
        let start = self.expect(TokenKind::LeftBrace)?.span;
        self.nested(start, |parser| parser.block_contents(start))
    }

    fn block_contents(&mut self, start: Span) -> Result<Block, Diagnostic> {
        let mut statements = Vec::new();
        let mut tail = None;

        while !self.check(&TokenKind::RightBrace) {
            if self.check(&TokenKind::Eof) {
                return Err(self.expected("`}`"));
            }
            if let Some(keyword) = self.nested_entrypoint_keyword() {
                return Err(Diagnostic::new(
                    "K1004",
                    format!("`{keyword}` declarations are only allowed at the top level"),
                    self.current().span,
                ));
            }

            if self.check(&TokenKind::Let)
                || (self.check(&TokenKind::Fn)
                    && matches!(
                        self.tokens.get(self.cursor + 1).map(|token| &token.kind),
                        Some(TokenKind::Identifier(_))
                    ))
            {
                statements.push(self.statement()?);
                continue;
            }

            if let Some(habit) = self.statement_habit() {
                return Err(habit);
            }
            let expression = self.expression()?;
            if let Some(assignment) = self.assignment_after(&expression) {
                return Err(assignment);
            }
            if self.consume(&TokenKind::Semicolon) {
                let end = self.previous().span;
                statements.push(Statement {
                    span: expression.span.join(end),
                    kind: StatementKind::Expression(expression),
                });
            } else if self.check(&TokenKind::RightBrace) {
                tail = Some(Box::new(expression));
                break;
            } else {
                return Err(self.expected("`;` or `}`"));
            }
        }

        let end = self.expect(TokenKind::RightBrace)?.span;
        Ok(Block {
            statements,
            tail,
            span: start.join(end),
        })
    }

    fn expression(&mut self) -> Result<Expression, Diagnostic> {
        self.logical_or()
    }

    fn logical_or(&mut self) -> Result<Expression, Diagnostic> {
        self.binary_chain(Self::logical_and, |kind| match kind {
            TokenKind::OrOr => Some(BinaryOperator::Or),
            _ => None,
        })
    }

    fn logical_and(&mut self) -> Result<Expression, Diagnostic> {
        self.binary_chain(Self::equality, |kind| match kind {
            TokenKind::AndAnd => Some(BinaryOperator::And),
            _ => None,
        })
    }

    fn equality(&mut self) -> Result<Expression, Diagnostic> {
        self.binary_chain(Self::comparison, |kind| match kind {
            TokenKind::EqualEqual => Some(BinaryOperator::Equal),
            TokenKind::BangEqual => Some(BinaryOperator::NotEqual),
            _ => None,
        })
    }

    fn comparison(&mut self) -> Result<Expression, Diagnostic> {
        self.binary_chain(Self::term, |kind| match kind {
            TokenKind::Less => Some(BinaryOperator::Less),
            TokenKind::LessEqual => Some(BinaryOperator::LessEqual),
            TokenKind::Greater => Some(BinaryOperator::Greater),
            TokenKind::GreaterEqual => Some(BinaryOperator::GreaterEqual),
            _ => None,
        })
    }

    fn term(&mut self) -> Result<Expression, Diagnostic> {
        self.binary_chain(Self::factor, |kind| match kind {
            TokenKind::Plus => Some(BinaryOperator::Add),
            TokenKind::Minus => Some(BinaryOperator::Subtract),
            _ => None,
        })
    }

    fn factor(&mut self) -> Result<Expression, Diagnostic> {
        self.binary_chain(Self::unary, |kind| match kind {
            TokenKind::Star => Some(BinaryOperator::Multiply),
            TokenKind::Slash => Some(BinaryOperator::Divide),
            TokenKind::Percent => Some(BinaryOperator::Remainder),
            _ => None,
        })
    }

    /// Parses one left-associative precedence level.
    ///
    /// A chain such as `a + b + c` is flat source, so it does not open
    /// syntactic levels, but the resulting tree nests to the left: `a` sits
    /// below both operators. Every operand is parsed at the chain's own depth
    /// and measured, and the chain checks the structural depth of the tree it
    /// builds after each operator, so a deep left operand counts every
    /// operator that ends up above it.
    fn binary_chain(
        &mut self,
        operand: impl Fn(&mut Self) -> Result<Expression, Diagnostic>,
        operator: impl Fn(&TokenKind) -> Option<BinaryOperator>,
    ) -> Result<Expression, Diagnostic> {
        let base = self.structural_depth();
        let outer_peak = std::mem::replace(&mut self.peak, base);
        let mut expression = operand(self)?;
        // Levels the tree built so far reaches below the chain's own depth.
        let mut height = self.peak - base;
        while let Some(operator) = operator(&self.current().kind) {
            let operator_span = self.advance().span;
            self.peak = base;
            let right = operand(self)?;
            height = 1 + height.max(self.peak - base);
            self.reach(base + height, operator_span)?;
            expression = binary(expression, operator, right);
        }
        self.peak = self.peak.max(outer_peak);
        Ok(expression)
    }

    fn unary(&mut self) -> Result<Expression, Diagnostic> {
        let (operator, start) = if self.consume(&TokenKind::Bang) {
            (UnaryOperator::Not, self.previous().span)
        } else if self.consume(&TokenKind::Minus) {
            (UnaryOperator::Negate, self.previous().span)
        } else {
            return self.call();
        };

        let operand = self.nested(start, Self::unary)?;
        let span = start.join(operand.span);
        Ok(Expression {
            kind: ExpressionKind::Unary {
                operator,
                operand: Box::new(operand),
            },
            span,
        })
    }

    fn call(&mut self) -> Result<Expression, Diagnostic> {
        let base = self.structural_depth();
        let outer_peak = std::mem::replace(&mut self.peak, base);
        let mut expression = self.primary()?;
        // Each call, field-access, or `?` link opens one level for the rest of
        // the chain. The tree nests the other way, with the first callee or
        // record deepest, so the chain also measures the tree it builds and
        // checks its structural depth after each link, as operator chains do.
        let mut height = self.peak - base;
        let depth = self.depth;
        loop {
            if self.check(&TokenKind::LeftParen) {
                let open = self.advance().span;
                self.enter(open)?;
                let arguments_depth = self.structural_depth();
                let callee_peak = std::mem::replace(&mut self.peak, arguments_depth);
                let mut arguments = Vec::new();
                if !self.check(&TokenKind::RightParen) {
                    loop {
                        arguments.push(self.expression()?);
                        if !self.consume(&TokenKind::Comma) {
                            break;
                        }
                        if self.check(&TokenKind::RightParen) {
                            break;
                        }
                    }
                }
                height = 1 + height.max(self.peak - arguments_depth);
                self.peak = self.peak.max(callee_peak);
                self.reach(base + height, open)?;
                let end = self.expect(TokenKind::RightParen)?.span;
                let span = expression.span.join(end);
                expression = Expression {
                    kind: ExpressionKind::Call {
                        callee: Box::new(expression),
                        arguments,
                    },
                    span,
                };
            } else if self.check(&TokenKind::Dot) {
                let dot = self.advance().span;
                self.enter(dot)?;
                height += 1;
                self.reach(base + height, dot)?;
                let (field, field_span) = self.identifier()?;
                let span = expression.span.join(field_span);
                expression = Expression {
                    kind: ExpressionKind::FieldAccess {
                        value: Box::new(expression),
                        field,
                    },
                    span,
                };
            } else if self.check(&TokenKind::LeftBracket)
                && self.previous().span.end == self.current().span.start
            {
                return Err(Habit::Indexing.diagnostic(
                    "indexing with `[...]` is not Krit syntax",
                    expression.span.join(self.current().span),
                ));
            } else if self.check(&TokenKind::Question) {
                let question = self.advance().span;
                if let Some(error) = self.question_misuse(question) {
                    return Err(error);
                }
                self.enter(question)?;
                height += 1;
                self.reach(base + height, question)?;
                let span = expression.span.join(question);
                expression = Expression {
                    kind: ExpressionKind::Propagate {
                        operand: Box::new(expression),
                        question,
                    },
                    span,
                };
            } else {
                break;
            }
        }
        self.depth = depth;
        self.peak = self.peak.max(outer_peak);
        Ok(expression)
    }

    fn primary(&mut self) -> Result<Expression, Diagnostic> {
        let token = self.advance().clone();
        match token.kind {
            TokenKind::Integer(value) => {
                let value = value.parse::<i128>().map_err(|_| {
                    Diagnostic::new("K4005", "integer literal is out of range", token.span)
                })?;
                Ok(Expression {
                    kind: ExpressionKind::Literal(ValueLiteral::Integer(value)),
                    span: token.span,
                })
            }
            TokenKind::String(value) => Ok(Expression {
                kind: ExpressionKind::Literal(ValueLiteral::String(value)),
                span: token.span,
            }),
            TokenKind::True => Ok(Expression {
                kind: ExpressionKind::Literal(ValueLiteral::Boolean(true)),
                span: token.span,
            }),
            TokenKind::False => Ok(Expression {
                kind: ExpressionKind::Literal(ValueLiteral::Boolean(false)),
                span: token.span,
            }),
            TokenKind::Identifier(name) => {
                if self.check(&TokenKind::FatArrow) {
                    return Err(arrow_function(token.span.join(self.current().span)));
                }
                if let Some(habit) = self.word_habit(&name, token.span) {
                    return Err(habit);
                }
                Ok(Expression {
                    kind: ExpressionKind::Variable(name),
                    span: token.span,
                })
            }
            TokenKind::LeftParen => self.parenthesized(token.span),
            TokenKind::LeftBracket => self.list(token.span),
            TokenKind::Record => self.record(token.span),
            TokenKind::LeftBrace => {
                self.cursor -= 1;
                let block = self.block()?;
                Ok(Expression {
                    span: block.span,
                    kind: ExpressionKind::Block(block),
                })
            }
            TokenKind::If => self.if_expression(token.span),
            TokenKind::Match => self.match_expression(token.span),
            TokenKind::Fn => self.function_expression(token.span),
            _ => Err(self.mutation_operator(&token).unwrap_or_else(|| {
                Diagnostic::new(
                    "K1001",
                    format!("unexpected {}", token.kind.description()),
                    token.span,
                )
            })),
        }
    }

    /// Parses `( expression )` after its `(` at `open`, and reports an arrow
    /// function's parameter list followed by `=>` (`K1109`): `(x) => e`,
    /// `(x, y) => e`, `() => e`.
    fn parenthesized(&mut self, open: Span) -> Result<Expression, Diagnostic> {
        let open_index = self.cursor - 1;
        let expression = match self.nested(open, Self::expression) {
            Ok(expression) => expression,
            // `()` and parameters such as `(...rest)` are not expressions.
            Err(error) if !is_settled(&error) => {
                return Err(self.arrow_after_group(open_index).unwrap_or(error));
            }
            Err(error) => return Err(error),
        };
        if !self.check(&TokenKind::RightParen) {
            // `(x, y)` or `(x: Int)`.
            return Err(self
                .arrow_after_group(open_index)
                .unwrap_or_else(|| self.expected(TokenKind::RightParen.description())));
        }
        let end = self.advance().span;
        if self.check(&TokenKind::FatArrow) {
            return Err(arrow_function(open.join(self.current().span)));
        }
        Ok(Expression {
            span: open.join(end),
            ..expression
        })
    }

    /// Reports a habit word used as another language's keyword: `word` at
    /// `span`, just consumed in expression position, followed by an operand
    /// (`return value`, `for item in items`, `const limit = 3`,
    /// `class Point`) or by a parenthesized group and `=>`
    /// (`async (x) => e`).
    ///
    /// An identifier expression is never followed by either, so a habit word
    /// used as an ordinary name is never reported.
    fn word_habit(&self, word: &str, span: Span) -> Option<Diagnostic> {
        let habit = Habit::of_word(word)?;
        let next = self.current();
        let foreign = self.follows_as_operand(span, next)
            || (next.kind == TokenKind::LeftParen && self.arrow_after_group(self.cursor).is_some());
        foreign.then(|| habit.word_diagnostic(word, span))
    }

    /// Reports a statement that starts with a habit word followed by `{`,
    /// `:`, or a parenthesized group and `{`: `try {`, `loop {`, `default:`,
    /// `while (ready) {`, `for (let i = 0; i < n; i++) {`.
    ///
    /// No expression statement continues that way, so a habit word used as an
    /// ordinary name is never reported. Habit words followed by an operand are
    /// reported when the expression is parsed ([`Self::word_habit`]).
    fn statement_habit(&self) -> Option<Diagnostic> {
        let word = self.current();
        let TokenKind::Identifier(name) = &word.kind else {
            return None;
        };
        let habit = Habit::of_word(name)?;
        let foreign = match &self.tokens.get(self.cursor + 1)?.kind {
            TokenKind::LeftBrace | TokenKind::Colon => true,
            TokenKind::LeftParen => self
                .matching_paren(self.cursor + 1, |kind| {
                    matches!(kind, TokenKind::LeftBrace | TokenKind::RightBrace)
                })
                .is_some_and(|close| {
                    self.tokens
                        .get(close + 1)
                        .is_some_and(|token| token.kind == TokenKind::LeftBrace)
                }),
            _ => false,
        };
        foreign.then(|| habit.word_diagnostic(name, word.span))
    }

    /// Reports `target = value` (`K1103`): an expression statement, a block's
    /// final expression, or a `let` value followed by `=`.
    fn assignment_after(&self, target: &Expression) -> Option<Diagnostic> {
        self.check(&TokenKind::Equal).then(|| {
            Habit::Mutation.diagnostic(
                "assignment is not Krit syntax",
                target.span.join(self.current().span),
            )
        })
    }

    /// Reports the compound assignment, increment, or decrement operator that
    /// ends at `unexpected`, the token `primary` just consumed and could not
    /// start an operand with: `x += 1`, `x++`, `x--` (`K1103`).
    ///
    /// Each lexes as an operator touching `=` or a second operator, which
    /// leaves an operand missing where the second token stands.
    fn mutation_operator(&self, unexpected: &Token) -> Option<Diagnostic> {
        // Consuming the end of the file leaves the cursor on it.
        let index = if unexpected.kind == TokenKind::Eof {
            self.cursor
        } else {
            self.cursor - 1
        };
        let before = &self.tokens[index.checked_sub(1)?];
        let (first, second, operator) = match (&before.kind, &unexpected.kind) {
            (TokenKind::Plus, TokenKind::Equal) => (before, unexpected, "+="),
            (TokenKind::Minus, TokenKind::Equal) => (before, unexpected, "-="),
            (TokenKind::Star, TokenKind::Equal) => (before, unexpected, "*="),
            (TokenKind::Slash, TokenKind::Equal) => (before, unexpected, "/="),
            (TokenKind::Percent, TokenKind::Equal) => (before, unexpected, "%="),
            (TokenKind::Plus, TokenKind::Plus) => (before, unexpected, "++"),
            // `x--` parses as `x - -` with the negated operand missing.
            (TokenKind::Minus, _) => {
                let first = &self.tokens[index.checked_sub(2)?];
                if first.kind != TokenKind::Minus {
                    return None;
                }
                (first, before, "--")
            }
            _ => return None,
        };
        (first.span.end == second.span.start).then(|| {
            Habit::Mutation.diagnostic(
                format!("`{operator}` is not Krit syntax"),
                first.span.join(second.span),
            )
        })
    }

    /// Diagnoses the `?` at `question`, just consumed after an operand, when it
    /// is not a postfix `?` operator, or returns `None` when it is one.
    ///
    /// `c ? a : b` is another language's conditional operator (`K1110`). The
    /// token after the `?` decides how it is recognized:
    ///
    /// - A token that cannot start an expression (`;`, `)`, `.`, an operator
    ///   other than `-`, `?`, ...) follows only a postfix `?`.
    /// - A token that can only start an operand (an identifier, a literal,
    ///   `!`, ...) never follows a postfix `?`, so the first branch is parsed
    ///   speculatively ([`Self::ternary_after`]). Without a conditional
    ///   operator, the `?` is unexpected (`K1001`) when that token is on the
    ///   same line; on a later line it is a postfix `?` before a missing `;`
    ///   or `,`, which the parser reports next.
    /// - `-`, `(`, `[`, or `{` may continue a postfix `?` (`x? - 1`,
    ///   `f()?(1)`, `if x? {`), so only the bracket scan decides
    ///   ([`ternary_colons`]): a `:` at the `?`'s bracket depth before the
    ///   branch could end makes it a conditional operator, which no valid
    ///   program contains there. Nothing is parsed twice, so a long chain of
    ///   postfix `?` operators parses in linear time.
    fn question_misuse(&mut self, question: Span) -> Option<Diagnostic> {
        let next = self.current().clone();
        if !starts_expression(&next.kind) {
            return None;
        }
        if !starts_operand_only(&next.kind) {
            let &(innermost, colon) = self.ternary_colons.get(&(self.cursor - 1))?;
            return Some(ternary(innermost.join(colon)));
        }
        if let Some(error) = self.ternary_after(question) {
            return Some(error);
        }
        self.same_line(question, next.span).then(|| {
            Diagnostic::new("K1001", "unexpected `?`", question).with_note(
                "postfix `?` takes no operand after it; to choose between two values, write \
                 `if condition { a } else { b }`",
            )
        })
    }

    /// Returns `K1110` when the `?` at `question`, followed by a token that can
    /// only start an operand, starts another language's conditional operator:
    /// the tokens after it are an expression and then `:`, or they start an
    /// expression that does not parse and a `:` follows at the `?`'s bracket
    /// depth ([`ternary_colons`]), as in `ready ? items[0] : None`.
    ///
    /// The first branch is parsed speculatively, one nesting level deeper, and
    /// the parser is restored either way, so the check never consumes input.
    /// A nesting limit (`K1005`) or a conditional operator reached inside the
    /// first branch is reported in place of the outer one.
    ///
    /// No valid program has an operand right after a `?`, so the parse fails
    /// once this check returns, whatever it finds: a `?` it does not report is
    /// either `K1001` or a postfix `?` that a missing `;` or `,` follows. A `?`
    /// inside the first branch that this check examines ends that branch the
    /// same way. The speculations therefore add at most one parse of the
    /// tokens after the outermost `?`, and only the outermost `?` looks up a
    /// `:` when its branch does not parse.
    fn ternary_after(&mut self, question: Span) -> Option<Diagnostic> {
        let question_index = self.cursor - 1;
        let saved = (self.cursor, self.depth, self.chain_depth, self.peak);
        self.speculations += 1;
        let branch = self.nested(question, Self::expression);
        self.speculations -= 1;
        let colon = self.check(&TokenKind::Colon).then(|| self.current().span);
        (self.cursor, self.depth, self.chain_depth, self.peak) = saved;
        let colon = match branch {
            Ok(_) => colon,
            Err(error) if error.code() == "K1005" || error.code() == Habit::Ternary.code() => {
                return Some(error);
            }
            Err(_) if self.speculations == 0 => self
                .ternary_colons
                .get(&question_index)
                .map(|&(_, colon)| colon),
            Err(_) => None,
        };
        colon.map(|colon| ternary(question.join(colon)))
    }

    /// Reports `else { return value }` or `else return value` (`K1112`): an
    /// early exit written as another language's guard clause.
    fn else_return(&self, else_span: Span) -> Option<Diagnostic> {
        let word = self.cursor + usize::from(self.check(&TokenKind::LeftBrace));
        let (word, next) = (self.tokens.get(word)?, self.tokens.get(word + 1)?);
        let is_return = matches!(&word.kind, TokenKind::Identifier(name) if name == "return");
        (is_return && self.follows_as_operand(word.span, next)).then(|| {
            Habit::ElseReturn.diagnostic(
                "`return` in an `else` block is not Krit syntax",
                else_span.join(word.span),
            )
        })
    }

    /// Whether `next` starts an operand right after the word ending at `word`,
    /// on the same line, which no expression is ever followed by:
    /// `return value`, `throw "boom"`, `await fetch()`, `return [a, b]`. A `[`
    /// touching the word is indexing (`K1108`) instead.
    ///
    /// An operand on a later line more likely starts the next statement or
    /// element after a missing `;` or `,` (`let x = from` and then
    /// `println(x);`), so the word is left to be an ordinary name.
    fn follows_as_operand(&self, word: Span, next: &Token) -> bool {
        self.same_line(word, next.span)
            && (starts_operand_only(&next.kind)
                || (next.kind == TokenKind::LeftBracket && next.span.start > word.end))
    }

    /// Whether no line break separates the end of `before` from the start of
    /// `after`.
    fn same_line(&self, before: Span, after: Span) -> bool {
        self.text
            .get(before.end..after.start)
            .is_some_and(|between| !between.contains(['\n', '\r']))
    }

    /// Reports `K1109` when the parenthesized group opening at token `open` is
    /// followed by `=>`, as an arrow function's parameter list is.
    fn arrow_after_group(&self, open: usize) -> Option<Diagnostic> {
        let close = self.matching_paren(open, |kind| *kind == TokenKind::Semicolon)?;
        let arrow = self.tokens.get(close + 1)?;
        (arrow.kind == TokenKind::FatArrow)
            .then(|| arrow_function(self.tokens[open].span.join(arrow.span)))
    }

    /// The index of the `)` that closes the `(` at token `open`, looking no
    /// further than the end of the file or the first token for which `stop`
    /// holds.
    fn matching_paren(&self, open: usize, stop: impl Fn(&TokenKind) -> bool) -> Option<usize> {
        let mut depth = 0_usize;
        for (index, token) in self.tokens.iter().enumerate().skip(open) {
            match &token.kind {
                TokenKind::LeftParen => depth += 1,
                TokenKind::RightParen => {
                    // A `)` before any `(` closes nothing.
                    depth = depth.checked_sub(1)?;
                    if depth == 0 {
                        return Some(index);
                    }
                }
                TokenKind::Eof => return None,
                kind if stop(kind) => return None,
                _ => {}
            }
        }
        None
    }

    fn list(&mut self, start: Span) -> Result<Expression, Diagnostic> {
        self.nested(start, |parser| parser.list_elements(start))
    }

    fn list_elements(&mut self, start: Span) -> Result<Expression, Diagnostic> {
        let mut elements = Vec::new();
        if !self.check(&TokenKind::RightBracket) {
            loop {
                elements.push(self.expression()?);
                if !self.consume(&TokenKind::Comma) {
                    break;
                }
                if self.check(&TokenKind::RightBracket) {
                    break;
                }
            }
        }
        let end = self.expect(TokenKind::RightBracket)?.span;
        Ok(Expression {
            kind: ExpressionKind::List(elements),
            span: start.join(end),
        })
    }

    fn record(&mut self, start: Span) -> Result<Expression, Diagnostic> {
        self.nested(start, |parser| parser.record_fields(start))
    }

    fn record_fields(&mut self, start: Span) -> Result<Expression, Diagnostic> {
        self.expect(TokenKind::LeftBrace)?;
        let mut fields = Vec::new();
        let mut seen = HashSet::new();
        if !self.check(&TokenKind::RightBrace) {
            loop {
                let (name, name_span) = self.identifier()?;
                if !seen.insert(name.clone()) {
                    return Err(Diagnostic::new(
                        "K2002",
                        format!("duplicate record field `{name}`"),
                        name_span,
                    ));
                }
                self.expect(TokenKind::Colon)?;
                let value = self.expression()?;
                fields.push(RecordField {
                    name,
                    span: name_span.join(value.span),
                    value,
                });
                if !self.consume(&TokenKind::Comma) {
                    break;
                }
                if self.check(&TokenKind::RightBrace) {
                    break;
                }
            }
        }
        let end = self.expect(TokenKind::RightBrace)?.span;
        Ok(Expression {
            kind: ExpressionKind::Record(fields),
            span: start.join(end),
        })
    }

    /// Parses an `if` expression and its whole `else if` chain.
    ///
    /// An `else if` continuation stays at the nesting level of the `if` it
    /// continues, so the chain is parsed iteratively and only counts toward
    /// the structural depth.
    fn if_expression(&mut self, start: Span) -> Result<Expression, Diagnostic> {
        let chain_depth = self.chain_depth;
        let mut links = Vec::new();
        let mut link_start = start;
        let alternative = loop {
            let condition = self.nested(link_start, Self::expression)?;
            let consequent = self.block()?;
            let else_span = self.expect(TokenKind::Else)?.span;
            if let Some(habit) = self.else_return(else_span) {
                return Err(habit);
            }
            links.push((link_start, condition, consequent));
            if self.check(&TokenKind::If) {
                link_start = self.advance().span;
                self.enter_chain_link(link_start)?;
            } else if self.check(&TokenKind::LeftBrace) {
                let block = self.block()?;
                break Expression {
                    span: block.span,
                    kind: ExpressionKind::Block(block),
                };
            } else {
                return Err(self.expected("`if` or `{`"));
            }
        };
        self.chain_depth = chain_depth;

        let mut expression = alternative;
        while let Some((start, condition, consequent)) = links.pop() {
            let span = start.join(expression.span);
            expression = Expression {
                kind: ExpressionKind::If {
                    condition: Box::new(condition),
                    consequent,
                    alternative: Box::new(expression),
                },
                span,
            };
        }
        Ok(expression)
    }

    fn function_expression(&mut self, start: Span) -> Result<Expression, Diagnostic> {
        if !self.check(&TokenKind::LeftParen) {
            return Err(self.expected("`(` after `fn`"));
        }
        let parameters = self.parameters()?;
        let return_type = if self.consume(&TokenKind::ThinArrow) {
            Some(self.type_annotation()?)
        } else {
            None
        };
        let body = self.block()?;
        let span = start.join(body.span);
        Ok(Expression {
            kind: ExpressionKind::Function {
                parameters,
                return_type,
                body,
            },
            span,
        })
    }

    fn match_expression(&mut self, start: Span) -> Result<Expression, Diagnostic> {
        let subject = self.nested(start, Self::expression)?;
        let open = self.expect(TokenKind::LeftBrace)?.span;
        let arms = self.nested(open, Self::match_arms)?;
        let end = self.expect(TokenKind::RightBrace)?.span;
        Ok(Expression {
            kind: ExpressionKind::Match {
                subject: Box::new(subject),
                arms,
            },
            span: start.join(end),
        })
    }

    /// Parses the comma-separated arms of a `match`, up to its `}`.
    ///
    /// Which patterns may stand together (one family at each position, no
    /// overlap, no top-level catch-all, every outcome named) is checked by
    /// analysis. An arm whose pattern nests variant or list patterns inside a
    /// payload nests its value one level deeper per nested pattern, as Core IR
    /// nests the matches that decide it.
    fn match_arms(&mut self) -> Result<Vec<MatchArm>, Diagnostic> {
        let mut arms = Vec::new();
        while !self.check(&TokenKind::RightBrace) {
            if self.check(&TokenKind::Eof) {
                return Err(self.expected("match arm or `}`"));
            }
            let pattern = self.pattern()?;
            self.expect(TokenKind::FatArrow)?;
            let levels = pattern.constructor_depth().saturating_sub(1);
            let value = self.nested_levels(levels, pattern.span, Self::expression)?;
            arms.push(MatchArm {
                span: pattern.span.join(value.span),
                pattern,
                value,
            });
            if !self.consume(&TokenKind::Comma) && !self.check(&TokenKind::RightBrace) {
                return Err(self.expected("`,` or `}`"));
            }
        }
        if arms.is_empty() {
            return Err(Diagnostic::new(
                "K1003",
                "a `match` needs an arm for each outcome, found `}`",
                self.current().span,
            ));
        }
        Ok(arms)
    }

    /// Parses a `match` pattern: a name, `_`, `Some(p)`, `Ok(p)`, `Err(p)`,
    /// `None`, `[]`, or `[head, ..tail]`.
    ///
    /// The parentheses of a variant pattern open one nesting level for its
    /// payload.
    fn pattern(&mut self) -> Result<Pattern, Diagnostic> {
        let token = self.current().clone();
        let name = match &token.kind {
            TokenKind::LeftBracket => return self.list_pattern(),
            TokenKind::Identifier(name) => name.clone(),
            _ => {
                return Err(Diagnostic::new(
                    "K1003",
                    format!(
                        "expected a match pattern, found {}",
                        token.kind.description()
                    ),
                    token.span,
                )
                .with_note(
                    "a pattern is a name, `_`, `Some(p)`, `None`, `Ok(p)`, `Err(p)`, `[]`, or \
                     `[head, ..tail]`; to compare values, use `if` and `==`",
                ));
            }
        };
        let follows_paren = matches!(
            self.tokens.get(self.cursor + 1).map(|token| &token.kind),
            Some(TokenKind::LeftParen)
        );
        let variant = match name.as_str() {
            "Some" => VariantName::Some,
            "None" => VariantName::None,
            "Ok" => VariantName::Ok,
            "Err" => VariantName::Err,
            "_" => {
                self.advance();
                return Ok(Pattern {
                    kind: PatternKind::Wildcard,
                    span: token.span,
                });
            }
            _ if follows_paren => {
                return Err(Diagnostic::new(
                    "K1003",
                    format!("unknown match variant `{name}`"),
                    token.span,
                ));
            }
            _ => {
                let (binding, span) = self.binding_name()?;
                return Ok(Pattern {
                    kind: PatternKind::Binder(binding),
                    span,
                });
            }
        };
        self.advance();
        if variant == VariantName::None {
            if self.check(&TokenKind::LeftParen) {
                return Err(Diagnostic::new(
                    "K1003",
                    "`None` does not bind a value",
                    self.current().span,
                ));
            }
            return Ok(Pattern {
                kind: PatternKind::Variant {
                    variant,
                    payload: None,
                },
                span: token.span,
            });
        }
        if !self.check(&TokenKind::LeftParen) {
            return Err(Diagnostic::new(
                "K1003",
                format!("`{}` must bind one value", variant.as_str()),
                token.span.join(self.current().span),
            ));
        }
        let open = self.advance().span;
        let payload = self.nested(open, Self::pattern)?;
        let end = self.expect(TokenKind::RightParen)?.span;
        Ok(Pattern {
            kind: PatternKind::Variant {
                variant,
                payload: Some(Box::new(payload)),
            },
            span: token.span.join(end),
        })
    }

    /// Parses `[]` or `[head, ..tail]`, whose head and tail are names or `_`.
    fn list_pattern(&mut self) -> Result<Pattern, Diagnostic> {
        let start = self.expect(TokenKind::LeftBracket)?.span;
        if let Some(end) = self.consume_span(&TokenKind::RightBracket) {
            return Ok(Pattern {
                kind: PatternKind::EmptyList,
                span: start.join(end),
            });
        }
        let shape = |parser: &Self| {
            Diagnostic::new(
                "K1003",
                "a list pattern is `[]` or `[head, ..tail]`",
                start.join(parser.current().span),
            )
        };
        let head = self.list_pattern_part()?;
        if !self.consume(&TokenKind::Comma) || !self.consume(&TokenKind::DotDot) {
            return Err(shape(self));
        }
        let tail = self.list_pattern_part()?;
        if let (PatternKind::Binder(head_name), PatternKind::Binder(tail_name)) =
            (&head.kind, &tail.kind)
            && head_name == tail_name
        {
            return Err(Diagnostic::new(
                "K2002",
                format!("duplicate match binding `{tail_name}`"),
                head.span.join(tail.span),
            ));
        }
        if !self.check(&TokenKind::RightBracket) {
            return Err(shape(self));
        }
        let end = self.advance().span;
        Ok(Pattern {
            kind: PatternKind::Cons {
                head: Box::new(head),
                tail: Box::new(tail),
            },
            span: start.join(end),
        })
    }

    /// Parses the head or tail of `[head, ..tail]`: a name or `_`.
    fn list_pattern_part(&mut self) -> Result<Pattern, Diagnostic> {
        let token = self.current().clone();
        match &token.kind {
            TokenKind::Identifier(name) if name == "_" => {
                self.advance();
                Ok(Pattern {
                    kind: PatternKind::Wildcard,
                    span: token.span,
                })
            }
            TokenKind::Identifier(name)
                if !matches!(name.as_str(), "Some" | "None" | "Ok" | "Err") =>
            {
                let (binding, span) = self.binding_name()?;
                Ok(Pattern {
                    kind: PatternKind::Binder(binding),
                    span,
                })
            }
            _ => Err(Diagnostic::new(
                "K1003",
                format!(
                    "the head and tail of a list pattern are names or `_`, found {}",
                    match &token.kind {
                        TokenKind::Identifier(name) => format!("`{name}`"),
                        kind => kind.description().to_owned(),
                    }
                ),
                token.span,
            )),
        }
    }

    /// Parses a variant pattern: `Some(name)`, `Ok(name)`, `Err(name)`, or
    /// `None`.
    fn variant_pattern(&mut self) -> Result<VariantPattern, Diagnostic> {
        let (variant, name_span) = self.variant_name()?;
        let binding = self.variant_binding(variant, name_span)?;
        Ok(VariantPattern {
            variant,
            binding,
            span: name_span.join(self.previous().span),
        })
    }

    /// Parses the variant name that starts a variant pattern.
    fn variant_name(&mut self) -> Result<(VariantName, Span), Diagnostic> {
        let token = self.advance().clone();
        let TokenKind::Identifier(name) = token.kind else {
            return Err(Diagnostic::new(
                "K1003",
                "expected `Some(name)`, `None`, `Ok(name)`, or `Err(name)`",
                token.span,
            ));
        };
        let variant = match name.as_str() {
            "Some" => VariantName::Some,
            "None" => VariantName::None,
            "Ok" => VariantName::Ok,
            "Err" => VariantName::Err,
            _ => {
                return Err(Diagnostic::new(
                    "K1003",
                    format!("unknown match variant `{name}`"),
                    token.span,
                ));
            }
        };
        Ok((variant, token.span))
    }

    /// Parses the `(name)` binder that follows `variant`, written at
    /// `name_span`; `None` binds nothing.
    fn variant_binding(
        &mut self,
        variant: VariantName,
        name_span: Span,
    ) -> Result<Option<String>, Diagnostic> {
        if variant == VariantName::None {
            if self.check(&TokenKind::LeftParen) {
                return Err(Diagnostic::new(
                    "K1003",
                    "`None` does not bind a value",
                    self.current().span,
                ));
            }
            return Ok(None);
        }
        if !self.consume(&TokenKind::LeftParen) {
            return Err(Diagnostic::new(
                "K1003",
                format!("`{}` must bind one value", variant.as_str()),
                name_span.join(self.current().span),
            ));
        }
        if self.at_variant_pattern() || self.check(&TokenKind::LeftBracket) {
            return Err(Diagnostic::new(
                "K3014",
                format!(
                    "let-else patterns do not nest: `{}(...)` binds its payload to a name or `_`",
                    variant.as_str()
                ),
                self.current().span,
            )
            .with_note(
                "bind the payload with this let-else, then decide it with another let-else or a \
                 `match`, whose patterns nest",
            ));
        }
        let (binding, _) = self.binding_name()?;
        self.expect(TokenKind::RightParen)?;
        Ok(Some(binding))
    }

    fn type_annotation(&mut self) -> Result<TypeAnnotation, Diagnostic> {
        let start = self.current().span;
        let token = self.advance().clone();
        let TokenKind::Identifier(name) = token.kind else {
            return Err(Diagnostic::new(
                "K1002",
                format!("expected built-in type, found {}", token.kind.description()),
                token.span,
            ));
        };
        let kind = match name.as_str() {
            "Int" => TypeKind::Int,
            "Bool" => TypeKind::Bool,
            "String" => TypeKind::String,
            "Unit" => TypeKind::Unit,
            "HttpHeader" => TypeKind::HttpHeader,
            "HttpRequest" => TypeKind::HttpRequest,
            "HttpResponse" => TypeKind::HttpResponse,
            "LogField" => TypeKind::LogField,
            "QueueJob" => TypeKind::QueueJob,
            "ScheduleEvent" => TypeKind::ScheduleEvent,
            "DatabaseTransaction" => TypeKind::DatabaseTransaction,
            "Secret" => TypeKind::Secret,
            "List" => {
                let open = self.expect(TokenKind::Less)?.span;
                let element = self.nested(open, Self::type_annotation)?;
                self.expect(TokenKind::Greater)?;
                TypeKind::List(Box::new(element))
            }
            "Option" => {
                let open = self.expect(TokenKind::Less)?.span;
                let element = self.nested(open, Self::type_annotation)?;
                self.expect(TokenKind::Greater)?;
                TypeKind::Option(Box::new(element))
            }
            "Result" => {
                let open = self.expect(TokenKind::Less)?.span;
                let (value, error) = self.nested(open, |parser| {
                    let value = parser.type_annotation()?;
                    parser.expect(TokenKind::Comma)?;
                    let error = parser.type_annotation()?;
                    Ok((value, error))
                })?;
                self.expect(TokenKind::Greater)?;
                TypeKind::Result(Box::new(value), Box::new(error))
            }
            "Record" => TypeKind::Record(self.record_type_fields()?),
            _ => {
                return Err(Diagnostic::new(
                    "K1002",
                    format!("unknown built-in type `{name}`"),
                    token.span,
                ));
            }
        };
        let end = self.previous().span;
        Ok(TypeAnnotation {
            kind,
            span: start.join(end),
        })
    }

    fn record_type_fields(&mut self) -> Result<Vec<RecordTypeField>, Diagnostic> {
        let open = self.expect(TokenKind::LeftBrace)?.span;
        self.nested(open, Self::record_type_field_list)
    }

    fn record_type_field_list(&mut self) -> Result<Vec<RecordTypeField>, Diagnostic> {
        let mut fields = Vec::new();
        let mut seen = HashSet::new();
        if !self.check(&TokenKind::RightBrace) {
            loop {
                let (name, name_span) = self.identifier()?;
                if !seen.insert(name.clone()) {
                    return Err(Diagnostic::new(
                        "K2002",
                        format!("duplicate record type field `{name}`"),
                        name_span,
                    ));
                }
                self.expect(TokenKind::Colon)?;
                let annotation = self.type_annotation()?;
                fields.push(RecordTypeField {
                    name,
                    span: name_span.join(annotation.span),
                    annotation,
                });
                if !self.consume(&TokenKind::Comma) {
                    break;
                }
                if self.check(&TokenKind::RightBrace) {
                    break;
                }
            }
        }
        self.expect(TokenKind::RightBrace)?;
        Ok(fields)
    }

    fn binding_name(&mut self) -> Result<(String, Span), Diagnostic> {
        let (name, span) = self.identifier()?;
        // `let mut x`, `fn f(mut x)`, and `Some(mut x)`; a binding named
        // `mut` is never followed by another name.
        if name == "mut" && matches!(self.current().kind, TokenKind::Identifier(_)) {
            return Err(Habit::Mutation.word_diagnostic(&name, span));
        }
        if matches!(
            name.as_str(),
            "print"
                | "println"
                | "ai_invoke"
                | "Some"
                | "None"
                | "Ok"
                | "Err"
                | "json_encode"
                | "json_decode"
                | "config_string"
                | "http_request"
                | "log_info"
                | "log_error"
                | "secret"
        ) {
            return Err(Diagnostic::new(
                "K2002",
                format!("`{name}` is a reserved built-in name"),
                span,
            ));
        }
        Ok((name, span))
    }

    fn identifier(&mut self) -> Result<(String, Span), Diagnostic> {
        let token = self.advance().clone();
        let TokenKind::Identifier(name) = token.kind else {
            return Err(Diagnostic::new(
                "K1002",
                format!("expected identifier, found {}", token.kind.description()),
                token.span,
            ));
        };
        Ok((name, token.span))
    }

    /// Parses one syntactic nesting level opened by the construct at `span`.
    fn nested<T>(
        &mut self,
        span: Span,
        parse: impl FnOnce(&mut Self) -> Result<T, Diagnostic>,
    ) -> Result<T, Diagnostic> {
        self.enter(span)?;
        let result = parse(self);
        self.depth -= 1;
        result
    }

    /// Parses with `levels` more syntactic nesting levels open, each opened
    /// by the construct at `span`.
    fn nested_levels<T>(
        &mut self,
        levels: usize,
        span: Span,
        parse: impl FnOnce(&mut Self) -> Result<T, Diagnostic>,
    ) -> Result<T, Diagnostic> {
        let mut opened = 0;
        let mut entered = Ok(());
        while opened < levels && entered.is_ok() {
            opened += 1;
            entered = self.enter(span);
        }
        let result = entered.and_then(|()| parse(self));
        self.depth -= opened;
        result
    }

    fn enter(&mut self, span: Span) -> Result<(), Diagnostic> {
        self.depth += 1;
        if self.depth > MAX_NESTING_DEPTH {
            return Err(nesting_error(span));
        }
        self.reach(self.structural_depth(), span)
    }

    /// Records one `else if` continuation.
    fn enter_chain_link(&mut self, span: Span) -> Result<(), Diagnostic> {
        self.chain_depth += 1;
        self.reach(self.structural_depth(), span)
    }

    const fn structural_depth(&self) -> usize {
        self.depth + self.chain_depth
    }

    /// Records that the tree reaches structural depth `depth` because of the
    /// construct at `span`.
    fn reach(&mut self, depth: usize, span: Span) -> Result<(), Diagnostic> {
        if depth > MAX_STRUCTURAL_DEPTH {
            return Err(structural_error(span));
        }
        self.peak = self.peak.max(depth);
        Ok(())
    }

    fn check(&self, kind: &TokenKind) -> bool {
        self.current().kind == *kind
    }

    /// Consumes the current token when it is `kind` and returns its span.
    fn consume_span(&mut self, kind: &TokenKind) -> Option<Span> {
        self.check(kind).then(|| self.advance().span)
    }

    fn consume(&mut self, kind: &TokenKind) -> bool {
        if self.check(kind) {
            self.advance();
            true
        } else {
            false
        }
    }

    fn expect(&mut self, kind: TokenKind) -> Result<Token, Diagnostic> {
        if self.check(&kind) {
            Ok(self.advance().clone())
        } else {
            Err(self.expected(kind.description()))
        }
    }

    fn expected(&self, expected: &str) -> Diagnostic {
        Diagnostic::new(
            "K1002",
            format!(
                "expected {expected}, found {}",
                self.current().kind.description()
            ),
            self.current().span,
        )
    }

    fn current(&self) -> &Token {
        &self.tokens[self.cursor]
    }

    fn previous(&self) -> &Token {
        &self.tokens[self.cursor - 1]
    }

    fn advance(&mut self) -> &Token {
        let token = &self.tokens[self.cursor];
        if !matches!(token.kind, TokenKind::Eof) {
            self.cursor += 1;
        }
        token
    }
}

/// An arrow function at `span` (`K1109`).
fn arrow_function(span: Span) -> Diagnostic {
    Habit::ArrowFunction.diagnostic("arrow functions are not Krit syntax", span)
}

/// A conditional operator at `span`, from its `?` to its `:` (`K1110`).
fn ternary(span: Span) -> Diagnostic {
    Habit::Ternary.diagnostic("the conditional operator `? :` is not Krit syntax", span)
}

/// Finds, for every `?` token, the `:` that would end a conditional
/// operator's first branch: the first `:` at the `?`'s bracket depth, looking
/// no further than the end of the file, a closing bracket opened before the
/// `?`, or the first `;`, `,`, or `let` at that depth, none of which a first
/// branch contains outside brackets.
///
/// Each `?` that has such a `:` maps to the `?` to report with it and to the
/// `:`. The `?` to report is the last one before the `:` at the same depth
/// that a token able to start an expression follows: the innermost
/// conditional operator ending at that `:` (`a ? -b ? c : d : e` reports
/// `? c :`). One pass over the tokens finds every `?`'s `:`.
fn ternary_colons(tokens: &[Token]) -> BTreeMap<usize, (Span, Span)> {
    let mut colons = BTreeMap::new();
    // The `?` tokens still looking for their `:`, one list per open bracket
    // level, outermost first. The outermost level is never closed.
    let mut levels: Vec<Vec<usize>> = vec![Vec::new()];
    for (index, token) in tokens.iter().enumerate() {
        let open = levels.len();
        let level = levels
            .last_mut()
            .expect("the outermost bracket level stays open");
        match &token.kind {
            TokenKind::Question => level.push(index),
            TokenKind::LeftParen | TokenKind::LeftBracket | TokenKind::LeftBrace => {
                levels.push(Vec::new());
            }
            TokenKind::RightParen | TokenKind::RightBracket | TokenKind::RightBrace => {
                // A bracket that closes the level of a `?` ends its search, and
                // so does one that closes nothing opened in the file.
                if open > 1 {
                    levels.pop();
                } else {
                    level.clear();
                }
            }
            TokenKind::Colon => {
                let pending = std::mem::take(level);
                let Some(&innermost) = pending.iter().rev().find(|question| {
                    tokens
                        .get(**question + 1)
                        .is_some_and(|next| starts_expression(&next.kind))
                }) else {
                    continue;
                };
                for question in pending {
                    colons.insert(question, (tokens[innermost].span, token.span));
                }
            }
            TokenKind::Semicolon | TokenKind::Comma | TokenKind::Let => level.clear(),
            TokenKind::Eof => break,
            _ => {}
        }
    }
    colons
}

/// Whether `error` already explains the construct it reports, so an
/// enclosing construct must pass it on rather than reinterpret it: a nesting
/// limit, or a construct from another language.
fn is_settled(error: &Diagnostic) -> bool {
    error.code() == "K1005" || error.code().starts_with("K11")
}

/// Whether a token of `kind` can start an operand but never continue an
/// expression.
fn starts_operand_only(kind: &TokenKind) -> bool {
    matches!(
        kind,
        TokenKind::Identifier(_)
            | TokenKind::Integer(_)
            | TokenKind::String(_)
            | TokenKind::True
            | TokenKind::False
            | TokenKind::Record
            | TokenKind::If
            | TokenKind::Match
            | TokenKind::Fn
            | TokenKind::Bang
    )
}

/// Whether a token of `kind` can start an expression.
fn starts_expression(kind: &TokenKind) -> bool {
    starts_operand_only(kind)
        || matches!(
            kind,
            TokenKind::LeftParen | TokenKind::LeftBracket | TokenKind::LeftBrace | TokenKind::Minus
        )
}

fn binary(left: Expression, operator: BinaryOperator, right: Expression) -> Expression {
    let span = left.span.join(right.span);
    Expression {
        kind: ExpressionKind::Binary {
            left: Box::new(left),
            operator,
            right: Box::new(right),
        },
        span,
    }
}

#[cfg(test)]
mod tests {
    use crate::{Source, lexer::lex};

    use super::*;

    fn parse_text(text: &str) -> Result<Program, Diagnostic> {
        let source = Source::new("test.krit", text);
        parse(&source, lex(&source)?)
    }

    #[test]
    fn parses_declarations_functions_lists_and_match() {
        let program = parse_text(
            r#"
            fn sum(items) {
                match items {
                    [] => 0,
                    [head, ..tail] => head + sum(tail),
                }
            }
            println(sum([1, 2, 3]));
            "#,
        )
        .expect("program should parse");

        assert_eq!(program.statements.len(), 2);
        assert!(matches!(
            program.statements[0].kind,
            StatementKind::Function { .. }
        ));
    }

    #[test]
    fn requires_statement_semicolons() {
        let error =
            parse_text("let answer = 42\nprintln(answer);").expect_err("program should not parse");
        assert_eq!(error.code(), "K1002");
    }

    #[test]
    fn rejects_non_exhaustive_match_shapes() {
        let error = parse_text("match items { [value] => value, [] => 0 };")
            .expect_err("invalid patterns should fail");
        assert_eq!(error.code(), "K1003");
    }

    #[test]
    fn parses_records_variants_and_annotations() {
        let program = parse_text(
            r#"
            let item: Record { name: String, values: List<Int> } =
                record { name: "agent", values: [1, 2] };
            fn unwrap(value: Option<String>) -> String {
                match value {
                    Some(name) => name,
                    None => "missing",
                }
            }
            item.name;
            unwrap(Some("ready"));
            "#,
        )
        .expect("data syntax should parse");

        let StatementKind::Let {
            annotation: Some(annotation),
            ..
        } = &program.statements[0].kind
        else {
            panic!("let annotation should be stored");
        };
        assert!(matches!(annotation.kind, TypeKind::Record(_)));
        let StatementKind::Function {
            parameters,
            return_type: Some(_),
            ..
        } = &program.statements[1].kind
        else {
            panic!("function annotations should be stored");
        };
        assert!(parameters[0].annotation.is_some());
    }

    #[test]
    fn rejects_duplicate_record_fields() {
        let duplicate_field =
            parse_text("record { value: 1, value: 2 };").expect_err("duplicate field should fail");
        assert_eq!(duplicate_field.code(), "K2002");

        let duplicate_type_field =
            parse_text("let value: Record { item: Int, item: Bool } = record {};")
                .expect_err("duplicate type field should fail");
        assert_eq!(duplicate_type_field.code(), "K2002");
    }

    /// Parses on the stack the CLI gives the compiler pipeline; unoptimized
    /// builds need more than a default test thread for 256 levels.
    fn parse_on_compiler_stack(text: String) -> Result<Program, Diagnostic> {
        crate::run_on_thread("parser-test", crate::COMPILER_STACK_BYTES, move || {
            parse_text(&text)
        })
        .expect("parser test thread should start")
    }

    /// Generates `depth` levels of one nesting construct.
    fn nested_source(construct: &str, depth: usize) -> String {
        let (open, inner, close, prefix) = match construct {
            "parens" => ("(", "1", ")", ""),
            "blocks" => ("{ ", "1", " }", ""),
            "lists" => ("[", "1", "]", ""),
            "records" => ("record { a: ", "1", " }", ""),
            "ifs" => ("if true { ", "1", " } else { 0 }", ""),
            "matches" => ("match Some(1) { None => 0, Some(x) => ", "1", " }", ""),
            "calls" => ("f(", "1", ")", "fn f(x) { x }\n"),
            "unary" => ("-", "1", "", ""),
            "functions" => ("fn() { ", "1", " }", ""),
            "fields" => ("", "r", ".a", "let r = record { a: 1 };\n"),
            "types" => {
                return format!(
                    "let value: {}Int{} = [];\n",
                    "List<".repeat(depth),
                    ">".repeat(depth)
                );
            }
            _ => unreachable!("unknown construct {construct}"),
        };
        format!(
            "{prefix}let value = {}{inner}{};\n",
            open.repeat(depth),
            close.repeat(depth)
        )
    }

    #[test]
    fn rejects_two_thousand_levels_of_every_nesting_construct() {
        for construct in [
            "parens",
            "blocks",
            "lists",
            "records",
            "ifs",
            "matches",
            "calls",
            "unary",
            "functions",
            "fields",
            "types",
        ] {
            let error = parse_on_compiler_stack(nested_source(construct, 2_000))
                .expect_err("deep nesting should be rejected");
            assert_eq!(error.code(), "K1005", "{construct}");
            assert!(
                error.message().contains("256 levels"),
                "{construct}: {}",
                error.message()
            );
        }
    }

    #[test]
    fn accepts_exactly_the_nesting_limit() {
        for construct in ["parens", "blocks", "lists", "types"] {
            parse_on_compiler_stack(nested_source(construct, MAX_NESTING_DEPTH))
                .unwrap_or_else(|error| panic!("{construct} at the limit should parse: {error:?}"));
            let error = parse_on_compiler_stack(nested_source(construct, MAX_NESTING_DEPTH + 1))
                .expect_err("one level past the limit should fail");
            assert_eq!(error.code(), "K1005", "{construct}");
        }
    }

    #[test]
    fn reports_nesting_at_the_construct_that_crosses_the_limit() {
        // `let value = ` is 12 bytes; the 257th `(` opens the 257th level.
        let error = parse_on_compiler_stack(nested_source("parens", 300))
            .expect_err("deep parentheses should fail");
        assert_eq!(error.span(), Span::new(12 + 256, 12 + 257));

        let error = parse_on_compiler_stack(nested_source("types", 300))
            .expect_err("deep types should fail");
        let generic_open = "let value: ".len() + 256 * "List<".len() + "List".len();
        assert_eq!(error.span(), Span::new(generic_open, generic_open + 1));
    }

    fn else_if_chain(branches: usize) -> String {
        let mut chain = String::from("if n == 0 { 0 }");
        for branch in 1..branches {
            chain.push_str(&format!(" else if n == {branch} {{ {branch} }}"));
        }
        chain.push_str(" else { -1 }");
        chain
    }

    #[test]
    fn else_if_continuations_do_not_add_nesting_levels() {
        // A 1,000-branch chain inside 250 blocks stays within 256 levels.
        let text = format!(
            "let n = 3;\nlet value = {}{}{};\n",
            "{ ".repeat(250),
            else_if_chain(1_000),
            " }".repeat(250)
        );
        let program = parse_on_compiler_stack(text).expect("long else-if chain should parse");
        let StatementKind::Let { value, .. } = &program.statements[1].kind else {
            panic!("second statement should be a let");
        };
        let mut current = value;
        for _ in 0..250 {
            let ExpressionKind::Block(block) = &current.kind else {
                panic!("expected a block");
            };
            current = block.tail.as_deref().expect("block should have a tail");
        }
        let mut links = 0;
        while let ExpressionKind::If { alternative, .. } = &current.kind {
            links += 1;
            current = alternative;
        }
        assert_eq!(links, 1_000);
        assert!(matches!(current.kind, ExpressionKind::Block(_)));
    }

    #[test]
    fn bounds_flat_chains_by_the_structural_depth() {
        let chain = |branches| format!("let n = 1;\nlet value = {};\n", else_if_chain(branches));
        parse_on_compiler_stack(chain(MAX_STRUCTURAL_DEPTH - 8))
            .expect("a chain just under the structural limit should parse");
        let error = parse_on_compiler_stack(chain(MAX_STRUCTURAL_DEPTH + 8))
            .expect_err("a chain past the structural limit should fail");
        assert_eq!(error.code(), "K1005");
        assert!(error.message().contains("4096"), "{}", error.message());

        let sum = |terms: usize| format!("let value = 1{};\n", " + 1".repeat(terms));
        parse_on_compiler_stack(sum(MAX_STRUCTURAL_DEPTH - 8))
            .expect("a long operator chain should parse");
        let error = parse_on_compiler_stack(sum(MAX_STRUCTURAL_DEPTH + 8))
            .expect_err("an operator chain past the structural limit should fail");
        assert_eq!(error.code(), "K1005");
    }

    /// Counts the operator, unary, call, and field-access nodes on the
    /// leftmost path down from `expression`; parentheses create no node.
    fn spine_height(expression: &Expression) -> usize {
        let mut height = 0;
        let mut current = expression;
        loop {
            current = match &current.kind {
                ExpressionKind::Binary { left, .. } => left,
                ExpressionKind::Unary { operand, .. } => operand,
                ExpressionKind::Call { callee, .. } => callee,
                ExpressionKind::FieldAccess { value, .. } => value,
                _ => return height,
            };
            height += 1;
        }
    }

    #[test]
    fn counts_every_operator_above_a_parenthesized_left_operand() {
        // Each parenthesized chain becomes the left operand of the next, so the
        // innermost `1` sits below every operator of every level.
        let spine = |levels: usize, operators: usize| {
            let mut expression = format!("1{}", " + 1".repeat(operators));
            for _ in 1..levels {
                expression = format!("({expression}){}", " + 1".repeat(operators));
            }
            format!("let value = {expression};\n")
        };
        let program =
            parse_on_compiler_stack(spine(2, 2_000)).expect("a 4,000-operator tree should parse");
        let StatementKind::Let { value, .. } = &program.statements[0].kind else {
            panic!("expected a let");
        };
        assert_eq!(spine_height(value), 4_000);

        let error = parse_on_compiler_stack(spine(3, 2_000))
            .expect_err("a 6,000-operator tree should fail");
        assert_eq!(error.code(), "K1005");
        assert!(error.message().contains("4096"), "{}", error.message());
        // The error points at the operator that takes the tree past the limit:
        // the third level's operators sit above the 4,002 levels of its left
        // operand (4,000 operators and two pairs of parentheses), so its 95th
        // operator crosses 4,096.
        let text = spine(3, 2_000);
        let third_level = text.rfind(") + 1").expect("third level should exist") + 1;
        assert_eq!(
            error.span(),
            Span::new(third_level + 1 + 94 * 4, third_level + 2 + 94 * 4)
        );

        // The reviewer's shape: 250 parenthesized levels of 3,800 operators.
        let error = parse_on_compiler_stack(spine(250, 3_800))
            .expect_err("a million-node operator spine should fail");
        assert_eq!(error.code(), "K1005");
    }

    #[test]
    fn counts_operators_above_a_deep_unary_operand() {
        // 250 nested negations stay within 256 levels, but every one of the
        // 3,900 additions sits above them.
        let text = format!(
            "let value = {}1{};\n",
            "-".repeat(250),
            " + 1".repeat(3_900)
        );
        let error = parse_on_compiler_stack(text).expect_err("4,150 levels should fail");
        assert_eq!(error.code(), "K1005");
        assert!(error.message().contains("4096"), "{}", error.message());

        let text = format!(
            "let value = {}1{};\n",
            "-".repeat(250),
            " + 1".repeat(3_800)
        );
        parse_on_compiler_stack(text).expect("4,050 levels should parse");
    }

    #[test]
    fn counts_postfix_links_above_a_parenthesized_callee() {
        // Parentheses reset nothing: each level's field accesses sit above the
        // parenthesized chain they apply to.
        let chain = |levels: usize, links: usize| {
            let mut expression = String::from("r");
            for _ in 0..levels {
                expression = format!("({expression}){}", ".a".repeat(links));
            }
            format!("let value = {expression};\n")
        };
        let program = parse_on_compiler_stack(chain(40, 50)).expect("2,000 links should parse");
        let StatementKind::Let { value, .. } = &program.statements[0].kind else {
            panic!("expected a let");
        };
        assert_eq!(spine_height(value), 2_000);

        let error = parse_on_compiler_stack(chain(200, 50)).expect_err("10,000 links should fail");
        assert_eq!(error.code(), "K1005");
        assert!(error.message().contains("4096"), "{}", error.message());

        // Calls nest their callee the same way.
        let calls = format!(
            "let value = {}f{};\n",
            "(".repeat(200),
            ")(1)(2)(3)(4)(5)(6)(7)(8)(9)(10)(11)(12)(13)(14)(15)(16)(17)(18)(19)(20)".repeat(200)
        );
        let error = parse_on_compiler_stack(calls).expect_err("4,000 calls should fail");
        assert_eq!(error.code(), "K1005");
    }

    /// Parses `text` and returns the error's code, the source text its span
    /// covers, and its notes.
    fn habit(text: &str) -> (&'static str, String, Vec<String>) {
        let error = parse_text(text).expect_err("foreign construct should not parse");
        let span = error.span();
        (
            error.code(),
            text[span.start..span.end].to_owned(),
            error.notes().to_vec(),
        )
    }

    #[test]
    fn reports_each_foreign_construct_with_its_code_span_and_hint() {
        for (text, code, spanned) in [
            ("fn f(x) { return x; }", "K1101", "return"),
            ("fn f(x) { return x }", "K1101", "return"),
            (
                "fn f(x) { if x { return 1; } else { 2 } }",
                "K1101",
                "return",
            ),
            (
                "match Some(1) { Some(v) => return v, None => 0 };",
                "K1101",
                "return",
            ),
            ("fn f() { return [1]; }", "K1101", "return"),
            ("fn f(xs) { for x in xs { x; } }", "K1102", "for"),
            (
                "fn f(n) { for (let i = 0; i < n; i++) { i; } }",
                "K1102",
                "for",
            ),
            ("fn f(x) { while x > 0 { x; } }", "K1102", "while"),
            ("fn f(x) { while (x > 0) { x; } }", "K1102", "while"),
            ("fn f() { loop { 1; } }", "K1102", "loop"),
            ("fn f() { do { 1; } }", "K1102", "do"),
            ("let x = 1;\nx = 2;", "K1103", "x ="),
            ("fn f(r) { r.count = 2; r }", "K1103", "r.count ="),
            ("fn f() { let x = 1; x = 2 }", "K1103", "x ="),
            ("let a = 1;\nlet b = a = 2;", "K1103", "a ="),
            ("let mut x = 1;", "K1103", "mut"),
            ("fn f(mut x) { x }", "K1103", "mut"),
            (
                "match Some(1) { Some(mut x) => x, None => 0 };",
                "K1103",
                "mut",
            ),
            ("var x = 1;", "K1103", "var"),
            ("const LIMIT = 3;", "K1103", "const"),
            ("fn f(x) { x += 1; x }", "K1103", "+="),
            ("fn f(x) { x -= 1; x }", "K1103", "-="),
            ("fn f(x) { x *= 2; x }", "K1103", "*="),
            ("fn f(x) { x /= 2; x }", "K1103", "/="),
            ("fn f(x) { x %= 2; x }", "K1103", "%="),
            ("fn f(x) { x++; x }", "K1103", "++"),
            ("fn f(x) { x--; x }", "K1103", "--"),
            ("fn f(x) { x-- }", "K1103", "--"),
            ("struct Point { x: Int }", "K1104", "struct"),
            ("class Point {}", "K1104", "class"),
            ("impl Point {}", "K1104", "impl"),
            ("trait Shape {}", "K1104", "trait"),
            ("interface Shape {}", "K1104", "interface"),
            ("import json;", "K1105", "import"),
            ("import { encode } from \"json\";", "K1105", "import"),
            ("use std::io;", "K1105", "use"),
            ("let json = require \"json\";", "K1105", "require"),
            ("from json import encode", "K1105", "from"),
            ("async fn f() { 1 }", "K1106", "async"),
            ("let f = async (x) => x;", "K1106", "async"),
            ("fn f(g) { await g() }", "K1106", "await"),
            ("fn f() { try { 1 } catch e { 2 } }", "K1107", "try"),
            ("fn f() { try: 1 }", "K1107", "try"),
            ("fn f() { throw \"boom\"; }", "K1107", "throw"),
            ("fn f() { raise \"boom\" }", "K1107", "raise"),
            ("let xs = [1];\nxs[0];", "K1108", "xs["),
            ("fn f(g) { g()[0] }", "K1108", "g()["),
            ("[1, 2][0];", "K1108", "[1, 2]["),
            ("let f = x => x + 1;", "K1109", "x =>"),
            ("let f = (x) => x + 1;", "K1109", "(x) =>"),
            ("let f = (x, y) => x + y;", "K1109", "(x, y) =>"),
            ("let f = (x: Int) => x;", "K1109", "(x: Int) =>"),
            ("let f = () => 1;", "K1109", "() =>"),
            ("let f = (...rest) => 1;", "K1109", "(...rest) =>"),
            ("let f = ((x, y) => x);", "K1109", "(x, y) =>"),
            ("let y = true ? 1 : 2;", "K1110", "? 1 :"),
            (
                "let x = 1;\nlet y = x > 0 ? f(x, 1) : 2;",
                "K1110",
                "? f(x, 1) :",
            ),
            ("let y = a ? b ? c : d : e;", "K1110", "? c :"),
            ("let y = a ? -1 : 1;", "K1110", "? -1 :"),
            ("fn f(x) { switch x { case 1: 2 } }", "K1111", "switch"),
            ("fn f(x) { switch (x) { case 1: 2 } }", "K1111", "switch"),
            ("fn f(x) { case 1: 2 }", "K1111", "case"),
            ("fn f() { default: 1 }", "K1111", "default"),
            (
                "fn f(x) { if x { 1 } else { return 0; } }",
                "K1112",
                "else { return",
            ),
            (
                "fn f(x) { if x { 1 } else return 0; }",
                "K1112",
                "else return",
            ),
        ] {
            let (actual, actual_span, notes) = habit(text);
            assert_eq!(actual, code, "{text}");
            assert_eq!(actual_span, spanned, "{text}");
            assert_eq!(notes.len(), 1, "{text}");
        }
    }

    #[test]
    fn habit_words_stay_ordinary_names_outside_foreign_constructs() {
        for text in [
            "let for = 1;\nfor;",
            "let while = [1];\nmatch while { [] => 0, [x, .._] => x };",
            "let return = 1;\nreturn - 1;",
            "let return = 1;\nlet x = return;",
            "fn use(x) { x }\nuse(1);",
            "let async = fn(x) { x };\nasync(1);",
            "let try = fn(x) { x };\ntry(1);",
            "let r = record { from: 1, case: 2, default: 3 };\nr.from + r.case;",
            "let loop = true;\nif loop { 1 } else { 2 };",
            "let default = Some(1);\nmatch default { Some(x) => x, None => 0 };",
            "let mut = 1;\nfn f(mut) { mut }",
            "let xs = [1];\n[xs, [1]];",
            "let class = 1;\n[class];",
            "fn f(x) { if x { 1 } else { return } }",
            "fn f(x) { if x { 1 } else { return; } }",
            "let await = record { value: 1 };\nawait.value;",
            "let x = 1;\nlet y = -x - -x;",
            "let x = 1;\nlet y = x - -x;",
        ] {
            parse_text(text).unwrap_or_else(|error| panic!("{text:?} should parse: {error:?}"));
        }
    }

    #[test]
    fn a_question_mark_before_an_operand_is_an_unexpected_token() {
        for (text, start) in [
            ("fn f(x) { x ? 1 }", 12),
            ("fn f(x) { x ?? 1 }", 13),
            ("fn f(x) { x? y }", 11),
        ] {
            let error = parse_text(text).expect_err("`?` before an operand should not parse");
            assert_eq!(error.code(), "K1001", "{text}");
            assert_eq!(error.span(), Span::new(start, start + 1), "{text}");
            assert_eq!(error.notes().len(), 1, "{text}");
        }
        // A `?` that starts an operand is an ordinary unexpected token.
        let error = parse_text("let y = ?;").expect_err("should not parse");
        assert_eq!(error.code(), "K1001");
        assert!(error.notes().is_empty());
        // The first `?` is a postfix `?` before a parenthesized call
        // argument; the second is a conditional operator.
        let error = parse_text("let y = a ? (b ? c : d);").expect_err("should not parse");
        assert_eq!(error.code(), "K1110");
        // An operand on a later line follows a postfix `?` whose `;` is
        // missing.
        let error = parse_text("fn f(x) -> Option<Int> {\n    let y = x?\n    Some(y)\n}")
            .expect_err("should not parse");
        assert_eq!(error.code(), "K1002");
        assert!(error.notes().is_empty());
    }

    /// The operand of the outermost postfix `?` in `expression`.
    fn propagated(expression: &Expression) -> &Expression {
        let ExpressionKind::Propagate { operand, .. } = &expression.kind else {
            panic!("expected a postfix `?`, found {:?}", expression.kind);
        };
        operand
    }

    /// The value of the single `let` statement in `text`.
    fn let_value(text: &str) -> Expression {
        let program = parse_text(text).unwrap_or_else(|error| panic!("{text}: {error:?}"));
        let [statement] = program.statements.as_slice() else {
            panic!("{text} should hold one statement");
        };
        let StatementKind::Let { value, .. } = &statement.kind else {
            panic!("{text} should be a `let`");
        };
        value.clone()
    }

    #[test]
    fn parses_postfix_question_marks_as_links_of_a_call_chain() {
        let text = "let v = load(1)?;";
        let value = let_value(text);
        assert_eq!(&text[value.span.start..value.span.end], "load(1)?");
        let ExpressionKind::Propagate { question, .. } = &value.kind else {
            panic!("expected a postfix `?`");
        };
        assert_eq!(&text[question.start..question.end], "?");
        assert!(matches!(
            propagated(&value).kind,
            ExpressionKind::Call { .. }
        ));

        // `?` binds like a call or field access: before `.`, `(`, and
        // another `?`, and tighter than unary and binary operators.
        let value = let_value("let v = a.b?.c;");
        let ExpressionKind::FieldAccess {
            value: inner,
            field,
        } = &value.kind
        else {
            panic!("expected a field access");
        };
        assert_eq!(field, "c");
        assert!(matches!(
            propagated(inner).kind,
            ExpressionKind::FieldAccess { .. }
        ));
        let value = let_value("let v = pick()?(1);");
        let ExpressionKind::Call { callee, .. } = &value.kind else {
            panic!("expected a call");
        };
        assert!(matches!(
            propagated(callee).kind,
            ExpressionKind::Call { .. }
        ));
        let value = let_value("let v = nested??;");
        assert!(matches!(
            propagated(propagated(&value)).kind,
            ExpressionKind::Variable(_)
        ));
        let value = let_value("let v = -count?;");
        let ExpressionKind::Unary { operand, .. } = &value.kind else {
            panic!("expected a unary operator");
        };
        propagated(operand);
        let value = let_value("let v = count? - 1;");
        let ExpressionKind::Binary { left, .. } = &value.kind else {
            panic!("expected a binary operator");
        };
        propagated(left);

        // Parenthesizing keeps the `?` node and its `?` span.
        let text = "let v = (count?);";
        let value = let_value(text);
        assert_eq!(&text[value.span.start..value.span.end], "(count?)");
        let ExpressionKind::Propagate { question, .. } = &value.kind else {
            panic!("expected a postfix `?`");
        };
        assert_eq!(question.start, 14);

        for text in [
            "let v = if ready? { 1 } else { 2 };",
            "let v = match found? { Some(x) => x, None => 0 };",
            "let v = [a?, b?];",
            "let v = record { a: a?, b: b? };",
            "let v = f(a?, g(b?)?);",
            "let v = a? == b?;",
            "let v = !ok?;",
        ] {
            parse_text(text).unwrap_or_else(|error| panic!("{text} should parse: {error:?}"));
        }
    }

    #[test]
    fn postfix_question_marks_count_toward_the_nesting_limits() {
        // Each `?` link opens one level for the rest of its chain.
        let text = format!("let v = {}x{};\n", "(".repeat(255), "?)".repeat(255));
        parse_on_compiler_stack(text).expect("255 parenthesized links should parse");
        let text = format!("let v = {}x{};\n", "(".repeat(256), "?)".repeat(256));
        let error = parse_on_compiler_stack(text).expect_err("too deep");
        assert_eq!(error.code(), "K1005");
        let text = format!("let v = x{};\n", "?".repeat(MAX_STRUCTURAL_DEPTH + 1));
        let error = parse_on_compiler_stack(text).expect_err("too long a chain");
        assert_eq!(error.code(), "K1005");
    }

    #[test]
    fn postfix_question_marks_parse_without_reparsing_what_follows() {
        // A `?` before `(`, `-`, or `{` is decided by the precomputed bracket
        // scan, so neither a nested call chain nor a long operator chain is
        // parsed more than once (a speculative parse of each first branch
        // would take exponential and quadratic time here).
        // Each `g()?(` opens three levels.
        let calls = format!(
            "fn f(g) -> Option<Int> {{ {}1{} }}\n",
            "g()?(".repeat(80),
            ")".repeat(80)
        );
        parse_on_compiler_stack(calls).expect("nested calls after `?` should parse");
        let terms = vec!["a?"; 4_000].join(" - ");
        parse_on_compiler_stack(format!("fn f(a) -> Option<Int> {{ Some({terms}) }}\n"))
            .expect("a long operator chain of `?` operands should parse");
        let branches = "if a? { 1 } else ".repeat(1_000);
        parse_on_compiler_stack(format!(
            "fn f(a) -> Option<Int> {{ Some({branches}{{ 0 }}) }}\n"
        ))
        .expect("a long `else if` chain of `?` conditions should parse");
    }

    #[test]
    fn spaced_operators_and_brackets_keep_their_generic_errors() {
        for (text, code) in [
            ("fn f(x) { x + = 1; x }", "K1001"),
            ("let xs = [1];\nlet y = xs [0];", "K1002"),
            ("fn f(x) { x - - }", "K1001"),
        ] {
            let error = parse_text(text).expect_err("should not parse");
            assert_eq!(error.code(), code, "{text}");
            assert!(error.notes().is_empty(), "{text}");
        }
    }

    #[test]
    fn ternary_speculation_is_bounded_by_the_nesting_limit() {
        // Each `?` examines its first branch one level deeper, so a long run of
        // unterminated conditionals stops at the nesting limit, which is
        // reported as such.
        let text = format!("let y = {}1;\n", "a ? ".repeat(2_000));
        let error = parse_on_compiler_stack(text).expect_err("should not parse");
        assert_eq!(error.code(), "K1005");
        let text = format!("let y = {}1{};\n", "a ? ".repeat(200), " : b".repeat(200));
        let error = parse_on_compiler_stack(text).expect_err("should not parse");
        assert_eq!(error.code(), "K1110");
        // A first branch that crosses the nesting limit reports the limit, as
        // the same branch does without the `?`.
        let nested = format!("{}1{}", "(".repeat(300), ")".repeat(300));
        let error =
            parse_on_compiler_stack(format!("let y = x ? f{nested} : 2;\n")).expect_err("deep");
        assert_eq!(error.code(), "K1005");
        let error = parse_on_compiler_stack(format!("let y = {nested};\n")).expect_err("deep");
        assert_eq!(error.code(), "K1005");
        // A first branch that starts with `(` may continue a postfix `?`, so
        // only the bracket scan examines it and nothing in it is parsed.
        let error =
            parse_on_compiler_stack(format!("let y = x ? {nested} : 2;\n")).expect_err("ternary");
        assert_eq!(error.code(), "K1110");
    }

    #[test]
    fn a_conditional_operator_after_a_postfix_question_mark_is_reported() {
        for (text, spanned) in [
            ("let y = a ? -b ? c : d : e;", "? c :"),
            ("let y = x? - c ? 1 : 2;", "? 1 :"),
            ("let y = a ? (b) ? c : d : e;", "? c :"),
            ("let y = a ? [1] : [2];", "? [1] :"),
            ("fn f(x) { x? ? 1 : 2 }", "? 1 :"),
        ] {
            let (code, actual_span, notes) = habit(text);
            assert_eq!(code, "K1110", "{text}");
            assert_eq!(actual_span, spanned, "{text}");
            assert_eq!(notes.len(), 1, "{text}");
        }
    }

    #[test]
    fn a_conditional_operator_whose_first_branch_does_not_parse_is_still_reported() {
        for (text, spanned) in [
            ("fn f(xs) { len(xs) > 0 ? xs[0] : None }", "? xs[0] :"),
            (
                "fn f(ok) { ok ? { status: 200 } : { status: 500 } }",
                "? { status: 200 } :",
            ),
            ("fn f(x) { x > 0 ? return 1 : 2 }", "? return 1 :"),
            ("fn f(c, x) { c ? x++ : x }", "? x++ :"),
            ("let y = c ? xs[0] : 0;", "? xs[0] :"),
            ("let y = c ? f(xs[0], 1) : 0;", "? f(xs[0], 1) :"),
            // The outer `?` comes first in source order.
            ("let y = a ? (b ? c[0] : d) : e;", "? (b ? c[0] : d) :"),
        ] {
            let (code, actual_span, notes) = habit(text);
            assert_eq!(code, "K1110", "{text}");
            assert_eq!(actual_span, spanned, "{text}");
            assert_eq!(notes.len(), 1, "{text}");
        }
        // Without a `:` at the `?`'s bracket depth before the first branch
        // could end, the `?` is an unexpected token.
        for text in [
            "let y = c ? xs[0];",
            "let y = c ? xs[0], 1;",
            "let y = c ? xs[0]\nlet z: Int = 1;",
            "fn f(c, xs) { c ? g(xs[0]) }\nlet z: Int = 1;",
            "let y = [c ? xs[0]]\nlet z: Int = 1;",
        ] {
            let error = parse_text(text).expect_err("should not parse");
            assert_eq!(error.code(), "K1001", "{text}");
            assert_eq!(&text[error.span().start..error.span().end], "?", "{text}");
            assert_eq!(error.notes().len(), 1, "{text}");
        }
    }

    #[test]
    fn a_habit_word_before_a_line_break_keeps_the_generic_error() {
        // A bound habit word ending a line, followed by an operand on the
        // next, is a missing `;` or `,`, not a foreign construct.
        for (text, code) in [
            ("let from = 1;\nlet x = from\nprintln(x);", "K1002"),
            ("let default = 1;\nlet y = default\nfn g() { 1 }", "K1002"),
            ("let case = 1;\nlet y = [case\ncase];", "K1002"),
            (
                "let from = 1;\nlet x = from // the source\n\"json\";",
                "K1002",
            ),
        ] {
            let error = parse_text(text).expect_err("should not parse");
            assert_eq!(error.code(), code, "{text}");
            assert!(error.notes().is_empty(), "{text}");
        }
        // On one line, the same words still start foreign constructs.
        for (text, code) in [
            ("let x = from json;", "K1105"),
            ("let y = [case 1];", "K1111"),
            ("fn f(x) {\n    return x\n}", "K1101"),
            ("fn f(x) { if x { 1 } else {\n    return 0\n} }", "K1112"),
        ] {
            let error = parse_text(text).expect_err("should not parse");
            assert_eq!(error.code(), code, "{text}");
        }
    }

    fn arm_patterns(text: &str) -> Vec<String> {
        let program = parse_text(text).expect("match should parse");
        let StatementKind::Expression(Expression {
            kind: ExpressionKind::Match { arms, .. },
            ..
        }) = &program.statements[0].kind
        else {
            panic!("expected a match statement");
        };
        arms.iter().map(|arm| arm.pattern.to_string()).collect()
    }

    #[test]
    fn parses_nested_patterns_wildcards_and_arms_in_any_order() {
        assert_eq!(
            arm_patterns(
                "match r { Err( _ ) => 0, Ok( Some( [ head , .. _ ] ) ) => head, Ok(None) => 1 };"
            ),
            ["Err(_)", "Ok(Some([head, .._]))", "Ok(None)"]
        );
        assert_eq!(
            arm_patterns("match items { [_, ..rest] => 1, [] => 0 };"),
            ["[_, ..rest]", "[]"]
        );
        // Which patterns may stand together is decided by analysis, so
        // incomplete, duplicate, mixed, and catch-all arms all parse.
        for text in [
            "match None { None => 0 };",
            "match Some(1) { Some(value) => value, Some(other) => other, None => 0 };",
            "match Some(1) { Some(value) => value, Err(error) => error };",
            "match Some(1) { _ => 0 };",
            "match Some(1) { other => 0 };",
        ] {
            parse_text(text).unwrap_or_else(|error| panic!("{text}: {error:?}"));
        }
    }

    #[test]
    fn rejects_malformed_patterns() {
        for (text, code, snippet) in [
            ("match o { } ;", "K1003", "}"),
            ("match o { 1 => 0 };", "K1003", "1"),
            ("match o { \"a\" => 0 };", "K1003", "\"a\""),
            ("match o { Foo(x) => 0 };", "K1003", "Foo"),
            ("match o { None(x) => 0 };", "K1003", "("),
            ("match o { Some => 0 };", "K1003", "Some =>"),
            ("match o { Some() => 0 };", "K1003", ")"),
            ("match o { Some(x, y) => 0 };", "K1002", ","),
            ("match o { [x] => 0 };", "K1003", "[x]"),
            ("match o { [x, y] => 0 };", "K1003", "[x, y"),
            ("match o { [Some(x), ..t] => 0 };", "K1003", "Some"),
            ("match o { [h, ..None] => 0 };", "K1003", "None"),
            ("match o { [h, ..[]] => 0 };", "K1003", "["),
            ("match o { [x, ..x] => 0 };", "K2002", "x, ..x"),
            ("match o { Some(print) => 0 };", "K2002", "print"),
            ("match o { Some(mut x) => 0 };", "K1103", "mut"),
            ("match o { Some(x) => 0 None => 1 };", "K1002", "None"),
        ] {
            let error = parse_text(text).expect_err(text);
            assert_eq!(error.code(), code, "{text}: {error:?}");
            let span = error.span();
            assert_eq!(&text[span.start..span.end], snippet, "{text}");
        }
    }

    #[test]
    fn nested_patterns_count_toward_the_nesting_limit() {
        // The payload of each variant pattern opens one level.
        let pattern = |depth: usize| format!("{}x{}", "Some(".repeat(depth), ")".repeat(depth));
        let at_limit = format!("match o {{ {} => 0 }};", pattern(MAX_NESTING_DEPTH - 1));
        parse_on_compiler_stack(at_limit).expect("patterns within the limit should parse");
        let error = parse_on_compiler_stack(format!(
            "match o {{ {} => 0 }};",
            pattern(MAX_NESTING_DEPTH)
        ))
        .expect_err("patterns beyond the limit should fail");
        assert_eq!(error.code(), "K1005");

        // An arm's value nests one level deeper per pattern nested in a
        // payload, as Core IR decides it with one nested match per level.
        let value = |levels: usize| format!("{}0{}", "(".repeat(levels), ")".repeat(levels));
        let nested = pattern(3).replace('x', "None");
        // The arm list opens one level and the nested patterns three more.
        let fits = MAX_NESTING_DEPTH - 1 - 3;
        parse_on_compiler_stack(format!("match o {{ {nested} => {} }};", value(fits)))
            .expect("the arm value should fit");
        let error =
            parse_on_compiler_stack(format!("match o {{ {nested} => {} }};", value(fits + 1)))
                .expect_err("the arm value should cross the limit");
        assert_eq!(error.code(), "K1005");
    }

    #[test]
    fn parses_let_else_with_either_variant_first() {
        let text = "fn f(r, o) {\n    let Ok(value): Result<Int, String> = r else Err(error) => 0;\n    \
                    let None = o else Some(found) => found;\n    value\n}\n";
        let program = parse_text(text).expect("let-else should parse");
        let StatementKind::Function { body, .. } = &program.statements[0].kind else {
            panic!("the function should parse");
        };
        let StatementKind::LetElse {
            pattern,
            annotation: Some(annotation),
            value,
            else_span,
            arm,
        } = &body.statements[0].kind
        else {
            panic!("the first statement should be a let-else");
        };
        assert_eq!(pattern.variant, VariantName::Ok);
        assert_eq!(pattern.binding.as_deref(), Some("value"));
        assert_eq!(&text[pattern.span.start..pattern.span.end], "Ok(value)");
        assert_eq!(annotation.to_string(), "Result<Int, String>");
        assert_eq!(&text[value.span.start..value.span.end], "r");
        assert_eq!(&text[else_span.start..else_span.end], "else");
        assert_eq!(arm.variant, VariantName::Err);
        assert_eq!(arm.binding.as_deref(), Some("error"));
        assert_eq!(&text[arm.span.start..arm.span.end], "Err(error) => 0");
        assert_eq!(
            &text[body.statements[0].span.start..body.statements[0].span.end],
            "let Ok(value): Result<Int, String> = r else Err(error) => 0;"
        );
        let StatementKind::LetElse { pattern, arm, .. } = &body.statements[1].kind else {
            panic!("the second statement should be a let-else");
        };
        assert_eq!(
            (pattern.variant, pattern.binding.as_deref()),
            (VariantName::None, None)
        );
        assert_eq!(arm.variant, VariantName::Some);

        // Mismatched patterns parse; analysis reports them (`K3014`).
        parse_text("fn f(r) { let Ok(v) = r else None => 0; v }")
            .expect("non-complementary patterns should parse");
        // `Ok` without a binder is still a reserved name, not a pattern.
        let error = parse_text("let Ok = 1;").expect_err("a reserved name");
        assert_eq!(error.code(), "K2002");
    }

    #[test]
    fn reports_malformed_let_else_statements() {
        for (text, code, at, notes) in [
            // A variant pattern needs a failure arm.
            ("fn f(o) { let Some(x) = o; x }", "K1002", ";", 1),
            // The value may not end in `}`.
            (
                "fn f(c) { let Ok(x) = if c { Ok(1) } else { Err(2) } else Err(e) => e; x }",
                "K3014",
                "if c { Ok(1) } else { Err(2) }",
                1,
            ),
            (
                "fn f(o) { let Some(x) = { o } else None => 0; x }",
                "K3014",
                "{ o }",
                1,
            ),
            // Another language's guard clause.
            (
                "fn f(o) { let Some(x) = o else { return 0; }; x }",
                "K1112",
                "else { return",
                1,
            ),
            (
                "fn f(o) { let Some(x) = o else { 0 }; x }",
                "K1003",
                "{ 0 }",
                1,
            ),
            (
                "fn f(o) { let Some(x) = o else _ => 0; x }",
                "K1003",
                "_",
                0,
            ),
            (
                "fn f(o) { let Some(x) = o else None(y) => 0; x }",
                "K1003",
                "(y)",
                0,
            ),
            (
                "fn f(o) { let Some(x) = o else Some => 0; x }",
                "K1003",
                "Some =>",
                0,
            ),
            (
                "fn f(o) { let None(x) = o else Some(y) => y; 0 }",
                "K1003",
                "(x)",
                0,
            ),
            (
                "fn f(r) { let Ok(Some(x)) = r else Err(e) => 0; x }",
                "K3014",
                "Some",
                1,
            ),
            (
                "fn f(r) { let Ok(x) = r else Err([]) => 0; x }",
                "K3014",
                "[",
                1,
            ),
            (
                "fn f(o) { let Some(mut x) = o else None => 0; x }",
                "K1103",
                "mut",
                1,
            ),
            (
                "fn f(o) { let Some(x) = o else None => 0 }",
                "K1002",
                "}",
                0,
            ),
        ] {
            let error = parse_text(text).expect_err("malformed let-else should not parse");
            assert_eq!(error.code(), code, "{text}");
            let start = text.find(at).expect("the span text should occur");
            assert_eq!(error.span().start, start, "{text}");
            assert_eq!(error.notes().len(), notes, "{text}");
        }
        // Parentheses keep a value ending in `}` apart from the `else`.
        parse_text("fn f(c) { let Ok(x) = (if c { Ok(1) } else { Err(2) }) else Err(e) => e; x }")
            .expect("a parenthesized value should parse");
    }

    #[test]
    fn let_else_arms_open_one_nesting_level() {
        let at_limit = format!(
            "fn f(o) {{ let Some(x) = o else None => {}0{}; x }}\n",
            "(".repeat(MAX_NESTING_DEPTH - 2),
            ")".repeat(MAX_NESTING_DEPTH - 2)
        );
        parse_on_compiler_stack(at_limit).expect("the arm at the limit should parse");
        let past = format!(
            "fn f(o) {{ let Some(x) = o else None => {}0{}; x }}\n",
            "(".repeat(MAX_NESTING_DEPTH - 1),
            ")".repeat(MAX_NESTING_DEPTH - 1)
        );
        let error = parse_on_compiler_stack(past).expect_err("one level too deep");
        assert_eq!(error.code(), "K1005");
        // The value does not open a level.
        let value = format!(
            "fn f(o) {{ let Some(x) = {}o{} else None => 0; x }}\n",
            "(".repeat(MAX_NESTING_DEPTH - 1),
            ")".repeat(MAX_NESTING_DEPTH - 1)
        );
        parse_on_compiler_stack(value).expect("the value at the limit should parse");
    }
}
