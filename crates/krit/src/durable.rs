use crate::{
    Block, Expression, ExpressionKind, MatchKind, Program, Span, Statement, StatementKind,
    StringPart, ValueLiteral,
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
    collect_program(program, &mut facts);
    facts.sort_by_key(|fact| fact.span);
    facts
}

/// One syntax-tree node awaiting collection.
enum Pending<'a> {
    Statement(&'a Statement),
    Block(&'a Block),
    Expression(&'a Expression),
}

/// Collects durable operation facts with an explicit work stack, so neither
/// deep nesting nor long `else if` and operator chains deepen native
/// recursion. Facts are sorted by span afterwards, so visiting order does not
/// matter.
fn collect_program(program: &Program, facts: &mut Vec<DurableOperationFact>) {
    let mut pending = program
        .statements
        .iter()
        .map(Pending::Statement)
        .collect::<Vec<_>>();
    while let Some(node) = pending.pop() {
        match node {
            Pending::Statement(statement) => match &statement.kind {
                StatementKind::Let { value, .. } => pending.push(Pending::Expression(value)),
                StatementKind::Function { body, .. }
                | StatementKind::Webhook { body, .. }
                | StatementKind::QueueConsumer { body, .. }
                | StatementKind::ScheduleHandler { body, .. } => {
                    pending.push(Pending::Block(body));
                }
                StatementKind::Expression(expression) => {
                    pending.push(Pending::Expression(expression));
                }
            },
            Pending::Block(block) => {
                pending.extend(block.statements.iter().map(Pending::Statement));
                if let Some(tail) = block.tail.as_deref() {
                    pending.push(Pending::Expression(tail));
                }
            }
            Pending::Expression(expression) => {
                collect_expression(expression, &mut pending, facts);
            }
        }
    }
}

fn collect_expression<'a>(
    expression: &'a Expression,
    pending: &mut Vec<Pending<'a>>,
    facts: &mut Vec<DurableOperationFact>,
) {
    match &expression.kind {
        ExpressionKind::Literal(_) | ExpressionKind::Unit | ExpressionKind::Variable(_) => {}
        ExpressionKind::Interpolation(parts) => {
            pending.extend(parts.iter().filter_map(|part| match part {
                StringPart::Hole { value, .. } => Some(Pending::Expression(value)),
                StringPart::Text { .. } => None,
            }));
        }
        ExpressionKind::List(elements) => {
            pending.extend(elements.iter().map(Pending::Expression));
        }
        ExpressionKind::Record(fields) => {
            pending.extend(fields.iter().map(|field| Pending::Expression(&field.value)));
        }
        ExpressionKind::Construct { fields, .. } => {
            pending.extend(fields.iter().map(|field| Pending::Expression(&field.value)));
        }
        ExpressionKind::EntryList(entries) => {
            for entry in entries {
                pending.push(Pending::Expression(&entry.key));
                pending.push(Pending::Expression(&entry.value));
            }
        }
        ExpressionKind::FieldAccess { value, .. } => pending.push(Pending::Expression(value)),
        ExpressionKind::Block(block) => pending.push(Pending::Block(block)),
        ExpressionKind::If {
            condition,
            consequent,
            alternative,
        } => {
            pending.push(Pending::Expression(condition));
            pending.push(Pending::Block(consequent));
            pending.push(Pending::Expression(alternative));
        }
        ExpressionKind::Function { body, .. } => pending.push(Pending::Block(body)),
        ExpressionKind::Call { callee, arguments } => {
            if let ExpressionKind::Variable(name) = &callee.kind
                && let Some(fact) = operation_fact(name, arguments, expression.span)
            {
                facts.push(fact);
            }
            pending.push(Pending::Expression(callee));
            pending.extend(arguments.iter().map(Pending::Expression));
        }
        ExpressionKind::Match { subject, kind } => {
            pending.push(Pending::Expression(subject));
            match kind {
                MatchKind::List {
                    empty_case,
                    cons_case,
                    ..
                } => {
                    pending.push(Pending::Expression(empty_case));
                    pending.push(Pending::Expression(cons_case));
                }
                MatchKind::Variants { arms, .. } => {
                    pending.extend(arms.iter().map(|arm| Pending::Expression(&arm.value)));
                }
            }
        }
        ExpressionKind::Unary { operand, .. } => pending.push(Pending::Expression(operand)),
        ExpressionKind::Binary { left, right, .. } => {
            pending.push(Pending::Expression(left));
            pending.push(Pending::Expression(right));
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

#[cfg(test)]
mod tests {
    use crate::{Source, parse_source};

    use super::*;

    #[test]
    fn collects_facts_from_long_else_if_chains() {
        // Runs on the default test stack: collection uses an explicit stack.
        let mut text = String::from(
            "fn pick(n) {\n    if n == 0 {\n        state_get(\"work\", \"first\")\n    }",
        );
        for branch in 1..1_000 {
            text.push_str(&format!(
                " else if n == {branch} {{\n        state_get(\"work\", \"key{branch}\")\n    }}"
            ));
        }
        text.push_str(" else {\n        None\n    }\n}\n");
        let program = parse_source(&Source::new("test.krit", text)).expect("chain should parse");

        let facts = durable_operations(&program);
        assert_eq!(facts.len(), 1_000);
        assert!(facts.windows(2).all(|pair| pair[0].span() < pair[1].span()));
        assert_eq!(facts[0].kind(), DurableOperationKind::StateGet);
        assert_eq!(facts[0].store(), Some("work"));
        assert_eq!(facts[0].identity(), Some("first"));
        assert_eq!(facts[999].identity(), Some("key999"));
    }
}
