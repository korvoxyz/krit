mod assist;
mod host_config;

use std::{
    collections::BTreeMap,
    env, fs,
    fs::OpenOptions,
    io::{self, Read, Write},
    net::SocketAddr,
    path::{Path, PathBuf},
    process::ExitCode,
};

use krit::{
    Analysis, CoreModule, Diagnostic, EntrypointKind, RequirementSet, Source, Span, analyze,
    execute, format_source, lower, parse_source,
};
use krit_package::Manifest;
use krit_runtime::{
    CancellationHandle, DeliveryOutcome, GrantSet, HttpHeader, HttpRequest, LogEvent, LogField,
    LogLevel, Runtime, RuntimeError, RuntimeErrorKind, RuntimeLimits,
};
use krit_wasm::{ArtifactMetadata, BuildErrorKind, BuildOptions, build_component};
use serde::Serialize;

const VERSION: &str = env!("CARGO_PKG_VERSION");
const GENERATION_PROMPT: &str = include_str!("../assets/KRIT-0.2-SYSTEM.md");

#[derive(Clone, Copy, Debug)]
enum DiagnosticFormat {
    Human,
    Json,
}

fn main() -> ExitCode {
    ExitCode::from(run(env::args().skip(1).collect()))
}

fn run(arguments: Vec<String>) -> u8 {
    let Some(command) = arguments.first().map(String::as_str) else {
        print_help();
        return 2;
    };

    match command {
        "-h" | "--help" | "help" => {
            print_help();
            0
        }
        "-V" | "--version" | "version" => {
            println!("Krit {VERSION}");
            0
        }
        "run" => source_command(&arguments[1..], SourceAction::Run),
        "check" => source_command(&arguments[1..], SourceAction::Check),
        "build" => build_command(&arguments[1..]),
        "assist" => assist::run(&arguments[1..]),
        "explain" => explain_command(&arguments[1..]),
        "fmt" => fmt_command(&arguments[1..]),
        "lsp" => lsp_command(&arguments[1..]),
        "prompt" => prompt_command(&arguments[1..]),
        "permissions" => permissions_command(&arguments[1..]),
        "sandbox" => sandbox_command(&arguments[1..]),
        "invoke" => invoke_command(&arguments[1..]),
        "worker" => delivery_command(&arguments[1..], DeliveryKind::Queue),
        "schedule" => delivery_command(&arguments[1..], DeliveryKind::Schedule),
        "serve" => serve_command(&arguments[1..]),
        "package" => package_command(&arguments[1..]),
        unknown => {
            eprintln!("krit: unknown command `{unknown}`");
            eprintln!("Run `krit --help` for usage.");
            2
        }
    }
}

fn build_command(arguments: &[String]) -> u8 {
    let (manifest_path, requested_output) = match parse_build_options(arguments) {
        Ok(options) => options,
        Err(message) => {
            eprintln!("krit: {message}");
            return 2;
        }
    };
    let manifest = match Manifest::load(&manifest_path) {
        Ok(manifest) => manifest,
        Err(error) => {
            eprintln!(
                "{}:1:1: error[K6001]: {error}",
                manifest_path.to_string_lossy()
            );
            return 3;
        }
    };
    let entry_path = match manifest.resolve_entry(&manifest_path) {
        Ok(path) => path,
        Err(error) => {
            eprintln!(
                "{}:1:1: error[K6001]: {error}",
                manifest_path.to_string_lossy()
            );
            return 3;
        }
    };
    let entry_name = manifest.package.entry.to_string_lossy().replace('\\', "/");
    let text = match fs::read_to_string(&entry_path) {
        Ok(text) => text,
        Err(error) => {
            eprintln!("{entry_name}:1:1: error[K7003]: could not read package entry: {error}");
            return 1;
        }
    };
    let source = Source::new(entry_name.clone(), text);
    let program = match parse_source(&source) {
        Ok(program) => program,
        Err(diagnostic) => return report(&diagnostic, &source, DiagnosticFormat::Human),
    };
    let analysis = match analyze(&program) {
        Ok(analysis) => analysis,
        Err(diagnostic) => return report(&diagnostic, &source, DiagnosticFormat::Human),
    };
    let module = match lower(&program, &analysis) {
        Ok(module) => module,
        Err(error) => return internal_error("KICE0001", "lowering Core IR", &error),
    };
    if let Some((requirement, span)) = first_missing_requirement(&analysis, &manifest) {
        let diagnostic = Diagnostic::new(
            "K5001",
            format!(
                "required capability `{}` for resource `{}` is not granted by the package",
                requirement.capability().as_str(),
                requirement.resource()
            ),
            span,
        );
        eprintln!("{}", diagnostic.render_human(&source));
        return 4;
    }

    let mut options = BuildOptions::new(
        &manifest.package.edition,
        &manifest.package.name,
        &manifest.package.version,
        &entry_name,
    );
    options.target.clone_from(&manifest.package.target);
    if manifest.capabilities.stdout {
        options.grant_effect("io.stdout");
    }
    if !manifest.capabilities.config.is_empty() {
        options.grant_effect("config.read");
    }
    if !manifest.capabilities.http.is_empty() {
        options.grant_effect("http.request");
    }
    if !manifest.capabilities.secrets.is_empty() {
        options.grant_effect("secret.read");
    }
    if !manifest.capabilities.ai.is_empty() {
        options.grant_effect("ai.invoke");
    }
    if manifest.capabilities.logs {
        options.grant_effect("observe.log");
    }
    if !manifest.capabilities.state.is_empty() {
        options.grant_effect("state.transaction");
    }
    if !manifest.capabilities.queues.is_empty() {
        options.grant_effect("queue.publish");
    }
    if !manifest.capabilities.consumes.is_empty() {
        options.grant_effect("queue.consume");
    }
    if !manifest.capabilities.schedules.is_empty() {
        options.grant_effect("schedule.trigger");
    }
    if !manifest.capabilities.buckets.is_empty() {
        options.grant_effect("object.write");
    }
    if !manifest.capabilities.buckets.is_empty()
        || !manifest.capabilities.read_only_buckets.is_empty()
    {
        options.grant_effect("object.read");
    }
    if !manifest.capabilities.cache_namespaces.is_empty() {
        options.grant_effect("cache.write");
    }
    if !manifest.capabilities.cache_namespaces.is_empty()
        || !manifest.capabilities.read_only_cache_namespaces.is_empty()
    {
        options.grant_effect("cache.read");
    }
    if !manifest.capabilities.search_indexes.is_empty() {
        options.grant_effect("search.query");
    }
    if !manifest.capabilities.vector_indexes.is_empty() {
        options.grant_effect("search.vector");
    }
    if !manifest.capabilities.databases.is_empty() {
        options.grant_effect("database.write");
    }
    if !manifest.capabilities.databases.is_empty()
        || !manifest.capabilities.read_only_databases.is_empty()
    {
        options.grant_effect("database.read");
    }
    let artifact = match build_component(&module, &options) {
        Ok(artifact) => artifact,
        Err(error) => {
            let span = error.span().unwrap_or_else(|| Span::new(0, 0));
            let diagnostic = Diagnostic::new(error.code(), error.message(), span);
            eprintln!("{}", diagnostic.render_human(&source));
            return if error.kind() == BuildErrorKind::Capability {
                4
            } else {
                1
            };
        }
    };

    let output =
        requested_output.unwrap_or_else(|| default_build_output(&manifest_path, &manifest));
    let metadata_path = metadata_path(&output);
    let mut metadata = match serde_json::to_vec_pretty(&artifact.metadata) {
        Ok(metadata) => metadata,
        Err(error) => return internal_error("KICE0003", "serializing artifact metadata", &error),
    };
    metadata.push(b'\n');
    if let Err(error) = replace_build_outputs(&output, &artifact.bytes, &metadata_path, &metadata) {
        eprintln!("{}:1:1: error[K7003]: {error}", output.to_string_lossy());
        return 1;
    }
    println!("built {}", output.display());
    println!("metadata {}", metadata_path.display());
    0
}

fn parse_build_options(arguments: &[String]) -> Result<(PathBuf, Option<PathBuf>), String> {
    let mut manifest = PathBuf::from("krit.pkg");
    let mut output = None;
    let mut index = 0;
    while index < arguments.len() {
        match arguments[index].as_str() {
            "--manifest" => {
                let value = arguments
                    .get(index + 1)
                    .ok_or_else(|| "`--manifest` requires a path".to_owned())?;
                manifest = PathBuf::from(value);
                index += 2;
            }
            argument if argument.starts_with("--manifest=") => {
                let value = argument
                    .split_once('=')
                    .map(|(_, value)| value)
                    .unwrap_or_default();
                if value.is_empty() {
                    return Err("`--manifest` requires a path".to_owned());
                }
                manifest = PathBuf::from(value);
                index += 1;
            }
            "--output" => {
                let value = arguments
                    .get(index + 1)
                    .ok_or_else(|| "`--output` requires a path".to_owned())?;
                output = Some(PathBuf::from(value));
                index += 2;
            }
            argument if argument.starts_with("--output=") => {
                let value = argument
                    .split_once('=')
                    .map(|(_, value)| value)
                    .unwrap_or_default();
                if value.is_empty() {
                    return Err("`--output` requires a path".to_owned());
                }
                output = Some(PathBuf::from(value));
                index += 1;
            }
            argument if argument.starts_with('-') => {
                return Err(format!("unknown option `{argument}`"));
            }
            argument => {
                return Err(format!("unexpected build argument `{argument}`"));
            }
        }
    }
    Ok((manifest, output))
}

fn default_build_output(manifest_path: &Path, manifest: &Manifest) -> PathBuf {
    let root = manifest_path
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let package = manifest
        .package
        .name
        .rsplit_once('/')
        .map_or(manifest.package.name.as_str(), |(_, name)| name);
    root.join("target/krit").join(format!("{package}.wasm"))
}

fn metadata_path(output: &Path) -> PathBuf {
    let mut path = output.as_os_str().to_os_string();
    path.push(".json");
    PathBuf::from(path)
}

struct BuildOutputFile {
    path: PathBuf,
    staged: PathBuf,
    backup: Option<PathBuf>,
}

fn replace_build_outputs(
    component_path: &Path,
    component: &[u8],
    metadata_path: &Path,
    metadata: &[u8],
) -> Result<(), String> {
    validate_build_destination(component_path)?;
    validate_build_destination(metadata_path)?;
    let component_staged = stage_build_output(component_path, component, "component")?;
    let metadata_staged = match stage_build_output(metadata_path, metadata, "metadata") {
        Ok(path) => path,
        Err(error) => {
            let _ = fs::remove_file(&component_staged);
            return Err(error);
        }
    };
    let mut files = [
        BuildOutputFile {
            path: component_path.to_owned(),
            staged: component_staged,
            backup: None,
        },
        BuildOutputFile {
            path: metadata_path.to_owned(),
            staged: metadata_staged,
            backup: None,
        },
    ];

    for index in 0..files.len() {
        if files[index].path.exists() {
            let backup = match allocate_build_sidecar(&files[index].path, "backup") {
                Ok(backup) => backup,
                Err(error) => {
                    restore_build_outputs(&mut files);
                    return Err(error);
                }
            };
            if let Err(error) = fs::rename(&files[index].path, &backup) {
                restore_build_outputs(&mut files);
                return Err(format!(
                    "could not stage existing output {}: {error}",
                    files[index].path.display()
                ));
            }
            files[index].backup = Some(backup);
        }
    }

    for index in 0..files.len() {
        if let Err(error) = fs::rename(&files[index].staged, &files[index].path) {
            for installed in &files[..index] {
                let _ = fs::remove_file(&installed.path);
            }
            restore_build_outputs(&mut files);
            return Err(format!(
                "could not replace output {}: {error}",
                files[index].path.display()
            ));
        }
    }
    for file in files {
        if let Some(backup) = file.backup {
            let _ = fs::remove_file(backup);
        }
    }
    Ok(())
}

fn validate_build_destination(path: &Path) -> Result<(), String> {
    if path.exists() && !path.is_file() {
        return Err(format!("output {} is not a regular file", path.display()));
    }
    let parent = output_parent(path);
    fs::create_dir_all(parent).map_err(|error| {
        format!(
            "could not create output directory {}: {error}",
            parent.display()
        )
    })
}

fn stage_build_output(path: &Path, bytes: &[u8], label: &str) -> Result<PathBuf, String> {
    let staged = allocate_build_sidecar(path, label)?;
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&staged)
        .map_err(|error| format!("could not create staged {label}: {error}"))?;
    if let Err(error) = output.write_all(bytes).and_then(|()| output.sync_all()) {
        drop(output);
        let _ = fs::remove_file(&staged);
        return Err(format!("could not write staged {label}: {error}"));
    }
    Ok(staged)
}

fn allocate_build_sidecar(path: &Path, label: &str) -> Result<PathBuf, String> {
    let parent = output_parent(path);
    let name = path
        .file_name()
        .ok_or_else(|| format!("invalid output path {}", path.display()))?;
    for attempt in 0..100 {
        let mut sidecar = std::ffi::OsString::from(".");
        sidecar.push(name);
        sidecar.push(format!(".krit-{label}-{}-{attempt}", std::process::id()));
        let candidate = parent.join(sidecar);
        if !candidate.exists() {
            return Ok(candidate);
        }
    }
    Err(format!(
        "could not allocate staged output beside {}",
        path.display()
    ))
}

fn output_parent(path: &Path) -> &Path {
    path.parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
}

fn restore_build_outputs(files: &mut [BuildOutputFile]) {
    for file in files {
        if file.staged.exists() {
            let _ = fs::remove_file(&file.staged);
        }
        if let Some(backup) = file.backup.take() {
            let _ = fs::rename(backup, &file.path);
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct JsonExplanation {
    schema: u8,
    entrypoint: JsonEntrypoint,
    entrypoints: JsonEntrypointFacts,
    bindings: Vec<JsonBinding>,
    durable_state: JsonDurableFacts,
    core: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct JsonDurableFacts {
    schema: u8,
    operations: Vec<JsonDurableOperation>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct JsonDurableOperation {
    kind: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    store: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    identity: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    external_capability: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    external_resource: Option<String>,
    span: JsonByteSpan,
}

#[derive(Serialize)]
struct JsonByteSpan {
    start: usize,
    end: usize,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct JsonEntrypoint {
    id: u32,
    kind: &'static str,
    result_type: String,
    effects: Vec<&'static str>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct JsonBinding {
    id: u32,
    name: String,
    kind: &'static str,
    r#type: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct JsonEntrypointFacts {
    schema: u8,
    items: Vec<JsonEntrypointFact>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct JsonEntrypointFact {
    name: String,
    kind: &'static str,
    function_id: u32,
    signature: String,
    effects: Vec<&'static str>,
    capability_requirements: Vec<JsonCapabilityRequirement>,
    #[serde(skip_serializing_if = "Option::is_none")]
    contract: Option<JsonWebhookContract>,
}

#[derive(Serialize)]
struct JsonCapabilityRequirement {
    capability: &'static str,
    resource: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct JsonWebhookContract {
    schema: u8,
    request_type: &'static str,
    response_type: &'static str,
    request_schema: JsonSchemaDocument,
    response_schema: JsonSchemaDocument,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct JsonSchemaDocument {
    #[serde(rename = "$schema")]
    dialect: &'static str,
    #[serde(rename = "$id")]
    id: &'static str,
    title: &'static str,
    #[serde(rename = "type")]
    kind: &'static str,
    additional_properties: bool,
    required: Vec<&'static str>,
    properties: BTreeMap<&'static str, JsonSchemaNode>,
    #[serde(rename = "$defs")]
    definitions: BTreeMap<&'static str, JsonSchemaNode>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct JsonSchemaNode {
    #[serde(rename = "type", skip_serializing_if = "Option::is_none")]
    kind: Option<&'static str>,
    #[serde(rename = "$ref", skip_serializing_if = "Option::is_none")]
    reference: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    items: Option<Box<Self>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    additional_properties: Option<bool>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    required: Vec<&'static str>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    properties: BTreeMap<&'static str, Self>,
}

fn explain_command(arguments: &[String]) -> u8 {
    let mut json = false;
    let mut path = None;
    for argument in arguments {
        match argument.as_str() {
            "--json" => json = true,
            argument if argument.starts_with('-') => {
                eprintln!("krit: unknown option `{argument}`");
                return 2;
            }
            argument if path.is_none() => path = Some(PathBuf::from(argument)),
            _ => {
                eprintln!("krit: expected exactly one source file");
                return 2;
            }
        }
    }
    let Some(path) = path else {
        eprintln!("krit: expected exactly one source file");
        return 2;
    };
    let text = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) => {
            eprintln!("krit: could not read {}: {error}", path.display());
            return 1;
        }
    };
    let source = Source::new(path.to_string_lossy().into_owned(), text);
    let program = match parse_source(&source) {
        Ok(program) => program,
        Err(diagnostic) => {
            return report(
                &diagnostic,
                &source,
                if json {
                    DiagnosticFormat::Json
                } else {
                    DiagnosticFormat::Human
                },
            );
        }
    };
    let analysis = match analyze(&program) {
        Ok(analysis) => analysis,
        Err(diagnostic) => {
            return report(
                &diagnostic,
                &source,
                if json {
                    DiagnosticFormat::Json
                } else {
                    DiagnosticFormat::Human
                },
            );
        }
    };
    let module = match lower(&program, &analysis) {
        Ok(module) => module,
        Err(error) => return internal_error("KICE0001", "lowering Core IR", &error),
    };
    if json {
        render_explanation_json(&program, &analysis, &module)
    } else {
        render_explanation_human(&program, &analysis, &module);
        0
    }
}

fn render_explanation_human(program: &krit::Program, analysis: &Analysis, module: &CoreModule) {
    let entrypoint = module.entrypoint_function();
    println!("Krit explanation (schema 1)");
    println!(
        "entrypoint: {} {}",
        module.entrypoints()[0].kind.as_str(),
        entrypoint.id
    );
    println!("result: {}", entrypoint.signature.result);
    println!("effects: {}", entrypoint.signature.effects);
    for entrypoint in module
        .entrypoints()
        .iter()
        .filter(|entrypoint| entrypoint.kind.is_exported())
    {
        let function = &module.functions()[entrypoint.function.as_u32() as usize];
        println!("{} contract (schema 1):", entrypoint.kind.as_str());
        println!(
            "  signature: {}",
            normalized_entrypoint_signature(entrypoint, function)
        );
        println!("  effects: {}", function.signature.effects);
        println!("  capabilities: {}", function.signature.requirements);
        if entrypoint.kind == EntrypointKind::Webhook {
            println!(
                "  request: HttpRequest {{ method: String, path: String, query: String, headers: List<HttpHeader>, body: String }}"
            );
            println!(
                "  response: HttpResponse {{ status: Int, headers: List<HttpHeader>, body: String }}"
            );
            println!("  JSON Schema: draft 2020-12 request/response contract v1");
        }
    }
    println!("durable operations:");
    let durable = krit::durable_operations(program);
    if durable.is_empty() {
        println!("  (none)");
    } else {
        for operation in durable {
            print!("  {}", operation.kind().as_str());
            if let Some(store) = operation.store() {
                print!(" store={store}");
            }
            if let Some(identity) = operation.identity() {
                print!(" identity={identity}");
            }
            if let (Some(capability), Some(resource)) = (
                operation.external_capability(),
                operation.external_resource(),
            ) {
                print!(" external={capability}({resource})");
            }
            println!();
        }
    }
    println!("top-level bindings:");
    let mut found = false;
    for binding in analysis
        .bindings()
        .iter()
        .filter(|binding| binding.is_top_level())
    {
        found = true;
        let core_binding = &module.bindings()[binding.id().as_u32() as usize];
        println!("  {} {}: {}", core_binding.id, binding.name(), binding.ty());
    }
    if !found {
        println!("  (none)");
    }
    println!("core:");
    print!("{}", module.render_text());
}

fn render_explanation_json(
    program: &krit::Program,
    analysis: &Analysis,
    module: &CoreModule,
) -> u8 {
    let entrypoint = module.entrypoint_function();
    let explanation = JsonExplanation {
        schema: 1,
        entrypoint: JsonEntrypoint {
            id: entrypoint.id.as_u32(),
            kind: module.entrypoints()[0].kind.as_str(),
            result_type: entrypoint.signature.result.to_string(),
            effects: entrypoint
                .signature
                .effects
                .iter()
                .map(|effect| effect.as_str())
                .collect(),
        },
        entrypoints: JsonEntrypointFacts {
            schema: 1,
            items: module
                .entrypoints()
                .iter()
                .map(|entrypoint| {
                    let function = &module.functions()[entrypoint.function.as_u32() as usize];
                    JsonEntrypointFact {
                        name: function
                            .debug_name
                            .clone()
                            .unwrap_or_else(|| "<anonymous>".to_owned()),
                        kind: entrypoint.kind.as_str(),
                        function_id: entrypoint.function.as_u32(),
                        signature: normalized_entrypoint_signature(entrypoint, function),
                        effects: function
                            .signature
                            .effects
                            .iter()
                            .map(|effect| effect.as_str())
                            .collect(),
                        capability_requirements: json_requirements(
                            &function.signature.requirements,
                        ),
                        contract: (entrypoint.kind == EntrypointKind::Webhook).then(|| {
                            JsonWebhookContract {
                                schema: 1,
                                request_type: "HttpRequest",
                                response_type: "HttpResponse",
                                request_schema: webhook_request_schema(),
                                response_schema: webhook_response_schema(),
                            }
                        }),
                    }
                })
                .collect(),
        },
        bindings: analysis
            .bindings()
            .iter()
            .filter(|binding| binding.is_top_level())
            .map(|binding| {
                let core_binding = &module.bindings()[binding.id().as_u32() as usize];
                JsonBinding {
                    id: core_binding.id.as_u32(),
                    name: binding.name().to_owned(),
                    kind: core_binding.kind.as_str(),
                    r#type: binding.ty().to_string(),
                }
            })
            .collect(),
        durable_state: JsonDurableFacts {
            schema: 1,
            operations: krit::durable_operations(program)
                .into_iter()
                .map(|operation| {
                    let span = operation.span();
                    JsonDurableOperation {
                        kind: operation.kind().as_str(),
                        store: operation.store().map(str::to_owned),
                        identity: operation.identity().map(str::to_owned),
                        external_capability: operation.external_capability(),
                        external_resource: operation.external_resource().map(str::to_owned),
                        span: JsonByteSpan {
                            start: span.start,
                            end: span.end,
                        },
                    }
                })
                .collect(),
        },
        core: module.render_text(),
    };
    match serde_json::to_string(&explanation) {
        Ok(json) => {
            println!("{json}");
            0
        }
        Err(error) => internal_error("KICE0002", "rendering an explanation", &error),
    }
}

fn normalized_entrypoint_signature(
    entrypoint: &krit::CoreEntrypoint,
    function: &krit::CoreFunction,
) -> String {
    match entrypoint.kind {
        EntrypointKind::ModuleInit => format!("fn() -> {}", function.signature.result),
        EntrypointKind::Webhook => format!(
            "webhook fn {}(request: HttpRequest) -> HttpResponse",
            function.debug_name.as_deref().unwrap_or("<anonymous>")
        ),
        EntrypointKind::QueueConsumer => format!(
            "queue fn {}(job: QueueJob) -> Result<String, String>",
            function.debug_name.as_deref().unwrap_or("<anonymous>")
        ),
        EntrypointKind::ScheduleHandler => format!(
            "schedule fn {}(event: ScheduleEvent) -> Result<String, String>",
            function.debug_name.as_deref().unwrap_or("<anonymous>")
        ),
        _ => format!("fn(...) -> {}", function.signature.result),
    }
}

fn json_requirements(requirements: &RequirementSet) -> Vec<JsonCapabilityRequirement> {
    requirements
        .iter()
        .map(|requirement| JsonCapabilityRequirement {
            capability: requirement.capability().as_str(),
            resource: requirement.resource().to_owned(),
        })
        .collect()
}

fn webhook_request_schema() -> JsonSchemaDocument {
    JsonSchemaDocument {
        dialect: "https://json-schema.org/draft/2020-12/schema",
        id: "https://krit.dev/schemas/webhook/request-1.json",
        title: "Krit HttpRequest contract v1",
        kind: "object",
        additional_properties: false,
        required: vec!["method", "path", "query", "headers", "body"],
        properties: BTreeMap::from([
            ("body", string_schema()),
            (
                "headers",
                array_schema(reference_schema("#/$defs/HttpHeader")),
            ),
            ("method", string_schema()),
            ("path", string_schema()),
            ("query", string_schema()),
        ]),
        definitions: BTreeMap::from([("HttpHeader", http_header_schema())]),
    }
}

fn webhook_response_schema() -> JsonSchemaDocument {
    JsonSchemaDocument {
        dialect: "https://json-schema.org/draft/2020-12/schema",
        id: "https://krit.dev/schemas/webhook/response-1.json",
        title: "Krit HttpResponse contract v1",
        kind: "object",
        additional_properties: false,
        required: vec!["status", "headers", "body"],
        properties: BTreeMap::from([
            ("body", string_schema()),
            (
                "headers",
                array_schema(reference_schema("#/$defs/HttpHeader")),
            ),
            ("status", integer_schema()),
        ]),
        definitions: BTreeMap::from([("HttpHeader", http_header_schema())]),
    }
}

fn http_header_schema() -> JsonSchemaNode {
    JsonSchemaNode {
        kind: Some("object"),
        reference: None,
        items: None,
        additional_properties: Some(false),
        required: vec!["name", "value"],
        properties: BTreeMap::from([("name", string_schema()), ("value", string_schema())]),
    }
}

fn string_schema() -> JsonSchemaNode {
    scalar_schema("string")
}

fn integer_schema() -> JsonSchemaNode {
    scalar_schema("integer")
}

fn scalar_schema(kind: &'static str) -> JsonSchemaNode {
    JsonSchemaNode {
        kind: Some(kind),
        reference: None,
        items: None,
        additional_properties: None,
        required: Vec::new(),
        properties: BTreeMap::new(),
    }
}

fn reference_schema(reference: &'static str) -> JsonSchemaNode {
    JsonSchemaNode {
        kind: None,
        reference: Some(reference),
        items: None,
        additional_properties: None,
        required: Vec::new(),
        properties: BTreeMap::new(),
    }
}

fn array_schema(items: JsonSchemaNode) -> JsonSchemaNode {
    JsonSchemaNode {
        kind: Some("array"),
        reference: None,
        items: Some(Box::new(items)),
        additional_properties: None,
        required: Vec::new(),
        properties: BTreeMap::new(),
    }
}

fn internal_error(id: &str, operation: &str, error: &dyn std::fmt::Display) -> u8 {
    eprintln!("krit: internal compiler error[{id}] while {operation}: {error}");
    101
}

struct FormattedFile {
    path: PathBuf,
    source: Source,
    formatted: String,
}

struct StagedFile {
    path: PathBuf,
    temporary: PathBuf,
}

fn fmt_command(arguments: &[String]) -> u8 {
    let (check, paths) = match parse_fmt_options(arguments) {
        Ok(options) => options,
        Err(message) => {
            eprintln!("krit: {message}");
            return 2;
        }
    };

    if paths.is_empty() {
        eprintln!("krit: expected at least one source file");
        return 2;
    }

    let mut files = Vec::with_capacity(paths.len());
    for path in paths {
        let text = match fs::read_to_string(&path) {
            Ok(text) => text,
            Err(error) => {
                eprintln!("krit: could not read {}: {error}", path.display());
                return 1;
            }
        };
        let source = Source::new(path.to_string_lossy().into_owned(), text);
        let formatted = match format_source(&source) {
            Ok(formatted) => formatted,
            Err(diagnostic) => return report(&diagnostic, &source, DiagnosticFormat::Human),
        };
        files.push(FormattedFile {
            path,
            source,
            formatted,
        });
    }

    let changed = files
        .iter()
        .filter(|file| file.source.text() != file.formatted)
        .collect::<Vec<_>>();
    if check {
        for file in &changed {
            let diagnostic = Diagnostic::new(
                "K8001",
                "source is not canonically formatted",
                Span::new(0, 0),
            );
            eprintln!("{}", diagnostic.render_human(&file.source));
        }
        return u8::from(!changed.is_empty());
    }

    let staged = match stage_formatted_files(&changed) {
        Ok(staged) => staged,
        Err(message) => {
            eprintln!("krit: {message}");
            return 1;
        }
    };
    for (index, file) in staged.iter().enumerate() {
        if let Err(error) = fs::rename(&file.temporary, &file.path) {
            for remaining in &staged[index..] {
                let _ = fs::remove_file(&remaining.temporary);
            }
            eprintln!("krit: could not replace {}: {error}", file.path.display());
            return 1;
        }
        println!("formatted {}", file.path.display());
    }
    0
}

fn parse_fmt_options(arguments: &[String]) -> Result<(bool, Vec<PathBuf>), String> {
    let mut check = false;
    let mut paths = Vec::new();
    let mut positional_only = false;

    for argument in arguments {
        if positional_only {
            paths.push(PathBuf::from(argument));
            continue;
        }
        match argument.as_str() {
            "--check" => check = true,
            "--" => positional_only = true,
            argument if argument.starts_with('-') => {
                return Err(format!("unknown option `{argument}`"));
            }
            argument => paths.push(PathBuf::from(argument)),
        }
    }

    Ok((check, paths))
}

fn stage_formatted_files(files: &[&FormattedFile]) -> Result<Vec<StagedFile>, String> {
    let mut staged = Vec::with_capacity(files.len());
    for (index, file) in files.iter().enumerate() {
        match stage_formatted_file(file, index) {
            Ok(temporary) => staged.push(StagedFile {
                path: file.path.clone(),
                temporary,
            }),
            Err(message) => {
                for file in staged {
                    let _ = fs::remove_file(file.temporary);
                }
                return Err(message);
            }
        }
    }
    Ok(staged)
}

fn stage_formatted_file(file: &FormattedFile, sequence: usize) -> Result<PathBuf, String> {
    let metadata = fs::metadata(&file.path)
        .map_err(|error| format!("could not inspect {}: {error}", file.path.display()))?;
    let parent = file.path.parent().unwrap_or_else(|| Path::new("."));
    let name = file
        .path
        .file_name()
        .ok_or_else(|| format!("invalid source path {}", file.path.display()))?;

    for attempt in 0..100 {
        let mut temporary_name = std::ffi::OsString::from(".");
        temporary_name.push(name);
        temporary_name.push(format!(
            ".krit-fmt-{}-{sequence}-{attempt}",
            std::process::id()
        ));
        let temporary = parent.join(temporary_name);
        let mut output = match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
        {
            Ok(output) => output,
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => {
                return Err(format!(
                    "could not create formatter output for {}: {error}",
                    file.path.display()
                ));
            }
        };

        let result = output
            .write_all(file.formatted.as_bytes())
            .and_then(|()| output.set_permissions(metadata.permissions()))
            .and_then(|()| output.sync_all());
        if let Err(error) = result {
            drop(output);
            let _ = fs::remove_file(&temporary);
            return Err(format!(
                "could not write formatter output for {}: {error}",
                file.path.display()
            ));
        }
        return Ok(temporary);
    }

    Err(format!(
        "could not allocate formatter output for {}",
        file.path.display()
    ))
}

#[derive(Clone, Copy)]
enum SourceAction {
    Run,
    Check,
}

fn source_command(arguments: &[String], action: SourceAction) -> u8 {
    let (format, positional) = match parse_source_options(arguments) {
        Ok(options) => options,
        Err(message) => {
            eprintln!("krit: {message}");
            return 2;
        }
    };

    if positional.len() != 1 {
        eprintln!("krit: expected exactly one source file");
        return 2;
    }

    let path = Path::new(&positional[0]);
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) => {
            eprintln!("krit: could not read {}: {error}", path.display());
            return 1;
        }
    };
    let source = Source::new(path.to_string_lossy().into_owned(), text);
    if matches!(action, SourceAction::Check) {
        let program = match parse_source(&source) {
            Ok(program) => program,
            Err(diagnostic) => return report(&diagnostic, &source, format),
        };
        let analysis = match analyze(&program) {
            Ok(analysis) => analysis,
            Err(diagnostic) => return report(&diagnostic, &source, format),
        };
        if let Err(error) = lower(&program, &analysis) {
            return internal_error("KICE0001", "lowering Core IR", &error);
        }
        println!("checked {}", path.display());
        0
    } else {
        execute_source(&source, format)
    }
}

fn execute_source(source: &Source, format: DiagnosticFormat) -> u8 {
    let program = match parse_source(source) {
        Ok(program) => program,
        Err(diagnostic) => return report(&diagnostic, source, format),
    };
    if let Some((kind, span)) = program.statements.iter().find_map(|statement| {
        let kind = match statement.kind {
            krit::StatementKind::Webhook { .. } => EntrypointKind::Webhook,
            krit::StatementKind::QueueConsumer { .. } => EntrypointKind::QueueConsumer,
            krit::StatementKind::ScheduleHandler { .. } => EntrypointKind::ScheduleHandler,
            _ => return None,
        };
        Some((kind, statement.span))
    }) {
        let diagnostic = Diagnostic::new(
            "K5003",
            format!(
                "{} entrypoints are unavailable in direct source execution",
                kind.as_str()
            ),
            span,
        );
        let _ = report(&diagnostic, source, format);
        return 4;
    }
    if let Ok(analysis) = analyze(&program)
        && let Some((requirement, span)) = first_analysis_requirement(&analysis)
    {
        let diagnostic = Diagnostic::new(
            "K5003",
            format!(
                "host capability `{}` for resource `{}` is unavailable in direct source execution",
                requirement.capability().as_str(),
                requirement.resource()
            ),
            span,
        );
        let _ = report(&diagnostic, source, format);
        return 4;
    }
    let mut output = io::stdout().lock();
    match execute(&program, &mut output) {
        Ok(_) => 0,
        Err(diagnostic) => {
            let _ = report(&diagnostic, source, format);
            if diagnostic.code() == "K5003" { 4 } else { 1 }
        }
    }
}

fn first_missing_requirement<'a>(
    analysis: &'a Analysis,
    manifest: &Manifest,
) -> Option<(&'a krit::CapabilityRequirement, Span)> {
    analysis.expressions().iter().find_map(|expression| {
        expression
            .requirements()
            .iter()
            .find(|requirement| !manifest_grants_requirement(manifest, requirement))
            .map(|requirement| (requirement, expression.span()))
    })
}

fn first_analysis_requirement(analysis: &Analysis) -> Option<(&krit::CapabilityRequirement, Span)> {
    analysis.expressions().iter().find_map(|expression| {
        expression
            .requirements()
            .iter()
            .next()
            .map(|requirement| (requirement, expression.span()))
    })
}

fn manifest_grants_requirement(
    manifest: &Manifest,
    requirement: &krit::CapabilityRequirement,
) -> bool {
    manifest.grants_permission(
        requirement.capability().as_str(),
        Some(requirement.resource()),
    )
}

fn parse_source_options(arguments: &[String]) -> Result<(DiagnosticFormat, Vec<String>), String> {
    let mut format = DiagnosticFormat::Human;
    let mut positional = Vec::new();
    let mut index = 0;

    while index < arguments.len() {
        match arguments[index].as_str() {
            "--diagnostic-format" => {
                let Some(value) = arguments.get(index + 1) else {
                    return Err("`--diagnostic-format` requires `human` or `json`".to_owned());
                };
                format = parse_diagnostic_format(value)?;
                index += 2;
            }
            argument if argument.starts_with("--diagnostic-format=") => {
                let value = argument
                    .split_once('=')
                    .map(|(_, value)| value)
                    .unwrap_or_default();
                format = parse_diagnostic_format(value)?;
                index += 1;
            }
            "--" => {
                positional.extend_from_slice(&arguments[index + 1..]);
                break;
            }
            argument if argument.starts_with('-') => {
                return Err(format!("unknown option `{argument}`"));
            }
            argument => {
                positional.push(argument.to_owned());
                index += 1;
            }
        }
    }

    Ok((format, positional))
}

fn parse_diagnostic_format(value: &str) -> Result<DiagnosticFormat, String> {
    match value {
        "human" => Ok(DiagnosticFormat::Human),
        "json" => Ok(DiagnosticFormat::Json),
        _ => Err(format!(
            "unknown diagnostic format `{value}`; expected `human` or `json`"
        )),
    }
}

fn report(diagnostic: &Diagnostic, source: &Source, format: DiagnosticFormat) -> u8 {
    match format {
        DiagnosticFormat::Human => eprintln!("{}", diagnostic.render_human(source)),
        DiagnosticFormat::Json => eprintln!("{}", diagnostic.render_json(source)),
    }
    1
}

fn package_command(arguments: &[String]) -> u8 {
    if arguments.first().map(String::as_str) != Some("check") || arguments.len() > 2 {
        eprintln!("krit: usage: krit package check [MANIFEST]");
        return 2;
    }

    let path = arguments
        .get(1)
        .map_or_else(|| PathBuf::from("krit.pkg"), PathBuf::from);
    match Manifest::load(&path) {
        Ok(manifest) => {
            println!("checked {} ({})", manifest.package.name, path.display());
            0
        }
        Err(error) => {
            eprintln!("{}:1:1: error[K6001]: {error}", path.to_string_lossy());
            3
        }
    }
}

fn lsp_command(arguments: &[String]) -> u8 {
    if !arguments.is_empty() {
        eprintln!("krit: `lsp` does not accept arguments");
        return 2;
    }
    match krit_lsp::run_stdio() {
        Ok(()) => 0,
        Err(error) => {
            eprintln!("krit: language server failed: {error}");
            1
        }
    }
}

fn prompt_command(arguments: &[String]) -> u8 {
    if !arguments.is_empty() {
        eprintln!("krit: `prompt` does not accept arguments");
        return 2;
    }
    print!("{GENERATION_PROMPT}");
    0
}

struct InvokeOptions {
    manifest: PathBuf,
    artifact: Option<PathBuf>,
    host_config: Option<PathBuf>,
    request: PathBuf,
}

struct ServeOptions {
    manifest: PathBuf,
    artifact: Option<PathBuf>,
    host_config: Option<PathBuf>,
    bind: SocketAddr,
    once: bool,
}

fn invoke_command(arguments: &[String]) -> u8 {
    let options = match parse_invoke_options(arguments) {
        Ok(options) => options,
        Err(message) => {
            eprintln!("krit: {message}");
            return 2;
        }
    };
    let manifest = match Manifest::load(&options.manifest) {
        Ok(manifest) => manifest,
        Err(error) => {
            eprintln!(
                "{}:1:1: error[K6001]: {error}",
                options.manifest.to_string_lossy()
            );
            return 3;
        }
    };
    let limits = RuntimeLimits::default();
    let artifact_path = options
        .artifact
        .unwrap_or_else(|| default_build_output(&options.manifest, &manifest));
    let artifact = match load_artifact(&artifact_path, limits) {
        Ok(artifact) => artifact,
        Err(message) => return report_artifact_error(&artifact_path, &message),
    };
    let agent_host = match host_config::load(options.host_config.as_deref(), &manifest, limits) {
        Ok(host) => host,
        Err(error) => {
            let code = error.code();
            eprintln!(
                "{}:1:1: error[{code}]: {}",
                options
                    .host_config
                    .as_deref()
                    .unwrap_or_else(|| Path::new("<host-config>"))
                    .to_string_lossy(),
                error.message()
            );
            return if error.kind() == host_config::HostConfigErrorKind::Authorization {
                4
            } else {
                1
            };
        }
    };
    let request_limit = limits
        .request_body_bytes()
        .saturating_add(limits.header_bytes())
        .saturating_add(64 * 1024);
    let request_bytes = match read_bounded(&options.request, request_limit) {
        Ok(bytes) => bytes,
        Err(error) => {
            eprintln!(
                "{}:1:1: error[K7003]: could not read request fixture: {error}",
                options.request.to_string_lossy()
            );
            return 1;
        }
    };
    let request: HttpRequest = match serde_json::from_slice(&request_bytes) {
        Ok(request) => request,
        Err(error) => {
            eprintln!(
                "{}:1:1: error[K4001]: invalid strict request fixture JSON: {error}",
                options.request.to_string_lossy()
            );
            return 1;
        }
    };
    let runtime = match Runtime::new(limits) {
        Ok(runtime) => runtime,
        Err(error) => return report_runtime_error(&artifact_path, &error),
    };
    let result = match runtime.invoke_webhook_with_host(
        &artifact.bytes,
        &artifact.metadata,
        &GrantSet::from_manifest(&manifest),
        &agent_host,
        request,
    ) {
        Ok(result) => result,
        Err(error) => return report_runtime_error(&artifact_path, &error),
    };
    if let Err(error) = publish_logs(&result.events, "success") {
        eprintln!("krit: error[K4007]: could not publish structured logs: {error}");
        return 1;
    }
    let mut response = match serde_json::to_vec(&result.response) {
        Ok(response) => response,
        Err(error) => return internal_error("KICE0003", "serializing webhook response", &error),
    };
    response.push(b'\n');
    if let Err(error) = write_buffered_output(&mut io::stdout().lock(), &response) {
        eprintln!("krit: error[K4007]: could not publish webhook response: {error}");
        return 1;
    }
    0
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum DeliveryKind {
    Queue,
    Schedule,
}

impl DeliveryKind {
    const fn command(self) -> &'static str {
        match self {
            Self::Queue => "worker",
            Self::Schedule => "schedule",
        }
    }

    const fn option(self) -> &'static str {
        match self {
            Self::Queue => "--queue",
            Self::Schedule => "--schedule",
        }
    }
}

struct DeliveryOptions {
    manifest: PathBuf,
    artifact: Option<PathBuf>,
    host_config: Option<PathBuf>,
    resource: String,
    max_deliveries: u32,
    now_millis: Option<i64>,
    json: bool,
}

/// Hard bound on deliveries one bounded dispatch invocation may process.
const MAX_DELIVERIES_PER_RUN: u32 = 1024;

/// Schema-1 delivery report.
///
/// `outcomes` and `outputs` are parallel arrays in dispatch order: `outputs[i]`
/// is the bounded standard output the guest produced for `outcomes[i]`. In JSON
/// mode standard output carries this document and nothing else, so a worker or
/// scheduler artifact that also holds `io.stdout` cannot corrupt the stream.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct DeliveryReport {
    schema: u32,
    resource: String,
    now_millis: i64,
    dispatched: u32,
    completed: u32,
    retried: u32,
    dead_lettered: u32,
    idle: bool,
    /// Set when dispatch stopped early because the collected guest output
    /// reached the runtime output budget.
    stopped_for_output_budget: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    materialized: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    skipped: Option<u64>,
    outcomes: Vec<DeliveryOutcome>,
    outputs: Vec<String>,
}

fn delivery_command(arguments: &[String], kind: DeliveryKind) -> u8 {
    let options = match parse_delivery_options(arguments, kind) {
        Ok(options) => options,
        Err(message) => {
            eprintln!("krit: {message}");
            return 2;
        }
    };
    let manifest = match Manifest::load(&options.manifest) {
        Ok(manifest) => manifest,
        Err(error) => {
            eprintln!(
                "{}:1:1: error[K6001]: {error}",
                options.manifest.to_string_lossy()
            );
            return 3;
        }
    };
    let limits = RuntimeLimits::default();
    let artifact_path = options
        .artifact
        .clone()
        .unwrap_or_else(|| default_build_output(&options.manifest, &manifest));
    let artifact = match load_artifact(&artifact_path, limits) {
        Ok(artifact) => artifact,
        Err(message) => return report_artifact_error(&artifact_path, &message),
    };
    let agent_host = match host_config::load(options.host_config.as_deref(), &manifest, limits) {
        Ok(host) => host,
        Err(error) => {
            let authorization = error.kind() == host_config::HostConfigErrorKind::Authorization;
            eprintln!(
                "{}:1:1: error[{}]: {}",
                options
                    .host_config
                    .as_deref()
                    .unwrap_or_else(|| Path::new("<host-config>"))
                    .to_string_lossy(),
                error.code(),
                error.message()
            );
            return if authorization { 4 } else { 1 };
        }
    };
    let now_millis = match options.now_millis {
        Some(now) => now,
        None => match host_wall_clock_millis() {
            Ok(now) => now,
            Err(message) => {
                eprintln!("krit: error[K7003]: {message}");
                return 1;
            }
        },
    };
    let runtime = match Runtime::new(limits) {
        Ok(runtime) => runtime,
        Err(error) => return report_runtime_error(&artifact_path, &error),
    };
    let grants = GrantSet::from_manifest(&manifest);
    let cancellation = CancellationHandle::new();
    let dispatch = krit_runtime::DeliveryRequest {
        bytes: &artifact.bytes,
        metadata: &artifact.metadata,
        grants: &grants,
        agent_host: &agent_host,
        resource: &options.resource,
        now_millis,
        cancellation: &cancellation,
    };
    let mut report = DeliveryReport {
        schema: 1,
        resource: options.resource.clone(),
        now_millis,
        dispatched: 0,
        completed: 0,
        retried: 0,
        dead_lettered: 0,
        idle: true,
        stopped_for_output_budget: false,
        materialized: None,
        skipped: None,
        outcomes: Vec::new(),
        outputs: Vec::new(),
    };
    let output_budget = limits.output_bytes();
    let mut collected_output_bytes = 0usize;
    for _ in 0..options.max_deliveries {
        let (catch_up, result) = match kind {
            DeliveryKind::Queue => (None, runtime.dispatch_job(dispatch)),
            DeliveryKind::Schedule => match runtime.dispatch_schedule(dispatch) {
                Ok((catch_up, result)) => (Some(catch_up), Ok(result)),
                Err(error) => (None, Err(error)),
            },
        };
        if let Some(catch_up) = catch_up {
            report.materialized = Some(report.materialized.unwrap_or(0) + catch_up.materialized);
            report.skipped = Some(report.skipped.unwrap_or(0) + catch_up.skipped);
        }
        let result = match result {
            Ok(result) => result,
            Err(error) => {
                let _ = publish_logs(error.events(), "failure");
                return report_runtime_error(&artifact_path, &error);
            }
        };
        if let Err(error) = publish_logs(&result.events, "success") {
            eprintln!("krit: error[K4007]: could not publish structured logs: {error}");
            return 1;
        }
        if result.outcome.is_idle() {
            break;
        }
        report.idle = false;
        report.dispatched += 1;
        match &result.outcome {
            DeliveryOutcome::Completed { .. } => report.completed += 1,
            DeliveryOutcome::Retried { .. } => report.retried += 1,
            DeliveryOutcome::DeadLettered { .. } => report.dead_lettered += 1,
            DeliveryOutcome::Idle => {}
        }
        report.outcomes.push(result.outcome);
        if options.json {
            let output = match String::from_utf8(result.output) {
                Ok(output) => output,
                Err(_) => {
                    eprintln!(
                        "krit: error[K4007]: guest output is not UTF-8 and cannot be reported as JSON"
                    );
                    return 1;
                }
            };
            collected_output_bytes = collected_output_bytes.saturating_add(output.len());
            report.outputs.push(output);
            if collected_output_bytes >= output_budget {
                report.stopped_for_output_budget = true;
                break;
            }
        } else if !result.output.is_empty()
            && let Err(error) = write_buffered_output(&mut io::stdout().lock(), &result.output)
        {
            eprintln!("krit: error[K4007]: could not publish guest output: {error}");
            return 1;
        }
    }
    if options.json {
        let mut rendered = match serde_json::to_vec(&report) {
            Ok(rendered) => rendered,
            Err(error) => return internal_error("KICE0003", "serializing delivery report", &error),
        };
        rendered.push(b'\n');
        if let Err(error) = write_buffered_output(&mut io::stdout().lock(), &rendered) {
            eprintln!("krit: error[K4007]: could not publish delivery report: {error}");
            return 1;
        }
    } else {
        println!(
            "{} {}: dispatched {} completed {} retried {} dead-lettered {}",
            kind.command(),
            report.resource,
            report.dispatched,
            report.completed,
            report.retried,
            report.dead_lettered
        );
    }
    0
}

fn host_wall_clock_millis() -> Result<i64, String> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|_| "host wall clock is before the UNIX epoch".to_owned())?;
    i64::try_from(now.as_millis()).map_err(|_| "host wall clock exceeds i64".to_owned())
}

fn parse_delivery_options(
    arguments: &[String],
    kind: DeliveryKind,
) -> Result<DeliveryOptions, String> {
    let mut manifest = PathBuf::from("krit.pkg");
    let mut artifact = None;
    let mut host_config = None;
    let mut resource = None;
    let mut max_deliveries = None;
    let mut now_millis = None;
    let mut json = false;
    let resource_option = kind.option();
    let resource_assignment = format!("{resource_option}=");
    let mut index = 0;
    while index < arguments.len() {
        let argument = arguments[index].as_str();
        match argument {
            "--manifest" => {
                manifest = option_path(arguments, index, "--manifest")?;
                index += 2;
            }
            argument if argument.starts_with("--manifest=") => {
                manifest = assigned_path(argument, "--manifest")?;
                index += 1;
            }
            "--artifact" => {
                artifact = Some(option_path(arguments, index, "--artifact")?);
                index += 2;
            }
            argument if argument.starts_with("--artifact=") => {
                artifact = Some(assigned_path(argument, "--artifact")?);
                index += 1;
            }
            "--host-config" => {
                host_config = Some(option_path(arguments, index, "--host-config")?);
                index += 2;
            }
            argument if argument.starts_with("--host-config=") => {
                host_config = Some(assigned_path(argument, "--host-config")?);
                index += 1;
            }
            argument if argument == resource_option => {
                resource = Some(option_text(arguments, index, resource_option)?);
                index += 2;
            }
            argument if argument.starts_with(&resource_assignment) => {
                resource = Some(assigned_text(argument, resource_option)?);
                index += 1;
            }
            "--once" => {
                max_deliveries = Some(1);
                index += 1;
            }
            "--max-deliveries" => {
                max_deliveries = Some(parse_count(&option_text(
                    arguments,
                    index,
                    "--max-deliveries",
                )?)?);
                index += 2;
            }
            argument if argument.starts_with("--max-deliveries=") => {
                max_deliveries = Some(parse_count(&assigned_text(argument, "--max-deliveries")?)?);
                index += 1;
            }
            "--now" => {
                now_millis = Some(parse_instant(&option_text(arguments, index, "--now")?)?);
                index += 2;
            }
            argument if argument.starts_with("--now=") => {
                now_millis = Some(parse_instant(&assigned_text(argument, "--now")?)?);
                index += 1;
            }
            "--json" => {
                json = true;
                index += 1;
            }
            argument if argument.starts_with('-') => {
                return Err(format!("unknown option `{argument}`"));
            }
            argument => {
                return Err(format!(
                    "unexpected {} argument `{argument}`",
                    kind.command()
                ));
            }
        }
    }
    Ok(DeliveryOptions {
        manifest,
        artifact,
        host_config,
        resource: resource
            .ok_or_else(|| format!("`{}` requires `{resource_option} NAME`", kind.command()))?,
        max_deliveries: max_deliveries.unwrap_or(1),
        now_millis,
        json,
    })
}

fn parse_count(value: &str) -> Result<u32, String> {
    let count: u32 = value
        .parse()
        .map_err(|_| "`--max-deliveries` requires a non-negative integer".to_owned())?;
    if count == 0 || count > MAX_DELIVERIES_PER_RUN {
        return Err(format!(
            "`--max-deliveries` must be between 1 and {MAX_DELIVERIES_PER_RUN}"
        ));
    }
    Ok(count)
}

fn parse_instant(value: &str) -> Result<i64, String> {
    let millis: i64 = value
        .parse()
        .map_err(|_| "`--now` requires UTC epoch milliseconds".to_owned())?;
    if millis < 0 {
        return Err("`--now` must not precede the UNIX epoch".to_owned());
    }
    Ok(millis)
}

fn option_text(arguments: &[String], index: usize, option: &str) -> Result<String, String> {
    arguments
        .get(index + 1)
        .filter(|value| !value.is_empty() && !value.starts_with('-'))
        .cloned()
        .ok_or_else(|| format!("`{option}` requires a value"))
}

fn assigned_text(argument: &str, option: &str) -> Result<String, String> {
    let value = argument
        .split_once('=')
        .map(|(_, value)| value)
        .unwrap_or_default();
    if value.is_empty() {
        Err(format!("`{option}` requires a value"))
    } else {
        Ok(value.to_owned())
    }
}

fn serve_command(arguments: &[String]) -> u8 {
    let options = match parse_serve_options(arguments) {
        Ok(options) => options,
        Err(message) => {
            eprintln!("krit: {message}");
            return 2;
        }
    };
    let manifest = match Manifest::load(&options.manifest) {
        Ok(manifest) => manifest,
        Err(error) => {
            eprintln!(
                "{}:1:1: error[K6001]: {error}",
                options.manifest.to_string_lossy()
            );
            return 3;
        }
    };
    let limits = RuntimeLimits::default();
    let artifact_path = options
        .artifact
        .unwrap_or_else(|| default_build_output(&options.manifest, &manifest));
    let artifact = match load_artifact(&artifact_path, limits) {
        Ok(artifact) => artifact,
        Err(message) => return report_artifact_error(&artifact_path, &message),
    };
    let agent_host = match host_config::load(options.host_config.as_deref(), &manifest, limits) {
        Ok(host) => host,
        Err(error) => {
            let authorization = error.kind() == host_config::HostConfigErrorKind::Authorization;
            let code = error.code();
            eprintln!(
                "{}:1:1: error[{code}]: {}",
                options
                    .host_config
                    .as_deref()
                    .unwrap_or_else(|| Path::new("<host-config>"))
                    .to_string_lossy(),
                error.message()
            );
            return if authorization { 4 } else { 1 };
        }
    };
    let runtime = match Runtime::new(limits) {
        Ok(runtime) => runtime,
        Err(error) => return report_runtime_error(&artifact_path, &error),
    };
    let grants = GrantSet::from_manifest(&manifest);
    let effective = match runtime.permissions(&artifact.bytes, &artifact.metadata, &grants) {
        Ok(effective) => effective,
        Err(error) => return report_runtime_error(&artifact_path, &error),
    };
    if !effective.allowed() {
        eprintln!(
            "{}:1:1: error[K5001]: artifact requirements are not granted by the manifest",
            artifact_path.to_string_lossy()
        );
        return 4;
    }
    if !options.bind.ip().is_loopback() {
        eprintln!("krit: `serve --bind` must use a loopback address");
        return 2;
    }
    let server = match tiny_http::Server::http(options.bind) {
        Ok(server) => server,
        Err(error) => {
            eprintln!("krit: error[K7003]: could not bind HTTP server: {error}");
            return 1;
        }
    };
    eprintln!("krit serve listening on http://{}", server.server_addr());
    loop {
        let mut incoming = match server.recv() {
            Ok(request) => request,
            Err(error) => {
                eprintln!("krit: error[K7003]: HTTP server failed: {error}");
                return 1;
            }
        };
        let request = match read_inbound_request(&mut incoming, limits) {
            Ok(request) => request,
            Err(error) => {
                if let Err(write_error) = respond_error(incoming, error.status, &error.message) {
                    eprintln!(
                        "krit: error[K4007]: could not publish rejected HTTP response: {write_error}"
                    );
                    return 1;
                }
                if options.once {
                    return 0;
                }
                continue;
            }
        };
        let result = runtime.invoke_webhook_with_host(
            &artifact.bytes,
            &artifact.metadata,
            &grants,
            &agent_host,
            request,
        );
        match result {
            Ok(result) => {
                if let Err(error) = publish_logs(&result.events, "success") {
                    eprintln!("krit: error[K4007]: could not publish structured logs: {error}");
                    return 1;
                }
                let mut response =
                    tiny_http::Response::from_data(result.response.body.into_bytes())
                        .with_status_code(result.response.status as u16);
                for header in result.response.headers {
                    let header = match tiny_http::Header::from_bytes(
                        header.name.as_bytes(),
                        header.value.as_bytes(),
                    ) {
                        Ok(header) => header,
                        Err(()) => {
                            eprintln!(
                                "krit: error[K4001]: validated guest response header could not be encoded"
                            );
                            return 1;
                        }
                    };
                    response.add_header(header);
                }
                if let Err(error) = incoming.respond(response) {
                    eprintln!("krit: error[K4007]: could not publish HTTP response: {error}");
                    return 1;
                }
                if let Err(error) = write_buffered_output(&mut io::stdout().lock(), &result.output)
                {
                    eprintln!("krit: error[K4007]: could not publish guest output: {error}");
                    return 1;
                }
            }
            Err(error) => {
                if let Err(write_error) = respond_error(incoming, 500, "webhook invocation failed")
                {
                    eprintln!(
                        "krit: error[K4007]: could not publish invocation failure response: {write_error}"
                    );
                    return 1;
                }
                let exit = report_runtime_error(&artifact_path, &error);
                if options.once {
                    return exit;
                }
            }
        }
        if options.once {
            return 0;
        }
    }
}

struct InboundError {
    status: u16,
    message: String,
}

fn read_inbound_request(
    request: &mut tiny_http::Request,
    limits: RuntimeLimits,
) -> Result<HttpRequest, InboundError> {
    if request.headers().len() > limits.header_count() {
        return Err(InboundError {
            status: 431,
            message: "too many request headers".to_owned(),
        });
    }
    let header_bytes = request.headers().iter().try_fold(0usize, |total, header| {
        total
            .checked_add(header.field.as_str().as_str().len())
            .and_then(|value| value.checked_add(header.value.as_str().len()))
    });
    if header_bytes.is_none_or(|bytes| bytes > limits.header_bytes()) {
        return Err(InboundError {
            status: 431,
            message: "request headers are too large".to_owned(),
        });
    }
    if request
        .body_length()
        .is_some_and(|length| length > limits.request_body_bytes())
    {
        return Err(InboundError {
            status: 413,
            message: "request body is too large".to_owned(),
        });
    }
    let mut body = Vec::new();
    request
        .as_reader()
        .take(limits.request_body_bytes().saturating_add(1) as u64)
        .read_to_end(&mut body)
        .map_err(|_| InboundError {
            status: 400,
            message: "could not read request body".to_owned(),
        })?;
    if body.len() > limits.request_body_bytes() {
        return Err(InboundError {
            status: 413,
            message: "request body is too large".to_owned(),
        });
    }
    let body = String::from_utf8(body).map_err(|_| InboundError {
        status: 400,
        message: "request body must be valid UTF-8".to_owned(),
    })?;
    let (path, query) = request
        .url()
        .split_once('?')
        .map_or((request.url(), ""), |(path, query)| (path, query));
    let headers = request
        .headers()
        .iter()
        .filter(|header| {
            !matches!(
                header.field.as_str().as_str().to_ascii_lowercase().as_str(),
                "connection"
                    | "content-length"
                    | "host"
                    | "keep-alive"
                    | "proxy-connection"
                    | "te"
                    | "trailer"
                    | "transfer-encoding"
                    | "upgrade"
            )
        })
        .map(|header| HttpHeader {
            name: header.field.as_str().as_str().to_owned(),
            value: header.value.as_str().to_owned(),
        })
        .collect();
    Ok(HttpRequest {
        method: request.method().as_str().to_owned(),
        path: path.to_owned(),
        query: query.to_owned(),
        headers,
        body,
    })
}

fn respond_error(request: tiny_http::Request, status: u16, message: &str) -> io::Result<()> {
    request.respond(tiny_http::Response::from_string(message).with_status_code(status))
}

fn parse_invoke_options(arguments: &[String]) -> Result<InvokeOptions, String> {
    let mut manifest = PathBuf::from("krit.pkg");
    let mut artifact = None;
    let mut host_config = None;
    let mut request = None;
    let mut index = 0;
    while index < arguments.len() {
        match arguments[index].as_str() {
            "--manifest" => {
                manifest = option_path(arguments, index, "--manifest")?;
                index += 2;
            }
            argument if argument.starts_with("--manifest=") => {
                manifest = assigned_path(argument, "--manifest")?;
                index += 1;
            }
            "--artifact" => {
                artifact = Some(option_path(arguments, index, "--artifact")?);
                index += 2;
            }
            argument if argument.starts_with("--artifact=") => {
                artifact = Some(assigned_path(argument, "--artifact")?);
                index += 1;
            }
            "--host-config" => {
                host_config = Some(option_path(arguments, index, "--host-config")?);
                index += 2;
            }
            argument if argument.starts_with("--host-config=") => {
                host_config = Some(assigned_path(argument, "--host-config")?);
                index += 1;
            }
            "--request" => {
                request = Some(option_path(arguments, index, "--request")?);
                index += 2;
            }
            argument if argument.starts_with("--request=") => {
                request = Some(assigned_path(argument, "--request")?);
                index += 1;
            }
            argument if argument.starts_with('-') => {
                return Err(format!("unknown option `{argument}`"));
            }
            argument => return Err(format!("unexpected invoke argument `{argument}`")),
        }
    }
    Ok(InvokeOptions {
        manifest,
        artifact,
        host_config,
        request: request.ok_or_else(|| "`invoke` requires `--request FILE`".to_owned())?,
    })
}

fn parse_serve_options(arguments: &[String]) -> Result<ServeOptions, String> {
    let mut manifest = PathBuf::from("krit.pkg");
    let mut artifact = None;
    let mut host_config = None;
    let mut bind = "127.0.0.1:3000"
        .parse()
        .expect("default loopback bind address should parse");
    let mut once = false;
    let mut index = 0;
    while index < arguments.len() {
        match arguments[index].as_str() {
            "--manifest" => {
                manifest = option_path(arguments, index, "--manifest")?;
                index += 2;
            }
            argument if argument.starts_with("--manifest=") => {
                manifest = assigned_path(argument, "--manifest")?;
                index += 1;
            }
            "--artifact" => {
                artifact = Some(option_path(arguments, index, "--artifact")?);
                index += 2;
            }
            argument if argument.starts_with("--artifact=") => {
                artifact = Some(assigned_path(argument, "--artifact")?);
                index += 1;
            }
            "--host-config" => {
                host_config = Some(option_path(arguments, index, "--host-config")?);
                index += 2;
            }
            argument if argument.starts_with("--host-config=") => {
                host_config = Some(assigned_path(argument, "--host-config")?);
                index += 1;
            }
            "--bind" => {
                let value = arguments
                    .get(index + 1)
                    .ok_or_else(|| "`--bind` requires a socket address".to_owned())?;
                bind = value
                    .parse()
                    .map_err(|_| "`--bind` requires an IP socket address".to_owned())?;
                index += 2;
            }
            argument if argument.starts_with("--bind=") => {
                let value = argument.split_once('=').map_or("", |(_, value)| value);
                bind = value
                    .parse()
                    .map_err(|_| "`--bind` requires an IP socket address".to_owned())?;
                index += 1;
            }
            "--once" => {
                once = true;
                index += 1;
            }
            argument if argument.starts_with('-') => {
                return Err(format!("unknown option `{argument}`"));
            }
            argument => return Err(format!("unexpected serve argument `{argument}`")),
        }
    }
    Ok(ServeOptions {
        manifest,
        artifact,
        host_config,
        bind,
        once,
    })
}

fn sandbox_command(arguments: &[String]) -> u8 {
    let (manifest_path, requested_artifact) = match parse_sandbox_options(arguments) {
        Ok(options) => options,
        Err(message) => {
            eprintln!("krit: {message}");
            return 2;
        }
    };
    let manifest = match Manifest::load(&manifest_path) {
        Ok(manifest) => manifest,
        Err(error) => {
            eprintln!(
                "{}:1:1: error[K6001]: {error}",
                manifest_path.to_string_lossy()
            );
            return 3;
        }
    };
    let artifact_path =
        requested_artifact.unwrap_or_else(|| default_build_output(&manifest_path, &manifest));
    let limits = RuntimeLimits::default();
    let artifact = match load_artifact(&artifact_path, limits) {
        Ok(artifact) => artifact,
        Err(message) => return report_artifact_error(&artifact_path, &message),
    };
    let runtime = match Runtime::new(limits) {
        Ok(runtime) => runtime,
        Err(error) => return report_runtime_error(&artifact_path, &error),
    };
    let result = match runtime.execute(
        &artifact.bytes,
        &artifact.metadata,
        &GrantSet::from_manifest(&manifest),
    ) {
        Ok(result) => result,
        Err(error) => return report_runtime_error(&artifact_path, &error),
    };
    let mut stdout = io::stdout().lock();
    if let Err(error) = write_buffered_output(&mut stdout, &result.output) {
        eprintln!("krit: error[K4007]: could not write buffered sandbox output: {error}");
        return 1;
    }
    0
}

fn write_buffered_output(output: &mut impl Write, bytes: &[u8]) -> io::Result<()> {
    output.write_all(bytes)?;
    output.flush()
}

fn parse_sandbox_options(arguments: &[String]) -> Result<(PathBuf, Option<PathBuf>), String> {
    let mut manifest = PathBuf::from("krit.pkg");
    let mut artifact = None;
    let mut index = 0;
    while index < arguments.len() {
        match arguments[index].as_str() {
            "--manifest" => {
                manifest = option_path(arguments, index, "--manifest")?;
                index += 2;
            }
            argument if argument.starts_with("--manifest=") => {
                manifest = assigned_path(argument, "--manifest")?;
                index += 1;
            }
            "--artifact" => {
                if artifact.is_some() {
                    return Err("`--artifact` may be specified only once".to_owned());
                }
                artifact = Some(option_path(arguments, index, "--artifact")?);
                index += 2;
            }
            argument if argument.starts_with("--artifact=") => {
                if artifact.is_some() {
                    return Err("`--artifact` may be specified only once".to_owned());
                }
                artifact = Some(assigned_path(argument, "--artifact")?);
                index += 1;
            }
            argument if argument.starts_with('-') => {
                return Err(format!("unknown option `{argument}`"));
            }
            argument => return Err(format!("unexpected sandbox argument `{argument}`")),
        }
    }
    Ok((manifest, artifact))
}

fn permissions_command(arguments: &[String]) -> u8 {
    let (json, artifact_path, path) = match parse_permissions_options(arguments) {
        Ok(options) => options,
        Err(message) => {
            eprintln!("krit: {message}");
            return 2;
        }
    };
    match Manifest::load(&path) {
        Ok(manifest) => {
            if let Some(artifact_path) = artifact_path {
                let limits = RuntimeLimits::default();
                let artifact = match load_artifact(&artifact_path, limits) {
                    Ok(artifact) => artifact,
                    Err(message) => return report_artifact_error(&artifact_path, &message),
                };
                let runtime = match Runtime::new(limits) {
                    Ok(runtime) => runtime,
                    Err(error) => return report_runtime_error(&artifact_path, &error),
                };
                let effective = match runtime.permissions(
                    &artifact.bytes,
                    &artifact.metadata,
                    &GrantSet::from_manifest(&manifest),
                ) {
                    Ok(effective) => effective,
                    Err(error) => return report_runtime_error(&artifact_path, &error),
                };
                if json {
                    println!("{}", effective.render_json());
                } else {
                    print!("{}", effective.render_human());
                }
                return if effective.allowed() { 0 } else { 4 };
            }
            let plan = manifest.permission_plan();
            if json {
                println!("{}", plan.render_json());
            } else {
                print!("{}", plan.render_human());
            }
            0
        }
        Err(error) => {
            eprintln!("{}:1:1: error[K6001]: {error}", path.to_string_lossy());
            3
        }
    }
}

fn parse_permissions_options(
    arguments: &[String],
) -> Result<(bool, Option<PathBuf>, PathBuf), String> {
    let mut json = false;
    let mut artifact = None;
    let mut manifest = None;
    let mut index = 0;
    while index < arguments.len() {
        match arguments[index].as_str() {
            "--json" => {
                json = true;
                index += 1;
            }
            "--artifact" => {
                if artifact.is_some() {
                    return Err("`--artifact` may be specified only once".to_owned());
                }
                artifact = Some(option_path(arguments, index, "--artifact")?);
                index += 2;
            }
            argument if argument.starts_with("--artifact=") => {
                if artifact.is_some() {
                    return Err("`--artifact` may be specified only once".to_owned());
                }
                artifact = Some(assigned_path(argument, "--artifact")?);
                index += 1;
            }
            argument if argument.starts_with('-') => {
                return Err(format!("unknown option `{argument}`"));
            }
            argument if manifest.is_none() => {
                manifest = Some(PathBuf::from(argument));
                index += 1;
            }
            _ => return Err("expected at most one manifest path".to_owned()),
        }
    }
    Ok((
        json,
        artifact,
        manifest.unwrap_or_else(|| PathBuf::from("krit.pkg")),
    ))
}

fn option_path(arguments: &[String], index: usize, option: &str) -> Result<PathBuf, String> {
    arguments
        .get(index + 1)
        .filter(|value| !value.is_empty() && !value.starts_with('-'))
        .map(PathBuf::from)
        .ok_or_else(|| format!("`{option}` requires a path"))
}

fn assigned_path(argument: &str, option: &str) -> Result<PathBuf, String> {
    let value = argument
        .split_once('=')
        .map(|(_, value)| value)
        .unwrap_or_default();
    if value.is_empty() {
        Err(format!("`{option}` requires a path"))
    } else {
        Ok(PathBuf::from(value))
    }
}

struct LoadedArtifact {
    bytes: Vec<u8>,
    metadata: ArtifactMetadata,
}

fn load_artifact(path: &Path, limits: RuntimeLimits) -> Result<LoadedArtifact, String> {
    let bytes = read_bounded(path, limits.component_bytes()).map_err(|error| {
        format!(
            "could not read WebAssembly artifact {}; run `krit build` first or pass `--artifact PATH`: {error}",
            path.display()
        )
    })?;
    let sidecar = metadata_path(path);
    let metadata_bytes = read_bounded(&sidecar, limits.metadata_bytes()).map_err(|error| {
        format!(
            "could not read adjacent artifact metadata {}; run `krit build` to create the component and sidecar together: {error}",
            sidecar.display()
        )
    })?;
    let metadata = serde_json::from_slice(&metadata_bytes).map_err(|error| {
        format!(
            "could not deserialize artifact metadata {} using schema 1: {error}",
            sidecar.display()
        )
    })?;
    Ok(LoadedArtifact { bytes, metadata })
}

fn read_bounded(path: &Path, limit: usize) -> Result<Vec<u8>, String> {
    let file = fs::File::open(path).map_err(|error| error.to_string())?;
    let mut bytes = Vec::new();
    file.take(limit.saturating_add(1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|error| error.to_string())?;
    if bytes.len() > limit {
        return Err(format!("file exceeds the {limit}-byte host input limit"));
    }
    Ok(bytes)
}

fn report_artifact_error(path: &Path, message: &str) -> u8 {
    eprintln!("{}:1:1: error[K7003]: {message}", path.to_string_lossy());
    1
}

fn report_runtime_error(path: &Path, error: &RuntimeError) -> u8 {
    if let Err(write_error) = publish_logs(error.events(), "failure") {
        eprintln!("krit: error[K4007]: could not publish structured logs: {write_error}");
        return 1;
    }
    eprintln!(
        "{}:1:1: error[{}]: {}",
        path.to_string_lossy(),
        error.code(),
        error.message()
    );
    if matches!(
        error.kind(),
        RuntimeErrorKind::Authorization | RuntimeErrorKind::ImportMismatch
    ) {
        4
    } else {
        1
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PublishedLogEvent<'a> {
    schema: u32,
    sequence: u64,
    level: LogLevel,
    event: &'a str,
    fields: &'a [LogField],
    outcome: &'a str,
}

fn publish_logs(events: &[LogEvent], outcome: &str) -> io::Result<()> {
    let mut stderr = io::stderr().lock();
    for event in events {
        let published = PublishedLogEvent {
            schema: 1,
            sequence: event.sequence,
            level: event.level,
            event: &event.event,
            fields: &event.fields,
            outcome,
        };
        serde_json::to_writer(&mut stderr, &published).map_err(io::Error::other)?;
        stderr.write_all(b"\n")?;
    }
    stderr.flush()
}

fn print_help() {
    println!(
        "\
Krit {VERSION}
An open, human-auditable language for the age of AI.

USAGE:
    krit assist inspect --provider-config PATH --manifest PATH --file FILE --range RANGE --intent TEXT [--kind completion|repair|cleanup] [--context FILE@RANGE] [--json]
    krit assist suggest --provider-config PATH --manifest PATH --file FILE --range RANGE --intent TEXT --proposal PATH.json [--kind completion|repair|cleanup] [--context FILE@RANGE] [--json]
    krit assist review --manifest PATH --proposal PATH.json [--json]
    krit assist accept --manifest PATH --proposal PATH.json --reviewed [--approve-permission CAPABILITY[=RESOURCE]] [--json]
    krit run [--diagnostic-format human|json] FILE
    krit check [--diagnostic-format human|json] FILE
    krit build [--manifest PATH] [--output PATH]
    krit explain [--json] FILE
    krit fmt [--check] FILE...
    krit lsp
    krit prompt
    krit permissions [--artifact PATH] [--json] [MANIFEST]
    krit sandbox [--manifest PATH] [--artifact PATH]
    krit invoke [--manifest PATH] [--artifact PATH] [--host-config PATH] --request FILE
    krit worker --queue NAME [--manifest PATH] [--artifact PATH] [--host-config PATH] [--once] [--max-deliveries N] [--now EPOCH_MILLIS] [--json]
    krit schedule --schedule NAME [--manifest PATH] [--artifact PATH] [--host-config PATH] [--once] [--max-deliveries N] [--now EPOCH_MILLIS] [--json]
    krit serve [--manifest PATH] [--artifact PATH] [--host-config PATH] [--bind IP:PORT] [--once]
    krit package check [MANIFEST]
    krit --version
    krit --help"
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_json_diagnostic_option() {
        let arguments = vec![
            "--diagnostic-format=json".to_owned(),
            "sample.krit".to_owned(),
        ];
        let (format, positional) = parse_source_options(&arguments).expect("options should parse");
        assert!(matches!(format, DiagnosticFormat::Json));
        assert_eq!(positional, ["sample.krit"]);
    }

    #[test]
    fn rejects_unknown_options() {
        let error =
            parse_source_options(&["--quiet".to_owned()]).expect_err("unknown option should fail");
        assert!(error.contains("unknown option"));
    }

    #[test]
    fn parses_formatter_options() {
        let (check, paths) = parse_fmt_options(&[
            "--check".to_owned(),
            "one.krit".to_owned(),
            "--".to_owned(),
            "-two.krit".to_owned(),
        ])
        .expect("options should parse");
        assert!(check);
        assert_eq!(
            paths,
            [PathBuf::from("one.krit"), PathBuf::from("-two.krit")]
        );
    }

    #[test]
    fn reports_buffered_output_write_failures() {
        struct FailedOutput;

        impl Write for FailedOutput {
            fn write(&mut self, _bytes: &[u8]) -> io::Result<usize> {
                Err(io::Error::other("closed output"))
            }

            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }

        let error = write_buffered_output(&mut FailedOutput, b"buffered")
            .expect_err("failed stdout should be reported");
        assert_eq!(error.kind(), io::ErrorKind::Other);
    }

    #[test]
    fn checks_every_prompt_example() {
        let mut remaining = GENERATION_PROMPT;
        let mut count = 0;
        while let Some(start) = remaining.find("```krit\n") {
            let code = &remaining[start + "```krit\n".len()..];
            let end = code.find("\n```").expect("Krit code fence should close");
            let source = Source::new(format!("<prompt-example-{count}>"), &code[..end]);
            let program = parse_source(&source).expect("prompt example should parse");
            analyze(&program).expect("prompt example should pass semantic analysis");
            let formatted = format_source(&source).expect("prompt example should format");
            assert_eq!(
                formatted,
                format!("{}\n", &code[..end]),
                "prompt example should be canonical"
            );
            let formatted_source = Source::new(
                format!("<formatted-prompt-example-{count}>"),
                formatted.clone(),
            );
            let program =
                parse_source(&formatted_source).expect("formatted prompt example should parse");
            analyze(&program).expect("formatted prompt example should pass semantic analysis");
            assert_eq!(
                format_source(&formatted_source).expect("formatting should be idempotent"),
                formatted
            );
            count += 1;
            remaining = &code[end + "\n```".len()..];
        }
        assert_eq!(count, 12, "prompt should contain twelve canonical examples");
    }
}
