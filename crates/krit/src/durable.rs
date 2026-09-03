use crate::{
    Block, Expression, ExpressionKind, MatchKind, Program, Span, Statement, StatementKind,
    ValueLiteral,
};

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum DurableOperationKind {
    StateGet,
    StatePut,
    StateDelete,
    CheckpointGet,
    CheckpointPut,
    ReplayHttp,
    ReplayAi,
    QueuePublish,
    ObjectGet,
    ObjectPut,
    ObjectDelete,
    DatabaseBeginRead,
    DatabaseBeginWrite,
    DatabaseQuery,
    DatabaseExecute,
    DatabaseCommit,
    DatabaseRollback,
    CacheGet,
    CachePut,
    CacheDelete,
    SearchQuery,
    VectorSearch,
}

impl DurableOperationKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::StateGet => "state-get",
            Self::StatePut => "state-put",
            Self::StateDelete => "state-delete",
            Self::CheckpointGet => "checkpoint-get",
            Self::CheckpointPut => "checkpoint-put",
            Self::ReplayHttp => "replay-http",
            Self::ReplayAi => "replay-ai",
            Self::QueuePublish => "queue-publish",
            Self::ObjectGet => "object-get",
            Self::ObjectPut => "object-put",
            Self::ObjectDelete => "object-delete",
            Self::DatabaseBeginRead => "database-begin-read",
            Self::DatabaseBeginWrite => "database-begin-write",
            Self::DatabaseQuery => "database-query",
            Self::DatabaseExecute => "database-execute",
            Self::DatabaseCommit => "database-commit",
            Self::DatabaseRollback => "database-rollback",
            Self::CacheGet => "cache-get",
            Self::CachePut => "cache-put",
            Self::CacheDelete => "cache-delete",
            Self::SearchQuery => "search-query",
            Self::VectorSearch => "vector-search",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DurableOperationFact {
    kind: DurableOperationKind,
    /// Durable store, queue, or bucket named by the first literal argument.
    store: Option<String>,
    identity: Option<String>,
    external_capability: Option<&'static str>,
    external_resource: Option<String>,
    span: Span,
}

impl DurableOperationFact {
    pub const fn kind(&self) -> DurableOperationKind {
        self.kind
    }

    /// The database, store, or bucket a durable operation names directly.
    /// Database operations that receive an opaque transaction handle instead of
    /// a literal database name report `None`; the handle's origin is reported by
    /// the corresponding `database-begin-*` fact.
    pub fn store(&self) -> Option<&str> {
        self.store.as_deref()
    }

    pub fn identity(&self) -> Option<&str> {
        self.identity.as_deref()
    }

    pub const fn external_capability(&self) -> Option<&'static str> {
        self.external_capability
    }

    pub fn external_resource(&self) -> Option<&str> {
        self.external_resource.as_deref()
    }

    pub const fn span(&self) -> Span {
        self.span
    }
}

pub fn durable_operations(program: &Program) -> Vec<DurableOperationFact> {
    let mut facts = Vec::new();
    for statement in &program.statements {
        collect_statement(statement, &mut facts);
    }
    facts.sort_by_key(|fact| fact.span);
    facts
}

fn collect_statement(statement: &Statement, facts: &mut Vec<DurableOperationFact>) {
    match &statement.kind {
        StatementKind::Let { value, .. } => collect_expression(value, facts),
        StatementKind::Function { body, .. }
        | StatementKind::Webhook { body, .. }
        | StatementKind::QueueConsumer { body, .. }
        | StatementKind::ScheduleHandler { body, .. } => {
            collect_block(body, facts);
        }
        StatementKind::Expression(expression) => collect_expression(expression, facts),
    }
}

fn collect_block(block: &Block, facts: &mut Vec<DurableOperationFact>) {
    for statement in &block.statements {
        collect_statement(statement, facts);
    }
    if let Some(tail) = block.tail.as_deref() {
        collect_expression(tail, facts);
    }
}

fn collect_expression(expression: &Expression, facts: &mut Vec<DurableOperationFact>) {
    match &expression.kind {
        ExpressionKind::Literal(_) | ExpressionKind::Variable(_) => {}
        ExpressionKind::List(elements) => {
            for element in elements {
                collect_expression(element, facts);
            }
        }
        ExpressionKind::Record(fields) => {
            for field in fields {
                collect_expression(&field.value, facts);
            }
        }
        ExpressionKind::FieldAccess { value, .. } => collect_expression(value, facts),
        ExpressionKind::Block(block) => collect_block(block, facts),
        ExpressionKind::If {
            condition,
            consequent,
            alternative,
        } => {
            collect_expression(condition, facts);
            collect_block(consequent, facts);
            collect_expression(alternative, facts);
        }
        ExpressionKind::Function { body, .. } => collect_block(body, facts),
        ExpressionKind::Call { callee, arguments } => {
            if let ExpressionKind::Variable(name) = &callee.kind
                && let Some(fact) = operation_fact(name, arguments, expression.span)
            {
                facts.push(fact);
            }
            collect_expression(callee, facts);
            for argument in arguments {
                collect_expression(argument, facts);
            }
        }
        ExpressionKind::Match { subject, kind } => {
            collect_expression(subject, facts);
            match kind {
                MatchKind::List {
                    empty_case,
                    cons_case,
                    ..
                } => {
                    collect_expression(empty_case, facts);
                    collect_expression(cons_case, facts);
                }
                MatchKind::Variants { arms, .. } => {
                    for arm in arms {
                        collect_expression(&arm.value, facts);
                    }
                }
            }
        }
        ExpressionKind::Unary { operand, .. } => collect_expression(operand, facts),
        ExpressionKind::Binary { left, right, .. } => {
            collect_expression(left, facts);
            collect_expression(right, facts);
        }
    }
}

fn operation_fact(
    name: &str,
    arguments: &[Expression],
    span: Span,
) -> Option<DurableOperationFact> {
    // Operations that receive an opaque transaction handle carry the statement
    // name, not a database name, so they are reported without a store.
    if let Some(kind) = transaction_operation_kind(name) {
        return Some(DurableOperationFact {
            kind,
            store: None,
            identity: string_argument(arguments, 1).map(str::to_owned),
            external_capability: None,
            external_resource: None,
            span,
        });
    }
    let store = Some(string_argument(arguments, 0)?.to_owned());
    let (kind, identity, external_capability, external_resource) = match name {
        "state_get" => (
            DurableOperationKind::StateGet,
            string_argument(arguments, 1).map(str::to_owned),
            None,
            None,
        ),
        "state_put" => (
            DurableOperationKind::StatePut,
            string_argument(arguments, 1).map(str::to_owned),
            None,
            None,
        ),
        "state_delete" => (
            DurableOperationKind::StateDelete,
            string_argument(arguments, 1).map(str::to_owned),
            None,
            None,
        ),
        "checkpoint_get" => (
            DurableOperationKind::CheckpointGet,
            string_argument(arguments, 1).map(str::to_owned),
            None,
            None,
        ),
        "checkpoint_put" => (
            DurableOperationKind::CheckpointPut,
            string_argument(arguments, 1).map(str::to_owned),
            None,
            None,
        ),
        "replay_http" => (
            DurableOperationKind::ReplayHttp,
            string_argument(arguments, 1).map(str::to_owned),
            Some("http.request"),
            string_argument(arguments, 2).map(str::to_owned),
        ),
        "replay_ai" => (
            DurableOperationKind::ReplayAi,
            string_argument(arguments, 1).map(str::to_owned),
            Some("ai.invoke"),
            string_argument(arguments, 2).map(str::to_owned),
        ),
        "queue_publish" => (DurableOperationKind::QueuePublish, None, None, None),
        "object_get" => (
            DurableOperationKind::ObjectGet,
            string_argument(arguments, 1).map(str::to_owned),
            None,
            None,
        ),
        "object_put" => (
            DurableOperationKind::ObjectPut,
            string_argument(arguments, 1).map(str::to_owned),
            None,
            None,
        ),
        "object_delete" => (
            DurableOperationKind::ObjectDelete,
            string_argument(arguments, 1).map(str::to_owned),
            None,
            None,
        ),
        "cache_get" => (
            DurableOperationKind::CacheGet,
            string_argument(arguments, 1).map(str::to_owned),
            None,
            None,
        ),
        "cache_put" => (
            DurableOperationKind::CachePut,
            string_argument(arguments, 1).map(str::to_owned),
            None,
            None,
        ),
        "cache_delete" => (
            DurableOperationKind::CacheDelete,
            string_argument(arguments, 1).map(str::to_owned),
            None,
            None,
        ),
        "search_query" => (DurableOperationKind::SearchQuery, None, None, None),
        "vector_search" => (DurableOperationKind::VectorSearch, None, None, None),
        "db_begin_read" => (DurableOperationKind::DatabaseBeginRead, None, None, None),
        "db_begin_write" => (DurableOperationKind::DatabaseBeginWrite, None, None, None),
        _ => return None,
    };
    Some(DurableOperationFact {
        kind,
        store,
        identity,
        external_capability,
        external_resource,
        span,
    })
}

const fn transaction_operation_kind(name: &str) -> Option<DurableOperationKind> {
    Some(match name.as_bytes() {
        b"db_query" => DurableOperationKind::DatabaseQuery,
        b"db_execute" => DurableOperationKind::DatabaseExecute,
        b"db_commit" => DurableOperationKind::DatabaseCommit,
        b"db_rollback" => DurableOperationKind::DatabaseRollback,
        _ => return None,
    })
}

fn string_argument(arguments: &[Expression], index: usize) -> Option<&str> {
    let ExpressionKind::Literal(ValueLiteral::String(value)) = &arguments.get(index)?.kind else {
        return None;
    };
    Some(value)
}
