use std::collections::{BTreeMap, BTreeSet};

use krit::{
    BinaryOperator, Builtin, CoreBlock, CoreFunction, CoreModule, CoreOperation, OperationKind,
    Span, Type, UnaryOperator, ValueLiteral,
};

use crate::BuildError;
use crate::{ApprovalRequirementMetadata, ResourceRequirementMetadata, wit::ProgramKind};

pub const SUPPORTED_BACKEND_SEMANTICS: &str = "\
Krit Wasm policy 1 supports Int (i64), Bool (i32), zero-width Unit, \
non-capturing function values (i32 table slots), recursive and higher-order calls, \
blocks, conditionals/short circuit, checked integer operators, primitive equality, \
and print/println of Int, Bool, or Unit. Webhook policy 2 additionally supports \
String values, the closed HttpHeader/HttpRequest/HttpResponse records, header lists, \
LogField records and field lists, Result/Option matching, exact config/secret/http/AI \
host calls, structured logging, unescaped JSON string decoding, and static \
non-capturing helper references. Other composites, general JSON, data captures, \
residual types, and list matching fail closed.";

pub(crate) struct CheckedModule {
    pub kind: ProgramKind,
    pub entrypoint: krit::FunctionId,
    pub effects: Vec<String>,
    pub requirements: Vec<ResourceRequirementMetadata>,
    pub approvals: Vec<ApprovalRequirementMetadata>,
    pub minimum_literal_operands: BTreeSet<krit::ValueId>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ValueUse {
    DirectNegation,
    Other,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RestrictedUse {
    DirectCall,
    HttpBearer,
    Other,
}

pub(crate) fn check_module(module: &CoreModule) -> Result<CheckedModule, BuildError> {
    module
        .verify()
        .map_err(|error| BuildError::invalid_core(format!("Core verification failed: {error}")))?;

    let webhook = module
        .entrypoints()
        .iter()
        .find(|entrypoint| entrypoint.kind == krit::EntrypointKind::Webhook);
    if webhook.is_some() && !module.entrypoint_function().signature.effects.is_empty() {
        return Err(BuildError::unsupported(
            "webhook components cannot contain effectful module-initialization statements",
            module.entrypoint_function().source,
        ));
    }
    let (kind, entrypoint) = webhook
        .map_or((ProgramKind::Module, module.entrypoint()), |entrypoint| {
            (ProgramKind::Webhook, entrypoint.function)
        });

    if module.has_residual_types() {
        return Err(BuildError::residual(
            "WebAssembly layout requires specialization of residual parametric types",
            first_residual_span(module),
        ));
    }

    let minimum_literal_operands = minimum_literal_operands(module);
    let static_functions = static_function_bindings(module);
    for function in module.functions() {
        check_function(
            function,
            &minimum_literal_operands,
            kind == ProgramKind::Webhook,
            &static_functions,
        )?;
    }
    if kind == ProgramKind::Webhook {
        check_restricted_uses(module)?;
    }

    let selected = &module.functions()[entrypoint.as_u32() as usize];
    let mut effects = selected
        .signature
        .effects
        .iter()
        .map(|effect| effect.as_str().to_owned())
        .collect::<Vec<_>>();
    effects.sort();
    effects.dedup();
    let mut requirements = selected
        .signature
        .requirements
        .iter()
        .map(|requirement| ResourceRequirementMetadata {
            capability: requirement.capability().as_str().to_owned(),
            resource: requirement.resource().to_owned(),
        })
        .collect::<Vec<_>>();
    requirements.sort();
    requirements.dedup();
    let approvals = approval_requirements(module, &requirements);
    Ok(CheckedModule {
        kind,
        entrypoint,
        effects,
        requirements,
        approvals,
        minimum_literal_operands,
    })
}

fn approval_requirements(
    module: &CoreModule,
    requirements: &[ResourceRequirementMetadata],
) -> Vec<ApprovalRequirementMetadata> {
    let mut approvals = requirements
        .iter()
        .filter(|requirement| requirement.capability == "ai.invoke")
        .map(|requirement| ApprovalRequirementMetadata {
            operation: "ai.invoke".to_owned(),
            resource: requirement.resource.clone(),
        })
        .collect::<BTreeSet<_>>();
    let mut builtins = BTreeMap::new();
    let mut strings = BTreeMap::new();
    let mut bearer_options = BTreeSet::new();
    for function in module.functions() {
        collect_approval_values(
            &function.body,
            &mut builtins,
            &mut strings,
            &mut bearer_options,
        );
    }
    for function in module.functions() {
        collect_bearer_approvals(
            &function.body,
            &builtins,
            &strings,
            &bearer_options,
            &mut approvals,
        );
    }
    approvals.into_iter().collect()
}

fn collect_approval_values(
    block: &CoreBlock,
    builtins: &mut BTreeMap<krit::ValueId, Builtin>,
    strings: &mut BTreeMap<krit::ValueId, String>,
    bearer_options: &mut BTreeSet<krit::ValueId>,
) {
    for operation in &block.operations {
        match &operation.kind {
            OperationKind::Builtin(builtin) => {
                builtins.insert(operation.result, *builtin);
            }
            OperationKind::Literal(ValueLiteral::String(value)) => {
                strings.insert(operation.result, value.clone());
            }
            OperationKind::Variant {
                variant: krit::VariantName::Some,
                payload: Some(_),
            } if matches!(
                operation.ty.as_ref(),
                Type::Option(element) if element.as_ref() == &Type::Secret
            ) =>
            {
                bearer_options.insert(operation.result);
            }
            OperationKind::Call { callee, .. }
                if builtins.get(callee) == Some(&Builtin::Some)
                    && matches!(
                        operation.ty.as_ref(),
                        Type::Option(element) if element.as_ref() == &Type::Secret
                    ) =>
            {
                bearer_options.insert(operation.result);
            }
            OperationKind::Block { block } => {
                collect_approval_values(block, builtins, strings, bearer_options);
            }
            OperationKind::If {
                consequent,
                alternative,
                ..
            } => {
                collect_approval_values(consequent, builtins, strings, bearer_options);
                collect_approval_values(alternative, builtins, strings, bearer_options);
            }
            OperationKind::MatchList { empty, cons, .. } => {
                collect_approval_values(empty, builtins, strings, bearer_options);
                collect_approval_values(cons, builtins, strings, bearer_options);
            }
            OperationKind::MatchVariant { arms, .. } => {
                for arm in arms {
                    collect_approval_values(&arm.block, builtins, strings, bearer_options);
                }
            }
            _ => {}
        }
    }
}

fn collect_bearer_approvals(
    block: &CoreBlock,
    builtins: &BTreeMap<krit::ValueId, Builtin>,
    strings: &BTreeMap<krit::ValueId, String>,
    bearer_options: &BTreeSet<krit::ValueId>,
    approvals: &mut BTreeSet<ApprovalRequirementMetadata>,
) {
    for operation in &block.operations {
        match &operation.kind {
            OperationKind::Call { callee, arguments }
                if builtins.get(callee) == Some(&Builtin::HttpRequest)
                    && arguments.len() == 3
                    && bearer_options.contains(&arguments[2]) =>
            {
                if let Some(origin) = strings.get(&arguments[0]) {
                    approvals.insert(ApprovalRequirementMetadata {
                        operation: "http.bearer".to_owned(),
                        resource: origin.clone(),
                    });
                }
            }
            OperationKind::Block { block } => {
                collect_bearer_approvals(block, builtins, strings, bearer_options, approvals)
            }
            OperationKind::If {
                consequent,
                alternative,
                ..
            } => {
                collect_bearer_approvals(consequent, builtins, strings, bearer_options, approvals);
                collect_bearer_approvals(alternative, builtins, strings, bearer_options, approvals);
            }
            OperationKind::MatchList { empty, cons, .. } => {
                collect_bearer_approvals(empty, builtins, strings, bearer_options, approvals);
                collect_bearer_approvals(cons, builtins, strings, bearer_options, approvals);
            }
            OperationKind::MatchVariant { arms, .. } => {
                for arm in arms {
                    collect_bearer_approvals(
                        &arm.block,
                        builtins,
                        strings,
                        bearer_options,
                        approvals,
                    );
                }
            }
            _ => {}
        }
    }
}

pub(crate) fn first_effect_span(module: &CoreModule) -> Option<Span> {
    first_effect_block_span(&module.entrypoint_function().body)
}

fn check_function(
    function: &CoreFunction,
    minimum_literal_operands: &BTreeSet<krit::ValueId>,
    webhook: bool,
    static_functions: &BTreeMap<krit::BindingId, krit::FunctionId>,
) -> Result<(), BuildError> {
    if function
        .captures
        .iter()
        .any(|capture| !static_functions.contains_key(&capture.binding))
    {
        return Err(BuildError::unsupported(
            "data captures are not supported by the bounded webhook ABI",
            function.source,
        ));
    }
    for ty in function
        .signature
        .parameters
        .iter()
        .chain(std::iter::once(&function.signature.result))
    {
        check_type(ty, function.source, webhook)?;
    }
    for parameter in &function.parameters {
        check_type(&parameter.ty, parameter.source.or(function.source), webhook)?;
    }
    if let Some(recursive) = &function.recursive {
        check_type(&recursive.ty, function.source, webhook)?;
    }
    check_block(
        &function.body,
        minimum_literal_operands,
        webhook,
        static_functions,
    )
}

fn check_block(
    block: &CoreBlock,
    minimum_literal_operands: &BTreeSet<krit::ValueId>,
    webhook: bool,
    static_functions: &BTreeMap<krit::BindingId, krit::FunctionId>,
) -> Result<(), BuildError> {
    if !webhook && !block.parameters.is_empty() {
        return Err(BuildError::unsupported(
            "match block parameters are not supported by WebAssembly policy 1",
            block.source,
        ));
    }
    for parameter in &block.parameters {
        check_type(&parameter.ty, block.source, webhook)?;
    }
    check_type(&block.ty, block.source, webhook)?;
    for operation in &block.operations {
        check_operation(
            operation,
            minimum_literal_operands,
            webhook,
            static_functions,
        )?;
    }
    Ok(())
}

fn check_operation(
    operation: &CoreOperation,
    minimum_literal_operands: &BTreeSet<krit::ValueId>,
    webhook: bool,
    static_functions: &BTreeMap<krit::BindingId, krit::FunctionId>,
) -> Result<(), BuildError> {
    check_type(&operation.ty, operation.source, webhook)?;
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
        OperationKind::Literal(ValueLiteral::String(_)) if !webhook => {
            return Err(unsupported_layout("String", operation.source));
        }
        OperationKind::Literal(ValueLiteral::String(_)) => {}
        OperationKind::Builtin(builtin) => {
            check_builtin(*builtin, &operation.ty, operation.source, webhook)?
        }
        OperationKind::Closure { captures, .. } => {
            if captures
                .iter()
                .any(|capture| !static_functions.contains_key(&capture.binding))
            {
                return Err(BuildError::unsupported(
                    "data captures are not supported by the bounded webhook ABI",
                    operation.source,
                ));
            }
        }
        OperationKind::Call { .. } => {}
        OperationKind::Block { block } => {
            check_block(block, minimum_literal_operands, webhook, static_functions)?
        }
        OperationKind::If {
            consequent,
            alternative,
            ..
        } => {
            check_block(
                consequent,
                minimum_literal_operands,
                webhook,
                static_functions,
            )?;
            check_block(
                alternative,
                minimum_literal_operands,
                webhook,
                static_functions,
            )?;
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
            | BinaryOperator::GreaterEqual => {
                if webhook
                    && matches!(
                        operator,
                        BinaryOperator::Add | BinaryOperator::Equal | BinaryOperator::NotEqual
                    )
                    && operation.ty.as_ref() == &Type::String
                {
                    return Err(BuildError::unsupported(
                        "dynamic String operators are outside the bounded webhook ABI",
                        operation.source,
                    ));
                }
            }
            BinaryOperator::And | BinaryOperator::Or => {
                return Err(BuildError::unsupported(
                    "short-circuit operators must be lowered to Core conditionals",
                    operation.source,
                ));
            }
        },
        OperationKind::Variant { .. } if !webhook => {
            return Err(BuildError::unsupported(
                "Option and Result layouts are not supported by WebAssembly policy 1",
                operation.source,
            ));
        }
        OperationKind::Variant { .. } => {}
        OperationKind::List(_)
            if webhook && (is_header_list(&operation.ty) || is_log_field_list(&operation.ty)) => {}
        OperationKind::List(_) | OperationKind::MatchList { .. } => {
            return Err(unsupported_layout("List", operation.source));
        }
        OperationKind::Record(_) | OperationKind::Field { .. }
            if webhook && (is_http_contract_type(&operation.ty) || is_log_field(&operation.ty)) => {
        }
        OperationKind::Field { .. } if webhook && is_supported_webhook_type(&operation.ty) => {}
        OperationKind::Record(_) | OperationKind::Field { .. } => {
            return Err(unsupported_layout("Record", operation.source));
        }
        OperationKind::MatchVariant { .. } if !webhook => {
            return Err(BuildError::unsupported(
                "Option and Result matching is not supported by WebAssembly policy 1",
                operation.source,
            ));
        }
        OperationKind::MatchVariant { arms, .. } => {
            for arm in arms {
                check_block(
                    &arm.block,
                    minimum_literal_operands,
                    webhook,
                    static_functions,
                )?;
            }
        }
    }
    Ok(())
}

fn check_builtin(
    builtin: Builtin,
    ty: &Type,
    span: Option<Span>,
    webhook: bool,
) -> Result<(), BuildError> {
    if !webhook && !matches!(builtin, Builtin::Print | Builtin::Println) {
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
            "built-in has a non-function layout",
            span,
        ));
    };
    match builtin {
        Builtin::AiInvoke
            if function.parameters()
                == [
                    std::sync::Arc::new(Type::String),
                    std::sync::Arc::new(Type::String),
                ]
                && matches!(
                    function.return_type(),
                    Type::Result(value, error)
                        if value.as_ref() == &Type::String && error.as_ref() == &Type::String
                ) =>
        {
            Ok(())
        }
        Builtin::Print | Builtin::Println => {
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
        Builtin::Some
            if function.parameters() == [std::sync::Arc::new(Type::Secret)]
                && matches!(
                    function.return_type(),
                    Type::Option(element) if element.as_ref() == &Type::Secret
                ) =>
        {
            Ok(())
        }
        Builtin::Ok | Builtin::Err
            if function.parameters().len() == 1
                && is_supported_webhook_type(function.return_type()) =>
        {
            Ok(())
        }
        Builtin::ConfigString
            if function.parameters() == [std::sync::Arc::new(Type::String)]
                && matches!(
                    function.return_type(),
                    Type::Result(value, error)
                        if value.as_ref() == &Type::String && error.as_ref() == &Type::String
                ) =>
        {
            Ok(())
        }
        Builtin::Secret
            if function.parameters() == [std::sync::Arc::new(Type::String)]
                && matches!(
                    function.return_type(),
                    Type::Result(value, error)
                        if value.as_ref() == &Type::Secret && error.as_ref() == &Type::String
                ) =>
        {
            Ok(())
        }
        Builtin::HttpRequest
            if function.parameters().len() == 3
                && function.parameters()[0].as_ref() == &Type::String
                && function.parameters()[1].as_ref() == &Type::HttpRequest
                && matches!(
                    function.parameters()[2].as_ref(),
                    Type::Option(element) if element.as_ref() == &Type::Secret
                )
                && matches!(
                    function.return_type(),
                    Type::Result(value, error)
                        if value.as_ref() == &Type::HttpResponse
                            && error.as_ref() == &Type::String
                ) =>
        {
            Ok(())
        }
        Builtin::LogInfo | Builtin::LogError
            if function.parameters()
                == [
                    std::sync::Arc::new(Type::String),
                    std::sync::Arc::new(Type::List(std::sync::Arc::new(Type::LogField))),
                ]
                && matches!(
                    function.return_type(),
                    Type::Result(value, error)
                        if value.as_ref() == &Type::Unit && error.as_ref() == &Type::String
                ) =>
        {
            Ok(())
        }
        Builtin::StateGet | Builtin::CheckpointGet
            if function.parameters()
                == [
                    std::sync::Arc::new(Type::String),
                    std::sync::Arc::new(Type::String),
                ]
                && matches!(
                    function.return_type(),
                    Type::Result(value, error)
                        if matches!(
                            value.as_ref(),
                            Type::Option(element) if element.as_ref() == &Type::String
                        ) && error.as_ref() == &Type::String
                ) =>
        {
            Ok(())
        }
        Builtin::StatePut | Builtin::CheckpointPut
            if function.parameters()
                == [
                    std::sync::Arc::new(Type::String),
                    std::sync::Arc::new(Type::String),
                    std::sync::Arc::new(Type::String),
                ]
                && matches!(
                    function.return_type(),
                    Type::Result(value, error)
                        if value.as_ref() == &Type::Unit && error.as_ref() == &Type::String
                ) =>
        {
            Ok(())
        }
        Builtin::StateDelete
            if function.parameters()
                == [
                    std::sync::Arc::new(Type::String),
                    std::sync::Arc::new(Type::String),
                ]
                && matches!(
                    function.return_type(),
                    Type::Result(value, error)
                        if value.as_ref() == &Type::Unit && error.as_ref() == &Type::String
                ) =>
        {
            Ok(())
        }
        Builtin::ReplayHttp
            if function.parameters()
                == [
                    std::sync::Arc::new(Type::String),
                    std::sync::Arc::new(Type::String),
                    std::sync::Arc::new(Type::String),
                    std::sync::Arc::new(Type::HttpRequest),
                ]
                && matches!(
                    function.return_type(),
                    Type::Result(value, error)
                        if value.as_ref() == &Type::HttpResponse
                            && error.as_ref() == &Type::String
                ) =>
        {
            Ok(())
        }
        Builtin::ReplayAi
            if function.parameters()
                == [
                    std::sync::Arc::new(Type::String),
                    std::sync::Arc::new(Type::String),
                    std::sync::Arc::new(Type::String),
                    std::sync::Arc::new(Type::String),
                ]
                && matches!(
                    function.return_type(),
                    Type::Result(value, error)
                        if value.as_ref() == &Type::String && error.as_ref() == &Type::String
                ) =>
        {
            Ok(())
        }
        Builtin::JsonDecode
            if function.parameters() == [std::sync::Arc::new(Type::String)]
                && function.return_type() == &Type::String =>
        {
            Ok(())
        }
        _ => Err(BuildError::unsupported(
            format!(
                "built-in `{}` has no bounded webhook ABI lowering for `{ty}`",
                builtin.as_str()
            ),
            span,
        )),
    }
}

fn check_type(ty: &Type, span: Option<Span>, webhook: bool) -> Result<(), BuildError> {
    let mut visited = BTreeSet::new();
    check_type_inner(ty, span, webhook, &mut visited)
}

fn check_type_inner(
    ty: &Type,
    span: Option<Span>,
    webhook: bool,
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
                check_type_inner(parameter, span, webhook, visited)?;
            }
            check_type_inner(function.return_type(), span, webhook, visited)
        }
        Type::Variable(_) => Err(BuildError::residual(
            "WebAssembly layout requires specialization of a parametric type",
            span,
        )),
        Type::String
        | Type::HttpHeader
        | Type::HttpRequest
        | Type::HttpResponse
        | Type::LogField
        | Type::Secret
            if webhook =>
        {
            Ok(())
        }
        Type::List(_) | Type::Record(_) | Type::Option(_) | Type::Result(_, _)
            if webhook && is_supported_webhook_type(ty) =>
        {
            Ok(())
        }
        Type::String => Err(unsupported_layout("String", span)),
        Type::HttpHeader => Err(unsupported_layout("HttpHeader", span)),
        Type::HttpRequest => Err(unsupported_layout("HttpRequest", span)),
        Type::HttpResponse => Err(unsupported_layout("HttpResponse", span)),
        Type::LogField => Err(unsupported_layout("LogField", span)),
        Type::Secret => Err(unsupported_layout("Secret", span)),
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

fn static_function_bindings(module: &CoreModule) -> BTreeMap<krit::BindingId, krit::FunctionId> {
    let mut values = BTreeMap::new();
    let mut bindings = BTreeMap::new();
    for operation in &module.entrypoint_function().body.operations {
        match &operation.kind {
            OperationKind::Closure {
                function, captures, ..
            } => {
                values.insert(
                    operation.result,
                    (
                        *function,
                        captures
                            .iter()
                            .map(|capture| capture.binding)
                            .collect::<Vec<_>>(),
                    ),
                );
            }
            OperationKind::Bind { binding, value } => {
                if let Some((function, captures)) = values.get(value)
                    && captures
                        .iter()
                        .all(|capture| bindings.contains_key(capture))
                {
                    bindings.insert(*binding, *function);
                }
            }
            _ => {}
        }
    }
    bindings
}

fn check_restricted_uses(module: &CoreModule) -> Result<(), BuildError> {
    let mut builtins = BTreeMap::new();
    let mut option_secrets = BTreeMap::new();
    for function in module.functions() {
        collect_restricted_values(&function.body, &mut builtins, &mut option_secrets);
    }
    let mut uses = BTreeMap::<krit::ValueId, Vec<RestrictedUse>>::new();
    for function in module.functions() {
        collect_restricted_uses(&function.body, &builtins, &mut uses);
    }
    for (value, (builtin, span)) in builtins {
        if matches!(builtin, Builtin::Print | Builtin::Println) {
            continue;
        }
        let value_uses = uses.get(&value).map(Vec::as_slice).unwrap_or_default();
        if value_uses.is_empty()
            || value_uses
                .iter()
                .any(|value_use| *value_use != RestrictedUse::DirectCall)
        {
            return Err(BuildError::unsupported(
                format!(
                    "built-in `{}` must remain a direct call in the bounded webhook ABI",
                    builtin.as_str()
                ),
                span,
            ));
        }
    }
    for (value, span) in option_secrets {
        let value_uses = uses.get(&value).map(Vec::as_slice).unwrap_or_default();
        if value_uses != [RestrictedUse::HttpBearer] {
            return Err(BuildError::unsupported(
                "Option<Secret> must be consumed directly by `http_request`",
                span,
            ));
        }
    }
    Ok(())
}

fn collect_restricted_values(
    block: &CoreBlock,
    builtins: &mut BTreeMap<krit::ValueId, (Builtin, Option<Span>)>,
    option_secrets: &mut BTreeMap<krit::ValueId, Option<Span>>,
) {
    for operation in &block.operations {
        if let OperationKind::Builtin(builtin) = operation.kind {
            builtins.insert(operation.result, (builtin, operation.source));
        }
        if matches!(
            operation.ty.as_ref(),
            Type::Option(element) if element.as_ref() == &Type::Secret
        ) {
            option_secrets.insert(operation.result, operation.source);
        }
        match &operation.kind {
            OperationKind::Block { block } => {
                collect_restricted_values(block, builtins, option_secrets);
            }
            OperationKind::If {
                consequent,
                alternative,
                ..
            } => {
                collect_restricted_values(consequent, builtins, option_secrets);
                collect_restricted_values(alternative, builtins, option_secrets);
            }
            OperationKind::MatchList { empty, cons, .. } => {
                collect_restricted_values(empty, builtins, option_secrets);
                collect_restricted_values(cons, builtins, option_secrets);
            }
            OperationKind::MatchVariant { arms, .. } => {
                for arm in arms {
                    collect_restricted_values(&arm.block, builtins, option_secrets);
                }
            }
            _ => {}
        }
    }
}

fn collect_restricted_uses(
    block: &CoreBlock,
    builtins: &BTreeMap<krit::ValueId, (Builtin, Option<Span>)>,
    uses: &mut BTreeMap<krit::ValueId, Vec<RestrictedUse>>,
) {
    for operation in &block.operations {
        match &operation.kind {
            OperationKind::Literal(_) | OperationKind::Unit | OperationKind::Builtin(_) => {}
            OperationKind::Call { callee, arguments } => {
                uses.entry(*callee)
                    .or_default()
                    .push(RestrictedUse::DirectCall);
                let http = builtins
                    .get(callee)
                    .is_some_and(|(builtin, _)| *builtin == Builtin::HttpRequest);
                for (index, argument) in arguments.iter().enumerate() {
                    uses.entry(*argument)
                        .or_default()
                        .push(if http && index == 2 {
                            RestrictedUse::HttpBearer
                        } else {
                            RestrictedUse::Other
                        });
                }
            }
            OperationKind::Variant { payload, .. } => {
                if let Some(payload) = payload {
                    uses.entry(*payload).or_default().push(RestrictedUse::Other);
                }
            }
            OperationKind::List(values) => {
                for value in values {
                    uses.entry(*value).or_default().push(RestrictedUse::Other);
                }
            }
            OperationKind::Record(fields) => {
                for field in fields {
                    uses.entry(field.value)
                        .or_default()
                        .push(RestrictedUse::Other);
                }
            }
            OperationKind::Field { value, .. }
            | OperationKind::Bind { value, .. }
            | OperationKind::Discard { value } => {
                uses.entry(*value).or_default().push(RestrictedUse::Other);
            }
            OperationKind::Block { block } => {
                collect_restricted_uses(block, builtins, uses);
            }
            OperationKind::Closure { captures, .. } => {
                for capture in captures {
                    uses.entry(capture.value)
                        .or_default()
                        .push(RestrictedUse::Other);
                }
            }
            OperationKind::If {
                condition,
                consequent,
                alternative,
            } => {
                uses.entry(*condition)
                    .or_default()
                    .push(RestrictedUse::Other);
                collect_restricted_uses(consequent, builtins, uses);
                collect_restricted_uses(alternative, builtins, uses);
            }
            OperationKind::MatchList {
                subject,
                empty,
                cons,
                ..
            } => {
                uses.entry(*subject).or_default().push(RestrictedUse::Other);
                collect_restricted_uses(empty, builtins, uses);
                collect_restricted_uses(cons, builtins, uses);
            }
            OperationKind::MatchVariant { subject, arms, .. } => {
                uses.entry(*subject).or_default().push(RestrictedUse::Other);
                for arm in arms {
                    collect_restricted_uses(&arm.block, builtins, uses);
                }
            }
            OperationKind::Unary { operand, .. } => {
                uses.entry(*operand).or_default().push(RestrictedUse::Other);
            }
            OperationKind::Binary { left, right, .. } => {
                uses.entry(*left).or_default().push(RestrictedUse::Other);
                uses.entry(*right).or_default().push(RestrictedUse::Other);
            }
        }
    }
    uses.entry(block.result)
        .or_default()
        .push(RestrictedUse::Other);
}

fn is_supported_webhook_type(ty: &Type) -> bool {
    match ty {
        Type::Int
        | Type::Bool
        | Type::String
        | Type::Unit
        | Type::HttpHeader
        | Type::HttpRequest
        | Type::HttpResponse
        | Type::LogField
        | Type::Secret => true,
        Type::List(_) => is_header_list(ty) || is_log_field_list(ty),
        Type::Record(_) => is_http_contract_type(ty) || is_log_field(ty),
        Type::Option(element) => matches!(element.as_ref(), Type::Secret | Type::String),
        Type::Result(value, error) => {
            error.as_ref() == &Type::String
                && (matches!(
                    value.as_ref(),
                    Type::Unit | Type::String | Type::Secret | Type::HttpResponse
                ) || matches!(
                    value.as_ref(),
                    Type::Option(element) if element.as_ref() == &Type::String
                ))
        }
        Type::Function(function) => {
            function
                .parameters()
                .iter()
                .all(|parameter| is_supported_webhook_type(parameter))
                && is_supported_webhook_type(function.return_type())
        }
        Type::Variable(_) => false,
    }
}

fn is_header_list(ty: &Type) -> bool {
    matches!(ty, Type::List(element) if is_http_header(element))
}

fn is_log_field_list(ty: &Type) -> bool {
    matches!(ty, Type::List(element) if is_log_field(element))
}

fn is_http_contract_type(ty: &Type) -> bool {
    matches!(
        ty,
        Type::HttpHeader | Type::HttpRequest | Type::HttpResponse
    ) || is_http_header(ty)
        || is_http_request(ty)
        || is_http_response(ty)
}

fn is_log_field(ty: &Type) -> bool {
    match ty {
        Type::LogField => true,
        Type::Record(fields) => {
            record_fields_match(fields, &[("name", Type::String), ("value", Type::String)])
        }
        _ => false,
    }
}

fn is_http_header(ty: &Type) -> bool {
    match ty {
        Type::HttpHeader => true,
        Type::Record(fields) => {
            record_fields_match(fields, &[("name", Type::String), ("value", Type::String)])
        }
        _ => false,
    }
}

fn is_http_request(ty: &Type) -> bool {
    match ty {
        Type::HttpRequest => true,
        Type::Record(fields) => record_fields_match(
            fields,
            &[
                ("method", Type::String),
                ("path", Type::String),
                ("query", Type::String),
                ("headers", Type::List(std::sync::Arc::new(Type::HttpHeader))),
                ("body", Type::String),
            ],
        ),
        _ => false,
    }
}

fn is_http_response(ty: &Type) -> bool {
    match ty {
        Type::HttpResponse => true,
        Type::Record(fields) => record_fields_match(
            fields,
            &[
                ("status", Type::Int),
                ("headers", Type::List(std::sync::Arc::new(Type::HttpHeader))),
                ("body", Type::String),
            ],
        ),
        _ => false,
    }
}

fn record_fields_match(fields: &[krit::RecordType], expected: &[(&str, Type)]) -> bool {
    fields.len() == expected.len()
        && expected.iter().all(|(name, ty)| {
            fields
                .iter()
                .find(|field| field.name() == *name)
                .is_some_and(|field| webhook_type_equivalent(field.ty(), ty))
        })
}

fn webhook_type_equivalent(left: &Type, right: &Type) -> bool {
    left == right
        || (is_http_header(left) && is_http_header(right))
        || (is_http_request(left) && is_http_request(right))
        || (is_http_response(left) && is_http_response(right))
        || (is_log_field(left) && is_log_field(right))
        || matches!(
            (left, right),
            (Type::List(left), Type::List(right))
                if webhook_type_equivalent(left, right)
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
        Type::Int
        | Type::Bool
        | Type::String
        | Type::Unit
        | Type::HttpHeader
        | Type::HttpRequest
        | Type::HttpResponse
        | Type::LogField
        | Type::Secret => false,
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

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use krit::Type;

    use super::is_supported_webhook_type;

    #[test]
    fn option_string_results_still_require_a_string_error() {
        let value = Arc::new(Type::Option(Arc::new(Type::String)));
        assert!(is_supported_webhook_type(&Type::Result(
            Arc::clone(&value),
            Arc::new(Type::String),
        )));
        assert!(!is_supported_webhook_type(&Type::Result(
            value,
            Arc::new(Type::Variable(0)),
        )));
    }
}
