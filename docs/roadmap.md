# Krit 0.2 → 1.0: Master Plan (revision 2)

> **Release numbering update (2026-10-11).** At the owner's request the work
> this plan schedules as 0.2.1 "Stop the bleeding", 0.2.2 "Honest check", and
> the flat-code and stdlib-v0 parts of 0.3 ships together as **0.3.0 "Honest
> and Flat"** ([prd-0.3.md](prd-0.3.md), [tracker-0.3.md](tracker-0.3.md)).
> The remaining 0.3 scope (NodeIds, DiagnosticSink, registry, Core
> interpreter, durable protocol 1.1, `krit-action`, KritBench) becomes **0.4
> "One semantics"**, and every later version below shifts by one minor
> version. Effort and dates in this document are the original estimates for
> a solo maintainer and are re-derived at each release.


*2026-10-11, against krit 0.2.0 (`e1bff4f`). This revision addresses every blocker and major issue raised by the feasibility, safety and completeness critics. It also takes most of the minor ones and keeps the strengths they listed. Where the plan keeps a position a critic challenged, the relevant section says so in a line marked **Held position**. The decision log at the end records every significant choice and the alternatives considered.*

*A "pw" is one person-week of a maintainer working with AI coding agents; review and validation are the bottleneck, not typing. All dates assume **0.8 effective pw per working week, 42 working weeks per year and about 20% maintenance**, which gives 33.6 pw per person-year. The staff count is 1 until a second maintainer is actually hired. Dates are re-derived from measured throughput at the 0.2.2 exit (§12).*

## Executive summary

- **What "new generation" means for Krit.** Every program ships with a one-page contract that a person approves without reading every line. The contract says:
  - what the program reaches and which secret may go to which origin;
  - what it does;
  - what happens on retry;
  - where untrusted text flows.

  Each line is tagged by how it is guaranteed: enforced at run time, re-derived from the artifact at load, or attested by the compiler. Krit already has the rare half: literal-resource capabilities re-proven against the Wasm imports, opaque secrets and exact-origin egress. What it lacks is one semantics.
- **The defining defect.** `run`, `check` and `build` accept three different languages. 7 of 8 realistic programs pass `check` and then fail `build` with K7002, and `if request.method == "POST"` runs nowhere. Fixing this is job one, because every differentiator depends on it.
- **Two safety holes found in review are fixed first.**
  1. **Secret misdirection.** Any granted secret can be sent as the bearer token to any granted origin. A swapped-token probe builds, and its permission report is identical to the original's. From 0.2.1 the host binds each secret to its origins (K5006). From 0.3 the compiler proves the (origin, secret) pairs.
  2. **Upgrades break deployments.** Every release rejects the previous release's artifacts (`validation.rs:503,543`) and orphans their durable records (`policy.rs:1055-1061`). 0.2.1 adds an artifact-compatibility window. 0.3 moves durable identity onto a declared namespace and adds `krit state migrate`. Every later release has an upgrade gate: 0 orphaned records and 0 duplicate effects.
- **Beachhead: self-hosted AI repo bots.**
  - 0.4: deploy inside GitHub Actions via a hardened `krit-action`.
  - 0.5: self-hosted, with host-verified webhooks and downscoped App tokens.
  - Then: chat bots, durable glue and MCP tools.
  - After 1.0: an embedded guest language.
  - Before 0.4, 0.2.2 already supports narrow real uses (§2.0): outbox workers, idempotent ingestion and RAG pass-through.
- **Releases:**
  - **0.2.1 (5 pw), stop the bleeding:** Wasmtime 48 LTS, depth guards instead of aborts, `run` gated on analysis, fail-closed `[dependencies]`, the artifact-compatibility window, secret→origin binding, SECURITY.md.
  - **0.2.2 (7 pw), honest check:** `check --target` matches build exactly and points every K7002 at the right expression; the guest string runtime (so `==` and `+` deploy, and R7 runs); path-injection hardening; fuzzing; a static KritBench v0 baseline.
  - **0.3 (33 pw), one semantics, flat code:** NodeIds; a desugaring pass for the flat sugar, DiagnosticSink and registry, in parallel; a Core interpreter with FakeHost; `evaluator.rs` deleted; durable protocol 1.1; the first three stdlib functions with path lints; the runtime-strategy RFC; `krit-action` v0.
  - **0.4 (35 pw), R1 deploys:** one LIR backend (layouts, monomorphization, closures); `type` and `enum` (string-valued variants included); `json_parse` and `json_stringify` under an I-JSON profile; about 35 stdlib functions; Actions deploy with host attenuation. After 0.4, a K7002 on code that passes `check` is a compiler bug.
  - **0.4.1 (15 pw), authoring loop:** `card/1` with enforcement tags; grammar, holes, `krit mcp`, repair loop and editors; the rest of stdlib v1; KritBench v1 frozen.
  - **0.5:** runs on your box, with concurrent but single-writer durable stores.
  - **0.6:** programs grow up.
  - **0.7:** trusted agents (provenance with an EXPOSES section for private data, `card --against`, tools).
  - **1.0:** edition 2027 frozen after an audit.
- **Beauty means reviewability, and it is measured.** The 0.3 sugar takes the reference webhook from 79 lines at depth 15 to 47 lines at depth 3. Every example reaches depth 3 or less. `krit metrics`, KritBench and a reviewer study decide what ships, but benchmark deltas can never veto a safety rule.
- **AI loop:**
  - machine fixes that may never change authority;
  - habit codes for the constructs models import from other languages;
  - typed holes and a GBNF grammar;
  - a registry-generated language card of at most 8k tokens;
  - a hardened `krit mcp` and a bounded repair loop;
  - KritBench, whose release gates are statistical non-regression on pinned open-weight models.
- **Effort and dates (honest arithmetic).** The plan totals about 198 pw, of which about 45 pw is explicitly cuttable.
  - **Solo:** R1 deploys in Feb 2029 and 1.0 lands in Aug 2032.
  - **With a funded second maintainer from Mar 2027:** R1 deploys in Apr 2028 and 1.0 lands in H1 2030.
  - Funding that second maintainer is the plan's top non-technical priority (§12). For scale, Gleam took about 8 years from start to 1.0.
  - The kill/pivot checkpoint moves to the 0.4 exit. Leading indicators are checked at 0.2.2 and 0.3.
- **Constraints honored:**
  - no `krit-lsp`→`krit-wasm` dependency (test-enforced, `krit-lsp/src/lib.rs:766-781`);
  - no on-disk `Component::deserialize`, because `unsafe` is forbidden;
  - no Wasm GC or guest async before 1.0;
  - no host-builtin respelling;
  - new names (`json_parse`, `json_stringify`) instead of changing what existing names mean;
  - no CLI flag that enables private egress.

## Roadmap at a glance

| Ver | Theme | Headline scope | Gate (abridged) | pw (cuttable) | Solo | +1 FTE from Mar 2027 (if funded) |
|---|---|---|---|---|---|---|
| 0.2.1 | Stop the bleeding | LTS runtime, deny/audit, 3-OS tests, K1005/K4010, `run` gated, K6003, artifact-compat window, secret→origin binding (host), SECURITY.md | 0.2.0 artifacts load; swapped token refused; green on 3 OSes | 5 (0) | late Nov 2026 | late Nov 2026 |
| 0.2.2 | Honest check | exact `check --target` with spans, guest strings, JSON unescape, path hardening, fuzzing, static KritBench v0 | check = build on every file; R7 runs; baseline with 95% CIs | 7 (1) | early Feb 2027 | early Feb 2027 |
| 0.3 | One semantics, flat code | NodeIds; desugar ∥ sink ∥ registry; Core interpreter + FakeHost; evaluator deleted; protocol 1.1; stdlib v0 + K9013; runtime RFC; `krit-action` v0 | interpreter = spec on every run case; 0.2.2→0.3 state migration loses 0 records; webhook-agent ≤50 lines/depth ≤3 | 33 (6.75) | Jan 2028 | Sep 2027 |
| 0.4 | R1 deploys | LIR/layouts/mono/closures, policy 3, `type`/`enum`, `json_parse`/`json_stringify`, ~35 stdlib fns, Actions deploy + attenuation, one AI adapter | 0 K7002 on verified Core; R1 (Actions), R2, R3, R6, R7; 0.3→0.4 upgrade 0 orphans | 35 (3) | Feb 2029 | Apr 2028 |
| 0.4.1 | Authoring loop | tagged `card/1`, grammar, holes, `krit mcp`, repair loop, editors, rest of stdlib, KritBench v1 | forged card refused; v1 frozen and re-baselined | 15 (10.5) | Jul 2029 | Jul 2028 |
| 0.5 | Runs on your box | HostService, cache/pool, single-writer stores, `serve`, verify, downscoped App tokens, protocol 2, AI budgets, inject kinds, Operate | ≤8 files/capability; 0 duplicate effects with 8 workers; R4, R5, R10 | 25 (7) | Apr 2030 | Dec 2028 |
| 0.6 | Programs grow up | HM, generics, effect rows, modules, lockfile, `Map`, `uses`, LSP v2, edition 2027 default | multi-module R5/R9; LSP p95 ≤50 ms | 29 (4.5) | Feb 2031 | Jun 2029 |
| 0.7 | Trusted agents | 0-CFA provenance, EXPOSES, `tool fn`/MCP, `card --against`, retry-safety, Adapter v2 (sim, eval) | injection + exfiltration suite 0 escapes; R8 | 31 (9.5) | Jan 2032 | Dec 2029 |
| 1.0 | Stable contract | edition 2027 frozen, normative `CORE.md`, audit, KritBench v2, reviewer study | audit criticals closed; no significant bench regression | 18 + audit (3) | Aug 2032 | H1 2030 |

Hiring the second maintainer at the 0.4 exit instead gives 1.0 around Q2 2031. **Held position:** the feasibility critic suggested a "1→2 FTE at 0.5" column. Hiring at 0.5 is too late to move the R1 date, which is the binding product risk, so the conditional column shows a Mar 2027 hire. Releases ship in order. With two people, the backend track (LIR design, runtime RFC) overlaps late 0.3.

---

## 1. Thesis

For Krit, "new generation" is not new syntax or an "AI-native" label; the research flags MoonBit-style branding as a hype risk. It is four machine-checked properties printed on one page that a reviewer approves:

1. **Authority.** Every effect names a literal resource, and every secret names the origins it may be sent to. The compiler proves both. The build re-proves reach against the component's imports, and the host enforces reach and secret binding at run time.
2. **Flow.** Model output, request bodies and tool results are untrusted by construction and cannot reach a privileged sink unseen. Data read with a credential cannot reach a write body unseen (EXPOSES, 0.7).
3. **Durability.** What happens on crash, retry, redelivery and upgrade is a static fact. No surveyed durable platform, from Temporal to Golem, enforces determinism with types.
4. **Learnability.** Models with no Krit training data write and repair Krit from docs and diagnostics alone. This is measured publicly, and benchmark deltas never outrank safety.

Krit already has most of property 1 and the substrate for 2 and 3. That substrate is:

- immutable values and inferred effects;
- an ANF Core with typed host operations;
- opaque secrets and exact-origin egress;
- atomic commit of state, queue and acknowledgement.

What Krit lacks is one semantics, and a card is worthless if it does not describe the program that ships. So the next phase goes to foundations:

- typed Core as the single executable truth;
- a backend that accepts the whole checked language;
- sound JSON;
- a data-driven registry;
- a pure stdlib;
- repairable diagnostics;
- an upgrade path that keeps deployed bots and their durable records alive.

These foundations make the moat cheap:

- Typed prompts need records and fallible decoding in Wasm.
- Provenance is a closure-flow analysis over monomorphized Core, which cannot be trusted next to a second evaluator.
- A capability family touches about 20 files across 7 crates today, and should touch at most 8 after the registry and HostService.

**Honest limits, stated in docs and marketing until they are closed:**

- Krit does not prevent exfiltration through endpoints the grant already allows; EXPOSES arrives in 0.7.
- In Actions mode Krit isolates the token from guest code, not from other workflow steps.
- Before 1.0 nothing is production-certified.

**Positioning:** *Krit is the small language in which AI writes the bots, tools and integrations that touch your systems. For each program, the compiler hands a person a one-page contract: what it can reach, which secret goes where, what it does and what happens on retry. Each line is marked by how it is enforced.*

## 2. Real-world use cases

### 2.0 Usable today and on 0.2.2

These uses are grounded in sketches built and run against the 0.2 binary:

| Use | Works on | Caveats |
|---|---|---|
| Reliable outbound or outbox worker (sketch 06 ran end to end: 2 s and 4 s backoff, then a dead letter) | 0.2 | no int-to-string for logging until 0.3; dead letters inspectable only via SQLite until 0.5 |
| Idempotent webhook-to-SQLite ingestion (03b) | 0.2 | no inbound authentication; parsing lives in host SQL |
| Queue ingress buffer | 0.2 | single host |
| Cache-fronted search proxy | 0.2 | no score filtering until 0.4.1 |
| AI pass-through or RAG endpoint (07) | 0.2.2 | prompt composition needs string `+` (0.2.2); typed decode waits for 0.4 |

### 2.1 Beachhead: self-hosted AI repo bots

The bots do issue and PR triage, PR summaries, release notes and digests to chat. The buyers are security-conscious platform and DevEx teams (50–5,000 engineers) and OSS maintainers.

**Message:** *"AI-written repo bots whose tokens can't be printed, can only go to the origins you bound them to, and whose egress you can read in the PR."*

Why this beachhead:
- The reference flow is already GitHub → AI → messaging.
- Recent incidents map onto existing guarantees:
  - tj-actions (CVE-2025-30066) printed secrets into logs, which unprintable opaque secrets rule out.
  - Shai-Hulud harvested CI tokens, which never enter Krit guest memory.
  - Injected text cannot redirect literal egress origins.
- The buyers read PRs, and one host suffices.
- GHCR images and native releases already exist.
- Krit complements gh-aw: it can be the deterministic executor for gh-aw's safe outputs.

**Deployment path:**

| Release | What partners can do |
|---|---|
| 0.2.2 | Run usable-today workloads (§2.0) as pilots. |
| 0.3 | Run `krit-action` v0 (check, build, fixture tests, escaped permission diff) in an unprivileged PR job. |
| 0.4 | Deploy bots inside GitHub Actions with host-enforced attenuation and a bound, scope-checked `GITHUB_TOKEN`. |
| 0.5 | Run the same artifact self-hosted under `krit serve`, with host-verified webhooks and a downscoped App installation token. |

**Actions-mode contract (0.4).** The action maps workflow inputs onto Krit inputs as follows:

| Actions input | Krit side |
|---|---|
| `GITHUB_EVENT_NAME` | header `x-github-event` |
| file at `GITHUB_EVENT_PATH` | `body`; oversize input is refused with K5105 |
| `GITHUB_RUN_ID` | header `x-krit-delivery: actions-<id>`; the protocol-2 instance key from 0.5 |
| method and path | `POST /github/<event>` |
| `on: schedule` | the `schedule` entrypoint; `scheduledAtMillis` is the event time, and the fire id is the run id |
| `GITHUB_TOKEN` | opaque secret `github-token`, bound to `https://api.github.com` |
| workflow or job `permissions:` | required, read from `GITHUB_WORKFLOW_REF`, and printed on the card; the action refuses to run if it is missing |
| response | a non-2xx status fails the step; the body is logged with redaction |

The profile allows webhook and schedule entrypoints only. State, queue, checkpoint, object and replay effects fail `check --target actions` with K7007, because the runner has no persistent store. `actions/cache` persistence is rejected: its loss semantics would make the replay and idempotency claims false. The host sends a default `User-Agent: krit/<ver> (<package>)` from 0.2.1, which GitHub's REST API requires; guests may override it. Docs require running Krit in its own job with no other steps.

| Blocker (sketch 01) | Fixed |
|---|---|
| string `==` and `+` rejected in Wasm; escaped JSON strings trap | 0.2.2 |
| 15-level pyramid, 6 identical `Err` arms | 0.3 |
| secrets sendable to any granted origin | 0.2.1 (host) / 0.3 (compiler) |
| unchecked `json_decode`; invalid JSON traps | 0.4 |
| no records, lists or list matching in Wasm; no string library; no enums | 0.4 |
| no `X-Hub-Signature-256` check; App tokens; one webhook per module | 0.5 |

**Beachhead exit:**
- about 10 partner teams, each running at least 3 production bots;
- the card used in PR review;
- zero secret exposures and zero private-data exposures;
- fixture tests in partner CI.

### 2.2 Reference applications

The apps live in `apps/<name>/` and are gated by the CI jobs `apps-gate` and `check-build-agreement`. A milestone whose apps fail has not shipped. Tests that need egress run through a `krit-eval` Rust harness using `NetworkPolicy::loopback_for_tests()`. No CLI flag ever enables private egress.

| ID | App | Proves | Deploys |
|---|---|---|---|
| R3 | flaky-api-worker (sketch 06) | regression guard and upgrade-gate subject | 0.2.1 |
| R7 | rag-answerer (sketch 07; blocked only by string `+`) | guest strings | 0.2.2 |
| R1 | github-triage: issue → AI verdict decoded into a closed enum → label | beachhead core | 0.4 Actions / 0.5 self-hosted |
| R2 | pr-digest: schedule → search → AI → chat, with a date window | records, lists, stdlib, `time_` subset | 0.4 |
| R6 | ticket-router (sketch 05): structured AI output with an in-process fallback | fallible decode, enums | 0.4 |
| R4 | slack-oncall: verified slash command → ack → queue → AI. Replaces sketch 02, whose dynamic origins are excluded by design | chat expansion | 0.5 |
| R5 | payment-ingest (sketch 03): stripe-v1, idempotent insert | durable glue, kill -9 drill | 0.5 |
| R10 | pr-summary: one comment per PR, replayed per delivery | protocol 2 | 0.5 |
| R9 | daily-report (sketch 04): group-by, time | Map, generics, modules | 0.6 |
| R8 | issue-tools: MCP `tool fn`s with an attenuated grant | tools, `card --against` | 0.7 |

### 2.3 Expansion and readiness

1. **Repo bots** (0.4–0.5).
2. **Internal chat bots** (0.5):
   - slack-v0 verification, with the timestamp window checked by the host;
   - `form_decode`, `route`, ack-then-enqueue;
   - the Discord bot-token header (0.5) and Discord Ed25519 verification (0.7).
3. **Durable integration glue** (0.5–0.7): stripe-v1, protocol 2, header and Basic auth, retry-safety.
4. **Safe MCP tools and code-mode scripts** (0.7): `tool fn`, `krit serve --mcp`, `card --against`.
5. **Embedded guest language** for skills, automation code steps and tenant rules (beyond 1.0; a pilot only if a platform partner pulls): embedding SDK, host-defined worlds, Decimal.

| Segment | Earliest release | Remaining blockers at that release |
|---|---|---|
| Outbox, ingestion and cache workers | 0.2 | no inbound auth, single host |
| RAG and AI pass-through endpoints | 0.2.2 | untyped decode until 0.4 |
| Repo bots (Actions) | 0.4 | no durable state in Actions; EXPOSES only at 0.7 |
| Repo bots (self-hosted) | 0.5 | per-key concurrency only if pulled |
| Internal chat bots | 0.5 (Slack), 0.7 (Discord verify) | Teams AAD token exchange after 1.0 |
| Durable glue (Stripe, CRM) | 0.5 | Decimal after 1.0 (Int cents work) |
| Compliance collectors | 0.5 | AWS SigV4 after 1.0; pagination needs 0.4 lists and strings |
| MCP tools | 0.7 | filesystem tools excluded by design |
| Skill and plugin marketplaces | after 1.0 | registry, signing, file grants |
| Tenant logic and embedded steps | after 1.0 | embedding SDK, Decimal, dates |
| Governed or regulated agents | 1.0 + audit | identity integration, SIEM, HA |

### 2.4 Anti-use-cases

- general-purpose backends
- ETL, analytics and numeric work
- policy engines (Cedar or OPA should authorize Krit, not be replaced by it)
- shell, CI build steps, IaC and file automation
- IoT and real-time systems
- selling sandbox compute
- autonomous agent loops (the host owns the loop)
- regulated production before 1.0 plus an audit
- natural-language-executable workflows

### 2.5 Design partners, leading indicators and the kill/pivot checkpoint

**Mix:** 4 OSS projects (more than 500 issues a year), 4 platform/DevEx teams and 2 security teams running bots with broad tokens. Each partner has a bot to replace and a named reviewer. Partners attend a biweekly group office hour plus an async channel; per-partner calls do not fit the 20% maintenance budget. Transcripts are shared only by opt-in. They pass through the exact-secret redactor under a written consent and licence policy, and never enter the public split.

**Targets** (product metrics, not release gates):

| Release | Partners |
|---|---|
| 0.2.2 | 3 pilots on usable-today workloads |
| 0.3 | 5 signed letters of intent |
| 0.4 | 10 |
| 0.5 | 12 |
| 0.7 | 15, plus 2 platform partners |
| 1.0 | 25 orgs |

**No telemetry.** Progress comes from partner reports, Marketplace and GHCR counts, and PRs carrying a card diff.

**Leading indicators:**
- at 0.2.2, at least 3 partners run a §2.0 workload;
- at 0.3, at least 3 partners each name a bot and a reviewer and review R1's projected card.

**Checkpoint at the 0.4 exit.** If fewer than 5 partners have run a bot in production in Actions for at least 4 weeks, re-interview them and pivot to chat bots or MCP tools. The compiler roadmap through 0.4 serves every segment and does not change.

## 3. Principles and decision rules

### 3.1 Charter items kept

Charter items 1–6, 8, 10 and 11 stay unchanged. "Explicit safety wins" remains the tie-breaker. Literal resources stay the backbone, and interpolated strings never count as literals.

### 3.2 Charter items revised

- **Charter 9 becomes "small core, complete foundations".** A pure stdlib and a backend that compiles every checked program are part of the core contract. Local sugar that shrinks what reviewers read is not core growth. (Krit has 36 builtins and none is a pure string or list op; read literally, item 9 would block `?`.)
- **Charter 7 adds "measured authoring".** KritBench and reviewer-study results ship alongside the performance baselines; GUIDED-AUTHORING.md:378-392 lists metrics that nobody measures. Charter 5, 10 and 11 outrank measured authoring (§3.4).
- **Typed Core is the semantic authority.** The "transitional" evaluator is deleted in 0.3 with no `--dynamic` escape hatch. A second semantics is the defect itself.
- **One Core IR plus a NodeId arena.** Span-keyed facts with uniqueness asserts (analysis.rs:65-78, 925-950) leave desugaring nowhere to live.
- **Artifact policy 3 admits bulk memory and tail calls only (0.4).** validation.rs:55 is `WasmFeatures::empty()`, and recursion is the only loop.
- **Connectors split in two.** Auth and verification are host-native, because secrets never enter the guest. API connectors are Krit packages.
- **Entrypoints, durable identity and assist:**
  - `route` arrives in 0.5 and `tool` in 0.7.
  - Durable identity becomes namespace-based in 0.3 (protocol 1.1) and per-instance in 0.5 (protocol 2).
  - Assist gets a bounded, compiler-gated repair loop, and a human still accepts (K8106).
- **The rule "a phase begins when a representative task requires it" stays.** The representative tasks are the apps, KritBench and partner bots.

### 3.3 Standing rules

- **Totality.** A feature ships only when the interpreter, Wasm, formatter, LSP, `explain`, prompt pack and dual-executor conformance all support it in the same release.
  - After 0.4, a K7002 on checked code is a compiler bug. Only resource limits (K5101–K5106), unfilled holes (K7005), the instantiation budget (K7006) and deployment-profile restrictions (K7007, which `check --target` reports) may fail a checked build.
  - From 0.2.2, `check --target` reports every K7002 at its true span.
- **Unprivileged interpreter.** `run` and `test` use FakeHost (fixtures and cassettes) and never hold real authority. Record and live modes run only through the real runtime with manifest ∩ host config ∩ approval (0.5).
- **Loud authority.** No calls in string holes, no operator overloading, no macros, no implicit receivers, no host-call respelling.
- **Machine fixes never change authority.** A fix that adds, changes or removes any of the following is at most `maybe`:
  - a resource literal or manifest entry;
  - `trusted()`, `uses` or `verify`;
  - an entrypoint kind or an idempotency flag.

  Such fixes are never applied by `krit fix`, repair round 0 or `--edition`. `krit fix` recomputes the permission report (the card from 0.4.1) and aborts if any line changes. Habit fixes that change types or semantics are also `maybe`.
- **One registry.** It holds generated host rows plus hand-written intrinsic rows in one table, and a lint allows exactly these two inputs. A hand-written authority snapshot is diffed against it in CI (§7.3).
- **Upgrades are gated.** A runtime loads every artifact built by the same or the two previous minor toolchains (§7.7). Each release from 0.3 on has a gate of 0 orphaned durable records and 0 duplicate effects across the upgrade.
- **No private egress from tooling.** Loopback and private origins are reachable only from Rust test harnesses, never through a CLI flag.
- **Tie-breakers.** Fix the backend rather than narrowing `check`. Reject any feature whose only implementation is a second semantics.

### 3.4 Admission rule

RFCs live in `rfcs/NNNN-name.md`. The template extends the processes in spec/README.md and CONTRIBUTING.md. Each RFC must cover:

1. **Pull.** A reference app, at least 2 partner bots, or at least 3 bench tasks need it. Show before/after code.
2. **Layer.** Use the cheapest layer that works: host config → stdlib → local sugar → type system → runtime semantics.
3. **Definition.** Grammar; a Core desugaring or a verified Core op; spec text; dual-executor cases.
4. **Charter.** No hidden authority, control flow or conversion; one spelling, enforced by the formatter.
5. **Proof.** `krit metrics` and KritBench deltas: no statistically significant regression on the frozen set, and no lost card information. **Precedence:** changes motivated by charter 5 (least authority), 10 (secure failure) or 11 (sandboxed deployment) report their benchmark deltas, but those deltas never block.
6. **Habits.** The top 3 predicted model mistakes each get a coded fix.
7. **Edition.** Additive, or migrated by `krit fix`.
8. **Cost.** More than 10 files touched means a refactor lands first.
9. **Security label.** Any change to `wit/`, the annotations sidecar, the authority snapshot or a verify scheme needs a security-labelled RFC and a second reviewer (§7.3).

After 0.4:
- at most 3 new syntax forms per minor release;
- experiments sit behind `--unstable=<name>`, are recorded in artifact metadata (K9010), are refused by `serve` and `krit-action` unless explicitly allowed, and expire after two releases;
- every rejection carries a reopen trigger (§9.6).

### 3.5 Conflicts resolved

| Topic | Decision (why) |
|---|---|
| Release spine | Foundations order, with 0.2.1 and 0.2.2 first and 0.4 split into 0.4 and 0.4.1. Host work is 0.5. (Every reviewer found the merged releases overloaded.) |
| Guest strings | **0.2.2, in today's `agent_compiler.rs`.** They do not depend on NodeIds or the interpreter. They unblock R7 and `method == "POST"` about a year earlier solo, and the primitives carry into RFC-0006 option (a). |
| Sugar versus interpreter order | **Desugaring pass right after NodeIds**, in parallel with the sink and registry, then interpreter, then evaluator deletion. One AST-to-AST pass feeds the evaluator, analyzer and Core, so nothing is built twice. (This reverses the earlier "rejected".) |
| Deployability pass | 0.2.2: `check --target` in the CLI runs full emission with the output discarded, so it is exact by construction, and every K7002 carries a Core-op span. 0.3: a mirrored pre-check moves into `crates/krit/src/target/` for the LSP, and CI proves the mirror equals emission. (krit-lsp may not depend on krit-wasm, lib.rs:767-780; `check_module` is `pub(crate)`, support.rs:44.) |
| Component cache | In-memory only. (`Component::deserialize` is an `unsafe fn`; Cargo.toml forbids unsafe; CONTRIBUTING.md:45 requires a reviewed design.) |
| `?` lowering | 0.3 limits `?` to the function-body **spine**: any subexpression of a body-level `let` initializer or expression statement that is not under `if`, a match arm or a lambda. Evaluation is left to right: earlier sibling operands are hoisted into fresh lets, then a let-else exits. A verified Core `exit` (0.4) lifts the restriction. |
| JSON names | New `json_parse` and `json_stringify` (0.4). `json_encode` keeps its exact output in every edition and is deprecated (K9008). `json_decode` is deprecated in 0.4 and removed in 2027. One text means one thing in every edition. |
| Stdlib naming | Flat and prefixed (`string_len`), with `::` for modules. No `ns.fn` and no `capability.operation` respelling. |
| Type application | None in expressions, because `a < 2 > (3)` already parses. Decoding is directed by annotation (K3018). |
| HMAC | Manifest-declared host verification is primary (0.5). The guest escape hatch is limited to the current request: the host reads the raw body and one named header. Its result must be used (K3022), and the card reads `INBOUND guest-checked (not enforced)`. |
| Secrets | Bound to origins and injection kinds in the manifest. The host enforces this from 0.2.1 (K5006), the compiler proves the pairs from 0.3, and more inject kinds arrive in 0.5. |
| Durable identity | Protocol 1.1 (0.3): package plus declared `durable_namespace`, no digest or world. Protocol 2 (0.5): adds a host-derived instance key. Replay conflict becomes a guest-visible `Err`. |
| Concurrency | 0.5 runs concurrently across stores and store-free invocations. Each durable store has a single writer, holding a lease from reservation to commit. |
| Provenance | 0-CFA closure-flow analysis over monomorphized Core. The only unendorsed exceptions are provider-assigned Int, Bool or enum fields from host-verified payloads. No origin-level endorsement. |
| LLM gates | Release gates are statistical non-regression on pinned open-weight models. Absolute targets and vendor-model scores are product metrics. |
| Workflows, `compat`, `patched`, signals, `parallel` | After 1.0. 0.7 ships retry-safety and a displayed fingerprint. |
| Codes | One allocation (§9.2). Habit codes are syntax-only, lints use a new K9 range, and dependencies get K6003. |
| Smaller calls | `uses [f]` for higher-order rows. `TypeName { ..r, f }` for record update, since a bare `{` is a block. `json_parse` ignores unknown fields, while `ai_json` denies them. Worker threads come before async host I/O. An in-house query driver, with salsa only if the LSP misses 50 ms. |

### 3.6 Corrections adopted

- **Baselines.** Conformance has 25 run, 29 check and 3 format cases, 57 in total. The test log shows 629 passed and 1 ignored. 11 of the run cases are check-rejected programs that expect K4xxx runtime codes, and 0.2.1 moves them.
- **Sketch 02** is excluded by design. The corpus metric counts the 8 eligible programs (01, 03, 03b, 04–08), and R4 replaces 02.
- **List spread needs layouts**, because policy 2 lowers only header, log-field and String lists. It moves to 0.4.
- **Habit categories.** `s.len()` parses and fails with K3004. `null`, `await` and `type` are identifiers and fail with K2001. Contextual keywords are additive.
- **Ordering fixes:**
  - the dummy-capability metric is defined in a script, and its target moves to 0.5;
  - list combinators stay native intrinsics through 1.0, and are never moved into a Krit prelude;
  - fallible decoding lands in the interpreter and Wasm together;
  - the WIT annotations sidecar lands in 0.4, not 0.5.
- **Idempotency.** GitHub add-labels is idempotent, so idempotency is declared per method and path, not inferred from POST.
- **Dropped as ungrammatical:** `f<T>(x)` calls, `parallel { a = … }`, and `keyed` clauses that reference later parameters.

## 4. Release roadmap

**Throughput model.** A solo maintainer delivers 0.646 pw per calendar week. The near-term weeks use an explicit calendar: 0.8 pw per working week, with the two weeks around Christmas and New Year at 0.2. A second maintainer works at 50% for 13 weeks, then the pair runs at 90% combined efficiency. Every release lists what is never cut, and each cuttable item names the gate that moves with it. A slip of more than 6 weeks applies the cut list. Gates are never weakened.

### 0.2.1 "Stop the bleeding" (~5 pw; late Nov 2026)

**Goal:** make 0.2 safe to depend on: no aborts, no silent dependency, no secret misdirection, and no upgrade that kills a deployment. **Depends on:** nothing.

**Scope:**
- **Runtime and CI.**
  - Wasmtime 48 LTS with matching wasm-tools crates (Cargo.toml:60-76) and re-baselined golden digests.
  - cargo-deny and cargo-audit as release gates.
  - Tests on Linux, macOS and Windows; today ci.yml runs tests on ubuntu-24.04 only.
- **Crash safety.** K1005 guards in the parser, analyzer, Core lowering and formatter. An interim K4010 in `evaluator.rs`.
- **Front-end honesty.**
  - `run` analyzes first (`krit-cli/src/main.rs:1176-1224`, `krit/src/lib.rs:43-46`), and the 11 ill-typed run cases move to `conformance/check`.
  - K6003 for `[dependencies]` (`krit-package/src/lib.rs:160-166`).
- **Artifact compatibility (§7.7).**
  - The exact `compiler_version == CARGO_PKG_VERSION` and `edition == "2026"` checks (validation.rs:503-504, 543-548) become membership in published supported sets of metadata schema, policy version, `krit:runtime` WIT version and edition.
  - Effects are still re-derived from imports.
  - `conformance/build/compat` checks in a 0.2.0-built artifact that must load.
- **Secret→origin binding (host side).**
  - The manifest gains `[secrets.<name>] origins = ["https://api.github.com"]`, with `inject = "bearer"` as the default.
  - The Secret resource carries its logical name. `network.rs:249-260` refuses injection to an unbound origin with K5006, before any bytes leave.
  - The legacy flat `secrets = [...]` form still parses. `krit permissions` labels it `UNBOUND (any granted origin)`.
- **Windows.** `invoke`, `worker` and `serve` with durable state or secrets refuse to start on non-Unix hosts, because owner-only checks are no-ops there (host_config.rs:1668-1671). WASM-SANDBOX.md documents Windows as supported for check, build, fmt, lsp and run only.
- **Cleanup.**
  - SECURITY.md, a private disclosure address and an advisory process (week 1).
  - The prompt-pack version is single-sourced (`krit-assist/src/protocol.rs:7` says 0.2.12; the pack says 0.2.15).
  - A host default User-Agent.
  - K7002 messages stop saying "policy 1" for webhooks, and config keys accept `_`.
  - Doc drift is fixed and `docs/roadmap.md` is published.

**Gate:**
- Wasmtime 48 LTS with clean advisories; all tests green on 3 OSes.
- 100k-deep recursion yields K4010; 2,000-deep nesting yields K1005 in all four stages.
- `run` refuses every `conformance/check` failure.
- `[dependencies]` yields K6003.
- The 0.2.0 artifact loads and runs on the 0.2.1 runtime.
- The critic's swapped-token probe (webhook-agent sending `github-token` to the message origin) is refused by the host with K5006 once bindings are declared.
- Windows host mode with durable state refuses to start.

**Size (pw):** runtime bump 1.25, deny/audit and 3-OS CI 0.5, guards 0.75, run gating 0.5, K6003 0.25, compatibility 0.5, binding 1.0, cleanup and Windows 0.25. **Cut list:** none (a safety release).

**Held position:** the feasibility critic asked for 4 pw. This release is 5, because the secret-binding blocker and the artifact-compatibility blocker both had to land in the first release.

### 0.2.2 "Honest check" (~7 pw; early Feb 2027)

**Goal:** limits are visible at check time, the cheapest deploy blocker is gone, and the starting point is measured. **Depends on:** 0.2.1.

**Scope:**
- **Fuzzing first, so CPU-hours accumulate before 0.3.** cargo-fuzz targets for lex, parse, analyze, format, the Core verifier and `validate_artifact`, plus a nightly job. The analyzer's `assert!` uniqueness checks (analysis.rs:925-950) are expected findings.
- **Exact `check --target component|module`.**
  - `support.rs` returns `Vec<Diagnostic>`, and DCE runs before the residual-type check (support.rs:76-81).
  - All 17 K7002 sites in `agent_compiler.rs` get the Core operation's source span. These include 768 (list matching), 1180/1188 (JSON decode), 2037 (composite equality), 2077 (String operator) and 2423-2545 (layouts); several pass `None` today.
  - `check --target` runs full emission with the output discarded.
  - `build --diagnostic-format json`.
  - A `check-build-agreement` CI job builds every corpus, conformance, example and app file and fails on any K7002 that check missed.
- **Guest strings.**
  - `krit-wasm/src/rt/strings.rs`: equality, ordering, concatenation and byte length, as plain-loop helpers inside today's emitter (support.rs:419-430). No new Wasm features are needed.
  - Escape handling in Wasm `json_decode` to String (agent_compiler.rs:1200-1287), so model output containing `\n` no longer traps.
  - Differentially tested against `evaluator.rs`.
- **Path-injection hardening, which ships with string building.**
  - The host rejects `.` and `..` path segments and percent-encoded `.`, `/` and `\` (`webhook.rs:115-140`).
  - The host sets `CURLOPT_PATH_AS_IS=1`, so the request sent is exactly the one validated.
- **KritBench v0, static** (§9.5):
  - 60 tasks with intent, minimal manifest, fixtures and a reference solution;
  - a minimal `crates/krit-eval` and `spec/BENCHMARK.md`;
  - scores are check@1, build@1 and perm-exact, plus func@1 on T1 pure tasks via `krit run`;
  - the B toolchain is the 0.2.2 compiler with the 0.2 prompt pack.
- **Apps.** R3 and R7 run under `apps-gate` through the loopback harness.

**Gate:**
- `check --target` equals build's K7002 set on every file, and no K7002 is reported at `1:1`.
- String `==`, `<` and `+` build and run with 0 evaluator/Wasm disagreements on the string cases and R7.
- An escaped-model-output build case passes.
- Dot-segment and encoded-slash conformance cases are refused.
- All stack-depth aborts are eliminated. Every crash found in 24 fuzz CPU-hours is triaged, and fixed or filed with a reproducer before 0.3.
- v0 baseline published with 95% intervals for at least 2 open-weight and 2 vendor models.
- Corpus deployable: 3 (03b, 06, 07).

**Size (pw):** fuzzing 1, `check --target` and agreement CI 2.5, strings and unescaping 2, path hardening 0.25, KritBench v0 1.25.

**Cut list:** KritBench v0 (1) ⇒ "baseline published" moves to 0.3. B stays defined as the 0.2.2 toolchain.

### 0.3 "One semantics, flat code" (~33 pw; solo Jan 2028, +1 FTE Sep 2027)

**Goal:** one front end, one executable semantics, flat code, and an upgrade path for deployed state. No new services. **Depends on:** 0.2.2 (guards, fuzzing, exact `check --target`, baseline).

**Order:** RFCs → NodeIds and module split → (desugaring pass ∥ DiagnosticSink ∥ registry) → target mirror → Core interpreter with FakeHost → evaluator deletion. The runtime-strategy RFC, protocol 1.1 and `krit-action` v0 form a second track that a second maintainer would own; solo, they interleave.

**RFCs first:**
- **RFC-0001:** one semantics; the `?` spine rule and evaluation order; Core `exit`.
- **RFC-0002:** flat sugar.
- **RFC-0003:** diagnostics schema 2 and the §9.2 table.
- **RFC-0004:** editions, JSON names, naming and reserved prefixes.
- **RFC-0005:** durable protocols 1.1 and 2 (§8.2).
- **RFC-0006:** guest runtime strategy (§7.2), informed by the 0.2.2 strings work.

**Language (edition 2026, additive; all via the desugaring pass):**
- `?` on the spine (K3012/K3013). A conformance case `f(log_info("a", []), g()?)` checks order.
- `let P = e else ARM;` (K3014); the scrutinee may not end in `}`.
- Nested built-in patterns and `_`; reading `_` warns with K2003.
- Named construction of built-in contract types, with punning and spec-fixed empty defaults (K3016).
- `["k": v]`, `()` and `///`.
- `\(name.path)` interpolation (K3015) and `"""` text blocks.

**Compiler:**
- A NodeId arena; the module split into `analysis/{resolve,infer,unify,effects,capabilities}.rs`, `core/{ir,lower,verify,render}.rs` and `visit.rs`.
- A DiagnosticSink with recovery and an Error type; schema 2 behind a flag.
- Habit codes, did-you-mean and K9001–K9005; `krit fix` with the authority invariant.
- Registry v1 in `krit-capability` (§7.1, item 4).
- The target pre-check mirrored into `crates/krit/src/target/`, so the LSP shows K7002. `check-build-agreement` proves the mirror equals emission.
- **Secret-pair analysis.**
  - A must-alias pass over Core emits `http.bearer(origin, secret)` requirement pairs. Secrets come only from `secret("literal")` and are second-class; a value that may be either of two secrets emits both pairs.
  - Package check fails with K5001 on an ungranted pair. The flat `secrets` form is an error once more than one HTTP origin is granted.
  - K9003 flags bound pairs that are never used.
- **Stdlib v0:** `int_to_string`, `url_path_segment` and `url_query(["k": v])`.
  - K9013 flags any hole or `+` operand flowing into an `HttpRequest` `path` or `query` unless it comes from those functions.
  - K9014 flags holes inside literals containing JSON or form structure (`{"`, `":`, `&`, `=`) that flow to a body, and suggests `json_stringify`.
  - Both are errors in `apps/` and in `krit-action` deploy mode.

**Executors:**
- A Core interpreter in `crates/krit/src/interp/`:
  - explicit frames and field-indexed records;
  - slice-view lists, which end the O(n²) copy at evaluator.rs:441;
  - K4010 for depth, K4011 for a step budget mirroring Wasm fuel, and a wall-clock deadline.
- `spec/CORE.md` defines resource exhaustion as a distinct outcome that both executors report. It is excluded from value agreement.
- **FakeHost.**
  - A host-interface trait is generated from the registry into a new leaf crate `krit-host-api`, with one method per WIT function.
  - `krit-fakehost`, which the CLI and krit-eval use but krit and the LSP do not, implements config, secret, HTTP, AI and log from fixtures and cassettes. It implements state, queue and database by wrapping `krit-state` and `krit-database` on in-memory SQLite, so durable semantics are never re-implemented.
  - The cassette format is defined here (`tests/*.cassette.json`): a request fingerprint maps to a response; no Authorization header is stored, headers are allow-listed, and each cassette carries a size cap and a digest.
  - `krit run --fixture` and `krit test` for fixture cases.
- `evaluator.rs` is deleted. Until then it is exempt from the one-table lint rather than migrated.

**Durable protocol 1.1 (§8.2):**
- Identity becomes (package name, manifest `durable_namespace`), with the digest, version and world removed.
- A replay conflict becomes a guest-visible `Err` (K5203) instead of a trap.
- `krit state migrate --from <old.wasm> --to <new.wasm>` rewrites protocol-1 rows in one transaction. It is a dry run by default and refuses on namespace collisions.

**Tooling:**
- Conformance runs with `--engine interp|wasm|both`.
- `krit metrics`.
- A registry-generated language card of at most 8k tokens, plus `llms.txt`.
- `krit permissions --diff`, with SECRETS pairs.

**Ecosystem:** `krit-action` v0, built last so it follows `permissions --diff` and `krit test`.
- Own repo, immutable releases, docs that require pinning by commit SHA, and the binary's SHA-256 embedded in the action source.
- GitHub artifact attestations for the binaries and the action from day one.
- **Two jobs.** An unprivileged `pull_request` job (`permissions: {}`, no secrets) runs check, build and test on fork code and uploads the permission-report JSON. A privileged `workflow_run` job only downloads and renders that JSON (escaped, §8.3) and never checks out PR code.
- `pull_request_target` with checkout is documented as unsupported, and the action refuses to build under it.

**Gate:**
- `evaluator.rs` is deleted, and the one-table lint passes.
- The interpreter matches `expect.stdout` and `expect.status` on every run case (oracle against spec). Interpreter and Wasm agree on the webhook fixtures (R3, R7, examples), excluding durable ops.
- `check-build-agreement` finds 0 K7002 that check missed; the LSP shows K7002 at the right span; the krit-lsp dependency test passes.
- Conformance has at least 60 run, 90 check, 15 format and 10 build cases.
- Multi-error recall is at least 90% on a 50-file seeded corpus, with at most 10% cascades.
- 12 habit codes ship with fixes, and `krit fix` changes no permission report across the corpus and apps.
- webhook-agent is at most 50 lines using only never-cut features, and every example has depth 3 or less.
- **Upgrade:** a 0.2.2-built R3 with queued jobs and replay records migrates with 0 orphaned records and 0 duplicate effects.
- The swapped-token case fails `build`, and the host still refuses it when the metadata is forged.
- RFC-0006 is accepted, with the 0.4 JSON and stdlib budgets re-estimated against it.
- KritBench: a full v0 run is published with no significant regression. func@1 and repair@3 baselines are published: stored 0.2.2-era outputs go through the fixture runner, and repair@3 runs with the 0.2.2 binary in the loop.

**Size (pw):** RFCs 1, NodeIds and split 5, desugaring 3, sink, diagnostics, habit codes and fix 5, registry 3.5, target mirror 1, interpreter 4, FakeHost 1.5, deletion and CORE.md 1.5, protocol 1.1 1.5, stdlib v0 and lints 1, secret pass 1, RFC-0006 and spike 1, tooling 1.25, action 1.5. Total 33.

**Never cut:** NodeIds, the desugaring pass (including named construction and entry lists, which the line gate depends on), sink, registry, target mirror, interpreter, non-durable FakeHost, evaluator deletion, protocol 1.1 and migrate, stdlib v0 and K9013, RFC-0006.

**Cut list (6.75 pw, about 20%):**

| Cut | Gate that moves |
|---|---|
| interpolation, text blocks and K9014 (1.25) | moves into 0.4 scope (R1's prompt needs them) |
| `krit-action` v0 (1.5) | the leading indicator uses CLI cards; "action on partner repos" moves to 0.4 |
| secret-pair pass (1) | SECRETS comes from runtime-enforced bindings only; the build-time pair check moves to 0.4.1 |
| language card and `llms.txt` (0.75) | the "card ≤8k tokens" gate moves to 0.4.1 |
| FakeHost durable wrappers (0.5) | `krit test` on durable ops moves to 0.4.1; R3 stays in the krit-eval harness |
| habit codes beyond the top 6, and K9001–K9005 (1) | "12 habit codes" becomes 6 |
| schema-2 flag (0.5) | moves to 0.4.1 |
| `krit metrics` (0.25) | gates measured by `scripts/metrics.py` |

**Held position:** the feasibility critic wants at least 30% cuttable scope per release. 0.3 is about 20%, because the semantic unification cannot ship in part. Padding the list with items that cannot really be cut would hide the slip; the date moves instead.

### 0.4 "R1 deploys" (~35 pw; solo Feb 2029, +1 FTE Apr 2028)

**Goal:** the deployable language equals the checked language for every closed type, and the beachhead bot runs in GitHub Actions. **Depends on:** the 0.3 NodeIds, registry, interpreter (as oracle), target pass, protocol 1.1 and RFC-0006.

**Language:**
- `type`, with optional wire names on fields (`plus_one: Int as "+1"`).
- `enum` with payloads, string-valued variants (`Bug = "bug"`) and at most one fallback variant (`Other(String) = ..`). Matching is exhaustive and has no catch-all arm.
- `[x, ..xs]` and `TypeName { ..r, f: v }`.
- `json_parse` and `json_stringify` (§5.6), with `json_decode` and `json_encode` deprecated (K9008); K3019 for types that are not JSON-codable.
- Second-class `Secret` and `DatabaseTransaction` parameters: they may be passed down, and an escape is K3009 or K3010. A closure that captures either becomes second-class itself.
- Unrestricted `?` via Core `exit`.
- `test "name" { }` with `expect` and `expect_eq`.
- Stdlib subset (§5.6) as registry intrinsics. Callbacks to higher-order functions must be pure (K3020) until 0.6.
- `ai_json("adapter", prompt)`, decoded by the generated codec, which denies unknown fields.

**Compiler:**
- Re-verified Core passes: monomorphize, closure-convert, DFE, constant folding, tail marking, `exit`.
- LIR, a layout engine and **one emitter**. They replace `compiler.rs` (861 lines), `agent_compiler.rs` (2,697 lines) and most of `support.rs` (1,546 lines).
- The guest runtime per RFC-0006.
- Policy 3 (bulk memory and tail calls) in `validation.rs`.
- WIT-driven host lowering via `wit-parser`, replacing the offsets at `agent_compiler.rs:1675-1708` and `2307-2505`.
- Dynamic worlds; the u16 mask and the 64 checked-in worlds are deleted.
- **The annotations sidecar lands here.** The registry is generated from `wit/runtime.wit` plus `wit/krit-annotations.toml`, plus hand-written intrinsic rows.
- A well-typed Core generator (including deep and non-terminating programs) drives differential testing.
- Metadata gains the Component Model version and the build id; a name section and `krit.sourcemap`.
- `--backend legacy` is kept for one release.

**Runtime:**
- The memory cap follows host limits (16 MiB default, 64 MiB hard).
- Per-kind deadline caps: webhooks 30 s; queue, schedule and Actions 120 s; AI 90 s.
- **One adapter protocol, `openai-chat`**, chosen because the most providers and local servers speak it.
  - The manifest adapter declares `model`, `max_output_tokens`, `temperature` and `system_prompt_file`. That file is a package file, hashed into the metadata and shown on the card.
  - The source prompt is the user message.
- **Minimal host attenuation.**
  - Per origin: `{methods = ["POST"], paths = ["/repos/acme/app/issues/*/labels"], idempotent = true}`.
  - Matching runs on the normalized path; `*` matches exactly one segment and never `/`.
  - K5005 yields an `Err`. `idempotent` is recorded for the card and K9007.

**Ecosystem:**
- **`krit-action` deploy mode:** the event-mapping table and profile from §2.1; the explicit-permissions check; the bound `GITHUB_TOKEN`. It recomputes the re-derivable permission sections from the artifact and refuses on mismatch. Attenuation is required.
- Sigstore-signed binaries and SBOMs.
- A repo-bot cookbook draft.

**Gate:**
- 0 K7001/K7002 on verified Core across conformance, examples, apps and bench references.
- 0 interpreter/Wasm value disagreements in 48 CPU-hours from the generator, with both executors reporting exhaustion on non-terminating programs. 100% stdout agreement on conformance.
- R1 runs in Actions against a real sandbox repo (explicit permissions block, attenuated grant, bound token). R2 (including its date window), R3, R6 and R7 build, pass fixtures and run.
- **Upgrade:** a 0.3-built R3 with queued jobs and replay records upgrades to 0.4 with 0 orphaned records and 0 duplicate effects, with no migration step.
- At least 6 of 8 corpus programs deploy: 03b, 06, 07, 01 (Actions variant), 05 and 08.
- JSON: 0 disagreements in 24 CPU-hours of three-way fuzzing (interpreter, guest, host validator). Cases for encode → `queue_publish` → worker `json_parse` and for `json/ignored-float` pass.
- A dummy capability touches at most 10 hand-edited files (§7.3).
- The webhook-agent component is at most 64 KiB.
- KritBench v0: no significant regression, and build@1 equals check@1 within the confidence interval.

**Size (pw):** LIR, layouts and emitter 10; Core passes 4; WIT lowering, worlds and sidecar 2; guest runtime and JSON 4; types and patterns 4; stdlib subset 3; generator and differential 2; action deploy 1.5; attenuation 1; adapter and caps 1.5; signing and SBOM 0.5; upgrade and compatibility cases 0.5; metadata and sourcemap 0.75.

**Cut list (3 pw):**

| Cut | Gate that moves |
|---|---|
| `test` blocks (0.75) | moves to 0.4.1; no gate moves |
| spread and update (0.75) | no gate moves |
| wire names (0.25) | no gate moves |
| `time_` subset (0.5) | R2 gate moves to 0.4.1 |
| name section and sourcemap (0.75) | trap locations stay at `artifact.wasm:1:1` until 0.4.1 |

**Held position:** only about 10% of 0.4 can be cut, because the backend is indivisible. Everything that is not backend has already moved to 0.4.1.

### 0.4.1 "Authoring loop" (~15 pw; solo Jul 2029, +1 FTE Jul 2028)

**Scope:**
- **`card/1`** (§8.3): in metadata, with every line tagged; `krit card --diff`, with `permissions` kept as an alias; the escaping renderer. `serve` and `krit-action` refuse artifacts whose attested card disagrees with re-derivation.
- `krit grammar --format ebnf|gbnf|lark` with CI acceptance.
- `todo("label")` and `check --holes --json`.
- Schema 2 becomes the default.
- `krit mcp`, hardened (§9.4).
- `assist suggest --repair-rounds N`.
- `krit new` with AGENTS.md/CLAUDE.md and a `.mcp.json` that names a version-pinned binary.
- A tree-sitter grammar and a VS Code extension (Marketplace and Open VSX).
- The rest of stdlib v1, including `int_parse_scaled`, `base64_*` and `hash_sha256_hex`. Host search results gain integer `score_bp`.
- The `anthropic-messages` adapter.
- **KritBench v1:** 100 new tasks, the 160-task scoring set frozen, and B re-baselined on v1 with the 0.2.2 toolchain.
- A reviewer pilot (n≥5).
- The `benchmarks/` runner.
- A reproducible-build check; JSON Schema generation moves out of `krit-cli/src/main.rs`.

**Gate:**
- A forged-metadata artifact (card claims narrower than its imports) is refused by `serve` and by the action.
- `krit fix` aborts on any card change (invariant suite).
- The grammar accepts every conformance and example file and rejects every K1xxx case.
- v1 is frozen and re-baselined, and a full run is published.
- `krit mcp` passes path-confinement tests.

**Never cut:** card/1 (2), KritBench v1 (2), reproducibility (0.5).

**Cut list (10.5 pw):**

| Cut | Gate that moves |
|---|---|
| editors (1.25) | no gate moves |
| `krit mcp` (1.5) | no gate moves |
| grammar (1.25) and holes (1.5) | the grammar gate moves to 0.5 |
| repair loop (1) | the repair@3 product goal is measured with the plain harness |
| second adapter (0.5) | no gate moves |
| remaining stdlib (2) | R9 helpers move with it to 0.6 |
| reviewer pilot (0.5) | moves to 0.5 |
| benchmarks runner (0.5) | "warm invoke measured" moves to 0.5 |

### 0.5 "Runs on your box" (~25 pw; solo Apr 2030, +1 FTE Dec 2028)

**Goal:** the same artifacts run self-hosted: verified, concurrent, durable per delivery and operable without SQL. **Depends on:** 0.4 WIT-driven lowering and the generated registry. Inside 0.5, HostService precedes every new credential, verification and adapter feature; the epoch ticker precedes the daemon; and the durable conformance suite precedes the worker pool.

**HostService first.**
- One trait per WIT interface (`add_to_linker`, policy, stats). It replaces the monolithic `HostState` (lib.rs:160-187), the roughly 390-line `validate_agent_host`, the closed 18-field `Capabilities`, and `ExecutionStats` built at 4 sites.
- The trait has **no default `policy` implementation**.
- One negative runtime test is generated per registry row: an ungranted resource is denied.
- Load-time "imports ⊆ grants" validation stays hand-written code.

**Then, in order:**
1. An epoch ticker with per-Store deadlines, replacing the global Mutex (lib.rs:147-158).
2. An in-memory `Component`/`InstancePre` cache keyed by (BLAKE3, engine fingerprint); validation once per artifact; the pooling allocator.
3. A durable-semantics conformance suite (`conformance/durable/`): leases, K5202, crash windows and inbound-idempotency leases. Today only 7 static check cases exist.
4. A bounded worker pool with a **single writer per durable store**, holding a lease from reservation to commit (§7.5). The K5202 count is reported in `ExecutionStats`.
5. The `krit serve` daemon:
   - loopback by default, with an explicit non-loopback bind;
   - an unverified webhook on a non-loopback bind is refused (K5008) unless `--allow-unverified` is passed, which the card shows;
   - `/healthz`, graceful drain, body limits and a reverse-proxy and TLS guide;
   - `krit serve --config` hosts N artifacts under route prefixes.
6. Manifest `verify = { scheme = "github-hmac-sha256" | "slack-v0" | "stripe-v1", secret }`, checked in constant time before the guest runs; K5004 answers 401. The restricted guest escape hatch `request_verify_hmac_sha256` must have its result used (K3022).
7. **An App installation-token secret kind with downscoping:** `[secrets.github-app] permissions = { issues = "write" }, repositories = "delivery"`. The host requests a token with exactly those `permissions` and `repositories` and fails closed if GitHub returns broader ones.
8. **Secret inject kinds:** `bearer`, `{ header, scheme? }` (for example `Authorization: Bot …` or `DD-API-KEY`), `basic { user, password }` and `url_path_suffix` (Slack, Discord and Teams incoming webhooks). Each is a HostService registry row shown on the card.
9. HTTP attenuation generalized through HostService (K5005).
10. **Durable protocol 2** (§8.2): host-derived instance keys; authenticated replay storing only the secret reference, re-checked against the current binding; migration of 1.1 rows.
11. **Host-side `ai_json`:** an adapter 1.1 output schema and re-validation. The reask template is fixed and versioned in `spec/AI-OBSERVABILITY.md`, the card shows the reask bound, and the reask chain is recorded as one replay unit. **Budgets** per adapter (`max_input_bytes`, `max_output_tokens`, `max_calls_per_invocation`, `tokens_per_day`) are host-enforced; K5107 yields an `Err`, never a trap. Token usage appears in `ExecutionStats`.
12. `route "POST /path" fn`.
13. **Operate:**
    - `krit inspect queue|dlq|replay|store`, redacted by default, and `krit dlq redrive`;
    - the host honors upstream 429 and `Retry-After`;
    - queue entrypoints may return `Err(Retry(ms))` or `Err(Fail(reason))` via a built-in `QueueError` enum;
    - OTel-compatible JSON spans per host call (op, resource, latency, outcome, retries, tokens; never payloads);
    - audit events for denials and approvals.
14. `krit test --record` through the real runtime only. Cassettes pass through the exact-secret redactor. Signed-fixture replay uses a test-only verifier behind a compile-time feature that is absent from `serve` and `invoke` builds.
15. SLSA provenance, a `slack-bot` template, and deletion of `--backend legacy`.

**Gate:**
- A dummy capability touches at most 8 hand-edited files. The authority-snapshot CI and the per-row negative tests pass.
- Webhook ack p95 stays under 300 ms while a worker holds a 20 s AI call. Warm in-process invoke p50 is at most 2 ms. Throughput is at least 500 req/s on 8 cores for store-free invocations.
- 1,000 concurrent deliveries on one store, with a counting non-replayed POST fixture, produce 0 duplicate effects. R5 and R10 with 8 workers show a K5202 rate below 0.1% and 0 duplicate effects.
- R1 runs self-hosted with github-hmac verification and a downscoped App token, and refuses a broader token.
- R4 and R5 pass.
- R10 survives 1,000 redeliveries with 0 duplicates, and a **version-bumped** redeploy keeps its in-flight deliveries.
- A kill -9 drill mid-delivery and a DLQ redrive on R5 succeed without SQL.
- A 10k-payload injection soak produces 0 budget overspend; fuzzed model output always yields `Err`.
- 7 of 8 corpus programs deploy (adding 03).

**Cut list (7 pw):**

| Cut | Gate that moves |
|---|---|
| slack-v0 (0.5) | R4 moves to 0.6 |
| stripe-v1 (0.5) | R5 and the kill -9 drill move to 0.6 (the drill runs on R10 instead) |
| `route` (1.5) | no gate moves; R4 uses one webhook |
| inject kinds (1) | the Discord/Teams fixture for R2 moves to 0.6 |
| OTel spans and audit events (1) | no gate moves |
| `serve --config` (0.5) | no gate moves; use 3 daemons |
| record mode (0.75) | no gate moves; cassettes stay hand-written |
| host reask (0.75) | no gate moves; library decoding remains |
| SLSA and template (0.5) | no gate moves |

github-hmac verification, protocol 2, budgets and the single-writer lease are never cut.

### 0.6 "Programs grow up" (~29 pw; solo Feb 2031, +1 FTE Jun 2029)

**Goal:** reusable code through generics, effect polymorphism, modules, incremental checking and a real IDE. **Depends on:** 0.4 monomorphization, the 0.3 NodeIds and module split, and registry type schemes.

**Language:**
- HM let-generalization. No value restriction is needed, because values are immutable.
- Generics in declarations, and generic `type`/`enum`.
- Bounds `comparable` and `json`.
- **Effect rows**, replacing the bidirectional `link_effects` (analysis.rs:3200-3209). This lifts K3020 (pure callbacks), and effects inside iteration get a REPEATS card line.
- An exact `uses [...]` clause, required on library `pub fn`.
- Modules: `use path::{a, b};`, `pub`, one module per file.
- Letrec mutual recursion.
- `Map<K: comparable, V>`, with key-sorted iteration.
- The remaining `time_*` functions.
- **List builders stay native intrinsics.** HM only replaces their hand-written registry type schemes with generalized ones. K9015 flags user-written recursive list builders, with a fix pointing at the matching stdlib function.
- Experiments: `--unstable=pipe` and `--unstable=with`.

**Compiler:** a module-granularity query driver keyed by content and interface hashes; the crate split into `krit-syntax`, `krit-sema`, `krit-core` and `krit-target`; K3017.

**Tooling and ecosystem:**
- LSP v2, `krit doc` and `krit repl`.
- Edition 2027 becomes the default for `krit new`, with `krit fix --edition 2027`.
- Path dependencies, `krit.lock` and a content store.
- `krit/github` and `krit/slack` packages written in Krit.
- The KritBench TypeScript twin and a public leaderboard.
- Per-key durable versions, only if a partner's throughput pulls them (§7.5).

**Gate:**
- R5 and R9 are built from at least 2 modules with generic helpers.
- The effect-precision suite passes: a pure lambda next to an effectful one stays pure, whereas 0.2 reports `io.stdout`.
- LSP p95 is at most 50 ms on 5k lines.
- `krit fix --edition 2027` mechanically migrates groups 1 and 3 on 100% of corpus and partner code, and no `json_decode` remains in apps or corpus.
- All 8 corpus programs deploy.

**Cut list (4.5 pw):**

| Cut | Gate that moves |
|---|---|
| `repl` (0.5), `doc` (0.5) | no gate moves |
| connector packages (1) | no gate moves |
| TS twin and leaderboard (1) | TS-twin comparisons move to 0.7 |
| experiments (1) | no gate moves |
| content store (0.5) | the lockfile still ships |

### 0.7 "Trusted agents" (~31 pw; solo Jan 2032, +1 FTE Dec 2029)

**Goal:** ship the differentiators on the finished foundation. **Depends on:** 0.5 HostService (the seam for fakes, simulation and MCP serving), 0.4 monomorphized Core, 0.4.1 card/1, and the 0.3 registry trust and idempotency roles.

**Analysis and language:**
- Provenance (§8.1) with 0-CFA, adding UNTRUSTED, ENDORSED, DECIDES and **EXPOSES** card sections, plus K9011 and K9012.
- `trusted(v, "literal reason")` (K3021, K9006).
- Retry-safety (K9007 and the ON RETRY section).
- Per-definition Core hashes, plus an effect fingerprint in metadata, `explain` and `card --diff`.
- `tool "name" fn f(input: In) -> Result<Out, ToolError>`.
- The decision on `prompt fn`.

**Runtime:**
- `krit serve --mcp` over stdio and streamable HTTP, pinned to MCP 2025-11-25, in `crates/krit-mcp`, hardened as described in §9.4.
- **Adapter v2:** typed message lists (system, user, assistant), stop sequences, and `ai_embed("adapter", text) -> Result<Embedding, String>`. `Embedding` is an opaque host value consumed only by vector search and upsert, so no floats enter the guest; it replaces R7's embedder-adapter hack.
- Discord Ed25519.

**Tooling:** `card --against grant.toml` with the widening definition from §8.3; `krit sim --seeds N`; `krit eval`; a browser playground.

**Gate:**
- Provenance is correct on a 20-program labelled suite, in at most 1 s on 5k lines.
- An injection suite of at least 50 payloads, including exfiltration-via-comment payloads, produces 0 effects outside the card, 0 unreported untrusted→privileged flows and 0 unreported EXPOSES flows.
- Endorsements, counting verified-scalar exceptions, stay at or below 2 per 100 lines.
- `card --against` is correct on a suite of at least 40 cases, including one per widening kind.
- K9007 has 0 false positives on the apps.
- R8 passes MCP Inspector in 2 clients, plus Origin-validation and loopback-default tests.
- If they ship: `sim` finds every planted bug in 5 seeded apps, and `eval` is byte-identical across runs.

**Cut list (9.5 pw):**

| Cut | Gate that moves |
|---|---|
| `sim` (3), `eval` (2.5) | their gates move past 1.0 |
| playground (1.5) | no gate moves |
| Discord Ed25519 (0.5) | no gate moves |
| `prompt fn` decision (0.5) | no gate moves |
| Adapter v2 (1.5) | R7 keeps its embedder adapter |

1.0 does not depend on any cuttable 0.7 item.

### 1.0 "Stable contract" (~18 pw + audit)

**Goal:** a contract partners can bet on. **Depends on:** everything above; the audit is booked at 0.6, with its funding source confirmed by 0.5 (§12).

**Freeze:**
- edition 2027, which becomes the default
- normative `spec/CORE.md` and `spec/JSON.md`
- diagnostic schema 2
- `krit:runtime@1.0.0` WIT
- the metadata schema and the artifact-compatibility policy
- durable protocol 2
- lockfile v1
- card/1

**Also:**
- At least 500 conformance cases, covering every normative section and every code.
- Git dependencies, each with a card derived from `uses`, and `krit add` with existence and typo checks; 19.7% of LLM-suggested packages are hallucinated.
- An opt-in strict provenance mode.
- An external audit and at least 1,000 cumulative fuzz CPU-hours.
- The next Wasmtime LTS (~60).
- A book, a registry-generated stdlib reference, and a cookbook for each app.
- KritBench v2 (at least 200 tasks, at least 3 model families).
- Notarized binaries.
- A stability policy: no breaking change within an edition, and at most one edition every two years.

**Gate:**
- Audit criticals are closed.
- A full KritBench v2 run is published with 95% intervals, with no significant regression on the pinned open-weight models.
- The reviewer study (n≥8) is published; a miss triggers a design-review RFC and does not block the release.
- Two consecutive minor releases ship with no breaking change.

**Product goals** (not gates):
- check@1 at least 80% on frontier models;
- func@1 at least 65%, and within 10 points of the TypeScript twin;
- repair@3 and perm-exact both at least 95%;
- open-weight models of 35B or smaller reach check@1 of at least 70% with grammar decoding;
- reviewers answer at least 2× faster than on TypeScript twins, with at least 95% accuracy;
- 25 orgs, 100 bots, and 0 secret or private-data exposures.

**Cut list (3 pw):**

| Cut | Gate that moves |
|---|---|
| KritBench v2 beyond 160 tasks (1) | no gate moves |
| book (1) | cookbooks remain |
| git dependencies and `krit add` (1) | 1.0 ships path dependencies only |

### Beyond 1.0 (each item only when pulled)

- `workflow "name" fn`, journaling every import by (instance, step), with fingerprints, `krit compat` and `patched("id")`; `keyed` goes after the parameter list. Per-instance checkpoints live here.
- Durable signals and approval waits.
- One structured `all` construct on WASI 0.3, with no assignment-shaped bindings.
- Streaming AI.
- An embedding SDK.
- A signed registry with a transparency log.
- Per-dependency components composed with wac.
- `mcp_call`.
- Capability values.
- Decidable boundary refinements.
- Decimal and calendar dates.
- Confidentiality labels beyond EXPOSES.
- AWS SigV4.
- A multi-host store.
- A `wasi:http/proxy` export.
- Wasm GC or Perceus, only for long-lived workflows.

## 5. Language design

### 5.1 Eliminating the three dialects

1. **One front end (0.2.1).** `run` stops on analysis errors. From 0.3 every command goes parse → desugar → analyze → lower → verify.
2. **One executable semantics (0.3).** `spec/CORE.md` defines evaluation order, checked arithmetic, structural equality, effect order, trap kinds and resource exhaustion. Its reference implementation is the Core interpreter, and Wasm is tested against it differentially. LANGUAGE.md keeps the surface syntax and the desugarings.
3. **Honest, then total targets.** From 0.2.2 every K7002 is reported at check time at its true span; from 0.3 the LSP shows it too. From 0.4 every verified Core module builds.
4. **Conformance matrix.**
   - Cases run on both executors.
   - `conformance/check` gains `--target` expectations.
   - `conformance/build` asserts imports, worlds and cross-version compatibility.
   - `conformance/durable` arrives in 0.5.
   - `conformance/README.md` stops saying it invokes `krit run`.

### 5.2 Edition strategy

**Additive features ship in edition 2026 immediately.** Each one either uses a token that is invalid today or is a contextual keyword:

| Syntax | Why it is safe to add |
|---|---|
| `?` | an error today (K0001) |
| `\(` | an error today (K0003) |
| `Ok(())` | an error today (K1001) |
| `"""` | invalid today |
| `type`, `enum`, `test`, `route`, `tool`, `uses`, `use`, `pub` | contextual at item start, so `let type = 1;` still checks |

**Edition 2027 holds exactly three breaking groups.** None of them changes what a surviving name means:

| # | Change in 2027 | Warning in 2026 | Migration |
|---|---|---|---|
| 1 | `_` becomes unreadable | K2003 | machine |
| 2 | `json_decode` is removed. `json_encode` stays, with output frozen in every edition, and is deprecated in favour of `json_stringify` | K9008 | `maybe` (needs a Result context) |
| 3 | stdlib **prefixes** are reserved: `string_`, `list_`, `int_`, `option_`, `result_`, `map_`, `time_`, `json_`, `http_`, `form_`, `url_`, `base64_`, `hash_`. Individual names are not reserved, so later stdlib additions are not breaking. `todo` stays shadowable with a K9009 warning | K9009 | machine (rename) |

**Rules:**
- Differences live only in the lexer, parser and resolver, and both editions lower to the same Core.
- **Edition discovery:** from the manifest `edition` field; standalone files take `--edition`, whose default is 2026 through 0.5 and 2027 from 0.6.
- `validation.rs` accepts the published set of editions; the 0.4 artifact policy adds 2027.
- Edition 2027 is available from 0.4, becomes the `krit new` default at 0.6, and is frozen at 1.0.
- `spec/EDITIONS.md` records the policy.

### 5.3 Priorities

- **P0**, everything through 0.4.1: one semantics, flat sugar, full backend, user types, sound JSON, stdlib v1, `test`, `todo()`, card/1.
- **P1**, 0.5–0.6: `route`, verified webhooks, attenuation, inject kinds, budgets, HM, generics, effect rows, `uses`, modules, `Map`.
- **P2**, 0.7: `tool fn`, `trusted`, EXPOSES, the `prompt fn` decision.
- **Experiments:** `|>`, `with`, labelled host arguments.
- **Rejected syntax** is listed in §6.

### 5.4 Type system

- **0.3** adds an Error type so one mistake does not cascade.
- **0.4** adds:
  - named, closed, structural `type`, with optional wire names;
  - nominal `enum` with payloads, string-valued variants and at most one `Other(String) = ..` fallback (exactly one, String payload, last);
  - pattern-matrix exhaustiveness without top-level catch-alls;
  - derived equality and JSON codecs;
  - the canonical-ABI layout fixed as the stable exported-type ABI, which closes that open decision.

  Declared and anonymous records with the same fields are the same type.
- **0.6** adds HM, explicit generics and generic types, with fully annotated public signatures. It bans polymorphic recursion (K3017) so monomorphization terminates, and fixes two bounds: `comparable` (excluding functions, Secret and transactions, which is K3006's domain) and `json` (excluding `Option<Option<_>>` and `Option<Unit>`, K3019).
- **Not in 1.0:** traits, row-polymorphic records, higher-kinded types, capability values.

### 5.5 Effects syntax

The open question of inline effects versus a `requires` clause is resolved with a trailing **`uses`** clause (0.6):

```krit
fn lookup(q: String) -> HttpResponse uses [cache.write("lookups"), search.query("docs")] { … }
fn retry<T>(f: fn() -> Result<T, String>) -> Result<T, String> uses [f] { … }
```

Rules for `uses`:
- It must equal the inferred set exactly, so it cannot lie.
- `uses []` is a checked purity promise.
- It is optional in applications and required on library `pub fn`, where it acts as the dependency's authority card.
- The formatter sorts it, and the LSP can insert it.
- A callback's parameter name stands for that callback's row.
- There are no handlers.
- A machine fix may never add or change it (§3.3).

### 5.6 Standard library and JSON

**Naming.** Names are flat, prefixed and subject-first, like `state_get`.

**Implementation.**
- Intrinsics live in the registry.
- Higher-order list functions are emitted as native loops and **stay native through 1.0**: the bump arena never frees, and `(ptr, len)` lists would copy O(n²) bytes under recursive building.
- Non-list functions follow RFC-0006, which prefers wasm-encoder primitives plus a Krit prelude.
- Callbacks must be pure (K3020) until 0.6.

| Family | 0.3 | 0.4 (used by R1, R2, R6, R7) | 0.4.1 (rest of v1) |
|---|---|---|---|
| `string_` | — | `len` (Unicode scalars), `byte_len`, `is_empty`, `contains`, `starts_with`, `ends_with`, `trim`, `split`, `join`, ASCII `lower`, `slice`, `truncate_bytes` (UTF-8-safe), `replace` | `find`, `lines`, `trim_start`/`_end`, `repeat`, ASCII `upper` |
| `int_` | `to_string` | `parse` → Result | `min`, `max`, `abs`, `clamp`, checked `pow`, `parse_scaled(text, places)` |
| `list_` | — | `len`, `is_empty`, `get` → Option, `first`, `map`, `filter`, `fold`, `any`, `take`, `append`, `concat`, `contains` | `last`, `reverse`, `all`, `find`, `drop`, bounded `range`, stable `sort_by` |
| `option_` / `result_` | — | `unwrap_or`, `map`, `map_err`, `ok_or` | `and_then`, `is_some`, `is_ok` |
| HTTP and URL | `url_path_segment`, `url_query` | `http_header` (case-insensitive), `url_percent_encode` | `form_decode`, `url_percent_decode` |
| `time_` (UTC, pure) | — | `format_rfc3339`, `parse_rfc3339` → Result, `date`, `add_days` | (remaining functions at 0.6) |
| Encoding and hashing | — | — | `base64_encode`, `base64_decode` (valid UTF-8 or Err), `hash_sha256_hex` (host-implemented, pure, never takes a Secret) |

**Determinism rules** (in `spec/STDLIB.md`, generated from the registry):
- String comparison is byte-lexicographic over UTF-8.
- `int_parse` accepts an optional `-` and ASCII digits, with no whitespace and no `+`, within the i64 range.
- `string_split(s, "")` yields single-scalar strings, and `string_split("", sep)` yields `[""]`.
- `Map` iterates in key order (0.6).

**Excluded through 1.0:**
- floats
- regex (ReDoS risk)
- bytes
- randomness
- a guest clock
- cryptography implemented in guest code

Host-implemented verify and hash imports are allowed, and those that need keys take opaque Secrets.

**`spec/JSON.md` (0.4), an I-JSON profile (RFC 7493).** One table generates the guest codec and the host validator, and the interpreter, guest and host are fuzzed three ways.
- Duplicate keys are an `Err`, as are lone surrogates.
- Numbers are lexed with the full RFC 8259 grammar. An Int field accepts only `-?(0|[1-9][0-9]*)` within i64. A non-integer number in a field absent from the target type is skipped, because GitHub search returns `"score": 1.0` on every item. A number decoded into a String field yields its exact source text.
- No BOM. Maximum depth and bytes come from host limits.
- The guarantee is stated as "`Err` on any malformed input, within the K510x resource bounds".

**`json_parse` (0.4):**
- Directed by annotation: `let e: IssueEvent = json_parse(body)?;`. An undetermined target is K3018.
- Returns `Result<T, String>`.
- **Ignores unknown fields**, because provider payloads carry dozens.
- null and missing both map to `None`.
- Enums decode from their declared string, from `{"tag": …, "value": …}` for payload variants, or into the fallback variant.
- In Option positions it also accepts the legacy reserved shapes `{"Some": v}` and `{"None": null}` through 1.0, so payloads persisted by `json_encode` keep decoding after upgrades.

**`json_stringify` (0.4):**
- A `None` field is omitted, and `None` inside a list is `null`.
- Enums write their declared string or `{"tag": …, "value": …}`.
- K3019 rejects types that are not JSON-codable.

**`ai_json`:** decodes with **unknown fields denied**, because unexpected fields in model output are a signal, and the reask handles them.

**Legacy names.**
- `json_encode` keeps its exact 0.2 output in every edition; it tags Option as `{"Some": …}`.
- `json_decode` is deprecated at 0.4 and removed in 2027.
- Conformance: encode → `queue_publish` → worker `json_parse` round-trips.

**Held position:** the completeness critic proposed that declared types encode Option as null under `json_encode`. Declared and anonymous records are the same structural type, so output cannot depend on the name. K9008 steers every Option-bearing `json_encode` to `json_stringify` instead.

## 6. Beauty program

Beauty for Krit means reviewability:
- the happy path stays flat (depth 3 or less);
- every exit (`?`, `else Arm =>`) is visible on its own line;
- authority is loud;
- names beat positions;
- information-free tokens are gone: `record`, `query: ""`, `Err(e) => Err(e)`;
- every outcome is named;
- each construct has one spelling, enforced by the formatter;
- spellings are mainstream (Rust `?`, let-else and `///`; Swift `\(…)` and `["k": v]`), so model habits produce valid Krit.

| Status | Syntax | Grammar (summary) | When | Rationale |
|---|---|---|---|---|
| Adopt | `?` (identical error type) | `call = primary ( "(" args? ")" \| "." ident \| "?" )*` | 0.3 (spine), 0.4 (anywhere) | removes the pyramid's root cause; the most common error idiom in training data |
| Adopt | let-else with a bound failure arm | `"let" pattern (":" type)? "=" expr "else" match-arm ";"` | 0.3 | webhooks return `HttpResponse`, so they cannot use `?` |
| Adopt | nested patterns, `_` | `pattern = binder \| ("Ok"\|"Err"\|"Some") "(" pattern ")" \| "None" \| "[" "]" \| "[" binder "," ".." binder "]"` | 0.3 | four host ops return `Result<Option<_>>` |
| Adopt | named construction, punning, spec defaults | `primary += TypeName "{" (ident (":" expr)?),* "}"` | 0.3 (never cut) | `record` count drops from 9 to 0 |
| Adopt | entry lists | `list = "[" (args \| expr ":" expr ("," expr ":" expr)*)? "]"` | 0.3 (never cut) | a 9-line log call becomes 1 line |
| Adopt | `()`, `///` | `primary += "(" ")"`; `doc = "///" text` | 0.3 | `Ok(())` prints today but does not parse |
| Adopt | `\(name.path)`, `"""` | `string-char += "\(" ident ("." ident)* ")"`; Swift-style dedent | 0.3 (cut ⇒ 0.4) | prompts and paths; holes are names only and never literals; K9013/K9014 guard sinks |
| Adopt | `type`, wire names | `"type" TypeName "{" (ident ":" type ("as" string)?),* "}"` | 0.4 | domain names; real payload keys such as `"+1"` |
| Adopt | `enum`, string-valued, fallback | `variant = TypeName ("(" type ")")? ("=" string \| "=" "..")?` | 0.4 | one validation point for model output; real vocabularies such as `"opened"` |
| Adopt | spread and update, `test` | `"[" args "," ".." expr "]"`; `TypeName "{" ".." expr ("," field)* "}"`; `"test" string block` | 0.4 | list building; executable examples |
| Later | `uses [...]` | §5.5 | 0.6 | needs effect rows |
| Experiment | `\|>`, `with`, labelled host args | — | 0.6–0.7 | respectively: needs a subject-first stdlib, adds a second exit target, adds a second call shape |
| Reject (reopen trigger) | literal patterns and match guards | — | — | they make `_ =>` tempting, and catch-alls are banned; use if/else-if and string-valued enums. **Reopen** if K1111 (`switch`/`case`) is a top-3 failure code in the 0.4.1 KritBench run |
| Reject | UFCS, loops, comprehensions, `if let`, `return`, mutation, ternary, `a?.b`, `_ =>`, auto-`Ok`, `${x}`, raw strings, default params, trailing closures, backpassing, exceptions, macros, traits, async/await, operator overloading, layout syntax, ASI, `capability.operation` | — | — | each hides control flow or authority, adds a second spelling, or collides with an existing meaning |

**Measured before/after** (canonical source; rewrites use only 0.3 never-cut features unless noted):

| Program | Lines | Depth | Other |
|---|---|---|---|
| examples/webhook-agent | 79 → 47 | 15 → 3 | nested matches 6 → 1; `record` 9 → 0; `Err` arms 6 → 1 |
| examples/jobs-worker | 15 → 7 | 5 → 1 | 3 propagation arms removed |
| examples/database-webhook | 21 → 18 | 5 → 2 | duplicated rollback match gone |
| examples/cached-search | 50 → 30 | 4 → 2 | code lines 34 → 18 |
| examples/stateful-webhook | 23 → 14 | 6 → 2 | |
| examples/jobs-schedule | 6 → 4 | 2 → 1 | |
| sketch 01 triage (0.4 features, HMAC added) | 76 → 49 | 5 → 2 | |
| sketch 05 ticket router (0.4 features) | 91 → 35 | 5 → 2 | real in-process fallback |

**After 0.3** (webhook-agent excerpt, 47 lines in total; the 0.2.2 unescaping makes the `json_decode` line safe on model output that contains `\n`):

```krit
fn review() -> Result<HttpResponse, String> {
    let repository = config_string("reference.repository")?;
    let issue = http_request(
        "https://github.example.invalid",
        HttpRequest { method: "GET", path: "/repos/example/issues/1",
                      headers: ["x-reference-repository": repository] },
        Some(secret("github-token")?),   // spine: args are evaluated left to right, then the exit
    )?;
    // Model output is raw, nondeterministic, untrusted text.
    let model_output = ai_invoke("reviewer", issue.body)?;
    let summary: String = json_decode(model_output);   // K9008 from 0.4: use json_parse
    // ...post the summary the same way...
    Ok(HttpResponse { status: 200, headers: posted.headers, body: summary })
}
```

**R1 at 0.4** (cannot be built today):

```krit
/// The only labels the model may choose; any other answer fails decoding.
enum Label { Bug = "bug", Feature = "enhancement", Question = "question", Spam = "spam" }
/// GitHub sends many actions; each one is named here, none is silently dropped.
enum Action { Opened = "opened", Other(String) = .. }
type Issue { number: Int, title: String, body: Option<String> }
type IssueEvent { action: Action, issue: Issue }
type Verdict { label: Label, reason: String }
type LabelsBody { labels: List<Label> }

fn label_issue(issue: Issue) -> Result<HttpResponse, String> {
    let body = option_unwrap_or(issue.body, "");
    // Untrusted model output; decoding into Verdict is the validation step.
    let verdict: Verdict = ai_json("triage", """
        Classify this GitHub issue as bug, enhancement, question or spam.
        Answer as JSON with fields "label" and "reason".
        Title: \(issue.title)
        Body: \(body)
        """)?;
    let number = int_to_string(issue.number);   // the only kind of value K9013 lets into a path
    http_request(
        "https://api.github.com",
        HttpRequest {
            method: "POST",
            path: "/repos/acme/app/issues/\(number)/labels",
            headers: ["accept": "application/vnd.github+json"],
            body: json_stringify(LabelsBody { labels: [verdict.label] }),
        },
        Some(secret("github-token")?),
    )?;
    log_info("triage.labelled", ["reason": verdict.reason]);
    Ok(HttpResponse { status: 200 })
}

fn triage(request: HttpRequest) -> Result<HttpResponse, String> {
    let event: IssueEvent = json_parse(request.body)?;
    match event.action {
        Opened => label_issue(event.issue),
        Other(_) => Ok(HttpResponse { status: 204 }),
    }
}

webhook fn handle(request: HttpRequest) -> HttpResponse {
    match triage(request) {
        Ok(response) => response,
        Err(reason) => {
            log_error("triage.failed", ["reason": reason]);
            HttpResponse { status: 502, body: "triage failed" }
        },
    }
}
```

**Self-hosting R1 at 0.5 changes no source.** The manifest gains:
- `verify = { scheme = "github-hmac-sha256", secret = "github-webhook" }`;
- the App token kind, with `permissions = { issues = "write" }, repositories = "delivery"`;
- the attenuated grant `{methods = ["POST"], paths = ["/repos/acme/app/issues/*/labels"], idempotent = true}`.

The grant syntax is illustrative, because it is still an open decision.

By 0.7 the card reads as follows. Tags show how each line is guaranteed (§8.3), and source-derived strings render verbatim in code spans:

```
INBOUND      [enforced]   webhook verified github-hmac-sha256 (secret github-webhook); replay key X-GitHub-Delivery
REACHES      [enforced]   http.request("https://api.github.com")  POST /repos/acme/app/issues/*/labels  idempotent
SECRETS      [enforced]   github-token: bearer, only to https://api.github.com; App token issues:write, repository of delivery
AI           [enforced]   triage: openai-chat, max_output_tokens 400, ≤2 calls/invocation, reask ≤1, prompt prompts/triage.txt (blake3 …)
UNTRUSTED    [attested]   request.body → triage prompt                    (prompt input: allowed)
DECIDES      [attested]   triage chooses label ∈ {bug, enhancement, question, spam} → POST body
EXPOSES      [attested]   none: no authenticated read reaches a write body
ON RETRY     [attested]   redelivery repeats POST …/labels: declared idempotent; replayed per delivery
NOTHING ELSE [re-derived] no other origins, models, stores or queues; secrets unreadable by code
```

**How beauty is measured:**
- `krit metrics --json` (0.3) reports lines, depth, nested matches, duplicated arms, `record` count, distinct concepts, tokens, longest line and authority literals.
- CI fails on any regression in `examples/` or `apps/`, or on depth greater than 3.
- Every RFC reports its deltas.
- Formatter fixes in 0.3: break `+` chains, keep `1_000_000` digit separators, and enforce 100 columns.
- Beauty counts only if it moves KritBench for that feature, or reviewer time and accuracy, and never at the expense of a safety rule (§3.4).

## 7. Compiler and runtime architecture

### 7.1 Prerequisite refactors, in order

1. **Depth guards, fuzzing, run gating and exact `check --target` (0.2.1–0.2.2).** These are the guardrails for refactoring about 13k lines with agents.
2. **NodeId arena, module split and Visitor/Fold (0.3).** Every desugaring and every provenance path needs these.
3. **AST desugaring pass (0.3),** fed by NodeIds, with an origin map so K3012–K3014 point at the source the user wrote.
4. **DiagnosticSink (0.3).** Stages push diagnostics instead of returning `Result<_, Diagnostic>` (lexer.rs:6, parser.rs:13, analysis.rs:863-867).
5. **Registry v1 in `krit-capability` (0.3).** This crate is already a leaf dependency of five crates.
   - **Each row holds:** name, type scheme, effect, resource-argument positions and validators, approval class, manifest key, access mode, durable kind, the determinism, idempotency and trust-role classes (used in 0.7), **argument roles for redaction**, **reserved-in-edition**, `must_use`, WIT interface and function, a stats counter, and docs.
   - **It replaces the copies in:**
     - `builtin.rs` and `parser.rs:916-942`, which reserves only 14 of the 36 names today;
     - three analyzer and two verifier sites;
     - `durable.rs:6-30` and `support.rs`;
     - the LSP map;
     - `krit-package` and `krit prompt`;
     - **`krit-assist/src/context.rs:1054-1078`** (per-builtin redaction roles before context goes to a provider) and **`proposal.rs:60`**;
     - **`krit-runtime/src/permissions.rs`**.
   - It also generates the `krit-host-api` trait.
6. **Target pre-check mirrored into `crates/krit/src/target/` (0.3).**
7. **Core interpreter and FakeHost (0.3).** The interpreter becomes the oracle for every later backend change.
8. **Core passes, LIR, layout engine, one emitter, WIT-driven lowering, the annotations sidecar and Core `exit` (0.4).**
9. **HostService (0.5).** The single dispatch point that cassettes, simulation and trace spans hook into. It implements the same generated trait as FakeHost.
10. **Query driver and crate split (0.6).**

### 7.2 Backend

- **Monomorphize rather than box.**
  - Artifacts are whole-program and small (1.4–8.4 KB).
  - Monomorphized records are canonical-ABI records, and Int stays i64.
  - Size is bounded by DFE, by sharing instantiations with identical layouts, by a CI budget, and by K7006.
- **Memory: keep the per-invocation bump arena.** Values are immutable, invocations are short, and the Store is discarded. Wasm GC is rejected for 1.0: the canonical ABI copies anyway, toolchains are uneven, and the validator would have to widen. Perceus waits for long-lived workflows.
- **Layouts:**
  - records in declaration order;
  - lists as `(ptr, len)`, so `[h, ..t]` is an O(1) slice;
  - Option, Result and enums as tagged unions;
  - strings as UTF-8;
  - closures as `(table index, env ptr)`.
- **Guest runtime: RFC-0006 decides in 0.3, before 0.4 begins.**
  - **Option (a):** about 8–10 wasm-encoder primitives (alloc, byte_len, byte_at, O(1) slice view, concat_list, eq, compare, memcpy, plus a native list accumulator if needed). The rest of the string, int, URL and HTTP functions are a Krit prelude compiled by the LIR backend. Prelude functions run on the interpreter for free, so only the primitives need twins.
  - **Option (b):** a reproducibly built `no_std` Rust runtime, after a reviewed unsafe design under CONTRIBUTING.md:45.
  - **Acceptance criteria:** `string_split`, the JSON tokenizer and `list_map` each allocate O(n) bytes on 1 MiB inputs; no `unsafe` in the workspace; the interpreter-twin cost is stated. The JSON and stdlib budgets in §4 are re-estimated against the chosen option.
  - Option (a) is the current preference: one toolchain, and twins come free.
  - List builders are native intrinsics either way.
- **Control flow.** `return_call` in tail positions (policy 3); `exit` lowers to `return`; the interpreter uses an explicit stack with a step budget.
- **Verification:**
  - golden digests, re-baselined on every Wasmtime or wasm-tools bump;
  - differential fuzzing from a well-typed Core generator (0.4);
  - one release of `--backend legacy`.

### 7.3 Host and capability extensibility

WIT is the single source of host signatures and records. WIT has no general annotation syntax, so `wit/krit-annotations.toml` (0.4) carries the effect, validators, approval, manifest key, durable, idempotency and trust classes, argument roles and stats. From 0.5 it also carries a host-config schema entry. `build.rs` generates from it: registry host rows, analyzer schemes, verifier rules, the `krit-host-api` trait, FakeHost dispatch, lowering, worlds, manifest validation and the host-config schema. Stdlib intrinsics are hand-written rows in the same table; they are not WIT imports.

**Guarding the single source.** A one-line mistake in a generated table would weaken every layer consistently and pass every differential test. Four guards prevent that:
- `wit/authority.snapshot.toml` is hand-written and lists every op's effect, resource positions, approval class and durable, idempotency and trust classes. CI diffs it against the generated registry.
- Changes to the sidecar or the snapshot need a security-labelled RFC and a second reviewer, recorded in CODEOWNERS. While solo, the second reviewer is an external reviewer from a design-partner security team, contracted if needed.
- The HostService trait has no default policy.
- Load-time "imports ⊆ grants" validation stays hand-written and is not generated from the registry.

`scripts/dummy-capability.sh` defines the files-per-capability metric. It adds `dummy.ping(resource)` with one resource argument and no host config. Generated files count as 0, and the script counts hand-edited files:

| When | Hand-edited files |
|---|---|
| Today | ~20 (cache touched 17 non-test .rs files, 5 specs and the WIT) |
| 0.4 | ≤10 |
| 0.5 | ≤8 (WIT, sidecar, snapshot, service impl, service tests, spec page, conformance case, CHANGELOG) |

After 1.0, connectors can become wac-composed components.

### 7.4 Performance and caching

**Budgets:**

| Measure | Budget | Today |
|---|---|---|
| `check` | ≤20 ms per 1k lines | 40 ms for 3,000 functions |
| `build` | ≤150 ms per 1k lines | — |
| LSP p95 | ≤50 ms | — |
| Startup | ≤8 ms | 4.5 ms |

**Runtime (0.5):** the in-memory cache removes the per-call Cranelift compile (lib.rs:407-417); there is no on-disk cache (§3.5).

**Benchmarks:** a `benchmarks/` runner per performance.md (at least 30 samples; median, p95 and MAD; environment and power mode recorded) lands at 0.4.1. It fills the Phase 6/7 gaps and tracks the 18.9 MB binary and the component sizes.

### 7.5 Concurrency, async and WASI 0.3

**Concurrency happens between invocations, not within them.**
- The epoch ticker and worker pool land before the daemon (0.5).
- **Durable stores keep a single writer.** A per-store lease is held from reservation to commit. Concurrency runs across stores and for store-free invocations.
- This is needed because the store commits with a whole-store revision check (DURABLE-STATE.md:90-93; JOBS-AND-STORAGE.md:307). Without the lease, two concurrent invocations would both run external effects, and the stale one would get K5202 and redo them on redelivery.
- Per-key or per-namespace versions are a 0.6 item, taken only if a partner's throughput pulls them.
- Blocking curl on worker threads suffices at bot scale. Wasmtime async is adopted only if the throughput gates fail.

**Held position:** the feasibility critic asked for per-key versions before the pool. A per-store lease gives the same no-duplicate guarantee at bot scale, without rewriting the commit protocol of the 3,266-line `krit-state`.

**No guest async before 1.0.** WASI 0.3 (June 2026) still polishes cancellation and defers threads, so artifacts stay P2. After 1.0, one structured `all` construct may lower to P3, with ordered results, propagated cancellation and journaling.

### 7.6 Wasmtime policy

- 48 LTS in week 1; 47's support ended 2026-09-20.
- LTS lines only afterwards, with advisories patched within 7 days and cargo-deny as a release gate.
- The next LTS (~60) before 1.0.
- Every bump reruns differential conformance, the compatibility cases and the benchmarks.

### 7.7 Artifact compatibility and upgrades

- **Load rule (0.2.1).** A runtime loads an artifact if its metadata schema, policy version, `krit:runtime` WIT package version and edition are all in the runtime's published supported sets. The compiler version is recorded but not compared, and effects are always re-derived from imports.
- **Window.** Pre-1.0, a runtime supports artifacts from the same and the two previous minor toolchains, and never newer ones; 1.0 freezes a longer window. `conformance/build/compat` keeps one golden artifact per release.
- **Upgrade gates.** Durable records survive upgrades:
  - by migration from 0.2.2 to 0.3 (`krit state migrate`);
  - with no step from 0.3 on, because identity no longer depends on the digest or the world;
  - across version bumps from 0.5 (protocol 2).

  Every release from 0.3 on is gated on 0 orphaned records and 0 duplicate effects for a mid-flight R3, and from 0.5 for R10.

## 8. Next-generation differentiators

| Idea | Decision and form | When |
|---|---|---|
| Authority card | **Flagship.** `card/1` lives in metadata, with every line tagged (§8.3) and `--diff` in PRs. `--against grant.toml` auto-approves only non-widening changes. Runtime sandboxes (Code Mode, E2B, Wassette) cannot produce this pre-deploy diff. | 0.4.1 / 0.7 |
| Secret binding | Secrets bound to origins and inject kinds; host-enforced; compiler-proven pairs; downscoped provider tokens. | 0.2.1 / 0.3 / 0.5 |
| Typed prompts | Library first: `ai_json` returns `Result<T, String>`; the host schema, fixed reask and budgets arrive in 0.5. If adopted at 0.7, the syntax is `prompt "adapter" fn f(…) -> Result<T, AiError>`. `explain` lists a prompt inventory either way. | 0.4–0.7 |
| Provenance and exposure | 0-CFA facts first (§8.1). | 0.7; strict at 1.0 |
| Durable workflows | Protocol 1.1 (0.3), protocol 2 (0.5), a displayed fingerprint (0.7), then `workflow fn`, `compat` and `patched` after 1.0. | 0.3 → 1.x |
| Effects as capabilities | Literal resources plus `uses`; capability values after 1.0. | 0.6 |
| Attenuation | Host-level grants by methods, normalized paths and an idempotent flag. | 0.4 (minimal) / 0.5 |
| Simulation | `krit sim`: seeded 5xx, timeouts, malformed AI output, crash between effect and commit, lease expiry, duplicate delivery. Invariant: keyed effects equal those of the crash-free run. | 0.7 (cuttable) |
| Evals as tests | `krit eval`: datasets, scorers and thresholds in the manifest; cassettes by default; live recording only through the real runtime. | 0.7 (cuttable) |
| MCP and tools | `krit mcp` for authoring (0.4.1); `tool fn` plus `serve --mcp` (0.7). The protocol stays at the edge, and both are hardened (§9.4). | 0.4.1 / 0.7 |
| Linear handles | Second-class Secret and transaction parameters, with capturing closures second-class too (0.4); a `with` experiment (0.6). | 0.4 / 0.6 |
| Retry safety | Registry classes plus manifest flags, K9007 and an ON RETRY line. Today a raw POST in a queue worker builds cleanly. | 0.7 |
| Refinements | Decidable, checked at decode, no SMT. | beyond |
| Content hashes | Per-definition Core hashes for caching, diffs and fingerprints. Text stays the source of truth. | 0.7 |

### 8.1 Provenance and exposure

- **Lattice.** Integrity (trusted ⊑ untrusted), plus one confidentiality fact: *authenticated-read*. Full confidentiality labels come after 1.0.
- **Analysis.** A 0-CFA closure-flow analysis over monomorphized ANF Core, because closures stay first-class values after conversion (`(table index, env ptr)`) and the stdlib takes callbacks.
  - It builds the call graph and then data and control dependence. It reports paths by NodeId.
  - Its worst case is O(n³) in Core size, measured to run in at most 1 s on 5k-line apps.
  - Budget: about 2 pw inside the 0.7 provenance work.
- **Sources** (registry roles):
  - always untrusted: AI output, tool and MCP inputs;
  - untrusted by default: request bodies, headers and paths; queue bodies; HTTP responses;
  - trusted: config and literals;
  - authenticated-read: responses to bearer-authenticated requests, and AI output whose prompt contained such data.
- **Sinks:**
  - privileged sinks are flagged: method, path and query of authenticated requests, and `db_execute` parameters;
  - content sinks are allowed and listed: bodies, logs, responses;
  - prompts are allowed.
- **EXPOSES (K9012).** Authenticated-read data reaching the body of a write request (non-GET) or a webhook response is listed in an EXPOSES card section. Grants may set `exposures = "deny"`. The injection suite includes exfiltration-via-comment payloads.
- **Endorsement:**
  - `trusted(v, "literal reason")` is greppable and shown on the card; K9006 flags weak reasons.
  - Decoding into an enum or Bool does **not** endorse a value. The card shows the choice set in DECIDES instead.
  - **The only unendorsed exception:** Int, Bool or enum fields that the registry or a connector marks as *provider-assigned* (for example `issue.number` or `installation.id`), decoded from a **host-verified** payload, may enter path segments. This never applies to String fields, and never to AI-derived values. HMAC proves the sender, not that the content is benign.
  - **No origin-level endorsement.** Where needed, endorsement is declared per (origin, method, path pattern, field) and shown in ENDORSED.
  - Every exception use is listed on the card and counts toward the budget of 2 endorsements per 100 lines.
- **Severity:** K9011 and K9012 warnings, plus `decisions = "deny"` and `exposures = "deny"`, in 0.7. An opt-in strict mode in 1.0.
- **Guards:** an injection suite of at least 50 payloads (100 at 1.0), and docs that state exactly what is proven, because sandboxing is not safety.

### 8.2 Durable identity (RFC-0005)

- **Protocol 1 (0.2).** Identity is (artifact identity = package name, version, world, digest; kind; literal operation) (`policy.rs:1055-1061`; `schema.rs:46-58`). Any rebuild orphans records, and a second input traps with K5203.
- **Protocol 1.1 (0.3).**
  - Identity is (package name, manifest `durable_namespace`, kind, literal operation). The namespace defaults to the package name, and changing it is a deliberate fresh start.
  - Provider idempotency keys derive from it and are stable across redeploys.
  - Each record also stores the request fingerprint and the call-site Core hash.
  - A conflict (same identity, different input) returns `Err` (K5203) and performs no effect.
  - `krit state migrate` rewrites protocol-1 rows.
  - Inbound idempotency uses the same namespace.
- **Protocol 2 (0.5).** Identity adds a **host-derived instance key** and is never a guest value. The key comes from:

  | Entrypoint | Instance key |
  |---|---|
  | webhook with `verify` | the scheme's delivery id (GitHub `X-GitHub-Delivery`, the Stripe event `id` from the verified body); for Slack slash commands, the inbound-idempotency digest of the full verified request |
  | Actions mode | the run id |
  | queue | `job.id` |
  | schedule | the fire id (schedule, dueAt) |
  | unverified webhook | **none**: singleton 1.1 semantics, and the card says so |

  - Authenticated replay stores only the secret's logical name and re-checks the binding on use.
  - Protocol 1.1 rows migrate with an empty instance key.
  - The card's ON RETRY line names the key.
  - `conformance/durable/` covers each entrypoint kind.
- **Checkpoints stay namespace-scoped named flags.** **Held position:** the completeness critic proposed an instance column. Checkpoints are named store flags by spec, and a hidden instance scope would change the meaning of existing programs. Per-instance progress belongs to `workflow fn` after 1.0.
- **Call-site Core hash is recorded, not keyed.** **Held position:** the safety critic proposed keying it, so that changed code misses instead of trapping. Keying it would turn every code edit into duplicate external effects. With conflicts already returned as `Err`, nothing traps.

### 8.3 The card: enforcement tags, rendering and widening

- **Tags.** Every card/1 line carries one of three tags:
  - `enforced`: checked by the runtime on every call. This covers REACHES, SECRETS pairs, INBOUND verification, attenuation, AI budgets and token scopes.
  - `re-derived`: recomputed from the artifact's imports and manifest at load. This covers effects and requirements.
  - `attested`: a compiler claim stored in metadata. This covers UNTRUSTED, DECIDES, EXPOSES, ON RETRY, REPEATS and ENDORSED.

  The card is bound to the artifact's BLAKE3 digest. `serve` and `krit-action` recompute the re-derivable sections and refuse to run on a mismatch.
- **INBOUND is mandatory.** It reads either `verified <scheme>`, `guest-checked (not enforced)`, or `unverified`. `serve` refuses a non-loopback unverified webhook without `--allow-unverified` (K5008), which also appears on the card.
- **Rendering.**
  - Every source-derived string (`trusted()` reasons, `///` docs, literals) renders verbatim inside code spans.
  - HTML and markdown are escaped.
  - `@`-mentions are stripped and lengths are capped.
  - MCP tool descriptions come from a reviewed manifest field (§9.4), never from doc comments.
- **Widening, for `card --against`.** Any change to REACHES, SECRETS pairs, INBOUND, ENDORSED, DECIDES sinks, EXPOSES, ON RETRY, REPEATS or AI budgets counts as widening. Auto-approval requires no widening, no new endorsements and no K9011, K9007 or K9012 findings. A new or changed `trusted()` call is never auto-approvable. The test suite has at least 40 cases, including one per widening kind.

### 8.4 Not doing

- "AI-native" branding without numbers
- guest effect handlers (Wasm stack switching is still Phase 3)
- SMT in the default checker (it would make `check` nondeterministic)
- a wholesale Wasm GC or WASI 0.3 retarget
- async, threads or actors in the language
- MCP or A2A draft churn in syntax
- label-polymorphic HM
- natural-language programs
- a custom VM
- macros
- a content-addressed source of truth
- a proprietary model
- a CLI flag that enables private egress

## 9. AI-authoring loop

### 9.1 Diagnostics schema 2

Schema 2 ships in 0.3 behind a flag and becomes the default in 0.4.1. Schema 1 stays available until 1.0.

**Fields:**
- `severity`
- primary and secondary `labels`
- `notes`, `help`
- `expected` and `found`
- `fixes: [{title, applicability: machine|maybe|placeholder, edits}]`
- `docs`

These replace the empty arrays hard-coded at diagnostic.rs:45-66.

**Behaviour:**
- Output shows source excerpts.
- All errors are reported, capped at 64 per file, with cascades suppressed.
- `krit fix [--code] [--dry-run]` applies only `machine` fixes and then formats. It is deterministic and is the same mechanism as edition migration, so no LLM ever rewrites source.
- **Authority invariant (§3.3).** Did-you-mean on a resource literal (`"githb-token"` → `"github-token"`) is `maybe`. So are `a[i]` → `list_get` (returns an Option) and `s.len()` → `string_len` (counts scalars, not bytes; `string_byte_len` is offered as the alternative). `krit fix` recomputes the permission report or card and aborts on any change. A test suite covers every fix producer.

### 9.2 One code allocation

All new codes below are free in spec/DIAGNOSTICS.md (verified).

| Code | Meaning | Rel. |
|---|---|---|
| K1005 | nesting depth limit | 0.2.1 |
| K4010 | interpreter recursion limit | 0.2.1 |
| K5006 | secret injected to an origin it is not bound to | 0.2.1 |
| K6003 | `[dependencies]` declared, resolution unsupported | 0.2.1 |
| K1101–K1112 | habit codes, syntax only, each with a fix: `return`; `for`/`while`; assignment/`mut`/`var`/`const`; `struct`/`class`/`impl`/`trait`; `import`; `async`/`await`; `try`/`catch`/`throw`; `a[i]`; `(x) => e`; ternary; `switch`/`case`; `else { return … }` | 0.3 |
| K2001 (extended) | did-you-mean suggestions (`maybe` for resource literals); `null` → `None` | 0.3 |
| K3005 (extended) | missing arms listed verbatim; catch-all `_ =>` rejected | 0.3 |
| K2003 | `_` read (warning in 2026, error in 2027) | 0.3 |
| K3011 | method call `s.len()` → `string_len(s)` (`maybe`) | 0.3 |
| K3012 / K3013 | `?` not allowed here / error types differ | 0.3 |
| K3014 | let-else not complementary, or not a body statement | 0.3 |
| K3015 / K3016 | bad interpolation hole / named-construction field error | 0.3 |
| K4011 | interpreter step budget exhausted | 0.3 |
| K5203 (changed) | replay conflict, now returned as `Err` | 0.3 |
| K9001–K9005 | unused binding; discarded Result; unused grant or secret pair; depth >3; `${x}` lookalike | 0.3 |
| K9013 | hole or `+` operand flowing into an `HttpRequest` path or query, not via `url_path_segment`, `url_query` or `int_to_string` | 0.3 |
| K9014 | hole inside a JSON- or form-shaped literal flowing to a body | 0.3 |
| K3017 / K3018 | polymorphic recursion (0.6) / `json_parse` target undetermined (0.4) | — |
| K3019 | type is not JSON-codable (`Option<Option<_>>`, `Option<Unit>`, functions, Secret, transactions) | 0.4 |
| K3020 | callback to a higher-order stdlib function must be pure (lifted at 0.6) | 0.4 |
| K7005 / K7006 / K7007 | build with `todo()` / instantiation budget exceeded / effect unavailable in the deployment profile (for example `actions`) | 0.4 / 0.4 / 0.4 |
| K9008 / K9009 | `json_decode` or `json_encode` deprecated / stdlib-prefix name or `todo` shadowed | 0.4 |
| K9010 | unstable feature recorded in artifact | 0.4 |
| K8108 | repair budget exhausted | 0.4.1 |
| K9100 | typed-hole report (info) | 0.4.1 |
| K3022 | result of a must-use host check discarded | 0.5 |
| K5004 / K5005 | inbound verification failed / outside an attenuated grant | 0.5 / 0.4 |
| K5008 | non-loopback unverified webhook refused | 0.5 |
| K5107 | AI adapter budget exceeded (returned as `Err`) | 0.5 |
| K9015 | user-written recursive list builder | 0.6 |
| K3021 | `trusted` reason is not a literal | 0.7 |
| K9006 / K9007 | weak `trusted` reason / unkeyed non-idempotent retried effect | 0.7 |
| K9011 / K9012 | untrusted value reaches a privileged sink / authenticated-read data reaches a write body | 0.7 |
| K3024 | reserved: unkeyed effect inside `workflow` | beyond |

`spec/DIAGNOSTICS.md` gains a new range: `K9000–K9999 | Lints (warning by default; severity configurable; K9013/K9014 are errors in apps and deploy mode)`. `.method()`, `null` and catch-all arms stay in their true categories; today they produce K3004, K2001 and K1003.

### 9.3 Holes, grammar and the language card

- **Typed holes (0.4.1).** `todo("label")` has type `'a` and no effect. `check --holes --json`, also available as an LSP inlay and an MCP tool, reports:
  - the expected type;
  - in-scope bindings ranked by fit;
  - matching stdlib functions;
  - unused grants.

  Build rejects holes with K7005. It is a builtin rather than `?name`, which would collide with `?`.
- **Grammar (0.4.1).** `spec/grammar/krit.ebnf` is the source for `krit grammar --format ebnf|gbnf|lark` (per edition) and for tree-sitter. CI checks that it accepts every conformance and example file and rejects every K1xxx case. Type-aware constrained decoding is post-1.0 research.
- **Language card (0.3).** It is registry-generated and at most 8k tokens; today's prompt is 21,780 B and mentions only K5003, K7001 and K7002. It contains:
  - a grammar summary and every signature;
  - a "Krit is not Rust/TypeScript" table mirroring K11xx;
  - at least 15 CI-checked examples;
  - a diagnostics cheat sheet.

  Commands: `krit prompt --format card|llms-txt --budget`, `--examples <tag>`.
- **`krit new` (0.4.1)** writes AGENTS.md/CLAUDE.md (check with JSON diagnostics, apply `krit fix`, never widen the manifest without asking) and a `.mcp.json` naming an absolute or version-pinned binary, because most Krit will be written in Claude Code, Cursor or Codex.

### 9.4 Repair loop and MCP hardening

**`assist suggest --repair-rounds N` (0.4.1):**
1. Round 0 applies machine fixes at no cost, under the authority invariant.
2. Each later round runs fmt → `check --target` → build → test, and feeds back the diagnostics, fixes and hole contexts.
3. It stops when everything passes, or at K8108.
4. Acceptance stays `accept --reviewed` (K8106).

**`krit mcp` (0.4.1, stdio, `crates/krit-mcp`):**
- It exposes `check`, `build`, `test`, `fix --dry-run`, `explain`, `card`, `fmt`, `holes`, `prompt` and `metrics`.
- All tools are deterministic and offline. Every path is confined to the workspace root.
- It never exposes `--live` or `--record`, never accepts proposals and never edits manifests.

**`krit serve --mcp` (0.7):**
- It binds to loopback by default and validates the `Origin` header to block DNS rebinding.
- Any non-loopback bind requires MCP authorization (bearer or OAuth per 2025-11-25).
- Tool descriptions come from a reviewed manifest field `[tools.<name>] description`, capped at 280 characters and shown on the card. They never come from `///` comments, which models wrote and which would become instructions to the calling model.

### 9.5 KritBench

**Tasks** (`bench/tasks/<tier>/<id>/`): intent, expected minimal manifest, fixtures, cassettes (from 0.3), hidden tests and a reference solution.

**Tiers:** T1 pure, T2 handlers, T3 integration agents (seeded with beachhead bot tasks), T4 repair, T5 authority exactness.

**Versions:**

| Version | Contents and scoring |
|---|---|
| v0 (0.2.2) | 60 tasks; **static scoring only**: check@1, build@1 and perm-exact, plus func@1 on T1 via `krit run`. T3 is near 0 in 0.2 and is reported, not targeted |
| 0.3 | func@1 baseline from stored 0.2.2-era outputs run through the fixture runner; repair@3 baseline with the 0.2.2 binary in the loop |
| v1 (0.4.1) | 160 tasks; the scoring set is **frozen**. B is re-baselined on v1 with the 0.2.2 toolchain. Rotated private tasks are reported separately and never enter a gate |
| v2 (1.0) | at least 200 tasks |

**Splits.** 70% public and 30% private, with a canary string. Partner-derived tasks go only to the private split, after redaction and consent.

**Conditions:**

| Condition | Setup |
|---|---|
| P0 | card |
| P1 | card + retrieval |
| P2 | P1 + grammar decoding, open-weight models |
| P3 | holes first |
| TS twin | 40 T2/T3 tasks, from 0.6 |

**Models and sampling.**
- Two open-weight models of 35B or smaller, pinned by weight hash and archived, are the **gate models**.
- Three vendors' flagships plus one cheap tier are **product-metric models**.
- Greedy decoding, plus n=5 at T=0.8 for unbiased pass@k.

**Release gates.**
- A full run is published with 95% intervals (bootstrap over tasks).
- The release shows **no statistically significant regression** (paired bootstrap, α = 0.05) against the previous release on the same frozen set and the same gate models.
- Why not absolute deltas: with 24 T1+T2 tasks, a 15-point delta is about 3.6 tasks, which is inside noise. At 160 tasks the 95% half-width is about ±7.7 points.
- **Retirement rule:** when a vendor model retires, rerun both the previous and the current release on its successor. Open-weight gate models never retire.
- **Safety precedence (§3.4):** changes motivated by charter 5, 10 or 11 report their deltas and never block.

**Scores.** check@1, build@1, func@1, repair@3, perm-exact (over- and under-request reported separately), per-code repair rate, and tokens per solved task. Results are reported for all tasks and for the tasks expressible in that version.

**Infrastructure.**
- Harness `crates/krit-eval`, which reaches fixtures through `loopback_for_tests()` in Rust only.
- Method in `spec/BENCHMARK.md`; results in `bench/results/<version>/<model-id>.jsonl`.
- A full run per minor release, and a weekly cheap-model smoke run that alerts on regressions beyond the interval.
- Absolute targets in §11 are product goals and recalibrate **once**, after the v1 re-baseline.
- **Budget (§12):** if funding is not secured by 0.5, the bench drops to 3 models with n=3.

### 9.6 Targets and reopen triggers

The targets are in §11. For the top 20 codes, the per-code repair rate should be at least 85% at 0.4.1 and at least 90% from 0.6; this is a product goal.

**Reopen triggers:**
- If K1101 or K1102 is still a top-3 failure code at 0.4.1 after fix-its, reopen a restricted-comprehension RFC.
- If K1111 is a top-3 failure code, reopen literal patterns.
- If perm-exact stalls below 90%, revisit labelled host arguments.

**No reopen trigger may relax a safety rule:** literal resources, no catch-all arms, identical error types for `?`, no implicit conversion, `uses` required on library `pub fn`, and secret binding.

## 10. Developer experience and ecosystem

- **Tests:**
  - fixture cases and the cassette format (0.3);
  - `test` blocks on both executors with `expect_eq` diffs (0.4);
  - recording through the real runtime with redaction and a test-only verifier (0.5);
  - definition-hash caching (0.6);
  - `sim` and `eval` (0.7);
  - `spec/TESTING.md`.
- **Operate (0.5):** `krit inspect`, `krit dlq redrive`, Retry-After and `QueueError`, OTel spans, audit events, `/healthz`, drain, and multi-artifact `serve`.
- **REPL and playground:** `krit repl` (0.6); a client-side browser playground with URL-fragment sharing (0.7, cuttable).
- **LSP:**
  - 0.3: every diagnostic including K7002, quick fixes, and the private builtin map deleted;
  - 0.4.1: holes and defaults on hover;
  - 0.6, v2: error tolerance, navigation, rename, signature help, type and effect inlays, semantic tokens, `?`/let-else exit highlighting and a card lens.
- **Editors and docs:**
  - tree-sitter and VS Code (Marketplace and Open VSX) at 0.4.1, reused by Zed, Neovim and Helix;
  - tour and cookbook (0.4–0.4.1); `krit doc` (0.6); book and diagnostics index (1.0).
- **Packages:**
  - fail closed (0.2.1);
  - modules, path dependencies, `krit.lock` and a content store (0.6);
  - git dependencies with per-dependency cards, `krit vendor` and typo checks (1.0);
  - only the root manifest grants authority, and there is no public registry before 1.0.
- **CI and supply chain:**

  | Release | Additions |
  |---|---|
  | 0.2.1 | 3-OS tests, deny and audit |
  | 0.2.2 | nightly fuzzing, `check-build-agreement` |
  | 0.3 | differential job; GitHub artifact attestations for binaries and the action |
  | 0.4 | Sigstore signing, SBOMs, `wasm-tools validate` |
  | 0.4.1 | reproducible-build check |
  | 0.5 | SLSA provenance |
  | 1.0 | notarized binaries |
- **Platforms:** Windows is supported for authoring commands. Host mode with durable state or secrets refuses to start until owner-only ACL checks exist, which happens only if a partner pulls it.
- **Governance:**
  - `rfcs/`;
  - SECURITY.md (0.2.1);
  - CODEOWNERS for `wit/`, the sidecar, the authority snapshot and verify schemes;
  - `docs/roadmap.md` replacing the "complete" `docs/agent-roadmap.md`, and re-dated at each minor release;
  - a CHANGELOG entry per normative change;
  - architecture docs refreshed each milestone.

## 11. Validation and metrics

Every row comes from a script: `krit metrics`, `krit-eval`, `benchmarks/`, the conformance harness, the fuzz dashboard, the upgrade harness, or `scripts/dummy-capability.sh`. CI regenerates `docs/status.md` for each release. "B" is the 0.2.2 toolchain baseline. The 0.2.x column means "by 0.2.2".

**Gates:**

| Metric | 0.2 | 0.2.x | 0.3 | 0.4 | 0.4.1 | 0.5 | 0.6 | 0.7 | 1.0 |
|---|---|---|---|---|---|---|---|---|---|
| Eligible corpus deployable (named) | 2: 03b, 06 | 3: +07 | 3 | 6: +01 (Actions), 05, 08 | 6 | 7: +03 | 8: +04 | 8 | 8 + all apps |
| Silent check-pass/build-fail | 7/8 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 |
| K7002 reported at `1:1` | common | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 |
| K7001/K7002 on verified Core | common | reported | reported | 0 | 0 | 0 | 0 | 0 | 0 |
| Interpreter oracle / differential | none | string cases vs evaluator | = spec on every run case; webhook fixtures agree | 0 in 48 CPU-h from generator | = | = | = | = | ≥1,000 h cumulative |
| Fuzz | aborts at ~4.7k frames | 0 stack aborts; every crash triaged | = | = | = | = | = | = | = |
| Conformance run/check/format/build | 25/29/3/0 | 30/40/3/5 | 60/90/15/10 | 150/150/30/40 | = | 180/170/35/60 + 20 durable | 220/200/40/70 | 260/220/45/80 | ≥500 |
| Multi-error recall | first only | — | ≥90% | ≥95% | = | = | = | = | = |
| Diagnostics with a machine fix | 0% | — | ≥30% | ≥45% | ≥60% | ≥65% | ≥70% | ≥75% | ≥80% |
| Machine fixes that changed authority | n/a | — | 0 | 0 | 0 | 0 | 0 | 0 | 0 |
| webhook-agent lines/depth; max example depth | 79/15; 15 | = | ≤50/≤3; ≤3 | ≤47/≤3 | = | = | = | = | = |
| Hand-edited files per new capability | ~20 | ~20 | ~20 | ≤10 | = | ≤8 | ≤8 | ≤8 | ≤8 |
| Orphaned durable records across upgrade | all | — | 0 (migrate) | 0 (no step) | 0 | 0 (version bump) | 0 | 0 | 0 |
| Duplicate effects, 8 workers / one store | n/a | — | — | — | — | 0 | 0 | 0 | 0 |
| Swapped-secret probe | builds and runs | host refuses | build fails | = | = | = | = | = | = |
| Path-traversal cases refused | no | yes | yes | yes + attenuation | = | = | = | = | = |
| KritBench full run, no significant regression (gate models) | — | v0 static B | v0 + func/repair B | v0 | v1 B′ | v1 | v1 | v1 | v2 |
| Language card | 21,780 B | = | ≤8k tok | ≤8k | ≤8k | ≤8k | ≤8k | ≤8k | ≤8k |
| Warm invoke p50 | compile per call | — | — | — | measured | ≤2 ms | ≤2 ms | ≤1.5 ms | ≤1 ms |
| Webhook ack p95 during 20 s AI call | blocked | — | — | — | — | <300 ms | = | = | = |
| `serve` throughput, 8 cores | sequential | — | — | — | — | ≥500 rps | = | = | ≥1,000 rps |
| webhook-agent component | 8,440 B | = | ≤16 KiB | ≤64 KiB | = | = | = | = | = |
| DLQ redrive without SQL | no | no | no | no | no | yes | = | = | = |
| AI budget overspend in 10k-payload soak | n/a | — | — | — | — | 0 | 0 | 0 | 0 |
| Injection + exfiltration escapes / endorsements per 100 lines | — | — | — | — | — | — | — | 0 of ≥50 / ≤2 | 0 of ≥100 / ≤2 |
| Wasmtime on supported LTS | no | yes | yes | yes | yes | yes | yes | yes | next LTS |

**Product goals** (not gates; LLM targets are provisional until the v1 recalibration):

| Metric | 0.2.x | 0.3 | 0.4 | 0.4.1 | 0.5 | 0.6 | 0.7 | 1.0 |
|---|---|---|---|---|---|---|---|---|
| *check@1 (frontier, P1)* | B | B+10 | B+20 | B+25 | ≥70% | ≥75% | ≥78% | ≥80% |
| *func@1* | T1 only | B | ≥40% | ≥45% | ≥55% | ≥60% | ≥63% | ≥65%, within 10 of TS |
| *repair@3* | — | B | ≥70% | ≥85% | ≥88% | ≥90% | ≥93% | ≥95% |
| *perm-exact* | B | ≥75% | ≥80% | ≥85% | ≥88% | ≥90% | ≥93% | ≥95% |
| *≤35B open models, check@1 with grammar* | — | — | — | measured | ≥50% | ≥60% | ≥65% | ≥70% |
| *Reviewer speed/accuracy vs TS twin* | unmeasured | — | — | pilot n≥5 | — | — | — | ≥2×, ≥95%, n≥8 (published) |
| *Partners / bots in production* | 3 pilots | 5 LOIs | 10/15 | 10/20 | 12/30 | 12/40 | 15/60 | 25 orgs/100 |
| *`krit new` → live bot* | — | — | ≤30 min | = | ≤20 min | = | = | ≤15 min |
| *Tokens and $ per bot-day vs budget* | — | — | — | — | reported | = | = | = |
| *Secret / private-data exposures* | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 |

**Reviewer-study protocol** (`spec/BENCHMARK.md`). 8–12 engineers review 6 counterbalanced programs, either Krit with its card or a TypeScript twin. They answer:
- what the program can reach;
- which secret goes where;
- what happens on upstream failure;
- whether a retry duplicates an effect;
- whether untrusted text reaches a sink;
- whether a seeded over-grant exists.

Time and accuracy are published next to KritBench. A miss at 1.0 triggers a design-review RFC.

## 12. Risks, staffing, funding and non-goals

**Staffing and throughput.**
- At 0.8 effective pw per working week and 42 working weeks a year, a solo maintainer delivers 33.6 pw per year. The roadmap's ~198 pw then puts R1 in Feb 2029 and 1.0 in Aug 2032.
- **Top non-technical priority: fund a second maintainer by Mar 2027.** That moves R1 to Apr 2028 and 1.0 to H1 2030. Hiring at the 0.4 exit instead gives 1.0 around Q2 2031.
- **Calibration.** At the 0.2.2 exit, estimated pw is compared with actual calendar time. If the measured rate differs by more than 20%, every date is re-derived and `docs/roadmap.md` is republished. AI agents may beat the 0.8 assumption, and the plan dates only measured throughput.
- **Maintenance tax (20%):** advisories within 7 days, CI, partner office hours (one biweekly group call plus async, not per-partner calls), the weekly bench smoke run, release signing and notarization.

**Funding lines** (owner: Akshay; all figures are estimates to confirm):

| Line | Estimate | Candidate sources | Deadline / fallback |
|---|---|---|---|
| Second maintainer | ~$150–220k per year fully loaded, depending on location | GitHub Secure Open Source Fund, Sovereign Tech Agency, Alpha-Omega (OpenSSF), NLnet NGI Zero (eligibility unverified), paid partner support contracts, corporate sponsorship (Krit is Apache-2.0) | applications in the first 90 days; decision by Mar 2027 |
| External audit (runtime host boundary, `krit-wasm` validator, secret handling, verify schemes, state commit) | ~$60–120k for a 3–4 engineer-week engagement (unverified quotes) | same sources plus partner co-funding | funding source by 0.5, booked at 0.6; if unfunded, scope narrows to the host boundary and validator |
| Bench API spend | ~$1–2k per full release run, ~$1k per year for smoke runs, plus GPU rental for open-weight gate models | sponsorship and credits programs | if unfunded by 0.5: 3 models, n=3 |

**Risk table:**

| Risk | Mitigation | Re-plan trigger |
|---|---|---|
| Solo throughput makes the beachhead late | §2.0 pilots on 0.2.2; a 0.3 leading indicator; 0.4 split; second-FTE funding; calibration | measured rate below 0.6 pw/week through the first half of 0.3: apply the 0.3 cuts and re-date |
| Upgrades orphan state or duplicate effects | compatibility window (0.2.1); protocol 1.1 and migrate (0.3); upgrade gates every release | any orphaned record or duplicate in an upgrade gate blocks the release |
| Secret misdirection | host binding (0.2.1), compiler pairs (0.3), downscoped tokens (0.5) | any K5006 reached at run time on an artifact built after 0.3 is a compiler bug |
| Injection through built strings | host path hardening (0.2.2), K9013/K9014 (0.3), attenuation (0.4), provenance (0.7) | an injection-suite escape blocks the release |
| Private-data exfiltration through granted writes | docs disclaim it until 0.7; EXPOSES and `exposures = "deny"` (0.7) | a partner incident: re-prioritize EXPOSES |
| Concurrency duplicates effects | single-writer stores; durable conformance suite | any duplicate in the concurrent gate |
| JSON parser differentials | `spec/JSON.md`, one generating table, three-way fuzzing | any disagreement blocks the release |
| Backend rewrite breaks programs | interpreter as oracle; generator; golden digests; `--backend legacy`; R3 gated from 0.2.1 | any differential disagreement blocks the release |
| Supply chain for `krit-action` | two-job split, SHA pinning, embedded hashes, attestations (0.3), Sigstore (0.4) | an advisory or pwn-request finding blocks the release |
| A single generated registry weakens all layers at once | hand-written authority snapshot, security RFC and second reviewer, per-row negative tests | snapshot drift fails CI |
| Benchmark noise or LLM veto over safety | statistical gates on open-weight models; safety precedence | — |
| Models write Krit poorly | mainstream spellings, habit codes, the card, GBNF, `krit mcp`, a public benchmark | the check@1 product goal is missed for 2 releases: design-review RFC |
| Edition churn | additive first; three breaking groups; prefixes rather than names; `krit fix` | partner migration below 100% for groups 1 and 3 |
| Provenance noise or false confidence | warnings first; choice sets on the card; narrow verified-scalar rule; endorsement budget | more than 2 endorsements per 100 lines |
| Incumbents (gh-aw, Code Mode, Wassette) absorb the niche | complementary positioning (safe-output executor that runs inside their sandboxes); compete on the pre-deploy card | a platform ships per-program authority diffs |
| Runtime security | LTS policy, advisories, fuzz hours, external audit, no production claims before 1.0 | an escape-class finding blocks the release |
| AI-written code hides defects (77.6k lines in about a week) | refactor-first ordering, property and differential tests, the audit | crash or bug rate rises |
| Funding falls through | cuts in §4; bench at 3 models; narrowed audit | no second maintainer by the end of 0.3: publish solo dates and keep the kill/pivot checkpoint |

**Non-goals through 1.0:**
- general-purpose positioning
- floats
- shell, filesystem, process or environment authority
- loops, mutation, `return`, exceptions, traits, macros
- guest async, threads or effect handlers
- SMT
- natural-language programs
- distributed exactly-once delivery or multi-host durability
- a public registry
- a custom VM
- hosted compute
- a proprietary model
- LLM rewriting of accepted source
- CLI flags that enable private egress
- durable state in Actions mode

## 13. First 90 days (12 Oct 2026 – 10 Jan 2027, solo)

A second maintainer cannot start before Mar 2027, so this plan is solo by construction. Capacity is 0.8 pw per working week; the two holiday weeks (Dec 21 – Jan 1) give 0.2 pw each. Partner outreach and funding applications come out of the 20% maintenance share.

| Week | Tasks (pw) | Done when |
|---|---|---|
| 1 (Oct 12) | Wasmtime 47.0.4 → 48 LTS plus the wasm-tools crates (Cargo.toml:60-76); fix `krit-runtime/src/{lib.rs,bindings.rs,limits.rs}` and `krit-wasm/src/validation.rs` (0.75). SECURITY.md and disclosure address (0.05). *Maintenance:* list of 30 partner candidates | workspace builds on 48 |
| 2 | Finish the bump; re-baseline golden digests; update `technical-design.md:296-300` and `WASM-SANDBOX.md:420-426` (0.5). `deny.toml`, cargo-audit, macOS and Windows test jobs in `ci.yml`; Windows host-mode refusal (`host_config.rs:1668`) and its doc (0.3) | 629 tests green on 3 OSes; advisories clean |
| 3 | K1005 in `parser.rs`, `analysis.rs`, `core.rs` and `formatter.rs`; interim K4010 in `evaluator.rs`; the K9 range and new codes in `spec/DIAGNOSTICS.md` (0.8). *Maintenance:* first funding applications | 100k recursion → K4010; 2k nesting → K1005 |
| 4 | `run` analyzes first (`main.rs:1176-1224`, `lib.rs:43-46`); move the 11 ill-typed run cases (0.5). K6003 (`krit-package/src/lib.rs:160-166`), `spec/PACKAGES.md` and a case (0.25) | `run` refuses every check failure |
| 5 | Artifact compatibility (`validation.rs:503-504, 543-548` → supported sets) and `conformance/build/compat` with a checked-in 0.2.0 artifact (0.5). Single-source `PROMPT_PACK_VERSION` (`protocol.rs:7`); host default User-Agent; K7002 policy-name and `_` config-key papercuts; `docs/roadmap.md` and `rfcs/0000-template.md` (0.3) | the 0.2.0 artifact runs on the new runtime |
| 6 | Secret→origin binding: manifest `[secrets.<name>] origins`; the Secret resource carries its logical name; `network.rs:249-260` refuses with K5006; `krit permissions` prints bindings and UNBOUND (0.8) | the swapped-token probe is refused |
| 7 (Nov 23) | Binding tests and release notes (0.2); **tag 0.2.1**. `fuzz/` with 6 targets and a nightly job (0.6) | 0.2.1 gate met |
| 8 | Finish the fuzz targets (0.4). `support.rs` returns `Vec<Diagnostic>`; DCE before `support.rs:76-81` (0.4) | fuzzing runs nightly |
| 9 | Core-op spans on all 17 K7002 sites in `agent_compiler.rs`; `check --target` runs full emission with the output discarded (0.8) | no K7002 at `1:1` on the sketches |
| 10 | `build --diagnostic-format json`; `check-build-agreement` job over sketches, examples and conformance (0.8). *Maintenance:* at least 8 partner discovery calls | check = build on every file |
| 11–12 (holiday) | Finish agreement CI and triage fuzz findings (0.4) | each crash fixed or filed with a reproducer |
| 13 (Jan 4) | `krit-wasm/src/rt/strings.rs`: equality, ordering, concatenation and byte length as plain-loop helpers (`support.rs:419-430` unblocked), differentially tested against `evaluator.rs` (0.8) | string conformance cases build and agree |

**Day-90 checkpoint (Jan 10):**
- 0.2.1 is shipped.
- Fuzzing has run nightly for 6 weeks.
- `check --target` is exact on `main`.
- The string runtime is in progress.
- At least 3 letters of intent are signed and funding applications are filed.

**Weeks 14–17 then finish 0.2.2:**
- Wasm `json_decode` unescaping (`agent_compiler.rs:1200-1287`);
- host path hardening (`webhook.rs:115-140`, `CURLOPT_PATH_AS_IS`);
- R7 under `apps-gate` through the krit-eval loopback harness;
- KritBench v0 (60 tasks, `spec/BENCHMARK.md`, static `crates/krit-eval`) and the baseline with 95% intervals.

**Tag 0.2.2 around Feb 5, 2027.** Throughput is then calibrated (§12), and RFC-0001 to RFC-0006 come before NodeIds.

**Held position:** the completeness critic wanted 0.2.2 in mid-December, with strings on a second track. The order is adopted, but a solo maintainer cannot hit that date.

If a second maintainer is funded, they own the second 0.3 track from day one: RFC-0006, FakeHost, protocol 1.1, `krit-action` v0, then the LIR design.

**Every week, regardless of staffing:** each change ships with conformance cases, fuzzers run nightly, and CI regenerates the §11 dashboard.

---

## Appendix: Decision log

| # | Decision | Alternatives considered | Why |
|---|---|---|---|
| 1 | Foundations-first spine: one Core semantics, then a total backend, then differentiators | beachhead-first; differentiation-first; ai-authoring-first | A card or provenance claim is trustworthy only over a single Core. All reviewers kept this spine. |
| 2 | Split the safety release into 0.2.1 (5 pw) and 0.2.2 (7 pw) | one 4-pw release | Its scope was 2–3× the estimate, and the fuzz gate was open-ended. |
| 3 | Guest strings in 0.2.2, inside today's emitter | last item of 0.3; wait for LIR at 0.4 | They do not depend on NodeIds or the interpreter, they unblock R7 and `method == "POST"` about a year earlier solo, and the primitives carry into RFC-0006. |
| 4 | Artifact loading by supported sets of schema, policy, WIT version and edition, with a two-minor window | exact compiler-version match | Every release rejected deployed artifacts (`validation.rs:503,543`). |
| 5 | Durable identity: namespace in protocol 1.1 (0.3), plus `krit state migrate`; host-derived instance key in protocol 2 (0.5) | keep the digest until 0.5; guest-supplied keys | The 0.4 backend swap would re-key everything; guest keys allow delivery-id squatting. |
| 6 | Call-site Core hash recorded, not keyed | key on it (safety critic) | Keying turns every edit into duplicate external effects; conflicts already return `Err`. |
| 7 | Checkpoints stay namespace-scoped named flags | instance column (completeness critic) | A hidden scope would change existing programs; per-instance progress waits for `workflow fn`. |
| 8 | Secret→origin binding: host 0.2.1, compiler 0.3, inject kinds and downscoped App tokens 0.5 | compiler-only; status quo | The beachhead pitch depends on it, and host enforcement survives forged metadata. |
| 9 | Single writer per durable store in 0.5; per-key versions only if pulled | per-key versions before the pool (feasibility critic) | Same no-duplicate guarantee at bot scale for far less change to `krit-state`. |
| 10 | Desugaring pass right after NodeIds, in parallel with the sink and registry | strict serial order with sugar last | One AST-to-AST pass feeds every consumer, nothing is built twice, and the headline win no longer comes last. |
| 11 | Interpreter step budget (K4011) and deadline; exhaustion excluded from value agreement | depth limit only | Tail calls never hit a depth limit and would hang the tools. |
| 12 | FakeHost implements a generated trait; durable ops wrap real `krit-state` and `krit-database` | fixture fakes for all 30 ops | Avoids a second implementation of durable semantics, and HostService reuses the trait. |
| 13 | 0.3 differential gate is oracle-versus-spec; the generator and 48 CPU-h gate move to 0.4 | generator gate at 0.3 | The deployable subset at 0.3 is too small for the gate to mean anything. |
| 14 | `check --target` uses full emission in the CLI (0.2.2) and a CI-proven mirror for the LSP (0.3) | a pre-check mirror only; full emission in the LSP | Exact by construction, while respecting the krit-lsp dependency ban. |
| 15 | RFC-0006 picks the guest runtime in 0.3, with an O(n) allocation criterion | hand-written wasm-encoder for everything; defer to 0.4 | An assembly-level stdlib is too costly, and the budget must exist before 0.4. |
| 16 | List builders stay native intrinsics through 1.0 | Krit prelude at 0.6 | The bump arena never frees, and `(ptr, len)` lists make recursive building O(n²). |
| 17 | Split 0.4 into 0.4 (R1 deploys) and 0.4.1 (authoring loop) | one 45–50 pw release | R1 deploys about a quarter earlier, and the tooling becomes cuttable. |
| 18 | `json_parse` and `json_stringify`; `json_encode` frozen; `json_decode` removed in 2027; legacy Option shapes decoded through 1.0 | change `json_encode` by edition; retype `json_decode` | One text means one thing, and persisted payloads must keep decoding. |
| 19 | Declared and anonymous records encode identically | declared types encode Option as null (completeness critic) | Structural identity; K9008 steers code to `json_stringify`. |
| 20 | `spec/JSON.md` I-JSON profile; one table generates guest and host; `ai_json` denies unknown fields while `json_parse` ignores them | leave the format underspecified | Prevents parser differentials; provider payloads differ from model output. |
| 21 | Floats are lexed, skipped in ignored fields, and kept as source text in String fields; `int_parse_scaled`; host `score_bp`; `ai_embed` returns an opaque handle | add floats | Charter and determinism; GitHub's `score: 1.0` would otherwise break R2. |
| 22 | String-valued enums, one `Other(String) = ..` fallback, wire names | decode into String and compare with `==`; reject | Real payload vocabularies, while DECIDES stays closed when there is no fallback. |
| 23 | Edition 2027: reserve prefixes, not names; `todo` shadowable; edition from manifest or `--edition` | reserve every stdlib name | The stdlib grows until 1.0, and reserving names would make each addition breaking. |
| 24 | 0-CFA provenance; exceptions limited to provider-assigned Int, Bool or enum fields; no origin endorsement | linear dataflow; origin-level endorsement | Closures are first-class; HMAC proves the sender, not the content. |
| 25 | EXPOSES for authenticated-read → write flows in 0.7, disclaimed until then | confidentiality after 1.0 | A repo bot's worst case is private data posted to a public sink. |
| 26 | Card lines tagged enforced, re-derived or attested; forged metadata refused; escaping renderer | one "proven" card | Flow sections are compiler claims only, and PR comments are an injection surface. |
| 27 | `card --against` widening covers flows, endorsements and retry; `trusted()` is never auto-approved | reach-only comparison | Otherwise model output gains authority. |
| 28 | Machine fixes never change authority; `krit fix` aborts on any card change | trust fix authors | Did-you-mean on a resource literal silently changes reach. |
| 29 | Host path hardening and K9013/K9014 in the releases that enable string building; minimal attenuation before deploy mode | wait for 0.7 provenance | String building opens path injection from 0.2.2. |
| 30 | Two-job `krit-action`, SHA pinning and attestations at 0.3; Sigstore and SBOMs at 0.4 | a single job; signing at 0.7 | Avoids the pwn-request shape; the beachhead story is tj-actions. |
| 31 | Actions mode requires explicit workflow `permissions:`; webhook and schedule only, with no store (K7007) | origin-only least authority; `actions/cache` persistence | GITHUB_TOKEN breadth is set outside Krit; cache loss would falsify durability claims. |
| 32 | Release gates are statistical non-regression on pinned open-weight models; absolute targets are product goals; safety changes are exempt | absolute deltas on pinned vendor models | Small task sets are noisy, vendor models retire, and LLM results must not veto safety. |
| 33 | Static KritBench v0; func and repair baselines at 0.3; cassettes defined at 0.3; B re-baselined on v1 | full v0 at 0.2.1 | No fixture runner existed, and gates must compare like with like. |
| 34 | No CLI flag ever enables private egress; tests use `loopback_for_tests()` in Rust | a `--allow-loopback` flag | Fail closed. |
| 35 | Record and live modes only through the real runtime, with redaction; test verifier behind a compile-time feature | record through the interpreter | Keeps secrets out of cassettes and verification bypasses out of `serve`. |
| 36 | MCP hardening: loopback default, Origin check, auth for non-loopback binds, manifest-sourced descriptions, workspace confinement | basic stdio and HTTP only | DNS rebinding and tool poisoning. |
| 37 | Registry = generated host rows plus hand-written intrinsics; hand-written authority snapshot; security RFC and second reviewer; no default policy; hand-written imports⊆grants | generate everything | One bad table line would weaken every layer at once. |
| 38 | Files-per-capability defined by `dummy-capability.sh`; sidecar at 0.4; ≤8 files at 0.5 | a vague ≤5 | Measurable, and includes the host-config schema. |
| 39 | Pure callbacks for stdlib higher-order functions until 0.6 (K3020) | allow effects from 0.4 | Replay singletons and attacker-sized lists. |
| 40 | Guest HMAC limited to the current request, must-use, shown as guest-checked | a general verification oracle | Advisory checks must look advisory. |
| 41 | Windows host mode refuses durable state and secrets in 0.2.1 | implement ACL checks now | Cheap and fail-closed. |
| 42 | 0.8 effective pw per working week; staff = 1 until hired; second-FTE funding as top priority; calibrate at 0.2.2 | 1 pw per week; assume 2 FTE | The previous arithmetic did not hold. |
| 43 | Conditional column shows a Mar 2027 hire | "1→2 FTE at 0.5" (feasibility critic) | A 0.5 hire cannot move the R1 date, which is the binding risk. |
| 44 | Kill/pivot at the 0.4 exit; leading indicators at 0.2.2 and 0.3 | checkpoint at the 0.5 exit | Otherwise about 85 pw would be spent before any signal. |
| 45 | Cut lists name the gate that moves; 0.3 (~20%) and 0.4 (~10%) stay below 30% | pad to 30% (feasibility critic) | Unification and the backend are indivisible; slips move dates, not gates. |
| 46 | `openai-chat` first, `anthropic-messages` at 0.4.1; system prompt as a hashed package file | both adapters at 0.4; system prompt as a manifest string | Widest reach; prompts stay reviewable text in the PR. |
| 47 | Operate block (inspect, DLQ redrive, Retry-After, spans, health) in 0.5 | leave operations to users | 30 production bots at 0.5 need it. |
| 48 | Literal patterns and guards rejected, with a K1111 reopen trigger | adopt with `_ =>` | Catch-alls hide future variants. |
| 49 | Partner contact as group office hours | per-partner biweekly calls | Fits the 20% maintenance budget. |
| 50 | Audit and bench budget lines with a 0.5 funding deadline and fallbacks | unfunded gates | 1.0 depends on the audit; the bench costs money every release. |