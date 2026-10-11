use std::{fmt, io::Write, rc::Rc};

use crate::{
    Diagnostic, Span,
    ast::{
        BinaryOperator, Block, Expression, ExpressionKind, MatchKind, Parameter, Program,
        Statement, StatementKind, UnaryOperator, ValueLiteral, VariantFamily, VariantName,
    },
    limits::{MAX_CALL_DEPTH, MAX_EVALUATION_DEPTH},
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
    AiInvoke,
    Print,
    Println,
    Some,
    Ok,
    Err,
    JsonEncode,
    JsonDecode,
    ConfigString,
    Secret,
    HttpRequest,
    LogInfo,
    LogError,
    StateGet,
    StatePut,
    StateDelete,
    CheckpointGet,
    CheckpointPut,
    ReplayHttp,
    ReplayAi,
    QueuePublish,
    CacheGet,
    CachePut,
    CacheDelete,
    SearchQuery,
    VectorSearch,
    ObjectGet,
    ObjectPut,
    ObjectDelete,
    DatabaseBeginRead,
    DatabaseBeginWrite,
    DatabaseQuery,
    DatabaseExecute,
    DatabaseCommit,
    DatabaseRollback,
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
            Self::Builtin(BuiltinFunction::AiInvoke) => "<function ai_invoke>".to_owned(),
            Self::Builtin(BuiltinFunction::Print) => "<function print>".to_owned(),
            Self::Builtin(BuiltinFunction::Println) => "<function println>".to_owned(),
            Self::Builtin(BuiltinFunction::Some) => "<function Some>".to_owned(),
            Self::Builtin(BuiltinFunction::Ok) => "<function Ok>".to_owned(),
            Self::Builtin(BuiltinFunction::Err) => "<function Err>".to_owned(),
            Self::Builtin(BuiltinFunction::JsonEncode) => "<function json_encode>".to_owned(),
            Self::Builtin(BuiltinFunction::JsonDecode) => "<function json_decode>".to_owned(),
            Self::Builtin(BuiltinFunction::ConfigString) => "<function config_string>".to_owned(),
            Self::Builtin(BuiltinFunction::Secret) => "<function secret>".to_owned(),
            Self::Builtin(BuiltinFunction::HttpRequest) => "<function http_request>".to_owned(),
            Self::Builtin(BuiltinFunction::LogInfo) => "<function log_info>".to_owned(),
            Self::Builtin(BuiltinFunction::LogError) => "<function log_error>".to_owned(),
            Self::Builtin(BuiltinFunction::StateGet) => "<function state_get>".to_owned(),
            Self::Builtin(BuiltinFunction::StatePut) => "<function state_put>".to_owned(),
            Self::Builtin(BuiltinFunction::StateDelete) => "<function state_delete>".to_owned(),
            Self::Builtin(BuiltinFunction::CheckpointGet) => "<function checkpoint_get>".to_owned(),
            Self::Builtin(BuiltinFunction::CheckpointPut) => "<function checkpoint_put>".to_owned(),
            Self::Builtin(BuiltinFunction::ReplayHttp) => "<function replay_http>".to_owned(),
            Self::Builtin(BuiltinFunction::ReplayAi) => "<function replay_ai>".to_owned(),
            Self::Builtin(BuiltinFunction::QueuePublish) => "<function queue_publish>".to_owned(),
            Self::Builtin(BuiltinFunction::CacheGet) => "<function cache_get>".to_owned(),
            Self::Builtin(BuiltinFunction::CachePut) => "<function cache_put>".to_owned(),
            Self::Builtin(BuiltinFunction::CacheDelete) => "<function cache_delete>".to_owned(),
            Self::Builtin(BuiltinFunction::SearchQuery) => "<function search_query>".to_owned(),
            Self::Builtin(BuiltinFunction::VectorSearch) => "<function vector_search>".to_owned(),
            Self::Builtin(BuiltinFunction::ObjectGet) => "<function object_get>".to_owned(),
            Self::Builtin(BuiltinFunction::ObjectPut) => "<function object_put>".to_owned(),
            Self::Builtin(BuiltinFunction::ObjectDelete) => "<function object_delete>".to_owned(),
            Self::Builtin(BuiltinFunction::DatabaseBeginRead) => {
                "<function db_begin_read>".to_owned()
            }
            Self::Builtin(BuiltinFunction::DatabaseBeginWrite) => {
                "<function db_begin_write>".to_owned()
            }
            Self::Builtin(BuiltinFunction::DatabaseQuery) => "<function db_query>".to_owned(),
            Self::Builtin(BuiltinFunction::DatabaseExecute) => "<function db_execute>".to_owned(),
            Self::Builtin(BuiltinFunction::DatabaseCommit) => "<function db_commit>".to_owned(),
            Self::Builtin(BuiltinFunction::DatabaseRollback) => "<function db_rollback>".to_owned(),
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

impl Drop for Environment {
    fn drop(&mut self) {
        // Release uniquely owned bindings one at a time, so dropping a scope
        // with many bindings does not recurse once per binding.
        let mut next = self.0.take();
        while let Some(binding) = next {
            next = match Rc::try_unwrap(binding) {
                Ok(mut binding) => binding.parent.0.take(),
                Err(_) => None,
            };
        }
    }
}

/// Evaluates a parsed program directly.
///
/// Evaluation fails with `K4010` once more than [`MAX_CALL_DEPTH`] function
/// calls or [`MAX_EVALUATION_DEPTH`] nested evaluations are active. Callers
/// that run untrusted programs must give the current thread at least
/// [`crate::EVALUATOR_STACK_BYTES`] of stack so those limits are always
/// reached before the native stack is exhausted.
pub fn execute(program: &Program, output: &mut dyn Write) -> Result<Value, Diagnostic> {
    Evaluator {
        output,
        depth: 0,
        calls: 0,
    }
    .program(program)
}

struct Evaluator<'a> {
    output: &'a mut dyn Write,
    /// Nested expression and block evaluations currently active.
    depth: usize,
    /// Function calls currently active.
    calls: usize,
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
            }
            | StatementKind::Webhook {
                name,
                parameters,
                body,
                ..
            }
            | StatementKind::QueueConsumer {
                name,
                parameters,
                body,
                ..
            }
            | StatementKind::ScheduleHandler {
                name,
                parameters,
                body,
                ..
            } => {
                let function = closure(Some(name), parameters, body, environment);
                Ok(environment.extend(name.clone(), function))
            }
            StatementKind::Expression(expression) => {
                self.expression(expression, environment)?;
                Ok(environment.clone())
            }
        }
    }

    fn enter(&mut self, span: Span) -> Result<(), Diagnostic> {
        if self.depth >= MAX_EVALUATION_DEPTH {
            return Err(Diagnostic::new(
                "K4010",
                format!(
                    "evaluation depth limit exceeded: more than {MAX_EVALUATION_DEPTH} nested \
                     evaluations"
                ),
                span,
            ));
        }
        self.depth += 1;
        Ok(())
    }

    fn block(&mut self, block: &Block, environment: &Environment) -> Result<Value, Diagnostic> {
        self.enter(block.span)?;
        let value = self.block_contents(block, environment);
        self.depth -= 1;
        value
    }

    fn block_contents(
        &mut self,
        block: &Block,
        environment: &Environment,
    ) -> Result<Value, Diagnostic> {
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
        self.enter(expression.span)?;
        let value = self.expression_contents(expression, environment);
        self.depth -= 1;
        value
    }

    fn expression_contents(
        &mut self,
        expression: &Expression,
        environment: &Environment,
    ) -> Result<Value, Diagnostic> {
        match &expression.kind {
            ExpressionKind::Literal(literal) => self.literal(literal, expression.span),
            ExpressionKind::Variable(name) => variable(name, environment, expression.span),
            // Operands on the recursive evaluation path are collected with
            // plain loops: iterator adapters add several native frames per
            // nested evaluation in unoptimized builds.
            ExpressionKind::List(elements) => {
                let mut values = Vec::with_capacity(elements.len());
                for element in elements {
                    values.push(self.expression(element, environment)?);
                }
                Ok(Value::List(values.into()))
            }
            ExpressionKind::Record(fields) => {
                let mut values = Vec::with_capacity(fields.len());
                for field in fields {
                    values.push((
                        field.name.clone(),
                        self.expression(&field.value, environment)?,
                    ));
                }
                Ok(Value::Record(values.into()))
            }
            ExpressionKind::FieldAccess { value, field } => {
                let value = self.expression(value, environment)?;
                field_value(value, field, expression.span)
            }
            ExpressionKind::Block(block) => self.block(block, environment),
            ExpressionKind::If { .. } => self.if_chain(expression, environment),
            ExpressionKind::Function {
                parameters, body, ..
            } => Ok(closure(None, parameters, body, environment)),
            ExpressionKind::Call { callee, arguments } => {
                self.call(callee, arguments, environment, expression.span)
            }
            ExpressionKind::Match { subject, kind } => {
                self.match_expression(subject, kind, environment)
            }
            ExpressionKind::Unary { operator, operand } => {
                self.unary(*operator, operand, environment, expression.span)
            }
            ExpressionKind::Binary { .. } => self.binary_chain(expression, environment),
        }
    }

    fn match_expression(
        &mut self,
        subject: &Expression,
        kind: &MatchKind,
        environment: &Environment,
    ) -> Result<Value, Diagnostic> {
        let subject_value = self.expression(subject, environment)?;
        let (arm, arm_environment) =
            select_match_arm(subject_value, subject.span, kind, environment)?;
        self.expression(arm, &arm_environment)
    }

    /// Evaluates an `if` expression and its `else if` chain iteratively, so a
    /// long chain does not deepen evaluation.
    fn if_chain(
        &mut self,
        expression: &Expression,
        environment: &Environment,
    ) -> Result<Value, Diagnostic> {
        let mut current = expression;
        loop {
            let ExpressionKind::If {
                condition,
                consequent,
                alternative,
            } = &current.kind
            else {
                return self.expression(current, environment);
            };
            let condition_value = self.expression(condition, environment)?;
            if expect_boolean(condition_value, condition.span)? {
                return self.block(consequent, environment);
            }
            current = alternative;
        }
    }

    /// Evaluates a left-nested chain of binary operators such as `a + b + c`
    /// iteratively, left to right, so a long chain does not deepen evaluation.
    fn binary_chain(
        &mut self,
        expression: &Expression,
        environment: &Environment,
    ) -> Result<Value, Diagnostic> {
        let mut spine = Vec::new();
        let mut current = expression;
        while let ExpressionKind::Binary { left, .. } = &current.kind {
            spine.push(current);
            current = left;
        }
        let mut value = self.expression(current, environment)?;
        while let Some(node) = spine.pop() {
            let ExpressionKind::Binary {
                operator, right, ..
            } = &node.kind
            else {
                unreachable!("the binary spine holds only binary expressions")
            };
            value = self.binary(*operator, value, right, environment, node.span)?;
        }
        Ok(value)
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
                self.call_function(&function, arguments, environment, span)
            }
            Value::Builtin(builtin) => self.call_builtin(builtin, arguments, environment, span),
            value => Err(not_callable(&value, callee.span)),
        }
    }

    fn call_function(
        &mut self,
        function: &Rc<FunctionValue>,
        arguments: &[Expression],
        environment: &Environment,
        span: Span,
    ) -> Result<Value, Diagnostic> {
        if arguments.len() != function.parameters.len() {
            return Err(arity_error(
                function.parameters.len(),
                arguments.len(),
                span,
            ));
        }

        let mut values = Vec::with_capacity(arguments.len());
        for argument in arguments {
            values.push(self.expression(argument, environment)?);
        }
        let mut call_environment = function.environment.clone();
        if let Some(name) = &function.name {
            call_environment =
                call_environment.extend(name.to_string(), Value::Function(Rc::clone(function)));
        }
        for (parameter, value) in function.parameters.iter().zip(values) {
            call_environment = call_environment.extend(parameter.clone(), value);
        }
        if self.calls >= MAX_CALL_DEPTH {
            return Err(Diagnostic::new(
                "K4010",
                format!(
                    "call depth limit exceeded: more than {MAX_CALL_DEPTH} nested function calls"
                ),
                span,
            ));
        }
        self.calls += 1;
        let value = self.block(&function.body, &call_environment);
        self.calls -= 1;
        value
    }

    fn call_builtin(
        &mut self,
        builtin: BuiltinFunction,
        arguments: &[Expression],
        environment: &Environment,
        span: Span,
    ) -> Result<Value, Diagnostic> {
        let expected_arity = builtin_arity(builtin);
        if arguments.len() != expected_arity {
            return Err(arity_error(expected_arity, arguments.len(), span));
        }
        if let Some(message) = unavailable_builtin_message(builtin) {
            for argument in arguments {
                self.expression(argument, environment)?;
            }
            return Err(Diagnostic::new("K5003", message, span));
        }
        let value = self.expression(&arguments[0], environment)?;
        self.apply_builtin(builtin, value, span)
    }

    /// Applies a pure or output built-in to its evaluated argument.
    fn apply_builtin(
        &mut self,
        builtin: BuiltinFunction,
        value: Value,
        span: Span,
    ) -> Result<Value, Diagnostic> {
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
            BuiltinFunction::AiInvoke
            | BuiltinFunction::ConfigString
            | BuiltinFunction::Secret
            | BuiltinFunction::HttpRequest
            | BuiltinFunction::LogInfo
            | BuiltinFunction::LogError
            | BuiltinFunction::StateGet
            | BuiltinFunction::StatePut
            | BuiltinFunction::StateDelete
            | BuiltinFunction::CheckpointGet
            | BuiltinFunction::CheckpointPut
            | BuiltinFunction::ReplayHttp
            | BuiltinFunction::ReplayAi
            | BuiltinFunction::QueuePublish
            | BuiltinFunction::ObjectGet
            | BuiltinFunction::ObjectPut
            | BuiltinFunction::ObjectDelete
            | BuiltinFunction::DatabaseBeginRead
            | BuiltinFunction::DatabaseBeginWrite
            | BuiltinFunction::DatabaseQuery
            | BuiltinFunction::DatabaseExecute
            | BuiltinFunction::DatabaseCommit
            | BuiltinFunction::DatabaseRollback
            | BuiltinFunction::CacheGet
            | BuiltinFunction::CachePut
            | BuiltinFunction::CacheDelete
            | BuiltinFunction::SearchQuery
            | BuiltinFunction::VectorSearch => {
                unreachable!("host operations are refused before built-in application")
            }
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
            return negate_literal(*value, span);
        }
        let value = self.expression(operand, environment)?;
        apply_unary(operator, value, span)
    }

    fn binary(
        &mut self,
        operator: BinaryOperator,
        left_value: Value,
        right: &Expression,
        environment: &Environment,
        span: Span,
    ) -> Result<Value, Diagnostic> {
        if operator == BinaryOperator::And {
            return if expect_boolean(left_value, span)? {
                let right = self.expression(right, environment)?;
                Ok(Value::Boolean(expect_boolean(right, span)?))
            } else {
                Ok(Value::Boolean(false))
            };
        }
        if operator == BinaryOperator::Or {
            return if expect_boolean(left_value, span)? {
                Ok(Value::Boolean(true))
            } else {
                let right = self.expression(right, environment)?;
                Ok(Value::Boolean(expect_boolean(right, span)?))
            };
        }

        let right_value = self.expression(right, environment)?;
        apply_binary(operator, left_value, right_value, span)
    }
}

/// Resolves a reserved built-in name to its value, or `None` for any other
/// name.
fn builtin_value(name: &str) -> Option<Value> {
    Some(match name {
        "ai_invoke" => Value::Builtin(BuiltinFunction::AiInvoke),
        "print" => Value::Builtin(BuiltinFunction::Print),
        "println" => Value::Builtin(BuiltinFunction::Println),
        "Some" => Value::Builtin(BuiltinFunction::Some),
        "None" => Value::Variant {
            name: VariantName::None,
            payload: None,
        },
        "Ok" => Value::Builtin(BuiltinFunction::Ok),
        "Err" => Value::Builtin(BuiltinFunction::Err),
        "json_encode" => Value::Builtin(BuiltinFunction::JsonEncode),
        "json_decode" => Value::Builtin(BuiltinFunction::JsonDecode),
        "config_string" => Value::Builtin(BuiltinFunction::ConfigString),
        "secret" => Value::Builtin(BuiltinFunction::Secret),
        "http_request" => Value::Builtin(BuiltinFunction::HttpRequest),
        "log_info" => Value::Builtin(BuiltinFunction::LogInfo),
        "log_error" => Value::Builtin(BuiltinFunction::LogError),
        "state_get" => Value::Builtin(BuiltinFunction::StateGet),
        "state_put" => Value::Builtin(BuiltinFunction::StatePut),
        "state_delete" => Value::Builtin(BuiltinFunction::StateDelete),
        "checkpoint_get" => Value::Builtin(BuiltinFunction::CheckpointGet),
        "checkpoint_put" => Value::Builtin(BuiltinFunction::CheckpointPut),
        "replay_http" => Value::Builtin(BuiltinFunction::ReplayHttp),
        "replay_ai" => Value::Builtin(BuiltinFunction::ReplayAi),
        "queue_publish" => Value::Builtin(BuiltinFunction::QueuePublish),
        "cache_get" => Value::Builtin(BuiltinFunction::CacheGet),
        "cache_put" => Value::Builtin(BuiltinFunction::CachePut),
        "cache_delete" => Value::Builtin(BuiltinFunction::CacheDelete),
        "search_query" => Value::Builtin(BuiltinFunction::SearchQuery),
        "vector_search" => Value::Builtin(BuiltinFunction::VectorSearch),
        "object_get" => Value::Builtin(BuiltinFunction::ObjectGet),
        "object_put" => Value::Builtin(BuiltinFunction::ObjectPut),
        "object_delete" => Value::Builtin(BuiltinFunction::ObjectDelete),
        "db_begin_read" => Value::Builtin(BuiltinFunction::DatabaseBeginRead),
        "db_begin_write" => Value::Builtin(BuiltinFunction::DatabaseBeginWrite),
        "db_query" => Value::Builtin(BuiltinFunction::DatabaseQuery),
        "db_execute" => Value::Builtin(BuiltinFunction::DatabaseExecute),
        "db_commit" => Value::Builtin(BuiltinFunction::DatabaseCommit),
        "db_rollback" => Value::Builtin(BuiltinFunction::DatabaseRollback),
        _ => return None,
    })
}

const fn builtin_arity(builtin: BuiltinFunction) -> usize {
    match builtin {
        BuiltinFunction::ReplayHttp | BuiltinFunction::ReplayAi | BuiltinFunction::CachePut => 4,
        BuiltinFunction::HttpRequest
        | BuiltinFunction::StatePut
        | BuiltinFunction::CheckpointPut
        | BuiltinFunction::ObjectPut
        | BuiltinFunction::SearchQuery
        | BuiltinFunction::VectorSearch
        | BuiltinFunction::DatabaseQuery
        | BuiltinFunction::DatabaseExecute => 3,
        BuiltinFunction::AiInvoke
        | BuiltinFunction::LogInfo
        | BuiltinFunction::LogError
        | BuiltinFunction::StateGet
        | BuiltinFunction::StateDelete
        | BuiltinFunction::CheckpointGet
        | BuiltinFunction::QueuePublish
        | BuiltinFunction::ObjectGet
        | BuiltinFunction::CacheGet
        | BuiltinFunction::CacheDelete
        | BuiltinFunction::ObjectDelete => 2,
        _ => 1,
    }
}

/// Returns the `K5003` message for a host built-in that direct source
/// execution refuses after evaluating its arguments, or `None` for a built-in
/// it can apply.
const fn unavailable_builtin_message(builtin: BuiltinFunction) -> Option<&'static str> {
    Some(match builtin {
        BuiltinFunction::HttpRequest => {
            "HTTP host operations are unavailable in direct source execution"
        }
        BuiltinFunction::AiInvoke => "AI invocation is unavailable in direct source execution",
        BuiltinFunction::LogInfo | BuiltinFunction::LogError => {
            "structured logging is unavailable in direct source execution"
        }
        BuiltinFunction::StateGet
        | BuiltinFunction::StatePut
        | BuiltinFunction::StateDelete
        | BuiltinFunction::CheckpointGet
        | BuiltinFunction::CheckpointPut
        | BuiltinFunction::ReplayHttp
        | BuiltinFunction::ReplayAi
        | BuiltinFunction::QueuePublish
        | BuiltinFunction::ObjectGet
        | BuiltinFunction::ObjectPut
        | BuiltinFunction::ObjectDelete
        | BuiltinFunction::DatabaseBeginRead
        | BuiltinFunction::DatabaseBeginWrite
        | BuiltinFunction::DatabaseQuery
        | BuiltinFunction::DatabaseExecute
        | BuiltinFunction::DatabaseCommit
        | BuiltinFunction::DatabaseRollback
        | BuiltinFunction::CacheGet
        | BuiltinFunction::CachePut
        | BuiltinFunction::CacheDelete
        | BuiltinFunction::SearchQuery
        | BuiltinFunction::VectorSearch => {
            "durable state, queue, schedule, object, database, cache, search, and replay \
             operations are unavailable in direct source execution"
        }
        BuiltinFunction::ConfigString => {
            "configuration host operations are unavailable in direct source execution"
        }
        BuiltinFunction::Secret => {
            "secret host operations are unavailable in direct source execution"
        }
        BuiltinFunction::Print
        | BuiltinFunction::Println
        | BuiltinFunction::Some
        | BuiltinFunction::Ok
        | BuiltinFunction::Err
        | BuiltinFunction::JsonEncode
        | BuiltinFunction::JsonDecode => return None,
    })
}

/// Applies a strict binary operator to its evaluated operands.
fn apply_binary(
    operator: BinaryOperator,
    left_value: Value,
    right_value: Value,
    span: Span,
) -> Result<Value, Diagnostic> {
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
            let (left, right) = integer_pair(left_value, right_value, span)?;
            if right == 0 {
                return Err(Diagnostic::new("K4004", "division by zero", span));
            }
            left.checked_div(right)
                .map(Value::Integer)
                .ok_or_else(|| Diagnostic::new("K4005", "integer overflow", span))
        }
        BinaryOperator::Remainder => {
            let (left, right) = integer_pair(left_value, right_value, span)?;
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
            let (left, right) = integer_pair(left_value, right_value, span)?;
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

fn apply_unary(operator: UnaryOperator, value: Value, span: Span) -> Result<Value, Diagnostic> {
    match operator {
        UnaryOperator::Not => Ok(Value::Boolean(!expect_boolean(value, span)?)),
        UnaryOperator::Negate => expect_integer(value, span)?
            .checked_neg()
            .map(Value::Integer)
            .ok_or_else(|| Diagnostic::new("K4005", "integer overflow during negation", span)),
    }
}

/// Negates an integer literal directly, so `-9223372036854775808` is
/// representable even though its magnitude is not.
fn negate_literal(value: i128, span: Span) -> Result<Value, Diagnostic> {
    let negated = value
        .checked_neg()
        .ok_or_else(|| Diagnostic::new("K4005", "integer overflow during negation", span))?;
    i64::try_from(negated)
        .map(Value::Integer)
        .map_err(|_| Diagnostic::new("K4005", "integer overflow during negation", span))
}

fn variable(name: &str, environment: &Environment, span: Span) -> Result<Value, Diagnostic> {
    builtin_value(name)
        .or_else(|| environment.lookup(name))
        .ok_or_else(|| Diagnostic::new("K2001", format!("undefined name `{name}`"), span))
}

fn field_value(value: Value, field: &str, span: Span) -> Result<Value, Diagnostic> {
    let Value::Record(fields) = value else {
        return Err(Diagnostic::new(
            "K4001",
            format!("expected record for field access, found {}", value.kind()),
            span,
        ));
    };
    fields
        .iter()
        .find(|(name, _)| name == field)
        .map(|(_, value)| value.clone())
        .ok_or_else(|| Diagnostic::new("K4001", format!("record has no field `{field}`"), span))
}

fn closure(
    name: Option<&str>,
    parameters: &[Parameter],
    body: &Block,
    environment: &Environment,
) -> Value {
    Value::Function(Rc::new(FunctionValue {
        name: name.map(Rc::from),
        parameters: parameters
            .iter()
            .map(|parameter| parameter.name.clone())
            .collect::<Vec<_>>()
            .into(),
        body: body.clone(),
        environment: environment.clone(),
    }))
}

fn not_callable(value: &Value, span: Span) -> Diagnostic {
    Diagnostic::new("K4002", format!("cannot call {}", value.kind()), span)
}

/// Selects the match arm for an evaluated subject and returns it with the
/// environment that binds its pattern names.
fn select_match_arm<'a>(
    subject_value: Value,
    subject_span: Span,
    kind: &'a MatchKind,
    environment: &Environment,
) -> Result<(&'a Expression, Environment), Diagnostic> {
    match kind {
        MatchKind::List {
            empty_case,
            head_name,
            tail_name,
            cons_case,
        } => {
            let Value::List(values) = subject_value else {
                return Err(Diagnostic::new(
                    "K4001",
                    format!("expected list, found {}", subject_value.kind()),
                    subject_span,
                ));
            };
            if values.is_empty() {
                Ok((empty_case, environment.clone()))
            } else {
                let with_head = environment.extend(head_name.clone(), values[0].clone());
                let with_tail =
                    with_head.extend(tail_name.clone(), Value::List(values[1..].to_vec().into()));
                Ok((cons_case, with_tail))
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
                    subject_span,
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
                    subject_span,
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
            Ok((&arm.value, arm_environment))
        }
    }
}

fn expect_boolean(value: Value, span: Span) -> Result<bool, Diagnostic> {
    match value {
        Value::Boolean(value) => Ok(value),
        value => Err(Diagnostic::new(
            "K4001",
            format!("expected boolean, found {}", value.kind()),
            span,
        )),
    }
}

fn expect_integer(value: Value, span: Span) -> Result<i64, Diagnostic> {
    match value {
        Value::Integer(value) => Ok(value),
        value => Err(Diagnostic::new(
            "K4001",
            format!("expected integer, found {}", value.kind()),
            span,
        )),
    }
}

fn integer_pair(left: Value, right: Value, span: Span) -> Result<(i64, i64), Diagnostic> {
    match (left, right) {
        (Value::Integer(left), Value::Integer(right)) => Ok((left, right)),
        (left, right) => Err(kind_pair_error("integer operator", &left, &right, span)),
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
    use crate::{Diagnostic, MAX_CALL_DEPTH, Source, Value, run_source};

    fn run(text: &str) -> Result<(String, Value), Diagnostic> {
        let source = Source::new("test.krit", text);
        let mut output = Vec::new();
        let value = run_source(&source, &mut output)?;
        Ok((
            String::from_utf8(output).expect("output should be UTF-8"),
            value,
        ))
    }

    /// Runs on the stack `krit run` gives the evaluator and returns the output
    /// or the diagnostic code.
    fn run_on_evaluator_stack(text: String) -> Result<String, Diagnostic> {
        crate::run_on_thread("evaluator-test", crate::EVALUATOR_STACK_BYTES, move || {
            run(&text).map(|(output, _)| output)
        })
        .expect("evaluator test thread should start")
    }

    fn count_to(depth: usize) -> String {
        format!(
            "fn count(n) {{ if n == 0 {{ 0 }} else {{ 1 + count(n - 1) }} }}\nprintln(count({depth}));\n"
        )
    }

    #[test]
    fn deep_recursion_within_the_call_limit_succeeds() {
        let output = run_on_evaluator_stack(count_to(9_000)).expect("9,000 calls should run");
        assert_eq!(output, "9000\n");
    }

    #[test]
    fn reports_the_call_depth_limit() {
        let error = run_on_evaluator_stack(count_to(100_000))
            .expect_err("100,000 nested calls should fail");
        assert_eq!(error.code(), "K4010");
        assert!(
            error.message().contains(&MAX_CALL_DEPTH.to_string()),
            "{}",
            error.message()
        );

        // The limit counts active frames, not total calls.
        let error = run_on_evaluator_stack(count_to(MAX_CALL_DEPTH))
            .expect_err("one frame past the limit should fail");
        assert_eq!(error.code(), "K4010");
        let output = run_on_evaluator_stack(format!(
            "{}println(count({}) + count({}));\n",
            count_to(0),
            MAX_CALL_DEPTH - 1,
            MAX_CALL_DEPTH - 1
        ))
        .expect("sequential deep calls should run");
        assert_eq!(output, format!("0\n{}\n", 2 * (MAX_CALL_DEPTH - 1)));
    }

    #[test]
    fn reports_the_evaluation_depth_limit_for_deeply_nested_bodies() {
        // Twenty nested blocks per call reach the evaluation bound well before
        // the call bound.
        let text = format!(
            "fn f(n) {{ if n == 0 {{ 0 }} else {{ {}1 + f(n - 1){} }} }}\nprintln(f(100000));\n",
            "{ ".repeat(20),
            " }".repeat(20)
        );
        let error = run_on_evaluator_stack(text).expect_err("deep evaluation should fail");
        assert_eq!(error.code(), "K4010");
        assert!(
            error.message().contains("evaluation depth"),
            "{}",
            error.message()
        );
    }

    #[test]
    fn evaluates_long_else_if_and_operator_chains_iteratively() {
        // Runs on the default test stack: recursive evaluation of either chain
        // would need far more.
        let mut text = String::from("fn pick(n) { if n == 0 { 0 }");
        for branch in 1..1_000 {
            text.push_str(&format!(" else if n == {branch} {{ {branch} }}"));
        }
        text.push_str(" else { -1 } }\nprintln(pick(999));\nprintln(pick(1000));\n");
        text.push_str(&format!("println(0{});\n", " + 2".repeat(4_000)));
        text.push_str(&format!("println(false{});\n", " || false".repeat(4_000)));
        // Short-circuiting still skips every later operand.
        text.push_str(&format!(
            "println(false{});\n",
            " && 1 / 0 == 0".repeat(4_000)
        ));
        let (output, _) = run(&text).expect("long chains should evaluate");
        assert_eq!(output, "999\n-1\n8000\nfalse\nfalse\n");
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
    fn run_source_keeps_annotations_dynamically_inert() {
        let (output, _) = run(r#"
            let value: Int = "still dynamic";
            fn identity(input: Bool) -> Unit { input }
            println(identity(value));
            "#)
        .expect("run_source should remain dynamically compatible");

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
