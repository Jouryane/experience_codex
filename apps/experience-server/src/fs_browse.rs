//! Local file browsing for the Agent form: users open the agent application
//! directory and pick the corresponding .exe / .lnk themselves. Experience
//! only reads directory listings and shortcut metadata — it never guesses
//! install paths for the user.

use std::path::Path;
use std::path::PathBuf;
use std::process::Command;

use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct FsEntry {
    pub name: String,
    pub path: String,
    pub is_dir: bool,
}

pub fn list_roots() -> Vec<String> {
    #[cfg(windows)]
    {
        ('A'..='Z')
            .map(|letter| format!("{letter}:\\"))
            .filter(|root| Path::new(root).exists())
            .collect()
    }
    #[cfg(not(windows))]
    {
        vec!["/".to_string()]
    }
}

const SELECTABLE_EXTENSIONS: &[&str] = &["exe", "lnk", "cmd", "bat", "com"];

fn is_selectable(name: &str) -> bool {
    Path::new(name)
        .extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| {
            SELECTABLE_EXTENSIONS.contains(&ext.to_lowercase().as_str())
        })
        .unwrap_or(false)
}

/// List a directory: subdirectories plus selectable launcher files.
pub fn browse(dir: &str) -> Result<(String, Vec<FsEntry>), String> {
    let path = PathBuf::from(dir);
    let path = path
        .canonicalize()
        .map_err(|error| format!("无法访问 {dir}: {error}"))?;
    if !path.is_dir() {
        return Err(format!("不是目录：{}", path.display()));
    }
    let mut directories = Vec::new();
    let mut files = Vec::new();
    let entries = std::fs::read_dir(&path)
        .map_err(|error| format!("读取目录失败 {}: {error}", path.display()))?;
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') {
            continue;
        }
        let full = entry.path();
        let is_dir = entry.file_type().map(|kind| kind.is_dir()).unwrap_or(false);
        if is_dir {
            directories.push(FsEntry {
                name,
                path: strip_extended_prefix(&full),
                is_dir: true,
            });
        } else if is_selectable(&name) {
            files.push(FsEntry {
                name,
                path: strip_extended_prefix(&full),
                is_dir: false,
            });
        }
    }
    let sort_key = |entry: &FsEntry| entry.name.to_lowercase();
    directories.sort_by_key(sort_key);
    files.sort_by_key(sort_key);
    directories.extend(files);
    Ok((strip_extended_prefix(&path), directories))
}

fn strip_extended_prefix(path: &Path) -> String {
    let text = path.to_string_lossy();
    text.strip_prefix(r"\\?\").unwrap_or(&text).to_string()
}

/// Resolve a Windows shortcut (.lnk) to (target, arguments, working dir).
pub fn shortcut_target(path: &str) -> Result<(String, String, String), String> {
    #[cfg(windows)]
    {
        let script = r#"
[Console]::OutputEncoding=[System.Text.Encoding]::UTF8
$s=(New-Object -ComObject WScript.Shell).CreateShortcut($env:EXP_LNK_PATH)
Write-Output $s.TargetPath
Write-Output $s.Arguments
Write-Output $s.WorkingDirectory
"#;
        let output = Command::new("powershell")
            .args(["-NoProfile", "-NonInteractive", "-Command", script])
            .env("EXP_LNK_PATH", path)
            .output()
            .map_err(|error| format!("无法解析快捷方式：{error}"))?;
        if !output.status.success() {
            return Err(format!(
                "快捷方式解析失败（exit {:?}）",
                output.status.code()
            ));
        }
        let lines: Vec<String> = String::from_utf8_lossy(&output.stdout)
            .lines()
            .map(str::trim)
            .map(str::to_string)
            .take(3)
            .collect();
        if lines.len() < 3 {
            return Err(format!("快捷方式信息不完整：{path}"));
        }
        Ok((lines[0].clone(), lines[1].clone(), lines[2].clone()))
    }
    #[cfg(not(windows))]
    {
        let _ = path;
        Err("仅 Windows 支持 .lnk".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn browse_lists_directories_and_selectable_launchers_only() {
        let dir = std::env::temp_dir().join(format!(
            "exp-fs-browse-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        std::fs::write(dir.join("agent.exe"), b"x").unwrap();
        std::fs::write(dir.join("notes.txt"), b"x").unwrap();
        let (_, entries) = browse(dir.to_str().unwrap()).unwrap();
        let names: Vec<&str> = entries.iter().map(|entry| entry.name.as_str()).collect();
        assert!(names.contains(&"agent.exe"));
        assert!(names.contains(&"sub"));
        assert!(!names.contains(&"notes.txt"));
        assert!(!list_roots().is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
