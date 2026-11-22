pub mod cranelift;
pub mod error;
pub mod wasm;
pub mod wgsl;

#[cfg(target_os = "linux")]
pub use cranelift::MmapPageAllocator;
pub use cranelift::CraneliftBackend;
pub use error::CodegenError;
pub use wasm::WasmBackend;
pub use wgsl::{CompiledShader, WgslBackend};
