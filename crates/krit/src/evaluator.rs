use std::{fmt, io::Write, ops::Deref, rc::Rc};

use crate::{
    Diagnostic, Span,
    analysis::check_construction_fields,
    ast::{
        BinaryOperator, Block, ConstructField, ContractRecord, Expression, ExpressionKind,
        FieldDefault, MatchKind, Parameter, Program, Statement, StatementKind, UnaryOperator,
        ValueLiteral, VariantFamily, VariantName,
    },
    limits::{MAX_CALL_DEPTH, MAX_EVALUATION_DEPTH, MAX_VALUE_DEPTH},
};

#[derive(Clone)]
pub enum Value {
    Integer(i64),
    Boolean(bool),
    String(Rc<str>),
    Unit,
    List(ListValue),
    Record(RecordValue),
    Variant {
        name: VariantName,
        payload: Option<VariantPayload>,
    },
    Function(Rc<FunctionValue>),
    Builtin(BuiltinFunction),
}

/// The elements of a list value.
///
/// Lists, records, and variants record how deeply they nest when they are
/// built, which is how the evaluator bounds value nesting at
/// [`MAX_VALUE_DEPTH`].
#[derive(Clone)]
pub struct ListValue {
    elements: Rc<[Value]>,
    depth: usize,
}

impl Deref for ListValue {
    type Target = [Value];

    fn deref(&self) -> &[Value] {
        &self.elements
    }
}

/// The fields of a record value, in construction order.
#[derive(Clone)]
pub struct RecordValue {
    fields: Rc<[(String, Value)]>,
    depth: usize,
}

impl Deref for RecordValue {
    type Target = [(String, Value)];

    fn deref(&self) -> &[(String, Value)] {
        &self.fields
    }
}

/// The value carried by `Some`, `Ok`, or `Err`.
#[derive(Clone)]
pub struct VariantPayload {
    value: Rc<Value>,
    /// The nesting depth of the variant that carries this payload.
    depth: usize,
}

impl Deref for VariantPayload {
    type Target = Value;

    fn deref(&self) -> &Value {
        &self.value
    }
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

    /// Returns how many lists, records, and variants nest in this value:
    /// zero for scalars and functions, one for an empty list.
    pub fn depth(&self) -> usize {
        match self {
            Self::List(list) => list.depth,
            Self::Record(record) => record.depth,
            Self::Variant { payload, .. } => payload.as_ref().map_or(1, |payload| payload.depth),
            Self::Integer(_)
            | Self::Boolean(_)
            | Self::String(_)
            | Self::Unit
            | Self::Function(_)
            | Self::Builtin(_) => 0,
        }
    }

    /// Builds a list value; fails with `K4010` at `span` when it would nest
    /// more than [`MAX_VALUE_DEPTH`] levels.
    fn list(elements: Vec<Self>, span: Span) -> Result<Self, Diagnostic> {
        let depth = container_depth(elements.iter(), span)?;
        Ok(Self::List(ListValue {
            elements: elements.into(),
            depth,
        }))
    }

    /// Builds a record value; fails with `K4010` at `span` when it would nest
    /// more than [`MAX_VALUE_DEPTH`] levels.
    fn record(fields: Vec<(String, Self)>, span: Span) -> Result<Self, Diagnostic> {
        let depth = container_depth(fields.iter().map(|(_, value)| value), span)?;
        Ok(Self::Record(RecordValue {
            fields: fields.into(),
            depth,
        }))
    }

    /// Builds `Some`, `Ok`, or `Err` around `payload`; fails with `K4010` at
    /// `span` when it would nest more than [`MAX_VALUE_DEPTH`] levels.
    fn variant(name: VariantName, payload: Self, span: Span) -> Result<Self, Diagnostic> {
        let depth = container_depth(std::iter::once(&payload), span)?;
        Ok(Self::Variant {
            name,
            payload: Some(VariantPayload {
                value: Rc::new(payload),
                depth,
            }),
        })
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
        // with many bindings does not recurse once per binding. A released
        // binding hands the captured scopes of the closures it owns
        // exclusively to the same loop, so a long chain of closures that each
        // capture the previous one does not recurse once per closure either.
        let mut pending = vec![self.0.take()];
        while let Some(mut next) = pending.pop() {
            while let Some(binding) = next {
                next = match Rc::try_unwrap(binding) {
                    Ok(mut binding) => {
                        detach_captured_scopes(&mut binding.value, &mut pending);
                        binding.parent.0.take()
                    }
                    Err(_) => None,
                };
            }
        }
    }
}

/// Moves the captured scope of every closure that `value` owns exclusively
/// into `pending`, so releasing `value` afterwards does not recurse into
/// those scopes.
///
/// This walk follows only lists, records, and variants, which nest at most
/// [`MAX_VALUE_DEPTH`] levels, so its own recursion is bounded.
fn detach_captured_scopes(value: &mut Value, pending: &mut Vec<Option<Rc<Binding>>>) {
    match value {
        Value::Function(function) => {
            if let Some(function) = Rc::get_mut(function) {
                pending.push(function.environment.0.take());
            }
        }
        Value::List(list) => {
            if let Some(elements) = Rc::get_mut(&mut list.elements) {
                for element in elements {
                    detach_captured_scopes(element, pending);
                }
            }
        }
        Value::Record(record) => {
            if let Some(fields) = Rc::get_mut(&mut record.fields) {
                for (_, field) in fields {
                    detach_captured_scopes(field, pending);
                }
            }
        }
        Value::Variant {
            payload: Some(payload),
            ..
        } => {
            if let Some(value) = Rc::get_mut(&mut payload.value) {
                detach_captured_scopes(value, pending);
            }
        }
        Value::Integer(_)
        | Value::Boolean(_)
        | Value::String(_)
        | Value::Unit
        | Value::Variant { payload: None, .. }
        | Value::Builtin(_) => {}
    }
}

/// Evaluates a parsed program directly, without analyzing it.
///
/// `krit run` and [`crate::run_source`] evaluate only programs that pass
/// analysis. The runtime kind, arity, and depth checks here remain so that a
/// program that bypassed analysis still fails with a diagnostic.
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
                Value::list(values, expression.span)
            }
            ExpressionKind::Record(fields) => {
                let mut values = Vec::with_capacity(fields.len());
                for field in fields {
                    values.push((
                        field.name.clone(),
                        self.expression(&field.value, environment)?,
                    ));
                }
                Value::record(values, expression.span)
            }
            ExpressionKind::Construct {
                contract,
                contract_span,
                fields,
            } => self.construction(
                *contract,
                *contract_span,
                fields,
                environment,
                expression.span,
            ),
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

    /// Evaluates a named construction: the written values in source order,
    /// then the record in the contract's field order, with every omitted
    /// field set to its spec-fixed default.
    ///
    /// A checked program always writes a valid field set; a program
    /// evaluated without checking reports the checker's `K3016` instead.
    fn construction(
        &mut self,
        contract: ContractRecord,
        contract_span: Span,
        fields: &[ConstructField],
        environment: &Environment,
        span: Span,
    ) -> Result<Value, Diagnostic> {
        check_construction_fields(contract, contract_span, fields)?;
        let mut written = Vec::with_capacity(fields.len());
        for field in fields {
            written.push((
                field.name.as_str(),
                Some(self.expression(&field.value, environment)?),
            ));
        }
        let mut values = Vec::with_capacity(contract.fields().len());
        for field in contract.fields() {
            let value = written
                .iter_mut()
                .find(|(name, _)| *name == field.name)
                .and_then(|(_, value)| value.take());
            let value = match (value, field.default) {
                (Some(value), _) => value,
                (None, Some(FieldDefault::EmptyString)) => Value::String(Rc::from("")),
                (None, Some(FieldDefault::EmptyList)) => Value::list(Vec::new(), span)?,
                (None, None) => unreachable!("checked constructions write every required field"),
            };
            values.push((field.name.to_owned(), value));
        }
        Value::record(values, span)
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
            BuiltinFunction::Some => Value::variant(VariantName::Some, value, span),
            BuiltinFunction::Ok => Value::variant(VariantName::Ok, value, span),
            BuiltinFunction::Err => Value::variant(VariantName::Err, value, span),
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
            // Strings order byte-lexicographically over their UTF-8
            // encoding: the first differing byte decides, and a proper
            // prefix orders before every longer string it begins.
            let ordering = match (left_value, right_value) {
                (Value::Integer(left), Value::Integer(right)) => left.cmp(&right),
                (Value::String(left), Value::String(right)) => {
                    left.as_bytes().cmp(right.as_bytes())
                }
                (left, right) => {
                    return Err(kind_pair_error("comparison operator", &left, &right, span));
                }
            };
            let result = match operator {
                BinaryOperator::Less => ordering.is_lt(),
                BinaryOperator::LessEqual => ordering.is_le(),
                BinaryOperator::Greater => ordering.is_gt(),
                BinaryOperator::GreaterEqual => ordering.is_ge(),
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

/// Resolves `name` as analysis does: the innermost lexical binding wins, and
/// a built-in applies only where no binding of that name is in scope. A match
/// binder may, for example, shadow a host built-in name that the parser does
/// not reserve.
fn variable(name: &str, environment: &Environment, span: Span) -> Result<Value, Diagnostic> {
    environment
        .lookup(name)
        .or_else(|| builtin_value(name))
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

/// Returns the nesting depth of a list, record, or variant holding
/// `children`, or `K4010` at `span` when it exceeds [`MAX_VALUE_DEPTH`].
fn container_depth<'a>(
    children: impl Iterator<Item = &'a Value>,
    span: Span,
) -> Result<usize, Diagnostic> {
    let depth = 1 + children.map(Value::depth).max().unwrap_or(0);
    if depth > MAX_VALUE_DEPTH {
        return Err(Diagnostic::new(
            "K4010",
            format!(
                "value depth limit exceeded: lists, records, and variants may nest at most \
                 {MAX_VALUE_DEPTH} levels"
            ),
            span,
        ));
    }
    Ok(depth)
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
            ..
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
                // The tail nests no deeper than the list it came from.
                let tail = Value::list(values[1..].to_vec(), subject_span)?;
                let with_tail = with_head.extend(tail_name.clone(), tail);
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
                (Some(binding), Some(payload)) => {
                    environment.extend(binding.clone(), Value::clone(&payload))
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
            .and_then(|values| Value::list(values, span)),
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
                        return Value::variant(name, json_to_value(payload, span)?, span);
                    }
                }
            }
            let mut fields = fields
                .into_iter()
                .map(|(name, value)| json_to_value(value, span).map(|value| (name, value)))
                .collect::<Result<Vec<_>, _>>()?;
            fields.sort_by(|(left, _), (right, _)| left.cmp(right));
            Value::record(fields, span)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::execute;
    use crate::{
        Diagnostic, MAX_CALL_DEPTH, MAX_VALUE_DEPTH, Source, Value, parse_source, run_source,
    };

    type Run = fn(&str) -> Result<(String, Value), Diagnostic>;

    /// Runs `text` as `krit run` does: analysis first, then evaluation.
    fn run(text: &str) -> Result<(String, Value), Diagnostic> {
        let source = Source::new("test.krit", text);
        let mut output = Vec::new();
        let value = run_source(&source, &mut output)?;
        Ok((
            String::from_utf8(output).expect("output should be UTF-8"),
            value,
        ))
    }

    /// Evaluates `text` without analyzing it, which exercises the runtime
    /// checks that guard programs analysis would reject.
    fn run_unchecked(text: &str) -> Result<(String, Value), Diagnostic> {
        let source = Source::new("test.krit", text);
        let program = parse_source(&source)?;
        let mut output = Vec::new();
        let value = execute(&program, &mut output)?;
        Ok((
            String::from_utf8(output).expect("output should be UTF-8"),
            value,
        ))
    }

    /// Runs on the stack `krit run` gives the evaluator and returns the output
    /// or the diagnostic code.
    fn run_on_evaluator_stack(text: String) -> Result<String, Diagnostic> {
        on_evaluator_stack(text, run)
    }

    fn on_evaluator_stack(text: String, run: Run) -> Result<String, Diagnostic> {
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

    /// A program that wraps `0` in `levels` layers of `wrapper`, `chunk` calls
    /// at a time so no call chain approaches the call-depth limit, and then
    /// evaluates `finish` with the result bound to `value`.
    ///
    /// `wrap` has no finite type, so analysis rejects the program and only
    /// unchecked evaluation can build these values.
    fn wrapped_value(wrapper: &str, levels: usize, chunk: usize, finish: &str) -> String {
        let mut text = format!(
            "fn wrap(x, n) {{ if n == 0 {{ x }} else {{ wrap({wrapper}, n - 1) }} }}\nlet value = 0;\n"
        );
        let mut remaining = levels;
        while remaining > 0 {
            let step = remaining.min(chunk);
            text.push_str(&format!("let value = wrap(value, {step});\n"));
            remaining -= step;
        }
        text.push_str(finish);
        text
    }

    #[test]
    fn releases_long_closure_chains_without_recursing() {
        // Each of 50,000 closures captures the previous one, through a
        // parameter and through a record. Releasing them recursively would
        // need far more than this thread's 16 MiB; no call chain is deeper
        // than 100.
        for (wrapper, first) in [
            ("fn() { f() + 1 }", "fn() { 0 }"),
            (
                "record { next: fn() { f.next() + 1 } }",
                "record { next: fn() { 0 } }",
            ),
        ] {
            let mut text = format!(
                "fn chain(f, n) {{ if n == 0 {{ f }} else {{ chain({wrapper}, n - 1) }} }}\n\
                 let last = {{\n    let c0 = {first};\n"
            );
            for link in 1..=500 {
                text.push_str(&format!("    let c{link} = chain(c{}, 100);\n", link - 1));
            }
            text.push_str("    c500\n};\nprintln(1);\n");
            let output = crate::run_on_thread("evaluator-test", 16 * 1024 * 1024, move || {
                run(&text).map(|(output, _)| output)
            })
            .expect("evaluator test thread should start")
            .expect("a long closure chain should run and be released");
            assert_eq!(output, "1\n", "{wrapper}");
        }
    }

    #[test]
    fn values_at_the_depth_limit_print_compare_and_encode() {
        let output = on_evaluator_stack(
            wrapped_value(
                "[x]",
                MAX_VALUE_DEPTH,
                5_000,
                "println(value == value);\nprintln(value);\nprintln(json_encode(value));\n",
            ),
            run_unchecked,
        )
        .expect("values at the depth limit should work");
        let list = format!(
            "{}0{}",
            "[".repeat(MAX_VALUE_DEPTH),
            "]".repeat(MAX_VALUE_DEPTH)
        );
        assert_eq!(output, format!("true\n{list}\n{list}\n"));
    }

    #[test]
    fn reports_the_value_depth_limit_for_every_container() {
        for wrapper in ["[x]", "record { inner: x }", "Some(x)", "Ok(x)", "Err(x)"] {
            // The reviewer's shape, 1,000 sequential calls each nesting the
            // value 9,000 levels deeper, fails during the second call instead
            // of building a value nine million levels deep.
            let error = on_evaluator_stack(
                wrapped_value(wrapper, 1_000 * 9_000, 9_000, "println(value);\n"),
                run_unchecked,
            )
            .expect_err("a nine-million-level value should be rejected");
            assert_eq!(error.code(), "K4010", "{wrapper}");
            assert!(
                error.message().contains("value depth"),
                "{wrapper}: {}",
                error.message()
            );

            let error = on_evaluator_stack(
                wrapped_value(wrapper, MAX_VALUE_DEPTH + 1, 5_000, "println(1);\n"),
                run_unchecked,
            )
            .expect_err("one level past the limit should be rejected");
            assert_eq!(error.code(), "K4010", "{wrapper}");

            // Analysis rejects the program, so `run` never reaches the limit.
            let error = run(&wrapped_value(wrapper, 2, 1, "println(value);\n"))
                .expect_err("analysis should reject an infinitely nested type");
            assert_eq!(error.code(), "K3001", "{wrapper}");
        }

        // Decoded JSON is bounded by the decoder's own nesting limit, far
        // below the value limit.
        let error = run(&format!(
            "json_decode(\"{}{}\");",
            "[".repeat(1_000),
            "]".repeat(1_000)
        ))
        .expect_err("deep JSON should be rejected");
        assert_eq!(error.code(), "K4009");
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
    fn orders_strings_byte_lexicographically_over_utf8() {
        let (output, _) = run(r#"
            println("" < "a");
            println("abc" < "abcd");
            println("abd" > "abcd");
            println("B" < "a");
            println("\u{7f}" < "\u{80}");
            println("e\u{301}" < "\u{e9}");
            println("\u{ffff}" < "\u{10000}");
            println("same" <= "same" && "same" >= "same");
            println("hi " + "there" > "hi ");
            "#)
        .expect("String ordering should run");
        assert_eq!(
            output,
            "true\ntrue\ntrue\ntrue\ntrue\ntrue\ntrue\ntrue\ntrue\n"
        );

        // Analysis rejects mixed kinds; the evaluator still refuses them.
        let error = run_unchecked("println(1 < \"one\");").expect_err("mixed kinds should fail");
        assert_eq!(error.code(), "K4001");
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
            let error = run_unchecked(source).expect_err("nested functions should not compare");
            assert_eq!(error.code(), "K4006");
            let error = run(source).expect_err("analysis should reject the comparison");
            assert_eq!(error.code(), "K3006");
        }
    }

    #[test]
    fn run_source_refuses_programs_that_fail_analysis() {
        // Each program would print before reaching its error if it were
        // evaluated; analysis refuses it first, so nothing is printed.
        for (source, code) in [
            ("println(\"before\");\nprintln(1 + true);\n", "K3001"),
            (
                "println(\"before\");\nlet value: Int = \"text\";\nprintln(value);\n",
                "K3001",
            ),
            ("println(\"before\");\nprintln(missing);\n", "K2001"),
            (
                "println(\"before\");\nlet value = 1;\nlet value = 2;\n",
                "K2002",
            ),
            (
                "println(\"before\");\nlet identity = fn(value) { value };\nidentity(1, 2);\n",
                "K3003",
            ),
        ] {
            let source_text = Source::new("test.krit", source);
            let mut output = Vec::new();
            let error = run_source(&source_text, &mut output)
                .expect_err("analysis should refuse the program");
            assert_eq!(error.code(), code, "{source}");
            assert!(output.is_empty(), "{source}");
        }
    }

    #[test]
    fn execute_keeps_annotations_dynamically_inert() {
        // `execute` itself does not consult annotations; only analysis does.
        let source = r#"
            let value: Int = "still dynamic";
            fn identity(input: Bool) -> Unit { input }
            println(identity(value));
            "#;
        let (output, _) = run_unchecked(source).expect("execute should ignore annotations");
        assert_eq!(output, "still dynamic\n");
        assert_eq!(
            run(source)
                .expect_err("analysis should enforce annotations")
                .code(),
            "K3001"
        );
    }

    #[test]
    fn reading_a_repeated_discard_evaluates_its_most_recent_binding() {
        // Analysis accepts repeated `_` bindings in one scope and resolves a
        // read to the most recent one (with warning K2003); evaluation must
        // agree, with and without analysis.
        let source = r#"
            let _ = 1;
            let _ = 2;
            println(_);
            fn second(_, _) { _ }
            println(second(3, 4));
            println(match [5, 6, 7] {
                [] => [],
                [_, .._] => _,
            });
            "#;
        for run in [run as Run, run_unchecked] {
            let (output, _) = run(source).expect("repeated discards should run");
            assert_eq!(output, "2\n4\n[6, 7]\n");
        }
    }

    #[test]
    fn bindings_shadow_unreserved_builtin_names() {
        // Analysis lets a match binder reuse a host built-in name that the
        // parser does not reserve, and resolves it to the binder. Evaluation
        // must agree, with and without analysis.
        let source = r#"
            let value = match Some(41) {
                Some(state_get) => state_get + 1,
                None => 0,
            };
            println(value);
            fn total(items) {
                match items {
                    [] => 0,
                    [cache_get, ..object_put] => cache_get + total(object_put),
                }
            }
            println(total([1, 2, 3]));
            let doubled = match Ok(fn(input) { input * 2 }) {
                Ok(queue_publish) => queue_publish(21),
                Err(db_commit) => db_commit,
            };
            println(doubled);
            println(match None {
                None => "unbound",
                Some(search_query) => search_query,
            });
            "#;
        for run in [run as Run, run_unchecked] {
            let (output, _) = run(source).expect("shadowing bindings should run");
            assert_eq!(output, "42\n6\n42\nunbound\n");
        }

        // Outside the binder's scope the name is the built-in again.
        let error = run(
            "let value = match Some(1) { Some(state_get) => state_get, None => 0 };\n\
             state_get(\"agent-work\", \"key\");\n",
        )
        .expect_err("the host built-in is unavailable in direct evaluation");
        assert_eq!(error.code(), "K5003");
    }

    #[test]
    fn encodes_and_decodes_json_deterministically() {
        let (output, _) = run(r#"
            let value = record { z: 2, option: Some([Some(true), None]), a: 1 };
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
            "{\"a\":1,\"option\":{\"Some\":[{\"Some\":true},{\"None\":null}]},\"z\":2}\ntrue\nErr(\"failed\")\nnull\ntrue\n"
        );
    }

    #[test]
    fn reports_json_failures() {
        let encode_error = run_unchecked("json_encode(fn(value) { value });")
            .expect_err("functions should not encode");
        assert_eq!(encode_error.code(), "K4008");
        let encode_error = run("json_encode(fn(value) { value });")
            .expect_err("analysis should reject encoding a function");
        assert_eq!(encode_error.code(), "K3006");

        let decode_error = run(r#"json_decode("{");"#).expect_err("invalid JSON should not decode");
        assert_eq!(decode_error.code(), "K4009");
    }

    #[test]
    fn named_construction_evaluates_in_source_order_and_fills_defaults() {
        let (output, _) = run(r#"
            fn note(text: String, value: Int) -> Int {
                print(text);
                value
            }
            let response = HttpResponse { body: { print("b"); "x" }, status: note("s", 201) };
            println("");
            println(response);
            println(HttpRequest { method: "GET", path: "/" });
            println(HttpResponse { status: 1 } == record { status: 1, headers: [], body: "" });
            "#)
        .expect("constructions should run");
        assert_eq!(
            output,
            "bs\nrecord { status: 201, headers: [], body: \"x\" }\n\
             record { method: \"GET\", path: \"/\", query: \"\", headers: [], body: \"\" }\n\
             true\n"
        );
    }

    #[test]
    fn unchecked_named_construction_reports_the_checkers_field_error() {
        for text in [
            "HttpResponse { body: \"x\" };",
            "HttpResponse { status: 1, extra: 2 };",
            "HttpResponse { status: 1, status: 2 };",
        ] {
            let error = run_unchecked(text).expect_err("an invalid field set should fail");
            assert_eq!(error.code(), "K3016", "{text}");
        }
    }
}
