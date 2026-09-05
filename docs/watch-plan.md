# Getting approvals onto the Watch

Last updated: 2026-09-05

[`apple-watch-path.md`](apple-watch-path.md) worked out the delivery
constraints and concluded that "no hosted relay" and "reliable Watch
notifications anywhere" could not both be true. One of its five options was
assessed too harshly, and correcting that is what makes this plan tractable.
This document is the plan that follows.

## The correction

Option 3 in that note rejected APNs because "APNs requires a provider server
holding the push key and a registry of device tokens. That is a hosted relay."

The provider server can be **the daemon on your own Mac**. APNs token-based
authentication uses a `.p8` signing key to mint short-lived JWTs; the provider
is then just an HTTP/2 client posting to `api.push.apple.com`. One key covers
every app on the team and both environments, it does not expire, and nothing
about it requires a machine other than yours. The device-token registry is a
file next to `learned-policy.yml`.

So there is no product-operated server, no account, and no third party in the
path — only Apple's push infrastructure, which every notification on the
platform traverses anyway.

Be precise about what this does and does not preserve. It keeps the promise as
written: no product-hosted relay, no account, no source upload. It does not
make the notification end-to-end encrypted — Apple can see the payload and the
timing metadata. That is exactly why the payload should carry "1 approval
waiting" and a request id, never the command text. State that in the UI rather
than implying more privacy than exists.

## The shape

Two channels, each doing the thing it is actually good at:

```
        ┌─────────────── wake-up (APNs) ────────────────┐
        │  "1 approval waiting", request id, risk tier  │
        │  no command text                              │
   Mac ─┤                                               ├─► iPhone ──► Watch
 daemon │                                               │   (mirrors)
        └─── detail + decisions (direct, LAN/Tailscale) ┘
             full request, Allow/Deny/similar, audit
```

The asymmetry is the important part. APNs solves *delivery* — waking a device
anywhere. It does not solve the *return path*: acting on the notification
still requires reaching your Mac. On the same network that is the LAN listener
that already exists. Away from it, that is a user-owned tunnel. A Watch tap
with no route home must fail visibly, never silently.

## Phase 0 — Prerequisites

These gate everything and involve waiting on Apple, so start them before
writing code.

1. **Apple Developer Program**, $99/year. Required for an APNs key, for
   installing on a real Watch beyond a 7-day dev build, and for TestFlight.
   Individual enrolment is usually same-day but identity verification can add
   days.
2. **Bundle identifiers** — an iOS app id and its paired watchOS app id.
3. **APNs auth key** (`.p8`) plus Team ID and Key ID. Download the key once;
   Apple will not show it again.
4. **Decide the remote path.** Tailscale is the recommended default: it gives
   the phone and Watch a stable route to the Mac from anywhere without
   exposing anything to the open internet. LAN-only is a legitimate choice if
   you only care about approving from the sofa.
5. **Decide the payload policy.** Default to no command text in the push. Make
   including it an explicit opt-in with the tradeoff spelled out.

## Phase 1 — Daemon

All Rust, all testable without any Apple hardware. This is the part to build
first, and most of it is worth having regardless of whether the Watch ever
ships.

**1.1 Device registry.** Replace the single shared pairing token with per-device
records: id, display name, credential, APNs token, created-at, last-seen,
revoked-at. Persist as YAML beside `learned-policy.yml`, read per request the
way learned rules are, so revoking a device takes effect immediately rather
than at the next restart. New endpoints for registration and listing; a
`agent-gate devices list | revoke <id>` CLI to match `agent-gate policy`.

This also fixes a limitation the README currently admits: today anyone holding
the one token can approve anything, and there is no way to revoke one device.

**1.2 QR pairing.** `agent-gate pair --qr` renders a short-lived pairing code as
a QR block in the terminal. The phone scans it and exchanges the code for its
own long-lived credential, so the durable secret is never displayed, typed, or
left in a URL. This retires the `?token=` query parameter for real clients,
though the web page keeps it.

**1.3 APNs sender.** ES256 JWT signing against the `.p8`, an HTTP/2 client to
`api.push.apple.com`, exponential backoff, and correct handling of `410
Unregistered` by dropping the device token. Configuration for key path, Team
ID, Key ID, topic, and sandbox-versus-production.

**The first checkpoint worth aiming at:** `agent-gate push test` sending to a
deliberately invalid token and getting `BadDeviceToken` back from Apple. That
response *proves the entire auth chain works* — key, JWT, HTTP/2, topic — and
you can reach it before an iOS app exists at all. It is the cheapest possible
validation of the riskiest dependency.

**1.4 Push on enqueue.** When a request lands in the pending queue as `Ask`,
push to every registered device. Payload: request id, risk tier, agent name,
project basename. Not the command, unless opted in.

**1.5 Escalation flag.** Mark each request with whether it may be decided from a
wrist. Anything `high` is phone-or-Mac only, per the design doc's list —
force-push, `rm -rf`, `terraform apply`, `kubectl delete`, publishes,
credential access. `blocked` never reaches the queue at all. The server decides
this, not the client, for the same reason classification lives in the daemon.

**1.6 Decision attribution and reason.** Accept an optional `reason` on the
decide endpoint and record which device decided. The reason field is what makes
a dictated denial useful later — `permissionDecisionReason` already flows back
into the agent's context, so "use the existing helper instead" would land where
the agent reads it.

**1.7 Policy endpoints.** `agent-gate policy list/forget` reads the file
directly today. The phone needs the same over HTTP to browse and revoke
learned rules.

## Phase 2 — iPhone app

SwiftUI, sharing nothing with the menu bar app except the wire format.

Pair by scanning the QR, register the APNs token with the daemon, and hold the
connection config (LAN address, Tailscale hostname, preference order with
automatic fallback). Subscribe to `/events` for the live queue — the same SSE
stream the web page already uses. Present the four actions with the same
two-step confirmation the learning answers get everywhere else, browse and
revoke learned rules, and show the audit log.

Notification handling is the point of the phase: a remote push while
backgrounded, with actionable Allow and Deny buttons, and a foreground path
that just updates the list.

## Phase 3 — watchOS app

Small on purpose. The Watch is a fast approval surface, not a policy editor.

The notification carries agent, project, risk tier, one-sentence reason, and as
much of the command as fits — with Allow and Deny as notification actions so a
decision needs no app launch. High-risk requests show "Open on iPhone" instead
of Allow; Deny stays available at every tier, because deny is always safe to
make one tap and allow is not. Haptic pattern by risk tier, so the wrist tells
you the severity before you look. A Smart Stack widget showing the pending
count.

The failure case deserves as much care as the happy path: when the Watch cannot
reach the Mac, say so on the watch face. An approval that silently failed to
send is the one outcome this system must never produce.

## Phase 4 — Living with it

TestFlight is sufficient indefinitely for your own devices and avoids App
Store review entirely. Consider the store only if other people want it.

The thing to watch after a week of real use is notification volume. If the
wrist buzzes too often the tool gets muted, and a muted gate protects nothing.
That pressure is what makes learned rules and, eventually, a smarter triage
model worth their complexity — the metric that matters is how many pushes you
get per day, and how many of them you would have wanted.

## Sequencing

The Apple dependencies are the long pole and the daemon work is independent of
them, so start both at once.

| | Work | Rough effort | Blocked by |
| --- | --- | --- | --- |
| Now | Phase 0 enrolment and keys | an hour, plus waiting | — |
| Now | 1.1 device registry, 1.2 QR pairing | 2–3 days | nothing |
| Then | 1.3 APNs sender → `BadDeviceToken` | 1–2 days | `.p8` key |
| Then | 1.4–1.7 push, escalation, endpoints | 2 days | 1.1, 1.3 |
| Then | Phase 2 iPhone app | 1–2 weeks | 1.1–1.7 |
| Then | Phase 3 watchOS app | 3–5 days | Phase 2 |

Estimates assume familiarity with SwiftUI and are the usual optimistic
engineering guesses; the Apple-side friction is the part that reliably takes
longer than planned.

## Risks

**Sandbox versus production APNs mismatch** is the classic time sink. Dev
builds use the sandbox endpoint, TestFlight and App Store builds use
production, and a token from one is rejected by the other. Make the
environment explicit in config and log which one is in use.

**No route home.** Notifications will arrive in places your Mac is not
reachable from. Without a tunnel the notification is informative only. Decide
whether that is acceptable or whether Tailscale is a hard prerequisite, and say
so in the UI.

**A late decision cannot retroactively allow execution.** If the agent has
already fallen back, an Allow from the wrist is an audit record of intent, not
a command that runs. What makes the tap genuinely useful is turning it into a
policy update — which is exactly what the learned rules shipped for. Wire the
Watch's "allow similar" to that, and a late tap still changes the future even
when it cannot change the past.

**The entitlement path remains open as a fallback.** If Apple's APNs route ever
proves unworkable, `NEAppPushProvider` from option 2 of the original note is
still the no-relay answer, at the cost of an entitlement request and Wi-Fi
scoping.
