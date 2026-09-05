use std::{collections::BTreeMap, time::Duration};

use rusqlite::{OptionalExtension, Transaction, TransactionBehavior, params};

use crate::{
    DurableStore, MAX_RESOURCE_NAME_BYTES, Mutation, StateError, checked_deadline, map_database,
    map_transaction, next_sequence, query_revision, valid_millis, validate_identity,
    validate_mutations,
};

/// Hard bound on the number of rows one reservation scan may skip.
const MAX_RESERVATION_SCAN: usize = 1024;
/// Hard bound on the number of keys one deterministic listing may return.
pub const MAX_OBJECT_LIST_KEYS: usize = 1024;

pub const MAX_QUEUES: usize = 16;
pub const MAX_SCHEDULES: usize = 16;
pub const MAX_BUCKETS: usize = 16;
pub const MAX_QUEUE_DEPTH: usize = 65_536;
pub const MAX_QUEUE_JOB_BYTES: usize = 1024 * 1024;
pub const MAX_QUEUE_BYTES: usize = 256 * 1024 * 1024;
pub const MAX_DELIVERY_ATTEMPTS: u32 = 16;
pub const MAX_DELIVERY_LEASE: Duration = Duration::from_secs(5 * 60);
pub const MAX_DELIVERY_BACKOFF: Duration = Duration::from_secs(60 * 60);
pub const MAX_DEAD_LETTER_ENTRIES: usize = 4096;
pub const MAX_DEAD_LETTER_RETENTION: Duration = Duration::from_secs(30 * 24 * 60 * 60);
pub const MIN_SCHEDULE_INTERVAL: Duration = Duration::from_secs(1);
pub const MAX_SCHEDULE_INTERVAL: Duration = Duration::from_secs(365 * 24 * 60 * 60);
pub const MAX_SCHEDULE_CATCH_UP: u32 = 64;
pub const MAX_SCHEDULE_RETENTION: Duration = Duration::from_secs(30 * 24 * 60 * 60);
pub const MAX_RETAINED_FIRES: usize = 4096;
pub const MAX_BUCKET_OBJECTS: usize = 65_536;
pub const MAX_OBJECT_KEY_BYTES: usize = 1024;
pub const MAX_OBJECT_BYTES: usize = 4 * 1024 * 1024;
pub const MAX_BUCKET_BYTES: usize = 1024 * 1024 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct QueuePolicy {
    pub max_depth: usize,
    pub max_job_bytes: usize,
    pub max_queue_bytes: usize,
    pub max_attempts: u32,
    pub lease: Duration,
    pub backoff: Duration,
    pub max_backoff: Duration,
    pub dead_letter_max_entries: usize,
    pub dead_letter_ttl: Duration,
}

impl QueuePolicy {
    /// Validates every protocol bound without accessing a store.
    pub fn validate(self) -> Result<(), StateError> {
        if !(1..=MAX_QUEUE_DEPTH).contains(&self.max_depth)
            || !(1..=MAX_QUEUE_JOB_BYTES).contains(&self.max_job_bytes)
            || self.max_queue_bytes < self.max_job_bytes
            || self.max_queue_bytes > MAX_QUEUE_BYTES
            || !(1..=MAX_DELIVERY_ATTEMPTS).contains(&self.max_attempts)
            || !valid_millis(self.lease, MAX_DELIVERY_LEASE)
            || !valid_millis(self.backoff, MAX_DELIVERY_BACKOFF)
            || self.backoff > self.max_backoff
            || !valid_millis(self.max_backoff, MAX_DELIVERY_BACKOFF)
            || !(1..=MAX_DEAD_LETTER_ENTRIES).contains(&self.dead_letter_max_entries)
            || !valid_millis(self.dead_letter_ttl, MAX_DEAD_LETTER_RETENTION)
        {
            return Err(StateError::limit("durable queue policy is invalid"));
        }
        Ok(())
    }

    fn backoff_for(self, attempts: i64) -> Duration {
        backoff_for(self.backoff, self.max_backoff, attempts)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BucketPolicy {
    pub max_objects: usize,
    pub max_key_bytes: usize,
    pub max_object_bytes: usize,
    pub max_bucket_bytes: usize,
}

impl BucketPolicy {
    /// Validates every protocol bound without accessing a store.
    pub fn validate(self) -> Result<(), StateError> {
        if !(1..=MAX_BUCKET_OBJECTS).contains(&self.max_objects)
            || !(1..=MAX_OBJECT_KEY_BYTES).contains(&self.max_key_bytes)
            || !(1..=MAX_OBJECT_BYTES).contains(&self.max_object_bytes)
            || self.max_bucket_bytes < self.max_object_bytes
            || self.max_bucket_bytes > MAX_BUCKET_BYTES
        {
            return Err(StateError::limit("durable bucket policy is invalid"));
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SchedulePolicy {
    pub interval: Duration,
    pub start_millis: i64,
    pub max_catch_up: u32,
    pub max_attempts: u32,
    pub lease: Duration,
    pub backoff: Duration,
    pub max_backoff: Duration,
    pub retention: Duration,
    pub max_retained_fires: usize,
}

impl SchedulePolicy {
    /// Validates every protocol bound without accessing a store.
    pub fn validate(self) -> Result<(), StateError> {
        if self.interval < MIN_SCHEDULE_INTERVAL
            || !valid_millis(self.interval, MAX_SCHEDULE_INTERVAL)
            || self.start_millis < 0
            || !(1..=MAX_SCHEDULE_CATCH_UP).contains(&self.max_catch_up)
            || !(1..=MAX_DELIVERY_ATTEMPTS).contains(&self.max_attempts)
            || !valid_millis(self.lease, MAX_DELIVERY_LEASE)
            || !valid_millis(self.backoff, MAX_DELIVERY_BACKOFF)
            || self.backoff > self.max_backoff
            || !valid_millis(self.max_backoff, MAX_DELIVERY_BACKOFF)
            || !valid_millis(self.retention, MAX_SCHEDULE_RETENTION)
            || !(1..=MAX_RETAINED_FIRES).contains(&self.max_retained_fires)
        {
            return Err(StateError::limit("durable schedule policy is invalid"));
        }
        Ok(())
    }

    /// Checks every lease, retry, and retention instant before a tick or reservation.
    pub fn validate_instant(self, now_millis: i64) -> Result<(), StateError> {
        self.validate()?;
        preflight_instants(now_millis, &[self.lease, self.max_backoff, self.retention])
    }

    fn interval_millis(self) -> Option<i64> {
        i64::try_from(self.interval.as_millis())
            .ok()
            .filter(|millis| *millis > 0)
    }

    fn backoff_for(self, attempts: i64) -> Duration {
        backoff_for(self.backoff, self.max_backoff, attempts)
    }
}

fn backoff_for(base: Duration, maximum: Duration, attempts: i64) -> Duration {
    let exponent = u32::try_from(attempts.saturating_sub(1).clamp(0, 16)).unwrap_or(0);
    base.saturating_mul(2u32.saturating_pow(exponent))
        .min(maximum)
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct JobLease {
    id: [u8; 16],
    queue: String,
    owner: [u8; 16],
}

impl JobLease {
    pub const fn id(&self) -> &[u8; 16] {
        &self.id
    }

    pub fn queue(&self) -> &str {
        &self.queue
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct JobDelivery {
    pub lease: JobLease,
    pub body: Vec<u8>,
    pub attempt: u32,
    pub max_attempts: u32,
    pub enqueued_at_millis: i64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum JobDisposition {
    Retried { visible_at_millis: i64 },
    DeadLettered,
    Lost,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DeadLetterEntry {
    pub id: [u8; 16],
    pub attempts: u32,
    pub reason: String,
    pub failed_at_millis: i64,
    pub body: Vec<u8>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FireLease {
    schedule: String,
    due_at_millis: i64,
    owner: [u8; 16],
}

impl FireLease {
    pub fn schedule(&self) -> &str {
        &self.schedule
    }

    pub const fn due_at_millis(&self) -> i64 {
        self.due_at_millis
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FireDelivery {
    pub lease: FireLease,
    pub attempt: u32,
    pub max_attempts: u32,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ScheduleCatchUp {
    pub materialized: u32,
    pub skipped: u64,
    pub cursor_due_millis: i64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ObjectEntry {
    pub key: String,
    pub size_bytes: usize,
    pub updated_at_millis: i64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Completion {
    Job(JobLease),
    Fire(FireLease),
}

/// One atomic outcome boundary: staged mutations plus an optional delivery
/// acknowledgement that must commit or roll back together.
pub struct CommitPlan<'a> {
    pub expected_revision: u64,
    pub mutations: &'a [Mutation],
    pub queues: &'a BTreeMap<String, QueuePolicy>,
    pub buckets: &'a BTreeMap<String, BucketPolicy>,
    pub now_millis: i64,
    pub completion: Option<&'a Completion>,
}

impl DurableStore {
    pub fn commit(
        &self,
        expected_revision: u64,
        mutations: &[Mutation],
    ) -> Result<u64, StateError> {
        static NO_QUEUES: BTreeMap<String, QueuePolicy> = BTreeMap::new();
        static NO_BUCKETS: BTreeMap<String, BucketPolicy> = BTreeMap::new();
        self.commit_plan(CommitPlan {
            expected_revision,
            mutations,
            queues: &NO_QUEUES,
            buckets: &NO_BUCKETS,
            now_millis: 0,
            completion: None,
        })
    }

    pub fn commit_plan(&self, plan: CommitPlan<'_>) -> Result<u64, StateError> {
        let CommitPlan {
            expected_revision,
            mutations,
            queues,
            buckets,
            now_millis,
            completion,
        } = plan;
        validate_mutations(mutations, self.limits())?;
        if queues.len() > MAX_QUEUES || buckets.len() > MAX_BUCKETS {
            return Err(StateError::limit(
                "configured queues or buckets exceed the protocol bounds",
            ));
        }
        for policy in queues.values() {
            policy.validate()?;
        }
        for policy in buckets.values() {
            policy.validate()?;
        }
        let mut connection = self.lock()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(map_transaction)?;
        // Queue publications are ordered by `meta.sequence`, so a publish-only
        // outcome neither reads nor advances the shared state revision and
        // cannot conflict with an unrelated concurrent publisher.
        let revision_sensitive = mutations.iter().any(Mutation::advances_revision);
        let current = query_revision(&transaction)?;
        if revision_sensitive && current != expected_revision {
            return Err(StateError::conflict(
                "durable state revision changed before commit",
            ));
        }
        for mutation in mutations {
            apply_mutation(&transaction, mutation, queues, buckets, now_millis)?;
        }
        match completion {
            Some(Completion::Job(lease)) => {
                let changed = transaction
                    .execute(
                        "DELETE FROM queue_jobs WHERE id = ?1 AND queue = ?2 AND owner = ?3",
                        params![&lease.id[..], lease.queue, &lease.owner[..]],
                    )
                    .map_err(map_database)?;
                if changed != 1 {
                    return Err(StateError::conflict(
                        "durable queue lease is stale or not owned",
                    ));
                }
            }
            Some(Completion::Fire(lease)) => {
                let changed = transaction
                    .execute(
                        "UPDATE schedule_fires
                         SET status = 1, owner = NULL, lease_until = NULL, updated_at = ?1
                         WHERE schedule = ?2 AND due_at = ?3 AND status = 0 AND owner = ?4",
                        params![
                            now_millis,
                            lease.schedule,
                            lease.due_at_millis,
                            &lease.owner[..]
                        ],
                    )
                    .map_err(map_database)?;
                if changed != 1 {
                    return Err(StateError::conflict(
                        "durable schedule lease is stale or not owned",
                    ));
                }
            }
            None => {}
        }
        if !revision_sensitive {
            transaction.commit().map_err(map_database)?;
            return Ok(current);
        }
        let next = current
            .checked_add(1)
            .ok_or_else(|| StateError::limit("durable state revision overflowed"))?;
        let next_sql = i64::try_from(next)
            .map_err(|_| StateError::limit("durable state revision exceeds SQLite"))?;
        transaction
            .execute("UPDATE meta SET revision = ?1 WHERE id = 1", [next_sql])
            .map_err(map_database)?;
        transaction.commit().map_err(map_database)?;
        Ok(next)
    }

    pub fn reserve_job(
        &self,
        queue: &str,
        policy: QueuePolicy,
        owner: &[u8; 16],
        now_millis: i64,
    ) -> Result<Option<JobDelivery>, StateError> {
        policy.validate()?;
        validate_identity(queue, MAX_RESOURCE_NAME_BYTES)?;
        preflight_instants(
            now_millis,
            &[policy.lease, policy.max_backoff, policy.dead_letter_ttl],
        )?;
        let mut connection = self.lock()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(map_transaction)?;
        cleanup_dead_letters(&transaction, queue, policy, now_millis)?;
        let scan_limit = policy.max_depth.min(MAX_RESERVATION_SCAN);
        for _ in 0..scan_limit {
            let candidate = transaction
                .query_row(
                    "SELECT id, sequence, body, attempts, size_bytes, enqueued_at
                     FROM queue_jobs
                     WHERE queue = ?1 AND visible_at <= ?2
                       AND (lease_until IS NULL OR lease_until <= ?2)
                     ORDER BY sequence ASC LIMIT 1",
                    params![queue, now_millis],
                    |row| {
                        Ok((
                            row.get::<_, Vec<u8>>(0)?,
                            row.get::<_, i64>(1)?,
                            row.get::<_, Vec<u8>>(2)?,
                            row.get::<_, i64>(3)?,
                            row.get::<_, i64>(4)?,
                            row.get::<_, i64>(5)?,
                        ))
                    },
                )
                .optional()
                .map_err(map_database)?;
            let Some((id, sequence, body, attempts, size_bytes, enqueued_at)) = candidate else {
                transaction.commit().map_err(map_database)?;
                return Ok(None);
            };
            let id = job_id(&id)?;
            let next_attempt = attempts
                .checked_add(1)
                .ok_or_else(|| StateError::limit("durable queue attempt count overflowed"))?;
            if next_attempt > i64::from(policy.max_attempts) {
                dead_letter(
                    &transaction,
                    &id,
                    queue,
                    sequence,
                    &body,
                    attempts,
                    size_bytes,
                    "attempt limit exhausted before delivery",
                    now_millis,
                    policy,
                )?;
                continue;
            }
            transaction
                .execute(
                    "UPDATE queue_jobs SET attempts = ?1, owner = ?2, lease_until = ?3
                     WHERE id = ?4",
                    params![
                        next_attempt,
                        &owner[..],
                        checked_deadline(now_millis, policy.lease)?,
                        &id[..]
                    ],
                )
                .map_err(map_database)?;
            transaction.commit().map_err(map_database)?;
            return Ok(Some(JobDelivery {
                lease: JobLease {
                    id,
                    queue: queue.to_owned(),
                    owner: *owner,
                },
                body,
                attempt: u32::try_from(next_attempt).unwrap_or(u32::MAX),
                max_attempts: policy.max_attempts,
                enqueued_at_millis: enqueued_at,
            }));
        }
        // The scan bound was reached while terminalizing exhausted jobs. Commit
        // those terminal transitions and report no delivery; the next call
        // continues from the persisted state instead of wedging the queue.
        transaction.commit().map_err(map_database)?;
        Ok(None)
    }

    pub fn fail_job(
        &self,
        lease: &JobLease,
        reason: &str,
        policy: QueuePolicy,
        now_millis: i64,
    ) -> Result<JobDisposition, StateError> {
        policy.validate()?;
        preflight_instants(
            now_millis,
            &[policy.lease, policy.max_backoff, policy.dead_letter_ttl],
        )?;
        let reason = bounded_reason(reason);
        let mut connection = self.lock()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(map_transaction)?;
        let row = transaction
            .query_row(
                "SELECT sequence, body, attempts, size_bytes FROM queue_jobs
                 WHERE id = ?1 AND queue = ?2 AND owner = ?3",
                params![&lease.id[..], lease.queue, &lease.owner[..]],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, Vec<u8>>(1)?,
                        row.get::<_, i64>(2)?,
                        row.get::<_, i64>(3)?,
                    ))
                },
            )
            .optional()
            .map_err(map_database)?;
        let Some((sequence, body, attempts, size_bytes)) = row else {
            transaction.commit().map_err(map_database)?;
            return Ok(JobDisposition::Lost);
        };
        if attempts >= i64::from(policy.max_attempts) {
            dead_letter(
                &transaction,
                &lease.id,
                &lease.queue,
                sequence,
                &body,
                attempts,
                size_bytes,
                &reason,
                now_millis,
                policy,
            )?;
            transaction.commit().map_err(map_database)?;
            return Ok(JobDisposition::DeadLettered);
        }
        let visible_at = checked_deadline(now_millis, policy.backoff_for(attempts))?;
        transaction
            .execute(
                "UPDATE queue_jobs SET visible_at = ?1, owner = NULL, lease_until = NULL
                 WHERE id = ?2 AND queue = ?3 AND owner = ?4",
                params![visible_at, &lease.id[..], lease.queue, &lease.owner[..]],
            )
            .map_err(map_database)?;
        transaction.commit().map_err(map_database)?;
        Ok(JobDisposition::Retried {
            visible_at_millis: visible_at,
        })
    }

    pub fn queue_stats(&self, queue: &str) -> Result<(usize, usize), StateError> {
        validate_identity(queue, MAX_RESOURCE_NAME_BYTES)?;
        let connection = self.lock()?;
        let stats = connection
            .query_row(
                "SELECT COUNT(*), COALESCE(SUM(size_bytes), 0) FROM queue_jobs WHERE queue = ?1",
                [queue],
                |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
            )
            .map_err(map_database)?;
        Ok((usize_of(stats.0), usize_of(stats.1)))
    }

    pub fn dead_letters(
        &self,
        queue: &str,
        limit: usize,
    ) -> Result<Vec<DeadLetterEntry>, StateError> {
        validate_identity(queue, MAX_RESOURCE_NAME_BYTES)?;
        let limit = i64::try_from(limit.min(MAX_OBJECT_LIST_KEYS))
            .map_err(|_| StateError::limit("dead-letter listing limit is invalid"))?;
        let connection = self.lock()?;
        let mut statement = connection
            .prepare(
                "SELECT id, attempts, reason, failed_at, body FROM queue_dead
                 WHERE queue = ?1 ORDER BY sequence ASC LIMIT ?2",
            )
            .map_err(map_database)?;
        let entries = statement
            .query_map(params![queue, limit], |row| {
                Ok((
                    row.get::<_, Vec<u8>>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, Vec<u8>>(4)?,
                ))
            })
            .map_err(map_database)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(map_database)?;
        entries
            .into_iter()
            .map(|(id, attempts, reason, failed_at, body)| {
                Ok(DeadLetterEntry {
                    id: job_id(&id)?,
                    attempts: u32::try_from(attempts).unwrap_or(u32::MAX),
                    reason,
                    failed_at_millis: failed_at,
                    body,
                })
            })
            .collect()
    }

    /// Materializes every due schedule occurrence up to the catch-up bound.
    ///
    /// Occurrences are host-owned UTC epoch instants derived from the policy
    /// start and interval, so restarts and repeated ticks never create a second
    /// committed fire for the same instant.
    pub fn materialize_schedule(
        &self,
        schedule: &str,
        policy: SchedulePolicy,
        now_millis: i64,
    ) -> Result<ScheduleCatchUp, StateError> {
        policy.validate_instant(now_millis)?;
        validate_identity(schedule, MAX_RESOURCE_NAME_BYTES)?;
        let interval = policy
            .interval_millis()
            .ok_or_else(|| StateError::limit("durable schedule interval is invalid"))?;
        let horizon = occurrence_bounds(policy, interval, now_millis)?;
        let mut connection = self.lock()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(map_transaction)?;
        cleanup_schedule_fires(&transaction, schedule, policy, now_millis)?;
        if now_millis < policy.start_millis {
            transaction.commit().map_err(map_database)?;
            return Ok(ScheduleCatchUp {
                materialized: 0,
                skipped: 0,
                cursor_due_millis: policy.start_millis,
            });
        }
        let OccurrenceBounds {
            current_index,
            cursor_due,
        } = horizon.ok_or_else(|| StateError::limit("durable schedule instant overflowed"))?;
        let cursor: Option<i64> = transaction
            .query_row(
                "SELECT last_due_at FROM schedule_cursors WHERE schedule = ?1",
                [schedule],
                |row| row.get(0),
            )
            .optional()
            .map_err(map_database)?;
        let first_index = match cursor {
            Some(last_due) => {
                let last_index = last_due
                    .checked_sub(policy.start_millis)
                    .ok_or_else(|| StateError::limit("durable schedule cursor is invalid"))?
                    / interval;
                last_index
                    .checked_add(1)
                    .ok_or_else(|| StateError::limit("durable schedule cursor overflowed"))?
            }
            None => current_index,
        };
        if first_index > current_index {
            transaction.commit().map_err(map_database)?;
            return Ok(ScheduleCatchUp {
                materialized: 0,
                skipped: 0,
                cursor_due_millis: cursor_due,
            });
        }
        let pending = current_index - first_index + 1;
        let catch_up = i64::from(policy.max_catch_up);
        let (first_index, skipped) = if pending > catch_up {
            (current_index - catch_up + 1, pending - catch_up)
        } else {
            (first_index, 0)
        };
        let mut materialized = 0u32;
        for index in first_index..=current_index {
            let due = occurrence_instant(policy.start_millis, index, interval)?;
            let inserted = transaction
                .execute(
                    "INSERT INTO schedule_fires(
                        schedule, due_at, status, attempts, visible_at, lease_until,
                        owner, updated_at
                     ) VALUES(?1, ?2, 0, 0, ?2, NULL, NULL, ?3)
                     ON CONFLICT(schedule, due_at) DO NOTHING",
                    params![schedule, due, now_millis],
                )
                .map_err(map_database)?;
            materialized = materialized.saturating_add(u32::try_from(inserted).unwrap_or(0));
        }
        transaction
            .execute(
                "INSERT INTO schedule_cursors(schedule, last_due_at, updated_at)
                 VALUES(?1, ?2, ?3)
                 ON CONFLICT(schedule) DO UPDATE SET
                    last_due_at = excluded.last_due_at,
                    updated_at = excluded.updated_at",
                params![schedule, cursor_due, now_millis],
            )
            .map_err(map_database)?;
        transaction.commit().map_err(map_database)?;
        Ok(ScheduleCatchUp {
            materialized,
            skipped: u64::try_from(skipped).unwrap_or(0),
            cursor_due_millis: cursor_due,
        })
    }

    pub fn reserve_schedule_fire(
        &self,
        schedule: &str,
        policy: SchedulePolicy,
        owner: &[u8; 16],
        now_millis: i64,
    ) -> Result<Option<FireDelivery>, StateError> {
        policy.validate_instant(now_millis)?;
        validate_identity(schedule, MAX_RESOURCE_NAME_BYTES)?;
        let mut connection = self.lock()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(map_transaction)?;
        let scan_limit = policy.max_retained_fires.min(MAX_RESERVATION_SCAN);
        for _ in 0..scan_limit {
            let candidate = transaction
                .query_row(
                    "SELECT due_at, attempts FROM schedule_fires
                     WHERE schedule = ?1 AND status = 0 AND visible_at <= ?2
                       AND (lease_until IS NULL OR lease_until <= ?2)
                     ORDER BY due_at ASC LIMIT 1",
                    params![schedule, now_millis],
                    |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
                )
                .optional()
                .map_err(map_database)?;
            let Some((due_at, attempts)) = candidate else {
                transaction.commit().map_err(map_database)?;
                return Ok(None);
            };
            let next_attempt = attempts
                .checked_add(1)
                .ok_or_else(|| StateError::limit("durable schedule attempt overflowed"))?;
            if next_attempt > i64::from(policy.max_attempts) {
                transaction
                    .execute(
                        "UPDATE schedule_fires
                         SET status = 2, owner = NULL, lease_until = NULL, updated_at = ?1
                         WHERE schedule = ?2 AND due_at = ?3",
                        params![now_millis, schedule, due_at],
                    )
                    .map_err(map_database)?;
                continue;
            }
            transaction
                .execute(
                    "UPDATE schedule_fires
                     SET attempts = ?1, owner = ?2, lease_until = ?3, updated_at = ?4
                     WHERE schedule = ?5 AND due_at = ?6",
                    params![
                        next_attempt,
                        &owner[..],
                        checked_deadline(now_millis, policy.lease)?,
                        now_millis,
                        schedule,
                        due_at
                    ],
                )
                .map_err(map_database)?;
            transaction.commit().map_err(map_database)?;
            return Ok(Some(FireDelivery {
                lease: FireLease {
                    schedule: schedule.to_owned(),
                    due_at_millis: due_at,
                    owner: *owner,
                },
                attempt: u32::try_from(next_attempt).unwrap_or(u32::MAX),
                max_attempts: policy.max_attempts,
            }));
        }
        // See `reserve_job`: terminal fire transitions must survive the bound.
        transaction.commit().map_err(map_database)?;
        Ok(None)
    }

    pub fn fail_schedule_fire(
        &self,
        lease: &FireLease,
        policy: SchedulePolicy,
        now_millis: i64,
    ) -> Result<JobDisposition, StateError> {
        policy.validate_instant(now_millis)?;
        let mut connection = self.lock()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(map_transaction)?;
        let attempts: Option<i64> = transaction
            .query_row(
                "SELECT attempts FROM schedule_fires
                 WHERE schedule = ?1 AND due_at = ?2 AND status = 0 AND owner = ?3",
                params![lease.schedule, lease.due_at_millis, &lease.owner[..]],
                |row| row.get(0),
            )
            .optional()
            .map_err(map_database)?;
        let Some(attempts) = attempts else {
            transaction.commit().map_err(map_database)?;
            return Ok(JobDisposition::Lost);
        };
        let disposition = if attempts >= i64::from(policy.max_attempts) {
            transaction
                .execute(
                    "UPDATE schedule_fires
                     SET status = 2, owner = NULL, lease_until = NULL, updated_at = ?1
                     WHERE schedule = ?2 AND due_at = ?3",
                    params![now_millis, lease.schedule, lease.due_at_millis],
                )
                .map_err(map_database)?;
            JobDisposition::DeadLettered
        } else {
            let visible_at = checked_deadline(now_millis, policy.backoff_for(attempts))?;
            transaction
                .execute(
                    "UPDATE schedule_fires
                     SET visible_at = ?1, owner = NULL, lease_until = NULL, updated_at = ?2
                     WHERE schedule = ?3 AND due_at = ?4",
                    params![visible_at, now_millis, lease.schedule, lease.due_at_millis],
                )
                .map_err(map_database)?;
            JobDisposition::Retried {
                visible_at_millis: visible_at,
            }
        };
        transaction.commit().map_err(map_database)?;
        Ok(disposition)
    }

    /// Returns `(pending, completed, dead)` fire counts for one schedule.
    pub fn schedule_stats(&self, schedule: &str) -> Result<(usize, usize, usize), StateError> {
        validate_identity(schedule, MAX_RESOURCE_NAME_BYTES)?;
        let connection = self.lock()?;
        let stats = connection
            .query_row(
                "SELECT
                    COALESCE(SUM(status = 0), 0),
                    COALESCE(SUM(status = 1), 0),
                    COALESCE(SUM(status = 2), 0)
                 FROM schedule_fires WHERE schedule = ?1",
                [schedule],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, i64>(2)?,
                    ))
                },
            )
            .map_err(map_database)?;
        Ok((usize_of(stats.0), usize_of(stats.1), usize_of(stats.2)))
    }

    pub fn object(&self, bucket: &str, key: &str) -> Result<Option<Vec<u8>>, StateError> {
        validate_identity(bucket, MAX_RESOURCE_NAME_BYTES)?;
        crate::validate_key(key, self.limits())?;
        let connection = self.lock()?;
        connection
            .query_row(
                "SELECT value FROM objects WHERE bucket = ?1 AND key = ?2",
                params![bucket, key],
                |row| row.get(0),
            )
            .optional()
            .map_err(map_database)
    }

    pub fn object_at_revision(
        &self,
        bucket: &str,
        key: &str,
        expected_revision: u64,
    ) -> Result<Option<Vec<u8>>, StateError> {
        validate_identity(bucket, MAX_RESOURCE_NAME_BYTES)?;
        crate::validate_key(key, self.limits())?;
        let mut connection = self.lock()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Deferred)
            .map_err(map_transaction)?;
        if query_revision(&transaction)? != expected_revision {
            return Err(StateError::conflict(
                "durable state revision changed before object read",
            ));
        }
        let value = transaction
            .query_row(
                "SELECT value FROM objects WHERE bucket = ?1 AND key = ?2",
                params![bucket, key],
                |row| row.get(0),
            )
            .optional()
            .map_err(map_database)?;
        transaction.commit().map_err(map_database)?;
        Ok(value)
    }

    /// Returns `(object count, retained bytes)` for one bucket.
    pub fn object_stats(&self, bucket: &str) -> Result<(usize, usize), StateError> {
        validate_identity(bucket, MAX_RESOURCE_NAME_BYTES)?;
        let connection = self.lock()?;
        let stats = connection
            .query_row(
                "SELECT COUNT(*), COALESCE(SUM(size_bytes), 0) FROM objects WHERE bucket = ?1",
                [bucket],
                |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
            )
            .map_err(map_database)?;
        Ok((usize_of(stats.0), usize_of(stats.1)))
    }

    /// Deterministic bounded key listing ordered by byte-wise key.
    pub fn object_keys(
        &self,
        bucket: &str,
        prefix: &str,
        limit: usize,
    ) -> Result<Vec<ObjectEntry>, StateError> {
        validate_identity(bucket, MAX_RESOURCE_NAME_BYTES)?;
        if prefix.len() > self.limits().max_key_bytes || prefix.contains('\0') {
            return Err(StateError::limit("durable object prefix is invalid"));
        }
        if limit == 0 || limit > MAX_OBJECT_LIST_KEYS {
            return Err(StateError::limit(
                "durable object listing limit is outside its bound",
            ));
        }
        let connection = self.lock()?;
        // `substr` plus the default BINARY collation makes prefix matching
        // exact and case-sensitive; `%` and `_` are ordinary characters. The
        // `key >= ?2` bound keeps the bucket's primary-key scan seekable.
        let mut statement = connection
            .prepare(
                "SELECT key, size_bytes, updated_at FROM objects
                 WHERE bucket = ?1 AND key >= ?2 AND substr(key, 1, ?3) = ?2
                 ORDER BY key ASC LIMIT ?4",
            )
            .map_err(map_database)?;
        let prefix_chars = i64::try_from(prefix.chars().count())
            .map_err(|_| StateError::limit("durable object prefix is invalid"))?;
        let limit = i64::try_from(limit)
            .map_err(|_| StateError::limit("durable object listing limit is invalid"))?;
        statement
            .query_map(params![bucket, prefix, prefix_chars, limit], |row| {
                Ok(ObjectEntry {
                    key: row.get(0)?,
                    size_bytes: usize_of(row.get::<_, i64>(1)?),
                    updated_at_millis: row.get(2)?,
                })
            })
            .map_err(map_database)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(map_database)
    }
}

fn apply_mutation(
    transaction: &Transaction<'_>,
    mutation: &Mutation,
    queues: &BTreeMap<String, QueuePolicy>,
    buckets: &BTreeMap<String, BucketPolicy>,
    now_millis: i64,
) -> Result<(), StateError> {
    match mutation {
        Mutation::Put { key, value } => {
            transaction
                .execute(
                    "INSERT INTO kv(key, value) VALUES(?1, ?2)
                     ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                    params![key, value],
                )
                .map_err(map_database)?;
        }
        Mutation::Delete { key } => {
            transaction
                .execute("DELETE FROM kv WHERE key = ?1", [key])
                .map_err(map_database)?;
        }
        Mutation::CheckpointPut { name, value } => {
            transaction
                .execute(
                    "INSERT INTO checkpoints(name, value) VALUES(?1, ?2)
                     ON CONFLICT(name) DO UPDATE SET value = excluded.value",
                    params![name, value],
                )
                .map_err(map_database)?;
        }
        Mutation::ObjectPut { bucket, key, value } => {
            let policy = *buckets
                .get(bucket)
                .ok_or_else(|| StateError::limit("durable object bucket is not configured"))?;
            if key.len() > policy.max_key_bytes || value.len() > policy.max_object_bytes {
                return Err(StateError::limit(
                    "durable object key or value exceeds its bucket bound",
                ));
            }
            let previous: Option<i64> = transaction
                .query_row(
                    "SELECT size_bytes FROM objects WHERE bucket = ?1 AND key = ?2",
                    params![bucket, key],
                    |row| row.get(0),
                )
                .optional()
                .map_err(map_database)?;
            let (count, bytes) = transaction
                .query_row(
                    "SELECT COUNT(*), COALESCE(SUM(size_bytes), 0) FROM objects
                     WHERE bucket = ?1",
                    [bucket],
                    |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
                )
                .map_err(map_database)?;
            if previous.is_none() && usize_of(count) >= policy.max_objects {
                return Err(StateError::limit(
                    "durable object count exceeds its bucket bound",
                ));
            }
            let retained = usize_of(bytes)
                .checked_sub(usize_of(previous.unwrap_or(0)))
                .and_then(|retained| retained.checked_add(value.len()))
                .ok_or_else(|| StateError::limit("durable object accounting overflowed"))?;
            if retained > policy.max_bucket_bytes {
                return Err(StateError::limit(
                    "durable object bytes exceed their bucket bound",
                ));
            }
            transaction
                .execute(
                    "INSERT INTO objects(bucket, key, value, size_bytes, updated_at)
                     VALUES(?1, ?2, ?3, ?4, ?5)
                     ON CONFLICT(bucket, key) DO UPDATE SET
                        value = excluded.value,
                        size_bytes = excluded.size_bytes,
                        updated_at = excluded.updated_at",
                    params![
                        bucket,
                        key,
                        value,
                        i64::try_from(value.len())
                            .map_err(|_| StateError::limit("durable object size exceeds SQLite"))?,
                        now_millis
                    ],
                )
                .map_err(map_database)?;
        }
        Mutation::ObjectDelete { bucket, key } => {
            if !buckets.contains_key(bucket) {
                return Err(StateError::limit("durable object bucket is not configured"));
            }
            transaction
                .execute(
                    "DELETE FROM objects WHERE bucket = ?1 AND key = ?2",
                    params![bucket, key],
                )
                .map_err(map_database)?;
        }
        Mutation::QueuePublish { queue, id, body } => {
            let policy = *queues
                .get(queue)
                .ok_or_else(|| StateError::limit("durable queue is not configured"))?;
            if body.len() > policy.max_job_bytes {
                return Err(StateError::limit(
                    "durable queue job exceeds its byte bound",
                ));
            }
            let (count, bytes) = transaction
                .query_row(
                    "SELECT COUNT(*), COALESCE(SUM(size_bytes), 0) FROM queue_jobs
                     WHERE queue = ?1",
                    [queue],
                    |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
                )
                .map_err(map_database)?;
            if usize_of(count) >= policy.max_depth {
                return Err(StateError::limit("durable queue depth is exhausted"));
            }
            if usize_of(bytes)
                .checked_add(body.len())
                .is_none_or(|total| total > policy.max_queue_bytes)
            {
                return Err(StateError::limit("durable queue bytes exceed their bound"));
            }
            let sequence = next_sequence(transaction)?;
            let inserted = transaction
                .execute(
                    "INSERT INTO queue_jobs(
                        id, queue, sequence, body, attempts, visible_at,
                        lease_until, owner, size_bytes, enqueued_at
                     ) VALUES(?1, ?2, ?3, ?4, 0, ?5, NULL, NULL, ?6, ?5)
                     ON CONFLICT(id) DO NOTHING",
                    params![
                        &id[..],
                        queue,
                        sequence,
                        body,
                        now_millis,
                        i64::try_from(body.len()).map_err(|_| StateError::limit(
                            "durable queue job size exceeds SQLite"
                        ))?
                    ],
                )
                .map_err(map_database)?;
            if inserted != 1 {
                return Err(StateError::conflict(
                    "durable queue job identity is already present",
                ));
            }
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn dead_letter(
    transaction: &Transaction<'_>,
    id: &[u8; 16],
    queue: &str,
    sequence: i64,
    body: &[u8],
    attempts: i64,
    size_bytes: i64,
    reason: &str,
    now_millis: i64,
    policy: QueuePolicy,
) -> Result<(), StateError> {
    transaction
        .execute("DELETE FROM queue_jobs WHERE id = ?1", [&id[..]])
        .map_err(map_database)?;
    transaction
        .execute(
            "INSERT INTO queue_dead(
                id, queue, sequence, body, attempts, reason, size_bytes, failed_at
             ) VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
             ON CONFLICT(id) DO UPDATE SET
                attempts = excluded.attempts,
                reason = excluded.reason,
                failed_at = excluded.failed_at",
            params![
                &id[..],
                queue,
                sequence,
                body,
                attempts,
                reason,
                size_bytes,
                now_millis
            ],
        )
        .map_err(map_database)?;
    enforce_dead_letter_bounds(transaction, queue, policy)
}

fn cleanup_dead_letters(
    transaction: &Transaction<'_>,
    queue: &str,
    policy: QueuePolicy,
    now_millis: i64,
) -> Result<(), StateError> {
    let ttl = i64::try_from(policy.dead_letter_ttl.as_millis())
        .map_err(|_| StateError::limit("dead-letter retention exceeds i64"))?;
    let horizon = now_millis
        .checked_sub(ttl)
        .ok_or_else(|| StateError::limit("dead-letter retention horizon overflowed"))?;
    transaction
        .execute(
            "DELETE FROM queue_dead WHERE queue = ?1 AND failed_at <= ?2",
            params![queue, horizon],
        )
        .map_err(map_database)?;
    enforce_dead_letter_bounds(transaction, queue, policy)
}

fn enforce_dead_letter_bounds(
    transaction: &Transaction<'_>,
    queue: &str,
    policy: QueuePolicy,
) -> Result<(), StateError> {
    let limit = i64::try_from(policy.dead_letter_max_entries)
        .map_err(|_| StateError::limit("dead-letter entry bound exceeds i64"))?;
    transaction
        .execute(
            "DELETE FROM queue_dead
             WHERE queue = ?1 AND id IN (
                SELECT id FROM queue_dead WHERE queue = ?1
                ORDER BY sequence DESC LIMIT -1 OFFSET ?2
             )",
            params![queue, limit],
        )
        .map_err(map_database)?;
    Ok(())
}

fn cleanup_schedule_fires(
    transaction: &Transaction<'_>,
    schedule: &str,
    policy: SchedulePolicy,
    now_millis: i64,
) -> Result<(), StateError> {
    let retention = i64::try_from(policy.retention.as_millis())
        .map_err(|_| StateError::limit("schedule retention exceeds i64"))?;
    let horizon = now_millis
        .checked_sub(retention)
        .ok_or_else(|| StateError::limit("schedule retention horizon overflowed"))?;
    transaction
        .execute(
            "DELETE FROM schedule_fires
             WHERE schedule = ?1 AND status IN (1, 2) AND updated_at <= ?2",
            params![schedule, horizon],
        )
        .map_err(map_database)?;
    let limit = i64::try_from(policy.max_retained_fires)
        .map_err(|_| StateError::limit("schedule retention bound exceeds i64"))?;
    transaction
        .execute(
            "DELETE FROM schedule_fires
             WHERE schedule = ?1 AND status IN (1, 2) AND due_at IN (
                SELECT due_at FROM schedule_fires
                WHERE schedule = ?1 AND status IN (1, 2)
                ORDER BY due_at DESC LIMIT -1 OFFSET ?2
             )",
            params![schedule, limit],
        )
        .map_err(map_database)?;
    Ok(())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct OccurrenceBounds {
    current_index: i64,
    cursor_due: i64,
}

/// Rejects a tick whose instants cannot be represented before anything moves.
fn preflight_instants(now_millis: i64, durations: &[Duration]) -> Result<(), StateError> {
    for duration in durations {
        checked_deadline(now_millis, *duration)?;
        let millis = i64::try_from(duration.as_millis())
            .map_err(|_| StateError::limit("durable retention exceeds i64"))?;
        now_millis
            .checked_sub(millis)
            .ok_or_else(|| StateError::limit("durable retention horizon overflowed"))?;
    }
    Ok(())
}

fn occurrence_instant(start_millis: i64, index: i64, interval: i64) -> Result<i64, StateError> {
    index
        .checked_mul(interval)
        .and_then(|offset| start_millis.checked_add(offset))
        .ok_or_else(|| StateError::limit("durable schedule instant overflowed"))
}

/// Returns the current occurrence index and instant, or `None` before the
/// configured start. Every product is checked, never saturated.
fn occurrence_bounds(
    policy: SchedulePolicy,
    interval: i64,
    now_millis: i64,
) -> Result<Option<OccurrenceBounds>, StateError> {
    if now_millis < policy.start_millis {
        return Ok(None);
    }
    let elapsed = now_millis
        .checked_sub(policy.start_millis)
        .ok_or_else(|| StateError::limit("durable schedule instant overflowed"))?;
    let current_index = elapsed / interval;
    let cursor_due = occurrence_instant(policy.start_millis, current_index, interval)?;
    Ok(Some(OccurrenceBounds {
        current_index,
        cursor_due,
    }))
}

fn job_id(bytes: &[u8]) -> Result<[u8; 16], StateError> {
    <[u8; 16]>::try_from(bytes)
        .map_err(|_| StateError::database("durable queue job identity is invalid"))
}

fn bounded_reason(reason: &str) -> String {
    const MAX_REASON_BYTES: usize = 256;
    let mut end = reason.len().min(MAX_REASON_BYTES);
    while end > 0 && !reason.is_char_boundary(end) {
        end -= 1;
    }
    reason[..end].replace(['\n', '\r', '\0'], " ")
}

fn usize_of(value: i64) -> usize {
    usize::try_from(value.max(0)).unwrap_or(usize::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_grows_geometrically_and_saturates_at_the_bound() {
        let base = Duration::from_millis(100);
        let maximum = Duration::from_millis(500);

        assert_eq!(backoff_for(base, maximum, 1), Duration::from_millis(100));
        assert_eq!(backoff_for(base, maximum, 2), Duration::from_millis(200));
        assert_eq!(backoff_for(base, maximum, 3), Duration::from_millis(400));
        assert_eq!(backoff_for(base, maximum, 40), maximum);
    }

    #[test]
    fn reasons_stay_single_line_and_bounded() {
        assert_eq!(bounded_reason("one\ntwo"), "one two");
        assert!(bounded_reason(&"x".repeat(4096)).len() <= 256);
    }

    #[test]
    fn extreme_instants_are_rejected_before_any_mutation() {
        let policy = SchedulePolicy {
            interval: Duration::from_secs(60),
            start_millis: 0,
            max_catch_up: 2,
            max_attempts: 2,
            lease: Duration::from_secs(30),
            backoff: Duration::from_secs(1),
            max_backoff: Duration::from_secs(8),
            retention: Duration::from_secs(3600),
            max_retained_fires: 8,
        };

        assert!(preflight_instants(i64::MAX, &[policy.lease]).is_err());
        assert!(preflight_instants(i64::MAX, &[policy.max_backoff]).is_err());
        assert!(preflight_instants(i64::MIN, &[policy.retention]).is_err());
        assert!(preflight_instants(3_600_000, &[policy.lease, policy.retention]).is_ok());
        assert!(occurrence_instant(i64::MAX - 1, 2, 60_000).is_err());
        assert_eq!(occurrence_instant(1_000, 3, 60_000).unwrap(), 181_000);
    }
}
