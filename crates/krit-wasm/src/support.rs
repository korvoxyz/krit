use std::collections::{BTreeMap, BTreeSet};

use krit::{
    BinaryOperator, Builtin, CoreBlock, CoreFunction, CoreModule, CoreOperation, OperationKind,
    Span, Type, UnaryOperator, ValueLiteral,
};

use crate::BuildError;

pub const SUPPORTED_BACKEND_SEMANTICS: &str = "\
Krit Wasm policy 1 supports Int (i64), Bool (i32), zero-width Unit, \
non-capturing function values (i32 table slots), recursive and higher-order calls, \
blocks, conditionals/short circuit, checked integer operators, primitive equality, \
and print/println of Int, Bool, or Unit. String, List, Record, Option, Result, JSON, \
lexical captures, residual types, matches, and all other built-ins fail closed.";

pub(crate) struct CheckedModule {
    pub effects: Vec<String>,
    pub minimum_literal_operands: BTreeSet<krit::ValueId>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ValueUse {
    DirectNegation,
    Other,
}

pub(crate) fn check_module(module: &CoreModule) -> Result<CheckedModule, BuildError> {
    module
        .verify()
        .map_err(|error| BuildError::invalid_core(format!("Core verification failed: {error}")))?;

    if module.has_residual_types() {
        return Err(BuildError::residual(
            "WebAssembly layout requires specialization of residual parametric types",
            first_residual_span(module),
        ));
    }

    let minimum_literal_operands = minimum_literal_operands(module);
    for function in module.functions() {
        check_function(function, &minimum_literal_operands)?;
    }

    let mut effects = module
        .entrypoint_function()
        .signature
        .effects
        .iter()
        .map(|effect| effect.as_str().to_owned())
        .collect::<Vec<_>>();
    effects.sort();
    effects.dedup();
    Ok(CheckedModule {
        effects,
        minimum_literal_operands,
    })
}

pub(crate) fn first_effect_span(module: &CoreModule) -> Option<Span> {
    first_effect_block_span(&module.entrypoint_function().body)
}

fn check_function(
    function: &CoreFunction,
    minimum_literal_operands: &BTreeSet<krit::ValueId>,
) -> Result<(), BuildError> {
    if !function.captures.is_empty() {
        return Err(BuildError::unsupported(
            "lexical captures are not supported by WebAssembly policy 1",
            function.source,
        ));
    }
    for ty in function
        .signature
        .parameters
        .iter()
        .chain(std::iter::once(&function.signature.result))
    {
        check_type(ty, function.source)?;
    }
    for parameter in &function.parameters {
        check_type(&parameter.ty, parameter.source.or(function.source))?;
    }
    if let Some(recursive) = &function.recursive {
        check_type(&recursive.ty, function.source)?;
    }
    check_block(&function.body, minimum_literal_operands)
}

fn check_block(
    block: &CoreBlock,
    minimum_literal_operands: &BTreeSet<krit::ValueId>,
) -> Result<(), BuildError> {
    if !block.parameters.is_empty() {
        return Err(BuildError::unsupported(
            "match block parameters are not supported by WebAssembly policy 1",
            block.source,
        ));
    }
    check_type(&block.ty, block.source)?;
    for operation in &block.operations {
        check_operation(operation, minimum_literal_operands)?;
    }
    Ok(())
}

fn check_operation(
    operation: &CoreOperation,
    minimum_literal_operands: &BTreeSet<krit::ValueId>,
) -> Result<(), BuildError> {
    check_type(&operation.ty, operation.source)?;
    match &operation.kind {
        OperationKind::Literal(ValueLiteral::Integer(value)) => {
            if i64::try_from(*value).is_err()
                && !minimum_literal_operands.contains(&operation.result)
            {
                return Err(BuildError::unsupported(
                    "integer literal is outside the concrete i64 WebAssembly layout",
                    operation.source,
                ));
            }
        }
        OperationKind::Literal(ValueLiteral::Boolean(_))
        | OperationKind::Unit
        | OperationKind::Bind { .. }
        | OperationKind::Discard { .. } => {}
        OperationKind::Literal(ValueLiteral::String(_)) => {
            return Err(unsupported_layout("String", operation.source));
        }
        OperationKind::Builtin(builtin) => {
            check_builtin(*builtin, &operation.ty, operation.source)?
        }
        OperationKind::Closure { captures, .. } => {
            if !captures.is_empty() {
                return Err(BuildError::unsupported(
                    "lexical captures are not supported by WebAssembly policy 1",
                    operation.source,
                ));
            }
        }
        OperationKind::Call { .. } => {}
        OperationKind::Block { block } => check_block(block, minimum_literal_operands)?,
        OperationKind::If {
            consequent,
            alternative,
            ..
        } => {
            check_block(consequent, minimum_literal_operands)?;
            check_block(alternative, minimum_literal_operands)?;
        }
        OperationKind::Unary { operator, .. } => match operator {
            UnaryOperator::Not | UnaryOperator::Negate => {}
        },
        OperationKind::Binary { operator, .. } => match operator {
            BinaryOperator::Add
            | BinaryOperator::Subtract
            | BinaryOperator::Multiply
            | BinaryOperator::Divide
            | BinaryOperator::Remainder
            | BinaryOperator::Equal
            | BinaryOperator::NotEqual
            | BinaryOperator::Less
            | BinaryOperator::LessEqual
            | BinaryOperator::Greater
            | BinaryOperator::GreaterEqual => {}
            BinaryOperator::And | BinaryOperator::Or => {
                return Err(BuildError::unsupported(
                    "short-circuit operators must be lowered to Core conditionals",
                    operation.source,
                ));
            }
        },
        OperationKind::Variant { .. } => {
            return Err(BuildError::unsupported(
                "Option and Result layouts are not supported by WebAssembly policy 1",
                operation.source,
            ));
        }
        OperationKind::List(_) | OperationKind::MatchList { .. } => {
            return Err(unsupported_layout("List", operation.source));
        }
        OperationKind::Record(_) | OperationKind::Field { .. } => {
            return Err(unsupported_layout("Record", operation.source));
        }
        OperationKind::MatchVariant { .. } => {
            return Err(BuildError::unsupported(
                "Option and Result matching is not supported by WebAssembly policy 1",
                operation.source,
            ));
        }
    }
    Ok(())
}

fn check_builtin(builtin: Builtin, ty: &Type, span: Option<Span>) -> Result<(), BuildError> {
    if !matches!(builtin, Builtin::Print | Builtin::Println) {
        return Err(BuildError::unsupported(
            format!(
                "built-in `{}` has no WebAssembly policy 1 lowering",
                builtin.as_str()
            ),
            span,
        ));
    }
    let Type::Function(function) = ty else {
        return Err(BuildError::unsupported(
            "stdout built-in has a non-function layout",
            span,
        ));
    };
    if function.parameters().len() != 1 || function.return_type() != &Type::Unit {
        return Err(BuildError::unsupported(
            "stdout built-in has an unsupported function signature",
            span,
        ));
    }
    match function.parameters()[0].as_ref() {
        Type::Int | Type::Bool | Type::Unit => Ok(()),
        unsupported => Err(BuildError::unsupported(
            format!("cannot print `{unsupported}` in WebAssembly policy 1"),
            span,
        )),
    }
}

fn check_type(ty: &Type, span: Option<Span>) -> Result<(), BuildError> {
    let mut visited = BTreeSet::new();
    check_type_inner(ty, span, &mut visited)
}

fn check_type_inner(
    ty: &Type,
    span: Option<Span>,
    visited: &mut BTreeSet<usize>,
) -> Result<(), BuildError> {
    let address = std::ptr::from_ref(ty) as usize;
    if !visited.insert(address) {
        return Ok(());
    }
    match ty {
        Type::Int | Type::Bool | Type::Unit => Ok(()),
        Type::Function(function) => {
            for parameter in function.parameters() {
                check_type_inner(parameter, span, visited)?;
            }
            check_type_inner(function.return_type(), span, visited)
        }
        Type::Variable(_) => Err(BuildError::residual(
            "WebAssembly layout requires specialization of a parametric type",
            span,
        )),
        Type::String => Err(unsupported_layout("String", span)),
        Type::List(_) => Err(unsupported_layout("List", span)),
        Type::Record(_) => Err(unsupported_layout("Record", span)),
        Type::Option(_) => Err(unsupported_layout("Option", span)),
        Type::Result(_, _) => Err(unsupported_layout("Result", span)),
    }
}

fn unsupported_layout(layout: &str, span: Option<Span>) -> BuildError {
    BuildError::unsupported(
        format!("{layout} has no concrete guest layout in WebAssembly policy 1"),
        span,
    )
}

fn first_residual_span(module: &CoreModule) -> Option<Span> {
    for function in module.functions() {
        if function
            .signature
            .parameters
            .iter()
            .any(|ty| contains_residual(ty))
            || contains_residual(&function.signature.result)
        {
            return function.source.or(function.body.source);
        }
        if let Some(span) = first_residual_block_span(&function.body) {
            return Some(span);
        }
    }
    None
}

fn first_residual_block_span(block: &CoreBlock) -> Option<Span> {
    if contains_residual(&block.ty) {
        return block.source;
    }
    for operation in &block.operations {
        if contains_residual(&operation.ty) {
            return operation.source.or(block.source);
        }
        let nested = match &operation.kind {
            OperationKind::Block { block } => first_residual_block_span(block),
            OperationKind::If {
                consequent,
                alternative,
                ..
            } => first_residual_block_span(consequent)
                .or_else(|| first_residual_block_span(alternative)),
            OperationKind::MatchList { empty, cons, .. } => {
                first_residual_block_span(empty).or_else(|| first_residual_block_span(cons))
            }
            OperationKind::MatchVariant { arms, .. } => arms
                .iter()
                .find_map(|arm| first_residual_block_span(&arm.block)),
            _ => None,
        };
        if nested.is_some() {
            return nested;
        }
    }
    None
}

fn first_effect_block_span(block: &CoreBlock) -> Option<Span> {
    for operation in &block.operations {
        if !operation.effects.is_empty() {
            return operation.source.or(block.source);
        }
        let nested = match &operation.kind {
            OperationKind::Block { block } => first_effect_block_span(block),
            OperationKind::If {
                consequent,
                alternative,
                ..
            } => {
                first_effect_block_span(consequent).or_else(|| first_effect_block_span(alternative))
            }
            OperationKind::MatchList { empty, cons, .. } => {
                first_effect_block_span(empty).or_else(|| first_effect_block_span(cons))
            }
            OperationKind::MatchVariant { arms, .. } => arms
                .iter()
                .find_map(|arm| first_effect_block_span(&arm.block)),
            _ => None,
        };
        if nested.is_some() {
            return nested;
        }
    }
    block.source
}

fn contains_residual(ty: &Type) -> bool {
    match ty {
        Type::Variable(_) => true,
        Type::List(element) | Type::Option(element) => contains_residual(element),
        Type::Record(fields) => fields.iter().any(|field| contains_residual(field.ty())),
        Type::Result(value, error) => contains_residual(value) || contains_residual(error),
        Type::Function(function) => {
            function
                .parameters()
                .iter()
                .any(|parameter| contains_residual(parameter))
                || contains_residual(function.return_type())
        }
        Type::Int | Type::Bool | Type::String | Type::Unit => false,
    }
}

fn minimum_literal_operands(module: &CoreModule) -> BTreeSet<krit::ValueId> {
    let mut candidates = BTreeSet::new();
    let mut uses = BTreeMap::<_, Vec<_>>::new();
    for function in module.functions() {
        collect_minimum_literals(&function.body, &mut candidates);
        collect_value_uses(&function.body, &mut uses);
    }
    candidates
        .into_iter()
        .filter(|value| {
            uses.get(value)
                .is_some_and(|uses| uses.as_slice() == [ValueUse::DirectNegation])
        })
        .collect()
}

fn collect_minimum_literals(block: &CoreBlock, candidates: &mut BTreeSet<krit::ValueId>) {
    for operation in &block.operations {
        if matches!(
            &operation.kind,
            OperationKind::Literal(ValueLiteral::Integer(value))
                if *value == i64::MAX as i128 + 1
        ) {
            candidates.insert(operation.result);
        }
        match &operation.kind {
            OperationKind::Block { block } => collect_minimum_literals(block, candidates),
            OperationKind::If {
                consequent,
                alternative,
                ..
            } => {
                collect_minimum_literals(consequent, candidates);
                collect_minimum_literals(alternative, candidates);
            }
            OperationKind::MatchList { empty, cons, .. } => {
                collect_minimum_literals(empty, candidates);
                collect_minimum_literals(cons, candidates);
            }
            OperationKind::MatchVariant { arms, .. } => {
                for arm in arms {
                    collect_minimum_literals(&arm.block, candidates);
                }
            }
            _ => {}
        }
    }
}

fn collect_value_uses(block: &CoreBlock, uses: &mut BTreeMap<krit::ValueId, Vec<ValueUse>>) {
    for operation in &block.operations {
        match &operation.kind {
            OperationKind::Literal(_) | OperationKind::Unit | OperationKind::Builtin(_) => {}
            OperationKind::Variant { payload, .. } => {
                if let Some(payload) = payload {
                    record_use(uses, *payload, ValueUse::Other);
                }
            }
            OperationKind::List(values) => record_uses(uses, values.iter().copied()),
            OperationKind::Record(fields) => {
                record_uses(uses, fields.iter().map(|field| field.value));
            }
            OperationKind::Field { value, .. }
            | OperationKind::Bind { value, .. }
            | OperationKind::Discard { value } => record_use(uses, *value, ValueUse::Other),
            OperationKind::Block { block } => collect_value_uses(block, uses),
            OperationKind::Closure { captures, .. } => {
                record_uses(uses, captures.iter().map(|capture| capture.value));
            }
            OperationKind::Call { callee, arguments } => {
                record_use(uses, *callee, ValueUse::Other);
                record_uses(uses, arguments.iter().copied());
            }
            OperationKind::If {
                condition,
                consequent,
                alternative,
            } => {
                record_use(uses, *condition, ValueUse::Other);
                collect_value_uses(consequent, uses);
                collect_value_uses(alternative, uses);
            }
            OperationKind::MatchList {
                subject,
                empty,
                cons,
                ..
            } => {
                record_use(uses, *subject, ValueUse::Other);
                collect_value_uses(empty, uses);
                collect_value_uses(cons, uses);
            }
            OperationKind::MatchVariant { subject, arms, .. } => {
                record_use(uses, *subject, ValueUse::Other);
                for arm in arms {
                    collect_value_uses(&arm.block, uses);
                }
            }
            OperationKind::Unary { operator, operand } => record_use(
                uses,
                *operand,
                if *operator == UnaryOperator::Negate {
                    ValueUse::DirectNegation
                } else {
                    ValueUse::Other
                },
            ),
            OperationKind::Binary { left, right, .. } => {
                record_use(uses, *left, ValueUse::Other);
                record_use(uses, *right, ValueUse::Other);
            }
        }
    }
    record_use(uses, block.result, ValueUse::Other);
}

fn record_uses(
    uses: &mut BTreeMap<krit::ValueId, Vec<ValueUse>>,
    values: impl IntoIterator<Item = krit::ValueId>,
) {
    for value in values {
        record_use(uses, value, ValueUse::Other);
    }
}

fn record_use(
    uses: &mut BTreeMap<krit::ValueId, Vec<ValueUse>>,
    value: krit::ValueId,
    kind: ValueUse,
) {
    uses.entry(value).or_default().push(kind);
}
