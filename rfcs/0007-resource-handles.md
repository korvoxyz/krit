# RFC-0007: Resource handles

- **Status:** Draft
- **Author:** proposal by Akshay Bhardwaj (language owner); options,
  counter-arguments, and evaluation plan drafted for review
- **Created:** 2026-10-12
- **Decider:** the language owner. This RFC accepts nothing.
- **Edition:** undecided: 2027 (option A), 2026 additive or 2027 (option B),
  none (option C)
- **Earliest release:** not before KritBench v0 and edition 2027 exist; the
  0.3 PRD lists resource handles as an open RFC, not a 0.3 feature
- **Security label:** yes. The proposal changes where authority is written
  and moves secret-to-origin binding into source.
- **Related:** [roadmap](../docs/roadmap.md) §3.3 (loud authority), §3.4
  (admission rule), §3.5 (stdlib naming), §5.2 (editions), §5.5 (`uses [...]`
  effect rows), §6 (beauty program), §8.2 (durable identity), §9.5
  (KritBench); [PRD 0.3](../docs/prd-0.3.md) §3; `spec/CAPABILITIES.md`,
  `spec/PACKAGES.md`, `spec/DURABLE-STATE.md`

Release numbers in parentheses below are the roadmap's. PRD 0.3 §9 moves the
roadmap's deep refactors into 0.4 and shifts its later releases by one.

## Summary

Declare a program's authority once, in a block that binds names to literal
resources, and call host operations on those named handles:

```text
uses {
    work: state("agent-work"),
    api: http("https://api.example.com", bearer: "api-token"),
    reviewer: ai("reviewer"),
}
let previous = work.get("last-body")?;
let fetched = step "fetch-issue" { api.send(outbound_request) }?;
```

The roadmap currently rejects this shape (`capability.operation` respelling,
implicit receivers) and schedules a different `uses [...]` clause for 0.6.
This RFC records the proposal, its benefits, those counter-arguments, three
options, and how to decide between them with evidence.

## Motivation

Today every host call names its resource at the call site. This 0.3 program
asks a reviewer about an issue whose body changed since the last delivery:

```krit
/// Asks the reviewer about an issue whose body changed since the last delivery.
fn review(request: HttpRequest) -> Result<HttpResponse, String> {
    let previous = state_get("agent-work", "last-body")?;
    let fetched = http_request(
        "https://api.example.com",
        HttpRequest { method: "GET", path: "/issues/1" },
        Some(secret("api-token")?),
    )?;
    let unchanged = match previous {
        Some(body) => body == fetched.body,
        None => false,
    };
    if unchanged {
        Ok(HttpResponse { status: 204 })
    } else {
        let verdict = ai_invoke("reviewer", fetched.body)?;
        state_put("agent-work", "last-body", fetched.body)?;
        Ok(HttpResponse { status: 200, body: verdict })
    }
}

webhook fn handle(request: HttpRequest) -> HttpResponse {
    match review(request) {
        Ok(response) => response,
        Err(reason) => HttpResponse { status: 502, body: reason },
    }
}
```

The proposal claims five benefits:

1. **The store literal is written once.** `"agent-work"` appears at every
   state call. A misspelled copy is a different store: `krit check` accepts
   it, and only `krit build` rejects it (`K5001`) when the manifest lacks it.
2. **Argument-swap type safety.** `state_put` is effectively
   `fn(String, String, String)`, and so are `checkpoint_put`, `object_put`,
   and `cache_put` before the TTL. With a handle the store is no longer a
   string argument.
3. **Secrets never become values.** Today `secret("api-token")` yields an
   opaque `Secret` that code must route to the right `http_request`. The
   manifest binds it to origins and the host refuses others (`K5006`), but
   only at invocation time. A handle declares the pair once
   (`http(origin, bearer: secret)`), so no `Secret` value exists in source and
   the compiler sees every pair.
4. **A one-page authority block.** A reviewer reads the block at the top of
   the file instead of collecting literals from every call.
5. **Step journaling keyed per instance.** `step "fetch-issue" { ... }`
   records a completed effect under the step name and, from durable protocol
   2, the host-derived instance key, so a redelivery replays it instead of
   repeating it. Today `replay_http` is anonymous (it cannot carry a bearer)
   and keeps one record per operation name, so a second input is a replay
   conflict (`K5203`).

### What the 0.3 compiler already catches

Measured with the 0.3 compiler, to size claim 2 honestly:

- A store/key swap with a computed key, `state_put(key, "agent-work", body)`,
  is already `K3008`: the store position accepts only a direct literal.
- A store/key swap of two literals, `state_put("last-body", "agent-work",
  body)`, checks, but `krit explain` shows `store=last-body` and `krit build`
  rejects the ungranted store (`K5001`).
- A key/value swap, `state_put("agent-work", body, key)`, checks and builds.
  Handles do not catch it either: `work.put(body, key)` still passes two
  strings. Only labelled arguments or distinct key types would.

So claim 2 holds for the store position, which literals and manifests already
guard, and not for the key/value positions, where the real swap risk lives.

## Syntax and semantics

This section sketches option A so the options can be compared. Nothing here is
normative.

```text
uses-block  = "uses", "{", handle, (",", handle)*, ","?, "}" ;   (* at most one, top level *)
handle      = identifier, ":", resource ;
resource    = kind, "(", string-literal, (",", label, ":", string-literal)*, ")" ;
kind        = "config" | "state" | "http" | "ai" | "queue" | "bucket"
            | "readonly_bucket" | "database" | "readonly_database" | "cache"
            | "readonly_cache" | "search" | "vector" ;
label       = "bearer" ;
call       += identifier, ".", identifier, "(", arguments?, ")" ;  (* identifier is a handle *)
step        = "step", string-literal, block ;
```

- **Meaning by desugaring.** Each handle is a compile-time name for its
  literal resource. `work.get(key)` desugars to `state_get("agent-work", key)`,
  `api.send(request)` to `http_request("https://api.example.com", request,
  Some(<bound secret>))`, `reviewer.invoke(input)` to
  `ai_invoke("reviewer", input)`, and so on for every host operation. Types,
  effects, literal requirements, and Core are exactly those of the desugared
  call, so the authority analysis and the import re-proof are unchanged.
- **Handles are second class.** A handle cannot be bound with `let`, passed,
  returned, stored, compared, printed, or captured; it is valid only as the
  receiver of one of its kind's operations. Passing handles to helpers would
  make them capability values, which the roadmap defers past 1.0 (§5.4, §8).
- **Secrets.** `bearer:` names a manifest secret. The pair must also appear in
  the manifest's `[secrets.<name>] origins`, which stays the reviewable grant;
  a pair the manifest does not bind fails the build. `secret(...)` and the
  `Secret` type leave source.
- **Steps.** `step "name" { expr }` evaluates `expr`, which must contain
  exactly one replay-safe host call, at most once per (durable namespace,
  instance key, step name), and otherwise returns the recorded result. Which
  store journals the step, and what happens without an instance key (an
  unverified webhook has none before protocol 2), are open questions.
- **Diagnostics.** New codes would cover an unknown handle kind or operation,
  a handle used as a value, a duplicate handle, and a step that is not
  replay-safe. None is allocated; roadmap §9.2 owns allocation.

Option B keeps only the block: handles replace the resource literal in the
existing flat calls, and there are no method calls.

```text
let previous = state_get(work, "last-body")?;
let fetched = http_request(api, HttpRequest { method: "GET", path: "/issues/1" })?;
let verdict = ai_invoke(reviewer, fetched.body)?;
```

Because `api` carries its bearer, `http_request` loses its third argument for
bound handles, which is itself a second shape of that call.

## Counter-arguments recorded in the roadmap

The master plan considered and rejected the method-call half of this shape:

- **A second spelling for every host operation.** Krit has about thirty host
  operations. Handle methods duplicate each one unless the flat form is
  removed, and "each construct has one spelling, enforced by the formatter" is
  a beauty-program rule (§6). The beauty table lists `capability.operation`
  under Reject: "each hides control flow or authority, adds a second spelling,
  or collides with an existing meaning".
- **The literal moves off the call site.** "Loud authority" (§3.3) means a
  reviewer sees the resource where the effect happens. `work.put(k, v)` sends
  the reader to the block to learn which store is written; diffs that change a
  handle's resource touch one line far from the calls they affect.
- **Implicit receivers.** §3.3 forbids "implicit receivers" and "host-call
  respelling", and §3.5 fixes stdlib naming as flat and prefixed with "no
  `ns.fn` and no `capability.operation` respelling". Today `x.f()` reads field
  `f` of record `x` and calls it, and `K3011` teaches models that "Krit values
  have no methods". Handle methods would make `s.len()` look half-valid and
  blur that hint.
- **A different `uses` is already scheduled.** §5.5 resolves the effects
  question with a trailing `uses [...]` clause in 0.6, an exact, checked
  effect row such as `uses [cache.write("lookups"), search.query("docs")]`,
  required on library `pub fn`. A `uses { ... }` declaration block would give
  the keyword two meanings.
- **Edition budget.** §5.2 says edition 2027 "holds exactly three breaking
  groups". Option A would add a fourth.

## Comparison with the planned alternatives

| Claimed benefit | Handles | Status quo and roadmap plan |
|---|---|---|
| Store literal written once | yes | typo fails `krit build` (`K5001`) and shows in `krit explain`; a named constant is `K3008` by design |
| Argument-swap safety | store position only | store position already guarded (`K3008`, `K5001`); key/value needs labelled host arguments (roadmap experiment, 0.6–0.7) or `type` wrappers (0.4) |
| Secrets never values | yes; pairs proven at compile time | opaque `Secret` (`K3009`), host-enforced binding (`K5006`), compiler-proven pairs from literal call sites (planned) |
| One-page authority block | yes, in source | manifest, `krit permissions`, and the generated authority card (0.4.1) |
| Step journaling per instance | `step` | `replay_http`/`replay_ai` per operation name now; instance keys in protocol 2 (0.5); `workflow fn` after 1.0 |

Measured on the program above, formatted in `krit fmt` style, counting the
literals `"agent-work"`, the origin, `"api-token"`, and `"reviewer"`:

| Form | Lines of `review` (with the block) | Authority literals in function bodies | Authority literals in total |
|---|---|---|---|
| 0.3 today | 20 | 5 | 5 |
| Option A | 23 | 0 | 4 |
| Option B | 22 | 0 | 4 |

Option A keeps the proposal's separate `outbound_request` binding and adds
journaling the other forms lack. Handles do not shorten small programs; they
relocate authority. The saving grows with the number of call sites per
resource, which the evaluation must measure on real programs rather than this
sketch.

## Options

### A. Adopt as the edition-2027 replacement

`uses { ... }` plus handle operations replace the flat host calls in edition
2027; edition 2026 keeps the flat calls, and both lower to the same Core.
`krit fix --edition 2027` migrates mechanically: one handle per distinct
resource literal, named from the literal, and one method call per host call.

- Gains every claimed benefit, including compile-time secret pairs and steps.
- Requires amending §3.3, §3.5, §5.2, and the beauty table; renaming either
  the block or the 0.6 effect row; and a migration that keeps the permission
  report byte-identical, since machine fixes never change authority (§3.3).
- Touches the parser, resolver, registry, formatter, LSP, prompt pack, every
  example, and conformance: more than ten files, so it lands after the 0.4
  NodeId and registry refactors.

### B. Adopt only the declaration block

Handles name resources once, and the flat calls take a handle where they take
a literal today (`state_get(work, key)`). There are no method calls, so no
second spelling of each operation and no receivers.

- Gains the single literal, the one-page block, typed store positions, and
  compile-time secret pairs.
- Keeps the K3011 teaching ("values have no methods") intact.
- Still moves the literal off the call site, still needs a name other than
  `uses` or a merge with the 0.6 effect row, and either accepts two spellings
  of the resource argument in edition 2026 or replaces literals in 2027.
- Steps would need their own RFC.

### C. Reject and keep literal resources at call sites

Keep the current form and close the gaps with work already planned:
compiler-proven secret pairs, the authority card as the one-page view,
labelled host arguments as the swap-safety experiment, protocol 2 instance
keys, and `workflow fn` for per-instance steps.

- No new syntax, no second spelling, loud authority preserved.
- Repetition of resource literals remains.
- A rejection records a reopen trigger, for example: authority-literal typos
  or resource/argument swaps rank among the top five KritBench failure
  categories at 0.4.1, or perm-exact stalls below 90%.

## Evaluation plan

The decision should rest on measurements, collected once KritBench v0 and
edition 2027 exist:

1. **Prototype.** Implement options A and B as desugarings behind
   `--unstable=resource-handles` (roadmap §3.4 experiments: recorded in
   artifact metadata, refused by `serve`, expiring after two releases). Both
   lower to today's Core, so artifacts, imports, and authority cards must be
   byte-identical to the flat form; CI proves it on every example.
2. **Metrics.** `krit metrics` on `examples/`, the apps, and the grounded
   sketches: lines, tokens, authority literals, call sites per resource, and
   the distance in lines between an effect and its resource.
3. **KritBench subset.** About 30 T3 (integration agent) and T5 (authority
   exactness) tasks that use at least two resources of one kind, a bearer
   secret, or a keyed write, plus seeded swap and wrong-resource traps checked
   by hidden tests. Run conditions A, B, and C with the same pinned gate
   models and product models, greedy plus n=5 sampling, and the prompt pack
   rewritten per condition. Score check@1, build@1, func@1, perm-exact (over-
   and under-request separately), repair@3, swap and wrong-resource defects,
   and tokens per solved task. Accept a form only with no statistically
   significant regression (paired bootstrap, α = 0.05) against C.
4. **Reviewer study.** At least eight reviewers, within-subject, randomized
   order: each reviews programs in all three forms with seeded authority
   defects (wrong store, secret sent to the wrong origin, extra grant,
   swapped key and value, model output reaching a path) and writes the
   program's authority summary. Measure time to decision, defect detection
   rate, and summary accuracy.
5. **Habits.** For each adopted form, the three likely model mistakes (for
   example invented methods such as `work.list()`, handles passed to helpers,
   interpolated resources in the block) each get a coded diagnostic and fix.

## Security, capability, and determinism

- Resources stay literal (in the block), so the compiler still derives every
  requirement and re-proves imports; manifests remain the grant, and source
  cannot approve itself.
- Option A and B move secret-to-origin pairs into source, where the compiler
  can prove them before deployment; the manifest binding must agree.
- Locality of authority decreases: a call no longer shows its resource. The
  authority card and `krit explain` must keep listing each effect with its
  resource and call site.
- `step` changes durable identity and needs RFC-0005's protocols: identity,
  conflicts returned as `Err`, and instance keys. Without an instance key a
  step is a singleton, and the card must say so.
- Determinism is unchanged: handles desugar to the same calls in the same
  order.

## Conformance

An adopted option needs check cases for every new diagnostic, run and build
cases proving the desugared and flat forms produce identical output, imports,
and permission reports, format cases for the block and every method call, and
package cases for secret pairs that disagree with the manifest. `step` needs
durable cases for redelivery, conflict, and the missing instance key.

## Compatibility and migration

- A: edition 2027 only, migrated by `krit fix --edition 2027`; edition 2026
  programs keep compiling unchanged.
- B: additive in 2026 if handles and literals may both fill the resource
  position (two spellings), or a 2027 replacement migrated by `krit fix`.
- C: nothing changes.
- Every migration must leave the permission report unchanged, or `krit fix`
  aborts (§3.3).

## Performance

Handles are compile-time names that desugar before analysis, so artifacts and
runtime cost are unchanged. Compile-time cost is one resolution pass over the
block.

## Unresolved questions

- The block's keyword: `uses`, given the 0.6 effect row, or another name.
- Whether config keys, log event names, checkpoints, and database statements
  become handle operations or stay literal call arguments.
- Which store journals a `step`, and whether steps belong in this RFC or with
  `workflow fn`.
- Whether option B's handle may also replace a literal in edition 2026.

## Decision

Left to the language owner. No option is accepted, and the roadmap's
rejection of `capability.operation` stands until the owner decides.
