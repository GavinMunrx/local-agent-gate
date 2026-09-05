# Getting an approval onto an Apple Watch

Last updated: 2026-09-05

The design doc treats the Watch as MVP 4, reached through an iPhone app in
MVP 3. This note works out what that actually requires, because the delivery
constraint is an Apple platform rule rather than something we can engineer
around, and it collides with one of the product's stated promises.

## The constraint

**macOS notifications do not mirror to Apple Watch.** Only *iPhone*
notifications do, and only while the phone is locked or the wrist is raised.
So the Mac app raising a local notification does nothing for the Watch, and no
amount of work on the Mac side changes that.

Every route to the Watch therefore goes through the iPhone, and the real
question is how a Mac event reaches a phone that is not currently being looked
at. iOS suspends background apps within seconds; a socket held open by a
backgrounded app does not survive. That is the whole problem.

## The options

### 1. Foreground LAN app

The iPhone app connects to the daemon over the local network (or Tailscale)
and raises a local notification, which mirrors to the Watch.

Works today, needs no entitlement, and the LAN listener and `/events` stream
this repo now has are exactly its server side. But it only delivers **while
the app is on screen**, which is precisely when you do not need a
notification. Useless for the "I walked away" case that motivates the Watch.

### 2. Local Push Connectivity (`NEAppPushProvider`)

Apple's supported mechanism for delivering notifications from a local network
without going through APNs. A Network Extension keeps a connection alive on
designated Wi-Fi networks and can post local notifications while the app is
backgrounded.

This is architecturally the right answer, and it is the only option that
delivers reliably without a relay. Two real costs: it requires the
`com.apple.developer.networking.networkextension` entitlement with the
`app-push-provider` capability, which is **granted by Apple on request** and
aimed at enterprise on-prem messaging; and it only activates on Wi-Fi networks
the user nominates, so it does nothing on cellular.

### 3. APNs through our own provider

A real remote notification: works anywhere, wakes the device, no entitlement
beyond normal push.

~~But APNs requires a provider server holding the push key and a registry of
device tokens. That is a hosted relay~~ *Wrong, and corrected in
[`watch-plan.md`](watch-plan.md).* The provider can be **the daemon on this
Mac**: token-based APNs auth is a `.p8` signing key, short-lived JWTs and an
HTTP/2 client, none of which requires a machine other than the user's own. The
token registry is a local file. There is no product-operated server and no
account, so the promise holds as written.

What it does *not* give is end-to-end encryption - Apple sees the payload and
the timing metadata. That is the reason to push only "an approval is waiting"
and a request id, never the command text, and to say so plainly rather than
implying more privacy than exists.

### 4. Third-party push bridge (ntfy, Pushover, Pushcut)

The Mac POSTs to a push service, its iOS app receives, the Watch mirrors.
Works today with no Apple approval and little code.

Worth being precise about a common misconception: self-hosting ntfy does not
avoid a third party for iOS, because its iOS app still receives via ntfy's own
APNs infrastructure. This is the fastest path to a working Watch notification
and the furthest from the product's stance.

### 5. watchOS background refresh

Budgeted at roughly one wake every 15-60 minutes. Even with the request TTL
now at 600 seconds, a wake can easily land after the request is gone, and the
agent stopped waiting eight minutes earlier. Not viable, at any amount of
effort.

## What this means for the product

~~The honest summary is that "no hosted relay" and "reliable Watch
notifications anywhere" cannot both be true.~~ *Revised.* Once option 3 is
read correctly - the Mac is its own APNs provider - the two are compatible.
Option 3 is now the recommended default, and [`watch-plan.md`](watch-plan.md)
sequences the work.

Option 2 remains the fallback if the APNs route ever proves unworkable: no
relay and reliable, at the cost of an entitlement Apple grants case by case and
a Wi-Fi-only scope. Option 4 stays rejected - it is the only one that puts a
genuine third party in the path. Option 1 is not a delivery mechanism but is
still worth having, since it is the same app and validates pairing and the
approval UI.

## Two design problems the Watch surfaces early

**The expiry window was too short for a human who has walked away.** ~~120
seconds~~ *Done.* The single timeout has been split: `agent_wait_seconds`
(120s) is how long the agent blocks before falling back to its own prompt, and
`request_ttl_seconds` (600s) is how long the request stays answerable
afterwards. A late decision is recorded with a receipt noting the agent had
already moved on.

What a late decision *means* is still open. It cannot retroactively allow
execution - the agent has moved on - so it is currently an audit record of
intent. Turning it into a policy update ("allow commands like this next time")
is the natural next step and is what would make a Watch tap genuinely useful
rather than merely informative.

**The Watch is a bad place to approve high-risk work.** A 40mm screen cannot
show enough of a command to judge it, and the design doc's instinct to
escalate high-risk requests to the phone or Mac is right. The Watch should be
for the medium-risk, high-frequency approvals that make up most of the queue,
plus a always-available deny. Deny is safe to make one tap; allow is not.

## Where this repo already is

The daemon now has the pieces every option above depends on:

- A TCP listener (`agent-gate daemon --lan`), off by default.
- A pairing token, 32 random bytes, stored 0600, required on every network
  request. The Unix socket stays unauthenticated because filesystem
  permissions already gate it.
- A `/events` SSE stream that pushes the pending queue on connect and on every
  change, so a client is told rather than polling.
- `agent-gate pair` to show the address and token.

What does not exist yet: QR pairing, device identity beyond the single shared
token, signed decisions, and any iOS or watchOS client.
