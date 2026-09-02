# Status

Last updated: 2026-09-02

Tracks what is actually built against the milestones in
[`local-agent-gate-design.md`](local-agent-gate-design.md). Machine-specific
setup (launchd, local wiring) is deliberately not here.

## Built

**MVP 0 — daemon + CLI.** Complete.

- Rule-based risk classifier covering the design doc's example set: `rm -rf` on
  root/home, credential exfiltration, `sudo` + network, force-push to a
  protected branch, `git reset --hard`, `git clean -fd`, `npm publish`,
  `terraform apply`, `kubectl delete`, package installs, migrations, Compose.
- `.agent-gate.yml` policy engine with the documented precedence order. A
  `blocked`-tier risk always denies and cannot be overridden by a user rule.
- Axum daemon over a Unix socket: `POST /approve` (classifies and evaluates
  server-side, then auto-decides or parks on the pending queue and blocks until
  resolved or a 5-minute expiry), `GET /pending`, `POST /pending/:id/decide`,
  `GET /health`. Every decision persists to SQLite.
- `agent-gate` CLI: `daemon`, `run -- <cmd>`, `approve`, `audit`,
  `hook claude-code`.
- 9 unit tests over the classifier and policy precedence.

**MVP 1 / Milestone 3 — agent adapters.** Two agents, one queue.

- `PreToolUse` adapters for **Claude Code** (`agent-gate hook claude-code`) and
  **Codex CLI** (`agent-gate hook codex`). Both read hook JSON on stdin,
  forward `Bash` calls to the daemon, and always exit 0 - a failing hook must
  not take the agent down with it.
- The two contracts are nearly identical, differing in one place: Claude Code
  has a `defer` decision, Codex documents only `allow` and `deny`. Where Claude
  Code defers, the Codex adapter omits `permissionDecision` entirely, which
  leaves Codex's own approval policy in charge. That path is used both when the
  daemon is unreachable and when a request expires unanswered, so neither agent
  ever hard-denies work the user was never shown.
- Verified live: requests from both agents queued simultaneously, were decided
  independently (one allowed, one denied), each decision returned to the right
  hook, and the audit log distinguishes them by agent. This is Milestone 3's
  exit criterion ("at least two AI agents can route approval requests through
  the same queue").
- `agent-gate adapters list | install | uninstall` manages the wiring, which
  completes Milestone 3. Writes are conservative because both config files are
  ones the user edits by hand: the file is backed up first, unrelated content
  survives (TOML comments and tables included, via `toml_edit` rather than a
  reparse), installing twice is a no-op, and uninstall removes only hooks that
  point at this binary. `list` doubles as the health check, using `/pending`
  rather than `/health` so a stale socket file cannot read as a live daemon.

**Structural risk classification.** Complete.

- Commands are parsed into pipelines and simple commands before classification,
  and each is judged on its own program name and arguments. Previously the
  classifier regexed the raw string, so it could not tell a command from text
  quoting one.
- Quoting, heredocs, redirection, pipelines and command substitution are all
  resolved. A heredoc body is data, not commands - which is what stops writing
  a file *about* dangerous commands from being classified as running them.
- Wrappers (`sudo`, `xargs`, `env`, ...) are stripped so the wrapped command is
  judged, and `sh -c` / `eval` arguments are recursively classified, since those
  really are code.
- Cross-command risks that only exist as data flow are evaluated per pipeline:
  a secret read piped into a network tool, or a substitution that reads a
  credential file.
- Verified against the live daemon across 23 cases: every catastrophic example
  still blocks, every risk level is unchanged, and commands that merely quote
  dangerous text now rate low.

**Request lifecycle hardening.** Complete.

- A background reaper sweeps expired requests out of the pending queue and
  writes an `expired` audit event for each. Expiry is the reaper's job *alone*:
  the request handler used to do this after its `await`, which never ran in the
  common case, because a disconnecting client causes hyper to drop the in-flight
  future along with any cleanup behind it. Symptom before the fix: expired
  requests accumulated in `/pending` indefinitely and left no audit trail.
- Request expiry and the reap interval are configurable on `DaemonConfig`.
- 8 daemon integration tests drive the HTTP API in-process, including a
  regression test that abandons a handler mid-flight the way a killed adapter
  does.

**MVP 2 — Mac app.** Partial, but the core loop is verified.

- Menu bar app (SwiftPM executable, no Xcode project). Polls the daemon every
  2s over the Unix socket via a hand-rolled POSIX socket + HTTP/1.1 client,
  since URLSession has no UDS support. Lists pending approvals with per-item
  Allow/Deny, shows daemon status, opens an audit log window.
- **Human-verified 2026-09-02.** A person clicked Allow on four queued
  approvals and Deny on a fifth, from the real menu bar. Each waiting client
  received its decision, and every click produced an audit receipt with the
  right risk level. This is Milestone 2's exit criterion ("user can install
  app, start daemon, approve from menu bar") met for the first time.
- Still unverified by a human: the audit log window, and behaviour when the
  daemon is stopped underneath a running app.

## Next

Rough priority, ahead of new milestones:

- **Adapter install UX.** `agent-gate adapters install claude-code` does not
  exist; wiring is hand-edited today.

Then, per the design doc's milestone order:

1. **Gemini CLI adapter (rest of Milestone 3).** Gemini's hook surface has not
   been investigated yet; the design doc suggests wrapper mode unless it has a
   stable hook surface. Codex turned out to have a `PreToolUse` hook closely
   matching Claude Code's, so the shared adapter core should extend cheaply if
   Gemini does too.
2. **Mac app polish (rest of Milestone 2).** Policy editor UI, local
   notifications for new pending approvals (currently poll-only, no banner),
   app bundle + icon, sign and notarize with a Developer ID, ship a `.dmg`.
3. **MVP 2 — local mobile web approval.** LAN server, pairing token/QR,
   responsive approval UI, WebSocket push, Tailscale/manual-URL support. The
   daemon listens only on the Unix socket today — no TCP interface exists.
4. **MVP 3 — native iPhone app.** QR pairing with device keypair exchange,
   native notifications, policy browsing, run receipts.
5. **MVP 4 — watchOS app.** Depends on MVP 3.

## Operational notes

- **Adapter timeouts must exceed the daemon's expiry window.** The daemon
  defaults to a 120s request expiry; the Claude Code hook is configured at 130s.
  If the hook's timeout is the shorter of the two, it is killed before the
  daemon's expiry response arrives and every unanswered request orphans.
- **Rebuilding the release binary breaks the running LaunchAgent.** launchd
  caches the code signature, so after `cargo build --release` a
  `launchctl kickstart -k` fails with `OS_REASON_CODESIGNING`. A full
  `bootout` + `bootstrap` reload picks up the new binary.

## Known limitations

- **Tilde expansion.** If a shell expands `~` before `agent-gate` sees argv,
  the classifier gets an absolute path instead of a literal `~` and rates the
  command `high` rather than `blocked`. Still denied by default either way
  (fail closed), but the `blocked` label won't always fire. The design doc
  explicitly accepts this class of shell-parsing imperfection for MVP.
- **The shell parser is deliberately partial.** It handles quoting, heredocs,
  redirection, pipelines and command substitution, but ignores parameter
  expansion, arithmetic, process substitution and control-flow keywords. A
  command hidden behind an expanded variable is not seen as a command. Unknown
  programs still default to medium risk, so the failure mode stays fail-safe.
- **`argv` on the wire is still a naive split.** The daemon records the adapter's
  own argv, not the parser's, so the audit field is unreliable for compound
  commands. Classification no longer depends on it, but `commandStartsWith`
  policy rules effectively assume a single simple command.
- **A decision on an orphaned request is silently dropped.** If the submitting
  client has disconnected, `POST /pending/:id/decide` returns `ok: false` and
  records nothing. The window is small (the reaper clears orphans within one
  interval) but it is not zero.
