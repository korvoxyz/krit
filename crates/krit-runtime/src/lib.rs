mod bindings;
mod error;
mod host;
mod limits;
mod network;
mod permissions;
mod webhook;

use std::{
    collections::BTreeSet,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc,
    },
    thread,
    time::Instant,
};

use krit_wasm::{
    ArtifactMetadata, ComponentInspection, PROGRAM_WORLD, PURE_PROGRAM_WORLD, WEBHOOK_INTERFACE,
    validate_artifact,
};
use serde::Serialize;
use wasmtime::{
    Config, Engine, ProfilingStrategy, Store, StoreLimits, StoreLimitsBuilder, Strategy, Trap,
    component::{Component, HasSelf, Linker, Resource, ResourceTable},
};

use error::HostLimitError;
pub use error::{RuntimeError, RuntimeErrorKind};
pub use host::{HostInputs, NetworkPolicy, SecretStore};
pub use limits::{
    DEFAULT_LIMITS, HARD_MAX_LIMITS, HOST_POLICY_VERSION, HOST_STACK_HEADROOM_BYTES, RuntimeLimits,
};
pub use permissions::{EffectivePermissions, GrantSet, PermissionFact};
pub use webhook::{HttpHeader, HttpRequest, HttpResponse};

#[derive(Debug, Eq, PartialEq)]
pub struct ExecutionResult {
    pub output: Vec<u8>,
    pub stats: ExecutionStats,
}

#[derive(Debug, Eq, PartialEq)]
pub struct WebhookExecutionResult {
    pub response: HttpResponse,
    pub output: Vec<u8>,
    pub stats: ExecutionStats,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecutionStats {
    pub policy_version: u32,
    pub fuel_budget: u64,
    pub fuel_consumed: u64,
    pub fuel_remaining: u64,
    pub host_calls: u64,
    pub http_calls: u64,
    pub output_bytes: usize,
    pub elapsed_micros: u128,
}

pub struct Runtime {
    engine: Engine,
    limits: RuntimeLimits,
    epoch_lock: Mutex<()>,
    active_deadline_workers: Arc<AtomicUsize>,
}

struct HostState {
    output: Vec<u8>,
    host_calls: u64,
    host_call_limit: u64,
    output_limit: usize,
    store_limits: StoreLimits,
    limits: RuntimeLimits,
    grants: Option<GrantSet>,
    requirements: BTreeSet<(String, String)>,
    inputs: HostInputs,
    resources: ResourceTable,
    http_calls: u64,
    invocation_deadline: Instant,
}

pub struct SecretHandle {
    bytes: Arc<host::SecretBytes>,
}

impl Runtime {
    pub fn new(limits: RuntimeLimits) -> Result<Self, RuntimeError> {
        limits.validate()?;
        curl::init();
        let mut config = Config::new();
        config
            .strategy(Strategy::Cranelift)
            .wasm_component_model(true)
            .consume_fuel(true)
            .epoch_interruption(true)
            .max_wasm_stack(limits.wasm_stack_bytes())
            .async_stack_size(limits.wasm_stack_bytes())
            .profiler(ProfilingStrategy::None);
        let engine = Engine::new(&config).map_err(|error| {
            RuntimeError::setup(format!("could not create Wasmtime engine: {error}"))
        })?;
        Ok(Self {
            engine,
            limits,
            epoch_lock: Mutex::new(()),
            active_deadline_workers: Arc::new(AtomicUsize::new(0)),
        })
    }

    pub fn permissions(
        &self,
        bytes: &[u8],
        metadata: &ArtifactMetadata,
        grants: &GrantSet,
    ) -> Result<EffectivePermissions, RuntimeError> {
        self.validate_inputs(bytes, metadata)?;
        let inspection = validate_artifact(bytes, metadata)?;
        self.preflight_resources(&inspection)?;
        Ok(grants.evaluate(metadata))
    }

    pub fn execute(
        &self,
        bytes: &[u8],
        metadata: &ArtifactMetadata,
        grants: &GrantSet,
    ) -> Result<ExecutionResult, RuntimeError> {
        self.validate_inputs(bytes, metadata)?;
        let inspection = validate_artifact(bytes, metadata)?;
        grants.authorize(metadata)?;
        self.preflight_resources(&inspection)?;

        let _epoch_guard = self
            .epoch_lock
            .lock()
            .map_err(|_| RuntimeError::setup("runtime epoch scheduler lock is poisoned"))?;
        let component = Component::new(&self.engine, bytes).map_err(|error| {
            RuntimeError::setup(format!(
                "validated component could not be compiled by Wasmtime 47.x: {error}"
            ))
        })?;
        let worker_stack_bytes = self
            .limits
            .wasm_stack_bytes()
            .checked_add(HOST_STACK_HEADROOM_BYTES)
            .ok_or_else(|| RuntimeError::setup("Wasm execution worker stack size overflowed"))?;

        thread::scope(|scope| {
            let worker = thread::Builder::new()
                .name("krit-wasm-execution".to_owned())
                .stack_size(worker_stack_bytes)
                .spawn_scoped(scope, || self.execute_component(&component, metadata))
                .map_err(|error| {
                    RuntimeError::setup(format!(
                        "could not start isolated Wasm execution worker: {error}"
                    ))
                })?;
            worker
                .join()
                .map_err(|_| RuntimeError::setup("isolated Wasm execution worker panicked"))?
        })
    }

    pub fn invoke_webhook(
        &self,
        bytes: &[u8],
        metadata: &ArtifactMetadata,
        grants: &GrantSet,
        inputs: &HostInputs,
        request: HttpRequest,
    ) -> Result<WebhookExecutionResult, RuntimeError> {
        request.validate(self.limits)?;
        self.validate_host_inputs(grants, inputs)?;
        self.validate_inputs(bytes, metadata)?;
        let inspection = validate_artifact(bytes, metadata)?;
        grants.authorize(metadata)?;
        self.preflight_resources(&inspection)?;
        if inspection.exports != [WEBHOOK_INTERFACE] {
            return Err(RuntimeError::import_mismatch(
                "artifact does not export the typed webhook interface",
            ));
        }

        let _epoch_guard = self
            .epoch_lock
            .lock()
            .map_err(|_| RuntimeError::setup("runtime epoch scheduler lock is poisoned"))?;
        let component = Component::new(&self.engine, bytes).map_err(|error| {
            RuntimeError::setup(format!(
                "validated webhook component could not be compiled by Wasmtime 47.x: {error}"
            ))
        })?;
        let worker_stack_bytes = self
            .limits
            .wasm_stack_bytes()
            .checked_add(HOST_STACK_HEADROOM_BYTES)
            .ok_or_else(|| RuntimeError::setup("Wasm execution worker stack size overflowed"))?;

        thread::scope(|scope| {
            let worker = thread::Builder::new()
                .name("krit-webhook-execution".to_owned())
                .stack_size(worker_stack_bytes)
                .spawn_scoped(scope, || {
                    self.invoke_webhook_component(
                        &component,
                        metadata,
                        grants.clone(),
                        inputs.clone(),
                        request,
                    )
                })
                .map_err(|error| {
                    RuntimeError::setup(format!(
                        "could not start isolated webhook execution worker: {error}"
                    ))
                })?;
            worker
                .join()
                .map_err(|_| RuntimeError::setup("isolated webhook execution worker panicked"))?
        })
    }

    fn invoke_webhook_component(
        &self,
        component: &Component,
        metadata: &ArtifactMetadata,
        grants: GrantSet,
        inputs: HostInputs,
        request: HttpRequest,
    ) -> Result<WebhookExecutionResult, RuntimeError> {
        let started = Instant::now();
        let mut store = Store::new(
            &self.engine,
            self.new_host_state(
                Some(grants),
                metadata
                    .requirements
                    .iter()
                    .map(|requirement| {
                        (requirement.capability.clone(), requirement.resource.clone())
                    })
                    .collect(),
                inputs,
                started,
            ),
        );
        store.limiter(|state| &mut state.store_limits);
        store
            .set_fuel(self.limits.fuel())
            .map_err(|error| RuntimeError::setup(format!("could not set Wasm fuel: {error}")))?;
        store.set_epoch_deadline(1);
        store.epoch_deadline_trap();
        let deadline = DeadlineWorker::start(
            self.engine.clone(),
            self.limits.deadline(),
            Arc::clone(&self.active_deadline_workers),
        )?;
        let call = self.call_webhook(component, &mut store, request);
        let timed_out = deadline.finish()?;
        if timed_out {
            return Err(RuntimeError::deadline("Wasm wall deadline exceeded"));
        }
        let response = call?;
        response.validate(self.limits)?;
        let remaining = store.get_fuel().map_err(|error| {
            RuntimeError::setup(format!("could not read remaining fuel: {error}"))
        })?;
        let state = store.into_data();
        Ok(WebhookExecutionResult {
            stats: ExecutionStats {
                policy_version: HOST_POLICY_VERSION,
                fuel_budget: self.limits.fuel(),
                fuel_consumed: self.limits.fuel().saturating_sub(remaining),
                fuel_remaining: remaining,
                host_calls: state.host_calls,
                http_calls: state.http_calls,
                output_bytes: state.output.len(),
                elapsed_micros: started.elapsed().as_micros(),
            },
            output: state.output,
            response,
        })
    }

    fn execute_component(
        &self,
        component: &Component,
        metadata: &ArtifactMetadata,
    ) -> Result<ExecutionResult, RuntimeError> {
        let started = Instant::now();
        let mut store = Store::new(
            &self.engine,
            self.new_host_state(None, BTreeSet::new(), HostInputs::default(), started),
        );
        store.limiter(|state| &mut state.store_limits);
        store
            .set_fuel(self.limits.fuel())
            .map_err(|error| RuntimeError::setup(format!("could not set Wasm fuel: {error}")))?;
        store.set_epoch_deadline(1);
        store.epoch_deadline_trap();

        let deadline = DeadlineWorker::start(
            self.engine.clone(),
            self.limits.deadline(),
            Arc::clone(&self.active_deadline_workers),
        )?;
        let call = match metadata.world.as_str() {
            PURE_PROGRAM_WORLD => self.call_pure(component, &mut store),
            PROGRAM_WORLD => self.call_stdout(component, &mut store),
            world => Err(RuntimeError::import_mismatch(format!(
                "validated artifact selected unsupported world `{world}`"
            ))),
        };
        let timed_out = deadline.finish()?;
        if timed_out {
            return Err(RuntimeError::deadline("Wasm wall deadline exceeded"));
        }
        call?;

        let remaining = store.get_fuel().map_err(|error| {
            RuntimeError::setup(format!("could not read remaining fuel: {error}"))
        })?;
        let state = store.into_data();
        Ok(ExecutionResult {
            stats: ExecutionStats {
                policy_version: HOST_POLICY_VERSION,
                fuel_budget: self.limits.fuel(),
                fuel_consumed: self.limits.fuel().saturating_sub(remaining),
                fuel_remaining: remaining,
                host_calls: state.host_calls,
                http_calls: state.http_calls,
                output_bytes: state.output.len(),
                elapsed_micros: started.elapsed().as_micros(),
            },
            output: state.output,
        })
    }

    pub fn limits(&self) -> RuntimeLimits {
        self.limits
    }

    pub fn active_deadline_workers(&self) -> usize {
        self.active_deadline_workers.load(Ordering::Acquire)
    }

    fn validate_inputs(
        &self,
        bytes: &[u8],
        metadata: &ArtifactMetadata,
    ) -> Result<(), RuntimeError> {
        if bytes.len() > self.limits.component_bytes() {
            return Err(RuntimeError::setup(format!(
                "component size {} exceeds the pre-compilation limit {}",
                bytes.len(),
                self.limits.component_bytes()
            )));
        }
        let metadata_size = serde_json::to_vec(metadata)
            .map_err(|error| {
                RuntimeError::setup(format!("could not size artifact metadata: {error}"))
            })?
            .len();
        if metadata_size > self.limits.metadata_bytes() {
            return Err(RuntimeError::setup(format!(
                "artifact metadata size {metadata_size} exceeds the pre-compilation limit {}",
                self.limits.metadata_bytes()
            )));
        }
        Ok(())
    }

    fn preflight_resources(&self, inspection: &ComponentInspection) -> Result<(), RuntimeError> {
        if usize::try_from(inspection.core_module_count)
            .map_or(true, |count| count > self.limits.instances())
        {
            return Err(RuntimeError::resource(
                "component instance count exceeds the host limit",
            ));
        }
        if usize::try_from(inspection.table_count)
            .map_or(true, |count| count > self.limits.tables())
        {
            return Err(RuntimeError::resource(
                "component table count exceeds the host limit",
            ));
        }
        if usize::try_from(inspection.table_elements)
            .map_or(true, |count| count > self.limits.table_elements())
        {
            return Err(RuntimeError::resource(
                "component table elements exceed the host limit",
            ));
        }
        if usize::try_from(inspection.memory_count)
            .map_or(true, |count| count > self.limits.memories())
        {
            return Err(RuntimeError::resource(
                "component memory count exceeds the host limit",
            ));
        }
        if usize::try_from(inspection.memory_minimum_bytes)
            .map_or(true, |bytes| bytes > self.limits.memory_bytes())
        {
            return Err(RuntimeError::resource(
                "component minimum linear memory exceeds the host byte limit",
            ));
        }
        Ok(())
    }

    fn validate_host_inputs(
        &self,
        grants: &GrantSet,
        inputs: &HostInputs,
    ) -> Result<(), RuntimeError> {
        let mut config_bytes = 0usize;
        for (name, value) in inputs.config() {
            if !grants.grants("config.read", Some(name)) {
                return Err(RuntimeError::authorization(format!(
                    "host configuration key `{name}` is not granted by the manifest"
                )));
            }
            config_bytes = config_bytes
                .checked_add(name.len())
                .and_then(|bytes| bytes.checked_add(value.len()))
                .ok_or_else(|| RuntimeError::resource("host configuration size overflowed"))?;
            if config_bytes > self.limits.host_config_bytes() {
                return Err(RuntimeError::resource(format!(
                    "host configuration exceeds the {}-byte limit",
                    self.limits.host_config_bytes()
                )));
            }
        }
        for (name, size) in inputs.secrets().iter() {
            if !grants.grants("secret.read", Some(name)) {
                return Err(RuntimeError::authorization(format!(
                    "host secret `{name}` is not granted by the manifest"
                )));
            }
            if size > self.limits.secret_bytes() {
                return Err(RuntimeError::resource(format!(
                    "host secret `{name}` exceeds the {}-byte limit",
                    self.limits.secret_bytes()
                )));
            }
        }
        Ok(())
    }

    fn new_host_state(
        &self,
        grants: Option<GrantSet>,
        requirements: BTreeSet<(String, String)>,
        inputs: HostInputs,
        started: Instant,
    ) -> HostState {
        let store_limits = StoreLimitsBuilder::new()
            .memory_size(self.limits.memory_bytes())
            .table_elements(self.limits.table_elements())
            .instances(self.limits.instances())
            .tables(self.limits.tables())
            .memories(self.limits.memories())
            .trap_on_grow_failure(true)
            .build();
        HostState {
            output: Vec::new(),
            host_calls: 0,
            host_call_limit: self.limits.host_calls(),
            output_limit: self.limits.output_bytes(),
            store_limits,
            limits: self.limits,
            grants,
            requirements,
            inputs,
            resources: ResourceTable::new(),
            http_calls: 0,
            invocation_deadline: started + self.limits.deadline(),
        }
    }

    fn call_pure(
        &self,
        component: &Component,
        store: &mut Store<HostState>,
    ) -> Result<(), RuntimeError> {
        let linker = Linker::new(&self.engine);
        let program = bindings::pure::PureProgram::instantiate(&mut *store, component, &linker)
            .map_err(map_wasmtime_error)?;
        program.call_run(&mut *store).map_err(map_wasmtime_error)
    }

    fn call_stdout(
        &self,
        component: &Component,
        store: &mut Store<HostState>,
    ) -> Result<(), RuntimeError> {
        let mut linker = Linker::new(&self.engine);
        bindings::stdout::Program::add_to_linker::<_, HasSelf<_>>(&mut linker, |state| state)
            .map_err(|error| {
                RuntimeError::import_mismatch(format!(
                    "could not link the exact Krit stdout interface: {error}"
                ))
            })?;
        let program = bindings::stdout::Program::instantiate(&mut *store, component, &linker)
            .map_err(map_wasmtime_error)?;
        program.call_run(&mut *store).map_err(map_wasmtime_error)
    }

    fn call_webhook(
        &self,
        component: &Component,
        store: &mut Store<HostState>,
        request: HttpRequest,
    ) -> Result<HttpResponse, RuntimeError> {
        let mut linker = Linker::new(&self.engine);
        bindings::webhook::WebhookHostProgram::add_to_linker::<_, HasSelf<_>>(
            &mut linker,
            |state| state,
        )
        .map_err(|error| {
            RuntimeError::import_mismatch(format!(
                "could not link the exact Krit webhook host interfaces: {error}"
            ))
        })?;
        let program =
            bindings::webhook::WebhookHostProgram::instantiate(&mut *store, component, &linker)
                .map_err(map_wasmtime_error)?;
        let request = bindings::webhook::exports::krit::runtime::webhook::Request {
            method: request.method,
            path: request.path,
            query: request.query,
            headers: request
                .headers
                .into_iter()
                .map(
                    |header| bindings::webhook::exports::krit::runtime::webhook::Header {
                        name: header.name,
                        value: header.value,
                    },
                )
                .collect(),
            body: request.body,
        };
        let response = program
            .krit_runtime_webhook()
            .call_handle(&mut *store, &request)
            .map_err(map_wasmtime_error)?;
        Ok(HttpResponse {
            status: response.status,
            headers: response
                .headers
                .into_iter()
                .map(|header| HttpHeader {
                    name: header.name,
                    value: header.value,
                })
                .collect(),
            body: response.body,
        })
    }
}

impl Default for Runtime {
    fn default() -> Self {
        Self::new(RuntimeLimits::default()).expect("default runtime limits are valid")
    }
}

impl HostState {
    fn write(&mut self, rendered: &str, newline: bool) -> wasmtime::Result<()> {
        let next_calls = self
            .host_calls
            .checked_add(1)
            .ok_or_else(|| wasmtime::Error::new(HostLimitError::Calls))?;
        if next_calls > self.host_call_limit {
            return Err(wasmtime::Error::new(HostLimitError::Calls));
        }
        let additional = rendered
            .len()
            .checked_add(usize::from(newline))
            .ok_or_else(|| wasmtime::Error::new(HostLimitError::Output))?;
        let next_bytes = self
            .output
            .len()
            .checked_add(additional)
            .ok_or_else(|| wasmtime::Error::new(HostLimitError::Output))?;
        if next_bytes > self.output_limit {
            return Err(wasmtime::Error::new(HostLimitError::Output));
        }
        self.output
            .try_reserve(additional)
            .map_err(|_| wasmtime::Error::new(HostLimitError::Output))?;
        self.host_calls = next_calls;
        self.output.extend_from_slice(rendered.as_bytes());
        if newline {
            self.output.push(b'\n');
        }
        Ok(())
    }
}

impl bindings::stdout::krit::runtime::stdout::Host for HostState {
    fn write_int(&mut self, value: i64, newline: bool) -> wasmtime::Result<()> {
        self.write(&value.to_string(), newline)
    }

    fn write_bool(&mut self, value: bool, newline: bool) -> wasmtime::Result<()> {
        self.write(if value { "true" } else { "false" }, newline)
    }

    fn write_unit(&mut self, newline: bool) -> wasmtime::Result<()> {
        self.write("()", newline)
    }
}

impl bindings::webhook::krit::runtime::stdout::Host for HostState {
    fn write_int(&mut self, value: i64, newline: bool) -> wasmtime::Result<()> {
        self.write(&value.to_string(), newline)
    }

    fn write_bool(&mut self, value: bool, newline: bool) -> wasmtime::Result<()> {
        self.write(if value { "true" } else { "false" }, newline)
    }

    fn write_unit(&mut self, newline: bool) -> wasmtime::Result<()> {
        self.write("()", newline)
    }
}

impl bindings::webhook::krit::runtime::config::Host for HostState {
    fn get_string(&mut self, key: String) -> wasmtime::Result<Result<String, String>> {
        self.record_host_call()?;
        self.require_grant("config.read", &key)?;
        Ok(self
            .inputs
            .config()
            .get(&key)
            .cloned()
            .ok_or_else(|| format!("configuration key `{key}` is not configured")))
    }
}

impl bindings::webhook::krit::runtime::secrets::HostSecret for HostState {
    fn drop(&mut self, secret: Resource<SecretHandle>) -> wasmtime::Result<()> {
        self.resources
            .delete(secret)
            .map(|_| ())
            .map_err(|error| wasmtime::Error::msg(format!("secret handle drop failed: {error}")))
    }
}

impl bindings::webhook::krit::runtime::secrets::Host for HostState {
    fn acquire(
        &mut self,
        name: String,
    ) -> wasmtime::Result<Result<Resource<SecretHandle>, String>> {
        self.record_host_call()?;
        self.require_grant("secret.read", &name)?;
        let Some(bytes) = self.inputs.secrets().get(&name) else {
            return Ok(Err(format!("secret `{name}` is not configured")));
        };
        let handle = self
            .resources
            .push(SecretHandle { bytes })
            .map_err(|error| {
                wasmtime::Error::new(RuntimeError::resource(format!(
                    "secret handle table is full: {error}"
                )))
            })?;
        Ok(Ok(handle))
    }
}

impl bindings::webhook::krit::runtime::http::Host for HostState {
    fn send(
        &mut self,
        origin: String,
        request: bindings::webhook::krit::runtime::http::Request,
        bearer: Option<Resource<SecretHandle>>,
    ) -> wasmtime::Result<Result<bindings::webhook::krit::runtime::http::Response, String>> {
        let request = HttpRequest {
            method: request.method,
            path: request.path,
            query: request.query,
            headers: request
                .headers
                .into_iter()
                .map(|header| HttpHeader {
                    name: header.name,
                    value: header.value,
                })
                .collect(),
            body: request.body,
        };
        let response = match self.perform_http(origin, request, bearer)? {
            Ok(response) => response,
            Err(error) => return Ok(Err(error)),
        };
        Ok(Ok(bindings::webhook::krit::runtime::http::Response {
            status: response.status,
            headers: response
                .headers
                .into_iter()
                .map(|header| bindings::webhook::krit::runtime::http::Header {
                    name: header.name,
                    value: header.value,
                })
                .collect(),
            body: response.body,
        }))
    }
}

impl bindings::webhook::krit::runtime::http_anonymous::Host for HostState {
    fn send(
        &mut self,
        origin: String,
        request: bindings::webhook::krit::runtime::http_anonymous::Request,
    ) -> wasmtime::Result<Result<bindings::webhook::krit::runtime::http_anonymous::Response, String>>
    {
        let request = HttpRequest {
            method: request.method,
            path: request.path,
            query: request.query,
            headers: request
                .headers
                .into_iter()
                .map(|header| HttpHeader {
                    name: header.name,
                    value: header.value,
                })
                .collect(),
            body: request.body,
        };
        let response = match self.perform_http(origin, request, None)? {
            Ok(response) => response,
            Err(error) => return Ok(Err(error)),
        };
        Ok(Ok(
            bindings::webhook::krit::runtime::http_anonymous::Response {
                status: response.status,
                headers: response
                    .headers
                    .into_iter()
                    .map(
                        |header| bindings::webhook::krit::runtime::http_anonymous::Header {
                            name: header.name,
                            value: header.value,
                        },
                    )
                    .collect(),
                body: response.body,
            },
        ))
    }
}

impl HostState {
    fn record_host_call(&mut self) -> wasmtime::Result<()> {
        let next = self
            .host_calls
            .checked_add(1)
            .ok_or_else(|| wasmtime::Error::new(HostLimitError::Calls))?;
        if next > self.host_call_limit {
            return Err(wasmtime::Error::new(HostLimitError::Calls));
        }
        self.host_calls = next;
        Ok(())
    }

    fn require_grant(&self, capability: &str, resource: &str) -> wasmtime::Result<()> {
        if self
            .grants
            .as_ref()
            .is_some_and(|grants| grants.grants(capability, Some(resource)))
            && self
                .requirements
                .contains(&(capability.to_owned(), resource.to_owned()))
        {
            Ok(())
        } else {
            Err(wasmtime::Error::new(RuntimeError::authorization(format!(
                "guest requested ungranted capability `{capability}` for resource `{resource}`"
            ))))
        }
    }

    fn perform_http(
        &mut self,
        origin: String,
        request: HttpRequest,
        bearer: Option<Resource<SecretHandle>>,
    ) -> wasmtime::Result<Result<HttpResponse, String>> {
        self.record_host_call()?;
        self.require_grant("http.request", &origin)?;
        let parsed = krit_capability::HttpOrigin::parse_exact(&origin).map_err(|error| {
            wasmtime::Error::new(RuntimeError::authorization(format!(
                "guest requested invalid HTTP origin `{origin}`: {error}"
            )))
        })?;
        let next_http_calls = self
            .http_calls
            .checked_add(1)
            .ok_or_else(|| wasmtime::Error::new(HostLimitError::Calls))?;
        if next_http_calls > self.limits.http_calls() {
            return Err(wasmtime::Error::new(RuntimeError::host_calls(
                "outbound HTTP call limit exceeded",
            )));
        }
        self.http_calls = next_http_calls;
        request
            .validate(self.limits)
            .map_err(wasmtime::Error::new)?;
        let bearer = bearer
            .as_ref()
            .map(|handle| {
                self.resources
                    .get(handle)
                    .map(|secret| Arc::clone(&secret.bytes))
            })
            .transpose()
            .map_err(|error| wasmtime::Error::msg(format!("invalid secret handle: {error}")))?;
        let remaining = self
            .invocation_deadline
            .checked_duration_since(Instant::now())
            .unwrap_or_default();
        Ok(network::send(
            &parsed,
            &request,
            bearer.as_deref(),
            self.inputs.network_policy(),
            self.limits,
            remaining,
        ))
    }
}

fn map_wasmtime_error(error: wasmtime::Error) -> RuntimeError {
    if let Some(runtime) = error.downcast_ref::<RuntimeError>() {
        return runtime.clone();
    }
    if let Some(host) = error.downcast_ref::<HostLimitError>() {
        return match host {
            HostLimitError::Calls => RuntimeError::host_calls("host-call limit exceeded"),
            HostLimitError::Output => RuntimeError::output("buffered output-byte limit exceeded"),
        };
    }
    if let Some(trap) = error.downcast_ref::<Trap>() {
        return match trap {
            Trap::OutOfFuel => RuntimeError::fuel("Wasm fuel budget exhausted"),
            Trap::Interrupt => RuntimeError::deadline("Wasm wall deadline exceeded"),
            Trap::StackOverflow
            | Trap::MemoryOutOfBounds
            | Trap::TableOutOfBounds
            | Trap::AllocationTooLarge => {
                RuntimeError::resource(format!("Wasm resource limit exceeded: {trap}"))
            }
            Trap::IntegerDivisionByZero => {
                RuntimeError::guest("K4004", "division or remainder by zero")
            }
            Trap::IntegerOverflow => RuntimeError::guest("K4005", "integer overflow"),
            Trap::UnreachableCodeReached => {
                RuntimeError::guest("K4001", "guest executed an unreachable instruction")
            }
            Trap::IndirectCallToNull | Trap::BadSignature => {
                RuntimeError::guest("K4001", "invalid indirect function call")
            }
            _ => RuntimeError::guest("K4001", format!("guest trapped: {trap}")),
        };
    }
    let message = error.to_string();
    if message.starts_with("resource limit exceeded:")
        || message.starts_with("forcing trap when growing ")
    {
        RuntimeError::resource(message)
    } else {
        RuntimeError::guest("K4001", format!("component execution failed: {message}"))
    }
}

struct DeadlineWorker {
    cancel: Option<mpsc::Sender<()>>,
    handle: Option<thread::JoinHandle<()>>,
    expired: Arc<AtomicBool>,
}

impl DeadlineWorker {
    fn start(
        engine: Engine,
        timeout: std::time::Duration,
        active: Arc<AtomicUsize>,
    ) -> Result<Self, RuntimeError> {
        let (cancel, receiver) = mpsc::channel();
        let expired = Arc::new(AtomicBool::new(false));
        let worker_expired = Arc::clone(&expired);
        let handle = thread::Builder::new()
            .name("krit-wasm-deadline".to_owned())
            .spawn(move || {
                active.fetch_add(1, Ordering::AcqRel);
                if matches!(
                    receiver.recv_timeout(timeout),
                    Err(mpsc::RecvTimeoutError::Timeout)
                ) {
                    worker_expired.store(true, Ordering::Release);
                    engine.increment_epoch();
                }
                active.fetch_sub(1, Ordering::AcqRel);
            })
            .map_err(|error| {
                RuntimeError::deadline(format!("could not start deadline worker: {error}"))
            })?;
        Ok(Self {
            cancel: Some(cancel),
            handle: Some(handle),
            expired,
        })
    }

    fn expired(&self) -> bool {
        self.expired.load(Ordering::Acquire)
    }

    fn finish(mut self) -> Result<bool, RuntimeError> {
        self.stop()?;
        Ok(self.expired())
    }

    fn stop(&mut self) -> Result<(), RuntimeError> {
        if let Some(cancel) = self.cancel.take() {
            let _ = cancel.send(());
        }
        if let Some(handle) = self.handle.take() {
            handle
                .join()
                .map_err(|_| RuntimeError::deadline("deadline worker panicked"))?;
        }
        Ok(())
    }
}

impl Drop for DeadlineWorker {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}
