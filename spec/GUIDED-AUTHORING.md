# Guided AI authoring

**Status:** Draft
**Target:** Krit 0.4

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

### 2. Predictive completion

An optional LLM proposes the next expression, block, handler, schema, or
connector call using:

- the current syntax tree
- visible symbols and inferred types
- approved package interfaces
- diagnostics near the cursor
- a bounded window of user-selected source
- project style and capability constraints

Suggestions are shown as provisional editor text. Nothing is written until the
user accepts it.

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

An LLM may propose a semantic refactor, but it must be a visible diff with an
explanation and fresh compiler results. Semantics-changing cleanup is never
applied silently.

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

## Provider neutrality

The authoring protocol uses a provider-independent request containing compiler
facts, bounded source context, edit intent, and output schema.

Providers return structured text edits, not executable tools. A local model,
hosted model, or deterministic completion engine can implement the protocol.
Core editor features must not depend on provider-specific prompt syntax.

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

`krit explain` produces human and JSON views of:

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

The first guided workflow should help an author build the reference webhook
agent from `AGENT-APPLICATIONS.md`:

1. complete a typed route and request schema
2. suggest approved connector operations
3. add explicit error handling
4. identify required capabilities
5. format accepted edits
6. explain the final behavior and permission diff

The same project must remain fully editable, checkable, and runnable with AI
assistance disabled.
