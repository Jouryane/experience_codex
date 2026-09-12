//! P0 workspace containment: every Experience/Executor file write must go
//! through `safe_join`, which refuses anything that could escape the
//! workspace (absolute/drive/UNC paths, `..`, symlink escape, ADS, reserved
//! Windows device names).

use std::path::Path;
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FsGuardError {
    Empty,
    Absolute,
    DriveRelative,
    Unc,
    Escapes,
    Ads,
    Reserved,
    SymlinkEscape,
    WorkspaceMissing,
    Io(String),
}

impl std::fmt::Display for FsGuardError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FsGuardError::Empty => write!(formatter, "empty path"),
            FsGuardError::Absolute => write!(formatter, "absolute path rejected"),
            FsGuardError::DriveRelative => write!(formatter, "drive-relative path rejected"),
            FsGuardError::Unc => write!(formatter, "UNC path rejected"),
            FsGuardError::Escapes => write!(formatter, "path escapes the workspace"),
            FsGuardError::Ads => write!(formatter, "alternate data stream rejected"),
            FsGuardError::Reserved => write!(formatter, "reserved device name rejected"),
            FsGuardError::SymlinkEscape => write!(formatter, "symlink escapes the workspace"),
            FsGuardError::WorkspaceMissing => write!(formatter, "workspace does not exist"),
            FsGuardError::Io(error) => write!(formatter, "io error: {error}"),
        }
    }
}

fn is_reserved_component(component: &str) -> bool {
    let stem = component
        .split('.')
        .next()
        .unwrap_or(component)
        .trim()
        .to_ascii_uppercase();
    matches!(
        stem.as_str(),
        "CON" | "PRN" | "AUX" | "NUL"
    ) || (stem.len() == 4
        && (stem.starts_with("COM") || stem.starts_with("LPT"))
        && stem.as_bytes()[3].is_ascii_digit())
}

/// Join `relative` under `workspace`, refusing escape vectors.
pub fn safe_join(workspace: &Path, relative: &str) -> Result<PathBuf, FsGuardError> {
    let text = relative.trim();
    if text.is_empty() {
        return Err(FsGuardError::Empty);
    }
    if text.starts_with(r"\\") || text.starts_with("//") {
        return Err(FsGuardError::Unc);
    }
    let bytes = text.as_bytes();
    if bytes.len() >= 2 && bytes[1] == b':' {
        if bytes[0].is_ascii_alphabetic() {
            return if bytes.len() == 2 || bytes[2] != b'\\' && bytes[2] != b'/' {
                Err(FsGuardError::DriveRelative)
            } else {
                Err(FsGuardError::Absolute)
            };
        }
    }
    if text.starts_with('/') || text.starts_with('\\') {
        return Err(FsGuardError::Absolute);
    }

    let canonical_root = workspace
        .canonicalize()
        .map_err(|_| FsGuardError::WorkspaceMissing)?;
    let root_text = canonical_root
        .to_string_lossy()
        .trim_end_matches(['\\', '/'])
        .to_ascii_lowercase();

    let mut candidate = canonical_root.clone();
    for raw in text.split(['/', '\\']) {
        let component = raw.trim();
        if component.is_empty() || component == "." {
            continue;
        }
        if component.contains(':') {
            return Err(FsGuardError::Ads);
        }
        if component == ".." {
            return Err(FsGuardError::Escapes);
        }
        if is_reserved_component(component) {
            return Err(FsGuardError::Reserved);
        }
        candidate.push(component);
    }

    // Lexical check (covers cases where nothing exists yet).
    let candidate_text = candidate.to_string_lossy().to_ascii_lowercase();
    if !(candidate_text == root_text
        || candidate_text.starts_with(&format!("{root_text}\\"))
        || candidate_text.starts_with(&format!("{root_text}/")))
    {
        return Err(FsGuardError::Escapes);
    }

    // Symlink check: canonicalize the nearest existing ancestor.
    let mut ancestor = candidate.clone();
    loop {
        if ancestor.exists() {
            let canonical = ancestor
                .canonicalize()
                .map_err(|error| FsGuardError::Io(error.to_string()))?;
            let canonical_text = canonical.to_string_lossy().to_ascii_lowercase();
            if !(canonical_text == root_text
                || canonical_text.starts_with(&format!("{root_text}\\"))
                || canonical_text.starts_with(&format!("{root_text}/")))
            {
                return Err(FsGuardError::SymlinkEscape);
            }
            break;
        }
        match ancestor.parent() {
            Some(parent) if parent != ancestor => ancestor = parent.to_path_buf(),
            _ => break,
        }
    }
    Ok(candidate)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_ws(tag: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("exp-safety-{tag}-{nanos}"));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn accepts_normal_nested_paths() {
        let ws = temp_ws("ok");
        let canonical_ws = ws.canonicalize().unwrap();
        let joined = safe_join(&ws, "src/index.ts").unwrap();
        assert!(joined.starts_with(&canonical_ws));
        let joined = safe_join(&ws, "docs/README.md").unwrap();
        assert!(joined.starts_with(&canonical_ws));
        let _ = std::fs::remove_dir_all(&ws);
    }

    #[test]
    fn rejects_escape_vectors() {
        let ws = temp_ws("reject");
        for bad in [
            r"C:\Windows\evil.txt",
            r"C:evil.txt",
            r"\\server\share\evil.txt",
            r"..\..\evil.txt",
            "../evil.txt",
            "/etc/passwd",
            r"file.txt:stream",
            "CON",
            r"sub\..\..\evil.txt",
        ] {
            assert!(safe_join(&ws, bad).is_err(), "must reject {bad}");
        }
        let _ = std::fs::remove_dir_all(&ws);
    }

    #[test]
    fn case_insensitive_root_is_accepted() {
        let ws = temp_ws("CaseRoot");
        let upper = ws.to_string_lossy().to_uppercase();
        let joined = safe_join(Path::new(&upper), "a.txt").unwrap();
        assert!(joined.to_string_lossy().to_lowercase().contains("a.txt"));
        let _ = std::fs::remove_dir_all(&ws);
    }
}
