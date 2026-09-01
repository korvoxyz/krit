use std::{
    fs,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
    thread,
};

use krit_assist::{
    AUTHORING_PROTOCOL_VERSION, AssistRequest, AssistResponse, ProviderConfig, RequestOptions,
    RequestedRange, ResponseDocument, SuggestionKind, SuggestionProvider, build_proposal,
    prepare_request,
};
use tiny_http::{Header, Response, Server};

static DIRECTORY_ID: AtomicU64 = AtomicU64::new(0);

struct TestDirectory {
    path: PathBuf,
}

impl TestDirectory {
    fn new() -> Self {
        let id = DIRECTORY_ID.fetch_add(1, Ordering::Relaxed);
        let path =
            std::env::temp_dir().join(format!("krit-assist-provider-{}-{id}", std::process::id()));
        fs::create_dir_all(&path).expect("test directory should be created");
        Self { path }
    }

    fn file(&self, name: &str, contents: &str) -> PathBuf {
        let path = self.path.join(name);
        fs::write(&path, contents).expect("test file should be written");
        path
    }
}

impl Drop for TestDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

fn package_manifest() -> &'static str {
    r#"
schema = 1

[package]
name = "example/provider"
version = "0.2.0"
edition = "2026"
entry = "main.krit"
license = "Apache-2.0"
"#
}

#[test]
fn generic_http_json_provider_round_trips_the_versioned_protocol() {
    let directory = TestDirectory::new();
    directory.file("krit.pkg", package_manifest());
    directory.file("main.krit", "let answer=6*;\n");
    let server = Server::http("127.0.0.1:0").expect("loopback provider should bind");
    let endpoint = format!("http://{}/suggest", server.server_addr());
    let worker = thread::spawn(move || {
        let mut incoming = server.recv().expect("provider should receive a request");
        let mut body = Vec::new();
        incoming
            .as_reader()
            .read_to_end(&mut body)
            .expect("provider request should be readable");
        let request: AssistRequest =
            serde_json::from_slice(&body).expect("provider request should use schema 1");
        assert_eq!(request.authoring_protocol, AUTHORING_PROTOCOL_VERSION);
        assert!(request.instruction.contains("untrusted proposal"));
        let response = AssistResponse {
            schema: 1,
            authoring_protocol: AUTHORING_PROTOCOL_VERSION,
            request_id: request.request_id,
            document: ResponseDocument {
                path: request.target.document.path,
                base_digest: request.target.document.digest,
            },
            summary: "repair arithmetic".to_owned(),
            edits: vec![krit_assist::ProposedTextEdit {
                range: request.target.selection,
                new_text: "let answer = 6 * 7;\n".to_owned(),
            }],
        };
        let body = serde_json::to_vec(&response).expect("response should serialize");
        let content_type =
            Header::from_bytes("Content-Type", "application/json").expect("header should be valid");
        incoming
            .respond(Response::from_data(body).with_header(content_type))
            .expect("provider should respond");
    });
    let config_path = directory.file(
        "provider.json",
        &format!(
            r#"{{
  "schema": 1,
  "enabled": true,
  "provider": {{
    "kind": "http-json",
    "endpoint": "{endpoint}",
    "credentialEnv": null,
    "connectTimeoutMs": 1000,
    "timeoutMs": 5000
  }}
}}"#
        ),
    );
    let config = ProviderConfig::load(&config_path).expect("provider config should load");
    let prepared = prepare_request(
        config.descriptor(),
        RequestOptions {
            manifest_path: directory.path.join("krit.pkg"),
            target_path: PathBuf::from("main.krit"),
            selection: RequestedRange::WholeDocument,
            contexts: Vec::new(),
            intent: "Repair the syntax error.".to_owned(),
            kind: SuggestionKind::DiagnosticRepair,
        },
    )
    .expect("request should prepare");
    let response = config
        .suggest(prepared.request())
        .expect("provider should return a response");
    let proposal = build_proposal(prepared, response).expect("response should validate");

    assert!(proposal.review.after.valid);
    assert!(proposal.review.after.diagnostics.is_empty());
    worker.join().expect("provider thread should finish");
}

#[test]
fn assistance_is_disabled_without_an_explicit_enabled_safe_config() {
    let directory = TestDirectory::new();
    let missing = ProviderConfig::load(&directory.path.join("missing.json"))
        .expect_err("missing config should keep assistance disabled");
    assert_eq!(missing.code(), "K8101");

    let disabled = directory.file(
        "disabled.json",
        r#"{
  "schema": 1,
  "enabled": false,
  "provider": {
    "kind": "http-json",
    "endpoint": "https://example.com/suggest",
    "credentialEnv": null,
    "connectTimeoutMs": 1000,
    "timeoutMs": 5000
  }
}"#,
    );
    assert_eq!(
        ProviderConfig::load(&disabled)
            .expect_err("disabled config should fail")
            .code(),
        "K8101"
    );

    let plaintext = directory.file(
        "plaintext.json",
        r#"{
  "schema": 1,
  "enabled": true,
  "provider": {
    "kind": "http-json",
    "endpoint": "http://example.com/suggest",
    "credentialEnv": null,
    "connectTimeoutMs": 1000,
    "timeoutMs": 5000
  }
}"#,
    );
    assert_eq!(
        ProviderConfig::load(&plaintext)
            .expect_err("non-loopback plaintext should fail")
            .code(),
        "K8101"
    );
}

#[test]
fn credential_values_are_host_managed_and_never_enter_requests() {
    let directory = TestDirectory::new();
    directory.file("krit.pkg", package_manifest());
    directory.file("main.krit", "let value = 1;\n");
    let config_path = directory.file(
        "provider.json",
        r#"{
  "schema": 1,
  "enabled": true,
  "provider": {
    "kind": "http-json",
    "endpoint": "http://127.0.0.1:9/suggest",
    "credentialEnv": "KRIT_ASSIST_TEST_MISSING_CREDENTIAL",
    "connectTimeoutMs": 100,
    "timeoutMs": 100
  }
}"#,
    );
    let config = ProviderConfig::load(&config_path).expect("provider config should load");
    let prepared = prepare_request(
        config.descriptor(),
        RequestOptions {
            manifest_path: directory.path.join("krit.pkg"),
            target_path: PathBuf::from("main.krit"),
            selection: RequestedRange::WholeDocument,
            contexts: Vec::new(),
            intent: "Keep the program valid.".to_owned(),
            kind: SuggestionKind::Completion,
        },
    )
    .expect("request should prepare");
    let request = serde_json::to_string(prepared.request()).expect("request should serialize");
    assert!(!request.contains("KRIT_ASSIST_TEST_MISSING_CREDENTIAL"));
    let error = config
        .suggest(prepared.request())
        .expect_err("missing host credential should fail");
    assert_eq!(error.code(), "K8103");
    assert!(
        !error
            .message()
            .contains("KRIT_ASSIST_TEST_MISSING_CREDENTIAL")
    );
}
