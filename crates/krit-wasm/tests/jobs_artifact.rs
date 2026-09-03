use krit::{Source, analyze, lower, parse_source};
use krit_wasm::{
    ArtifactMetadata, BuildOptions, JOB_INTERFACE, OBJECTS_READ_INTERFACE, OBJECTS_WRITE_INTERFACE,
    QUEUE_INTERFACE, SCHEDULE_INTERFACE, STATE_ARTIFACT_POLICY_VERSION, STATE_INTERFACE,
    WEBHOOK_INTERFACE, build_component, validate_artifact, validate_component,
};

fn compile(source_text: &str, effects: &[&str]) -> krit_wasm::BuiltComponent {
    let source = Source::new("jobs.krit", source_text);
    let program = parse_source(&source).expect("test source should parse");
    let analysis = analyze(&program).expect("test source should analyze");
    let module = lower(&program, &analysis).expect("test source should lower");
    let mut options = BuildOptions::new("2026", "test/jobs", "1.0.0", "src/main.krit");
    for effect in effects {
        options.grant_effect(*effect);
    }
    build_component(&module, &options).expect("test source should build")
}

fn queue_worker() -> krit_wasm::BuiltComponent {
    compile(
        r#"
queue "render-jobs" fn handle(job: QueueJob) -> Result<String, String> {
    match object_get("render-output", job.id) {
        Ok(existing) => match existing {
            Some(previous) => Ok(previous),
            None => match object_put("render-output", job.id, job.body) {
                Ok(stored) => match checkpoint_put("agent-work", "last-render", job.id) {
                    Ok(marked) => Ok(job.id),
                    Err(error) => Err(error),
                },
                Err(error) => Err(error),
            },
        },
        Err(error) => Err(error),
    }
}
"#,
        &[
            "object.read",
            "object.write",
            "queue.consume",
            "state.transaction",
        ],
    )
}

#[test]
fn queue_workers_select_the_least_authority_job_world() {
    let artifact = queue_worker();

    assert_eq!(
        artifact.metadata.world,
        "krit:runtime/job-state-objread-objwrite-program@0.2.0"
    );
    assert_eq!(
        artifact.metadata.imports,
        [
            OBJECTS_READ_INTERFACE,
            OBJECTS_WRITE_INTERFACE,
            STATE_INTERFACE
        ]
    );
    assert_eq!(
        artifact.metadata.effects,
        [
            "object.read",
            "object.write",
            "queue.consume",
            "state.transaction"
        ]
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
            ("object.read", "render-output"),
            ("object.write", "render-output"),
            ("queue.consume", "render-jobs"),
            ("state.transaction", "agent-work"),
        ]
    );
    assert_eq!(
        artifact.metadata.policy_version,
        STATE_ARTIFACT_POLICY_VERSION
    );

    let inspection = validate_artifact(&artifact.bytes, &artifact.metadata)
        .expect("queue artifact should validate");
    assert_eq!(inspection.exports, [JOB_INTERFACE]);
    assert_eq!(inspection.memory_count, 1);
}

#[test]
fn schedule_handlers_select_the_least_authority_schedule_world() {
    let artifact = compile(
        r#"
schedule "hourly-sweep" fn handle(event: ScheduleEvent) -> Result<String, String> {
    match object_put("render-output", event.id, event.schedule) {
        Ok(stored) => Ok(event.id),
        Err(error) => Err(error),
    }
}
"#,
        &["object.read", "object.write", "schedule.trigger"],
    );

    assert_eq!(
        artifact.metadata.world,
        "krit:runtime/schedule-objwrite-program@0.2.0"
    );
    assert_eq!(artifact.metadata.imports, [OBJECTS_WRITE_INTERFACE]);
    assert_eq!(
        artifact.metadata.effects,
        ["object.write", "schedule.trigger"]
    );
    let inspection = validate_artifact(&artifact.bytes, &artifact.metadata)
        .expect("schedule artifact should validate");
    assert_eq!(inspection.exports, [SCHEDULE_INTERFACE]);
}

#[test]
fn queue_publishers_stay_webhook_shaped_with_publish_only_authority() {
    let artifact = compile(
        r#"
webhook fn handle(request: HttpRequest) -> HttpResponse {
    match queue_publish("render-jobs", request.body) {
        Ok(id) => record { status: 202, headers: [], body: id },
        Err(error) => record { status: 500, headers: [], body: error },
    }
}
"#,
        &["queue.publish"],
    );

    assert_eq!(
        artifact.metadata.world,
        "krit:runtime/webhook-queue-program@0.2.0"
    );
    assert_eq!(artifact.metadata.imports, [QUEUE_INTERFACE]);
    assert_eq!(artifact.metadata.effects, ["queue.publish"]);
    let inspection = validate_artifact(&artifact.bytes, &artifact.metadata)
        .expect("publisher artifact should validate");
    assert_eq!(inspection.exports, [WEBHOOK_INTERFACE]);
}

#[test]
fn job_artifacts_are_byte_deterministic() {
    let first = queue_worker();
    let second = queue_worker();

    assert_eq!(first.bytes, second.bytes);
    assert_eq!(first.metadata.digest, second.metadata.digest);
}

#[test]
fn tampered_job_metadata_is_rejected() {
    let artifact = queue_worker();

    for mutate in [
        |metadata: &mut ArtifactMetadata| {
            metadata.requirements[2].resource = "other-queue".to_owned();
        },
        |metadata: &mut ArtifactMetadata| {
            metadata.effects.retain(|effect| effect != "queue.consume");
        },
        |metadata: &mut ArtifactMetadata| {
            metadata.world = "krit:runtime/job-program@0.2.0".to_owned();
        },
        |metadata: &mut ArtifactMetadata| {
            metadata.imports.push("krit:runtime/queue@0.2.0".to_owned());
        },
        |metadata: &mut ArtifactMetadata| metadata.policy_version = 1,
    ] {
        let mut metadata = artifact.metadata.clone();
        mutate(&mut metadata);
        validate_artifact(&artifact.bytes, &metadata)
            .expect_err("tampered job metadata must be rejected");
    }
}

#[test]
fn job_components_revalidate_from_their_own_bytes() {
    let artifact = queue_worker();

    let inspection =
        validate_component(&artifact.bytes).expect("component should validate on its own");

    assert_eq!(inspection.world, artifact.metadata.world);
    assert_eq!(inspection.effects, artifact.metadata.effects);
    assert_eq!(inspection.requirements, artifact.metadata.requirements);
    assert_eq!(inspection.imports, artifact.metadata.imports);
}

#[test]
fn ungranted_job_effects_fail_the_build_closed() {
    let source = Source::new(
        "jobs.krit",
        r#"
webhook fn handle(request: HttpRequest) -> HttpResponse {
    match queue_publish("render-jobs", request.body) {
        Ok(id) => record { status: 202, headers: [], body: id },
        Err(error) => record { status: 500, headers: [], body: error },
    }
}
"#,
    );
    let program = parse_source(&source).expect("source should parse");
    let analysis = analyze(&program).expect("source should analyze");
    let module = lower(&program, &analysis).expect("source should lower");
    let options = BuildOptions::new("2026", "test/jobs", "1.0.0", "src/main.krit");

    let error = build_component(&module, &options)
        .expect_err("an ungranted publish effect must fail the build");

    assert_eq!(error.kind(), krit_wasm::BuildErrorKind::Capability);
}
