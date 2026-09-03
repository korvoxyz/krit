use std::{
    collections::BTreeSet,
    sync::{Arc, atomic::Ordering},
    thread,
    time::Instant,
};

use krit_state::{Completion, JobDisposition, ScheduleCatchUp};
use krit_wasm::{ArtifactMetadata, JOB_INTERFACE, SCHEDULE_INTERFACE, validate_artifact};
use serde::Serialize;
use wasmtime::{
    Store,
    component::{Component, HasSelf, Linker},
};

use crate::{
    DeadlineWorker, ExecutionStats, GrantSet, HOST_STACK_HEADROOM_BYTES, HostState,
    HostStateConfig, LogEvent, Runtime, RuntimeError, STATE_HOST_POLICY_VERSION, bindings,
    hex_identity, map_wasmtime_error, policy,
    policy::{AgentHost, CancellationHandle},
};

/// Hard bound on the bytes one delivery outcome detail may carry.
pub const MAX_OUTCOME_DETAIL_BYTES: usize = 4 * 1024;

/// Deterministic disposition of one durable delivery attempt.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", tag = "outcome")]
pub enum DeliveryOutcome {
    /// No reservable work existed for this queue or schedule at this instant.
    Idle,
    /// The guest acknowledged the delivery and its state committed.
    Completed { id: String, attempt: u32 },
    /// The guest reported failure and the host scheduled another attempt.
    Retried {
        id: String,
        attempt: u32,
        visible_at_millis: i64,
    },
    /// The attempt budget was exhausted and the delivery moved to its terminal
    /// dead-letter outcome.
    DeadLettered { id: String, attempt: u32 },
}

impl DeliveryOutcome {
    pub const fn is_idle(&self) -> bool {
        matches!(self, Self::Idle)
    }

    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Completed { .. } => "completed",
            Self::Retried { .. } => "retried",
            Self::DeadLettered { .. } => "dead-lettered",
        }
    }
}

#[derive(Debug, Eq, PartialEq)]
pub struct DeliveryExecutionResult {
    pub outcome: DeliveryOutcome,
    /// Bounded guest-supplied outcome detail. Empty when the host produced the
    /// outcome without a guest result.
    pub detail: String,
    pub output: Vec<u8>,
    pub events: Vec<LogEvent>,
    pub stats: ExecutionStats,
}

/// Typed queue delivery facts handed to the guest. The host owns every value.
#[derive(Clone, Debug, Eq, PartialEq)]
struct JobEvent {
    queue: String,
    id: String,
    body: String,
    attempt: i64,
    max_attempts: i64,
}

/// Typed schedule trigger facts handed to the guest.
#[derive(Clone, Debug, Eq, PartialEq)]
struct ScheduleEvent {
    schedule: String,
    id: String,
    scheduled_at_millis: i64,
    fired_at_millis: i64,
    attempt: i64,
    max_attempts: i64,
}

/// One bounded delivery dispatch request.
///
/// `now_millis` is the host-supplied UTC wall instant; guests never read a
/// clock. Every state change commits only at the outcome boundary.
#[derive(Clone, Copy)]
pub struct DeliveryRequest<'a> {
    pub bytes: &'a [u8],
    pub metadata: &'a ArtifactMetadata,
    pub grants: &'a GrantSet,
    pub agent_host: &'a AgentHost,
    /// Queue or schedule name the artifact must already require.
    pub resource: &'a str,
    pub now_millis: i64,
    pub cancellation: &'a CancellationHandle,
}

struct DeliveryPlan<'a> {
    request: DeliveryRequest<'a>,
    store: String,
    call: DeliveryCall,
}

impl Runtime {
    /// Reserves and dispatches at most one durable queue delivery.
    pub fn dispatch_job(
        &self,
        request: DeliveryRequest<'_>,
    ) -> Result<DeliveryExecutionResult, RuntimeError> {
        let DeliveryRequest {
            agent_host,
            resource: queue,
            now_millis,
            ..
        } = request;
        let component = self.prepare_delivery(request, JOB_INTERFACE, "queue.consume")?;
        let durable = agent_host.durable_state().clone();
        let (binding, policy) = durable.queue(queue)?;
        let store_name = durable.queue_store(queue)?;
        let owner = agent_host.next_lease_owner();
        let Some(delivery) = binding
            .store()
            .reserve_job(queue, policy, &owner, now_millis)
            .map_err(crate::state::map_state_error)?
        else {
            return Ok(self.idle_result());
        };
        let identity = hex_identity(delivery.lease.id());
        let body = String::from_utf8(delivery.body.clone()).map_err(|_| {
            RuntimeError::durable_state("durable queue job body is not valid UTF-8")
        })?;
        let event = JobEvent {
            queue: queue.to_owned(),
            id: identity.clone(),
            body,
            attempt: i64::from(delivery.attempt),
            max_attempts: i64::from(delivery.max_attempts),
        };
        let execution = self.run_delivery(
            &component,
            DeliveryPlan {
                request,
                store: store_name,
                call: DeliveryCall::Job(event),
            },
        );
        let failure_reason = |detail: &str| {
            if detail.is_empty() {
                "guest reported a delivery failure".to_owned()
            } else {
                detail.to_owned()
            }
        };
        match execution {
            Ok(mut completed) => match completed.result {
                Ok(detail) => {
                    let detail = bounded_detail(&detail);
                    completed
                        .state
                        .commit_outcome(
                            &durable,
                            Some(&Completion::Job(delivery.lease.clone())),
                            now_millis,
                        )
                        .map_err(|error| error.with_events(completed.events.clone()))?;
                    Ok(DeliveryExecutionResult {
                        outcome: DeliveryOutcome::Completed {
                            id: identity,
                            attempt: delivery.attempt,
                        },
                        detail,
                        output: completed.output,
                        events: completed.events,
                        stats: completed.stats,
                    })
                }
                Err(detail) => {
                    let detail = bounded_detail(&detail);
                    let disposition = binding
                        .store()
                        .fail_job(
                            &delivery.lease,
                            &failure_reason(&detail),
                            policy,
                            now_millis,
                        )
                        .map_err(crate::state::map_state_error)
                        .map_err(|error| error.with_events(completed.events.clone()))?;
                    Ok(DeliveryExecutionResult {
                        outcome: disposition_outcome(disposition, identity, delivery.attempt)?,
                        detail,
                        output: completed.output,
                        events: completed.events,
                        stats: completed.stats,
                    })
                }
            },
            Err(error) => {
                binding
                    .store()
                    .fail_job(
                        &delivery.lease,
                        "guest execution failed before acknowledgement",
                        policy,
                        now_millis,
                    )
                    .map_err(crate::state::map_state_error)?;
                Err(error)
            }
        }
    }

    /// Materializes due schedule occurrences and dispatches at most one fire.
    pub fn dispatch_schedule(
        &self,
        request: DeliveryRequest<'_>,
    ) -> Result<(ScheduleCatchUp, DeliveryExecutionResult), RuntimeError> {
        let DeliveryRequest {
            agent_host,
            resource: schedule,
            now_millis,
            ..
        } = request;
        let component = self.prepare_delivery(request, SCHEDULE_INTERFACE, "schedule.trigger")?;
        let durable = agent_host.durable_state().clone();
        let (binding, policy) = durable.schedule(schedule)?;
        let store_name = durable.schedule_store(schedule)?;
        let catch_up = binding
            .store()
            .materialize_schedule(schedule, policy, now_millis)
            .map_err(crate::state::map_state_error)?;
        let owner = agent_host.next_lease_owner();
        let Some(delivery) = binding
            .store()
            .reserve_schedule_fire(schedule, policy, &owner, now_millis)
            .map_err(crate::state::map_state_error)?
        else {
            return Ok((catch_up, self.idle_result()));
        };
        let identity = format!("{schedule}@{}", delivery.lease.due_at_millis());
        let event = ScheduleEvent {
            schedule: schedule.to_owned(),
            id: identity.clone(),
            scheduled_at_millis: delivery.lease.due_at_millis(),
            fired_at_millis: now_millis,
            attempt: i64::from(delivery.attempt),
            max_attempts: i64::from(delivery.max_attempts),
        };
        let execution = self.run_delivery(
            &component,
            DeliveryPlan {
                request,
                store: store_name,
                call: DeliveryCall::Schedule(event),
            },
        );
        match execution {
            Ok(mut completed) => match completed.result {
                Ok(detail) => {
                    let detail = bounded_detail(&detail);
                    completed
                        .state
                        .commit_outcome(
                            &durable,
                            Some(&Completion::Fire(delivery.lease.clone())),
                            now_millis,
                        )
                        .map_err(|error| error.with_events(completed.events.clone()))?;
                    Ok((
                        catch_up,
                        DeliveryExecutionResult {
                            outcome: DeliveryOutcome::Completed {
                                id: identity,
                                attempt: delivery.attempt,
                            },
                            detail,
                            output: completed.output,
                            events: completed.events,
                            stats: completed.stats,
                        },
                    ))
                }
                Err(detail) => {
                    let detail = bounded_detail(&detail);
                    let disposition = binding
                        .store()
                        .fail_schedule_fire(&delivery.lease, policy, now_millis)
                        .map_err(crate::state::map_state_error)
                        .map_err(|error| error.with_events(completed.events.clone()))?;
                    Ok((
                        catch_up,
                        DeliveryExecutionResult {
                            outcome: disposition_outcome(disposition, identity, delivery.attempt)?,
                            detail,
                            output: completed.output,
                            events: completed.events,
                            stats: completed.stats,
                        },
                    ))
                }
            },
            Err(error) => {
                binding
                    .store()
                    .fail_schedule_fire(&delivery.lease, policy, now_millis)
                    .map_err(crate::state::map_state_error)?;
                Err(error)
            }
        }
    }

    fn prepare_delivery(
        &self,
        request: DeliveryRequest<'_>,
        export: &str,
        capability: &str,
    ) -> Result<Component, RuntimeError> {
        let DeliveryRequest {
            bytes,
            metadata,
            grants,
            agent_host,
            resource,
            cancellation,
            ..
        } = request;
        self.validate_inputs(bytes, metadata)?;
        let inspection = validate_artifact(bytes, metadata)?;
        grants.authorize(metadata)?;
        self.preflight_resources(&inspection)?;
        self.validate_agent_host(grants, metadata, agent_host)?;
        if inspection.exports != [export] {
            return Err(RuntimeError::import_mismatch(
                "artifact does not export the requested typed entrypoint interface",
            ));
        }
        if !metadata.requirements.iter().any(|requirement| {
            requirement.capability == capability && requirement.resource == resource
        }) {
            return Err(RuntimeError::authorization(
                "artifact does not require the requested delivery resource",
            ));
        }
        if !grants.grants(capability, Some(resource)) {
            return Err(RuntimeError::authorization(
                "delivery resource is not granted by the manifest",
            ));
        }
        if cancellation.is_cancelled() {
            return Err(RuntimeError::cancelled(
                "embedding cancellation requested before guest execution",
            ));
        }
        Component::new(&self.engine, bytes).map_err(|error| {
            RuntimeError::setup(format!(
                "validated delivery component could not be compiled by Wasmtime 47.x: {error}"
            ))
        })
    }

    fn run_delivery(
        &self,
        component: &Component,
        plan: DeliveryPlan<'_>,
    ) -> Result<CompletedDelivery, RuntimeError> {
        let _epoch_guard = self
            .epoch_lock
            .lock()
            .map_err(|_| RuntimeError::setup("runtime epoch scheduler lock is poisoned"))?;
        let worker_stack_bytes = self
            .limits
            .wasm_stack_bytes()
            .checked_add(HOST_STACK_HEADROOM_BYTES)
            .ok_or_else(|| RuntimeError::setup("Wasm execution worker stack size overflowed"))?;
        thread::scope(|scope| {
            let worker = thread::Builder::new()
                .name("krit-delivery-execution".to_owned())
                .stack_size(worker_stack_bytes)
                .spawn_scoped(scope, || self.invoke_delivery_component(component, &plan))
                .map_err(|error| {
                    RuntimeError::setup(format!(
                        "could not start isolated delivery execution worker: {error}"
                    ))
                })?;
            worker
                .join()
                .map_err(|_| RuntimeError::setup("isolated delivery execution worker panicked"))?
        })
    }

    fn invoke_delivery_component(
        &self,
        component: &Component,
        plan: &DeliveryPlan<'_>,
    ) -> Result<CompletedDelivery, RuntimeError> {
        let started = Instant::now();
        let metadata = plan.request.metadata;
        let durable = plan.request.agent_host.durable_state().clone();
        let mut store = Store::new(
            &self.engine,
            self.new_host_state(HostStateConfig {
                grants: Some(plan.request.grants.clone()),
                effects: metadata.effects.iter().cloned().collect(),
                requirements: metadata
                    .requirements
                    .iter()
                    .map(|requirement| {
                        (requirement.capability.clone(), requirement.resource.clone())
                    })
                    .collect::<BTreeSet<_>>(),
                agent_host: plan.request.agent_host.clone(),
                cancellation: plan.request.cancellation.clone(),
                started,
                artifact_identity: policy::artifact_identity(metadata),
            }),
        );
        store.limiter(|state| &mut state.store_limits);
        store
            .set_fuel(self.limits.fuel())
            .map_err(|error| RuntimeError::setup(format!("could not set Wasm fuel: {error}")))?;
        store.set_epoch_deadline(1);
        store.epoch_deadline_trap();
        store.data_mut().state.bind(&durable, &plan.store)?;
        let deadline = DeadlineWorker::start(
            self.engine.clone(),
            self.limits.deadline(),
            Arc::clone(&self.active_deadline_workers),
        )?;
        let outcome = match &plan.call {
            DeliveryCall::Job(event) => self.call_job(component, &mut store, event),
            DeliveryCall::Schedule(event) => self.call_schedule(component, &mut store, event),
        };
        let timed_out = deadline
            .finish()
            .map_err(|error| error.with_events(store.data().events.clone()))?;
        if timed_out {
            return Err(RuntimeError::deadline("Wasm wall deadline exceeded")
                .with_events(store.data().events.clone()));
        }
        let result = outcome.map_err(|error| error.with_events(store.data().events.clone()))?;
        if store.data().cancellation.is_cancelled() {
            return Err(RuntimeError::cancelled(
                "embedding cancellation requested before durable outcome commit",
            )
            .with_events(store.data().events.clone()));
        }
        for detail in [result.as_ref().ok(), result.as_ref().err()]
            .into_iter()
            .flatten()
        {
            if detail.len() > MAX_OUTCOME_DETAIL_BYTES {
                return Err(RuntimeError::delivery(format!(
                    "delivery outcome detail exceeds the {MAX_OUTCOME_DETAIL_BYTES}-byte limit"
                ))
                .with_events(store.data().events.clone()));
            }
        }
        let remaining = store.get_fuel().map_err(|error| {
            RuntimeError::setup(format!("could not read remaining fuel: {error}"))
                .with_events(store.data().events.clone())
        })?;
        let mut state = store.into_data();
        let stats = ExecutionStats {
            policy_version: STATE_HOST_POLICY_VERSION,
            fuel_budget: self.limits.fuel(),
            fuel_consumed: self.limits.fuel().saturating_sub(remaining),
            fuel_remaining: remaining,
            host_calls: state.host_calls,
            http_calls: state.http_calls,
            ai_calls: state.ai_calls,
            network_attempts: state.network_attempts,
            retries: state.retries,
            rate_limit_denials: state.rate_limit_denials,
            idempotency_replayed: false,
            state_operations: state.state.operations(),
            state_reads: state.state.reads(),
            state_writes: state.state.writes(),
            checkpoint_reads: state.state.checkpoint_reads(),
            checkpoint_writes: state.state.checkpoint_writes(),
            replay_hits: state.state.replay_hits(),
            replay_misses: state.state.replay_misses(),
            object_reads: state.state.object_reads(),
            object_writes: state.state.object_writes(),
            queue_publishes: state.state.queue_publishes(),
            output_bytes: state.output.len(),
            elapsed_micros: started.elapsed().as_micros(),
        };
        Ok(CompletedDelivery {
            result,
            output: std::mem::take(&mut state.output),
            events: std::mem::take(&mut state.events),
            stats,
            state: state.state,
        })
    }

    fn call_job(
        &self,
        component: &Component,
        store: &mut Store<HostState>,
        event: &JobEvent,
    ) -> Result<Result<String, String>, RuntimeError> {
        let mut linker = Linker::new(&self.engine);
        bindings::job::JobHostProgram::add_to_linker::<_, HasSelf<_>>(&mut linker, |state| state)
            .map_err(|error| {
            RuntimeError::import_mismatch(format!(
                "could not link the exact Krit job host interfaces: {error}"
            ))
        })?;
        let program = bindings::job::JobHostProgram::instantiate(&mut *store, component, &linker)
            .map_err(map_wasmtime_error)?;
        let delivery = bindings::job::exports::krit::runtime::job::Delivery {
            queue: event.queue.clone(),
            id: event.id.clone(),
            body: event.body.clone(),
            attempt: event.attempt,
            max_attempts: event.max_attempts,
        };
        program
            .krit_runtime_job()
            .call_handle(&mut *store, &delivery)
            .map_err(map_wasmtime_error)
    }

    fn call_schedule(
        &self,
        component: &Component,
        store: &mut Store<HostState>,
        event: &ScheduleEvent,
    ) -> Result<Result<String, String>, RuntimeError> {
        let mut linker = Linker::new(&self.engine);
        bindings::schedule::ScheduleHostProgram::add_to_linker::<_, HasSelf<_>>(
            &mut linker,
            |state| state,
        )
        .map_err(|error| {
            RuntimeError::import_mismatch(format!(
                "could not link the exact Krit schedule host interfaces: {error}"
            ))
        })?;
        let program =
            bindings::schedule::ScheduleHostProgram::instantiate(&mut *store, component, &linker)
                .map_err(map_wasmtime_error)?;
        let trigger = bindings::schedule::exports::krit::runtime::schedule::Trigger {
            schedule: event.schedule.clone(),
            id: event.id.clone(),
            scheduled_at_millis: event.scheduled_at_millis,
            fired_at_millis: event.fired_at_millis,
            attempt: event.attempt,
            max_attempts: event.max_attempts,
        };
        program
            .krit_runtime_schedule()
            .call_handle(&mut *store, &trigger)
            .map_err(map_wasmtime_error)
    }

    fn idle_result(&self) -> DeliveryExecutionResult {
        DeliveryExecutionResult {
            outcome: DeliveryOutcome::Idle,
            detail: String::new(),
            output: Vec::new(),
            events: Vec::new(),
            stats: self.empty_stats(false, true),
        }
    }

    pub fn active_dns_worker_count(&self) -> usize {
        self.active_dns_workers.load(Ordering::Acquire)
    }
}

enum DeliveryCall {
    Job(JobEvent),
    Schedule(ScheduleEvent),
}

struct CompletedDelivery {
    result: Result<String, String>,
    output: Vec<u8>,
    events: Vec<LogEvent>,
    stats: ExecutionStats,
    state: crate::state::InvocationState,
}

fn disposition_outcome(
    disposition: JobDisposition,
    id: String,
    attempt: u32,
) -> Result<DeliveryOutcome, RuntimeError> {
    match disposition {
        JobDisposition::Retried { visible_at_millis } => Ok(DeliveryOutcome::Retried {
            id,
            attempt,
            visible_at_millis,
        }),
        JobDisposition::DeadLettered => Ok(DeliveryOutcome::DeadLettered { id, attempt }),
        JobDisposition::Lost => Err(RuntimeError::delivery(
            "durable delivery lease expired before its outcome was recorded",
        )),
    }
}

fn bounded_detail(detail: &str) -> String {
    let mut end = detail.len().min(MAX_OUTCOME_DETAIL_BYTES);
    while end > 0 && !detail.is_char_boundary(end) {
        end -= 1;
    }
    detail[..end].to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn outcome_details_stay_within_their_byte_bound() {
        let detail = "é".repeat(MAX_OUTCOME_DETAIL_BYTES);

        let bounded = bounded_detail(&detail);

        assert!(bounded.len() <= MAX_OUTCOME_DETAIL_BYTES);
        assert!(detail.starts_with(&bounded));
        assert_eq!(bounded_detail("ok"), "ok");
    }
}
