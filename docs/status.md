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

**MVP 1 — Claude Code adapter.** Complete.

- `PreToolUse` hook adapter. Reads hook JSON on stdin, forwards `Bash` calls to
  the daemon, emits `allow`/`deny`/`defer`, always exits 0. Falls back to
  `defer` when the daemon is unreachable.

**MVP 2 — Mac app.** Partial.

- Menu bar app (SwiftPM executable, no Xcode project). Polls the daemon every
  2s over the Unix socket via a hand-rolled POSIX socket + HTTP/1.1 client,
  since URLSession has no UDS support. Lists pending approvals with per-item
  Allow/Deny, shows daemon status, opens an audit log window.

## Next

Rough priority, ahead of new milestones:

- **Daemon integration tests.** Nothing yet spins the daemon up in-process and
  exercises the HTTP API; all daemon verification has been manual smoke-testing.
  This is what makes the pending-queue and policy-precedence logic
  regression-safe, and it gates comfortable work on everything below.
- **Human-verify the Mac app.** Its socket client, JSON decoding, and decide
  flow were confirmed programmatically, but nobody has clicked the real menu.
- **Adapter install UX.** `agent-gate adapters install claude-code` does not
  exist; wiring is hand-edited today.

Then, per the design doc's milestone order:

1. **More adapters (Milestone 3).** Codex CLI and Gemini CLI, integrated
   through supported config/wrapper points rather than by patching internals.
   Neither's hook surface has been investigated yet.
2. **Mac app polish (rest of Milestone 2).** Policy editor UI, local
   notifications for new pending approvals (currently poll-only, no banner),
   app bundle + icon, sign and notarize with a Developer ID, ship a `.dmg`.
3. **MVP 2 — local mobile web approval.** LAN server, pairing token/QR,
   responsive approval UI, WebSocket push, Tailscale/manual-URL support. The
   daemon listens only on the Unix socket today — no TCP interface exists.
4. **MVP 3 — native iPhone app.** QR pairing with device keypair exchange,
   native notifications, policy browsing, run receipts.
5. **MVP 4 — watchOS app.** Depends on MVP 3.

## Known limitations

- **Tilde expansion.** If a shell expands `~` before `agent-gate` sees argv,
  the classifier gets an absolute path instead of a literal `~` and rates the
  command `high` rather than `blocked`. Still denied by default either way
  (fail closed), but the `blocked` label won't always fire. The design doc
  explicitly accepts this class of shell-parsing imperfection for MVP.
