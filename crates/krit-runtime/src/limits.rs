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
