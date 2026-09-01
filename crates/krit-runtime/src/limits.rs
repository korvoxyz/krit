use std::time::Duration;

use crate::{RuntimeError, RuntimeErrorKind};

pub const HOST_POLICY_VERSION: u32 = 1;
pub const HOST_STACK_HEADROOM_BYTES: usize = 2 * 1024 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RuntimeLimits {
    component_bytes: usize,
    metadata_bytes: usize,
    memory_bytes: usize,
    table_elements: usize,
    instances: usize,
    tables: usize,
    memories: usize,
    wasm_stack_bytes: usize,
    fuel: u64,
    deadline: Duration,
    host_calls: u64,
    output_bytes: usize,
    request_body_bytes: usize,
    response_body_bytes: usize,
    header_count: usize,
    header_bytes: usize,
    http_calls: u64,
    connect_timeout: Duration,
    read_timeout: Duration,
    http_timeout: Duration,
    host_config_bytes: usize,
    secret_bytes: usize,
}

pub const HARD_MAX_LIMITS: RuntimeLimits = RuntimeLimits {
    component_bytes: 16 * 1024 * 1024,
    metadata_bytes: 1024 * 1024,
    memory_bytes: 64 * 1024 * 1024,
    table_elements: 65_536,
    instances: 64,
    tables: 32,
    memories: 8,
    wasm_stack_bytes: 8 * 1024 * 1024,
    fuel: 1_000_000_000,
    deadline: Duration::from_secs(30),
    host_calls: 1_000_000,
    output_bytes: 16 * 1024 * 1024,
    request_body_bytes: 16 * 1024 * 1024,
    response_body_bytes: 16 * 1024 * 1024,
    header_count: 1024,
    header_bytes: 1024 * 1024,
    http_calls: 1024,
    connect_timeout: Duration::from_secs(5),
    read_timeout: Duration::from_secs(10),
    http_timeout: Duration::from_secs(20),
    host_config_bytes: 1024 * 1024,
    secret_bytes: 1024 * 1024,
};

pub const DEFAULT_LIMITS: RuntimeLimits = RuntimeLimits {
    component_bytes: 4 * 1024 * 1024,
    metadata_bytes: 64 * 1024,
    memory_bytes: 16 * 1024 * 1024,
    table_elements: 4096,
    instances: 16,
    tables: 8,
    memories: 1,
    wasm_stack_bytes: 512 * 1024,
    fuel: 10_000_000,
    deadline: Duration::from_secs(1),
    host_calls: 1024,
    output_bytes: 1024 * 1024,
    request_body_bytes: 1024 * 1024,
    response_body_bytes: 1024 * 1024,
    header_count: 128,
    header_bytes: 64 * 1024,
    http_calls: 16,
    connect_timeout: Duration::from_millis(250),
    read_timeout: Duration::from_millis(500),
    http_timeout: Duration::from_millis(750),
    host_config_bytes: 64 * 1024,
    secret_bytes: 64 * 1024,
};

impl Default for RuntimeLimits {
    fn default() -> Self {
        DEFAULT_LIMITS
    }
}

impl RuntimeLimits {
    pub const fn component_bytes(self) -> usize {
        self.component_bytes
    }

    pub const fn metadata_bytes(self) -> usize {
        self.metadata_bytes
    }

    pub const fn memory_bytes(self) -> usize {
        self.memory_bytes
    }

    pub const fn table_elements(self) -> usize {
        self.table_elements
    }

    pub const fn instances(self) -> usize {
        self.instances
    }

    pub const fn tables(self) -> usize {
        self.tables
    }

    pub const fn memories(self) -> usize {
        self.memories
    }

    pub const fn wasm_stack_bytes(self) -> usize {
        self.wasm_stack_bytes
    }

    pub const fn fuel(self) -> u64 {
        self.fuel
    }

    pub const fn deadline(self) -> Duration {
        self.deadline
    }

    pub const fn host_calls(self) -> u64 {
        self.host_calls
    }

    pub const fn output_bytes(self) -> usize {
        self.output_bytes
    }

    pub const fn request_body_bytes(self) -> usize {
        self.request_body_bytes
    }

    pub const fn response_body_bytes(self) -> usize {
        self.response_body_bytes
    }

    pub const fn header_count(self) -> usize {
        self.header_count
    }

    pub const fn header_bytes(self) -> usize {
        self.header_bytes
    }

    pub const fn http_calls(self) -> u64 {
        self.http_calls
    }

    pub const fn connect_timeout(self) -> Duration {
        self.connect_timeout
    }

    pub const fn read_timeout(self) -> Duration {
        self.read_timeout
    }

    pub const fn http_timeout(self) -> Duration {
        self.http_timeout
    }

    pub const fn host_config_bytes(self) -> usize {
        self.host_config_bytes
    }

    pub const fn secret_bytes(self) -> usize {
        self.secret_bytes
    }

    pub fn narrow_component_bytes(&mut self, value: usize) -> Result<(), RuntimeError> {
        narrow(&mut self.component_bytes, value, "component byte")
    }

    pub fn narrow_metadata_bytes(&mut self, value: usize) -> Result<(), RuntimeError> {
        narrow(&mut self.metadata_bytes, value, "metadata byte")
    }

    pub fn narrow_memory_bytes(&mut self, value: usize) -> Result<(), RuntimeError> {
        narrow(&mut self.memory_bytes, value, "memory byte")
    }

    pub fn narrow_table_elements(&mut self, value: usize) -> Result<(), RuntimeError> {
        narrow(&mut self.table_elements, value, "table element")
    }

    pub fn narrow_instances(&mut self, value: usize) -> Result<(), RuntimeError> {
        narrow(&mut self.instances, value, "instance")
    }

    pub fn narrow_tables(&mut self, value: usize) -> Result<(), RuntimeError> {
        narrow(&mut self.tables, value, "table")
    }

    pub fn narrow_memories(&mut self, value: usize) -> Result<(), RuntimeError> {
        narrow(&mut self.memories, value, "memory count")
    }

    pub fn narrow_wasm_stack_bytes(&mut self, value: usize) -> Result<(), RuntimeError> {
        narrow_nonzero(&mut self.wasm_stack_bytes, value, "Wasm stack byte")
    }

    pub fn narrow_fuel(&mut self, value: u64) -> Result<(), RuntimeError> {
        narrow_nonzero(&mut self.fuel, value, "fuel")
    }

    pub fn narrow_deadline(&mut self, value: Duration) -> Result<(), RuntimeError> {
        if value.is_zero() || value > self.deadline {
            return Err(limit_error(
                "deadline",
                value.as_millis(),
                self.deadline.as_millis(),
            ));
        }
        self.deadline = value;
        Ok(())
    }

    pub fn narrow_host_calls(&mut self, value: u64) -> Result<(), RuntimeError> {
        narrow(&mut self.host_calls, value, "host-call")
    }

    pub fn narrow_output_bytes(&mut self, value: usize) -> Result<(), RuntimeError> {
        narrow(&mut self.output_bytes, value, "output byte")
    }

    pub fn narrow_request_body_bytes(&mut self, value: usize) -> Result<(), RuntimeError> {
        narrow(&mut self.request_body_bytes, value, "request body byte")
    }

    pub fn narrow_response_body_bytes(&mut self, value: usize) -> Result<(), RuntimeError> {
        narrow(&mut self.response_body_bytes, value, "response body byte")
    }

    pub fn narrow_header_count(&mut self, value: usize) -> Result<(), RuntimeError> {
        narrow(&mut self.header_count, value, "header count")
    }

    pub fn narrow_header_bytes(&mut self, value: usize) -> Result<(), RuntimeError> {
        narrow(&mut self.header_bytes, value, "header byte")
    }

    pub fn narrow_http_calls(&mut self, value: u64) -> Result<(), RuntimeError> {
        narrow(&mut self.http_calls, value, "HTTP call")
    }

    pub fn narrow_connect_timeout(&mut self, value: Duration) -> Result<(), RuntimeError> {
        narrow_duration(&mut self.connect_timeout, value, "connect timeout")
    }

    pub fn narrow_read_timeout(&mut self, value: Duration) -> Result<(), RuntimeError> {
        narrow_duration(&mut self.read_timeout, value, "read timeout")
    }

    pub fn narrow_http_timeout(&mut self, value: Duration) -> Result<(), RuntimeError> {
        narrow_duration(&mut self.http_timeout, value, "HTTP timeout")
    }

    pub fn narrow_host_config_bytes(&mut self, value: usize) -> Result<(), RuntimeError> {
        narrow(&mut self.host_config_bytes, value, "host config byte")
    }

    pub fn narrow_secret_bytes(&mut self, value: usize) -> Result<(), RuntimeError> {
        narrow(&mut self.secret_bytes, value, "secret byte")
    }

    pub(crate) fn validate(self) -> Result<(), RuntimeError> {
        if self.component_bytes > HARD_MAX_LIMITS.component_bytes
            || self.metadata_bytes > HARD_MAX_LIMITS.metadata_bytes
            || self.memory_bytes > HARD_MAX_LIMITS.memory_bytes
            || self.table_elements > HARD_MAX_LIMITS.table_elements
            || self.instances > HARD_MAX_LIMITS.instances
            || self.tables > HARD_MAX_LIMITS.tables
            || self.memories > HARD_MAX_LIMITS.memories
            || self.wasm_stack_bytes == 0
            || self.wasm_stack_bytes > HARD_MAX_LIMITS.wasm_stack_bytes
            || self.fuel == 0
            || self.fuel > HARD_MAX_LIMITS.fuel
            || self.deadline.is_zero()
            || self.deadline > HARD_MAX_LIMITS.deadline
            || self.host_calls > HARD_MAX_LIMITS.host_calls
            || self.output_bytes > HARD_MAX_LIMITS.output_bytes
            || self.request_body_bytes > HARD_MAX_LIMITS.request_body_bytes
            || self.response_body_bytes > HARD_MAX_LIMITS.response_body_bytes
            || self.header_count > HARD_MAX_LIMITS.header_count
            || self.header_bytes > HARD_MAX_LIMITS.header_bytes
            || self.http_calls > HARD_MAX_LIMITS.http_calls
            || self.connect_timeout.is_zero()
            || self.connect_timeout > HARD_MAX_LIMITS.connect_timeout
            || self.read_timeout.is_zero()
            || self.read_timeout > HARD_MAX_LIMITS.read_timeout
            || self.http_timeout.is_zero()
            || self.http_timeout > HARD_MAX_LIMITS.http_timeout
            || self.host_config_bytes > HARD_MAX_LIMITS.host_config_bytes
            || self.secret_bytes > HARD_MAX_LIMITS.secret_bytes
        {
            return Err(RuntimeError::new(
                "K5103",
                RuntimeErrorKind::ResourceLimit,
                "runtime limits exceed host policy 1 maxima",
            ));
        }

        Ok(())
    }
}

fn narrow_duration(
    current: &mut Duration,
    value: Duration,
    name: &str,
) -> Result<(), RuntimeError> {
    if value.is_zero() || value > *current {
        return Err(limit_error(name, value.as_millis(), current.as_millis()));
    }
    *current = value;
    Ok(())
}

fn narrow<T>(current: &mut T, value: T, name: &str) -> Result<(), RuntimeError>
where
    T: Copy + Ord + std::fmt::Display,
{
    if value > *current {
        return Err(limit_error(name, value, *current));
    }
    *current = value;
    Ok(())
}

fn narrow_nonzero<T>(current: &mut T, value: T, name: &str) -> Result<(), RuntimeError>
where
    T: Copy + Ord + Default + std::fmt::Display,
{
    if value == T::default() || value > *current {
        return Err(limit_error(name, value, *current));
    }
    *current = value;
    Ok(())
}

fn limit_error(
    name: &str,
    requested: impl std::fmt::Display,
    current: impl std::fmt::Display,
) -> RuntimeError {
    RuntimeError::resource(format!(
        "{name} limit {requested} cannot raise the host limit {current}"
    ))
}
