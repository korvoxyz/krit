use std::{
    env, fs,
    fs::OpenOptions,
    io::{self, Write},
    path::{Path, PathBuf},
    process::ExitCode,
};

use krit::{
    Analysis, CoreModule, Diagnostic, Source, Span, analyze, format_source, lower, parse_source,
    run_source,
};
use krit_package::Manifest;
use krit_wasm::{BuildErrorKind, BuildOptions, build_component};
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
        "explain" => explain_command(&arguments[1..]),
        "fmt" => fmt_command(&arguments[1..]),
        "prompt" => prompt_command(&arguments[1..]),
        "permissions" => permissions_command(&arguments[1..]),
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
    bindings: Vec<JsonBinding>,
    core: String,
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
        render_explanation_json(&analysis, &module)
    } else {
        render_explanation_human(&analysis, &module);
        0
    }
}

fn render_explanation_human(analysis: &Analysis, module: &CoreModule) {
    let entrypoint = module.entrypoint_function();
    println!("Krit explanation (schema 1)");
    println!(
        "entrypoint: {} {}",
        module.entrypoints()[0].kind.as_str(),
        entrypoint.id
    );
    println!("result: {}", entrypoint.signature.result);
    println!("effects: {}", entrypoint.signature.effects);
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

fn render_explanation_json(analysis: &Analysis, module: &CoreModule) -> u8 {
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
    let mut output = io::stdout().lock();
    match run_source(source, &mut output) {
        Ok(_) => 0,
        Err(diagnostic) => report(&diagnostic, source, format),
    }
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

fn prompt_command(arguments: &[String]) -> u8 {
    if !arguments.is_empty() {
        eprintln!("krit: `prompt` does not accept arguments");
        return 2;
    }
    print!("{GENERATION_PROMPT}");
    0
}

fn permissions_command(arguments: &[String]) -> u8 {
    let mut json = false;
    let mut manifest_path = None;
    for argument in arguments {
        match argument.as_str() {
            "--json" => json = true,
            argument if argument.starts_with('-') => {
                eprintln!("krit: unknown option `{argument}`");
                return 2;
            }
            argument if manifest_path.is_none() => manifest_path = Some(argument),
            _ => {
                eprintln!("krit: expected at most one manifest path");
                return 2;
            }
        }
    }

    let path = manifest_path.map_or_else(|| PathBuf::from("krit.pkg"), PathBuf::from);
    match Manifest::load(&path) {
        Ok(manifest) => {
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

fn print_help() {
    println!(
        "\
Krit {VERSION}
An open, human-auditable language for the age of AI.

USAGE:
    krit run [--diagnostic-format human|json] FILE
    krit check [--diagnostic-format human|json] FILE
    krit build [--manifest PATH] [--output PATH]
    krit explain [--json] FILE
    krit fmt [--check] FILE...
    krit prompt
    krit permissions [--json] [MANIFEST]
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
        assert_eq!(count, 6, "prompt should contain six canonical examples");
    }
}
