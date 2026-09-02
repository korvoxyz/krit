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
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DurableOperationFact {
    kind: DurableOperationKind,
    store: String,
    identity: Option<String>,
    external_capability: Option<&'static str>,
    external_resource: Option<String>,
    span: Span,
}

impl DurableOperationFact {
    pub const fn kind(&self) -> DurableOperationKind {
        self.kind
    }

    pub fn store(&self) -> &str {
        &self.store
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
        StatementKind::Function { body, .. } | StatementKind::Webhook { body, .. } => {
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
    let store = string_argument(arguments, 0)?.to_owned();
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

fn string_argument(arguments: &[Expression], index: usize) -> Option<&str> {
    let ExpressionKind::Literal(ValueLiteral::String(value)) = &arguments.get(index)?.kind else {
        return None;
    };
    Some(value)
}
