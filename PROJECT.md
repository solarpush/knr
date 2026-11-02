# Project: knr Linear SSA Language & Toolchain

## Architecture
The `knr` programming language is a high-performance, verifiable systems language based on a linear Single Static Assignment (SSA) intermediate representation. It enforces memory safety without garbage collection or reference counting via a compile-time linear ownership state machine (`Alive` vs `Moved`).

The system is organized into a modular 6-crate Rust workspace:
1. `knr-core`: Abstract Syntax Tree (AST), Types, Effects, Opcodes, Textual `.ssa` parser/formatter, and Binary `.knrbin` codec with `"KNIR"` magic header.
2. `knr-verify`: Formal static analysis passes (monotonic SSA dominance, linear memory state machine, static typing, effect capabilities, and structured JSON diagnostics).
3. `knr-runtime`: Abstract `HostPlatform` capability trait and concrete platform adapters (`Net` via Tokio, `Ui` via VDOM tree, `Hw` via 64-byte aligned SIMD XOR buffers) plus `extern "C"` ABI bindings.
4. `knr-interp`: Deterministic reference VM oracle, cycle-counting gas metering (`max_steps`), and step-by-step JSON execution tracing (`--trace`).
5. `knr-codegen`: Dual compilation backends: Cranelift native code generation (supporting JIT execution and AOT ELF/Mach-O object file emission) and WebAssembly binary generation (`wasm-encoder`).
6. `knr-cli`: Universal command-line tool `knr` exposing subcommands `check`, `run`, `build`, `asm`, and `disasm`.

Differential fuzzing with `proptest` guarantees exact bitwise and semantic parity between the `knr-interp` reference oracle and `knr-codegen` native execution across at least 1,000 generated valid programs.

```text
                  ┌───────────────────────┐
                  │      knr-core         │
                  │ (AST, Types, Codecs)  │
                  └──────────┬────────────┘
                             │
            ┌────────────────┼────────────────┐
            ▼                ▼                ▼
     ┌────────────┐   ┌─────────────┐   ┌────────────┐
     │ knr-verify │   │ knr-runtime │   │ knr-interp │
     └──────┬─────┘   └──────┬──────┘   └─────┬──────┘
            │                │                │
            └───────────┬────┴────────────────┘
                        ▼
                 ┌─────────────┐
                 │ knr-codegen │ (Cranelift + Wasm)
                 └──────┬──────┘
                        │
                        ▼
                 ┌─────────────┐
                 │   knr-cli   │ (check, run, build, asm, disasm)
                 └─────────────┘
```

## Feature Inventory

| # | Feature | Description | Milestone | Source |
|---|---------|-------------|-----------|--------|
| 1 | Cargo Workspace Setup | Root `Cargo.toml` with resolver "2", member crates, and workspace dependencies | M1 | SPEC.md §1 |
| 2 | Crate Skeletons | Creation of 6 crate skeletons in `crates/` with complete `Cargo.toml` manifests | M1 | SPEC.md §1 |
| 3 | Scalar Types | Implementation of `i64`, `f64`, `bool`, `void` as `Copy` types in `Type` enum | M1 | SPEC.md §2.1 |
| 4 | Linear Types | Implementation of `buf`, `str`, `vec[T]`, `map[K, V]`, `chan[T]` as `Move` types | M1 | SPEC.md §2.1 |
| 5 | Linear Predicate | `Type::is_linear()` predicate identifying move-semantics types | M1 | SPEC.md §2.3 |
| 6 | Effect Enum | Capability effects: `Net`, `Ui`, `Hw`, `Async` (pure functions have empty list) | M1 | SPEC.md §2.3 |
| 7 | Constant Opcodes | `const_i64`, `const_f64`, `const_bool`, `const_str` opcodes | M1 | SPEC.md §2.2 |
| 8 | Arithmetic Opcodes | `add`, `sub`, `mul`, `div` opcodes for `i64` and `f64` | M1 | SPEC.md §2.2 |
| 9 | Comparison Opcodes | `cmp_eq`, `cmp_lt` opcodes returning boolean | M1 | SPEC.md §2.2 |
| 10 | Buffer Opcodes | `buf_alloc`, `buf_read_u8` (borrow), `buf_write_u8` (consume), `buf_slice` | M1 | SPEC.md §2.2 |
| 11 | Vector Opcodes | `vec_new`, `vec_push` (consume), `vec_get` (borrow), `vec_len` | M1 | SPEC.md §2.2 |
| 12 | Map Opcodes | `map_new`, `map_set` (consume), `map_get` (borrow), `map_del` | M1 | SPEC.md §2.2 |
| 13 | Channel Opcodes | `chan_new`, `chan_send` (consume val, borrow chan), `chan_recv` | M1 | SPEC.md §2.2 |
| 14 | Task & System Opcodes | `spawn`, `syscall`, `free` (explicit deallocation) | M1 | SPEC.md §2.2 |
| 15 | Terminators | `ret`, `br`, `br_if` terminating basic blocks | M1 | SPEC.md §2.3 |
| 16 | Core AST Structures | `Instruction`, `BasicBlock`, `Function`, `Module` structs | M1 | SPEC.md §2.3 |
| 17 | Textual SSA Parser | Line-by-line / tokenized parser for `.ssa` source files with line numbers | M1 | SPEC.md §2.4 |
| 18 | Canonical Formatter | Normalizing `.ssa` printer for canonical formatting | M1 | SPEC.md §2.4 |
| 19 | Binary Header Codec | `"KNIR"` magic header (`0x4B 0x4E 0x49 0x52`) and version (`0x0001`) | M1 | SPEC.md §2.4 |
| 20 | Binary Constant Pool | Deduplicated string and IEEE-754 float constant pool | M1 | SPEC.md §2.4 |
| 21 | Fixed 64-Bit Instruction Codec | Fixed 8-byte instruction encoding `[opcode, ty, dst, src1, src2]` | M1 | SPEC.md §2.4 |
| 22 | Extra Operands Pool | Binary pool for 3-operand instructions (`buf_write_u8`, `map_set`, `br_if`) | M1 | SPEC.md §2.4 |
| 23 | 1:1 Roundtrip Bijection | Exact byte-for-byte bijection: `format(decode(encode(parse(text)))) == text` | M1 | SPEC.md §8 |
| 24 | SSA Monotonicity Pass | Validates strictly increasing register IDs (`%0`, `%1`, `%n`) | M2 | SPEC.md §3 |
| 25 | SSA Dominance Pass | Validates operand registers strictly dominate instructions where used | M2 | SPEC.md §3 |
| 26 | SSA Single Assignment Pass | Validates no register is defined more than once | M2 | SPEC.md §3 |
| 27 | Linear State Machine Tracker | Tracks per-register state: `Alive(Type)` vs `Moved { consumed_at_line }` | M2 | SPEC.md §3 |
| 28 | Use-After-Move Detection | Prohibits borrowing or consuming a register in `Moved` state | M2 | SPEC.md §3 |
| 29 | Linear Leak Detection | Prohibits unreturned, unfreed linear registers at `Terminator::Ret` | M2 | SPEC.md §3 |
| 30 | Static Type Checker | Verifies operand types match operation signatures across all opcodes | M2 | SPEC.md §3 |
| 31 | Effect Capability Pass | Verifies `Syscall`, `ChanNew`, `Spawn` match function effect declarations | M2 | SPEC.md §3 |
| 32 | Structured JSON Diagnostics | Outputs structured diagnostic with error_kind, line, violated_register, alive_regs, hint | M2 | SPEC.md §3 |
| 33 | 20 Validation Unit Tests | Standard test suite for SSA, linear rules, types, effects, and valid programs | M2 | SPEC.md §8 |
| 34 | HostPlatform Trait | Trait `HostPlatform: Send + Sync` for effect-isolated host operations | M3 | SPEC.md §5 |
| 35 | Net Adapter (`net.listen`) | TCP socket listening pushing requests (`Map`) into channel via Tokio | M3 | SPEC.md §5.1 |
| 36 | Net Adapter (`net.respond`) | Zero-copy response streaming to TCP socket | M3 | SPEC.md §5.1 |
| 37 | Ui Adapter (VDOM Schema) | Virtual DOM schema convention in `Map` (`"tag"`, `"props"`, `"text"`, `"children"`) | M3 | SPEC.md §5.2 |
| 38 | Ui Adapter (`ui.mount`) | Mounts VDOM tree and returns user event channel | M3 | SPEC.md §5.2 |
| 39 | Ui Adapter Native SSR | Serializes VDOM `Map` directly to HTML string | M3 | SPEC.md §5.2 |
| 40 | Hw Adapter 64-Byte Alignment | Strict 64-byte aligned buffer allocation using `Layout::from_size_align` | M3 | SPEC.md §5.3 |
| 41 | Hw Adapter (`hw.mmio_map`) | Maps physical memory or mmap into aligned `buf` | M3 | SPEC.md §5.3 |
| 42 | Hw Adapter (`hw.simd_xor`) | Vector SIMD XOR on two 64-byte aligned buffers | M3 | SPEC.md §5.3 |
| 43 | C-ABI Runtime Bindings | Exported `extern "C"` functions for in-place data structures and buffer memory | M3 | SPEC.md §6.1 |
| 44 | Reference VM Oracle | Deterministic execution semantics for all 28 SSA instructions | M4 | SPEC.md §4 |
| 45 | Value Representation | `Value` enum (`I64`, `F64`, `Bool`, `Str`, `Buf`, `Vec`, `Map` with BTreeMap, `Chan`) | M4 | SPEC.md §4 |
| 46 | Gas Metering (`max_steps`) | Execution step limiter preventing infinite loops (`VmError::GasExhausted`) | M4 | SPEC.md §4 |
| 47 | Execution Tracing (`--trace`) | Records `(step, line, block, opcode, dst, val)` to structured JSON | M4 | SPEC.md §4 |
| 48 | Deterministic Control Flow | Program counter and basic block terminator execution | M4 | SPEC.md §4 |
| 49 | Cranelift Scalar Lowering | Lowers scalar types to Cranelift `I64` and `F64` | M5 | SPEC.md §6.1 |
| 50 | Cranelift Linear Lowering | Lowers linear types to opaque 64-bit machine pointers (`types::I64`) | M5 | SPEC.md §6.1 |
| 51 | Cranelift Pure Instruction Lowering | Lowers arithmetic and comparison to native machine instructions | M5 | SPEC.md §6.1 |
| 52 | Cranelift In-Place Runtime Calls | Lowers container operations to direct C-ABI runtime calls | M5 | SPEC.md §6.1 |
| 53 | Cranelift JIT Module | In-memory JIT execution of SSA functions for differential testing | M5 | SPEC.md §6.1 |
| 54 | Cranelift Object Emission | Compiles SSA module to native object file (`.o`) via `cranelift-object` | M5 | SPEC.md §6.1 |
| 55 | Native Executable Linker | Links object file with runtime library to produce standalone executable | M5 | SPEC.md §6.1 |
| 56 | Wasm Module Generation | Emits standard `.wasm` binary module via `wasm-encoder` | M5 | SPEC.md §6.2 |
| 57 | Wasm Linear Memory Model | Configures 64 KiB memory pages via `MemorySection` | M5 | SPEC.md §6.2 |
| 58 | Wasm Runtime Imports | Emits imports from `"env"` module (`env.rt_map_set`, etc.) | M5 | SPEC.md §6.2 |
| 59 | Wasm Local Variable Lowering | Maps SSA registers to typed Wasm locals (`local.get`, `local.set`) | M5 | SPEC.md §6.2 |
| 60 | Wasm Control Flow Lowering | Lowers SSA basic blocks and branches to structured Wasm control flow | M5 | SPEC.md §6.2 |
| 61 | CLI Main Dispatcher | Universal entry point with subcommands (`check`, `run`, `build`, `asm`, `disasm`) | M6 | SPEC.md §7 |
| 62 | CLI Check Subcommand | Statically verifies `.ssa` program with optional `--json` structured errors | M6 | SPEC.md §7 |
| 63 | CLI Run Subcommand | Verifies and runs program in `knr-interp` with `--gas` and `--trace` | M6 | SPEC.md §7 |
| 64 | CLI Build Subcommand | Ahead-Of-Time compilation via `--target native|wasm` and `-o <path>` | M6 | SPEC.md §7 |
| 65 | CLI Asm Subcommand | Assembles textual `.ssa` into compact `.knrbin` binary | M6 | SPEC.md §7 |
| 66 | CLI Disasm Subcommand | Disassembles `.knrbin` into canonical `.ssa` text | M6 | SPEC.md §7 |
| 67 | Valid & Invalid Fixtures Suite | Suite of valid and negative `.ssa` programs in `tests/fixtures/` | M6 | SPEC.md §8 |
| 68 | Generative SSA Strategy | `proptest` strategy generating terminating, type-correct SSA programs | M6 | SPEC.md §8 |
| 69 | Differential Fuzzing Runner | Compares VM oracle and Cranelift JIT outputs across 1,000 valid programs | M6 | SPEC.md §8 |
| 70 | E2E Test Suite Tiers 1-4 | End-to-end requirement tests (Features, Boundaries, Pairwise, Real-world) | M7 | Dual Track |
| 71 | Tier 5 Adversarial Coverage | Adversarial white-box tests closing code coverage and integrity gaps | M7 | Dual Track |

## Milestones

| # | Name | Scope | Dependencies | Status |
|---|------|-------|-------------|--------|
| 1 | Wave 1: Workspace & knr-core | Root `Cargo.toml`, 6 crate skeletons, AST types/effects/opcodes, `.ssa` parser/formatter, `.knrbin` codec with `"KNIR"` header, 1:1 roundtrip bijection | none | DONE |
| 2 | Wave 2: knr-verify | 4 static analysis passes, linear ownership state machine, JSON diagnostics, 20 validation unit tests | M1 | DONE |
| 3 | Wave 2: knr-runtime | `HostPlatform` trait, Net (Tokio), Ui (VDOM), Hw (64-byte alignment & SIMD XOR), C-ABI symbols | M1 | DONE |
| 4 | Wave 2: knr-interp | Deterministic reference VM oracle, `Value` enum, gas metering `max_steps`, JSON trace `--trace` | M1, M3 | DONE |
| 5 | Wave 2: knr-codegen | Cranelift native backend (JIT + AOT) and WebAssembly backend (`wasm-encoder`) | M1, M2, M3 | DONE |
| 6 | Wave 3: knr-cli & Differential Fuzzing | CLI binary (`check`, `run`, `build`, `asm`, `disasm`), `tests/fixtures/`, `proptest` differential fuzzing | M1, M2, M3, M4, M5 | PLANNED |
| 7 | Final Milestone: E2E Tests & Adversarial Hardening | Phase 1: 100% pass of E2E test suite (Tiers 1-4); Phase 2: Tier 5 adversarial coverage hardening | M6, TEST_READY.md | PLANNED |

## Interface Contracts

### knr-core ↔ All Crates
- `knr_core::Type`: `I64`, `F64`, `Bool`, `Str`, `Buf`, `Vec(Box<Type>)`, `Map(Box<Type>, Box<Type>)`, `Chan(Box<Type>)`, `Void`.
- `knr_core::Effect`: `Net`, `Ui`, `Hw`, `Async`.
- `knr_core::Op`: 28 opcodes (`ConstI64`..`Free`).
- `knr_core::Instruction`: `{ dst: Option<RegId>, ty: Type, op: Op, line: usize }`.
- `knr_core::Terminator`: `Ret(Option<RegId>)`, `Br(BlockId)`, `BrIf { cond: RegId, then_blk: BlockId, else_blk: BlockId }`.
- `knr_core::BasicBlock`: `{ id: BlockId, instructions: Vec<Instruction>, terminator: Terminator }`.
- `knr_core::Function`: `{ name: String, params: Vec<(RegId, Type)>, ret_ty: Type, effects: Vec<Effect>, blocks: Vec<BasicBlock> }`.
- `knr_core::Module`: `{ functions: Vec<Function> }`.
- `knr_core::parse_ssa(text: &str) -> Result<Module, ParseError>`.
- `knr_core::format_ssa(module: &Module) -> String`.
- `knr_core::encode_knrbin(module: &Module) -> Result<Vec<u8>, BinaryError>`.
- `knr_core::decode_knrbin(bytes: &[u8]) -> Result<Module, BinaryError>`.

### knr-verify ↔ Consumers (CLI, Codegen)
- `pub fn verify_function(func: &Function) -> Result<(), Diagnostic>`.
- `pub fn verify_module(module: &Module) -> Result<(), Vec<Diagnostic>>`.
- Diagnostic schema: `{ status: "error", error_kind: String, function: String, line: usize, instruction: String, violated_register: Option<String>, consumed_at_line: Option<usize>, alive_registers: Option<BTreeMap<String, String>>, fix_hint: Option<String> }`.

### knr-runtime ↔ knr-interp & knr-codegen
- `pub trait HostPlatform: Send + Sync { fn host_call(&mut self, effect: Effect, op: &str, payload: Value) -> Result<Value, RuntimeError>; }`.
- Implementations: `NetPlatform`, `UiPlatform`, `HwPlatform`, `MockPlatform`.
- Buffer memory layout: 64-byte alignment strictly enforced via `std::alloc::Layout::from_size_align(size, 64)`.
- C-ABI symbols: `extern "C" fn rt_map_new()`, `rt_map_set()`, `rt_map_get()`, `rt_vec_new()`, `rt_vec_push()`, `rt_buf_alloc_aligned()`, `rt_buf_free()`.

### knr-interp ↔ Consumers (CLI, Differential Fuzzing)
- `pub fn execute(func: &Function, args: Vec<Value>, platform: &mut dyn HostPlatform, max_steps: u64, trace: bool) -> Result<(Value, Vec<TraceRecord>), VmError>`.
- `enum Value`: `I64(i64)`, `F64(f64)`, `Bool(bool)`, `Str(String)`, `Buf(Vec<u8>)`, `Vec(Vec<Value>)`, `Map(BTreeMap<String, Value>)`, `Chan(usize)`, `Void`.
- `struct TraceRecord`: `{ step: u64, line: usize, block: BlockId, opcode: String, dst_reg: Option<String>, new_value: Option<serde_json::Value> }`.

### knr-codegen ↔ Consumers (CLI, Differential Fuzzing)
- `CraneliftBackend::new() -> Result<Self, CodegenError>`.
- `CraneliftBackend::compile_jit(&mut self, func: &Function) -> Result<*const u8, CodegenError>`.
- `CraneliftBackend::compile_object(&mut self, module: &Module) -> Result<Vec<u8>, CodegenError>`.
- `WasmBackend::new() -> Self`.
- `WasmBackend::compile(&self, module: &Module) -> Result<Vec<u8>, CodegenError>`.

### knr-cli ↔ End User
- Subcommands: `check <file> [--json]`, `run <file> [--gas N] [--trace]`, `build <file> --target <native|wasm> -o <out>`, `asm <file> -o <out>`, `disasm <file> [-o <out>]`.

## Code Layout

```text
/home/pierre/Projects/krn/
├── Cargo.toml                              # Root workspace configuration
├── README.md                               # Project documentation
├── SPEC.md                                 # Technical specification
├── PROJECT.md                              # Living orchestrator index
├── TEST_INFRA.md                           # E2E Test Suite architecture and coverage matrix
├── TEST_READY.md                           # Signal that E2E test suite is published
├── crates/
│   ├── knr-core/
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── ast.rs
│   │       ├── error.rs
│   │       ├── parser.rs
│   │       ├── formatter.rs
│   │       └── binary/
│   │           ├── mod.rs
│   │           ├── header.rs
│   │           ├── const_pool.rs
│   │           └── instruction.rs
│   ├── knr-verify/
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── ssa.rs
│   │       ├── linear.rs
│   │       ├── types.rs
│   │       ├── effects.rs
│   │       └── diagnostic.rs
│   ├── knr-runtime/
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── platform.rs
│   │       ├── net.rs
│   │       ├── ui.rs
│   │       ├── hw.rs
│   │       └── ffi.rs
│   ├── knr-interp/
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── value.rs
│   │       ├── vm.rs
│   │       └── trace.rs
│   ├── knr-codegen/
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── cranelift/
│   │       └── wasm/
│   └── knr-cli/
│       ├── Cargo.toml
│       └── src/
│           └── main.rs
└── tests/
    ├── fixtures/
    │   ├── valid/
    │   └── invalid/
    ├── differential/
    │   └── proptest_vm_codegen.rs
    └── e2e/
        ├── harness/
        ├── tier1_features/
        ├── tier2_boundaries/
        ├── tier3_pairwise/
        └── tier4_workloads/
```
