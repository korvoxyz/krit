//! Artifacts built by earlier supported toolchains keep loading.
//!
//! The golden artifacts under `conformance/build/compat` were written by
//! released Krit binaries. These tests pin that they validate on the current
//! runtime by supported-set membership and the toolchain window, never by an
//! exact compiler-version match, while self-consistency stays exact.

use std::{fs, path::PathBuf};

use krit_wasm::{
    ArtifactMetadata, BuildErrorKind, EMBEDDED_METADATA_SECTION, PROGRAM_WORLD, STDOUT_INTERFACE,
    digest_bytes, toolchain_in_window, validate_artifact, validate_component,
};
use wasm_encoder::{Component, CustomSection, RawSection};
use wasmparser::{Parser, Payload};

struct Golden {
    bytes: Vec<u8>,
    metadata: ArtifactMetadata,
}

/// Rewrites one adjacent metadata fact; the second argument is a version one
/// minor newer than the running toolchain.
type Tamper = fn(&mut ArtifactMetadata, &str);

fn golden(toolchain: &str, case: &str) -> Golden {
    let directory = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../conformance/build/compat")
        .join(toolchain)
        .join(case);
    let bytes = fs::read(directory.join("artifact.wasm")).expect("golden artifact should exist");
    let metadata = fs::read(directory.join("artifact.wasm.json"))
        .expect("golden artifact metadata should exist");
    let metadata = serde_json::from_slice(&metadata).expect("golden metadata should use schema 1");
    Golden { bytes, metadata }
}

/// A version one minor newer than the running toolchain, so these tests keep
/// meaning the same thing after every version bump.
fn next_minor() -> String {
    let mut parts = env!("CARGO_PKG_VERSION").split('.');
    let major = parts.next().expect("major version");
    let minor = parts
        .next()
        .and_then(|minor| minor.parse::<u64>().ok())
        .expect("minor version");
    format!("{major}.{}.0", minor + 1)
}

#[test]
fn golden_0_2_0_artifacts_load_with_effects_rederived_from_imports() {
    let factorial = golden("0.2.0", "factorial");
    assert_eq!(factorial.metadata.compiler.version, "0.2.0");
    assert!(toolchain_in_window("0.2.0", env!("CARGO_PKG_VERSION")));
    let inspection = validate_artifact(&factorial.bytes, &factorial.metadata)
        .expect("0.2.0 module artifact should load");
    assert_eq!(inspection.compiler_version, "0.2.0");
    assert_eq!(inspection.language_version, "0.2.0");
    assert_eq!(inspection.edition, "2026");
    assert_eq!(inspection.world, PROGRAM_WORLD);
    assert_eq!(inspection.imports, [STDOUT_INTERFACE]);
    assert_eq!(inspection.exports, ["run"]);
    assert_eq!(inspection.effects, ["io.stdout"]);
    assert_eq!(factorial.metadata.policy_version, 1);

    let webhook = golden("0.2.0", "stateful-webhook");
    let inspection = validate_artifact(&webhook.bytes, &webhook.metadata)
        .expect("0.2.0 policy-2 webhook artifact should load");
    assert_eq!(inspection.compiler_version, "0.2.0");
    assert_eq!(inspection.world, "krit:runtime/webhook-state-program@0.2.0");
    assert_eq!(inspection.imports, ["krit:runtime/state@0.2.0"]);
    assert_eq!(inspection.exports, ["krit:runtime/webhook@0.2.0"]);
    assert_eq!(inspection.effects, ["state.transaction"]);
    assert_eq!(webhook.metadata.policy_version, 2);
    assert_eq!(inspection.requirements, webhook.metadata.requirements);
}

#[test]
fn adjacent_metadata_outside_the_supported_sets_is_refused() {
    let golden = golden("0.2.0", "factorial");
    let newer = next_minor();
    let cases: [(&str, Tamper); 7] = [
        ("newer compiler", |metadata, newer| {
            metadata.compiler.version = newer.to_owned();
        }),
        ("other major compiler", |metadata, _| {
            metadata.compiler.version = "99.0.0".to_owned();
        }),
        ("unparsable compiler", |metadata, _| {
            metadata.compiler.version = "0.2".to_owned();
        }),
        ("newer language", |metadata, newer| {
            metadata.language.version = newer.to_owned();
        }),
        ("unsupported edition", |metadata, _| {
            metadata.edition = "2027".to_owned();
        }),
        ("unsupported schema", |metadata, _| metadata.schema = 2),
        ("unsupported policy", |metadata, _| {
            metadata.policy_version = 3
        }),
    ];
    for (name, tamper) in cases {
        let mut metadata = golden.metadata.clone();
        tamper(&mut metadata, &newer);
        let error =
            validate_artifact(&golden.bytes, &metadata).expect_err("tampered metadata must fail");
        assert_eq!(error.kind(), BuildErrorKind::Metadata, "{name}: {error}");
        assert_eq!(error.code(), "K7004", "{name}: {error}");
        assert!(error.message().contains("artifact metadata"), "{name}");
    }

    let mut metadata = golden.metadata.clone();
    metadata.compiler.version = newer.clone();
    let error = validate_artifact(&golden.bytes, &metadata).expect_err("newer compiler must fail");
    assert!(
        error.message().contains("compatibility window"),
        "{}",
        error.message()
    );

    let mut metadata = golden.metadata.clone();
    metadata.edition = "\u{1b}[31m2027".to_owned();
    let error = validate_artifact(&golden.bytes, &metadata).expect_err("bad edition must fail");
    assert!(
        !error.message().contains('\u{1b}'),
        "untrusted text is echoed"
    );
}

#[test]
fn recorded_toolchain_must_still_agree_with_itself_exactly() {
    let golden = golden("0.2.0", "factorial");
    // Inside the window on its own, but not what the component records.
    let mut metadata = golden.metadata.clone();
    metadata.compiler.version = "0.2.1".to_owned();
    let error = validate_artifact(&golden.bytes, &metadata)
        .expect_err("adjacent compiler must match embedded compiler");
    assert_eq!(error.kind(), BuildErrorKind::Metadata);
    assert!(error.message().contains("does not match the component"));

    let mut metadata = golden.metadata.clone();
    metadata.language.version = "0.2.1".to_owned();
    let error = validate_artifact(&golden.bytes, &metadata)
        .expect_err("adjacent language must match producers metadata");
    assert!(error.message().contains("does not match the component"));
}

#[test]
fn embedded_metadata_outside_the_window_or_disagreeing_with_producers_is_refused() {
    let golden = golden("0.2.0", "factorial");
    let embedded = |compiler: &str, edition: &str, schema: u32| {
        serde_json::json!({
            "schema": schema,
            "compilerVersion": compiler,
            "edition": edition,
            "world": PROGRAM_WORLD,
            "effects": ["io.stdout"],
            "policyVersion": 1,
        })
    };

    let untouched = replace_embedded_metadata(&golden.bytes, &embedded("0.2.0", "2026", 1));
    validate_component(&untouched).expect("re-encoding the same facts should still load");

    for (name, facts, expected) in [
        (
            "newer compiler",
            embedded(&next_minor(), "2026", 1),
            "compatibility window",
        ),
        (
            "unsupported edition",
            embedded("0.2.0", "2027", 1),
            "edition",
        ),
        (
            "unsupported schema",
            embedded("0.2.0", "2026", 2),
            "metadata schema 2",
        ),
        (
            "producers disagree",
            embedded("0.2.1", "2026", 1),
            "producers metadata does not match",
        ),
    ] {
        let bytes = replace_embedded_metadata(&golden.bytes, &facts);
        let error = validate_component(&bytes).expect_err("tampered embedded facts must fail");
        assert_eq!(error.kind(), BuildErrorKind::InvalidArtifact, "{name}");
        assert_eq!(error.code(), "K7003", "{name}");
        assert!(
            error.message().contains(expected),
            "{name}: {}",
            error.message()
        );

        let mut metadata = golden.metadata.clone();
        metadata.digest = digest_bytes(&bytes);
        metadata.byte_size = bytes.len() as u64;
        validate_artifact(&bytes, &metadata).expect_err("tampered artifact must not load");
    }
}

#[test]
fn unsupported_runtime_wit_package_versions_are_named() {
    let golden = golden("0.2.0", "factorial");
    let mut bytes = golden.bytes.clone();
    let from = STDOUT_INTERFACE.as_bytes();
    let to = b"krit:runtime/stdout@0.9.0";
    assert_eq!(from.len(), to.len());
    let mut replaced = 0;
    for index in 0..=bytes.len() - from.len() {
        if &bytes[index..index + from.len()] == from {
            bytes[index..index + to.len()].copy_from_slice(to);
            replaced += 1;
        }
    }
    assert!(replaced > 0, "stdout import name should exist");

    let error = validate_component(&bytes).expect_err("unknown WIT version must fail");
    assert_eq!(error.kind(), BuildErrorKind::InvalidArtifact);
    assert!(
        error.message().contains("WIT package version"),
        "{}",
        error.message()
    );
}

fn replace_embedded_metadata(bytes: &[u8], metadata: &serde_json::Value) -> Vec<u8> {
    let metadata = serde_json::to_vec(metadata).expect("replacement metadata should serialize");
    let mut component = Component::new();
    let mut depth = 0u32;
    let mut replaced = false;
    for payload in Parser::new(0).parse_all(bytes) {
        let payload = payload.expect("component should parse");
        let top_level = depth == 0;
        let is_metadata = matches!(
            &payload,
            Payload::CustomSection(section)
                if top_level && section.name() == EMBEDDED_METADATA_SECTION
        );
        if top_level && !is_metadata {
            if let Some((id, range)) = payload.as_section() {
                let start = usize::try_from(range.start).expect("section offset should fit usize");
                let end = usize::try_from(range.end).expect("section offset should fit usize");
                component.section(&RawSection {
                    id,
                    data: &bytes[start..end],
                });
            }
        } else if is_metadata {
            replaced = true;
        }
        match payload {
            Payload::ModuleSection { .. } | Payload::ComponentSection { .. } => depth += 1,
            Payload::End(_) if depth > 0 => depth -= 1,
            _ => {}
        }
    }
    assert!(replaced, "component metadata section should exist");
    component.section(&CustomSection {
        name: EMBEDDED_METADATA_SECTION.into(),
        data: metadata.into(),
    });
    component.finish()
}
