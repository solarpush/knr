use crate::CodegenError;
use knr_core::{BasicBlock, Function, Instruction as KnrInst, Module, Op, RegId, Terminator, Type};
use std::collections::{BTreeMap, HashMap};
use wasm_encoder::{
    BlockType, CodeSection, ConstExpr, DataSection, EntityType, ExportKind, ExportSection,
    Function as WasmFunction, FunctionSection, ImportSection, Instruction, MemArg, MemorySection,
    MemoryType, Module as WasmModule, TypeSection, ValType,
};

type ImportSpec = (&'static str, Vec<ValType>, Vec<ValType>);

/// WebAssembly code generator lowering SSA `Module` / `Function` to `.wasm` binary bytes.
#[derive(Debug, Default, Clone)]
pub struct WasmBackend;

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

        // Helper to register function signatures in the type section
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

        // Determine if external runtime imports from "env" are needed
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

        // Register signatures for all functions defined in the module
        let num_imports = if needs_imports {
            import_specs.len() as u32
        } else {
            0
        };
        let mut func_type_indices = Vec::with_capacity(module.functions.len());

        // Lookup map: function name -> WebAssembly function index
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

        // 5. Memory Section: 1 page minimum (64 KiB)
        let mut memories = MemorySection::new();
        memories.memory(MemoryType {
            minimum: 1,
            maximum: None,
            memory64: false,
            shared: false,
            page_size_log2: None,
        });
        wasm.section(&memories);

        // 7. Export Section: export memory and functions
        let mut exports = ExportSection::new();
        exports.export("memory", ExportKind::Memory, 0);

        for (i, func) in module.functions.iter().enumerate() {
            let func_idx = num_imports + i as u32;
            exports.export(&func.name, ExportKind::Func, func_idx);
        }
        wasm.section(&exports);

        // Constant pool for string literals in linear memory (offset starts at 1024)
        let mut string_pool = StringPool::new(1024);
        for func in &module.functions {
            for block in &func.blocks {
                for inst in &block.instructions {
                    if let Op::ConstStr(s) = &inst.op {
                        string_pool.intern(s);
                    }
                }
            }
        }

        // 10. Code Section: function bodies
        let mut code = CodeSection::new();
        for func in &module.functions {
            let wasm_fn =
                Self::compile_func_body(func, &import_indices, &func_indices, &string_pool)?;
            code.function(&wasm_fn);
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

    /// Maps knr Types to Wasm ValTypes.
    fn type_to_wasm(ty: &Type) -> Option<ValType> {
        match ty {
            Type::I64 => Some(ValType::I64),
            Type::F64 => Some(ValType::F64),
            Type::Bool => Some(ValType::I32),
            Type::Void => None,
            // Linear references (Buf, Str, Vec, Map, Chan, Struct) mapped to 64-bit pointers/handles
            Type::Str | Type::Buf | Type::Vec(_) | Type::Map(_, _) | Type::Chan(_) | Type::Struct(_) => {
                Some(ValType::I64)
            }
        }
    }

    fn collect_imports(module: &Module) -> (bool, Vec<ImportSpec>) {
        let mut needs = false;
        for func in &module.functions {
            for block in &func.blocks {
                for inst in &block.instructions {
                    match &inst.op {
                        Op::BufAlloc(_)
                        | Op::Free(_)
                        | Op::VecNew(_)
                        | Op::VecPush(_, _)
                        | Op::VecGet(_, _)
                        | Op::VecLen(_)
                        | Op::MapNew(_, _)
                        | Op::MapSet(_, _, _)
                        | Op::MapGet(_, _)
                        | Op::MapDel(_, _)
                        | Op::ChanNew(_, _)
                        | Op::ChanSend(_, _)
                        | Op::ChanRecv(_)
                        | Op::Spawn(_, _)
                        | Op::Syscall(_, _, _) => {
                            needs = true;
                            break;
                        }
                        _ => {}
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

    /// Compiles a function's instructions and terminators into a `WasmFunction`.
    fn compile_func_body(
        func: &Function,
        import_indices: &HashMap<&'static str, u32>,
        func_indices: &HashMap<String, u32>,
        string_pool: &StringPool,
    ) -> Result<WasmFunction, CodegenError> {
        let mut reg_types: HashMap<RegId, Type> = HashMap::new();
        let mut reg_to_local: HashMap<RegId, u32> = HashMap::new();

        // 1. Assign local indices for function parameters
        for (i, (reg, ty)) in func.params.iter().enumerate() {
            reg_types.insert(*reg, ty.clone());
            reg_to_local.insert(*reg, i as u32);
        }
        let param_count = func.params.len() as u32;

        // 2. Discover all non-param destination registers
        for block in &func.blocks {
            for inst in &block.instructions {
                if let Some(dst) = inst.dst {
                    reg_types.insert(dst, inst.ty.clone());
                }
            }
        }

        // Group non-param locals by ValType
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

        // Sort for deterministic bytecode
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

        // If multiple blocks or single-block loops/branches, allocate an extra i32 local for basic block state dispatching
        let needs_dispatch = func.blocks.len() > 1
            || func.blocks.iter().any(|b| matches!(b.terminator, Terminator::Br(_) | Terminator::BrIf { .. }));
        let state_local = if needs_dispatch {
            let l = next_local;
            Some(l)
        } else {
            None
        };

        // Build locals declaration
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
            // Straight-line single basic block: execute straight-through
            if let Some(block) = func.blocks.first() {
                Self::emit_block_instructions(
                    block,
                    &mut wasm_fn,
                    &reg_to_local,
                    &reg_types,
                    import_indices,
                    func_indices,
                    string_pool,
                )?;
                Self::emit_single_block_terminator(&block.terminator, &mut wasm_fn, &reg_to_local);
            } else {
                wasm_fn.instruction(&Instruction::Return);
            }
        } else {
            // Multi-block: structured dispatch loop
            let state_var = state_local.unwrap();
            let first_block_id = func.blocks[0].id;

            // Initialize state_var = first_block_id
            wasm_fn.instruction(&Instruction::I32Const(first_block_id as i32));
            wasm_fn.instruction(&Instruction::LocalSet(state_var));

            // loop $dispatch
            wasm_fn.instruction(&Instruction::Loop(BlockType::Empty));

            for block in &func.blocks {
                // if (state_var == block.id)
                wasm_fn.instruction(&Instruction::LocalGet(state_var));
                wasm_fn.instruction(&Instruction::I32Const(block.id as i32));
                wasm_fn.instruction(&Instruction::I32Eq);
                wasm_fn.instruction(&Instruction::If(BlockType::Empty));

                Self::emit_block_instructions(
                    block,
                    &mut wasm_fn,
                    &reg_to_local,
                    &reg_types,
                    import_indices,
                    func_indices,
                    string_pool,
                )?;

                // Terminator for block inside loop
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
                        // Jump back to loop header (depth 1 from inside if)
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
                        // Jump back to loop header
                        wasm_fn.instruction(&Instruction::Br(1));
                    }
                }

                wasm_fn.instruction(&Instruction::End); // end if (state_var == block.id)
            }

            // At end of loop: loop back
            wasm_fn.instruction(&Instruction::Br(0));
            wasm_fn.instruction(&Instruction::End); // end loop

            if func.ret_ty != Type::Void {
                wasm_fn.instruction(&Instruction::Unreachable);
            }
        }

        wasm_fn.instruction(&Instruction::End); // end function body
        Ok(wasm_fn)
    }

    fn emit_block_instructions(
        block: &BasicBlock,
        wasm_fn: &mut WasmFunction,
        reg_to_local: &HashMap<RegId, u32>,
        reg_types: &HashMap<RegId, Type>,
        import_indices: &HashMap<&'static str, u32>,
        func_indices: &HashMap<String, u32>,
        string_pool: &StringPool,
    ) -> Result<(), CodegenError> {
        for inst in &block.instructions {
            Self::emit_instruction(
                inst,
                wasm_fn,
                reg_to_local,
                reg_types,
                import_indices,
                func_indices,
                string_pool,
            )?;
        }
        Ok(())
    }

    fn emit_instruction(
        inst: &KnrInst,
        wasm_fn: &mut WasmFunction,
        reg_to_local: &HashMap<RegId, u32>,
        reg_types: &HashMap<RegId, Type>,
        import_indices: &HashMap<&'static str, u32>,
        func_indices: &HashMap<String, u32>,
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
            Op::CmpEq(a, b) => {
                wasm_fn.instruction(&Instruction::LocalGet(reg_to_local[a]));
                wasm_fn.instruction(&Instruction::LocalGet(reg_to_local[b]));
                let ty_a = reg_types.get(a).unwrap_or(&Type::I64);
                if *ty_a == Type::F64 {
                    wasm_fn.instruction(&Instruction::F64Eq);
                } else if *ty_a == Type::Bool {
                    wasm_fn.instruction(&Instruction::I32Eq);
                } else {
                    wasm_fn.instruction(&Instruction::I64Eq);
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
            Op::BufAlloc(size) => {
                let call_idx = import_indices.get("rt_buf_alloc_aligned").copied().unwrap_or(0);
                wasm_fn.instruction(&Instruction::LocalGet(reg_to_local[size]));
                wasm_fn.instruction(&Instruction::I64Const(64));
                wasm_fn.instruction(&Instruction::Call(call_idx));
                if let Some(dst) = dst_loc {
                    wasm_fn.instruction(&Instruction::LocalSet(dst));
                }
            }
            Op::BufReadU8(buf, idx) => {
                // address = (buf + idx) as i32
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
                // address = (buf + idx) as i32
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
                // Returns the consumed buffer
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
                let call_idx = import_indices.get("rt_vec_new").copied().unwrap_or(0);
                wasm_fn.instruction(&Instruction::Call(call_idx));
                if let Some(dst) = dst_loc {
                    wasm_fn.instruction(&Instruction::LocalSet(dst));
                }
            }
            Op::VecPush(v, val) => {
                let call_idx = import_indices.get("rt_vec_push").copied().unwrap_or(0);
                wasm_fn.instruction(&Instruction::LocalGet(reg_to_local[v]));
                wasm_fn.instruction(&Instruction::LocalGet(reg_to_local[val]));
                wasm_fn.instruction(&Instruction::Call(call_idx));
                if let Some(dst) = dst_loc {
                    wasm_fn.instruction(&Instruction::LocalSet(dst));
                }
            }
            Op::VecGet(v, idx) => {
                let call_idx = import_indices.get("rt_vec_get").copied().unwrap_or(0);
                wasm_fn.instruction(&Instruction::LocalGet(reg_to_local[v]));
                wasm_fn.instruction(&Instruction::LocalGet(reg_to_local[idx]));
                wasm_fn.instruction(&Instruction::Call(call_idx));
                if let Some(dst) = dst_loc {
                    wasm_fn.instruction(&Instruction::LocalSet(dst));
                }
            }
            Op::VecLen(v) => {
                let call_idx = import_indices.get("rt_vec_len").copied().unwrap_or(0);
                wasm_fn.instruction(&Instruction::LocalGet(reg_to_local[v]));
                wasm_fn.instruction(&Instruction::Call(call_idx));
                if let Some(dst) = dst_loc {
                    wasm_fn.instruction(&Instruction::LocalSet(dst));
                }
            }
            Op::MapNew(_, _) => {
                let call_idx = import_indices.get("rt_map_new").copied().unwrap_or(0);
                wasm_fn.instruction(&Instruction::Call(call_idx));
                if let Some(dst) = dst_loc {
                    wasm_fn.instruction(&Instruction::LocalSet(dst));
                }
            }
            Op::MapSet(m, k, v) => {
                let call_idx = import_indices.get("rt_map_set").copied().unwrap_or(0);
                wasm_fn.instruction(&Instruction::LocalGet(reg_to_local[m]));
                wasm_fn.instruction(&Instruction::LocalGet(reg_to_local[k]));
                wasm_fn.instruction(&Instruction::I64Const(0)); // len if pointer or 0
                wasm_fn.instruction(&Instruction::LocalGet(reg_to_local[v]));
                wasm_fn.instruction(&Instruction::Call(call_idx));
                if let Some(dst) = dst_loc {
                    wasm_fn.instruction(&Instruction::LocalSet(dst));
                }
            }
            Op::MapGet(m, k) => {
                let call_idx = import_indices.get("rt_map_get").copied().unwrap_or(0);
                wasm_fn.instruction(&Instruction::LocalGet(reg_to_local[m]));
                wasm_fn.instruction(&Instruction::LocalGet(reg_to_local[k]));
                wasm_fn.instruction(&Instruction::I64Const(0));
                wasm_fn.instruction(&Instruction::Call(call_idx));
                if let Some(dst) = dst_loc {
                    wasm_fn.instruction(&Instruction::LocalSet(dst));
                }
            }
            Op::MapDel(m, _k) => {
                // map_del returns consumed map
                if let Some(dst) = dst_loc {
                    wasm_fn.instruction(&Instruction::LocalGet(reg_to_local[m]));
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
            Op::Free(r) => {
                match reg_types.get(r) {
                    Some(Type::Vec(_)) => {
                        let call_idx = import_indices.get("rt_vec_free").copied().unwrap_or(0);
                        wasm_fn.instruction(&Instruction::LocalGet(reg_to_local[r]));
                        wasm_fn.instruction(&Instruction::Call(call_idx));
                    }
                    Some(Type::Map(_, _)) => {
                        let call_idx = import_indices.get("rt_map_free").copied().unwrap_or(0);
                        wasm_fn.instruction(&Instruction::LocalGet(reg_to_local[r]));
                        wasm_fn.instruction(&Instruction::Call(call_idx));
                    }
                    _ => {
                        let call_idx = import_indices.get("rt_buf_free").copied().unwrap_or(0);
                        wasm_fn.instruction(&Instruction::LocalGet(reg_to_local[r]));
                        wasm_fn.instruction(&Instruction::I64Const(0));
                        wasm_fn.instruction(&Instruction::I64Const(64));
                        wasm_fn.instruction(&Instruction::Call(call_idx));
                    }
                }
            }
            Op::StructNew(_) | Op::StructGet(_, _) | Op::StructSet(_, _, _) => {
                return Err(CodegenError::Wasm(format!("Unsupported struct opcode: {:?}", inst.op)));
            }
            Op::Call(fn_name, args) => {
                let target_idx = func_indices
                    .get(fn_name)
                    .ok_or_else(|| CodegenError::Wasm(format!("Unknown function in call: {fn_name}")))?;

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
            Terminator::Br(_) | Terminator::BrIf { .. } => {
                // Branching terminators are routed through the dispatch loop
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
        // Align to 8 bytes
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
