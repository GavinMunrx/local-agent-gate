//! Install, remove, and inspect the agent hook wiring.
//!
//! Every agent keeps its hook config in a file the user also edits by hand, so
//! each write is conservative: existing content is preserved (comments and
//! formatting included, for TOML), a backup is written first, and installing
//! twice is a no-op rather than a duplicate hook.
//!
//! The agents agree on almost nothing about config shape, so [`Format`]
//! carries the differences and the install/uninstall logic is shared.

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

/// How an agent's hook config is laid out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Format {
    /// `{ <container>: { <event>: [ { matcher, hooks: [ {type, command, timeout} ] } ] } }`
    ///
    /// Used by Claude Code and Gemini CLI (container `hooks`), and by
    /// Antigravity, which uses a named container instead of `hooks`.
    NestedJson {
        container: &'static str,
        event: &'static str,
        matcher: &'static str,
    },
    /// Cursor: `{ version: 1, hooks: { beforeShellExecution: [ {command, timeout} ] } }`.
    /// The entries are flat - no matcher, no nested hook list.
    CursorJson,
    /// Codex: `[[hooks.PreToolUse]]` with a nested `[[hooks.PreToolUse.hooks]]`.
    CodexToml,
}

impl Adapter {
    fn format(self) -> Format {
        match self {
            Adapter::ClaudeCode => Format::NestedJson {
                container: "hooks",
                event: "PreToolUse",
                matcher: "Bash",
            },
            Adapter::GeminiCli => Format::NestedJson {
                container: "hooks",
                event: "BeforeTool",
                matcher: "run_shell_command",
            },
            Adapter::Antigravity => Format::NestedJson {
                container: "local-agent-gate",
                event: "PreToolUse",
                matcher: "run_command",
            },
            Adapter::Cursor => Format::CursorJson,
            Adapter::Codex => Format::CodexToml,
        }
    }

    fn hook_subcommand(self) -> &'static str {
        match self {
            Adapter::ClaudeCode => "claude-code",
            Adapter::Codex => "codex",
            Adapter::Cursor => "cursor",
            Adapter::GeminiCli => "gemini-cli",
            Adapter::Antigravity => "antigravity",
        }
    }

    /// Gemini expresses hook timeouts in milliseconds; every other agent uses
    /// seconds. Getting this wrong would give Gemini a 130ms budget.
    fn timeout_value(self, seconds: u64) -> i64 {
        match self {
            Adapter::GeminiCli => (seconds * 1000) as i64,
            _ => seconds as i64,
        }
    }

    /// Whether this agent reads project-scoped hook config at all.
    fn supports_project_scope(self) -> bool {
        // Codex ignores hook config in untrusted project layers, so the global
        // file is the only reliable target.
        self != Adapter::Codex
    }
}

pub struct Target {
    adapter: Adapter,
    scope: Scope,
    path: PathBuf,
}

impl Target {
    pub fn resolve(adapter: Adapter, scope: Scope, project: &Path) -> Result<Target> {
        let home = home_dir()?;
        let scope = if adapter.supports_project_scope() {
            scope
        } else {
            Scope::Global
        };
        let path = match (adapter, scope) {
            (Adapter::ClaudeCode, Scope::Global) => home.join(".claude/settings.json"),
            (Adapter::ClaudeCode, Scope::Project) => project.join(".claude/settings.json"),
            (Adapter::Codex, _) => home.join(".codex/config.toml"),
            (Adapter::Cursor, Scope::Global) => home.join(".cursor/hooks.json"),
            (Adapter::Cursor, Scope::Project) => project.join(".cursor/hooks.json"),
            (Adapter::GeminiCli, Scope::Global) => home.join(".gemini/settings.json"),
            (Adapter::GeminiCli, Scope::Project) => project.join(".gemini/settings.json"),
            (Adapter::Antigravity, Scope::Global) => home.join(".gemini/config/hooks.json"),
            (Adapter::Antigravity, Scope::Project) => project.join(".agents/hooks.json"),
        };
        Ok(Target {
            adapter,
            scope,
            path,
        })
    }

    pub fn label(&self) -> String {
        let scope = match self.scope {
            Scope::Global => "global",
            Scope::Project => "project",
        };
        format!("{} ({scope})", self.adapter.hook_subcommand())
    }
}

/// The command string a hook should invoke. The absolute path to this binary
/// is used rather than a bare name, so the hook works without `agent-gate`
/// being on the agent's `PATH`.
fn hook_command(adapter: Adapter) -> Result<String> {
    let exe = std::env::current_exe().context("locating the agent-gate binary")?;
    Ok(format!("{} hook {}", exe.display(), adapter.hook_subcommand()))
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
        std::fs::copy(path, &backup).with_context(|| format!("backing up {}", path.display()))?;
        println!("  backed up -> {}", backup.display());
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
    let found = match target.adapter.format() {
        Format::CodexToml => find_in_toml(&contents),
        format => find_in_json(&contents, format),
    };
    Ok(match found {
        Some(command) => State::Installed { command },
        None => State::NotInstalled,
    })
}

/// Walks every command string under the format's event list.
fn json_commands(doc: &Value, format: Format) -> Vec<String> {
    let (container, event) = match format {
        Format::NestedJson {
            container, event, ..
        } => (container, event),
        Format::CursorJson => ("hooks", "beforeShellExecution"),
        Format::CodexToml => return Vec::new(),
    };
    let Some(entries) = doc.get(container).and_then(|c| c.get(event)).and_then(|e| e.as_array())
    else {
        return Vec::new();
    };
    let mut found = Vec::new();
    for entry in entries {
        match format {
            // Cursor entries hold the command directly.
            Format::CursorJson => {
                if let Some(c) = entry.get("command").and_then(|c| c.as_str()) {
                    found.push(c.to_string());
                }
            }
            _ => {
                if let Some(hooks) = entry.get("hooks").and_then(|h| h.as_array()) {
                    for hook in hooks {
                        if let Some(c) = hook.get("command").and_then(|c| c.as_str()) {
                            found.push(c.to_string());
                        }
                    }
                }
            }
        }
    }
    found
}

fn find_in_json(contents: &str, format: Format) -> Option<String> {
    let doc: Value = serde_json::from_str(contents).ok()?;
    json_commands(&doc, format)
        .into_iter()
        .find(|c| is_our_hook(c))
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

pub fn install(target: &Target, timeout_seconds: u64) -> Result<bool> {
    let command = hook_command(target.adapter)?;
    if let State::Installed { command: existing } = inspect(target)? {
        if existing == command {
            println!("  {} already installed", target.label());
            return Ok(false);
        }
        uninstall(target)?; // stale entry from a different checkout
    }
    back_up(&target.path)?;
    if let Some(parent) = target.path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let existing = std::fs::read_to_string(&target.path).unwrap_or_default();
    let timeout = target.adapter.timeout_value(timeout_seconds);
    let updated = match target.adapter.format() {
        Format::CodexToml => install_toml(&existing, &command, timeout)?,
        format => install_json(&existing, format, &command, timeout)?,
    };
    std::fs::write(&target.path, updated)?;
    println!("  installed {} -> {}", target.label(), target.path.display());
    Ok(true)
}

fn install_json(existing: &str, format: Format, command: &str, timeout: i64) -> Result<String> {
    let mut doc: Value = if existing.trim().is_empty() {
        json!({})
    } else {
        serde_json::from_str(existing).context("parsing existing hook config")?
    };
    let object = doc.as_object_mut().context("hook config is not an object")?;

    let (container, event, entry) = match format {
        Format::NestedJson {
            container,
            event,
            matcher,
        } => (
            container,
            event,
            json!({
                "matcher": matcher,
                "hooks": [{ "type": "command", "command": command, "timeout": timeout }],
            }),
        ),
        Format::CursorJson => {
            // Cursor requires a schema version at the top level.
            object.entry("version").or_insert(json!(1));
            (
                "hooks",
                "beforeShellExecution",
                json!({ "command": command, "timeout": timeout }),
            )
        }
        Format::CodexToml => anyhow::bail!("TOML handled separately"),
    };

    let events = object.entry(container).or_insert_with(|| json!({}));
    let list = events
        .as_object_mut()
        .with_context(|| format!("{container} is not an object"))?
        .entry(event)
        .or_insert_with(|| json!([]));
    list.as_array_mut()
        .with_context(|| format!("{event} is not an array"))?
        .push(entry);

    Ok(format!("{}\n", serde_json::to_string_pretty(&doc)?))
}

fn install_toml(existing: &str, command: &str, timeout: i64) -> Result<String> {
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
    inner["timeout"] = toml_edit::value(timeout);
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
    let updated = match target.adapter.format() {
        Format::CodexToml => uninstall_toml(&existing)?,
        format => uninstall_json(&existing, format)?,
    };
    std::fs::write(&target.path, updated)?;
    println!("  removed {} from {}", target.label(), target.path.display());
    Ok(true)
}

fn uninstall_json(existing: &str, format: Format) -> Result<String> {
    let mut doc: Value = serde_json::from_str(existing)?;
    let (container, event) = match format {
        Format::NestedJson {
            container, event, ..
        } => (container, event),
        Format::CursorJson => ("hooks", "beforeShellExecution"),
        Format::CodexToml => anyhow::bail!("TOML handled separately"),
    };

    if let Some(list) = doc
        .get_mut(container)
        .and_then(|c| c.get_mut(event))
        .and_then(|e| e.as_array_mut())
    {
        if format == Format::CursorJson {
            list.retain(|e| {
                !e.get("command")
                    .and_then(|c| c.as_str())
                    .is_some_and(is_our_hook)
            });
        } else {
            for entry in list.iter_mut() {
                if let Some(hooks) = entry.get_mut("hooks").and_then(|h| h.as_array_mut()) {
                    hooks.retain(|h| {
                        !h.get("command")
                            .and_then(|c| c.as_str())
                            .is_some_and(is_our_hook)
                    });
                }
            }
            // Drop entries whose hook list we just emptied.
            list.retain(|e| {
                e.get("hooks")
                    .and_then(|h| h.as_array())
                    .is_none_or(|h| !h.is_empty())
            });
        }
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
            !entry
                .get("hooks")
                .and_then(|h| h.as_array_of_tables())
                .is_some_and(|hooks| {
                    hooks.iter().any(|h| {
                        h.get("command")
                            .and_then(|c| c.as_str())
                            .is_some_and(is_our_hook)
                    })
                })
        });
    }
    Ok(doc.to_string())
}

// -------------------------------------------------------------------- command

pub const ALL_ADAPTERS: &[Adapter] = &[
    Adapter::ClaudeCode,
    Adapter::Codex,
    Adapter::Cursor,
    Adapter::GeminiCli,
    Adapter::Antigravity,
];

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

    for adapter in ALL_ADAPTERS {
        for scope in [Scope::Project, Scope::Global] {
            if scope == Scope::Project && !adapter.supports_project_scope() {
                continue;
            }
            let target = Target::resolve(*adapter, scope, project)?;
            let status = match inspect(&target)? {
                State::Installed { .. } => "installed",
                State::NotInstalled => "not installed",
                State::NoConfigFile => "no config file",
            };
            println!("  {:<24} {:<16} {}", target.label(), status, target.path.display());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const CMD: &str = "/usr/local/bin/agent-gate hook claude-code";

    fn format_of(adapter: Adapter) -> Format {
        adapter.format()
    }

    #[test]
    fn round_trips_every_json_agent() {
        for adapter in [
            Adapter::ClaudeCode,
            Adapter::GeminiCli,
            Adapter::Antigravity,
            Adapter::Cursor,
        ] {
            let format = format_of(adapter);
            let installed = install_json("", format, CMD, 130).unwrap();
            assert_eq!(
                find_in_json(&installed, format).as_deref(),
                Some(CMD),
                "{adapter:?} install not found"
            );
            let removed = uninstall_json(&installed, format).unwrap();
            assert_eq!(
                find_in_json(&removed, format),
                None,
                "{adapter:?} uninstall left the hook behind"
            );
        }
    }

    #[test]
    fn each_agent_gets_its_own_event_and_matcher() {
        let claude = install_json("", format_of(Adapter::ClaudeCode), CMD, 130).unwrap();
        assert!(claude.contains("\"PreToolUse\"") && claude.contains("\"Bash\""));

        let gemini = install_json("", format_of(Adapter::GeminiCli), CMD, 130).unwrap();
        assert!(gemini.contains("\"BeforeTool\"") && gemini.contains("run_shell_command"));

        let anti = install_json("", format_of(Adapter::Antigravity), CMD, 130).unwrap();
        assert!(anti.contains("local-agent-gate") && anti.contains("run_command"));

        let cursor = install_json("", format_of(Adapter::Cursor), CMD, 130).unwrap();
        assert!(cursor.contains("beforeShellExecution"));
    }

    #[test]
    fn cursor_entries_are_flat_and_versioned() {
        let out = install_json("", format_of(Adapter::Cursor), CMD, 30).unwrap();
        let doc: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(doc["version"], json!(1));
        let entry = &doc["hooks"]["beforeShellExecution"][0];
        assert_eq!(entry["command"], json!(CMD));
        // Flat: no nested hook list, unlike every other agent.
        assert!(entry.get("hooks").is_none());
    }

    #[test]
    fn gemini_timeout_is_milliseconds() {
        assert_eq!(Adapter::GeminiCli.timeout_value(130), 130_000);
        assert_eq!(Adapter::ClaudeCode.timeout_value(130), 130);
        assert_eq!(Adapter::Cursor.timeout_value(30), 30);
    }

    #[test]
    fn install_preserves_unrelated_settings() {
        let existing = r#"{"model":"opus","hooks":{"PostToolUse":[{"matcher":"Bash"}]}}"#;
        let out = install_json(existing, format_of(Adapter::ClaudeCode), CMD, 130).unwrap();
        let doc: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(doc["model"], json!("opus"));
        assert!(doc["hooks"]["PostToolUse"].is_array());
    }

    #[test]
    fn uninstall_removes_only_our_hook() {
        let existing = r#"{"hooks":{"PreToolUse":[
            {"matcher":"Bash","hooks":[{"type":"command","command":"other-tool check"}]}
        ]}}"#;
        let format = format_of(Adapter::ClaudeCode);
        let installed = install_json(existing, format, CMD, 130).unwrap();
        let removed = uninstall_json(&installed, format).unwrap();
        assert_eq!(find_in_json(&removed, format), None);
        assert!(removed.contains("other-tool check"), "foreign hook survives");
    }

    #[test]
    fn cursor_uninstall_spares_foreign_hooks() {
        let existing = r#"{"version":1,"hooks":{"beforeShellExecution":[{"command":"other-gate run"}]}}"#;
        let format = format_of(Adapter::Cursor);
        let installed = install_json(existing, format, CMD, 30).unwrap();
        let removed = uninstall_json(&installed, format).unwrap();
        assert_eq!(find_in_json(&removed, format), None);
        assert!(removed.contains("other-gate run"));
    }

    #[test]
    fn toml_install_preserves_comments_and_other_config() {
        let existing = "# my notes\nmodel = \"gpt-5\"\n\n[mcp_servers.thing]\ncommand = \"x\"\n";
        let out = install_toml(existing, "/usr/local/bin/agent-gate hook codex", 130).unwrap();
        assert!(out.contains("# my notes"), "comments preserved");
        assert!(out.contains("[mcp_servers.thing]"), "other tables preserved");
        assert!(out.contains("^Bash$"), "codex matcher is a regex");
        let removed = uninstall_toml(&out).unwrap();
        assert_eq!(find_in_toml(&removed), None);
        assert!(removed.contains("model = \"gpt-5\""));
    }

    #[test]
    fn foreign_hooks_are_not_mistaken_for_ours() {
        assert!(!is_our_hook("some-other-gate hook claude-code"));
        assert!(is_our_hook("/opt/bin/agent-gate hook codex"));
    }

    #[test]
    fn codex_is_forced_to_global_scope() {
        // Codex ignores hooks in untrusted project layers, so a project-scoped
        // request must not silently write a file Codex will never read.
        let target = Target::resolve(Adapter::Codex, Scope::Project, Path::new("/proj")).unwrap();
        assert!(target.path.ends_with(".codex/config.toml"));
        assert!(!target.path.starts_with("/proj"));
    }
}
