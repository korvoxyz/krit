use std::{
    fs,
    path::{Path, PathBuf},
};

use krit::{
    Analysis, Block, Expression, ExpressionKind, MatchKind, Source, StatementKind, analyze, lower,
    parse_source,
};

fn lower_source(name: impl Into<String>, text: impl Into<String>) -> String {
    let source = Source::new(name.into(), text.into());
    let program = parse_source(&source).expect("valid source should parse");
    let analysis = analyze(&program).expect("valid source should analyze");
    let module = lower(&program, &analysis).expect("valid source should lower and verify");
    module.render_text()
}

#[test]
fn stable_core_rendering_matches_golden_cases() {
    let repository = repository_root();
    for (source_path, snapshot_path) in [
        (
            "examples/factorial.krit",
            "crates/krit/tests/snapshots/core-factorial.snap",
        ),
        (
            "examples/lists.krit",
            "crates/krit/tests/snapshots/core-list-sum.snap",
        ),
        (
            "conformance/cases/json/round-trip/program.krit",
            "crates/krit/tests/snapshots/core-records-variants-json.snap",
        ),
        (
            "conformance/cases/scope/closures/program.krit",
            "crates/krit/tests/snapshots/core-closures.snap",
        ),
        (
            "conformance/cases/variants/option-match/program.krit",
            "crates/krit/tests/snapshots/core-branches-matches.snap",
        ),
        (
            "conformance/check/valid/webhook-contract/program.krit",
            "crates/krit/tests/snapshots/core-webhook-contract.snap",
        ),
    ] {
        let source_path = repository.join(source_path);
        let text = fs::read_to_string(&source_path).expect("snapshot source should be readable");
        let actual = lower_source(source_path.to_string_lossy(), text);
        let expected = fs::read_to_string(repository.join(snapshot_path))
            .expect("Core snapshot should be readable");
        assert_eq!(
            actual,
            expected,
            "snapshot mismatch for {}",
            source_path.display()
        );
    }
}

#[test]
fn lowering_is_deterministic_and_resolves_names_to_ids() {
    let source = Source::new(
        "deterministic.krit",
        r#"
        let offset = 2;
        let add = fn(value) { value + offset };
        println(add(40));
        "#,
    );
    let program = parse_source(&source).expect("source should parse");
    let first_analysis = analyze(&program).expect("source should analyze");
    let second_analysis = analyze(&program).expect("source should reanalyze");
    let first = lower(&program, &first_analysis).expect("source should lower");
    let second = lower(&program, &second_analysis).expect("source should lower again");

    assert_eq!(first_analysis, second_analysis);
    assert_eq!(first.render_text(), second.render_text());
    assert_eq!(
        first
            .bindings()
            .iter()
            .map(|binding| binding.id.as_u32())
            .collect::<Vec<_>>(),
        (0..first.bindings().len() as u32).collect::<Vec<_>>()
    );

    let StatementKind::Expression(call) = &program.statements[2].kind else {
        panic!("third statement should be an expression");
    };
    let ExpressionKind::Call { callee, arguments } = &call.kind else {
        panic!("third statement should be a call");
    };
    assert!(
        first_analysis
            .expression(callee.span)
            .and_then(|fact| fact.resolved_name())
            .is_some()
    );
    let ExpressionKind::Call {
        callee: nested_callee,
        ..
    } = &arguments[0].kind
    else {
        panic!("println argument should be a call");
    };
    assert!(
        first_analysis
            .expression(nested_callee.span)
            .and_then(|fact| fact.resolved_name())
            .is_some()
    );
}

#[test]
fn every_executable_variable_reference_has_a_resolved_identity() {
    let source = Source::new(
        "resolved.krit",
        r#"
        fn sum(items) {
            match items {
                [] => 0,
                [head, ..tail] => head + sum(tail),
            }
        }
        let offset = 2;
        let add = fn(value) { value + offset };
        println(add(sum([1, 2, 3])));
        "#,
    );
    let program = parse_source(&source).expect("source should parse");
    let analysis = analyze(&program).expect("source should analyze");
    for statement in &program.statements {
        assert_statement_names_resolved(statement, &analysis);
    }
    let rendered = lower(&program, &analysis)
        .expect("source should lower")
        .render_text();
    assert!(!rendered.contains("load-name"));
}

#[test]
fn core_operations_preserve_left_to_right_evaluation_order() {
    let rendered = lower_source(
        "order.krit",
        r#"
        fn mark(value) {
            println(value);
            value
        }
        println(record {
            first: mark(1),
            second: mark(2),
        });
        "#,
    );

    let first_literal = rendered.find("int 1").expect("first argument should exist");
    let first_call = rendered[first_literal..]
        .find(" = call ")
        .map(|index| first_literal + index)
        .expect("first call should follow its argument");
    let second_literal = rendered
        .find("int 2")
        .expect("second argument should exist");
    let second_call = rendered[second_literal..]
        .find(" = call ")
        .map(|index| second_literal + index)
        .expect("second call should follow its argument");
    let record = rendered
        .find(" = record {")
        .expect("record should be constructed");
    let output = rendered[record..]
        .find(" = call ")
        .map(|index| record + index)
        .expect("outer output call should follow record construction");
    assert!(
        first_literal < first_call
            && first_call < second_literal
            && second_literal < second_call
            && second_call < record
            && record < output
    );
}

#[test]
fn nested_closures_thread_lexical_captures_explicitly() {
    let source = Source::new(
        "nested-captures.krit",
        r#"
        let root = 1;
        let outer = fn(first) {
            let middle = fn(second) {
                fn(third) {
                    root + first + second + third
                }
            };
            middle
        };
        println(outer(2)(3)(4));
        "#,
    );
    let program = parse_source(&source).expect("source should parse");
    let analysis = analyze(&program).expect("source should analyze");
    let module = lower(&program, &analysis).expect("source should lower and verify");
    assert_eq!(
        module
            .functions()
            .iter()
            .map(|function| function.captures.len())
            .collect::<Vec<_>>(),
        [0, 1, 2, 3]
    );
    for function in &module.functions()[1..] {
        for capture in &function.captures {
            assert_eq!(
                capture.ty.as_ref(),
                module.bindings()[capture.binding.as_u32() as usize]
                    .ty
                    .as_ref()
            );
        }
    }
}

#[test]
fn every_valid_repository_program_lowers_and_verifies() {
    let repository = repository_root();
    let mut programs = Vec::new();

    collect_success_cases(
        &repository.join("conformance/cases"),
        "expect.status",
        &mut programs,
    );
    collect_success_cases(
        &repository.join("conformance/check"),
        "expect.status",
        &mut programs,
    );
    collect_files(&repository.join("examples"), "krit", &mut programs);
    collect_files(
        &repository.join("conformance/format"),
        "krit",
        &mut programs,
    );
    programs.sort();

    for path in programs {
        let text = fs::read_to_string(&path).expect("program should be readable");
        let source = Source::new(path.to_string_lossy().into_owned(), text);
        let program = parse_source(&source).unwrap_or_else(|diagnostic| {
            panic!(
                "{} should parse: {}",
                path.display(),
                diagnostic.render_human(&source)
            )
        });
        let analysis = analyze(&program).unwrap_or_else(|diagnostic| {
            panic!(
                "{} should analyze: {}",
                path.display(),
                diagnostic.render_human(&source)
            )
        });
        let module = lower(&program, &analysis)
            .unwrap_or_else(|error| panic!("{} should lower: {error}", path.display()));
        module
            .verify()
            .unwrap_or_else(|error| panic!("{} should verify: {error}", path.display()));
    }

    let prompt = fs::read_to_string(repository.join("crates/krit-cli/assets/KRIT-0.2-SYSTEM.md"))
        .expect("prompt should be readable");
    let mut remaining = prompt.as_str();
    let mut count = 0;
    while let Some(start) = remaining.find("```krit\n") {
        let code = &remaining[start + "```krit\n".len()..];
        let end = code.find("\n```").expect("Krit prompt fence should close");
        lower_source(format!("<prompt-{count}>"), &code[..end]);
        count += 1;
        remaining = &code[end + "\n```".len()..];
    }
    assert_eq!(count, 10);
}

#[test]
fn large_analysis_and_lowering_use_complete_deterministic_fact_indexes() {
    const STATEMENT_COUNT: usize = 4_000;
    let mut text = String::from("let value0 = 0;\n");
    for index in 1..STATEMENT_COUNT {
        text.push_str(&format!("let value{index} = value{} + 1;\n", index - 1));
    }
    text.push_str(&format!("println(value{});\n", STATEMENT_COUNT - 1));

    let source = Source::new("large.krit", text);
    let program = parse_source(&source).expect("large source should parse");
    let first = analyze(&program).expect("large source should analyze");
    let second = analyze(&program).expect("large source should analyze deterministically");
    assert_eq!(first, second);
    assert_eq!(first.symbols().len(), STATEMENT_COUNT);

    for (index, statement) in program.statements[..STATEMENT_COUNT].iter().enumerate() {
        let StatementKind::Let { name, value, .. } = &statement.kind else {
            panic!("generated statement should be a let");
        };
        let symbol = first
            .symbol(statement.span, name, krit::SymbolKind::Let)
            .expect("every declaration should be indexed");
        assert_eq!(symbol.id().as_u32(), index as u32);
        assert!(
            first.expression(value.span).is_some(),
            "every initializer should have an indexed expression fact"
        );
    }

    let module = lower(&program, &first).expect("large source should lower and verify");
    assert_eq!(module.bindings().len(), STATEMENT_COUNT);
    assert!(!module.has_residual_types());
}

fn repository_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("repository root should exist")
}

fn collect_success_cases(directory: &Path, status_name: &str, programs: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(directory).expect("conformance directory should be readable") {
        let path = entry.expect("directory entry should be readable").path();
        if !path.is_dir() {
            continue;
        }
        let program = path.join("program.krit");
        if program.is_file() {
            let status = fs::read_to_string(path.join(status_name))
                .expect("conformance status should be readable");
            if status.trim() == "0" {
                programs.push(program);
            }
        } else {
            collect_success_cases(&path, status_name, programs);
        }
    }
}

fn collect_files(directory: &Path, extension: &str, files: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(directory).expect("source directory should be readable") {
        let path = entry.expect("directory entry should be readable").path();
        if path.is_dir() {
            collect_files(&path, extension, files);
        } else if path.extension().and_then(|value| value.to_str()) == Some(extension) {
            files.push(path);
        }
    }
}

fn assert_statement_names_resolved(statement: &krit::Statement, analysis: &Analysis) {
    match &statement.kind {
        StatementKind::Let { value, .. } | StatementKind::Expression(value) => {
            assert_expression_names_resolved(value, analysis);
        }
        StatementKind::Function { body, .. }
        | StatementKind::Webhook { body, .. }
        | StatementKind::QueueConsumer { body, .. }
        | StatementKind::ScheduleHandler { body, .. } => {
            assert_block_names_resolved(body, analysis)
        }
    }
}

fn assert_block_names_resolved(block: &Block, analysis: &Analysis) {
    for statement in &block.statements {
        assert_statement_names_resolved(statement, analysis);
    }
    if let Some(tail) = block.tail.as_deref() {
        assert_expression_names_resolved(tail, analysis);
    }
}

fn assert_expression_names_resolved(expression: &Expression, analysis: &Analysis) {
    if matches!(expression.kind, ExpressionKind::Variable(_)) {
        assert!(
            analysis
                .expression(expression.span)
                .and_then(|fact| fact.resolved_name())
                .is_some(),
            "variable at {:?} should be resolved",
            expression.span
        );
    }
    match &expression.kind {
        ExpressionKind::Literal(_) | ExpressionKind::Variable(_) => {}
        ExpressionKind::List(elements) => {
            for element in elements {
                assert_expression_names_resolved(element, analysis);
            }
        }
        ExpressionKind::Record(fields) => {
            for field in fields {
                assert_expression_names_resolved(&field.value, analysis);
            }
        }
        ExpressionKind::FieldAccess { value, .. } => {
            assert_expression_names_resolved(value, analysis);
        }
        ExpressionKind::Block(block) | ExpressionKind::Function { body: block, .. } => {
            assert_block_names_resolved(block, analysis);
        }
        ExpressionKind::If {
            condition,
            consequent,
            alternative,
        } => {
            assert_expression_names_resolved(condition, analysis);
            assert_block_names_resolved(consequent, analysis);
            assert_expression_names_resolved(alternative, analysis);
        }
        ExpressionKind::Call { callee, arguments } => {
            assert_expression_names_resolved(callee, analysis);
            for argument in arguments {
                assert_expression_names_resolved(argument, analysis);
            }
        }
        ExpressionKind::Match { subject, kind } => {
            assert_expression_names_resolved(subject, analysis);
            match kind {
                MatchKind::List {
                    empty_case,
                    cons_case,
                    ..
                } => {
                    assert_expression_names_resolved(empty_case, analysis);
                    assert_expression_names_resolved(cons_case, analysis);
                }
                MatchKind::Variants { arms, .. } => {
                    for arm in arms {
                        assert_expression_names_resolved(&arm.value, analysis);
                    }
                }
            }
        }
        ExpressionKind::Unary { operand, .. } => {
            assert_expression_names_resolved(operand, analysis);
        }
        ExpressionKind::Binary { left, right, .. } => {
            assert_expression_names_resolved(left, analysis);
            assert_expression_names_resolved(right, analysis);
        }
    }
}
