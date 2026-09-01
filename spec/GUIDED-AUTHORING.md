# Guided AI authoring

**Status:** Implemented Phase 5 baseline
**Target:** Krit 0.2, authoring protocol 1
**Deterministic language-server status:** Implemented
**Review-gated assistance status:** Implemented

## Goal

Krit should guide a person toward compact, understandable, valid agent code as
they type. AI assistance predicts and proposes code; deterministic tools decide
whether code is valid, formatted, authorized, and ready to run.

The editor experience is optional. Krit source, builds, and deployments never
require an LLM service.

## Trust boundary

LLM output is untrusted source text.

An authoring model cannot:

- bypass the parser, type/effect checker, or capability policy
- grant permissions
- execute code or package installation
- silently modify committed files
- suppress diagnostics
- read secrets
- upload source without explicit configuration
- mark its own suggestion as reviewed

Accepted suggestions enter the same compiler pipeline as handwritten code.

## Guidance layers

### 1. Deterministic guidance

Always available and offline:

- parser-aware completion
- type and field completion
- import completion
- stable diagnostics and fixes
- canonical formatting
- unused binding and capability detection
- exhaustiveness checks
- route, effect, and permission summaries

This layer supplies facts to both people and models.

The 0.2 bootstrap provides the comment-preserving canonical formatter,
`krit fmt --check`, and the first deterministic language-server milestone.
`krit lsp` now supplies compiler diagnostics, canonical document edits, a
format fix action, type/effect/resource hover, bounded completion, document
symbols, and versioned compiler facts. Import completion remains unavailable
because edition 2026 has no implemented import syntax. Broader deterministic
refactors and provider-neutral prediction remain later authoring milestones.

### 2. Predictive completion

The implemented optional provider path proposes one bounded edit to the
package entry source using:

- the current syntax tree
- visible symbols and inferred types
- approved package interfaces
- diagnostics near the cursor
- a bounded window of user-selected source
- project style and capability constraints

Suggestions are shown as provisional editor text. Nothing is written until the
user accepts it.

`krit assist inspect` prints the exact provider-neutral request without
contacting a provider. `krit assist suggest` prints and flushes the same
inspection before provider invocation, validates the strict response, and
writes only a proposal JSON artifact. It never writes source.

### 3. Readability cleanup

After acceptance, deterministic formatting runs automatically. Static tools
then identify:

- unnecessary nesting
- repeated expressions suitable for a named function
- unclear or inconsistent names
- hidden control flow
- overly broad effects or capabilities
- ignored error results
- duplicated connector policy

An LLM may propose a semantic refactor through `--kind cleanup`, but it uses
the same proposal validation as completion and diagnostic repair. The exact
canonical unified diff, deterministic explanation, and fresh compiler and
permission facts are visible before acceptance. Semantics-changing cleanup is
never applied silently.

## Authoring loop

```text
person types intent or code
    -> local parser/type context updates
    -> optional model predicts a small edit
    -> person accepts, edits, or rejects
    -> canonical formatter runs
    -> checker reports types/effects/permissions
    -> optional model proposes a diagnostic repair
    -> person reviews the exact diff
    -> conformance/tests run
```

Predictions should be small and interruptible. Whole-project regeneration is a
separate explicit action.

The implemented CLI loop is:

```text
krit assist inspect ...                         # no provider call, no write
krit assist suggest ... --proposal edit.json   # provider call, proposal write only
krit assist review --proposal edit.json ...    # revalidate and show exact diff
krit assist accept --proposal edit.json --reviewed \
  [--approve-permission capability[=resource]] # atomic source write
```

The target is exactly one validated package-entry `.krit` document. Multi-file
model edits and autonomous project rewriting are not implemented.

## Compiler facts API

The language server exposes versioned structured facts:

- syntax node and expected grammar positions
- visible symbols
- inferred and declared types
- effect rows
- required and granted capabilities
- package interface documentation
- diagnostic codes, spans, and applicable deterministic fixes
- canonical formatting edits

The implemented protocol is synchronous LSP over stdio. It uses full-document
synchronization and UTF-16 positions, publishes compiler diagnostics, and
supports standard hover, completion, document symbols, formatting, and code
actions. `krit/compilerFacts` accepts a standard `textDocument` identifier and
returns schema 1 with:

- language and authoring-protocol versions
- stable diagnostics with byte spans and UTF-16 ranges
- module effects, literal-resource requirements, and entrypoints
- symbols with inferred/declared types, reference status, and visibility
- expression syntax kinds, inferred types, effects, requirements, and
  resolved symbol/built-in identities
- applicable package metadata, requested/required permissions, usage, and
  local manifest grant status
- canonical whole-document formatting edits

The server limits each protocol frame to 16 MiB, each open document to 1 MiB,
the open set to 128 documents, and the applicable manifest read to 256 KiB.
Completion, document symbols, recursive type rendering, and compiler-fact
responses are separately bounded. Package facts are attached only when the
nearest `krit.pkg` passes the normal canonical entry-containment validation.
The server reads only local source supplied by the editor and that validated
manifest. It does not invoke the evaluator, lower/build components, install
packages, open sockets, perform network requests, read host
configuration/secrets, or call a model.

The LLM consumes these facts instead of guessing from text alone.

## Privacy

Default behavior is local deterministic tooling with LLM assistance disabled.

When a model is enabled:

- the user chooses local or remote provider
- the UI shows which files and ranges may be sent
- `.kritignore` and package policy exclude paths
- secrets and capability values are always excluded
- prompts are not stored by Krit unless the user enables local history
- telemetry is off by default
- enterprise hosts can enforce provider and retention policy

Remote provider credentials are host-managed and never exposed to Krit
components or model prompt context.

The implemented request requires:

- an explicit strict schema-1 provider config with `enabled: true`
- an explicit package manifest, target file, intent, and UTF-16 range or
  `all`
- explicit additional `PATH@RANGE` context selections

Relative selections resolve from the canonical package root. Only `.krit`
files are eligible. Canonical containment, package-entry identity,
`.kritignore`, `.git`/`target` exclusion, and symbolic-link escape checks run
before reading context. Host configuration, runtime requests/responses,
artifacts, proposal files, manifests, lockfiles, credentials, and non-Krit
files cannot enter model context.

Direct `config_string`, `secret`, `http_request`, and `ai_invoke` resource
literals, structured log event names, and recognized secret-like string values
are replaced with explicit redaction markers. Compiler facts remove canonical
source edits, retain only diagnostics/symbols/expressions intersecting the
selected target range, and redact every structured permission resource plus
resource-bearing rendered types and diagnostic messages. Real source/context
digests remain host-local; provider-visible preconditions hash only redacted
representations. Prompt injection text remains inert, visibly untrusted source
context; it is not interpreted as policy or a tool request.

On Unix, selected source is opened by traversing from an already opened
canonical package-root directory descriptor with `NOFOLLOW` on every path
component. Containment and ignore decisions therefore cannot be followed by a
pathname-based symlink swap that reads an outside file.

## Provider neutrality

The authoring protocol uses a provider-independent request containing compiler
facts, bounded source context, edit intent, and output schema.

Providers return structured text edits, not executable tools. A local model,
hosted model, or deterministic completion engine can implement the protocol.
Core editor features must not depend on provider-specific prompt syntax.

The implemented adapter is `http-json`. The endpoint must be HTTPS or loopback
HTTP and cannot contain user information, a query, or a fragment. Redirects
and inherited proxies are disabled. Connect/overall timeouts and
request/response sizes are bounded. An optional `credentialEnv` names one host
environment variable used only for a bearer Authorization header. Its value is
never serialized, displayed, logged, stored in a proposal, compiled, or passed
to Krit source/runtime.

Provider config example:

```json
{
  "schema": 1,
  "enabled": true,
  "provider": {
    "kind": "http-json",
    "endpoint": "https://authoring.example.test/krit/suggest",
    "credentialEnv": "KRIT_ASSIST_TOKEN",
    "connectTimeoutMs": 5000,
    "timeoutMs": 20000
  }
}
```

There is no default provider lookup. Missing, disabled, unknown, or unsafe
configuration is `K8101`.

## Authoring protocol 1

Requests and responses are strict JSON with unknown fields rejected. A request
contains:

- schema, authoring protocol, prompt-pack, language, and edition versions
- a deterministic BLAKE3 request identity
- completion, diagnostic-repair, or semantic-cleanup intent
- the fixed authoring instruction
- one package-relative target path, provider-visible digest/length of the
  redacted selected representation, and selected byte plus UTF-16 range
- sorted explicit context slices with redacted representation preconditions,
  redacted text, redaction ranges, and `untrusted: true`
- bounded range-filtered schema-1 language-server compiler facts

A response repeats the request identity and exact target path/base digest,
adds an untrusted bounded summary, and supplies 1-64 sorted non-overlapping
text edits. Every edit contains matching UTF-8 byte and UTF-16 ranges and must
remain inside the selected target range. Cross-document edits, ambiguous
insertions, NUL, malformed/unknown fields, stale digests, and unsupported
versions fail with stable `K8103`/`K8104` diagnostics.

Current hard limits are 1 MiB per source, 16 context ranges, 64 KiB per range,
256 KiB total redacted source context, 512 KiB per request, 1 MiB per provider
response, 64 edits, 64 KiB per edit, 256 KiB total replacement text, 4 KiB
provider summary, 4 MiB proposal, 256 KiB manifest, and 64 KiB
`.kritignore`.

## Proposal review and acceptance

Provider edits are applied only in memory. Krit then canonicalizes the complete
candidate, parses and analyzes it, and verifies Core IR without evaluating,
building, installing, invoking runtime code, or performing provider/network
work. Invalid candidates are `K8105`.

The proposal records no timestamp or reviewed claim. It contains deterministic
request/response identities, host-local full-source/context digests that were
never sent to the provider, the canonical candidate digest, a unified diff,
before/after diagnostics and top-level types, effect deltas,
manifest-requested permission usage, exact required/granted/missing permission
facts, and added or removed authority. The provider summary is labeled
untrusted.

Review recomputes every source/context/manifest digest, redaction, filtered
compiler fact, response range, candidate, diff, and report. `accept` requires
the separate `--reviewed` flag. Every newly required permission must be named
exactly with `--approve-permission`; extra or missing approvals fail. Approval
does not grant authority: all candidate requirements must already be present
in the unchanged manifest. The canonical source is staged beside the target,
receives the target permissions before bytes are written, and is synchronized.
On macOS and Linux, Krit atomically exchanges staged and target paths, validates
the displaced source digest, removes it only on a match, and otherwise
exchanges it back before reporting staleness. The single-document protocol
cannot partially update multiple files.

Other platforms may inspect, suggest, and review proposals, but protocol-1
acceptance fails closed with `K8107` before changing source until an audited
safe displaced-file replacement primitive is available there.

## Versioned generation prompt

Every compiler release ships one provider-neutral generation prompt tied to a
specific language version and edition. The pack contains:

- implemented grammar and value kinds
- compiler and standard-library constraints
- canonical readable examples
- unsupported-feature boundaries
- diagnostic repair instructions
- prompt schema and compatibility metadata

Claude, ChatGPT, Gemini, and local models receive the same semantic material.
Provider adapters change transport, not language rules.

Every embedded Krit example is parsed in CI. Draft features are excluded until
their compiler support ships. This prevents a model from generating future
syntax against the current compiler.

The exact current pack is printable with `krit prompt`. Generated text remains
untrusted and enters the normal parse, check, permission, and review loop.

## Prompt-injection resistance

Source comments, strings, package documentation, API responses, and generated
files are untrusted model context. They cannot grant tools or change system
policy.

The authoring service has no runtime capabilities. If an explicit agentic edit
mode later receives file or command tools, each tool is separately scoped,
logged, and approved according to host policy.

## Readability contract

The canonical formatter and checker optimize for human review:

- one stable layout
- explicit imports and entry points
- explicit error paths
- explicit external effects
- no implicit truthiness or conversions
- exhaustive variants and list matching
- named schemas at API boundaries
- permission changes shown beside source edits

Readability diagnostics should explain cost rather than enforce arbitrary line
counts. Teams can choose stricter profiles.

## Explainability

The implemented bootstrap `krit explain` produces deterministic human and
schema-1 JSON views of the synthetic module-init entrypoint, inferred effects,
top-level binding/function types, and resolved typed Core IR.

As the corresponding language and host features arrive, the command extends
those compiler facts with:

- exported routes, webhooks, schedules, queues, and tools
- external services and model providers
- data schemas
- call and effect paths
- requested and granted capabilities
- retry, timeout, and resource policies
- possible failure variants

The explanation is derived from compiler facts, never generated solely by an
LLM.

## Quality evaluation

Representative tasks measure:

- suggestion acceptance rate
- first-check success
- diagnostic repair success
- compilation and runtime correctness
- permission over-request rate
- edit and review time
- source complexity relative to generated Rust
- human comprehension accuracy

Model-generated code is not considered good because it is shorter. It must
remain explicit about effects, failures, authority, and external contracts.

## Initial milestone

The first guided workflow is implemented for the reference webhook agent from
`AGENT-APPLICATIONS.md`:

1. complete a typed route and request schema
2. suggest approved connector operations
3. add explicit error handling
4. identify required capabilities
5. format accepted edits
6. explain the final behavior and permission diff

The same project remains fully editable, checkable, buildable, and runnable
with assistance disabled. Tests compare offline compiler behavior before and
after a disabled assist attempt and keep `krit-assist` out of compiler, LSP,
package, Wasm, and runtime dependency graphs.
