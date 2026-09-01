use std::thread;

use krit_lsp::serve;
use lsp_server::{Connection, ErrorCode, Message, Notification, Request, RequestId};
use serde_json::{Value, json};

fn send(connection: &Connection, message: Message) {
    connection
        .sender
        .send(message)
        .expect("client message should be sent");
}

fn receive(connection: &Connection) -> Message {
    connection
        .receiver
        .recv()
        .expect("server message should be received")
}

fn response_result(message: Message, expected_id: i32) -> Result<Value, lsp_server::ResponseError> {
    let Message::Response(response) = message else {
        panic!("expected a response")
    };
    assert_eq!(response.id, RequestId::from(expected_id));
    response.response_result
}

#[test]
fn handles_protocol_errors_deterministically_and_shuts_down_gracefully() {
    let (server, client) = Connection::memory();
    let server_thread = thread::spawn(move || serve(server));

    send(
        &client,
        Message::Request(Request::new(
            RequestId::from(1),
            "initialize".to_owned(),
            json!({
                "processId": null,
                "capabilities": {},
                "workspaceFolders": null
            }),
        )),
    );
    let initialize =
        response_result(receive(&client), 1).expect("initialize request should succeed");
    assert_eq!(initialize["capabilities"]["positionEncoding"], "utf-16");
    assert_eq!(
        initialize["capabilities"]["experimental"]["kritCompilerFacts"]["schema"],
        1
    );
    send(
        &client,
        Message::Notification(Notification::new("initialized".to_owned(), json!({}))),
    );

    let uri = "file:///tmp/krit-lsp-protocol.krit";
    send(
        &client,
        Message::Notification(Notification::new(
            "textDocument/didOpen".to_owned(),
            json!({
                "textDocument": {
                    "uri": uri,
                    "languageId": "krit",
                    "version": 1,
                    "text": "let value = missing;\n"
                }
            }),
        )),
    );
    let Message::Notification(diagnostics) = receive(&client) else {
        panic!("didOpen should publish diagnostics")
    };
    assert_eq!(diagnostics.method, "textDocument/publishDiagnostics");
    assert_eq!(diagnostics.params["diagnostics"][0]["code"], "K2001");

    send(
        &client,
        Message::Request(Request::new(
            RequestId::from(2),
            "textDocument/hover".to_owned(),
            json!({}),
        )),
    );
    let error = response_result(receive(&client), 2)
        .expect_err("malformed request params should be rejected");
    assert_eq!(error.code, ErrorCode::InvalidParams as i32);

    send(
        &client,
        Message::Notification(Notification::new(
            "textDocument/didChange".to_owned(),
            json!({
                "textDocument": {"uri": uri, "version": 2},
                "contentChanges": [{
                    "text": "println(42);\n",
                    "range": {
                        "start": {"line": 0, "character": 0},
                        "end": {"line": 0, "character": 0}
                    }
                }]
            }),
        )),
    );
    send(
        &client,
        Message::Request(Request::new(
            RequestId::from(3),
            "krit/compilerFacts".to_owned(),
            json!({"textDocument": {"uri": uri}}),
        )),
    );
    let unchanged =
        response_result(receive(&client), 3).expect("compiler facts request should succeed");
    assert_eq!(unchanged["documentVersion"], 1);
    assert_eq!(unchanged["diagnostics"][0]["code"], "K2001");

    send(
        &client,
        Message::Notification(Notification::new(
            "textDocument/didChange".to_owned(),
            json!({
                "textDocument": {"uri": uri, "version": 2},
                "contentChanges": [{"text": "println(42);\n"}]
            }),
        )),
    );
    let Message::Notification(diagnostics) = receive(&client) else {
        panic!("valid didChange should publish diagnostics")
    };
    assert_eq!(diagnostics.params["diagnostics"], json!([]));

    for id in [4, 5] {
        send(
            &client,
            Message::Request(Request::new(
                RequestId::from(id),
                "krit/compilerFacts".to_owned(),
                json!({"textDocument": {"uri": uri}}),
            )),
        );
    }
    let first = response_result(receive(&client), 4).expect("facts should succeed");
    let second = response_result(receive(&client), 5).expect("facts should succeed");
    assert_eq!(first, second);
    assert_eq!(first["module"]["effects"], json!(["io.stdout"]));

    send(
        &client,
        Message::Request(Request::new(
            RequestId::from(6),
            "krit/unknown".to_owned(),
            json!({}),
        )),
    );
    let error =
        response_result(receive(&client), 6).expect_err("unknown request should be rejected");
    assert_eq!(error.code, ErrorCode::MethodNotFound as i32);

    send(
        &client,
        Message::Request(Request::new(
            RequestId::from(7),
            "shutdown".to_owned(),
            Value::Null,
        )),
    );
    send(
        &client,
        Message::Notification(Notification::new("exit".to_owned(), Value::Null)),
    );
    response_result(receive(&client), 7).expect("shutdown should succeed");
    server_thread
        .join()
        .expect("server thread should not panic")
        .expect("server should shut down cleanly");
}
