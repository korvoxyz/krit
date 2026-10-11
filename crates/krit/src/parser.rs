use std::collections::HashSet;

use crate::{
    Diagnostic, Span,
    ast::{
        BinaryOperator, Block, Expression, ExpressionKind, MatchKind, Parameter, Program,
        RecordField, RecordTypeField, Statement, StatementKind, TypeAnnotation, TypeKind,
        UnaryOperator, ValueLiteral, VariantArm, VariantFamily, VariantName,
    },
    limits::{MAX_NESTING_DEPTH, MAX_STRUCTURAL_DEPTH, nesting_error, structural_error},
    token::{Token, TokenKind},
};

pub fn parse(tokens: Vec<Token>) -> Result<Program, Diagnostic> {
    Parser::new(tokens).program()
}

struct Parser {
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
}

impl Parser {
    fn new(tokens: Vec<Token>) -> Self {
        Self {
            tokens,
            cursor: 0,
            depth: 0,
            chain_depth: 0,
            peak: 0,
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
            let expression = self.expression()?;
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

            let expression = self.expression()?;
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
            TokenKind::Identifier(name) => Ok(Expression {
                kind: ExpressionKind::Variable(name),
                span: token.span,
            }),
            TokenKind::LeftParen => {
                let expression = self.nested(token.span, Self::expression)?;
                let end = self.expect(TokenKind::RightParen)?.span;
                Ok(Expression {
                    span: token.span.join(end),
                    ..expression
                })
            }
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
            _ => Err(Diagnostic::new(
                "K1001",
                format!("unexpected {}", token.kind.description()),
                token.span,
            )),
        }
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
            self.expect(TokenKind::Else)?;
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
        self.nested(open, |parser| {
            if parser.check(&TokenKind::LeftBracket) {
                parser.list_match(start, subject)
            } else {
                parser.variant_match(start, subject)
            }
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
        parse(lex(&source)?)
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
