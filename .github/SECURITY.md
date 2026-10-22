# Security Policy

## Supported Versions

Freminal is in active pre-1.0 development. Security fixes are applied to
the **latest released version** only. There are no long-term-support
branches; older releases do not receive backported fixes.

| Version        | Supported          |
| -------------- | ------------------ |
| Latest release | :white_check_mark: |
| Older releases | :x:                |
| Nightly builds | Best effort        |

The current release line is tracked in [`MASTER_PLAN.md`](../Documents/MASTER_PLAN.md)
and on the [Releases page](https://github.com/fredsystems/freminal/releases).
If you are affected by a security issue on an older version, upgrade to the
latest release; the fix will land there.

## Reporting a Vulnerability

You may report a vulnerability either privately or publicly; both are
accepted.

- **Private (preferred for anything that is not already public).** Use
  GitHub's
  [private vulnerability reporting](https://github.com/fredsystems/freminal/security/advisories/new).
  This opens a private security advisory that only the maintainers can see.
  It gives us time to fix the issue before details circulate, and lets us
  collaborate with you on the fix and, if warranted, request a CVE.
- **Public.** If you prefer, or if the issue is already public (for
  example, it was found by the public fuzz targets or discussed elsewhere),
  you may open a public GitHub issue or start a discussion.

The maintainers may publish a security advisory **before** a fix ships when
they judge that disclosure serves users better than waiting. A typical case
is a finding that the public fuzz targets rediscover within seconds, where
withholding details would not keep it confidential. In that case remediation
is tracked openly, in the published advisory and in
[`PLAN_VERSION_130.md`](../Documents/PLAN_VERSION_130.md), and the advisory
is updated with the patched version once a release contains the fix.

When reporting, please include as much of the following as you can:

- The affected version (or commit) and platform (Linux, macOS, Windows).
- A description of the vulnerability and its impact.
- Steps to reproduce, ideally a minimal proof of concept (for example, an
  escape-sequence stream or `.frec` recording that triggers the issue).
- Any suggested mitigation, if you have one.

## What to Expect

Freminal is maintained by a small team, so response times reflect that.
As a guideline:

- **Acknowledgement** of your report within **7 days**.
- An initial **assessment** (accepted, needs more information, or declined,
  with reasoning) within **14 days**.
- If accepted, we will work on a fix and coordinate a release and public
  disclosure with you (or, for a report that is already public, track the
  fix openly). We aim to publish a fix within **90 days** of
  acknowledgement; complex issues may take longer, and we will keep you
  updated on the advisory.
- If declined, we will explain why (for example, the behaviour is intended,
  out of scope, or not reproducible).

We are happy to credit reporters in the published advisory unless you ask to
remain anonymous.

## Scope

In scope:

- The Freminal terminal emulator and its workspace crates
  (`freminal`, `freminal-terminal-emulator`, `freminal-buffer`,
  `freminal-common`, `freminal-notify`, `freminal-windowing`).
- Escape-sequence, OSC, DCS, APC, and image-protocol parsing and handling.
- Recording (`.frec`) and layout file parsing.
- Configuration file loading.

Out of scope:

- Vulnerabilities in third-party dependencies (report those upstream).
  Freminal checks its dependencies against the RustSec advisory database
  with cargo-deny on every pull request and daily on `main`, and Renovate
  proposes dependency updates.
- Issues that require an attacker to already have arbitrary code execution
  or local shell access equivalent to the user running Freminal.
- Denial of service caused by deliberately pathological input that a normal
  shell session would not produce, unless it is trivially triggerable by a
  remote party (for example, via output from a benign command).
