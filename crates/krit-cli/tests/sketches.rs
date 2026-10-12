//! Release gate 8 of PRD 0.3 §5: the grounded use-case sketches.
//!
//! The sketches in `benchmarks/sketches/` are realistic agent programs (issue
//! triage, chat summaries, a payment webhook, a daily report, a ticket router,
//! a retrying worker, RAG answers, and a data transform) that measure how much
//! real work deploys. A sketch counts when it passes `krit check` and builds
//! from its own manifest. [`SKETCHES`] records the outcome and the scope of
//! each one, and these tests measure the corpus again on every run, so the
//! figures in the 0.3.0 CHANGELOG (2/8 with full behavior, 5/8 when
//! reduced-scope rewrites count) cannot drift silently. If a sketch stops
//! building, or starts building, the tests fail until the table,
//! `benchmarks/sketches/README.md`, and the CHANGELOG agree again.
//!
//! The release owner decides whether reduced-scope rewrites count towards the
//! gate's target of 4/8 (PRD 0.3 §5, item 8). These tests only keep the
//! measurement reproducible.

use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
    sync::atomic::{AtomicU64, Ordering},
};

static DIRECTORY_ID: AtomicU64 = AtomicU64::new(0);

const CORPUS: &str = "benchmarks/sketches";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Scope {
    /// The sketch keeps the whole behavior of its use case.
    Full,
    /// The sketch drops part of its use case, and its header says what.
    Reduced,
}

struct Sketch {
    /// The file stem under [`CORPUS`].
    name: &'static str,
    /// Whether the sketch is one of the eight that PRD 0.3 §5 counts. `03b`,
    /// the 0.2 buildable variant of use case 3, is not; the master roadmap's
    /// corpus (`docs/roadmap.md` §3.6) counts it in place of sketch 02.
    counted: bool,
    scope: Scope,
    /// Whether the sketch passes `krit check` and builds in 0.3.
    builds: bool,
}

const SKETCHES: [Sketch; 9] = [
    Sketch {
        name: "01-github-triage",
        counted: true,
        scope: Scope::Full,
        builds: false,
    },
    Sketch {
        name: "02-slack-summarize",
        counted: true,
        scope: Scope::Reduced,
        builds: true,
    },
    Sketch {
        name: "03-payment-webhook",
        counted: true,
        scope: Scope::Reduced,
        builds: true,
    },
    Sketch {
        name: "03b-payment-webhook-buildable",
        counted: false,
        scope: Scope::Reduced,
        builds: true,
    },
    Sketch {
        name: "04-daily-report",
        counted: true,
        scope: Scope::Full,
        builds: false,
    },
    Sketch {
        name: "05-ticket-router",
        counted: true,
        scope: Scope::Reduced,
        builds: true,
    },
    Sketch {
        name: "06-flaky-worker",
        counted: true,
        scope: Scope::Full,
        builds: true,
    },
    Sketch {
        name: "07-rag-answerer",
        counted: true,
        scope: Scope::Full,
        builds: true,
    },
    Sketch {
        name: "08-orders-transform",
        counted: true,
        scope: Scope::Full,
        builds: false,
    },
];

struct ScratchDirectory {
    path: PathBuf,
}

impl ScratchDirectory {
    fn new(name: &str) -> Self {
        let id = DIRECTORY_ID.fetch_add(1, Ordering::Relaxed);
        let path = repository_root()
            .join("target/krit-cli-tests")
            .join(format!("sketches-{name}-{}-{id}", std::process::id()));
        fs::create_dir_all(&path).expect("scratch directory should be created");
        Self { path }
    }
}

impl Drop for ScratchDirectory {
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

/// The `(code, span)` of every diagnostic in JSON Lines `stderr`, in order.
fn diagnostics(output: &Output) -> Vec<(String, String)> {
    String::from_utf8(output.stderr.clone())
        .expect("diagnostics should be UTF-8")
        .lines()
        .map(|line| {
            let diagnostic = serde_json::from_str::<serde_json::Value>(line)
                .unwrap_or_else(|error| panic!("diagnostic should be JSON ({error}): {line}"));
            (
                diagnostic["code"].as_str().unwrap_or_default().to_owned(),
                diagnostic["span"].to_string(),
            )
        })
        .collect()
}

fn codes(diagnostics: &[(String, String)]) -> BTreeSet<&str> {
    diagnostics.iter().map(|(code, _)| code.as_str()).collect()
}

#[test]
fn the_corpus_holds_exactly_the_recorded_sketches() {
    let mut found = fs::read_dir(repository_root().join(CORPUS))
        .expect("the sketch corpus should be readable")
        .map(|entry| entry.expect("directory entry should be readable").path())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "krit")
        })
        .map(|path| {
            path.file_stem()
                .expect("sketch should have a name")
                .to_string_lossy()
                .into_owned()
        })
        .collect::<Vec<_>>();
    found.sort();
    let recorded = SKETCHES
        .iter()
        .map(|sketch| sketch.name.to_owned())
        .collect::<Vec<_>>();
    assert_eq!(
        found, recorded,
        "every sketch in {CORPUS} should be recorded in SKETCHES, in order"
    );

    for sketch in &SKETCHES {
        let manifest = format!("{CORPUS}/{}.krit.pkg", sketch.name);
        let text = fs::read_to_string(repository_root().join(&manifest))
            .unwrap_or_else(|error| panic!("{manifest} should be readable: {error}"));
        let parsed = krit_package::Manifest::parse(&text)
            .unwrap_or_else(|error| panic!("{manifest} should parse: {error}"));
        assert_eq!(
            parsed.package.entry,
            format!("{}.krit", sketch.name),
            "{manifest} should build its own sketch"
        );
    }
}

/// Each sketch passes `krit check` and builds exactly when [`SKETCHES`] says
/// so. A sketch that cannot build fails only on WebAssembly policy limits
/// (`K7002`), never on a source error, and `check --target component`
/// reports exactly the findings that `build` reports.
#[test]
fn every_sketch_checks_and_builds_exactly_as_recorded() {
    let directory = ScratchDirectory::new("measure");
    for sketch in &SKETCHES {
        let source = format!("{CORPUS}/{}.krit", sketch.name);
        let manifest = format!("{source}.pkg");
        let artifact = directory.path.join(format!("{}.wasm", sketch.name));

        let check = krit()
            .args(["check", "--diagnostic-format", "json", &source])
            .output()
            .expect("check should start");
        let target = krit()
            .args([
                "check",
                "--target",
                "component",
                "--diagnostic-format",
                "json",
                &source,
            ])
            .output()
            .expect("target check should start");
        let build = krit()
            .args([
                "build",
                "--diagnostic-format",
                "json",
                "--manifest",
                &manifest,
                "--output",
            ])
            .arg(&artifact)
            .output()
            .expect("build should start");
        let build_findings = diagnostics(&build);

        if sketch.builds {
            assert!(
                check.status.success() && check.stderr.is_empty(),
                "{source} should check without diagnostics: {}",
                String::from_utf8_lossy(&check.stderr)
            );
            assert!(
                build.status.success() && artifact.is_file(),
                "{manifest} should build {source}: {}",
                String::from_utf8_lossy(&build.stderr)
            );
        } else {
            assert!(
                !build.status.success() && !artifact.exists(),
                "{source} now builds: record it in SKETCHES and update \
                 benchmarks/sketches/README.md and the CHANGELOG metrics"
            );
            assert!(
                !build_findings.is_empty() && codes(&build_findings) == BTreeSet::from(["K7002"]),
                "{source} should fail to build only on WebAssembly policy limits (K7002): {}",
                String::from_utf8_lossy(&build.stderr)
            );
            // An entrypoint file is checked against the component target by
            // default and reports the same findings; a module program passes.
            let check_findings = diagnostics(&check);
            assert!(
                (check.status.success() && check_findings.is_empty())
                    || (!check.status.success() && check_findings == build_findings),
                "{source} should either check cleanly or report its build findings: {}",
                String::from_utf8_lossy(&check.stderr)
            );
        }

        assert_eq!(
            target.status.success(),
            build.status.success(),
            "check --target component and build should agree on {source}"
        );
        let mut target_findings = diagnostics(&target);
        let mut build_findings = build_findings;
        target_findings.sort();
        build_findings.sort();
        assert_eq!(
            target_findings, build_findings,
            "check --target component should report exactly the build findings for {source}"
        );
    }
}

/// The published gate-8 figures follow from the measured table: of the eight
/// sketches PRD 0.3 §5 counts, two build with full behavior and five build
/// when reduced-scope rewrites count.
#[test]
fn gate_eight_figures_match_the_published_metrics() {
    let counted = SKETCHES
        .iter()
        .filter(|sketch| sketch.counted)
        .collect::<Vec<_>>();
    assert_eq!(counted.len(), 8, "PRD 0.3 §5 counts eight sketches");

    let full = counted
        .iter()
        .filter(|sketch| sketch.builds && sketch.scope == Scope::Full)
        .map(|sketch| sketch.name)
        .collect::<Vec<_>>();
    let reduced_counted = counted
        .iter()
        .filter(|sketch| sketch.builds)
        .map(|sketch| sketch.name)
        .collect::<Vec<_>>();
    assert_eq!(
        full,
        ["06-flaky-worker", "07-rag-answerer"],
        "2/8 build with full behavior; update the CHANGELOG metrics, PRD 0.3 §5 and \
         benchmarks/sketches/README.md if this changes"
    );
    assert_eq!(
        reduced_counted,
        [
            "02-slack-summarize",
            "03-payment-webhook",
            "05-ticket-router",
            "06-flaky-worker",
            "07-rag-answerer",
        ],
        "5/8 build when reduced-scope rewrites count; update the CHANGELOG metrics, \
         PRD 0.3 §5 and benchmarks/sketches/README.md if this changes"
    );
}

/// The reduced payment sketch is deployable, not only buildable: against an
/// operator-provisioned SQLite store it records a payment once, answers a
/// repeat or an unrelated event without recording it, and refuses a wrong
/// token and a non-POST method. Durable host mode needs owner-only files,
/// which only Unix hosts enforce.
#[cfg(unix)]
#[test]
fn the_reduced_payment_sketch_records_each_payment_once() {
    use std::os::unix::fs::{DirBuilderExt, PermissionsExt};

    let directory = ScratchDirectory::new("payment");
    fs::set_permissions(&directory.path, fs::Permissions::from_mode(0o700))
        .expect("scratch directory should be owner-only");
    let data = directory.path.join("data");
    fs::DirBuilder::new()
        .mode(0o700)
        .create(&data)
        .expect("data directory should be owner-only");
    let database = data.join("payments.db");
    let schema = fs::read_to_string(
        repository_root().join(format!("{CORPUS}/03-payment-webhook.schema.sql")),
    )
    .expect("payment schema should be readable");
    let connection = rusqlite::Connection::open(&database).expect("database should open");
    connection
        .execute_batch(&schema)
        .expect("payment schema should apply");
    drop(connection);
    fs::set_permissions(&database, fs::Permissions::from_mode(0o600))
        .expect("database should be owner-only");
    // The host file names `data/payments.db` relative to itself, so a copy
    // next to the scratch database configures that database.
    let host = directory.path.join("host.json");
    fs::copy(
        repository_root().join(format!("{CORPUS}/03-payment-webhook.host.json")),
        &host,
    )
    .expect("host configuration should be copied");

    let manifest = format!("{CORPUS}/03-payment-webhook.krit.pkg");
    let artifact = directory.path.join("payment.wasm");
    let build = krit()
        .args(["build", "--manifest", &manifest, "--output"])
        .arg(&artifact)
        .output()
        .expect("build should start");
    assert!(
        build.status.success(),
        "{}",
        String::from_utf8_lossy(&build.stderr)
    );

    let payment = serde_json::json!({
        "id": "evt_1",
        "type": "payment_intent.succeeded",
        "data": { "object": { "id": "pi_1", "amount": 1200, "currency": "usd" } },
    })
    .to_string();
    let customer = serde_json::json!({
        "id": "evt_2",
        "type": "customer.created",
        "data": { "object": { "id": "cus_1" } },
    })
    .to_string();
    let token = "token=fixture-token";
    let cases = [
        ("POST", token, payment.as_str(), 200, "recorded"),
        ("POST", token, payment.as_str(), 200, "duplicate or ignored"),
        (
            "POST",
            token,
            customer.as_str(),
            200,
            "duplicate or ignored",
        ),
        ("POST", "token=wrong", payment.as_str(), 401, "unauthorized"),
        ("POST", "", payment.as_str(), 401, "unauthorized"),
        ("GET", token, "", 405, "POST only"),
    ];
    for (index, (method, query, body, status, answer)) in cases.into_iter().enumerate() {
        let request = directory.path.join(format!("request-{index}.json"));
        let fields = serde_json::json!({
            "method": method,
            "path": "/payments",
            "query": query,
            "headers": [],
            "body": body,
        });
        fs::write(&request, fields.to_string()).expect("request should be written");
        let invoke = krit()
            .args(["invoke", "--manifest", &manifest, "--host-config"])
            .arg(&host)
            .arg("--request")
            .arg(&request)
            .arg("--artifact")
            .arg(&artifact)
            .output()
            .expect("invoke should start");
        assert!(
            invoke.status.success(),
            "request {index} should be answered: {}",
            String::from_utf8_lossy(&invoke.stderr)
        );
        let response = serde_json::from_slice::<serde_json::Value>(&invoke.stdout)
            .expect("response should be JSON");
        assert_eq!(
            (response["status"].as_i64(), response["body"].as_str()),
            (Some(status), Some(answer)),
            "request {index} ({method} {query}) got {response}"
        );
    }

    let connection = rusqlite::Connection::open(&database).expect("database should reopen");
    let rows = connection
        .prepare("SELECT event_id, intent_id, amount_cents, currency FROM payments")
        .expect("query should prepare")
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, String>(3)?,
            ))
        })
        .expect("query should run")
        .collect::<Result<Vec<_>, _>>()
        .expect("rows should decode");
    assert_eq!(
        rows,
        [(
            "evt_1".to_owned(),
            "pi_1".to_owned(),
            1200,
            "usd".to_owned()
        )],
        "only the payment should be recorded, and only once"
    );
}
