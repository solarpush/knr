use crate::platform::{HostPlatform, RuntimeError, Value};
use knr_core::Effect;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// A record of a host platform call made during execution.
#[derive(Debug, Clone, PartialEq)]
pub struct RecordedCall {
    pub effect: Effect,
    pub op: String,
    pub payload: Value,
}

/// In-memory mock platform for deterministic testing and recording host interactions.
#[derive(Debug, Clone)]
pub struct MockPlatform {
    calls: Arc<Mutex<Vec<RecordedCall>>>,
    responses: Arc<Mutex<HashMap<(Effect, String), Value>>>,
    default_response: Value,
}

impl Default for MockPlatform {
    fn default() -> Self {
        Self::new()
    }
}

impl MockPlatform {
    /// Creates a new empty `MockPlatform`.
    pub fn new() -> Self {
        Self {
            calls: Arc::new(Mutex::new(Vec::new())),
            responses: Arc::new(Mutex::new(HashMap::new())),
            default_response: Value::Void,
        }
    }

    /// Returns a copy of all recorded calls.
    pub fn calls(&self) -> Vec<RecordedCall> {
        self.calls.lock().unwrap().clone()
    }

    /// Returns the total number of calls recorded.
    pub fn call_count(&self) -> usize {
        self.calls.lock().unwrap().len()
    }

    /// Returns the most recently recorded call, if any.
    pub fn last_call(&self) -> Option<RecordedCall> {
        self.calls.lock().unwrap().last().cloned()
    }

    /// Clears the recorded calls.
    pub fn clear(&self) {
        self.calls.lock().unwrap().clear();
    }

    /// Sets a canned response for a specific effect and operation name.
    pub fn set_response(&mut self, effect: Effect, op: impl Into<String>, response: Value) {
        self.responses
            .lock()
            .unwrap()
            .insert((effect, op.into()), response);
    }

    /// Sets the default response returned when no specific response is configured.
    pub fn set_default_response(&mut self, response: Value) {
        self.default_response = response;
    }

    /// Checks if a call with the given effect and operation name has been recorded.
    pub fn has_called(&self, effect: Effect, op: &str) -> bool {
        self.calls
            .lock()
            .unwrap()
            .iter()
            .any(|c| c.effect == effect && c.op == op)
    }
}

impl HostPlatform for MockPlatform {
    fn host_call(&mut self, effect: Effect, op: &str, payload: Value) -> Result<Value, RuntimeError> {
        self.calls.lock().unwrap().push(RecordedCall {
            effect,
            op: op.to_string(),
            payload,
        });

        let responses = self.responses.lock().unwrap();
        if let Some(resp) = responses.get(&(effect, op.to_string())) {
            Ok(resp.clone())
        } else {
            Ok(self.default_response.clone())
        }
    }
}
