# Krit AI generation pack

Krit ships provider-neutral material for generating valid, readable source
with Claude, ChatGPT, Gemini, or a local model. An LLM is an authoring tool,
not part of the compiler or runtime.

## Current pack

- Prompt: [`KRIT-0.2-SYSTEM.md`](../crates/krit-cli/assets/KRIT-0.2-SYSTEM.md)
- Metadata: [`prompt-pack.json`](prompt-pack.json)
- Language authority: [`../spec/LANGUAGE.md`](../spec/LANGUAGE.md)

The prompt contains only implemented Krit 0.2 syntax. Draft agent, HTTP,
WebAssembly, type, and capability designs are intentionally excluded so a
model does not generate code the current compiler cannot accept.

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
krit check generated.krit
```

For a failed check, send the model:

1. the same versioned system prompt
2. the generated source
3. JSON diagnostics from `krit check --diagnostic-format json`
4. the instruction: “Make the smallest edit that fixes these diagnostics.
   Do not add unsupported language features.”

Run and review permissions only after checking:

```sh
krit run generated.krit
krit permissions
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
5. record compatibility in `prompt-pack.json`
6. update the changelog

Old packs remain available for projects pinned to an older language version.

## Privacy

Krit does not choose a provider or upload source automatically. The user or
editor configures a local or remote model and explicitly selects context.
Secrets, capability values, ignored files, and runtime data are excluded from
prompt context.
