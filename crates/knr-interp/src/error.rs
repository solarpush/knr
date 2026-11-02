use knr_runtime::RuntimeError;
use thiserror::Error;

/// Errors that can occur during SSA reference interpretation.
#[derive(Debug, Error, Clone, PartialEq)]
pub enum VmError {
    #[error("Gas exhausted: exceeded {max_steps} steps")]
    GasExhausted { max_steps: u64 },

    #[error("Division by zero at line {line}")]
    DivisionByZero { line: usize },

    #[error("Invalid register access: %{reg}")]
    InvalidRegisterAccess { reg: usize },

    #[error("Type mismatch at line {line}: expected {expected}, found {actual}")]
    TypeMismatch {
        expected: String,
        actual: String,
        line: usize,
    },

    #[error("No such block: bb{block_id}")]
    NoSuchBlock { block_id: usize },

    #[error("No such function: {name}")]
    NoSuchFunction { name: String },

    #[error("Host call error: {0}")]
    HostCallError(#[from] RuntimeError),

    #[error("Stack overflow")]
    StackOverflow,

    #[error("Index out of bounds at line {line}: index {index} >= len {len}")]
    IndexOutOfBounds {
        index: usize,
        len: usize,
        line: usize,
    },

    #[error("Invalid instruction at line {line}: {message}")]
    InvalidInstruction {
        message: String,
        line: usize,
    },

    #[error("Runtime error: {0}")]
    Runtime(String),

    #[error("VM error: {0}")]
    Other(String),
}
