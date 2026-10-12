# Grounded use-case sketches

These are the realistic agent programs that release gate 8 of
[PRD 0.3](../../docs/prd-0.3.md) (section 5) measures: how much real work
passes `krit check` and builds from its own manifest. They are measurement
inputs, not examples: like every Krit source in the repository they are
canonically formatted, but three of them are expected not to build.

`cargo test -p krit-cli --test sketches` measures the corpus again and fails
if any outcome below changes, so the figures in the 0.3.0 CHANGELOG cannot
drift without notice. The same test builds the reduced payment sketch and
invokes it against a SQLite store provisioned from
[`03-payment-webhook.schema.sql`](03-payment-webhook.schema.sql), with
[`03-payment-webhook.host.json`](03-payment-webhook.host.json) as the host
configuration. To measure one sketch by hand:

```sh
krit check benchmarks/sketches/06-flaky-worker.krit
krit build --manifest benchmarks/sketches/06-flaky-worker.krit.pkg
```

## Results on 0.3.0

| Sketch | Use case | Scope | Check and build | What full behavior still needs |
| --- | --- | --- | --- | --- |
| 01 | GitHub issue triage | full | no: 6 × K7002 | header lookup by list matching, `Some` outside the bearer argument, typed `json_decode` into records, record layouts (0.4) |
| 02 | Slack `/summarize` | reduced: a shim verifies the signature and forwards the documentation path as the body; no form decoding, user name, or `in_channel` JSON reply | yes | excluded by design, because it fetches dynamic origins ([roadmap](../../docs/roadmap.md) §3.6) |
| 03 | Payment webhook | reduced: shared token in the `token=` query instead of a header; host SQL decodes the event; duplicate and ignored events share one answer; no HMAC | yes, and invokes | header lookup and typed decoding (0.4), HMAC (0.5) |
| 03b | Payment webhook, the 0.2 buildable variant | reduced: no authentication; host SQL decodes the event | yes (it also built in 0.2) | as 03 |
| 04 | Daily sales report | full (the 0.2 sketch) | no: 23 × K7002 | record and list layouts, list matching, typed decoding (0.4); maps (0.6) |
| 05 | Support ticket router | reduced: the model returns one category word as a JSON string; priority, the tier clamp, and the draft reply are dropped | yes | typed decoding into records and enums (0.4) |
| 06 | Retrying delivery worker | full | yes | — |
| 07 | RAG answerer | full | yes | — |
| 08 | Orders transform | full (the 0.2 sketch) | no: checks as a module program, build 90 × K7002 | `String`, `List`, and record layouts in WebAssembly policy 1 (0.4) |

The header comment of each reduced sketch says exactly what it drops.

## Counting

PRD 0.3 counts the eight sketches 01–08:

- with full behavior, 2/8 build (06, 07), up from 1/8 in 0.2.0 (06);
- when reduced-scope rewrites count, 5/8 build (02, 03, 05, 06, 07), up from
  2/8 in 0.2.0 (06 and the 03b variant).

The master roadmap's corpus ([docs/roadmap.md](../../docs/roadmap.md) §3.6)
counts 01, 03, 03b, and 04–08 instead, dropping 02. It gives the same figures:
2/8 with full behavior and 5/8 counting reduced scope (03, 03b, 05, 06, 07).

The gate's target is 4/8. Either way it is met only when reduced-scope
rewrites count, and PRD 0.3 §5 leaves that decision to the release owner.

## Provenance

01, 02, 03, 05, 06, and 07 were rewritten for 0.3 during wave 3. The wave-3
copies were kept only in a worktree that was later removed, so the release-gate
audit recreated them, and they reproduce the published figures. 03b, 04, and
08 are the 0.2 sketches; 04 and 08 are reformatted by `krit fmt` to the 0.3
canonical style, with no other change.
