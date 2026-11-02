# End-to-End Test Infrastructure & Architecture Specification (`TEST_INFRA.md`)

> **Project**: `knr` Linear SSA Language & Toolchain  
> **Status**: Living Architectural Specification  
> **Methodology**: Dual Track Opaque-Box Requirement-Driven Testing  
> **Coverage Scope**: Complete 71-Feature Inventory (`PROJECT.md § Feature Inventory`)  
> **Test Tiers**: Tier 1 (Feature Coverage), Tier 2 (Boundaries & Corners), Tier 3 (Pairwise Interactions), Tier 4 (Real-World Workloads), Tier 5 (Adversarial Hardening)

---

## 1. Testing Philosophy & Dual Track Methodology

The `knr` test infrastructure operates under a **Dual Track Architecture**: an independent testing track running parallel to the implementation waves. The testing track authors, maintains, and executes tests against formal specifications before, during, and after feature implementation.

```text
 ┌────────────────────────────────────────────────────────────────────────┐
 │                        AUTHORITATIVE SPECIFICATIONS                    │
 │         ORIGINAL_REQUEST.md  │  SPEC.md  │  README.md  │  PROJECT.md   │
 └───────────────────┬────────────────────────────────┬───────────────────┘
                     │                                │
                     ▼                                ▼
     ┌───────────────────────────────┐┌───────────────────────────────┐
     │      IMPLEMENTATION TRACK     ││         TESTING TRACK         │
     │  Wave 1: knr-core             ││  Tier 1: Feature Coverage     │
     │  Wave 2: verify/interp/runtime││  Tier 2: Boundary & Corners   │
     │  Wave 3: codegen/cli/fuzzing  ││  Tier 3: Pairwise Interactions│
     │  Wave 4: final integration    ││  Tier 4: Real-World Workloads │
     └───────────────┬───────────────┘└───────────────┬───────────────┘
                     │                                │
                     └────────────────┬───────────────┘
                                      ▼
                      ┌───────────────────────────────┐
                      │   PROGRESSIVE VERIFICATION    │
                      │  cargo test / scripts/run_e2e │
                      └───────────────────────────────┘
```

### 1.1 Core Principles

1. **Opaque-Box & Requirement-Driven**: Tests derive strictly from user requirements, syntax grammar (`SPEC.md § 2.4`), opcode tables (`SPEC.md § 2.2`), verification invariants (`SPEC.md § 3`), VM semantics (`SPEC.md § 4`), runtime bridges (`SPEC.md § 5`), and CLI interface contracts (`SPEC.md § 7`). Tests never inspect or depend upon private implementation details.
2. **Authoritative Expected Output Derivation**: For every test case, the expected output is derived from:
   - Direct mathematical properties (e.g. arithmetic, IEEE-754 floats).
   - Exact binary format layout (4-byte `"KNIR"` magic header, fixed 8-byte instruction words).
   - Reference VM execution output (`knr-interp`) serving as semantic oracle.
   - Structured JSON diagnostic schemas (`SPEC.md § 3`).
   - Exact CLI exit codes and formatted outputs (`SPEC.md § 7`).
3. **Progressive Testability**: Tests must be executable as soon as the corresponding milestone features are implemented, without requiring downstream milestone components:
   - **Wave 1 Tests**: Executable against `knr-core` parser, AST, and binary codec.
   - **Wave 2 Tests**: Executable against `knr-verify`, `knr-runtime`, and `knr-interp`.
   - **Wave 3 Tests**: Executable against `knr-codegen` (Cranelift/WASM) and `knr-cli`.
   - **Wave 4 Tests**: Full cross-tier regression, differential fuzzing, and adversarial stress.
4. **Self-Contained & Isolated**: Every test manages its own fixtures, temporary files, and state, cleaning up afterwards and running deterministically in parallel.

---

## 2. 4-Tier Test Architecture & Coverage Thresholds

The testing framework organizes all validation into 4 structured tiers (plus Tier 5 adversarial hardening):

```text
 ┌────────────────────────────────────────────────────────────────────────┐
 │ Tier 4: Real-World Workloads (>=5 End-to-End Macro Scenarios)          │
 │         HTTP Server, VDOM UI, MMIO/SIMD, Compiler Pipeline, Concurrency│
 ├────────────────────────────────────────────────────────────────────────┤
 │ Tier 3: Cross-Feature Interactions & Pairwise Combinations             │
 │         State sharing, Data flow, Control flow across subsystem pairs  │
 ├────────────────────────────────────────────────────────────────────────┤
 │ Tier 2: Boundary & Corner Cases (>=5 test cases per feature)           │
 │         Domain extremes, empty/max inputs, NaN/Inf, register overflow  │
 ├────────────────────────────────────────────────────────────────────────┤
 │ Tier 1: Feature Coverage (>=5 test cases per feature)                  │
 │         Happy-path, nominal execution, isolated contract verification  │
 └────────────────────────────────────────────────────────────────────────┘
```

### 2.1 Tier Definitions and Thresholds

| Tier | Name | Mandatory Threshold | Scope & Methodology |
|:---:|---|:---:|---|
| **Tier 1** | **Feature Coverage** | $\ge 5$ test cases per feature | Verifies nominal, happy-path functionality of each feature in isolation. Category-partition testing across valid input equivalence classes. |
| **Tier 2** | **Boundary & Corner Cases** | $\ge 5$ test cases per feature | Boundary Value Analysis (BVA), extreme values (empty strings, zero buffers, `i64::MIN`/`MAX`, `NaN`, `-0.0`, register limit `65535`), corrupted inputs, error triggers. |
| **Tier 3** | **Pairwise Combinations** | Combinatorial matrix across subsystems | Orthogonal array / pairwise testing of interacting features (e.g. Linear Types $\times$ Branching, Buffer Mutation $\times$ SIMD XOR, Gas Exhaustion $\times$ JSON Tracing). |
| **Tier 4** | **Real-World Workloads** | $\ge 5$ end-to-end macro applications | Production-grade realistic workloads executing across the full stack: HTTP Web API, Reactive VDOM UI, MMIO Hardware Packet Processing, Compiler Roundtrip Pipeline, Concurrent Worker Pool. |
| **Tier 5** | **Adversarial Hardening** | Differential & Fuzzing | Differential fuzzing between VM oracle and JIT compiler ($\ge 1,000$ valid programs via `proptest`), malformed binary injection, resource exhaustion stress. |

---

## 3. Complete 71-Feature Coverage Matrix

Every feature in `PROJECT.md § Feature Inventory` is mapped to Tiers 1–4, with explicit verification mechanisms and authoritative sources:

| # | Feature Name | M | Tier 1: Feature Coverage ($\ge 5$ Cases) | Tier 2: Boundary Cases ($\ge 5$ Cases) | Tier 3: Pairwise Combinations | Tier 4: Workload Integration | Verification Method |
|:---:|---|:---:|---|---|---|---|---|
| **1** | **Cargo Workspace Setup** | M1 | Root manifest resolution; member discovery; workspace dependency propagation; profiles (`dev`, `release`); resolver "2" validation. | Empty crate set; cyclic dependencies; missing member directory; duplicate member name; invalid toml syntax. | Workspace $\times$ Feature flags; Workspace $\times$ Target triples (`x86_64`, `wasm32`). | Build artifact packaging for all 5 macro workloads. | `cargo check --workspace` & Cargo manifest parser |
| **2** | **Crate Skeletons** | M1 | Skeleton build for 6 crates (`core`, `verify`, `runtime`, `interp`, `codegen`, `cli`); `src/lib.rs` / `main.rs` presence; crate exports; intra-workspace dependencies; edition 2021. | Missing dependency declarations; mismatched version strings; empty source files; compilation with no-default-features; circular dev-dependencies. | Skeletons $\times$ Cross-compilation; Skeletons $\times$ Doc generation. | Compilation of full macro pipeline. | `cargo build --workspace` |
| **3** | **Scalar Types** | M1 | Instantiation of `i64`, `f64`, `bool`, `void`; equality checking; copy semantics; `Debug`/`Clone` traits; Serde serialization. | Extreme integer bit patterns; IEEE-754 denormals; float negative zero `-0.0`; `NaN` equality behavior; void type representation in registers. | Scalar Types $\times$ Binary Constant Pool; Scalar Types $\times$ Arithmetic Opcodes. | Type signatures in HTTP Server and Matrix math workloads. | AST type unit tests & Serde JSON roundtrip |
| **4** | **Linear Types** | M1 | Instantiation of `buf`, `str`, `vec[T]`, `map[K, V]`, `chan[T]`; recursive parametric nesting (`vec[map[str, buf]]`); move semantics tagging; Serde serialization; equality check. | Empty parametric containers; deeply nested container depth ($\ge 16$ levels); self-referential type attempts; large type tree; single-element containers. | Linear Types $\times$ State Machine Tracker; Linear Types $\times$ Formatter. | Buffer allocation and VDOM tree structures in UI/Net workloads. | AST type unit tests & Serde roundtrip |
| **5** | **Linear Predicate** | M1 | `is_linear()` returns true for `buf`, `str`, `vec`, `map`, `chan`; false for `i64`, `f64`, `bool`, `void`; nested `vec[i64]` linear check; nested `map[str, i64]` linear check; nested `chan[buf]` linear check. | All 9 variants evaluated in loop; custom nested types; recursive match exhaustiveness; immutable borrow safety; performance consistency. | Linear Predicate $\times$ Type Checker; Linear Predicate $\times$ Linear Leak Pass. | Memory cleanup validation in all workloads. | Property-based testing on arbitrary `Type` instances |
| **6** | **Effect Enum** | M1 | `Net`, `Ui`, `Hw`, `Async` representation; pure function representation (`vec![]`); hashing behavior in `HashSet`; serialization; clone and equality. | All $2^4 = 16$ power set combinations of effects; duplicate effect entries; invalid effect string; effect ordering normalization; empty effect list. | Effect Enum $\times$ Syscall Opcode; Effect Enum $\times$ Capability Verifier Pass. | Capability validation in HTTP (`Net`), VDOM (`Ui`), MMIO (`Hw`) workloads. | AST enum assertions & Serde JSON checks |
| **7** | **Constant Opcodes** | M1 | `const_i64` positive/negative; `const_f64` floats; `const_bool` true/false; `const_str` ASCII/UTF-8; instruction line mapping. | `i64::MIN` & `i64::MAX`; `f64::EPSILON`, `f64::INFINITY`, `NaN`; empty string `""`; multibyte unicode (`"🦀"`); escape sequences (`\n`, `\t`, `\"`). | Constant Opcodes $\times$ Binary Constant Pool; Constant Opcodes $\times$ Parser. | Constant literal initialization across all 5 macro fixtures. | Text parser & binary codec roundtrip |
| **8** | **Arithmetic Opcodes** | M1 | `add`, `sub`, `mul`, `div` for `i64`; `add`, `sub`, `mul`, `div` for `f64`; register operand binding; instruction line tagging; opcode enum discrimination. | Integer division by zero representation; float division by zero (`+Inf`); signed integer overflow (`i64::MAX + 1`); subnormal float math; square root approximation sequences. | Arithmetic Opcodes $\times$ SSA Dominance; Arithmetic Opcodes $\times$ Cranelift Lowering. | Packet checksum computation and Matrix math workloads. | AST construction & VM execution parity |
| **9** | **Comparison Opcodes** | M1 | `cmp_eq` for `i64`; `cmp_eq` for `f64`; `cmp_lt` for `i64`; `cmp_lt` for `f64`; boolean result destination register binding. | `NaN == NaN` evaluation (false); `-0.0 == +0.0` (true); `i64::MIN < i64::MAX` (true); identical register operands (`cmp_eq %0, %0`); extreme value comparison. | Comparison Opcodes $\times$ Conditional Branch (`br_if`); Comparison Opcodes $\times$ Verifier Type Check. | Loop termination checks in packet processing and sorting workloads. | AST construction & VM evaluation assertions |
| **10** | **Buffer Opcodes** | M1 | `buf_alloc(size)`; `buf_read_u8(buf, idx)`; `buf_write_u8(buf, idx, val)`; `buf_slice(buf, start, len)`; linear consumption of write. | Zero-length allocation `buf_alloc(0)`; out-of-bounds read; write at exact buffer boundary `size - 1`; slice full buffer; empty slice `len = 0`. | Buffer Opcodes $\times$ 64-byte Alignment; Buffer Opcodes $\times$ SIMD XOR. | Network packet framing and MMIO DMA ring buffers. | AST representation & VM memory model validation |
| **11** | **Vector Opcodes** | M1 | `vec_new(T)`; `vec_push(vec, val)`; `vec_get(vec, idx)`; `vec_len(vec)`; linear consumption of vector on push. | Empty vector length (`vec_len == 0`); get out-of-bounds index; push multiple items sequentially; nested vectors (`vec[vec[i64]]`); push to capacity reallocation. | Vector Opcodes $\times$ In-Place Runtime Calls; Vector Opcodes $\times$ Use-After-Move. | Dynamic request collection in HTTP server workload. | AST representation & VM vector operations |
| **12** | **Map Opcodes** | M1 | `map_new(K, V)`; `map_set(m, k, v)`; `map_get(m, k)`; `map_del(m, k)`; linear consumption of map on set/del. | Set existing key (overwrite); get non-existent key; delete non-existent key; map with empty string key; map with large number of keys ($\ge 100$). | Map Opcodes $\times$ VDOM Schema; Map Opcodes $\times$ In-place Mutation. | VDOM props tree and HTTP header dictionary. | AST representation & VM BTreeMap operations |
| **13** | **Channel Opcodes** | M1 | `chan_new(T, cap)`; `chan_send(chan, val)`; `chan_recv(chan)`; FIFO ordering; channel capacity binding. | Zero-capacity channel; full channel send; empty channel recv; send linear container into channel; channel with capacity `1`. | Channel Opcodes $\times$ Async Effect; Channel Opcodes $\times$ Spawn Opcode. | Worker thread task queue and UI event dispatch. | AST representation & VM channel semantics |
| **14** | **Task & System Opcodes** | M1 | `spawn(fn_name, chan)`; `syscall(effect, op, payload)`; `free(reg)`; linear destruction; effect binding. | Double free attempt; free already moved register; syscall with empty payload; spawn nonexistent function; free scalar (ignored or rejected). | Task Opcodes $\times$ HostPlatform Trait; Task Opcodes $\times$ Capability Pass. | Background worker spawning and host I/O invocations. | AST representation & VM host calls |
| **15** | **Terminators** | M1 | `ret %reg`; `ret` (void); `br bbN`; `br_if %cond bbThen bbElse`; basic block termination invariant. | Branch to self block; conditional branch where then == else; return of linear value; missing terminator in block; unreachable block after ret. | Terminators $\times$ SSA Dominance; Terminators $\times$ Wasm Control Flow. | Control flow in all 5 macro fixtures. | AST terminator invariants & CFG validation |
| **16** | **Core AST Structures** | M1 | `Instruction`, `BasicBlock`, `Function`, `Module` structs; block ID mapping; instruction line numbering; function signature serialization. | Module with 0 functions; function with 0 blocks; block with 0 instructions; function with 100 blocks; max register ID within function. | Core AST $\times$ Parser; Core AST $\times$ Binary Codec. | Structure container for all fixtures and workloads. | Struct unit tests & Serde serialization |
| **17** | **Textual SSA Parser** | M1 | Line-by-line parsing of valid `.ssa`; parameter lists; block labels `bb0:`; opcode parsing; line number attribution. | Unterminated string literal; register with invalid prefix (e.g. `$0`, `@1`); trailing comments; multiple empty lines; unknown opcode keyword. | Textual Parser $\times$ Canonical Formatter; Textual Parser $\times$ Error Reporting. | Ingestion of all fixture programs in `tests/fixtures/`. | `knr_core::parse_ssa` returning exact AST |
| **18** | **Canonical Formatter** | M1 | Formats `Module` to canonical text; 4-space block indentation; normalized whitespace; comma-separated parameters; line terminators. | Formatter output on empty function; formatting function with all 28 opcodes; formatting deeply nested types; roundtrip format stability; whitespace normalization. | Canonical Formatter $\times$ Parser; Canonical Formatter $\times$ CLI Disasm. | Canonicalization of all test fixtures. | Exact string comparison against canonical references |
| **19** | **Binary Header Codec** | M1 | Encodes `"KNIR"` (`0x4B 0x4E 0x49 0x52`); version `0x0001` (little-endian); decodes valid header; validates magic bytes; version rejection. | Header truncated to $< 6$ bytes; invalid magic bytes (`"TEST"`); version `0x0002` (unsupported); extra header padding; null byte magic (`0x00 0x00 0x00 0x00`). | Binary Header $\times$ CLI Asm/Disasm; Binary Header $\times$ File I/O. | Binary packaging of all compiled `.knrbin` artifacts. | Byte slice comparisons & `BinaryError` assertions |
| **20** | **Binary Constant Pool** | M1 | UTF-8 string deduplication; IEEE-754 float pool encoding; indexed lookup by instruction operands; pool length prefixes; empty pool serialization. | 1,000 duplicate strings; strings with embedded nulls `\0`; `NaN` bit patterns; pool size $> 65535$ index overflow; float pool with `+0.0` and `-0.0`. | Constant Pool $\times$ Constant Opcodes; Constant Pool $\times$ Roundtrip Bijection. | Constant table serialization for macro workloads. | Binary codec unit tests & pool index checks |
| **21** | **Fixed 64-Bit Instruction Codec** | M1 | Packs instruction into 8 bytes: `[opcode, ty, dst, src1, src2]`; decodes back to `Instruction`; opcodes `0x01`–`0x1F`; `NO_REG` (`0xFFFF`) encoding for `None`. | Instruction with no dst; instruction with 1 operand; max register index `0xFFFE`; opcode `0xFF` (invalid); type ID overflow $> 255$. | 64-Bit Codec $\times$ Extra Operands Pool; 64-Bit Codec $\times$ AST Instruction. | Bytecode emission for all compiled fixtures. | Byte-level packing/unpacking bitwise tests |
| **22** | **Extra Operands Pool** | M1 | Encodes 3-operand instructions (`buf_write_u8`, `buf_slice`, `map_set`, `br_if`); `src2` points to extra pool tuple; decodes back faithfully; empty extra pool. | Multiple 3-operand instructions sharing pool; extra pool index $> 65535$ overflow; invalid index in `src2`; extra pool with max capacity; sequential 3-operand calls. | Extra Operands Pool $\times$ 64-Bit Codec; Extra Operands Pool $\times$ Map/Buffer Opcodes. | Serialization of VDOM map writes and buffer slices. | Binary encoder/decoder roundtrip validation |
| **23** | **1:1 Roundtrip Bijection** | M1 | `format(decode(encode(parse(text)))) == text` on standard 15-line program; roundtrip on arithmetic module; roundtrip on linear types module; roundtrip on CFG branching; bitwise byte equality. | Roundtrip on minimal 1-instruction function; roundtrip with large constants; roundtrip with complex nested types; roundtrip with comments/whitespace variance; roundtrip across all 28 opcodes. | Roundtrip Bijection $\times$ Parser; Roundtrip Bijection $\times$ Formatter; Roundtrip Bijection $\times$ Codec. | Certification of all valid fixtures in `tests/fixtures/valid/`. | `assert_eq!(format_ssa(&decode_knrbin(&encode_knrbin(&parse_ssa(src)?)?)?), canonical_src)` |
| **24** | **SSA Monotonicity Pass** | M2 | Verifies `%0`, `%1`, `%2` monotonic order; consecutive IDs; single block monotonicity; multi-block monotonic flow; parameter register ordering. | Non-monotonic register jump (e.g. `%0` then `%3` then `%1`); descending registers; negative register ID parsing; gap in register IDs; duplicate register ID. | Monotonicity $\times$ Dominance Pass; Monotonicity $\times$ Single Assignment Pass. | Ingestion check on all generated fixtures. | `knr_verify::verify_function` & JSON diagnostic |
| **25** | **SSA Dominance Pass** | M2 | Validates operand registers defined before use; dominator tree validation; multi-block dominance across forward branches; loop dominance; phi-node dominance. | Use of register defined in parallel block; use of register defined in child block; self-referential instruction (`%1 = add %1 %0`); use of undefined register `%999`; use before definition in same block. | Dominance Pass $\times$ Terminator Branching; Dominance Pass $\times$ In-Place Runtime Calls. | Flow validation in HTTP parser and Matrix loops. | `knr_verify::verify_function` error kind `DominanceViolation` |
| **26** | **SSA Single Assignment Pass** | M2 | Accepts single assignment per register; validates across all basic blocks; parameter register single assignment; terminator register compliance. | Register redefined in same block; register redefined in subsequent block; parameter register overwritten; register defined in multiple branch paths; reuse of destination register. | Single Assignment $\times$ Linear State Machine; Single Assignment $\times$ Cranelift JIT. | Static verification of all valid macro fixtures. | `knr_verify::verify_function` error kind `MultipleAssignment` |
| **27** | **Linear State Machine Tracker** | M2 | Transitions register from `Alive(Type)` to `Moved { line }` on consumption; tracks borrows (`Alive` maintained); tracks multiple linear registers independently; verifies state at each line; function return state. | Transition of all 5 linear types (`buf`, `str`, `vec`, `map`, `chan`); multiple moves in sequential lines; move inside conditional branch; borrow after borrow; move inside loop body. | Linear State Machine $\times$ Use-After-Move; Linear State Machine $\times$ Linear Leak. | Memory lifecycle in HTTP Server and VDOM UI. | State tracker unit inspections & verifier tests |
| **28** | **Use-After-Move Detection** | M2 | Rejects borrowing moved register (`map_get` after `map_set`); rejects consuming moved register (`free` after `free`); rejects moving already moved vector; rejects sending moved buffer; reports line of prior consumption. | Double `map_set` on original register; `vec_push` on original vector; `buf_write_u8` on original buffer; use in terminator after `free`; branch path with move in one path and use in merge. | Use-After-Move $\times$ Structured JSON Diagnostics; Use-After-Move $\times$ Linear Types. | Invalid fixtures `tests/fixtures/invalid/use_after_move_*.ssa`. | `knr_verify::verify_function` error kind `UseAfterMove` |
| **29** | **Linear Leak Detection** | M2 | Rejects unreturned `Alive` linear register at `ret`; rejects unreturned buffer; rejects unreturned map; rejects unreturned vector; rejects unreturned channel. | Multiple linear registers leaked simultaneously; linear register allocated in branch and not freed; linear register replaced but old one leaked; leak at void `ret`; leak in loop exit. | Linear Leak $\times$ Terminator Ret; Linear Leak $\times$ Free Opcode. | Invalid fixtures `tests/fixtures/invalid/linear_leak_*.ssa`. | `knr_verify::verify_function` error kind `LinearLeak` |
| **30** | **Static Type Checker** | M2 | Verifies arithmetic operand types (`i64`+`i64`, `f64`+`f64`); verifies comparison types match; verifies `map_set` key/val match map declaration; verifies `vec_push` element match; verifies `buf_read_u8` returns `i64`. | Adding `i64` to `f64`; pushing `i64` into `vec[str]`; setting `i64` key on `map[str, buf]`; `br_if` with non-boolean condition; returning wrong type for function signature. | Type Checker $\times$ Scalar Types; Type Checker $\times$ Linear Types. | Rejection of ill-typed fixtures. | `knr_verify::verify_function` error kind `TypeMismatch` |
| **31** | **Effect Capability Pass** | M2 | Verifies `Syscall(Net)` allowed only if `Net` in `fn.effects`; verifies `Syscall(Ui)` requires `Ui`; verifies `Syscall(Hw)` requires `Hw`; verifies `chan_new`/`spawn` require `Async`; pure functions reject all effects. | Undeclared `Net` syscall; undeclared `Ui` syscall; undeclared `Hw` MMIO; undeclared `spawn`; function with all 4 capabilities calling pure function (valid). | Effect Pass $\times$ Effect Enum; Effect Pass $\times$ HostPlatform Trait. | Rejection of unauthorized fixtures `effect_omitted_*.ssa`. | `knr_verify::verify_function` error kind `UndeclaredEffect` |
| **32** | **Structured JSON Diagnostics** | M2 | Formats JSON matching exact schema: `status: "error"`, `error_kind`, `function`, `line`, `instruction`, `violated_register`, `consumed_at_line`, `alive_registers_at_line_<N>`, `fix_hint`; validates with `serde_json`. | Missing optional fields serialize as `null` or omitted; alive registers map formatted correctly; fix hint string present; invalid character escaping; JSON schema conformance validation. | JSON Diagnostics $\times$ CLI Check (`--json`); JSON Diagnostics $\times$ Verifier Errors. | Structured error reporting on all invalid fixtures. | JSON schema validator against `knr check --json` output |
| **33** | **20 Validation Unit Tests** | M2 | Executes the 20 authoritative validation tests specified in `ORIGINAL_REQUEST.md:38` and `SPEC.md:384-386`: 5 SSA, 5 linear rules, 5 types/effects, 5 valid programs; 100% pass. | Extreme edge variants of each of the 20 cases; mutation testing of verifier rules; execution time $< 100\ \mu\text{s}$ per test; deterministic error messaging. | 20 Tests $\times$ Verifier Passes; 20 Tests $\times$ Test Fixtures. | Baseline validation suite for CI/CD gates. | `cargo test -p knr-verify` & fixture runners |
| **34** | **HostPlatform Trait** | M3 | `HostPlatform: Send + Sync` trait definition; `host_call` method signature; `MockPlatform` implementation; effect routing; error handling with `RuntimeError`. | Host call with unrecognized op; host call with malformed payload; re-entrant host calls; thread-safety (`Send + Sync`) verification across Tokio tasks; host call returning error. | HostPlatform $\times$ Syscall Opcode; HostPlatform $\times$ VM Context. | Host bridge in all 5 macro workloads. | Trait unit tests & mock invocations |
| **35** | **Net Adapter (`net.listen`)** | M3 | Listens on TCP port via Tokio; receives connection; pushes request `Map` to channel with `"method"`, `"path"`, `"body"` (`Buf`); non-blocking operation; channel backpressure. | Bind to invalid/occupied port; listen on port 0; empty request body; large request payload ($\ge 1\text{ MB}$); client disconnect before reading. | Net Adapter $\times$ Channel Opcodes; Net Adapter $\times$ Map Opcodes. | Macro Workload 1: HTTP Web Server. | Integration test with Tokio TCP client sending HTTP request |
| **36** | **Net Adapter (`net.respond`)** | M3 | Sends zero-copy response to TCP socket; consumes response `Buf`; sets HTTP status code; closes or keeps connection alive; returns success boolean. | Response with empty body; response with 10 MB body; response to closed connection; invalid request ID; concurrent response streaming. | Net Adapter $\times$ Buffer Opcodes; Net Adapter $\times$ Linear Consumption. | Macro Workload 1: HTTP Web Server. | Verification of received HTTP response payload and headers |
| **37** | **Ui Adapter (VDOM Schema)** | M3 | Validates VDOM `Map` structure: `"tag"` (`Str`), `"props"` (`Map[Str, Str]`), `"text"` (`Str`), `"children"` (`Vec[Map]`); recursive tree structure; serializes VDOM. | Missing optional `"text"` or `"props"`; deeply nested VDOM tree ($\ge 10$ levels); VDOM node with 50 children; invalid tag type; cyclic VDOM references. | VDOM Schema $\times$ Map Opcodes; VDOM Schema $\times$ Vector Opcodes. | Macro Workload 2: Reactive VDOM UI. | VDOM validator unit tests |
| **38** | **Ui Adapter (`ui.mount`)** | M3 | Mounts VDOM tree; registers DOM listeners; returns event `chan[Map]`; receives click event; updates root reference. | Mount empty VDOM; remount updated VDOM; event channel full; dispatch rapid events ($\ge 1000$ events); unmount cleanup. | `ui.mount` $\times$ Channel Opcodes; `ui.mount` $\times$ Async Effect. | Macro Workload 2: Reactive VDOM UI. | Simulated event dispatch and channel recv verification |
| **39** | **Ui Adapter Native SSR** | M3 | Serializes VDOM `Map` directly to HTML string; attributes formatted `key="value"`; child recursion; self-closing tags (`<img>`, `<input>`); HTML entity escaping. | Node with no children; node with multiple text siblings; malicious string in props (XSS injection test `"<script>"`); empty attribute value; deeply nested tree SSR. | Native SSR $\times$ VDOM Schema; Native SSR $\times$ String Buffer. | Server-side rendering in HTTP server fixture. | Exact HTML string comparison against expected DOM markup |
| **40** | **Hw Adapter 64-Byte Alignment** | M3 | Allocates buffer via `Layout::from_size_align(size, 64)`; verifies pointer `addr % 64 == 0`; deallocates via matching layout; prevents memory leaks; validates SIMD readiness. | Allocation of size 1 byte (aligned to 64); allocation of size 63 bytes; allocation of 4096 bytes; allocation of 0 bytes; multiple consecutive aligned allocations. | 64-Byte Alignment $\times$ SIMD XOR; 64-Byte Alignment $\times$ Buffer Alloc. | Macro Workload 3: MMIO Packet Processing. | Pointer address modulo assertions (`ptr as usize % 64 == 0`) |
| **41** | **Hw Adapter (`hw.mmio_map`)** | M3 | Maps simulated memory or mmap into aligned `buf`; read-only and read-write mappings; length validation; zero-copy view. | Mapping address 0; mapping past end of simulated memory; length not a multiple of 64; unaligned base address mapping; mapping read-only memory for write. | MMIO Map $\times$ Buffer Opcodes; MMIO Map $\times$ Hw Effect. | Macro Workload 3: MMIO Packet Processing. | Memory read/write integrity against mmap backing store |
| **42** | **Hw Adapter (`hw.simd_xor`)** | M3 | Computes vector XOR between two 64-byte aligned buffers; AVX-512 / NEON acceleration; output buffer byte-for-byte exact; in-place / zero-copy operation. | Buffers of size 64; buffers of size 1024; unaligned buffer rejection; mismatched buffer lengths; XOR buffer with itself (produces all zeros). | SIMD XOR $\times$ 64-Byte Alignment; SIMD XOR $\times$ Buffer Opcodes. | Macro Workload 3: MMIO Packet Processing. | Bitwise XOR verification against reference software loop |
| **43** | **C-ABI Runtime Bindings** | M3 | `extern "C"` exports: `rt_map_new`, `rt_map_set`, `rt_map_get`, `rt_vec_new`, `rt_vec_push`, `rt_buf_alloc_aligned`, `rt_buf_free`; ABI stability; memory safety. | Null pointer passed to C-ABI function; double-free across C-ABI; calling `rt_map_set` with invalid key pointer; large payload allocation; thread safety of runtime globals. | C-ABI Bindings $\times$ Cranelift In-Place Calls; C-ABI Bindings $\times$ Linear Lowering. | Native Cranelift binary execution of all workloads. | Foreign function call unit tests in Rust |
| **44** | **Reference VM Oracle** | M4 | Deterministic execution semantics for all 28 SSA opcodes; program counter advancement; basic block sequencing; environment value binding; terminal value extraction. | Infinite loop execution (halts via gas); division by zero handling; branch on computed condition; sequential function calls; complex arithmetic expressions. | VM Oracle $\times$ Value Representation; VM Oracle $\times$ Gas Metering. | Ground truth semantic reference for Differential Fuzzing. | Direct execution comparison against derived outputs |
| **45** | **Value Representation** | M4 | `Value` enum: `I64`, `F64`, `Bool`, `Str`, `Buf`, `Vec`, `Map` (BTreeMap), `Chan`; equality testing; cloning; conversion to JSON; type discriminant checks. | Deeply nested `Value::Vec(vec![Value::Map(...)])`; `Value::F64(f64::NAN)` equality rules; string with non-UTF8 binary data; zero-length containers; value print formatting. | Value Enum $\times$ VM Context; Value Enum $\times$ Execution Tracing. | Data representation in all VM execution tests. | Enum variant pattern matching & Serde tests |
| **46** | **Gas Metering (`max_steps`)** | M4 | Default gas `1_000_000`; decrements by 1 per instruction; terminates cleanly with `VmError::GasExhausted` at 0; custom gas limit; steps counter validation. | Loop designed to run 10 iterations with gas 5 (terminates at 5); loop with exact gas needed; gas limit `0`; gas limit `u64::MAX`; gas decrement across branches. | Gas Metering $\times$ Deterministic Control Flow; Gas Metering $\times$ CLI Run (`--gas`). | Infinite loop protection across all test suites. | `assert_matches!(res, Err(VmError::GasExhausted))` |
| **47** | **Execution Tracing (`--trace`)** | M4 | Records `(step, line, block, opcode, dst, val)` for each instruction; exports structured JSON array; captures intermediate states; preserves order. | Trace disabled (zero overhead); trace with 10,000 steps; trace on function with branching; JSON serialization of trace records; formatting float values in trace. | Execution Tracing $\times$ VM Oracle; Execution Tracing $\times$ CLI Run. | Debugging inspection of test fixtures. | JSON array validation of emitted trace records |
| **48** | **Deterministic Control Flow** | M4 | PC increments sequentially; `Terminator::Br` jumps to target block; `Terminator::BrIf` evaluates boolean and jumps to `then` or `else`; `Terminator::Ret` returns value. | Multiple branch conditions in sequence; loop with counter; diamond CFG control flow; backward branches; unreachable code handling. | Control Flow $\times$ BasicBlock; Control Flow $\times$ Dominance Pass. | Control logic across all 5 macro workloads. | Path coverage tests with state validation |
| **49** | **Cranelift Scalar Lowering** | M5 | Lowers `Type::I64` to `cranelift::ir::types::I64`; `Type::Bool` to `types::I64`; `Type::F64` to `types::F64`; maintains bit-patterns; ABI compliant return. | Lowering maximum `i64`; lowering `-1i64`; lowering float `0.0` vs `-0.0`; boolean values 0 and 1; conversion between scalar types. | Scalar Lowering $\times$ Pure Instruction Lowering; Scalar Lowering $\times$ JIT Module. | Macro Workload 4: Matrix Math & Arithmetic. | Cranelift JIT execution return value assertions |
| **50** | **Cranelift Linear Lowering** | M5 | Lowers linear types (`buf`, `str`, `vec`, `map`, `chan`) to opaque machine pointers (`types::I64`); pointer passing across ABI; preserves reference integrity. | Passing null pointer; casting pointer back to Rust struct; multiple linear arguments to function; passing linear type through phi node; returning linear pointer. | Linear Lowering $\times$ C-ABI Bindings; Linear Lowering $\times$ In-Place Calls. | Macro Workloads 1, 2, 3 native compilation. | Native pointer validity & memory inspection |
| **51** | **Cranelift Pure Instruction Lowering** | M5 | Lowers `add`, `sub`, `mul`, `div` to `iadd`, `isub`, `imul`, `sdiv` / `fadd`, `fsub`, `fmul`, `fdiv`; lowers `cmp_eq`, `cmp_lt` to `icmp`, `fcmp`. | Division by zero trap; signed integer overflow wrapping; floating point comparisons with `NaN`; chained arithmetic operations; comparison truth tables. | Pure Lowering $\times$ Scalar Lowering; Pure Lowering $\times$ VM Oracle Parity. | Pure arithmetic loops in packet processing. | JIT output equivalence against reference VM |
| **52** | **Cranelift In-Place Runtime Calls** | M5 | Lowers `map_set`, `vec_push`, `buf_write_u8` to direct runtime C-ABI calls; passes mutable pointer; returns updated pointer; zero clones; no refcounts. | Sequential `map_set` calls mutating same map in-place; rapid `vec_push` causing buffer growth; writing buffer in loop; verify memory address unchanged when capacity allows. | In-Place Calls $\times$ Linear Ownership; In-Place Calls $\times$ C-ABI Bindings. | High-performance VDOM mutations and buffer writes. | Memory allocation profiling & address stability assertions |
| **53** | **Cranelift JIT Module** | M5 | In-memory compilation via `cranelift-jit`; obtains callable function pointer; invokes with arguments; captures return value; executes in $< 5\text{ ms}$. | JIT compilation of empty function; function with 50 instructions; concurrent JIT executions; JIT function calling runtime helper; recompilation after modification. | JIT Module $\times$ Differential Fuzzing Runner; JIT Module $\times$ VM Oracle. | Differential testing runner execution. | Invoking compiled function pointer and checking output |
| **54** | **Cranelift Object Emission** | M5 | Compiles SSA module to native object file (`.o`) via `cranelift-object`; ELF format on Linux; Mach-O on macOS; exports symbol table; relocations. | Object emission for multi-function module; emission with external symbols; emission of static data sections; verifying object file header with `readelf`/`objdump`. | Object Emission $\times$ Native Executable Linker; Object Emission $\times$ CLI Build. | Ahead-Of-Time compilation of macro applications. | `file` and `readelf` inspections on generated `.o` |
| **55** | **Native Executable Linker** | M5 | Invokes system linker (`cc` / `ld`); links `.o` with `knr-runtime` static library; produces standalone executable; execution succeeds; exits 0. | Linking on systems without gcc (uses clang); linking with debug symbols; executable run produces expected stdout; standalone execution without Cargo; exit code propagation. | Native Linker $\times$ Object Emission; Native Linker $\times$ CLI Build (`--target native`). | Macro Workload standalone binaries. | Running emitted binary via `std::process::Command` |
| **56** | **Wasm Module Generation** | M5 | Generates valid `.wasm` binary via `wasm-encoder`; emits `TypeSection`, `ImportSection`, `FunctionSection`, `CodeSection`, `ExportSection`; magic `\0asm`. | Wasm module with 0 functions; module with 10 functions; validation with `wasmparser` or `wasm-tools`; module with memory imports; export name validation. | Wasm Generation $\times$ Wasm Linear Memory; Wasm Generation $\times$ CLI Build (`--target wasm`). | Browser target for Macro Workload 2 (VDOM UI). | `wasmparser::validate` & `wasmtime` execution |
| **57** | **Wasm Linear Memory Model** | M5 | Configures 64 KiB memory pages via `MemorySection`; minimum 1 page; maximum bounds; memory loads/stores; offset calculations. | Memory access at page boundary; out-of-bounds memory trap; memory growth via `memory.grow`; zero-initialized memory check; multiple memory pages. | Linear Memory $\times$ Wasm Runtime Imports; Linear Memory $\times$ Buffer Opcodes. | Low-level buffer storage in WASM runtime. | Wasm runtime memory inspection tests |
| **58** | **Wasm Runtime Imports** | M5 | Emits imports from `"env"` module (`env.rt_map_set`, `env.rt_ui_mount`, `env.rt_chan_recv`); signature matching; function index mapping. | Missing import trap; import signature mismatch; invoking imported function with parameters; return values from host imports; import table ordering. | Runtime Imports $\times$ Ui Adapter; Runtime Imports $\times$ Wasm Module. | Web browser DOM bridge integration. | Execution in `wasmtime` with mock host imports |
| **59** | **Wasm Local Variable Lowering** | M5 | Maps SSA registers `%n` to typed Wasm locals (`i64`, `f64`); emits `local.get`, `local.set`; local variable index allocation; scope validity. | Function with 100 locals; reusing dead local indices; locals for function parameters; type mismatch between local and value; local zero initialization. | Local Lowering $\times$ Wasm Control Flow; Local Lowering $\times$ SSA Registers. | All WebAssembly compiled functions. | Disassembly validation with `wasmprinter` |
| **60** | **Wasm Control Flow Lowering** | M5 | Lowers SSA CFG to structured Wasm control flow (`block`, `loop`, `br`, `br_if`, `br_table`); state machine dispatch loop for arbitrary CFG; join handling. | Irreducible CFG graphs; nested loops; conditional early exit; multi-way branch table; backwards branches with loop header. | Control Flow Lowering $\times$ Terminators; Control Flow Lowering $\times$ BasicBlock. | All WebAssembly control flow execution. | `wasmtime` validation on complex CFG fixtures |
| **61** | **CLI Main Dispatcher** | M6 | Universal `knr` entry point; parses subcommands `check`, `run`, `build`, `asm`, `disasm`; `--help` flag; `--version` flag; argument validation. | Invocation with no arguments (prints help, exit 1); unknown subcommand (exit 1); invalid flag (exit 1); trailing arguments; help formatting compliance. | CLI Dispatcher $\times$ All Subcommands; CLI Dispatcher $\times$ Clap Derivation. | Command interface for all automated test runs. | CLI execution tests verifying exit codes & stderr |
| **62** | **CLI Check Subcommand** | M6 | `knr check <file.ssa>`; exits 0 on valid program; exits 1 on verification failure; `--json` outputs structured JSON diagnostics to stdout; quiet mode. | Check non-existent file; check malformed syntax file; check with invalid flag; verify output formatting with `--json` is valid JSON; multiple files check. | CLI Check $\times$ Structured JSON Diagnostics; CLI Check $\times$ Verifier Passes. | Automated validation of all valid & invalid fixtures. | Process execution asserting exit code and JSON stdout |
| **63** | **CLI Run Subcommand** | M6 | `knr run <file.ssa> [--gas N] [--trace]`; verifies then runs in VM; prints return value; `--gas` limits steps; `--trace` prints execution JSON; exits 0 or 1. | Run with `--gas 10` on long loop (exits with gas exhausted); run with invalid file path; run with `--trace` capturing trace JSON to file or stdout; run program that fails verification (aborts before run). | CLI Run $\times$ Reference VM Oracle; CLI Run $\times$ Gas Metering. | End-to-end execution of all valid fixtures. | Process execution asserting stdout value & trace JSON |
| **64** | **CLI Build Subcommand** | M6 | `knr build <file.ssa> --target <native|wasm> -o <out>`; compiles AOT; creates executable or `.wasm`; respects output path; exits 0 on success. | Build with invalid target name; build without `-o` flag; build to non-writable output directory; build program with verification errors (aborts compilation); verify generated file executable permissions (`chmod +x`). | CLI Build $\times$ Cranelift Backend; CLI Build $\times$ Wasm Backend. | Automated compilation of macro workload binaries. | Process execution asserting output file existence & execution |
| **65** | **CLI Asm Subcommand** | M6 | `knr asm <file.ssa> -o <out.knrbin>`; assembles text to binary; verifies magic header `"KNIR"`; compact 8-byte instructions; exits 0. | Assemble empty file; assemble invalid syntax (fails gracefully); output file already exists (overwrites or error); missing `-o` option; assembling large file. | CLI Asm $\times$ Binary Codec; CLI Asm $\times$ Textual Parser. | Binary compilation step in compiler roundtrip workload. | Process execution asserting `.knrbin` magic bytes |
| **66** | **CLI Disasm Subcommand** | M6 | `knr disasm <file.knrbin> [-o <out.ssa>]`; disassembles binary to canonical text; prints to stdout if no `-o`; matches 1:1 roundtrip. | Disassemble corrupted binary (not starting with `"KNIR"`); disassemble truncated binary; disassemble file with unknown opcode; verify stdout contains canonical syntax. | CLI Disasm $\times$ Canonical Formatter; CLI Disasm $\times$ Binary Codec. | Disassembly step in compiler roundtrip workload. | Process execution asserting disassembled text matches source |
| **67** | **Valid & Invalid Fixtures Suite** | M6 | Maintains comprehensive test fixtures in `tests/fixtures/valid/` ($\ge 15$ files) and `tests/fixtures/invalid/` ($\ge 20$ files); metadata headers; canonical format. | Every opcode covered in valid fixtures; all 20 error categories covered in invalid fixtures; edge case fixtures (empty, max constants); deterministic fixture loader. | Fixtures Suite $\times$ CLI Check; Fixtures Suite $\times$ CLI Run. | Test fixtures feed all 4 test tiers. | Filesystem loader & batch verification runner |
| **68** | **Generative SSA Strategy** | M6 | `proptest` generator for syntactically valid, type-safe, monotonically indexed SSA modules; generates valid arithmetic, containers, and CFGs; bounded loop generation. | Generator with depth parameter 0; generator with max depth ($\ge 20$); generating programs with all 28 opcodes; seed determinism; shrinker reduces failing programs to minimal repro. | Generative Strategy $\times$ Differential Fuzzing; Generative Strategy $\times$ SSA Invariants. | Fuzzing pipeline for Tier 5 adversarial testing. | Property testing running generator and checking validity |
| **69** | **Differential Fuzzing Runner** | M6 | Executes $\ge 1,000$ generated valid programs on `knr-interp` and `knr-codegen` (Cranelift JIT); asserts bitwise identity of return values; zero regressions. | Generator produces loop constructs; generator produces linear container operations; handling gas exhaustion uniformly across both targets; logging divergent test cases; performance $\ge 100\text{ programs/sec}$. | Differential Runner $\times$ VM Oracle; Differential Runner $\times$ Cranelift JIT. | Semantic certification across execution backends. | `cargo test -p knr-codegen --test differential` |
| **70** | **E2E Test Suite Tiers 1-4** | M7 | Full execution of Tier 1 (71 features $\times \ge 5$ cases), Tier 2 (71 features $\times \ge 5$ boundaries), Tier 3 pairwise matrix, and Tier 4 (5 macro workloads); 100% pass rate. | Execution across multiple threads; regression detection; execution reporting (JUnit XML / JSON summary); timeout enforcement; gas limit bounds. | E2E Suite $\times$ All 6 crates; E2E Suite $\times$ All Milestones. | Final gate certification before `TEST_READY.md`. | `cargo test --test e2e` / `tests/e2e/run_tests.sh` |
| **71** | **Tier 5 Adversarial Coverage** | M7 | Adversarial white-box tests closing code coverage and integrity gaps; malformed binary injection; extreme resource stress; boundary mutations; fuzzing. | Corrupted instruction bits; out-of-order pools; deeply recursive types ($\ge 100$ levels); multi-gigabyte buffer allocation attempts; fuzzing verifier parser. | Tier 5 $\times$ Verifier Robustness; Tier 5 $\times$ Codec Resiliency. | Final security and robustness certification. | `cargo test --test adversarial` & memory sanitizers |

---

## 4. Cross-Feature Pairwise Interaction Matrix (Tier 3)

Tier 3 validates the emergent behaviors of features when interacting across module boundaries. The pairwise interaction matrix targets subsystem intersections where state, data flow, or control flow are shared:

```text
Pairwise Interaction Focus Areas:
1. Linear Types (F4)      × Control Flow Branches (F15, F48) -> Ownership merging across paths
2. Linear Mutation (F10)   × SIMD Hardware Ops (F41, F42)     -> 64-byte alignment preservation
3. Containers (F11, F12)   × In-Place Runtime Calls (F43, F52)-> Reallocation & pointer stability
4. Gas Metering (F46)      × Execution Tracing (F47)          -> Step limit halts trace cleanly
5. Constant Pool (F20)     × Multi-Operand Codec (F21, F22)   -> 64-bit word packing & pool index
6. Effect Capabilities (F6)× HostPlatform Bridges (F34-F42)   -> Permission validation on syscalls
7. SSA Monotonicity (F24)  × Cranelift Variable Map (F49, F50)-> Dominance tree phi insertion
8. Wasm Locals (F59)       × Linear Memory Model (F57, F58)   -> Pointer offsets in Wasm memory
```

### 4.1 Concrete Tier 3 Test Scenarios

1. **`test_p01_linear_ownership_across_branches`**:
   - *Features*: F4 (Linear Types), F15 (Terminators `br_if`), F27 (State Machine), F28 (Use-After-Move).
   - *Scenario*: Linear `Map` is mutated along the `then` branch and a different `Map` is mutated along the `else` branch. Verify that state machine correctly tracks ownership in both paths and validates merge at join block.
2. **`test_p02_buffer_allocation_simd_xor_in_place`**:
   - *Features*: F10 (Buffer Opcodes), F40 (64-byte Alignment), F42 (SIMD XOR), F43 (C-ABI Runtime).
   - *Scenario*: Allocate two 1024-byte buffers. Verify 64-byte alignment of addresses. Execute SIMD XOR. Verify result buffer contains bitwise XOR of inputs and remains valid for subsequent in-place writes.
3. **`test_p03_vector_growth_in_place_reallocation`**:
   - *Features*: F11 (Vector Opcodes), F50 (Cranelift Linear Lowering), F52 (In-Place Runtime Calls).
   - *Scenario*: Push 10,000 integers into a dynamic `vec[i64]`. Verify runtime triggers buffer reallocations in-place while keeping ownership linear without leaking prior allocations.
4. **`test_p04_gas_exhaustion_with_tracing`**:
   - *Features*: F46 (Gas Metering), F47 (Execution Tracing), F63 (CLI Run).
   - *Scenario*: Execute infinite loop in VM with `--gas 100` and `--trace`. Verify execution halts at step 100 with `VmError::GasExhausted` and emitted JSON trace contains exactly 100 step records.
5. **`test_p05_binary_codec_deduplicated_pool_3operands`**:
   - *Features*: F20 (Binary Constant Pool), F21 (64-Bit Codec), F22 (Extra Operands Pool), F23 (1:1 Roundtrip).
   - *Scenario*: Program with 50 `map_set` instructions sharing 5 distinct key strings and 5 value strings. Verify constant pool deduplicates to 10 entries and extra operand pool links third operand cleanly without bloat.
6. **`test_p06_effect_capability_enforcement_on_syscall`**:
   - *Features*: F6 (Effect Enum), F31 (Effect Capability Pass), F34 (HostPlatform), F35 (Net Adapter).
   - *Scenario*: Function without declared `Effect::Net` invokes `syscall(Effect::Net, "listen", ...)`. Verify verifier rejects statically with `UndeclaredEffect`. Add `effects(Net)` to signature and verify it accepts and routes to `NetPlatform`.
7. **`test_p07_cranelift_jit_vs_vm_oracle_complex_math`**:
   - *Features*: F8 (Arithmetic Opcodes), F44 (VM Oracle), F51 (Cranelift Pure Lowering), F53 (Cranelift JIT).
   - *Scenario*: Function computing iterative Fibonacci and floating-point polynomial. Run on `knr-interp` and Cranelift JIT. Assert bitwise identical results.
8. **`test_p08_wasm_lowering_linear_memory_roundtrip`**:
   - *Features*: F56 (Wasm Module Generation), F57 (Linear Memory Model), F60 (Wasm Control Flow).
   - *Scenario*: Compile multi-block SSA function with buffer manipulation to `.wasm`. Execute in `wasmtime`. Assert memory writes match expected bytes.

---

## 5. Real-World Application Workloads (Tier 4)

Tier 4 implements $\ge 5$ end-to-end macro application scenarios representing realistic production systems. Each workload runs through the complete toolchain (Parsing $\rightarrow$ Verification $\rightarrow$ VM Interpretation $\rightarrow$ Compilation $\rightarrow$ Execution):

### 5.1 Workload 1: High-Throughput HTTP JSON Microservice
- **Path**: `tests/fixtures/valid/workload_http_service.ssa`
- **Capabilities**: `effects(Net, Async)`
- **Description**: Listens on TCP port via `net.listen`. Receives HTTP `POST /api/v1/compute` with JSON payload in `Buf`. Parses payload, creates response `Map`, serializes to `Buf`, and streams response via `net.respond` with status 200 OK.
- **Validation**: Spin up simulated or real Tokio client, send 100 requests, verify 100% 200 OK responses with exact JSON payload.

### 5.2 Workload 2: Reactive Virtual DOM UI Component with SSR
- **Path**: `tests/fixtures/valid/workload_vdom_ui.ssa`
- **Capabilities**: `effects(Ui, Async)`
- **Description**: Builds a dynamic UI counter component using standard VDOM `Map` schema (`"tag"`, `"props"`, `"text"`, `"children"`). Dispatches click events through channel, mutates state in-place, rerenders VDOM tree, and generates server-side rendered (SSR) HTML string.
- **Validation**: Verify initial SSR HTML matches `<div class="counter"><button id="inc">+</button><span>Count: 0</span></div>`. Dispatch 5 simulated click events, verify updated count is 5.

### 5.3 Workload 3: MMIO Zero-Copy Network Packet Filter & SIMD Checksum
- **Path**: `tests/fixtures/valid/workload_packet_filter.ssa`
- **Capabilities**: `effects(Hw)`
- **Description**: Maps simulated network interface ring buffer into 64-byte aligned memory via `hw.mmio_map`. Inspects packet headers (Ethernet / IPv4 / TCP). Computes vector SIMD XOR checksum over packet payload using `hw.simd_xor`. Drops malformed packets and forwards valid packets.
- **Validation**: Feed 50 synthetic network packets (mix of valid and corrupted). Verify all valid packets pass with correct checksum and corrupted packets are dropped.

### 5.4 Workload 4: Lossless Text Compression Pipeline (LZ77 / Run-Length)
- **Path**: `tests/fixtures/valid/workload_compression.ssa`
- **Capabilities**: Pure function (`effects()`)
- **Description**: Ingests raw UTF-8 `str` into `buf`. Scans buffer for repeated byte sequences, constructs run-length encoded dictionary in `map[str, i64]`, packs compressed tokens into dynamic `vec[i64]`, and unpacks back to verify lossless roundtrip.
- **Validation**: Compress 10 KB sample text, assert compressed size $< 6\text{ KB}$, decompress and assert `decompressed == original`.

### 5.5 Workload 5: Multi-Stage Compiler Roundtrip Pipeline
- **Path**: `tests/fixtures/valid/workload_compiler_pipeline.ssa`
- **Capabilities**: Pure & System
- **Description**: Complete end-to-end roundtrip pipeline: Textual `.ssa` $\rightarrow$ AST `Module` $\rightarrow$ Binary `.knrbin` $\rightarrow$ Decoded AST $\rightarrow$ Canonical `.ssa` $\rightarrow$ Cranelift Object $\rightarrow$ Native Executable $\rightarrow$ Standalone execution.
- **Validation**: Execute pipeline on complex multi-function SSA program. Verify text roundtrip identity and verify compiled standalone binary produces identical exit code and stdout.

---

## 6. Test Runner Architecture & Progressive Execution

### 6.1 Invocation Commands

The test runner provides uniform command interfaces across development and CI/CD environments:

```bash
# 1. Run all E2E tests (all tiers)
cargo test --test e2e

# 2. Run tests by tier
cargo test --test e2e -- tier1
cargo test --test e2e -- tier2
cargo test --test e2e -- tier3
cargo test --test e2e -- tier4

# 3. Run tests by milestone wave
cargo test --test e2e -- wave1    # knr-core, codecs, parser, roundtrip
cargo test --test e2e -- wave2    # verify, runtime, interp, codegen
cargo test --test e2e -- wave3    # cli, differential fuzzing, workloads

# 4. Run standalone test runner script (with JSON & JUnit XML output)
./tests/e2e/run_tests.sh --all
./tests/e2e/run_tests.sh --tier 1
./tests/e2e/run_tests.sh --wave 1
```

### 6.2 Pass/Fail Semantics & Exit Codes

| Exit Code | Semantic Meaning | Description |
|:---:|---|---|
| `0` | **SUCCESS** | All executed test cases passed assertion checks. |
| `1` | **TEST_FAILURE** | At least one test assertion failed (value mismatch, schema error). |
| `2` | **TIMEOUT_OR_GAS** | Test exceeded execution deadline (default 10s) or gas limit. |
| `3` | **INFRA_ERROR** | Environment defect (missing compiler, missing fixture file, permission error). |

### 6.3 Test Harness Directory Structure

```text
tests/
├── Cargo.toml                          # Test suite manifest (or tests/e2e integration)
├── fixtures/
│   ├── valid/                          # Valid .ssa source files
│   │   ├── 01_minimal.ssa
│   │   ├── 02_arithmetic.ssa
│   │   ├── 03_linear_containers.ssa
│   │   ├── 04_branching.ssa
│   │   ├── workload_http_service.ssa
│   │   ├── workload_vdom_ui.ssa
│   │   ├── workload_packet_filter.ssa
│   │   ├── workload_compression.ssa
│   │   └── workload_compiler_pipeline.ssa
│   └── invalid/                        # Rejection test fixtures (20 canonical cases)
│       ├── err01_ssa_non_monotonic.ssa
│       ├── err02_ssa_dominance.ssa
│       ├── err03_ssa_duplicate_reg.ssa
│       ├── err04_use_after_move_map.ssa
│       ├── err05_use_after_move_vec.ssa
│       ├── err06_use_after_move_buf.ssa
│       ├── err07_linear_leak_buf.ssa
│       ├── err08_linear_leak_vec.ssa
│       ├── err09_type_mismatch_arith.ssa
│       ├── err10_type_mismatch_map_key.ssa
│       ├── err11_undeclared_effect_net.ssa
│       ├── err12_undeclared_effect_hw.ssa
│       ├── err13_unterminated_string.ssa
│       ├── err14_invalid_register_name.ssa
│       ├── err15_missing_block_terminator.ssa
│       ├── err16_double_free.ssa
│       ├── err17_free_unallocated.ssa
│       ├── err18_branch_to_missing_block.ssa
│       ├── err19_conditional_branch_non_bool.ssa
│       └── err20_return_type_mismatch.ssa
├── differential/
│   └── proptest_vm_codegen.rs          # 1,000-program differential fuzzer
└── e2e/
    ├── Cargo.toml                      # E2E test harness crate manifest
    ├── run_tests.sh                    # Standalone test runner script
    ├── harness/                        # Reusable test assertions & runners
    │   ├── mod.rs
    │   ├── fixture_loader.rs
    │   ├── process_runner.rs
    │   └── json_assert.rs
    ├── tier1_features/                 # Tier 1 feature coverage tests
    │   ├── mod.rs
    │   ├── test_wave1_core.rs
    │   ├── test_wave2_verify.rs
    │   ├── test_wave2_runtime.rs
    │   ├── test_wave2_interp.rs
    │   ├── test_wave2_codegen.rs
    │   └── test_wave3_cli.rs
    ├── tier2_boundaries/               # Tier 2 boundary & corner tests
    │   ├── mod.rs
    │   ├── test_wave1_boundaries.rs
    │   ├── test_wave2_boundaries.rs
    │   └── test_wave3_boundaries.rs
    ├── tier3_pairwise/                 # Tier 3 cross-feature interaction tests
    │   ├── mod.rs
    │   └── test_pairwise_interactions.rs
    └── tier4_workloads/                # Tier 4 macro workload tests
        ├── mod.rs
        ├── test_workload_http.rs
        ├── test_workload_vdom.rs
        ├── test_workload_packet.rs
        ├── test_workload_compression.rs
        └── test_workload_pipeline.rs
```

---

## 7. Initial Wave 1 Test Suite (Core, Grammar & Binary Codec)

For Milestone 1 (Wave 1: `knr-core`), the following initial test suites are implemented in `tests/e2e/tier1_features/test_wave1_core.rs` and `tests/e2e/tier2_boundaries/test_wave1_boundaries.rs`:

### 7.1 Wave 1 Tier 1 Tests (Feature Coverage $\ge 5$ per feature)

1. **`test_t1_f01_workspace_dependencies`**: Verifies workspace root and member dependencies load without error.
2. **`test_t1_f03_scalar_types`**: Validates creation, copy semantics, and string representation of `i64`, `f64`, `bool`, `void`.
3. **`test_t1_f04_linear_types`**: Validates creation, move tagging, and parametric nesting of `buf`, `str`, `vec`, `map`, `chan`.
4. **`test_t1_f05_linear_predicate`**: Evaluates `is_linear()` on all 9 type variants and verifies exact boolean results.
5. **`test_t1_f06_effect_enum`**: Checks all 4 effect variants (`Net`, `Ui`, `Hw`, `Async`) and empty pure effect list.
6. **`test_t1_f07_constant_opcodes`**: Verifies `const_i64`, `const_f64`, `const_bool`, `const_str` AST creation and attribute access.
7. **`test_t1_f08_arithmetic_opcodes`**: Verifies `add`, `sub`, `mul`, `div` opcode representations for `i64` and `f64`.
8. **`test_t1_f09_comparison_opcodes`**: Verifies `cmp_eq` and `cmp_lt` AST representations.
9. **`test_t1_f10_buffer_opcodes`**: Verifies `buf_alloc`, `buf_read_u8`, `buf_write_u8`, `buf_slice` AST construction.
10. **`test_t1_f11_vector_opcodes`**: Verifies `vec_new`, `vec_push`, `vec_get`, `vec_len` AST construction.
11. **`test_t1_f12_map_opcodes`**: Verifies `map_new`, `map_set`, `map_get`, `map_del` AST construction.
12. **`test_t1_f13_channel_opcodes`**: Verifies `chan_new`, `chan_send`, `chan_recv` AST construction.
13. **`test_t1_f14_task_system_opcodes`**: Verifies `spawn`, `syscall`, `free` AST construction.
14. **`test_t1_f15_terminators`**: Verifies `ret`, `br`, `br_if` block terminator representation.
15. **`test_t1_f16_ast_module_hierarchy`**: Verifies `Module` $\rightarrow$ `Function` $\rightarrow$ `BasicBlock` $\rightarrow$ `Instruction` traversal.
16. **`test_t1_f17_textual_parser_valid_ssa`**: Parses representative valid SSA source files and verifies AST output.
17. **`test_t1_f18_canonical_formatter`**: Verifies canonical 4-space indentation and formatted textual output.
18. **`test_t1_f19_binary_header_magic_and_version`**: Verifies encoding and decoding of `"KNIR"` (`0x4B 0x4E 0x49 0x52`) and version `1`.
19. **`test_t1_f20_constant_pool_deduplication`**: Verifies string pool deduplicates identical literals.
20. **`test_t1_f21_fixed_64bit_instruction_codec`**: Verifies 8-byte packing and unpacking of single- and double-operand opcodes.
21. **`test_t1_f22_extra_operands_pool`**: Verifies packing and unpacking of 3-operand opcodes (`buf_write_u8`, `map_set`, `br_if`).
22. **`test_t1_f23_exact_roundtrip_bijection`**: Evaluates `format(decode(encode(parse(text)))) == text` on canonical 15-line program.

### 7.2 Wave 1 Tier 2 Boundary Tests ($\ge 5$ per feature)

1. **`test_t2_f03_scalar_extremes`**: Tests `i64::MIN`, `i64::MAX`, `f64::NAN`, `-0.0`, `f64::INFINITY`.
2. **`test_t2_f04_empty_containers`**: Tests zero-sized container types and 16-level nested container types.
3. **`test_t2_f07_empty_and_unicode_strings`**: Tests empty string `""`, multibyte emojis (`"🦀🚀"`), and control characters.
4. **`test_t2_f16_empty_basic_block`**: Tests basic block with 0 instructions followed immediately by `ret`.
5. **`test_t2_f17_parser_syntax_errors`**: Tests graceful error returns on unterminated strings, invalid registers (`%foo`), and bad opcodes.
6. **`test_t2_f19_corrupted_magic_header`**: Rejects bytes with bad magic (`"NOPE"`), truncated header ($< 6$ bytes), and unsupported version (`version = 2`).
7. **`test_t2_f20_large_constant_pool`**: Tests constant pool with 1,000 distinct strings and float bit-pattern fidelity.
8. **`test_t2_f21_max_register_index`**: Tests packing instructions with register ID `65534` and `NO_REG` (`0xFFFF`).
9. **`test_t2_f22_extra_pool_sequential_3ops`**: Tests multiple sequential 3-operand instructions referencing different extra pool slots.
10. **`test_t2_f23_whitespace_and_comment_invariance`**: Tests that parser/formatter/codec roundtrip normalizes tabs, spaces, and comments canonically.

---

## 8. Progressive Verification Gates

| Gate | Wave Trigger | Required Passing Targets | Artifact Published |
|:---:|---|---|:---:|
| **Gate 1** | Wave 1 Complete | Tier 1 (F1–F23) + Tier 2 (F1–F23 boundaries) + Roundtrip bijection | `TEST_WAVE1_PASSED.md` |
| **Gate 2** | Wave 2 Complete | Tier 1 (F24–F60) + Tier 2 (F24–F60 boundaries) + 20 Verifier tests | `TEST_WAVE2_PASSED.md` |
| **Gate 3** | Wave 3 Complete | Tier 1 (F61–F69) + CLI tests + Differential Fuzzing ($\ge 1,000$ runs) | `TEST_WAVE3_PASSED.md` |
| **Gate 4** | Final Integration | Tier 3 Pairwise Matrix + Tier 4 Workloads (1–5) + Tier 5 Adversarial | `TEST_READY.md` |
