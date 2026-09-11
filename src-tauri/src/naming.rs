use std::{
    io::Read,
    path::Path,
    process::{Command, Stdio},
    sync::mpsc,
    time::Duration,
};

use serde::Serialize;

use crate::runner::configure_child;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskNaming {
    pub title: String,
    pub branch: String,
}

pub fn sanitize_slug(input: &str) -> String {
    let mut slug = String::new();
    let mut previous_dash = false;
    for character in input.chars() {
        if character.is_ascii_lowercase() || character.is_ascii_digit() {
            slug.push(character);
            previous_dash = false;
        } else if character.is_ascii_uppercase() {
            slug.push(character.to_ascii_lowercase());
            previous_dash = false;
        } else if !slug.is_empty() && !previous_dash {
            slug.push('-');
            previous_dash = true;
        }
    }
    let slug = slug.trim_matches('-');
    let capped = &slug[..slug
        .char_indices()
        .nth(40)
        .map(|(index, _)| index)
        .unwrap_or(slug.len())];
    let capped = capped.trim_matches('-');
    if capped.is_empty() {
        "task".to_string()
    } else {
        capped.to_string()
    }
}

pub fn fallback_title(prompt: &str) -> String {
    let line = prompt.lines().next().unwrap_or("").trim();
    let capped = &line[..line
        .char_indices()
        .nth(60)
        .map(|(index, _)| index)
        .unwrap_or(line.len())];
    if capped.is_empty() {
        "New task".to_string()
    } else {
        capped.to_string()
    }
}

pub fn fallback_branch_name(prompt: &str, task_id: &str) -> String {
    let suffix: String = task_id.chars().take(6).collect();
    format!("task/{}-{suffix}", sanitize_slug(&fallback_title(prompt)))
}

pub fn naming_prompt(prompt: &str) -> String {
    let task: String = prompt.chars().take(800).collect();
    format!(
        "You are naming a coding task. Reply with ONLY a JSON object, no markdown, in the form {{\"title\":\"<human-readable task title, max 60 chars>\",\"branch\":\"<kebab-case slug, 2-6 words, lowercase letters/digits/hyphens>\"}}. Task:\n{task}"
    )
}

pub fn extract_json_object(text: &str) -> Option<serde_json::Value> {
    crate::commands::extract_json_objects(text)
        .into_iter()
        .next()
}

pub fn extract_text_from_codex_jsonl(stdout: &str) -> Option<String> {
    stdout
        .lines()
        .filter_map(|line| {
            let value: serde_json::Value = serde_json::from_str(line).ok()?;
            (value.get("type")?.as_str()? == "item.completed"
                && value.get("item")?.get("type")?.as_str()? == "agent_message")
                .then(|| {
                    value
                        .get("item")?
                        .get("content")?
                        .as_str()
                        .map(str::to_string)
                })
                .flatten()
        })
        .last()
}

pub fn generate_task_naming(
    agent_id: &str,
    binary: &str,
    model: &str,
    effort: Option<&str>,
    prompt: &str,
    cwd: &Path,
) -> TaskNaming {
    let fallback = TaskNaming {
        title: fallback_title(prompt),
        branch: sanitize_slug(&fallback_title(prompt)),
    };
    let naming_prompt = naming_prompt(prompt);
    let mut command = Command::new(binary);
    match agent_id {
        "codex" => {
            command.args(["exec", "--json", "--sandbox", "read-only", "--model", model]);
            if model.starts_with('o') {
                if let Some(effort) = effort {
                    command.args(["--effort", effort]);
                }
            }
            command.arg(&naming_prompt);
        }
        "claude" => {
            command.args(["-p", &naming_prompt, "--model", model]);
        }
        "opencode" => {
            command.args(["run", &naming_prompt, "--model", model]);
        }
        _ => return fallback,
    }
    command
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    configure_child(&mut command);
    let Ok(mut child) = command.spawn() else {
        return fallback;
    };
    let Some(mut stdout) = child.stdout.take() else {
        return fallback;
    };
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut output = String::new();
        let _ = stdout.read_to_string(&mut output);
        let _ = tx.send(output);
    });
    let Ok(stdout) = rx.recv_timeout(Duration::from_secs(20)) else {
        let _ = child.kill();
        let _ = child.wait();
        return fallback;
    };
    let _ = child.wait();
    let text = if agent_id == "codex" {
        extract_text_from_codex_jsonl(&stdout).unwrap_or(stdout)
    } else {
        stdout
    };
    let object = extract_json_object(&text);
    let title = object
        .as_ref()
        .and_then(|v| v.get("title"))
        .and_then(|v| v.as_str())
        .map(|s| {
            let trimmed = s.trim();
            trimmed[..trimmed
                .char_indices()
                .nth(60)
                .map(|(i, _)| i)
                .unwrap_or(trimmed.len())]
                .to_string()
        })
        .filter(|s| !s.is_empty())
        .unwrap_or(fallback.title);
    let branch = object
        .as_ref()
        .and_then(|v| v.get("branch"))
        .and_then(|v| v.as_str())
        .map(sanitize_slug)
        .filter(|s| s != "task")
        .unwrap_or(fallback.branch);
    TaskNaming { title, branch }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn slug_is_safe_and_capped() {
        assert_eq!(sanitize_slug(" --Hello, WORLD!-- "), "hello-world");
        assert_eq!(sanitize_slug("日本語"), "task");
        assert_eq!(sanitize_slug(&"a".repeat(50)).len(), 40);
    }
    #[test]
    fn fallbacks_are_stable() {
        assert_eq!(fallback_title(" hello\nnext"), "hello");
        assert_eq!(fallback_title(" \nnext"), "New task");
        assert_eq!(
            fallback_branch_name("Hello world", "12345678"),
            "task/hello-world-123456"
        );
    }
    #[test]
    fn object_extraction_ignores_prose() {
        assert_eq!(
            extract_json_object("hi {bad} {\"title\":\"ok\"}").unwrap()["title"],
            "ok"
        );
    }
    #[test]
    fn codex_extraction_uses_last_message() {
        assert_eq!(
            extract_text_from_codex_jsonl(
                "bad\n{\"type\":\"item.completed\",\"item\":{\"type\":\"agent_message\",\"content\":\"one\"}}\n{\"type\":\"item.completed\",\"item\":{\"type\":\"agent_message\",\"content\":\"two\"}}"
            ),
            Some("two".into())
        );
    }
}
