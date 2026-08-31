use std::collections::HashSet;

use crate::{
    Diagnostic, Span,
    ast::{
        BinaryOperator, Block, Expression, ExpressionKind, MatchKind, Parameter, Program,
        RecordField, RecordTypeField, Statement, StatementKind, TypeAnnotation, TypeKind,
        UnaryOperator, ValueLiteral, VariantArm, VariantFamily, VariantName,
    },
    token::{Token, TokenKind},
};

pub fn parse(tokens: Vec<Token>) -> Result<Program, Diagnostic> {
    Parser::new(tokens).program()
}

struct Parser {
    tokens: Vec<Token>,
    cursor: usize,
}

impl Parser {
    fn new(tokens: Vec<Token>) -> Self {
        Self { tokens, cursor: 0 }
    }

    fn program(mut self) -> Result<Program, Diagnostic> {
        let mut statements = Vec::new();
        while !self.check(&TokenKind::Eof) {
            statements.push(self.statement()?);
        }
        Ok(Program { statements })
    }

    fn statement(&mut self) -> Result<Statement, Diagnostic> {
        if self.check(&TokenKind::Let) {
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

    fn parameters(&mut self) -> Result<Vec<Parameter>, Diagnostic> {
        self.expect(TokenKind::LeftParen)?;
        let mut parameters = Vec::new();
        let mut seen = HashSet::new();

        if !self.check(&TokenKind::RightParen) {
            loop {
                let (name, span) = self.binding_name()?;
                if !seen.insert(name.clone()) {
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
        let mut statements = Vec::new();
        let mut tail = None;

        while !self.check(&TokenKind::RightBrace) {
            if self.check(&TokenKind::Eof) {
                return Err(self.expected("`}`"));
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
        let mut expression = self.logical_and()?;
        while self.consume(&TokenKind::OrOr) {
            let right = self.logical_and()?;
            expression = binary(expression, BinaryOperator::Or, right);
        }
        Ok(expression)
    }

    fn logical_and(&mut self) -> Result<Expression, Diagnostic> {
        let mut expression = self.equality()?;
        while self.consume(&TokenKind::AndAnd) {
            let right = self.equality()?;
            expression = binary(expression, BinaryOperator::And, right);
        }
        Ok(expression)
    }

    fn equality(&mut self) -> Result<Expression, Diagnostic> {
        let mut expression = self.comparison()?;
        loop {
            let operator = if self.consume(&TokenKind::EqualEqual) {
                Some(BinaryOperator::Equal)
            } else if self.consume(&TokenKind::BangEqual) {
                Some(BinaryOperator::NotEqual)
            } else {
                None
            };

            let Some(operator) = operator else {
                break;
            };
            let right = self.comparison()?;
            expression = binary(expression, operator, right);
        }
        Ok(expression)
    }

    fn comparison(&mut self) -> Result<Expression, Diagnostic> {
        let mut expression = self.term()?;
        loop {
            let operator = if self.consume(&TokenKind::Less) {
                Some(BinaryOperator::Less)
            } else if self.consume(&TokenKind::LessEqual) {
                Some(BinaryOperator::LessEqual)
            } else if self.consume(&TokenKind::Greater) {
                Some(BinaryOperator::Greater)
            } else if self.consume(&TokenKind::GreaterEqual) {
                Some(BinaryOperator::GreaterEqual)
            } else {
                None
            };

            let Some(operator) = operator else {
                break;
            };
            let right = self.term()?;
            expression = binary(expression, operator, right);
        }
        Ok(expression)
    }

    fn term(&mut self) -> Result<Expression, Diagnostic> {
        let mut expression = self.factor()?;
        loop {
            let operator = if self.consume(&TokenKind::Plus) {
                Some(BinaryOperator::Add)
            } else if self.consume(&TokenKind::Minus) {
                Some(BinaryOperator::Subtract)
            } else {
                None
            };

            let Some(operator) = operator else {
                break;
            };
            let right = self.factor()?;
            expression = binary(expression, operator, right);
        }
        Ok(expression)
    }

    fn factor(&mut self) -> Result<Expression, Diagnostic> {
        let mut expression = self.unary()?;
        loop {
            let operator = if self.consume(&TokenKind::Star) {
                Some(BinaryOperator::Multiply)
            } else if self.consume(&TokenKind::Slash) {
                Some(BinaryOperator::Divide)
            } else if self.consume(&TokenKind::Percent) {
                Some(BinaryOperator::Remainder)
            } else {
                None
            };

            let Some(operator) = operator else {
                break;
            };
            let right = self.unary()?;
            expression = binary(expression, operator, right);
        }
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

        let operand = self.unary()?;
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
        let mut expression = self.primary()?;
        loop {
            if self.consume(&TokenKind::LeftParen) {
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
                let end = self.expect(TokenKind::RightParen)?.span;
                let span = expression.span.join(end);
                expression = Expression {
                    kind: ExpressionKind::Call {
                        callee: Box::new(expression),
                        arguments,
                    },
                    span,
                };
            } else if self.consume(&TokenKind::Dot) {
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
                let expression = self.expression()?;
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

    fn if_expression(&mut self, start: Span) -> Result<Expression, Diagnostic> {
        let condition = self.expression()?;
        let consequent = self.block()?;
        self.expect(TokenKind::Else)?;
        let alternative = if self.check(&TokenKind::If) {
            let nested_start = self.advance().span;
            self.if_expression(nested_start)?
        } else if self.check(&TokenKind::LeftBrace) {
            let block = self.block()?;
            Expression {
                span: block.span,
                kind: ExpressionKind::Block(block),
            }
        } else {
            return Err(self.expected("`if` or `{`"));
        };
        let span = start.join(alternative.span);
        Ok(Expression {
            kind: ExpressionKind::If {
                condition: Box::new(condition),
                consequent,
                alternative: Box::new(alternative),
            },
            span,
        })
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
        let subject = self.expression()?;
        self.expect(TokenKind::LeftBrace)?;
        if self.check(&TokenKind::LeftBracket) {
            self.list_match(start, subject)
        } else {
            self.variant_match(start, subject)
        }
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
        if head_name == tail_name {
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
                    tail_name,
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
            "List" => {
                self.expect(TokenKind::Less)?;
                let element = self.type_annotation()?;
                self.expect(TokenKind::Greater)?;
                TypeKind::List(Box::new(element))
            }
            "Option" => {
                self.expect(TokenKind::Less)?;
                let element = self.type_annotation()?;
                self.expect(TokenKind::Greater)?;
                TypeKind::Option(Box::new(element))
            }
            "Result" => {
                self.expect(TokenKind::Less)?;
                let value = self.type_annotation()?;
                self.expect(TokenKind::Comma)?;
                let error = self.type_annotation()?;
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
        self.expect(TokenKind::LeftBrace)?;
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
            "print" | "println" | "Some" | "None" | "Ok" | "Err" | "json_encode" | "json_decode"
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
