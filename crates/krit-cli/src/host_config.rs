use std::{
    collections::BTreeMap,
    fs,
    io::Read,
    path::{Component, Path, PathBuf},
};

use krit_package::Manifest;
use krit_runtime::{HostInputs, RuntimeLimits, SecretStore};
use serde::Deserialize;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum HostConfigErrorKind {
    Authorization,
    Input,
}

#[derive(Debug)]
pub(crate) struct HostConfigError {
    kind: HostConfigErrorKind,
    message: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct HostConfigFile {
    schema: u32,
    #[serde(default)]
    config: BTreeMap<String, String>,
    #[serde(default)]
    secrets: BTreeMap<String, SecretReference>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SecretReference {
    file: PathBuf,
}

impl HostConfigError {
    pub(crate) const fn kind(&self) -> HostConfigErrorKind {
        self.kind
    }

    pub(crate) fn message(&self) -> &str {
        &self.message
    }

    fn input(message: impl Into<String>) -> Self {
        Self {
            kind: HostConfigErrorKind::Input,
            message: message.into(),
        }
    }

    fn authorization(message: impl Into<String>) -> Self {
        Self {
            kind: HostConfigErrorKind::Authorization,
            message: message.into(),
        }
    }
}

pub(crate) fn load(
    path: Option<&Path>,
    manifest: &Manifest,
    limits: RuntimeLimits,
) -> Result<HostInputs, HostConfigError> {
    let Some(path) = path else {
        return HostInputs::new(BTreeMap::new(), SecretStore::default())
            .map_err(|error| HostConfigError::input(error.message()));
    };
    let bytes = read_bounded(path, limits.host_config_bytes()).map_err(|error| {
        HostConfigError::input(format!(
            "could not read host config {}: {error}",
            path.display()
        ))
    })?;
    let parsed: HostConfigFile = serde_json::from_slice(&bytes).map_err(|error| {
        HostConfigError::input(format!(
            "invalid strict host config JSON {}: {error}",
            path.display()
        ))
    })?;
    if parsed.schema != 1 {
        return Err(HostConfigError::input(format!(
            "unsupported host config schema {}; expected 1",
            parsed.schema
        )));
    }
    for name in parsed.config.keys() {
        if !manifest.capabilities.config.contains(name) {
            return Err(HostConfigError::authorization(format!(
                "host configuration key `{name}` is not granted by the package manifest"
            )));
        }
    }
    for name in parsed.secrets.keys() {
        if !manifest.capabilities.secrets.contains(name) {
            return Err(HostConfigError::authorization(format!(
                "host secret `{name}` is not granted by the package manifest"
            )));
        }
    }

    let root = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let canonical_root = root.canonicalize().map_err(|error| {
        HostConfigError::input(format!(
            "could not resolve host config directory {}: {error}",
            root.display()
        ))
    })?;
    let mut secrets = BTreeMap::new();
    for (name, reference) in parsed.secrets {
        validate_secret_reference(&reference.file)?;
        let unresolved = canonical_root.join(&reference.file);
        let link_metadata = fs::symlink_metadata(&unresolved).map_err(|error| {
            HostConfigError::input(format!(
                "could not inspect secret file for `{name}`: {error}"
            ))
        })?;
        if link_metadata.file_type().is_symlink() {
            return Err(HostConfigError::input(format!(
                "secret file for `{name}` must not be a symbolic link"
            )));
        }
        let canonical = unresolved.canonicalize().map_err(|error| {
            HostConfigError::input(format!(
                "could not resolve secret file for `{name}`: {error}"
            ))
        })?;
        if !canonical.starts_with(&canonical_root) || !canonical.is_file() {
            return Err(HostConfigError::input(format!(
                "secret file for `{name}` must be a regular file inside the host config directory"
            )));
        }
        let bytes =
            read_secret_bounded(&canonical, limits.secret_bytes(), &name).map_err(|error| {
                HostConfigError::input(format!("could not read secret file for `{name}`: {error}"))
            })?;
        secrets.insert(name, bytes);
    }
    let secrets = SecretStore::new(secrets)
        .map_err(|error| HostConfigError::input(error.message().to_owned()))?;
    HostInputs::new(parsed.config, secrets)
        .map_err(|error| HostConfigError::input(error.message().to_owned()))
}

fn validate_secret_reference(path: &Path) -> Result<(), HostConfigError> {
    if path.as_os_str().is_empty()
        || path.is_absolute()
        || path.to_string_lossy().contains('\\')
        || path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(HostConfigError::input(
            "secret file references must be relative paths without `.` or `..`",
        ));
    }
    Ok(())
}

#[cfg(unix)]
fn read_secret_bounded(path: &Path, limit: usize, name: &str) -> Result<Vec<u8>, String> {
    use std::os::unix::fs::PermissionsExt;

    use rustix::fs::{Mode, OFlags};

    let descriptor = rustix::fs::open(
        path,
        OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NOFOLLOW,
        Mode::empty(),
    )
    .map_err(|error| error.to_string())?;
    let file = fs::File::from(descriptor);
    let metadata = file.metadata().map_err(|error| error.to_string())?;
    if !metadata.is_file() {
        return Err("not a regular file".to_owned());
    }
    let mode = metadata.permissions().mode();
    if mode & 0o077 != 0 {
        return Err(format!(
            "secret file for `{name}` must not grant group or other permissions"
        ));
    }
    read_file_bounded(file, limit)
}

#[cfg(not(unix))]
fn read_secret_bounded(path: &Path, limit: usize, _name: &str) -> Result<Vec<u8>, String> {
    let file = fs::File::open(path).map_err(|error| error.to_string())?;
    read_file_bounded(file, limit)
}

fn read_bounded(path: &Path, limit: usize) -> Result<Vec<u8>, String> {
    let file = fs::File::open(path).map_err(|error| error.to_string())?;
    read_file_bounded(file, limit)
}

fn read_file_bounded(file: fs::File, limit: usize) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    file.take(limit.saturating_add(1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|error| error.to_string())?;
    if bytes.len() > limit {
        return Err(format!("file exceeds the {limit}-byte host input limit"));
    }
    Ok(bytes)
}
