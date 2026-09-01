mod document;
mod position;

use std::{
    error::Error,
    fmt, io,
    io::{BufRead, Write},
    path::PathBuf,
    thread,
};

use document::{ServerState, request_failed, uri_to_file_path};
use lsp_server::{Connection, ErrorCode, Message, Notification, Request, RequestId, Response};
use lsp_types::{
    CodeActionKind, CodeActionOptions, CodeActionProviderCapability, CompletionOptions,
    CompletionParams, DidChangeTextDocumentParams, DidCloseTextDocumentParams,
    DidOpenTextDocumentParams, DocumentFormattingParams, DocumentSymbolParams, HoverParams,
    HoverProviderCapability, InitializeParams, InitializeResult, OneOf, PositionEncodingKind,
    PublishDiagnosticsParams, ServerCapabilities, ServerInfo, TextDocumentIdentifier,
    TextDocumentSyncCapability, TextDocumentSyncKind, TextDocumentSyncOptions,
};
use serde::{Deserialize, de::DeserializeOwned};
use serde_json::json;

const SERVER_NAME: &str = "krit-lsp";
const SERVER_VERSION: &str = env!("CARGO_PKG_VERSION");
const MAX_PROTOCOL_MESSAGE_BYTES: usize = 16 * 1024 * 1024;
const MAX_PROTOCOL_HEADER_BYTES: usize = 8 * 1024;

#[derive(Debug)]
pub enum ServerError {
    Json(serde_json::Error),
    Io(io::Error),
    Disconnected,
    InvalidInitialize,
    ExitBeforeShutdown,
}

impl fmt::Display for ServerError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Json(error) => error.fmt(formatter),
            Self::Io(error) => error.fmt(formatter),
            Self::Disconnected => formatter.write_str("language-server connection disconnected"),
            Self::InvalidInitialize => formatter.write_str("invalid initialize request"),
            Self::ExitBeforeShutdown => {
                formatter.write_str("received `exit` before a successful `shutdown` request")
            }
        }
    }
}

impl Error for ServerError {}

impl From<serde_json::Error> for ServerError {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error)
    }
}

impl From<io::Error> for ServerError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

pub fn run_stdio() -> Result<(), ServerError> {
    let (input_sender, input_receiver) = crossbeam_channel::bounded(0);
    let (output_sender, output_receiver) = crossbeam_channel::bounded(0);
    let connection = Connection {
        sender: output_sender,
        receiver: input_receiver,
    };
    let reader = thread::Builder::new()
        .name("KritLspReader".to_owned())
        .spawn(move || {
            let stdin = io::stdin();
            let mut stdin = stdin.lock();
            while let Some(message) = read_protocol_message(&mut stdin)? {
                let is_exit =
                    matches!(&message, Message::Notification(notification) if notification.method == "exit");
                input_sender
                    .send(message)
                    .map_err(|error| io::Error::other(error.to_string()))?;
                if is_exit {
                    break;
                }
            }
            Ok(())
        })
        .map_err(ServerError::Io)?;
    let writer = thread::Builder::new()
        .name("KritLspWriter".to_owned())
        .spawn(move || {
            let stdout = io::stdout();
            let mut stdout = stdout.lock();
            for message in output_receiver {
                write_protocol_message(&mut stdout, &message)?;
            }
            Ok(())
        })
        .map_err(ServerError::Io)?;

    let server_result = serve(connection);
    let reader_result = if server_result.is_ok() || reader.is_finished() {
        join_io_thread(reader, "reader")
    } else {
        Ok(())
    };
    let writer_result = join_io_thread(writer, "writer");
    reader_result.and(server_result).and(writer_result)
}

pub fn serve(connection: Connection) -> Result<(), ServerError> {
    let initialize = initialize(&connection)?;
    let roots = workspace_roots(&initialize);
    let mut state = ServerState::new(roots);
    let mut shutting_down = false;
    loop {
        let message = connection
            .receiver
            .recv()
            .map_err(|_| ServerError::Disconnected)?;
        match message {
            Message::Request(request) => {
                if shutting_down {
                    send_response(
                        &connection,
                        Response::new_err(
                            request.id,
                            ErrorCode::InvalidRequest as i32,
                            "language server is shutting down".to_owned(),
                        ),
                    )?;
                } else if request.method == "shutdown" {
                    if request.params.is_null() {
                        send_response(
                            &connection,
                            Response {
                                id: request.id,
                                response_result: Ok(serde_json::Value::Null),
                            },
                        )?;
                        shutting_down = true;
                    } else {
                        send_response(
                            &connection,
                            Response::new_err(
                                request.id,
                                ErrorCode::InvalidParams as i32,
                                "`shutdown` params must be null".to_owned(),
                            ),
                        )?;
                    }
                } else {
                    let response = handle_request(&state, request);
                    send_response(&connection, response)?;
                }
            }
            Message::Notification(notification) => {
                if notification.method == "exit" {
                    if shutting_down {
                        break;
                    }
                    return Err(ServerError::ExitBeforeShutdown);
                }
                if !shutting_down
                    && let Some(diagnostics) = handle_notification(&mut state, notification)
                {
                    send_notification(&connection, "textDocument/publishDiagnostics", diagnostics)?;
                }
            }
            Message::Response(_) => {}
        }
    }
    Ok(())
}

fn initialize(connection: &Connection) -> Result<InitializeParams, ServerError> {
    let (initialize_id, initialize_value) = loop {
        let message = connection
            .receiver
            .recv()
            .map_err(|_| ServerError::Disconnected)?;
        match message {
            Message::Request(request) if request.method == "initialize" => {
                break (request.id, request.params);
            }
            Message::Request(request) => {
                send_response(
                    connection,
                    Response::new_err(
                        request.id,
                        ErrorCode::ServerNotInitialized as i32,
                        "language server has not been initialized".to_owned(),
                    ),
                )?;
            }
            Message::Notification(notification) if notification.method == "exit" => {
                return Err(ServerError::ExitBeforeShutdown);
            }
            Message::Notification(_) | Message::Response(_) => {}
        }
    };
    let initialize: InitializeParams = match serde_json::from_value(initialize_value) {
        Ok(initialize) => initialize,
        Err(_) => {
            send_response(
                connection,
                Response::new_err(
                    initialize_id,
                    ErrorCode::InvalidParams as i32,
                    "invalid initialize params".to_owned(),
                ),
            )?;
            return Err(ServerError::InvalidInitialize);
        }
    };
    let result = InitializeResult {
        capabilities: server_capabilities(),
        server_info: Some(ServerInfo {
            name: SERVER_NAME.to_owned(),
            version: Some(SERVER_VERSION.to_owned()),
        }),
    };
    send_response(
        connection,
        Response {
            id: initialize_id,
            response_result: Ok(serde_json::to_value(result)?),
        },
    )?;
    loop {
        let message = connection
            .receiver
            .recv()
            .map_err(|_| ServerError::Disconnected)?;
        match message {
            Message::Notification(notification) if notification.method == "initialized" => {
                return Ok(initialize);
            }
            Message::Request(request) => {
                send_response(
                    connection,
                    Response::new_err(
                        request.id,
                        ErrorCode::ServerNotInitialized as i32,
                        "language server is waiting for `initialized`".to_owned(),
                    ),
                )?;
            }
            Message::Notification(notification) if notification.method == "exit" => {
                return Err(ServerError::ExitBeforeShutdown);
            }
            Message::Notification(_) | Message::Response(_) => {}
        }
    }
}

fn handle_request(state: &ServerState, request: Request) -> Response {
    let Request { id, method, params } = request;
    match method.as_str() {
        "textDocument/hover" => request_result::<HoverParams, _>(id, &method, params, |params| {
            let position = params.text_document_position_params.position;
            let uri = params.text_document_position_params.text_document.uri;
            state.hover(&uri, position)
        }),
        "textDocument/completion" => {
            request_result::<CompletionParams, _>(id, &method, params, |params| {
                let position = params.text_document_position.position;
                let uri = params.text_document_position.text_document.uri;
                state.completion(&uri, position)
            })
        }
        "textDocument/formatting" => {
            request_result::<DocumentFormattingParams, _>(id, &method, params, |params| {
                state.formatting(&params.text_document.uri)
            })
        }
        "textDocument/documentSymbol" => {
            request_result::<DocumentSymbolParams, _>(id, &method, params, |params| {
                state.document_symbols(&params.text_document.uri)
            })
        }
        "textDocument/codeAction" => {
            request_result::<lsp_types::CodeActionParams, _>(id, &method, params, |params| {
                state.code_actions(&params.text_document.uri)
            })
        }
        "krit/compilerFacts" => {
            request_result::<CompilerFactsParams, _>(id, &method, params, |params| {
                state.compiler_facts(&params.text_document.uri)
            })
        }
        _ => Response::new_err(
            id,
            ErrorCode::MethodNotFound as i32,
            "unsupported request method".to_owned(),
        ),
    }
}

fn request_result<P, T>(
    id: RequestId,
    method: &str,
    params: serde_json::Value,
    operation: impl FnOnce(P) -> Result<T, String>,
) -> Response
where
    P: DeserializeOwned,
    T: serde::Serialize,
{
    let params = match serde_json::from_value(params) {
        Ok(params) => params,
        Err(_) => {
            return Response::new_err(
                id,
                ErrorCode::InvalidParams as i32,
                format!("invalid params for `{method}`"),
            );
        }
    };
    match operation(params) {
        Ok(result) => match serde_json::to_value(result) {
            Ok(result)
                if serde_json::to_vec(&result)
                    .is_ok_and(|bytes| bytes.len() <= MAX_PROTOCOL_MESSAGE_BYTES) =>
            {
                Response {
                    id,
                    response_result: Ok(result),
                }
            }
            Ok(_) => Response::new_err(
                id,
                ErrorCode::RequestFailed as i32,
                format!("`{method}` response exceeds the bounded output limit"),
            ),
            Err(error) => Response::new_err(
                id,
                ErrorCode::InternalError as i32,
                format!("could not serialize `{method}` response: {error}"),
            ),
        },
        Err(message) => {
            let (code, message) = request_failed(message);
            Response::new_err(id, code, message)
        }
    }
}

fn handle_notification(
    state: &mut ServerState,
    notification: Notification,
) -> Option<PublishDiagnosticsParams> {
    let Notification { method, params } = notification;
    match method.as_str() {
        "textDocument/didOpen" => {
            let params = parse_notification::<DidOpenTextDocumentParams>(&method, params)?;
            Some(state.open(
                params.text_document.uri,
                params.text_document.version,
                params.text_document.text,
            ))
        }
        "textDocument/didChange" => {
            let params = parse_notification::<DidChangeTextDocumentParams>(&method, params)?;
            if params.content_changes.len() != 1
                || params.content_changes[0].range.is_some()
                || params.content_changes[0].range_length.is_some()
            {
                eprintln!("krit-lsp: ignored malformed full-document change for an open document");
                return None;
            }
            let change = params
                .content_changes
                .into_iter()
                .next()
                .expect("one change was checked above");
            match state.change(
                &params.text_document.uri,
                params.text_document.version,
                change.text,
            ) {
                Ok(diagnostics) => Some(diagnostics),
                Err(error) => {
                    eprintln!("krit-lsp: ignored document change: {error}");
                    None
                }
            }
        }
        "textDocument/didClose" => {
            let params = parse_notification::<DidCloseTextDocumentParams>(&method, params)?;
            Some(state.close(&params.text_document.uri))
        }
        _ => None,
    }
}

fn parse_notification<P: DeserializeOwned>(method: &str, params: serde_json::Value) -> Option<P> {
    match serde_json::from_value(params) {
        Ok(params) => Some(params),
        Err(_) => {
            eprintln!("krit-lsp: ignored invalid `{method}` notification");
            None
        }
    }
}

fn send_response(connection: &Connection, response: Response) -> Result<(), ServerError> {
    let message = Message::Response(response);
    validate_outgoing_message(&message)?;
    connection
        .sender
        .send(message)
        .map_err(|_| ServerError::Disconnected)
}

fn send_notification(
    connection: &Connection,
    method: &str,
    params: impl serde::Serialize,
) -> Result<(), ServerError> {
    let message = Message::Notification(Notification::new(method.to_owned(), params));
    validate_outgoing_message(&message)?;
    connection
        .sender
        .send(message)
        .map_err(|_| ServerError::Disconnected)
}

fn read_protocol_message(reader: &mut impl BufRead) -> io::Result<Option<Message>> {
    let mut content_length = None;
    let mut header_bytes = 0;
    loop {
        let line = read_header_line(reader, MAX_PROTOCOL_HEADER_BYTES - header_bytes)?;
        let Some(line) = line else {
            return if header_bytes == 0 {
                Ok(None)
            } else {
                Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "incomplete LSP header",
                ))
            };
        };
        header_bytes += line.len();
        if line == b"\r\n" {
            break;
        }
        let Some(line) = line.strip_suffix(b"\r\n") else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "LSP headers must use CRLF line endings",
            ));
        };
        let line = std::str::from_utf8(line)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "LSP headers must be ASCII"))?;
        let Some((name, value)) = line.split_once(':') else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "malformed LSP header",
            ));
        };
        if name.eq_ignore_ascii_case("Content-Length") {
            if content_length.is_some() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "duplicate LSP Content-Length header",
                ));
            }
            let length = value.trim().parse::<usize>().map_err(|_| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    "invalid LSP Content-Length header",
                )
            })?;
            if length > MAX_PROTOCOL_MESSAGE_BYTES {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "LSP payload exceeds the bounded input limit",
                ));
            }
            content_length = Some(length);
        }
    }
    let content_length = content_length.ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "missing LSP Content-Length header",
        )
    })?;
    let mut body = vec![0; content_length];
    reader.read_exact(&mut body)?;
    serde_json::from_slice(&body).map(Some).map_err(|error| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "malformed LSP JSON payload at line {} column {}",
                error.line(),
                error.column()
            ),
        )
    })
}

fn read_header_line(reader: &mut impl BufRead, max_bytes: usize) -> io::Result<Option<Vec<u8>>> {
    let mut line = Vec::new();
    loop {
        let available = reader.fill_buf()?;
        if available.is_empty() {
            return if line.is_empty() {
                Ok(None)
            } else {
                Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "incomplete LSP header line",
                ))
            };
        }
        let take = available
            .iter()
            .position(|byte| *byte == b'\n')
            .map_or(available.len(), |position| position + 1);
        if line.len().saturating_add(take) > max_bytes {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "LSP header exceeds the bounded input limit",
            ));
        }
        line.extend_from_slice(&available[..take]);
        reader.consume(take);
        if line.last() == Some(&b'\n') {
            return Ok(Some(line));
        }
    }
}

fn write_protocol_message(writer: &mut impl Write, message: &Message) -> io::Result<()> {
    let mut frame = Vec::new();
    message.write(&mut frame)?;
    if protocol_body_length(&frame)? > MAX_PROTOCOL_MESSAGE_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "LSP response exceeds the bounded output limit",
        ));
    }
    writer.write_all(&frame)?;
    writer.flush()
}

fn validate_outgoing_message(message: &Message) -> Result<(), ServerError> {
    let mut frame = Vec::new();
    message.write(&mut frame).map_err(ServerError::Io)?;
    if protocol_body_length(&frame)? > MAX_PROTOCOL_MESSAGE_BYTES {
        return Err(ServerError::Io(io::Error::new(
            io::ErrorKind::InvalidData,
            "LSP response exceeds the bounded output limit",
        )));
    }
    Ok(())
}

fn protocol_body_length(frame: &[u8]) -> io::Result<usize> {
    let separator = frame
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "invalid LSP output frame"))?;
    Ok(frame.len() - separator - 4)
}

fn join_io_thread(
    thread: thread::JoinHandle<io::Result<()>>,
    name: &str,
) -> Result<(), ServerError> {
    match thread.join() {
        Ok(result) => result.map_err(ServerError::Io),
        Err(_) => Err(ServerError::Io(io::Error::other(format!(
            "language-server {name} thread panicked"
        )))),
    }
}

fn server_capabilities() -> ServerCapabilities {
    ServerCapabilities {
        position_encoding: Some(PositionEncodingKind::UTF16),
        text_document_sync: Some(TextDocumentSyncCapability::Options(
            TextDocumentSyncOptions {
                open_close: Some(true),
                change: Some(TextDocumentSyncKind::FULL),
                ..TextDocumentSyncOptions::default()
            },
        )),
        hover_provider: Some(HoverProviderCapability::Simple(true)),
        completion_provider: Some(CompletionOptions {
            resolve_provider: Some(false),
            trigger_characters: Some(vec![".".to_owned(), "\"".to_owned()]),
            ..CompletionOptions::default()
        }),
        document_symbol_provider: Some(OneOf::Left(true)),
        code_action_provider: Some(CodeActionProviderCapability::Options(CodeActionOptions {
            code_action_kinds: Some(vec![CodeActionKind::SOURCE_FIX_ALL]),
            resolve_provider: Some(false),
            ..CodeActionOptions::default()
        })),
        document_formatting_provider: Some(OneOf::Left(true)),
        experimental: Some(json!({
            "kritCompilerFacts": {
                "schema": 1,
                "authoringProtocol": 1,
                "method": "krit/compilerFacts"
            }
        })),
        ..ServerCapabilities::default()
    }
}

#[allow(deprecated)]
fn workspace_roots(initialize: &InitializeParams) -> Vec<PathBuf> {
    let mut roots = initialize
        .workspace_folders
        .as_ref()
        .into_iter()
        .flatten()
        .filter_map(|folder| uri_to_file_path(&folder.uri))
        .collect::<Vec<_>>();
    if roots.is_empty()
        && let Some(root) = initialize.root_uri.as_ref().and_then(uri_to_file_path)
    {
        roots.push(root);
    }
    if roots.is_empty()
        && let Some(root) = &initialize.root_path
    {
        roots.push(PathBuf::from(root));
    }
    roots
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CompilerFactsParams {
    text_document: TextDocumentIdentifier,
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use super::*;

    #[test]
    fn rejects_oversized_and_malformed_protocol_payloads_without_echoing_them() {
        let oversized = format!("Content-Length: {}\r\n\r\n", MAX_PROTOCOL_MESSAGE_BYTES + 1);
        let error = read_protocol_message(&mut Cursor::new(oversized))
            .expect_err("oversized payload should fail");
        assert!(error.to_string().contains("bounded input limit"));

        let overlong_header = vec![b'a'; MAX_PROTOCOL_HEADER_BYTES + 1];
        let error = read_protocol_message(&mut Cursor::new(overlong_header))
            .expect_err("an unterminated oversized header should fail");
        assert!(error.to_string().contains("bounded input limit"));

        let malformed = b"Content-Length: 13\r\n\r\n{\"secret\":\"x\"";
        let error = read_protocol_message(&mut Cursor::new(malformed))
            .expect_err("malformed payload should fail");
        assert!(error.to_string().contains("malformed LSP JSON payload"));
        assert!(!error.to_string().contains("secret"));

        let response = handle_request(
            &ServerState::new(Vec::new()),
            Request::new(
                RequestId::from(1),
                "textDocument/hover".to_owned(),
                "SUPERSECRET",
            ),
        );
        let response = serde_json::to_string(&response).expect("response should serialize");
        assert!(!response.contains("SUPERSECRET"));
    }

    #[test]
    fn language_server_dependencies_exclude_execution_and_network_hosts() {
        let manifest = include_str!("../Cargo.toml");
        for dependency in [
            "krit-runtime",
            "krit-wasm",
            "curl",
            "tiny_http",
            "wasmtime",
            "tokio",
        ] {
            assert!(
                !manifest.contains(dependency),
                "{dependency} must not be a language-server dependency"
            );
        }
    }

    #[test]
    fn rejects_outbound_messages_above_the_json_body_limit() {
        let message = Message::Notification(Notification::new(
            "textDocument/publishDiagnostics".to_owned(),
            json!({
                "uri": format!("file:///{}", "x".repeat(MAX_PROTOCOL_MESSAGE_BYTES)),
                "diagnostics": []
            }),
        ));

        let error =
            validate_outgoing_message(&message).expect_err("oversized output should fail closed");
        assert!(error.to_string().contains("bounded output limit"));
    }
}
