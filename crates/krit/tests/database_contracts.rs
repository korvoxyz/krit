use krit::{Effect, Source, SymbolKind, Type, analyze, format_source, lower, parse_source};

fn analyze_source(text: &str) -> Result<krit::Analysis, krit::Diagnostic> {
    let source = Source::new("database.krit", text);
    let program = parse_source(&source)?;
    analyze(&program)
}

fn diagnostic_code(text: &str) -> &'static str {
    let source = Source::new("database.krit", text);
    match parse_source(&source) {
        Ok(program) => analyze(&program)
            .expect_err("source should fail checking")
            .code(),
        Err(diagnostic) => diagnostic.code(),
    }
}

const REFERENCE: &str = r#"
webhook fn handle(request: HttpRequest) -> HttpResponse {
    match db_begin_write("catalog") {
        Ok(transaction) => match db_execute(transaction, "record-visit", [request.path]) {
            Ok(changed) => match db_query(transaction, "count-visits", []) {
                Ok(rows) => match db_commit(transaction) {
                    Ok(committed) => record { status: 200, headers: [], body: rows },
                    Err(error) => record { status: 500, headers: [], body: error },
                },
                Err(error) => match db_rollback(transaction) {
                    Ok(undone) => record { status: 500, headers: [], body: error },
                    Err(fatal) => record { status: 500, headers: [], body: fatal },
                },
            },
            Err(error) => match db_rollback(transaction) {
                Ok(undone) => record { status: 500, headers: [], body: error },
                Err(fatal) => record { status: 500, headers: [], body: fatal },
            },
        },
        Err(error) => record { status: 503, headers: [], body: error },
    }
}
"#;

#[test]
fn write_transactions_report_exact_database_authority() {
    let analysis = analyze_source(REFERENCE).expect("reference source should analyze");
    let webhook = analysis
        .symbols()
        .iter()
        .find(|symbol| symbol.kind() == SymbolKind::Webhook)
        .expect("webhook should exist");
    let Type::Function(function) = webhook.ty() else {
        panic!("webhook should have a function type")
    };

    assert_eq!(
        function
            .effects()
            .iter()
            .map(Effect::as_str)
            .collect::<Vec<_>>(),
        ["database.write"]
    );
    assert_eq!(
        function
            .requirements()
            .iter()
            .map(|requirement| (requirement.capability().as_str(), requirement.resource()))
            .collect::<Vec<_>>(),
        [("database.write", "catalog")]
    );
}

#[test]
fn read_transactions_never_request_write_authority() {
    let source = r#"
webhook fn handle(request: HttpRequest) -> HttpResponse {
    match db_begin_read("catalog") {
        Ok(transaction) => match db_query(transaction, "count-visits", []) {
            Ok(rows) => match db_commit(transaction) {
                Ok(committed) => record { status: 200, headers: [], body: rows },
                Err(error) => record { status: 500, headers: [], body: error },
            },
            Err(error) => record { status: 500, headers: [], body: error },
        },
        Err(error) => record { status: 503, headers: [], body: error },
    }
}
"#;

    let analysis = analyze_source(source).expect("read source should analyze");
    let webhook = analysis
        .symbols()
        .iter()
        .find(|symbol| symbol.kind() == SymbolKind::Webhook)
        .expect("webhook should exist");
    let Type::Function(function) = webhook.ty() else {
        panic!("webhook should have a function type")
    };

    assert_eq!(
        function
            .effects()
            .iter()
            .map(Effect::as_str)
            .collect::<Vec<_>>(),
        ["database.read"]
    );
    assert_eq!(
        function
            .requirements()
            .iter()
            .map(|requirement| (requirement.capability().as_str(), requirement.resource()))
            .collect::<Vec<_>>(),
        [("database.read", "catalog")]
    );
}

#[test]
fn database_and_statement_identities_are_direct_canonical_literals() {
    for source in [
        r#"let name = "catalog"; db_begin_read(name);"#,
        r#"db_begin_write("Catalog Name");"#,
        r#"db_begin_read("-bad-");"#,
        r#"let begin = db_begin_read; begin("catalog");"#,
    ] {
        assert_eq!(diagnostic_code(source), "K3008", "source: {source}");
    }

    let statement_cases = r#"
webhook fn handle(request: HttpRequest) -> HttpResponse {
    match db_begin_read("catalog") {
        Ok(transaction) => match db_query(transaction, "Count Visits", []) {
            Ok(rows) => record { status: 200, headers: [], body: rows },
            Err(error) => record { status: 500, headers: [], body: error },
        },
        Err(error) => record { status: 503, headers: [], body: error },
    }
}
"#;
    assert_eq!(diagnostic_code(statement_cases), "K3008");
}

#[test]
fn transaction_handles_are_opaque_like_secrets() {
    // Every one of these is a fail-closed rejection. Structural and revealing
    // uses report the opacity code; positions with a concrete declared type
    // report the ordinary type mismatch first.
    let opaque_uses: [(&str, &str); 6] = [
        ("println(transaction)", "K3010"),
        ("json_encode(transaction)", "K3010"),
        ("record { handle: transaction }", "K3010"),
        ("[transaction]", "K3010"),
        (
            "state_put(\"agent-work\", \"handle\", transaction)",
            "K3001",
        ),
        ("db_query(\"count-visits\", transaction, [])", "K3008"),
    ];

    for (expression, expected) in opaque_uses {
        let source = format!(
            r#"
webhook fn handle(request: HttpRequest) -> HttpResponse {{
    match db_begin_read("catalog") {{
        Ok(transaction) => {{
            let leaked = {expression};
            record {{ status: 200, headers: [], body: request.path }}
        }},
        Err(error) => record {{ status: 503, headers: [], body: error }},
    }}
}}
"#
        );
        assert_eq!(
            diagnostic_code(&source),
            expected,
            "expression: {expression}"
        );
    }
}

#[test]
fn transaction_handles_cannot_be_compared_or_returned_as_data() {
    let source = r#"
webhook fn handle(request: HttpRequest) -> HttpResponse {
    match db_begin_read("catalog") {
        Ok(transaction) => if transaction == transaction {
            record { status: 200, headers: [], body: request.path }
        } else {
            record { status: 500, headers: [], body: request.path }
        },
        Err(error) => record { status: 503, headers: [], body: error },
    }
}
"#;

    assert_eq!(diagnostic_code(source), "K3010");
}

#[test]
fn database_operations_type_check_their_fixed_shapes() {
    for (source, expected) in [
        (
            r#"
webhook fn handle(request: HttpRequest) -> HttpResponse {
    match db_begin_read("catalog") {
        Ok(transaction) => match db_query(transaction, "count-visits", [1]) {
            Ok(rows) => record { status: 200, headers: [], body: rows },
            Err(error) => record { status: 500, headers: [], body: error },
        },
        Err(error) => record { status: 503, headers: [], body: error },
    }
}
"#,
            "K3001",
        ),
        (
            r#"
webhook fn handle(request: HttpRequest) -> HttpResponse {
    match db_begin_read("catalog") {
        Ok(transaction) => match db_commit(transaction, "extra") {
            Ok(done) => record { status: 200, headers: [], body: request.path },
            Err(error) => record { status: 500, headers: [], body: error },
        },
        Err(error) => record { status: 503, headers: [], body: error },
    }
}
"#,
            "K3003",
        ),
    ] {
        assert_eq!(diagnostic_code(source), expected, "source: {source}");
    }
}

#[test]
fn execute_returns_a_typed_row_count_and_query_returns_text() {
    let source = r#"
webhook fn handle(request: HttpRequest) -> HttpResponse {
    match db_begin_write("catalog") {
        Ok(transaction) => match db_execute(transaction, "record-visit", [request.path]) {
            Ok(changed) => match db_commit(transaction) {
                Ok(done) => record { status: 200 + changed, headers: [], body: request.path },
                Err(error) => record { status: 500, headers: [], body: error },
            },
            Err(error) => record { status: 500, headers: [], body: error },
        },
        Err(error) => record { status: 503, headers: [], body: error },
    }
}
"#;

    analyze_source(source).expect("an Int row count should be usable as an integer");
}

#[test]
fn database_sources_lower_and_format_canonically() {
    let source = Source::new("database.krit", REFERENCE.trim_start());
    let program = parse_source(&source).expect("source should parse");
    let analysis = analyze(&program).expect("source should analyze");
    lower(&program, &analysis).expect("source should lower to Core");

    let formatted = format_source(&source).expect("source should format");
    let reformatted = format_source(&Source::new("database.krit", formatted.clone()))
        .expect("formatted source should format");

    assert_eq!(formatted, reformatted);
    analyze_source(&formatted).expect("formatted source should still analyze");
}

#[test]
fn database_names_stay_usable_as_ordinary_identifiers() {
    let source = r#"
fn describe(transaction: String, statement: String) -> String {
    transaction
}

let statement = "not-a-keyword";
"#;

    analyze_source(source).expect("database vocabulary must not reserve identifiers");
}
