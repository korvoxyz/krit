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
secret acquisition, and exact-origin `http_request` forms. Raw sockets,
ambient host values, secret revelation, broad network grants, and AI calls
remain explicitly unavailable. The compiler can explain exact requirements,
build scalar or bounded webhook artifacts, inspect effective local
permissions, and execute them through the sandbox, fixture invocation, or
loopback server paths.

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
krit sandbox # module-init artifacts
krit invoke --request request.json --host-config host.json # webhook artifacts
```

`krit check` accepts the full bootstrap language. `krit build` accepts the
scalar policy-1 subset and a bounded webhook subset containing strings, fixed
HTTP records, header lists, Result/Option matching, static non-capturing
helpers, config, secrets, and HTTP. A model must still avoid arbitrary
composites, JSON in components, data captures, and dynamic string operators.
A K7001 or K7002 build diagnostic must be repaired in source rather than
bypassed with host interpretation.
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
config JSON. `krit run` still returns K5003 rather than simulating hosts.
`krit invoke` and `krit serve` never build or fall back to interpretation.

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
