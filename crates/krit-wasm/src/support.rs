use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

use krit::{
    BinaryOperator, Builtin, CoreBlock, CoreFunction, CoreModule, CoreOperation, OperationKind,
    Span, Type, UnaryOperator, ValueLiteral,
};

use crate::{
    ApprovalRequirementMetadata, BuildError, ResourceRequirementMetadata, error::normalize,
    strings::StringHelper, wit::ProgramKind,
};

pub const SUPPORTED_BACKEND_SEMANTICS: &str = "\
Krit Wasm policy 1 supports Int (i64), Bool (i32), zero-width Unit, \
non-capturing function values (i32 table slots), recursive and higher-order calls, \
blocks, conditionals/short circuit, checked integer operators, primitive equality, \
and print/println of Int, Bool, or Unit. Webhook policy 2 additionally supports \
String values with concatenation, equality, and byte-lexicographic ordering, \
the closed HttpHeader/HttpRequest/HttpResponse records, header lists, \
LogField records and field lists, Result/Option matching, postfix `?` and let-else \
exits (a core `return` of the function result), exact config/secret/http/AI \
host calls, structured logging, json_decode into String (every RFC 8259 escape), \
and static non-capturing helper references. Helpers unreachable from the selected \
entrypoint are eliminated before layout checks. Other composites, general JSON, data \
captures, residual types, and list matching fail closed.";

pub(crate) struct CheckedModule {
    pub kind: ProgramKind,
    pub entrypoint: krit::FunctionId,
    pub effects: Vec<String>,
    pub requirements: Vec<ResourceRequirementMetadata>,
    pub approvals: Vec<ApprovalRequirementMetadata>,
    pub minimum_literal_operands: BTreeSet<krit::ValueId>,
    pub live: LiveFunctions,
}

/// Core functions that survive dead-function elimination.
///
/// Module initialization and the selected entrypoint are roots. A function is
/// live when a live function uses its closure value as an operand (a call,
/// argument, field, list element, branch result, ...) or captures a binding
/// that names it. Binding or discarding a closure is not a use, so an
/// unreferenced top-level helper is dead. Dead functions keep their Core
/// identifiers and table slots but are neither checked nor lowered: the
/// emitters replace each one with a trapping stub that nothing references.
#[derive(Clone, Debug, Default)]
pub(crate) struct LiveFunctions {
    functions: BTreeSet<krit::FunctionId>,
}

impl LiveFunctions {
    fn compute(module: &CoreModule, entrypoint: krit::FunctionId) -> Self {
        let mut closures = BTreeMap::new();
        for function in module.functions() {
            collect_closure_values(&function.body, &mut closures);
        }
        let mut bound = BTreeMap::new();
        for function in module.functions() {
            if let Some(recursive) = &function.recursive {
                bound.insert(recursive.binding, function.id);
            }
            collect_bound_functions(&function.body, &closures, &mut bound);
        }

        let mut functions = BTreeSet::new();
        let mut pending = vec![module.entrypoint(), entrypoint];
        while let Some(id) = pending.pop() {
            if !functions.insert(id) {
                continue;
            }
            let Some(function) = module.functions().get(id.as_u32() as usize) else {
                continue;
            };
            pending.extend(
                function
                    .captures
                    .iter()
                    .filter_map(|capture| bound.get(&capture.binding).copied()),
            );
            collect_used_functions(&function.body, &closures, &mut pending);
        }
        Self { functions }
    }

    pub(crate) fn contains(&self, function: krit::FunctionId) -> bool {
        self.functions.contains(&function)
    }

    /// Whether `operation` creates a closure over an eliminated function.
    pub(crate) fn is_dead_closure(&self, operation: &CoreOperation) -> bool {
        matches!(
            operation.kind,
            OperationKind::Closure { function, .. } if !self.contains(function)
        )
    }

    /// Live functions in Core order.
    pub(crate) fn functions<'a>(
        &'a self,
        module: &'a CoreModule,
    ) -> impl Iterator<Item = &'a CoreFunction> + 'a {
        module
            .functions()
            .iter()
            .filter(|function| self.contains(function.id))
    }
}

fn collect_closure_values(
    block: &CoreBlock,
    closures: &mut BTreeMap<krit::ValueId, krit::FunctionId>,
) {
    for operation in &block.operations {
        if let OperationKind::Closure { function, .. } = operation.kind {
            closures.insert(operation.result, function);
        }
        for_each_nested_block(operation, |nested| collect_closure_values(nested, closures));
    }
}

fn collect_bound_functions(
    block: &CoreBlock,
    closures: &BTreeMap<krit::ValueId, krit::FunctionId>,
    bound: &mut BTreeMap<krit::BindingId, krit::FunctionId>,
) {
    for operation in &block.operations {
        if let OperationKind::Bind { binding, value } = operation.kind
            && let Some(function) = closures.get(&value)
        {
            bound.insert(binding, *function);
        }
        for_each_nested_block(operation, |nested| {
            collect_bound_functions(nested, closures, bound);
        });
    }
}

fn collect_used_functions(
    block: &CoreBlock,
    closures: &BTreeMap<krit::ValueId, krit::FunctionId>,
    used: &mut Vec<krit::FunctionId>,
) {
    let mut mark = |value: &krit::ValueId| {
        if let Some(function) = closures.get(value) {
            used.push(*function);
        }
    };
    for operation in &block.operations {
        match &operation.kind {
            OperationKind::Literal(_)
            | OperationKind::Unit
            | OperationKind::Builtin(_)
            | OperationKind::Closure { .. }
            | OperationKind::Bind { .. }
            | OperationKind::Discard { .. }
            | OperationKind::Block { .. } => {}
            OperationKind::Variant { payload, .. } => payload.iter().for_each(&mut mark),
            OperationKind::List(values) => values.iter().for_each(&mut mark),
            OperationKind::Record(fields) => fields.iter().for_each(|field| mark(&field.value)),
            OperationKind::Field { value, .. } | OperationKind::Exit { value } => mark(value),
            OperationKind::Call { callee, arguments } => {
                mark(callee);
                arguments.iter().for_each(&mut mark);
            }
            OperationKind::If { condition, .. } => mark(condition),
            OperationKind::MatchList { subject, .. }
            | OperationKind::MatchVariant { subject, .. } => mark(subject),
            OperationKind::Unary { operand, .. } => mark(operand),
            OperationKind::Binary { left, right, .. } => {
                mark(left);
                mark(right);
            }
        }
    }
    mark(&block.result);
    for operation in &block.operations {
        for_each_nested_block(operation, |nested| {
            collect_used_functions(nested, closures, used);
        });
    }
}

/// Calls `visit` for every block nested directly inside `operation`.
pub(crate) fn for_each_nested_block(operation: &CoreOperation, mut visit: impl FnMut(&CoreBlock)) {
    match &operation.kind {
        OperationKind::Block { block } => visit(block),
        OperationKind::If {
            consequent,
            alternative,
            ..
        } => {
            visit(consequent);
            visit(alternative);
        }
        OperationKind::MatchList { empty, cons, .. } => {
            visit(empty);
            visit(cons);
        }
        OperationKind::MatchVariant { arms, .. } => {
            for arm in arms {
                visit(&arm.block);
            }
        }
        _ => {}
    }
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

/// Checks that every live Core construct has a lowering for the selected
/// artifact policy.
///
/// The checker never stops at the first problem: it returns every finding,
/// each at the originating Core operation's source span (falling back to the
/// enclosing block or function), sorted by span with one report per code and
/// span. Emission only runs after this check passes.
pub(crate) fn check_module(module: &CoreModule) -> Result<CheckedModule, Vec<BuildError>> {
    module.verify().map_err(|error| {
        vec![BuildError::invalid_core(format!(
            "Core verification failed: {error}"
        ))]
    })?;

    let exported = module
        .entrypoints()
        .iter()
        .find(|entrypoint| entrypoint.kind.is_exported());
    let (kind, entrypoint) = match exported {
        Some(entrypoint) => (
            match entrypoint.kind {
                krit::EntrypointKind::Webhook => ProgramKind::Webhook,
                krit::EntrypointKind::QueueConsumer => ProgramKind::Job,
                krit::EntrypointKind::ScheduleHandler => ProgramKind::Schedule,
                _ => {
                    return Err(vec![BuildError::invalid_core(
                        "module initialization is not an exported entrypoint",
                    )]);
                }
            },
            entrypoint.function,
        ),
        None => (ProgramKind::Module, module.entrypoint()),
    };
    let webhook = kind != ProgramKind::Module;

    let mut findings = Vec::new();
    if webhook && !module.entrypoint_function().signature.effects.is_empty() {
        findings.push(BuildError::unsupported(
            "entrypoint components cannot contain effectful module-initialization statements",
            first_effect_span(module),
        ));
    }

    let live = LiveFunctions::compute(module, entrypoint);
    for function in live.functions(module) {
        collect_residual_function(function, &live, &mut findings);
    }

    let minimum_literal_operands = minimum_literal_operands(module, &live);
    let static_functions = static_function_bindings(module);
    let mut checker = SupportChecker {
        minimum_literal_operands: &minimum_literal_operands,
        webhook,
        static_functions: &static_functions,
        live: &live,
        findings: &mut findings,
    };
    for function in live.functions(module) {
        checker.check_function(function);
    }
    if webhook {
        check_restricted_uses(module, &live, &mut findings);
    }
    if !findings.is_empty() {
        normalize(&mut findings);
        return Err(findings);
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
    let approvals = approval_requirements(module, &live, &requirements);
    Ok(CheckedModule {
        kind,
        entrypoint,
        effects,
        requirements,
        approvals,
        minimum_literal_operands,
        live,
    })
}
fn approval_requirements(
    module: &CoreModule,
    live: &LiveFunctions,
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
    for function in live.functions(module) {
        collect_approval_values(
            &function.body,
            &mut builtins,
            &mut strings,
            &mut bearer_options,
        );
    }
    for function in live.functions(module) {
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

/// Locates the first use of `effect` in the selected entrypoint: the
/// innermost operation that performs it, else the entrypoint declaration
/// (whose export implies effects such as `queue.consume`).
pub(crate) fn effect_span(
    module: &CoreModule,
    entrypoint: krit::FunctionId,
    effect: &str,
) -> Option<Span> {
    module
        .functions()
        .get(entrypoint.as_u32() as usize)
        .and_then(|function| find_effect_span(&function.body, effect).or(function.source))
        .or_else(|| first_effect_span(module))
}

fn find_effect_span(block: &CoreBlock, effect: &str) -> Option<Span> {
    let operation = block.operations.iter().find(|operation| {
        operation
            .effects
            .iter()
            .any(|candidate| candidate.as_str() == effect)
    })?;
    let mut nested = None;
    for_each_nested_block(operation, |inner| {
        if nested.is_none() {
            nested = find_effect_span(inner, effect);
        }
    });
    nested.or(operation.source).or(block.source)
}

/// Names the backend layout policy a program kind selects in diagnostics.
///
/// Module programs lower through the scalar policy-1 compiler; webhook, queue,
/// and schedule entrypoints lower through the bounded policy-2 webhook ABI.
const fn policy_name(webhook: bool) -> &'static str {
    if webhook {
        "the bounded webhook ABI (WebAssembly policy 2)"
    } else {
        "WebAssembly policy 1"
    }
}

struct SupportChecker<'a> {
    minimum_literal_operands: &'a BTreeSet<krit::ValueId>,
    webhook: bool,
    static_functions: &'a BTreeMap<krit::BindingId, krit::FunctionId>,
    live: &'a LiveFunctions,
    findings: &'a mut Vec<BuildError>,
}

impl SupportChecker<'_> {
    fn report(&mut self, result: Result<(), BuildError>) {
        if let Err(error) = result {
            self.findings.push(error);
        }
    }

    fn check_type(&mut self, ty: &Type, span: Option<Span>) {
        let result = check_type(ty, span, self.webhook);
        self.report(result);
    }

    fn check_function(&mut self, function: &CoreFunction) {
        if function
            .captures
            .iter()
            .any(|capture| !self.static_functions.contains_key(&capture.binding))
        {
            self.findings.push(BuildError::unsupported(
                format!(
                    "data captures are not supported by {}",
                    policy_name(self.webhook)
                ),
                function.source,
            ));
        }
        for (index, ty) in function.signature.parameters.iter().enumerate() {
            self.check_type(ty, parameter_span(function, index));
        }
        for (index, parameter) in function.parameters.iter().enumerate() {
            self.check_type(&parameter.ty, parameter_span(function, index));
        }
        // The recursive self binding and every closure over this function
        // share this signature, so it is checked once, here.
        self.check_type(&function.signature.result, function.source);
        let types = function_value_types(function);
        self.check_block(&function.body, &types, function.source);
    }

    fn check_block(
        &mut self,
        block: &CoreBlock,
        types: &BTreeMap<krit::ValueId, Arc<Type>>,
        fallback: Option<Span>,
    ) {
        let span = block.source.or(fallback);
        if !self.webhook && !block.parameters.is_empty() {
            self.findings.push(BuildError::unsupported(
                "match block parameters are not supported by WebAssembly policy 1",
                span,
            ));
        }
        for parameter in &block.parameters {
            self.check_type(&parameter.ty, span);
        }
        self.check_block_body(block, types, span);
    }

    /// Checks a block's result and operations but not its match binders.
    ///
    /// Arms of a match that is itself unsupported use this so that the
    /// binders are not reported a second time.
    fn check_block_body(
        &mut self,
        block: &CoreBlock,
        types: &BTreeMap<krit::ValueId, Arc<Type>>,
        span: Option<Span>,
    ) {
        self.check_type(&block.ty, span);
        for operation in &block.operations {
            if self.live.is_dead_closure(operation) {
                continue;
            }
            self.check_operation(operation, types, span);
        }
    }

    fn check_operation(
        &mut self,
        operation: &CoreOperation,
        types: &BTreeMap<krit::ValueId, Arc<Type>>,
        fallback: Option<Span>,
    ) {
        let span = operation.source.or(fallback);
        if contains_residual(&operation.ty)
            && !matches!(
                operation.kind,
                OperationKind::Block { .. }
                    | OperationKind::If { .. }
                    | OperationKind::MatchList { .. }
                    | OperationKind::MatchVariant { .. }
            )
        {
            // The residual pass reports this operation as K7001; its layout
            // cannot be judged until the type is specialized.
            return;
        }
        self.check_operation_kind(operation, types, span);
        if !matches!(operation.kind, OperationKind::Closure { .. }) {
            // A closure has its target's signature, checked with that function.
            // The kind-specific explanation above wins when both apply.
            self.check_type(&operation.ty, span);
        }
    }

    fn check_operation_kind(
        &mut self,
        operation: &CoreOperation,
        types: &BTreeMap<krit::ValueId, Arc<Type>>,
        span: Option<Span>,
    ) {
        let webhook = self.webhook;
        match &operation.kind {
            OperationKind::Literal(ValueLiteral::Integer(value)) => {
                if i64::try_from(*value).is_err()
                    && !self.minimum_literal_operands.contains(&operation.result)
                {
                    self.findings.push(BuildError::unsupported(
                        "integer literal is outside the concrete i64 WebAssembly layout",
                        span,
                    ));
                }
            }
            OperationKind::Literal(ValueLiteral::Boolean(_))
            | OperationKind::Unit
            | OperationKind::Bind { .. }
            | OperationKind::Discard { .. }
            | OperationKind::Call { .. } => {}
            // The exited value has the function's result type, which
            // `check_function` checks, and lowers to a WebAssembly `return`.
            OperationKind::Exit { .. } => {}
            OperationKind::Literal(ValueLiteral::String(_)) => {
                if !webhook {
                    self.findings
                        .push(unsupported_layout("String", span, webhook));
                }
            }
            OperationKind::Builtin(builtin) => {
                let result = check_builtin(*builtin, &operation.ty, span, webhook);
                self.report(result);
            }
            OperationKind::Closure { captures, .. } => {
                if captures
                    .iter()
                    .any(|capture| !self.static_functions.contains_key(&capture.binding))
                {
                    self.findings.push(BuildError::unsupported(
                        format!(
                            "data captures are not supported by {}",
                            policy_name(webhook)
                        ),
                        span,
                    ));
                }
            }
            OperationKind::Block { block } => self.check_block(block, types, span),
            OperationKind::If {
                consequent,
                alternative,
                ..
            } => {
                self.check_block(consequent, types, span);
                self.check_block(alternative, types, span);
            }
            OperationKind::Unary { operator, .. } => match operator {
                UnaryOperator::Not | UnaryOperator::Negate => {}
            },
            OperationKind::Binary { left, operator, .. } => {
                if let Some(message) = unsupported_binary(*operator, types.get(left), webhook) {
                    self.findings.push(BuildError::unsupported(message, span));
                }
            }
            OperationKind::Variant { .. } => {
                if !webhook {
                    self.findings.push(BuildError::unsupported(
                        "Option and Result layouts are not supported by WebAssembly policy 1",
                        span,
                    ));
                }
            }
            OperationKind::List(_)
                if webhook
                    && (is_header_list(&operation.ty)
                        || is_log_field_list(&operation.ty)
                        || is_string_list(&operation.ty)) => {}
            OperationKind::List(_) => {
                self.findings
                    .push(unsupported_layout("List", span, webhook));
            }
            OperationKind::MatchList { empty, cons, .. } => {
                self.findings.push(BuildError::unsupported(
                    format!("list matching is outside {}", policy_name(webhook)),
                    span,
                ));
                self.check_block_body(empty, types, empty.source.or(span));
                self.check_block_body(cons, types, cons.source.or(span));
            }
            OperationKind::Record(_) | OperationKind::Field { .. }
                if webhook
                    && (is_http_contract_type(&operation.ty) || is_log_field(&operation.ty)) => {}
            OperationKind::Field { .. } if webhook && is_supported_webhook_type(&operation.ty) => {}
            OperationKind::Record(_) | OperationKind::Field { .. } => {
                self.findings
                    .push(unsupported_layout("Record", span, webhook));
            }
            OperationKind::MatchVariant { arms, .. } if !webhook => {
                self.findings.push(BuildError::unsupported(
                    "Option and Result matching is not supported by WebAssembly policy 1",
                    span,
                ));
                for arm in arms {
                    self.check_block_body(&arm.block, types, arm.block.source.or(span));
                }
            }
            OperationKind::MatchVariant { arms, .. } => {
                for arm in arms {
                    self.check_block(&arm.block, types, span);
                }
            }
        }
    }
}

/// Explains why a binary operator has no lowering, or returns `None`.
///
/// Policy 1 only admits scalar operands, which the operand definitions
/// already enforce (a String operand is reported where the String value is
/// produced), so only the policy-2 operand shapes need inspection here.
/// Policy 2 lowers String concatenation, equality, and ordering through the
/// guest string runtime.
fn unsupported_binary(
    operator: BinaryOperator,
    operand: Option<&Arc<Type>>,
    webhook: bool,
) -> Option<String> {
    let operand = operand.map(Arc::as_ref);
    match operator {
        BinaryOperator::And | BinaryOperator::Or => {
            Some("short-circuit operators must be lowered to Core conditionals".to_owned())
        }
        _ if !webhook => None,
        // The residual pass reports an unspecialized operand as K7001.
        _ if operand.is_some_and(contains_residual) => None,
        _ if operand == Some(&Type::String) && StringHelper::for_operator(operator).is_some() => {
            None
        }
        BinaryOperator::Equal | BinaryOperator::NotEqual => match operand {
            Some(Type::Int | Type::Bool | Type::Unit | Type::Function(_)) => None,
            _ => Some(format!(
                "composite equality is outside {}",
                policy_name(webhook)
            )),
        },
        _ if operand == Some(&Type::Int) => None,
        _ => Some(format!(
            "operator `{}` has no lowering for `{}` operands in {}",
            operator_symbol(operator),
            operand.map_or_else(|| "unknown".to_owned(), ToString::to_string),
            policy_name(webhook)
        )),
    }
}

const fn operator_symbol(operator: BinaryOperator) -> &'static str {
    match operator {
        BinaryOperator::Add => "+",
        BinaryOperator::Subtract => "-",
        BinaryOperator::Multiply => "*",
        BinaryOperator::Divide => "/",
        BinaryOperator::Remainder => "%",
        BinaryOperator::Equal => "==",
        BinaryOperator::NotEqual => "!=",
        BinaryOperator::Less => "<",
        BinaryOperator::LessEqual => "<=",
        BinaryOperator::Greater => ">",
        BinaryOperator::GreaterEqual => ">=",
        BinaryOperator::And => "&&",
        BinaryOperator::Or => "||",
    }
}

/// Types of every value a function body can name, as emission sees them.
pub(crate) fn function_value_types(function: &CoreFunction) -> BTreeMap<krit::ValueId, Arc<Type>> {
    let mut types = BTreeMap::new();
    for parameter in &function.parameters {
        types.insert(parameter.value, Arc::clone(&parameter.ty));
    }
    for capture in &function.captures {
        types.insert(capture.value, Arc::clone(&capture.ty));
    }
    if let Some(recursive) = &function.recursive {
        types.insert(recursive.value, Arc::clone(&recursive.ty));
    }
    collect_block_value_types(&function.body, &mut types);
    types
}

fn collect_block_value_types(block: &CoreBlock, types: &mut BTreeMap<krit::ValueId, Arc<Type>>) {
    for parameter in &block.parameters {
        types.insert(parameter.value, Arc::clone(&parameter.ty));
    }
    for operation in &block.operations {
        types.insert(operation.result, Arc::clone(&operation.ty));
        for_each_nested_block(operation, |nested| collect_block_value_types(nested, types));
    }
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
                    format!("cannot print `{unsupported}` in {}", policy_name(webhook)),
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
        Builtin::QueuePublish
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
        Builtin::ObjectGet
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
        Builtin::ObjectPut
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
        Builtin::ObjectDelete
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
        Builtin::CacheGet
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
        Builtin::CachePut
            if function.parameters()
                == [
                    std::sync::Arc::new(Type::String),
                    std::sync::Arc::new(Type::String),
                    std::sync::Arc::new(Type::String),
                    std::sync::Arc::new(Type::Int),
                ]
                && matches!(
                    function.return_type(),
                    Type::Result(value, error)
                        if value.as_ref() == &Type::Unit && error.as_ref() == &Type::String
                ) =>
        {
            Ok(())
        }
        Builtin::CacheDelete
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
        Builtin::SearchQuery | Builtin::VectorSearch
            if function.parameters()
                == [
                    std::sync::Arc::new(Type::String),
                    std::sync::Arc::new(Type::String),
                    std::sync::Arc::new(Type::Int),
                ]
                && matches!(
                    function.return_type(),
                    Type::Result(value, error)
                        if value.as_ref() == &Type::String && error.as_ref() == &Type::String
                ) =>
        {
            Ok(())
        }
        Builtin::DatabaseBeginRead | Builtin::DatabaseBeginWrite
            if function.parameters() == [std::sync::Arc::new(Type::String)]
                && matches!(
                    function.return_type(),
                    Type::Result(value, error)
                        if value.as_ref() == &Type::DatabaseTransaction
                            && error.as_ref() == &Type::String
                ) =>
        {
            Ok(())
        }
        Builtin::DatabaseQuery
            if database_operation_parameters(function)
                && matches!(
                    function.return_type(),
                    Type::Result(value, error)
                        if value.as_ref() == &Type::String && error.as_ref() == &Type::String
                ) =>
        {
            Ok(())
        }
        Builtin::DatabaseExecute
            if database_operation_parameters(function)
                && matches!(
                    function.return_type(),
                    Type::Result(value, error)
                        if value.as_ref() == &Type::Int && error.as_ref() == &Type::String
                ) =>
        {
            Ok(())
        }
        Builtin::DatabaseCommit | Builtin::DatabaseRollback
            if function.parameters() == [std::sync::Arc::new(Type::DatabaseTransaction)]
                && matches!(
                    function.return_type(),
                    Type::Result(value, error)
                        if value.as_ref() == &Type::Unit && error.as_ref() == &Type::String
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

/// Checks that `ty` has a concrete layout in the selected policy.
///
/// Types that still contain inference variables are left to the residual
/// pass, which reports them as `K7001` at the same locations.
fn check_type(ty: &Type, span: Option<Span>, webhook: bool) -> Result<(), BuildError> {
    if contains_residual(ty) {
        return Ok(());
    }
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
        | Type::QueueJob
        | Type::ScheduleEvent
        | Type::DatabaseTransaction
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
        Type::String => Err(unsupported_layout("String", span, webhook)),
        Type::HttpHeader => Err(unsupported_layout("HttpHeader", span, webhook)),
        Type::HttpRequest => Err(unsupported_layout("HttpRequest", span, webhook)),
        Type::HttpResponse => Err(unsupported_layout("HttpResponse", span, webhook)),
        Type::LogField => Err(unsupported_layout("LogField", span, webhook)),
        Type::QueueJob => Err(unsupported_layout("QueueJob", span, webhook)),
        Type::ScheduleEvent => Err(unsupported_layout("ScheduleEvent", span, webhook)),
        Type::DatabaseTransaction => Err(unsupported_layout("DatabaseTransaction", span, webhook)),
        Type::Secret => Err(unsupported_layout("Secret", span, webhook)),
        Type::List(_) => Err(unsupported_layout("List", span, webhook)),
        Type::Record(_) => Err(unsupported_layout("Record", span, webhook)),
        Type::Option(_) => Err(unsupported_layout("Option", span, webhook)),
        Type::Result(_, _) => Err(unsupported_layout("Result", span, webhook)),
    }
}

fn unsupported_layout(layout: &str, span: Option<Span>, webhook: bool) -> BuildError {
    BuildError::unsupported(
        format!(
            "{layout} has no concrete guest layout in {}",
            policy_name(webhook)
        ),
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

fn check_restricted_uses(
    module: &CoreModule,
    live: &LiveFunctions,
    findings: &mut Vec<BuildError>,
) {
    let mut builtins = BTreeMap::new();
    let mut option_secrets = BTreeMap::new();
    for function in live.functions(module) {
        collect_restricted_values(
            &function.body,
            function.source,
            &mut builtins,
            &mut option_secrets,
        );
    }
    let mut uses = BTreeMap::<krit::ValueId, Vec<RestrictedUse>>::new();
    for function in live.functions(module) {
        collect_restricted_uses(&function.body, live, &builtins, &mut uses);
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
            findings.push(BuildError::unsupported(
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
            findings.push(BuildError::unsupported(
                "Option<Secret> must be consumed directly by `http_request`",
                span,
            ));
        }
    }
}

fn collect_restricted_values(
    block: &CoreBlock,
    fallback: Option<Span>,
    builtins: &mut BTreeMap<krit::ValueId, (Builtin, Option<Span>)>,
    option_secrets: &mut BTreeMap<krit::ValueId, Option<Span>>,
) {
    let block_span = block.source.or(fallback);
    for operation in &block.operations {
        let span = operation.source.or(block_span);
        if let OperationKind::Builtin(builtin) = operation.kind {
            builtins.insert(operation.result, (builtin, span));
        }
        if matches!(
            operation.ty.as_ref(),
            Type::Option(element) if element.as_ref() == &Type::Secret
        ) {
            option_secrets.insert(operation.result, span);
        }
        for_each_nested_block(operation, |nested| {
            collect_restricted_values(nested, span, builtins, option_secrets);
        });
    }
}

fn collect_restricted_uses(
    block: &CoreBlock,
    live: &LiveFunctions,
    builtins: &BTreeMap<krit::ValueId, (Builtin, Option<Span>)>,
    uses: &mut BTreeMap<krit::ValueId, Vec<RestrictedUse>>,
) {
    for operation in &block.operations {
        if live.is_dead_closure(operation) {
            continue;
        }
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
            | OperationKind::Discard { value }
            | OperationKind::Exit { value } => {
                uses.entry(*value).or_default().push(RestrictedUse::Other);
            }
            OperationKind::Block { block } => {
                collect_restricted_uses(block, live, builtins, uses);
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
                collect_restricted_uses(consequent, live, builtins, uses);
                collect_restricted_uses(alternative, live, builtins, uses);
            }
            OperationKind::MatchList {
                subject,
                empty,
                cons,
                ..
            } => {
                uses.entry(*subject).or_default().push(RestrictedUse::Other);
                collect_restricted_uses(empty, live, builtins, uses);
                collect_restricted_uses(cons, live, builtins, uses);
            }
            OperationKind::MatchVariant { subject, arms, .. } => {
                uses.entry(*subject).or_default().push(RestrictedUse::Other);
                for arm in arms {
                    collect_restricted_uses(&arm.block, live, builtins, uses);
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

/// Whether a database operation carries the fixed `(handle, name, params)` shape.
fn database_operation_parameters(function: &krit::FunctionType) -> bool {
    function.parameters()
        == [
            std::sync::Arc::new(Type::DatabaseTransaction),
            std::sync::Arc::new(Type::String),
            std::sync::Arc::new(Type::List(std::sync::Arc::new(Type::String))),
        ]
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
        | Type::QueueJob
        | Type::ScheduleEvent
        | Type::DatabaseTransaction
        | Type::Secret => true,
        Type::List(_) => is_header_list(ty) || is_log_field_list(ty) || is_string_list(ty),
        Type::Record(_) => is_http_contract_type(ty) || is_log_field(ty),
        Type::Option(element) => matches!(element.as_ref(), Type::Secret | Type::String),
        Type::Result(value, error) => {
            error.as_ref() == &Type::String
                && (matches!(
                    value.as_ref(),
                    Type::Unit
                        | Type::Int
                        | Type::String
                        | Type::Secret
                        | Type::HttpResponse
                        | Type::DatabaseTransaction
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

fn is_string_list(ty: &Type) -> bool {
    matches!(ty, Type::List(element) if element.as_ref() == &Type::String)
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

/// Reports every live location whose type still contains an inference
/// variable, at the same spans the layout checks use.
fn collect_residual_function(
    function: &CoreFunction,
    live: &LiveFunctions,
    findings: &mut Vec<BuildError>,
) {
    for (index, ty) in function.signature.parameters.iter().enumerate() {
        push_residual(ty, parameter_span(function, index), findings);
    }
    for (index, parameter) in function.parameters.iter().enumerate() {
        push_residual(&parameter.ty, parameter_span(function, index), findings);
    }
    push_residual(&function.signature.result, function.source, findings);
    collect_residual_block(&function.body, live, function.source, findings);
}

/// Span of a function's `index`th parameter, or of the function itself.
fn parameter_span(function: &CoreFunction, index: usize) -> Option<Span> {
    function
        .parameters
        .get(index)
        .and_then(|parameter| parameter.source)
        .or(function.source)
}

fn collect_residual_block(
    block: &CoreBlock,
    live: &LiveFunctions,
    fallback: Option<Span>,
    findings: &mut Vec<BuildError>,
) {
    let span = block.source.or(fallback);
    for parameter in &block.parameters {
        push_residual(&parameter.ty, span, findings);
    }
    push_residual(&block.ty, span, findings);
    for operation in &block.operations {
        if live.is_dead_closure(operation) {
            continue;
        }
        let operation_span = operation.source.or(span);
        // A closure has its target's signature, reported with that function.
        if !matches!(operation.kind, OperationKind::Closure { .. }) {
            push_residual(&operation.ty, operation_span, findings);
        }
        for_each_nested_block(operation, |nested| {
            collect_residual_block(nested, live, operation_span, findings);
        });
    }
}

fn push_residual(ty: &Type, span: Option<Span>, findings: &mut Vec<BuildError>) {
    if contains_residual(ty) {
        findings.push(BuildError::residual(
            format!("WebAssembly layout requires a concrete type, found parametric `{ty}`"),
            span,
        ));
    }
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
        | Type::QueueJob
        | Type::ScheduleEvent
        | Type::DatabaseTransaction
        | Type::Secret => false,
    }
}

fn minimum_literal_operands(module: &CoreModule, live: &LiveFunctions) -> BTreeSet<krit::ValueId> {
    let mut candidates = BTreeSet::new();
    let mut uses = BTreeMap::<_, Vec<_>>::new();
    for function in live.functions(module) {
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
            | OperationKind::Discard { value }
            | OperationKind::Exit { value } => record_use(uses, *value, ValueUse::Other),
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

    use krit::{CoreModule, Source, Type, analyze, lower, parse_source};

    use super::{LiveFunctions, is_supported_webhook_type};

    fn lower_source(text: &str) -> CoreModule {
        let source = Source::new("main.krit", text);
        let program = parse_source(&source).expect("source should parse");
        let analysis = analyze(&program).expect("source should analyze");
        lower(&program, &analysis).expect("source should lower")
    }

    fn live_names(text: &str) -> Vec<String> {
        let module = lower_source(text);
        let entrypoint = module
            .entrypoints()
            .iter()
            .find(|entrypoint| entrypoint.kind.is_exported())
            .map_or_else(|| module.entrypoint(), |entrypoint| entrypoint.function);
        let live = LiveFunctions::compute(&module, entrypoint);
        live.functions(&module)
            .map(|function| function.debug_name.clone().unwrap_or_default())
            .collect()
    }

    #[test]
    fn dead_function_elimination_follows_uses_and_captures() {
        assert_eq!(
            live_names(
                "fn leaf() { 1 }\nfn unused() { leaf() }\nfn used() { 2 }\nprintln(used());\n"
            ),
            ["$module_init", "used"]
        );
        assert_eq!(
            live_names(
                "fn apply(function, value) { function(value) }\n\
                 fn increment(value) { value + 1 }\n\
                 println(apply(increment, 1));\n"
            ),
            ["$module_init", "apply", "increment"]
        );
        assert_eq!(
            live_names(
                "fn increment(value) { value + 1 }\nlet alias = increment;\nprintln(alias(1));\n"
            ),
            ["$module_init", "increment"]
        );
        assert_eq!(
            live_names(
                "fn countdown(value) { if value == 0 { 0 } else { countdown(value - 1) } }\n\
                 fn helper() { 200 }\n\
                 fn unused() { countdown(3) }\n\
                 webhook fn handle(request: HttpRequest) -> HttpResponse {\n\
                 record { status: helper(), headers: [], body: request.path }\n\
                 }\n"
            ),
            ["$module_init", "helper", "handle"]
        );
    }

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
