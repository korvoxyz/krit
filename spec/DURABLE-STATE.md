# Durable state, checkpoints, replay, and idempotency

**Status:** Normative bounded Phase 6 local host
**Store schema:** 2 (schema 1 migrates forward in place)
**Host config schema:** 4 (schemas 1-3 remain readable)
**Artifact validation policy:** 2 for state-enabled artifacts

## Scope

Krit durable state provides capability-scoped local persistence for one host
machine:

- transactional bounded string key/value state
- explicit named workflow checkpoints
- completed HTTP/AI operation replay records
- opt-in durable inbound idempotency responses

It does not provide distributed consensus, cross-region transactions,
provider-side exactly once, arbitrary SQL, ambient filesystem access, raw
database handles, guest-selected database paths, or durable secret storage.

## Authority

Schema-1 manifests request named stores:

```toml
[capabilities]
state = ["agent-work"]
```

The compiler effect is `state.transaction`. Every state, checkpoint, or replay
operation adds an exact `state.transaction("store")` requirement. Replay also
adds the existing exact `http.request("origin")` or `ai.invoke("adapter")`
requirement. A package requests authority but never chooses a database path,
durability setting, credential, or deployment grant.

Store, checkpoint, replay-operation, HTTP-origin, and AI-adapter identities
must be direct canonical string literals. Store/checkpoint/operation names use
the existing canonical resource-name grammar. Keys and values are bounded
ordinary UTF-8 strings. `Secret` is not a string and cannot be written,
checkpointed, replayed, encoded, logged, or included in state metadata.

## Source API

Edition 2026 adds these built-ins:

```krit
state_get("agent-work", key)
// Result<Option<String>, String>

state_put("agent-work", key, value)
// Result<Unit, String>

state_delete("agent-work", key)
// Result<Unit, String>

checkpoint_get("agent-work", "posted-message")
// Result<Option<String>, String>

checkpoint_put("agent-work", "posted-message", value)
// Result<Unit, String>

replay_http("agent-work", "fetch-issue", "https://api.example.com", request)
// Result<HttpResponse, String>

replay_ai("agent-work", "summarize-issue", "reviewer", input)
// Result<String, String>
```

`replay_http` is anonymous in protocol 1. Authenticated replay remains
unavailable because an opaque secret handle cannot be serialized and the
initial replay WIT interface does not accept one.

The direct evaluator has no state host and reports `K5003`. Source checking,
formatting, explanation, LSP facts, and builds do not open a database.

## Invocation transaction

Each fresh component Store owns one invocation-local state transaction.
`state_get` and `checkpoint_get` read through staged mutations and then the
durable snapshot. Put/delete/checkpoint changes remain in host memory until the
guest returns a valid response and all ordinary runtime completion checks pass.

The runtime commits the one touched store before publishing response, stdout,
or success logs. A trap, invalid response,
cancellation, deadline, output failure, state conflict, or database failure
commits no ordinary state/checkpoint mutation and publishes no successful
response/output.

Each store snapshot records a revision. Commit uses a SQLite immediate
transaction and succeeds only if the current revision equals the invocation's
base revision. Concurrent committed work therefore serializes; a stale
invocation receives `K5202` rather than overwriting newer state. There is no
multi-database atomic commit: an invocation using more than one durable store
is rejected in protocol 1.

## Checkpoints

A checkpoint is a named bounded string value in the same store transaction as
ordinary state. Applications explicitly read a checkpoint and choose the
remaining deterministic work. Krit does not capture a Wasm stack, linear
memory, secret handle, closure, clock, or arbitrary value graph.

Checkpoint names are stable source literals. Updating a checkpoint commits
only with successful invocation completion. A failed invocation therefore
cannot claim workflow progress it did not complete.

## Replay records

Replay operations use `(artifact identity, operation kind, operation name)` as
their durable identity and store one input digest plus one completed bounded
result. The input digest includes the exact external resource and request/input
bytes but never a secret value.

Before returning a recorded result, the host rechecks:

1. the artifact's exact state and external-resource requirements
2. current manifest/deployment grants
3. current AI approval policy where applicable
4. the recorded input digest
5. record TTL and schema

A matching completed record returns without an external call. A differing
input is a replay conflict. In-progress records have bounded leases; a live
lease refuses duplicate work. An expired lease permits recovery under the
crash rules below.

Completed replay results commit immediately after the external operation,
independently of the invocation's state/checkpoint transaction. This is
intentional: if later guest work traps, the next invocation can reuse the
completed side effect and continue toward its checkpoint.

### HTTP safety

`replay_http` permits GET/HEAD or a request containing one valid ordinary
`Idempotency-Key`. Unsafe non-idempotent requests without that key are rejected
before network access. The host still cannot prove that a remote service
honors the key.

### AI safety

`replay_ai` derives a stable provider idempotency key from artifact identity,
store, operation, adapter, and input digest. The current `http-json` adapter
passes that key on every attempt and recovery. Approval, rate, retry, deadline,
and response bounds remain in force.

## Crash model and exactly-once limits

SQLite transactions make database commits atomic and durable according to the
configured synchronous mode. WAL recovery handles process termination and
partial pages.

There remains an unavoidable external-effect crash window:

```text
provider completed effect
    -> host process crashes before replay completion commit
```

GET/HEAD may safely repeat. For keyed HTTP/AI operations, Krit reuses the same
idempotency key, but provider-side deduplication is outside Krit's transaction.
Krit therefore provides durable local replay and at-least-once recovery with
idempotency-key protection, not distributed exactly once.

## Durable inbound idempotency

Host config may opt the existing inbound `Idempotency-Key` policy into one
manifest-granted durable store. Without that setting, Phase 4 process-local
TTL/LRU behavior is unchanged.

Durable keys remain scoped by artifact identity and request key. The
credential-sensitive request digest retains method, path, query, body, and all
headers except `Idempotency-Key`. Lookup/lease creation is one immediate SQLite
transaction:

- completed + matching digest: replay the response without Store creation
- completed + different digest: deterministic HTTP 409 conflict
- unexpired in-progress lease: reject duplicate in-progress work
- absent or expired lease: reserve and execute

Only a successful validated guest response completes the record. Failure,
trap, cancellation, or state-commit failure removes the owned reservation.
Process death leaves a lease that becomes recoverable after its bounded TTL.
Cleanup enforces expiry, LRU entry count, and retained response-byte limits
transactionally.

The invocation state commit and inbound-idempotency response completion are
two SQLite transactions. A process crash between them can leave committed
state with an expiring in-progress inbound lease. Recovery may re-enter guest
code after lease expiry; applications that combine durable inbound
idempotency with external effects must use explicit replay operations, and
state updates should be written idempotently from the request identity.

## SQLite store

The host uses bundled SQLite through a dedicated `krit-state` crate. Guest code
never receives SQL, paths, connections, transactions, row IDs, or handles.

Each store is one database file with:

- SQLite `application_id` identifying Krit durable state
- `user_version = 2`
- WAL journaling
- foreign keys and defensive/trusted-schema restrictions
- configurable `FULL` (default) or `NORMAL` synchronous durability
- bounded busy timeout
- a page ceiling derived from the database-byte limit, installed before any
  schema work and re-verified against the materialized `page_count` afterwards
- indexed key/checkpoint/replay/idempotency tables
- one monotonically increasing store revision and LRU sequence

Schema 0 initializes in one exclusive transaction. Schema 1 migrates forward to
schema 2 in one exclusive transaction that validates the complete existing
schema-1 object set — exact table and index name lists, declared definitions,
ordered columns and constraints, and the absence of views and triggers — before
adding the queue, schedule, and object tables, and revalidates the finished
schema before committing; existing rows, revisions, and sequences are preserved
and a rejected migration leaves the database byte-for-byte unchanged. Schema 2 opens only when the strict
table definitions, ordered columns, primary keys, constraints, and cleanup
indexes match exactly; extra tables, indexes, views, or triggers are rejected. A
newer schema, wrong application ID, malformed schema, corruption, failed
integrity check, or failed migration is `K5201`; Krit never resets or replaces
an unknown database automatically.

## Host configuration

Schemas 1 and 2 remain readable. Schema 3 extends schema 2 with `state`, and
schema 4 adds the `jobs` section defined in `JOBS-AND-STORAGE.md`:

```json
{
  "schema": 3,
  "config": {},
  "secrets": {},
  "state": {
    "stores": {
      "agent-work": {
        "path": "state/agent-work.db",
        "durability": "full",
        "busyTimeoutMs": 250,
        "maxOperations": 128,
        "maxKeyBytes": 256,
        "maxValueBytes": 65536,
        "maxTransactionBytes": 1048576,
        "maxDatabaseBytes": 67108864,
        "maxReplayEntries": 1024,
        "maxReplayBytes": 16777216,
        "replayTtlSeconds": 604800,
        "leaseSeconds": 30
      }
    },
    "durableIdempotencyStore": "agent-work"
  }
}
```

No state path exists by default. Configured stores must be requested by the
manifest. Paths are host-config-relative normal paths and cannot escape through
symlinks. The containing directory must already exist, be owner-only on Unix,
and contain no symlink component. Existing database/WAL/SHM files must be
regular owner-only files; new database files are created mode `0600`.
Credentials, state values, keys, checkpoint values, replay bodies, and paths
never enter artifacts, metadata, permission output, diagnostics, logs, or
stats.

## Limits

Protocol-1 hard maxima:

| Resource | Default | Hard maximum |
|---|---:|---:|
| Configured stores | 4 | 16 |
| State/checkpoint operations per invocation | 128 | 1,024 |
| Key bytes | 256 | 4 KiB |
| Value/checkpoint bytes | 64 KiB | 1 MiB |
| Staged transaction bytes | 1 MiB | 16 MiB |
| Database bytes | 64 MiB | 1 MiB minimum, 1 GiB maximum |
| SQLite busy timeout | 250 ms | 5 s |
| Replay entries per store | 1,024 | 65,536 |
| Replay retained bytes | 16 MiB | 256 MiB |
| Replay result bytes | existing HTTP/AI response limits | 16 MiB |
| Replay TTL | 7 days | 30 days |
| In-progress lease | 30 seconds | 5 minutes |

Database time is bounded by indexed operations, the busy timeout, host-call
limits, and the invocation deadline. SQLite page-cache and WAL behavior add
local disk latency; state is a correctness feature, not a cache.

## WIT and artifact policy

`krit:runtime/state@0.2.0` is a typed interface containing the seven source
operations above. A state-enabled webhook selects one of the finite existing
world combinations plus exactly the state interface. Old worlds, imports,
metadata, and component bytes remain unchanged when state is unused.

State-enabled artifacts use validation policy 2. Validation re-derives
`state.transaction` from the actual state interface import and requires exact
embedded/adjacent state and external replay resource requirements. Policy-1
module artifacts and state-free policy-2 webhook artifacts remain validation
policy 1.

## Observability

Stats may report numeric state reads, staged writes, committed writes,
checkpoint reads/writes, replay hits/misses, and durable-idempotency replay.
They never report keys, values, checkpoint names/values, database paths,
external request/response bodies, or digests.

## Non-goals

- distributed locks, consensus, or multi-host exactly once
- arbitrary guest database queries
- cross-store atomic transactions
- authenticated replay HTTP in protocol 1
- storing opaque secrets or host capability handles

Durable queues, scheduled triggers, and object storage are normative in
[JOBS-AND-STORAGE.md](JOBS-AND-STORAGE.md); they reuse this store, its
transaction boundary, and its crash model.
