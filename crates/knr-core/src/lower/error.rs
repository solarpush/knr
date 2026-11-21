use thiserror::Error;

#[derive(Error, Debug, Clone, PartialEq)]
pub enum LowerError {
    #[error("Lower error at line {line}: unknown schema '{name}'")]
    UnknownSchema { line: usize, name: String },

    #[error("Lower error at line {line}: undeclared field '{field}' in schema '{schema}'")]
    UndeclaredField {
        line: usize,
        schema: String,
        field: String,
    },

    #[error("Lower error at line {line}: missing field '{field}' in struct literal '{schema}'")]
    MissingField {
        line: usize,
        schema: String,
        field: String,
    },

    #[error("Lower error at line {line}: type mismatch: expected {expected}, found {actual}")]
    TypeMismatch {
        line: usize,
        expected: String,
        actual: String,
    },

    #[error("Lower error at line {line}: undefined variable '{name}'")]
    UndefinedVariable { line: usize, name: String },

    #[error("Lower error at line {line}: variable '{name}' was already moved at line {moved_line}")]
    UseAfterMove {
        line: usize,
        name: String,
        moved_line: usize,
    },

    #[error("Lower error at line {line}: unknown function '{name}'")]
    UnknownFunction { line: usize, name: String },

    #[error("Lower error at line {line}: argument count mismatch for '{func}': expected {expected}, got {actual}")]
    ArgCountMismatch {
        line: usize,
        func: String,
        expected: usize,
        actual: usize,
    },

    #[error("Lower error at line {line}: missing return statement in function '{func}' returning {ret_ty}")]
    MissingReturn {
        line: usize,
        func: String,
        ret_ty: String,
    },

    #[error("Lower error at line {line}: unsupported binary operator '{op}' for types {lhs} and {rhs}")]
    UnsupportedBinaryOp {
        line: usize,
        op: String,
        lhs: String,
        rhs: String,
    },

    #[error("Lower error at line {line}: undeclared effect '{effect}': {msg}")]
    UndeclaredEffect {
        line: usize,
        effect: String,
        msg: String,
    },

    #[error("Lower error at line {line}: {msg}")]
    InvalidConstruct { line: usize, msg: String },
}

impl LowerError {
    pub fn line(&self) -> usize {
        match self {
            LowerError::UnknownSchema { line, .. }
            | LowerError::UndeclaredField { line, .. }
            | LowerError::MissingField { line, .. }
            | LowerError::TypeMismatch { line, .. }
            | LowerError::UndefinedVariable { line, .. }
            | LowerError::UseAfterMove { line, .. }
            | LowerError::UnknownFunction { line, .. }
            | LowerError::ArgCountMismatch { line, .. }
            | LowerError::MissingReturn { line, .. }
            | LowerError::UnsupportedBinaryOp { line, .. }
            | LowerError::UndeclaredEffect { line, .. }
            | LowerError::InvalidConstruct { line, .. } => *line,
        }
    }

    pub fn kind_str(&self) -> &'static str {
        match self {
            LowerError::UnknownSchema { .. } => "UnknownSchema",
            LowerError::UndeclaredField { .. } => "UndeclaredField",
            LowerError::MissingField { .. } => "MissingField",
            LowerError::TypeMismatch { .. } => "TypeMismatch",
            LowerError::UndefinedVariable { .. } => "UndeclaredVariable",
            LowerError::UseAfterMove { .. } => "UseAfterMove",
            LowerError::UnknownFunction { .. } => "UnknownFunction",
            LowerError::ArgCountMismatch { .. } => "ArgCountMismatch",
            LowerError::MissingReturn { .. } => "MissingReturn",
            LowerError::UnsupportedBinaryOp { .. } => "UnsupportedBinaryOp",
            LowerError::UndeclaredEffect { .. } => "UndeclaredEffect",
            LowerError::InvalidConstruct { .. } => "InvalidConstruct",
        }
    }

    pub fn message(&self) -> String {
        self.to_string()
    }

    pub fn variable_name(&self) -> Option<String> {
        match self {
            LowerError::UndefinedVariable { name, .. }
            | LowerError::UseAfterMove { name, .. } => Some(name.clone()),
            _ => None,
        }
    }
}
