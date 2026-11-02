use thiserror::Error;

#[derive(Debug, Error)]
pub enum CodegenError {
    #[error("Cranelift codegen error: {0}")]
    Cranelift(String),
    #[error("Wasm codegen error: {0}")]
    Wasm(String),
    #[error("Verification error: {0}")]
    Verify(String),
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Module error: {0}")]
    Module(String),
}

impl From<cranelift_module::ModuleError> for CodegenError {
    fn from(err: cranelift_module::ModuleError) -> Self {
        CodegenError::Cranelift(err.to_string())
    }
}
