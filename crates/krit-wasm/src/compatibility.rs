//! Artifact compatibility: the published sets of recorded artifact facts a
//! runtime loads, and the toolchain window for recorded tool versions.
//!
//! A runtime loads an artifact only when its metadata schema, validation
//! policy version, `krit:runtime` WIT package version, and edition are all
//! members of the sets below. The recorded compiler and language versions are
//! never required to equal the running toolchain; they must only fall inside
//! [`toolchain_in_window`]. Whatever an artifact records, its effects and world
//! are still re-derived from the validated component imports.

use crate::{
    ARTIFACT_METADATA_SCHEMA, ARTIFACT_POLICY_VERSION, STATE_ARTIFACT_POLICY_VERSION,
    wit::RUNTIME_WIT_VERSION,
};

/// Embedded and adjacent artifact metadata schema versions this runtime loads.
pub const SUPPORTED_METADATA_SCHEMAS: [u32; 1] = [ARTIFACT_METADATA_SCHEMA];
/// Artifact validation policy versions this runtime loads.
pub const SUPPORTED_POLICY_VERSIONS: [u32; 2] =
    [ARTIFACT_POLICY_VERSION, STATE_ARTIFACT_POLICY_VERSION];
/// `krit:runtime` WIT package versions whose worlds this runtime links.
pub const SUPPORTED_RUNTIME_WIT_VERSIONS: [&str; 1] = [RUNTIME_WIT_VERSION];
/// Language editions this runtime loads artifacts for.
pub const SUPPORTED_EDITIONS: [&str; 1] = ["2026"];
/// How many minor toolchains before the runtime's own still have their
/// artifacts loaded.
pub const PREVIOUS_MINOR_TOOLCHAINS: u64 = 2;

const RUNTIME_WIT_PREFIX: &str = "krit:runtime/";

/// Returns whether an artifact that records toolchain version `artifact` may
/// be loaded by a runtime whose toolchain version is `runtime`.
///
/// Both must be strict semantic versions with the same major version, and the
/// artifact's minor version must be the runtime's own or one of the
/// [`PREVIOUS_MINOR_TOOLCHAINS`] before it: a 0.3 runtime loads 0.1, 0.2, and
/// 0.3 artifacts. Newer minors, other majors, and unparsable versions are
/// rejected. Patch, pre-release, and build components never affect the
/// outcome.
pub fn toolchain_in_window(artifact: &str, runtime: &str) -> bool {
    match (toolchain_line(artifact), toolchain_line(runtime)) {
        (Some((artifact_major, artifact_minor)), Some((runtime_major, runtime_minor))) => {
            artifact_major == runtime_major
                && artifact_minor <= runtime_minor
                && artifact_minor >= runtime_minor.saturating_sub(PREVIOUS_MINOR_TOOLCHAINS)
        }
        _ => false,
    }
}

/// Describes the window [`toolchain_in_window`] accepts for `runtime`, such
/// as `0.1 through 0.3`.
pub(crate) fn window_label(runtime: &str) -> String {
    match toolchain_line(runtime) {
        Some((major, minor)) => format!(
            "{major}.{} through {major}.{minor}",
            minor.saturating_sub(PREVIOUS_MINOR_TOOLCHAINS)
        ),
        None => "no versions".to_owned(),
    }
}

/// The `major.minor` toolchain line of a well-formed version, for messages.
/// Unparsable input yields `None` so untrusted text is never echoed.
pub(crate) fn toolchain_label(version: &str) -> Option<String> {
    toolchain_line(version).map(|(major, minor)| format!("{major}.{minor}"))
}

/// Returns the version of a `krit:runtime/<item>@<version>` name, or `None`
/// for any other name.
pub(crate) fn runtime_wit_version(name: &str) -> Option<&str> {
    let (item, version) = name.strip_prefix(RUNTIME_WIT_PREFIX)?.rsplit_once('@')?;
    (!item.is_empty()).then_some(version)
}

/// Parses a strict Semantic Versioning 2.0 version
/// (`MAJOR.MINOR.PATCH`, optional `-pre-release` and `+build` identifiers)
/// and returns its `(major, minor)` line.
fn toolchain_line(version: &str) -> Option<(u64, u64)> {
    let (version, build) = match version.split_once('+') {
        Some((version, build)) => (version, Some(build)),
        None => (version, None),
    };
    let (core, pre_release) = match version.split_once('-') {
        Some((core, pre_release)) => (core, Some(pre_release)),
        None => (version, None),
    };
    if !build.is_none_or(|build| identifiers(build, false))
        || !pre_release.is_none_or(|pre_release| identifiers(pre_release, true))
    {
        return None;
    }
    let mut parts = core.split('.');
    let major = numeric(parts.next()?)?;
    let minor = numeric(parts.next()?)?;
    numeric(parts.next()?)?;
    if parts.next().is_some() {
        return None;
    }
    Some((major, minor))
}

fn numeric(part: &str) -> Option<u64> {
    if part.is_empty()
        || !part.bytes().all(|byte| byte.is_ascii_digit())
        || (part.len() > 1 && part.starts_with('0'))
    {
        return None;
    }
    part.parse().ok()
}

fn identifiers(identifiers: &str, pre_release: bool) -> bool {
    identifiers.split('.').all(|identifier| {
        !identifier.is_empty()
            && identifier
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
            && !(pre_release
                && identifier.len() > 1
                && identifier.starts_with('0')
                && identifier.bytes().all(|byte| byte.is_ascii_digit()))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_runtime_loads_its_own_and_the_two_previous_minor_toolchains() {
        for artifact in [
            "0.1.0",
            "0.1.9",
            "0.2.0",
            "0.2.7",
            "0.3.0",
            "0.3.4",
            "0.3.0-rc.1",
            "0.2.0+build.5",
        ] {
            assert!(toolchain_in_window(artifact, "0.3.0"), "{artifact}");
            assert!(toolchain_in_window(artifact, "0.3.2"), "{artifact}");
        }
        assert!(toolchain_in_window("0.0.1", "0.2.0"));
        assert!(toolchain_in_window("0.0.0", "0.0.0"));
        assert!(toolchain_in_window("1.4.0", "1.6.0"));
    }

    #[test]
    fn newer_older_other_major_and_unparsable_toolchains_fail_closed() {
        for artifact in [
            "0.4.0",
            "0.4.0-alpha",
            "0.0.9",
            "1.0.0",
            "1.3.0",
            "",
            "0.3",
            "0.3.0.0",
            "v0.3.0",
            " 0.3.0",
            "0.3.0 ",
            "00.3.0",
            "0.03.0",
            "0.3.00",
            "0.3.0-",
            "0.3.0+",
            "0.3.0-01",
            "0.3.0-rc..1",
            "0.3.0-rc_1",
            "0.3.x",
            "-0.3.0",
            "0.3.18446744073709551616",
            "garbage",
        ] {
            assert!(!toolchain_in_window(artifact, "0.3.0"), "{artifact:?}");
        }
        assert!(!toolchain_in_window("0.9.0", "1.0.0"));
        assert!(!toolchain_in_window("1.2.0", "1.5.0"));
        assert!(!toolchain_in_window("0.3.0", "garbage"));
    }

    #[test]
    fn the_running_toolchain_is_inside_its_own_window() {
        let runtime = env!("CARGO_PKG_VERSION");
        assert!(toolchain_in_window(runtime, runtime));
        assert!(toolchain_in_window(
            crate::LANGUAGE_VERSION,
            crate::LANGUAGE_VERSION
        ));
        let (major, minor) = toolchain_line(runtime).expect("Cargo versions are SemVer");
        assert!(!toolchain_in_window(
            &format!("{major}.{}.0", minor + 1),
            runtime
        ));
        assert!(!toolchain_in_window(&format!("{}.0.0", major + 1), runtime));
    }

    #[test]
    fn labels_never_echo_unparsable_input() {
        assert_eq!(window_label("0.3.1"), "0.1 through 0.3");
        assert_eq!(window_label("0.1.0"), "0.0 through 0.1");
        assert_eq!(toolchain_label("0.4.2-rc.1"), Some("0.4".to_owned()));
        assert_eq!(toolchain_label("0.4\u{1b}[31m"), None);
    }

    #[test]
    fn runtime_wit_versions_come_only_from_krit_runtime_names() {
        assert_eq!(
            runtime_wit_version("krit:runtime/stdout@0.2.0"),
            Some("0.2.0")
        );
        assert_eq!(
            runtime_wit_version("krit:runtime/state@9.0.0"),
            Some("9.0.0")
        );
        assert_eq!(runtime_wit_version("krit:runtime/@0.2.0"), None);
        assert_eq!(runtime_wit_version("wasi:cli/stdout@0.2.0"), None);
        assert_eq!(runtime_wit_version("run"), None);
        for name in [
            crate::STDOUT_INTERFACE,
            crate::STATE_INTERFACE,
            crate::WEBHOOK_INTERFACE,
            crate::PROGRAM_WORLD,
        ] {
            let version = runtime_wit_version(name).expect("built-in names are versioned");
            assert!(SUPPORTED_RUNTIME_WIT_VERSIONS.contains(&version), "{name}");
        }
    }
}
