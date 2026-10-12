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
fn postfix_question_marks_run_explain_format_and_report_misplacement() {
    let directory = TestDirectory::new("postfix-question");
    directory.file(
        "main.krit",
        "fn digit(text: String) -> Result<Int, String> {\n    if text == \"1\" {\n        Ok(1)\n    \
         } else {\n        Err(\"bad \" + text)\n    }\n}\n\nfn twice(text: String) -> Result<Int, \
         String> {\n    Ok(digit(text)? * 2)\n}\n\nprintln(twice(\"1\"));\nprintln(twice(\"x\"));\n",
    );
    directory.file(
        "webhook.krit",
        "webhook fn handle(request: HttpRequest) -> HttpResponse {\n    \
         let model = config_string(\"agent.model\")?;\n    \
         record { status: 200, headers: [], body: model }\n}\n",
    );

    let run = krit_in(&directory)
        .args(["run", "main.krit"])
        .output()
        .expect("Krit should start");
    assert!(
        run.status.success(),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
    assert_eq!(run.stdout, b"Ok(2)\nErr(\"bad x\")\n");

    let format = krit_in(&directory)
        .args(["fmt", "--check", "main.krit"])
        .output()
        .expect("Krit should start");
    assert!(format.status.success());

    let explain = krit_in(&directory)
        .args(["explain", "main.krit"])
        .output()
        .expect("Krit should start");
    assert!(explain.status.success());
    let explanation = String::from_utf8(explain.stdout).expect("explanation should be UTF-8");
    assert!(explanation.contains("match-result"), "{explanation}");
    assert!(explanation.contains(": Int = exit v"), "{explanation}");

    for command in ["check", "run"] {
        let refused = krit_in(&directory)
            .args([command, "webhook.krit"])
            .output()
            .expect("Krit should start");
        assert_eq!(refused.status.code(), Some(1), "{command}");
        assert!(refused.stdout.is_empty(), "{command}");
        let stderr = String::from_utf8(refused.stderr).expect("diagnostics should be UTF-8");
        assert!(
            stderr.starts_with(
                "webhook.krit:2:45: error[K3012]: `?` is not allowed in a webhook entrypoint, \
                 which returns `HttpResponse`\nhelp: bind each fallible step with let-else here"
            ),
            "{command}: {stderr}"
        );
    }
}

#[test]
fn let_else_runs_explains_formats_and_reports_misplacement() {
    let directory = TestDirectory::new("let-else");
    directory.file(
        "main.krit",
        "fn digit(text: String) -> Result<Int, String> {\n    if text == \"1\" {\n        Ok(1)\n    \
         } else {\n        Err(\"bad \" + text)\n    }\n}\n\nfn describe(text: String) -> String {\n    \
         let Ok(value) = digit(text) else Err(error) => \"failed: \" + error;\n    \
         json_encode(value * 2)\n}\n\nprintln(describe(\"1\"));\nprintln(describe(\"x\"));\n",
    );
    directory.file(
        "nested.krit",
        "fn describe(input: Option<String>) -> String {\n    let text = {\n        \
         let Some(text) = input else None => \"none\";\n        text\n    };\n    text\n}\n",
    );

    let run = krit_in(&directory)
        .args(["run", "main.krit"])
        .output()
        .expect("Krit should start");
    assert!(
        run.status.success(),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
    assert_eq!(run.stdout, b"2\nfailed: bad x\n");

    let format = krit_in(&directory)
        .args(["fmt", "--check", "main.krit"])
        .output()
        .expect("Krit should start");
    assert!(
        format.status.success(),
        "{}",
        String::from_utf8_lossy(&format.stderr)
    );

    let explain = krit_in(&directory)
        .args(["explain", "main.krit"])
        .output()
        .expect("Krit should start");
    assert!(explain.status.success());
    let explanation = String::from_utf8(explain.stdout).expect("explanation should be UTF-8");
    assert!(explanation.contains("let value: Int"), "{explanation}");
    assert!(explanation.contains("match error: String"), "{explanation}");
    assert!(explanation.contains(": Int = exit v"), "{explanation}");

    for command in ["check", "run"] {
        let refused = krit_in(&directory)
            .args([command, "nested.krit"])
            .output()
            .expect("Krit should start");
        assert_eq!(refused.status.code(), Some(1), "{command}");
        assert!(refused.stdout.is_empty(), "{command}");
        let stderr = String::from_utf8(refused.stderr).expect("diagnostics should be UTF-8");
        assert!(
            stderr.starts_with(
                "nested.krit:3:9: error[K3014]: let-else must be a statement of the function \
                 body itself, not of a nested block\nhelp: "
            ),
            "{command}: {stderr}"
        );
    }
}

#[test]
fn nested_patterns_run_explain_format_and_report_missing_outcomes() {
    let directory = TestDirectory::new("nested-patterns");
    directory.file(
        "main.krit",
        "fn describe(stored: Result<Option<String>, String>) -> String {\n    match stored {\n        \
         Ok(Some(value)) => \"found \" + value,\n        Ok(None) => \"missing\",\n        \
         Err(_) => \"failed\",\n    }\n}\n\nprintln(describe(Ok(Some(\"a\"))));\n\
         println(describe(Ok(None)));\nprintln(describe(Err(\"b\")));\n",
    );
    directory.file(
        "missing.krit",
        "fn describe(stored: Result<Option<String>, String>) -> String {\n    match stored {\n        \
         Ok(Some(value)) => value,\n    }\n}\n",
    );
    directory.file(
        "catch-all.krit",
        "fn describe(stored: Option<String>) -> String {\n    match stored {\n        \
         Some(value) => value,\n        _ => \"none\",\n    }\n}\n",
    );

    let run = krit_in(&directory)
        .args(["run", "main.krit"])
        .output()
        .expect("Krit should start");
    assert!(
        run.status.success(),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
    assert_eq!(run.stdout, b"found a\nmissing\nfailed\n");

    let format = krit_in(&directory)
        .args(["fmt", "--check", "main.krit"])
        .output()
        .expect("Krit should start");
    assert!(
        format.status.success(),
        "{}",
        String::from_utf8_lossy(&format.stderr)
    );

    let explain = krit_in(&directory)
        .args(["explain", "main.krit"])
        .output()
        .expect("Krit should start");
    assert!(explain.status.success());
    let explanation = String::from_utf8(explain.stdout).expect("explanation should be UTF-8");
    assert!(explanation.contains("match value: String"), "{explanation}");
    assert!(
        explanation.contains("match $payload: Option<String>"),
        "{explanation}"
    );
    assert!(explanation.contains("match _: String"), "{explanation}");

    for (file, expected) in [
        (
            "missing.krit",
            "missing.krit:2:5: error[K3005]: non-exhaustive match; missing: `Ok(None)`, \
             `Err(_)`\nhelp: add an arm for each missing pattern: `Ok(None) => ...` and \
             `Err(_) => ...`\n",
        ),
        (
            "catch-all.krit",
            "catch-all.krit:4:9: error[K3005]: a match arm cannot match every value: `_` would \
             stand for outcomes it does not name\nhelp: name every outcome: replace `_ => ...` \
             with `None => ...`\n",
        ),
    ] {
        for command in ["check", "run"] {
            let refused = krit_in(&directory)
                .args([command, file])
                .output()
                .expect("Krit should start");
            assert_eq!(refused.status.code(), Some(1), "{command} {file}");
            assert!(refused.stdout.is_empty(), "{command} {file}");
            let stderr = String::from_utf8(refused.stderr).expect("diagnostics should be UTF-8");
            assert_eq!(stderr, expected, "{command} {file}");
        }
    }
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
fn direct_run_rejects_all_deployment_entrypoints_before_module_execution() {
    for (kind, declaration) in [
        (
            "webhook",
            r#"webhook fn handle(input: HttpRequest) -> HttpResponse {
    record { status: 200, headers: [], body: input.path }
}"#,
        ),
        (
            "queue",
            r#"queue "private-jobs" fn handle(input: QueueJob) -> Result<String, String> {
    Ok(input.id)
}"#,
        ),
        (
            "schedule",
            r#"schedule "private-tick" fn handle(input: ScheduleEvent) -> Result<String, String> {
    Ok(input.id)
}"#,
        ),
    ] {
        // `run` checks first, so a module that also fails checking reports
        // check's diagnostic and status instead of the entrypoint refusal.
        let directory = TestDirectory::new("run-ill-typed-deployment-entrypoint");
        let source =
            format!("println(\"must not run\");\nlet invalid = 1 + true;\n{declaration}\n");
        directory.file("main.krit", &source);
        for format in ["human", "json"] {
            let check = krit_in(&directory)
                .args(["check", "--diagnostic-format", format, "main.krit"])
                .output()
                .expect("Krit should start");
            let run = krit_in(&directory)
                .args(["run", "--diagnostic-format", format, "main.krit"])
                .output()
                .expect("Krit should start");
            assert_eq!(run.status.code(), Some(1), "{kind} {format}");
            assert_eq!(run.status.code(), check.status.code(), "{kind} {format}");
            assert!(run.stdout.is_empty(), "{kind} {format}");
            assert_eq!(run.stderr, check.stderr, "{kind} {format}");
            assert!(
                String::from_utf8_lossy(&run.stderr).contains("K3001"),
                "{kind} {format}"
            );
        }

        for prefix in ["println(\"must not run\");\n"] {
            let directory = TestDirectory::new("run-deployment-entrypoint");
            let source = format!("{prefix}{declaration}\nprintln(\"must not run either\");\n");
            let path = directory.file("main.krit", &source);
            let message = format!("{kind} entrypoints are unavailable in direct source execution");
            let human = krit_in(&directory)
                .args(["run", "main.krit"])
                .output()
                .expect("Krit should start");
            assert_eq!(human.status.code(), Some(4), "{kind}");
            assert!(human.stdout.is_empty(), "{kind}");
            assert_eq!(
                String::from_utf8(human.stderr).unwrap(),
                format!(
                    "main.krit:{}:1: error[K5003]: {message}\n",
                    prefix.lines().count() + 1
                )
            );

            let mut previous = None;
            for _ in 0..2 {
                let output = krit_in(&directory)
                    .args(["run", "--diagnostic-format=json", "main.krit"])
                    .output()
                    .expect("Krit should start");
                assert_eq!(output.status.code(), Some(4), "{kind}");
                assert!(output.stdout.is_empty(), "{kind}");
                let diagnostic: serde_json::Value =
                    serde_json::from_slice(&output.stderr).expect("diagnostic should be JSON");
                assert_eq!(diagnostic["schema"], 1);
                assert_eq!(diagnostic["code"], "K5003");
                assert_eq!(diagnostic["message"], message);
                assert_eq!(diagnostic["span"]["start"]["byte"], prefix.len());
                assert_eq!(
                    diagnostic["span"]["end"]["byte"],
                    prefix.len() + declaration.len()
                );
                if let Some(previous) = previous.replace(output.stderr.clone()) {
                    assert_eq!(previous, output.stderr, "diagnostics must be deterministic");
                }
            }
            assert_eq!(fs::read_to_string(path).unwrap(), source);
            assert_eq!(fs::read_dir(&directory.path).unwrap().count(), 1);
        }
    }
}

#[test]
fn direct_run_keeps_queue_and_schedule_contextual() {
    let directory = TestDirectory::new("run-contextual-entrypoint-names");
    let source = directory.file(
        "main.krit",
        r#"fn queue(value: String) -> String { value }
let schedule = record { queue: "ordinary" };
println(queue(schedule.queue));
"#,
    );
    let output = krit()
        .arg("run")
        .arg(source)
        .output()
        .expect("Krit should start");
    assert!(output.status.success());
    assert_eq!(output.stdout, b"ordinary\n");
    assert!(output.stderr.is_empty());
}

#[test]
fn explains_queue_and_schedule_entrypoint_contracts() {
    for (kind, parameter, ty, capability) in [
        ("queue", "job", "QueueJob", "queue.consume"),
        ("schedule", "event", "ScheduleEvent", "schedule.trigger"),
    ] {
        let directory = TestDirectory::new("explain-deployment-entrypoint");
        let source = directory.file(
            "main.krit",
            &format!(
                r#"fn model() -> Result<String, String> {{
    config_string("agent.model")
}}
{kind} "private-work" fn handle(input: {ty}) -> Result<String, String> {{
    let configured = model();
    Ok(input.id)
}}
"#
            ),
        );
        let signature = format!("{kind} fn handle({parameter}: {ty}) -> Result<String, String>");
        let json = krit()
            .args(["explain", "--json"])
            .arg(&source)
            .output()
            .expect("Krit should start");
        assert!(json.status.success());
        assert!(json.stderr.is_empty());
        let facts: serde_json::Value =
            serde_json::from_slice(&json.stdout).expect("explanation should be JSON");
        assert_eq!(facts["entrypoints"]["items"].as_array().unwrap().len(), 2);
        let entrypoint = &facts["entrypoints"]["items"][1];
        assert_eq!(entrypoint["kind"], kind);
        assert_eq!(entrypoint["signature"], signature);
        assert_eq!(
            entrypoint["effects"],
            serde_json::json!(["config.read", capability])
        );
        assert_eq!(
            entrypoint["capabilityRequirements"],
            serde_json::json!([
                {"capability": "config.read", "resource": "agent.model"},
                {"capability": capability, "resource": "private-work"}
            ])
        );
        assert!(entrypoint.get("contract").is_none());

        let human = krit()
            .arg("explain")
            .arg(&source)
            .output()
            .expect("Krit should start");
        assert!(human.status.success());
        assert!(human.stderr.is_empty());
        let human = String::from_utf8(human.stdout).unwrap();
        assert!(human.contains(&format!("{kind} contract (schema 1):\n")));
        assert!(human.contains(&format!("  signature: {signature}\n")));
        assert!(human.contains(&format!("  effects: {{config.read, {capability}}}\n")));
        assert!(human.contains(&format!(r#"{capability}("private-work")"#)));
        assert!(!human.contains("webhook contract"));
        assert!(!human.contains("JSON Schema:"));
    }
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

    // Checking comes first: an ill-typed program that also uses an
    // unavailable host reports check's diagnostic, not the host refusal.
    let mixed = directory.file(
        "mixed.krit",
        "let model = config_string(\"agent.model\");\nlet bad = 1 + true;\n",
    );
    let mixed_output = krit()
        .arg("run")
        .arg(&mixed)
        .output()
        .expect("Krit should start");
    assert_eq!(mixed_output.status.code(), Some(1));
    assert!(mixed_output.stdout.is_empty());
    assert!(
        String::from_utf8(mixed_output.stderr)
            .expect("diagnostic should be UTF-8")
            .contains("error[K3001]")
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
            "conformance/check/name/undefined-argument/program.krit",
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

const DISCARD_READ_SOURCE: &str = "let _ = 1;\nprintln(_);\n";
const DISCARD_READ_WARNING: &str = "main.krit:2:9: warning[K2003]: `_` is a discard; reading it will be an error in edition 2027\n";

fn assert_discard_read_json_warning(stderr: &[u8]) {
    let diagnostics = parse_json_lines(stderr);
    assert_eq!(diagnostics.len(), 1, "{}", String::from_utf8_lossy(stderr));
    let warning = &diagnostics[0];
    assert_eq!(warning["schema"], 1);
    assert_eq!(warning["severity"], "warning");
    assert_eq!(warning["code"], "K2003");
    assert_eq!(warning["file"], "main.krit");
    assert_eq!(warning["span"]["start"]["line"], 2);
    assert_eq!(warning["span"]["start"]["column"], 9);
    assert_eq!(warning["span"]["end"]["column"], 10);
}

#[test]
fn check_and_run_print_fix_hints_as_help_lines_and_json_notes() {
    let directory = TestDirectory::new("habit-hints");
    directory.file("main.krit", "fn f(x) {\n    return x;\n}\n");
    directory.file("typo.krit", "let total = 1;\nprintln(totl);\n");

    for command in ["check", "run"] {
        let human = krit_in(&directory)
            .args([command, "main.krit"])
            .output()
            .expect("Krit should start");
        assert_eq!(human.status.code(), Some(1), "{command}");
        assert!(human.stdout.is_empty(), "{command}");
        assert_eq!(
            String::from_utf8_lossy(&human.stderr),
            "main.krit:2:5: error[K1101]: `return` is not Krit syntax\n\
             help: a block's value is its final expression, written without `return` or `;`; \
             for early exit, use postfix `?` on a `Result` or `Option`, or let-else: \
             `let Some(x) = value else None => fallback;`\n",
            "{command}"
        );

        let json = krit_in(&directory)
            .args([command, "--diagnostic-format", "json", "typo.krit"])
            .output()
            .expect("Krit should start");
        assert_eq!(json.status.code(), Some(1), "{command}");
        let diagnostics = parse_json_lines(&json.stderr);
        assert_eq!(diagnostics.len(), 1, "{command}");
        assert_eq!(diagnostics[0]["code"], "K2001");
        assert_eq!(diagnostics[0]["message"], "undefined name `totl`");
        assert_eq!(
            diagnostics[0]["notes"],
            serde_json::json!(["did you mean `total`?"])
        );
    }
}

#[test]
fn check_run_and_explain_print_warnings_without_changing_the_exit_status() {
    let directory = TestDirectory::new("discard-warning");
    directory.file("main.krit", DISCARD_READ_SOURCE);

    let check = krit_in(&directory)
        .args(["check", "main.krit"])
        .output()
        .expect("Krit should start");
    assert_eq!(check.status.code(), Some(0));
    assert_eq!(check.stdout, b"checked main.krit\n");
    assert_eq!(String::from_utf8_lossy(&check.stderr), DISCARD_READ_WARNING);

    let check_json = krit_in(&directory)
        .args(["check", "--diagnostic-format", "json", "main.krit"])
        .output()
        .expect("Krit should start");
    assert_eq!(check_json.status.code(), Some(0));
    assert_eq!(check_json.stdout, b"checked main.krit\n");
    assert_discard_read_json_warning(&check_json.stderr);

    let run = krit_in(&directory)
        .args(["run", "main.krit"])
        .output()
        .expect("Krit should start");
    assert_eq!(run.status.code(), Some(0));
    assert_eq!(run.stdout, b"1\n");
    assert_eq!(String::from_utf8_lossy(&run.stderr), DISCARD_READ_WARNING);

    let run_json = krit_in(&directory)
        .args(["run", "--diagnostic-format=json", "main.krit"])
        .output()
        .expect("Krit should start");
    assert_eq!(run_json.status.code(), Some(0));
    assert_eq!(run_json.stdout, b"1\n");
    assert_discard_read_json_warning(&run_json.stderr);

    let explain = krit_in(&directory)
        .args(["explain", "main.krit"])
        .output()
        .expect("Krit should start");
    assert_eq!(explain.status.code(), Some(0));
    assert!(explain.stdout.starts_with(b"Krit explanation (schema 1)\n"));
    assert_eq!(
        String::from_utf8_lossy(&explain.stderr),
        DISCARD_READ_WARNING
    );

    let explain_json = krit_in(&directory)
        .args(["explain", "--json", "main.krit"])
        .output()
        .expect("Krit should start");
    assert_eq!(explain_json.status.code(), Some(0));
    let explanation: serde_json::Value =
        serde_json::from_slice(&explain_json.stdout).expect("explanation should be JSON");
    assert_eq!(explanation["schema"], 1);
    assert_discard_read_json_warning(&explain_json.stderr);
}

#[test]
fn build_prints_warnings_and_still_builds() {
    let directory = TestDirectory::new("discard-warning-build");
    directory.file("main.krit", DISCARD_READ_SOURCE);
    directory.file("krit.pkg", &package_manifest(true));

    let build = krit_in(&directory)
        .arg("build")
        .output()
        .expect("Krit should build");
    assert_eq!(
        build.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&build.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&build.stderr), DISCARD_READ_WARNING);
    assert!(build.stdout.starts_with(b"built "));
    assert_valid_artifact(&directory.path.join("target/krit/program.wasm"));
}

#[test]
fn warnings_found_before_an_error_are_printed_before_it() {
    let directory = TestDirectory::new("discard-warning-error");
    directory.file("main.krit", "let _ = 1;\nlet text = _ + \"a\";\n");

    for command in ["check", "run"] {
        let human = krit_in(&directory)
            .args([command, "main.krit"])
            .output()
            .expect("Krit should start");
        assert_eq!(human.status.code(), Some(1), "{command}");
        assert!(human.stdout.is_empty(), "{command}");
        let stderr = String::from_utf8(human.stderr).expect("diagnostics should be UTF-8");
        let lines = stderr.lines().collect::<Vec<_>>();
        assert_eq!(lines.len(), 2, "{command}: {stderr}");
        assert!(
            lines[0].starts_with("main.krit:2:12: warning[K2003]: "),
            "{command}: {stderr}"
        );
        assert!(
            lines[1].starts_with("main.krit:2:12: error[K3001]: "),
            "{command}: {stderr}"
        );

        let json = krit_in(&directory)
            .args([command, "--diagnostic-format", "json", "main.krit"])
            .output()
            .expect("Krit should start");
        assert_eq!(json.status.code(), Some(1), "{command}");
        let diagnostics = parse_json_lines(&json.stderr);
        assert_eq!(
            diagnostics
                .iter()
                .map(|diagnostic| (
                    diagnostic["severity"].as_str().unwrap_or_default(),
                    diagnostic["code"].as_str().unwrap_or_default()
                ))
                .collect::<Vec<_>>(),
            [("warning", "K2003"), ("error", "K3001")],
            "{command}"
        );
    }
}

/// Every conformance case directory under `conformance/<suite>`, as sorted
/// repository-relative paths.
fn conformance_cases(suite: &str) -> Vec<PathBuf> {
    let root = repository_root();
    let mut pending = vec![root.join("conformance").join(suite)];
    let mut cases = Vec::new();
    while let Some(directory) = pending.pop() {
        for entry in fs::read_dir(&directory).expect("conformance directory should be readable") {
            let path = entry.expect("directory entry should be readable").path();
            if path.join("program.krit").is_file() {
                cases.push(
                    path.strip_prefix(root)
                        .expect("case should be inside the repository")
                        .to_path_buf(),
                );
            } else if path.is_dir() {
                pending.push(path);
            }
        }
    }
    cases.sort();
    assert!(!cases.is_empty(), "no {suite} conformance cases found");
    cases
}

fn conformance_expectation(case: &Path, name: &str) -> String {
    let path = repository_root().join(case).join(name);
    if path.is_file() {
        fs::read_to_string(path).expect("expectation should be readable")
    } else {
        String::new()
    }
}

fn conformance_codes(case: &Path, name: &str) -> Vec<String> {
    conformance_expectation(case, name)
        .lines()
        .map(str::to_owned)
        .collect()
}

/// The codes of the JSON Lines diagnostics with `severity`, in output order.
fn diagnostic_codes(diagnostics: &[serde_json::Value], severity: &str) -> Vec<String> {
    diagnostics
        .iter()
        .filter(|diagnostic| diagnostic["severity"] == severity)
        .map(|diagnostic| {
            diagnostic["code"]
                .as_str()
                .expect("diagnostic code should be a string")
                .to_owned()
        })
        .collect()
}

/// An error carries the number of JSON `notes` in `expect.notes`, which
/// defaults to exactly one fix for habit codes (`K1101`-`K1112`), method
/// calls (`K3011`), misplaced or mismatched `?` (`K3012`, `K3013`), and
/// misplaced or mismatched let-else (`K3014`) and to none otherwise; every
/// note is a one-line string.
fn assert_notes(diagnostics: &[serde_json::Value], case: &Path) {
    let expected = conformance_expectation(case, "expect.notes");
    for diagnostic in diagnostics {
        let code = diagnostic["code"]
            .as_str()
            .expect("code should be a string");
        let notes = diagnostic["notes"]
            .as_array()
            .expect("notes should be an array");
        if diagnostic["severity"] == "error" {
            let habit = code
                .strip_prefix("K11")
                .and_then(|number| number.parse::<u8>().ok())
                .is_some_and(|number| (1..=12).contains(&number));
            let expected = if expected.is_empty() {
                usize::from(habit || matches!(code, "K3011" | "K3012" | "K3013" | "K3014"))
            } else {
                expected
                    .trim()
                    .parse::<usize>()
                    .expect("expect.notes should contain a number")
            };
            assert_eq!(
                notes.len(),
                expected,
                "{}: note count of {code}",
                case.display()
            );
        }
        assert!(
            notes.iter().all(|note| note
                .as_str()
                .is_some_and(|note| !note.is_empty() && !note.contains('\n'))),
            "{}: every note is a one-line string",
            case.display()
        );
    }
}

#[test]
fn run_refuses_every_check_failure_with_checks_diagnostic_and_status() {
    for case in conformance_cases("check") {
        let program = case.join("program.krit");
        let expected_status = conformance_expectation(&case, "expect.status");
        for format in ["human", "json"] {
            let check = krit()
                .args(["check", "--diagnostic-format", format])
                .arg(&program)
                .output()
                .expect("Krit should start");
            assert_eq!(
                check.status.code().map(|status| status.to_string()),
                Some(expected_status.trim().to_owned()),
                "{} {format}",
                case.display()
            );
            if format == "json" {
                let diagnostics = parse_json_lines(&check.stderr);
                assert_eq!(
                    diagnostic_codes(&diagnostics, "error"),
                    conformance_codes(&case, "expect.diagnostics"),
                    "{}",
                    case.display()
                );
                assert_eq!(
                    diagnostic_codes(&diagnostics, "warning"),
                    conformance_codes(&case, "expect.warnings"),
                    "{}",
                    case.display()
                );
                assert_eq!(
                    diagnostics.len(),
                    diagnostic_codes(&diagnostics, "error").len()
                        + diagnostic_codes(&diagnostics, "warning").len(),
                    "{}: every diagnostic is an error or a warning",
                    case.display()
                );
                assert_notes(&diagnostics, &case);
            }
            let run = krit()
                .args(["run", "--diagnostic-format", format])
                .arg(&program)
                .output()
                .expect("Krit should start");
            // `run` performs check's front-end checking but no build-target
            // check, since direct execution builds no artifact: a case that
            // fails only with target diagnostics (`K7xxx`) passes it.
            let expected_errors = conformance_codes(&case, "expect.diagnostics");
            let target_only = !expected_errors.is_empty()
                && expected_errors.iter().all(|code| code.starts_with("K7"));
            if check.status.success() || target_only {
                // A valid program runs, or is refused only because direct
                // execution has no entrypoint or host to offer (K5003).
                assert!(
                    matches!(run.status.code(), Some(0 | 4)),
                    "{} {format}: {}",
                    case.display(),
                    String::from_utf8_lossy(&run.stderr)
                );
                continue;
            }
            assert_eq!(
                run.status.code(),
                check.status.code(),
                "{} {format}",
                case.display()
            );
            assert_eq!(
                String::from_utf8_lossy(&run.stderr),
                String::from_utf8_lossy(&check.stderr),
                "{} {format}",
                case.display()
            );
            assert!(run.stdout.is_empty(), "{} {format}", case.display());
        }
    }
}

#[test]
fn runtime_conformance_cases_pass_check_and_run_through_the_cli() {
    for case in conformance_cases("cases") {
        let program = case.join("program.krit");
        let check = krit()
            .arg("check")
            .arg(&program)
            .output()
            .expect("Krit should start");
        assert!(
            check.status.success(),
            "runtime case {} must pass check: {}",
            case.display(),
            String::from_utf8_lossy(&check.stderr)
        );

        let run = krit()
            .args(["run", "--diagnostic-format", "json"])
            .arg(&program)
            .output()
            .expect("Krit should start");
        assert_eq!(
            run.status.code().map(|status| status.to_string()),
            Some(
                conformance_expectation(&case, "expect.status")
                    .trim()
                    .to_owned()
            ),
            "{}",
            case.display()
        );
        assert_eq!(
            String::from_utf8_lossy(&run.stdout),
            conformance_expectation(&case, "expect.stdout"),
            "{}",
            case.display()
        );
        let diagnostics = parse_json_lines(&run.stderr);
        assert_eq!(
            diagnostic_codes(&diagnostics, "error"),
            conformance_codes(&case, "expect.diagnostics"),
            "{}",
            case.display()
        );
        assert_eq!(
            diagnostic_codes(&diagnostics, "warning"),
            conformance_codes(&case, "expect.warnings"),
            "{}",
            case.display()
        );
        assert_eq!(
            diagnostics.len(),
            diagnostic_codes(&diagnostics, "error").len()
                + diagnostic_codes(&diagnostics, "warning").len(),
            "{}: every diagnostic is an error or a warning",
            case.display()
        );
    }
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

/// Generates 2,000 levels of one nesting construct as a package entry.
fn deeply_nested_source(construct: &str) -> String {
    const DEPTH: usize = 2_000;
    let (prefix, open, inner, close) = match construct {
        "parens" => ("", "(", "1", ")"),
        "blocks" => ("", "{ ", "1", " }"),
        "lists" => ("", "[", "1", "]"),
        "records" => ("", "record { a: ", "1", " }"),
        "if" => ("", "if true { ", "1", " } else { 0 }"),
        "match" => ("", "match Some(1) { None => 0, Some(x) => ", "1", " }"),
        "calls" => ("fn f(x) { x }\n", "f(", "1", ")"),
        "types" => {
            return format!(
                "let value: {}Int{} = [];\nprintln(1);\n",
                "List<".repeat(DEPTH),
                ">".repeat(DEPTH)
            );
        }
        _ => unreachable!("unknown construct {construct}"),
    };
    format!(
        "{prefix}println({}{inner}{});\n",
        open.repeat(DEPTH),
        close.repeat(DEPTH)
    )
}

#[test]
fn deep_nesting_reports_k1005_from_every_command_without_aborting() {
    for construct in [
        "parens", "blocks", "lists", "records", "if", "match", "calls", "types",
    ] {
        let directory = TestDirectory::new("deep-nesting");
        let source = directory.file("main.krit", &deeply_nested_source(construct));
        directory.file("krit.pkg", &package_manifest(true));
        for command in [
            vec!["check".into(), source.clone().into_os_string()],
            vec!["run".into(), source.clone().into_os_string()],
            vec!["fmt".into(), source.clone().into_os_string()],
            vec!["build".into()],
        ] {
            let output = krit_in(&directory)
                .args(&command)
                .output()
                .expect("Krit should start");
            let stderr = String::from_utf8_lossy(&output.stderr);
            // A native stack overflow aborts with SIGABRT (shell status 134).
            assert_ne!(output.status.code(), Some(134), "{construct} {command:?}");
            assert_eq!(
                output.status.code(),
                Some(1),
                "{construct} {command:?}: {stderr}"
            );
            assert!(
                stderr.contains("error[K1005]"),
                "{construct} {command:?}: {stderr}"
            );
            assert!(output.stdout.is_empty(), "{construct} {command:?}");
        }
    }
}

fn countdown_source(depth: usize) -> String {
    format!(
        "fn count(n) {{\n    if n == 0 {{\n        0\n    }} else {{\n        1 + count(n - 1)\n    }}\n}}\n\nprintln(count({depth}));\n"
    )
}

#[test]
fn direct_run_reports_k4010_for_runaway_recursion_and_runs_deep_recursion() {
    let directory = TestDirectory::new("recursion-depth");
    let runaway = directory.file("runaway.krit", &countdown_source(100_000));
    let output = krit()
        .args(["run", "--diagnostic-format", "json"])
        .arg(&runaway)
        .output()
        .expect("Krit should start");
    assert_ne!(output.status.code(), Some(134));
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    let diagnostics = parse_json_lines(&output.stderr);
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(diagnostics[0]["code"], "K4010");

    let deep = directory.file("deep.krit", &countdown_source(9_000));
    let output = krit()
        .arg("run")
        .arg(&deep)
        .output()
        .expect("Krit should start");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(output.stdout, b"9000\n");
    assert!(output.stderr.is_empty());
}

/// Operator chains that each sit inside the parentheses of the next, so the
/// innermost operand lies below every operator of every level.
fn parenthesized_spine(levels: usize, operators: usize, operator: &str) -> String {
    let mut expression = format!("1{}", operator.repeat(operators));
    for _ in 1..levels {
        expression = format!("({expression}){}", operator.repeat(operators));
    }
    expression
}

#[test]
fn parenthesized_operator_spines_report_k1005_from_every_command() {
    let directory = TestDirectory::new("operator-spine");
    let source = directory.file(
        "main.krit",
        &format!("println({});\n", parenthesized_spine(3, 2_000, " + 1")),
    );
    directory.file("krit.pkg", &package_manifest(true));
    for command in [
        vec!["check".into(), source.clone().into_os_string()],
        vec!["run".into(), source.clone().into_os_string()],
        vec!["fmt".into(), source.clone().into_os_string()],
        vec!["build".into()],
    ] {
        let output = krit_in(&directory)
            .args(&command)
            .output()
            .expect("Krit should start");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert_ne!(output.status.code(), Some(134), "{command:?}");
        assert_eq!(output.status.code(), Some(1), "{command:?}: {stderr}");
        assert!(stderr.contains("error[K1005]"), "{command:?}: {stderr}");
        assert!(output.stdout.is_empty(), "{command:?}");
    }
}

#[test]
fn language_server_reports_k1005_for_a_parenthesized_operator_spine() {
    // 125 levels of 3,900 operators: a document just under the language
    // server's 1 MiB limit whose syntax tree would be almost half a million
    // operators deep.
    let directory = TestDirectory::new("lsp-operator-spine");
    let text = format!("let value = {};\n", parenthesized_spine(125, 3_900, "+1"));
    assert!(text.len() < 1024 * 1024);
    let source = directory.file("main.krit", &text);
    let uri = format!("file://{}", source.display());
    let messages = [
        serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {"processId": null, "capabilities": {}, "workspaceFolders": null}
        }),
        serde_json::json!({"jsonrpc": "2.0", "method": "initialized", "params": {}}),
        serde_json::json!({
            "jsonrpc": "2.0",
            "method": "textDocument/didOpen",
            "params": {
                "textDocument": {"uri": uri, "languageId": "krit", "version": 1, "text": text}
            }
        }),
        serde_json::json!({"jsonrpc": "2.0", "id": 2, "method": "shutdown", "params": null}),
        serde_json::json!({"jsonrpc": "2.0", "method": "exit", "params": null}),
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

    assert_ne!(output.status.code(), Some(134));
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let frames = parse_lsp_frames(&output.stdout);
    assert_eq!(frames[1]["method"], "textDocument/publishDiagnostics");
    assert_eq!(frames[1]["params"]["diagnostics"][0]["code"], "K1005");
}

#[test]
fn direct_run_refuses_runaway_value_nesting_before_evaluation() {
    // Evaluated, this ill-typed program would wrap the value in 9,000 more
    // lists on each of 1,000 sequential calls. `run` refuses it with check's
    // diagnostic before evaluating anything, so it never reaches the
    // evaluator's value-depth limit (covered by the evaluator's own tests).
    let directory = TestDirectory::new("value-depth");
    let mut text = String::from(
        "fn wrap(x, n) {\n    if n == 0 {\n        x\n    } else {\n        wrap([x], n - 1)\n    }\n}\n\nlet v0 = 0;\n",
    );
    for level in 1..=1_000 {
        text.push_str(&format!("let v{level} = wrap(v{}, 9000);\n", level - 1));
    }
    text.push_str("println(v1000);\nprintln(json_encode(v1000));\nprintln(v1000 == v1000);\n");
    let source = directory.file("main.krit", &text);
    let check = krit()
        .args(["check", "--diagnostic-format", "json"])
        .arg(&source)
        .output()
        .expect("Krit should start");
    let output = krit()
        .args(["run", "--diagnostic-format", "json"])
        .arg(&source)
        .output()
        .expect("Krit should start");
    assert_ne!(output.status.code(), Some(134));
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    assert_eq!(output.stderr, check.stderr);
    let diagnostics = parse_json_lines(&output.stderr);
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(diagnostics[0]["code"], "K3001");
}

#[test]
fn long_else_if_chains_check_run_format_build_and_execute() {
    let directory = TestDirectory::new("else-if-chain");
    let mut text = String::from("fn pick(n) {\n    if n == 0 {\n        0\n    }");
    for branch in 1..1_000 {
        text.push_str(&format!(
            " else if n == {branch} {{\n        {}\n    }}",
            branch * 3
        ));
    }
    text.push_str(" else {\n        -1\n    }\n}\n\nprintln(pick(999));\n");
    let source = directory.file("main.krit", &text);
    directory.file("krit.pkg", &package_manifest(true));

    let check = krit()
        .arg("check")
        .arg(&source)
        .output()
        .expect("Krit should start");
    assert!(
        check.status.success(),
        "{}",
        String::from_utf8_lossy(&check.stderr)
    );

    let run = krit()
        .arg("run")
        .arg(&source)
        .output()
        .expect("Krit should start");
    assert!(
        run.status.success(),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
    assert_eq!(run.stdout, b"2997\n");

    let format = krit()
        .args(["fmt", "--check"])
        .arg(&source)
        .output()
        .expect("Krit should start");
    assert!(
        format.status.success(),
        "{}",
        String::from_utf8_lossy(&format.stderr)
    );

    let build = krit_in(&directory)
        .arg("build")
        .output()
        .expect("Krit should start");
    assert!(
        build.status.success(),
        "{}",
        String::from_utf8_lossy(&build.stderr)
    );
    let sandbox = krit_in(&directory)
        .arg("sandbox")
        .output()
        .expect("Krit should start");
    assert!(
        sandbox.status.success(),
        "{}",
        String::from_utf8_lossy(&sandbox.stderr)
    );
    assert_eq!(sandbox.stdout, b"2997\n");
}

/// The deepest syntax tree the parser accepts: 256 nested `else` blocks (or
/// block expressions), each called as often as the nesting limit allows until
/// the innermost point reaches the structural limit. Analysis and Core
/// lowering recurse through more levels for it than for any other tree.
/// Every call applies `g`, a curried function of 200 parameters, so the
/// program prints `0`.
fn deepest_tree_source(else_blocks: bool) -> String {
    const ARITY: usize = 200;
    let mut text = String::from("fn g(x0) { ");
    for parameter in 1..ARITY {
        text.push_str(&format!("fn(x{parameter}) {{ "));
    }
    text.push('0');
    text.push_str(&" }".repeat(ARITY));
    text.push('\n');
    let mut remaining = krit::MAX_STRUCTURAL_DEPTH - krit::MAX_NESTING_DEPTH;
    let links = (1..=krit::MAX_NESTING_DEPTH)
        .map(|nesting| {
            let count = ARITY
                .min(krit::MAX_NESTING_DEPTH + 1 - nesting)
                .min(remaining);
            remaining -= count;
            count
        })
        .collect::<Vec<_>>();
    let mut expression = String::from("0");
    for count in links.into_iter().rev() {
        let level = if else_blocks {
            format!("if false {{ g }} else {{ let h = {expression}; g }}")
        } else {
            format!("{{ let h = {expression}; g }}")
        };
        expression = format!("{level}{}", "(1)".repeat(count));
    }
    text.push_str(&format!("let value = {expression};\nprintln(value);\n"));
    text
}

#[test]
fn deepest_accepted_trees_pass_every_command() {
    for else_blocks in [true, false] {
        let directory = TestDirectory::new("deepest-tree");
        directory.file("main.krit", &deepest_tree_source(else_blocks));
        directory.file("krit.pkg", &package_manifest(true));
        // Formatting rewrites the file, so checking it again shows the
        // formatted program stays within the limits too.
        for (command, stdout) in [
            (vec!["check", "main.krit"], None),
            (vec!["explain", "--json", "main.krit"], None),
            (vec!["run", "main.krit"], Some(&b"0\n"[..])),
            (vec!["build"], None),
            (vec!["sandbox"], Some(&b"0\n"[..])),
            (vec!["fmt", "main.krit"], None),
            (vec!["check", "main.krit"], None),
        ] {
            let output = krit_in(&directory)
                .args(&command)
                .output()
                .expect("Krit should start");
            let stderr = String::from_utf8_lossy(&output.stderr);
            assert!(
                output.status.success(),
                "else blocks {else_blocks} {command:?}: {stderr}"
            );
            assert!(
                stderr.is_empty(),
                "else blocks {else_blocks} {command:?}: {stderr}"
            );
            if let Some(stdout) = stdout {
                assert_eq!(
                    output.stdout, stdout,
                    "else blocks {else_blocks} {command:?}"
                );
            }
        }
    }
}

#[test]
fn unifying_solved_types_reports_limits_without_aborting() {
    // Unifying `shallow` with `deep` deepens `top` to 260 levels; unifying
    // `e` with a type containing it would be infinite.
    let directory = TestDirectory::new("solved-type-unification");
    let cases = [
        (
            format!(
                "let shallow = [[[]]];\nlet top = {}shallow{};\n\
                 let deep = [[[[[[[[[[1]]]]]]]]]];\nlet grows = shallow == deep;\n",
                "[".repeat(250),
                "]".repeat(250)
            ),
            "K1005",
        ),
        (
            "let e = [];\nlet wrapped = [[e]];\nlet same = e == wrapped;\n".to_owned(),
            "K3001",
        ),
    ];
    for (index, (text, code)) in cases.iter().enumerate() {
        let source = directory.file(&format!("case{index}.krit"), text);
        for command in ["check", "explain", "run"] {
            let output = krit()
                .arg(command)
                .arg(&source)
                .output()
                .expect("Krit should start");
            let stderr = String::from_utf8_lossy(&output.stderr);
            assert_ne!(output.status.code(), Some(134), "{code} {command}");
            assert_eq!(output.status.code(), Some(1), "{code} {command}: {stderr}");
            assert!(
                stderr.contains(&format!("error[{code}]")),
                "{code} {command}: {stderr}"
            );
            assert!(output.stdout.is_empty(), "{code} {command}");
        }
    }
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
fn every_manifest_command_refuses_declared_dependencies() {
    let directory = TestDirectory::new("dependencies-fail-closed");
    directory.file("main.krit", "println(1);\n");
    let without_dependencies = r#"
schema = 1

[package]
name = "test/program"
version = "1.0.0"
edition = "2026"
entry = "main.krit"
license = "Apache-2.0"

[dependencies]

[capabilities]
stdout = true
"#;
    let manifest = directory.file(
        "krit.pkg",
        &without_dependencies.replace(
            "[dependencies]\n",
            "[dependencies]\n\"krit/json\" = \"1.2.3\"\n",
        ),
    );
    let manifest_arg = manifest.to_string_lossy().into_owned();
    let request = directory
        .path
        .join("request.json")
        .to_string_lossy()
        .into_owned();
    for arguments in [
        vec!["package", "check", &manifest_arg],
        vec!["build", "--manifest", &manifest_arg],
        vec!["sandbox", "--manifest", &manifest_arg],
        vec!["permissions", &manifest_arg],
        vec!["permissions", "--json", &manifest_arg],
        vec!["invoke", "--manifest", &manifest_arg, "--request", &request],
        vec!["worker", "--manifest", &manifest_arg, "--queue", "jobs"],
        vec![
            "schedule",
            "--manifest",
            &manifest_arg,
            "--schedule",
            "hourly",
        ],
        vec!["serve", "--manifest", &manifest_arg, "--once"],
    ] {
        let output = krit_in(&directory)
            .args(&arguments)
            .output()
            .expect("Krit should start");
        assert_eq!(output.status.code(), Some(3), "{arguments:?}");
        assert!(output.stdout.is_empty(), "{arguments:?}");
        assert_eq!(
            String::from_utf8(output.stderr).expect("diagnostic should be UTF-8"),
            format!(
                "{manifest_arg}:1:1: error[K6003]: dependency resolution is not supported; remove [dependencies]\n"
            ),
            "{arguments:?}"
        );
    }
    assert!(!directory.path.join("target").exists());

    fs::write(&manifest, without_dependencies).expect("manifest should be rewritten");
    let output = krit_in(&directory)
        .args(["package", "check", &manifest_arg])
        .output()
        .expect("Krit should start");
    assert!(output.status.success());
    assert!(output.stderr.is_empty());
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

#[test]
fn invoke_routes_on_strings_and_composes_responses() {
    let directory = TestDirectory::new("invoke-string-routing");
    let source = r#"
webhook fn handle(request: HttpRequest) -> HttpResponse {
    if request.method == "POST" && request.path == "/x" {
        record { status: 200, headers: [], body: "hi " + request.path }
    } else {
        if request.method != "POST" {
            record { status: 405, headers: [], body: "use POST, not " + request.method }
        } else {
            record { status: 404, headers: [], body: "no route for " + request.path }
        }
    }
}
"#;
    let program = directory.file("main.krit", source);
    let manifest = directory.file(
        "krit.pkg",
        r#"
schema = 1

[package]
name = "test/routing"
version = "1.0.0"
edition = "2026"
entry = "main.krit"
license = "Apache-2.0"

[capabilities]
"#,
    );
    let checked = krit()
        .arg("check")
        .arg(&program)
        .output()
        .expect("Krit should check");
    assert!(
        checked.status.success(),
        "{}",
        String::from_utf8_lossy(&checked.stderr)
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
    for (name, method, path, expected) in [
        (
            "routed",
            "POST",
            "/x",
            r#"{"status":200,"headers":[],"body":"hi /x"}"#,
        ),
        (
            "method",
            "GET",
            "/x",
            r#"{"status":405,"headers":[],"body":"use POST, not GET"}"#,
        ),
        (
            "prefix",
            "POST",
            "/x/y",
            r#"{"status":404,"headers":[],"body":"no route for /x/y"}"#,
        ),
        (
            "multibyte",
            "POST",
            "/é",
            r#"{"status":404,"headers":[],"body":"no route for /é"}"#,
        ),
    ] {
        let request = directory.file(
            &format!("{name}.json"),
            &format!(
                r#"{{"method":"{method}","path":"{path}","query":"","headers":[],"body":""}}"#
            ),
        );
        let invoked = krit_in(&directory)
            .args(["invoke", "--manifest"])
            .arg(&manifest)
            .arg("--request")
            .arg(&request)
            .output()
            .expect("Krit should invoke");
        assert!(
            invoked.status.success(),
            "{name}: {}",
            String::from_utf8_lossy(&invoked.stderr)
        );
        assert!(invoked.stderr.is_empty(), "{name}");
        assert_eq!(
            String::from_utf8(invoked.stdout).expect("response should be UTF-8"),
            format!("{expected}\n"),
            "{name}"
        );
    }
}

#[test]
fn invoke_decodes_escaped_json_strings_and_reports_invalid_json() {
    let directory = TestDirectory::new("invoke-json-escapes");
    let program = directory.file(
        "main.krit",
        r#"
webhook fn handle(request: HttpRequest) -> HttpResponse {
    let summary: String = json_decode(request.body);
    record { status: 200, headers: [], body: "summary: " + summary }
}
"#,
    );
    let manifest = directory.file(
        "krit.pkg",
        r#"
schema = 1

[package]
name = "test/json-escapes"
version = "1.0.0"
edition = "2026"
entry = "main.krit"
license = "Apache-2.0"

[capabilities]
"#,
    );
    let checked = krit()
        .args(["check", "--target", "component"])
        .arg(&program)
        .output()
        .expect("Krit should check");
    assert!(
        checked.status.success(),
        "{}",
        String::from_utf8_lossy(&checked.stderr)
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
    let invoke = |name: &str, body: &str| {
        let request = directory.file(
            &format!("{name}.json"),
            &serde_json::json!({
                "method": "POST",
                "path": "/summary",
                "query": "",
                "headers": [],
                "body": body,
            })
            .to_string(),
        );
        krit_in(&directory)
            .args(["invoke", "--manifest"])
            .arg(&manifest)
            .arg("--request")
            .arg(&request)
            .output()
            .expect("Krit should invoke")
    };

    // Model output: a JSON string with every escape kind and a trailing
    // newline outside the string.
    let escaped = invoke(
        "escaped",
        "\"line one\\nsaid \\\"hi\\\"\\t\\\\ \\/ \\u00e9 \\ud83d\\ude00\\r\\b\\f\"\n",
    );
    assert!(
        escaped.status.success(),
        "{}",
        String::from_utf8_lossy(&escaped.stderr)
    );
    let response: serde_json::Value =
        serde_json::from_slice(&escaped.stdout).expect("response should be JSON");
    assert_eq!(response["status"], 200);
    assert_eq!(
        response["body"],
        "summary: line one\nsaid \"hi\"\t\\ / \u{e9} \u{1f600}\r\u{8}\u{c}"
    );

    // A lone surrogate is invalid JSON, reported like the evaluator reports
    // it (K4009), with no response and without echoing the input.
    let lone = invoke("lone", "\"secret \\ud800 text\"");
    assert_eq!(lone.status.code(), Some(1));
    assert!(lone.stdout.is_empty());
    let stderr = String::from_utf8(lone.stderr).expect("stderr should be UTF-8");
    assert!(stderr.contains("error[K4009]: invalid JSON"), "{stderr}");
    assert!(!stderr.contains("secret"), "{stderr}");

    let source = directory.file(
        "evaluate.krit",
        "let summary: String = json_decode(\"\\\"secret \\\\ud800 text\\\"\");\n",
    );
    let evaluated = krit()
        .args(["run", "--diagnostic-format", "json"])
        .arg(&source)
        .output()
        .expect("Krit should run");
    assert_eq!(evaluated.status.code(), Some(1));
    let diagnostic: serde_json::Value = serde_json::from_slice(&evaluated.stderr)
        .expect("the evaluator should report one JSON diagnostic");
    assert_eq!(diagnostic["code"], "K4009");
}

#[test]
fn invoke_refuses_request_paths_with_traversal_forms() {
    let directory = TestDirectory::new("invoke-path-traversal");
    directory.file(
        "main.krit",
        r#"
webhook fn handle(request: HttpRequest) -> HttpResponse {
    record { status: 200, headers: [], body: request.path }
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

    for (name, path) in [
        ("dot.json", "/hooks/hidden-seg/../admin"),
        ("encoded-dot.json", "/hooks/hidden-seg/%2E%2e/admin"),
        ("encoded-slash.json", "/hooks/hidden-seg%2Fadmin"),
        ("encoded-backslash.json", "/hooks/hidden-seg%5C..%5Cadmin"),
        ("path-parameter.json", "/hooks/hidden-seg/..;/admin"),
        ("double-encoded.json", "/hooks/hidden-seg/%252e%252e/admin"),
        ("overlong.json", "/hooks/hidden-seg/%c0%ae%c0%ae/admin"),
        ("encoded-nul.json", "/hooks/hidden-seg/..%00/admin"),
        ("encoded-query.json", "/hooks/hidden-seg/..%3F/admin"),
        ("encoded-newline.json", "/hooks/hidden-seg/..%0a/admin"),
        ("padded.json", "/hooks/hidden-seg/..%20/admin"),
        ("lenient-overlong.json", "/hooks/hidden-seg/%c0n%c0n/admin"),
    ] {
        let request = directory.file(
            name,
            &format!(r#"{{"method":"POST","path":"{path}","query":"","headers":[],"body":""}}"#),
        );
        let invoked = krit_in(&directory)
            .args(["invoke", "--manifest"])
            .arg(&manifest)
            .arg("--request")
            .arg(&request)
            .output()
            .expect("Krit should invoke");
        assert_eq!(invoked.status.code(), Some(1), "`{path}`");
        assert!(invoked.stdout.is_empty(), "`{path}`");
        let stderr = String::from_utf8(invoked.stderr).expect("stderr should be UTF-8");
        assert!(stderr.contains("error[K4001]"), "`{path}`: {stderr}");
        // The diagnostic never echoes the refused path.
        assert!(!stderr.contains("hidden-seg"), "`{path}` leaked: {stderr}");
    }

    let request = directory.file(
        "ordinary.json",
        r#"{"method":"POST","path":"/hooks/v1.2/a%20b","query":"","headers":[],"body":""}"#,
    );
    let invoked = krit_in(&directory)
        .args(["invoke", "--manifest"])
        .arg(&manifest)
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
        b"{\"status\":200,\"headers\":[],\"body\":\"/hooks/v1.2/a%20b\"}\n"
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
fn underscored_configuration_keys_flow_from_manifest_to_guest() {
    let directory = TestDirectory::new("config-underscore");
    directory.file(
        "main.krit",
        r#"
webhook fn handle(request: HttpRequest) -> HttpResponse {
    match config_string("agent.timeout_ms") {
        Ok(value) => record { status: 200, headers: [], body: value },
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
name = "test/config-underscore"
version = "1.0.0"
edition = "2026"
entry = "main.krit"
license = "Apache-2.0"

[capabilities]
config = ["agent.timeout_ms"]
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
    let host = directory.file(
        "host.json",
        r#"{"schema":1,"config":{"agent.timeout_ms":"1500"},"secrets":{}}"#,
    );
    let request = directory.file(
        "request.json",
        r#"{"method":"GET","path":"/","query":"","headers":[],"body":""}"#,
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
        b"{\"status\":200,\"headers\":[],\"body\":\"1500\"}\n"
    );

    // The hyphenated spelling is a different key and is not granted.
    fs::write(
        &host,
        r#"{"schema":1,"config":{"agent.timeout-ms":"1500"},"secrets":{}}"#,
    )
    .expect("host config should be replaced");
    let denied = krit_in(&directory)
        .args(["invoke", "--manifest"])
        .arg(&manifest)
        .arg("--host-config")
        .arg(&host)
        .arg("--request")
        .arg(&request)
        .output()
        .expect("Krit should reject an ungranted key");
    assert_eq!(denied.status.code(), Some(4));
    assert!(denied.stdout.is_empty());
    assert!(String::from_utf8_lossy(&denied.stderr).contains("error[K5001]"));
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

#[cfg(unix)]
#[test]
fn schema_three_state_survives_invoke_process_restarts_and_reports_permissions() {
    use std::os::unix::fs::{DirBuilderExt, PermissionsExt};

    let directory = TestDirectory::new("schema-three-state");
    fs::set_permissions(&directory.path, fs::Permissions::from_mode(0o700))
        .expect("test root should be owner-only");
    fs::DirBuilder::new()
        .mode(0o700)
        .create(directory.path.join("state"))
        .expect("state directory should be owner-only");
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
    assert_eq!(
        fs::metadata(&database).unwrap().permissions().mode() & 0o777,
        0o600
    );
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

/// Secret files and durable stores rely on owner-only file permissions, which
/// only Unix hosts enforce. Elsewhere every host-mode command refuses such a
/// configuration before reading a secret or resolving a store; on Unix the same
/// configurations reach those file checks and fail there instead.
#[test]
fn host_mode_refuses_secrets_and_durable_state_where_owner_only_files_are_unenforced() {
    let directory = TestDirectory::new("owner-only-platform");
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
name = "example/owner-only-platform"
version = "0.2.0"
edition = "2026"
entry = "main.krit"
license = "Apache-2.0"

[capabilities]
secrets = ["token"]
state = ["agent-work"]
"#,
    );
    // Neither the secret file nor the state directory exists, so Unix hosts
    // fail on them; other hosts must refuse before looking for them.
    directory.file(
        "secrets.host.json",
        r#"{"schema":1,"secrets":{"token":{"file":"token"}}}"#,
    );
    directory.file(
        "state.host.json",
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
    }
  }
}"#,
    );
    directory.file(
        "request.json",
        r#"{"method":"POST","path":"/","query":"","headers":[],"body":""}"#,
    );
    let build = krit_in(&directory)
        .arg("build")
        .output()
        .expect("pure webhook should build");
    assert!(
        build.status.success(),
        "{}",
        String::from_utf8_lossy(&build.stderr)
    );

    for config in ["secrets.host.json", "state.host.json"] {
        for command in [
            vec!["invoke", "--request", "request.json"],
            vec!["worker", "--queue", "render-jobs", "--once"],
            vec!["schedule", "--schedule", "hourly", "--once"],
            vec!["serve", "--bind", "127.0.0.1:0", "--once"],
        ] {
            let output = krit_in(&directory)
                .args(&command)
                .args(["--host-config", config])
                .output()
                .expect("host mode should start");
            let stderr = String::from_utf8_lossy(&output.stderr);
            assert!(output.stdout.is_empty(), "{command:?} {config}");
            if cfg!(unix) {
                assert_eq!(
                    output.status.code(),
                    Some(1),
                    "{command:?} {config}: {stderr}"
                );
                assert!(
                    stderr.contains("error[K7003]") && !stderr.contains("cannot enforce"),
                    "{command:?} {config}: {stderr}"
                );
            } else {
                assert_eq!(
                    output.status.code(),
                    Some(4),
                    "{command:?} {config}: {stderr}"
                );
                assert!(
                    stderr.contains("error[K5001]")
                        && stderr.contains(
                            "owner-only file permissions that this platform cannot enforce"
                        ),
                    "{command:?} {config}: {stderr}"
                );
            }
            assert!(
                !directory.path.join("state").exists(),
                "{command:?} {config}"
            );
        }
    }
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

/// Host mode with durable state needs owner-only files, which only Unix hosts
/// enforce; elsewhere it is refused (see
/// `host_mode_refuses_secrets_and_durable_state_where_owner_only_files_are_unenforced`),
/// so the durable host-mode tests are Unix-only.
#[cfg(unix)]
fn owner_only_directory(name: &str) -> TestDirectory {
    use std::os::unix::fs::{DirBuilderExt, PermissionsExt};

    let directory = TestDirectory::new(name);
    fs::set_permissions(&directory.path, fs::Permissions::from_mode(0o700))
        .expect("test root should be owner-only");
    fs::DirBuilder::new()
        .mode(0o700)
        .create(directory.path.join("state"))
        .expect("state directory should be owner-only");
    directory
}

#[cfg(unix)]
const JOBS_STORE: &str = r#"
  "state": {
    "stores": {
      "agent-work": {
        "path": "state/jobs.db",
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
    }
  }
"#;

#[cfg(unix)]
const JOBS_QUEUE: &str = r#"
      "render-jobs": {
        "store": "agent-work",
        "maxDepth": 1024,
        "maxJobBytes": 65536,
        "maxQueueBytes": 8388608,
        "maxAttempts": 2,
        "leaseSeconds": 30,
        "backoffSeconds": 1,
        "maxBackoffSeconds": 60,
        "deadLetterMaxEntries": 16,
        "deadLetterRetentionSeconds": 3600
      }
"#;

#[cfg(unix)]
const JOBS_BUCKET: &str = r#"
      "render-output": {
        "store": "agent-work",
        "maxObjects": 64,
        "maxKeyBytes": 128,
        "maxObjectBytes": 4096,
        "maxBucketBytes": 65536
      }
"#;

#[cfg(unix)]
#[test]
fn worker_once_dispatches_one_durable_delivery_across_processes() {
    let directory = owner_only_directory("worker-once");
    directory.file(
        "publish.krit",
        r#"
webhook fn handle(request: HttpRequest) -> HttpResponse {
    match queue_publish("render-jobs", request.body) {
        Ok(id) => record { status: 202, headers: [], body: id },
        Err(error) => record { status: 500, headers: [], body: error },
    }
}
"#,
    );
    directory.file(
        "publish.pkg",
        r#"
schema = 1

[package]
name = "example/jobs-publish"
version = "0.2.0"
edition = "2026"
entry = "publish.krit"
license = "Apache-2.0"

[capabilities]
queues = ["render-jobs"]
"#,
    );
    directory.file(
        "worker.krit",
        r#"
queue "render-jobs" fn handle(job: QueueJob) -> Result<String, String> {
    match object_put("render-output", job.id, job.body) {
        Ok(stored) => match checkpoint_put("agent-work", "last-render", job.id) {
            Ok(marked) => Ok(job.body),
            Err(error) => Err(error),
        },
        Err(error) => Err(error),
    }
}
"#,
    );
    directory.file(
        "worker.pkg",
        r#"
schema = 1

[package]
name = "example/jobs-worker"
version = "0.2.0"
edition = "2026"
entry = "worker.krit"
license = "Apache-2.0"

[capabilities]
buckets = ["render-output"]
consumes = ["render-jobs"]
state = ["agent-work"]
"#,
    );
    directory.file(
        "publish.host.json",
        &format!("{{\n  \"schema\": 4,{JOBS_STORE},\n  \"jobs\": {{\n    \"queues\": {{{JOBS_QUEUE}}}\n  }}\n}}"),
    );
    directory.file(
        "worker.host.json",
        &format!(
            "{{\n  \"schema\": 4,{JOBS_STORE},\n  \"jobs\": {{\n    \"queues\": {{{JOBS_QUEUE}}},\n    \"buckets\": {{{JOBS_BUCKET}}}\n  }}\n}}"
        ),
    );
    directory.file(
        "request.json",
        r#"{"method":"POST","path":"/render","query":"","headers":[],"body":"payload"}"#,
    );

    for manifest in ["publish.pkg", "worker.pkg"] {
        let build = krit_in(&directory)
            .args(["build", "--manifest", manifest])
            .output()
            .expect("job package should build");
        assert!(
            build.status.success(),
            "{}",
            String::from_utf8_lossy(&build.stderr)
        );
    }

    let permissions = krit_in(&directory)
        .args([
            "permissions",
            "--json",
            "--artifact",
            "target/krit/jobs-worker.wasm",
            "worker.pkg",
        ])
        .output()
        .expect("worker permissions should run");
    assert!(permissions.status.success());
    let permissions: serde_json::Value =
        serde_json::from_slice(&permissions.stdout).expect("permissions should be JSON");
    assert_eq!(
        permissions["required"],
        serde_json::json!([
            {"capability": "object.write", "resource": "render-output"},
            {"capability": "queue.consume", "resource": "render-jobs"},
            {"capability": "state.transaction", "resource": "agent-work"},
        ])
    );
    assert_eq!(permissions["localGrantStatus"], "allowed");
    let rendered = serde_json::to_string(&permissions).expect("permissions should render");
    assert!(!rendered.contains("state/jobs.db"));

    let publish = krit_in(&directory)
        .args([
            "invoke",
            "--manifest",
            "publish.pkg",
            "--host-config",
            "publish.host.json",
            "--request",
            "request.json",
        ])
        .output()
        .expect("publish should run");
    assert!(
        publish.status.success(),
        "{}",
        String::from_utf8_lossy(&publish.stderr)
    );
    let response: serde_json::Value =
        serde_json::from_slice(&publish.stdout).expect("response should be JSON");
    assert_eq!(response["status"], 202);
    let identity = response["body"].as_str().expect("identity should be text");

    let worker = krit_in(&directory)
        .args([
            "worker",
            "--queue",
            "render-jobs",
            "--manifest",
            "worker.pkg",
            "--host-config",
            "worker.host.json",
            "--once",
            "--json",
        ])
        .output()
        .expect("worker should run");
    assert!(
        worker.status.success(),
        "{}",
        String::from_utf8_lossy(&worker.stderr)
    );
    let report: serde_json::Value =
        serde_json::from_slice(&worker.stdout).expect("report should be JSON");
    assert_eq!(report["dispatched"], 1);
    assert_eq!(report["completed"], 1);
    assert_eq!(report["outcomes"][0]["outcome"], "completed");
    assert_eq!(report["outcomes"][0]["id"], identity);

    let idle = krit_in(&directory)
        .args([
            "worker",
            "--queue",
            "render-jobs",
            "--manifest",
            "worker.pkg",
            "--host-config",
            "worker.host.json",
            "--once",
            "--json",
        ])
        .output()
        .expect("idle worker should run");
    assert!(idle.status.success());
    let idle: serde_json::Value =
        serde_json::from_slice(&idle.stdout).expect("report should be JSON");
    assert_eq!(idle["dispatched"], 0);
    assert_eq!(idle["idle"], true);
    assert!(directory.path.join("state/jobs.db").is_file());
}

#[cfg(unix)]
#[test]
fn schedule_once_fires_exactly_once_per_host_supplied_instant() {
    let directory = owner_only_directory("schedule-once");
    directory.file(
        "sweep.krit",
        r#"
schedule "hourly-sweep" fn handle(event: ScheduleEvent) -> Result<String, String> {
    match object_put("render-output", event.id, event.schedule) {
        Ok(stored) => Ok(event.id),
        Err(error) => Err(error),
    }
}
"#,
    );
    directory.file(
        "sweep.pkg",
        r#"
schema = 1

[package]
name = "example/jobs-sweep"
version = "0.2.0"
edition = "2026"
entry = "sweep.krit"
license = "Apache-2.0"

[capabilities]
buckets = ["render-output"]
schedules = ["hourly-sweep"]
"#,
    );
    directory.file(
        "sweep.host.json",
        &format!(
            r#"{{
  "schema": 4,{JOBS_STORE},
  "jobs": {{
    "schedules": {{
      "hourly-sweep": {{
        "store": "agent-work",
        "intervalSeconds": 3600,
        "startEpochMillis": 0,
        "maxCatchUp": 2,
        "maxAttempts": 2,
        "leaseSeconds": 30,
        "backoffSeconds": 1,
        "maxBackoffSeconds": 60,
        "retentionSeconds": 3600,
        "maxRetainedFires": 16
      }}
    }},
    "buckets": {{{JOBS_BUCKET}}}
  }}
}}"#
        ),
    );

    let build = krit_in(&directory)
        .args(["build", "--manifest", "sweep.pkg"])
        .output()
        .expect("schedule package should build");
    assert!(
        build.status.success(),
        "{}",
        String::from_utf8_lossy(&build.stderr)
    );

    let arguments = [
        "schedule",
        "--schedule",
        "hourly-sweep",
        "--manifest",
        "sweep.pkg",
        "--host-config",
        "sweep.host.json",
        "--once",
        "--now",
        "7200000",
        "--json",
    ];
    let first = krit_in(&directory)
        .args(arguments)
        .output()
        .expect("first tick should run");
    assert!(
        first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );
    let first: serde_json::Value =
        serde_json::from_slice(&first.stdout).expect("report should be JSON");
    assert_eq!(first["materialized"], 1);
    assert_eq!(first["completed"], 1);
    assert_eq!(first["outcomes"][0]["id"], "hourly-sweep@7200000");

    let repeated = krit_in(&directory)
        .args(arguments)
        .output()
        .expect("repeated tick should run");
    assert!(repeated.status.success());
    let repeated: serde_json::Value =
        serde_json::from_slice(&repeated.stdout).expect("report should be JSON");
    assert_eq!(repeated["materialized"], 0);
    assert_eq!(repeated["dispatched"], 0);
    assert_eq!(repeated["idle"], true);
}

#[cfg(unix)]
#[test]
fn delivery_commands_reject_unbounded_and_ungranted_requests() {
    let directory = owner_only_directory("delivery-guards");
    directory.file(
        "worker.krit",
        r#"
queue "render-jobs" fn handle(job: QueueJob) -> Result<String, String> {
    Ok(job.body)
}
"#,
    );
    directory.file(
        "krit.pkg",
        r#"
schema = 1

[package]
name = "example/jobs-guard"
version = "0.2.0"
edition = "2026"
entry = "worker.krit"
license = "Apache-2.0"

[capabilities]
consumes = ["render-jobs"]
"#,
    );
    directory.file(
        "host.json",
        &format!("{{\n  \"schema\": 4,{JOBS_STORE},\n  \"jobs\": {{\n    \"queues\": {{{JOBS_QUEUE}}}\n  }}\n}}"),
    );
    directory.file(
        "ungranted.host.json",
        &format!(
            "{{\n  \"schema\": 4,{JOBS_STORE},\n  \"jobs\": {{\n    \"queues\": {{{JOBS_QUEUE}}},\n    \"buckets\": {{{JOBS_BUCKET}}}\n  }}\n}}"
        ),
    );
    let build = krit_in(&directory)
        .arg("build")
        .output()
        .expect("guard package should build");
    assert!(
        build.status.success(),
        "{}",
        String::from_utf8_lossy(&build.stderr)
    );

    let missing = krit_in(&directory)
        .args(["worker", "--host-config", "host.json", "--once"])
        .output()
        .expect("worker should run");
    assert_eq!(missing.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&missing.stderr).contains("--queue"));

    let unbounded = krit_in(&directory)
        .args([
            "worker",
            "--queue",
            "render-jobs",
            "--host-config",
            "host.json",
            "--max-deliveries",
            "0",
        ])
        .output()
        .expect("worker should run");
    assert_eq!(unbounded.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&unbounded.stderr).contains("--max-deliveries"));

    let ungranted = krit_in(&directory)
        .args([
            "worker",
            "--queue",
            "render-jobs",
            "--host-config",
            "ungranted.host.json",
            "--once",
        ])
        .output()
        .expect("worker should run");
    assert_eq!(ungranted.status.code(), Some(4));
    assert!(
        String::from_utf8_lossy(&ungranted.stderr).contains("render-output"),
        "{}",
        String::from_utf8_lossy(&ungranted.stderr)
    );
}

// ---------------------------------------------------------------------------
// Regressions for the phase6-jobs-storage review findings.
// ---------------------------------------------------------------------------

/// Finding 10: a worker artifact that also holds `io.stdout` must not corrupt
/// the `--json` report stream.
#[cfg(unix)]
#[test]
fn json_delivery_reports_stay_parseable_when_the_guest_writes_stdout() {
    let directory = owner_only_directory("json-stdout-worker");
    directory.file(
        "publish.krit",
        r#"
webhook fn handle(request: HttpRequest) -> HttpResponse {
    match queue_publish("render-jobs", request.body) {
        Ok(id) => record { status: 202, headers: [], body: id },
        Err(error) => record { status: 500, headers: [], body: error },
    }
}
"#,
    );
    directory.file(
        "publish.pkg",
        r#"
schema = 1

[package]
name = "example/stdout-publish"
version = "0.2.0"
edition = "2026"
entry = "publish.krit"
license = "Apache-2.0"

[capabilities]
queues = ["render-jobs"]
"#,
    );
    directory.file(
        "worker.krit",
        r#"
queue "render-jobs" fn handle(job: QueueJob) -> Result<String, String> {
    let noise = println(job.attempt);
    Ok(job.id)
}
"#,
    );
    directory.file(
        "worker.pkg",
        r#"
schema = 1

[package]
name = "example/stdout-worker"
version = "0.2.0"
edition = "2026"
entry = "worker.krit"
license = "Apache-2.0"

[capabilities]
consumes = ["render-jobs"]
stdout = true
"#,
    );
    directory.file(
        "host.json",
        &format!("{{\n  \"schema\": 4,{JOBS_STORE},\n  \"jobs\": {{\n    \"queues\": {{{JOBS_QUEUE}}}\n  }}\n}}"),
    );
    directory.file(
        "request.json",
        r#"{"method":"POST","path":"/render","query":"","headers":[],"body":"payload"}"#,
    );
    for manifest in ["publish.pkg", "worker.pkg"] {
        let build = krit_in(&directory)
            .args(["build", "--manifest", manifest])
            .output()
            .expect("package should build");
        assert!(
            build.status.success(),
            "{}",
            String::from_utf8_lossy(&build.stderr)
        );
    }
    let publish = krit_in(&directory)
        .args([
            "invoke",
            "--manifest",
            "publish.pkg",
            "--host-config",
            "host.json",
            "--request",
            "request.json",
        ])
        .output()
        .expect("publish should run");
    assert!(
        publish.status.success(),
        "{}",
        String::from_utf8_lossy(&publish.stderr)
    );

    let worker = krit_in(&directory)
        .args([
            "worker",
            "--queue",
            "render-jobs",
            "--manifest",
            "worker.pkg",
            "--host-config",
            "host.json",
            "--once",
            "--json",
        ])
        .output()
        .expect("worker should run");
    assert!(
        worker.status.success(),
        "{}",
        String::from_utf8_lossy(&worker.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&worker.stdout)
        .expect("JSON mode standard output must parse as one report");
    assert_eq!(report["dispatched"], 1);
    assert_eq!(report["completed"], 1);
    assert_eq!(report["outputs"], serde_json::json!(["1\n"]));
    assert_eq!(report["stoppedForOutputBudget"], false);
    assert_eq!(
        report["outcomes"].as_array().map(Vec::len),
        report["outputs"].as_array().map(Vec::len),
        "outcomes and outputs must be parallel arrays"
    );

    // Human mode still streams the guest bytes on standard output.
    let publish = krit_in(&directory)
        .args([
            "invoke",
            "--manifest",
            "publish.pkg",
            "--host-config",
            "host.json",
            "--request",
            "request.json",
        ])
        .output()
        .expect("second publish should run");
    assert!(publish.status.success());
    let human = krit_in(&directory)
        .args([
            "worker",
            "--queue",
            "render-jobs",
            "--manifest",
            "worker.pkg",
            "--host-config",
            "host.json",
            "--once",
        ])
        .output()
        .expect("human worker should run");
    assert!(human.status.success());
    let human = String::from_utf8_lossy(&human.stdout);
    assert!(human.contains("1\n"), "{human}");
    assert!(
        human.contains("worker render-jobs: dispatched 1"),
        "{human}"
    );
}

/// Finding 10: the same guarantee for scheduled triggers.
#[cfg(unix)]
#[test]
fn json_schedule_reports_stay_parseable_when_the_guest_writes_stdout() {
    let directory = owner_only_directory("json-stdout-schedule");
    directory.file(
        "sweep.krit",
        r#"
schedule "hourly-sweep" fn handle(event: ScheduleEvent) -> Result<String, String> {
    let noise = println(event.attempt);
    Ok(event.id)
}
"#,
    );
    directory.file(
        "krit.pkg",
        r#"
schema = 1

[package]
name = "example/stdout-sweep"
version = "0.2.0"
edition = "2026"
entry = "sweep.krit"
license = "Apache-2.0"

[capabilities]
schedules = ["hourly-sweep"]
stdout = true
"#,
    );
    directory.file(
        "host.json",
        &format!(
            r#"{{
  "schema": 4,{JOBS_STORE},
  "jobs": {{
    "schedules": {{
      "hourly-sweep": {{
        "store": "agent-work",
        "intervalSeconds": 3600,
        "startEpochMillis": 0,
        "maxCatchUp": 2,
        "maxAttempts": 2,
        "leaseSeconds": 30,
        "backoffSeconds": 1,
        "maxBackoffSeconds": 60,
        "retentionSeconds": 3600,
        "maxRetainedFires": 16
      }}
    }}
  }}
}}"#
        ),
    );
    let build = krit_in(&directory)
        .arg("build")
        .output()
        .expect("schedule package should build");
    assert!(
        build.status.success(),
        "{}",
        String::from_utf8_lossy(&build.stderr)
    );

    let tick = krit_in(&directory)
        .args([
            "schedule",
            "--schedule",
            "hourly-sweep",
            "--host-config",
            "host.json",
            "--once",
            "--now",
            "7200000",
            "--json",
        ])
        .output()
        .expect("tick should run");
    assert!(
        tick.status.success(),
        "{}",
        String::from_utf8_lossy(&tick.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&tick.stdout)
        .expect("JSON mode standard output must parse as one report");
    assert_eq!(report["completed"], 1);
    assert_eq!(report["outputs"], serde_json::json!(["1\n"]));
    assert_eq!(report["outcomes"][0]["id"], "hourly-sweep@7200000");
}

/// Finding 8: an unrepresentable `--now` fails without moving the cursor, and a
/// later ordinary tick still fires.
#[cfg(unix)]
#[test]
fn extreme_schedule_instants_fail_without_disturbing_later_ticks() {
    let directory = owner_only_directory("extreme-now");
    directory.file(
        "sweep.krit",
        r#"
schedule "hourly-sweep" fn handle(event: ScheduleEvent) -> Result<String, String> {
    Ok(event.id)
}
"#,
    );
    directory.file(
        "krit.pkg",
        r#"
schema = 1

[package]
name = "example/extreme-sweep"
version = "0.2.0"
edition = "2026"
entry = "sweep.krit"
license = "Apache-2.0"

[capabilities]
schedules = ["hourly-sweep"]
"#,
    );
    directory.file(
        "host.json",
        &format!(
            r#"{{
  "schema": 4,{JOBS_STORE},
  "jobs": {{
    "schedules": {{
      "hourly-sweep": {{
        "store": "agent-work",
        "intervalSeconds": 3600,
        "startEpochMillis": 0,
        "maxCatchUp": 2,
        "maxAttempts": 2,
        "leaseSeconds": 30,
        "backoffSeconds": 1,
        "maxBackoffSeconds": 60,
        "retentionSeconds": 3600,
        "maxRetainedFires": 16
      }}
    }}
  }}
}}"#
        ),
    );
    let build = krit_in(&directory)
        .arg("build")
        .output()
        .expect("schedule package should build");
    assert!(build.status.success());

    let extreme = krit_in(&directory)
        .args([
            "schedule",
            "--schedule",
            "hourly-sweep",
            "--host-config",
            "host.json",
            "--once",
            "--now",
            &i64::MAX.to_string(),
            "--json",
        ])
        .output()
        .expect("extreme tick should run");
    assert!(!extreme.status.success());
    assert!(
        String::from_utf8_lossy(&extreme.stderr).contains("K5202"),
        "{}",
        String::from_utf8_lossy(&extreme.stderr)
    );
    assert!(
        extreme.stdout.is_empty(),
        "a refused tick must not emit a report"
    );

    let normal = krit_in(&directory)
        .args([
            "schedule",
            "--schedule",
            "hourly-sweep",
            "--host-config",
            "host.json",
            "--once",
            "--now",
            "7200000",
            "--json",
        ])
        .output()
        .expect("later tick should run");
    assert!(
        normal.status.success(),
        "{}",
        String::from_utf8_lossy(&normal.stderr)
    );
    let report: serde_json::Value =
        serde_json::from_slice(&normal.stdout).expect("report should be JSON");
    assert_eq!(report["materialized"], 1);
    assert_eq!(report["completed"], 1);
    assert_eq!(report["outcomes"][0]["id"], "hourly-sweep@7200000");
}

/// Finding 5: an invalid or ungranted `jobs` section is rejected before any
/// database is created, opened, or migrated.
#[cfg(unix)]
#[test]
fn invalid_job_configuration_never_touches_a_store() {
    let directory = owner_only_directory("jobs-no-side-effects");
    directory.file(
        "worker.krit",
        r#"
queue "render-jobs" fn handle(job: QueueJob) -> Result<String, String> {
    Ok(job.id)
}
"#,
    );
    directory.file(
        "krit.pkg",
        r#"
schema = 1

[package]
name = "example/jobs-side-effects"
version = "0.2.0"
edition = "2026"
entry = "worker.krit"
license = "Apache-2.0"

[capabilities]
consumes = ["render-jobs"]
"#,
    );
    let build = krit_in(&directory)
        .arg("build")
        .output()
        .expect("package should build");
    assert!(build.status.success());

    let bad_depth = format!(
        "{{\n  \"schema\": 4,{JOBS_STORE},\n  \"jobs\": {{\n    \"queues\": {{\n      \"render-jobs\": {{\n        \"store\": \"agent-work\",\n        \"maxDepth\": 0,\n        \"maxJobBytes\": 65536,\n        \"maxQueueBytes\": 8388608,\n        \"maxAttempts\": 2,\n        \"leaseSeconds\": 30,\n        \"backoffSeconds\": 1,\n        \"maxBackoffSeconds\": 60,\n        \"deadLetterMaxEntries\": 16,\n        \"deadLetterRetentionSeconds\": 3600\n      }}\n    }}\n  }}\n}}"
    );
    directory.file("bad-depth.host.json", &bad_depth);
    directory.file(
        "ungranted-bucket.host.json",
        &format!(
            "{{\n  \"schema\": 4,{JOBS_STORE},\n  \"jobs\": {{\n    \"queues\": {{{JOBS_QUEUE}}},\n    \"buckets\": {{{JOBS_BUCKET}}}\n  }}\n}}"
        ),
    );
    directory.file(
        "missing-store.host.json",
        &format!(
            "{{\n  \"schema\": 4,{JOBS_STORE},\n  \"jobs\": {{\n    \"queues\": {{\n      \"render-jobs\": {{\n        \"store\": \"absent\",\n        \"maxDepth\": 8,\n        \"maxJobBytes\": 65536,\n        \"maxQueueBytes\": 8388608,\n        \"maxAttempts\": 2,\n        \"leaseSeconds\": 30,\n        \"backoffSeconds\": 1,\n        \"maxBackoffSeconds\": 60,\n        \"deadLetterMaxEntries\": 16,\n        \"deadLetterRetentionSeconds\": 3600\n      }}\n    }}\n  }}\n}}"
        ),
    );

    let database = directory.path.join("state/jobs.db");
    for (config, expected) in [
        ("bad-depth.host.json", 1),
        ("ungranted-bucket.host.json", 4),
        ("missing-store.host.json", 1),
    ] {
        assert!(
            !database.exists(),
            "no store may exist before `{config}` is evaluated"
        );
        let output = krit_in(&directory)
            .args([
                "worker",
                "--queue",
                "render-jobs",
                "--host-config",
                config,
                "--once",
            ])
            .output()
            .expect("worker should run");
        assert_eq!(output.status.code(), Some(expected), "config: {config}");
        assert!(
            !database.exists(),
            "`{config}` must not create the store: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    // A valid configuration still creates and uses the store.
    directory.file(
        "valid.host.json",
        &format!("{{\n  \"schema\": 4,{JOBS_STORE},\n  \"jobs\": {{\n    \"queues\": {{{JOBS_QUEUE}}}\n  }}\n}}"),
    );
    let output = krit_in(&directory)
        .args([
            "worker",
            "--queue",
            "render-jobs",
            "--host-config",
            "valid.host.json",
            "--once",
            "--json",
        ])
        .output()
        .expect("worker should run");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(database.is_file());
}

const DATABASE_STATEMENTS: &str = r#"
      "statements": {
        "record-visit": {
          "kind": "execute",
          "sql": "INSERT INTO visits(path) VALUES(?1)",
          "parameters": ["text"],
          "columns": []
        },
        "count-visits": {
          "kind": "query",
          "sql": "SELECT COUNT(*) AS total FROM visits",
          "parameters": [],
          "columns": ["total"]
        }
      }"#;

/// Builds a schema-5 host configuration around one application database.
fn database_host_config(path: &str, mode: &str, statements: &str) -> String {
    format!(
        r#"{{
  "schema": 5,
  "state": {{ "stores": {{}} }},
  "maxTransactionsPerInvocation": 1,
  "databases": {{
    "catalog": {{
      "path": "{path}",
      "mode": "{mode}",
      "busyTimeoutMs": 250,
      "maxDatabaseBytes": 16777216,
      "maxTransactionMillis": 500,
      "maxOperationsPerTransaction": 16,
      "maxParameterBytes": 4096,
      "maxRows": 64,
      "maxColumns": 8,
      "maxResultBytes": 65536,{statements}
    }}
  }}
}}"#
    )
}

/// Creates the owner-only application database fixture that the operator, not
/// Krit, is responsible for provisioning and migrating.
fn create_database_fixture(directory: &TestDirectory) -> PathBuf {
    let data = directory.path.join("data");
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;

        fs::DirBuilder::new()
            .mode(0o700)
            .create(&data)
            .expect("fixture directory should be owner-only");
    }
    #[cfg(not(unix))]
    fs::create_dir(&data).expect("fixture directory should exist");

    let path = data.join("catalog.db");
    let connection = rusqlite::Connection::open(&path).expect("fixture database should open");
    connection
        .execute_batch(
            "CREATE TABLE visits(id INTEGER PRIMARY KEY AUTOINCREMENT, path TEXT NOT NULL);",
        )
        .expect("fixture schema should apply");
    drop(connection);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        fs::set_permissions(&path, fs::Permissions::from_mode(0o600))
            .expect("fixture database should be owner-only");
    }
    path
}

fn database_package(directory: &TestDirectory, capability: &str) {
    directory.file(
        "main.krit",
        r#"
webhook fn handle(request: HttpRequest) -> HttpResponse {
    match db_begin_write("catalog") {
        Ok(transaction) => match db_execute(transaction, "record-visit", [request.body]) {
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
    );
    directory.file(
        "krit.pkg",
        &format!(
            r#"
schema = 1

[package]
name = "example/database"
version = "0.2.0"
edition = "2026"
entry = "main.krit"
license = "Apache-2.0"

[capabilities]
{capability} = ["catalog"]
"#
        ),
    );
}

fn invoke_database(directory: &TestDirectory, config: &str, request: &str) -> std::process::Output {
    krit_in(directory)
        .args(["invoke", "--host-config", config, "--request", request])
        .output()
        .expect("database invocation should run")
}

#[cfg(unix)]
#[test]
fn schema_five_database_transactions_commit_across_invoke_processes() {
    use std::os::unix::fs::PermissionsExt;

    let directory = TestDirectory::new("schema-five-database");
    fs::set_permissions(&directory.path, fs::Permissions::from_mode(0o700))
        .expect("test root should be owner-only");
    let database = create_database_fixture(&directory);
    database_package(&directory, "databases");
    directory.file(
        "host.json",
        &database_host_config("data/catalog.db", "read-write", DATABASE_STATEMENTS),
    );
    directory.file(
        "request-one.json",
        r#"{"method":"POST","path":"/","query":"","headers":[],"body":"first"}"#,
    );
    // The payload below is stored as an ordinary bound parameter; it can never
    // reach the SQL text because the operator owns the statement catalog.
    directory.file(
        "request-two.json",
        r#"{"method":"POST","path":"/","query":"","headers":[],"body":"x'); DROP TABLE visits; --"}"#,
    );

    let build = krit_in(&directory)
        .arg("build")
        .output()
        .expect("database package should build");
    assert!(
        build.status.success(),
        "{}",
        String::from_utf8_lossy(&build.stderr)
    );
    let artifact = directory.path.join("target/krit/database.wasm");
    assert_valid_artifact(&artifact);

    let permissions = krit_in(&directory)
        .args(["permissions", "--json", "--artifact"])
        .arg(&artifact)
        .output()
        .expect("database permissions should run");
    assert!(permissions.status.success());
    let permissions: serde_json::Value =
        serde_json::from_slice(&permissions.stdout).expect("permissions should be JSON");
    assert_eq!(
        permissions["required"],
        serde_json::json!([{ "capability": "database.write", "resource": "catalog" }])
    );
    assert_eq!(
        permissions["imports"],
        serde_json::json!(["krit:runtime/database@0.2.0"])
    );
    let rendered = serde_json::to_string(&permissions).expect("permissions should serialise");
    assert!(
        !rendered.contains("catalog.db")
            && !rendered.contains("INSERT")
            && !rendered.contains("data/")
            && !rendered.contains(".db"),
        "permissions must never disclose host paths or SQL: {rendered}"
    );

    let explain = krit_in(&directory)
        .args(["explain", "--json", "main.krit"])
        .output()
        .expect("database explanation should run");
    assert!(explain.status.success());
    let explain: serde_json::Value =
        serde_json::from_slice(&explain.stdout).expect("explanation should be JSON");
    let operations = explain["durableState"]["operations"]
        .as_array()
        .expect("durable operations should be an array")
        .iter()
        .map(|operation| {
            (
                operation["kind"].as_str().unwrap_or_default().to_owned(),
                operation["store"].as_str().map(str::to_owned),
                operation["identity"].as_str().map(str::to_owned),
            )
        })
        .collect::<Vec<_>>();
    // Every transaction boundary and every named statement is auditable, and a
    // handle-taking operation never invents a database name it cannot prove.
    assert_eq!(
        operations,
        [
            (
                "database-begin-write".to_owned(),
                Some("catalog".to_owned()),
                None
            ),
            (
                "database-execute".to_owned(),
                None,
                Some("record-visit".to_owned())
            ),
            (
                "database-query".to_owned(),
                None,
                Some("count-visits".to_owned())
            ),
            ("database-commit".to_owned(), None, None),
            ("database-rollback".to_owned(), None, None),
            ("database-rollback".to_owned(), None, None),
        ]
    );
    let rendered = serde_json::to_string(&explain).expect("explanation should serialise");
    assert!(
        !rendered.contains("catalog.db") && !rendered.contains("INSERT"),
        "explanations must never disclose host paths or SQL: {rendered}"
    );

    let first = invoke_database(&directory, "host.json", "request-one.json");
    assert!(
        first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );
    let first: serde_json::Value =
        serde_json::from_slice(&first.stdout).expect("response should be JSON");
    assert_eq!(first["status"], 200);
    assert_eq!(first["body"], "{\"columns\":[\"total\"],\"rows\":[[1]]}");

    // A separate process observes the committed row, proving durability.
    let second = invoke_database(&directory, "host.json", "request-two.json");
    assert!(
        second.status.success(),
        "{}",
        String::from_utf8_lossy(&second.stderr)
    );
    let second: serde_json::Value =
        serde_json::from_slice(&second.stdout).expect("response should be JSON");
    assert_eq!(second["body"], "{\"columns\":[\"total\"],\"rows\":[[2]]}");

    let connection = rusqlite::Connection::open(&database).expect("fixture should reopen");
    let stored: String = connection
        .query_row(
            "SELECT path FROM visits ORDER BY id DESC LIMIT 1",
            [],
            |row| row.get(0),
        )
        .expect("the injection payload should be stored as literal data");
    assert_eq!(stored, "x'); DROP TABLE visits; --");
    let total: i64 = connection
        .query_row("SELECT COUNT(*) FROM visits", [], |row| row.get(0))
        .expect("the table must still exist");
    assert_eq!(total, 2);
}

#[test]
fn schema_five_rejects_unsafe_database_configuration_without_touching_the_file() {
    let directory = TestDirectory::new("schema-five-database-rejects");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        fs::set_permissions(&directory.path, fs::Permissions::from_mode(0o700))
            .expect("test root should be owner-only");
    }
    let database = create_database_fixture(&directory);
    database_package(&directory, "databases");
    directory.file(
        "request.json",
        r#"{"method":"POST","path":"/","query":"","headers":[],"body":"first"}"#,
    );
    let build = krit_in(&directory)
        .arg("build")
        .output()
        .expect("database package should build");
    assert!(build.status.success());

    let before = fs::read(&database).expect("fixture should be readable");
    let mut rejected = vec![
        // The manifest grants `database.write`, never read-only authority.
        (
            "read-only.host.json",
            database_host_config("data/catalog.db", "read-only", DATABASE_STATEMENTS),
        ),
        // Krit never creates or migrates an application database.
        (
            "missing.host.json",
            database_host_config("data/absent.db", "read-write", DATABASE_STATEMENTS),
        ),
        // Host paths may not escape the host configuration directory.
        (
            "escape.host.json",
            database_host_config("../catalog.db", "read-write", DATABASE_STATEMENTS),
        ),
        // Transaction control is host-owned and cannot be smuggled in.
        (
            "forbidden.host.json",
            database_host_config(
                "data/catalog.db",
                "read-write",
                r#"
      "statements": {
        "record-visit": {
          "kind": "execute",
          "sql": "BEGIN IMMEDIATE",
          "parameters": [],
          "columns": []
        }
      }"#,
            ),
        ),
        // Exactly one statement per catalog entry.
        (
            "multiple.host.json",
            database_host_config(
                "data/catalog.db",
                "read-write",
                r#"
      "statements": {
        "record-visit": {
          "kind": "execute",
          "sql": "INSERT INTO visits(path) VALUES(?1); DELETE FROM visits",
          "parameters": ["text"],
          "columns": []
        }
      }"#,
            ),
        ),
        // The declared placeholder count must match the SQL exactly.
        (
            "placeholders.host.json",
            database_host_config(
                "data/catalog.db",
                "read-write",
                r#"
      "statements": {
        "record-visit": {
          "kind": "execute",
          "sql": "INSERT INTO visits(path) VALUES(?1)",
          "parameters": ["text", "text"],
          "columns": []
        }
      }"#,
            ),
        ),
        // A mutation may not be declared as a read-only query.
        (
            "kind.host.json",
            database_host_config(
                "data/catalog.db",
                "read-write",
                r#"
      "statements": {
        "record-visit": {
          "kind": "query",
          "sql": "INSERT INTO visits(path) VALUES(?1)",
          "parameters": ["text"],
          "columns": []
        }
      }"#,
            ),
        ),
        // Unknown configuration fields are rejected rather than ignored.
        (
            "unknown.host.json",
            database_host_config("data/catalog.db", "read-write", DATABASE_STATEMENTS).replace(
                "\"schema\": 5,",
                "\"schema\": 5,\n  \"driver\": \"postgres\",",
            ),
        ),
    ];
    #[cfg(unix)]
    {
        // A symbolic link must never be followed into another file.
        std::os::unix::fs::symlink(&database, directory.path.join("data/link.db"))
            .expect("symlink fixture should be created");
        rejected.push((
            "symlink.host.json",
            database_host_config("data/link.db", "read-write", DATABASE_STATEMENTS),
        ));
    }

    for (name, config) in rejected {
        directory.file(name, &config);
        let output = invoke_database(&directory, name, "request.json");
        assert!(
            !output.status.success(),
            "`{name}` must be rejected: {}",
            String::from_utf8_lossy(&output.stdout)
        );
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            !stderr.contains("INSERT INTO visits"),
            "`{name}` must not echo catalog SQL: {stderr}"
        );
        assert_eq!(
            fs::read(&database).expect("fixture should be readable"),
            before,
            "`{name}` must not modify the application database"
        );
    }
    assert!(!directory.path.join("data/absent.db").exists());
}

#[test]
fn database_free_programs_still_run_under_schema_five_configuration() {
    let directory = TestDirectory::new("schema-five-no-database");
    directory.file(
        "main.krit",
        r#"
webhook fn handle(request: HttpRequest) -> HttpResponse {
    record { status: 200, headers: [], body: request.body }
}
"#,
    );
    directory.file(
        "krit.pkg",
        r#"
schema = 1

[package]
name = "example/plain"
version = "0.2.0"
edition = "2026"
entry = "main.krit"
license = "Apache-2.0"
"#,
    );
    directory.file(
        "host.json",
        r#"{
  "schema": 5,
  "state": { "stores": {} }
}"#,
    );
    directory.file(
        "request.json",
        r#"{"method":"POST","path":"/","query":"","headers":[],"body":"plain"}"#,
    );

    let build = krit_in(&directory)
        .arg("build")
        .output()
        .expect("plain package should build");
    assert!(build.status.success());
    let output = invoke_database(&directory, "host.json", "request.json");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let response: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("response should be JSON");
    assert_eq!(response["body"], "plain");
}

/// Builds a schema-5 config with both a durable store and an application
/// database, so ordering and aliasing rules can be exercised together.
#[cfg(unix)]
fn state_and_database_config(database_path: &str, extra: &str) -> String {
    format!(
        r#"{{
  "schema": 5,
  "state": {{
    "stores": {{
      "agent-work": {{
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
      }}
    }}
  }},
  "maxTransactionsPerInvocation": 1,{extra}
  "databases": {{
    "catalog": {{
      "path": "{database_path}",
      "mode": "read-write",
      "busyTimeoutMs": 250,
      "maxDatabaseBytes": 16777216,
      "maxTransactionMillis": 500,
      "maxOperationsPerTransaction": 16,
      "maxParameterBytes": 4096,
      "maxRows": 64,
      "maxColumns": 8,
      "maxResultBytes": 65536,{DATABASE_STATEMENTS}
    }}
  }}
}}"#
    )
}

#[cfg(unix)]
#[test]
fn schema_five_validates_every_policy_before_creating_any_durable_store() {
    use std::os::unix::fs::{DirBuilderExt, PermissionsExt};

    let directory = TestDirectory::new("schema-five-ordering");
    fs::set_permissions(&directory.path, fs::Permissions::from_mode(0o700))
        .expect("test root should be owner-only");
    create_database_fixture(&directory);
    fs::DirBuilder::new()
        .mode(0o700)
        .create(directory.path.join("state"))
        .expect("state directory should be owner-only");
    directory.file(
        "main.krit",
        r#"
webhook fn handle(request: HttpRequest) -> HttpResponse {
    match db_begin_write("catalog") {
        Ok(transaction) => match db_execute(transaction, "record-visit", [request.body]) {
            Ok(changed) => match db_commit(transaction) {
                Ok(committed) => record { status: 200, headers: [], body: "ok" },
                Err(error) => record { status: 500, headers: [], body: error },
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
    );
    directory.file(
        "krit.pkg",
        r#"
schema = 1

[package]
name = "example/ordering"
version = "0.2.0"
edition = "2026"
entry = "main.krit"
license = "Apache-2.0"

[capabilities]
databases = ["catalog"]
state = ["agent-work"]
secrets = ["token"]
"#,
    );
    directory.file(
        "request.json",
        r#"{"method":"POST","path":"/","query":"","headers":[],"body":"one"}"#,
    );
    let build = krit_in(&directory)
        .arg("build")
        .output()
        .expect("package should build");
    assert!(
        build.status.success(),
        "{}",
        String::from_utf8_lossy(&build.stderr)
    );

    let state_database = directory.path.join("state/agent-work.db");
    // Every rejection below is a *later* validation step than the durable store
    // creation used to be, so each one proves the reordering.
    let rejected = [
        // Invalid database limit.
        (
            "bad-limit.host.json",
            state_and_database_config("data/catalog.db", "").replace(
                "\"maxResultBytes\": 65536,",
                "\"maxResultBytes\": 999999999,",
            ),
        ),
        // Statement that does not match the live application schema.
        (
            "bad-statement.host.json",
            state_and_database_config("data/catalog.db", "").replace(
                "INSERT INTO visits(path) VALUES(?1)",
                "INSERT INTO absent_table(path) VALUES(?1)",
            ),
        ),
        // Application database the manifest never granted.
        (
            "ungranted.host.json",
            state_and_database_config("data/catalog.db", "")
                .replace("\"catalog\": {", "\"other\": {"),
        ),
        // Invalid rate-limit policy, validated after every database rule.
        (
            "bad-rate.host.json",
            state_and_database_config(
                "data/catalog.db",
                "\n  \"rateLimits\": { \"maxTrackedResources\": 0 },",
            ),
        ),
        // Secret file that does not exist.
        (
            "bad-secret.host.json",
            state_and_database_config(
                "data/catalog.db",
                "\n  \"secrets\": { \"token\": { \"file\": \"absent.secret\" } },",
            ),
        ),
    ];

    for (name, config) in rejected {
        directory.file(name, &config);
        let output = invoke_database(&directory, name, "request.json");
        assert!(
            !output.status.success(),
            "`{name}` must be rejected: {}",
            String::from_utf8_lossy(&output.stdout)
        );
        assert!(
            !state_database.exists(),
            "`{name}` must not create the durable state store: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    // A valid configuration still creates the store and runs.
    directory.file(
        "valid.host.json",
        &state_and_database_config("data/catalog.db", ""),
    );
    let output = invoke_database(&directory, "valid.host.json", "request.json");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(state_database.is_file());

    // The state store now exists at schema 2; a later invalid configuration
    // must leave its bytes untouched.
    let before = fs::read(&state_database).expect("state store should be readable");
    directory.file(
        "late-invalid.host.json",
        &state_and_database_config(
            "data/catalog.db",
            "\n  \"rateLimits\": { \"maxTrackedResources\": 0 },",
        ),
    );
    let output = invoke_database(&directory, "late-invalid.host.json", "request.json");
    assert!(!output.status.success());
    assert_eq!(
        fs::read(&state_database).expect("state store should be readable"),
        before,
        "a rejected configuration must not migrate or rewrite the state store"
    );
}

#[cfg(unix)]
#[test]
fn schema_five_rejects_state_and_application_database_aliases() {
    let directory = TestDirectory::new("schema-five-alias");
    use std::os::unix::fs::{DirBuilderExt, PermissionsExt};

    fs::set_permissions(&directory.path, fs::Permissions::from_mode(0o700))
        .expect("test root should be owner-only");
    create_database_fixture(&directory);
    fs::DirBuilder::new()
        .mode(0o700)
        .create(directory.path.join("state"))
        .expect("state directory should be owner-only");
    database_package(&directory, "databases");
    directory.file(
        "krit.pkg",
        r#"
schema = 1

[package]
name = "example/alias"
version = "0.2.0"
edition = "2026"
entry = "main.krit"
license = "Apache-2.0"

[capabilities]
databases = ["catalog"]
state = ["agent-work"]
"#,
    );
    directory.file(
        "request.json",
        r#"{"method":"POST","path":"/","query":"","headers":[],"body":"one"}"#,
    );
    let build = krit_in(&directory)
        .arg("build")
        .output()
        .expect("package should build");
    assert!(build.status.success());

    // Create the state store so the aliasing checks have a real file to match.
    directory.file(
        "seed.host.json",
        &state_and_database_config("data/catalog.db", ""),
    );
    let seed = invoke_database(&directory, "seed.host.json", "request.json");
    assert!(
        seed.status.success(),
        "{}",
        String::from_utf8_lossy(&seed.stderr)
    );
    let state_database = directory.path.join("state/agent-work.db");
    assert!(state_database.is_file());
    let before = fs::read(&state_database).expect("state store should be readable");

    fs::hard_link(&state_database, directory.path.join("state/linked.db"))
        .expect("hard link fixture should be created");

    for (name, database_path) in [
        // The application database is literally Krit's own internal store.
        ("same.host.json", "state/agent-work.db"),
        // The same file reached through a `.` alias.
        ("dot.host.json", "state/./agent-work.db"),
        // The same inode reached through a hard link under another name.
        ("link.host.json", "state/linked.db"),
    ] {
        directory.file(name, &state_and_database_config(database_path, ""));
        let output = invoke_database(&directory, name, "request.json");
        assert!(
            !output.status.success(),
            "`{name}` must not expose Krit's internal schema to `database.write`: {}",
            String::from_utf8_lossy(&output.stdout)
        );
        assert_eq!(
            fs::read(&state_database).expect("state store should be readable"),
            before,
            "`{name}` must not touch the durable state store"
        );
    }
}

const CACHE_SEARCH_SOURCE: &str = r#"
fn lookup(query: String, reason: String) -> HttpResponse {
    match search_query("docs", query, 3) {
        Ok(results) => match cache_put("lookups", query, results, 60) {
            Ok(stored) => record {
                status: 200,
                headers: [record { name: "x-cache", value: reason }],
                body: results,
            },
            Err(problem) => record {
                status: 200,
                headers: [record { name: "x-cache", value: "uncached" }],
                body: results,
            },
        },
        Err(problem) => record {
            status: 503,
            headers: [record { name: "x-cache", value: reason }],
            body: problem,
        },
    }
}

webhook fn handle(request: HttpRequest) -> HttpResponse {
    match cache_get("lookups", request.query) {
        Ok(found) => match found {
            Some(value) => record {
                status: 200,
                headers: [record { name: "x-cache", value: "hit" }],
                body: value,
            },
            None => lookup(request.query, "miss"),
        },
        Err(outage) => lookup(request.query, "unavailable"),
    }
}
"#;

const CACHE_SEARCH_MANIFEST: &str = r#"
schema = 1

[package]
name = "example/cached-search"
version = "0.2.0"
edition = "2026"
entry = "main.krit"
license = "Apache-2.0"

[capabilities]
cacheNamespaces = ["lookups"]
searchIndexes = ["docs"]
"#;

/// A schema-6 configuration with a bounded cache and a deterministic connector.
fn cache_search_config(cache: &str, search: &str) -> String {
    format!(
        r#"{{
  "schema": 6,
  "state": {{ "stores": {{}} }},
  "cache": {cache},
  "search": {search}
}}"#
    )
}

const VALID_CACHE: &str = r#"{
    "namespaces": {
      "lookups": {
        "mode": "read-write",
        "maxEntries": 16,
        "maxBytes": 262144,
        "maxKeyBytes": 256,
        "maxValueBytes": 8192,
        "maxTtlSeconds": 300
      }
    },
    "maxTotalEntries": 32,
    "maxTotalBytes": 524288
  }"#;

const VALID_SEARCH: &str = r#"{
    "docs": {
      "kind": "query",
      "maxResults": 5,
      "transport": {
        "type": "local",
        "documents": [
          { "id": "cache", "text": "The Krit cache is bounded and never load bearing." },
          { "id": "search", "text": "Search connectors are provider neutral." }
        ]
      }
    }
  }"#;

fn cache_search_package(directory: &TestDirectory) {
    directory.file("main.krit", CACHE_SEARCH_SOURCE);
    directory.file("krit.pkg", CACHE_SEARCH_MANIFEST);
    directory.file(
        "request.json",
        r#"{"method":"GET","path":"/","query":"cache","headers":[],"body":""}"#,
    );
}

#[test]
fn schema_six_cache_and_search_report_exact_permissions_and_privacy() {
    let directory = TestDirectory::new("schema-six-cache");
    cache_search_package(&directory);
    directory.file("host.json", &cache_search_config(VALID_CACHE, VALID_SEARCH));

    let build = krit_in(&directory)
        .arg("build")
        .output()
        .expect("package should build");
    assert!(
        build.status.success(),
        "{}",
        String::from_utf8_lossy(&build.stderr)
    );
    let artifact = directory.path.join("target/krit/cached-search.wasm");
    assert_valid_artifact(&artifact);

    let permissions = krit_in(&directory)
        .args(["permissions", "--json", "--artifact"])
        .arg(&artifact)
        .output()
        .expect("permissions should run");
    assert!(permissions.status.success());
    let permissions: serde_json::Value =
        serde_json::from_slice(&permissions.stdout).expect("permissions should be JSON");
    assert_eq!(
        permissions["required"],
        serde_json::json!([
            { "capability": "cache.read", "resource": "lookups" },
            { "capability": "cache.write", "resource": "lookups" },
            { "capability": "search.query", "resource": "docs" }
        ])
    );
    assert_eq!(
        permissions["imports"],
        serde_json::json!([
            "krit:runtime/cache-read@0.2.0",
            "krit:runtime/cache-write@0.2.0",
            "krit:runtime/search-query@0.2.0"
        ])
    );
    let rendered = serde_json::to_string(&permissions).expect("permissions should serialise");
    assert!(
        !rendered.contains("load bearing") && !rendered.contains("provider neutral"),
        "permissions must never disclose connector documents: {rendered}"
    );

    let explain = krit_in(&directory)
        .args(["explain", "--json", "main.krit"])
        .output()
        .expect("explain should run");
    assert!(explain.status.success());
    let explain: serde_json::Value =
        serde_json::from_slice(&explain.stdout).expect("explain should be JSON");
    let operations = explain["durableState"]["operations"]
        .as_array()
        .expect("operations should be an array")
        .iter()
        .map(|operation| operation["kind"].as_str().unwrap_or_default().to_owned())
        .collect::<Vec<_>>();
    assert!(operations.contains(&"cache-get".to_owned()));
    assert!(operations.contains(&"cache-put".to_owned()));
    assert!(operations.contains(&"search-query".to_owned()));
}

#[test]
fn schema_six_invocations_are_correct_with_a_process_local_cache() {
    let directory = TestDirectory::new("schema-six-invoke");
    cache_search_package(&directory);
    directory.file("host.json", &cache_search_config(VALID_CACHE, VALID_SEARCH));
    let build = krit_in(&directory)
        .arg("build")
        .output()
        .expect("package should build");
    assert!(build.status.success());

    // Each `invoke` is a separate process, so the process-local cache starts
    // empty every time. The answer is identical either way, which is the whole
    // point: correctness never depends on the cache.
    let mut bodies = Vec::new();
    for _ in 0..2 {
        let output = krit_in(&directory)
            .args([
                "invoke",
                "--host-config",
                "host.json",
                "--request",
                "request.json",
            ])
            .output()
            .expect("invocation should run");
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let response: serde_json::Value =
            serde_json::from_slice(&output.stdout).expect("response should be JSON");
        assert_eq!(response["status"], 200);
        assert_eq!(
            response["headers"][0]["value"], "miss",
            "a fresh process must always miss"
        );
        bodies.push(response["body"].as_str().unwrap_or_default().to_owned());
    }
    assert_eq!(
        bodies[0], bodies[1],
        "the answer must not depend on the cache"
    );
    assert!(bodies[0].starts_with("{\"results\":["));
}

#[test]
fn schema_six_runs_the_same_artifact_with_the_cache_disabled() {
    let directory = TestDirectory::new("schema-six-disabled");
    cache_search_package(&directory);
    // No cache section at all: the deployment simply has no cache.
    directory.file("host.json", &cache_search_config("{}", VALID_SEARCH));
    let build = krit_in(&directory)
        .arg("build")
        .output()
        .expect("package should build");
    assert!(build.status.success());

    let output = krit_in(&directory)
        .args([
            "invoke",
            "--host-config",
            "host.json",
            "--request",
            "request.json",
        ])
        .output()
        .expect("invocation should run");

    assert!(
        output.status.success(),
        "a disabled cache must not stop the program: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let response: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("response should be JSON");
    assert_eq!(response["status"], 200);
    // The read outage sent the guest down its fallback path, and the write
    // outage was handled there too, so the response reports `uncached`. Both
    // failures were explicit values; neither changed the answer.
    assert_eq!(
        response["headers"][0]["value"], "uncached",
        "the guest must observe both cache outages and handle each explicitly"
    );
    assert!(
        response["body"]
            .as_str()
            .unwrap_or_default()
            .starts_with("{\"results\":["),
        "the fallback answer must still be correct"
    );

    // The identical artifact, with the cache configured, produces the identical
    // answer: correctness cannot depend on cache availability.
    directory.file(
        "enabled.host.json",
        &cache_search_config(VALID_CACHE, VALID_SEARCH),
    );
    let enabled = krit_in(&directory)
        .args([
            "invoke",
            "--host-config",
            "enabled.host.json",
            "--request",
            "request.json",
        ])
        .output()
        .expect("invocation should run");
    assert!(enabled.status.success());
    let enabled: serde_json::Value =
        serde_json::from_slice(&enabled.stdout).expect("response should be JSON");
    assert_eq!(enabled["body"], response["body"]);
    assert_eq!(enabled["headers"][0]["value"], "miss");
}

#[test]
fn schema_six_rejects_invalid_configuration_without_side_effects() {
    let directory = TestDirectory::new("schema-six-rejects");
    cache_search_package(&directory);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        fs::set_permissions(&directory.path, fs::Permissions::from_mode(0o700))
            .expect("test root should be owner-only");
    }
    let build = krit_in(&directory)
        .arg("build")
        .output()
        .expect("package should build");
    assert!(build.status.success());

    let state_store = directory.path.join("state/agent-work.db");
    let rejected = [
        // A namespace the manifest never granted.
        (
            "ungranted.host.json",
            cache_search_config(&VALID_CACHE.replace("lookups", "other"), VALID_SEARCH),
        ),
        // A namespace budget that cannot hold one maximum-size entry.
        (
            "unusable.host.json",
            cache_search_config(
                &VALID_CACHE.replace("\"maxBytes\": 262144", "\"maxBytes\": 64"),
                VALID_SEARCH,
            ),
        ),
        // A time to live beyond the Phase 7 ceiling.
        (
            "ttl.host.json",
            cache_search_config(
                &VALID_CACHE.replace("\"maxTtlSeconds\": 300", "\"maxTtlSeconds\": 99999999"),
                VALID_SEARCH,
            ),
        ),
        // Cache totals declared with no namespace at all.
        (
            "orphan-totals.host.json",
            cache_search_config(
                "{ \"maxTotalEntries\": 32, \"maxTotalBytes\": 524288 }",
                VALID_SEARCH,
            ),
        ),
        // A connector the manifest never granted.
        (
            "ungranted-search.host.json",
            cache_search_config(VALID_CACHE, &VALID_SEARCH.replace("\"docs\"", "\"other\"")),
        ),
        // A text connector that wrongly declares vector dimensions.
        (
            "dimensions.host.json",
            cache_search_config(
                VALID_CACHE,
                &VALID_SEARCH.replace(
                    "\"kind\": \"query\",",
                    "\"kind\": \"query\",\n      \"dimensions\": 8,",
                ),
            ),
        ),
        // A result bound beyond the Phase 7 ceiling.
        (
            "results.host.json",
            cache_search_config(
                VALID_CACHE,
                &VALID_SEARCH.replace("\"maxResults\": 5", "\"maxResults\": 9999"),
            ),
        ),
        // An unknown field is refused rather than ignored.
        (
            "unknown.host.json",
            cache_search_config(VALID_CACHE, VALID_SEARCH).replace(
                "\"schema\": 6,",
                "\"schema\": 6,\n  \"provider\": \"acme\",",
            ),
        ),
        // An http-json connector reaching an origin the manifest never granted.
        (
            "ungranted-origin.host.json",
            cache_search_config(
                VALID_CACHE,
                r#"{
    "docs": {
      "kind": "query",
      "maxResults": 5,
      "transport": {
        "type": "http-json",
        "origin": "https://search.example",
        "path": "/query",
        "maxResponseBytes": 4096,
        "timeoutMs": 1000
      }
    }
  }"#,
            ),
        ),
    ];

    for (name, config) in rejected {
        directory.file(name, &config);
        let output = krit_in(&directory)
            .args(["invoke", "--host-config", name, "--request", "request.json"])
            .output()
            .expect("invocation should run");
        assert!(
            !output.status.success(),
            "`{name}` must be rejected: {}",
            String::from_utf8_lossy(&output.stdout)
        );
        assert!(
            !state_store.exists(),
            "`{name}` must not create durable state"
        );
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            !stderr.contains("load bearing"),
            "`{name}` must not echo connector documents: {stderr}"
        );
    }
}

#[test]
fn schemas_one_through_five_remain_accepted_alongside_schema_six() {
    let directory = TestDirectory::new("schema-six-compat");
    directory.file(
        "main.krit",
        r#"
webhook fn handle(request: HttpRequest) -> HttpResponse {
    record { status: 200, headers: [], body: request.body }
}
"#,
    );
    directory.file(
        "krit.pkg",
        r#"
schema = 1

[package]
name = "example/plain"
version = "0.2.0"
edition = "2026"
entry = "main.krit"
license = "Apache-2.0"
"#,
    );
    directory.file(
        "request.json",
        r#"{"method":"POST","path":"/","query":"","headers":[],"body":"plain"}"#,
    );
    let build = krit_in(&directory)
        .arg("build")
        .output()
        .expect("package should build");
    assert!(build.status.success());

    for (name, config) in [
        ("v1.host.json", r#"{"schema": 1}"#.to_owned()),
        ("v2.host.json", r#"{"schema": 2}"#.to_owned()),
        (
            "v3.host.json",
            r#"{"schema": 3, "state": { "stores": {} }}"#.to_owned(),
        ),
        (
            "v4.host.json",
            r#"{"schema": 4, "state": { "stores": {} }}"#.to_owned(),
        ),
        (
            "v5.host.json",
            r#"{"schema": 5, "state": { "stores": {} }}"#.to_owned(),
        ),
        (
            "v6.host.json",
            r#"{"schema": 6, "state": { "stores": {} }}"#.to_owned(),
        ),
    ] {
        directory.file(name, &config);
        let output = krit_in(&directory)
            .args(["invoke", "--host-config", name, "--request", "request.json"])
            .output()
            .expect("invocation should run");
        assert!(
            output.status.success(),
            "`{name}` should still load: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let response: serde_json::Value =
            serde_json::from_slice(&output.stdout).expect("response should be JSON");
        assert_eq!(response["body"], "plain");
    }

    // An unknown schema is still refused, and the message lists exactly the
    // supported set.
    directory.file("v7.host.json", r#"{"schema": 7}"#);
    let output = krit_in(&directory)
        .args([
            "invoke",
            "--host-config",
            "v7.host.json",
            "--request",
            "request.json",
        ])
        .output()
        .expect("invocation should run");
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("1, 2, 3, 4, 5, or 6"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn a_served_host_serves_a_hit_from_the_shared_cache() {
    let directory = TestDirectory::new("schema-six-serve");
    cache_search_package(&directory);
    directory.file("host.json", &cache_search_config(VALID_CACHE, VALID_SEARCH));
    let build = krit_in(&directory)
        .arg("build")
        .output()
        .expect("package should build");
    assert!(build.status.success());

    let mut child = krit_in(&directory)
        .args([
            "serve",
            "--host-config",
            "host.json",
            "--bind",
            "127.0.0.1:0",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("serve should start");
    let stderr = child.stderr.take().expect("server stderr should be piped");
    let mut stderr = BufReader::new(stderr);
    let mut listening = String::new();
    stderr
        .read_line(&mut listening)
        .expect("listening line should read");
    let address = listening
        .trim()
        .strip_prefix("krit serve listening on http://")
        .expect("listening prefix should exist")
        .to_owned();

    // One host process, one shared cache, a fresh Wasm store per request.
    let first = serve_get(&address, "/lookup?cache");
    let second = serve_get(&address, "/lookup?cache");
    let third = serve_get(&address, "/lookup?search");

    let _ = child.kill();
    let _ = child.wait();

    assert!(first.contains("x-cache: miss"), "first response: {first}");
    assert!(
        second.contains("x-cache: hit"),
        "a shared host must serve a hit: {second}"
    );
    assert!(third.contains("x-cache: miss"), "third response: {third}");
}

/// Sends one bounded GET over a fresh connection and returns the response text.
fn serve_get(address: &str, target: &str) -> String {
    let mut stream = TcpStream::connect(address).expect("server should accept connections");
    stream
        .write_all(
            format!("GET {target} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
                .as_bytes(),
        )
        .expect("request should write");
    let mut response = Vec::new();
    stream
        .read_to_end(&mut response)
        .expect("response should read");
    String::from_utf8_lossy(&response).into_owned()
}

#[test]
fn schema_six_rejects_duplicate_cache_and_search_keys_without_side_effects() {
    let directory = TestDirectory::new("schema-six-duplicates");
    cache_search_package(&directory);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        fs::set_permissions(&directory.path, fs::Permissions::from_mode(0o700))
            .expect("test root should be owner-only");
    }
    let build = krit_in(&directory)
        .arg("build")
        .output()
        .expect("package should build");
    assert!(build.status.success());
    let state_store = directory.path.join("state/agent-work.db");

    // A duplicate namespace with conflicting bounds: keeping the last value
    // would silently discard the stricter definition.
    let duplicate_namespace = r#"{
    "namespaces": {
      "lookups": {
        "mode": "read-only",
        "maxEntries": 1,
        "maxBytes": 262144,
        "maxKeyBytes": 256,
        "maxValueBytes": 8192,
        "maxTtlSeconds": 5
      },
      "lookups": {
        "mode": "read-write",
        "maxEntries": 16,
        "maxBytes": 262144,
        "maxKeyBytes": 256,
        "maxValueBytes": 8192,
        "maxTtlSeconds": 300
      }
    },
    "maxTotalEntries": 32,
    "maxTotalBytes": 524288
  }"#;
    // A duplicate connector with a conflicting endpoint and credential policy.
    let duplicate_connector = r#"{
    "docs": {
      "kind": "query",
      "maxResults": 5,
      "transport": {
        "type": "http-json",
        "origin": "https://first.example",
        "path": "/query",
        "secret": "search-token",
        "maxResponseBytes": 4096,
        "timeoutMs": 1000
      }
    },
    "docs": {
      "kind": "query",
      "maxResults": 5,
      "transport": {
        "type": "local",
        "documents": []
      }
    }
  }"#;

    for (name, config) in [
        (
            "duplicate-namespace.host.json",
            cache_search_config(duplicate_namespace, VALID_SEARCH),
        ),
        (
            "duplicate-connector.host.json",
            cache_search_config(VALID_CACHE, duplicate_connector),
        ),
    ] {
        directory.file(name, &config);
        let output = krit_in(&directory)
            .args(["invoke", "--host-config", name, "--request", "request.json"])
            .output()
            .expect("invocation should run");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            !output.status.success(),
            "`{name}` must be rejected: {}",
            String::from_utf8_lossy(&output.stdout)
        );
        assert!(
            stderr.contains("duplicate key"),
            "`{name}` must name the duplicate: {stderr}"
        );
        assert!(
            !state_store.exists(),
            "`{name}` must not create durable state"
        );
    }
}

#[test]
fn schema_six_requires_https_for_an_http_json_connector() {
    let directory = TestDirectory::new("schema-six-https");
    cache_search_package(&directory);
    directory.file(
        "krit.pkg",
        r#"
schema = 1

[package]
name = "example/cached-search"
version = "0.2.0"
edition = "2026"
entry = "main.krit"
license = "Apache-2.0"

[capabilities]
cacheNamespaces = ["lookups"]
searchIndexes = ["docs"]
http = ["http://search.example", "https://search.example"]
"#,
    );
    let build = krit_in(&directory)
        .arg("build")
        .output()
        .expect("package should build");
    assert!(build.status.success());

    for (name, origin, path, accepted) in [
        (
            "plaintext.host.json",
            "http://search.example",
            "/query",
            false,
        ),
        ("secure.host.json", "https://search.example", "/query", true),
        // The path is held to the same origin-form rule as any host-owned path.
        (
            "authority.host.json",
            "https://search.example",
            "//evil.example/query",
            false,
        ),
        (
            "query.host.json",
            "https://search.example",
            "/query?inject=1",
            false,
        ),
        (
            "fragment.host.json",
            "https://search.example",
            "/query#f",
            false,
        ),
        (
            "relative.host.json",
            "https://search.example",
            "query",
            false,
        ),
        (
            "dot-segment.host.json",
            "https://search.example",
            "/v1/../admin",
            false,
        ),
        (
            "encoded-slash.host.json",
            "https://search.example",
            "/v1%2Fadmin",
            false,
        ),
        (
            "path-parameter.host.json",
            "https://search.example",
            "/v1/..;/admin",
            false,
        ),
        (
            "escaped.host.json",
            "https://search.example",
            "/v1.2/search%20all",
            true,
        ),
    ] {
        let search = format!(
            r#"{{
    "docs": {{
      "kind": "query",
      "maxResults": 5,
      "transport": {{
        "type": "http-json",
        "origin": "{origin}",
        "path": "{path}",
        "maxResponseBytes": 4096,
        "timeoutMs": 1000
      }}
    }}
  }}"#
        );
        directory.file(name, &cache_search_config(VALID_CACHE, &search));
        let output = krit_in(&directory)
            .args(["invoke", "--host-config", name, "--request", "request.json"])
            .output()
            .expect("invocation should run");
        if accepted {
            // A well-formed connector loads; the call itself then fails because
            // the origin is unreachable, which is a handled guest value.
            assert!(
                output.status.success(),
                "`{name}` should load: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        } else {
            assert!(
                !output.status.success(),
                "`{name}` must be rejected: {}",
                String::from_utf8_lossy(&output.stdout)
            );
        }
    }
}

/// The reference webhook-agent manifest with every secret bound to its origin,
/// plus one legacy unbound secret so both report forms are exercised.
const BOUND_SECRETS_MANIFEST: &str = r#"
schema = 1

[package]
name = "test/bound-secrets"
version = "1.0.0"
edition = "2026"
entry = "main.krit"
license = "Apache-2.0"

[capabilities]
http = [
    "https://ai.example.invalid",
    "https://github.example.invalid",
    "https://message.example.invalid",
]
secrets = ["legacy-token"]

[secrets.github-token]
origins = ["https://github.example.invalid"]

[secrets.message-token]
origins = ["https://message.example.invalid"]
inject = "bearer"
"#;

/// A webhook that presents `secret` as the bearer credential of `origin`.
fn bearer_source(secret: &str, origin: &str) -> String {
    format!(
        r#"
webhook fn handle(request: HttpRequest) -> HttpResponse {{
    match secret("{secret}") {{
        Ok(token) => match http_request("{origin}", request, Some(token)) {{
            Ok(response) => response,
            Err(error) => record {{ status: 502, headers: [], body: error }},
        }},
        Err(error) => record {{ status: 500, headers: [], body: error }},
    }}
}}
"#
    )
}

#[test]
fn permissions_report_secret_bindings_and_label_legacy_secrets_unbound() {
    let directory = TestDirectory::new("permissions-secret-bindings");
    directory.file(
        "main.krit",
        &bearer_source("github-token", "https://github.example.invalid"),
    );
    let manifest = directory.file("krit.pkg", BOUND_SECRETS_MANIFEST);

    let human = krit_in(&directory)
        .args(["permissions"])
        .arg(&manifest)
        .output()
        .expect("Krit should start");
    assert!(human.status.success());
    assert!(human.stderr.is_empty());
    assert_eq!(
        String::from_utf8(human.stdout).expect("report should be UTF-8"),
        "Requested capabilities for test/bound-secrets:\n  \
         http.request: https://ai.example.invalid\n  \
         http.request: https://github.example.invalid\n  \
         http.request: https://message.example.invalid\n  \
         secret.read: github-token\n  \
         secret.read: legacy-token\n  \
         secret.read: message-token\n\
         Secret bindings:\n  \
         github-token (bearer): https://github.example.invalid\n  \
         legacy-token (bearer): UNBOUND (any granted origin)\n  \
         message-token (bearer): https://message.example.invalid\n\
         Deployment grants: not evaluated\n"
    );

    let json = krit_in(&directory)
        .args(["permissions", "--json"])
        .arg(&manifest)
        .output()
        .expect("Krit should start");
    assert!(json.status.success());
    let report: serde_json::Value =
        serde_json::from_slice(&json.stdout).expect("report should be JSON");
    assert_eq!(
        report["secretBindings"],
        serde_json::json!([
            {"secret": "github-token", "inject": "bearer", "binding": "bound",
             "origins": ["https://github.example.invalid"]},
            {"secret": "legacy-token", "inject": "bearer", "binding": "unbound", "origins": []},
            {"secret": "message-token", "inject": "bearer", "binding": "bound",
             "origins": ["https://message.example.invalid"]},
        ])
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
    let artifact = directory.path.join("target/krit/bound-secrets.wasm");
    let human = krit_in(&directory)
        .args(["permissions", "--artifact"])
        .arg(&artifact)
        .arg(&manifest)
        .output()
        .expect("Krit should start");
    assert!(human.status.success());
    let human = String::from_utf8(human.stdout).expect("report should be UTF-8");
    assert!(human.contains(
        "Secret bindings:\n  \
         github-token (bearer): https://github.example.invalid\n  \
         legacy-token (bearer): UNBOUND (any granted origin)\n"
    ));
    let json = krit_in(&directory)
        .args(["permissions", "--json", "--artifact"])
        .arg(&artifact)
        .arg(&manifest)
        .output()
        .expect("Krit should start");
    assert!(json.status.success());
    let artifact_report: serde_json::Value =
        serde_json::from_slice(&json.stdout).expect("report should be JSON");
    assert_eq!(artifact_report["secretBindings"], report["secretBindings"]);
}

#[test]
fn invalid_secret_bindings_are_manifest_errors() {
    let directory = TestDirectory::new("invalid-secret-bindings");
    directory.file("main.krit", "1\n");
    for (from, to, expected) in [
        (
            "origins = [\"https://github.example.invalid\"]",
            "origins = [\"https://api.github.com\"]",
            "not a granted `http` origin",
        ),
        (
            "inject = \"bearer\"",
            "inject = \"basic\"",
            "unknown variant `basic`",
        ),
    ] {
        let manifest = directory.file("krit.pkg", &BOUND_SECRETS_MANIFEST.replacen(from, to, 1));
        let output = krit_in(&directory)
            .args(["permissions"])
            .arg(&manifest)
            .output()
            .expect("Krit should start");
        assert_eq!(output.status.code(), Some(3));
        assert!(output.stdout.is_empty());
        let stderr = String::from_utf8(output.stderr).expect("diagnostic should be UTF-8");
        assert!(stderr.contains("error[K6001]"), "{stderr}");
        assert!(stderr.contains(expected), "{stderr}");
    }
}

#[cfg(unix)]
#[test]
fn invoke_refuses_the_swapped_token_probe_with_k5006() {
    use std::os::unix::fs::PermissionsExt;

    // The critic's probe: webhook-agent's GitHub credential sent to its
    // messaging origin. It builds, because both resources are granted, but the
    // host refuses the pairing before any request is resolved or sent.
    let directory = TestDirectory::new("swapped-token-probe");
    directory.file(
        "main.krit",
        &bearer_source("github-token", "https://message.example.invalid"),
    );
    let manifest = directory.file("krit.pkg", BOUND_SECRETS_MANIFEST);
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
    let secret_value = "fixture-github-value";
    for (name, value) in [
        ("github.secret", secret_value),
        ("message.secret", "fixture-message-value"),
    ] {
        let path = directory.file(name, value);
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600))
            .expect("secret permissions should set");
    }
    let host = directory.file(
        "host.json",
        r#"{
  "schema": 2,
  "secrets": {
    "github-token": {"file": "github.secret"},
    "message-token": {"file": "message.secret"}
  },
  "approvals": [
    {"operation": "http.bearer", "resource": "https://github.example.invalid"},
    {"operation": "http.bearer", "resource": "https://message.example.invalid"}
  ]
}"#,
    );
    let request = directory.file(
        "request.json",
        r#"{"method":"POST","path":"/messages","query":"","headers":[],"body":"summary"}"#,
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
    assert_eq!(invoked.status.code(), Some(4));
    assert!(invoked.stdout.is_empty());
    let stderr = String::from_utf8(invoked.stderr).expect("diagnostic should be UTF-8");
    assert!(
        stderr.contains(
            "error[K5006]: secret `github-token` is not bound to origin `https://message.example.invalid`"
        ),
        "{stderr}"
    );
    assert!(!stderr.contains(secret_value));
}
