use krit::{Source, analyze, lower, parse_source};
use krit_wasm::{
    ArtifactMetadata, BuildOptions, CACHE_READ_INTERFACE, CACHE_WRITE_INTERFACE,
    SEARCH_QUERY_INTERFACE, SEARCH_VECTOR_INTERFACE, STATE_ARTIFACT_POLICY_VERSION,
    WEBHOOK_INTERFACE, build_component, validate_artifact, validate_component,
};

fn compile(source_text: &str, effects: &[&str]) -> krit_wasm::BuiltComponent {
    let source = Source::new("cache.krit", source_text);
    let program = parse_source(&source).expect("source should parse");
    let analysis = analyze(&program).expect("source should analyze");
    let module = lower(&program, &analysis).expect("source should lower");
    let mut options = BuildOptions::new("2026", "test/cache", "1.0.0", "src/main.krit");
    for effect in effects {
        options.grant_effect(*effect);
    }
    build_component(&module, &options).expect("source should build")
}

const CACHED_SEARCH: &str = r#"
webhook fn handle(request: HttpRequest) -> HttpResponse {
    match cache_get("lookups", request.query) {
        Ok(found) => match found {
            Some(value) => record { status: 200, headers: [], body: value },
            None => match search_query("docs", request.query, 3) {
                Ok(results) => match cache_put("lookups", request.query, results, 60) {
                    Ok(stored) => record { status: 200, headers: [], body: results },
                    Err(problem) => record { status: 200, headers: [], body: results },
                },
                Err(problem) => record { status: 502, headers: [], body: problem },
            },
        },
        Err(outage) => record { status: 503, headers: [], body: outage },
    }
}
"#;

fn cached_search() -> krit_wasm::BuiltComponent {
    compile(
        CACHED_SEARCH,
        &["cache.read", "cache.write", "search.query"],
    )
}

#[test]
fn a_cached_search_selects_exactly_three_narrow_interfaces() {
    let artifact = cached_search();

    assert_eq!(
        artifact.metadata.world,
        "krit:runtime/webhook-cacheread-cachewrite-searchquery-program@0.2.0"
    );
    assert_eq!(
        artifact.metadata.imports,
        [
            CACHE_READ_INTERFACE,
            CACHE_WRITE_INTERFACE,
            SEARCH_QUERY_INTERFACE
        ]
    );
    assert_eq!(
        artifact.metadata.effects,
        ["cache.read", "cache.write", "search.query"]
    );
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
        [
            ("cache.read", "lookups"),
            ("cache.write", "lookups"),
            ("search.query", "docs")
        ]
    );
    assert_eq!(
        artifact.metadata.policy_version,
        STATE_ARTIFACT_POLICY_VERSION
    );

    let inspection =
        validate_artifact(&artifact.bytes, &artifact.metadata).expect("artifact should validate");
    assert_eq!(inspection.exports, [WEBHOOK_INTERFACE]);
}

#[test]
fn read_only_cache_use_never_imports_the_write_interface() {
    let artifact = compile(
        r#"
webhook fn handle(request: HttpRequest) -> HttpResponse {
    match cache_get("lookups", request.query) {
        Ok(found) => match found {
            Some(value) => record { status: 200, headers: [], body: value },
            None => record { status: 404, headers: [], body: "miss" },
        },
        Err(outage) => record { status: 503, headers: [], body: outage },
    }
}
"#,
        &["cache.read"],
    );

    assert_eq!(artifact.metadata.imports, [CACHE_READ_INTERFACE]);
    assert_eq!(artifact.metadata.effects, ["cache.read"]);
    validate_artifact(&artifact.bytes, &artifact.metadata).expect("artifact should validate");
}

#[test]
fn a_vector_program_selects_only_the_vector_interface() {
    let artifact = compile(
        r#"
webhook fn handle(request: HttpRequest) -> HttpResponse {
    match vector_search("vectors", request.body, 3) {
        Ok(results) => record { status: 200, headers: [], body: results },
        Err(problem) => record { status: 502, headers: [], body: problem },
    }
}
"#,
        &["search.vector"],
    );

    assert_eq!(artifact.metadata.imports, [SEARCH_VECTOR_INTERFACE]);
    assert_eq!(artifact.metadata.effects, ["search.vector"]);
    validate_artifact(&artifact.bytes, &artifact.metadata).expect("artifact should validate");
}

#[test]
fn cache_artifacts_are_byte_deterministic() {
    let first = cached_search();
    let second = cached_search();

    assert_eq!(first.bytes, second.bytes);
    assert_eq!(first.metadata.digest, second.metadata.digest);
}

#[test]
fn cache_components_revalidate_from_their_own_bytes() {
    let artifact = cached_search();

    let inspection =
        validate_component(&artifact.bytes).expect("component should validate on its own");

    assert_eq!(inspection.world, artifact.metadata.world);
    assert_eq!(inspection.effects, artifact.metadata.effects);
    assert_eq!(inspection.requirements, artifact.metadata.requirements);
    assert_eq!(inspection.imports, artifact.metadata.imports);
}

#[test]
fn tampered_cache_metadata_is_rejected() {
    let artifact = cached_search();

    for mutate in [
        |metadata: &mut ArtifactMetadata| {
            metadata.requirements[0].resource = "other-namespace".to_owned();
        },
        |metadata: &mut ArtifactMetadata| {
            // Silently widening a read grant into a write grant must fail.
            metadata.requirements[0].capability = "cache.write".to_owned();
        },
        |metadata: &mut ArtifactMetadata| {
            metadata.effects = vec!["cache.read".to_owned()];
        },
        |metadata: &mut ArtifactMetadata| metadata.effects.clear(),
        |metadata: &mut ArtifactMetadata| metadata.requirements.clear(),
        |metadata: &mut ArtifactMetadata| metadata.imports.clear(),
        |metadata: &mut ArtifactMetadata| {
            metadata.imports = vec![CACHE_READ_INTERFACE.to_owned()];
        },
        |metadata: &mut ArtifactMetadata| metadata.policy_version = 1,
        |metadata: &mut ArtifactMetadata| {
            metadata.world = "krit:runtime/webhook-program@0.2.0".to_owned();
        },
    ] {
        let mut metadata = artifact.metadata.clone();
        mutate(&mut metadata);
        validate_artifact(&artifact.bytes, &metadata)
            .expect_err("tampered cache metadata must be rejected");
    }
}

#[test]
fn ungranted_cache_and_search_effects_fail_the_build_closed() {
    for (source_text, granted) in [
        (CACHED_SEARCH, vec!["cache.read", "cache.write"]),
        (CACHED_SEARCH, vec!["search.query"]),
        (CACHED_SEARCH, vec![]),
    ] {
        let source = Source::new("cache.krit", source_text);
        let program = parse_source(&source).expect("source should parse");
        let analysis = analyze(&program).expect("source should analyze");
        let module = lower(&program, &analysis).expect("source should lower");
        let mut options = BuildOptions::new("2026", "test/cache", "1.0.0", "src/main.krit");
        for effect in &granted {
            options.grant_effect(*effect);
        }

        let error = build_component(&module, &options)
            .expect_err("an ungranted effect must fail the build");

        assert_eq!(error.kind(), krit_wasm::BuildErrorKind::Capability);
    }
}

#[test]
fn cache_free_artifacts_keep_their_existing_shape() {
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
    assert_eq!(
        artifact.metadata.policy_version, 1,
        "an unrelated program must keep policy 1"
    );
}

#[test]
fn cache_worlds_compose_with_other_host_surfaces() {
    let artifact = compile(
        r#"
webhook fn handle(request: HttpRequest) -> HttpResponse {
    match cache_get("lookups", request.query) {
        Ok(found) => match state_get("work", "counter") {
            Ok(previous) => record { status: 200, headers: [], body: "ok" },
            Err(problem) => record { status: 500, headers: [], body: problem },
        },
        Err(outage) => record { status: 503, headers: [], body: outage },
    }
}
"#,
        &["cache.read", "state.transaction"],
    );

    assert_eq!(
        artifact.metadata.effects,
        ["cache.read", "state.transaction"]
    );
    assert_eq!(
        artifact.metadata.world,
        "krit:runtime/webhook-state-cacheread-program@0.2.0"
    );
    validate_artifact(&artifact.bytes, &artifact.metadata)
        .expect("a composed world should validate");
}
