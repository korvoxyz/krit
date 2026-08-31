use std::{
    env, fs, io,
    path::{Path, PathBuf},
    process::ExitCode,
};

use krit::{Diagnostic, Source, parse_source, run_source};
use krit_package::Manifest;

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
        match parse_source(&source) {
            Ok(_) => {
                println!("checked {}", path.display());
                0
            }
            Err(diagnostic) => report(&diagnostic, &source, format),
        }
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
    fn parses_every_prompt_example() {
        let mut remaining = GENERATION_PROMPT;
        let mut count = 0;
        while let Some(start) = remaining.find("```krit\n") {
            let code = &remaining[start + "```krit\n".len()..];
            let end = code.find("\n```").expect("Krit code fence should close");
            let source = Source::new(format!("<prompt-example-{count}>"), &code[..end]);
            parse_source(&source).expect("prompt example should parse");
            count += 1;
            remaining = &code[end + "\n```".len()..];
        }
        assert_eq!(count, 6, "prompt should contain six canonical examples");
    }
}
