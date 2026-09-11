use std::{path::Path, process::Command};

use crate::error::AppError;
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EditorInfo {
    pub id: String,
    pub label: String,
    pub command: String,
}

struct Editor {
    id: &'static str,
    label: &'static str,
    commands: &'static [&'static str],
    windows_paths: &'static [&'static str],
    terminal: bool,
}
const EDITORS: &[Editor] = &[
    Editor {
        id: "vscode",
        label: "VS Code",
        commands: &["code"],
        windows_paths: &[
            "Programs/Microsoft VS Code/bin/code.cmd",
            "C:/Program Files/Microsoft VS Code/bin/code.cmd",
        ],
        terminal: false,
    },
    Editor {
        id: "vscode-insiders",
        label: "VS Code Insiders",
        commands: &["code-insiders"],
        windows_paths: &[
            "Programs/Microsoft VS Code Insiders/bin/code-insiders.cmd",
            "C:/Program Files/Microsoft VS Code Insiders/bin/code-insiders.cmd",
        ],
        terminal: false,
    },
    Editor {
        id: "cursor",
        label: "Cursor",
        commands: &["cursor"],
        windows_paths: &["Programs/cursor/resources/app/bin/cursor.cmd"],
        terminal: false,
    },
    Editor {
        id: "windsurf",
        label: "Windsurf",
        commands: &["windsurf"],
        windows_paths: &["Programs/Windsurf/resources/app/bin/windsurf.cmd"],
        terminal: false,
    },
    Editor {
        id: "zed",
        label: "Zed",
        commands: &["zed"],
        windows_paths: &["Programs/Zed/Zed.exe"],
        terminal: false,
    },
    Editor {
        id: "jetbrains",
        label: "JetBrains IDE",
        commands: &["idea", "idea64"],
        windows_paths: &["JetBrains/Toolbox/scripts/idea.cmd"],
        terminal: false,
    },
    Editor {
        id: "nvim",
        label: "Neovim",
        commands: &["nvim"],
        windows_paths: &[],
        terminal: true,
    },
    Editor {
        id: "vim",
        label: "Vim",
        commands: &["vim"],
        windows_paths: &[],
        terminal: true,
    },
    Editor {
        id: "subl",
        label: "Sublime Text",
        commands: &["subl"],
        windows_paths: &[],
        terminal: false,
    },
];

fn find_in_path(command: &str) -> Option<String> {
    #[cfg(target_os = "windows")]
    let output = {
        use std::os::windows::process::CommandExt;
        Command::new("where.exe")
            .arg(command)
            .creation_flags(0x0800_0000)
            .output()
            .ok()?
    };
    #[cfg(not(target_os = "windows"))]
    let output = Command::new("which").arg(command).output().ok()?;
    output
        .status
        .success()
        .then(|| {
            String::from_utf8_lossy(&output.stdout)
                .lines()
                .next()
                .map(str::to_string)
        })
        .flatten()
}
fn resolve(editor: &Editor) -> Option<String> {
    editor
        .commands
        .iter()
        .find_map(|command| find_in_path(command))
        .or_else(|| {
            #[cfg(target_os = "windows")]
            {
                editor.windows_paths.iter().find_map(|path| {
                    let path = if let Some(relative) = path.strip_prefix("C:/") {
                        std::path::PathBuf::from("C:/").join(relative)
                    } else {
                        std::env::var_os("LOCALAPPDATA")
                            .map(std::path::PathBuf::from)?
                            .join(path)
                    };
                    path.exists().then(|| path.to_string_lossy().into_owned())
                })
            }
            #[cfg(not(target_os = "windows"))]
            {
                None
            }
        })
}
pub fn detect_editors() -> Vec<EditorInfo> {
    EDITORS
        .iter()
        .filter_map(|editor| {
            resolve(editor).map(|command| EditorInfo {
                id: editor.id.into(),
                label: editor.label.into(),
                command,
            })
        })
        .collect()
}
pub fn launch_editor(editor_id: &str, worktree_path: &Path) -> Result<(), AppError> {
    let editor = EDITORS
        .iter()
        .find(|editor| editor.id == editor_id)
        .ok_or_else(|| AppError::InvalidOperation(format!("Unsupported editor '{editor_id}'")))?;
    let command = resolve(editor).ok_or_else(|| {
        AppError::InvalidOperation(format!("Editor '{}' is no longer available", editor.label))
    })?;
    let mut launch = if editor.terminal {
        #[cfg(target_os = "windows")]
        {
            let mut cmd = Command::new("cmd.exe");
            cmd.args(["/K", &command, &worktree_path.to_string_lossy()]);
            cmd
        }
        #[cfg(not(target_os = "windows"))]
        {
            if find_in_path("x-terminal-emulator").is_none() {
                return Err(AppError::InvalidOperation(
                    "x-terminal-emulator is required to open terminal editors".into(),
                ));
            }
            let mut cmd = Command::new("x-terminal-emulator");
            cmd.args(["-e", &command, &worktree_path.to_string_lossy()]);
            cmd
        }
    } else {
        let mut cmd = Command::new(command);
        cmd.arg(worktree_path);
        cmd
    };
    launch.spawn().map(|_| ()).map_err(|error| {
        AppError::InvalidOperation(format!(
            "Failed to launch editor '{}': {error}",
            editor.label
        ))
    })
}
