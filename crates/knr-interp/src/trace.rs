use serde::{Deserialize, Serialize};

/// A single step execution record for instruction-level tracing.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TraceRecord {
    pub step: u64,
    pub line: usize,
    pub block: usize,
    pub opcode: String,
    pub dst_reg: Option<String>,
    pub new_value: Option<serde_json::Value>,
}

impl TraceRecord {
    /// Constructs a new trace record.
    pub fn new(
        step: u64,
        line: usize,
        block: usize,
        opcode: impl Into<String>,
        dst_reg: Option<String>,
        new_value: Option<serde_json::Value>,
    ) -> Self {
        Self {
            step,
            line,
            block,
            opcode: opcode.into(),
            dst_reg,
            new_value,
        }
    }
}
