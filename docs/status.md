# Status

Last updated: 2026-09-04

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
  server-side, then auto-decides or parks on the pending queue and blocks for
  the agent wait), `GET /pending`, `POST /pending/:id/decide`, `GET /health`,
  `GET /events`. Every decision persists to SQLite.
- `agent-gate` CLI: `daemon`, `run -- <cmd>`, `approve`, `audit`, `hook`,
  `adapters`, `policy`, `pair`.
- Unit tests over the classifier, the shell parser, policy precedence and
  learned rules.

**MVP 1 / Milestone 3 — agent adapters.** Five agents, one queue.

- Adapters for **Claude Code**, **Codex CLI**, **Cursor**, **Gemini CLI** and
  **Antigravity**. All read hook JSON on stdin and always exit 0 - a failing
  hook must not take the agent down with it.
- No two agents agree on either side of the contract, so `Adapter::parse` and
  `Adapter::render` hold every difference and the logic between them is shared.
  Claude Code, Codex and Gemini nest the command under `tool_input.command` but
  name the shell tool differently; Cursor puts it at the top level with no tool
  name; Antigravity nests it under `toolCall.args.CommandLine`.
- The verdict vocabulary differs too, and "no opinion" is the case that matters:
  the gate must never hard-deny work the user was never shown. Claude Code
  spells that `defer`, Cursor and Antigravity `ask`, Codex an omitted decision
  field, Gemini an empty object. A test asserts no adapter can emit a denial
  for an undecided verdict.
- **Gemini limitation:** `BeforeTool` can deny or rewrite a call but cannot
  say "approved, skip your own confirmation", so an allow and no-opinion are
  the same empty object. Gemini still prompts for commands the gate allowed;
  denials work fully.
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
- **Two timeouts, not one.** `agent_wait_seconds` (120s) is how long an
  agent's hook blocks before falling back to its own prompt;
  `request_ttl_seconds` (600s) is how long the request stays answerable by a
  human afterwards. They were one value, which meant a request died at the
  moment the agent gave up - fine at a desk, useless for a notification that
  has to reach a phone or watch first.
- A decision that arrives after the agent stopped waiting is still recorded,
  with the receipt saying so. Before the split that was a rare race; it is now
  the normal path for anything approved away from the Mac.
- Daemon integration tests drive the HTTP API in-process, including a
  regression test that abandons a handler mid-flight the way a killed adapter
  does.

**Learned rules — "allow similar".** Complete, and reachable from every
surface.

- Deciding a pending request with `allow_similar` or `block_similar` writes a
  rule that answers the same shape of command without asking again. Offered by
  the terminal (`agent-gate approve`) and the Mac menu bar; `agent-gate policy
  list | forget | forget-all` reviews and revokes.
- **The generalisation is mechanical, never fuzzy.** A single simple command
  widens to its program plus a subcommand; anything compound, or anything that
  redirects to a file, is pinned to its exact text. "Commands like `a && b`"
  has no honest meaning, and `echo x > a` and `echo x > b` do not write to the
  same place.
- Rules are scoped to the project they were learned in, and cannot override the
  built-in catastrophic tier - `evaluate` denies `blocked` risk before it
  consults any rule at all. Both are asserted by tests.
- **Revocation is immediate.** The daemon reads the rule file per request
  rather than caching it, so `agent-gate policy forget` takes effect on the
  next command instead of the next restart. A rule that silently allows
  commands has to be revocable now. Writes go through a temp file and a rename,
  so a per-request read can never catch a half-written file.
- Every surface shows the scope *before* it is granted, worded by the daemon
  (`similarScope` on the request) rather than derived separately by each
  client. This is the only answer in the UI that changes what runs later
  without asking, so it must not be a blind tap.
- Verified live end to end: an `allow similar` from the terminal returned
  `allow` to a waiting Claude Code hook, the next matching command was
  auto-allowed in 50ms with the learned rule id in its receipt, `policy forget`
  sent the following one back to the queue with the daemon still running, and
  `rm -rf /` stayed blocked throughout.

**MVP 2 / Milestone 4 — approval from a phone.** The exit criterion is met.

- `GET /` serves a responsive approval page from the network listener: the
  pending queue live over SSE, each request with its risk tier, reasons,
  project, directory and a countdown, and all four actions from the design
  doc's approval card.
- One self-contained file, no external assets. An approval surface that needs
  the internet to render is useless on the LAN it was built for; a test asserts
  the page pulls nothing remote.
- **The token had to reach a browser somehow.** A browser cannot set a header
  when it navigates, and `EventSource` cannot set one at all, so the daemon
  also accepts `?token=`, and `agent-gate pair --show-token` prints a URL that
  carries it. The page stores it in `localStorage` and strips it from the
  address bar at once, because a token left in browser history outlives the tab
  it was pasted into. `GET /` is the only route exempt from the token, and it
  carries no data - it exists so a device with no token has somewhere to type
  one in.
- The two "similar" answers take two taps. They are the only controls that
  change what runs later without asking, and a phone is where a stray thumb
  lands.
- A decision that fails to send restores the card rather than clearing it. An
  approval that silently failed to send is the one failure this page must not
  hide.
- **Verified live in a browser 2026-09-04.** Two agents queued simultaneously;
  "always allow similar" (two taps) on the Claude Code request and "Deny" on
  the Codex one. Each hook received the right verdict, the rule was learned,
  the audit log recorded both, the queue emptied over SSE without a reload, and
  the console was clean.

**MVP 2 — Mac app.** Partial, but the core loop is verified.

- Menu bar app (SwiftPM executable, no Xcode project). Polls the daemon every
  2s over the Unix socket via a hand-rolled POSIX socket + HTTP/1.1 client,
  since URLSession has no UDS support. Lists pending approvals with per-item
  Allow/Deny, shows daemon status, opens an audit log window.
- Approval submenus carry Allow, Deny, Always Allow Similar and Block Similar,
  the four actions the design doc's approval card specifies.
- **Human-verified 2026-09-02.** A person clicked Allow on four queued
  approvals and Deny on a fifth, from the real menu bar. Each waiting client
  received its decision, and every click produced an audit receipt with the
  right risk level. This is Milestone 2's exit criterion ("user can install
  app, start daemon, approve from menu bar") met for the first time.
- Still unverified by a human: the audit log window, the two new "similar"
  menu items (the daemon side of them is verified from the terminal), and
  behaviour when the daemon is stopped underneath a running app.

## Next

Per the design doc's milestone order:

1. **Milestone 3 is complete.** All five agents route through one queue and the
   wiring is managed by `agent-gate adapters`.
2. **Mac app polish (rest of Milestone 2).** Policy editor UI, local
   notifications for new pending approvals (currently poll-only, no banner),
   app bundle + icon, sign and notarize with a Developer ID, ship a `.dmg`.
3. **Milestone 4's remainder.** The approval UI, the listener, the token and
   the SSE stream are done (see above). Still missing: QR pairing, per-device
   identity beyond the one shared token, and signed decisions - all three are
   the same piece of work, replacing one shared secret with a keypair per
   device.
4. **MVP 3 — native iPhone app.** QR pairing with device keypair exchange,
   native notifications, policy browsing, run receipts.
5. **MVP 4 — watchOS app.** Depends on MVP 3. macOS notifications do not
   mirror to the Watch, so delivery has to go through the iPhone.
   [`apple-watch-path.md`](apple-watch-path.md) assessed the options and
   [`watch-plan.md`](watch-plan.md) is the plan that follows from them: the
   daemon acts as its own APNs provider, which keeps the no-relay promise, and
   the Phase 1 daemon work (per-device identity, QR pairing, push sender) is
   worth having on its own terms.

## Network access

Off by default. `agent-gate daemon --lan` adds a TCP listener; every request on
it must carry the pairing token (32 random bytes, stored 0600, generated on
first use). The Unix socket stays unauthenticated because reaching it already
implies local filesystem access as this user. Anyone holding the token can
approve commands, so a user-owned tunnel is safer than an open LAN.

## Operational notes

- **Adapter timeouts must exceed the daemon's agent wait.** The daemon waits
  120s before telling an agent to fall back; hooks are configured at 130s. If a
  hook's timeout is the shorter of the two, it is killed before the daemon's
  response arrives and the request orphans. The request TTL (600s) is
  deliberately longer than both and does not constrain hook timeouts.
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
- **A learned rule is keyed on the project's path.** Moving or renaming a
  checkout silently drops its rules, which fails safe (more gets asked) but is
  surprising. Learned rules also never expire.
- **A decision on an already-reaped request is dropped.** Once the reaper has
  expired a request at its TTL, `POST /pending/:id/decide` returns `ok: false`.
  Decisions arriving before that are recorded whether or not an agent is still
  listening.
