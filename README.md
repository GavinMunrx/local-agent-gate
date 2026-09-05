# Local Agent Gate

[![CI](https://github.com/GavinMunrx/local-agent-gate/actions/workflows/ci.yml/badge.svg)](https://github.com/GavinMunrx/local-agent-gate/actions/workflows/ci.yml)
[![License](https://img.shields.io/badge/license-Apache--2.0-blue.svg)](LICENSE)

A local command firewall for AI coding agents.

AI agents increasingly want to run shell commands — install dependencies, run
migrations, force-push, publish packages, delete things. Local Agent Gate sits
in front of those calls, classifies them by risk, applies your policy, and asks
you before the dangerous ones run. Your Mac stays the authority: no hosted
relay, no account, no source upload.

Approve from the terminal, the menu bar, or your phone.

```
agent (hook, or `agent-gate run`)
   │  command + repo/branch context
   ▼
daemon  ──►  classify risk  ──►  apply policy  ──►  allow / deny / ask
   │                                                        │
   │                                              pending queue
   ▼                                                        │
SQLite audit log                          approval surfaces ┘
                                    (terminal, menu bar, phone)
```

Risk classification and policy evaluation happen **in the daemon**, not the
client, so an agent cannot talk its way into a lower risk tier. Anything
classified `blocked` is denied unconditionally — no user rule can override it.

## Status

Early, and honest about it: this is working software that has not been packaged
for anyone but its author.

**Works today.** The daemon, the risk classifier, the policy engine, five agent
adapters, learned rules, the terminal and menu bar approval surfaces, and
approval from a phone browser over your LAN or a tunnel.

**Not yet.** No signed, notarized release or `.dmg` — you build from source. No
native iPhone or Watch app. The menu bar app has no notifications and no policy
editor.

[`docs/status.md`](docs/status.md) tracks what is built against the milestones
in [`docs/local-agent-gate-design.md`](docs/local-agent-gate-design.md), and is
kept current with what has actually been verified by a human.

## Requirements

- **macOS.** The daemon and CLI are portable Rust, but the state directory and
  install paths assume macOS, and the menu bar app is AppKit.
- **Rust** (stable, 2021 edition) to build the CLI and daemon.
- **Swift 5.9+** only if you want the menu bar app.

## Quick start

```sh
cargo build --release
```

Run the daemon in the foreground:

```sh
./target/release/agent-gate daemon
```

Wire up an agent — this project, or `--global` for all of them:

```sh
./target/release/agent-gate adapters install claude-code
```

Now ask the agent to run something destructive. It will block, and the request
appears wherever you are watching:

```sh
./target/release/agent-gate approve          # terminal
cd apps/mac/LocalAgentGateMac && swift run   # menu bar app
```

You can also gate a command yourself, with no agent involved:

```sh
./target/release/agent-gate run -- rm -rf ./build
```

And review what happened:

```sh
./target/release/agent-gate audit --limit 20
```

## Risk classification

Every command is parsed into pipelines and simple commands, then each is judged
on its own program and arguments — not by matching the raw string. Quoting,
heredocs, redirection, pipelines and command substitution are all resolved, so
writing a file *about* `rm -rf /` is not the same as running it. Wrappers
(`sudo`, `xargs`, `env`) are stripped so the wrapped command is what gets
judged, and `sh -c` / `eval` arguments are classified recursively.

| Tier | Default | Examples |
| --- | --- | --- |
| `low` | allow | `git status`, `ls`, `npm test`, `cargo build` |
| `medium` | ask | `npm install`, `rm ./file`, `git checkout -b`, migrations, Docker Compose, anything unrecognised |
| `high` | ask | `git push --force` (unprotected branch), `git reset --hard`, `git clean -fd`, `npm publish`, `terraform apply`, `kubectl delete`, `aws` mutations, recursive force-delete outside a build directory |
| `blocked` | **always denied** | `rm -rf /` or `~`, force-push to `main`/`master`, a credential file piped to a network tool, `sudo` combined with an outbound transfer |

Two things to note. **Unknown programs are `medium`, not `low`** — the failure
mode is asking too often, never running something unseen. And the `blocked`
tier is a floor: no policy rule and no learned rule can lift it.

## Policy

Drop an `.agent-gate.yml` in a repo to override the defaults. See
[`examples/agent-gate.yml`](examples/agent-gate.yml).

```yaml
version: 1
defaults:
  lowRisk: allow
  mediumRisk: ask
  highRisk: ask
rules:
  - id: block-force-push-main
    match:
      commandContains: "git push --force"
      branch: "main"
    decision: deny
  - id: allow-tests
    match:
      commandRegex: "^(npm|pnpm|yarn) (test|lint|typecheck)"
    decision: allow
```

Precedence, strongest first: built-in `blocked` risks → deny rules → allow
rules → ask rules → per-risk-level defaults.

## Learned rules

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

## Agents

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
agent spells that differently — `defer` for Claude Code, `ask` for Cursor and
Antigravity, an omitted field for Codex, an empty object for Gemini.

**Gemini caveat:** its `BeforeTool` hook can deny or rewrite a call but has no
way to say "approved, skip your own confirmation". An allow is therefore
indistinguishable from no opinion, so Gemini still runs its normal approval
prompt for commands the gate allowed. Denials work fully.

**Hook timeouts must exceed the daemon's agent wait.** The daemon waits 120s
before telling an agent to fall back, so hooks are configured at 130s. A
shorter hook timeout kills the adapter before the answer arrives, orphaning the
request.

## Approving from a phone

Off by default. To let a phone (or eventually a watch) approve:

```sh
agent-gate daemon --lan          # adds a TCP listener on :8787
agent-gate pair --show-token     # prints a URL to open on the phone
```

Opening that URL gives you the approval page: the pending queue, live, with
Allow, Deny, Always Allow Similar and Block Similar on each request, and the
scope a "similar" answer would grant printed above it. The two learning
answers take two taps, because a thumb on a phone is the easiest place to
grant a standing rule by accident.

The page is one file with no external assets, so it renders on a LAN with no
internet, and it holds no state beyond the token: every decision goes to the
daemon, which remains the only authority.

Every network request must carry `Authorization: Bearer <token>`; the Unix
socket stays unauthenticated because filesystem permissions already gate it.
A browser cannot set a header when it navigates and `EventSource` cannot set
one at all, so the token is also accepted as a `?token=` query parameter —
which is what the pairing URL uses. The page stores it and strips it from the
address bar immediately, but treat the URL itself as being as sensitive as the
token. `GET /` is the only route served without one, and it carries no data.

`GET /events` is an SSE stream that pushes the pending queue on connect and on
every change, so the page is told rather than polling.

Anyone with the token can approve commands. Prefer a user-owned tunnel
(Tailscale, WireGuard) over an open LAN.

For why the Apple Watch is harder than it looks, see
[`docs/apple-watch-path.md`](docs/apple-watch-path.md).

## What this protects against — and what it does not

Local Agent Gate is a guardrail against an agent doing something destructive
before you have seen it. It is **not** a sandbox, and it is not a defence
against an attacker who already runs code on your machine.

**It helps with:**

- An agent running a destructive command you would not have approved.
- An agent that has been misled — by a prompt injection in a file, a web page,
  or a dependency — into trying something dangerous.
- Unattended runs leaving no trace: every decision, including automatic ones,
  lands in the audit log with its risk tier and reason.

**It does not help with:**

- **Anything that does not go through a hook.** An agent's file writes, network
  calls through its own tools, or a command you run yourself in a terminal are
  all invisible to the gate.
- **Deferred execution.** An agent can write a script, a `package.json`
  `postinstall`, a git hook or a Makefile that runs later. The write is not a
  command, so the gate does not see it; whatever runs it may well be approved.
- **A determined local attacker.** Any process running as you can talk to the
  Unix socket, edit `.agent-gate.yml`, or write learned rules. The trust
  boundary is "my agent might make a mistake", not "my machine is hostile".
- **Perfect shell parsing.** The parser handles quoting, heredocs, redirection,
  pipelines and substitution, but ignores parameter expansion, arithmetic,
  process substitution and control flow. A command hidden behind an expanded
  variable is not seen as a command. Unknown programs default to `medium`, so
  the failure mode stays fail-safe.
- **Multi-user trust.** The pairing token is a single shared secret. Anyone
  holding it can approve anything. There is no per-device identity yet.

Known limitations are tracked in detail at the end of
[`docs/status.md`](docs/status.md).

## Local state

Lives in `~/Library/Application Support/local-agent-gate/`:

- `agent-gate.sock` — the daemon's Unix socket
- `audit.db` — SQLite audit log
- `learned-policy.yml` — rules from "allow similar" / "block similar"
- `pairing-token` — the network bearer token (0600, created on first use)

Nothing leaves the machine. Don't delete that directory while the daemon is
running; the process survives but its socket doesn't, and every client gets
connection-refused until the daemon restarts.

## Development

| Path | What it is |
| --- | --- |
| `crates/agent-gate-policy` | Shell parser, risk classifier (low/medium/high/blocked), the `.agent-gate.yml` policy engine, and rules learned from decisions |
| `crates/agent-gate-daemon` | Axum HTTP server over a Unix socket and an optional TCP listener; pending-approval queue; SSE stream; SQLite audit log; the phone approval page |
| `crates/agent-gate-cli` | The `agent-gate` binary: daemon, command wrapper, agent adapters, terminal approvals, audit viewer, policy and pairing commands |
| `apps/mac/LocalAgentGateMac` | AppKit menu bar app — pending approvals with all four actions, daemon status, audit log |

```sh
cargo test --workspace                              # unit + integration tests
cargo clippy --workspace --all-targets -- -D warnings
cd apps/mac/LocalAgentGateMac && swift build        # menu bar app
```

CI runs all three on macOS for every push and pull request. The tree is not
rustfmt-formatted, so `cargo fmt` is deliberately not part of it — match the
surrounding style rather than reformatting a file you touch.

Unit tests cover the shell parser, the risk classifier, policy precedence and
learned-rule scoping. The daemon's integration tests drive its HTTP API
in-process, including the request lifecycle when a client disconnects
mid-approval.

This repo gates its own development: `.claude/settings.json` wires Claude Code
to `target/release/agent-gate`, and the root `.agent-gate.yml` allows medium
risk so the build loop does not interrupt itself. Build the release binary
before you start, or run `agent-gate adapters uninstall claude-code` if you
would rather not have it in the loop.

Two conventions worth knowing before contributing:

- **The daemon decides.** Classification and policy never move to the client,
  because a client is something an agent can influence.
- **Fail safe, and say so.** When the gate cannot form an opinion it defers to
  the agent's own prompt rather than allowing or denying silently, and anything
  that changes future behaviour has to be visible and revocable.

## Contributing

Issues and pull requests are welcome. There is no CLA; contributions are
accepted under the terms of the repository's license (Apache-2.0), as
[section 5](LICENSE) of that license provides.

If you are reporting a security issue, please read
[`SECURITY.md`](SECURITY.md) first rather than opening a public issue.

## License

Licensed under the Apache License, Version 2.0. See [`LICENSE`](LICENSE) and
[`NOTICE`](NOTICE).
