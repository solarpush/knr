use crate::CodegenError;
use knr_core::{BasicBlock, BlockId, Function, Instruction as KnrInst, Module, Op, RegId, Schema, Terminator, Type};
use std::collections::{BTreeMap, HashMap};
use wasm_encoder::{
    BlockType, CodeSection, ConstExpr, DataSection, EntityType, ExportKind, ExportSection,
    Function as WasmFunction, FunctionSection, GlobalSection, GlobalType, ImportSection,
    Instruction, MemArg, MemorySection, MemoryType, Module as WasmModule, TypeSection, ValType,
};

type ImportSpec = (&'static str, Vec<ValType>, Vec<ValType>);

/// WebAssembly code generator lowering SSA `Module` / `Function` to `.wasm` binary bytes.
/// Supports block parameters, structs, enums, vectors, strings, and inter-function calls
/// using an embedded linear-memory allocator so pure apps run with ZERO external JS imports.
#[derive(Debug, Default, Clone)]
pub struct WasmBackend;

#[derive(Default)]
struct BuiltinIndices {
    heap_mark: u32,
    heap_reset: u32,
    heap_used: u32,
    heap_capacity: u32,
    alloc: Option<u32>,
    str_len: Option<u32>,
    str_eq: Option<u32>,
    str_concat: Option<u32>,
    int_to_str: Option<u32>,
    f64_to_str: Option<u32>,
    vec_new: Option<u32>,
    vec_push: Option<u32>,
    vec_get: Option<u32>,
    vec_len: Option<u32>,
    map_new: Option<u32>,
    map_set: Option<u32>,
    map_get: Option<u32>,
    map_del: Option<u32>,
    buf_alloc: Option<u32>,
    math_sin: Option<u32>,
    math_cos: Option<u32>,
    math_tan: Option<u32>,
    math_pow: Option<u32>,
    math_atan2: Option<u32>,
}

impl WasmBackend {
    pub fn new() -> Self {
        Self
    }

    /// Compiles a single SSA `Function` to a self-contained WebAssembly binary.
    pub fn compile_function(&self, func: &Function) -> Result<Vec<u8>, CodegenError> {
        let mut module = Module::new();
        module.add_function(func.clone());
        self.compile(&module)
    }

    /// Compiles an entire SSA `Module` to a valid WebAssembly binary module.
    pub fn compile(&self, module: &Module) -> Result<Vec<u8>, CodegenError> {
        let mut wasm = WasmModule::new();
        let mut types = TypeSection::new();
        let mut type_map: HashMap<String, u32> = HashMap::new();

        let mut register_type = |params: &[ValType], results: &[ValType]| -> u32 {
            let key = format!("{params:?}->{results:?}");
            if let Some(&idx) = type_map.get(&key) {
                idx
            } else {
                let idx = type_map.len() as u32;
                types.function(params.iter().copied(), results.iter().copied());
                type_map.insert(key, idx);
                idx
            }
        };

        // Constant pool for string literals in linear memory (offset starts at 1024)
        let mut string_pool = StringPool::new(1024);
        let mut needs_bool_strings = false;
        for func in &module.functions {
            let mut reg_types: HashMap<RegId, Type> = HashMap::new();
            for (r, t) in &func.params {
                reg_types.insert(*r, t.clone());
            }
            for block in &func.blocks {
                for (r, t) in &block.params {
                    reg_types.insert(*r, t.clone());
                }
                for inst in &block.instructions {
                    if let Some(d) = inst.dst {
                        reg_types.insert(d, inst.ty.clone());
                    }
                    match &inst.op {
                        Op::ConstStr(s) => {
                            string_pool.intern(s);
                        }
                        Op::IntToStr(a) if reg_types.get(a) == Some(&Type::Bool) => {
                            needs_bool_strings = true;
                        }
                        Op::MapSet(_, k, _) | Op::MapGet(_, k) | Op::MapDel(_, k)
                            if reg_types.get(k) == Some(&Type::Bool) =>
                        {
                            needs_bool_strings = true;
                        }
                        _ => {}
                    }
                }
            }
        }
        if needs_bool_strings {
            string_pool.intern("true");
            string_pool.intern("false");
        }

        // Compute initial bump-allocator heap base safely past the constant string pool
        let heap_base: u32 = ((1024u32.saturating_add(string_pool.data.len() as u32) + 7) & !7).max(65536);

        // Determine if external host imports from "env" are needed (channels / spawn / syscalls)
        let (needs_imports, import_specs) = Self::collect_imports(module);
        let mut import_indices: HashMap<&'static str, u32> = HashMap::new();

        let mut imports = ImportSection::new();
        if needs_imports {
            for (idx, (name, params, results)) in import_specs.iter().enumerate() {
                let type_idx = register_type(params, results);
                imports.import("env", name, EntityType::Function(type_idx));
                import_indices.insert(*name, idx as u32);
            }
        }

        let num_imports = if needs_imports {
            import_specs.len() as u32
        } else {
            0
        };

        // Determine if in-Wasm heap/string/vec/map/buf/math builtins are needed
        let needs_builtins = Self::needs_in_wasm_runtime(module);

        // Register signatures for all user functions first, then arena telemetry, then internal builtins
        let mut func_type_indices = Vec::new();
        let mut func_indices: HashMap<String, u32> = HashMap::with_capacity(module.functions.len());
        for (i, func) in module.functions.iter().enumerate() {
            func_indices.insert(func.name.clone(), num_imports + i as u32);
        }

        for func in &module.functions {
            let params: Vec<ValType> = func
                .params
                .iter()
                .filter_map(|(_, ty)| Self::type_to_wasm(ty))
                .collect();
            let results: Vec<ValType> = match Self::type_to_wasm(&func.ret_ty) {
                Some(vt) => vec![vt],
                None => vec![],
            };
            let type_idx = register_type(&params, &results);
            func_type_indices.push(type_idx);
        }

        // Always emit zero-cost arena telemetry & mark/reset helpers right after user functions
        let arena_base = num_imports + module.functions.len() as u32;
        let mut builtins = BuiltinIndices {
            heap_mark: arena_base,
            heap_reset: arena_base + 1,
            heap_used: arena_base + 2,
            heap_capacity: arena_base + 3,
            ..Default::default()
        };
        func_type_indices.push(register_type(&[], &[ValType::I32])); // __knr_heap_mark() -> i32
        func_type_indices.push(register_type(&[ValType::I32], &[])); // __knr_heap_reset(mark: i32)
        func_type_indices.push(register_type(&[], &[ValType::I32])); // __knr_heap_used() -> i32
        func_type_indices.push(register_type(&[], &[ValType::I32])); // __knr_heap_capacity() -> i32

        // Register internal runtime builtins if needed
        if needs_builtins {
            let base = arena_base + 4;
            // 0: __knr_alloc(size: i32) -> i64
            builtins.alloc = Some(base);
            func_type_indices.push(register_type(&[ValType::I32], &[ValType::I64]));
            // 1: __knr_str_len(ptr: i64) -> i64
            builtins.str_len = Some(base + 1);
            func_type_indices.push(register_type(&[ValType::I64], &[ValType::I64]));
            // 2: __knr_str_eq(a: i64, b: i64) -> i32
            builtins.str_eq = Some(base + 2);
            func_type_indices.push(register_type(&[ValType::I64, ValType::I64], &[ValType::I32]));
            // 3: __knr_str_concat(a: i64, b: i64) -> i64
            builtins.str_concat = Some(base + 3);
            func_type_indices.push(register_type(&[ValType::I64, ValType::I64], &[ValType::I64]));
            // 4: __knr_int_to_str(n: i64) -> i64
            builtins.int_to_str = Some(base + 4);
            func_type_indices.push(register_type(&[ValType::I64], &[ValType::I64]));
            // 5: __knr_vec_new() -> i64
            builtins.vec_new = Some(base + 5);
            func_type_indices.push(register_type(&[], &[ValType::I64]));
            // 6: __knr_vec_push(vec: i64, val: i64) -> i64
            builtins.vec_push = Some(base + 6);
            func_type_indices.push(register_type(&[ValType::I64, ValType::I64], &[ValType::I64]));
            // 7: __knr_vec_get(vec: i64, idx: i64) -> i64
            builtins.vec_get = Some(base + 7);
            func_type_indices.push(register_type(&[ValType::I64, ValType::I64], &[ValType::I64]));
            // 8: __knr_vec_len(vec: i64) -> i64
            builtins.vec_len = Some(base + 8);
            func_type_indices.push(register_type(&[ValType::I64], &[ValType::I64]));
            // 9: __knr_sin(x: f64) -> f64
            builtins.math_sin = Some(base + 9);
            func_type_indices.push(register_type(&[ValType::F64], &[ValType::F64]));
            // 10: __knr_cos(x: f64) -> f64
            builtins.math_cos = Some(base + 10);
            func_type_indices.push(register_type(&[ValType::F64], &[ValType::F64]));
            // 11: __knr_tan(x: f64) -> f64
            builtins.math_tan = Some(base + 11);
            func_type_indices.push(register_type(&[ValType::F64], &[ValType::F64]));
            // 12: __knr_pow(a: f64, b: f64) -> f64
            builtins.math_pow = Some(base + 12);
            func_type_indices.push(register_type(&[ValType::F64, ValType::F64], &[ValType::F64]));
            // 13: __knr_atan2(y: f64, x: f64) -> f64
            builtins.math_atan2 = Some(base + 13);
            func_type_indices.push(register_type(&[ValType::F64, ValType::F64], &[ValType::F64]));
            // 14: __knr_map_new() -> i64
            builtins.map_new = Some(base + 14);
            func_type_indices.push(register_type(&[], &[ValType::I64]));
            // 15: __knr_map_set(map: i64, key: i64, val: i64) -> i64
            builtins.map_set = Some(base + 15);
            func_type_indices.push(register_type(
                &[ValType::I64, ValType::I64, ValType::I64],
                &[ValType::I64],
            ));
            // 16: __knr_map_get(map: i64, key: i64) -> i64
            builtins.map_get = Some(base + 16);
            func_type_indices.push(register_type(&[ValType::I64, ValType::I64], &[ValType::I64]));
            // 17: __knr_map_del(map: i64, key: i64) -> i64
            builtins.map_del = Some(base + 17);
            func_type_indices.push(register_type(&[ValType::I64, ValType::I64], &[ValType::I64]));
            // 18: __knr_buf_alloc(size: i64) -> i64
            builtins.buf_alloc = Some(base + 18);
            func_type_indices.push(register_type(&[ValType::I64], &[ValType::I64]));
            // 19: __knr_f64_to_str(x: f64) -> i64
            builtins.f64_to_str = Some(base + 19);
            func_type_indices.push(register_type(&[ValType::F64], &[ValType::I64]));
        }

        // 1. Type Section
        wasm.section(&types);

        // 2. Import Section (if any)
        if needs_imports {
            wasm.section(&imports);
        }

        // 3. Function Section
        let mut functions = FunctionSection::new();
        for &t_idx in &func_type_indices {
            functions.function(t_idx);
        }
        wasm.section(&functions);

        // 5. Memory Section: at least 4 pages (256 KiB) if heap used, 1 page otherwise (covering heap_base)
        let pages_for_base = u64::from(heap_base.div_ceil(65536).max(1));
        let min_pages = if needs_builtins {
            pages_for_base.max(4)
        } else {
            pages_for_base
        };
        let mut memories = MemorySection::new();
        memories.memory(MemoryType {
            minimum: min_pages,
            maximum: None,
            memory64: false,
            shared: false,
            page_size_log2: None,
        });
        wasm.section(&memories);

        // 6. Global Section (bump allocator pointer initialized to heap_base)
        let mut globals = GlobalSection::new();
        globals.global(
            GlobalType {
                val_type: ValType::I32,
                mutable: true,
                shared: false,
            },
            &ConstExpr::i32_const(heap_base as i32),
        );
        wasm.section(&globals);

        // 7. Export Section
        let mut exports = ExportSection::new();
        exports.export("memory", ExportKind::Memory, 0);
        for (i, func) in module.functions.iter().enumerate() {
            let func_idx = num_imports + i as u32;
            exports.export(&func.name, ExportKind::Func, func_idx);
        }
        exports.export("__knr_heap", ExportKind::Global, 0);
        exports.export("__knr_heap_mark", ExportKind::Func, builtins.heap_mark);
        exports.export("__knr_heap_reset", ExportKind::Func, builtins.heap_reset);
        exports.export("__knr_heap_used", ExportKind::Func, builtins.heap_used);
        exports.export("__knr_heap_capacity", ExportKind::Func, builtins.heap_capacity);
        if let Some(alloc_idx) = builtins.alloc {
            exports.export("__knr_alloc", ExportKind::Func, alloc_idx);
        }
        wasm.section(&exports);

        // 10. Code Section
        let mut code = CodeSection::new();
        for func in &module.functions {
            let wasm_fn = Self::compile_func_body(
                func,
                &module.schemas,
                &import_indices,
                &func_indices,
                &builtins,
                &string_pool,
            )?;
            code.function(&wasm_fn);
        }
        // Always emit zero-cost arena telemetry & mark/reset helpers
        code.function(&Self::emit_builtin_heap_mark());
        code.function(&Self::emit_builtin_heap_reset());
        code.function(&Self::emit_builtin_heap_used(heap_base as i32));
        code.function(&Self::emit_builtin_heap_capacity());
        if needs_builtins {
            let alloc_idx = builtins.alloc.unwrap();
            let str_eq_idx = builtins.str_eq.unwrap();
            let int_to_str_idx = builtins.int_to_str.unwrap();
            code.function(&Self::emit_builtin_alloc());
            code.function(&Self::emit_builtin_str_len());
            code.function(&Self::emit_builtin_str_eq());
            code.function(&Self::emit_builtin_str_concat(alloc_idx));
            code.function(&Self::emit_builtin_int_to_str(alloc_idx));
            code.function(&Self::emit_builtin_vec_new(alloc_idx));
            code.function(&Self::emit_builtin_vec_push(alloc_idx));
            code.function(&Self::emit_builtin_vec_get());
            code.function(&Self::emit_builtin_vec_len());
            code.function(&Self::emit_builtin_sin());
            code.function(&Self::emit_builtin_cos(builtins.math_sin.unwrap()));
            code.function(&Self::emit_builtin_tan(
                builtins.math_sin.unwrap(),
                builtins.math_cos.unwrap(),
            ));
            code.function(&Self::emit_builtin_pow());
            code.function(&Self::emit_builtin_atan2());
            code.function(&Self::emit_builtin_map_new(alloc_idx));
            code.function(&Self::emit_builtin_map_set(alloc_idx, str_eq_idx));
            code.function(&Self::emit_builtin_map_get(str_eq_idx));
            code.function(&Self::emit_builtin_map_del(str_eq_idx));
            code.function(&Self::emit_builtin_buf_alloc(alloc_idx));
            code.function(&Self::emit_builtin_f64_to_str(alloc_idx, int_to_str_idx));
        }
        wasm.section(&code);

        // 11. Data Section (if string pool has data)
        if !string_pool.data.is_empty() {
            let mut data = DataSection::new();
            data.active(
                0,
                &ConstExpr::i32_const(string_pool.base_offset as i32),
                string_pool.data.iter().copied(),
            );
            wasm.section(&data);
        }

        Ok(wasm.finish())
    }

    fn type_to_wasm(ty: &Type) -> Option<ValType> {
        match ty {
            Type::I64 => Some(ValType::I64),
            Type::F64 => Some(ValType::F64),
            Type::Bool => Some(ValType::I32),
            Type::Void => None,
            Type::Str | Type::Buf | Type::Vec(_) | Type::Map(_, _) | Type::Chan(_) | Type::Struct(_) => {
                Some(ValType::I64)
            }
        }
    }

    fn needs_in_wasm_runtime(module: &Module) -> bool {
        for func in &module.functions {
            let mut reg_types: HashMap<RegId, Type> = HashMap::new();
            for (r, t) in &func.params {
                reg_types.insert(*r, t.clone());
            }
            for block in &func.blocks {
                for (r, t) in &block.params {
                    reg_types.insert(*r, t.clone());
                }
                for inst in &block.instructions {
                    if let Some(d) = inst.dst {
                        reg_types.insert(d, inst.ty.clone());
                    }
                    match &inst.op {
                        Op::StructNew(_)
                        | Op::StructGet(_, _)
                        | Op::StructSet(_, _, _)
                        | Op::IntToStr(_)
                        | Op::StrLen(_)
                        | Op::VecNew(_)
                        | Op::VecPush(_, _)
                        | Op::VecGet(_, _)
                        | Op::VecLen(_)
                        | Op::MapNew(_, _)
                        | Op::MapSet(_, _, _)
                        | Op::MapGet(_, _)
                        | Op::MapDel(_, _)
                        | Op::BufAlloc(_)
                        | Op::Sin(_)
                        | Op::Cos(_)
                        | Op::Tan(_)
                        | Op::Pow(_, _)
                        | Op::Atan2(_, _) => return true,
                        Op::Add(_, _) if inst.ty == Type::Str => return true,
                        Op::CmpEq(a, _)
                            if matches!(reg_types.get(a), Some(Type::Str | Type::Struct(_))) =>
                        {
                            return true
                        }
                        _ => {}
                    }
                }
            }
        }
        false
    }

    fn collect_imports(module: &Module) -> (bool, Vec<ImportSpec>) {
        let mut needs = false;
        for func in &module.functions {
            for block in &func.blocks {
                for inst in &block.instructions {
                    if matches!(
                        &inst.op,
                        Op::ChanNew(_, _)
                            | Op::ChanSend(_, _)
                            | Op::ChanRecv(_)
                            | Op::Spawn(_, _)
                            | Op::Syscall(_, _, _)
                    ) {
                        needs = true;
                        break;
                    }
                }
                if needs {
                    break;
                }
            }
            if needs {
                break;
            }
        }

        if !needs {
            return (false, Vec::new());
        }

        let specs = vec![
            ("rt_map_new", vec![], vec![ValType::I64]),
            (
                "rt_map_set",
                vec![ValType::I64, ValType::I64, ValType::I64, ValType::I64],
                vec![ValType::I64],
            ),
            (
                "rt_map_get",
                vec![ValType::I64, ValType::I64, ValType::I64],
                vec![ValType::I64],
            ),
            ("rt_map_free", vec![ValType::I64], vec![]),
            ("rt_vec_new", vec![], vec![ValType::I64]),
            (
                "rt_vec_push",
                vec![ValType::I64, ValType::I64],
                vec![ValType::I64],
            ),
            (
                "rt_vec_get",
                vec![ValType::I64, ValType::I64],
                vec![ValType::I64],
            ),
            ("rt_vec_len", vec![ValType::I64], vec![ValType::I64]),
            ("rt_vec_free", vec![ValType::I64], vec![]),
            (
                "rt_buf_alloc_aligned",
                vec![ValType::I64, ValType::I64],
                vec![ValType::I64],
            ),
            (
                "rt_buf_free",
                vec![ValType::I64, ValType::I64, ValType::I64],
                vec![],
            ),
        ];

        (true, specs)
    }

    // ========================================================================
    // Zero-Cost Arena Telemetry & Management Exports
    // ========================================================================

    /// `__knr_heap_mark() -> i32`: returns current bump allocator offset.
    fn emit_builtin_heap_mark() -> WasmFunction {
        let mut f = WasmFunction::new([]);
        f.instruction(&Instruction::GlobalGet(0));
        f.instruction(&Instruction::End);
        f
    }

    /// `__knr_heap_reset(mark: i32)`: restores bump allocator offset to `mark`.
    fn emit_builtin_heap_reset() -> WasmFunction {
        let mut f = WasmFunction::new([]);
        f.instruction(&Instruction::LocalGet(0));
        f.instruction(&Instruction::GlobalSet(0));
        f.instruction(&Instruction::End);
        f
    }

    /// `__knr_heap_used() -> i32`: returns exact bytes currently allocated (`__knr_heap - heap_base`).
    fn emit_builtin_heap_used(heap_base: i32) -> WasmFunction {
        let mut f = WasmFunction::new([]);
        f.instruction(&Instruction::GlobalGet(0));
        f.instruction(&Instruction::I32Const(heap_base));
        f.instruction(&Instruction::I32Sub);
        f.instruction(&Instruction::End);
        f
    }

    /// `__knr_heap_capacity() -> i32`: returns current linear memory size in bytes (`memory.size * 65536`).
    fn emit_builtin_heap_capacity() -> WasmFunction {
        let mut f = WasmFunction::new([]);
        f.instruction(&Instruction::MemorySize(0));
        f.instruction(&Instruction::I32Const(16));
        f.instruction(&Instruction::I32Shl);
        f.instruction(&Instruction::End);
        f
    }

    // ========================================================================
    // Embedded Pure-WASM Runtime Helpers (Zero JS Imports!)
    // ========================================================================

    /// `__knr_alloc(size: i32) -> i64` : 8-byte aligned, zero-initialized bump allocator over linear memory.
    fn emit_builtin_alloc() -> WasmFunction {
        // param 0: size (i32)
        // locals: 1=aligned_size(i32), 2=ptr(i32), 3=new_end(i32), 4=cur_bytes(i32), 5=p(i32)
        let mut f = WasmFunction::new([(5, ValType::I32)]);
        // if size <= 0 { aligned_size = 8 } else { aligned_size = (size + 7) & !7 }
        f.instruction(&Instruction::LocalGet(0));
        f.instruction(&Instruction::I32Const(0));
        f.instruction(&Instruction::I32LeS);
        f.instruction(&Instruction::If(BlockType::Empty));
        f.instruction(&Instruction::I32Const(8));
        f.instruction(&Instruction::LocalSet(1));
        f.instruction(&Instruction::Else);
        f.instruction(&Instruction::LocalGet(0));
        f.instruction(&Instruction::I32Const(7));
        f.instruction(&Instruction::I32Add);
        f.instruction(&Instruction::I32Const(-8));
        f.instruction(&Instruction::I32And);
        f.instruction(&Instruction::LocalSet(1));
        f.instruction(&Instruction::End);

        // ptr = global[0]; new_end = ptr + aligned_size
        f.instruction(&Instruction::GlobalGet(0));
        f.instruction(&Instruction::LocalTee(2));
        f.instruction(&Instruction::LocalGet(1));
        f.instruction(&Instruction::I32Add);
        f.instruction(&Instruction::LocalTee(3));

        // cur_bytes = memory.size * 65536
        f.instruction(&Instruction::MemorySize(0));
        f.instruction(&Instruction::I32Const(16));
        f.instruction(&Instruction::I32Shl);
        f.instruction(&Instruction::LocalTee(4));
        f.instruction(&Instruction::I32GtU);
        f.instruction(&Instruction::If(BlockType::Empty));
        // grow by ((new_end - cur_bytes + 65535) >> 16) + 1 pages
        f.instruction(&Instruction::LocalGet(3));
        f.instruction(&Instruction::LocalGet(4));
        f.instruction(&Instruction::I32Sub);
        f.instruction(&Instruction::I32Const(65535));
        f.instruction(&Instruction::I32Add);
        f.instruction(&Instruction::I32Const(16));
        f.instruction(&Instruction::I32ShrU);
        f.instruction(&Instruction::I32Const(1));
        f.instruction(&Instruction::I32Add);
        f.instruction(&Instruction::MemoryGrow(0));
        f.instruction(&Instruction::Drop);
        f.instruction(&Instruction::End);

        // global[0] = new_end
        f.instruction(&Instruction::LocalGet(3));
        f.instruction(&Instruction::GlobalSet(0));

        // Zero-initialize [ptr .. new_end) in 8-byte chunks (safe across arena resets!)
        f.instruction(&Instruction::LocalGet(2));
        f.instruction(&Instruction::LocalSet(5));
        f.instruction(&Instruction::Block(BlockType::Empty));
        f.instruction(&Instruction::Loop(BlockType::Empty));
        f.instruction(&Instruction::LocalGet(5));
        f.instruction(&Instruction::LocalGet(3));
        f.instruction(&Instruction::I32GeU);
        f.instruction(&Instruction::BrIf(1));
        f.instruction(&Instruction::LocalGet(5));
        f.instruction(&Instruction::I64Const(0));
        f.instruction(&Instruction::I64Store(MemArg {
            offset: 0,
            align: 3,
            memory_index: 0,
        }));
        f.instruction(&Instruction::LocalGet(5));
        f.instruction(&Instruction::I32Const(8));
        f.instruction(&Instruction::I32Add);
        f.instruction(&Instruction::LocalSet(5));
        f.instruction(&Instruction::Br(0));
        f.instruction(&Instruction::End);
        f.instruction(&Instruction::End);

        // return ptr as i64
        f.instruction(&Instruction::LocalGet(2));
        f.instruction(&Instruction::I64ExtendI32U);
        f.instruction(&Instruction::End);
        f
    }

    /// `__knr_str_len(ptr: i64) -> i64`: reads 8-byte length prefix at `ptr - 8`.
    fn emit_builtin_str_len() -> WasmFunction {
        let mut f = WasmFunction::new([]);
        f.instruction(&Instruction::LocalGet(0));
        f.instruction(&Instruction::I64Const(8));
        f.instruction(&Instruction::I64LtU);
        f.instruction(&Instruction::If(BlockType::Empty));
        f.instruction(&Instruction::I64Const(0));
        f.instruction(&Instruction::Return);
        f.instruction(&Instruction::End);
        f.instruction(&Instruction::LocalGet(0));
        f.instruction(&Instruction::I32WrapI64);
        f.instruction(&Instruction::I32Const(8));
        f.instruction(&Instruction::I32Sub);
        f.instruction(&Instruction::I64Load(MemArg {
            offset: 0,
            align: 3,
            memory_index: 0,
        }));
        f.instruction(&Instruction::End);
        f
    }

    /// `__knr_str_eq(a: i64, b: i64) -> i32`: compares length prefixes & UTF-8 bytes.
    /// Treats null pointers (`< 8`) as empty strings (len 0) so unset struct string fields compare equal to `""`.
    fn emit_builtin_str_eq() -> WasmFunction {
        // params: 0=a (i64), 1=b (i64)
        // locals: 2=pa (i32), 3=pb (i32), 4=len_a (i32), 5=len_b (i32), 6=i (i32)
        let mut f = WasmFunction::new([(5, ValType::I32)]);
        // fast path: a == b -> 1
        f.instruction(&Instruction::LocalGet(0));
        f.instruction(&Instruction::LocalGet(1));
        f.instruction(&Instruction::I64Eq);
        f.instruction(&Instruction::If(BlockType::Empty));
        f.instruction(&Instruction::I32Const(1));
        f.instruction(&Instruction::Return);
        f.instruction(&Instruction::End);

        f.instruction(&Instruction::LocalGet(0));
        f.instruction(&Instruction::I32WrapI64);
        f.instruction(&Instruction::LocalSet(2));
        f.instruction(&Instruction::LocalGet(1));
        f.instruction(&Instruction::I32WrapI64);
        f.instruction(&Instruction::LocalSet(3));

        // len_a = (a >= 8) ? *(pa - 8) : 0
        f.instruction(&Instruction::LocalGet(0));
        f.instruction(&Instruction::I64Const(8));
        f.instruction(&Instruction::I64GeU);
        f.instruction(&Instruction::If(BlockType::Empty));
        f.instruction(&Instruction::LocalGet(2));
        f.instruction(&Instruction::I32Const(8));
        f.instruction(&Instruction::I32Sub);
        f.instruction(&Instruction::I64Load(MemArg { offset: 0, align: 3, memory_index: 0 }));
        f.instruction(&Instruction::I32WrapI64);
        f.instruction(&Instruction::LocalSet(4));
        f.instruction(&Instruction::End);

        // len_b = (b >= 8) ? *(pb - 8) : 0
        f.instruction(&Instruction::LocalGet(1));
        f.instruction(&Instruction::I64Const(8));
        f.instruction(&Instruction::I64GeU);
        f.instruction(&Instruction::If(BlockType::Empty));
        f.instruction(&Instruction::LocalGet(3));
        f.instruction(&Instruction::I32Const(8));
        f.instruction(&Instruction::I32Sub);
        f.instruction(&Instruction::I64Load(MemArg { offset: 0, align: 3, memory_index: 0 }));
        f.instruction(&Instruction::I32WrapI64);
        f.instruction(&Instruction::LocalSet(5));
        f.instruction(&Instruction::End);

        // if len_a != len_b -> 0
        f.instruction(&Instruction::LocalGet(4));
        f.instruction(&Instruction::LocalGet(5));
        f.instruction(&Instruction::I32Ne);
        f.instruction(&Instruction::If(BlockType::Empty));
        f.instruction(&Instruction::I32Const(0));
        f.instruction(&Instruction::Return);
        f.instruction(&Instruction::End);

        // if len_a == 0 -> 1
        f.instruction(&Instruction::LocalGet(4));
        f.instruction(&Instruction::I32Eqz);
        f.instruction(&Instruction::If(BlockType::Empty));
        f.instruction(&Instruction::I32Const(1));
        f.instruction(&Instruction::Return);
        f.instruction(&Instruction::End);

        // loop i = 0..len_a
        f.instruction(&Instruction::I32Const(0));
        f.instruction(&Instruction::LocalSet(6));
        f.instruction(&Instruction::Block(BlockType::Empty));
        f.instruction(&Instruction::Loop(BlockType::Empty));
        f.instruction(&Instruction::LocalGet(6));
        f.instruction(&Instruction::LocalGet(4));
        f.instruction(&Instruction::I32GeU);
        f.instruction(&Instruction::BrIf(1));
        f.instruction(&Instruction::LocalGet(2));
        f.instruction(&Instruction::LocalGet(6));
        f.instruction(&Instruction::I32Add);
        f.instruction(&Instruction::I32Load8U(MemArg { offset: 0, align: 0, memory_index: 0 }));
        f.instruction(&Instruction::LocalGet(3));
        f.instruction(&Instruction::LocalGet(6));
        f.instruction(&Instruction::I32Add);
        f.instruction(&Instruction::I32Load8U(MemArg { offset: 0, align: 0, memory_index: 0 }));
        f.instruction(&Instruction::I32Ne);
        f.instruction(&Instruction::If(BlockType::Empty));
        f.instruction(&Instruction::I32Const(0));
        f.instruction(&Instruction::Return);
        f.instruction(&Instruction::End);
        f.instruction(&Instruction::LocalGet(6));
        f.instruction(&Instruction::I32Const(1));
        f.instruction(&Instruction::I32Add);
        f.instruction(&Instruction::LocalSet(6));
        f.instruction(&Instruction::Br(0));
        f.instruction(&Instruction::End);
        f.instruction(&Instruction::End);

        f.instruction(&Instruction::I32Const(1));
        f.instruction(&Instruction::End);
        f
    }

    /// `__knr_str_concat(a: i64, b: i64) -> i64`
    fn emit_builtin_str_concat(alloc_idx: u32) -> WasmFunction {
        // params: 0=a(i64), 1=b(i64)
        // locals: 2=pa, 3=pb, 4=la, 5=lb, 6=base, 7=dst, 8=i (all i32)
        let mut f = WasmFunction::new([(7, ValType::I32)]);
        f.instruction(&Instruction::LocalGet(0));
        f.instruction(&Instruction::I32WrapI64);
        f.instruction(&Instruction::LocalSet(2));
        f.instruction(&Instruction::LocalGet(1));
        f.instruction(&Instruction::I32WrapI64);
        f.instruction(&Instruction::LocalSet(3));

        // la = (pa >= 8) ? *(pa - 8) : 0
        f.instruction(&Instruction::LocalGet(2));
        f.instruction(&Instruction::I32Const(8));
        f.instruction(&Instruction::I32GeU);
        f.instruction(&Instruction::If(BlockType::Empty));
        f.instruction(&Instruction::LocalGet(2));
        f.instruction(&Instruction::I32Const(8));
        f.instruction(&Instruction::I32Sub);
        f.instruction(&Instruction::I64Load(MemArg { offset: 0, align: 3, memory_index: 0 }));
        f.instruction(&Instruction::I32WrapI64);
        f.instruction(&Instruction::LocalSet(4));
        f.instruction(&Instruction::End);

        // lb = (pb >= 8) ? *(pb - 8) : 0
        f.instruction(&Instruction::LocalGet(3));
        f.instruction(&Instruction::I32Const(8));
        f.instruction(&Instruction::I32GeU);
        f.instruction(&Instruction::If(BlockType::Empty));
        f.instruction(&Instruction::LocalGet(3));
        f.instruction(&Instruction::I32Const(8));
        f.instruction(&Instruction::I32Sub);
        f.instruction(&Instruction::I64Load(MemArg { offset: 0, align: 3, memory_index: 0 }));
        f.instruction(&Instruction::I32WrapI64);
        f.instruction(&Instruction::LocalSet(5));
        f.instruction(&Instruction::End);

        // base = alloc(8 + la + lb + 1) as i32
        f.instruction(&Instruction::I32Const(9));
        f.instruction(&Instruction::LocalGet(4));
        f.instruction(&Instruction::I32Add);
        f.instruction(&Instruction::LocalGet(5));
        f.instruction(&Instruction::I32Add);
        f.instruction(&Instruction::Call(alloc_idx));
        f.instruction(&Instruction::I32WrapI64);
        f.instruction(&Instruction::LocalTee(6));
        // write total length at base
        f.instruction(&Instruction::LocalGet(4));
        f.instruction(&Instruction::LocalGet(5));
        f.instruction(&Instruction::I32Add);
        f.instruction(&Instruction::I64ExtendI32U);
        f.instruction(&Instruction::I64Store(MemArg { offset: 0, align: 3, memory_index: 0 }));

        // dst = base + 8
        f.instruction(&Instruction::LocalGet(6));
        f.instruction(&Instruction::I32Const(8));
        f.instruction(&Instruction::I32Add);
        f.instruction(&Instruction::LocalSet(7));

        // copy a
        f.instruction(&Instruction::I32Const(0));
        f.instruction(&Instruction::LocalSet(8));
        f.instruction(&Instruction::Block(BlockType::Empty));
        f.instruction(&Instruction::Loop(BlockType::Empty));
        f.instruction(&Instruction::LocalGet(8));
        f.instruction(&Instruction::LocalGet(4));
        f.instruction(&Instruction::I32GeU);
        f.instruction(&Instruction::BrIf(1));
        f.instruction(&Instruction::LocalGet(7));
        f.instruction(&Instruction::LocalGet(8));
        f.instruction(&Instruction::I32Add);
        f.instruction(&Instruction::LocalGet(2));
        f.instruction(&Instruction::LocalGet(8));
        f.instruction(&Instruction::I32Add);
        f.instruction(&Instruction::I32Load8U(MemArg { offset: 0, align: 0, memory_index: 0 }));
        f.instruction(&Instruction::I32Store8(MemArg { offset: 0, align: 0, memory_index: 0 }));
        f.instruction(&Instruction::LocalGet(8));
        f.instruction(&Instruction::I32Const(1));
        f.instruction(&Instruction::I32Add);
        f.instruction(&Instruction::LocalSet(8));
        f.instruction(&Instruction::Br(0));
        f.instruction(&Instruction::End);
        f.instruction(&Instruction::End);

        // copy b
        f.instruction(&Instruction::I32Const(0));
        f.instruction(&Instruction::LocalSet(8));
        f.instruction(&Instruction::Block(BlockType::Empty));
        f.instruction(&Instruction::Loop(BlockType::Empty));
        f.instruction(&Instruction::LocalGet(8));
        f.instruction(&Instruction::LocalGet(5));
        f.instruction(&Instruction::I32GeU);
        f.instruction(&Instruction::BrIf(1));
        f.instruction(&Instruction::LocalGet(7));
        f.instruction(&Instruction::LocalGet(4));
        f.instruction(&Instruction::I32Add);
        f.instruction(&Instruction::LocalGet(8));
        f.instruction(&Instruction::I32Add);
        f.instruction(&Instruction::LocalGet(3));
        f.instruction(&Instruction::LocalGet(8));
        f.instruction(&Instruction::I32Add);
        f.instruction(&Instruction::I32Load8U(MemArg { offset: 0, align: 0, memory_index: 0 }));
        f.instruction(&Instruction::I32Store8(MemArg { offset: 0, align: 0, memory_index: 0 }));
        f.instruction(&Instruction::LocalGet(8));
        f.instruction(&Instruction::I32Const(1));
        f.instruction(&Instruction::I32Add);
        f.instruction(&Instruction::LocalSet(8));
        f.instruction(&Instruction::Br(0));
        f.instruction(&Instruction::End);
        f.instruction(&Instruction::End);

        // null terminator
        f.instruction(&Instruction::LocalGet(7));
        f.instruction(&Instruction::LocalGet(4));
        f.instruction(&Instruction::I32Add);
        f.instruction(&Instruction::LocalGet(5));
        f.instruction(&Instruction::I32Add);
        f.instruction(&Instruction::I32Const(0));
        f.instruction(&Instruction::I32Store8(MemArg { offset: 0, align: 0, memory_index: 0 }));

        // return dst as i64
        f.instruction(&Instruction::LocalGet(7));
        f.instruction(&Instruction::I64ExtendI32U);
        f.instruction(&Instruction::End);
        f
    }

    /// `__knr_int_to_str(n: i64) -> i64`: formats signed i64 as decimal ASCII string with 8-byte len prefix.
    fn emit_builtin_int_to_str(alloc_idx: u32) -> WasmFunction {
        // param 0: n (i64)
        // locals: 1=base(i32), 2=pos(i32), 3=len(i32), 4=neg(i32), 5=dst(i32), 6=i(i32), 7=abs_n(i64)
        let mut f = WasmFunction::new([(6, ValType::I32), (1, ValType::I64)]);
        // Allocate 64 bytes: [0..32) scratch digits reversed, [32..40) len, [40..64) final string
        f.instruction(&Instruction::I32Const(64));
        f.instruction(&Instruction::Call(alloc_idx));
        f.instruction(&Instruction::I32WrapI64);
        f.instruction(&Instruction::LocalSet(1));

        // Handle 0 directly
        f.instruction(&Instruction::LocalGet(0));
        f.instruction(&Instruction::I64Eqz);
        f.instruction(&Instruction::If(BlockType::Empty));
        f.instruction(&Instruction::LocalGet(1));
        f.instruction(&Instruction::I64Const(1));
        f.instruction(&Instruction::I64Store(MemArg { offset: 32, align: 3, memory_index: 0 }));
        f.instruction(&Instruction::LocalGet(1));
        f.instruction(&Instruction::I32Const(48)); // '0'
        f.instruction(&Instruction::I32Store8(MemArg { offset: 40, align: 0, memory_index: 0 }));
        f.instruction(&Instruction::LocalGet(1));
        f.instruction(&Instruction::I32Const(0));
        f.instruction(&Instruction::I32Store8(MemArg { offset: 41, align: 0, memory_index: 0 }));
        f.instruction(&Instruction::LocalGet(1));
        f.instruction(&Instruction::I32Const(40));
        f.instruction(&Instruction::I32Add);
        f.instruction(&Instruction::I64ExtendI32U);
        f.instruction(&Instruction::Return);
        f.instruction(&Instruction::End);

        // neg = (n < 0)
        f.instruction(&Instruction::LocalGet(0));
        f.instruction(&Instruction::I64Const(0));
        f.instruction(&Instruction::I64LtS);
        f.instruction(&Instruction::LocalTee(4));
        f.instruction(&Instruction::If(BlockType::Empty));
        f.instruction(&Instruction::I64Const(0));
        f.instruction(&Instruction::LocalGet(0));
        f.instruction(&Instruction::I64Sub);
        f.instruction(&Instruction::LocalSet(7));
        f.instruction(&Instruction::Else);
        f.instruction(&Instruction::LocalGet(0));
        f.instruction(&Instruction::LocalSet(7));
        f.instruction(&Instruction::End);

        // Extract digits in reverse into scratch[0..pos]
        f.instruction(&Instruction::I32Const(0));
        f.instruction(&Instruction::LocalSet(2));
        f.instruction(&Instruction::Block(BlockType::Empty));
        f.instruction(&Instruction::Loop(BlockType::Empty));
        f.instruction(&Instruction::LocalGet(7));
        f.instruction(&Instruction::I64Eqz);
        f.instruction(&Instruction::BrIf(1));
        // scratch[pos] = '0' + (abs_n % 10)
        f.instruction(&Instruction::LocalGet(1));
        f.instruction(&Instruction::LocalGet(2));
        f.instruction(&Instruction::I32Add);
        f.instruction(&Instruction::LocalGet(7));
        f.instruction(&Instruction::I64Const(10));
        f.instruction(&Instruction::I64RemU);
        f.instruction(&Instruction::I32WrapI64);
        f.instruction(&Instruction::I32Const(48));
        f.instruction(&Instruction::I32Add);
        f.instruction(&Instruction::I32Store8(MemArg { offset: 0, align: 0, memory_index: 0 }));
        // abs_n /= 10
        f.instruction(&Instruction::LocalGet(7));
        f.instruction(&Instruction::I64Const(10));
        f.instruction(&Instruction::I64DivU);
        f.instruction(&Instruction::LocalSet(7));
        // pos += 1
        f.instruction(&Instruction::LocalGet(2));
        f.instruction(&Instruction::I32Const(1));
        f.instruction(&Instruction::I32Add);
        f.instruction(&Instruction::LocalSet(2));
        f.instruction(&Instruction::Br(0));
        f.instruction(&Instruction::End);
        f.instruction(&Instruction::End);

        // len = pos + neg
        f.instruction(&Instruction::LocalGet(2));
        f.instruction(&Instruction::LocalGet(4));
        f.instruction(&Instruction::I32Add);
        f.instruction(&Instruction::LocalSet(3));

        // store len at base + 32
        f.instruction(&Instruction::LocalGet(1));
        f.instruction(&Instruction::LocalGet(3));
        f.instruction(&Instruction::I64ExtendI32U);
        f.instruction(&Instruction::I64Store(MemArg { offset: 32, align: 3, memory_index: 0 }));

        // dst = base + 40
        f.instruction(&Instruction::LocalGet(1));
        f.instruction(&Instruction::I32Const(40));
        f.instruction(&Instruction::I32Add);
        f.instruction(&Instruction::LocalSet(5));

        f.instruction(&Instruction::LocalGet(4));
        f.instruction(&Instruction::If(BlockType::Empty));
        f.instruction(&Instruction::LocalGet(5));
        f.instruction(&Instruction::I32Const(45)); // '-'
        f.instruction(&Instruction::I32Store8(MemArg { offset: 0, align: 0, memory_index: 0 }));
        f.instruction(&Instruction::End);

        // copy reversed digits: dst[neg + i] = scratch[pos - 1 - i]
        f.instruction(&Instruction::I32Const(0));
        f.instruction(&Instruction::LocalSet(6));
        f.instruction(&Instruction::Block(BlockType::Empty));
        f.instruction(&Instruction::Loop(BlockType::Empty));
        f.instruction(&Instruction::LocalGet(6));
        f.instruction(&Instruction::LocalGet(2));
        f.instruction(&Instruction::I32GeU);
        f.instruction(&Instruction::BrIf(1));
        f.instruction(&Instruction::LocalGet(5));
        f.instruction(&Instruction::LocalGet(4));
        f.instruction(&Instruction::I32Add);
        f.instruction(&Instruction::LocalGet(6));
        f.instruction(&Instruction::I32Add);
        f.instruction(&Instruction::LocalGet(1));
        f.instruction(&Instruction::LocalGet(2));
        f.instruction(&Instruction::I32Const(1));
        f.instruction(&Instruction::I32Sub);
        f.instruction(&Instruction::LocalGet(6));
        f.instruction(&Instruction::I32Sub);
        f.instruction(&Instruction::I32Add);
        f.instruction(&Instruction::I32Load8U(MemArg { offset: 0, align: 0, memory_index: 0 }));
        f.instruction(&Instruction::I32Store8(MemArg { offset: 0, align: 0, memory_index: 0 }));
        f.instruction(&Instruction::LocalGet(6));
        f.instruction(&Instruction::I32Const(1));
        f.instruction(&Instruction::I32Add);
        f.instruction(&Instruction::LocalSet(6));
        f.instruction(&Instruction::Br(0));
        f.instruction(&Instruction::End);
        f.instruction(&Instruction::End);

        // null terminator
        f.instruction(&Instruction::LocalGet(5));
        f.instruction(&Instruction::LocalGet(3));
        f.instruction(&Instruction::I32Add);
        f.instruction(&Instruction::I32Const(0));
        f.instruction(&Instruction::I32Store8(MemArg { offset: 0, align: 0, memory_index: 0 }));

        f.instruction(&Instruction::LocalGet(5));
        f.instruction(&Instruction::I64ExtendI32U);
        f.instruction(&Instruction::End);
        f
    }

    /// `__knr_vec_new() -> i64`: allocates 24-byte header `[len=0, cap=8, data_ptr]`.
    fn emit_builtin_vec_new(alloc_idx: u32) -> WasmFunction {
        // locals: 0=hdr(i32), 1=data(i64)
        let mut f = WasmFunction::new([(1, ValType::I32), (1, ValType::I64)]);
        f.instruction(&Instruction::I32Const(24));
        f.instruction(&Instruction::Call(alloc_idx));
        f.instruction(&Instruction::I32WrapI64);
        f.instruction(&Instruction::LocalSet(0));

        f.instruction(&Instruction::I32Const(64)); // 8 * 8 bytes
        f.instruction(&Instruction::Call(alloc_idx));
        f.instruction(&Instruction::LocalSet(1));

        // len = 0
        f.instruction(&Instruction::LocalGet(0));
        f.instruction(&Instruction::I64Const(0));
        f.instruction(&Instruction::I64Store(MemArg { offset: 0, align: 3, memory_index: 0 }));
        // cap = 8
        f.instruction(&Instruction::LocalGet(0));
        f.instruction(&Instruction::I64Const(8));
        f.instruction(&Instruction::I64Store(MemArg { offset: 8, align: 3, memory_index: 0 }));
        // data_ptr = data
        f.instruction(&Instruction::LocalGet(0));
        f.instruction(&Instruction::LocalGet(1));
        f.instruction(&Instruction::I64Store(MemArg { offset: 16, align: 3, memory_index: 0 }));

        f.instruction(&Instruction::LocalGet(0));
        f.instruction(&Instruction::I64ExtendI32U);
        f.instruction(&Instruction::End);
        f
    }

    /// `__knr_vec_push(vec: i64, val: i64) -> i64`
    fn emit_builtin_vec_push(alloc_idx: u32) -> WasmFunction {
        // params: 0=vec(i64), 1=val(i64)
        // locals: 2=hdr(i32), 3=len(i32), 4=cap(i32), 5=data(i32), 6=new_cap(i32), 7=new_data(i32), 8=i(i32)
        let mut f = WasmFunction::new([(7, ValType::I32)]);
        f.instruction(&Instruction::LocalGet(0));
        f.instruction(&Instruction::I32WrapI64);
        f.instruction(&Instruction::LocalSet(2));

        f.instruction(&Instruction::LocalGet(2));
        f.instruction(&Instruction::I64Load(MemArg { offset: 0, align: 3, memory_index: 0 }));
        f.instruction(&Instruction::I32WrapI64);
        f.instruction(&Instruction::LocalSet(3));

        f.instruction(&Instruction::LocalGet(2));
        f.instruction(&Instruction::I64Load(MemArg { offset: 8, align: 3, memory_index: 0 }));
        f.instruction(&Instruction::I32WrapI64);
        f.instruction(&Instruction::LocalSet(4));

        f.instruction(&Instruction::LocalGet(2));
        f.instruction(&Instruction::I64Load(MemArg { offset: 16, align: 3, memory_index: 0 }));
        f.instruction(&Instruction::I32WrapI64);
        f.instruction(&Instruction::LocalSet(5));

        // Grow if len >= cap
        f.instruction(&Instruction::LocalGet(3));
        f.instruction(&Instruction::LocalGet(4));
        f.instruction(&Instruction::I32GeU);
        f.instruction(&Instruction::If(BlockType::Empty));
        f.instruction(&Instruction::LocalGet(4));
        f.instruction(&Instruction::I32Const(2));
        f.instruction(&Instruction::I32Mul);
        f.instruction(&Instruction::LocalTee(6));
        f.instruction(&Instruction::I32Const(8));
        f.instruction(&Instruction::I32Mul);
        f.instruction(&Instruction::Call(alloc_idx));
        f.instruction(&Instruction::I32WrapI64);
        f.instruction(&Instruction::LocalSet(7));

        // copy old items
        f.instruction(&Instruction::I32Const(0));
        f.instruction(&Instruction::LocalSet(8));
        f.instruction(&Instruction::Block(BlockType::Empty));
        f.instruction(&Instruction::Loop(BlockType::Empty));
        f.instruction(&Instruction::LocalGet(8));
        f.instruction(&Instruction::LocalGet(3));
        f.instruction(&Instruction::I32GeU);
        f.instruction(&Instruction::BrIf(1));
        f.instruction(&Instruction::LocalGet(7));
        f.instruction(&Instruction::LocalGet(8));
        f.instruction(&Instruction::I32Const(8));
        f.instruction(&Instruction::I32Mul);
        f.instruction(&Instruction::I32Add);
        f.instruction(&Instruction::LocalGet(5));
        f.instruction(&Instruction::LocalGet(8));
        f.instruction(&Instruction::I32Const(8));
        f.instruction(&Instruction::I32Mul);
        f.instruction(&Instruction::I32Add);
        f.instruction(&Instruction::I64Load(MemArg { offset: 0, align: 3, memory_index: 0 }));
        f.instruction(&Instruction::I64Store(MemArg { offset: 0, align: 3, memory_index: 0 }));
        f.instruction(&Instruction::LocalGet(8));
        f.instruction(&Instruction::I32Const(1));
        f.instruction(&Instruction::I32Add);
        f.instruction(&Instruction::LocalSet(8));
        f.instruction(&Instruction::Br(0));
        f.instruction(&Instruction::End);
        f.instruction(&Instruction::End);

        f.instruction(&Instruction::LocalGet(7));
        f.instruction(&Instruction::LocalSet(5));
        f.instruction(&Instruction::LocalGet(2));
        f.instruction(&Instruction::LocalGet(6));
        f.instruction(&Instruction::I64ExtendI32U);
        f.instruction(&Instruction::I64Store(MemArg { offset: 8, align: 3, memory_index: 0 }));
        f.instruction(&Instruction::LocalGet(2));
        f.instruction(&Instruction::LocalGet(5));
        f.instruction(&Instruction::I64ExtendI32U);
        f.instruction(&Instruction::I64Store(MemArg { offset: 16, align: 3, memory_index: 0 }));
        f.instruction(&Instruction::End);

        // data[len] = val
        f.instruction(&Instruction::LocalGet(5));
        f.instruction(&Instruction::LocalGet(3));
        f.instruction(&Instruction::I32Const(8));
        f.instruction(&Instruction::I32Mul);
        f.instruction(&Instruction::I32Add);
        f.instruction(&Instruction::LocalGet(1));
        f.instruction(&Instruction::I64Store(MemArg { offset: 0, align: 3, memory_index: 0 }));

        // len += 1
        f.instruction(&Instruction::LocalGet(2));
        f.instruction(&Instruction::LocalGet(3));
        f.instruction(&Instruction::I32Const(1));
        f.instruction(&Instruction::I32Add);
        f.instruction(&Instruction::I64ExtendI32U);
        f.instruction(&Instruction::I64Store(MemArg { offset: 0, align: 3, memory_index: 0 }));

        f.instruction(&Instruction::LocalGet(0));
        f.instruction(&Instruction::End);
        f
    }

    /// `__knr_vec_get(vec: i64, idx: i64) -> i64`
    fn emit_builtin_vec_get() -> WasmFunction {
        // params: 0=vec(i64), 1=idx(i64)
        let mut f = WasmFunction::new([]);
        f.instruction(&Instruction::LocalGet(0));
        f.instruction(&Instruction::I32WrapI64);
        f.instruction(&Instruction::I64Load(MemArg { offset: 16, align: 3, memory_index: 0 }));
        f.instruction(&Instruction::I32WrapI64);
        f.instruction(&Instruction::LocalGet(1));
        f.instruction(&Instruction::I32WrapI64);
        f.instruction(&Instruction::I32Const(8));
        f.instruction(&Instruction::I32Mul);
        f.instruction(&Instruction::I32Add);
        f.instruction(&Instruction::I64Load(MemArg { offset: 0, align: 3, memory_index: 0 }));
        f.instruction(&Instruction::End);
        f
    }

    /// `__knr_vec_len(vec: i64) -> i64`
    fn emit_builtin_vec_len() -> WasmFunction {
        let mut f = WasmFunction::new([]);
        f.instruction(&Instruction::LocalGet(0));
        f.instruction(&Instruction::I32WrapI64);
        f.instruction(&Instruction::I64Load(MemArg { offset: 0, align: 3, memory_index: 0 }));
        f.instruction(&Instruction::End);
        f
    }

    /// Pure-WASM `__knr_sin(x: f64) -> f64` (zero external imports, ~1e-11 precision).
    fn emit_builtin_sin() -> WasmFunction {
        // params: 0 = x(f64); locals: 1 = r(f64), 2 = x2(f64)
        let mut f = WasmFunction::new([(2, ValType::F64)]);
        let two_pi = std::f64::consts::TAU;
        let pi = std::f64::consts::PI;
        let half_pi = std::f64::consts::FRAC_PI_2;

        // r = x - 2*PI * floor((x + PI) / (2*PI)) -> [-PI, PI]
        f.instruction(&Instruction::LocalGet(0));
        f.instruction(&Instruction::F64Const(two_pi));
        f.instruction(&Instruction::LocalGet(0));
        f.instruction(&Instruction::F64Const(pi));
        f.instruction(&Instruction::F64Add);
        f.instruction(&Instruction::F64Const(two_pi));
        f.instruction(&Instruction::F64Div);
        f.instruction(&Instruction::F64Floor);
        f.instruction(&Instruction::F64Mul);
        f.instruction(&Instruction::F64Sub);
        f.instruction(&Instruction::LocalSet(1));

        // Fold into [-PI/2, PI/2]
        f.instruction(&Instruction::LocalGet(1));
        f.instruction(&Instruction::F64Const(half_pi));
        f.instruction(&Instruction::F64Gt);
        f.instruction(&Instruction::If(BlockType::Empty));
        f.instruction(&Instruction::F64Const(pi));
        f.instruction(&Instruction::LocalGet(1));
        f.instruction(&Instruction::F64Sub);
        f.instruction(&Instruction::LocalSet(1));
        f.instruction(&Instruction::Else);
        f.instruction(&Instruction::LocalGet(1));
        f.instruction(&Instruction::F64Const(-half_pi));
        f.instruction(&Instruction::F64Lt);
        f.instruction(&Instruction::If(BlockType::Empty));
        f.instruction(&Instruction::F64Const(-pi));
        f.instruction(&Instruction::LocalGet(1));
        f.instruction(&Instruction::F64Sub);
        f.instruction(&Instruction::LocalSet(1));
        f.instruction(&Instruction::End);
        f.instruction(&Instruction::End);

        // x2 = r * r
        f.instruction(&Instruction::LocalGet(1));
        f.instruction(&Instruction::LocalGet(1));
        f.instruction(&Instruction::F64Mul);
        f.instruction(&Instruction::LocalSet(2));

        // Horner polynomial: r * (1 + x2*(-1/6 + x2*(1/120 + x2*(-1/5040 + x2*(1/362880 - x2/39916800)))))
        f.instruction(&Instruction::LocalGet(1));
        f.instruction(&Instruction::F64Const(1.0));
        f.instruction(&Instruction::LocalGet(2));
        f.instruction(&Instruction::F64Const(-1.0 / 6.0));
        f.instruction(&Instruction::LocalGet(2));
        f.instruction(&Instruction::F64Const(1.0 / 120.0));
        f.instruction(&Instruction::LocalGet(2));
        f.instruction(&Instruction::F64Const(-1.0 / 5040.0));
        f.instruction(&Instruction::LocalGet(2));
        f.instruction(&Instruction::F64Const(1.0 / 362880.0));
        f.instruction(&Instruction::LocalGet(2));
        f.instruction(&Instruction::F64Const(-1.0 / 39916800.0));
        f.instruction(&Instruction::F64Mul);
        f.instruction(&Instruction::F64Add);
        f.instruction(&Instruction::F64Mul);
        f.instruction(&Instruction::F64Add);
        f.instruction(&Instruction::F64Mul);
        f.instruction(&Instruction::F64Add);
        f.instruction(&Instruction::F64Mul);
        f.instruction(&Instruction::F64Add);
        f.instruction(&Instruction::F64Mul);
        f.instruction(&Instruction::F64Add);
        f.instruction(&Instruction::F64Mul);
        f.instruction(&Instruction::End);
        f
    }

    /// Pure-WASM `__knr_cos(x: f64) -> f64` = `__knr_sin(x + PI/2)`
    fn emit_builtin_cos(sin_idx: u32) -> WasmFunction {
        let mut f = WasmFunction::new([]);
        f.instruction(&Instruction::LocalGet(0));
        f.instruction(&Instruction::F64Const(std::f64::consts::FRAC_PI_2));
        f.instruction(&Instruction::F64Add);
        f.instruction(&Instruction::Call(sin_idx));
        f.instruction(&Instruction::End);
        f
    }

    /// Pure-WASM `__knr_tan(x: f64) -> f64` = `__knr_sin(x) / __knr_cos(x)`
    fn emit_builtin_tan(sin_idx: u32, cos_idx: u32) -> WasmFunction {
        let mut f = WasmFunction::new([]);
        f.instruction(&Instruction::LocalGet(0));
        f.instruction(&Instruction::Call(sin_idx));
        f.instruction(&Instruction::LocalGet(0));
        f.instruction(&Instruction::Call(cos_idx));
        f.instruction(&Instruction::F64Div);
        f.instruction(&Instruction::End);
        f
    }

    /// Pure-WASM `__knr_pow(base: f64, exp: f64) -> f64` (zero external imports).
    fn emit_builtin_pow() -> WasmFunction {
        // params: 0=base(f64), 1=exp(f64)
        // locals: 2=m(f64), 3=k(f64), 4=t(f64), 5=t2(f64), 6=y(f64), 7=r(f64), 8=er(f64); 9=n(i32)
        let mut f = WasmFunction::new([(7, ValType::F64), (1, ValType::I32)]);
        let ln2 = std::f64::consts::LN_2;

        // if exp == 0.0 -> return 1.0
        f.instruction(&Instruction::LocalGet(1));
        f.instruction(&Instruction::F64Const(0.0));
        f.instruction(&Instruction::F64Eq);
        f.instruction(&Instruction::If(BlockType::Empty));
        f.instruction(&Instruction::F64Const(1.0));
        f.instruction(&Instruction::Return);
        f.instruction(&Instruction::End);

        // if base <= 0.0 -> return 0.0
        f.instruction(&Instruction::LocalGet(0));
        f.instruction(&Instruction::F64Const(0.0));
        f.instruction(&Instruction::F64Le);
        f.instruction(&Instruction::If(BlockType::Empty));
        f.instruction(&Instruction::F64Const(0.0));
        f.instruction(&Instruction::Return);
        f.instruction(&Instruction::End);

        // Reduce m into [0.7071, 1.4142]
        f.instruction(&Instruction::LocalGet(0));
        f.instruction(&Instruction::LocalSet(2));
        f.instruction(&Instruction::F64Const(0.0));
        f.instruction(&Instruction::LocalSet(3));

        f.instruction(&Instruction::Block(BlockType::Empty));
        f.instruction(&Instruction::Loop(BlockType::Empty));
        f.instruction(&Instruction::LocalGet(2));
        f.instruction(&Instruction::F64Const(std::f64::consts::SQRT_2));
        f.instruction(&Instruction::F64Le);
        f.instruction(&Instruction::BrIf(1));
        f.instruction(&Instruction::LocalGet(2));
        f.instruction(&Instruction::F64Const(0.5));
        f.instruction(&Instruction::F64Mul);
        f.instruction(&Instruction::LocalSet(2));
        f.instruction(&Instruction::LocalGet(3));
        f.instruction(&Instruction::F64Const(1.0));
        f.instruction(&Instruction::F64Add);
        f.instruction(&Instruction::LocalSet(3));
        f.instruction(&Instruction::Br(0));
        f.instruction(&Instruction::End);
        f.instruction(&Instruction::End);

        f.instruction(&Instruction::Block(BlockType::Empty));
        f.instruction(&Instruction::Loop(BlockType::Empty));
        f.instruction(&Instruction::LocalGet(2));
        f.instruction(&Instruction::F64Const(std::f64::consts::FRAC_1_SQRT_2));
        f.instruction(&Instruction::F64Ge);
        f.instruction(&Instruction::BrIf(1));
        f.instruction(&Instruction::LocalGet(2));
        f.instruction(&Instruction::F64Const(2.0));
        f.instruction(&Instruction::F64Mul);
        f.instruction(&Instruction::LocalSet(2));
        f.instruction(&Instruction::LocalGet(3));
        f.instruction(&Instruction::F64Const(1.0));
        f.instruction(&Instruction::F64Sub);
        f.instruction(&Instruction::LocalSet(3));
        f.instruction(&Instruction::Br(0));
        f.instruction(&Instruction::End);
        f.instruction(&Instruction::End);

        // t = (m - 1) / (m + 1); t2 = t * t
        f.instruction(&Instruction::LocalGet(2));
        f.instruction(&Instruction::F64Const(1.0));
        f.instruction(&Instruction::F64Sub);
        f.instruction(&Instruction::LocalGet(2));
        f.instruction(&Instruction::F64Const(1.0));
        f.instruction(&Instruction::F64Add);
        f.instruction(&Instruction::F64Div);
        f.instruction(&Instruction::LocalTee(4));
        f.instruction(&Instruction::LocalGet(4));
        f.instruction(&Instruction::F64Mul);
        f.instruction(&Instruction::LocalSet(5));

        // y = exp * (2*t*(1 + t2*(1/3 + t2*(1/5 + t2*(1/7 + t2*(1/9 + t2/11))))) + k*ln2)
        f.instruction(&Instruction::LocalGet(1));
        f.instruction(&Instruction::F64Const(2.0));
        f.instruction(&Instruction::LocalGet(4));
        f.instruction(&Instruction::F64Mul);
        f.instruction(&Instruction::F64Const(1.0));
        f.instruction(&Instruction::LocalGet(5));
        f.instruction(&Instruction::F64Const(1.0 / 3.0));
        f.instruction(&Instruction::LocalGet(5));
        f.instruction(&Instruction::F64Const(1.0 / 5.0));
        f.instruction(&Instruction::LocalGet(5));
        f.instruction(&Instruction::F64Const(1.0 / 7.0));
        f.instruction(&Instruction::LocalGet(5));
        f.instruction(&Instruction::F64Const(1.0 / 9.0));
        f.instruction(&Instruction::LocalGet(5));
        f.instruction(&Instruction::F64Const(1.0 / 11.0));
        f.instruction(&Instruction::F64Mul);
        f.instruction(&Instruction::F64Add);
        f.instruction(&Instruction::F64Mul);
        f.instruction(&Instruction::F64Add);
        f.instruction(&Instruction::F64Mul);
        f.instruction(&Instruction::F64Add);
        f.instruction(&Instruction::F64Mul);
        f.instruction(&Instruction::F64Add);
        f.instruction(&Instruction::F64Mul);
        f.instruction(&Instruction::F64Add);
        f.instruction(&Instruction::F64Mul);
        f.instruction(&Instruction::LocalGet(3));
        f.instruction(&Instruction::F64Const(ln2));
        f.instruction(&Instruction::F64Mul);
        f.instruction(&Instruction::F64Add);
        f.instruction(&Instruction::F64Mul);
        f.instruction(&Instruction::LocalSet(6));

        // exp(y): n = floor(y/ln2 + 0.5), r = y - n*ln2
        f.instruction(&Instruction::LocalGet(6));
        f.instruction(&Instruction::F64Const(ln2));
        f.instruction(&Instruction::F64Div);
        f.instruction(&Instruction::F64Const(0.5));
        f.instruction(&Instruction::F64Add);
        f.instruction(&Instruction::F64Floor);
        f.instruction(&Instruction::LocalTee(3));
        f.instruction(&Instruction::I32TruncSatF64S);
        f.instruction(&Instruction::LocalSet(9));

        f.instruction(&Instruction::LocalGet(6));
        f.instruction(&Instruction::LocalGet(3));
        f.instruction(&Instruction::F64Const(ln2));
        f.instruction(&Instruction::F64Mul);
        f.instruction(&Instruction::F64Sub);
        f.instruction(&Instruction::LocalSet(7));

        // er = 1 + r*(1 + r*(1/2 + r*(1/6 + r*(1/24 + r*(1/120 + r*(1/720 + r/5040))))))
        f.instruction(&Instruction::F64Const(1.0));
        f.instruction(&Instruction::LocalGet(7));
        f.instruction(&Instruction::F64Const(1.0));
        f.instruction(&Instruction::LocalGet(7));
        f.instruction(&Instruction::F64Const(0.5));
        f.instruction(&Instruction::LocalGet(7));
        f.instruction(&Instruction::F64Const(1.0 / 6.0));
        f.instruction(&Instruction::LocalGet(7));
        f.instruction(&Instruction::F64Const(1.0 / 24.0));
        f.instruction(&Instruction::LocalGet(7));
        f.instruction(&Instruction::F64Const(1.0 / 120.0));
        f.instruction(&Instruction::LocalGet(7));
        f.instruction(&Instruction::F64Const(1.0 / 720.0));
        f.instruction(&Instruction::LocalGet(7));
        f.instruction(&Instruction::F64Const(1.0 / 5040.0));
        f.instruction(&Instruction::F64Mul);
        f.instruction(&Instruction::F64Add);
        f.instruction(&Instruction::F64Mul);
        f.instruction(&Instruction::F64Add);
        f.instruction(&Instruction::F64Mul);
        f.instruction(&Instruction::F64Add);
        f.instruction(&Instruction::F64Mul);
        f.instruction(&Instruction::F64Add);
        f.instruction(&Instruction::F64Mul);
        f.instruction(&Instruction::F64Add);
        f.instruction(&Instruction::F64Mul);
        f.instruction(&Instruction::F64Add);
        f.instruction(&Instruction::F64Mul);
        f.instruction(&Instruction::F64Add);
        f.instruction(&Instruction::LocalSet(8));

        // Scale er by 2^n
        f.instruction(&Instruction::Block(BlockType::Empty));
        f.instruction(&Instruction::Loop(BlockType::Empty));
        f.instruction(&Instruction::LocalGet(9));
        f.instruction(&Instruction::I32Const(0));
        f.instruction(&Instruction::I32LeS);
        f.instruction(&Instruction::BrIf(1));
        f.instruction(&Instruction::LocalGet(8));
        f.instruction(&Instruction::F64Const(2.0));
        f.instruction(&Instruction::F64Mul);
        f.instruction(&Instruction::LocalSet(8));
        f.instruction(&Instruction::LocalGet(9));
        f.instruction(&Instruction::I32Const(1));
        f.instruction(&Instruction::I32Sub);
        f.instruction(&Instruction::LocalSet(9));
        f.instruction(&Instruction::Br(0));
        f.instruction(&Instruction::End);
        f.instruction(&Instruction::End);

        f.instruction(&Instruction::Block(BlockType::Empty));
        f.instruction(&Instruction::Loop(BlockType::Empty));
        f.instruction(&Instruction::LocalGet(9));
        f.instruction(&Instruction::I32Const(0));
        f.instruction(&Instruction::I32GeS);
        f.instruction(&Instruction::BrIf(1));
        f.instruction(&Instruction::LocalGet(8));
        f.instruction(&Instruction::F64Const(0.5));
        f.instruction(&Instruction::F64Mul);
        f.instruction(&Instruction::LocalSet(8));
        f.instruction(&Instruction::LocalGet(9));
        f.instruction(&Instruction::I32Const(1));
        f.instruction(&Instruction::I32Add);
        f.instruction(&Instruction::LocalSet(9));
        f.instruction(&Instruction::Br(0));
        f.instruction(&Instruction::End);
        f.instruction(&Instruction::End);

        f.instruction(&Instruction::LocalGet(8));
        f.instruction(&Instruction::End);
        f
    }

    /// Pure-WASM `__knr_atan2(y: f64, x: f64) -> f64` (zero external imports).
    fn emit_builtin_atan2() -> WasmFunction {
        // params: 0=y(f64), 1=x(f64)
        // locals: 2=ax(f64), 3=ay(f64), 4=z(f64), 5=z2(f64), 6=a(f64)
        let mut f = WasmFunction::new([(5, ValType::F64)]);
        let pi = std::f64::consts::PI;
        let half_pi = std::f64::consts::FRAC_PI_2;

        f.instruction(&Instruction::LocalGet(1));
        f.instruction(&Instruction::F64Abs);
        f.instruction(&Instruction::LocalSet(2));
        f.instruction(&Instruction::LocalGet(0));
        f.instruction(&Instruction::F64Abs);
        f.instruction(&Instruction::LocalSet(3));

        // If ax == 0 && ay == 0 -> 0.0
        f.instruction(&Instruction::LocalGet(2));
        f.instruction(&Instruction::LocalGet(3));
        f.instruction(&Instruction::F64Add);
        f.instruction(&Instruction::F64Const(0.0));
        f.instruction(&Instruction::F64Eq);
        f.instruction(&Instruction::If(BlockType::Empty));
        f.instruction(&Instruction::F64Const(0.0));
        f.instruction(&Instruction::Return);
        f.instruction(&Instruction::End);

        // z = min(ax, ay) / max(ax, ay)
        f.instruction(&Instruction::LocalGet(2));
        f.instruction(&Instruction::LocalGet(3));
        f.instruction(&Instruction::F64Min);
        f.instruction(&Instruction::LocalGet(2));
        f.instruction(&Instruction::LocalGet(3));
        f.instruction(&Instruction::F64Max);
        f.instruction(&Instruction::F64Div);
        f.instruction(&Instruction::LocalTee(4));
        f.instruction(&Instruction::LocalGet(4));
        f.instruction(&Instruction::F64Mul);
        f.instruction(&Instruction::LocalSet(5));

        // Minimax atan(z) on [0, 1]
        f.instruction(&Instruction::LocalGet(4));
        f.instruction(&Instruction::F64Const(0.9999993329));
        f.instruction(&Instruction::LocalGet(5));
        f.instruction(&Instruction::F64Const(-0.3332985605));
        f.instruction(&Instruction::LocalGet(5));
        f.instruction(&Instruction::F64Const(0.1994653599));
        f.instruction(&Instruction::LocalGet(5));
        f.instruction(&Instruction::F64Const(-0.1308779333));
        f.instruction(&Instruction::LocalGet(5));
        f.instruction(&Instruction::F64Const(0.0680899895));
        f.instruction(&Instruction::LocalGet(5));
        f.instruction(&Instruction::F64Const(-0.0176071186));
        f.instruction(&Instruction::F64Mul);
        f.instruction(&Instruction::F64Add);
        f.instruction(&Instruction::F64Mul);
        f.instruction(&Instruction::F64Add);
        f.instruction(&Instruction::F64Mul);
        f.instruction(&Instruction::F64Add);
        f.instruction(&Instruction::F64Mul);
        f.instruction(&Instruction::F64Add);
        f.instruction(&Instruction::F64Mul);
        f.instruction(&Instruction::F64Add);
        f.instruction(&Instruction::F64Mul);
        f.instruction(&Instruction::LocalSet(6));

        // if ay > ax: a = PI/2 - a
        f.instruction(&Instruction::LocalGet(3));
        f.instruction(&Instruction::LocalGet(2));
        f.instruction(&Instruction::F64Gt);
        f.instruction(&Instruction::If(BlockType::Empty));
        f.instruction(&Instruction::F64Const(half_pi));
        f.instruction(&Instruction::LocalGet(6));
        f.instruction(&Instruction::F64Sub);
        f.instruction(&Instruction::LocalSet(6));
        f.instruction(&Instruction::End);

        // if x < 0: a = PI - a
        f.instruction(&Instruction::LocalGet(1));
        f.instruction(&Instruction::F64Const(0.0));
        f.instruction(&Instruction::F64Lt);
        f.instruction(&Instruction::If(BlockType::Empty));
        f.instruction(&Instruction::F64Const(pi));
        f.instruction(&Instruction::LocalGet(6));
        f.instruction(&Instruction::F64Sub);
        f.instruction(&Instruction::LocalSet(6));
        f.instruction(&Instruction::End);

        // if y < 0: a = -a
        f.instruction(&Instruction::LocalGet(0));
        f.instruction(&Instruction::F64Const(0.0));
        f.instruction(&Instruction::F64Lt);
        f.instruction(&Instruction::If(BlockType::Empty));
        f.instruction(&Instruction::LocalGet(6));
        f.instruction(&Instruction::F64Neg);
        f.instruction(&Instruction::LocalSet(6));
        f.instruction(&Instruction::End);

        f.instruction(&Instruction::LocalGet(6));
        f.instruction(&Instruction::End);
        f
    }

    /// `__knr_map_new() -> i64`: allocates 24-byte header `[len=0, cap=8, entries_ptr]`
    /// with initial 8-entry table (each entry is 16 bytes: `[key_str_ptr: i64, val: i64]`).
    fn emit_builtin_map_new(alloc_idx: u32) -> WasmFunction {
        // locals: 0=hdr(i32), 1=data(i64)
        let mut f = WasmFunction::new([(1, ValType::I32), (1, ValType::I64)]);
        f.instruction(&Instruction::I32Const(24));
        f.instruction(&Instruction::Call(alloc_idx));
        f.instruction(&Instruction::I32WrapI64);
        f.instruction(&Instruction::LocalSet(0));

        f.instruction(&Instruction::I32Const(128)); // 8 * 16 bytes
        f.instruction(&Instruction::Call(alloc_idx));
        f.instruction(&Instruction::LocalSet(1));

        // len = 0
        f.instruction(&Instruction::LocalGet(0));
        f.instruction(&Instruction::I64Const(0));
        f.instruction(&Instruction::I64Store(MemArg { offset: 0, align: 3, memory_index: 0 }));
        // cap = 8
        f.instruction(&Instruction::LocalGet(0));
        f.instruction(&Instruction::I64Const(8));
        f.instruction(&Instruction::I64Store(MemArg { offset: 8, align: 3, memory_index: 0 }));
        // entries_ptr = data
        f.instruction(&Instruction::LocalGet(0));
        f.instruction(&Instruction::LocalGet(1));
        f.instruction(&Instruction::I64Store(MemArg { offset: 16, align: 3, memory_index: 0 }));

        f.instruction(&Instruction::LocalGet(0));
        f.instruction(&Instruction::I64ExtendI32U);
        f.instruction(&Instruction::End);
        f
    }

    /// `__knr_map_set(map: i64, key: i64, val: i64) -> i64`: upserts `(key, val)` by string equality.
    fn emit_builtin_map_set(alloc_idx: u32, str_eq_idx: u32) -> WasmFunction {
        // params: 0=map(i64), 1=key(i64), 2=val(i64)
        // locals: 3=hdr, 4=len, 5=cap, 6=data, 7=i, 8=new_cap, 9=new_data, 10=entry_ptr (all i32)
        let mut f = WasmFunction::new([(8, ValType::I32)]);
        f.instruction(&Instruction::LocalGet(0));
        f.instruction(&Instruction::I32WrapI64);
        f.instruction(&Instruction::LocalSet(3));

        f.instruction(&Instruction::LocalGet(3));
        f.instruction(&Instruction::I64Load(MemArg { offset: 0, align: 3, memory_index: 0 }));
        f.instruction(&Instruction::I32WrapI64);
        f.instruction(&Instruction::LocalSet(4));

        f.instruction(&Instruction::LocalGet(3));
        f.instruction(&Instruction::I64Load(MemArg { offset: 8, align: 3, memory_index: 0 }));
        f.instruction(&Instruction::I32WrapI64);
        f.instruction(&Instruction::LocalSet(5));

        f.instruction(&Instruction::LocalGet(3));
        f.instruction(&Instruction::I64Load(MemArg { offset: 16, align: 3, memory_index: 0 }));
        f.instruction(&Instruction::I32WrapI64);
        f.instruction(&Instruction::LocalSet(6));

        // Search existing entry
        f.instruction(&Instruction::I32Const(0));
        f.instruction(&Instruction::LocalSet(7));
        f.instruction(&Instruction::Block(BlockType::Empty));
        f.instruction(&Instruction::Loop(BlockType::Empty));
        f.instruction(&Instruction::LocalGet(7));
        f.instruction(&Instruction::LocalGet(4));
        f.instruction(&Instruction::I32GeU);
        f.instruction(&Instruction::BrIf(1));
        f.instruction(&Instruction::LocalGet(6));
        f.instruction(&Instruction::LocalGet(7));
        f.instruction(&Instruction::I32Const(16));
        f.instruction(&Instruction::I32Mul);
        f.instruction(&Instruction::I32Add);
        f.instruction(&Instruction::LocalTee(10));
        f.instruction(&Instruction::I64Load(MemArg { offset: 0, align: 3, memory_index: 0 }));
        f.instruction(&Instruction::LocalGet(1));
        f.instruction(&Instruction::Call(str_eq_idx));
        f.instruction(&Instruction::If(BlockType::Empty));
        // Overwrite existing entry value in-place
        f.instruction(&Instruction::LocalGet(10));
        f.instruction(&Instruction::LocalGet(2));
        f.instruction(&Instruction::I64Store(MemArg { offset: 8, align: 3, memory_index: 0 }));
        f.instruction(&Instruction::LocalGet(0));
        f.instruction(&Instruction::Return);
        f.instruction(&Instruction::End);
        f.instruction(&Instruction::LocalGet(7));
        f.instruction(&Instruction::I32Const(1));
        f.instruction(&Instruction::I32Add);
        f.instruction(&Instruction::LocalSet(7));
        f.instruction(&Instruction::Br(0));
        f.instruction(&Instruction::End);
        f.instruction(&Instruction::End);

        // Grow if len >= cap
        f.instruction(&Instruction::LocalGet(4));
        f.instruction(&Instruction::LocalGet(5));
        f.instruction(&Instruction::I32GeU);
        f.instruction(&Instruction::If(BlockType::Empty));
        f.instruction(&Instruction::LocalGet(5));
        f.instruction(&Instruction::I32Const(2));
        f.instruction(&Instruction::I32Mul);
        f.instruction(&Instruction::LocalTee(8));
        f.instruction(&Instruction::I32Const(16));
        f.instruction(&Instruction::I32Mul);
        f.instruction(&Instruction::Call(alloc_idx));
        f.instruction(&Instruction::I32WrapI64);
        f.instruction(&Instruction::LocalSet(9));

        // Copy old entries
        f.instruction(&Instruction::I32Const(0));
        f.instruction(&Instruction::LocalSet(7));
        f.instruction(&Instruction::Block(BlockType::Empty));
        f.instruction(&Instruction::Loop(BlockType::Empty));
        f.instruction(&Instruction::LocalGet(7));
        f.instruction(&Instruction::LocalGet(4));
        f.instruction(&Instruction::I32GeU);
        f.instruction(&Instruction::BrIf(1));
        // copy key
        f.instruction(&Instruction::LocalGet(9));
        f.instruction(&Instruction::LocalGet(7));
        f.instruction(&Instruction::I32Const(16));
        f.instruction(&Instruction::I32Mul);
        f.instruction(&Instruction::I32Add);
        f.instruction(&Instruction::LocalGet(6));
        f.instruction(&Instruction::LocalGet(7));
        f.instruction(&Instruction::I32Const(16));
        f.instruction(&Instruction::I32Mul);
        f.instruction(&Instruction::I32Add);
        f.instruction(&Instruction::LocalTee(10));
        f.instruction(&Instruction::I64Load(MemArg { offset: 0, align: 3, memory_index: 0 }));
        f.instruction(&Instruction::I64Store(MemArg { offset: 0, align: 3, memory_index: 0 }));
        // copy val
        f.instruction(&Instruction::LocalGet(9));
        f.instruction(&Instruction::LocalGet(7));
        f.instruction(&Instruction::I32Const(16));
        f.instruction(&Instruction::I32Mul);
        f.instruction(&Instruction::I32Add);
        f.instruction(&Instruction::LocalGet(10));
        f.instruction(&Instruction::I64Load(MemArg { offset: 8, align: 3, memory_index: 0 }));
        f.instruction(&Instruction::I64Store(MemArg { offset: 8, align: 3, memory_index: 0 }));
        f.instruction(&Instruction::LocalGet(7));
        f.instruction(&Instruction::I32Const(1));
        f.instruction(&Instruction::I32Add);
        f.instruction(&Instruction::LocalSet(7));
        f.instruction(&Instruction::Br(0));
        f.instruction(&Instruction::End);
        f.instruction(&Instruction::End);

        f.instruction(&Instruction::LocalGet(9));
        f.instruction(&Instruction::LocalSet(6));
        f.instruction(&Instruction::LocalGet(3));
        f.instruction(&Instruction::LocalGet(8));
        f.instruction(&Instruction::I64ExtendI32U);
        f.instruction(&Instruction::I64Store(MemArg { offset: 8, align: 3, memory_index: 0 }));
        f.instruction(&Instruction::LocalGet(3));
        f.instruction(&Instruction::LocalGet(6));
        f.instruction(&Instruction::I64ExtendI32U);
        f.instruction(&Instruction::I64Store(MemArg { offset: 16, align: 3, memory_index: 0 }));
        f.instruction(&Instruction::End);

        // Append new entry at entries[len]
        f.instruction(&Instruction::LocalGet(6));
        f.instruction(&Instruction::LocalGet(4));
        f.instruction(&Instruction::I32Const(16));
        f.instruction(&Instruction::I32Mul);
        f.instruction(&Instruction::I32Add);
        f.instruction(&Instruction::LocalTee(10));
        f.instruction(&Instruction::LocalGet(1));
        f.instruction(&Instruction::I64Store(MemArg { offset: 0, align: 3, memory_index: 0 }));
        f.instruction(&Instruction::LocalGet(10));
        f.instruction(&Instruction::LocalGet(2));
        f.instruction(&Instruction::I64Store(MemArg { offset: 8, align: 3, memory_index: 0 }));

        // len += 1
        f.instruction(&Instruction::LocalGet(3));
        f.instruction(&Instruction::LocalGet(4));
        f.instruction(&Instruction::I32Const(1));
        f.instruction(&Instruction::I32Add);
        f.instruction(&Instruction::I64ExtendI32U);
        f.instruction(&Instruction::I64Store(MemArg { offset: 0, align: 3, memory_index: 0 }));

        f.instruction(&Instruction::LocalGet(0));
        f.instruction(&Instruction::End);
        f
    }

    /// `__knr_map_get(map: i64, key: i64) -> i64`: returns value bits for matching key, or 0 if absent.
    fn emit_builtin_map_get(str_eq_idx: u32) -> WasmFunction {
        // params: 0=map(i64), 1=key(i64)
        // locals: 2=hdr, 3=len, 4=data, 5=i, 6=entry_ptr (all i32)
        let mut f = WasmFunction::new([(5, ValType::I32)]);
        f.instruction(&Instruction::LocalGet(0));
        f.instruction(&Instruction::I64Eqz);
        f.instruction(&Instruction::If(BlockType::Empty));
        f.instruction(&Instruction::I64Const(0));
        f.instruction(&Instruction::Return);
        f.instruction(&Instruction::End);

        f.instruction(&Instruction::LocalGet(0));
        f.instruction(&Instruction::I32WrapI64);
        f.instruction(&Instruction::LocalSet(2));
        f.instruction(&Instruction::LocalGet(2));
        f.instruction(&Instruction::I64Load(MemArg { offset: 0, align: 3, memory_index: 0 }));
        f.instruction(&Instruction::I32WrapI64);
        f.instruction(&Instruction::LocalSet(3));
        f.instruction(&Instruction::LocalGet(2));
        f.instruction(&Instruction::I64Load(MemArg { offset: 16, align: 3, memory_index: 0 }));
        f.instruction(&Instruction::I32WrapI64);
        f.instruction(&Instruction::LocalSet(4));

        f.instruction(&Instruction::I32Const(0));
        f.instruction(&Instruction::LocalSet(5));
        f.instruction(&Instruction::Block(BlockType::Empty));
        f.instruction(&Instruction::Loop(BlockType::Empty));
        f.instruction(&Instruction::LocalGet(5));
        f.instruction(&Instruction::LocalGet(3));
        f.instruction(&Instruction::I32GeU);
        f.instruction(&Instruction::BrIf(1));
        f.instruction(&Instruction::LocalGet(4));
        f.instruction(&Instruction::LocalGet(5));
        f.instruction(&Instruction::I32Const(16));
        f.instruction(&Instruction::I32Mul);
        f.instruction(&Instruction::I32Add);
        f.instruction(&Instruction::LocalTee(6));
        f.instruction(&Instruction::I64Load(MemArg { offset: 0, align: 3, memory_index: 0 }));
        f.instruction(&Instruction::LocalGet(1));
        f.instruction(&Instruction::Call(str_eq_idx));
        f.instruction(&Instruction::If(BlockType::Empty));
        f.instruction(&Instruction::LocalGet(6));
        f.instruction(&Instruction::I64Load(MemArg { offset: 8, align: 3, memory_index: 0 }));
        f.instruction(&Instruction::Return);
        f.instruction(&Instruction::End);
        f.instruction(&Instruction::LocalGet(5));
        f.instruction(&Instruction::I32Const(1));
        f.instruction(&Instruction::I32Add);
        f.instruction(&Instruction::LocalSet(5));
        f.instruction(&Instruction::Br(0));
        f.instruction(&Instruction::End);
        f.instruction(&Instruction::End);

        f.instruction(&Instruction::I64Const(0));
        f.instruction(&Instruction::End);
        f
    }

    /// `__knr_map_del(map: i64, key: i64) -> i64`: removes matching key via swap-remove with last entry.
    fn emit_builtin_map_del(str_eq_idx: u32) -> WasmFunction {
        // params: 0=map(i64), 1=key(i64)
        // locals: 2=hdr, 3=len, 4=data, 5=i, 6=entry_ptr, 7=last_idx, 8=last_ptr (all i32)
        let mut f = WasmFunction::new([(7, ValType::I32)]);
        f.instruction(&Instruction::LocalGet(0));
        f.instruction(&Instruction::I64Eqz);
        f.instruction(&Instruction::If(BlockType::Empty));
        f.instruction(&Instruction::LocalGet(0));
        f.instruction(&Instruction::Return);
        f.instruction(&Instruction::End);

        f.instruction(&Instruction::LocalGet(0));
        f.instruction(&Instruction::I32WrapI64);
        f.instruction(&Instruction::LocalSet(2));
        f.instruction(&Instruction::LocalGet(2));
        f.instruction(&Instruction::I64Load(MemArg { offset: 0, align: 3, memory_index: 0 }));
        f.instruction(&Instruction::I32WrapI64);
        f.instruction(&Instruction::LocalSet(3));
        f.instruction(&Instruction::LocalGet(2));
        f.instruction(&Instruction::I64Load(MemArg { offset: 16, align: 3, memory_index: 0 }));
        f.instruction(&Instruction::I32WrapI64);
        f.instruction(&Instruction::LocalSet(4));

        f.instruction(&Instruction::I32Const(0));
        f.instruction(&Instruction::LocalSet(5));
        f.instruction(&Instruction::Block(BlockType::Empty));
        f.instruction(&Instruction::Loop(BlockType::Empty));
        f.instruction(&Instruction::LocalGet(5));
        f.instruction(&Instruction::LocalGet(3));
        f.instruction(&Instruction::I32GeU);
        f.instruction(&Instruction::BrIf(1));
        f.instruction(&Instruction::LocalGet(4));
        f.instruction(&Instruction::LocalGet(5));
        f.instruction(&Instruction::I32Const(16));
        f.instruction(&Instruction::I32Mul);
        f.instruction(&Instruction::I32Add);
        f.instruction(&Instruction::LocalTee(6));
        f.instruction(&Instruction::I64Load(MemArg { offset: 0, align: 3, memory_index: 0 }));
        f.instruction(&Instruction::LocalGet(1));
        f.instruction(&Instruction::Call(str_eq_idx));
        f.instruction(&Instruction::If(BlockType::Empty));
        // last_idx = len - 1
        f.instruction(&Instruction::LocalGet(3));
        f.instruction(&Instruction::I32Const(1));
        f.instruction(&Instruction::I32Sub);
        f.instruction(&Instruction::LocalTee(7));
        // if i < last_idx, move last entry into slot i
        f.instruction(&Instruction::LocalGet(5));
        f.instruction(&Instruction::I32GtU);
        f.instruction(&Instruction::If(BlockType::Empty));
        f.instruction(&Instruction::LocalGet(4));
        f.instruction(&Instruction::LocalGet(7));
        f.instruction(&Instruction::I32Const(16));
        f.instruction(&Instruction::I32Mul);
        f.instruction(&Instruction::I32Add);
        f.instruction(&Instruction::LocalSet(8));
        f.instruction(&Instruction::LocalGet(6));
        f.instruction(&Instruction::LocalGet(8));
        f.instruction(&Instruction::I64Load(MemArg { offset: 0, align: 3, memory_index: 0 }));
        f.instruction(&Instruction::I64Store(MemArg { offset: 0, align: 3, memory_index: 0 }));
        f.instruction(&Instruction::LocalGet(6));
        f.instruction(&Instruction::LocalGet(8));
        f.instruction(&Instruction::I64Load(MemArg { offset: 8, align: 3, memory_index: 0 }));
        f.instruction(&Instruction::I64Store(MemArg { offset: 8, align: 3, memory_index: 0 }));
        f.instruction(&Instruction::End);
        // len = last_idx
        f.instruction(&Instruction::LocalGet(2));
        f.instruction(&Instruction::LocalGet(7));
        f.instruction(&Instruction::I64ExtendI32U);
        f.instruction(&Instruction::I64Store(MemArg { offset: 0, align: 3, memory_index: 0 }));
        f.instruction(&Instruction::LocalGet(0));
        f.instruction(&Instruction::Return);
        f.instruction(&Instruction::End);
        f.instruction(&Instruction::LocalGet(5));
        f.instruction(&Instruction::I32Const(1));
        f.instruction(&Instruction::I32Add);
        f.instruction(&Instruction::LocalSet(5));
        f.instruction(&Instruction::Br(0));
        f.instruction(&Instruction::End);
        f.instruction(&Instruction::End);

        f.instruction(&Instruction::LocalGet(0));
        f.instruction(&Instruction::End);
        f
    }

    /// `__knr_buf_alloc(size: i64) -> i64`: allocates zero-initialized byte buffer in linear memory.
    fn emit_builtin_buf_alloc(alloc_idx: u32) -> WasmFunction {
        let mut f = WasmFunction::new([]);
        f.instruction(&Instruction::LocalGet(0));
        f.instruction(&Instruction::I32WrapI64);
        f.instruction(&Instruction::Call(alloc_idx));
        f.instruction(&Instruction::End);
        f
    }

    /// `__knr_f64_to_str(x: f64) -> i64`: formats finite f64 with up to 6 fractional digits (trailing zeros stripped).
    fn emit_builtin_f64_to_str(alloc_idx: u32, int_to_str_idx: u32) -> WasmFunction {
        // param 0: x (f64)
        // locals:
        //   i32 (8): 1=is_neg, 2=base, 3=pos, 4=fdigits, 5=len, 6=dst, 7=i, 8=cur
        //   f64 (2): 9=ax, 10=frac
        //   i64 (2): 11=whole, 12=frac_scaled
        let mut f = WasmFunction::new([
            (8, ValType::I32),
            (2, ValType::F64),
            (2, ValType::I64),
        ]);

        // is_neg = (x < 0.0)
        f.instruction(&Instruction::LocalGet(0));
        f.instruction(&Instruction::F64Const(0.0));
        f.instruction(&Instruction::F64Lt);
        f.instruction(&Instruction::LocalSet(1));

        // ax = abs(x)
        f.instruction(&Instruction::LocalGet(0));
        f.instruction(&Instruction::F64Abs);
        f.instruction(&Instruction::LocalTee(9));

        // whole = i64.trunc_sat_f64_s(ax)
        f.instruction(&Instruction::I64TruncSatF64S);
        f.instruction(&Instruction::LocalSet(11));

        // frac = ax - f64(whole)
        f.instruction(&Instruction::LocalGet(9));
        f.instruction(&Instruction::LocalGet(11));
        f.instruction(&Instruction::F64ConvertI64S);
        f.instruction(&Instruction::F64Sub);
        f.instruction(&Instruction::LocalTee(10));

        // frac_scaled = i64.trunc_sat_f64_s(frac * 1_000_000.0 + 0.5)
        f.instruction(&Instruction::F64Const(1_000_000.0));
        f.instruction(&Instruction::F64Mul);
        f.instruction(&Instruction::F64Const(0.5));
        f.instruction(&Instruction::F64Add);
        f.instruction(&Instruction::I64TruncSatF64S);
        f.instruction(&Instruction::LocalTee(12));

        // if frac_scaled >= 1_000_000 { whole += 1; frac_scaled = 0; }
        f.instruction(&Instruction::I64Const(1_000_000));
        f.instruction(&Instruction::I64GeS);
        f.instruction(&Instruction::If(BlockType::Empty));
        f.instruction(&Instruction::LocalGet(11));
        f.instruction(&Instruction::I64Const(1));
        f.instruction(&Instruction::I64Add);
        f.instruction(&Instruction::LocalSet(11));
        f.instruction(&Instruction::I64Const(0));
        f.instruction(&Instruction::LocalSet(12));
        f.instruction(&Instruction::End);

        // If exact integer (frac_scaled == 0), delegate to int_to_str
        f.instruction(&Instruction::LocalGet(12));
        f.instruction(&Instruction::I64Eqz);
        f.instruction(&Instruction::If(BlockType::Empty));
        f.instruction(&Instruction::LocalGet(1));
        f.instruction(&Instruction::If(BlockType::Empty));
        f.instruction(&Instruction::I64Const(0));
        f.instruction(&Instruction::LocalGet(11));
        f.instruction(&Instruction::I64Sub);
        f.instruction(&Instruction::Call(int_to_str_idx));
        f.instruction(&Instruction::Return);
        f.instruction(&Instruction::Else);
        f.instruction(&Instruction::LocalGet(11));
        f.instruction(&Instruction::Call(int_to_str_idx));
        f.instruction(&Instruction::Return);
        f.instruction(&Instruction::End);
        f.instruction(&Instruction::End);

        // Allocate 80-byte scratch+result block:
        // [0..24) rev int digits, [24..32) rev frac digits, [32..40) len, [40..80) final str
        f.instruction(&Instruction::I32Const(80));
        f.instruction(&Instruction::Call(alloc_idx));
        f.instruction(&Instruction::I32WrapI64);
        f.instruction(&Instruction::LocalSet(2));

        // Integer digits
        f.instruction(&Instruction::LocalGet(11));
        f.instruction(&Instruction::I64Eqz);
        f.instruction(&Instruction::If(BlockType::Empty));
        f.instruction(&Instruction::LocalGet(2));
        f.instruction(&Instruction::I32Const(48));
        f.instruction(&Instruction::I32Store8(MemArg { offset: 0, align: 0, memory_index: 0 }));
        f.instruction(&Instruction::I32Const(1));
        f.instruction(&Instruction::LocalSet(3));
        f.instruction(&Instruction::Else);
        f.instruction(&Instruction::I32Const(0));
        f.instruction(&Instruction::LocalSet(3));
        f.instruction(&Instruction::Block(BlockType::Empty));
        f.instruction(&Instruction::Loop(BlockType::Empty));
        f.instruction(&Instruction::LocalGet(11));
        f.instruction(&Instruction::I64Eqz);
        f.instruction(&Instruction::BrIf(1));
        f.instruction(&Instruction::LocalGet(2));
        f.instruction(&Instruction::LocalGet(3));
        f.instruction(&Instruction::I32Add);
        f.instruction(&Instruction::LocalGet(11));
        f.instruction(&Instruction::I64Const(10));
        f.instruction(&Instruction::I64RemU);
        f.instruction(&Instruction::I32WrapI64);
        f.instruction(&Instruction::I32Const(48));
        f.instruction(&Instruction::I32Add);
        f.instruction(&Instruction::I32Store8(MemArg { offset: 0, align: 0, memory_index: 0 }));
        f.instruction(&Instruction::LocalGet(11));
        f.instruction(&Instruction::I64Const(10));
        f.instruction(&Instruction::I64DivU);
        f.instruction(&Instruction::LocalSet(11));
        f.instruction(&Instruction::LocalGet(3));
        f.instruction(&Instruction::I32Const(1));
        f.instruction(&Instruction::I32Add);
        f.instruction(&Instruction::LocalSet(3));
        f.instruction(&Instruction::Br(0));
        f.instruction(&Instruction::End);
        f.instruction(&Instruction::End);
        f.instruction(&Instruction::End);

        // Extract 6 fractional digits reversed into base[24..30]
        f.instruction(&Instruction::I32Const(0));
        f.instruction(&Instruction::LocalSet(7));
        f.instruction(&Instruction::Block(BlockType::Empty));
        f.instruction(&Instruction::Loop(BlockType::Empty));
        f.instruction(&Instruction::LocalGet(7));
        f.instruction(&Instruction::I32Const(6));
        f.instruction(&Instruction::I32GeU);
        f.instruction(&Instruction::BrIf(1));
        f.instruction(&Instruction::LocalGet(2));
        f.instruction(&Instruction::I32Const(24));
        f.instruction(&Instruction::I32Add);
        f.instruction(&Instruction::LocalGet(7));
        f.instruction(&Instruction::I32Add);
        f.instruction(&Instruction::LocalGet(12));
        f.instruction(&Instruction::I64Const(10));
        f.instruction(&Instruction::I64RemU);
        f.instruction(&Instruction::I32WrapI64);
        f.instruction(&Instruction::I32Const(48));
        f.instruction(&Instruction::I32Add);
        f.instruction(&Instruction::I32Store8(MemArg { offset: 0, align: 0, memory_index: 0 }));
        f.instruction(&Instruction::LocalGet(12));
        f.instruction(&Instruction::I64Const(10));
        f.instruction(&Instruction::I64DivU);
        f.instruction(&Instruction::LocalSet(12));
        f.instruction(&Instruction::LocalGet(7));
        f.instruction(&Instruction::I32Const(1));
        f.instruction(&Instruction::I32Add);
        f.instruction(&Instruction::LocalSet(7));
        f.instruction(&Instruction::Br(0));
        f.instruction(&Instruction::End);
        f.instruction(&Instruction::End);

        // Strip trailing zeros: fdigits = 6; i = 0; while fdigits > 1 && base[24+i] == '0' { i++; fdigits--; }
        f.instruction(&Instruction::I32Const(6));
        f.instruction(&Instruction::LocalSet(4));
        f.instruction(&Instruction::I32Const(0));
        f.instruction(&Instruction::LocalSet(7));
        f.instruction(&Instruction::Block(BlockType::Empty));
        f.instruction(&Instruction::Loop(BlockType::Empty));
        f.instruction(&Instruction::LocalGet(4));
        f.instruction(&Instruction::I32Const(1));
        f.instruction(&Instruction::I32LeU);
        f.instruction(&Instruction::BrIf(1));
        f.instruction(&Instruction::LocalGet(2));
        f.instruction(&Instruction::I32Const(24));
        f.instruction(&Instruction::I32Add);
        f.instruction(&Instruction::LocalGet(7));
        f.instruction(&Instruction::I32Add);
        f.instruction(&Instruction::I32Load8U(MemArg { offset: 0, align: 0, memory_index: 0 }));
        f.instruction(&Instruction::I32Const(48));
        f.instruction(&Instruction::I32Ne);
        f.instruction(&Instruction::BrIf(1));
        f.instruction(&Instruction::LocalGet(7));
        f.instruction(&Instruction::I32Const(1));
        f.instruction(&Instruction::I32Add);
        f.instruction(&Instruction::LocalSet(7));
        f.instruction(&Instruction::LocalGet(4));
        f.instruction(&Instruction::I32Const(1));
        f.instruction(&Instruction::I32Sub);
        f.instruction(&Instruction::LocalSet(4));
        f.instruction(&Instruction::Br(0));
        f.instruction(&Instruction::End);
        f.instruction(&Instruction::End);

        // len = is_neg + pos + 1 + fdigits
        f.instruction(&Instruction::LocalGet(1));
        f.instruction(&Instruction::LocalGet(3));
        f.instruction(&Instruction::I32Add);
        f.instruction(&Instruction::I32Const(1));
        f.instruction(&Instruction::I32Add);
        f.instruction(&Instruction::LocalGet(4));
        f.instruction(&Instruction::I32Add);
        f.instruction(&Instruction::LocalSet(5));

        // store len at base + 32
        f.instruction(&Instruction::LocalGet(2));
        f.instruction(&Instruction::LocalGet(5));
        f.instruction(&Instruction::I64ExtendI32U);
        f.instruction(&Instruction::I64Store(MemArg { offset: 32, align: 3, memory_index: 0 }));

        // dst = base + 40; cur = 0
        f.instruction(&Instruction::LocalGet(2));
        f.instruction(&Instruction::I32Const(40));
        f.instruction(&Instruction::I32Add);
        f.instruction(&Instruction::LocalSet(6));
        f.instruction(&Instruction::I32Const(0));
        f.instruction(&Instruction::LocalSet(8));

        // Optional '-'
        f.instruction(&Instruction::LocalGet(1));
        f.instruction(&Instruction::If(BlockType::Empty));
        f.instruction(&Instruction::LocalGet(6));
        f.instruction(&Instruction::I32Const(45));
        f.instruction(&Instruction::I32Store8(MemArg { offset: 0, align: 0, memory_index: 0 }));
        f.instruction(&Instruction::I32Const(1));
        f.instruction(&Instruction::LocalSet(8));
        f.instruction(&Instruction::End);

        // Copy integer digits forward: dst[cur] = base[pos - 1 - i]
        f.instruction(&Instruction::I32Const(0));
        f.instruction(&Instruction::LocalSet(7));
        f.instruction(&Instruction::Block(BlockType::Empty));
        f.instruction(&Instruction::Loop(BlockType::Empty));
        f.instruction(&Instruction::LocalGet(7));
        f.instruction(&Instruction::LocalGet(3));
        f.instruction(&Instruction::I32GeU);
        f.instruction(&Instruction::BrIf(1));
        f.instruction(&Instruction::LocalGet(6));
        f.instruction(&Instruction::LocalGet(8));
        f.instruction(&Instruction::I32Add);
        f.instruction(&Instruction::LocalGet(2));
        f.instruction(&Instruction::LocalGet(3));
        f.instruction(&Instruction::I32Const(1));
        f.instruction(&Instruction::I32Sub);
        f.instruction(&Instruction::LocalGet(7));
        f.instruction(&Instruction::I32Sub);
        f.instruction(&Instruction::I32Add);
        f.instruction(&Instruction::I32Load8U(MemArg { offset: 0, align: 0, memory_index: 0 }));
        f.instruction(&Instruction::I32Store8(MemArg { offset: 0, align: 0, memory_index: 0 }));
        f.instruction(&Instruction::LocalGet(8));
        f.instruction(&Instruction::I32Const(1));
        f.instruction(&Instruction::I32Add);
        f.instruction(&Instruction::LocalSet(8));
        f.instruction(&Instruction::LocalGet(7));
        f.instruction(&Instruction::I32Const(1));
        f.instruction(&Instruction::I32Add);
        f.instruction(&Instruction::LocalSet(7));
        f.instruction(&Instruction::Br(0));
        f.instruction(&Instruction::End);
        f.instruction(&Instruction::End);

        // Write '.'
        f.instruction(&Instruction::LocalGet(6));
        f.instruction(&Instruction::LocalGet(8));
        f.instruction(&Instruction::I32Add);
        f.instruction(&Instruction::I32Const(46));
        f.instruction(&Instruction::I32Store8(MemArg { offset: 0, align: 0, memory_index: 0 }));
        f.instruction(&Instruction::LocalGet(8));
        f.instruction(&Instruction::I32Const(1));
        f.instruction(&Instruction::I32Add);
        f.instruction(&Instruction::LocalSet(8));

        // Copy fractional digits forward: dst[cur] = base[24 + 5 - i]
        f.instruction(&Instruction::I32Const(0));
        f.instruction(&Instruction::LocalSet(7));
        f.instruction(&Instruction::Block(BlockType::Empty));
        f.instruction(&Instruction::Loop(BlockType::Empty));
        f.instruction(&Instruction::LocalGet(7));
        f.instruction(&Instruction::LocalGet(4));
        f.instruction(&Instruction::I32GeU);
        f.instruction(&Instruction::BrIf(1));
        f.instruction(&Instruction::LocalGet(6));
        f.instruction(&Instruction::LocalGet(8));
        f.instruction(&Instruction::I32Add);
        f.instruction(&Instruction::LocalGet(2));
        f.instruction(&Instruction::I32Const(29)); // 24 + 5
        f.instruction(&Instruction::LocalGet(7));
        f.instruction(&Instruction::I32Sub);
        f.instruction(&Instruction::I32Add);
        f.instruction(&Instruction::I32Load8U(MemArg { offset: 0, align: 0, memory_index: 0 }));
        f.instruction(&Instruction::I32Store8(MemArg { offset: 0, align: 0, memory_index: 0 }));
        f.instruction(&Instruction::LocalGet(8));
        f.instruction(&Instruction::I32Const(1));
        f.instruction(&Instruction::I32Add);
        f.instruction(&Instruction::LocalSet(8));
        f.instruction(&Instruction::LocalGet(7));
        f.instruction(&Instruction::I32Const(1));
        f.instruction(&Instruction::I32Add);
        f.instruction(&Instruction::LocalSet(7));
        f.instruction(&Instruction::Br(0));
        f.instruction(&Instruction::End);
        f.instruction(&Instruction::End);

        // Null terminator
        f.instruction(&Instruction::LocalGet(6));
        f.instruction(&Instruction::LocalGet(8));
        f.instruction(&Instruction::I32Add);
        f.instruction(&Instruction::I32Const(0));
        f.instruction(&Instruction::I32Store8(MemArg { offset: 0, align: 0, memory_index: 0 }));

        f.instruction(&Instruction::LocalGet(6));
        f.instruction(&Instruction::I64ExtendI32U);
        f.instruction(&Instruction::End);
        f
    }

    // ========================================================================
    // Function Compilation
    // ========================================================================

    fn compile_func_body(
        func: &Function,
        schemas: &[Schema],
        import_indices: &HashMap<&'static str, u32>,
        func_indices: &HashMap<String, u32>,
        builtins: &BuiltinIndices,
        string_pool: &StringPool,
    ) -> Result<WasmFunction, CodegenError> {
        let mut reg_types: HashMap<RegId, Type> = HashMap::new();
        let mut reg_to_local: HashMap<RegId, u32> = HashMap::new();
        let block_params_map: HashMap<BlockId, Vec<RegId>> = func
            .blocks
            .iter()
            .map(|b| (b.id, b.params.iter().map(|(r, _)| *r).collect()))
            .collect();

        // 1. Assign local indices for function parameters (skipping Void params consistently with signature)
        let mut param_count = 0u32;
        for (reg, ty) in &func.params {
            reg_types.insert(*reg, ty.clone());
            if Self::type_to_wasm(ty).is_some() {
                reg_to_local.insert(*reg, param_count);
                param_count += 1;
            }
        }

        // 2. Discover all non-param registers (block params + instruction dsts)
        for block in &func.blocks {
            for (preg, pty) in &block.params {
                reg_types.insert(*preg, pty.clone());
            }
            for inst in &block.instructions {
                if let Some(dst) = inst.dst {
                    reg_types.insert(dst, inst.ty.clone());
                }
            }
        }

        let mut non_param_i64: Vec<RegId> = Vec::new();
        let mut non_param_f64: Vec<RegId> = Vec::new();
        let mut non_param_i32: Vec<RegId> = Vec::new();

        for (&reg, ty) in &reg_types {
            if func.params.iter().any(|(p, _)| *p == reg) {
                continue;
            }
            match Self::type_to_wasm(ty) {
                Some(ValType::I64) => non_param_i64.push(reg),
                Some(ValType::F64) => non_param_f64.push(reg),
                Some(ValType::I32) => non_param_i32.push(reg),
                _ => {}
            }
        }

        non_param_i64.sort_unstable();
        non_param_f64.sort_unstable();
        non_param_i32.sort_unstable();

        let mut next_local = param_count;
        for &reg in &non_param_i64 {
            reg_to_local.insert(reg, next_local);
            next_local += 1;
        }
        for &reg in &non_param_f64 {
            reg_to_local.insert(reg, next_local);
            next_local += 1;
        }
        for &reg in &non_param_i32 {
            reg_to_local.insert(reg, next_local);
            next_local += 1;
        }

        let needs_dispatch = func.blocks.len() > 1
            || func.blocks.iter().any(|b| {
                matches!(
                    b.terminator,
                    Terminator::Br(_)
                        | Terminator::BrIf { .. }
                        | Terminator::BrArgs { .. }
                        | Terminator::BrIfArgs { .. }
                )
            });
        let state_local = if needs_dispatch {
            Some(next_local)
        } else {
            None
        };

        let mut locals_decl = Vec::new();
        if !non_param_i64.is_empty() {
            locals_decl.push((non_param_i64.len() as u32, ValType::I64));
        }
        if !non_param_f64.is_empty() {
            locals_decl.push((non_param_f64.len() as u32, ValType::F64));
        }
        let total_i32 = non_param_i32.len() as u32 + if needs_dispatch { 1 } else { 0 };
        if total_i32 > 0 {
            locals_decl.push((total_i32, ValType::I32));
        }

        let mut wasm_fn = WasmFunction::new(locals_decl);

        if !needs_dispatch {
            if let Some(block) = func.blocks.first() {
                Self::emit_block_instructions(
                    block,
                    schemas,
                    &mut wasm_fn,
                    &reg_to_local,
                    &reg_types,
                    import_indices,
                    func_indices,
                    builtins,
                    string_pool,
                )?;
                Self::emit_single_block_terminator(&block.terminator, &mut wasm_fn, &reg_to_local);
            } else {
                wasm_fn.instruction(&Instruction::Return);
            }
        } else {
            let state_var = state_local.unwrap();
            let first_block_id = func.blocks[0].id;

            wasm_fn.instruction(&Instruction::I32Const(first_block_id as i32));
            wasm_fn.instruction(&Instruction::LocalSet(state_var));

            wasm_fn.instruction(&Instruction::Loop(BlockType::Empty));

            for block in &func.blocks {
                wasm_fn.instruction(&Instruction::LocalGet(state_var));
                wasm_fn.instruction(&Instruction::I32Const(block.id as i32));
                wasm_fn.instruction(&Instruction::I32Eq);
                wasm_fn.instruction(&Instruction::If(BlockType::Empty));

                Self::emit_block_instructions(
                    block,
                    schemas,
                    &mut wasm_fn,
                    &reg_to_local,
                    &reg_types,
                    import_indices,
                    func_indices,
                    builtins,
                    string_pool,
                )?;

                match &block.terminator {
                    Terminator::Ret(Some(r)) => {
                        let loc = reg_to_local[r];
                        wasm_fn.instruction(&Instruction::LocalGet(loc));
                        wasm_fn.instruction(&Instruction::Return);
                    }
                    Terminator::Ret(None) => {
                        wasm_fn.instruction(&Instruction::Return);
                    }
                    Terminator::Br(target) => {
                        wasm_fn.instruction(&Instruction::I32Const(*target as i32));
                        wasm_fn.instruction(&Instruction::LocalSet(state_var));
                        wasm_fn.instruction(&Instruction::Br(1));
                    }
                    Terminator::BrArgs { target, args } => {
                        Self::emit_parallel_param_copy(
                            *target,
                            args,
                            &block_params_map,
                            &reg_to_local,
                            &mut wasm_fn,
                        );
                        wasm_fn.instruction(&Instruction::I32Const(*target as i32));
                        wasm_fn.instruction(&Instruction::LocalSet(state_var));
                        wasm_fn.instruction(&Instruction::Br(1));
                    }
                    Terminator::BrIf {
                        cond,
                        then_blk,
                        else_blk,
                    } => {
                        let c_loc = reg_to_local[cond];
                        wasm_fn.instruction(&Instruction::LocalGet(c_loc));
                        wasm_fn.instruction(&Instruction::If(BlockType::Empty));
                        wasm_fn.instruction(&Instruction::I32Const(*then_blk as i32));
                        wasm_fn.instruction(&Instruction::LocalSet(state_var));
                        wasm_fn.instruction(&Instruction::Else);
                        wasm_fn.instruction(&Instruction::I32Const(*else_blk as i32));
                        wasm_fn.instruction(&Instruction::LocalSet(state_var));
                        wasm_fn.instruction(&Instruction::End);
                        wasm_fn.instruction(&Instruction::Br(1));
                    }
                    Terminator::BrIfArgs {
                        cond,
                        then_blk,
                        then_args,
                        else_blk,
                        else_args,
                    } => {
                        let c_loc = reg_to_local[cond];
                        wasm_fn.instruction(&Instruction::LocalGet(c_loc));
                        wasm_fn.instruction(&Instruction::If(BlockType::Empty));
                        Self::emit_parallel_param_copy(
                            *then_blk,
                            then_args,
                            &block_params_map,
                            &reg_to_local,
                            &mut wasm_fn,
                        );
                        wasm_fn.instruction(&Instruction::I32Const(*then_blk as i32));
                        wasm_fn.instruction(&Instruction::LocalSet(state_var));
                        wasm_fn.instruction(&Instruction::Else);
                        Self::emit_parallel_param_copy(
                            *else_blk,
                            else_args,
                            &block_params_map,
                            &reg_to_local,
                            &mut wasm_fn,
                        );
                        wasm_fn.instruction(&Instruction::I32Const(*else_blk as i32));
                        wasm_fn.instruction(&Instruction::LocalSet(state_var));
                        wasm_fn.instruction(&Instruction::End);
                        wasm_fn.instruction(&Instruction::Br(1));
                    }
                }

                wasm_fn.instruction(&Instruction::End);
            }

            wasm_fn.instruction(&Instruction::Br(0));
            wasm_fn.instruction(&Instruction::End);

            if func.ret_ty != Type::Void {
                wasm_fn.instruction(&Instruction::Unreachable);
            }
        }

        wasm_fn.instruction(&Instruction::End);
        Ok(wasm_fn)
    }

    /// Parallel-copies branch `args` into `target` block parameters via the Wasm operand stack
    /// (pushes all args first, then pops in reverse order so swaps/self-references never clobber).
    fn emit_parallel_param_copy(
        target: BlockId,
        args: &[RegId],
        block_params_map: &HashMap<BlockId, Vec<RegId>>,
        reg_to_local: &HashMap<RegId, u32>,
        wasm_fn: &mut WasmFunction,
    ) {
        if args.is_empty() {
            return;
        }
        if let Some(params) = block_params_map.get(&target) {
            let count = args.len().min(params.len());
            let mut active_params: Vec<u32> = Vec::with_capacity(count);
            for (arg, param) in args[..count].iter().zip(&params[..count]) {
                if let (Some(&arg_loc), Some(&param_loc)) =
                    (reg_to_local.get(arg), reg_to_local.get(param))
                {
                    wasm_fn.instruction(&Instruction::LocalGet(arg_loc));
                    active_params.push(param_loc);
                }
            }
            for param_loc in active_params.into_iter().rev() {
                wasm_fn.instruction(&Instruction::LocalSet(param_loc));
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn emit_block_instructions(
        block: &BasicBlock,
        schemas: &[Schema],
        wasm_fn: &mut WasmFunction,
        reg_to_local: &HashMap<RegId, u32>,
        reg_types: &HashMap<RegId, Type>,
        import_indices: &HashMap<&'static str, u32>,
        func_indices: &HashMap<String, u32>,
        builtins: &BuiltinIndices,
        string_pool: &StringPool,
    ) -> Result<(), CodegenError> {
        for inst in &block.instructions {
            Self::emit_instruction(
                inst,
                schemas,
                wasm_fn,
                reg_to_local,
                reg_types,
                import_indices,
                func_indices,
                builtins,
                string_pool,
            )?;
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn emit_instruction(
        inst: &KnrInst,
        schemas: &[Schema],
        wasm_fn: &mut WasmFunction,
        reg_to_local: &HashMap<RegId, u32>,
        reg_types: &HashMap<RegId, Type>,
        import_indices: &HashMap<&'static str, u32>,
        func_indices: &HashMap<String, u32>,
        builtins: &BuiltinIndices,
        string_pool: &StringPool,
    ) -> Result<(), CodegenError> {
        let dst_loc = inst.dst.and_then(|d| reg_to_local.get(&d).copied());

        match &inst.op {
            Op::ConstI64(val) => {
                wasm_fn.instruction(&Instruction::I64Const(*val));
                if let Some(dst) = dst_loc {
                    wasm_fn.instruction(&Instruction::LocalSet(dst));
                }
            }
            Op::ConstF64(val) => {
                wasm_fn.instruction(&Instruction::F64Const(*val));
                if let Some(dst) = dst_loc {
                    wasm_fn.instruction(&Instruction::LocalSet(dst));
                }
            }
            Op::ConstBool(val) => {
                wasm_fn.instruction(&Instruction::I32Const(if *val { 1 } else { 0 }));
                if let Some(dst) = dst_loc {
                    wasm_fn.instruction(&Instruction::LocalSet(dst));
                }
            }
            Op::ConstStr(s) => {
                let offset = string_pool.get_offset(s);
                wasm_fn.instruction(&Instruction::I64Const(offset as i64));
                if let Some(dst) = dst_loc {
                    wasm_fn.instruction(&Instruction::LocalSet(dst));
                }
            }
            Op::Add(a, b) => {
                wasm_fn.instruction(&Instruction::LocalGet(reg_to_local[a]));
                wasm_fn.instruction(&Instruction::LocalGet(reg_to_local[b]));
                if inst.ty == Type::F64 {
                    wasm_fn.instruction(&Instruction::F64Add);
                } else if inst.ty == Type::Str {
                    wasm_fn.instruction(&Instruction::Call(builtins.str_concat.unwrap()));
                } else {
                    wasm_fn.instruction(&Instruction::I64Add);
                }
                if let Some(dst) = dst_loc {
                    wasm_fn.instruction(&Instruction::LocalSet(dst));
                }
            }
            Op::Sub(a, b) => {
                wasm_fn.instruction(&Instruction::LocalGet(reg_to_local[a]));
                wasm_fn.instruction(&Instruction::LocalGet(reg_to_local[b]));
                if inst.ty == Type::F64 {
                    wasm_fn.instruction(&Instruction::F64Sub);
                } else {
                    wasm_fn.instruction(&Instruction::I64Sub);
                }
                if let Some(dst) = dst_loc {
                    wasm_fn.instruction(&Instruction::LocalSet(dst));
                }
            }
            Op::Mul(a, b) => {
                wasm_fn.instruction(&Instruction::LocalGet(reg_to_local[a]));
                wasm_fn.instruction(&Instruction::LocalGet(reg_to_local[b]));
                if inst.ty == Type::F64 {
                    wasm_fn.instruction(&Instruction::F64Mul);
                } else {
                    wasm_fn.instruction(&Instruction::I64Mul);
                }
                if let Some(dst) = dst_loc {
                    wasm_fn.instruction(&Instruction::LocalSet(dst));
                }
            }
            Op::Div(a, b) => {
                wasm_fn.instruction(&Instruction::LocalGet(reg_to_local[a]));
                wasm_fn.instruction(&Instruction::LocalGet(reg_to_local[b]));
                if inst.ty == Type::F64 {
                    wasm_fn.instruction(&Instruction::F64Div);
                } else {
                    wasm_fn.instruction(&Instruction::I64DivS);
                }
                if let Some(dst) = dst_loc {
                    wasm_fn.instruction(&Instruction::LocalSet(dst));
                }
            }
            Op::Rem(a, b) => {
                if inst.ty == Type::F64 {
                    // IEEE-754 truncated remainder: a - trunc(a / b) * b
                    wasm_fn.instruction(&Instruction::LocalGet(reg_to_local[a]));
                    wasm_fn.instruction(&Instruction::LocalGet(reg_to_local[a]));
                    wasm_fn.instruction(&Instruction::LocalGet(reg_to_local[b]));
                    wasm_fn.instruction(&Instruction::F64Div);
                    wasm_fn.instruction(&Instruction::F64Trunc);
                    wasm_fn.instruction(&Instruction::LocalGet(reg_to_local[b]));
                    wasm_fn.instruction(&Instruction::F64Mul);
                    wasm_fn.instruction(&Instruction::F64Sub);
                } else {
                    wasm_fn.instruction(&Instruction::LocalGet(reg_to_local[a]));
                    wasm_fn.instruction(&Instruction::LocalGet(reg_to_local[b]));
                    wasm_fn.instruction(&Instruction::I64RemS);
                }
                if let Some(dst) = dst_loc {
                    wasm_fn.instruction(&Instruction::LocalSet(dst));
                }
            }
            Op::CmpEq(a, b) => {
                let ty_a = reg_types.get(a).unwrap_or(&Type::I64);
                match ty_a {
                    Type::Str => {
                        wasm_fn.instruction(&Instruction::LocalGet(reg_to_local[a]));
                        wasm_fn.instruction(&Instruction::LocalGet(reg_to_local[b]));
                        wasm_fn.instruction(&Instruction::Call(builtins.str_eq.unwrap()));
                    }
                    Type::F64 => {
                        wasm_fn.instruction(&Instruction::LocalGet(reg_to_local[a]));
                        wasm_fn.instruction(&Instruction::LocalGet(reg_to_local[b]));
                        wasm_fn.instruction(&Instruction::F64Eq);
                    }
                    Type::Bool => {
                        wasm_fn.instruction(&Instruction::LocalGet(reg_to_local[a]));
                        wasm_fn.instruction(&Instruction::LocalGet(reg_to_local[b]));
                        wasm_fn.instruction(&Instruction::I32Eq);
                    }
                    Type::Struct(sname) => {
                        Self::emit_wasm_struct_eq(
                            reg_to_local[a],
                            reg_to_local[b],
                            sname,
                            schemas,
                            builtins,
                            wasm_fn,
                        );
                    }
                    _ => {
                        wasm_fn.instruction(&Instruction::LocalGet(reg_to_local[a]));
                        wasm_fn.instruction(&Instruction::LocalGet(reg_to_local[b]));
                        wasm_fn.instruction(&Instruction::I64Eq);
                    }
                }
                if let Some(dst) = dst_loc {
                    wasm_fn.instruction(&Instruction::LocalSet(dst));
                }
            }
            Op::CmpLt(a, b) => {
                wasm_fn.instruction(&Instruction::LocalGet(reg_to_local[a]));
                wasm_fn.instruction(&Instruction::LocalGet(reg_to_local[b]));
                let ty_a = reg_types.get(a).unwrap_or(&Type::I64);
                if *ty_a == Type::F64 {
                    wasm_fn.instruction(&Instruction::F64Lt);
                } else {
                    wasm_fn.instruction(&Instruction::I64LtS);
                }
                if let Some(dst) = dst_loc {
                    wasm_fn.instruction(&Instruction::LocalSet(dst));
                }
            }
            Op::IntToStr(a) => {
                let ty_a = reg_types.get(a).unwrap_or(&Type::I64);
                Self::emit_wasm_to_str(reg_to_local[a], ty_a, builtins, string_pool, wasm_fn);
                if let Some(dst) = dst_loc {
                    wasm_fn.instruction(&Instruction::LocalSet(dst));
                }
            }
            Op::StrLen(a) => {
                wasm_fn.instruction(&Instruction::LocalGet(reg_to_local[a]));
                wasm_fn.instruction(&Instruction::Call(builtins.str_len.unwrap()));
                if let Some(dst) = dst_loc {
                    wasm_fn.instruction(&Instruction::LocalSet(dst));
                }
            }
            Op::Sin(a) => {
                wasm_fn.instruction(&Instruction::LocalGet(reg_to_local[a]));
                wasm_fn.instruction(&Instruction::Call(builtins.math_sin.unwrap()));
                if let Some(dst) = dst_loc {
                    wasm_fn.instruction(&Instruction::LocalSet(dst));
                }
            }
            Op::Cos(a) => {
                wasm_fn.instruction(&Instruction::LocalGet(reg_to_local[a]));
                wasm_fn.instruction(&Instruction::Call(builtins.math_cos.unwrap()));
                if let Some(dst) = dst_loc {
                    wasm_fn.instruction(&Instruction::LocalSet(dst));
                }
            }
            Op::Tan(a) => {
                wasm_fn.instruction(&Instruction::LocalGet(reg_to_local[a]));
                wasm_fn.instruction(&Instruction::Call(builtins.math_tan.unwrap()));
                if let Some(dst) = dst_loc {
                    wasm_fn.instruction(&Instruction::LocalSet(dst));
                }
            }
            Op::Sqrt(a) => {
                wasm_fn.instruction(&Instruction::LocalGet(reg_to_local[a]));
                wasm_fn.instruction(&Instruction::F64Sqrt);
                if let Some(dst) = dst_loc {
                    wasm_fn.instruction(&Instruction::LocalSet(dst));
                }
            }
            Op::Abs(a) => {
                wasm_fn.instruction(&Instruction::LocalGet(reg_to_local[a]));
                wasm_fn.instruction(&Instruction::F64Abs);
                if let Some(dst) = dst_loc {
                    wasm_fn.instruction(&Instruction::LocalSet(dst));
                }
            }
            Op::Floor(a) => {
                wasm_fn.instruction(&Instruction::LocalGet(reg_to_local[a]));
                wasm_fn.instruction(&Instruction::F64Floor);
                if let Some(dst) = dst_loc {
                    wasm_fn.instruction(&Instruction::LocalSet(dst));
                }
            }
            Op::Ceil(a) => {
                wasm_fn.instruction(&Instruction::LocalGet(reg_to_local[a]));
                wasm_fn.instruction(&Instruction::F64Ceil);
                if let Some(dst) = dst_loc {
                    wasm_fn.instruction(&Instruction::LocalSet(dst));
                }
            }
            Op::Pow(a, b) => {
                wasm_fn.instruction(&Instruction::LocalGet(reg_to_local[a]));
                wasm_fn.instruction(&Instruction::LocalGet(reg_to_local[b]));
                wasm_fn.instruction(&Instruction::Call(builtins.math_pow.unwrap()));
                if let Some(dst) = dst_loc {
                    wasm_fn.instruction(&Instruction::LocalSet(dst));
                }
            }
            Op::Atan2(y, x) => {
                wasm_fn.instruction(&Instruction::LocalGet(reg_to_local[y]));
                wasm_fn.instruction(&Instruction::LocalGet(reg_to_local[x]));
                wasm_fn.instruction(&Instruction::Call(builtins.math_atan2.unwrap()));
                if let Some(dst) = dst_loc {
                    wasm_fn.instruction(&Instruction::LocalSet(dst));
                }
            }
            Op::Min(a, b) => {
                wasm_fn.instruction(&Instruction::LocalGet(reg_to_local[a]));
                wasm_fn.instruction(&Instruction::LocalGet(reg_to_local[b]));
                wasm_fn.instruction(&Instruction::F64Min);
                if let Some(dst) = dst_loc {
                    wasm_fn.instruction(&Instruction::LocalSet(dst));
                }
            }
            Op::Max(a, b) => {
                wasm_fn.instruction(&Instruction::LocalGet(reg_to_local[a]));
                wasm_fn.instruction(&Instruction::LocalGet(reg_to_local[b]));
                wasm_fn.instruction(&Instruction::F64Max);
                if let Some(dst) = dst_loc {
                    wasm_fn.instruction(&Instruction::LocalSet(dst));
                }
            }
            Op::IntToFloat(a) => {
                wasm_fn.instruction(&Instruction::LocalGet(reg_to_local[a]));
                wasm_fn.instruction(&Instruction::F64ConvertI64S);
                if let Some(dst) = dst_loc {
                    wasm_fn.instruction(&Instruction::LocalSet(dst));
                }
            }
            Op::FloatToInt(a) => {
                wasm_fn.instruction(&Instruction::LocalGet(reg_to_local[a]));
                wasm_fn.instruction(&Instruction::I64TruncSatF64S);
                if let Some(dst) = dst_loc {
                    wasm_fn.instruction(&Instruction::LocalSet(dst));
                }
            }
            Op::BufAlloc(size) => {
                let call_idx = builtins.buf_alloc.unwrap();
                wasm_fn.instruction(&Instruction::LocalGet(reg_to_local[size]));
                wasm_fn.instruction(&Instruction::Call(call_idx));
                if let Some(dst) = dst_loc {
                    wasm_fn.instruction(&Instruction::LocalSet(dst));
                }
            }
            Op::BufReadU8(buf, idx) => {
                wasm_fn.instruction(&Instruction::LocalGet(reg_to_local[buf]));
                wasm_fn.instruction(&Instruction::I32WrapI64);
                wasm_fn.instruction(&Instruction::LocalGet(reg_to_local[idx]));
                wasm_fn.instruction(&Instruction::I32WrapI64);
                wasm_fn.instruction(&Instruction::I32Add);
                wasm_fn.instruction(&Instruction::I32Load8U(MemArg {
                    offset: 0,
                    align: 0,
                    memory_index: 0,
                }));
                wasm_fn.instruction(&Instruction::I64ExtendI32U);
                if let Some(dst) = dst_loc {
                    wasm_fn.instruction(&Instruction::LocalSet(dst));
                }
            }
            Op::BufWriteU8(buf, idx, val) => {
                wasm_fn.instruction(&Instruction::LocalGet(reg_to_local[buf]));
                wasm_fn.instruction(&Instruction::I32WrapI64);
                wasm_fn.instruction(&Instruction::LocalGet(reg_to_local[idx]));
                wasm_fn.instruction(&Instruction::I32WrapI64);
                wasm_fn.instruction(&Instruction::I32Add);
                wasm_fn.instruction(&Instruction::LocalGet(reg_to_local[val]));
                wasm_fn.instruction(&Instruction::I32WrapI64);
                wasm_fn.instruction(&Instruction::I32Store8(MemArg {
                    offset: 0,
                    align: 0,
                    memory_index: 0,
                }));
                if let Some(dst) = dst_loc {
                    wasm_fn.instruction(&Instruction::LocalGet(reg_to_local[buf]));
                    wasm_fn.instruction(&Instruction::LocalSet(dst));
                }
            }
            Op::BufSlice(buf, start, _len) => {
                wasm_fn.instruction(&Instruction::LocalGet(reg_to_local[buf]));
                wasm_fn.instruction(&Instruction::LocalGet(reg_to_local[start]));
                wasm_fn.instruction(&Instruction::I64Add);
                if let Some(dst) = dst_loc {
                    wasm_fn.instruction(&Instruction::LocalSet(dst));
                }
            }
            Op::VecNew(_) => {
                let call_idx = builtins.vec_new.unwrap();
                wasm_fn.instruction(&Instruction::Call(call_idx));
                if let Some(dst) = dst_loc {
                    wasm_fn.instruction(&Instruction::LocalSet(dst));
                }
            }
            Op::VecPush(v, val) => {
                let call_idx = builtins.vec_push.unwrap();
                wasm_fn.instruction(&Instruction::LocalGet(reg_to_local[v]));
                wasm_fn.instruction(&Instruction::LocalGet(reg_to_local[val]));
                match reg_types.get(val) {
                    Some(Type::F64) => {
                        wasm_fn.instruction(&Instruction::I64ReinterpretF64);
                    }
                    Some(Type::Bool) => {
                        wasm_fn.instruction(&Instruction::I64ExtendI32U);
                    }
                    _ => {}
                }
                wasm_fn.instruction(&Instruction::Call(call_idx));
                if let Some(dst) = dst_loc {
                    wasm_fn.instruction(&Instruction::LocalSet(dst));
                }
            }
            Op::VecGet(v, idx) => {
                let call_idx = builtins.vec_get.unwrap();
                wasm_fn.instruction(&Instruction::LocalGet(reg_to_local[v]));
                wasm_fn.instruction(&Instruction::LocalGet(reg_to_local[idx]));
                wasm_fn.instruction(&Instruction::Call(call_idx));
                match inst.ty {
                    Type::F64 => {
                        wasm_fn.instruction(&Instruction::F64ReinterpretI64);
                    }
                    Type::Bool => {
                        wasm_fn.instruction(&Instruction::I32WrapI64);
                    }
                    _ => {}
                }
                if let Some(dst) = dst_loc {
                    wasm_fn.instruction(&Instruction::LocalSet(dst));
                }
            }
            Op::VecLen(v) => {
                let call_idx = builtins.vec_len.unwrap();
                wasm_fn.instruction(&Instruction::LocalGet(reg_to_local[v]));
                wasm_fn.instruction(&Instruction::Call(call_idx));
                if let Some(dst) = dst_loc {
                    wasm_fn.instruction(&Instruction::LocalSet(dst));
                }
            }
            Op::MapNew(_, _) => {
                let call_idx = builtins.map_new.unwrap();
                wasm_fn.instruction(&Instruction::Call(call_idx));
                if let Some(dst) = dst_loc {
                    wasm_fn.instruction(&Instruction::LocalSet(dst));
                }
            }
            Op::MapSet(m, k, v) => {
                let call_idx = builtins.map_set.unwrap();
                let key_ty = reg_types.get(k).unwrap_or(&Type::Str);
                wasm_fn.instruction(&Instruction::LocalGet(reg_to_local[m]));
                Self::emit_wasm_to_str(reg_to_local[k], key_ty, builtins, string_pool, wasm_fn);
                wasm_fn.instruction(&Instruction::LocalGet(reg_to_local[v]));
                match reg_types.get(v) {
                    Some(Type::F64) => {
                        wasm_fn.instruction(&Instruction::I64ReinterpretF64);
                    }
                    Some(Type::Bool) => {
                        wasm_fn.instruction(&Instruction::I64ExtendI32U);
                    }
                    _ => {}
                }
                wasm_fn.instruction(&Instruction::Call(call_idx));
                if let Some(dst) = dst_loc {
                    wasm_fn.instruction(&Instruction::LocalSet(dst));
                }
            }
            Op::MapGet(m, k) => {
                let call_idx = builtins.map_get.unwrap();
                let key_ty = reg_types.get(k).unwrap_or(&Type::Str);
                wasm_fn.instruction(&Instruction::LocalGet(reg_to_local[m]));
                Self::emit_wasm_to_str(reg_to_local[k], key_ty, builtins, string_pool, wasm_fn);
                wasm_fn.instruction(&Instruction::Call(call_idx));
                match inst.ty {
                    Type::F64 => {
                        wasm_fn.instruction(&Instruction::F64ReinterpretI64);
                    }
                    Type::Bool => {
                        wasm_fn.instruction(&Instruction::I32WrapI64);
                    }
                    _ => {}
                }
                if let Some(dst) = dst_loc {
                    wasm_fn.instruction(&Instruction::LocalSet(dst));
                }
            }
            Op::MapDel(m, k) => {
                let call_idx = builtins.map_del.unwrap();
                let key_ty = reg_types.get(k).unwrap_or(&Type::Str);
                wasm_fn.instruction(&Instruction::LocalGet(reg_to_local[m]));
                Self::emit_wasm_to_str(reg_to_local[k], key_ty, builtins, string_pool, wasm_fn);
                wasm_fn.instruction(&Instruction::Call(call_idx));
                if let Some(dst) = dst_loc {
                    wasm_fn.instruction(&Instruction::LocalSet(dst));
                }
            }
            Op::ChanNew(_, _) => {
                if let Some(dst) = dst_loc {
                    wasm_fn.instruction(&Instruction::I64Const(1));
                    wasm_fn.instruction(&Instruction::LocalSet(dst));
                }
            }
            Op::ChanSend(_, _) => {
                if let Some(dst) = dst_loc {
                    wasm_fn.instruction(&Instruction::I32Const(1));
                    wasm_fn.instruction(&Instruction::LocalSet(dst));
                }
            }
            Op::ChanRecv(_) => {
                if let Some(dst) = dst_loc {
                    wasm_fn.instruction(&Instruction::I64Const(0));
                    wasm_fn.instruction(&Instruction::LocalSet(dst));
                }
            }
            Op::Spawn(_, _) => {
                if let Some(dst) = dst_loc {
                    wasm_fn.instruction(&Instruction::I32Const(1));
                    wasm_fn.instruction(&Instruction::LocalSet(dst));
                }
            }
            Op::Syscall(_, _, _) => {
                if let Some(dst) = dst_loc {
                    match inst.ty {
                        Type::Bool => wasm_fn.instruction(&Instruction::I32Const(1)),
                        Type::F64 => wasm_fn.instruction(&Instruction::F64Const(0.0)),
                        _ => wasm_fn.instruction(&Instruction::I64Const(0)),
                    };
                    wasm_fn.instruction(&Instruction::LocalSet(dst));
                }
            }
            Op::Free(_r) => {
                // Structs, Vecs, Maps, and Bufs all live in Wasm linear memory
                // and are reclaimed deterministically via `__knr_heap_reset`.
                let _ = import_indices;
            }
            Op::StructNew(name) => {
                let field_count = schemas
                    .iter()
                    .find(|s| s.name == *name)
                    .map(|sc| {
                        sc.fields
                            .iter()
                            .map(|f| f.index + 1)
                            .max()
                            .unwrap_or(sc.fields.len() as u32)
                    })
                    .unwrap_or(8)
                    .max(1);
                wasm_fn.instruction(&Instruction::I32Const((field_count * 8) as i32));
                wasm_fn.instruction(&Instruction::Call(builtins.alloc.unwrap()));
                if let Some(dst) = dst_loc {
                    wasm_fn.instruction(&Instruction::LocalSet(dst));
                }
            }
            Op::StructGet(s, field_idx) => {
                let byte_off = (*field_idx as u64) * 8;
                wasm_fn.instruction(&Instruction::LocalGet(reg_to_local[s]));
                wasm_fn.instruction(&Instruction::I32WrapI64);
                match inst.ty {
                    Type::F64 => {
                        wasm_fn.instruction(&Instruction::F64Load(MemArg {
                            offset: byte_off,
                            align: 3,
                            memory_index: 0,
                        }));
                    }
                    Type::Bool => {
                        wasm_fn.instruction(&Instruction::I64Load(MemArg {
                            offset: byte_off,
                            align: 3,
                            memory_index: 0,
                        }));
                        wasm_fn.instruction(&Instruction::I32WrapI64);
                    }
                    _ => {
                        wasm_fn.instruction(&Instruction::I64Load(MemArg {
                            offset: byte_off,
                            align: 3,
                            memory_index: 0,
                        }));
                    }
                }
                if let Some(dst) = dst_loc {
                    wasm_fn.instruction(&Instruction::LocalSet(dst));
                }
            }
            Op::StructSet(s, field_idx, val) => {
                let byte_off = (*field_idx as u64) * 8;
                wasm_fn.instruction(&Instruction::LocalGet(reg_to_local[s]));
                wasm_fn.instruction(&Instruction::I32WrapI64);
                wasm_fn.instruction(&Instruction::LocalGet(reg_to_local[val]));
                match reg_types.get(val) {
                    Some(Type::F64) => {
                        wasm_fn.instruction(&Instruction::F64Store(MemArg {
                            offset: byte_off,
                            align: 3,
                            memory_index: 0,
                        }));
                    }
                    Some(Type::Bool) => {
                        wasm_fn.instruction(&Instruction::I64ExtendI32U);
                        wasm_fn.instruction(&Instruction::I64Store(MemArg {
                            offset: byte_off,
                            align: 3,
                            memory_index: 0,
                        }));
                    }
                    _ => {
                        wasm_fn.instruction(&Instruction::I64Store(MemArg {
                            offset: byte_off,
                            align: 3,
                            memory_index: 0,
                        }));
                    }
                }
                if let Some(dst) = dst_loc {
                    wasm_fn.instruction(&Instruction::LocalGet(reg_to_local[s]));
                    wasm_fn.instruction(&Instruction::LocalSet(dst));
                }
            }
            Op::Call(fn_name, args) => {
                let target_idx = func_indices.get(fn_name).ok_or_else(|| {
                    CodegenError::Wasm(format!("Unknown function in call: {fn_name}"))
                })?;

                for arg in args {
                    let loc = reg_to_local.get(arg).ok_or_else(|| {
                        CodegenError::Wasm(format!(
                            "Unknown argument register %{arg} in call to {fn_name}"
                        ))
                    })?;
                    wasm_fn.instruction(&Instruction::LocalGet(*loc));
                }

                wasm_fn.instruction(&Instruction::Call(*target_idx));

                if let Some(dst) = dst_loc {
                    wasm_fn.instruction(&Instruction::LocalSet(dst));
                } else if inst.ty != Type::Void {
                    wasm_fn.instruction(&Instruction::Drop);
                }
            }
        }

        Ok(())
    }

    /// Pushes a string pointer (`i64`) representing `local_idx` of type `ty` onto the Wasm stack.
    fn emit_wasm_to_str(
        local_idx: u32,
        ty: &Type,
        builtins: &BuiltinIndices,
        string_pool: &StringPool,
        wasm_fn: &mut WasmFunction,
    ) {
        match ty {
            Type::Str => {
                wasm_fn.instruction(&Instruction::LocalGet(local_idx));
            }
            Type::Bool => {
                let true_off = string_pool.get_offset("true") as i64;
                let false_off = string_pool.get_offset("false") as i64;
                wasm_fn.instruction(&Instruction::I64Const(true_off));
                wasm_fn.instruction(&Instruction::I64Const(false_off));
                wasm_fn.instruction(&Instruction::LocalGet(local_idx));
                wasm_fn.instruction(&Instruction::Select);
            }
            Type::F64 => {
                wasm_fn.instruction(&Instruction::LocalGet(local_idx));
                wasm_fn.instruction(&Instruction::Call(builtins.f64_to_str.unwrap()));
            }
            _ => {
                wasm_fn.instruction(&Instruction::LocalGet(local_idx));
                wasm_fn.instruction(&Instruction::Call(builtins.int_to_str.unwrap()));
            }
        }
    }

    /// Emits structural field-by-field equality (`i32` 0/1) for two struct/enum pointers in linear memory.
    fn emit_wasm_struct_eq(
        loc_a: u32,
        loc_b: u32,
        schema_name: &str,
        schemas: &[Schema],
        builtins: &BuiltinIndices,
        wasm_fn: &mut WasmFunction,
    ) {
        fn emit_load_path(root: u32, path: &[u64], leaf_is_f64: bool, wasm_fn: &mut WasmFunction) {
            wasm_fn.instruction(&Instruction::LocalGet(root));
            for (idx, &off) in path.iter().enumerate() {
                wasm_fn.instruction(&Instruction::I32WrapI64);
                let is_last = idx + 1 == path.len();
                if is_last && leaf_is_f64 {
                    wasm_fn.instruction(&Instruction::F64Load(MemArg {
                        offset: off,
                        align: 3,
                        memory_index: 0,
                    }));
                } else {
                    wasm_fn.instruction(&Instruction::I64Load(MemArg {
                        offset: off,
                        align: 3,
                        memory_index: 0,
                    }));
                }
            }
        }

        #[allow(clippy::too_many_arguments)]
        fn inner(
            loc_a: u32,
            loc_b: u32,
            ty: &Type,
            path: &mut Vec<u64>,
            schemas: &[Schema],
            builtins: &BuiltinIndices,
            wasm_fn: &mut WasmFunction,
            depth: usize,
        ) {
            if depth > 16 {
                emit_load_path(loc_a, path, false, wasm_fn);
                emit_load_path(loc_b, path, false, wasm_fn);
                wasm_fn.instruction(&Instruction::I64Eq);
                return;
            }
            match ty {
                Type::Struct(sname) => {
                    if let Some(sc) = schemas.iter().find(|s| s.name == *sname) {
                        if sc.fields.is_empty() {
                            wasm_fn.instruction(&Instruction::I32Const(1));
                            return;
                        }
                        for (i, f) in sc.fields.iter().enumerate() {
                            path.push((f.index as u64) * 8);
                            inner(loc_a, loc_b, &f.ty, path, schemas, builtins, wasm_fn, depth + 1);
                            path.pop();
                            if i > 0 {
                                wasm_fn.instruction(&Instruction::I32And);
                            }
                        }
                    } else {
                        emit_load_path(loc_a, path, false, wasm_fn);
                        emit_load_path(loc_b, path, false, wasm_fn);
                        wasm_fn.instruction(&Instruction::I64Eq);
                    }
                }
                Type::F64 => {
                    emit_load_path(loc_a, path, true, wasm_fn);
                    emit_load_path(loc_b, path, true, wasm_fn);
                    wasm_fn.instruction(&Instruction::F64Eq);
                }
                Type::Str => {
                    emit_load_path(loc_a, path, false, wasm_fn);
                    emit_load_path(loc_b, path, false, wasm_fn);
                    wasm_fn.instruction(&Instruction::Call(builtins.str_eq.unwrap()));
                }
                _ => {
                    emit_load_path(loc_a, path, false, wasm_fn);
                    emit_load_path(loc_b, path, false, wasm_fn);
                    wasm_fn.instruction(&Instruction::I64Eq);
                }
            }
        }

        let mut path = Vec::new();
        inner(
            loc_a,
            loc_b,
            &Type::Struct(schema_name.to_string()),
            &mut path,
            schemas,
            builtins,
            wasm_fn,
            0,
        );
    }

    fn emit_single_block_terminator(
        term: &Terminator,
        wasm_fn: &mut WasmFunction,
        reg_to_local: &HashMap<RegId, u32>,
    ) {
        match term {
            Terminator::Ret(Some(r)) => {
                let loc = reg_to_local[r];
                wasm_fn.instruction(&Instruction::LocalGet(loc));
                wasm_fn.instruction(&Instruction::Return);
            }
            Terminator::Ret(None) => {
                wasm_fn.instruction(&Instruction::Return);
            }
            Terminator::Br(_)
            | Terminator::BrIf { .. }
            | Terminator::BrArgs { .. }
            | Terminator::BrIfArgs { .. } => {
                wasm_fn.instruction(&Instruction::Unreachable);
            }
        }
    }
}

/// Helper tracking constant strings placed into WebAssembly linear memory data segments.
#[derive(Debug)]
struct StringPool {
    base_offset: u32,
    data: Vec<u8>,
    offsets: BTreeMap<String, u32>,
}

impl StringPool {
    fn new(base_offset: u32) -> Self {
        Self {
            base_offset,
            data: Vec::new(),
            offsets: BTreeMap::new(),
        }
    }

    fn intern(&mut self, s: &str) -> u32 {
        if let Some(&off) = self.offsets.get(s) {
            return off;
        }
        let offset = self.base_offset + self.data.len() as u32;
        let bytes = s.as_bytes();
        // Store 8-byte length prefix + string bytes + null terminator
        self.data.extend_from_slice(&(bytes.len() as u64).to_le_bytes());
        self.data.extend_from_slice(bytes);
        self.data.push(0);
        while !self.data.len().is_multiple_of(8) {
            self.data.push(0);
        }
        self.offsets.insert(s.to_string(), offset + 8);
        offset + 8
    }

    fn get_offset(&self, s: &str) -> u32 {
        self.offsets.get(s).copied().unwrap_or(self.base_offset)
    }
}
