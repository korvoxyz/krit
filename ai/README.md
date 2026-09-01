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
contains the implemented contracts-only typed webhook, literal configuration
read, and opaque secret acquisition forms. Socket serving, outbound HTTP/TLS,
host value providers, connectors, and AI calls remain explicitly unavailable
so a model does not invent Phase 4 runtime APIs. The compiler can explain
checked contracts, build the narrower policy-1 scalar/stdout artifact subset,
inspect its effective local permissions, and execute that subset in the
bounded sandbox.

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
krit build
krit permissions --artifact target/krit/<package>.wasm
krit sandbox
```

`krit check` accepts the full bootstrap language. `krit build` currently
accepts only Int, Bool, Unit, non-capturing functions, primitive control flow
and operators, and scalar stdout. A model asked for a buildable artifact must
avoid strings, lists, records, Option/Result, JSON, and lexical captures until
their guest layouts exist. It must also avoid webhook/config/secret contracts:
they intentionally fail build until `phase4-http-runtime`. A K7001 or K7002
build diagnostic must be repaired in source rather than bypassed with host
interpretation.
`krit sandbox` never builds or falls back to the full direct evaluator, so the
artifact must already exist and validate with its adjacent metadata.

For an AI-authored webhook contract, use:

```sh
krit fmt agent.krit
krit fmt --check agent.krit
krit check agent.krit
krit explain --json agent.krit
```

Do not run or build that source yet. `krit run` returns K5003 and `krit build`
returns K7002 rather than simulating configuration, secrets, or HTTP.

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
prompt context.
