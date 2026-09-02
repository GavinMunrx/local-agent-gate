# Getting an approval onto an Apple Watch

Last updated: 2026-09-02

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

But APNs requires a provider server holding the push key and a registry of
device tokens. That is a hosted relay, which the product explicitly promises
not to have ("no hosted relay, no required account, no source upload"). The
payload could carry no command text - only "an approval is waiting" - which
keeps repository content off the wire, but the relay still exists and still
learns when and how often the user's agents ask for dangerous things.

### 4. Third-party push bridge (ntfy, Pushover, Pushcut)

The Mac POSTs to a push service, its iOS app receives, the Watch mirrors.
Works today with no Apple approval and little code.

Worth being precise about a common misconception: self-hosting ntfy does not
avoid a third party for iOS, because its iOS app still receives via ntfy's own
APNs infrastructure. This is the fastest path to a working Watch notification
and the furthest from the product's stance.

### 5. watchOS background refresh

Budgeted at roughly one wake every 15-60 minutes. The request expiry is 120
seconds. Not viable, at any amount of effort.

## What this means for the product

The honest summary is that **"no hosted relay" and "reliable Watch
notifications anywhere" cannot both be true.** Option 2 gets closest: no
relay, reliable, but Wi-Fi-scoped and gated on an Apple entitlement. Options 3
and 4 work everywhere and break the promise. Option 1 keeps the promise and
does not solve the problem.

The recommended shape is to be explicit rather than split the difference
silently:

- Default to **option 2**, and start the entitlement request early, since it
  is the long pole and may be refused.
- Ship **option 1** first regardless. It is the same app, it validates pairing
  and the approval UI, and it is genuinely useful at a desk with the phone
  awake.
- Treat **option 3 or 4 as an opt-in fallback** the user turns on knowingly,
  with the tradeoff stated in the UI rather than buried. A notification that
  says only "1 approval waiting" leaks far less than one carrying the command.

## Two design problems the Watch surfaces early

**The expiry window is too short for a human who has walked away.** 120
seconds is tuned so an adapter's hook does not hang. But a notification is
worth little if the request dies before a wrist is raised. These are different
timeouts pretending to be one: how long the *agent* waits, and how long the
*request* stays answerable. Splitting them - let the agent fall through to its
own prompt quickly, while the request stays live and answerable for longer,
with the decision recorded as a policy update rather than an execution
approval - is probably the right model, and it is a daemon change, not a Watch
change.

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
