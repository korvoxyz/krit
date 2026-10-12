# Krit AI generation pack

Krit ships provider-neutral material for generating valid, readable source
with Claude, ChatGPT, Gemini, or a local model. An LLM is an authoring tool,
not part of the compiler or runtime.

## Current pack

- Prompt: [`KRIT-0.3-SYSTEM.md`](../crates/krit-cli/assets/KRIT-0.3-SYSTEM.md)
  (prompt version 0.3.0, within an 8,000-token budget)
- Metadata: [`prompt-pack.json`](prompt-pack.json)
- Assist instruction: [`KRIT-AUTHORING-1.md`](../crates/krit-assist/assets/KRIT-AUTHORING-1.md)
- Language authority: [`../spec/LANGUAGE.md`](../spec/LANGUAGE.md)

The prompt teaches idiomatic Krit 0.3 (edition 2026) and nothing else:

- a grammar summary with every 0.3 construct: postfix `?`, let-else, nested
  `Option`/`Result` patterns with `_` and exhaustive arms, named construction
  of `HttpRequest`/`HttpResponse`/`HttpHeader`/`LogField` with defaults and
  punning, entry lists `["k": v]`, the unit value `()`, `///` doc comments,
  interpolation `\(name.field)`, and `"""` text blocks;
- the standard library v0 signatures, including
  `url_path_segment: fn(String) -> Result<String, String>`;
- every host operation's signature, the three entrypoint signatures, and the
  host-owned semantics (commit on success, replay, catalogued SQL, lossy
  cache, host retries);
- authority rules: literal resources, origin-bound secrets in the manifest,
  opaque `Secret` values, untrusted model output that must be validated, and
  K9013-safe paths and queries;
- a "Krit is not Rust, TypeScript, or Python" table mapping every habit code
  (K1101–K1112, K3011) and other common mistakes to the Krit spelling;
- idioms as checked examples: `Result` helpers with `?`, let-else in
  webhooks, string routing, nested patterns, text-block prompts, encoded
  paths, queue and schedule handlers, database rollback, cache fallbacks,
  and pure list recursion;
- the bounded component subset, a diagnostics cheat sheet, and the
  canonical style.

It no longer recommends the 0.2 workarounds that 0.3 replaces: nested
`match` pyramids with `Err(e) => Err(e)` arms, `record { ... }` for contract
types, "no early exit", or keeping every effect at the top level. Raw
sockets, ambient host values, secret revelation, broad network grants,
provider SDKs, and self-approval remain unavailable.

`krit lsp` exposes the same deterministic compiler, package, permission,
completion, and formatting facts offline without executing source or
granting runtime or network authority. `krit assist` optionally sends only
explicit bounded redacted context and range-filtered compiler facts to a
configured provider, then treats the response as an untrusted review-gated
source proposal.

## Use

Print the exact prompt shipped with the compiler:

```sh
krit prompt
```

Use that output as the system or project instruction for the chosen model,
then give it the developer's task, for example:

```text
Write a Krit webhook that stores a POSTed body under the request path in the
"agent-work" store and answers 405 to every other method.
```

A good answer is one canonical block that checks and builds:

```krit
/// Stores the body under the request path; only POST is accepted.
webhook fn handle(request: HttpRequest) -> HttpResponse {
    if request.method == "POST" {
        match state_put("agent-work", request.path, request.body) {
            Ok(_) => HttpResponse { status: 201 },
            Err(reason) => HttpResponse { status: 500, body: reason },
        }
    } else {
        HttpResponse { status: 405, headers: ["allow": "POST"] }
    }
}
```

Save the returned block and check it:

```sh
krit fmt generated.krit
krit fmt --check generated.krit
krit check generated.krit            # entrypoints: also the component target
krit explain --json generated.krit
krit build
krit permissions --artifact target/krit/<package>.wasm
krit invoke --request request.json --host-config host.json   # webhooks
krit sandbox                                                 # module programs
```

`krit check` runs the same backend as `krit build` for a file that declares a
webhook, queue, or schedule, so it reports exactly the K7001/K7002 findings
the build would, at their source spans. Those must be repaired in source,
never bypassed with host interpretation. `krit run` evaluates module programs
after the same checks and refuses host operations with K5003; `krit invoke`,
`krit serve`, `krit worker`, `krit schedule`, and `krit sandbox` never build
or fall back to interpretation. Warnings (K2003, K9013) do not change the
exit status, but generated code should have none.

For a failed check, send the model:

1. the same versioned system prompt
2. the generated source
3. JSON diagnostics from `krit check --diagnostic-format json`
4. the instruction: "Make the smallest edit that fixes these diagnostics.
   Do not add unsupported language features or new authority."

Habit diagnostics (K1101–K1112, K2001 did-you-mean, K3011) carry a one-line
fix in `notes`, and the prompt's habit table names the same fix, so a repair
round usually converges in one step.

AI output at runtime is nondeterministic, untrusted raw UTF-8. Generated
source must handle the `Result`, validate the text before acting on it, and
never execute it. Prompts, responses, credentials, and sensitive request
values must not be logged by default. `invoke` response JSON stays on stdout;
structured log JSON Lines use stderr.

State-enabled source additionally uses:

```sh
mkdir -m 700 state
krit build --manifest examples/stateful-webhook.krit.pkg
krit invoke \
  --manifest examples/stateful-webhook.krit.pkg \
  --host-config examples/stateful-webhook.host.json \
  --request examples/webhook-agent.request.json
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
edits for the selected package-entry document, following the
[assist instruction](../crates/krit-assist/assets/KRIT-AUTHORING-1.md).
`inspect` prints the exact request without a provider call. `suggest` prints
that request before invoking the provider and writes only a proposal JSON
file. `review` recomputes the canonical diff and compiler/permission facts.
`accept --reviewed` writes only after fresh checks and exact approval of newly
required permissions.

```sh
krit assist inspect --provider-config assist-provider.json --manifest krit.pkg --file generated.krit --range all --intent "Repair the current diagnostics."
krit assist suggest --provider-config assist-provider.json --manifest krit.pkg --file generated.krit --range all --intent "Repair the current diagnostics." --proposal target/assist-proposal.json
krit assist review --manifest krit.pkg --proposal target/assist-proposal.json
```

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

Every prompt pack is tied to a compiler and language version. A change must:

1. describe only implemented syntax, built-ins, and host operations
2. include canonical readable examples in ```` ```krit ```` blocks; proposed
   or deliberately invalid syntax goes in ```` ```text ```` blocks
3. keep the system prompt within about 8,000 tokens (bytes / 4)
4. pass `crates/krit-cli/tests/prompt_examples.rs`, which extracts every
   `krit` block from the prompt, this README, the assist instruction, and
   `rfcs/`, then requires `krit fmt --check` and a diagnostic-free
   `krit check` (warnings included, component target for entrypoints) and a
   `krit run` that succeeds or stops only at K5003; it also requires every
   cited diagnostic code to exist in `spec/DIAGNOSTICS.md`, a habit-table row
   for every habit code, and every standard-library signature from
   `spec/LANGUAGE.md`
5. add or update generation evaluation tasks
6. record compatibility in `prompt-pack.json`; `krit assist` advertises its
   `promptVersion` directly, so bumping it there is the only version edit,
   and the prompt's title and file name carry the same language minor
7. update the changelog

Old packs remain available in git history for projects pinned to an older
language version.

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
