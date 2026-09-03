use krit::{Source, analyze, lower, parse_source};
use krit_wasm::{
    ArtifactMetadata, BuildOptions, DATABASE_INTERFACE, STATE_ARTIFACT_POLICY_VERSION,
    WEBHOOK_INTERFACE, build_component, validate_artifact, validate_component,
};

fn compile(source_text: &str, effects: &[&str]) -> krit_wasm::BuiltComponent {
    let source = Source::new("database.krit", source_text);
    let program = parse_source(&source).expect("test source should parse");
    let analysis = analyze(&program).expect("test source should analyze");
    let module = lower(&program, &analysis).expect("test source should lower");
    let mut options = BuildOptions::new("2026", "test/database", "1.0.0", "src/main.krit");
    for effect in effects {
        options.grant_effect(*effect);
    }
    build_component(&module, &options).expect("test source should build")
}

fn writer() -> krit_wasm::BuiltComponent {
    compile(
        r#"
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
"#,
        &["database.write"],
    )
}

#[test]
fn database_webhooks_select_the_narrow_database_world() {
    let artifact = writer();

    assert_eq!(
        artifact.metadata.world,
        "krit:runtime/webhook-db-program@0.2.0"
    );
    assert_eq!(artifact.metadata.imports, [DATABASE_INTERFACE]);
    assert_eq!(artifact.metadata.effects, ["database.write"]);
    assert_eq!(
        artifact
            .metadata
            .requirements
            .iter()
            .map(|requirement| (
                requirement.capability.as_str(),
                requirement.resource.as_str()
            ))
            .collect::<Vec<_>>(),
        [("database.write", "catalog")]
    );
    assert_eq!(
        artifact.metadata.policy_version,
        STATE_ARTIFACT_POLICY_VERSION
    );

    let inspection = validate_artifact(&artifact.bytes, &artifact.metadata)
        .expect("database artifact should validate");
    assert_eq!(inspection.exports, [WEBHOOK_INTERFACE]);
}

#[test]
fn read_only_database_artifacts_never_carry_write_authority() {
    let artifact = compile(
        r#"
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
"#,
        &["database.read"],
    );

    assert_eq!(artifact.metadata.effects, ["database.read"]);
    assert_eq!(artifact.metadata.imports, [DATABASE_INTERFACE]);
    validate_artifact(&artifact.bytes, &artifact.metadata)
        .expect("read-only database artifact should validate");
}

#[test]
fn database_artifacts_are_byte_deterministic() {
    let first = writer();
    let second = writer();

    assert_eq!(first.bytes, second.bytes);
    assert_eq!(first.metadata.digest, second.metadata.digest);
}

#[test]
fn database_components_revalidate_from_their_own_bytes() {
    let artifact = writer();

    let inspection =
        validate_component(&artifact.bytes).expect("component should validate on its own");

    assert_eq!(inspection.world, artifact.metadata.world);
    assert_eq!(inspection.effects, artifact.metadata.effects);
    assert_eq!(inspection.requirements, artifact.metadata.requirements);
    assert_eq!(inspection.imports, artifact.metadata.imports);
}

#[test]
fn tampered_database_metadata_is_rejected() {
    let artifact = writer();

    for mutate in [
        |metadata: &mut ArtifactMetadata| {
            metadata.requirements[0].resource = "other-database".to_owned();
        },
        |metadata: &mut ArtifactMetadata| {
            // Downgrading the declared authority must not silently succeed.
            metadata.effects = vec!["database.read".to_owned()];
            metadata.requirements[0].capability = "database.read".to_owned();
        },
        |metadata: &mut ArtifactMetadata| {
            metadata.effects.clear();
            metadata.requirements.clear();
        },
        |metadata: &mut ArtifactMetadata| metadata.imports.clear(),
        |metadata: &mut ArtifactMetadata| metadata.policy_version = 1,
        |metadata: &mut ArtifactMetadata| {
            metadata.world = "krit:runtime/webhook-program@0.2.0".to_owned();
        },
    ] {
        let mut metadata = artifact.metadata.clone();
        mutate(&mut metadata);
        validate_artifact(&artifact.bytes, &metadata)
            .expect_err("tampered database metadata must be rejected");
    }
}

#[test]
fn ungranted_database_effects_fail_the_build_closed() {
    let source = Source::new(
        "database.krit",
        r#"
webhook fn handle(request: HttpRequest) -> HttpResponse {
    match db_begin_write("catalog") {
        Ok(transaction) => match db_commit(transaction) {
            Ok(committed) => record { status: 200, headers: [], body: request.path },
            Err(error) => record { status: 500, headers: [], body: error },
        },
        Err(error) => record { status: 503, headers: [], body: error },
    }
}
"#,
    );
    let program = parse_source(&source).expect("source should parse");
    let analysis = analyze(&program).expect("source should analyze");
    let module = lower(&program, &analysis).expect("source should lower");
    let options = BuildOptions::new("2026", "test/database", "1.0.0", "src/main.krit");

    let error = build_component(&module, &options)
        .expect_err("an ungranted database effect must fail the build");

    assert_eq!(error.kind(), krit_wasm::BuildErrorKind::Capability);
}

#[test]
fn database_free_artifacts_keep_their_existing_shape() {
    let artifact = compile(
        r#"
webhook fn handle(request: HttpRequest) -> HttpResponse {
    record { status: 200, headers: [], body: request.path }
}
"#,
        &[],
    );

    assert_eq!(
        artifact.metadata.world,
        "krit:runtime/webhook-program@0.2.0"
    );
    assert!(artifact.metadata.imports.is_empty());
    assert!(artifact.metadata.effects.is_empty());
    assert_eq!(artifact.metadata.policy_version, 1);
}
