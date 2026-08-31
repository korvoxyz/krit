use std::{fmt, io::Write, rc::Rc};

use crate::{
    Diagnostic, Span,
    ast::{
        BinaryOperator, Block, Expression, ExpressionKind, Program, Statement, StatementKind,
        UnaryOperator, ValueLiteral,
    },
};

#[derive(Clone)]
pub enum Value {
    Integer(i64),
    Boolean(bool),
    String(Rc<str>),
    Unit,
    List(Rc<[Value]>),
    Function(Rc<FunctionValue>),
    Builtin(BuiltinFunction),
}

#[derive(Clone)]
pub struct FunctionValue {
    name: Option<Rc<str>>,
    parameters: Rc<[String]>,
    body: Block,
    environment: Environment,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BuiltinFunction {
    Print,
    Println,
}

impl fmt::Debug for Value {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.render())
    }
}

impl Value {
    pub fn render(&self) -> String {
        match self {
            Self::Integer(value) => value.to_string(),
            Self::Boolean(value) => value.to_string(),
            Self::String(value) => value.to_string(),
            Self::Unit => "()".to_owned(),
            Self::List(values) => {
                let contents = values
                    .iter()
                    .map(Self::render_nested)
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("[{contents}]")
            }
            Self::Function(function) => function.name.as_deref().map_or_else(
                || "<function>".to_owned(),
                |name| format!("<function {name}>"),
            ),
            Self::Builtin(BuiltinFunction::Print) => "<function print>".to_owned(),
            Self::Builtin(BuiltinFunction::Println) => "<function println>".to_owned(),
        }
    }

    fn render_nested(&self) -> String {
        match self {
            Self::String(value) => format!("{value:?}"),
            _ => self.render(),
        }
    }

    fn kind(&self) -> &'static str {
        match self {
            Self::Integer(_) => "integer",
            Self::Boolean(_) => "boolean",
            Self::String(_) => "string",
            Self::Unit => "unit",
            Self::List(_) => "list",
            Self::Function(_) | Self::Builtin(_) => "function",
        }
    }

    fn contains_function(&self) -> bool {
        match self {
            Self::Function(_) | Self::Builtin(_) => true,
            Self::List(values) => values.iter().any(Self::contains_function),
            _ => false,
        }
    }
}

#[derive(Clone)]
struct Environment(Option<Rc<Binding>>);

struct Binding {
    name: String,
    value: Value,
    parent: Environment,
}

impl Environment {
    const fn empty() -> Self {
        Self(None)
    }

    fn extend(&self, name: String, value: Value) -> Self {
        Self(Some(Rc::new(Binding {
            name,
            value,
            parent: self.clone(),
        })))
    }

    fn lookup(&self, name: &str) -> Option<Value> {
        let mut current = self.0.as_ref().map(Rc::clone);
        while let Some(binding) = current {
            if binding.name == name {
                return Some(binding.value.clone());
            }
            current = binding.parent.0.as_ref().map(Rc::clone);
        }
        None
    }
}

pub fn execute(program: &Program, output: &mut dyn Write) -> Result<Value, Diagnostic> {
    Evaluator { output }.program(program)
}

struct Evaluator<'a> {
    output: &'a mut dyn Write,
}

impl Evaluator<'_> {
    fn program(&mut self, program: &Program) -> Result<Value, Diagnostic> {
        let mut environment = Environment::empty();
        for statement in &program.statements {
            environment = self.statement(statement, &environment)?;
        }
        Ok(Value::Unit)
    }

    fn statement(
        &mut self,
        statement: &Statement,
        environment: &Environment,
    ) -> Result<Environment, Diagnostic> {
        match &statement.kind {
            StatementKind::Let { name, value } => {
                let value = self.expression(value, environment)?;
                Ok(environment.extend(name.clone(), value))
            }
            StatementKind::Function {
                name,
                parameters,
                body,
            } => {
                let function = Value::Function(Rc::new(FunctionValue {
                    name: Some(Rc::from(name.as_str())),
                    parameters: parameters.clone().into(),
                    body: body.clone(),
                    environment: environment.clone(),
                }));
                Ok(environment.extend(name.clone(), function))
            }
            StatementKind::Expression(expression) => {
                self.expression(expression, environment)?;
                Ok(environment.clone())
            }
        }
    }

    fn block(&mut self, block: &Block, environment: &Environment) -> Result<Value, Diagnostic> {
        let mut local = environment.clone();
        for statement in &block.statements {
            local = self.statement(statement, &local)?;
        }
        block
            .tail
            .as_deref()
            .map_or(Ok(Value::Unit), |tail| self.expression(tail, &local))
    }

    fn expression(
        &mut self,
        expression: &Expression,
        environment: &Environment,
    ) -> Result<Value, Diagnostic> {
        match &expression.kind {
            ExpressionKind::Literal(literal) => self.literal(literal, expression.span),
            ExpressionKind::Variable(name) => match name.as_str() {
                "print" => Ok(Value::Builtin(BuiltinFunction::Print)),
                "println" => Ok(Value::Builtin(BuiltinFunction::Println)),
                _ => environment.lookup(name).ok_or_else(|| {
                    Diagnostic::new("K2001", format!("undefined name `{name}`"), expression.span)
                }),
            },
            ExpressionKind::List(elements) => {
                let values = elements
                    .iter()
                    .map(|element| self.expression(element, environment))
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(Value::List(values.into()))
            }
            ExpressionKind::Block(block) => self.block(block, environment),
            ExpressionKind::If {
                condition,
                consequent,
                alternative,
            } => {
                let condition_value = self.expression(condition, environment)?;
                if self.boolean(condition_value, condition.span)? {
                    self.block(consequent, environment)
                } else {
                    self.expression(alternative, environment)
                }
            }
            ExpressionKind::Function { parameters, body } => {
                Ok(Value::Function(Rc::new(FunctionValue {
                    name: None,
                    parameters: parameters.clone().into(),
                    body: body.clone(),
                    environment: environment.clone(),
                })))
            }
            ExpressionKind::Call { callee, arguments } => {
                self.call(callee, arguments, environment, expression.span)
            }
            ExpressionKind::Match {
                subject,
                empty_case,
                head_name,
                tail_name,
                cons_case,
            } => {
                let subject_value = self.expression(subject, environment)?;
                let values = self.list(subject_value, subject.span)?;
                if values.is_empty() {
                    self.expression(empty_case, environment)
                } else {
                    let with_head = environment.extend(head_name.clone(), values[0].clone());
                    let with_tail = with_head
                        .extend(tail_name.clone(), Value::List(values[1..].to_vec().into()));
                    self.expression(cons_case, &with_tail)
                }
            }
            ExpressionKind::Unary { operator, operand } => {
                self.unary(*operator, operand, environment, expression.span)
            }
            ExpressionKind::Binary {
                left,
                operator,
                right,
            } => self.binary(*operator, left, right, environment, expression.span),
        }
    }

    fn literal(&self, literal: &ValueLiteral, span: Span) -> Result<Value, Diagnostic> {
        match literal {
            ValueLiteral::Integer(value) => i64::try_from(*value)
                .map(Value::Integer)
                .map_err(|_| Diagnostic::new("K4005", "integer literal is out of range", span)),
            ValueLiteral::Boolean(value) => Ok(Value::Boolean(*value)),
            ValueLiteral::String(value) => Ok(Value::String(Rc::from(value.as_str()))),
        }
    }

    fn call(
        &mut self,
        callee: &Expression,
        arguments: &[Expression],
        environment: &Environment,
        span: Span,
    ) -> Result<Value, Diagnostic> {
        let callee_value = self.expression(callee, environment)?;
        match callee_value {
            Value::Function(function) => {
                if arguments.len() != function.parameters.len() {
                    return Err(arity_error(
                        function.parameters.len(),
                        arguments.len(),
                        span,
                    ));
                }

                let values = arguments
                    .iter()
                    .map(|argument| self.expression(argument, environment))
                    .collect::<Result<Vec<_>, _>>()?;
                let mut call_environment = function.environment.clone();
                if let Some(name) = &function.name {
                    call_environment = call_environment
                        .extend(name.to_string(), Value::Function(Rc::clone(&function)));
                }
                for (parameter, value) in function.parameters.iter().zip(values) {
                    call_environment = call_environment.extend(parameter.clone(), value);
                }
                self.block(&function.body, &call_environment)
            }
            Value::Builtin(builtin) => {
                if arguments.len() != 1 {
                    return Err(arity_error(1, arguments.len(), span));
                }
                let value = self.expression(&arguments[0], environment)?;
                let rendered = value.render();
                self.output
                    .write_all(rendered.as_bytes())
                    .map_err(|error| {
                        Diagnostic::new("K4007", format!("output failed: {error}"), span)
                    })?;
                if builtin == BuiltinFunction::Println {
                    self.output.write_all(b"\n").map_err(|error| {
                        Diagnostic::new("K4007", format!("output failed: {error}"), span)
                    })?;
                }
                Ok(Value::Unit)
            }
            value => Err(Diagnostic::new(
                "K4002",
                format!("cannot call {}", value.kind()),
                callee.span,
            )),
        }
    }

    fn unary(
        &mut self,
        operator: UnaryOperator,
        operand: &Expression,
        environment: &Environment,
        span: Span,
    ) -> Result<Value, Diagnostic> {
        if operator == UnaryOperator::Negate
            && let ExpressionKind::Literal(ValueLiteral::Integer(value)) = &operand.kind
        {
            let negated = value.checked_neg().ok_or_else(|| {
                Diagnostic::new("K4005", "integer overflow during negation", span)
            })?;
            return i64::try_from(negated)
                .map(Value::Integer)
                .map_err(|_| Diagnostic::new("K4005", "integer overflow during negation", span));
        }

        let value = self.expression(operand, environment)?;
        match operator {
            UnaryOperator::Not => Ok(Value::Boolean(!self.boolean(value, span)?)),
            UnaryOperator::Negate => self
                .integer(value, span)?
                .checked_neg()
                .map(Value::Integer)
                .ok_or_else(|| Diagnostic::new("K4005", "integer overflow during negation", span)),
        }
    }

    fn binary(
        &mut self,
        operator: BinaryOperator,
        left: &Expression,
        right: &Expression,
        environment: &Environment,
        span: Span,
    ) -> Result<Value, Diagnostic> {
        if operator == BinaryOperator::And {
            let left = self.expression(left, environment)?;
            return if self.boolean(left, span)? {
                let right = self.expression(right, environment)?;
                Ok(Value::Boolean(self.boolean(right, span)?))
            } else {
                Ok(Value::Boolean(false))
            };
        }
        if operator == BinaryOperator::Or {
            let left = self.expression(left, environment)?;
            return if self.boolean(left, span)? {
                Ok(Value::Boolean(true))
            } else {
                let right = self.expression(right, environment)?;
                Ok(Value::Boolean(self.boolean(right, span)?))
            };
        }

        let left_value = self.expression(left, environment)?;
        let right_value = self.expression(right, environment)?;
        match operator {
            BinaryOperator::Add => match (left_value, right_value) {
                (Value::Integer(left), Value::Integer(right)) => left
                    .checked_add(right)
                    .map(Value::Integer)
                    .ok_or_else(|| Diagnostic::new("K4005", "integer overflow", span)),
                (Value::String(left), Value::String(right)) => {
                    Ok(Value::String(Rc::from(format!("{left}{right}"))))
                }
                (left, right) => Err(kind_pair_error("`+`", &left, &right, span)),
            },
            BinaryOperator::Subtract => {
                checked_integer_operation(left_value, right_value, span, i64::checked_sub)
            }
            BinaryOperator::Multiply => {
                checked_integer_operation(left_value, right_value, span, i64::checked_mul)
            }
            BinaryOperator::Divide => {
                let (left, right) = self.integer_pair(left_value, right_value, span)?;
                if right == 0 {
                    return Err(Diagnostic::new("K4004", "division by zero", span));
                }
                left.checked_div(right)
                    .map(Value::Integer)
                    .ok_or_else(|| Diagnostic::new("K4005", "integer overflow", span))
            }
            BinaryOperator::Remainder => {
                let (left, right) = self.integer_pair(left_value, right_value, span)?;
                if right == 0 {
                    return Err(Diagnostic::new("K4004", "remainder by zero", span));
                }
                left.checked_rem(right)
                    .map(Value::Integer)
                    .ok_or_else(|| Diagnostic::new("K4005", "integer overflow", span))
            }
            BinaryOperator::Equal | BinaryOperator::NotEqual => {
                let equal = value_equal(&left_value, &right_value, span)?;
                Ok(Value::Boolean(if operator == BinaryOperator::Equal {
                    equal
                } else {
                    !equal
                }))
            }
            BinaryOperator::Less
            | BinaryOperator::LessEqual
            | BinaryOperator::Greater
            | BinaryOperator::GreaterEqual => {
                let (left, right) = self.integer_pair(left_value, right_value, span)?;
                let result = match operator {
                    BinaryOperator::Less => left < right,
                    BinaryOperator::LessEqual => left <= right,
                    BinaryOperator::Greater => left > right,
                    BinaryOperator::GreaterEqual => left >= right,
                    _ => unreachable!("operator was matched above"),
                };
                Ok(Value::Boolean(result))
            }
            BinaryOperator::And | BinaryOperator::Or => {
                unreachable!("short-circuit operators returned above")
            }
        }
    }

    fn boolean(&self, value: Value, span: Span) -> Result<bool, Diagnostic> {
        match value {
            Value::Boolean(value) => Ok(value),
            value => Err(Diagnostic::new(
                "K4001",
                format!("expected boolean, found {}", value.kind()),
                span,
            )),
        }
    }

    fn integer(&self, value: Value, span: Span) -> Result<i64, Diagnostic> {
        match value {
            Value::Integer(value) => Ok(value),
            value => Err(Diagnostic::new(
                "K4001",
                format!("expected integer, found {}", value.kind()),
                span,
            )),
        }
    }

    fn integer_pair(
        &self,
        left: Value,
        right: Value,
        span: Span,
    ) -> Result<(i64, i64), Diagnostic> {
        match (left, right) {
            (Value::Integer(left), Value::Integer(right)) => Ok((left, right)),
            (left, right) => Err(kind_pair_error("integer operator", &left, &right, span)),
        }
    }

    fn list(&self, value: Value, span: Span) -> Result<Rc<[Value]>, Diagnostic> {
        match value {
            Value::List(values) => Ok(values),
            value => Err(Diagnostic::new(
                "K4001",
                format!("expected list, found {}", value.kind()),
                span,
            )),
        }
    }
}

fn arity_error(expected: usize, actual: usize, span: Span) -> Diagnostic {
    Diagnostic::new(
        "K4003",
        format!("function expects {expected} argument(s), found {actual}"),
        span,
    )
}

fn kind_pair_error(operation: &str, left: &Value, right: &Value, span: Span) -> Diagnostic {
    Diagnostic::new(
        "K4001",
        format!(
            "{operation} does not accept {} and {}",
            left.kind(),
            right.kind()
        ),
        span,
    )
}

fn checked_integer_operation(
    left: Value,
    right: Value,
    span: Span,
    operation: fn(i64, i64) -> Option<i64>,
) -> Result<Value, Diagnostic> {
    let (Value::Integer(left), Value::Integer(right)) = (&left, &right) else {
        return Err(kind_pair_error("integer operator", &left, &right, span));
    };
    operation(*left, *right)
        .map(Value::Integer)
        .ok_or_else(|| Diagnostic::new("K4005", "integer overflow", span))
}

fn value_equal(left: &Value, right: &Value, span: Span) -> Result<bool, Diagnostic> {
    if left.contains_function() || right.contains_function() {
        return Err(Diagnostic::new(
            "K4006",
            "functions cannot be compared",
            span,
        ));
    }
    match (left, right) {
        (Value::Integer(left), Value::Integer(right)) => Ok(left == right),
        (Value::Boolean(left), Value::Boolean(right)) => Ok(left == right),
        (Value::String(left), Value::String(right)) => Ok(left == right),
        (Value::Unit, Value::Unit) => Ok(true),
        (Value::List(left), Value::List(right)) => {
            if left.len() != right.len() {
                return Ok(false);
            }
            for (left, right) in left.iter().zip(right.iter()) {
                if !value_equal(left, right, span)? {
                    return Ok(false);
                }
            }
            Ok(true)
        }
        _ => Ok(false),
    }
}

#[cfg(test)]
mod tests {
    use crate::{Diagnostic, Source, Value, run_source};

    fn run(text: &str) -> Result<(String, Value), Diagnostic> {
        let source = Source::new("test.krit", text);
        let mut output = Vec::new();
        let value = run_source(&source, &mut output)?;
        Ok((
            String::from_utf8(output).expect("output should be UTF-8"),
            value,
        ))
    }

    #[test]
    fn evaluates_closures_recursion_and_lists() {
        let (output, _) = run(r#"
            fn sum(items) {
                match items {
                    [] => 0,
                    [head, ..tail] => head + sum(tail),
                }
            }
            let offset = 2;
            let add_offset = fn(value) { value + offset };
            println(add_offset(sum([10, 20, 10])));
            "#)
        .expect("program should run");

        assert_eq!(output, "42\n");
    }

    #[test]
    fn detects_checked_overflow() {
        let error = run("println(9223372036854775807 + 1);").expect_err("program should fail");
        assert_eq!(error.code(), "K4005");
    }

    #[test]
    fn renders_strings_unambiguously_inside_lists() {
        let (output, _) = run(r#"println(["human", "AI"]);"#).expect("program should run");
        assert_eq!(output, "[\"human\", \"AI\"]\n");
    }
}
