use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

/// Every golden artifact under `conformance/build/compat/<toolchain>/<case>`
/// was written by that released toolchain and must still load and run with
/// the current `krit` exactly as checked in.
#[test]
fn compat_conformance_cases() {
    let root = repository_root().join("conformance/build/compat");
    let mut cases = Vec::new();
    collect_cases(&root, &mut cases);
    cases.sort();

    assert!(
        !cases.is_empty(),
        "no compatibility conformance cases found"
    );
    for case in cases {
        run_case(&case, &root);
    }
}

/// An artifact claiming a toolchain newer than the runtime fails closed with
/// K7004 before any guest code runs, even when it is otherwise intact.
#[test]
fn sandbox_refuses_an_artifact_from_a_newer_toolchain() {
    let case = repository_root().join("conformance/build/compat/0.2.0/factorial");
    let scratch = Scratch::new("newer-toolchain");
    copy_tree(&case, &scratch.path);
    let sidecar = scratch.path.join("artifact.wasm.json");
    let mut metadata: serde_json::Value =
        serde_json::from_slice(&fs::read(&sidecar).expect("adjacent metadata should be readable"))
            .expect("adjacent metadata should be JSON");
    let mut version = env!("CARGO_PKG_VERSION").split('.');
    let major = version.next().expect("major version");
    let minor = version
        .next()
        .and_then(|minor| minor.parse::<u64>().ok())
        .expect("minor version");
    metadata["compiler"]["version"] = format!("{major}.{}.0", minor + 1).into();
    fs::write(
        &sidecar,
        serde_json::to_vec(&metadata).expect("metadata should serialize"),
    )
    .expect("tampered metadata should be written");

    let output = Command::new(env!("CARGO_BIN_EXE_krit"))
        .args([
            "sandbox",
            "--manifest",
            "krit.pkg",
            "--artifact",
            "artifact.wasm",
        ])
        .current_dir(&scratch.path)
        .output()
        .expect("Krit should start");
    let stderr = String::from_utf8(output.stderr).expect("diagnostics should be UTF-8");
    assert_eq!(output.status.code(), Some(1), "{stderr}");
    assert!(output.stdout.is_empty());
    assert!(
        stderr.starts_with("artifact.wasm:1:1: error[K7004]: "),
        "{stderr}"
    );
    assert!(stderr.contains("compatibility window"), "{stderr}");
}

fn repository_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("CLI crate should be inside the workspace")
}

fn collect_cases(directory: &Path, cases: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(directory).expect("compat conformance directory should be readable") {
        let path = entry.expect("directory entry should be readable").path();
        if path.is_dir() {
            if path.join("artifact.wasm").is_file() {
                cases.push(path);
            } else {
                collect_cases(&path, cases);
            }
        }
    }
}

fn run_case(case: &Path, root: &Path) {
    let relative = case.strip_prefix(root).unwrap_or(case);
    let case_name = relative.display().to_string();
    let toolchain = relative
        .components()
        .next()
        .and_then(|component| component.as_os_str().to_str())
        .unwrap_or_else(|| panic!("{case_name}: case must sit under a toolchain directory"));
    let metadata: serde_json::Value = serde_json::from_slice(
        &fs::read(case.join("artifact.wasm.json")).expect("adjacent metadata should be readable"),
    )
    .expect("adjacent metadata should be JSON");
    assert_eq!(
        metadata["compiler"]["version"], toolchain,
        "{case_name}: golden artifact must be built by its directory's toolchain"
    );

    let host_config = case.join("host.json");
    // Host mode with durable state requires owner-only state directories,
    // which only Unix hosts can enforce.
    if host_config.is_file() && !cfg!(unix) {
        return;
    }

    // Run from a scratch copy so durable state never lands in the checkout,
    // with relative paths so the outcome does not depend on the host path.
    let scratch = Scratch::new(&case_name);
    copy_tree(case, &scratch.path);
    let mut command = Command::new(env!("CARGO_BIN_EXE_krit"));
    command.current_dir(&scratch.path);
    if case.join("request.json").is_file() {
        command.args([
            "invoke",
            "--manifest",
            "krit.pkg",
            "--artifact",
            "artifact.wasm",
            "--request",
            "request.json",
        ]);
        if host_config.is_file() {
            prepare_state_directories(&scratch.path, &host_config);
            command.args(["--host-config", "host.json"]);
        }
    } else {
        command.args([
            "sandbox",
            "--manifest",
            "krit.pkg",
            "--artifact",
            "artifact.wasm",
        ]);
    }
    let output = command.output().expect("Krit should start");

    let expected_status = fs::read_to_string(case.join("expect.status"))
        .expect("expect.status should be readable")
        .trim()
        .parse::<i32>()
        .expect("expect.status should contain a number");
    let expected_stdout = fs::read(case.join("expect.stdout")).unwrap_or_default();
    assert_eq!(
        output.status.code(),
        Some(expected_status),
        "status mismatch in {case_name}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&expected_stdout),
        "stdout mismatch in {case_name}"
    );
}

/// Creates the owner-only parent directory of every durable store the host
/// configuration names, as an operator would before starting the host.
fn prepare_state_directories(scratch: &Path, host_config: &Path) {
    let config: serde_json::Value = serde_json::from_slice(
        &fs::read(host_config).expect("host configuration should be readable"),
    )
    .expect("host configuration should be JSON");
    let Some(stores) = config["state"]["stores"].as_object() else {
        return;
    };
    for store in stores.values() {
        let path = store["path"]
            .as_str()
            .expect("durable store path should be a string");
        let parent = Path::new(path)
            .parent()
            .expect("durable store path should have a parent");
        create_owner_only_directory(&scratch.join(parent));
    }
}

#[cfg(unix)]
fn create_owner_only_directory(path: &Path) {
    use std::os::unix::fs::DirBuilderExt;

    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(path)
        .expect("state directory should be created owner-only");
}

#[cfg(not(unix))]
fn create_owner_only_directory(path: &Path) {
    fs::create_dir_all(path).expect("state directory should be created");
}

fn copy_tree(from: &Path, to: &Path) {
    for entry in fs::read_dir(from).expect("case directory should be readable") {
        let entry = entry.expect("directory entry should be readable");
        let source = entry.path();
        let target = to.join(entry.file_name());
        if source.is_dir() {
            fs::create_dir_all(&target).expect("case subdirectory should be created");
            copy_tree(&source, &target);
        } else {
            fs::copy(&source, &target).expect("case file should be copied");
        }
    }
}

struct Scratch {
    path: PathBuf,
}

impl Scratch {
    fn new(case_name: &str) -> Self {
        let name = case_name.replace(['/', '\\'], "-");
        let path = repository_root()
            .join("target/krit-cli-tests")
            .join(format!("compat-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&path);
        create_owner_only_directory(&path);
        Self { path }
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}
