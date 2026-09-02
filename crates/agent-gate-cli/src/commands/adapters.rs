//! Install, remove, and inspect the agent hook wiring.
//!
//! Both supported agents keep their hook config in a file the user also edits
//! by hand, so every write here is conservative: existing content is preserved
//! (comments and formatting included, for TOML), a backup is written first,
//! and installing twice is a no-op rather than a duplicate hook.

use crate::commands::hook::Adapter;
use crate::paths;
use anyhow::{Context, Result};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

/// Where an adapter's hook configuration lives.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scope {
    /// The user's global agent config, applying to every project.
    Global,
    /// This project only.
    Project,
}

pub struct Target {
    adapter: Adapter,
    scope: Scope,
    path: PathBuf,
}

impl Target {
    pub fn resolve(adapter: Adapter, scope: Scope, project: &Path) -> Result<Target> {
        let home = home_dir()?;
        let path = match (adapter, scope) {
            (Adapter::ClaudeCode, Scope::Global) => home.join(".claude/settings.json"),
            (Adapter::ClaudeCode, Scope::Project) => project.join(".claude/settings.json"),
            // Codex ignores hook config in project-local layers unless that
            // layer is trusted, so the global file is the reliable target.
            (Adapter::Codex, _) => home.join(".codex/config.toml"),
        };
        Ok(Target {
            adapter,
            scope,
            path,
        })
    }

    fn label(&self) -> String {
        match self.adapter {
            Adapter::ClaudeCode => match self.scope {
                Scope::Global => "claude-code (global)".to_string(),
                Scope::Project => "claude-code (project)".to_string(),
            },
            Adapter::Codex => "codex (global)".to_string(),
        }
    }
}

/// The command string a hook should invoke. The absolute path to this binary
/// is used rather than a bare name, so the hook works without `agent-gate`
/// being on the agent's `PATH`.
fn hook_command(adapter: Adapter) -> Result<String> {
    let exe = std::env::current_exe().context("locating the agent-gate binary")?;
    let subcommand = match adapter {
        Adapter::ClaudeCode => "claude-code",
        Adapter::Codex => "codex",
    };
    Ok(format!("{} hook {subcommand}", exe.display()))
}

/// Recognises our hook regardless of which binary path installed it, so an
/// entry written by an older checkout is still found and replaced.
fn is_our_hook(command: &str) -> bool {
    command.contains("agent-gate") && command.contains("hook")
}

fn home_dir() -> Result<PathBuf> {
    std::env::var("HOME")
        .map(PathBuf::from)
        .context("HOME is not set")
}

fn back_up(path: &Path) -> Result<()> {
    if path.exists() {
        let backup = path.with_extension(format!(
            "{}.agent-gate-backup",
            path.extension().and_then(|e| e.to_str()).unwrap_or("bak")
        ));
        std::fs::copy(path, &backup)
            .with_context(|| format!("backing up {}", path.display()))?;
        println!("  backed up {} -> {}", path.display(), backup.display());
    }
    Ok(())
}

// ---------------------------------------------------------------- inspection

#[derive(Debug, PartialEq, Eq)]
pub enum State {
    Installed { command: String },
    NotInstalled,
    NoConfigFile,
}

pub fn inspect(target: &Target) -> Result<State> {
    if !target.path.exists() {
        return Ok(State::NoConfigFile);
    }
    let contents = std::fs::read_to_string(&target.path)?;
    let found = match target.adapter {
        Adapter::ClaudeCode => find_in_json(&contents),
        Adapter::Codex => find_in_toml(&contents),
    };
    Ok(match found {
        Some(command) => State::Installed { command },
        None => State::NotInstalled,
    })
}

fn find_in_json(contents: &str) -> Option<String> {
    let doc: Value = serde_json::from_str(contents).ok()?;
    let entries = doc.get("hooks")?.get("PreToolUse")?.as_array()?;
    for entry in entries {
        for hook in entry.get("hooks")?.as_array()? {
            let command = hook.get("command")?.as_str()?;
            if is_our_hook(command) {
                return Some(command.to_string());
            }
        }
    }
    None
}

fn find_in_toml(contents: &str) -> Option<String> {
    let doc = contents.parse::<toml_edit::DocumentMut>().ok()?;
    let entries = doc.get("hooks")?.get("PreToolUse")?.as_array_of_tables()?;
    for entry in entries {
        let hooks = entry.get("hooks")?.as_array_of_tables()?;
        for hook in hooks {
            let command = hook.get("command")?.as_str()?;
            if is_our_hook(command) {
                return Some(command.to_string());
            }
        }
    }
    None
}

// ----------------------------------------------------------------- installing

pub fn install(target: &Target, timeout: u64) -> Result<bool> {
    let command = hook_command(target.adapter)?;
    if let State::Installed { command: existing } = inspect(target)? {
        if existing == command {
            println!("  {} already installed", target.label());
            return Ok(false);
        }
        // A stale entry from a different checkout: replace it.
        uninstall(target)?;
    }
    back_up(&target.path)?;
    if let Some(parent) = target.path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let existing = std::fs::read_to_string(&target.path).unwrap_or_default();
    let updated = match target.adapter {
        Adapter::ClaudeCode => install_json(&existing, &command, timeout)?,
        Adapter::Codex => install_toml(&existing, &command, timeout)?,
    };
    std::fs::write(&target.path, updated)?;
    println!("  installed {} -> {}", target.label(), target.path.display());
    Ok(true)
}

fn install_json(existing: &str, command: &str, timeout: u64) -> Result<String> {
    let mut doc: Value = if existing.trim().is_empty() {
        json!({})
    } else {
        serde_json::from_str(existing).context("parsing existing settings.json")?
    };

    let entry = json!({
        "matcher": "Bash",
        "hooks": [{ "type": "command", "command": command, "timeout": timeout }],
    });

    let hooks = doc
        .as_object_mut()
        .context("settings.json is not an object")?
        .entry("hooks")
        .or_insert_with(|| json!({}));
    let pre = hooks
        .as_object_mut()
        .context("hooks is not an object")?
        .entry("PreToolUse")
        .or_insert_with(|| json!([]));
    pre.as_array_mut()
        .context("PreToolUse is not an array")?
        .push(entry);

    Ok(format!("{}\n", serde_json::to_string_pretty(&doc)?))
}

fn install_toml(existing: &str, command: &str, timeout: u64) -> Result<String> {
    let mut doc = existing
        .parse::<toml_edit::DocumentMut>()
        .context("parsing existing config.toml")?;

    let hooks = doc
        .entry("hooks")
        .or_insert(toml_edit::Item::Table(toml_edit::Table::new()))
        .as_table_mut()
        .context("hooks is not a table")?;
    hooks.set_implicit(true);

    let pre = hooks
        .entry("PreToolUse")
        .or_insert(toml_edit::Item::ArrayOfTables(
            toml_edit::ArrayOfTables::new(),
        ))
        .as_array_of_tables_mut()
        .context("hooks.PreToolUse is not an array of tables")?;

    let mut inner = toml_edit::Table::new();
    inner["type"] = toml_edit::value("command");
    inner["command"] = toml_edit::value(command);
    inner["timeout"] = toml_edit::value(timeout as i64);
    inner["statusMessage"] = toml_edit::value("Checking with Local Agent Gate");

    let mut nested = toml_edit::ArrayOfTables::new();
    nested.push(inner);

    let mut entry = toml_edit::Table::new();
    // Codex matches on a regular expression, unlike Claude Code's plain string.
    entry["matcher"] = toml_edit::value("^Bash$");
    entry.insert("hooks", toml_edit::Item::ArrayOfTables(nested));

    pre.push(entry);
    Ok(doc.to_string())
}

// --------------------------------------------------------------- uninstalling

pub fn uninstall(target: &Target) -> Result<bool> {
    match inspect(target)? {
        State::NoConfigFile | State::NotInstalled => {
            println!("  {} not installed", target.label());
            return Ok(false);
        }
        State::Installed { .. } => {}
    }
    back_up(&target.path)?;
    let existing = std::fs::read_to_string(&target.path)?;
    let updated = match target.adapter {
        Adapter::ClaudeCode => uninstall_json(&existing)?,
        Adapter::Codex => uninstall_toml(&existing)?,
    };
    std::fs::write(&target.path, updated)?;
    println!("  removed {} from {}", target.label(), target.path.display());
    Ok(true)
}

fn uninstall_json(existing: &str) -> Result<String> {
    let mut doc: Value = serde_json::from_str(existing)?;
    if let Some(pre) = doc
        .get_mut("hooks")
        .and_then(|h| h.get_mut("PreToolUse"))
        .and_then(|p| p.as_array_mut())
    {
        for entry in pre.iter_mut() {
            if let Some(hooks) = entry.get_mut("hooks").and_then(|h| h.as_array_mut()) {
                hooks.retain(|h| {
                    !h.get("command")
                        .and_then(|c| c.as_str())
                        .is_some_and(is_our_hook)
                });
            }
        }
        // Drop entries whose hook list we just emptied.
        pre.retain(|e| {
            e.get("hooks")
                .and_then(|h| h.as_array())
                .is_none_or(|h| !h.is_empty())
        });
    }
    Ok(format!("{}\n", serde_json::to_string_pretty(&doc)?))
}

fn uninstall_toml(existing: &str) -> Result<String> {
    let mut doc = existing.parse::<toml_edit::DocumentMut>()?;
    if let Some(pre) = doc
        .get_mut("hooks")
        .and_then(|h| h.get_mut("PreToolUse"))
        .and_then(|p| p.as_array_of_tables_mut())
    {
        pre.retain(|entry| {
            let ours = entry
                .get("hooks")
                .and_then(|h| h.as_array_of_tables())
                .is_some_and(|hooks| {
                    hooks.iter().any(|h| {
                        h.get("command")
                            .and_then(|c| c.as_str())
                            .is_some_and(is_our_hook)
                    })
                });
            !ours
        });
    }
    Ok(doc.to_string())
}

// -------------------------------------------------------------------- command

pub async fn list(project: &Path) -> Result<()> {
    let socket = paths::socket_path();
    // `/pending` rather than `/health`: it is a real JSON endpoint, so a
    // successful parse proves the daemon is serving rather than merely that a
    // stale socket file is lying around.
    match crate::client::get_json(&socket, "/pending").await {
        Ok(pending) => {
            let waiting = pending.as_array().map(|a| a.len()).unwrap_or(0);
            println!("daemon: running ({waiting} pending) at {}", socket.display());
        }
        Err(err) => println!("daemon: not reachable at {} ({err})", socket.display()),
    }
    println!();

    for (adapter, scope) in [
        (Adapter::ClaudeCode, Scope::Project),
        (Adapter::ClaudeCode, Scope::Global),
        (Adapter::Codex, Scope::Global),
    ] {
        let target = Target::resolve(adapter, scope, project)?;
        let state = inspect(&target)?;
        let status = match &state {
            State::Installed { command } => format!("installed  ({command})"),
            State::NotInstalled => "not installed".to_string(),
            State::NoConfigFile => "no config file".to_string(),
        };
        println!("{:<22} {}", target.label(), status);
        println!("{:<22} {}", "", target.path.display());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const CMD: &str = "/usr/local/bin/agent-gate hook claude-code";

    #[test]
    fn installs_into_empty_json_and_is_idempotent_by_content() {
        let out = install_json("", CMD, 130).unwrap();
        assert_eq!(find_in_json(&out).as_deref(), Some(CMD));
        let doc: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(doc["hooks"]["PreToolUse"][0]["matcher"], json!("Bash"));
        assert_eq!(doc["hooks"]["PreToolUse"][0]["hooks"][0]["timeout"], json!(130));
    }

    #[test]
    fn json_install_preserves_unrelated_settings() {
        let existing = r#"{"model":"opus","hooks":{"PostToolUse":[{"matcher":"Bash"}]}}"#;
        let out = install_json(existing, CMD, 130).unwrap();
        let doc: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(doc["model"], json!("opus"));
        assert!(doc["hooks"]["PostToolUse"].is_array());
        assert_eq!(find_in_json(&out).as_deref(), Some(CMD));
    }

    #[test]
    fn json_uninstall_removes_only_our_hook() {
        let existing = r#"{"hooks":{"PreToolUse":[
            {"matcher":"Bash","hooks":[{"type":"command","command":"other-tool check"}]}
        ]}}"#;
        let installed = install_json(existing, CMD, 130).unwrap();
        assert_eq!(find_in_json(&installed).as_deref(), Some(CMD));

        let removed = uninstall_json(&installed).unwrap();
        assert_eq!(find_in_json(&removed), None);
        assert!(removed.contains("other-tool check"), "foreign hook survives");
    }

    #[test]
    fn toml_install_preserves_comments_and_other_config() {
        let existing = "# my notes\nmodel = \"gpt-5\"\n\n[mcp_servers.thing]\ncommand = \"x\"\n";
        let out = install_toml(existing, "/usr/local/bin/agent-gate hook codex", 130).unwrap();
        assert!(out.contains("# my notes"), "comments preserved");
        assert!(out.contains("[mcp_servers.thing]"), "other tables preserved");
        assert_eq!(
            find_in_toml(&out).as_deref(),
            Some("/usr/local/bin/agent-gate hook codex")
        );
        assert!(out.contains("^Bash$"), "codex matcher is a regex");
    }

    #[test]
    fn toml_uninstall_leaves_the_rest_intact() {
        let existing = "model = \"gpt-5\"\n";
        let installed = install_toml(existing, "/x/agent-gate hook codex", 130).unwrap();
        let removed = uninstall_toml(&installed).unwrap();
        assert_eq!(find_in_toml(&removed), None);
        assert!(removed.contains("model = \"gpt-5\""));
    }

    #[test]
    fn foreign_hooks_are_not_mistaken_for_ours() {
        assert!(!is_our_hook("some-other-gate hook claude-code"));
        assert!(is_our_hook("/opt/bin/agent-gate hook codex"));
    }
}
