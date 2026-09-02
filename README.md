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
| `crates/agent-gate-policy` | Rule-based risk classifier (low/medium/high/blocked) and the `.agent-gate.yml` policy engine |
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

Both supported agents expose a `PreToolUse` hook that reads JSON on stdin and
writes a decision to stdout, so each routes into the same approval queue.

**Claude Code** — register the hook so Bash tool calls route through the gate.
For a single project, copy [`.claude/settings.json`](.claude/settings.json); to
gate every project, see
[`examples/claude-code-hooks.settings.json`](examples/claude-code-hooks.settings.json)
(which expects `agent-gate` on your `PATH`).

**Codex CLI** — merge
[`examples/codex-hooks.config.toml`](examples/codex-hooks.config.toml) into your
user-level `~/.codex/config.toml`.

If the daemon is unreachable, or a request expires with nobody watching an
approval surface, the hook declines to decide and the agent falls back to its
own permission prompt. It never denies work the user was never shown. Claude
Code spells that `defer`; Codex has no such value, so the adapter omits the
decision field instead.

> The project-scoped hook points at `target/release/agent-gate`. After changing
> Rust code, re-run `cargo build --release` or the hook keeps running stale logic.

### Policy

Drop an `.agent-gate.yml` in a repo to override the defaults. See
[`examples/agent-gate.yml`](examples/agent-gate.yml). Precedence, strongest
first: built-in `blocked` risks → deny rules → allow rules → ask rules →
per-risk-level defaults.

## Local state

Lives in `~/Library/Application Support/local-agent-gate/`:

- `agent-gate.sock` — the daemon's Unix socket
- `audit.db` — SQLite audit log

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
