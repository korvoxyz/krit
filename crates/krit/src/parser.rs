use std::collections::HashSet;

use crate::{
    Diagnostic, Span,
    ast::{
        BinaryOperator, Block, Expression, ExpressionKind, Program, Statement, StatementKind,
        UnaryOperator, ValueLiteral,
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
        self.expect(TokenKind::Equal)?;
        let value = self.expression()?;
        let end = self.expect(TokenKind::Semicolon)?.span;
        Ok(Statement {
            kind: StatementKind::Let { name, value },
            span: start.join(end),
        })
    }

    fn function_declaration(&mut self) -> Result<Statement, Diagnostic> {
        let start = self.expect(TokenKind::Fn)?.span;
        let (name, _) = self.binding_name()?;
        let parameters = self.parameters()?;
        let body = self.block()?;
        let span = start.join(body.span);
        Ok(Statement {
            kind: StatementKind::Function {
                name,
                parameters,
                body,
            },
            span,
        })
    }

    fn parameters(&mut self) -> Result<Vec<String>, Diagnostic> {
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
                parameters.push(name);

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
        while self.consume(&TokenKind::LeftParen) {
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
        let body = self.block()?;
        let span = start.join(body.span);
        Ok(Expression {
            kind: ExpressionKind::Function { parameters, body },
            span,
        })
    }

    fn match_expression(&mut self, start: Span) -> Result<Expression, Diagnostic> {
        let subject = self.expression()?;
        self.expect(TokenKind::LeftBrace)?;

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
                empty_case: Box::new(empty_case),
                head_name,
                tail_name,
                cons_case: Box::new(cons_case),
            },
            span: start.join(end),
        })
    }

    fn binding_name(&mut self) -> Result<(String, Span), Diagnostic> {
        let token = self.advance().clone();
        let TokenKind::Identifier(name) = token.kind else {
            return Err(Diagnostic::new(
                "K1002",
                format!("expected identifier, found {}", token.kind.description()),
                token.span,
            ));
        };
        if matches!(name.as_str(), "print" | "println") {
            return Err(Diagnostic::new(
                "K2002",
                format!("`{name}` is a reserved built-in name"),
                token.span,
            ));
        }
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
}
