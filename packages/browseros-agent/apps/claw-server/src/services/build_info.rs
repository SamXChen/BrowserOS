//! What this binary is, checked against the artifact manifest shipped beside it.
//!
//! The deployment problem this answers: a hand-replaced binary or a stale resource copy
//! looks exactly like a fresh install from the outside, so "which build is running" was
//! unanswerable. `releaseSha` cannot answer it either — the build records
//! `git rev-parse HEAD`, which repeats across builds of uncommitted work.
//!
//! A mismatch lands in `health` and in the log while the process keeps serving: a server
//! that names its own mismatch is easier to diagnose than a closed port.

use std::{
    fs,
    path::{Path, PathBuf},
};

use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::VERSION;

/// The manifest entry that describes the server binary itself.
const BINARY_ENTRY: &str = "resources/bin/browseros-claw-server";

/// Distance from the executable to its release directory:
/// `<release>/resources/bin/browseros-claw-server`.
const RELEASE_DIR_DEPTH: usize = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Integrity {
    /// The hash is still being computed.
    Pending,
    /// The manifest lists this exact binary.
    Ok,
    /// The manifest lists a different binary.
    Mismatch,
    /// There is no manifest to compare against.
    Absent,
}

impl Integrity {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Ok => "ok",
            Self::Mismatch => "mismatch",
            Self::Absent => "absent",
        }
    }
}

#[derive(Debug, Clone)]
pub struct BuildInfo {
    pub version: String,
    pub release_sha: Option<String>,
    pub binary_sha256: Option<String>,
    pub integrity: Integrity,
    /// Why the answer is `absent` or `mismatch`; logged at startup, not served.
    pub detail: Option<String>,
}

impl BuildInfo {
    #[must_use]
    pub fn pending() -> Self {
        Self {
            version: VERSION.to_string(),
            release_sha: None,
            binary_sha256: None,
            integrity: Integrity::Pending,
            detail: None,
        }
    }

    #[must_use]
    pub fn unknown(reason: impl Into<String>) -> Self {
        Self {
            version: VERSION.to_string(),
            release_sha: None,
            binary_sha256: None,
            integrity: Integrity::Absent,
            detail: Some(reason.into()),
        }
    }
}

#[derive(Debug, Deserialize)]
struct Manifest {
    #[serde(rename = "releaseSha")]
    release_sha: Option<String>,
    files: Vec<ManifestFile>,
}

#[derive(Debug, Deserialize)]
struct ManifestFile {
    path: String,
    sha256: String,
}

/// The manifest that ships next to the binary, derived from the running executable so a
/// binary moved into `releases/<sha>/` still finds its own manifest.
#[must_use]
pub fn manifest_path_for(exe: &Path) -> Option<PathBuf> {
    // `<release>/resources/bin/browseros-claw-server` → `<release>/artifact-metadata.json`
    exe.ancestors()
        .nth(RELEASE_DIR_DEPTH)
        .map(|release| release.join("artifact-metadata.json"))
}

/// Compares the running executable with its manifest. Pure, so the cases are testable
/// without a real build.
#[must_use]
pub fn evaluate(exe: &Path, manifest_path: &Path) -> BuildInfo {
    let bytes = match fs::read(exe) {
        Ok(bytes) => bytes,
        Err(error) => return BuildInfo::unknown(format!("cannot read {}: {error}", exe.display())),
    };
    let binary_sha256 = hex(&Sha256::digest(&bytes));
    let raw = match fs::read_to_string(manifest_path) {
        Ok(raw) => raw,
        Err(error) => {
            return BuildInfo {
                binary_sha256: Some(binary_sha256),
                ..BuildInfo::unknown(format!(
                    "no manifest at {}: {error}",
                    manifest_path.display()
                ))
            };
        }
    };
    let manifest: Manifest = match serde_json::from_str(&raw) {
        Ok(manifest) => manifest,
        Err(error) => {
            return BuildInfo {
                binary_sha256: Some(binary_sha256),
                ..BuildInfo::unknown(format!("manifest is not valid JSON: {error}"))
            };
        }
    };
    let expected = manifest
        .files
        .iter()
        .find(|file| file.path == BINARY_ENTRY)
        .map(|file| file.sha256.clone());
    match expected {
        Some(expected) if expected == binary_sha256 => BuildInfo {
            version: VERSION.to_string(),
            release_sha: manifest.release_sha,
            binary_sha256: Some(binary_sha256),
            integrity: Integrity::Ok,
            detail: None,
        },
        Some(expected) => BuildInfo {
            version: VERSION.to_string(),
            release_sha: manifest.release_sha,
            binary_sha256: Some(binary_sha256),
            integrity: Integrity::Mismatch,
            detail: Some(format!("manifest lists {expected}")),
        },
        None => BuildInfo {
            binary_sha256: Some(binary_sha256),
            ..BuildInfo::unknown(format!("manifest has no {BINARY_ENTRY} entry"))
        },
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use sha2::{Digest, Sha256};

    use super::{BuildInfo, Integrity, evaluate, manifest_path_for};

    fn layout(content: &[u8]) -> (tempfile::TempDir, std::path::PathBuf, std::path::PathBuf) {
        let root = tempdir().expect("tempdir");
        let release = root.path().join("releases/abc123");
        let bin = release.join("resources/bin/browseros-claw-server");
        fs::create_dir_all(bin.parent().expect("parent")).expect("mkdir");
        fs::write(&bin, content).expect("write binary");
        let manifest = release.join("artifact-metadata.json");
        (root, bin, manifest)
    }

    fn manifest_for(hash: &str) -> String {
        format!(
            r#"{{"version":"0.0.1","releaseSha":"deadbeef","files":[{{"path":"resources/bin/browseros-claw-server","sha256":"{hash}","size":1}}]}}"#
        )
    }

    #[test]
    fn a_matching_manifest_is_ok() {
        let (_root, bin, manifest_path) = layout(b"binary");
        let hash = super::hex(&Sha256::digest(b"binary"));
        fs::write(&manifest_path, manifest_for(&hash)).expect("write manifest");
        let info = evaluate(&bin, &manifest_path);
        assert_eq!(info.integrity, Integrity::Ok);
        assert_eq!(info.binary_sha256.as_deref(), Some(hash.as_str()));
        assert_eq!(info.release_sha.as_deref(), Some("deadbeef"));
    }

    #[test]
    fn a_different_binary_is_reported_not_refused() {
        let (_root, bin, manifest_path) = layout(b"replaced");
        fs::write(&manifest_path, manifest_for(&"a".repeat(64))).expect("write manifest");
        let info = evaluate(&bin, &manifest_path);
        assert_eq!(info.integrity, Integrity::Mismatch);
        assert!(info.detail.unwrap_or_default().contains("manifest lists"));
    }

    #[test]
    fn a_missing_manifest_is_absent() {
        let (_root, bin, manifest_path) = layout(b"binary");
        let info = evaluate(&bin, &manifest_path);
        assert_eq!(info.integrity, Integrity::Absent);
        assert!(info.binary_sha256.is_some());
    }

    #[test]
    fn broken_json_is_absent_with_a_reason() {
        let (_root, bin, manifest_path) = layout(b"binary");
        fs::write(&manifest_path, "{not json").expect("write manifest");
        let info: BuildInfo = evaluate(&bin, &manifest_path);
        assert_eq!(info.integrity, Integrity::Absent);
        assert!(info.detail.unwrap_or_default().contains("not valid JSON"));
    }

    #[test]
    fn the_manifest_sits_beside_the_release_directory() {
        let (_root, bin, manifest_path) = layout(b"binary");
        assert_eq!(manifest_path_for(&bin), Some(manifest_path));
    }
}
