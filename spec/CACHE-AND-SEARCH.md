# Cache and Search

Status: normative for Krit 0.2, protocol 1.

This specification defines two *optional* capabilities: a bounded namespaced
TTL cache, and provider-neutral search and vector connectors.

## The fundamental rule

**Correctness may never depend on cache availability.**

A cache that is disabled, empty, expired, evicted, full, or broken must produce
an explicit value that source code handles. The host never substitutes a value,
never silently downgrades an outage into a miss, and never treats a cached value
as authoritative. The same artifact must produce the same answer with the cache
configured and with the cache absent; only the amount of work changes.

The type of `cache_get` enforces this. It returns
`Result<Option<String>, String>`, so source cannot reach a cached value without
deciding, in writing, what a miss and an outage mean.

| Outcome | Value | Meaning |
|---|---|---|
| Hit | `Ok(Some(value))` | A live entry existed. |
| Miss | `Ok(None)` | Absent, expired, or evicted - deliberately indistinguishable. |
| Outage | `Err(reason)` | The namespace is unconfigured, refused, or unusable. |

A miss and an outage are distinguishable from each other and from a hit. Absence
and expiry are *not* distinguishable from each other, and must not need to be:
treating an expired entry differently from an absent one would make the cache
load bearing.

## Authority

| Effect | Grants | Resource |
|---|---|---|
| `cache.read` | `cache_get` | cache namespace |
| `cache.write` | `cache_put`, `cache_delete` | cache namespace |
| `search.query` | `search_query` | search index |
| `search.vector` | `vector_search` | vector index |

Manifest capabilities:

```toml
[capabilities]
cacheNamespaces = ["lookups"]            # read and write
readOnlyCacheNamespaces = ["reference"]  # read only
searchIndexes = ["docs"]                 # search.query
vectorIndexes = ["embeddings"]           # search.vector
```

A namespace may not appear in both `cacheNamespaces` and
`readOnlyCacheNamespaces`. Host configuration can only narrow what the manifest
grants; it can never widen it.

## Source API

```krit
cache_get(namespace: String, key: String) -> Result<Option<String>, String>
cache_put(namespace: String, key: String, value: String, ttlSeconds: Int)
    -> Result<Unit, String>
cache_delete(namespace: String, key: String) -> Result<Unit, String>

search_query(index: String, query: String, limit: Int) -> Result<String, String>
vector_search(index: String, vector: String, limit: Int) -> Result<String, String>
```

Namespaces and index names are **direct canonical literals**: a computed name is
`K3008`. Keys, values, queries, and encoded vectors are bounded ordinary
strings. The time to live is explicit, in seconds, and bounded; there is no
default and no "forever".

`Secret` and `DatabaseTransaction` are opaque and can never be cached, sent to a
connector, or used as a key: every such position is a static error.

## Cache semantics

- **Namespaced.** Namespaces are isolated; the same key in two namespaces is two
  entries.
- **Explicit TTL.** Each write carries its own bounded time to live. Expiry is
  exact and inclusive: an entry with deadline `t` is already gone at `t`.
- **Monotonic clock.** Expiry is measured on the cache's own monotonic
  timeline, never a wall clock. A wall clock can jump backwards, which would
  silently extend an entry past its declared time to live, and reading one can
  fail, which would turn a host clock fault into a guest-visible trap. Neither
  is possible. The guest can neither observe nor influence the instant, and
  deadline arithmetic is checked for overflow.
- **LRU eviction.** A read refreshes recency. When the *destination namespace*
  is at its entry or byte bound, its own least recently used entry is evicted.
  When the *whole cache* is at its bound but the destination namespace is not,
  the globally least recently used entry is evicted, which may live in another
  namespace. Global pressure is therefore resolved by exact global recency
  rather than by penalising the namespace being written.
- **Exact accounting.** An entry costs `key + value + 64` bytes of overhead. A
  replacement releases the old cost before charging the new one.
- **Bounded cleanup.** Each write reclaims at most eight expired entries.
  Correctness never depends on this sweep: expiry is enforced on every read, and
  eviction reclaims capacity on demand.
- **Deleting an absent key succeeds.** The postcondition - the key is not
  cached - already holds. This is a statement of fact, not a swallowed error.

### Read-only namespaces and host seeding

A `read-only` namespace refuses `cache_put` and `cache_delete` from guest code.
It is not, however, permanently empty: the embedding that owns the cache may
populate it through a host-only seeding API that is deliberately separate from
guest write authority. Seeding is exempt *only* from the read-only refusal -
key bytes, value bytes, time to live, namespace entry and byte budgets, the
whole-cache budget, eviction, and expiry all apply exactly as they do to a guest
write. A seeded entry is an ordinary entry that happens to have been written by
the host, and guest code can still only read it.

### Not durable, not transactional

The cache is **process local**. It is shared by every invocation on one host, so
a fresh Wasm `Store` still sees it, and it is **lost when the host process
restarts**. That loss is a normal, documented outcome, not a fault.

The cache is **not part of the invocation outcome**. A trap, a cancellation, a
failed queue delivery, or a rolled-back durable transaction does **not** undo an
earlier `cache_put`. This is stated plainly rather than hidden, and it is
precisely why a cached value may never be load bearing. If a value must survive
failure or restart, use durable state, an object, or a database - not the cache.

There is no cross-host coherence, no invalidation protocol, no pub/sub, and no
distributed cache.

## Search and vector connectors

A connector is a **named host-owned binding**. Guest code names an index and
supplies bounded input; it never sees an endpoint, path, credential, model,
provider identity, driver option, or raw handle, and it has no general HTTP
reach through this surface.

Two transports ship:

- `http-json` - a strict generic JSON request and response over an exact HTTPS
  origin the manifest already grants. There is no branded SDK and no
  provider-specific protocol. **HTTPS is required**: a connector carries user
  text and, when configured, a credential, so plaintext transport is refused.
  Only an explicit loopback test policy relaxes this, exactly as it does for a
  bearer credential. The connector path must be a safe origin-form absolute
  path under the same rule every other host-owned path obeys: no authority
  form, no scheme, no backslash, no query or fragment delimiter, no control
  byte, and ASCII only.
- `local` - a deterministic in-process document set, for reference examples and
  tests. It performs no I/O.

Request bodies are exactly `{"query": ..., "limit": ...}` or
`{"vector": [...], "limit": ...}`. A credential, when configured, travels as a
bearer header and **never** appears in a body, a cache key, an artifact,
metadata, a log, a statistic, or an error.

### Results are untrusted input

A provider response is parsed against a strict schema that rejects unknown
fields, then **re-encoded** by the host into a fixed shape:

```json
{"results":[{"id":"a","score":0.500000,"snippet":"text"}]}
```

Scores render at fixed precision so identical inputs produce identical bytes.
Identifiers, snippets, counts, and total bytes are all bounded. Because the host
re-encodes, a hostile or buggy provider cannot control the structure guest code
parses. Results are **data**: nothing in a result is ever executed, evaluated,
or used to select code.

Search results are **nondeterministic external input**. Cache reads are
nondeterministic too, in that a hit depends on prior traffic and on host
lifetime. Neither may be used where determinism is required.

### Vectors

Protocol 1 has no float type, so a vector arrives as bounded JSON text. The host
validates it strictly before any request is built: a flat array of finite
numbers whose length exactly matches the connector's declared dimensions. A
malformed vector never reaches a provider.

Similarity is computed in a numerically stable form: both vectors are scaled by
their largest magnitude before any product is accumulated, so a legitimate
finite vector containing values near `f64::MAX` cannot overflow to infinity and
produce a `NaN` score with no JSON spelling. A zero vector scores zero. Any
non-finite intermediate or result is reported as an error rather than encoded,
so **every `Ok` result is valid, deterministic, bounded JSON**.

## Reliability and safety

Connector calls reuse the existing network stack unchanged: statically linked
rustls, exact origin matching, DNS and SSRF filtering, no proxy, no redirect
following, bounded connect, read, and total timeouts, retry and rate policy,
cancellation, and bounded response bytes. A connector timeout is additionally
clamped to the invocation deadline.

A connector's authority is complete and is checked twice. At setup, the
connector name, its exact `http.request` origin, and its `secret.read`
credential must all be granted by the manifest, and its transport is
re-validated against the host's network policy. At dispatch the origin and the
credential are rechecked against the current grant set, so an embedding that
builds a catalog directly through the public API cannot reach authority the
package never requested.

A connector that presents a credential additionally requires explicit
default-deny `http.bearer` approval, and that approval is re-checked **before
every attempt**, including every retry. Withdrawing approval between attempts
stops the next one.

Retry and rate policy are selected exactly as they are for guest HTTP: an exact
origin override wins, otherwise the default applies. Rate is charged against the
**origin**, so every connector and every direct request to one origin share a
single bucket; charging per connector name would silently multiply a configured
cap by the number of names pointed at that origin. The bucket identity and the
guest-visible name are separate: a rate denial, and an approval denial on the
first attempt or on any retry, name the connector's **index**, never its
endpoint, because the guest is never told which origin a connector reaches. The
same holds for every other guest-visible connector failure: a network policy
denial reports an address *class*, not an address, and a transfer failure
reports a fixed category - timeout, host resolution, TLS verification, redirect
refusal, unsupported target, cancellation, connection failure, or a generic
failure. The HTTP driver's own error text is discarded rather than forwarded,
because it routinely carries the host, port, resolved address, proxy, URL, or
certificate detail. Retry classification is preserved independently of the
message.

Because a connector's bucket is its origin and that origin lives in host
configuration rather than in the artifact, the pre-execution
`maxTrackedResources` check counts the exact set of buckets an invocation can
reach: required AI adapters, required direct HTTP origins, and the origins of
the HTTP connectors the artifact requires. Connector and direct traffic to one
origin count once; a local connector and a configured-but-unrequired connector
count for nothing. An invocation that could exceed the bound is refused before
it runs, so LRU replacement can never silently reset a live rate counter.

A search is read-only by construction, so a transient failure may be re-sent
safely. The host marks the operation read-only internally rather than
generating or transmitting an `Idempotency-Key`; no key is invented, and none
appears on the wire. Rate limiting, cancellation, deadlines, and per-attempt
approval apply to every attempt.

A search call may perform a network round trip, so it is **refused while a
database transaction is open**; holding a database lock across a network call
would make the lock window unbounded.

Cache operations count against the invocation's host-call budget, and are
refused after cancellation.

## Host configuration

Schema 6 adds `cache` and `search`. Schemas 1 through 5 remain valid and
unchanged.

```json
{
  "schema": 6,
  "state": { "stores": {} },
  "cache": {
    "namespaces": {
      "lookups": {
        "mode": "read-write",
        "maxEntries": 256,
        "maxBytes": 1048576,
        "maxKeyBytes": 256,
        "maxValueBytes": 16384,
        "maxTtlSeconds": 300
      }
    },
    "maxTotalEntries": 512,
    "maxTotalBytes": 2097152
  },
  "search": {
    "docs": {
      "kind": "query",
      "maxResults": 10,
      "transport": {
        "type": "http-json",
        "origin": "https://search.example",
        "path": "/query",
        "secret": "search-token",
        "maxResponseBytes": 65536,
        "timeoutMs": 2000
      }
    }
  }
}
```

Loading is strictly two-phase, as for schemas 4 and 5. Every cache namespace,
connector, grant, limit, endpoint, secret reference, approval, retry, and rate
setting is validated **before** any durable store, application database, or
cache is created or opened. An invalid configuration leaves the filesystem
untouched.

A duplicate namespace or connector key is refused rather than resolved. JSON
object keys are not unique by construction, and keeping the last value would
silently discard an earlier, possibly stricter, definition of the same
namespace or endpoint. The document is rejected in phase one instead.

There is no default provider and no ambient default cache. A namespace budget
that could not hold one maximum-size entry is rejected at configuration time
rather than refusing every write later. Cache totals declared without any
namespace are rejected. An `http-json` connector may only reach an origin the
manifest already grants, so configuration cannot invent network authority.

## Absent versus misconfigured

These are deliberately different:

- **Absent** - the host configures no such namespace or connector. The program
  still runs, and the operation returns an explicit error the guest handles.
  This is what makes "the cache is off" a supported deployment.
- **Misconfigured** - the resource *is* configured but contradicts the artifact,
  for example write authority requested on a read-only namespace, or a text
  connector registered where a vector connector is required. This fails closed
  before the guest runs.

## WIT and artifacts

Four versioned interfaces, each least authority:

| Interface | Functions |
|---|---|
| `krit:runtime/cache-read@0.2.0` | `cache-get` |
| `krit:runtime/cache-write@0.2.0` | `cache-put`, `cache-delete` |
| `krit:runtime/search-query@0.2.0` | `search-query` |
| `krit:runtime/search-vector@0.2.0` | `vector-search` |

Read and write authority live in **separate** interfaces, so an artifact that
only reads the cache cannot import the write surface at all, and effects are
re-derived directly from component imports with no shared-interface compromise.
Using any of these surfaces selects artifact policy 2. An artifact that uses
none of them is byte-identical to before.

## Limits

| Resource | Hard maximum |
|---|---:|
| Cache namespaces | 16 |
| Entries per namespace | 4,096 |
| Entries in total | 16,384 |
| Cache key bytes | 512 |
| Cache value bytes | 64 KiB |
| Bytes per namespace | 8 MiB |
| Bytes in total | 64 MiB |
| Time to live | 1 s to 7 days |
| Per-entry accounting overhead | 64 bytes |
| Expired entries reclaimed per write | 8 |
| Search connectors | 8 |
| Query bytes | 4 KiB |
| Encoded vector bytes | 64 KiB |
| Vector dimensions | 4,096 |
| Results per call | 100 |
| Encoded result bytes | 256 KiB |
| Result identifier bytes | 512 |
| Result snippet bytes | 8 KiB |

## Observability

Statistics report numeric cache hits, misses, writes, deletes, errors, search
calls, and vector calls. A failure is counted as an error, never as a miss. No
key, value, query, vector, result, endpoint, path, or credential ever appears in
statistics, logs, metadata, permission output, or error text.

`krit explain` and the LSP durable-fact stream report `cache-get`, `cache-put`,
`cache-delete`, `search-query`, and `vector-search` with their literal namespace
or index, and the literal cache key when one is written directly in source.

Guided-authoring context redacts every sensitive literal before it can reach a
provider: the namespace or index, the cache key, the cached **value**, the
search query, and the encoded vector.

Redaction is *conservative and propagating*, not literal-only. Every string
literal that can contribute to a sensitive argument inherits that argument's
category, whether it is written inline, nested inside a call, record, list, or
conditional, or reached through an immutable `let` alias or chain of aliases.
Shadowing resolves to the binding actually in scope, and a function parameter
stops tracing rather than guessing at a caller's value. Redaction stays inside
the sensitive argument's own expression, so unrelated literals elsewhere in the
document remain visible.

One shared table drives both the parsed and the malformed-source paths. The
malformed path additionally tracks nested call frames, argument positions,
bracketed groups, and prior `let` bindings including chains of aliases, and
closes a binding at a statement-terminating newline as well as a semicolon
while leaving a multiline call, record, or list open.

It also mirrors lexical scoping closely enough for broken code: a block opens a
binding scope and closing it restores whatever it shadowed, a `record { … }`
literal groups commas without opening a scope, and function, webhook, queue,
schedule, and closure parameters are installed as untraceable shadows so a name
never resolves to a same-named outer binding.

Exact tracking of a binding's contributing literals is bounded. Exceeding the
bound never drops a literal: the binding switches to redacting its **whole value
range**, and an alias inherits that range, so an overflowing value is
over-redacted rather than partially exposed. Redacted ranges are clamped
outward to UTF-8 character boundaries. Text outside the binding stays visible.

Incomplete or nested source therefore never redacts less than well-formed
source.

## Non-goals

- durability, transactionality, or cross-host coherence for the cache
- cache invalidation protocols, pub/sub, tagging, or dependency tracking
- a distributed cache, a shared cache tier, or a cache warming service
- branded provider SDKs, provider-specific query languages, or ranking control
- embedding generation, model selection, or index management from Krit
- general HTTP reach through the connector surface
- executing, evaluating, or dispatching on search results
- guest-visible endpoints, credentials, provider identities, or raw handles
