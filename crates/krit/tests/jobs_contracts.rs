use krit::{
    Effect, EntrypointKind, Source, SymbolKind, Type, analyze, format_source, lower, parse_source,
};

fn analyze_source(text: &str) -> Result<krit::Analysis, krit::Diagnostic> {
    let source = Source::new("jobs.krit", text);
    let program = parse_source(&source)?;
    analyze(&program)
}

fn diagnostic_code(text: &str) -> &'static str {
    analyze_or_parse_error(text).code()
}

fn analyze_or_parse_error(text: &str) -> krit::Diagnostic {
    let source = Source::new("jobs.krit", text);
    match parse_source(&source) {
        Ok(program) => analyze(&program).expect_err("source should fail checking"),
        Err(diagnostic) => diagnostic,
    }
}

fn entrypoint_facts(text: &str, kind: SymbolKind) -> (Vec<&'static str>, Vec<(String, String)>) {
    let analysis = analyze_source(text).expect("source should analyze");
    let symbol = analysis
        .symbols()
        .iter()
        .find(|symbol| symbol.kind() == kind)
        .expect("entrypoint symbol should exist");
    let Type::Function(function) = symbol.ty() else {
        panic!("entrypoint should have function type")
    };
    (
        function
            .effects()
            .iter()
            .map(Effect::as_str)
            .collect::<Vec<_>>(),
        function
            .requirements()
            .iter()
            .map(|requirement| {
                (
                    requirement.capability().as_str().to_owned(),
                    requirement.resource().to_owned(),
                )
            })
            .collect::<Vec<_>>(),
    )
}

const QUEUE_WORKER: &str = r#"
queue "render-jobs" fn handle(job: QueueJob) -> Result<String, String> {
    match object_put("render-output", job.id, job.body) {
        Ok(stored) => Ok(job.queue),
        Err(error) => Err(error),
    }
}
"#;

const SCHEDULE_HANDLER: &str = r#"
schedule "hourly-sweep" fn handle(event: ScheduleEvent) -> Result<String, String> {
    match object_get("render-output", event.id) {
        Ok(found) => Ok(event.schedule),
        Err(error) => Err(error),
    }
}
"#;

#[test]
fn queue_consumer_facts_report_exact_consume_and_object_authority() {
    let (effects, requirements) = entrypoint_facts(QUEUE_WORKER, SymbolKind::QueueConsumer);

    assert_eq!(effects, ["object.write", "queue.consume"]);
    assert_eq!(
        requirements,
        [
            ("object.write".to_owned(), "render-output".to_owned()),
            ("queue.consume".to_owned(), "render-jobs".to_owned()),
        ]
    );
}

#[test]
fn schedule_handler_facts_report_exact_trigger_and_read_authority() {
    let (effects, requirements) = entrypoint_facts(SCHEDULE_HANDLER, SymbolKind::ScheduleHandler);

    assert_eq!(effects, ["object.read", "schedule.trigger"]);
    assert_eq!(
        requirements,
        [
            ("object.read".to_owned(), "render-output".to_owned()),
            ("schedule.trigger".to_owned(), "hourly-sweep".to_owned()),
        ]
    );
}

#[test]
fn queue_publish_reports_exact_publish_authority_without_consume() {
    let source = r#"
webhook fn handle(request: HttpRequest) -> HttpResponse {
    match queue_publish("render-jobs", request.body) {
        Ok(id) => record { status: 202, headers: [], body: id },
        Err(error) => record { status: 500, headers: [], body: error },
    }
}
"#;

    let (effects, requirements) = entrypoint_facts(source, SymbolKind::Webhook);

    assert_eq!(effects, ["queue.publish"]);
    assert_eq!(
        requirements,
        [("queue.publish".to_owned(), "render-jobs".to_owned())]
    );
}

#[test]
fn queue_and_schedule_entrypoints_lower_to_typed_core_boundaries() {
    for (text, kind) in [
        (QUEUE_WORKER, EntrypointKind::QueueConsumer),
        (SCHEDULE_HANDLER, EntrypointKind::ScheduleHandler),
    ] {
        let source = Source::new("jobs.krit", text);
        let program = parse_source(&source).expect("source should parse");
        let analysis = analyze(&program).expect("source should analyze");
        let module = lower(&program, &analysis).expect("source should lower");

        let entrypoints = module.entrypoints();
        assert_eq!(entrypoints.len(), 2);
        assert_eq!(entrypoints[0].kind, EntrypointKind::ModuleInit);
        assert_eq!(entrypoints[1].kind, kind);
        assert!(kind.is_exported());
        let function = &module.functions()[entrypoints[1].function.as_u32() as usize];
        assert_eq!(function.signature.parameters.len(), 1);
        assert_eq!(
            function.signature.result.as_ref(),
            &Type::Result(
                std::sync::Arc::new(Type::String),
                std::sync::Arc::new(Type::String)
            )
        );
        assert!(module.render_text().contains(kind.as_str()));
    }
}

#[test]
fn job_and_schedule_signatures_are_fixed() {
    for source in [
        r#"queue "render-jobs" fn handle(job: HttpRequest) -> Result<String, String> { Ok("") }"#,
        r#"queue "render-jobs" fn handle(job: QueueJob) -> String { "" }"#,
        r#"queue "render-jobs" fn handle(job: QueueJob, extra: String) -> Result<String, String> { Ok("") }"#,
        r#"schedule "sweep" fn handle(event: QueueJob) -> Result<String, String> { Ok("") }"#,
        r#"schedule "sweep" fn handle(event: ScheduleEvent) -> Result<Int, String> { Ok(1) }"#,
    ] {
        assert_eq!(diagnostic_code(source), "K3007", "source: {source}");
    }
}

#[test]
fn entrypoint_resource_names_must_be_canonical_literals() {
    assert_eq!(
        diagnostic_code(
            r#"queue "Render Jobs" fn handle(job: QueueJob) -> Result<String, String> { Ok("") }"#
        ),
        "K3008"
    );
    assert_eq!(
        diagnostic_code(
            r#"schedule "-bad-" fn handle(event: ScheduleEvent) -> Result<String, String> { Ok("") }"#
        ),
        "K3008"
    );
}

#[test]
fn queue_and_object_resources_are_direct_canonical_literals() {
    for source in [
        r#"let name = "render-jobs"; queue_publish(name, "body");"#,
        r#"queue_publish("Render Jobs", "body");"#,
        r#"object_get("Render Output", "key");"#,
        r#"let put = object_put; put("render-output", "key", "value");"#,
        r#"object_delete("render output", "key");"#,
    ] {
        assert_eq!(diagnostic_code(source), "K3008", "source: {source}");
    }
}

#[test]
fn a_module_declares_at_most_one_typed_entrypoint() {
    let source = r#"
webhook fn handle(request: HttpRequest) -> HttpResponse {
    record { status: 200, headers: [], body: "" }
}

queue "render-jobs" fn work(job: QueueJob) -> Result<String, String> {
    Ok(job.id)
}
"#;

    assert_eq!(diagnostic_code(source), "K2002");
}

#[test]
fn entrypoints_are_rejected_inside_nested_scopes() {
    let source = r#"
fn outer() -> Int {
    queue "render-jobs" fn inner(job: QueueJob) -> Result<String, String> { Ok(job.id) }
    1
}
"#;

    assert_eq!(diagnostic_code(source), "K1004");
}

#[test]
fn queue_and_schedule_stay_usable_as_ordinary_names() {
    let source = r#"
fn describe(queue: String, schedule: String) -> String {
    queue
}

let queue = "not-a-keyword";
"#;

    analyze_source(source).expect("contextual keywords should not reserve identifiers");
}

#[test]
fn delivery_contract_fields_are_typed_and_closed() {
    let source = r#"
queue "render-jobs" fn handle(job: QueueJob) -> Result<String, String> {
    if job.attempt < job.maxAttempts { Ok(job.body) } else { Err(job.queue) }
}
"#;
    analyze_source(source).expect("typed delivery fields should check");

    assert_eq!(
        diagnostic_code(
            r#"queue "render-jobs" fn handle(job: QueueJob) -> Result<String, String> { Ok(job.missing) }"#
        ),
        "K3004"
    );
    assert_eq!(
        diagnostic_code(
            r#"schedule "sweep" fn handle(event: ScheduleEvent) -> Result<String, String> { Ok(event.scheduledAtMillis) }"#
        ),
        "K3001"
    );
}

#[test]
fn canonical_formatting_round_trips_entrypoint_declarations() {
    for text in [QUEUE_WORKER, SCHEDULE_HANDLER] {
        let source = Source::new("jobs.krit", text.trim_start());
        let formatted = format_source(&source).expect("source should format");
        let reformatted = format_source(&Source::new("jobs.krit", formatted.clone()))
            .expect("formatted source should format");

        assert_eq!(formatted, reformatted);
        assert!(
            formatted.contains("queue \"render-jobs\" fn") || formatted.contains("schedule \"")
        );
        analyze_source(&formatted).expect("formatted source should still analyze");
    }
}
