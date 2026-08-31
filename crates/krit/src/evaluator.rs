use std::{fmt, io::Write, rc::Rc};

use crate::{
    Diagnostic, Span,
    ast::{
        BinaryOperator, Block, Expression, ExpressionKind, MatchKind, Program, Statement,
        StatementKind, UnaryOperator, ValueLiteral, VariantFamily, VariantName,
    },
};

#[derive(Clone)]
pub enum Value {
    Integer(i64),
    Boolean(bool),
    String(Rc<str>),
    Unit,
    List(Rc<[Value]>),
    Record(Rc<[(String, Value)]>),
    Variant {
        name: VariantName,
        payload: Option<Rc<Value>>,
    },
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
    Some,
    Ok,
    Err,
    JsonEncode,
    JsonDecode,
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
            Self::Record(fields) => {
                if fields.is_empty() {
                    return "record {}".to_owned();
                }
                let contents = fields
                    .iter()
                    .map(|(name, value)| format!("{name}: {}", value.render_nested()))
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("record {{ {contents} }}")
            }
            Self::Variant { name, payload } => payload.as_ref().map_or_else(
                || name.as_str().to_owned(),
                |value| format!("{}({})", name.as_str(), value.render_nested()),
            ),
            Self::Function(function) => function.name.as_deref().map_or_else(
                || "<function>".to_owned(),
                |name| format!("<function {name}>"),
            ),
            Self::Builtin(BuiltinFunction::Print) => "<function print>".to_owned(),
            Self::Builtin(BuiltinFunction::Println) => "<function println>".to_owned(),
            Self::Builtin(BuiltinFunction::Some) => "<function Some>".to_owned(),
            Self::Builtin(BuiltinFunction::Ok) => "<function Ok>".to_owned(),
            Self::Builtin(BuiltinFunction::Err) => "<function Err>".to_owned(),
            Self::Builtin(BuiltinFunction::JsonEncode) => "<function json_encode>".to_owned(),
            Self::Builtin(BuiltinFunction::JsonDecode) => "<function json_decode>".to_owned(),
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
            Self::Record(_) => "record",
            Self::Variant { name, .. } => match name {
                VariantName::Some | VariantName::None => "option",
                VariantName::Ok | VariantName::Err => "result",
            },
            Self::Function(_) | Self::Builtin(_) => "function",
        }
    }

    fn contains_function(&self) -> bool {
        match self {
            Self::Function(_) | Self::Builtin(_) => true,
            Self::List(values) => values.iter().any(Self::contains_function),
            Self::Record(fields) => fields.iter().any(|(_, value)| value.contains_function()),
            Self::Variant { payload, .. } => {
                payload.as_deref().is_some_and(Self::contains_function)
            }
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
            StatementKind::Let { name, value, .. } => {
                let value = self.expression(value, environment)?;
                Ok(environment.extend(name.clone(), value))
            }
            StatementKind::Function {
                name,
                parameters,
                body,
                ..
            } => {
                let function = Value::Function(Rc::new(FunctionValue {
                    name: Some(Rc::from(name.as_str())),
                    parameters: parameters
                        .iter()
                        .map(|parameter| parameter.name.clone())
                        .collect::<Vec<_>>()
                        .into(),
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
                "Some" => Ok(Value::Builtin(BuiltinFunction::Some)),
                "None" => Ok(Value::Variant {
                    name: VariantName::None,
                    payload: None,
                }),
                "Ok" => Ok(Value::Builtin(BuiltinFunction::Ok)),
                "Err" => Ok(Value::Builtin(BuiltinFunction::Err)),
                "json_encode" => Ok(Value::Builtin(BuiltinFunction::JsonEncode)),
                "json_decode" => Ok(Value::Builtin(BuiltinFunction::JsonDecode)),
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
            ExpressionKind::Record(fields) => {
                let values = fields
                    .iter()
                    .map(|field| {
                        self.expression(&field.value, environment)
                            .map(|value| (field.name.clone(), value))
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(Value::Record(values.into()))
            }
            ExpressionKind::FieldAccess { value, field } => {
                let value = self.expression(value, environment)?;
                let Value::Record(fields) = value else {
                    return Err(Diagnostic::new(
                        "K4001",
                        format!("expected record for field access, found {}", value.kind()),
                        expression.span,
                    ));
                };
                fields
                    .iter()
                    .find(|(name, _)| name == field)
                    .map(|(_, value)| value.clone())
                    .ok_or_else(|| {
                        Diagnostic::new(
                            "K4001",
                            format!("record has no field `{field}`"),
                            expression.span,
                        )
                    })
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
            ExpressionKind::Function {
                parameters, body, ..
            } => Ok(Value::Function(Rc::new(FunctionValue {
                name: None,
                parameters: parameters
                    .iter()
                    .map(|parameter| parameter.name.clone())
                    .collect::<Vec<_>>()
                    .into(),
                body: body.clone(),
                environment: environment.clone(),
            }))),
            ExpressionKind::Call { callee, arguments } => {
                self.call(callee, arguments, environment, expression.span)
            }
            ExpressionKind::Match { subject, kind } => {
                let subject_value = self.expression(subject, environment)?;
                match kind {
                    MatchKind::List {
                        empty_case,
                        head_name,
                        tail_name,
                        cons_case,
                    } => {
                        let values = self.list(subject_value, subject.span)?;
                        if values.is_empty() {
                            self.expression(empty_case, environment)
                        } else {
                            let with_head =
                                environment.extend(head_name.clone(), values[0].clone());
                            let with_tail = with_head.extend(
                                tail_name.clone(),
                                Value::List(values[1..].to_vec().into()),
                            );
                            self.expression(cons_case, &with_tail)
                        }
                    }
                    MatchKind::Variants { family, arms } => {
                        let Value::Variant { name, payload } = subject_value else {
                            return Err(Diagnostic::new(
                                "K4001",
                                format!(
                                    "expected {} for variant match, found {}",
                                    variant_family_name(*family),
                                    subject_value.kind()
                                ),
                                subject.span,
                            ));
                        };
                        if variant_family(name) != *family {
                            return Err(Diagnostic::new(
                                "K4001",
                                format!(
                                    "expected {} for variant match, found {}",
                                    variant_family_name(*family),
                                    variant_family_name(variant_family(name))
                                ),
                                subject.span,
                            ));
                        }
                        let arm = arms
                            .iter()
                            .find(|arm| arm.variant == name)
                            .expect("parser guarantees exhaustive variant arms");
                        let arm_environment = match (&arm.binding, payload) {
                            (Some(binding), Some(value)) => {
                                environment.extend(binding.clone(), value.as_ref().clone())
                            }
                            (None, None) => environment.clone(),
                            _ => unreachable!("variant payload shape is fixed"),
                        };
                        self.expression(&arm.value, &arm_environment)
                    }
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
                match builtin {
                    BuiltinFunction::Print | BuiltinFunction::Println => {
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
                    BuiltinFunction::Some => Ok(Value::Variant {
                        name: VariantName::Some,
                        payload: Some(Rc::new(value)),
                    }),
                    BuiltinFunction::Ok => Ok(Value::Variant {
                        name: VariantName::Ok,
                        payload: Some(Rc::new(value)),
                    }),
                    BuiltinFunction::Err => Ok(Value::Variant {
                        name: VariantName::Err,
                        payload: Some(Rc::new(value)),
                    }),
                    BuiltinFunction::JsonEncode => json_encode(&value, span),
                    BuiltinFunction::JsonDecode => json_decode(value, span),
                }
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
        (Value::Record(left), Value::Record(right)) => {
            if left.len() != right.len() {
                return Ok(false);
            }
            for (name, left_value) in left.iter() {
                let Some((_, right_value)) =
                    right.iter().find(|(right_name, _)| right_name == name)
                else {
                    return Ok(false);
                };
                if !value_equal(left_value, right_value, span)? {
                    return Ok(false);
                }
            }
            Ok(true)
        }
        (
            Value::Variant {
                name: left_name,
                payload: left_payload,
            },
            Value::Variant {
                name: right_name,
                payload: right_payload,
            },
        ) => {
            if left_name != right_name {
                return Ok(false);
            }
            match (left_payload, right_payload) {
                (None, None) => Ok(true),
                (Some(left), Some(right)) => value_equal(left, right, span),
                _ => Ok(false),
            }
        }
        _ => Ok(false),
    }
}

const fn variant_family(name: VariantName) -> VariantFamily {
    match name {
        VariantName::Some | VariantName::None => VariantFamily::Option,
        VariantName::Ok | VariantName::Err => VariantFamily::Result,
    }
}

const fn variant_family_name(family: VariantFamily) -> &'static str {
    match family {
        VariantFamily::Option => "option",
        VariantFamily::Result => "result",
    }
}

fn json_encode(value: &Value, span: Span) -> Result<Value, Diagnostic> {
    let json = value_to_json(value, span)?;
    serde_json::to_string(&json)
        .map(|json| Value::String(Rc::from(json)))
        .map_err(|error| Diagnostic::new("K4008", format!("JSON encoding failed: {error}"), span))
}

fn value_to_json(value: &Value, span: Span) -> Result<serde_json::Value, Diagnostic> {
    match value {
        Value::Integer(value) => Ok((*value).into()),
        Value::Boolean(value) => Ok((*value).into()),
        Value::String(value) => Ok(value.as_ref().into()),
        Value::Unit => Ok(serde_json::Value::Null),
        Value::List(values) => values
            .iter()
            .map(|value| value_to_json(value, span))
            .collect::<Result<Vec<_>, _>>()
            .map(serde_json::Value::Array),
        Value::Record(fields) => {
            let mut object = serde_json::Map::new();
            let mut fields = fields.iter().collect::<Vec<_>>();
            fields.sort_by(|(left, _), (right, _)| left.cmp(right));
            for (name, value) in fields {
                object.insert(name.clone(), value_to_json(value, span)?);
            }
            Ok(serde_json::Value::Object(object))
        }
        Value::Variant { name, payload } => {
            let mut object = serde_json::Map::new();
            let payload = payload
                .as_deref()
                .map_or(Ok(serde_json::Value::Null), |value| {
                    value_to_json(value, span)
                })?;
            object.insert(name.as_str().to_owned(), payload);
            Ok(serde_json::Value::Object(object))
        }
        Value::Function(_) | Value::Builtin(_) => Err(Diagnostic::new(
            "K4008",
            "functions cannot be encoded as JSON",
            span,
        )),
    }
}

fn json_decode(value: Value, span: Span) -> Result<Value, Diagnostic> {
    let Value::String(text) = value else {
        return Err(Diagnostic::new(
            "K4001",
            format!("expected string, found {}", value.kind()),
            span,
        ));
    };
    let json: serde_json::Value = serde_json::from_str(&text)
        .map_err(|error| Diagnostic::new("K4009", format!("invalid JSON: {error}"), span))?;
    json_to_value(json, span)
}

fn json_to_value(value: serde_json::Value, span: Span) -> Result<Value, Diagnostic> {
    match value {
        serde_json::Value::Null => Ok(Value::Unit),
        serde_json::Value::Bool(value) => Ok(Value::Boolean(value)),
        serde_json::Value::Number(value) => value.as_i64().map(Value::Integer).ok_or_else(|| {
            Diagnostic::new("K4009", "JSON number is not a signed 64-bit integer", span)
        }),
        serde_json::Value::String(value) => Ok(Value::String(Rc::from(value))),
        serde_json::Value::Array(values) => values
            .into_iter()
            .map(|value| json_to_value(value, span))
            .collect::<Result<Vec<_>, _>>()
            .map(|values| Value::List(values.into())),
        serde_json::Value::Object(mut fields) => {
            if fields.len() == 1 {
                for (tag, name) in [
                    ("Some", VariantName::Some),
                    ("None", VariantName::None),
                    ("Ok", VariantName::Ok),
                    ("Err", VariantName::Err),
                ] {
                    if let Some(payload) = fields.remove(tag) {
                        if name == VariantName::None {
                            if !payload.is_null() {
                                return Err(Diagnostic::new(
                                    "K4009",
                                    "the JSON `None` tag must contain null",
                                    span,
                                ));
                            }
                            return Ok(Value::Variant {
                                name,
                                payload: None,
                            });
                        }
                        return Ok(Value::Variant {
                            name,
                            payload: Some(Rc::new(json_to_value(payload, span)?)),
                        });
                    }
                }
            }
            let mut fields = fields
                .into_iter()
                .map(|(name, value)| json_to_value(value, span).map(|value| (name, value)))
                .collect::<Result<Vec<_>, _>>()?;
            fields.sort_by(|(left, _), (right, _)| left.cmp(right));
            Ok(Value::Record(fields.into()))
        }
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

    #[test]
    fn evaluates_records_fields_and_structural_equality() {
        let (output, _) = run(r#"
            let first = record { name: "agent", ready: true };
            let second = record { ready: true, name: "agent" };
            println(first);
            println(first.name);
            println(first == second);
            "#)
        .expect("records should run");

        assert_eq!(
            output,
            "record { name: \"agent\", ready: true }\nagent\ntrue\n"
        );
    }

    #[test]
    fn evaluates_option_and_result_matches() {
        let (output, _) = run(r#"
            let option = Some("ready");
            println(match option { None => "missing", Some(value) => value });
            let result = Err("failed");
            println(match result { Ok(value) => value, Err(error) => error });
            println(Some([1, 2]) == Some([1, 2]));
            "#)
        .expect("variants should run");

        assert_eq!(output, "ready\nfailed\ntrue\n");
    }

    #[test]
    fn functions_nested_in_data_remain_non_comparable() {
        for source in [
            "println(record { callback: fn(value) { value } } == record { callback: fn(value) { value } });",
            "println(Some(fn(value) { value }) == Some(fn(value) { value }));",
            "println(Err(fn(value) { value }) == Err(fn(value) { value }));",
        ] {
            let error = run(source).expect_err("nested functions should not compare");
            assert_eq!(error.code(), "K4006");
        }
    }

    #[test]
    fn annotations_do_not_change_dynamic_evaluation() {
        let (output, _) = run(r#"
            let value: Int = "still dynamic";
            fn identity(input: Bool) -> Unit { input }
            println(identity(value));
            "#)
        .expect("annotations should not be checked yet");

        assert_eq!(output, "still dynamic\n");
    }

    #[test]
    fn encodes_and_decodes_json_deterministically() {
        let (output, _) = run(r#"
            let value = record { z: 2, option: Some([true, None]), a: 1 };
            let encoded = json_encode(value);
            println(encoded);
            println(json_decode(encoded) == value);
            println(json_decode("{\"Err\":\"failed\"}"));
            let unit = {};
            println(json_encode(unit));
            println(json_decode("null") == unit);
            "#)
        .expect("JSON values should round trip");

        assert_eq!(
            output,
            "{\"a\":1,\"option\":{\"Some\":[true,{\"None\":null}]},\"z\":2}\ntrue\nErr(\"failed\")\nnull\ntrue\n"
        );
    }

    #[test]
    fn reports_json_failures() {
        let encode_error =
            run("json_encode(fn(value) { value });").expect_err("functions should not encode");
        assert_eq!(encode_error.code(), "K4008");

        let decode_error = run(r#"json_decode("{");"#).expect_err("invalid JSON should not decode");
        assert_eq!(decode_error.code(), "K4009");
    }
}
