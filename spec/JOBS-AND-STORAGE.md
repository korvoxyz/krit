# Durable queues, scheduled triggers, and object storage

**Status:** Normative bounded Phase 6 local host
**Store schema:** 2
**Host config schema:** 4
**Artifact validation policy:** 2 for durable artifacts
**Delivery protocol:** 1

## Scope

This milestone adds three capability-scoped durable services to the same
single-host `krit-state` SQLite store that Phase 6 state uses:

- typed durable queues with owner leases, bounded retries, and dead letters
- host-owned scheduled triggers with durable fire identities and catch-up bounds
- capability-scoped bounded object storage backed by SQLite blobs

It does not provide distributed queues, cross-host leader election, broker
protocols, cron expressions, guest-selected clocks, guest filesystem paths,
streaming object bodies, or provider-side exactly once.

## Authority

Schema-1 manifests request four new capability families:

```toml
[capabilities]
queues = ["render-jobs"]          # queue.publish
consumes = ["render-jobs"]        # queue.consume
schedules = ["hourly-sweep"]      # schedule.trigger
buckets = ["render-output"]       # object.read + object.write
readOnlyBuckets = ["reference"]   # object.read only
```

`buckets` and `readOnlyBuckets` are disjoint. Publishing and consuming are
separate grants: an ingress webhook that only enqueues never gains the authority
to consume, and a worker never gains the authority to enqueue unless it also
requests `queues`.

Compiler effects and their exact resource requirements:

| Effect | Source | Resource |
|---|---|---|
| `queue.publish` | `queue_publish` | queue name |
| `queue.consume` | `queue "name" fn` | queue name |
| `schedule.trigger` | `schedule "name" fn` | schedule name |
| `object.read` | `object_get` | bucket name |
| `object.write` | `object_put`, `object_delete` | bucket name |

Queue, schedule, and bucket identities must be direct canonical string literals
using the existing resource-name grammar (1-64 lowercase letters, digits, `.`,
or `-`, without leading or trailing punctuation or `..`/`--`). Object keys and
job bodies are bounded ordinary UTF-8 strings and may be dynamic. `Secret` is
not a string: it cannot be published, stored as an object, or named as a
resource.

A source package never chooses a database path, a durability setting, a lease
duration, an attempt budget, or a deployment grant.

## Source API

Edition 2026 adds one entrypoint form per delivery kind and four built-ins.

```krit
queue "render-jobs" fn handle(job: QueueJob) -> Result<String, String> {
    match object_put("render-output", job.id, job.body) {
        Ok(stored) => Ok(job.id),
        Err(error) => Err(error),
    }
}
```

```krit
schedule "hourly-sweep" fn handle(event: ScheduleEvent) -> Result<String, String> {
    Ok(event.id)
}
```

```krit
queue_publish("render-jobs", body)
// Result<String, String> — the durable job identity

object_get("render-output", key)
// Result<Option<String>, String>

object_put("render-output", key, value)
// Result<Unit, String>

object_delete("render-output", key)
// Result<Unit, String>
```

`queue` and `schedule` are contextual: they introduce an entrypoint only when
followed by a direct name literal and `fn`, so existing bindings, parameters,
and record fields keep those spellings.

### Fixed typed contracts

```krit
QueueJob {
    queue: String,
    id: String,
    body: String,
    attempt: Int,
    maxAttempts: Int,
}

ScheduleEvent {
    schedule: String,
    id: String,
    scheduledAtMillis: Int,
    firedAtMillis: Int,
    attempt: Int,
    maxAttempts: Int,
}
```

Both instants are host-supplied UTC epoch milliseconds. The guest has no clock,
no timer, and no way to change a due time.

### Outcomes

Both entrypoints return `Result<String, String>`:

- `Ok(detail)` acknowledges the delivery. Staged state, checkpoints, object
  writes, queue publishes, and the acknowledgement commit in one transaction.
- `Err(detail)` reports failure. Nothing the delivery staged is committed and
  the host applies the configured retry or dead-letter policy.

`detail` is bounded to 4 KiB and is never interpreted as control flow.

A module declares at most one `webhook`, `queue`, or `schedule` entrypoint.
The direct evaluator has no queue, schedule, or object host and reports
`K5003`; checking, formatting, explanation, LSP facts, and builds never open a
database.

## Queue lifecycle

```text
published --reserve--> leased --Ok--> committed and removed
                          |
                          +--Err (attempts < max)--> visible after backoff
                          +--Err (attempts = max)--> dead-lettered
                          +--lease expiry--------->  reservable again
```

- **Ordering.** Reservation is deterministic FIFO by publication sequence within
  one queue. Retried jobs keep their original sequence but become invisible
  until their backoff instant, so a retry can be overtaken by newer work. There
  is no cross-queue ordering and no global total order.
- **Attempts.** `attempts` increments at reservation, not at completion. A
  crashed worker therefore consumes exactly one attempt when its lease expires,
  which bounds poison-job redelivery.
- **Leases.** Reservation records a 16-byte owner and an expiry instant. Only
  the recorded owner may acknowledge or fail a delivery; a stale owner receives
  a `Lost` disposition and mutates nothing. Expiry permits another reservation;
  it does not by itself replace the owner. A late, still-current owner may
  complete, but a replaced owner cannot commit staged state or acknowledgement.
- **Scheduler ownership.** A Runtime acquires its execution scheduler before
  queue reservation or schedule materialization/reservation and retains it
  through the outcome commit. Cancellation while waiting is checked before
  consuming an attempt. Lease and outcome instants advance from the supplied
  host timestamp by monotonic elapsed time, including compilation and scheduler
  waiting; the original schedule tick remains the materialization cutoff.
  The host checks the remaining lease again before guest execution, after any
  SQLite wait. A failed outcome commit follows the same bounded retry/dead-letter
  path as a failed invocation, without publishing staged state or output.
- **Backoff.** Retry visibility is `now + min(backoff * 2^(attempts - 1),
  maxBackoff)`.
- **Dead letters.** Exhausting the attempt budget moves the job body, attempt
  count, and a bounded 256-byte failure reason into a dead-letter table. Dead
  letters are never redelivered and are pruned transactionally by age and count.
- **No lock over guest execution.** Reservation, acknowledgement, and failure
  are three short SQLite immediate transactions. No database lock is held across
  guest execution or network access. Because no lock spans execution, a
  configured delivery lease must be at least the runtime execution deadline plus
  the backing store's busy timeout; a shorter lease is rejected at host
  validation. This protects the bounded execution window, not arbitrary process
  suspension or cross-host exactly-once delivery; replaced owners cannot commit.
- **Bounded terminalization.** One reservation call inspects at most
  `min(bound, 1024)` candidates, where `bound` is the queue depth or the
  schedule's retained-fire count. Terminal transitions discovered during that
  scan are always committed, and the call then reports no delivery. A depth-one
  queue whose only job has exhausted its attempts therefore dead-letters and
  stays usable instead of failing every future reservation.
- **Ordering and the state revision.** Publication order comes from the
  transactional `meta.sequence` counter, not from the store revision. A
  publish-only outcome neither reads nor advances `meta.revision`, so
  independent publishers never conflict; an outcome that also writes key/value
  state, a checkpoint, or an object still checks and advances the revision
  exactly once.

## Schedule semantics

- Occurrences are `start + k * interval` in UTC epoch milliseconds. There are no
  cron expressions, no local time zones, and no daylight-saving rules.
- A tick supplies one host instant. The host materializes every occurrence in
  `(cursor, now]` and records a durable cursor.
- On first sight of a schedule the host materializes only the current
  occurrence, so enabling a schedule never replays history.
- **Catch-up bound.** At most `maxCatchUp` occurrences materialize per tick.
  Older missed occurrences are skipped and reported as a `skipped` count; they
  are never silently converted into work.
- **Fire identity.** `(schedule, dueAtMillis)` is the durable primary key, and
  the guest sees it as `schedule@dueAtMillis`. Re-ticking the same instant, or
  restarting mid-flight, cannot create a second committed fire.
- **Instant arithmetic.** Every instant a tick could record — the occurrence,
  the lease expiry, the maximum retry visibility, and the retention horizon — is
  validated with checked arithmetic before the cursor or any fire row moves. An
  unrepresentable instant such as `--now 9223372036854775807` is refused with
  `K5202` and leaves the cursor, the fire rows, and the queue untouched, so the
  next ordinary tick behaves normally.
- Fires reuse the queue lease, attempt, backoff, and dead-letter machinery.
  Retention prunes completed and dead fires by age and count.

## Object storage

- Buckets are host-owned SQLite blob namespaces bound to one store. Guests never
  see a path, a file handle, a directory, or a symlink.
- `object_get` reads through the invocation's staged writes and then the durable
  snapshot at the invocation's base revision.
- `object_put` and `object_delete` stage a mutation; they commit only at the
  outcome boundary. A put replaces an existing key and its retained bytes are
  accounted as `retained - previous + new`.
- Bounds are per bucket: object count, key bytes, object bytes, and total bucket
  bytes. Exceeding any bound aborts the whole outcome. Queue depth is likewise
  per queue: an atomic fan-out that publishes to several queues charges each
  queue only for its own staged jobs.
- Listing is available to the host and to auditing tools as a deterministic,
  byte-ordered, prefix-filtered, count-bounded query. It is **not** exposed to
  guests in protocol 1 because the bounded guest ABI cannot iterate lists.
  Prefix matching is exact and case sensitive under SQLite's default `BINARY`
  collation: `a` never matches `Apple`, and `%` and `_` are ordinary prefix
  characters rather than wildcards.

## Atomicity and crash model

One invocation transacts against exactly one durable store. Queues, schedules,
and buckets resolve to their configured store, so mixing resources bound to
different stores is rejected with `K5202`.

The outcome boundary commits, in one SQLite transaction:

1. staged key/value state and checkpoints,
2. staged object writes and deletions,
3. staged queue publishes,
4. the delivery acknowledgement (job deletion or fire completion).

A trap, deadline, cancellation, invalid result, limit violation, or database
failure commits none of them. Delivery failure additionally records one bounded
attempt.

Crash windows that remain, stated honestly:

```text
guest finished -> host process dies before the outcome commit
    => the lease expires, the delivery is retried, and the guest must be
       idempotent or use explicit replay records

external effect completed -> host dies before the replay completion commit
    => at-least-once with idempotency-key protection, not exactly once
```

Because acknowledgement and state share one transaction, Krit does not lose or
silently duplicate *committed* queue, schedule, or object state on a single
host. It does not claim distributed exactly once, provider-side deduplication,
or multi-host coordination.

## Store schema 2

Schema 1 stores migrate forward in one exclusive transaction that validates the
existing schema-1 definitions, adds the new tables and indexes, and then sets
`user_version = 2`. Existing key/value, checkpoint, replay, and idempotency rows
are preserved; the revision and sequence counters are untouched. A failed
migration leaves schema 1 intact. A newer schema, a foreign application ID, a
missing or extra table, a missing or altered index, a trigger, or a view is
`K5201`; Krit never resets or replaces a database.

Added strict tables:

| Table | Purpose | Key |
|---|---|---|
| `queue_jobs` | reservable jobs, leases, attempts | `id` |
| `queue_dead` | terminal dead letters | `id` |
| `schedule_fires` | durable fire identities and status | `(schedule, due_at)` |
| `schedule_cursors` | last materialized occurrence | `schedule` |
| `objects` | bucket blobs and byte accounting | `(bucket, key)` |

Added indexes: `queue_jobs_ready`, `queue_dead_cleanup`, `schedule_fires_ready`.
Every reservation, cleanup, and accounting query uses an index or a primary key.

The migration runs entirely inside one exclusive transaction. It first validates
the complete schema-1 object set — the exact table and index name lists, every
declared definition, every ordered column and constraint, and the absence of any
view or trigger — before emitting DDL, and it revalidates the finished schema-2
shape before committing. A rejected migration therefore leaves the database
byte-for-byte unchanged.

An empty schema-2 store occupies 27 SQLite pages, 108 KiB at the 4 KiB default
page size. The page ceiling derived from the configured byte budget is installed
*before* initialization or migration, and the resulting `page_count` and
effective `max_page_count` are both re-checked against the budget before the
store is usable, so a database can never be grown past its budget and then
rejected.

Queue, schedule, and object mutations use `meta.sequence`; only key/value,
checkpoint, and object mutations advance `meta.revision`, so concurrent workers
do not create spurious revision conflicts.

## Runtime and CLI

- One fresh Wasmtime `Store`, instance, resource table, and invocation
  transaction per delivery or trigger.
- The host reserves before instantiating and commits after validating the guest
  result. Fuel, memory, stack, host-call, output, and deadline limits are
  unchanged.
- `krit worker --queue NAME --once` reserves and dispatches at most one
  delivery. `--max-deliveries N` bounds a batch at 1..=1024; there is no
  unbounded loop, no sleep, and no polling daemon.
- `krit schedule --schedule NAME --once [--now EPOCH_MILLIS]` materializes due
  occurrences and dispatches at most one fire. `--now` makes tests deterministic;
  without it the host reads its own wall clock.
- Both commands report a schema-1 JSON summary with counts, outcomes, and
  identities. They never print keys, values, bodies, paths, or credentials.

With `--json`, standard output carries exactly one report document and nothing
else, even when the artifact also holds `io.stdout`:

```json
{
  "schema": 1,
  "resource": "render-jobs",
  "nowMillis": 7200000,
  "dispatched": 1,
  "completed": 1,
  "retried": 0,
  "deadLettered": 0,
  "idle": false,
  "stoppedForOutputBudget": false,
  "materialized": 1,
  "skipped": 0,
  "outcomes": [{ "outcome": "completed", "id": "hourly-sweep@7200000", "attempt": 1 }],
  "outputs": ["1\n"]
}
```

`materialized` and `skipped` appear only for schedules. `outcomes` and `outputs`
are parallel arrays in dispatch order: `outputs[i]` is the bounded standard
output the guest produced for `outcomes[i]`. Guest output that is not valid
UTF-8 is an operational failure rather than a corrupted document. Dispatch stops
early and sets `stoppedForOutputBudget` once collected output reaches the
runtime output limit, so a batch run never accumulates more than twice that
limit. Human mode still streams guest bytes to standard output unchanged.

## Host configuration

Schemas 1, 2, and 3 remain readable. Schema 4 extends schema 3 with a `jobs`
section:

```json
{
  "schema": 4,
  "state": { "stores": { "agent-work": { "path": "state/jobs.db", "...": "..." } } },
  "jobs": {
    "queues": {
      "render-jobs": {
        "store": "agent-work",
        "maxDepth": 1024,
        "maxJobBytes": 65536,
        "maxQueueBytes": 8388608,
        "maxAttempts": 3,
        "leaseSeconds": 30,
        "backoffSeconds": 1,
        "maxBackoffSeconds": 60,
        "deadLetterMaxEntries": 256,
        "deadLetterRetentionSeconds": 604800
      }
    },
    "schedules": {
      "hourly-sweep": {
        "store": "agent-work",
        "intervalSeconds": 3600,
        "startEpochMillis": 0,
        "maxCatchUp": 4,
        "maxAttempts": 3,
        "leaseSeconds": 30,
        "backoffSeconds": 1,
        "maxBackoffSeconds": 60,
        "retentionSeconds": 604800,
        "maxRetainedFires": 256
      }
    },
    "buckets": {
      "render-output": {
        "store": "agent-work",
        "maxObjects": 1024,
        "maxKeyBytes": 256,
        "maxObjectBytes": 65536,
        "maxBucketBytes": 8388608
      }
    }
  }
}
```

Configuration can only narrow what a manifest already requests. A configured
queue, schedule, or bucket that the manifest does not grant is `K5001`. A store
that backs only job resources is host-owned and needs no `state.transaction`
grant, which keeps an ingress publisher free of state authority. Store paths
keep the schema-3 filesystem policy: host-config-relative normal `.db` paths,
no `.` or `..`, no symlink component, an existing owner-only directory, and
owner-only `0600` files. No queue, schedule, or bucket has a default.

## Limits

Protocol-1 hard maxima:

| Resource | Default | Hard maximum |
|---|---:|---:|
| Configured queues | none | 16 |
| Configured schedules | none | 16 |
| Configured buckets | none | 16 |
| Queue depth | none | 65,536 |
| Job body bytes | none | 1 MiB |
| Retained queue bytes | none | 256 MiB |
| Delivery attempts | none | 16 |
| Delivery lease | none | 5 minutes |
| Retry backoff | none | 1 hour |
| Dead-letter entries | none | 4,096 |
| Dead-letter retention | none | 30 days |
| Dead-letter reason bytes | 256 | 256 |
| Schedule interval | none | 1 second .. 365 days |
| Schedule catch-up per tick | none | 64 |
| Retained fires | none | 4,096 |
| Schedule retention | none | 30 days |
| Objects per bucket | none | 65,536 |
| Object key bytes | none | 1 KiB |
| Object bytes | none | 4 MiB |
| Bucket bytes | none | 1 GiB |
| Outcome detail bytes | 4 KiB | 4 KiB |
| Deliveries per CLI run | 1 | 1,024 |
| Reservation scan per call | — | 1,024 rows |
| Host-side object listing | — | 1,024 keys |
| Database budget | 64 MiB | 1 MiB minimum, 1 GiB maximum |
| Delivery lease minimum | — | execution deadline + store busy timeout |
| Collected `--json` guest output | — | twice the runtime output limit |

Every configured value is validated against these maxima before any database is
opened. There are no implicit defaults: an unconfigured resource is unavailable.

## Observability

Stats may report numeric object reads, object writes, queue publishes, state and
checkpoint counters, and replay hit/miss counts. Delivery reports may include
the delivery identity, attempt number, outcome name, and next visibility
instant. They never report job bodies, object keys or values, checkpoint values,
database paths, schedule cursors, credentials, or digests.

## Non-goals

- distributed queues, brokers, fan-out, or consumer groups
- cron expressions, calendars, time zones, or guest-selected schedules
- guest-visible object listing, streaming bodies, or byte ranges
- guest filesystem paths, directories, or ambient file access
- cross-store atomic delivery
- provider-side or multi-host exactly once
- unbounded worker daemons or background polling inside the CLI
