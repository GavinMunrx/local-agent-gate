# Security

Local Agent Gate is a guardrail against an AI agent running a destructive
command before a human has seen it. Please read the trust boundary below before
reporting — several things that look like vulnerabilities are documented design
limits, and a few are genuinely serious.

## Reporting a vulnerability

Report privately, not in a public issue: open a
[GitHub security advisory](https://docs.github.com/en/code-security/security-advisories/guiding-contributors-through-security-vulnerabilities/privately-reporting-a-security-vulnerability)
on this repository.

This is a personal project with no funded security team and no bug bounty. You
should expect a first response within a couple of weeks, and a fix timeline
that depends on severity. Please give the fix a reasonable window before
disclosing publicly.

Include what you would want to receive: the version or commit, the exact
command or payload, what you expected the gate to do, and what it did.

## What is in scope

The bugs that matter most are the ones that make the gate lie about what it is
about to run:

- **A `blocked` command that is not denied.** This tier is unconditional and no
  rule may override it; anything that gets past it is the highest severity here.
- **Classifier evasion.** A command that runs something dangerous while being
  rated `low`, or that hides its real program from the shell parser. Parser
  gaps that make something *more* cautious are not vulnerabilities.
- **Decisions attributed wrongly**, delivered to the wrong request, or lost —
  including an approval that appears to be recorded but is not.
- **Network listener flaws.** Bypassing the pairing token, leaking it, or
  reaching an authenticated route without it. Note the listener is off unless
  the daemon is started with `--lan`.
- **A learned rule escaping its scope** — applying outside the project it was
  learned in, surviving revocation, or widening beyond the scope shown to the
  user when it was granted.

## What is out of scope

These are documented limits, not vulnerabilities. The README's *"What this
protects against — and what it does not"* section covers them in full.

- **Anything that never reaches a hook.** File writes, an agent's own network
  tools, and commands you run yourself are invisible to the gate by design.
- **Deferred execution.** Writing a `postinstall` script, a git hook or a
  Makefile that runs dangerous code later. The write is not a command.
- **A local process running as your user.** It can talk to the Unix socket,
  edit `.agent-gate.yml`, or write learned rules directly. The trust boundary
  is "my agent might make a mistake", not "my machine is hostile".
- **Anyone holding the pairing token.** It is a single shared secret that
  grants approval authority; per-device identity and signed decisions are not
  built yet.
- **Unknown commands rated `medium` rather than `low`.** That is the intended
  fail-safe direction.

## Reminder on the threat model

The gate reduces the blast radius of an agent's mistakes and gives you a
receipt for every decision. It is not a sandbox, not a substitute for
least-privilege credentials, and not a defence against an attacker who already
executes code as you.
