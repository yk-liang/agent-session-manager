use crate::agents::{find_executable, is_executable};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

const APP_EXECUTABLES: &[&str] = &[
    "Contents/Resources/codex",
    "Contents/Resources/codex-cli/CodexCLI.app/Contents/MacOS/codex",
    "Contents/Resources/codex-cli/bin/codex",
];

pub(super) fn executables() -> Vec<PathBuf> {
    let mut paths: Vec<_> = find_executable("codex").into_iter().collect();
    let home = dirs::home_dir();
    if let Some(home) = &home {
        paths.extend(app_executables(&home.join("Applications")));
    }
    paths.extend(app_executables(Path::new("/Applications")));
    if let Some(home) = &home {
        for directory in [".vscode", ".vscode-insiders"] {
            paths.extend(extension_executables(
                &home.join(directory).join("extensions"),
            ));
        }
    }
    let mut seen = HashSet::new();
    paths.retain(|path| {
        is_executable(path) && seen.insert(path.canonicalize().unwrap_or_else(|_| path.clone()))
    });
    paths
}

fn app_executables(directory: &Path) -> impl Iterator<Item = PathBuf> + '_ {
    ["ChatGPT.app", "Codex.app"]
        .into_iter()
        .filter_map(move |app| {
            let bundle = directory.join(app);
            APP_EXECUTABLES
                .iter()
                .map(|relative| bundle.join(relative))
                .find(|path| is_executable(path))
        })
}

fn extension_executables(directory: &Path) -> Vec<PathBuf> {
    let platform = match std::env::consts::ARCH {
        "aarch64" => "macos-aarch64",
        "x86_64" => "macos-x86_64",
        _ => return Vec::new(),
    };
    let Ok(entries) = directory.read_dir() else {
        return Vec::new();
    };
    let mut candidates: Vec<_> = entries
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name();
            let version = name
                .to_str()?
                .strip_prefix("openai.chatgpt-")?
                .split('-')
                .next()?;
            let version = version
                .split('.')
                .map(str::parse::<u32>)
                .collect::<Result<Vec<_>, _>>()
                .ok()?;
            Some((
                version,
                entry.path().join("bin").join(platform).join("codex"),
            ))
        })
        .collect();
    candidates.sort_unstable_by(|a, b| b.cmp(a));
    candidates.into_iter().map(|(_, path)| path).collect()
}
