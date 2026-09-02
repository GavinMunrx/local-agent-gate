use crate::{client, gitinfo, paths};
use serde::Deserialize;
use serde_json::json;
use std::io::Read;

/// Which agent is invoking the hook.
///
/// Claude Code and Codex both expose a `PreToolUse` hook that reads JSON on
/// stdin and writes a `hookSpecificOutput` decision to stdout, so the two
/// adapters share everything except how they say "no opinion" - see
/// [`Adapter::undecided`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Adapter {
    ClaudeCode,
    Codex,
}

impl Adapter {
    fn agent_id(self) -> &'static str {
        match self {
            Adapter::ClaudeCode => "claude-code",
            Adapter::Codex => "codex",
        }
    }

    fn agent_name(self) -> &'static str {
        match self {
            Adapter::ClaudeCode => "Claude Code",
            Adapter::Codex => "Codex CLI",
        }
    }

    /// The `permissionDecision` used when the gate has no answer - the daemon
    /// is unreachable, or the request expired with nobody watching an approval
    /// surface. Either way the user never saw the request, so hard-denying
    /// their work would be wrong; the agent should fall back to its own
    /// permission prompt.
    ///
    /// Claude Code spells that `defer`. Codex documents only `allow` and
    /// `deny`, so there the decision field is omitted entirely, which leaves
    /// its normal approval policy in charge.
    fn undecided(self) -> Option<&'static str> {
        match self {
            Adapter::ClaudeCode => Some("defer"),
            Adapter::Codex => None,
        }
    }
}

/// The subset of the `PreToolUse` payload both agents provide.
#[derive(Deserialize)]
struct HookInput {
    #[serde(default)]
    session_id: Option<String>,
    #[serde(default)]
    cwd: Option<String>,
    #[serde(default)]
    tool_name: String,
    #[serde(default)]
    tool_input: serde_json::Value,
}

/// Implements the `PreToolUse` hook contract for an agent: reads the hook
/// payload from stdin, and for `Bash` tool calls forwards the command to the
/// Local Agent Gate daemon. Prints `hookSpecificOutput` JSON to stdout and
/// always exits 0 - the hook itself never errors, because a failing hook
/// should not be able to take the agent down with it.
pub async fn run(adapter: Adapter) -> anyhow::Result<i32> {
    let mut input = String::new();
    std::io::stdin().read_to_string(&mut input)?;

    let parsed: HookInput = match serde_json::from_str(&input) {
        Ok(v) => v,
        Err(_) => return Ok(0),
    };

    if parsed.tool_name != "Bash" {
        return Ok(0);
    }

    let command = parsed
        .tool_input
        .get("command")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    if command.is_empty() {
        return Ok(0);
    }

    let cwd = parsed
        .cwd
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
    let git = gitinfo::discover(&cwd);
    let argv = shell_words::split(&command).unwrap_or_else(|_| vec![command.clone()]);

    let payload = json!({
        "agent": {
            "id": adapter.agent_id(),
            "name": adapter.agent_name(),
            "sessionId": parsed.session_id,
        },
        "projectPath": git.project_path.to_string_lossy(),
        "gitRemote": git.remote,
        "gitBranch": git.branch,
        "command": command,
        "argv": argv,
        "workingDirectory": cwd.to_string_lossy(),
    });

    let (decision, reason) =
        match client::post_json(&paths::socket_path(), "/approve", &payload).await {
            Ok(response) => {
                let decision = response
                    .get("decision")
                    .and_then(|v| v.as_str())
                    .unwrap_or("deny_once");
                let risk = response
                    .get("riskLevel")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown");
                let daemon_reason = response.get("reason").and_then(|v| v.as_str()).unwrap_or("");
                let permission = match decision {
                    "allow_once" | "allow_similar" | "auto_allowed" => Some("allow"),
                    "expired" => adapter.undecided(),
                    _ => Some("deny"),
                };
                (
                    permission,
                    format!("Local Agent Gate ({risk} risk): {daemon_reason}"),
                )
            }
            Err(_) => (
                adapter.undecided(),
                "Local Agent Gate daemon not running; falling back to normal agent permissions"
                    .to_string(),
            ),
        };

    println!("{}", hook_output(decision, &reason));
    Ok(0)
}

/// Builds the `hookSpecificOutput` object. A `None` decision deliberately
/// omits `permissionDecision`, which both agents read as "this hook has no
/// opinion" rather than as an allow or a deny.
fn hook_output(decision: Option<&str>, reason: &str) -> serde_json::Value {
    let mut output = json!({
        "hookEventName": "PreToolUse",
        "permissionDecisionReason": reason,
    });
    if let Some(decision) = decision {
        output["permissionDecision"] = json!(decision);
    }
    json!({ "hookSpecificOutput": output })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claude_code_defers_when_undecided() {
        assert_eq!(Adapter::ClaudeCode.undecided(), Some("defer"));
        let out = hook_output(Adapter::ClaudeCode.undecided(), "daemon down");
        assert_eq!(
            out["hookSpecificOutput"]["permissionDecision"],
            json!("defer")
        );
    }

    #[test]
    fn codex_omits_the_decision_when_undecided() {
        // Codex documents only "allow" and "deny", so there is no value that
        // means defer. Omitting the field leaves its own policy in charge.
        assert_eq!(Adapter::Codex.undecided(), None);
        let out = hook_output(Adapter::Codex.undecided(), "daemon down");
        assert!(out["hookSpecificOutput"].get("permissionDecision").is_none());
        assert_eq!(
            out["hookSpecificOutput"]["hookEventName"],
            json!("PreToolUse")
        );
    }

    #[test]
    fn decisions_always_carry_a_reason() {
        let out = hook_output(Some("deny"), "Local Agent Gate (blocked risk): nope");
        assert_eq!(out["hookSpecificOutput"]["permissionDecision"], json!("deny"));
        assert_eq!(
            out["hookSpecificOutput"]["permissionDecisionReason"],
            json!("Local Agent Gate (blocked risk): nope")
        );
    }

    #[test]
    fn agents_are_reported_distinctly() {
        assert_eq!(Adapter::ClaudeCode.agent_id(), "claude-code");
        assert_eq!(Adapter::Codex.agent_id(), "codex");
        assert_ne!(Adapter::ClaudeCode.agent_name(), Adapter::Codex.agent_name());
    }
}
