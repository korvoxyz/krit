//! The current runtime executes artifacts built by earlier supported
//! toolchains.
//!
//! The golden artifacts under `conformance/build/compat` were written by
//! released Krit binaries and are loaded exactly as checked in.

use std::{
    collections::BTreeMap,
    fs,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use krit_package::Manifest;
use krit_runtime::{
    AgentHost, AgentHostPolicy, DenyAllApprovalPolicy, Durability, DurableState,
    DurableStoreDefinition, DurableStoreLimits, GrantSet, HostInputs, HttpRequest, RetentionPolicy,
    Runtime, SecretStore,
};
use krit_wasm::ArtifactMetadata;

static DIRECTORY_ID: AtomicU64 = AtomicU64::new(0);

struct Golden {
    bytes: Vec<u8>,
    metadata: ArtifactMetadata,
    manifest: Manifest,
}

fn golden(toolchain: &str, case: &str) -> Golden {
    let directory = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../conformance/build/compat")
        .join(toolchain)
        .join(case);
    let bytes = fs::read(directory.join("artifact.wasm")).expect("golden artifact should exist");
    let metadata = fs::read(directory.join("artifact.wasm.json"))
        .expect("golden artifact metadata should exist");
    let metadata: ArtifactMetadata =
        serde_json::from_slice(&metadata).expect("golden metadata should use schema 1");
    assert_eq!(metadata.compiler.version, toolchain);
    let manifest =
        Manifest::load(&directory.join("krit.pkg")).expect("golden package manifest should load");
    Golden {
        bytes,
        metadata,
        manifest,
    }
}

struct TestDirectory {
    path: PathBuf,
}

impl TestDirectory {
    fn new(name: &str) -> Self {
        let id = DIRECTORY_ID.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "krit-runtime-compat-{name}-{}-{id}",
            std::process::id()
        ));
        fs::create_dir_all(&path).expect("test directory should be created");
        Self { path }
    }
}

impl Drop for TestDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

fn durable(path: PathBuf) -> DurableState {
    DurableState::open(
        BTreeMap::from([(
            "agent-work".to_owned(),
            DurableStoreDefinition {
                path,
                durability: Durability::Full,
                limits: DurableStoreLimits {
                    busy_timeout: Duration::from_millis(250),
                    max_operations: 128,
                    max_key_bytes: 256,
                    max_value_bytes: 64 * 1024,
                    max_transaction_bytes: 1024 * 1024,
                    max_database_bytes: 64 * 1024 * 1024,
                    max_replay_entries: 1024,
                    max_replay_bytes: 16 * 1024 * 1024,
                },
                replay: RetentionPolicy {
                    max_entries: 1024,
                    max_bytes: 16 * 1024 * 1024,
                    ttl: Duration::from_secs(7 * 24 * 60 * 60),
                    lease: Duration::from_secs(30),
                },
            },
        )]),
        Some("agent-work".to_owned()),
    )
    .expect("durable state should open")
}

fn request(body: &str) -> HttpRequest {
    HttpRequest {
        method: "POST".to_owned(),
        path: "/incoming".to_owned(),
        query: String::new(),
        headers: Vec::new(),
        body: body.to_owned(),
    }
}

#[test]
fn a_0_2_0_module_artifact_runs_on_the_current_runtime() {
    let golden = golden("0.2.0", "factorial");
    let result = Runtime::default()
        .execute(
            &golden.bytes,
            &golden.metadata,
            &GrantSet::from_manifest(&golden.manifest),
        )
        .expect("0.2.0 factorial artifact should run");
    assert_eq!(result.output, b"720\n");
    assert_eq!(result.stats.policy_version, 1);
    assert!(result.events.is_empty());
}

#[test]
fn a_0_2_0_policy_two_webhook_artifact_invokes_with_durable_state() {
    let golden = golden("0.2.0", "stateful-webhook");
    let directory = TestDirectory::new("stateful-webhook");
    let database = directory.path.join("agent-work.db");
    let grants = GrantSet::from_manifest(&golden.manifest);
    let runtime = Runtime::default();
    let inputs = HostInputs::new(BTreeMap::new(), SecretStore::default())
        .expect("empty host inputs should be valid");
    let host = AgentHost::new_with_state(
        inputs,
        AgentHostPolicy::default(),
        Arc::new(DenyAllApprovalPolicy),
        durable(database),
    )
    .expect("agent host should build");

    let first = runtime
        .invoke_webhook_with_host(
            &golden.bytes,
            &golden.metadata,
            &grants,
            &host,
            request("first"),
        )
        .expect("0.2.0 stateful webhook should invoke");
    assert_eq!(first.response.status, 200);
    assert_eq!(first.response.body, "first");
    assert_eq!(first.stats.policy_version, 2);
    assert_eq!(first.stats.checkpoint_reads, 1);
    assert_eq!(first.stats.checkpoint_writes, 1);
    assert_eq!(first.stats.state_writes, 1);

    // The committed checkpoint answers every later delivery.
    let second = runtime
        .invoke_webhook_with_host(
            &golden.bytes,
            &golden.metadata,
            &grants,
            &host,
            request("second"),
        )
        .expect("0.2.0 stateful webhook should invoke again");
    assert_eq!(second.response.body, "first");
    assert_eq!(second.stats.state_writes, 0);
}
