# Capability-scoped database access

**Status:** Normative bounded Phase 7 local host
**Host config schema:** 5 (schemas 1-4 remain readable)
**Artifact validation policy:** 2 for database-enabled artifacts
**Database protocol:** 1
**Connector:** bundled SQLite through the isolated `krit-database` crate

## Scope

Krit database access provides parameterized, catalogued, transactional reads and
writes against an operator-owned application database:

- an opaque non-serializable `DatabaseTransaction` handle
- explicit `begin`, named parameterized query/execute, `commit`, `rollback`
- a host-owned strict statement catalog with declared parameter and column
  contracts
- bounded rows, columns, bytes, operations, and transaction time

It does not provide raw SQL from source, connection strings, driver options,
credentials, schema ownership, migrations, ambient database authority,
cross-database transactions, or distributed atomicity.

## Authority

Schema-1 manifests request databases by logical name:

```toml
[capabilities]
databases = ["catalog"]              # database.read and database.write
readOnlyDatabases = ["reference"]    # database.read only
```

The two lists are disjoint. Compiler effects:

| Effect | Source | Resource |
|---|---|---|
| `database.read` | `db_begin_read` | database name |
| `database.write` | `db_begin_write` | database name |

`db_query`, `db_execute`, `db_commit`, and `db_rollback` add no capability of
their own: their authority is the transaction they are handed. Database and
statement identities must be direct canonical string literals using the existing
resource-name grammar. Parameters are bounded ordinary strings.

A package requests authority but never chooses a file, DSN, driver option,
credential, isolation level, statement text, or schema.

## Source API

```krit
db_begin_read("catalog")                          // Result<DatabaseTransaction, String>
db_begin_write("catalog")                         // Result<DatabaseTransaction, String>
db_query(transaction, "count-visits", [])         // Result<String, String>
db_execute(transaction, "record-visit", [path])   // Result<Int, String>
db_commit(transaction)                            // Result<Unit, String>
db_rollback(transaction)                          // Result<Unit, String>
```

`DatabaseTransaction` is opaque exactly as `Secret` is. It may appear only as the
first argument of `db_query`, `db_execute`, `db_commit`, or `db_rollback`.
Printing, comparing, JSON-encoding, logging, storing in a record or list, or
placing it into durable state, an object, or a queue is `K3010`. It has no
fields, no constructor, and no representation the guest can observe.

`db_execute` returns the affected row count. `db_query` returns bounded
deterministic JSON text because protocol 1 has no richer typed row value in the
guest language:

```json
{"columns":["id","name"],"rows":[[1,"alice"]]}
```

Column order follows the statement, row order follows the query, and only
INTEGER, TEXT, and NULL are representable. A REAL or BLOB result value, or text
that is not valid UTF-8, fails closed rather than being rendered with an
implementation-defined spelling. Escaping matches the language's `json_encode`,
so the document is stable across runs and platforms.

The direct evaluator has no database host and reports `K5003`. Checking,
formatting, explanation, LSP facts, and builds never open a database.

## Transaction lifecycle

```text
db_begin_read / db_begin_write
    -> open  --db_query / db_execute-->  open
             --db_commit-------------->  completed (published)
             --db_rollback------------>  completed (discarded)
             --trap / cancel / deadline / drop --> rolled back, invocation fails
```

- **Explicit completion is mandatory.** An invocation that returns with a
  transaction still open is rolled back and then fails with `K5302`. Reporting
  success for work the guest never completed would be a success-shaped error.
- **One transaction at a time.** Protocol 1 allows at most one open transaction
  per invocation and a bounded number of transactions in total. Nested or
  concurrent transactions are refused.
- **Handles are single-use after completion.** Any operation on a committed or
  rolled-back handle is `K5302`. A handle from another database cannot be used
  because a statement is resolved only in its own database's catalog.
- **Read transactions cannot mutate.** `db_execute` on a read transaction is
  refused before any SQL runs, and a read-only database refuses
  `db_begin_write` outright.
- **Isolation.** A read transaction is `BEGIN DEFERRED`; a write transaction is
  `BEGIN IMMEDIATE`, so a writer conflict surfaces at begin rather than at
  commit. Isolation is SQLite's, scoped to one file.

## Lock safety and bounded time

Because no lock may be held across unbounded work:

- **External effects are refused while a transaction is open.** `http_request`,
  `ai_invoke`, `replay_http`, and `replay_ai` all fail with `K5302` until the
  transaction is completed. This makes the lock window a function of local
  computation and database calls only.
- Every transaction carries a configured wall bound, checked before each
  operation and at commit, and that bound must be smaller than the invocation
  deadline.
- SQLite busy waiting is bounded and must stay below the transaction bound.
- Operations per transaction, parameters, parameter bytes, rows, columns, and
  result bytes are all bounded.

## Atomicity: an honest two-resource boundary

Krit's outcome model commits durable state, checkpoints, objects, queue
publications, and the delivery acknowledgement in one SQLite transaction on the
Krit store. **An application database is a separate durable resource in a
separate file.** Two SQLite files cannot share one atomic commit, and Krit does
not pretend otherwise:

- `db_commit` publishes immediately. It is not deferred to the invocation
  outcome, because deferring it would require holding a write lock across the
  remainder of guest execution.
- A trap, cancellation, deadline, invalid response, or state conflict *after* a
  successful `db_commit` rolls back Krit state while the database commit stands.
- Stats report `databaseWriteCommitted` so an operator can see that an
  invocation published an external durable effect, and
  `databaseTransactionsAbandoned` so a host that had to roll back on the
  guest's behalf is visible rather than silent.

This is the same honest window Krit already documents for replay records. The
mitigations are operator-side and explicit: write idempotent catalog statements
(`INSERT ... ON CONFLICT DO NOTHING`, `UPDATE ... WHERE version = ?`), and use
Krit checkpoints so a redelivered queue job can detect completed work. Krit
provides no two-phase commit, no XA, and no distributed transaction.

For queue and schedule deliveries the same rule applies: an `Err` outcome rolls
back Krit state and retries the delivery, but a database commit that already
published is **not** undone. A worker that must be safe under redelivery must
make its catalog statements idempotent.

## Statement catalog

The host owns every statement. Configuration validates each entry by preparing
it against the live schema of the configured file, so a statement that does not
match the operator's schema fails at configuration time, not at request time.

Each entry declares:

- `kind`: `query` or `execute`
- `sql`: exactly one statement
- `parameters`: ordered declared types (`text` or `integer`)
- `columns`: the exact expected result column names

Validation rejects:

- SQL that does not begin with `SELECT`, `WITH`, `INSERT`, `UPDATE`, `DELETE`,
  or `REPLACE`
- any `PRAGMA`, `ATTACH`, `DETACH`, `VACUUM`, `ANALYZE`, `REINDEX`, `EXPLAIN`,
  `BEGIN`, `COMMIT`, `ROLLBACK`, `SAVEPOINT`, `RELEASE`, `CREATE`, `DROP`, or
  `ALTER`
- more than one statement, detected with a lexical scan that understands string
  literals, quoted and bracketed identifiers, and line and block comments, so a
  `;` inside a literal or comment is data and a second statement is not
- a placeholder count that differs from the declared parameters
- non-ordinal placeholders (only `?N` is allowed, so binding order is exact)
- a `query` that is not read-only according to SQLite, or that returns no
  columns
- an `execute` that is read-only, or that returns rows
- result columns that differ from the declared contract
- a mutating statement on a read-only database
- unknown configuration fields, an empty catalog, or more than 64 statements

Parameters are always bound, never interpolated. SQL injection payloads are
stored and compared as ordinary data.

## Schema ownership

The application schema, its migrations, its indexes, and its backups belong to
the operator. Krit never creates, migrates, resets, repairs, or vacuums an
application database, and an absent file is a configuration error rather than an
implicitly created empty database. This is deliberately the opposite of the Krit
durable-state store, which owns its own schema and migrates it; the two never
share a file, a table, or a migration path.

## Host configuration

Schemas 1-4 remain readable. Schema 5 adds `databases` and
`maxTransactionsPerInvocation`:

```json
{
  "schema": 5,
  "state": { "stores": {} },
  "maxTransactionsPerInvocation": 1,
  "databases": {
    "catalog": {
      "path": "data/catalog.db",
      "mode": "read-write",
      "busyTimeoutMs": 250,
      "maxDatabaseBytes": 16777216,
      "maxTransactionMillis": 500,
      "maxOperationsPerTransaction": 16,
      "maxParameterBytes": 4096,
      "maxRows": 64,
      "maxColumns": 8,
      "maxResultBytes": 65536,
      "statements": {
        "record-visit": {
          "kind": "execute",
          "sql": "INSERT INTO visits(path) VALUES(?1)",
          "parameters": ["text"],
          "columns": []
        },
        "count-visits": {
          "kind": "query",
          "sql": "SELECT COUNT(*) AS total FROM visits",
          "parameters": [],
          "columns": ["total"]
        }
      }
    }
  }
}
```

Configuration can only narrow the manifest. A configured database the manifest
does not grant is `K5001`; a `read-write` database requires a `databases` grant,
and a `read-only` database is satisfied by either list.

Loading a host configuration has two strictly ordered phases. The first is
side-effect free: every state store, job definition, database definition, grant,
limit, statement, path shape, secret, AI adapter, retry, rate limit,
idempotency, and approval rule is parsed and validated, and no durable store or
application database is created, opened, or migrated. Only when the whole
configuration is known to be valid does the second phase run. Within it,
application databases open first - opening one never creates a file - so a
catalog or live-schema failure cannot leave a freshly created or migrated state
store behind.

No durable state store and application database may resolve to the same file.
The check compares canonical paths and, on Unix, device and inode numbers, so
relative aliases, case aliases on case-insensitive filesystems, and hard links
are all rejected. A `database` grant can therefore never reach Krit's own
internal schema.

## Filesystem and security policy

- Paths are host-config-relative normal `.db` paths with no `.` or `..`, no
  symlink component, and no escape from the host config root, exactly like
  durable state paths.
- The containing directory must already exist and be owner-only on Unix; the
  database file must already exist and be an owner-only regular file.
- The connection is opened without `SQLITE_OPEN_CREATE` and without URI
  handling, so a path can never carry driver options.
- Extension loading is compiled out of the bundled build; defensive mode,
  `trusted_schema = OFF`, non-writable schema, and double-quoted-string rejection
  are all enabled. There is no system SQLite linkage.
- A page ceiling derived from the byte budget is installed at open, the current
  page count is verified against the budget, and `PRAGMA quick_check` must pass.
- A corrupt, foreign, oversized, or missing file fails closed. Krit never resets
  or replaces it.
- No path, SQL text, parameter value, row value, column value, or driver message
  ever enters artifacts, metadata, permission output, diagnostics, logs, stats,
  or error text. Connector errors are fixed operator-facing strings.

## Auditable facts

`krit explain` and the LSP durable-fact stream report every database operation
in source order: `database-begin-read` and `database-begin-write` carry the
literal database name as `store`, and `database-query` and `database-execute`
carry the literal statement name as `identity`. Operations that receive an
opaque transaction handle report no `store`, because the database name is not
present at that call site; the handle's origin is the preceding
`database-begin-*` fact. `database-commit` and `database-rollback` carry
neither. Facts never contain a path, SQL text, or a parameter value.

## WIT and artifact policy

`krit:runtime/database@0.2.0` is one typed interface holding the opaque
`transaction` resource and the six operations. Read and write authority share
that interface because both need the same lifecycle operations; the split lives
in the embedded requirement contract, which validation re-checks and the runtime
enforces on every call against both the manifest grant and the artifact's own
requirement set. An artifact whose database imports and declared database
effects disagree is rejected.

Database-enabled artifacts use validation policy 2. Existing worlds, imports,
metadata, and component bytes are unchanged when no database is used.

## Interruption and cleanup

Wasmtime's epoch deadline interrupts *guest* code. It cannot stop a statement
that is already executing inside SQLite, so every database operation
additionally installs a SQLite progress handler bounded by the earliest of the
transaction's own wall bound, the invocation deadline, and embedding
cancellation. A recursive CTE, a large scan, or a lock wait is therefore stopped
promptly, reported as `K5303`, and rolled back. Busy waiting is separately
clamped to the time actually remaining, so a lock wait can never outlast the
work it is waiting for.

Every invocation exit path - normal return, trap, deadline, cancellation,
invalid response, unclosed transaction, or a failed queue or schedule outcome -
rolls back every still-open transaction *before* the store is dropped or the
host is reused. A `Drop` fail-safe covers any unforeseen unwind. If a rollback
itself fails, the connector marks the database poisoned and refuses further
transactions rather than reusing a connection whose state is unknown; a
connection found inside a transaction at `begin` is likewise refused. Cleanup
never converts a failure into a success: an invocation that left a transaction
open still fails with `K5302`.

## Result encoding is incrementally bounded

Rows are encoded while the statement is stepped, never collected first. Every
append - including each JSON escape - is checked against the remaining byte
budget *before* any byte is copied, and stepping stops at the first breach. Host
memory for a query is therefore bounded by `maxResultBytes` plus one in-flight
column value even at the hard row, column, and cell maxima.

## On-disk footprint

The declared `maxDatabaseBytes` budget covers the main file **and** its
`-journal`, `-wal`, and `-shm` sidecars, measured together.

Write-ahead logging is refused. A WAL database's footprint is not bounded by its
main file: a long-lived reader can hold back checkpointing until the `-wal`
sidecar grows without limit, which would make the declared budget
unenforceable. Krit instead requires a rollback journal, installs
`journal_mode = TRUNCATE` with a journal size limit on read-write connections,
and truncates the journal at each commit. The budget is re-checked inside every
mutation - before the commit publishes - and again after commit, so an
over-budget write fails and rolls back rather than being published.

The trade-off is stated plainly: with a rollback journal a pinned reader
*serialises* writers instead of letting a log grow. A writer that cannot acquire
its lock fails with a bounded busy conflict (`K5303`) after its clamped busy
timeout. Krit prefers a bounded refusal over an unbounded file.

An operator converts a WAL database with `PRAGMA journal_mode = DELETE` before
configuring it.

## Limits

Protocol-1 hard maxima:

| Resource | Hard maximum |
|---|---:|
| Configured databases | 8 |
| Catalog statements per database | 64 |
| Statement SQL bytes | 4 KiB |
| Declared parameters per statement | 16 |
| Result columns per statement | 32 |
| Parameter bytes | 64 KiB |
| Result rows | 4,096 |
| Encoded result bytes | 256 KiB |
| Operations per transaction | 256 |
| Transactions per invocation | 8 (default 1) |
| Open transactions at once | 1 |
| Transaction wall bound | 5 s, and below the invocation deadline |
| SQLite busy timeout | 5 s, and below the transaction bound |
| Database bytes, including `-journal`/`-wal`/`-shm` | 64 KiB minimum, 1 GiB maximum |
| Rollback journal size limit | 8 MiB |

## Observability

Stats may report numeric database queries, executes, commits, rollbacks,
abandoned transactions, and a `databaseWriteCommitted` flag. They never report
SQL, parameters, rows, columns, database paths, or driver text.

## Non-goals

- raw guest SQL, DSNs, connection strings, driver options, or credentials
- schema ownership, migrations, `PRAGMA`, `ATTACH`, or `VACUUM` from Krit
- cross-database or cross-resource atomic transactions, two-phase commit, or XA
- non-SQLite backends in protocol 1; the source and WIT contracts are
  backend-neutral, but only the local SQLite connector exists today
- long-lived connections, pools, cursors, streaming results, or server-side state
- guest-visible isolation levels, savepoints, or nested transactions
- REAL or BLOB result values, and typed row values richer than bounded JSON text
