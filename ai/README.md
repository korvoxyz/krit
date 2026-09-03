# Krit AI generation pack

Krit ships provider-neutral material for generating valid, readable source
with Claude, ChatGPT, Gemini, or a local model. An LLM is an authoring tool,
not part of the compiler or runtime.

## Current pack

- Prompt: [`KRIT-0.2-SYSTEM.md`](../crates/krit-cli/assets/KRIT-0.2-SYSTEM.md)
- Metadata: [`prompt-pack.json`](prompt-pack.json)
- Language authority: [`../spec/LANGUAGE.md`](../spec/LANGUAGE.md)

The prompt contains only implemented Krit 0.2 syntax, including records,
built-in Option and Result values, checked annotations, JSON conversion,
canonical formatting, and the static rules enforced by `krit check`. It also
contains the implemented typed webhook, literal configuration read, opaque
secret acquisition, exact-origin `http_request`, provider-neutral
`ai_invoke`, and structured `log_info`/`log_error` forms. Raw sockets, ambient
host values, secret revelation, broad network grants, provider SDKs, and
self-approval remain explicitly unavailable. The compiler can explain exact
requirements, build scalar or bounded webhook artifacts, inspect effective
local permissions, and execute them through the sandbox, fixture invocation,
or loopback server paths. `krit lsp` exposes the same deterministic compiler,
package, permission, completion, and formatting facts offline without
executing source or granting runtime/network authority.
`krit assist` optionally sends only explicit bounded redacted context and
range-filtered compiler facts to a configured provider, then treats the
response as an untrusted review-gated source proposal.
The current pack also includes named transactional state, workflow
checkpoints, durable HTTP/AI replay, typed durable queues with host-owned
leases and dead letters, host-owned UTC scheduled triggers, capability-scoped
bounded object buckets, catalogued parameterized database transactions, and
schema-5 host configuration. It does not claim
distributed exactly once, expose database paths or SQL to source, provide a
guest clock or cron expression, or expose guest-visible object listing.

## Use

Print the exact prompt shipped with the compiler:

```sh
krit prompt
```

Use that output as the system instruction or project instruction for the
chosen model, then provide the developer's task. For example:

```text
Write a Krit program that recursively sums a list of integers and prints the
result for [10, 20, 12].
```

Save the returned Krit block and check it:

```sh
krit fmt generated.krit
krit fmt --check generated.krit
krit check generated.krit
krit explain --json generated.krit
krit lsp # editor stdio integration
krit assist inspect --provider-config assist-provider.json --manifest krit.pkg --file generated.krit --range all --intent "Repair the current diagnostics."
krit assist suggest --provider-config assist-provider.json --manifest krit.pkg --file generated.krit --range all --intent "Repair the current diagnostics." --proposal target/assist-proposal.json
krit assist review --manifest krit.pkg --proposal target/assist-proposal.json
krit build
krit permissions --artifact target/krit/<package>.wasm
krit sandbox # module-init artifacts
krit invoke --request request.json --host-config host.json # webhook artifacts
```

`krit check` accepts the full bootstrap language. `krit build` accepts the
scalar policy-1 subset and a bounded webhook subset containing strings, fixed
HTTP records, header lists, Result/Option matching, static non-capturing
helpers, config, secrets, HTTP, AI strings, log fields, and bounded unescaped
JSON-string decoding. A model must still avoid arbitrary composites, general
JSON in components, escaped JSON strings, data captures, and dynamic string
operators. A K7001 or K7002 build diagnostic must be repaired in source
rather than bypassed with host interpretation.
`krit sandbox` never builds or falls back to the full direct evaluator, so the
artifact must already exist and validate with its adjacent metadata.

For an AI-authored webhook contract, use:

```sh
krit fmt agent.krit
krit fmt --check agent.krit
krit check agent.krit
krit explain --json agent.krit
```

Build it with an exact manifest, then invoke it using strict request and host
config JSON. Schema 2 host config owns adapter mappings, retries, finite rates,
process-local idempotency, and explicit noninteractive approvals.
`krit run` still returns K5003 rather than simulating hosts.
`krit invoke` and `krit serve` never build or fall back to interpretation.

State-enabled source additionally uses:

```sh
mkdir -m 700 state
krit build --manifest examples/stateful-webhook.krit.pkg
krit invoke \
  --manifest examples/stateful-webhook.krit.pkg \
  --host-config examples/stateful-webhook.host.json \
  --request examples/webhook-agent.request.json
```

`state_get`/`state_put`/`state_delete` and named checkpoint operations use
bounded strings. `replay_http` and `replay_ai` record completed results under
stable operation names. A provider-side effect can still complete before its
local replay record commits, so generated code must use safe methods or stable
idempotency keys and must not describe the result as distributed exactly once.

AI output is nondeterministic untrusted raw UTF-8. Generated source must match
the `Result`, explicitly parse or validate data before structured use, and
never execute it. Prompts, responses, credentials, and sensitive request
values must not be logged by default. `invoke` response JSON stays on stdout;
structured log JSON Lines use stderr.

For a failed check, send the model:

1. the same versioned system prompt
2. the generated source
3. JSON diagnostics from `krit check --diagnostic-format json`
4. the instruction: “Make the smallest edit that fixes these diagnostics.
   Do not add unsupported language features.”

Inspect inferred types/effects, then run and review permissions only after
checking:

```sh
krit explain generated.krit
krit run generated.krit
krit permissions
krit permissions --artifact target/krit/<package>.wasm
krit sandbox
```

## Provider contract

Provider adapters may change transport and message formatting, but must not
change language rules. Each request includes:

- prompt-pack schema and version
- Krit language version and edition
- the developer's task
- approved package interfaces when those exist
- bounded source context
- relevant compiler diagnostics

The response is untrusted text. Krit parses and checks it exactly like
handwritten source.

The implemented authoring-protocol response is stricter than free-form
generation: it is one schema-1 JSON object containing sorted non-overlapping
edits for the selected package-entry document. `inspect` prints the exact
request without a provider call. `suggest` prints that request before invoking
the provider and writes only a proposal JSON file. `review` recomputes the
canonical diff and compiler/permission facts. `accept --reviewed` writes only
after fresh checks and exact approval of newly required permissions.

The generic provider config is explicit and disabled by default:

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

The endpoint is provider-neutral. Remote transport requires HTTPS; loopback
HTTP supports local models and deterministic test providers. Redirects and
inherited proxies are disabled. The named environment credential is used only
as an HTTP bearer header and is never model context or proposal data.

## Updating the pack

Every prompt pack is tied to a compiler/language version. A change must:

1. describe only implemented syntax
2. include canonical readable examples
3. add or update generation evaluation tasks
4. pass parsing tests for every embedded Krit block
5. pass canonical formatting and idempotence tests for every embedded Krit
   block
6. record compatibility in `prompt-pack.json`
7. update the changelog

Old packs remain available for projects pinned to an older language version.

## Privacy

Krit does not choose a provider or upload source automatically. The user or
editor configures a local or remote model and explicitly selects context.
Secrets, capability values, ignored files, and runtime data are excluded from
prompt context. Runtime AI adapter prompts and responses are not telemetry,
permission facts, cache keys, diagnostics, or default structured logs.

`.kritignore` is enforced relative to the canonical package root. Only
explicit `.krit` ranges may be included; generated paths, non-source files,
symlink escapes, host config, runtime data, and paths outside the package are
rejected. Capability-resource/event literals and recognized secret-like
strings are replaced with visible redaction markers. Comments and strings that
contain prompt-injection text remain inert untrusted text and cannot alter the
protocol or mark a proposal reviewed.
