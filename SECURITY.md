# Security policy

Krit exists so that people can trust AI-written programs with narrow,
explicit authority. A way around that authority is a vulnerability, and we
treat it as one even though Krit is pre-1.0 and not yet production-ready.

## Supported versions

| Version | Status |
|---|---|
| 0.3.x | Supported: security fixes and regular releases |
| 0.2.x | Security fixes only, until 0.4.0 is released |
| < 0.2 | Not supported |

Security fixes ship as new patch releases of each supported line. The
archived Racket prototype (tag `racket-v0.1.0`) is not supported.

## Verifying a release

From 0.3.3 on, the repository's Release workflow attests every release
archive, `SHA256SUMS`, and the multi-architecture container image with signed
SLSA build provenance. Verify a download or image with the
[GitHub CLI](https://cli.github.com/manual/gh_attestation_verify) before
trusting it:

```sh
gh attestation verify krit-0.3.3-x86_64-unknown-linux-gnu.tar.gz --repo korvoxyz/krit
gh attestation verify oci://ghcr.io/korvoxyz/krit:0.3.3 --repo korvoxyz/krit
```

Add `--signer-workflow korvoxyz/krit/.github/workflows/release.yml` to also
require the Release workflow as the signer. Published releases are never
modified: their assets are uploaded to a draft, verified, and only then
published, and a fix ships as a new patch release. A download or image
presented as Krit that fails verification is in scope; report it privately as
described below.

## Reporting a vulnerability

Do not report a suspected vulnerability in a public issue, discussion, pull
request, or commit message.

Report it privately with GitHub private vulnerability reporting:

1. Open the [Krit repository](https://github.com/korvoxyz/krit) on GitHub.
2. Open the **Security** tab.
3. Select **Report a vulnerability** and complete the form.

This creates a private draft advisory visible only to you and the
maintainers. If you cannot use GitHub private vulnerability reporting, open a
public issue that asks for a private channel and leave out every detail of
the problem.

A useful report includes:

- the affected version (`krit --version`) and host platform
- the affected component, such as the compiler, a CLI command, the runtime
  host, the language server, or `krit assist`
- a minimal reproduction: Krit source, `krit.pkg`, host configuration, and
  the exact commands run
- what happened, what you expected, and the impact you believe it has
- any known workaround or suggested fix

Replace real secrets, credentials, tokens, and personal data with placeholder
values before you send anything. We never need a working credential to
reproduce a problem.

## What to expect

These are the targets of a small maintainer team, not contractual
guarantees:

| Step | Target |
|---|---|
| Acknowledge the report | within 3 business days |
| Initial assessment: validity, severity, affected versions | within 10 business days |
| Status updates until resolution | at least every 14 days |
| Fix released for critical or high severity | within 30 days of confirmation |
| Fix released for medium or low severity | within 90 days, or the next scheduled release |

If we decide a report is not a vulnerability, we explain why in the advisory.
We ask reporters to keep the details private until the advisory is published,
or for 90 days after the report, whichever comes first. We will agree on a
different date with you when a fix needs longer or a problem is already being
exploited.

## Scope

In scope are weaknesses in Krit itself that let code, input, or
configuration obtain more authority or information than Krit promises:

- **Compiler.** `krit check` or `krit build` accepting a program whose
  artifact exceeds its declared effects and capability requirements; artifact
  metadata that misstates effects, requirements, or approvals; components that
  import more than their selected WIT world; or artifact validation that
  accepts tampered bytes or metadata.
- **Runtime host.** `krit sandbox`, `invoke`, `worker`, `schedule`, `serve`,
  and the embedding crates: bypass of fuel, deadline, memory, host-call,
  output, or other configured limits; corruption or cross-store access of
  durable state, queues, objects, or databases; or untrusted request, guest,
  provider, or stored data that crashes the host or subverts its policy.
- **Sandbox escapes.** Guest code reaching files, sockets, processes,
  environment variables, clocks, randomness, or any other ambient host
  authority, or running Wasm features outside the documented policy.
- **Capability bypass.** Any operation on a resource that the manifest and
  host configuration do not grant exactly, including host configuration that
  widens manifest grants, redirects or DNS rebinding that expand an HTTP
  origin grant, and bypass of the default-deny AI-adapter or bearer-HTTP
  approval.
- **Secret disclosure.** Secret bytes reaching diagnostics, logs, artifacts,
  metadata, durable state, responses, compiler facts, `krit assist` provider
  requests or proposals, or error messages; or an assist provider credential
  sent anywhere other than its configured endpoint.

Also in scope are `krit assist` applying model output without the documented
review and approval steps, `krit lsp` executing source or reaching the
network, and tampering with published release archives, checksums, or
container images.

Out of scope:

- flaws in the logic of a Krit program, or harm done through authority that
  its manifest and host configuration deliberately grant, including a model
  steering a program to misuse a channel it was granted
- resource exhaustion that stays within the limits an operator configured
- limitations the documentation already states, such as the absence of
  production multi-tenant OS isolation
- vulnerabilities in dependencies that Krit cannot reach; report those
  upstream, but do tell us when one is exploitable through Krit
- versions marked unsupported above

When unsure, report it privately anyway. We would rather triage a report
that turns out to be out of scope than miss a real problem.

## Advisory process

1. We confirm the problem and assess severity with CVSS in the private
   advisory, where we may invite you to collaborate.
2. We develop the fix in a temporary private fork attached to the advisory,
   with regression tests, and a conformance case when the behavior is visible
   in the language.
3. We request a CVE identifier through GitHub when the problem warrants one.
4. We release patched versions for every affected supported line through the
   normal release workflow and record the fix under a security heading in
   `CHANGELOG.md`.
5. We publish the GitHub security advisory with affected and patched
   versions, severity, workarounds, and credit to the reporter, unless you
   prefer to stay anonymous.

Please test only against installations and data that you own or are
authorized to use, and do not degrade services that other people rely on.
