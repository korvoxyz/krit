use std::{
    collections::BTreeSet,
    io::{self, Write},
    path::PathBuf,
};

use krit_assist::{
    AssistError, ContextSelection, PermissionKey, ProviderConfig, RequestOptions, SuggestionKind,
    SuggestionProvider, accept_reviewed, build_proposal, escape_human_text, prepare_request,
    render_inspection_human, render_proposal_human, render_review_human, review_proposal,
    write_proposal,
};

pub(crate) fn run(arguments: &[String]) -> u8 {
    let json = arguments.iter().any(|argument| argument == "--json");
    match run_inner(arguments) {
        Ok(()) => 0,
        Err(CommandError::Usage(message)) => {
            eprintln!("krit: {message}");
            2
        }
        Err(CommandError::Assist(error)) => {
            if json {
                eprintln!("{}", error.render_json());
            } else {
                eprintln!("krit: {error}");
            }
            error.exit_status()
        }
    }
}

fn run_inner(arguments: &[String]) -> Result<(), CommandError> {
    let Some(command) = arguments.first().map(String::as_str) else {
        return Err(CommandError::Usage(usage().to_owned()));
    };
    match command {
        "inspect" => inspect_command(&arguments[1..]),
        "suggest" => suggest_command(&arguments[1..]),
        "review" => review_command(&arguments[1..]),
        "accept" => accept_command(&arguments[1..]),
        _ => Err(CommandError::Usage(format!(
            "unknown assist command `{command}`; {}",
            usage()
        ))),
    }
}

fn inspect_command(arguments: &[String]) -> Result<(), CommandError> {
    let options = parse_request_options(arguments, false)?;
    let provider = ProviderConfig::load(&options.provider_config)?;
    let prepared = prepare_request(provider.descriptor(), options.request)?;
    if options.json {
        write_json_line(serde_json::json!({
            "schema": 1,
            "event": "inspection",
            "inspection": prepared.inspection()
        }))?;
    } else {
        write_stdout(&render_inspection_human(prepared.inspection())?)?;
    }
    Ok(())
}

fn suggest_command(arguments: &[String]) -> Result<(), CommandError> {
    let options = parse_request_options(arguments, true)?;
    let proposal_path = options
        .proposal
        .as_ref()
        .expect("suggest parsing requires a proposal path");
    let provider = ProviderConfig::load(&options.provider_config)?;
    let prepared = prepare_request(provider.descriptor(), options.request)?;

    if options.json {
        write_json_line(serde_json::json!({
            "schema": 1,
            "event": "inspection",
            "inspection": prepared.inspection()
        }))?;
    } else {
        write_stdout(&render_inspection_human(prepared.inspection())?)?;
    }
    io::stdout()
        .flush()
        .map_err(|_| AssistError::io("could not flush context inspection"))?;

    let response = provider.suggest(prepared.request())?;
    let proposal = build_proposal(prepared, response)?;
    write_proposal(proposal_path, &proposal)?;
    if options.json {
        write_json_line(serde_json::json!({
            "schema": 1,
            "event": "proposal",
            "proposalPath": proposal_path.to_string_lossy(),
            "proposal": proposal
        }))?;
    } else {
        write_stdout(&format!(
            "proposal written to {}\n{}",
            escape_human_text(&proposal_path.to_string_lossy()),
            render_proposal_human(&proposal)?
        ))?;
    }
    Ok(())
}

fn review_command(arguments: &[String]) -> Result<(), CommandError> {
    let options = parse_review_options(arguments, false)?;
    let reviewed = review_proposal(&options.manifest, &options.proposal)?;
    if options.json {
        write_json_line(serde_json::json!({
            "schema": 1,
            "event": "review",
            "proposalId": reviewed.proposal().proposal_id,
            "review": reviewed.review(),
            "diff": reviewed.diff()
        }))?;
    } else {
        write_stdout(&render_review_human(&reviewed)?)?;
    }
    Ok(())
}

fn accept_command(arguments: &[String]) -> Result<(), CommandError> {
    let options = parse_review_options(arguments, true)?;
    if !options.reviewed {
        return Err(CommandError::Usage(
            "`assist accept` requires the explicit `--reviewed` flag".to_owned(),
        ));
    }
    let reviewed = review_proposal(&options.manifest, &options.proposal)?;
    if options.json {
        write_json_line(serde_json::json!({
            "schema": 1,
            "event": "review",
            "proposalId": reviewed.proposal().proposal_id,
            "review": reviewed.review(),
            "diff": reviewed.diff()
        }))?;
    } else {
        write_stdout(&render_review_human(&reviewed)?)?;
    }
    io::stdout()
        .flush()
        .map_err(|_| AssistError::io("could not flush proposal review"))?;

    let accepted = accept_reviewed(reviewed, &options.approvals)?;
    if options.json {
        write_json_line(serde_json::json!({
            "schema": 1,
            "event": "accepted",
            "accepted": accepted
        }))?;
    } else {
        write_stdout(&format!(
            "accepted {} as {}\n",
            escape_human_text(&accepted.target),
            accepted.digest
        ))?;
    }
    Ok(())
}

struct ParsedRequestOptions {
    provider_config: PathBuf,
    request: RequestOptions,
    proposal: Option<PathBuf>,
    json: bool,
}

fn parse_request_options(
    arguments: &[String],
    require_proposal: bool,
) -> Result<ParsedRequestOptions, CommandError> {
    let mut provider_config = None;
    let mut manifest = None;
    let mut file = None;
    let mut range = None;
    let mut intent = None;
    let mut kind = SuggestionKind::Completion;
    let mut contexts = Vec::new();
    let mut proposal = None;
    let mut json = false;
    let mut index = 0;
    while index < arguments.len() {
        match arguments[index].as_str() {
            "--provider-config" => {
                provider_config = Some(PathBuf::from(required_value(arguments, index)?));
                index += 2;
            }
            "--manifest" => {
                manifest = Some(PathBuf::from(required_value(arguments, index)?));
                index += 2;
            }
            "--file" => {
                file = Some(PathBuf::from(required_value(arguments, index)?));
                index += 2;
            }
            "--range" => {
                range = Some(required_value(arguments, index)?.parse()?);
                index += 2;
            }
            "--intent" => {
                intent = Some(required_value(arguments, index)?.to_owned());
                index += 2;
            }
            "--kind" => {
                kind = parse_kind(required_value(arguments, index)?)?;
                index += 2;
            }
            "--context" => {
                contexts.push(parse_context(required_value(arguments, index)?)?);
                index += 2;
            }
            "--proposal" if require_proposal => {
                proposal = Some(PathBuf::from(required_value(arguments, index)?));
                index += 2;
            }
            "--json" => {
                json = true;
                index += 1;
            }
            argument if argument.starts_with('-') => {
                return Err(CommandError::Usage(format!(
                    "unknown assist option `{argument}`"
                )));
            }
            argument => {
                return Err(CommandError::Usage(format!(
                    "unexpected assist argument `{argument}`"
                )));
            }
        }
    }
    let provider_config = provider_config
        .ok_or_else(|| CommandError::Usage("missing `--provider-config PATH`".to_owned()))?;
    let manifest =
        manifest.ok_or_else(|| CommandError::Usage("missing `--manifest PATH`".to_owned()))?;
    let file = file.ok_or_else(|| CommandError::Usage("missing `--file PATH`".to_owned()))?;
    let range = range.ok_or_else(|| CommandError::Usage("missing `--range RANGE`".to_owned()))?;
    let intent = intent.ok_or_else(|| CommandError::Usage("missing `--intent TEXT`".to_owned()))?;
    if require_proposal && proposal.is_none() {
        return Err(CommandError::Usage(
            "missing `--proposal PATH.json`".to_owned(),
        ));
    }
    Ok(ParsedRequestOptions {
        provider_config,
        request: RequestOptions {
            manifest_path: manifest,
            target_path: file,
            selection: range,
            contexts,
            intent,
            kind,
        },
        proposal,
        json,
    })
}

struct ReviewOptions {
    manifest: PathBuf,
    proposal: PathBuf,
    approvals: BTreeSet<PermissionKey>,
    reviewed: bool,
    json: bool,
}

fn parse_review_options(arguments: &[String], accept: bool) -> Result<ReviewOptions, CommandError> {
    let mut manifest = None;
    let mut proposal = None;
    let mut approvals = BTreeSet::new();
    let mut reviewed = false;
    let mut json = false;
    let mut index = 0;
    while index < arguments.len() {
        match arguments[index].as_str() {
            "--manifest" => {
                manifest = Some(PathBuf::from(required_value(arguments, index)?));
                index += 2;
            }
            "--proposal" => {
                proposal = Some(PathBuf::from(required_value(arguments, index)?));
                index += 2;
            }
            "--approve-permission" if accept => {
                let approval = required_value(arguments, index)?.parse()?;
                if !approvals.insert(approval) {
                    return Err(CommandError::Usage(
                        "duplicate `--approve-permission` value".to_owned(),
                    ));
                }
                index += 2;
            }
            "--reviewed" if accept => {
                reviewed = true;
                index += 1;
            }
            "--json" => {
                json = true;
                index += 1;
            }
            argument if argument.starts_with('-') => {
                return Err(CommandError::Usage(format!(
                    "unknown assist option `{argument}`"
                )));
            }
            argument => {
                return Err(CommandError::Usage(format!(
                    "unexpected assist argument `{argument}`"
                )));
            }
        }
    }
    Ok(ReviewOptions {
        manifest: manifest
            .ok_or_else(|| CommandError::Usage("missing `--manifest PATH`".to_owned()))?,
        proposal: proposal
            .ok_or_else(|| CommandError::Usage("missing `--proposal PATH.json`".to_owned()))?,
        approvals,
        reviewed,
        json,
    })
}

fn required_value(arguments: &[String], index: usize) -> Result<&str, CommandError> {
    arguments
        .get(index + 1)
        .map(String::as_str)
        .ok_or_else(|| CommandError::Usage(format!("`{}` requires a value", arguments[index])))
}

fn parse_kind(value: &str) -> Result<SuggestionKind, CommandError> {
    match value {
        "completion" => Ok(SuggestionKind::Completion),
        "repair" => Ok(SuggestionKind::DiagnosticRepair),
        "cleanup" => Ok(SuggestionKind::SemanticCleanup),
        _ => Err(CommandError::Usage(
            "`--kind` must be `completion`, `repair`, or `cleanup`".to_owned(),
        )),
    }
}

fn parse_context(value: &str) -> Result<ContextSelection, CommandError> {
    let (path, range) = value.rsplit_once('@').ok_or_else(|| {
        CommandError::Usage("context must be `PATH@RANGE`, where RANGE may be `all`".to_owned())
    })?;
    if path.is_empty() {
        return Err(CommandError::Usage(
            "context path cannot be empty".to_owned(),
        ));
    }
    Ok(ContextSelection {
        path: PathBuf::from(path),
        range: range.parse()?,
    })
}

fn write_stdout(text: &str) -> Result<(), AssistError> {
    let mut stdout = io::stdout().lock();
    stdout
        .write_all(text.as_bytes())
        .and_then(|()| stdout.flush())
        .map_err(|_| AssistError::io("could not write assist output"))
}

fn write_json_line(value: serde_json::Value) -> Result<(), AssistError> {
    let mut stdout = io::stdout().lock();
    serde_json::to_writer(&mut stdout, &value)
        .map_err(|_| AssistError::io("could not serialize assist JSON output"))?;
    stdout
        .write_all(b"\n")
        .and_then(|()| stdout.flush())
        .map_err(|_| AssistError::io("could not write assist JSON output"))
}

fn usage() -> &'static str {
    "usage: krit assist inspect|suggest|review|accept ..."
}

enum CommandError {
    Usage(String),
    Assist(AssistError),
}

impl From<AssistError> for CommandError {
    fn from(error: AssistError) -> Self {
        Self::Assist(error)
    }
}
