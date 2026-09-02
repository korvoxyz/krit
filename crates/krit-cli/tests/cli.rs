use std::{
    fs,
    io::{BufRead, BufReader, Read, Write},
    net::TcpStream,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        atomic::{AtomicU64, Ordering},
        mpsc,
    },
    thread,
    time::Duration,
};

use krit_wasm::{ArtifactMetadata, validate_artifact};
use tiny_http::{Header, Response, Server};

static TEST_DIRECTORY_ID: AtomicU64 = AtomicU64::new(0);

struct TestDirectory {
    path: PathBuf,
}

impl TestDirectory {
    fn new(name: &str) -> Self {
        let id = TEST_DIRECTORY_ID.fetch_add(1, Ordering::Relaxed);
        let path = repository_root()
            .join("target/krit-cli-tests")
            .join(format!("{name}-{}-{id}", std::process::id()));
        fs::create_dir_all(&path).expect("test directory should be created");
        Self { path }
    }

    fn file(&self, name: &str, contents: &str) -> PathBuf {
        let path = self.path.join(name);
        fs::write(&path, contents).expect("test source should be written");
        path
    }
}

impl Drop for TestDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

fn repository_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("CLI crate should be inside the workspace")
}

fn krit() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_krit"));
    command.current_dir(repository_root());
    command
}

fn krit_in(directory: &TestDirectory) -> Command {
    let mut command = krit();
    command.current_dir(&directory.path);
    command
}

fn lsp_frame(value: &serde_json::Value) -> Vec<u8> {
    let body = serde_json::to_vec(value).expect("LSP message should serialize");
    let mut frame = format!("Content-Length: {}\r\n\r\n", body.len()).into_bytes();
    frame.extend_from_slice(&body);
    frame
}

fn parse_lsp_frames(bytes: &[u8]) -> Vec<serde_json::Value> {
    let mut messages = Vec::new();
    let mut cursor = 0;
    while cursor < bytes.len() {
        let separator = bytes[cursor..]
            .windows(4)
            .position(|window| window == b"\r\n\r\n")
            .map(|position| cursor + position)
            .expect("LSP frame should contain a header separator");
        let headers =
            std::str::from_utf8(&bytes[cursor..separator]).expect("LSP headers should be UTF-8");
        let length = headers
            .lines()
            .find_map(|line| line.strip_prefix("Content-Length: "))
            .expect("LSP frame should contain Content-Length")
            .parse::<usize>()
            .expect("Content-Length should be numeric");
        let body_start = separator + 4;
        let body_end = body_start + length;
        assert!(body_end <= bytes.len(), "LSP frame body should be complete");
        messages.push(
            serde_json::from_slice(&bytes[body_start..body_end])
                .expect("LSP frame body should be JSON"),
        );
        cursor = body_end;
    }

    messages
}

fn parse_json_lines(bytes: &[u8]) -> Vec<serde_json::Value> {
    String::from_utf8(bytes.to_vec())
        .expect("JSON Lines output should be UTF-8")
        .lines()
        .map(|line| serde_json::from_str(line).expect("line should be valid JSON"))
        .collect()
}

#[test]
fn runs_an_example() {
    let output = krit()
        .args(["run", "examples/factorial.krit"])
        .output()
        .expect("Krit should start");

    assert!(output.status.success());
    assert_eq!(output.stdout, b"720\n");
    assert!(output.stderr.is_empty());
}

#[test]
fn language_server_uses_pure_stdio_framing_and_shuts_down_cleanly() {
    let directory = TestDirectory::new("lsp-stdio");
    let source = directory.file("main.krit", "let answer=6*7;\n");
    let uri = format!("file://{}", source.display());
    let root_uri = format!("file://{}", directory.path.display());
    let messages = [
        serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "processId": null,
                "capabilities": {},
                "rootUri": root_uri,
                "workspaceFolders": null
            }
        }),
        serde_json::json!({
            "jsonrpc": "2.0",
            "method": "initialized",
            "params": {}
        }),
        serde_json::json!({
            "jsonrpc": "2.0",
            "method": "textDocument/didOpen",
            "params": {
                "textDocument": {
                    "uri": uri,
                    "languageId": "krit",
                    "version": 1,
                    "text": "let answer=6*7;\n"
                }
            }
        }),
        serde_json::json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "textDocument/hover",
            "params": {
                "textDocument": {"uri": uri},
                "position": {"line": 0, "character": 5}
            }
        }),
        serde_json::json!({
            "jsonrpc": "2.0",
            "id": 3,
            "method": "textDocument/formatting",
            "params": {
                "textDocument": {"uri": uri},
                "options": {"tabSize": 4, "insertSpaces": true}
            }
        }),
        serde_json::json!({
            "jsonrpc": "2.0",
            "id": 4,
            "method": "krit/compilerFacts",
            "params": {"textDocument": {"uri": uri}}
        }),
        serde_json::json!({
            "jsonrpc": "2.0",
            "id": 5,
            "method": "shutdown",
            "params": null
        }),
        serde_json::json!({
            "jsonrpc": "2.0",
            "method": "exit",
            "params": null
        }),
    ];
    let mut input = Vec::new();
    for message in &messages {
        input.extend_from_slice(&lsp_frame(message));
    }

    let mut child = krit_in(&directory)
        .arg("lsp")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("Krit language server should start");
    child
        .stdin
        .take()
        .expect("language server stdin should be piped")
        .write_all(&input)
        .expect("LSP input should be written");
    let output = child
        .wait_with_output()
        .expect("language server should finish");

    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    let frames = parse_lsp_frames(&output.stdout);
    assert_eq!(frames.len(), 6);
    assert_eq!(frames[0]["id"], 1);
    assert_eq!(
        frames[0]["result"]["capabilities"]["positionEncoding"],
        "utf-16"
    );
    assert_eq!(frames[1]["method"], "textDocument/publishDiagnostics");
    assert_eq!(frames[1]["params"]["diagnostics"], serde_json::json!([]));
    assert_eq!(frames[2]["id"], 2);
    assert!(
        frames[2]["result"]["contents"]["value"]
            .as_str()
            .expect("hover should contain markdown")
            .contains("answer: Int")
    );
    assert_eq!(frames[3]["result"][0]["newText"], "let answer = 6 * 7;\n");
    assert_eq!(frames[4]["result"]["schema"], 1);
    assert_eq!(frames[4]["result"]["formatting"]["canonical"], false);
    assert_eq!(frames[5]["id"], 5);
    assert_eq!(frames[5]["result"], serde_json::Value::Null);
}

#[test]
fn language_server_routes_malformed_payload_failures_to_stderr() {
    let initialize = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "processId": null,
            "capabilities": {},
            "workspaceFolders": null
        }
    });
    let initialized = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "initialized",
        "params": {}
    });
    let mut input = lsp_frame(&initialize);
    input.extend_from_slice(&lsp_frame(&initialized));
    input.extend_from_slice(b"Content-Length: 1\r\n\r\n{");

    let mut child = krit()
        .arg("lsp")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("Krit language server should start");
    child
        .stdin
        .take()
        .expect("language server stdin should be piped")
        .write_all(&input)
        .expect("malformed LSP input should be written");
    let output = child
        .wait_with_output()
        .expect("language server should finish");

    assert_eq!(output.status.code(), Some(1));
    let frames = parse_lsp_frames(&output.stdout);
    assert_eq!(
        frames.len(),
        1,
        "stdout must contain only the initialize frame"
    );
    assert_eq!(frames[0]["id"], 1);
    assert!(
        String::from_utf8(output.stderr)
            .expect("stderr should be UTF-8")
            .contains("language server failed")
    );
}

#[test]
fn language_server_rejects_invalid_initialization_without_waiting_for_stdin_eof() {
    let initialize = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "processId": null,
            "capabilities": "SUPERSECRET",
            "workspaceFolders": null
        }
    });
    let mut child = krit()
        .arg("lsp")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("Krit language server should start");
    let mut stdin = child
        .stdin
        .take()
        .expect("language server stdin should be piped");
    stdin
        .write_all(&lsp_frame(&initialize))
        .expect("invalid initialize request should be written");
    stdin.flush().expect("initialize request should be flushed");

    let mut exited = false;
    for _ in 0..50 {
        if child
            .try_wait()
            .expect("language server status should be readable")
            .is_some()
        {
            exited = true;
            break;
        }
        thread::sleep(Duration::from_millis(20));
    }
    if !exited {
        child
            .kill()
            .expect("stuck language server should be stopped");
    }
    drop(stdin);
    let output = child
        .wait_with_output()
        .expect("language server output should be collected");

    assert!(exited, "invalid initialization must terminate promptly");
    assert_eq!(output.status.code(), Some(1));
    let frames = parse_lsp_frames(&output.stdout);
    assert_eq!(frames.len(), 1);
    assert_eq!(frames[0]["error"]["code"], -32602);
    assert_eq!(frames[0]["error"]["message"], "invalid initialize params");
    let stderr = String::from_utf8(output.stderr).expect("stderr should be UTF-8");
    assert!(stderr.contains("invalid initialize request"));
    assert!(!stderr.contains("SUPERSECRET"));
}

#[test]
fn language_server_rejects_cli_arguments_without_starting_protocol_io() {
    let output = krit()
        .args(["lsp", "--tcp"])
        .output()
        .expect("Krit should start");

    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    assert_eq!(output.stderr, b"krit: `lsp` does not accept arguments\n");
}

#[test]
fn assist_reference_webhook_inspects_suggests_reviews_and_accepts_explicitly() {
    let directory = TestDirectory::new("assist-reference");
    let source = r#"webhook fn handle(request: HttpRequest) -> HttpResponse {
    record { status: 200, headers: [], body: request.path }
}
"#;
    directory.file("main.krit", source);
    directory.file(
        "krit.pkg",
        r#"
schema = 1

[package]
name = "example/assist"
version = "0.2.0"
edition = "2026"
entry = "main.krit"
license = "Apache-2.0"

[capabilities]
config = ["agent.model"]
"#,
    );
    let server = Server::http("127.0.0.1:0").expect("fake provider should bind");
    let endpoint = format!("http://{}/suggest", server.server_addr());
    directory.file(
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

    let inspect = krit_in(&directory)
        .args([
            "assist",
            "inspect",
            "--provider-config",
            "provider.json",
            "--manifest",
            "krit.pkg",
            "--file",
            "main.krit",
            "--range",
            "all",
            "--intent",
            "Read the configured model explicitly.",
            "--json",
        ])
        .output()
        .expect("assist inspection should run");
    assert!(inspect.status.success());
    assert!(inspect.stderr.is_empty());
    let inspect = parse_json_lines(&inspect.stdout);
    assert_eq!(inspect.len(), 1);
    assert_eq!(inspect[0]["event"], "inspection");
    assert_eq!(
        fs::read_to_string(directory.path.join("main.krit")).unwrap(),
        source
    );

    let insertion_byte = source.find("    record").expect("record line") + 4;
    let (release_sender, release_receiver) = mpsc::channel();
    let provider = thread::spawn(move || {
        let mut incoming = server.recv().expect("provider should receive request");
        let mut body = Vec::new();
        incoming
            .as_reader()
            .read_to_end(&mut body)
            .expect("provider request should be readable");
        let request: serde_json::Value =
            serde_json::from_slice(&body).expect("provider request should be JSON");
        release_receiver
            .recv_timeout(Duration::from_secs(5))
            .expect("test should release provider response");
        let response = serde_json::json!({
            "schema": 1,
            "authoringProtocol": 1,
            "requestId": request["requestId"],
            "document": {
                "path": request["target"]["document"]["path"],
                "baseDigest": request["target"]["document"]["digest"]
            },
            "summary": "add an explicit config read",
            "edits": [{
                "range": {
                    "startByte": insertion_byte,
                    "endByte": insertion_byte,
                    "start": {"line": 1, "character": 4},
                    "end": {"line": 1, "character": 4}
                },
                "newText": "let model=config_string(\"agent.model\");\n    "
            }]
        });
        let content_type =
            Header::from_bytes("Content-Type", "application/json").expect("header should be valid");
        incoming
            .respond(
                Response::from_data(serde_json::to_vec(&response).unwrap())
                    .with_header(content_type),
            )
            .expect("provider should respond");
    });

    let mut child = krit_in(&directory)
        .args([
            "assist",
            "suggest",
            "--provider-config",
            "provider.json",
            "--manifest",
            "krit.pkg",
            "--file",
            "main.krit",
            "--range",
            "all",
            "--intent",
            "Read the configured model explicitly.",
            "--proposal",
            "proposal.json",
            "--json",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("assist suggestion should start");
    let mut stdout = BufReader::new(
        child
            .stdout
            .take()
            .expect("assist suggestion stdout should be piped"),
    );
    let mut inspection_line = String::new();
    stdout
        .read_line(&mut inspection_line)
        .expect("inspection should be written before provider completion");
    let inspection: serde_json::Value =
        serde_json::from_str(inspection_line.trim_end()).expect("inspection should be JSON");
    assert_eq!(inspection["event"], "inspection");
    release_sender
        .send(())
        .expect("provider response should be released after inspection");
    let mut remaining_stdout = Vec::new();
    stdout
        .read_to_end(&mut remaining_stdout)
        .expect("remaining assist output should be readable");
    let mut stderr = Vec::new();
    child
        .stderr
        .take()
        .expect("assist suggestion stderr should be piped")
        .read_to_end(&mut stderr)
        .expect("assist stderr should be readable");
    let status = child.wait().expect("assist suggestion should finish");
    assert!(status.success());
    assert!(stderr.is_empty());
    let proposal_lines = parse_json_lines(&remaining_stdout);
    assert_eq!(proposal_lines.len(), 1);
    assert_eq!(proposal_lines[0]["event"], "proposal");
    provider.join().expect("provider should finish");
    assert_eq!(
        fs::read_to_string(directory.path.join("main.krit")).unwrap(),
        source
    );
    assert!(directory.path.join("proposal.json").is_file());

    let review = krit_in(&directory)
        .args([
            "assist",
            "review",
            "--manifest",
            "krit.pkg",
            "--proposal",
            "proposal.json",
            "--json",
        ])
        .output()
        .expect("proposal review should run");
    assert!(review.status.success());
    let review = parse_json_lines(&review.stdout);
    assert_eq!(review[0]["event"], "review");
    assert_eq!(
        review[0]["review"]["permissions"]["approvalRequired"][0],
        serde_json::json!({
            "capability": "config.read",
            "resource": "agent.model"
        })
    );

    let unreviewed = krit_in(&directory)
        .args([
            "assist",
            "accept",
            "--manifest",
            "krit.pkg",
            "--proposal",
            "proposal.json",
            "--json",
        ])
        .output()
        .expect("unreviewed acceptance should report usage");
    assert_eq!(unreviewed.status.code(), Some(2));
    assert!(unreviewed.stdout.is_empty());
    assert_eq!(
        fs::read_to_string(directory.path.join("main.krit")).unwrap(),
        source
    );

    let denied = krit_in(&directory)
        .args([
            "assist",
            "accept",
            "--manifest",
            "krit.pkg",
            "--proposal",
            "proposal.json",
            "--reviewed",
            "--json",
        ])
        .output()
        .expect("unapproved acceptance should run");
    assert_eq!(denied.status.code(), Some(4));
    assert_eq!(parse_json_lines(&denied.stdout)[0]["event"], "review");
    assert_eq!(parse_json_lines(&denied.stderr)[0]["code"], "K8106");
    assert_eq!(
        fs::read_to_string(directory.path.join("main.krit")).unwrap(),
        source
    );

    let accepted = krit_in(&directory)
        .args([
            "assist",
            "accept",
            "--manifest",
            "krit.pkg",
            "--proposal",
            "proposal.json",
            "--reviewed",
            "--approve-permission",
            "config.read=agent.model",
            "--json",
        ])
        .output()
        .expect("approved acceptance should run");
    assert!(accepted.status.success());
    assert!(accepted.stderr.is_empty());
    let accepted_lines = parse_json_lines(&accepted.stdout);
    assert_eq!(accepted_lines.len(), 2);
    assert_eq!(accepted_lines[0]["event"], "review");
    assert_eq!(accepted_lines[1]["event"], "accepted");
    let accepted_source =
        fs::read_to_string(directory.path.join("main.krit")).expect("source should be readable");
    assert!(accepted_source.contains("let model = config_string(\"agent.model\");"));
    let checked = krit_in(&directory)
        .args(["check", "main.krit"])
        .output()
        .expect("accepted source should check");
    assert!(checked.status.success());
}

#[test]
fn help_lists_the_complete_assist_workflow() {
    let output = krit().arg("--help").output().expect("Krit should start");

    assert!(output.status.success());
    let help = String::from_utf8(output.stdout).expect("help should be UTF-8");
    for command in [
        "krit assist inspect",
        "krit assist suggest",
        "krit assist review",
        "krit assist accept",
    ] {
        assert!(help.contains(command));
    }
}

#[test]
fn disabled_assistance_does_not_change_offline_compiler_availability() {
    let directory = TestDirectory::new("assist-disabled");
    directory.file("main.krit", "let answer = 6 * 7;\n");
    directory.file(
        "krit.pkg",
        r#"
schema = 1

[package]
name = "example/offline"
version = "0.2.0"
edition = "2026"
entry = "main.krit"
license = "Apache-2.0"
"#,
    );
    let before = krit_in(&directory)
        .args(["check", "main.krit"])
        .output()
        .expect("offline check should run");
    let assist = krit_in(&directory)
        .args([
            "assist",
            "inspect",
            "--provider-config",
            "missing.json",
            "--manifest",
            "krit.pkg",
            "--file",
            "main.krit",
            "--range",
            "all",
            "--intent",
            "No provider is configured.",
            "--json",
        ])
        .output()
        .expect("disabled assist should report");
    assert_eq!(assist.status.code(), Some(1));
    assert!(assist.stdout.is_empty());
    assert_eq!(parse_json_lines(&assist.stderr)[0]["code"], "K8101");
    let after = krit_in(&directory)
        .args(["check", "main.krit"])
        .output()
        .expect("offline check should still run");
    assert_eq!(before.status.code(), after.status.code());
    assert_eq!(before.stdout, after.stdout);
    assert_eq!(before.stderr, after.stderr);
    assert_eq!(
        fs::read_to_string(directory.path.join("main.krit")).unwrap(),
        "let answer = 6 * 7;\n"
    );
}

#[test]
fn checks_without_executing_effects() {
    let output = krit()
        .args(["check", "examples/factorial.krit"])
        .output()
        .expect("Krit should start");

    assert!(output.status.success());
    assert_eq!(output.stdout, b"checked examples/factorial.krit\n");
    assert!(output.stderr.is_empty());
}

#[test]
fn check_accepts_pure_builtins_with_conservative_latent_effects() {
    let directory = TestDirectory::new("conservative-builtin-effects");
    for (name, source) in [
        (
            "branch-constructor.krit",
            r#"
let wrap = if true {
    Some
} else {
    fn(value) {
        println(value);
        Some(value)
    }
};
let result = wrap(1);
"#,
        ),
        (
            "higher-order-some.krit",
            r#"
fn apply(constructor, value) {
    constructor(value)
}
fn loud_some(value) {
    println(value);
    Some(value)
}
let first = apply(Some, 1);
let second = apply(loud_some, 2);
"#,
        ),
        (
            "json-encode.krit",
            r#"
let encode = if true {
    json_encode
} else {
    fn(value) {
        println(value);
        json_encode(value)
    }
};
let encoded = encode(1);
"#,
        ),
        (
            "json-decode.krit",
            r#"
let decode = if true {
    json_decode
} else {
    fn(value) {
        println(value);
        json_decode(value)
    }
};
let decoded: Int = decode("1");
"#,
        ),
    ] {
        let path = directory.file(name, source);
        let output = krit()
            .arg("check")
            .arg(&path)
            .output()
            .expect("Krit should start");

        assert!(
            output.status.success(),
            "{name}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            String::from_utf8(output.stdout).expect("output should be UTF-8"),
            format!("checked {}\n", path.display())
        );
        assert!(output.stderr.is_empty(), "{name}");
    }
}

#[test]
fn explains_typed_core_facts_in_human_form() {
    let output = krit()
        .args(["explain", "examples/factorial.krit"])
        .output()
        .expect("Krit should start");

    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    let explanation = String::from_utf8(output.stdout).expect("explanation should be UTF-8");
    assert!(explanation.starts_with("Krit explanation (schema 1)\n"));
    assert!(explanation.contains("entrypoint: module-init f0\n"));
    assert!(explanation.contains("effects: {io.stdout}\n"));
    assert!(explanation.contains("b0 factorial: fn(Int) -> Int effects {}\n"));
    assert!(explanation.contains("core:\ncore module\n"));
    assert!(!explanation.contains(repository_root().to_string_lossy().as_ref()));
}

#[test]
fn explains_typed_core_facts_as_deterministic_json() {
    let first = krit()
        .args(["explain", "--json", "examples/factorial.krit"])
        .output()
        .expect("Krit should start");
    let second = krit()
        .args(["explain", "--json", "examples/factorial.krit"])
        .output()
        .expect("Krit should start");

    assert!(first.status.success());
    assert!(first.stderr.is_empty());
    assert_eq!(first.stdout, second.stdout);
    let explanation: serde_json::Value =
        serde_json::from_slice(&first.stdout).expect("explanation should be valid JSON");
    assert_eq!(explanation["schema"], 1);
    assert_eq!(explanation["entrypoint"]["id"], 0);
    assert_eq!(explanation["entrypoint"]["kind"], "module-init");
    assert_eq!(explanation["entrypoint"]["resultType"], "Unit");
    assert_eq!(explanation["entrypoint"]["effects"][0], "io.stdout");
    assert_eq!(explanation["bindings"][0]["name"], "factorial");
    assert_eq!(
        explanation["bindings"][0]["type"],
        "fn(Int) -> Int effects {}"
    );
    assert!(
        explanation["core"]
            .as_str()
            .expect("Core rendering should be a string")
            .starts_with("core module\n")
    );
}

#[test]
fn explanation_json_uses_serde_escaping_and_json_diagnostics() {
    let directory = TestDirectory::new("explain-json");
    let valid = directory.file("valid.krit", "let text = \"line\\n\\\"quoted\\\"\";\n");
    let output = krit()
        .arg("explain")
        .arg("--json")
        .arg(&valid)
        .output()
        .expect("Krit should start");
    assert!(output.status.success());
    let explanation: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("explanation should be valid JSON");
    assert!(
        explanation["core"]
            .as_str()
            .expect("Core rendering should be a string")
            .contains(r#"string "line\n\"quoted\"""#)
    );

    let invalid = directory.file("invalid.krit", "let value = 1 + true;\n");
    let diagnostic = krit()
        .arg("explain")
        .arg("--json")
        .arg(&invalid)
        .output()
        .expect("Krit should start");
    assert_eq!(diagnostic.status.code(), Some(1));
    assert!(diagnostic.stdout.is_empty());
    assert!(
        String::from_utf8(diagnostic.stderr)
            .expect("diagnostic should be UTF-8")
            .starts_with("{\"schema\":1,")
    );
}

#[test]
fn explains_exact_versioned_webhook_contracts_deterministically() {
    let directory = TestDirectory::new("explain-webhook-contract");
    let source = directory.file(
        "agent.krit",
        r#"
fn read_model() -> Result<String, String> {
    config_string("agent.model")
}

webhook fn handle(request: HttpRequest) -> HttpResponse {
    let model = read_model();
    record {
        status: 200,
        headers: [record { name: "x-note", value: "\"quoted\"" }],
        body: request.path,
    }
}
"#,
    );
    let first = krit()
        .args(["explain", "--json"])
        .arg(&source)
        .output()
        .expect("Krit should start");
    let second = krit()
        .args(["explain", "--json"])
        .arg(&source)
        .output()
        .expect("Krit should start");

    assert!(first.status.success());
    assert!(first.stderr.is_empty());
    assert_eq!(first.stdout, second.stdout);
    let explanation: serde_json::Value =
        serde_json::from_slice(&first.stdout).expect("explanation should be valid JSON");
    assert_eq!(explanation["entrypoints"]["schema"], 1);
    let webhook = &explanation["entrypoints"]["items"][1];
    assert_eq!(webhook["name"], "handle");
    assert_eq!(webhook["kind"], "webhook");
    assert_eq!(
        webhook["signature"],
        "webhook fn handle(request: HttpRequest) -> HttpResponse"
    );
    assert_eq!(webhook["effects"], serde_json::json!(["config.read"]));
    assert_eq!(
        webhook["capabilityRequirements"],
        serde_json::json!([{
            "capability": "config.read",
            "resource": "agent.model"
        }])
    );
    let contract = &webhook["contract"];
    assert_eq!(contract["schema"], 1);
    assert_eq!(contract["requestType"], "HttpRequest");
    assert_eq!(contract["responseType"], "HttpResponse");
    assert_eq!(
        contract["requestSchema"],
        serde_json::json!({
            "$schema": "https://json-schema.org/draft/2020-12/schema",
            "$id": "https://krit.dev/schemas/webhook/request-1.json",
            "title": "Krit HttpRequest contract v1",
            "type": "object",
            "additionalProperties": false,
            "required": ["method", "path", "query", "headers", "body"],
            "properties": {
                "body": {"type": "string"},
                "headers": {"type": "array", "items": {"$ref": "#/$defs/HttpHeader"}},
                "method": {"type": "string"},
                "path": {"type": "string"},
                "query": {"type": "string"}
            },
            "$defs": {
                "HttpHeader": {
                    "type": "object",
                    "additionalProperties": false,
                    "required": ["name", "value"],
                    "properties": {
                        "name": {"type": "string"},
                        "value": {"type": "string"}
                    }
                }
            }
        })
    );
    assert_eq!(
        contract["responseSchema"],
        serde_json::json!({
            "$schema": "https://json-schema.org/draft/2020-12/schema",
            "$id": "https://krit.dev/schemas/webhook/response-1.json",
            "title": "Krit HttpResponse contract v1",
            "type": "object",
            "additionalProperties": false,
            "required": ["status", "headers", "body"],
            "properties": {
                "body": {"type": "string"},
                "headers": {"type": "array", "items": {"$ref": "#/$defs/HttpHeader"}},
                "status": {"type": "integer"}
            },
            "$defs": {
                "HttpHeader": {
                    "type": "object",
                    "additionalProperties": false,
                    "required": ["name", "value"],
                    "properties": {
                        "name": {"type": "string"},
                        "value": {"type": "string"}
                    }
                }
            }
        })
    );
    assert!(
        explanation["core"]
            .as_str()
            .expect("Core rendering should be a string")
            .contains(r#"string "\"quoted\"""#)
    );

    let human = krit()
        .arg("explain")
        .arg(&source)
        .output()
        .expect("Krit should start");
    assert!(human.status.success());
    let human = String::from_utf8(human.stdout).expect("explanation should be UTF-8");
    assert!(human.contains("webhook contract (schema 1):\n"));
    assert!(human.contains("signature: webhook fn handle(request: HttpRequest) -> HttpResponse"));
    assert!(human.contains("JSON Schema: draft 2020-12 request/response contract v1"));
}

#[test]
fn direct_run_fails_closed_for_unavailable_agent_hosts() {
    let directory = TestDirectory::new("run-agent-host-unavailable");
    let config = directory.file(
        "config.krit",
        "let model = config_string(\"agent.model\");\n",
    );
    let config_output = krit()
        .args(["run", "--diagnostic-format=json"])
        .arg(&config)
        .output()
        .expect("Krit should start");
    assert_eq!(config_output.status.code(), Some(4));
    assert!(config_output.stdout.is_empty());
    let config_diagnostic: serde_json::Value =
        serde_json::from_slice(&config_output.stderr).expect("diagnostic should be valid JSON");
    assert_eq!(config_diagnostic["code"], "K5003");
    assert_eq!(config_diagnostic["span"]["start"]["column"], 13);

    let mixed = directory.file(
        "mixed.krit",
        "let model = config_string(\"agent.model\");\nlet bad = 1 + true;\n",
    );
    let mixed_output = krit()
        .arg("run")
        .arg(&mixed)
        .output()
        .expect("Krit should start");
    assert_eq!(mixed_output.status.code(), Some(4));
    assert!(mixed_output.stdout.is_empty());
    assert!(
        String::from_utf8(mixed_output.stderr)
            .expect("diagnostic should be UTF-8")
            .contains("error[K5003]")
    );

    let webhook = directory.file(
        "webhook.krit",
        r#"webhook fn handle(request: HttpRequest) -> HttpResponse {
    record { status: 200, headers: [], body: request.path }
}
"#,
    );
    let webhook_output = krit()
        .arg("run")
        .arg(&webhook)
        .output()
        .expect("Krit should start");
    assert_eq!(webhook_output.status.code(), Some(4));
    assert!(webhook_output.stdout.is_empty());
    assert!(
        String::from_utf8(webhook_output.stderr)
            .expect("diagnostic should be UTF-8")
            .contains("error[K5003]: webhook entrypoints are unavailable")
    );

    let http = directory.file(
        "http.krit",
        r#"let response = http_request(
    "https://api.example.com",
    record { method: "GET", path: "/", query: "", headers: [], body: "" },
    None,
);
"#,
    );
    let http_output = krit()
        .arg("run")
        .arg(&http)
        .output()
        .expect("Krit should start");
    assert_eq!(http_output.status.code(), Some(4));
    assert!(http_output.stdout.is_empty());
    assert!(
        String::from_utf8(http_output.stderr)
            .expect("diagnostic should be UTF-8")
            .contains("error[K5003]")
    );

    for (name, source) in [
        ("ai.krit", "ai_invoke(\"reviewer\", \"input\");\n"),
        ("log.krit", "log_info(\"request.started\", []);\n"),
    ] {
        let path = directory.file(name, source);
        let output = krit()
            .arg("run")
            .arg(&path)
            .output()
            .expect("Krit should start");
        assert_eq!(output.status.code(), Some(4), "{name}");
        assert!(output.stdout.is_empty(), "{name}");
        assert!(
            String::from_utf8(output.stderr)
                .expect("diagnostic should be UTF-8")
                .contains("error[K5003]"),
            "{name}"
        );
    }
}

#[test]
fn direct_run_fails_closed_for_durable_state_hosts() {
    let directory = TestDirectory::new("run-state-host-unavailable");
    let source = directory.file(
        "state.krit",
        "let value = state_get(\"agent-work\", \"key\");\n",
    );
    let output = krit()
        .args(["run", "--diagnostic-format=json"])
        .arg(&source)
        .output()
        .expect("Krit should start");

    assert_eq!(output.status.code(), Some(4));
    assert!(output.stdout.is_empty());
    let diagnostic: serde_json::Value =
        serde_json::from_slice(&output.stderr).expect("diagnostic should be JSON");
    assert_eq!(diagnostic["code"], "K5003");
    assert!(
        diagnostic["message"]
            .as_str()
            .unwrap()
            .contains("state.transaction")
    );
}

#[test]
fn explain_rejects_invalid_usage() {
    for arguments in [
        vec!["explain"],
        vec!["explain", "--unknown", "examples/factorial.krit"],
        vec!["explain", "examples/factorial.krit", "examples/lists.krit"],
    ] {
        let output = krit().args(arguments).output().expect("Krit should start");
        assert_eq!(output.status.code(), Some(2));
        assert!(output.stdout.is_empty());
        assert!(
            String::from_utf8(output.stderr)
                .expect("diagnostic should be UTF-8")
                .starts_with("krit: ")
        );
    }
}

#[test]
fn formatter_check_is_read_only_and_reports_noncanonical_files() {
    let directory = TestDirectory::new("fmt-check");
    let ugly = directory.file("ugly.krit", "let value=1;\r\n");
    let canonical = directory.file("canonical.krit", "let value = 1;\n");

    let output = krit()
        .arg("fmt")
        .arg("--check")
        .arg(&ugly)
        .arg(&canonical)
        .output()
        .expect("Krit should start");

    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    assert_eq!(
        fs::read(&ugly).expect("source should be readable"),
        b"let value=1;\r\n"
    );
    assert_eq!(
        fs::read(&canonical).expect("source should be readable"),
        b"let value = 1;\n"
    );
    let diagnostic = String::from_utf8(output.stderr).expect("diagnostic should be UTF-8");
    assert_eq!(diagnostic.matches("error[K8001]").count(), 1);
    assert!(diagnostic.contains(ugly.to_string_lossy().as_ref()));
}

#[test]
fn formatter_writes_multiple_files_in_argument_order() {
    let directory = TestDirectory::new("fmt-write");
    let first = directory.file("first.krit", "\tlet first=record{value:1,};\r\n");
    let second = directory.file("second.krit", "println(  \"ready\"  );");

    let output = krit()
        .arg("fmt")
        .arg(&first)
        .arg(&second)
        .output()
        .expect("Krit should start");

    assert!(output.status.success());
    assert_eq!(
        fs::read_to_string(&first).expect("source should be readable"),
        "let first = record { value: 1 };\n"
    );
    assert_eq!(
        fs::read_to_string(&second).expect("source should be readable"),
        "println(\"ready\");\n"
    );
    assert_eq!(
        String::from_utf8(output.stdout).expect("output should be UTF-8"),
        format!(
            "formatted {}\nformatted {}\n",
            first.display(),
            second.display()
        )
    );
    assert!(output.stderr.is_empty());

    let checked = krit()
        .arg("fmt")
        .arg("--check")
        .arg(&first)
        .arg(&second)
        .output()
        .expect("Krit should start");
    assert!(checked.status.success());
    assert!(checked.stdout.is_empty());
    assert!(checked.stderr.is_empty());
    assert_eq!(
        fs::read_dir(&directory.path)
            .expect("test directory should be readable")
            .count(),
        2,
        "formatter should not leave staged files"
    );
}

#[test]
fn formatter_width_repros_are_exact_and_stable_after_write() {
    let directory = TestDirectory::new("fmt-width-stability");
    let growth = directory.file(
        "growth.krit",
        "let vvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvv: List<Int> = [1, 2, 3];\nprintln(match vvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvv { [] => 0, [head, ..tail] => head });\n",
    );
    let shrink = directory.file(
        "shrink.krit",
        "let wwwwwwwwwwwwwwwwwwwwwwwwwwwwwwwwwwww = record { one: [1, 2, 3,], two: [4, 5, 6,] };\nprintln(wwwwwwwwwwwwwwwwwwwwwwwwwwwwwwwwwwww);\n",
    );

    let output = krit()
        .arg("fmt")
        .arg(&growth)
        .arg(&shrink)
        .output()
        .expect("Krit should start");
    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    assert_eq!(
        fs::read_to_string(&growth).expect("growth source should be readable"),
        "let vvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvv: List<Int> = [1, 2, 3];\n\nprintln(match vvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvv {\n    [] => 0,\n    [head, ..tail] => head,\n});\n"
    );
    assert_eq!(
        fs::read_to_string(&shrink).expect("shrink source should be readable"),
        "let wwwwwwwwwwwwwwwwwwwwwwwwwwwwwwwwwwww = record { one: [1, 2, 3], two: [4, 5, 6] };\n\nprintln(wwwwwwwwwwwwwwwwwwwwwwwwwwwwwwwwwwww);\n"
    );

    let checked = krit()
        .arg("fmt")
        .arg("--check")
        .arg(&growth)
        .arg(&shrink)
        .output()
        .expect("Krit should start");
    assert!(checked.status.success());
    assert!(checked.stdout.is_empty());
    assert!(checked.stderr.is_empty());
}

#[test]
fn formatter_does_not_write_any_file_when_batch_validation_fails() {
    let directory = TestDirectory::new("fmt-batch-failure");
    let valid = directory.file("valid.krit", "let value=1;\n");
    let invalid = directory.file("invalid.krit", "let broken = ;\n");

    let output = krit()
        .arg("fmt")
        .arg(&valid)
        .arg(&invalid)
        .output()
        .expect("Krit should start");

    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    assert_eq!(
        fs::read_to_string(&valid).expect("source should be readable"),
        "let value=1;\n"
    );
    assert!(
        String::from_utf8(output.stderr)
            .expect("diagnostic should be UTF-8")
            .contains("error[K1001]")
    );
    let entries = fs::read_dir(&directory.path)
        .expect("test directory should be readable")
        .count();
    assert_eq!(entries, 2, "formatter should not leave staged files");
}

#[test]
fn formatter_does_not_write_any_file_when_batch_read_fails() {
    let directory = TestDirectory::new("fmt-batch-read-failure");
    let valid = directory.file("valid.krit", "let value=1;\n");
    let missing = directory.path.join("missing.krit");

    let output = krit()
        .arg("fmt")
        .arg(&valid)
        .arg(&missing)
        .output()
        .expect("Krit should start");

    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    assert_eq!(
        fs::read_to_string(&valid).expect("source should be readable"),
        "let value=1;\n"
    );
    assert!(
        String::from_utf8(output.stderr)
            .expect("diagnostic should be UTF-8")
            .contains("krit: could not read")
    );
    assert_eq!(
        fs::read_dir(&directory.path)
            .expect("test directory should be readable")
            .count(),
        1,
        "formatter should not leave staged files"
    );
}

#[test]
fn formatter_rejects_invalid_usage() {
    for arguments in [vec!["fmt"], vec!["fmt", "--unknown", "file.krit"]] {
        let output = krit().args(arguments).output().expect("Krit should start");
        assert_eq!(output.status.code(), Some(2));
        assert!(output.stdout.is_empty());
        assert!(
            String::from_utf8(output.stderr)
                .expect("diagnostic should be UTF-8")
                .starts_with("krit: ")
        );
    }
}

#[test]
fn check_rejects_static_errors_without_partial_output() {
    let output = krit()
        .args([
            "check",
            "--diagnostic-format",
            "json",
            "conformance/check/type/mixed-addition/program.krit",
        ])
        .output()
        .expect("Krit should start");

    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    let diagnostic = String::from_utf8(output.stderr).expect("diagnostic should be UTF-8");
    assert!(diagnostic.contains("\"code\":\"K3001\""));
}

#[test]
fn check_preserves_fields_through_returned_records() {
    for path in [
        "conformance/check/type/returned-record-missing/program.krit",
        "conformance/check/type/returned-record-field-type/program.krit",
    ] {
        let output = krit()
            .args(["check", path])
            .output()
            .expect("Krit should start");

        assert_eq!(output.status.code(), Some(1), "{path}");
        assert!(output.stdout.is_empty(), "{path}");
        assert!(
            String::from_utf8(output.stderr)
                .expect("diagnostic should be UTF-8")
                .contains("error[K3001]"),
            "{path}"
        );
    }

    for path in [
        "conformance/check/valid/returned-record-presence/program.krit",
        "conformance/check/valid/returned-record-field-type/program.krit",
    ] {
        let output = krit()
            .args(["check", path])
            .output()
            .expect("Krit should start");

        assert!(output.status.success(), "{path}");
        assert!(output.stderr.is_empty(), "{path}");
    }
}

#[test]
fn emits_machine_readable_diagnostics() {
    let output = krit()
        .args([
            "run",
            "--diagnostic-format",
            "json",
            "conformance/cases/errors/undefined-name/program.krit",
        ])
        .output()
        .expect("Krit should start");

    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    let diagnostic = String::from_utf8(output.stderr).expect("diagnostic should be UTF-8");
    assert!(diagnostic.starts_with("{\"schema\":1,"));
    assert!(diagnostic.contains("\"code\":\"K2001\""));
    assert!(diagnostic.ends_with('\n'));
}

#[test]
fn validates_the_workspace_manifest() {
    let output = krit()
        .args(["package", "check"])
        .output()
        .expect("Krit should start");

    assert!(output.status.success());
    assert_eq!(output.stdout, b"checked akshay/krit (krit.pkg)\n");
    assert!(output.stderr.is_empty());
}

#[test]
fn displays_requested_permissions() {
    let output = krit()
        .args(["permissions"])
        .output()
        .expect("Krit should start");

    assert!(output.status.success());
    assert_eq!(
        output.stdout,
        b"Requested capabilities for akshay/krit:\n  io.stdout\nDeployment grants: not evaluated\n"
    );
    assert!(output.stderr.is_empty());
}

#[test]
fn emits_permissions_as_json() {
    let output = krit()
        .args(["permissions", "--json"])
        .output()
        .expect("Krit should start");

    assert!(output.status.success());
    assert_eq!(
        output.stdout,
        b"{\"schema\":1,\"package\":\"akshay/krit\",\"requested\":[{\"capability\":\"io.stdout\"}],\"grantStatus\":\"not-evaluated\"}\n"
    );
    assert!(output.stderr.is_empty());
}

#[test]
fn prints_the_versioned_generation_prompt() {
    let output = krit().arg("prompt").output().expect("Krit should start");

    assert!(output.status.success());
    assert!(
        output
            .stdout
            .starts_with(b"# Krit 0.2 code-generation instruction\n")
    );
    assert!(
        output
            .stdout
            .windows(b"Never invent syntax".len())
            .any(|window| window == b"Never invent syntax")
    );
    assert!(output.stderr.is_empty());
}

#[test]
fn builds_default_and_custom_component_outputs() {
    let directory = TestDirectory::new("build-outputs");
    directory.file("main.krit", "println(720);\n");
    let manifest = directory.file(
        "krit.pkg",
        r#"
schema = 1

[package]
name = "test/program"
version = "1.2.3"
edition = "2026"
entry = "main.krit"
license = "Apache-2.0"

[capabilities]
stdout = true
"#,
    );

    let default = krit()
        .arg("build")
        .arg("--manifest")
        .arg(&manifest)
        .output()
        .expect("Krit should start");
    assert!(
        default.status.success(),
        "{}",
        String::from_utf8_lossy(&default.stderr)
    );
    let default_component = directory.path.join("target/krit/program.wasm");
    assert!(default_component.is_file());
    assert_valid_artifact(&default_component);
    assert_eq!(
        String::from_utf8(default.stdout).expect("build output should be UTF-8"),
        format!(
            "built {}\nmetadata {}.json\n",
            default_component.display(),
            default_component.display()
        )
    );

    let custom_component = directory.path.join("dist/custom.component.wasm");
    let custom = krit()
        .arg("build")
        .arg(format!("--manifest={}", manifest.display()))
        .arg("--output")
        .arg(&custom_component)
        .output()
        .expect("Krit should start");
    assert!(
        custom.status.success(),
        "{}",
        String::from_utf8_lossy(&custom.stderr)
    );
    assert_valid_artifact(&custom_component);

    let mut relative_command = krit();
    relative_command.current_dir(&directory.path);
    let relative = relative_command
        .args([
            "build",
            "--manifest",
            "krit.pkg",
            "--output",
            "relative.wasm",
        ])
        .output()
        .expect("Krit should start");
    assert!(
        relative.status.success(),
        "{}",
        String::from_utf8_lossy(&relative.stderr)
    );
    assert_valid_artifact(&directory.path.join("relative.wasm"));
}

#[test]
fn builds_and_sandboxes_a_package_without_implicit_fallbacks() {
    let directory = TestDirectory::new("sandbox-factorial");
    directory.file(
        "main.krit",
        r#"
fn factorial(value) {
    if value == 0 {
        1
    } else {
        value * factorial(value - 1)
    }
}
println(factorial(6));
"#,
    );
    directory.file("krit.pkg", &package_manifest(true));

    let build = krit_in(&directory)
        .arg("build")
        .output()
        .expect("Krit should build");
    assert!(
        build.status.success(),
        "{}",
        String::from_utf8_lossy(&build.stderr)
    );

    let sandbox = krit_in(&directory)
        .arg("sandbox")
        .output()
        .expect("Krit should sandbox");
    assert!(sandbox.status.success());
    assert_eq!(sandbox.stdout, b"720\n");
    assert!(sandbox.stderr.is_empty());
}

#[test]
fn sandbox_reports_missing_component_and_sidecar_actionably() {
    let directory = TestDirectory::new("sandbox-missing");
    directory.file("main.krit", "println(1);\n");
    directory.file("krit.pkg", &package_manifest(true));

    let missing_component = krit_in(&directory)
        .arg("sandbox")
        .output()
        .expect("Krit should start");
    assert_eq!(missing_component.status.code(), Some(1));
    assert!(missing_component.stdout.is_empty());
    let diagnostic =
        String::from_utf8(missing_component.stderr).expect("diagnostic should be UTF-8");
    assert!(diagnostic.contains("error[K7003]"));
    assert!(diagnostic.contains("run `krit build` first"));

    let missing_permissions = krit_in(&directory)
        .args(["permissions", "--artifact"])
        .arg(directory.path.join("missing.wasm"))
        .output()
        .expect("Krit should start");
    assert_eq!(missing_permissions.status.code(), Some(1));
    assert!(missing_permissions.stdout.is_empty());
    assert!(
        String::from_utf8(missing_permissions.stderr)
            .expect("diagnostic should be UTF-8")
            .contains("error[K7003]")
    );

    let artifact = directory.path.join("program.wasm");
    fs::write(&artifact, b"not wasm").expect("placeholder artifact should be written");
    let missing_sidecar = krit_in(&directory)
        .args(["sandbox", "--artifact"])
        .arg(&artifact)
        .output()
        .expect("Krit should start");
    assert_eq!(missing_sidecar.status.code(), Some(1));
    assert!(missing_sidecar.stdout.is_empty());
    let diagnostic = String::from_utf8(missing_sidecar.stderr).expect("diagnostic should be UTF-8");
    assert!(diagnostic.contains("error[K7003]"));
    assert!(diagnostic.contains("adjacent artifact metadata"));
}

#[test]
fn sandbox_rejects_tampering_and_manifest_grant_denial() {
    let directory = TestDirectory::new("sandbox-denial");
    directory.file("main.krit", "println(42);\n");
    let manifest = directory.file("krit.pkg", &package_manifest(true));
    let build = krit_in(&directory)
        .arg("build")
        .output()
        .expect("Krit should build");
    assert!(build.status.success());
    let artifact = directory.path.join("target/krit/program.wasm");

    let mut tampered = fs::read(&artifact).expect("artifact should be readable");
    *tampered.last_mut().expect("artifact should not be empty") ^= 1;
    fs::write(&artifact, tampered).expect("artifact should be tampered");
    let rejected = krit_in(&directory)
        .arg("sandbox")
        .output()
        .expect("Krit should start");
    assert_eq!(rejected.status.code(), Some(1));
    assert!(rejected.stdout.is_empty());
    assert!(
        String::from_utf8(rejected.stderr)
            .expect("diagnostic should be UTF-8")
            .contains("error[K7004]")
    );

    let rebuild = krit_in(&directory)
        .arg("build")
        .output()
        .expect("Krit should rebuild");
    assert!(rebuild.status.success());
    let sidecar = metadata_path(&artifact);
    let mut metadata: serde_json::Value =
        serde_json::from_slice(&fs::read(&sidecar).expect("metadata should be readable"))
            .expect("metadata should be JSON");
    metadata["unexpected"] = serde_json::json!(true);
    fs::write(
        &sidecar,
        serde_json::to_vec(&metadata).expect("metadata should serialize"),
    )
    .expect("metadata should be tampered");
    let rejected_metadata = krit_in(&directory)
        .arg("sandbox")
        .output()
        .expect("Krit should start");
    assert_eq!(rejected_metadata.status.code(), Some(1));
    assert!(rejected_metadata.stdout.is_empty());
    assert!(
        String::from_utf8(rejected_metadata.stderr)
            .expect("diagnostic should be UTF-8")
            .contains("error[K7003]")
    );

    let rebuild = krit_in(&directory)
        .arg("build")
        .output()
        .expect("Krit should rebuild");
    assert!(rebuild.status.success());
    fs::write(&manifest, package_manifest(false)).expect("manifest should be narrowed");
    let denied = krit_in(&directory)
        .arg("sandbox")
        .output()
        .expect("Krit should start");
    assert_eq!(denied.status.code(), Some(4));
    assert!(denied.stdout.is_empty());
    assert!(
        String::from_utf8(denied.stderr)
            .expect("diagnostic should be UTF-8")
            .contains("error[K5001]")
    );
}

#[test]
fn sandbox_supports_pure_and_repeated_buffered_output() {
    let pure = TestDirectory::new("sandbox-pure");
    pure.file("main.krit", "let answer = 6 * 7;\n");
    pure.file("krit.pkg", &package_manifest(false));
    assert!(
        krit_in(&pure)
            .arg("build")
            .output()
            .expect("Krit should build")
            .status
            .success()
    );
    let pure_run = krit_in(&pure)
        .arg("sandbox")
        .output()
        .expect("Krit should start");
    assert!(pure_run.status.success());
    assert!(pure_run.stdout.is_empty());
    assert!(pure_run.stderr.is_empty());

    let repeated = TestDirectory::new("sandbox-repeated");
    repeated.file("main.krit", "print(1);\nprintln(true);\nprintln({});\n");
    repeated.file("krit.pkg", &package_manifest(true));
    assert!(
        krit_in(&repeated)
            .arg("build")
            .output()
            .expect("Krit should build")
            .status
            .success()
    );
    for _ in 0..2 {
        let run = krit_in(&repeated)
            .arg("sandbox")
            .output()
            .expect("Krit should start");
        assert!(run.status.success());
        assert_eq!(run.stdout, b"1true\n()\n");
        assert!(run.stderr.is_empty());
    }
}

#[test]
fn sandbox_rolls_back_buffered_output_when_the_guest_fails() {
    let directory = TestDirectory::new("sandbox-output-rollback");
    directory.file("main.krit", "println(1);\nprintln(1 / 0);\n");
    directory.file("krit.pkg", &package_manifest(true));
    assert!(
        krit_in(&directory)
            .arg("build")
            .output()
            .expect("Krit should build")
            .status
            .success()
    );

    let failed = krit_in(&directory)
        .arg("sandbox")
        .output()
        .expect("Krit should start");
    assert_eq!(failed.status.code(), Some(1));
    assert!(failed.stdout.is_empty());
    assert!(
        String::from_utf8(failed.stderr)
            .expect("diagnostic should be UTF-8")
            .contains("error[K4004]")
    );
}

#[test]
fn artifact_permissions_report_allowed_and_denied_human_and_json() {
    let directory = TestDirectory::new("artifact-permissions");
    directory.file("main.krit", "println(42);\n");
    let manifest = directory.file("krit.pkg", &package_manifest(true));
    assert!(
        krit_in(&directory)
            .arg("build")
            .output()
            .expect("Krit should build")
            .status
            .success()
    );
    let artifact = directory.path.join("target/krit/program.wasm");

    let allowed_human = krit_in(&directory)
        .args(["permissions", "--artifact"])
        .arg(&artifact)
        .output()
        .expect("Krit should start");
    assert!(allowed_human.status.success());
    assert!(allowed_human.stderr.is_empty());
    let allowed_human = String::from_utf8(allowed_human.stdout).expect("report should be UTF-8");
    assert!(allowed_human.contains("Required:\n  io.stdout\n"));
    assert!(allowed_human.contains("Effective:\n  io.stdout\n"));
    assert!(allowed_human.contains("Local manifest grants: allowed\n"));
    assert!(allowed_human.contains("Deployment grants: not evaluated\n"));

    let allowed_json = krit_in(&directory)
        .args(["permissions", "--json", "--artifact"])
        .arg(&artifact)
        .output()
        .expect("Krit should start");
    assert!(allowed_json.status.success());
    let report: serde_json::Value =
        serde_json::from_slice(&allowed_json.stdout).expect("report should be JSON");
    assert_eq!(report["localGrantStatus"], "allowed");
    assert_eq!(report["deploymentGrantStatus"], "not-evaluated");
    assert_eq!(report["effective"][0]["capability"], "io.stdout");

    fs::write(&manifest, package_manifest(false)).expect("manifest should be narrowed");
    let denied_human = krit_in(&directory)
        .args(["permissions", "--artifact"])
        .arg(&artifact)
        .output()
        .expect("Krit should start");
    assert_eq!(denied_human.status.code(), Some(4));
    assert!(denied_human.stderr.is_empty());
    let denied_human = String::from_utf8(denied_human.stdout).expect("report should be UTF-8");
    assert!(denied_human.contains("Denied:\n  io.stdout\n"));
    assert!(denied_human.contains("Local manifest grants: denied\n"));
    assert!(denied_human.contains("Deployment grants: not evaluated\n"));

    let denied_json = krit_in(&directory)
        .args(["permissions", "--json", "--artifact"])
        .arg(&artifact)
        .output()
        .expect("Krit should start");
    assert_eq!(denied_json.status.code(), Some(4));
    assert!(denied_json.stderr.is_empty());
    let report: serde_json::Value =
        serde_json::from_slice(&denied_json.stdout).expect("report should be JSON");
    assert_eq!(report["localGrantStatus"], "denied");
    assert_eq!(report["denied"][0]["capability"], "io.stdout");
    assert_eq!(report["deploymentGrantStatus"], "not-evaluated");
}

#[test]
fn sandbox_and_artifact_permissions_use_documented_usage_and_manifest_exits() {
    for arguments in [
        vec!["sandbox", "--artifact"],
        vec!["sandbox", "--unknown"],
        vec!["sandbox", "unexpected"],
        vec!["permissions", "--artifact"],
        vec!["permissions", "--artifact=a", "--artifact=b"],
    ] {
        let output = krit().args(arguments).output().expect("Krit should start");
        assert_eq!(output.status.code(), Some(2));
        assert!(output.stdout.is_empty());
        assert!(
            String::from_utf8(output.stderr)
                .expect("diagnostic should be UTF-8")
                .starts_with("krit: ")
        );
    }

    let directory = TestDirectory::new("sandbox-manifest-exit");
    let missing = directory.path.join("missing.pkg");
    for arguments in [
        vec![
            "sandbox".to_owned(),
            "--manifest".to_owned(),
            missing.to_string_lossy().into_owned(),
        ],
        vec![
            "permissions".to_owned(),
            "--artifact".to_owned(),
            directory
                .path
                .join("missing.wasm")
                .to_string_lossy()
                .into_owned(),
            missing.to_string_lossy().into_owned(),
        ],
    ] {
        let output = krit().args(arguments).output().expect("Krit should start");
        assert_eq!(output.status.code(), Some(3));
        assert!(output.stdout.is_empty());
        assert!(
            String::from_utf8(output.stderr)
                .expect("diagnostic should be UTF-8")
                .contains("error[K6001]")
        );
    }
}

#[test]
fn build_fails_closed_for_capabilities_and_unsupported_layouts() {
    let directory = TestDirectory::new("build-fail-closed");
    directory.file("main.krit", "println(1);\n");
    let manifest = directory.file(
        "krit.pkg",
        r#"
schema = 1

[package]
name = "test/program"
version = "1.0.0"
edition = "2026"
entry = "main.krit"
license = "Apache-2.0"
"#,
    );
    let output = directory.path.join("program.wasm");
    let denied = krit()
        .arg("build")
        .arg("--manifest")
        .arg(&manifest)
        .arg("--output")
        .arg(&output)
        .output()
        .expect("Krit should start");
    assert_eq!(denied.status.code(), Some(4));
    assert!(denied.stdout.is_empty());
    assert!(
        String::from_utf8(denied.stderr)
            .expect("diagnostic should be UTF-8")
            .contains("error[K5001]")
    );
    assert!(!output.exists());
    assert!(!metadata_path(&output).exists());

    fs::write(directory.path.join("main.krit"), "println(\"text\");\n")
        .expect("source should be replaced");
    let manifest_text = fs::read_to_string(&manifest).expect("manifest should be readable");
    fs::write(
        &manifest,
        format!("{manifest_text}\n[capabilities]\nstdout = true\n"),
    )
    .expect("manifest should be replaced");
    let unsupported = krit()
        .arg("build")
        .arg("--manifest")
        .arg(&manifest)
        .arg("--output")
        .arg(&output)
        .output()
        .expect("Krit should start");
    assert_eq!(unsupported.status.code(), Some(1));
    assert!(unsupported.stdout.is_empty());
    assert!(
        String::from_utf8(unsupported.stderr)
            .expect("diagnostic should be UTF-8")
            .contains("error[K7002]")
    );
    assert!(!output.exists());
    assert!(!metadata_path(&output).exists());
}

#[test]
fn build_checks_agent_resources_then_builds_the_webhook_runtime() {
    let directory = TestDirectory::new("build-agent-contracts");
    directory.file(
        "main.krit",
        r#"
webhook fn handle(request: HttpRequest) -> HttpResponse {
    let model = config_string("agent.model");
    let token = secret("github-token");
    record { status: 200, headers: [], body: request.path }
}
"#,
    );
    let manifest = directory.file(
        "krit.pkg",
        r#"
schema = 1

[package]
name = "test/agent"
version = "1.0.0"
edition = "2026"
entry = "main.krit"
license = "Apache-2.0"

[capabilities]
config = ["agent.model"]
"#,
    );
    let output = directory.path.join("agent.wasm");
    let missing = krit()
        .arg("build")
        .arg("--manifest")
        .arg(&manifest)
        .arg("--output")
        .arg(&output)
        .output()
        .expect("Krit should start");
    assert_eq!(missing.status.code(), Some(4));
    assert!(missing.stdout.is_empty());
    let missing = String::from_utf8(missing.stderr).expect("diagnostic should be UTF-8");
    assert!(missing.contains("error[K5001]"));
    assert!(missing.contains("secret.read"));
    assert!(missing.contains("github-token"));
    assert!(!output.exists());
    assert!(!metadata_path(&output).exists());

    fs::write(
        &manifest,
        r#"
schema = 1

[package]
name = "test/agent"
version = "1.0.0"
edition = "2026"
entry = "main.krit"
license = "Apache-2.0"

[capabilities]
config = ["agent.model"]
secrets = ["github-token"]
"#,
    )
    .expect("manifest should be widened");
    let built = krit()
        .arg("build")
        .arg("--manifest")
        .arg(&manifest)
        .arg("--output")
        .arg(&output)
        .output()
        .expect("Krit should start");
    assert!(built.status.success());
    assert!(built.stderr.is_empty());
    assert!(output.exists());
    let metadata: ArtifactMetadata = serde_json::from_slice(
        &fs::read(metadata_path(&output)).expect("metadata should be readable"),
    )
    .expect("metadata should parse");
    assert_eq!(metadata.effects, ["config.read", "secret.read"]);
    assert_eq!(metadata.requirements.len(), 2);
    validate_artifact(
        &fs::read(&output).expect("component should be readable"),
        &metadata,
    )
    .expect("webhook component should validate");
}

#[test]
fn invoke_runs_the_typed_handler_and_prints_only_response_json() {
    let directory = TestDirectory::new("invoke-webhook");
    directory.file(
        "main.krit",
        r#"
webhook fn handle(request: HttpRequest) -> HttpResponse {
    record { status: 201, headers: request.headers, body: request.body }
}
"#,
    );
    let manifest = directory.file(
        "krit.pkg",
        r#"
schema = 1

[package]
name = "test/webhook"
version = "1.0.0"
edition = "2026"
entry = "main.krit"
license = "Apache-2.0"

[capabilities]
"#,
    );
    let built = krit_in(&directory)
        .args(["build", "--manifest"])
        .arg(&manifest)
        .output()
        .expect("Krit should build");
    assert!(
        built.status.success(),
        "{}",
        String::from_utf8_lossy(&built.stderr)
    );
    let request = directory.file(
        "request.json",
        r#"{"method":"POST","path":"/echo","query":"","headers":[{"name":"x-test","value":"one"}],"body":"hello"}"#,
    );
    let invoked = krit_in(&directory)
        .args(["invoke", "--manifest"])
        .arg(&manifest)
        .arg("--request")
        .arg(&request)
        .output()
        .expect("Krit should invoke");
    assert!(invoked.status.success());
    assert!(invoked.stderr.is_empty());
    assert_eq!(
        invoked.stdout,
        b"{\"status\":201,\"headers\":[{\"name\":\"x-test\",\"value\":\"one\"}],\"body\":\"hello\"}\n"
    );
}

#[cfg(unix)]
#[test]
fn invoke_loads_owner_only_secret_files_without_disclosure() {
    use std::os::unix::fs::PermissionsExt;

    let directory = TestDirectory::new("invoke-secret-file");
    directory.file(
        "main.krit",
        r#"
webhook fn handle(request: HttpRequest) -> HttpResponse {
    match secret("unit-secret") {
        Ok(value) => record { status: 204, headers: [], body: "" },
        Err(error) => record { status: 500, headers: [], body: error },
    }
}
"#,
    );
    let manifest = directory.file(
        "krit.pkg",
        r#"
schema = 1

[package]
name = "test/secret-webhook"
version = "1.0.0"
edition = "2026"
entry = "main.krit"
license = "Apache-2.0"

[capabilities]
secrets = ["unit-secret"]
"#,
    );
    let built = krit_in(&directory)
        .args(["build", "--manifest"])
        .arg(&manifest)
        .output()
        .expect("Krit should build");
    assert!(
        built.status.success(),
        "{}",
        String::from_utf8_lossy(&built.stderr)
    );
    let secret_value = "fixture-private-value";
    let secret = directory.file("secret.bin", secret_value);
    fs::set_permissions(&secret, fs::Permissions::from_mode(0o600))
        .expect("secret permissions should set");
    let host = directory.file(
        "host.json",
        r#"{"schema":1,"config":{},"secrets":{"unit-secret":{"file":"secret.bin"}}}"#,
    );
    let request = directory.file(
        "request.json",
        r#"{"method":"POST","path":"/","query":"","headers":[],"body":""}"#,
    );
    let invoked = krit_in(&directory)
        .args(["invoke", "--manifest"])
        .arg(&manifest)
        .arg("--host-config")
        .arg(&host)
        .arg("--request")
        .arg(&request)
        .output()
        .expect("Krit should invoke");
    assert!(
        invoked.status.success(),
        "{}",
        String::from_utf8_lossy(&invoked.stderr)
    );
    assert_eq!(
        invoked.stdout,
        b"{\"status\":204,\"headers\":[],\"body\":\"\"}\n"
    );
    assert!(!String::from_utf8_lossy(&invoked.stdout).contains(secret_value));
    assert!(!String::from_utf8_lossy(&invoked.stderr).contains(secret_value));
    let artifact = directory.path.join("target/krit/secret-webhook.wasm");
    assert!(
        !fs::read(&artifact)
            .expect("artifact should read")
            .windows(secret_value.len())
            .any(|window| window == secret_value.as_bytes())
    );
    assert!(
        !fs::read(metadata_path(&artifact))
            .expect("metadata should read")
            .windows(secret_value.len())
            .any(|window| window == secret_value.as_bytes())
    );

    fs::set_permissions(&secret, fs::Permissions::from_mode(0o644))
        .expect("secret permissions should widen");
    let denied = krit_in(&directory)
        .args(["invoke", "--manifest"])
        .arg(&manifest)
        .arg("--host-config")
        .arg(&host)
        .arg("--request")
        .arg(&request)
        .output()
        .expect("Krit should reject broad secret permissions");
    assert_eq!(denied.status.code(), Some(1));
    assert!(denied.stdout.is_empty());
    assert!(String::from_utf8_lossy(&denied.stderr).contains("group or other permissions"));
    assert!(!String::from_utf8_lossy(&denied.stderr).contains(secret_value));
}

#[test]
fn host_config_cannot_add_manifest_grants() {
    let directory = TestDirectory::new("host-config-grants");
    directory.file(
        "main.krit",
        r#"
webhook fn handle(request: HttpRequest) -> HttpResponse {
    record { status: 200, headers: [], body: request.body }
}
"#,
    );
    let manifest = directory.file(
        "krit.pkg",
        r#"
schema = 1

[package]
name = "test/grants"
version = "1.0.0"
edition = "2026"
entry = "main.krit"
license = "Apache-2.0"

[capabilities]
"#,
    );
    assert!(
        krit_in(&directory)
            .args(["build", "--manifest"])
            .arg(&manifest)
            .output()
            .expect("Krit should build")
            .status
            .success()
    );
    let host = directory.file(
        "host.json",
        r#"{"schema":1,"config":{"extra.key":"value"},"secrets":{}}"#,
    );
    let request = directory.file(
        "request.json",
        r#"{"method":"GET","path":"/","query":"","headers":[],"body":""}"#,
    );
    let denied = krit_in(&directory)
        .args(["invoke", "--manifest"])
        .arg(&manifest)
        .arg("--host-config")
        .arg(&host)
        .arg("--request")
        .arg(&request)
        .output()
        .expect("Krit should reject extra host grants");
    assert_eq!(denied.status.code(), Some(4));
    assert!(denied.stdout.is_empty());
    assert!(String::from_utf8_lossy(&denied.stderr).contains("error[K5001]"));

    fs::write(
        &host,
        r#"{"schema":1,"config":{},"secrets":{},"inlineSecret":"forbidden"}"#,
    )
    .expect("host config should be replaced");
    let malformed = krit_in(&directory)
        .args(["invoke", "--manifest"])
        .arg(&manifest)
        .arg("--host-config")
        .arg(&host)
        .arg("--request")
        .arg(&request)
        .output()
        .expect("Krit should reject unknown host config fields");
    assert_eq!(malformed.status.code(), Some(1));
    assert!(malformed.stdout.is_empty());
    assert!(String::from_utf8_lossy(&malformed.stderr).contains("error[K7003]"));
}

#[test]
fn invoke_keeps_response_json_on_stdout_and_publishes_logs_on_stderr() {
    let directory = TestDirectory::new("invoke-structured-logs");
    directory.file(
        "main.krit",
        r#"
webhook fn handle(request: HttpRequest) -> HttpResponse {
    log_info(
        "request.received",
        [
            record { name: "authorization", value: request.body },
            record { name: "path", value: request.path },
        ],
    );
    record { status: 200, headers: [], body: "ok" }
}
"#,
    );
    let manifest = directory.file(
        "krit.pkg",
        r#"
schema = 1

[package]
name = "test/log-webhook"
version = "1.0.0"
edition = "2026"
entry = "main.krit"
license = "Apache-2.0"

[capabilities]
logs = true
"#,
    );
    let built = krit_in(&directory)
        .args(["build", "--manifest"])
        .arg(&manifest)
        .output()
        .expect("Krit should build");
    assert!(
        built.status.success(),
        "{}",
        String::from_utf8_lossy(&built.stderr)
    );
    let request = directory.file(
        "request.json",
        r#"{"method":"POST","path":"/logged","query":"","headers":[],"body":"private"}"#,
    );
    let invoked = krit_in(&directory)
        .args(["invoke", "--manifest"])
        .arg(&manifest)
        .arg("--request")
        .arg(&request)
        .output()
        .expect("Krit should invoke");
    assert!(invoked.status.success());
    assert_eq!(
        invoked.stdout,
        b"{\"status\":200,\"headers\":[],\"body\":\"ok\"}\n"
    );
    let line: serde_json::Value =
        serde_json::from_slice(&invoked.stderr).expect("stderr should be one JSON log line");
    assert_eq!(line["schema"], 1);
    assert_eq!(line["sequence"], 0);
    assert_eq!(line["event"], "request.received");
    assert_eq!(line["outcome"], "success");
    assert_eq!(line["fields"][0]["value"], "[REDACTED]");
    assert_eq!(line["fields"][1]["value"], "/logged");
    assert!(!String::from_utf8_lossy(&invoked.stderr).contains("private"));
}

#[test]
fn failed_invoke_publishes_only_redacted_failure_logs_and_no_response() {
    let directory = TestDirectory::new("invoke-failure-logs");
    directory.file(
        "main.krit",
        r#"
webhook fn handle(request: HttpRequest) -> HttpResponse {
    log_error(
        "request.failed",
        [record { name: "api-key", value: request.body }],
    );
    record { status: 99, headers: [], body: "not-published" }
}
"#,
    );
    let manifest = directory.file(
        "krit.pkg",
        r#"
schema = 1

[package]
name = "test/failure-log-webhook"
version = "1.0.0"
edition = "2026"
entry = "main.krit"
license = "Apache-2.0"

[capabilities]
logs = true
"#,
    );
    assert!(
        krit_in(&directory)
            .args(["build", "--manifest"])
            .arg(&manifest)
            .output()
            .expect("Krit should build")
            .status
            .success()
    );
    let request = directory.file(
        "request.json",
        r#"{"method":"POST","path":"/","query":"","headers":[],"body":"private-failure"}"#,
    );
    let invoked = krit_in(&directory)
        .args(["invoke", "--manifest"])
        .arg(&manifest)
        .arg("--request")
        .arg(&request)
        .output()
        .expect("Krit should invoke");
    assert_eq!(invoked.status.code(), Some(1));
    assert!(invoked.stdout.is_empty());
    let stderr = String::from_utf8(invoked.stderr).expect("stderr should be UTF-8");
    let mut lines = stderr.lines();
    let log: serde_json::Value =
        serde_json::from_str(lines.next().expect("failure log")).expect("valid log JSON");
    assert_eq!(log["outcome"], "failure");
    assert_eq!(log["fields"][0]["value"], "[REDACTED]");
    assert!(lines.next().expect("diagnostic").contains("error[K4001]"));
    assert!(!stderr.contains("private-failure"));
    assert!(!stderr.contains("not-published"));
}

#[test]
fn schema_two_host_policy_cannot_add_ai_or_http_authority() {
    let directory = TestDirectory::new("host-config-schema-two-grants");
    directory.file(
        "main.krit",
        r#"
webhook fn handle(request: HttpRequest) -> HttpResponse {
    record { status: 200, headers: [], body: request.body }
}
"#,
    );
    let manifest = directory.file(
        "krit.pkg",
        r#"
schema = 1

[package]
name = "test/schema-two"
version = "1.0.0"
edition = "2026"
entry = "main.krit"
license = "Apache-2.0"

[capabilities]
"#,
    );
    assert!(
        krit_in(&directory)
            .args(["build", "--manifest"])
            .arg(&manifest)
            .output()
            .expect("Krit should build")
            .status
            .success()
    );
    let host = directory.file(
        "host.json",
        r#"{
  "schema": 2,
  "aiAdapters": {
    "reviewer": {
      "kind": "http-json",
      "origin": "https://ai.example",
      "path": "/invoke",
      "model": "test",
      "maxInputBytes": 1024,
      "maxResponseBytes": 1024,
      "timeoutMs": 100
    }
  }
}"#,
    );
    let request = directory.file(
        "request.json",
        r#"{"method":"POST","path":"/","query":"","headers":[],"body":""}"#,
    );
    let denied = krit_in(&directory)
        .args(["invoke", "--manifest"])
        .arg(&manifest)
        .arg("--host-config")
        .arg(&host)
        .arg("--request")
        .arg(&request)
        .output()
        .expect("Krit should reject host-added AI authority");
    assert_eq!(denied.status.code(), Some(4));
    assert!(denied.stdout.is_empty());
    assert!(String::from_utf8_lossy(&denied.stderr).contains("AI adapter `reviewer`"));
}

#[test]
fn schema_two_host_policy_is_compatible_with_a_nonexecuted_ai_branch() {
    let directory = TestDirectory::new("host-config-schema-two-valid");
    directory.file(
        "main.krit",
        r#"
webhook fn handle(request: HttpRequest) -> HttpResponse {
    if true {
        record { status: 200, headers: [], body: request.body }
    } else {
        match ai_invoke("reviewer", request.body) {
            Ok(output) => record { status: 200, headers: [], body: output },
            Err(error) => record { status: 503, headers: [], body: error },
        }
    }
}
"#,
    );
    let manifest = directory.file(
        "krit.pkg",
        r#"
schema = 1

[package]
name = "test/schema-two-valid"
version = "1.0.0"
edition = "2026"
entry = "main.krit"
license = "Apache-2.0"

[capabilities]
http = ["https://ai.example.invalid"]
ai = ["reviewer"]
"#,
    );
    assert!(
        krit_in(&directory)
            .args(["build", "--manifest"])
            .arg(&manifest)
            .output()
            .expect("Krit should build")
            .status
            .success()
    );
    let host = directory.file(
        "host.json",
        r#"{
  "schema": 2,
  "aiAdapters": {
    "reviewer": {
      "kind": "http-json",
      "origin": "https://ai.example.invalid",
      "path": "/invoke",
      "model": "test",
      "maxInputBytes": 1024,
      "maxResponseBytes": 1024,
      "timeoutMs": 100
    }
  },
  "approvals": [
    {"operation": "ai.invoke", "resource": "reviewer"}
  ]
}"#,
    );
    let request = directory.file(
        "request.json",
        r#"{"method":"POST","path":"/","query":"","headers":[],"body":"ok"}"#,
    );
    let invoked = krit_in(&directory)
        .args(["invoke", "--manifest"])
        .arg(&manifest)
        .arg("--host-config")
        .arg(&host)
        .arg("--request")
        .arg(&request)
        .output()
        .expect("Krit should invoke");
    assert!(
        invoked.status.success(),
        "{}",
        String::from_utf8_lossy(&invoked.stderr)
    );
    assert_eq!(
        invoked.stdout,
        b"{\"status\":200,\"headers\":[],\"body\":\"ok\"}\n"
    );
}

#[test]
fn serve_once_handles_a_real_http_request() {
    let directory = TestDirectory::new("serve-once");
    directory.file(
        "main.krit",
        r#"
webhook fn handle(request: HttpRequest) -> HttpResponse {
    log_info("serve.received", [record { name: "path", value: request.path }]);
    record {
        status: 203,
        headers: [record { name: "x-krit", value: "served" }],
        body: request.body,
    }
}
"#,
    );
    let manifest = directory.file(
        "krit.pkg",
        r#"
schema = 1

[package]
name = "test/server"
version = "1.0.0"
edition = "2026"
entry = "main.krit"
license = "Apache-2.0"

[capabilities]
logs = true
"#,
    );
    assert!(
        krit_in(&directory)
            .args(["build", "--manifest"])
            .arg(&manifest)
            .output()
            .expect("Krit should build")
            .status
            .success()
    );
    let mut child = krit_in(&directory)
        .args(["serve", "--manifest"])
        .arg(&manifest)
        .args(["--bind", "127.0.0.1:0", "--once"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("Krit server should start");
    let stderr = child.stderr.take().expect("server stderr should be piped");
    let mut stderr = BufReader::new(stderr);
    let mut listening = String::new();
    stderr
        .read_line(&mut listening)
        .expect("server listening line should read");
    assert!(listening.starts_with("krit serve listening on http://127.0.0.1:"));
    let address = listening
        .trim()
        .strip_prefix("krit serve listening on http://")
        .expect("listening prefix should exist");
    let mut stream = TcpStream::connect(address).expect("server should accept connections");
    stream
        .write_all(
            b"POST /hook?one=1 HTTP/1.1\r\nHost: localhost\r\nContent-Length: 5\r\nConnection: close\r\n\r\nhello",
        )
        .expect("request should write");
    let mut response = Vec::new();
    stream
        .read_to_end(&mut response)
        .expect("response should read");
    let mut log_line = String::new();
    stderr
        .read_line(&mut log_line)
        .expect("structured log line should read");
    let log: serde_json::Value = serde_json::from_str(&log_line).expect("serve log should be JSON");
    assert_eq!(log["event"], "serve.received");
    assert_eq!(log["outcome"], "success");
    let output = child
        .wait_with_output()
        .expect("one-shot server should exit");
    assert!(output.status.success());
    assert!(output.stdout.is_empty());
    let response = String::from_utf8(response).expect("response should be UTF-8");
    assert!(response.starts_with("HTTP/1.1 203 "));
    assert!(
        response
            .to_ascii_lowercase()
            .contains("\r\nx-krit: served\r\n")
    );
    assert!(response.ends_with("\r\n\r\nhello"));
}

#[test]
fn serve_once_rejects_oversized_input_without_guest_execution() {
    let directory = TestDirectory::new("serve-oversized");
    directory.file(
        "main.krit",
        r#"
webhook fn handle(request: HttpRequest) -> HttpResponse {
    println(999);
    record { status: 200, headers: [], body: request.body }
}
"#,
    );
    let manifest = directory.file(
        "krit.pkg",
        r#"
schema = 1

[package]
name = "test/oversized"
version = "1.0.0"
edition = "2026"
entry = "main.krit"
license = "Apache-2.0"

[capabilities]
stdout = true
"#,
    );
    assert!(
        krit_in(&directory)
            .args(["build", "--manifest"])
            .arg(&manifest)
            .output()
            .expect("Krit should build")
            .status
            .success()
    );
    let mut child = krit_in(&directory)
        .args(["serve", "--manifest"])
        .arg(&manifest)
        .args(["--bind", "127.0.0.1:0", "--once"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("Krit server should start");
    let stderr = child.stderr.take().expect("server stderr should be piped");
    let mut stderr = BufReader::new(stderr);
    let mut listening = String::new();
    stderr
        .read_line(&mut listening)
        .expect("server listening line should read");
    let address = listening
        .trim()
        .strip_prefix("krit serve listening on http://")
        .expect("listening prefix should exist");
    let mut stream = TcpStream::connect(address).expect("server should accept connections");
    stream
        .write_all(
            b"POST / HTTP/1.1\r\nHost: localhost\r\nContent-Length: 1048577\r\nConnection: close\r\n\r\n",
        )
        .expect("oversized request headers should write");
    stream
        .write_all(&vec![b'x'; 1_048_577])
        .expect("oversized request body should write");
    let mut response = Vec::new();
    stream
        .read_to_end(&mut response)
        .expect("rejection response should read");
    let output = child
        .wait_with_output()
        .expect("one-shot server should exit");
    assert!(output.status.success());
    assert!(output.stdout.is_empty(), "guest must not execute");
    assert!(
        String::from_utf8(response)
            .expect("response should be UTF-8")
            .starts_with("HTTP/1.1 413 ")
    );
}

#[test]
fn build_output_failure_preserves_the_previous_pair() {
    let directory = TestDirectory::new("build-atomic-failure");
    directory.file("main.krit", "println(1);\n");
    let manifest = directory.file(
        "krit.pkg",
        r#"
schema = 1

[package]
name = "test/program"
version = "1.0.0"
edition = "2026"
entry = "main.krit"
license = "Apache-2.0"

[capabilities]
stdout = true
"#,
    );
    let output = directory.file("program.wasm", "previous component");
    let metadata = metadata_path(&output);
    fs::create_dir(&metadata).expect("metadata destination should be a directory");

    let failed = krit()
        .arg("build")
        .arg("--manifest")
        .arg(&manifest)
        .arg("--output")
        .arg(&output)
        .output()
        .expect("Krit should start");
    assert_eq!(failed.status.code(), Some(1));
    assert!(failed.stdout.is_empty());
    assert!(
        String::from_utf8(failed.stderr)
            .expect("diagnostic should be UTF-8")
            .contains("error[K7003]")
    );
    assert_eq!(
        fs::read_to_string(&output).expect("old component should remain"),
        "previous component"
    );
    assert!(metadata.is_dir());
    assert_eq!(
        fs::read_dir(&directory.path)
            .expect("test directory should be readable")
            .filter_map(Result::ok)
            .filter(|entry| entry.file_name().to_string_lossy().contains(".krit-"))
            .count(),
        0,
        "failed build should not leave staged files"
    );
}

#[test]
fn build_rejects_invalid_options() {
    for arguments in [
        vec!["build", "--unknown"],
        vec!["build", "--manifest"],
        vec!["build", "--output"],
        vec!["build", "krit.pkg"],
    ] {
        let output = krit().args(arguments).output().expect("Krit should start");
        assert_eq!(output.status.code(), Some(2));
        assert!(output.stdout.is_empty());
        assert!(
            String::from_utf8(output.stderr)
                .expect("diagnostic should be UTF-8")
                .starts_with("krit: ")
        );
    }
}

#[cfg(unix)]
#[test]
fn build_rejects_an_entry_symlink_that_escapes_the_package() {
    use std::os::unix::fs::symlink;

    let directory = TestDirectory::new("build-entry-symlink");
    let outside = directory
        .path
        .parent()
        .expect("test directory should have a parent")
        .join(format!(
            "outside-{}-{}.krit",
            std::process::id(),
            TEST_DIRECTORY_ID.fetch_add(1, Ordering::Relaxed)
        ));
    fs::write(&outside, "println(1);\n").expect("outside source should be written");
    symlink(&outside, directory.path.join("main.krit")).expect("entry symlink should be created");
    let manifest = directory.file(
        "krit.pkg",
        r#"
schema = 1

[package]
name = "test/program"
version = "1.0.0"
edition = "2026"
entry = "main.krit"
license = "Apache-2.0"

[capabilities]
stdout = true
"#,
    );

    let output = krit()
        .arg("build")
        .arg("--manifest")
        .arg(&manifest)
        .output()
        .expect("Krit should start");
    let _ = fs::remove_file(&outside);
    assert_eq!(output.status.code(), Some(3));
    assert!(output.stdout.is_empty());
    assert!(
        String::from_utf8(output.stderr)
            .expect("diagnostic should be UTF-8")
            .contains("error[K6001]")
    );
    assert!(!directory.path.join("target").exists());
}

fn assert_valid_artifact(path: &Path) {
    let bytes = fs::read(path).expect("component should be readable");
    let metadata: ArtifactMetadata = serde_json::from_slice(
        &fs::read(metadata_path(path)).expect("artifact metadata should be readable"),
    )
    .expect("artifact metadata should be valid JSON");
    validate_artifact(&bytes, &metadata).expect("CLI artifact should validate");
}

#[test]
fn schema_three_state_survives_invoke_process_restarts_and_reports_permissions() {
    let directory = TestDirectory::new("schema-three-state");
    #[cfg(unix)]
    {
        use std::os::unix::fs::{DirBuilderExt, PermissionsExt};

        fs::set_permissions(&directory.path, fs::Permissions::from_mode(0o700))
            .expect("test root should be owner-only");
        fs::DirBuilder::new()
            .mode(0o700)
            .create(directory.path.join("state"))
            .expect("state directory should be owner-only");
    }
    #[cfg(not(unix))]
    fs::create_dir(directory.path.join("state")).expect("state directory should exist");
    directory.file(
        "main.krit",
        r#"
webhook fn handle(request: HttpRequest) -> HttpResponse {
    match state_get("agent-work", "last") {
        Ok(previous) => match state_put("agent-work", "last", request.body) {
            Ok(done) => match previous {
                Some(value) => record { status: 200, headers: [], body: value },
                None => record { status: 200, headers: [], body: "none" },
            },
            Err(error) => record { status: 500, headers: [], body: error },
        },
        Err(error) => record { status: 500, headers: [], body: error },
    }
}
"#,
    );
    directory.file(
        "krit.pkg",
        r#"
schema = 1

[package]
name = "example/state"
version = "0.2.0"
edition = "2026"
entry = "main.krit"
license = "Apache-2.0"

[capabilities]
state = ["agent-work"]
"#,
    );
    directory.file(
        "host.json",
        r#"{
  "schema": 3,
  "state": {
    "stores": {
      "agent-work": {
        "path": "state/agent-work.db",
        "durability": "full",
        "busyTimeoutMs": 250,
        "maxOperations": 128,
        "maxKeyBytes": 256,
        "maxValueBytes": 65536,
        "maxTransactionBytes": 1048576,
        "maxDatabaseBytes": 67108864,
        "maxReplayEntries": 1024,
        "maxReplayBytes": 16777216,
        "replayTtlSeconds": 604800,
        "leaseSeconds": 30
      }
    },
    "durableIdempotencyStore": "agent-work"
  }
}"#,
    );
    directory.file(
        "request-one.json",
        r#"{"method":"POST","path":"/","query":"","headers":[],"body":"one"}"#,
    );
    directory.file(
        "request-two.json",
        r#"{"method":"POST","path":"/","query":"","headers":[],"body":"two"}"#,
    );

    let build = krit_in(&directory)
        .arg("build")
        .output()
        .expect("state package should build");
    assert!(
        build.status.success(),
        "{}",
        String::from_utf8_lossy(&build.stderr)
    );
    let explain = krit_in(&directory)
        .args(["explain", "--json", "main.krit"])
        .output()
        .expect("state explanation should run");
    assert!(explain.status.success());
    let explain: serde_json::Value =
        serde_json::from_slice(&explain.stdout).expect("explanation should be JSON");
    assert_eq!(explain["durableState"]["schema"], 1);
    assert_eq!(
        explain["durableState"]["operations"][0]["kind"],
        "state-get"
    );
    assert_eq!(
        explain["durableState"]["operations"][0]["store"],
        "agent-work"
    );
    let artifact = directory.path.join("target/krit/state.wasm");
    let permissions = krit_in(&directory)
        .args(["permissions", "--json", "--artifact"])
        .arg(&artifact)
        .output()
        .expect("state permissions should run");
    assert!(permissions.status.success());
    let permissions: serde_json::Value =
        serde_json::from_slice(&permissions.stdout).expect("permissions should be JSON");
    assert_eq!(
        permissions["required"][0],
        serde_json::json!({
            "capability": "state.transaction",
            "resource": "agent-work"
        })
    );
    assert_eq!(
        permissions["imports"],
        serde_json::json!(["krit:runtime/state@0.2.0"])
    );

    let first = krit_in(&directory)
        .args([
            "invoke",
            "--host-config",
            "host.json",
            "--request",
            "request-one.json",
        ])
        .output()
        .expect("first state invocation should run");
    assert!(
        first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );
    let first: serde_json::Value =
        serde_json::from_slice(&first.stdout).expect("response should be JSON");
    assert_eq!(first["body"], "none");

    let second = krit_in(&directory)
        .args([
            "invoke",
            "--host-config",
            "host.json",
            "--request",
            "request-two.json",
        ])
        .output()
        .expect("second state invocation should run");
    assert!(
        second.status.success(),
        "{}",
        String::from_utf8_lossy(&second.stderr)
    );
    let second: serde_json::Value =
        serde_json::from_slice(&second.stdout).expect("response should be JSON");
    assert_eq!(second["body"], "one");
    let database = directory.path.join("state/agent-work.db");
    assert!(database.is_file());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        assert_eq!(
            fs::metadata(&database).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
}

#[cfg(unix)]
#[test]
fn schema_three_rejects_insecure_state_directories_and_ungranted_stores() {
    use std::os::unix::fs::PermissionsExt;

    let directory = TestDirectory::new("schema-three-state-denial");
    fs::create_dir(directory.path.join("state")).expect("state directory should exist");
    fs::set_permissions(
        directory.path.join("state"),
        fs::Permissions::from_mode(0o755),
    )
    .expect("state directory mode should be set");
    directory.file(
        "main.krit",
        r#"webhook fn handle(request: HttpRequest) -> HttpResponse {
    record { status: 200, headers: [], body: request.body }
}
"#,
    );
    directory.file(
        "krit.pkg",
        r#"
schema = 1

[package]
name = "example/state-denial"
version = "0.2.0"
edition = "2026"
entry = "main.krit"
license = "Apache-2.0"
"#,
    );
    directory.file(
        "host.json",
        r#"{
  "schema": 3,
  "state": {
    "stores": {
      "agent-work": {
        "path": "state/agent-work.db",
        "durability": "full",
        "busyTimeoutMs": 250,
        "maxOperations": 128,
        "maxKeyBytes": 256,
        "maxValueBytes": 65536,
        "maxTransactionBytes": 1048576,
        "maxDatabaseBytes": 67108864,
        "maxReplayEntries": 1024,
        "maxReplayBytes": 16777216,
        "replayTtlSeconds": 604800,
        "leaseSeconds": 30
      }
    },
    "durableIdempotencyStore": null
  }
}"#,
    );
    directory.file(
        "request.json",
        r#"{"method":"POST","path":"/","query":"","headers":[],"body":""}"#,
    );
    assert!(
        krit_in(&directory)
            .arg("build")
            .output()
            .expect("pure webhook should build")
            .status
            .success()
    );
    let denied = krit_in(&directory)
        .args([
            "invoke",
            "--host-config",
            "host.json",
            "--request",
            "request.json",
        ])
        .output()
        .expect("state denial should run");
    assert_eq!(denied.status.code(), Some(4));
    assert!(
        String::from_utf8(denied.stderr)
            .unwrap()
            .contains("durable state store `agent-work` is not granted")
    );

    let granted = fs::read_to_string(directory.path.join("krit.pkg"))
        .unwrap()
        .replace(
            "license = \"Apache-2.0\"",
            "license = \"Apache-2.0\"\n\n[capabilities]\nstate = [\"agent-work\"]",
        );
    fs::write(directory.path.join("krit.pkg"), granted).unwrap();
    let insecure = krit_in(&directory)
        .args([
            "invoke",
            "--host-config",
            "host.json",
            "--request",
            "request.json",
        ])
        .output()
        .expect("insecure state path should run");
    assert_eq!(insecure.status.code(), Some(1));
    assert!(
        String::from_utf8(insecure.stderr)
            .unwrap()
            .contains("owner-only")
    );

    fs::set_permissions(
        directory.path.join("state"),
        fs::Permissions::from_mode(0o700),
    )
    .unwrap();
    let outside = directory.file("outside.db", "");
    std::os::unix::fs::symlink(&outside, directory.path.join("state/agent-work.db"))
        .expect("state symlink should be created");
    let symlinked = krit_in(&directory)
        .args([
            "invoke",
            "--host-config",
            "host.json",
            "--request",
            "request.json",
        ])
        .output()
        .expect("symlinked state path should run");
    assert_eq!(symlinked.status.code(), Some(1));
    assert!(
        String::from_utf8(symlinked.stderr)
            .unwrap()
            .contains("unsafe")
    );

    fs::remove_file(directory.path.join("state/agent-work.db")).unwrap();
    fs::write(directory.path.join("state/agent-work.db"), b"not sqlite").unwrap();
    fs::set_permissions(
        directory.path.join("state/agent-work.db"),
        fs::Permissions::from_mode(0o600),
    )
    .unwrap();
    let corrupt = krit_in(&directory)
        .args([
            "invoke",
            "--host-config",
            "host.json",
            "--request",
            "request.json",
        ])
        .output()
        .expect("corrupt state path should run");
    assert_eq!(corrupt.status.code(), Some(1));
    assert!(
        String::from_utf8(corrupt.stderr)
            .unwrap()
            .contains("error[K5201]")
    );
}

fn metadata_path(path: &Path) -> PathBuf {
    let mut metadata = path.as_os_str().to_os_string();
    metadata.push(".json");
    PathBuf::from(metadata)
}

fn package_manifest(stdout: bool) -> String {
    format!(
        r#"
schema = 1

[package]
name = "test/program"
version = "1.2.3"
edition = "2026"
entry = "main.krit"
license = "Apache-2.0"

[capabilities]
stdout = {stdout}
"#
    )
}
