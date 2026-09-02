use crate::{client, gitinfo, paths};
use serde_json::{json, Value};
use std::io::Read;

/// Which agent is invoking the hook.
///
/// Every supported agent exposes a pre-execution hook that reads JSON on stdin
/// and writes a decision to stdout, but no two agree on the shape of either
/// side. [`Adapter::parse`] and [`Adapter::render`] are the only places those
/// differences live; everything between them is shared.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Adapter {
    ClaudeCode,
    Codex,
    Cursor,
    GeminiCli,
    Antigravity,
}

/// What the gate concluded, before it is phrased in an agent's own vocabulary.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    Allow,
    Deny,
    /// The gate has no answer: the daemon is unreachable, or the request
    /// expired with nobody watching an approval surface. The user never saw
    /// the request, so denying their work would be wrong - the agent should
    /// fall back to whatever it would normally do.
    Undecided,
}

/// The fields the gate needs, extracted from an agent's own payload shape.
#[derive(Debug, PartialEq, Eq)]
pub struct Request {
    pub command: String,
    pub cwd: Option<String>,
    pub session_id: Option<String>,
}

impl Adapter {
    pub fn agent_id(self) -> &'static str {
        match self {
            Adapter::ClaudeCode => "claude-code",
            Adapter::Codex => "codex",
            Adapter::Cursor => "cursor",
            Adapter::GeminiCli => "gemini-cli",
            Adapter::Antigravity => "antigravity",
        }
    }

    pub fn agent_name(self) -> &'static str {
        match self {
            Adapter::ClaudeCode => "Claude Code",
            Adapter::Codex => "Codex CLI",
            Adapter::Cursor => "Cursor",
            Adapter::GeminiCli => "Gemini CLI",
            Adapter::Antigravity => "Antigravity",
        }
    }

    /// Pulls the shell command out of an agent's hook payload, or `None` when
    /// this call is not a shell command the gate should judge.
    pub fn parse(self, input: &Value) -> Option<Request> {
        let string = |v: Option<&Value>| v.and_then(|v| v.as_str()).map(str::to_string);
        match self {
            // Claude Code, Codex and Gemini share a payload shape and differ
            // only in what they call the shell tool.
            Adapter::ClaudeCode | Adapter::Codex | Adapter::GeminiCli => {
                let expected = if self == Adapter::GeminiCli {
                    "run_shell_command"
                } else {
                    "Bash"
                };
                if input.get("tool_name")?.as_str()? != expected {
                    return None;
                }
                Some(Request {
                    command: string(input.get("tool_input")?.get("command"))?,
                    cwd: string(input.get("cwd")),
                    session_id: string(input.get("session_id")),
                })
            }
            // Cursor's beforeShellExecution only fires for shell commands, so
            // there is no tool name to check, and the command is top level.
            Adapter::Cursor => Some(Request {
                command: string(input.get("command"))?,
                cwd: string(input.get("cwd")),
                session_id: string(input.get("conversation_id")),
            }),
            Adapter::Antigravity => {
                let tool_call = input.get("toolCall")?;
                if tool_call.get("name")?.as_str()? != "run_command" {
                    return None;
                }
                let args = tool_call.get("args")?;
                Some(Request {
                    command: string(args.get("CommandLine"))?,
                    cwd: string(args.get("Cwd")),
                    session_id: string(input.get("conversationId")),
                })
            }
        }
    }

    /// Phrases a verdict in the agent's own output vocabulary.
    pub fn render(self, verdict: Verdict, reason: &str) -> Value {
        match self {
            Adapter::ClaudeCode | Adapter::Codex => {
                let mut output = json!({
                    "hookEventName": "PreToolUse",
                    "permissionDecisionReason": reason,
                });
                let decision = match verdict {
                    Verdict::Allow => Some("allow"),
                    Verdict::Deny => Some("deny"),
                    // Claude Code has a word for "no opinion"; Codex documents
                    // only allow and deny, so there the field is omitted, which
                    // leaves Codex's own approval policy in charge.
                    Verdict::Undecided => match self {
                        Adapter::ClaudeCode => Some("defer"),
                        _ => None,
                    },
                };
                if let Some(decision) = decision {
                    output["permissionDecision"] = json!(decision);
                }
                json!({ "hookSpecificOutput": output })
            }
            Adapter::Cursor => json!({
                "permission": match verdict {
                    Verdict::Allow => "allow",
                    Verdict::Deny => "deny",
                    Verdict::Undecided => "ask",
                },
                "user_message": reason,
                "agent_message": reason,
            }),
            // Gemini's BeforeTool can deny or rewrite a call, but has no way to
            // say "approved, skip your own confirmation". An allow is therefore
            // indistinguishable from no opinion: an empty object, leaving
            // Gemini's normal approval flow to run.
            Adapter::GeminiCli => match verdict {
                Verdict::Deny => json!({ "decision": "deny", "reason": reason }),
                _ => json!({}),
            },
            Adapter::Antigravity => json!({
                "decision": match verdict {
                    Verdict::Allow => "allow",
                    Verdict::Deny => "deny",
                    Verdict::Undecided => "ask",
                },
                "reason": reason,
            }),
        }
    }
}

/// Implements an agent's pre-execution hook contract: reads the hook payload
/// from stdin, forwards shell commands to the Local Agent Gate daemon, and
/// prints the agent's own decision JSON to stdout. Always exits 0 - a failing
/// hook must not be able to take the agent down with it.
pub async fn run(adapter: Adapter) -> anyhow::Result<i32> {
    let mut input = String::new();
    std::io::stdin().read_to_string(&mut input)?;

    let parsed: Value = match serde_json::from_str(&input) {
        Ok(v) => v,
        Err(_) => return Ok(0),
    };

    // Not a shell command this gate judges: stay silent rather than emit an
    // opinion about a call we never inspected.
    let Some(request) = adapter.parse(&parsed) else {
        return Ok(0);
    };
    if request.command.trim().is_empty() {
        return Ok(0);
    }

    let cwd = request
        .cwd
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
    let git = gitinfo::discover(&cwd);
    let argv =
        shell_words::split(&request.command).unwrap_or_else(|_| vec![request.command.clone()]);

    let payload = json!({
        "agent": {
            "id": adapter.agent_id(),
            "name": adapter.agent_name(),
            "sessionId": request.session_id,
        },
        "projectPath": git.project_path.to_string_lossy(),
        "gitRemote": git.remote,
        "gitBranch": git.branch,
        "command": request.command,
        "argv": argv,
        "workingDirectory": cwd.to_string_lossy(),
    });

    let (verdict, reason) = match client::post_json(&paths::socket_path(), "/approve", &payload).await
    {
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
            let verdict = match decision {
                "allow_once" | "allow_similar" | "auto_allowed" => Verdict::Allow,
                "expired" => Verdict::Undecided,
                _ => Verdict::Deny,
            };
            (
                verdict,
                format!("Local Agent Gate ({risk} risk): {daemon_reason}"),
            )
        }
        Err(_) => (
            Verdict::Undecided,
            "Local Agent Gate daemon not running; falling back to normal agent permissions"
                .to_string(),
        ),
    };

    println!("{}", adapter.render(verdict, &reason));
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: &[Adapter] = &[
        Adapter::ClaudeCode,
        Adapter::Codex,
        Adapter::Cursor,
        Adapter::GeminiCli,
        Adapter::Antigravity,
    ];

    #[test]
    fn every_adapter_reports_a_distinct_agent() {
        let mut ids: Vec<_> = ALL.iter().map(|a| a.agent_id()).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), ALL.len());
    }

    #[test]
    fn parses_claude_code_and_codex_payloads() {
        let input = json!({
            "session_id": "s1", "cwd": "/w", "tool_name": "Bash",
            "tool_input": {"command": "ls -la"}
        });
        for adapter in [Adapter::ClaudeCode, Adapter::Codex] {
            let parsed = adapter.parse(&input).expect("parsed");
            assert_eq!(parsed.command, "ls -la");
            assert_eq!(parsed.cwd.as_deref(), Some("/w"));
            assert_eq!(parsed.session_id.as_deref(), Some("s1"));
        }
    }

    #[test]
    fn gemini_uses_its_own_shell_tool_name() {
        let bash = json!({"tool_name": "Bash", "tool_input": {"command": "ls"}});
        let gemini = json!({"tool_name": "run_shell_command", "tool_input": {"command": "ls"}});
        assert!(Adapter::GeminiCli.parse(&bash).is_none());
        assert_eq!(Adapter::GeminiCli.parse(&gemini).unwrap().command, "ls");
        // ...and the reverse: Claude Code does not answer for Gemini's tool.
        assert!(Adapter::ClaudeCode.parse(&gemini).is_none());
    }

    #[test]
    fn parses_cursor_top_level_command() {
        let input = json!({
            "command": "rm file", "cwd": "/w", "conversation_id": "c1",
            "hook_event_name": "beforeShellExecution"
        });
        let parsed = Adapter::Cursor.parse(&input).expect("parsed");
        assert_eq!(parsed.command, "rm file");
        assert_eq!(parsed.session_id.as_deref(), Some("c1"));
    }

    #[test]
    fn parses_antigravity_nested_tool_call() {
        let input = json!({
            "toolCall": {"name": "run_command", "args": {"CommandLine": "npm test", "Cwd": "/w"}},
            "conversationId": "ec33"
        });
        let parsed = Adapter::Antigravity.parse(&input).expect("parsed");
        assert_eq!(parsed.command, "npm test");
        assert_eq!(parsed.cwd.as_deref(), Some("/w"));
        assert_eq!(parsed.session_id.as_deref(), Some("ec33"));

        let other = json!({"toolCall": {"name": "read_file", "args": {}}});
        assert!(Adapter::Antigravity.parse(&other).is_none());
    }

    #[test]
    fn a_deny_is_expressed_in_every_agents_vocabulary() {
        let r = "blocked";
        assert_eq!(
            Adapter::ClaudeCode.render(Verdict::Deny, r)["hookSpecificOutput"]
                ["permissionDecision"],
            json!("deny")
        );
        assert_eq!(Adapter::Cursor.render(Verdict::Deny, r)["permission"], json!("deny"));
        assert_eq!(Adapter::GeminiCli.render(Verdict::Deny, r)["decision"], json!("deny"));
        assert_eq!(
            Adapter::Antigravity.render(Verdict::Deny, r)["decision"],
            json!("deny")
        );
    }

    #[test]
    fn undecided_never_denies_for_any_agent() {
        // The whole point of Undecided: work the user never saw must not be
        // blocked, whatever the agent calls that.
        for adapter in ALL {
            let out = adapter.render(Verdict::Undecided, "daemon down");
            let text = out.to_string();
            assert!(
                !text.contains("\"deny\""),
                "{:?} denied on an undecided verdict: {text}",
                adapter
            );
        }
        assert_eq!(
            Adapter::ClaudeCode.render(Verdict::Undecided, "")["hookSpecificOutput"]
                ["permissionDecision"],
            json!("defer")
        );
        assert!(Adapter::Codex.render(Verdict::Undecided, "")["hookSpecificOutput"]
            .get("permissionDecision")
            .is_none());
        assert_eq!(
            Adapter::Cursor.render(Verdict::Undecided, "")["permission"],
            json!("ask")
        );
        assert_eq!(
            Adapter::Antigravity.render(Verdict::Undecided, "")["decision"],
            json!("ask")
        );
        assert_eq!(Adapter::GeminiCli.render(Verdict::Undecided, ""), json!({}));
    }

    #[test]
    fn gemini_cannot_express_a_positive_allow() {
        // Documented limitation: an allow and "no opinion" are the same empty
        // object, so Gemini still runs its own confirmation for allowed work.
        assert_eq!(Adapter::GeminiCli.render(Verdict::Allow, "fine"), json!({}));
        assert_eq!(
            Adapter::GeminiCli.render(Verdict::Allow, "fine"),
            Adapter::GeminiCli.render(Verdict::Undecided, "fine")
        );
    }
}
