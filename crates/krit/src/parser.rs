use std::collections::HashSet;

use crate::{
    Diagnostic, Source, Span,
    ast::{
        BinaryOperator, Block, ConstructField, ContractRecord, DocComment, Expression,
        ExpressionKind, ListEntry, MatchKind, Parameter, Program, RecordField, RecordTypeField,
        Statement, StatementKind, TypeAnnotation, TypeKind, UnaryOperator, ValueLiteral,
        VariantArm, VariantFamily, VariantName,
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
    /// Whether a contract type name followed by `{` starts a named
    /// construction here. It does not directly in an `if` condition or a
    /// `match` subject, where the `{` opens the block or the arms, until a
    /// bracketed group or block inside them allows it again
    /// ([`Self::with_construction`]).
    construction_allowed: bool,
}

impl<'source> Parser<'source> {
    fn new(text: &'source str, tokens: Vec<Token>) -> Self {
        Self {
            text,
            tokens,
            cursor: 0,
            depth: 0,
            chain_depth: 0,
            peak: 0,
            speculations: 0,
            construction_allowed: true,
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

    fn function_declaration(&mut self) -> Result<Statement, Diagnostic> {
        let doc = self.doc_comment();
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
                doc,
            },
            span,
        })
    }

    fn webhook_declaration(&mut self) -> Result<Statement, Diagnostic> {
        let doc = self.doc_comment();
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
                doc,
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
        let doc = self.doc_comment();
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
                doc,
            }
        } else {
            StatementKind::ScheduleHandler {
                schedule: resource,
                schedule_span: resource_span,
                name,
                parameters,
                return_type,
                body,
                doc,
            }
        };
        Ok(Statement { kind, span })
    }

    /// The doc comment of the item whose first token is the current one: the
    /// `///` lines directly before it ([`doc_comment_before`]).
    fn doc_comment(&self) -> Option<DocComment> {
        let previous = self
            .cursor
            .checked_sub(1)
            .map(|index| self.tokens[index].span);
        doc_comment_before(
            self.text,
            previous.map_or(0, |span| span.end),
            self.current().span.start,
            previous.is_some(),
        )
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
        self.nested(start, |parser| {
            parser.with_construction(true, |parser| parser.block_contents(start))
        })
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
        // Each call or field-access link opens one level for the rest of the
        // chain. The tree nests the other way, with the first callee or record
        // deepest, so the chain also measures the tree it builds and checks
        // its structural depth after each link, as operator chains do.
        let mut height = self.peak - base;
        let depth = self.depth;
        loop {
            if self.check(&TokenKind::LeftParen) {
                let open = self.advance().span;
                self.enter(open)?;
                let arguments_depth = self.structural_depth();
                let callee_peak = std::mem::replace(&mut self.peak, arguments_depth);
                let arguments = self.with_construction(true, Self::arguments)?;
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
                return Err(self.question_mark());
            } else {
                break;
            }
        }
        self.depth = depth;
        self.peak = self.peak.max(outer_peak);
        Ok(expression)
    }

    /// Parses a call's arguments up to, but not including, its `)`.
    fn arguments(&mut self) -> Result<Vec<Expression>, Diagnostic> {
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
        Ok(arguments)
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
                if let Some(contract) = ContractRecord::from_name(&name)
                    && self.check(&TokenKind::LeftBrace)
                {
                    if self.construction_allowed {
                        return self.construction(contract, token.span);
                    }
                    if let Some(error) = self.restricted_construction(contract, token.span) {
                        return Err(error);
                    }
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

    /// Parses `( expression )` or the unit value `()` after its `(` at `open`,
    /// and reports an arrow function's parameter list followed by `=>`
    /// (`K1109`): `(x) => e`, `(x, y) => e`, `() => e`.
    fn parenthesized(&mut self, open: Span) -> Result<Expression, Diagnostic> {
        let open_index = self.cursor - 1;
        if self.check(&TokenKind::RightParen) {
            let end = self.advance().span;
            if self.check(&TokenKind::FatArrow) {
                return Err(arrow_function(open.join(self.current().span)));
            }
            return Ok(Expression {
                kind: ExpressionKind::Unit,
                span: open.join(end),
            });
        }
        let expression = match self.nested(open, |parser| {
            parser.with_construction(true, Self::expression)
        }) {
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

    /// Diagnoses a `?` that follows an expression.
    ///
    /// `c ? a : b` is another language's conditional operator (`K1110`, see
    /// [`Self::ternary_after`]). Any other `?` there is a postfix operator that
    /// Krit does not have yet.
    fn question_mark(&mut self) -> Diagnostic {
        let question = self.advance().span;
        self.ternary_after(question).unwrap_or_else(|| {
            Diagnostic::new("K1001", "unexpected `?`", question).with_note(
                "postfix `?` arrives in Krit 0.3; until then, handle the `Result` or \
                 `Option` with `match`",
            )
        })
    }

    /// Returns `K1110` when the `?` at `question` starts another language's
    /// conditional operator: the tokens after it are an expression and then
    /// `:`, or they start an expression that does not parse and a `:` follows
    /// at the `?`'s bracket depth ([`Self::ternary_colon`]), as in
    /// `ready ? items[0] : None`.
    ///
    /// The first branch is parsed speculatively, one nesting level deeper, and
    /// the parser is restored either way, so the check never consumes input.
    /// A nesting limit (`K1005`) or a conditional operator reached inside the
    /// first branch is reported in place of the outer one.
    ///
    /// Every `?` ends the parse with an error, and only this speculation
    /// recovers from one, so only the outermost `?` is examined outside a
    /// speculation. The speculations add at most one parse of the tokens after
    /// it, and only the outermost `?` scans for a `:`.
    fn ternary_after(&mut self, question: Span) -> Option<Diagnostic> {
        if !starts_expression(&self.current().kind) {
            return None;
        }
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
            Err(_) if self.speculations == 0 => self.ternary_colon(question_index),
            Err(_) => None,
        };
        colon.map(|colon| {
            Habit::Ternary.diagnostic(
                "the conditional operator `? :` is not Krit syntax",
                question.join(colon),
            )
        })
    }

    /// The `:` that would end a conditional operator's first branch after the
    /// `?` at token `question`: the first `:` at the `?`'s bracket depth,
    /// looking no further than the end of the file, a closing bracket opened
    /// before the `?`, or the first `;`, `,`, or `let` at that depth, none of
    /// which a first branch contains outside brackets.
    fn ternary_colon(&self, question: usize) -> Option<Span> {
        let mut depth = 0_usize;
        for token in &self.tokens[question + 1..] {
            match &token.kind {
                TokenKind::LeftParen | TokenKind::LeftBracket | TokenKind::LeftBrace => {
                    depth += 1;
                }
                TokenKind::RightParen | TokenKind::RightBracket | TokenKind::RightBrace => {
                    depth = depth.checked_sub(1)?;
                }
                TokenKind::Colon if depth == 0 => return Some(token.span),
                TokenKind::Semicolon | TokenKind::Comma | TokenKind::Let if depth == 0 => {
                    return None;
                }
                TokenKind::Eof => return None,
                _ => {}
            }
        }
        None
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
        self.nested(start, |parser| {
            parser.with_construction(true, |parser| parser.list_elements(start))
        })
    }

    /// Parses the elements of a list literal after its `[` at `start`: plain
    /// elements (`[a, b]`), or `key: value` entries (`["k": v]`), which make
    /// it an entry list. The first element decides; a list that mixes the two
    /// forms is `K1001` at the first element written the other way.
    fn list_elements(&mut self, start: Span) -> Result<Expression, Diagnostic> {
        if self.check(&TokenKind::Colon)
            && self
                .tokens
                .get(self.cursor + 1)
                .is_some_and(|token| token.kind == TokenKind::RightBracket)
        {
            let end = self.tokens[self.cursor + 1].span;
            return Err(
                Diagnostic::new("K1001", "`[:]` is not Krit syntax", start.join(end))
                    .with_note("the empty list is `[]`, for entry lists too"),
            );
        }
        let mut elements = Vec::new();
        let mut entries = Vec::new();
        if !self.check(&TokenKind::RightBracket) {
            loop {
                let element = self.expression()?;
                if self.check(&TokenKind::Colon) {
                    if !elements.is_empty() {
                        return Err(mixed_list(element.span.join(self.current().span)));
                    }
                    self.advance();
                    let value = self.expression()?;
                    entries.push(ListEntry {
                        span: element.span.join(value.span),
                        key: element,
                        value,
                    });
                } else if !entries.is_empty() {
                    return Err(mixed_list(element.span));
                } else {
                    elements.push(element);
                }
                if !self.consume(&TokenKind::Comma) {
                    break;
                }
                if self.check(&TokenKind::RightBracket) {
                    break;
                }
            }
        }
        let end = self.expect(TokenKind::RightBracket)?.span;
        let kind = if entries.is_empty() {
            ExpressionKind::List(elements)
        } else {
            ExpressionKind::EntryList(entries)
        };
        Ok(Expression {
            kind,
            span: start.join(end),
        })
    }

    fn record(&mut self, start: Span) -> Result<Expression, Diagnostic> {
        self.nested(start, |parser| {
            parser.with_construction(true, |parser| parser.record_fields(start))
        })
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

    /// Parses a named construction `TypeName { field: value, punned, ... }`
    /// after its type name at `start`; the current token is its `{`.
    ///
    /// The parser accepts any field names. Which fields a contract has,
    /// requires, and defaults is checked by analysis (`K3016`).
    fn construction(
        &mut self,
        contract: ContractRecord,
        start: Span,
    ) -> Result<Expression, Diagnostic> {
        self.expect(TokenKind::LeftBrace)?;
        self.nested(start, |parser| {
            parser.with_construction(true, |parser| parser.construction_fields(contract, start))
        })
    }

    fn construction_fields(
        &mut self,
        contract: ContractRecord,
        start: Span,
    ) -> Result<Expression, Diagnostic> {
        let mut fields = Vec::new();
        if !self.check(&TokenKind::RightBrace) {
            loop {
                let (name, name_span) = self.identifier()?;
                let value = if self.consume(&TokenKind::Colon) {
                    self.expression()?
                } else if self.check(&TokenKind::Comma) || self.check(&TokenKind::RightBrace) {
                    // A punned field `body` means `body: body`.
                    Expression {
                        kind: ExpressionKind::Variable(name.clone()),
                        span: name_span,
                    }
                } else {
                    return Err(self.expected("`:`, `,`, or `}`"));
                };
                fields.push(ConstructField {
                    span: name_span.join(value.span),
                    name,
                    name_span,
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
            kind: ExpressionKind::Construct {
                contract,
                contract_span: start,
                fields,
            },
            span: start.join(end),
        })
    }

    /// Reports a named construction written directly as an `if` condition or
    /// a `match` subject, where its `{` would open the block or the arms
    /// instead: the type name at `name` is followed by `{` and then a field
    /// name with `:` or `,`, which never start a block.
    ///
    /// `if HttpResponse { x } else { y }` stays a condition named
    /// `HttpResponse`, since `{ x }` is a valid block.
    fn restricted_construction(&self, contract: ContractRecord, name: Span) -> Option<Diagnostic> {
        let field = self.tokens.get(self.cursor + 1)?;
        let after = self.tokens.get(self.cursor + 2)?;
        let starts_field = matches!(field.kind, TokenKind::Identifier(_))
            && matches!(after.kind, TokenKind::Colon | TokenKind::Comma);
        starts_field.then(|| {
            Diagnostic::new(
                "K1001",
                format!(
                    "`{} {{ ... }}` cannot be written directly as an `if` condition or a \
                     `match` subject",
                    contract.name()
                ),
                name.join(self.current().span),
            )
            .with_note(format!(
                "wrap the construction in parentheses: `({} {{ ... }})`",
                contract.name()
            ))
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
            let condition = self.nested(link_start, |parser| {
                parser.with_construction(false, Self::expression)
            })?;
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
        let subject = self.nested(start, |parser| {
            parser.with_construction(false, Self::expression)
        })?;
        let open = self.expect(TokenKind::LeftBrace)?.span;
        self.nested(open, |parser| {
            parser.with_construction(true, |parser| {
                if parser.check(&TokenKind::LeftBracket) {
                    parser.list_match(start, subject)
                } else {
                    parser.variant_match(start, subject)
                }
            })
        })
    }

    fn list_match(&mut self, start: Span, subject: Expression) -> Result<Expression, Diagnostic> {
        let empty_pattern_start = self.current().span;
        if !self.consume(&TokenKind::LeftBracket) || !self.consume(&TokenKind::RightBracket) {
            return Err(Diagnostic::new(
                "K1003",
                "the first match pattern must be `[]`",
                empty_pattern_start.join(self.current().span),
            ));
        }
        self.expect(TokenKind::FatArrow)?;
        let empty_case = self.expression()?;
        self.expect(TokenKind::Comma)?;

        let cons_pattern_start = self.current().span;
        if !self.consume(&TokenKind::LeftBracket) {
            return Err(Diagnostic::new(
                "K1003",
                "the second match pattern must be `[head, ..tail]`",
                self.current().span,
            ));
        }
        let (head_name, head_span) = self.binding_name()?;
        self.expect(TokenKind::Comma)?;
        if !self.consume(&TokenKind::DotDot) {
            return Err(Diagnostic::new(
                "K1003",
                "the second match pattern must bind a `..tail`",
                cons_pattern_start.join(self.current().span),
            ));
        }
        let (tail_name, tail_span) = self.binding_name()?;
        // The discard name `_` may be bound more than once.
        if head_name == tail_name && head_name != "_" {
            return Err(Diagnostic::new(
                "K2002",
                format!("duplicate match binding `{tail_name}`"),
                head_span.join(tail_span),
            ));
        }
        self.expect(TokenKind::RightBracket)?;
        self.expect(TokenKind::FatArrow)?;
        let cons_case = self.expression()?;
        self.consume(&TokenKind::Comma);
        let end = self.expect(TokenKind::RightBrace)?.span;

        Ok(Expression {
            kind: ExpressionKind::Match {
                subject: Box::new(subject),
                kind: MatchKind::List {
                    empty_case: Box::new(empty_case),
                    head_name,
                    head_span,
                    tail_name,
                    tail_span,
                    cons_case: Box::new(cons_case),
                },
            },
            span: start.join(end),
        })
    }

    fn variant_match(
        &mut self,
        start: Span,
        subject: Expression,
    ) -> Result<Expression, Diagnostic> {
        let mut arms = Vec::new();
        let mut seen = HashSet::new();

        while !self.check(&TokenKind::RightBrace) {
            if self.check(&TokenKind::Eof) {
                return Err(self.expected("variant match arm or `}`"));
            }
            let pattern_start = self.current().span;
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
            if !seen.insert(variant) {
                return Err(Diagnostic::new(
                    "K1003",
                    format!("duplicate `{}` match arm", variant.as_str()),
                    token.span,
                ));
            }
            let binding = if variant == VariantName::None {
                if self.check(&TokenKind::LeftParen) {
                    return Err(Diagnostic::new(
                        "K1003",
                        "`None` does not bind a value",
                        self.current().span,
                    ));
                }
                None
            } else {
                if !self.consume(&TokenKind::LeftParen) {
                    return Err(Diagnostic::new(
                        "K1003",
                        format!("`{}` must bind one value", variant.as_str()),
                        token.span.join(self.current().span),
                    ));
                }
                let (binding, _) = self.binding_name()?;
                self.expect(TokenKind::RightParen)?;
                Some(binding)
            };
            self.expect(TokenKind::FatArrow)?;
            let value = self.expression()?;
            let arm_span = pattern_start.join(value.span);
            arms.push(VariantArm {
                variant,
                binding,
                value,
                span: arm_span,
            });
            if !self.consume(&TokenKind::Comma) && !self.check(&TokenKind::RightBrace) {
                return Err(self.expected("`,` or `}`"));
            }
        }
        let end = self.expect(TokenKind::RightBrace)?.span;
        let family = if seen.len() == 2
            && seen.contains(&VariantName::Some)
            && seen.contains(&VariantName::None)
        {
            VariantFamily::Option
        } else if seen.len() == 2
            && seen.contains(&VariantName::Ok)
            && seen.contains(&VariantName::Err)
        {
            VariantFamily::Result
        } else {
            return Err(Diagnostic::new(
                "K1003",
                "variant match must contain exactly `Some(name)` and `None`, or exactly `Ok(name)` and `Err(name)`",
                start.join(end),
            ));
        };

        Ok(Expression {
            kind: ExpressionKind::Match {
                subject: Box::new(subject),
                kind: MatchKind::Variants { family, arms },
            },
            span: start.join(end),
        })
    }

    fn type_annotation(&mut self) -> Result<TypeAnnotation, Diagnostic> {
        let start = self.current().span;
        let token = self.advance().clone();
        let TokenKind::Identifier(name) = token.kind else {
            let error = Diagnostic::new(
                "K1002",
                format!("expected built-in type, found {}", token.kind.description()),
                token.span,
            );
            // `-> ()`: the unit value is not a type.
            return Err(
                if token.kind == TokenKind::LeftParen && self.check(&TokenKind::RightParen) {
                    error.with_note("the unit type is spelled `Unit`; `()` is the unit value")
                } else {
                    error
                },
            );
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

    /// Runs `parse` with named construction allowed or not
    /// ([`Self::construction_allowed`]), and restores the enclosing setting
    /// whether `parse` succeeds or not, so a failed speculative parse leaves
    /// it unchanged too.
    fn with_construction<T>(&mut self, allowed: bool, parse: impl FnOnce(&mut Self) -> T) -> T {
        let outer = std::mem::replace(&mut self.construction_allowed, allowed);
        let result = parse(self);
        self.construction_allowed = outer;
        result
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

/// A list literal at `span` that writes one element in the other form than
/// its first element: a plain element in an entry list, or a `key: value`
/// entry in a list of plain elements (`K1001`).
fn mixed_list(span: Span) -> Diagnostic {
    Diagnostic::new(
        "K1001",
        "a list literal cannot mix `key: value` entries and plain elements",
        span,
    )
    .with_note(
        "write every element as `key: value` for a list of `record { name, value }` entries, \
         or write none that way",
    )
}

/// The doc comment written directly before an item that starts at
/// `item_start`, found in the trivia between `trivia_start` and the item:
/// the end of the previous token (`after_token`) or the start of the file.
///
/// Trivia holds only whitespace and `//` comments, and a comment ends its
/// line, so each line of it is blank or holds one comment after its
/// indentation. The doc comment is the run of `///` lines that ends at the
/// item, read upward and skipping blank lines; it stops at the first other
/// comment, including a `////` banner, and at the line of the previous token,
/// where a comment is an end-of-line comment.
fn doc_comment_before(
    text: &str,
    trivia_start: usize,
    item_start: usize,
    after_token: bool,
) -> Option<DocComment> {
    let trivia = &text[trivia_start..item_start];
    let mut lines = Vec::new();
    let mut line_start = 0;
    for (index, character) in trivia.char_indices() {
        if character == '\n' || character == '\r' {
            lines.push((trivia_start + line_start, &trivia[line_start..index]));
            line_start = index + 1;
        }
    }
    // The last piece is the item's own indentation, and with `after_token`
    // the first shares the previous token's line.
    let own_lines = lines.get(usize::from(after_token)..)?;
    let mut doc_lines = Vec::new();
    for (start, line) in own_lines.iter().rev() {
        let comment = line.trim_start();
        if comment.is_empty() {
            continue;
        }
        let Some(line_text) = doc_line_text(comment) else {
            break;
        };
        let comment_start = start + (line.len() - comment.len());
        doc_lines.push((
            Span::new(comment_start, comment_start + comment.trim_end().len()),
            line_text.trim_end(),
        ));
    }
    let (last, _) = *doc_lines.first()?;
    let (first, _) = *doc_lines.last()?;
    let text = doc_lines
        .iter()
        .rev()
        .map(|(_, line)| *line)
        .collect::<Vec<_>>()
        .join("\n");
    Some(DocComment {
        text,
        span: first.join(last),
    })
}

/// The text of a doc comment line `comment` (from its `//` to the end of the
/// line): what follows `///`, without one leading space. `None` when the
/// comment is not a doc line, including a `////` banner.
pub(crate) fn doc_line_text(comment: &str) -> Option<&str> {
    let rest = comment.strip_prefix("///")?;
    if rest.starts_with('/') {
        return None;
    }
    Some(rest.strip_prefix(' ').unwrap_or(rest))
}

/// An arrow function at `span` (`K1109`).
fn arrow_function(span: Span) -> Diagnostic {
    Habit::ArrowFunction.diagnostic("arrow functions are not Krit syntax", span)
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
    fn rejects_duplicate_record_fields_and_variant_arms() {
        let duplicate_field =
            parse_text("record { value: 1, value: 2 };").expect_err("duplicate field should fail");
        assert_eq!(duplicate_field.code(), "K2002");

        let duplicate_type_field =
            parse_text("let value: Record { item: Int, item: Bool } = record {};")
                .expect_err("duplicate type field should fail");
        assert_eq!(duplicate_type_field.code(), "K2002");

        let duplicate_arm =
            parse_text("match Some(1) { Some(value) => value, Some(other) => other, None => 0 };")
                .expect_err("duplicate arm should fail");
        assert_eq!(duplicate_arm.code(), "K1003");
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
            "constructions" => ("HttpResponse { status: ", "1", " }", ""),
            "entries" => ("[\"k\": ", "\"v\"", "]", ""),
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
            "constructions",
            "entries",
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
    fn a_question_mark_outside_the_ternary_shape_is_an_unexpected_token() {
        for (text, spanned) in [
            ("fn f(x) { x? }", "?"),
            ("fn f(x) { x?.y }", "?"),
            ("fn f(x) { x ?? 1 }", "?"),
            ("fn f(x) { x ? 1 }", "?"),
            ("let y = ?;", "?"),
        ] {
            let error = parse_text(text).expect_err("`?` should not parse yet");
            assert_eq!(error.code(), "K1001", "{text}");
            assert_eq!(
                &text[error.span().start..error.span().end],
                spanned,
                "{text}"
            );
        }
        // The ternary check leaves the parser where it was: the first `?`
        // here is not a ternary, so the second one is never examined.
        let error = parse_text("let y = a ? (b ? c : d);").expect_err("should not parse");
        assert_eq!(error.code(), "K1110");
        let error = parse_text("let y = a ?;").expect_err("should not parse");
        assert_eq!(error.code(), "K1001");
        assert_eq!(error.notes().len(), 1);
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
            parse_on_compiler_stack(format!("let y = x ? {nested} : 2;\n")).expect_err("deep");
        assert_eq!(error.code(), "K1005");
        let error = parse_on_compiler_stack(format!("let y = {nested};\n")).expect_err("deep");
        assert_eq!(error.code(), "K1005");
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

    /// The written fields of the named construction `text` is the value of,
    /// as `(name, punned)` pairs.
    fn construction_fields(text: &str) -> (ContractRecord, Vec<(String, bool)>) {
        let program = parse_text(text).expect("construction should parse");
        let StatementKind::Let { value, .. } = &program.statements.last().unwrap().kind else {
            panic!("the last statement should be a let");
        };
        let ExpressionKind::Construct {
            contract, fields, ..
        } = &value.kind
        else {
            panic!("the value should be a named construction: {value:?}");
        };
        let fields = fields
            .iter()
            .map(|field| (field.name.clone(), field.is_punned()))
            .collect();
        (*contract, fields)
    }

    #[test]
    fn parses_named_construction_with_punning_and_trailing_commas() {
        let (contract, fields) =
            construction_fields("let body = \"x\";\nlet r = HttpResponse { status: 200, body, };");
        assert_eq!(contract, ContractRecord::HttpResponse);
        assert_eq!(
            fields,
            [("status".to_owned(), false), ("body".to_owned(), true)]
        );

        let (contract, fields) = construction_fields("let r = HttpRequest {};");
        assert_eq!(contract, ContractRecord::HttpRequest);
        assert!(fields.is_empty());

        for (name, contract) in [
            ("HttpHeader", ContractRecord::HttpHeader),
            ("LogField", ContractRecord::LogField),
        ] {
            let (parsed, fields) =
                construction_fields(&format!("let r = {name} {{ name: \"a\", value }};"));
            assert_eq!(parsed, contract);
            assert_eq!(
                fields,
                [("name".to_owned(), false), ("value".to_owned(), true)]
            );
        }

        // A punned field's value is the variable at the field name's span.
        let program = parse_text("let r = HttpResponse { status };").expect("should parse");
        let StatementKind::Let { value, .. } = &program.statements[0].kind else {
            panic!("let expected");
        };
        let ExpressionKind::Construct {
            contract_span,
            fields,
            ..
        } = &value.kind
        else {
            panic!("construction expected");
        };
        assert_eq!(*contract_span, Span::new(8, 20));
        assert_eq!(value.span, Span::new(8, 31));
        assert_eq!(fields[0].name_span, Span::new(23, 29));
        assert_eq!(fields[0].value.span, fields[0].name_span);
        assert!(
            matches!(&fields[0].value.kind, ExpressionKind::Variable(name) if name == "status")
        );
    }

    #[test]
    fn only_the_four_contract_names_start_a_named_construction() {
        for text in [
            "let r = QueueJob { id: \"x\" };",
            "let r = Response { status: 200 };",
            "let r = httpResponse { status: 200 };",
        ] {
            let error = parse_text(text).expect_err("other names are not constructions");
            assert_eq!(error.code(), "K1002", "{text}");
        }
        let error = parse_text("let r = HttpResponse { status 200 };")
            .expect_err("a field needs `:`, `,`, or `}` after its name");
        assert_eq!(error.code(), "K1002");
        assert!(error.message().contains("`:`, `,`, or `}`"));
    }

    #[test]
    fn named_construction_needs_parentheses_as_a_condition_or_subject() {
        for (text, span) in [
            (
                "if HttpResponse { status: 1 }.status == 1 { 1 } else { 2 };",
                Span::new(3, 17),
            ),
            (
                "let x = 1;\nif x == 1 { 1 } else if HttpHeader { name, value: \"\" }.name == \"\" { 2 } else { 3 };",
                Span::new(35, 47),
            ),
            (
                "match HttpRequest { method: \"GET\", path: \"/\" }.method { _ => 1 };",
                Span::new(6, 19),
            ),
            (
                "if !LogField { name: \"a\", value: \"b\" }.name { 1 } else { 2 };",
                Span::new(4, 14),
            ),
        ] {
            let error = parse_text(text).expect_err("a bare construction condition should fail");
            assert_eq!(error.code(), "K1001", "{text}");
            assert_eq!(error.span(), span, "{text}");
            assert_eq!(error.notes().len(), 1, "{text}");
        }

        // Parentheses, brackets, call arguments, blocks, and match arms
        // inside a condition or subject allow it again.
        for text in [
            "if (HttpResponse { status: 1 }).status == 1 { 1 } else { 2 };",
            "fn f(r) { r.status == 1 }\nif f(HttpResponse { status: 1 }) { 1 } else { 2 };",
            "if [HttpHeader { name: \"a\", value: \"b\" }] == [] { 1 } else { 2 };",
            "if { HttpResponse { status: 1 } }.status == 1 { 1 } else { 2 };",
            "if match Some(1) { Some(x) => HttpResponse { status: x }, None => HttpResponse { status: 0 } }.status == 1 { 1 } else { 2 };",
            "match (HttpRequest { method: \"GET\", path: \"/\" }).headers { [] => 0, [h, ..t] => 1 };",
            "if true { HttpResponse { status: 1 } } else { HttpResponse { status: 2 } };",
        ] {
            parse_text(text).unwrap_or_else(|error| panic!("{text}: {error:?}"));
        }
    }

    #[test]
    fn contract_names_before_a_block_keep_their_meaning_in_conditions() {
        // `{ x }` is a valid block, so the condition is the name itself.
        let program = parse_text(
            "let HttpResponse = true;\nlet x = 1;\nlet y = if HttpResponse { x } else { 2 };",
        )
        .expect("a bound contract name is an ordinary condition");
        let StatementKind::Let { value, .. } = &program.statements[2].kind else {
            panic!("let expected");
        };
        let ExpressionKind::If { condition, .. } = &value.kind else {
            panic!("if expected");
        };
        assert!(
            matches!(&condition.kind, ExpressionKind::Variable(name) if name == "HttpResponse")
        );
        parse_text("let HttpRequest = [];\nmatch HttpRequest { [] => 0, [h, ..t] => 1 };")
            .expect("a bound contract name is an ordinary match subject");
    }

    #[test]
    fn a_conditional_operator_around_a_construction_is_still_reported() {
        for text in [
            "if ready ? HttpResponse { status: 1 } : other { 1 } else { 2 };",
            "let r = ready ? HttpResponse { status: 1 } : HttpResponse { status: 2 };",
        ] {
            let error = parse_text(text).expect_err("should not parse");
            assert_eq!(error.code(), "K1110", "{text}");
        }
    }

    fn let_value(program: &Program, index: usize) -> &Expression {
        let StatementKind::Let { value, .. } = &program.statements[index].kind else {
            panic!("statement {index} should be a `let`");
        };
        value
    }

    #[test]
    fn parses_entry_lists_with_entry_spans_and_trailing_commas() {
        let text = "let fields = [\"x-a\": a, \"x-\" + b: f(c),];";
        let program = parse_text(text).expect("entry list should parse");
        let value = let_value(&program, 0);
        let ExpressionKind::EntryList(entries) = &value.kind else {
            panic!("an entry list expected, found {:?}", value.kind);
        };
        assert_eq!(entries.len(), 2);
        assert_eq!(
            &text[entries[0].span.start..entries[0].span.end],
            "\"x-a\": a"
        );
        assert!(matches!(entries[0].key.kind, ExpressionKind::Literal(_)));
        assert_eq!(
            &text[entries[1].span.start..entries[1].span.end],
            "\"x-\" + b: f(c)"
        );
        assert!(matches!(entries[1].key.kind, ExpressionKind::Binary { .. }));
        assert!(matches!(entries[1].value.kind, ExpressionKind::Call { .. }));
        assert_eq!(
            &text[value.span.start..value.span.end],
            &text[13..text.len() - 1]
        );

        // `[]` stays the plain empty list.
        let program = parse_text("let empty = [];").expect("empty list should parse");
        assert!(matches!(
            &let_value(&program, 0).kind,
            ExpressionKind::List(elements) if elements.is_empty()
        ));
    }

    #[test]
    fn a_list_cannot_mix_entries_and_plain_elements() {
        for (text, offending) in [
            ("let l = [\"k\": \"v\", plain];", "plain"),
            ("let l = [plain, \"k\": \"v\"];", "\"k\":"),
            ("let l = [\"k\": \"v\", \"j\": \"w\", [1]];", "[1]"),
        ] {
            let error = parse_text(text).expect_err("a mixed list should not parse");
            assert_eq!(error.code(), "K1001", "{text}");
            assert_eq!(
                &text[error.span().start..error.span().end],
                offending,
                "{text}"
            );
            assert_eq!(error.notes().len(), 1, "{text}");
        }

        let error = parse_text("let l = [:];").expect_err("`[:]` should not parse");
        assert_eq!(error.code(), "K1001");
        assert_eq!(error.span(), Span::new(8, 11));
        assert_eq!(error.notes().len(), 1);

        let error = parse_text("let l = [\"k\" \"v\"];").expect_err("a missing `,` fails");
        assert_eq!(error.code(), "K1002");
    }

    #[test]
    fn parses_the_unit_value_and_keeps_unit_types_spelled_unit() {
        let text = "let done = Ok(( ));\nlet unit: Unit = ();";
        let program = parse_text(text).expect("unit values should parse");
        let ExpressionKind::Call { arguments, .. } = &let_value(&program, 0).kind else {
            panic!("a call expected");
        };
        assert!(matches!(arguments[0].kind, ExpressionKind::Unit));
        assert_eq!(&text[arguments[0].span.start..arguments[0].span.end], "( )");
        assert!(matches!(let_value(&program, 1).kind, ExpressionKind::Unit));

        for text in ["let f = () => 1;", "let f = (  ) => { 1 };"] {
            let error = parse_text(text).expect_err("an arrow function should not parse");
            assert_eq!(error.code(), "K1109", "{text}");
        }

        let error = parse_text("fn f() -> () { () }").expect_err("`()` is not a type");
        assert_eq!(error.code(), "K1002");
        assert_eq!(error.span(), Span::new(10, 11));
        assert_eq!(
            error.notes(),
            ["the unit type is spelled `Unit`; `()` is the unit value"]
        );
        let error = parse_text("fn f() -> [Int] { [] }").expect_err("not a type either");
        assert!(error.notes().is_empty());
    }

    /// The doc comment of each item statement of `program`, in order.
    fn docs(program: &Program) -> Vec<Option<String>> {
        program
            .statements
            .iter()
            .filter(|statement| !matches!(statement.kind, StatementKind::Let { .. }))
            .map(|statement| statement.doc().map(|doc| doc.text.clone()))
            .collect()
    }

    #[test]
    fn attaches_doc_comments_to_the_next_item() {
        let text = "/// First line.\n///\n///   indented\n/// trailing   \nfn first() {}\n";
        let program = parse_text(text).expect("documented function should parse");
        let doc = program.statements[0].doc().expect("the doc should attach");
        assert_eq!(doc.text, "First line.\n\n  indented\ntrailing");
        assert_eq!(
            &text[doc.span.start..doc.span.end],
            &text[..text.find("   \n").unwrap()]
        );

        // Blank lines are whitespace: they neither split nor end a doc.
        let program = parse_text(
            "///A\n\n///B\n\nwebhook fn handle(request: HttpRequest) -> HttpResponse { x }\n",
        )
        .expect("documented webhook should parse");
        assert_eq!(docs(&program), [Some("A\nB".to_owned())]);
        let program = parse_text(
            "/// Queue.\nqueue \"jobs\" fn work(job: QueueJob) -> Result<String, String> { x }\n",
        )
        .expect("documented queue consumer should parse");
        assert_eq!(docs(&program), [Some("Queue.".to_owned())]);

        let program = parse_text(
            "/// Schedule.\r\nschedule \"hourly\" fn tick(event: ScheduleEvent) -> Result<String, String> {\n    /// Nested.\n    fn inner() {}\n    x\n}\n",
        )
        .expect("documented schedule should parse");
        assert_eq!(docs(&program), [Some("Schedule.".to_owned())]);
        let StatementKind::ScheduleHandler { body, .. } = &program.statements[0].kind else {
            panic!("schedule expected");
        };
        assert_eq!(
            body.statements[0].doc().map(|doc| doc.text.as_str()),
            Some("Nested.")
        );
    }

    #[test]
    fn other_comments_and_end_of_line_doc_comments_document_nothing() {
        for text in [
            // Four slashes are a banner.
            "//// Banner.\nfn f() {}\n",
            // An ordinary comment ends the run before the item.
            "/// Lost.\n// ordinary\nfn f() {}\n",
            // An end-of-line comment belongs to its line.
            "let x = 1; /// end of line\nfn f() {}\n",
            // A `let` and a function expression are not items.
            "/// Not an item.\nlet g = fn() { 1 };\nfn f() {}\n",
            // No doc at all.
            "fn f() {}\n",
            "let x = 1; fn f() {}\n",
        ] {
            let program = parse_text(text).expect("source should parse");
            assert_eq!(docs(&program), [None], "{text:?}");
        }

        let program =
            parse_text("/// Kept.\n//// banner\n/// Only this.\nfn f() {}\n/// Dangling.\n")
                .expect("source should parse");
        assert_eq!(docs(&program), [Some("Only this.".to_owned())]);
        let program = parse_text("fn f() { /// after the brace\n    fn g() {}\n}\n")
            .expect("source should parse");
        let StatementKind::Function { body, .. } = &program.statements[0].kind else {
            panic!("function expected");
        };
        assert!(body.statements[0].doc().is_none());
    }

    #[test]
    fn rejects_incomplete_or_mixed_variant_matches() {
        let missing = parse_text("match None { None => 0 };")
            .expect_err("non-exhaustive option match should fail");
        assert_eq!(missing.code(), "K1003");

        let mixed = parse_text("match Some(1) { Some(value) => value, Err(error) => error };")
            .expect_err("mixed variant families should fail");
        assert_eq!(mixed.code(), "K1003");
    }
}
