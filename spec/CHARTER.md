# Krit language charter

**Status:** Normative  
**Edition:** 2026

## Mission

Krit is an open text programming language for small, sandboxed agent and
integration applications written with AI and trusted by humans.

AI systems should be free to compose useful programs, but generated programs
must remain precise, reviewable, portable, and constrained by authority that a
human or host explicitly grants.

## Principles

### 1. Human-auditable

Valid Krit code has one unambiguous parse and a small set of unsurprising
constructs. Important behavior must be visible in source, package metadata, or
the lockfile. Generated code is not accepted merely because a model explains
what it intended.

### 2. Open text

Krit source is UTF-8 text stored in ordinary files. The language
specification, compiler, package formats, conformance suite, and registry
protocol are openly implementable. A Krit program must not require a specific
editor, model provider, registry, or hosted service.

### 3. Precise, not natural-language executable

Natural language may describe intent and assist generation, but it is not
executable Krit. The compiler accepts deterministic grammar and reports exact
source locations. AI translates intent into Krit; the compiler verifies Krit.

### 4. Pure and deterministic by default

Values are immutable. Evaluation is deterministic unless a declared effect is
used. Time, randomness, files, network access, process execution, environment
variables, secrets, and model calls are effects rather than ambient features.

### 5. Least authority

Code receives only capabilities granted by the application or host. A
dependency can request a capability but cannot grant one to itself or widen a
grant. Package installation never executes package code.

### 6. Explainable tooling

Compiler decisions must be available as structured facts. Diagnostics,
resolved dependencies, inferred types, effects, capabilities, and build plans
must have stable machine-readable forms suitable for both people and agents.

AI predictions are optional authoring suggestions, never semantic authority.
Deterministic formatting, checking, permission analysis, and user-visible
diffs remain the gate between generated text and executable code.

### 7. Fast feedback, measured performance

Startup, checking, incremental compilation, cached execution, and predictable
memory are primary performance goals. Krit publishes reproducible measurements
and does not claim performance from implementation language or architecture
alone.

### 8. Provider neutrality

AI and external services are accessed through capability-scoped interfaces.
Core language semantics do not depend on one provider, model, transport, or
commercial API.

### 9. Small stable core

New syntax must justify its cognitive cost. Prefer libraries, explicit data,
and tooling over aliases and hidden compiler behavior. There should be one
canonical formatter.

### 10. Secure failure

Unknown syntax, types, effects, capabilities, editions, package fields, or
lockfile formats fail closed. Krit does not silently continue with reduced
checks or broader authority.

### 11. Sandboxed deployment

Deployable Krit applications are WebAssembly components with no ambient
authority. External operations cross narrow typed host interfaces. Resource
limits and operating-system isolation provide defense in depth around the
WebAssembly boundary.

## Non-goals

Krit does not initially aim to:

- replace general-purpose systems languages
- execute arbitrary natural-language instructions
- provide unrestricted shell scripting
- reproduce Python's package ecosystem
- optimize scientific numerical workloads
- preserve accidental behavior from the Racket prototype
- guarantee sandboxing through language syntax alone
- build or train a proprietary code-generation model
- silently rewrite accepted source through an LLM

## Decision test

A feature belongs in Krit when it improves AI generation or automation while
remaining easy for a human to inspect and for the compiler to check. When
convenience conflicts with explicit authority or deterministic behavior,
explicit safety wins.
