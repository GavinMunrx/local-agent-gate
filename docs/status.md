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

- **Classifier code-vs-data confusion.** See Known limitations. This is now the
  most disruptive open issue in practice: during the session that verified the
  Mac app, two of the operator's own commands were gated purely because their
  text quoted dangerous commands as test data. One had to be approved by hand
  to let ordinary work continue.
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
- **No distinction between a command and text quoting one.** The classifier
  regexes the raw command string, so it fires on a command that merely *contains*
  a dangerous string — writing a file whose contents mention `rm -rf` gets
  blocked. This cuts the safe way too (a payload buried in a compound command is
  still caught), but the false positives are frequent enough during ordinary
  work to need addressing.
- **`argv` is not a real shell parse.** Compound commands are split naively, so
  the field is unreliable for anything with `&&`, `;`, or pipes. Classification
  does not depend on it, but `commandStartsWith` policy rules effectively assume
  a single simple command.
- **A decision on an orphaned request is silently dropped.** If the submitting
  client has disconnected, `POST /pending/:id/decide` returns `ok: false` and
  records nothing. The window is small (the reaper clears orphans within one
  interval) but it is not zero.
