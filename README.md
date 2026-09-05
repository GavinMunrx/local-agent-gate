# Local Agent Gate

A local command firewall for AI coding agents.

AI agents increasingly want to run shell commands — install dependencies, run
migrations, force-push, publish packages, delete things. Local Agent Gate sits
in front of those calls, classifies them by risk, applies your policy, and asks
you before the dangerous ones run. Your Mac stays the authority: no hosted
relay, no account, no source upload.

Full design rationale is in [`docs/local-agent-gate-design.md`](docs/local-agent-gate-design.md).

## Status

Early. MVP 0 and most of MVP 1 exist and work; everything past that is design
doc only. See [`docs/status.md`](docs/status.md) for what's built and what's next.

## How it works

```
agent (Claude Code hook, or `agent-gate run`)
   │  command + repo/branch context
   ▼
daemon  ──►  classify risk  ──►  apply policy  ──►  allow / deny / ask
   │                                                        │
   │                                              pending queue
   ▼                                                        │
SQLite audit log                          approval surfaces ┘
                                          (terminal, menu bar app)
```

Risk classification and policy evaluation happen **in the daemon**, not the
client, so an agent cannot talk its way into a lower risk tier. Anything
classified `blocked` is denied unconditionally — no user rule can override it.

## Components

| Path | What it is |
| --- | --- |
| `crates/agent-gate-policy` | Rule-based risk classifier (low/medium/high/blocked), the `.agent-gate.yml` policy engine, and rules learned from decisions |
| `crates/agent-gate-daemon` | Axum HTTP server over a Unix socket; pending-approval queue; SQLite audit log |
| `crates/agent-gate-cli` | The `agent-gate` binary: daemon, command wrapper, agent adapters, terminal approvals, audit viewer |
| `apps/mac/LocalAgentGateMac` | AppKit menu bar app — pending approvals with Allow/Deny, daemon status, audit log |

## Getting started

```sh
cargo build --release
```

Run the daemon in the foreground:

```sh
./target/release/agent-gate daemon
```

Gate an arbitrary command:

```sh
./target/release/agent-gate run -- rm -rf ./build
```

Approve from the terminal (in another shell), or run the menu bar app:

```sh
./target/release/agent-gate approve
cd apps/mac/LocalAgentGateMac && swift run
```

Review what happened:

```sh
./target/release/agent-gate audit --limit 20
```

### Agents

Five agents are supported. Each exposes a pre-execution hook, but no two agree
on the shape of the payload or the vocabulary of the answer, so the adapter
translates in both directions:

| Agent | Hook event | Config |
| --- | --- | --- |
| Claude Code | `PreToolUse` (`Bash`) | `.claude/settings.json` |
| Codex CLI | `PreToolUse` (`^Bash$`) | `~/.codex/config.toml` |
| Cursor | `beforeShellExecution` | `.cursor/hooks.json` |
| Gemini CLI | `BeforeTool` (`run_shell_command`) | `.gemini/settings.json` |
| Antigravity | `PreToolUse` (`run_command`) | `.agents/hooks.json` |

Let the CLI do the wiring:

```sh
agent-gate adapters list                     # who is wired up, and is the daemon live
agent-gate adapters install claude-code      # this project
agent-gate adapters install cursor --global  # ~/.cursor/hooks.json
agent-gate adapters uninstall codex --global
```

Installing backs the file up first, preserves everything else in it (including
TOML comments), and is a no-op if the hook is already there. Uninstalling
removes only hooks pointing at this binary, so another tool's hooks survive.
To wire things by hand instead, see [`examples/`](examples).

If the daemon is unreachable, or a request expires with nobody watching an
approval surface, the hook declines to decide and the agent falls back to its
own permission prompt. It never denies work the user was never shown. Each
agent spells that differently - `defer` for Claude Code, `ask` for Cursor and
Antigravity, an omitted field for Codex, an empty object for Gemini.

**Gemini caveat:** its `BeforeTool` hook can deny or rewrite a call but has no
way to say "approved, skip your own confirmation". An allow is therefore
indistinguishable from no opinion, so Gemini still runs its normal approval
prompt for commands the gate allowed. Denials work fully.

### Policy

Drop an `.agent-gate.yml` in a repo to override the defaults. See
[`examples/agent-gate.yml`](examples/agent-gate.yml). Precedence, strongest
first: built-in `blocked` risks → deny rules → allow rules → ask rules →
per-risk-level defaults.

### Learned rules

Answering an approval with **allow similar** or **block similar** writes a rule,
so the same shape of command is not asked about again. Every approval surface
offers it, and shows the scope before you grant it:

```
Similar:      commands starting with `npm install`, in this project

  [y] allow once          [s] allow similar from now on
  [n] deny (default)      [b] block similar from now on
```

The generalisation is narrow and mechanical, never a guess. A single simple
command widens to its program and subcommand; anything compound, or anything
that redirects to a file, is pinned to its exact text, because "commands like
`a && b`" has no honest meaning. Rules are scoped to the project they were
learned in, so approving something in a scratch repo cannot loosen a production
one, and a learned rule can never override the built-in `blocked` tier.

Review and revoke them:

```sh
agent-gate policy list
agent-gate policy forget <id>
agent-gate policy forget-all
```

Revocation takes effect on the next command, not the next daemon restart: the
daemon re-reads the rule file on every request, precisely because a rule that
silently allows commands has to be revocable now.

## Approving from another device

Off by default. To let a phone (or eventually a watch) approve:

```sh
agent-gate daemon --lan          # adds a TCP listener on :8787
agent-gate pair --show-token     # address + bearer token
```

Every network request must carry `Authorization: Bearer <token>`; the Unix
socket stays unauthenticated because filesystem permissions already gate it.
`GET /events` is an SSE stream that pushes the pending queue on connect and on
every change, so a client is told rather than polling.

Anyone with the token can approve commands. Prefer a user-owned tunnel
(Tailscale, WireGuard) over an open LAN.

For why the Apple Watch is harder than it looks, see
[`docs/apple-watch-path.md`](docs/apple-watch-path.md).

## Local state

Lives in `~/Library/Application Support/local-agent-gate/`:

- `agent-gate.sock` — the daemon's Unix socket
- `audit.db` — SQLite audit log
- `learned-policy.yml` — rules from "allow similar" / "block similar"

Don't delete that directory while the daemon is running; the process survives
but its socket doesn't, and every client gets connection-refused until the
daemon restarts.

## Tests

```sh
cargo test --workspace
```

Unit tests cover the risk classifier and policy precedence; the daemon's
integration tests drive its HTTP API in-process, including the request
lifecycle when a client disconnects mid-approval.
