use thiserror::Error;

#[derive(Error, Debug)]
pub enum CliError {
    #[error("{0}")]
    Args(String),

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Parse error: {0}")]
    Parse(#[from] knr_core::ParseError),

    #[error("Verification failed with {0} diagnostic(s)")]
    Verify(usize),

    #[error("VM error: {0}")]
    Vm(#[from] knr_interp::VmError),

    #[error("Binary format error: {0}")]
    Binary(#[from] knr_core::BinaryError),

    #[error("Codegen error: {0}")]
    Codegen(#[from] knr_codegen::CodegenError),

    #[error("Runtime error: {0}")]
    Runtime(#[from] knr_runtime::RuntimeError),

    #[error("Linker error: {0}")]
    Linker(String),

    #[error("{0}")]
    Other(String),
}
