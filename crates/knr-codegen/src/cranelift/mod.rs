use crate::CodegenError;
use cranelift::codegen::ir::{types, AbiParam, InstBuilder, MemFlags, UserFuncName};
use cranelift::codegen::isa::TargetIsa;
use cranelift::codegen::settings::{self, Configurable, Flags};
use cranelift::codegen::Context;
use cranelift::prelude::{FloatCC, FunctionBuilder, FunctionBuilderContext, IntCC, Variable};
use cranelift_jit::{JITBuilder, JITModule};
use cranelift_module::{default_libcall_names, DataDescription, FuncId, Linkage, Module as _};
use cranelift_object::{ObjectBuilder, ObjectModule};
use knr_core::{Effect, Function, Instruction as KnrInst, Module, Op, RegId, Schema, Terminator, Type};
use std::collections::HashMap;
use std::sync::Arc;

#[inline]
fn var(reg: RegId) -> Variable {
    Variable::from_u32(reg)
}

/// Native code generator lowering SSA `Module` / `Function` using Cranelift.
/// Supports both in-memory JIT execution (`compile_jit`, `compile_jit_module`) and
/// native object emission (`compile_object`) with full support for `Call`, `Struct*`,
/// block parameters, strings, channels, and `Syscall`.
pub struct CraneliftBackend {
    isa: Arc<dyn TargetIsa>,
    // Kept alive so JIT-compiled function memory pages remain executable
    jit_module: Option<JITModule>,
}

impl CraneliftBackend {
    /// Creates a new `CraneliftBackend` targeting the host platform ISA.
    pub fn new() -> Result<Self, CodegenError> {
        let mut flag_builder = settings::builder();
        flag_builder
            .set("use_colocated_libcalls", "false")
            .map_err(|e| CodegenError::Cranelift(e.to_string()))?;
        flag_builder
            .set("is_pic", "false")
            .map_err(|e| CodegenError::Cranelift(e.to_string()))?;
        let isa_builder = cranelift_native::builder()
            .map_err(|e| CodegenError::Cranelift(e.to_string()))?;
        let isa = isa_builder
            .finish(Flags::new(flag_builder))
            .map_err(|e| CodegenError::Cranelift(e.to_string()))?;

        Ok(Self {
            isa,
            jit_module: None,
        })
    }

    fn register_jit_runtime_symbols(jit_builder: &mut JITBuilder) {
        jit_builder.symbol("rt_map_new", knr_runtime::rt_map_new as *const u8);
        jit_builder.symbol("rt_map_set", knr_runtime::rt_map_set as *const u8);
        jit_builder.symbol("rt_map_get", knr_runtime::rt_map_get as *const u8);
        jit_builder.symbol("rt_map_del", knr_runtime::rt_map_del as *const u8);
        jit_builder.symbol("rt_map_free", knr_runtime::rt_map_free as *const u8);
        jit_builder.symbol("rt_vec_new", knr_runtime::rt_vec_new as *const u8);
        jit_builder.symbol("rt_vec_push", knr_runtime::rt_vec_push as *const u8);
        jit_builder.symbol("rt_vec_get", knr_runtime::rt_vec_get as *const u8);
        jit_builder.symbol("rt_vec_len", knr_runtime::rt_vec_len as *const u8);
        jit_builder.symbol("rt_vec_free", knr_runtime::rt_vec_free as *const u8);
        jit_builder.symbol(
            "rt_buf_alloc_aligned",
            knr_runtime::rt_buf_alloc_aligned as *const u8,
        );
        jit_builder.symbol("rt_buf_free", knr_runtime::rt_buf_free as *const u8);
        jit_builder.symbol("rt_struct_new", knr_runtime::rt_struct_new as *const u8);
        jit_builder.symbol("rt_struct_get", knr_runtime::rt_struct_get as *const u8);
        jit_builder.symbol("rt_struct_set", knr_runtime::rt_struct_set as *const u8);
        jit_builder.symbol("rt_struct_free", knr_runtime::rt_struct_free as *const u8);
        jit_builder.symbol("rt_str_concat", knr_runtime::rt_str_concat as *const u8);
        jit_builder.symbol("rt_str_eq", knr_runtime::rt_str_eq as *const u8);
        jit_builder.symbol("rt_str_len", knr_runtime::rt_str_len as *const u8);
        jit_builder.symbol("rt_int_to_str", knr_runtime::rt_int_to_str as *const u8);
        jit_builder.symbol("rt_bool_to_str", knr_runtime::rt_bool_to_str as *const u8);
        jit_builder.symbol("rt_f64_to_str", knr_runtime::rt_f64_to_str as *const u8);
        jit_builder.symbol("rt_chan_new", knr_runtime::rt_chan_new as *const u8);
        jit_builder.symbol("rt_chan_send", knr_runtime::rt_chan_send as *const u8);
        jit_builder.symbol("rt_chan_recv", knr_runtime::rt_chan_recv as *const u8);
        jit_builder.symbol("rt_syscall", knr_runtime::rt_syscall as *const u8);
    }

    /// Compiles a single SSA `Function` to executable machine code via Cranelift JIT.
    pub fn compile_jit(&mut self, func: &Function) -> Result<*const u8, CodegenError> {
        let mut module = Module::new();
        module.add_function(func.clone());
        self.compile_jit_module(&module, &func.name)
    }

    /// Compiles an entire SSA `Module` (with structs, enums, and inter-function calls)
    /// to executable machine code via Cranelift JIT and returns a raw pointer to `entry`.
    pub fn compile_jit_module(
        &mut self,
        module: &Module,
        entry: &str,
    ) -> Result<*const u8, CodegenError> {
        let mut jit_builder = JITBuilder::with_isa(self.isa.clone(), default_libcall_names());
        Self::register_jit_runtime_symbols(&mut jit_builder);

        let mut jit_mod = JITModule::new(jit_builder);
        let runtime_funcs = declare_runtime_symbols(&mut jit_mod)?;
        let user_funcs = declare_user_functions(&mut jit_mod, &module.functions)?;

        let mut ctx = jit_mod.make_context();
        for func in &module.functions {
            let fid = user_funcs[&func.name];
            compile_function_into_module(
                &mut jit_mod,
                &mut ctx,
                fid,
                func,
                &module.schemas,
                &runtime_funcs,
                &user_funcs,
            )?;
            ctx.clear();
        }

        jit_mod
            .finalize_definitions()
            .map_err(|e| CodegenError::Cranelift(e.to_string()))?;

        let entry_id = *user_funcs.get(entry).ok_or_else(|| {
            CodegenError::Cranelift(format!("Entry function '{entry}' not found in module"))
        })?;
        let code_ptr = jit_mod.get_finalized_function(entry_id);
        self.jit_module = Some(jit_mod);

        Ok(code_ptr)
    }

    /// Compiles an SSA `Module` into an AOT native object file (`.o`) via `cranelift-object`.
    pub fn compile_object(&mut self, module: &Module) -> Result<Vec<u8>, CodegenError> {
        let obj_builder =
            ObjectBuilder::new(self.isa.clone(), "knr_module", default_libcall_names())?;
        let mut obj_module = ObjectModule::new(obj_builder);
        let runtime_funcs = declare_runtime_symbols(&mut obj_module)?;
        let user_funcs = declare_user_functions(&mut obj_module, &module.functions)?;

        let mut ctx = obj_module.make_context();
        for func in &module.functions {
            let fid = user_funcs[&func.name];
            compile_function_into_module(
                &mut obj_module,
                &mut ctx,
                fid,
                func,
                &module.schemas,
                &runtime_funcs,
                &user_funcs,
            )?;
            ctx.clear();
        }

        let product = obj_module.finish();
        let bytes = product
            .emit()
            .map_err(|e| CodegenError::Cranelift(e.to_string()))?;
        Ok(bytes)
    }
}

fn declare_user_functions<M: cranelift_module::Module>(
    module: &mut M,
    functions: &[Function],
) -> Result<HashMap<String, FuncId>, CodegenError> {
    let mut user_funcs = HashMap::with_capacity(functions.len());
    for func in functions {
        let mut sig = module.make_signature();
        for (_, ty) in &func.params {
            sig.params.push(AbiParam::new(clif_type(ty)));
        }
        if func.ret_ty != Type::Void {
            sig.returns.push(AbiParam::new(clif_type(&func.ret_ty)));
        }
        // Export "main" as "knr_main" only? No — keep exact function names so both
        // C entry wrappers (`ENTRY_FN`) and direct `main` exports work cleanly.
        let id = module.declare_function(&func.name, Linkage::Export, &sig)?;
        user_funcs.insert(func.name.clone(), id);
    }
    Ok(user_funcs)
}

fn decl_fn<M: cranelift_module::Module>(
    module: &mut M,
    funcs: &mut HashMap<&'static str, FuncId>,
    name: &'static str,
    params: &[types::Type],
    returns: &[types::Type],
) -> Result<(), CodegenError> {
    let mut sig = module.make_signature();
    for &p in params {
        sig.params.push(AbiParam::new(p));
    }
    for &r in returns {
        sig.returns.push(AbiParam::new(r));
    }
    let id = module.declare_function(name, Linkage::Import, &sig)?;
    funcs.insert(name, id);
    Ok(())
}

/// Declares external C-ABI runtime symbols in the Cranelift module.
fn declare_runtime_symbols<M: cranelift_module::Module>(
    module: &mut M,
) -> Result<HashMap<&'static str, FuncId>, CodegenError> {
    let mut f = HashMap::new();
    let i64t = types::I64;
    let f64t = types::F64;

    decl_fn(module, &mut f, "rt_map_new", &[], &[i64t])?;
    decl_fn(module, &mut f, "rt_map_set", &[i64t, i64t, i64t, i64t], &[i64t])?;
    decl_fn(module, &mut f, "rt_map_get", &[i64t, i64t, i64t], &[i64t])?;
    decl_fn(module, &mut f, "rt_map_del", &[i64t, i64t, i64t], &[i64t])?;
    decl_fn(module, &mut f, "rt_map_free", &[i64t], &[])?;

    decl_fn(module, &mut f, "rt_vec_new", &[], &[i64t])?;
    decl_fn(module, &mut f, "rt_vec_push", &[i64t, i64t], &[i64t])?;
    decl_fn(module, &mut f, "rt_vec_get", &[i64t, i64t], &[i64t])?;
    decl_fn(module, &mut f, "rt_vec_len", &[i64t], &[i64t])?;
    decl_fn(module, &mut f, "rt_vec_free", &[i64t], &[])?;

    decl_fn(module, &mut f, "rt_buf_alloc_aligned", &[i64t, i64t], &[i64t])?;
    decl_fn(module, &mut f, "rt_buf_free", &[i64t, i64t, i64t], &[])?;

    decl_fn(module, &mut f, "rt_struct_new", &[i64t], &[i64t])?;
    decl_fn(module, &mut f, "rt_struct_get", &[i64t, i64t], &[i64t])?;
    decl_fn(module, &mut f, "rt_struct_set", &[i64t, i64t, i64t], &[i64t])?;
    decl_fn(module, &mut f, "rt_struct_free", &[i64t], &[])?;

    decl_fn(module, &mut f, "rt_str_concat", &[i64t, i64t], &[i64t])?;
    decl_fn(module, &mut f, "rt_str_eq", &[i64t, i64t], &[i64t])?;
    decl_fn(module, &mut f, "rt_str_len", &[i64t], &[i64t])?;
    decl_fn(module, &mut f, "rt_int_to_str", &[i64t], &[i64t])?;
    decl_fn(module, &mut f, "rt_bool_to_str", &[i64t], &[i64t])?;
    decl_fn(module, &mut f, "rt_f64_to_str", &[f64t], &[i64t])?;

    decl_fn(module, &mut f, "rt_chan_new", &[i64t], &[i64t])?;
    decl_fn(module, &mut f, "rt_chan_send", &[i64t, i64t], &[i64t])?;
    decl_fn(module, &mut f, "rt_chan_recv", &[i64t], &[i64t])?;

    decl_fn(module, &mut f, "rt_syscall", &[i64t, i64t, i64t, i64t], &[i64t])?;

    Ok(f)
}

/// Translates a knr SSA Type to a Cranelift IR Type.
fn clif_type(ty: &Type) -> types::Type {
    match ty {
        Type::I64 | Type::Bool => types::I64,
        Type::F64 => types::F64,
        Type::Void => types::I64,
        Type::Buf | Type::Str | Type::Vec(_) | Type::Map(_, _) | Type::Chan(_) | Type::Struct(_) => {
            types::I64
        }
    }
}

/// Compiles a single SSA Function into a Cranelift module.
fn compile_function_into_module<M: cranelift_module::Module>(
    module: &mut M,
    ctx: &mut Context,
    func_id: FuncId,
    func: &Function,
    schemas: &[Schema],
    runtime_funcs: &HashMap<&'static str, FuncId>,
    user_funcs: &HashMap<String, FuncId>,
) -> Result<(), CodegenError> {
    let mut sig = module.make_signature();
    for (_, ty) in &func.params {
        sig.params.push(AbiParam::new(clif_type(ty)));
    }
    if func.ret_ty != Type::Void {
        sig.returns.push(AbiParam::new(clif_type(&func.ret_ty)));
    }

    ctx.func.signature = sig;
    ctx.func.name = UserFuncName::user(0, func_id.as_u32());

    let mut builder_context = FunctionBuilderContext::new();
    let mut builder = FunctionBuilder::new(&mut ctx.func, &mut builder_context);

    // Map basic blocks to Cranelift blocks
    let mut block_map = HashMap::new();
    for b in &func.blocks {
        let clif_block = builder.create_block();
        block_map.insert(b.id, clif_block);
    }

    if func.blocks.is_empty() {
        let entry = builder.create_block();
        builder.switch_to_block(entry);
        builder.seal_block(entry);
        builder.ins().return_(&[]);
        builder.finalize();
        module.define_function(func_id, ctx)?;
        return Ok(());
    }

    let entry_block = block_map[&func.blocks[0].id];
    builder.append_block_params_for_function_params(entry_block);

    // Append Cranelift block parameters for every block's SSA parameters
    for b in &func.blocks {
        let clif_blk = block_map[&b.id];
        for (_, ty) in &b.params {
            builder.append_block_param(clif_blk, clif_type(ty));
        }
    }

    // Record types for all registers (function params, block params, instructions)
    let mut reg_types: HashMap<RegId, Type> = HashMap::new();
    for (reg, ty) in &func.params {
        reg_types.insert(*reg, ty.clone());
    }
    for b in &func.blocks {
        for (reg, ty) in &b.params {
            reg_types.insert(*reg, ty.clone());
        }
        for inst in &b.instructions {
            if let Some(dst) = inst.dst {
                reg_types.insert(dst, inst.ty.clone());
            }
        }
    }

    // Declare Cranelift variables for all registers
    for (&reg, ty) in &reg_types {
        let v = var(reg);
        builder.declare_var(v, clif_type(ty));
    }

    // Define function parameter variables in the entry block
    builder.switch_to_block(entry_block);
    let entry_params = builder.block_params(entry_block).to_vec();
    for (i, (reg, _)) in func.params.iter().enumerate() {
        let v = var(*reg);
        builder.def_var(v, entry_params[i]);
    }

    let mut str_counter = 0usize;

    // Translate all basic blocks
    for (block_idx, b) in func.blocks.iter().enumerate() {
        let clif_block = block_map[&b.id];
        builder.switch_to_block(clif_block);

        // Bind SSA block parameter registers to the Cranelift block params
        if !b.params.is_empty() {
            let offset = if block_idx == 0 { func.params.len() } else { 0 };
            let blk_params = builder.block_params(clif_block).to_vec();
            for (i, (preg, _)) in b.params.iter().enumerate() {
                builder.def_var(var(*preg), blk_params[offset + i]);
            }
        }

        for inst in &b.instructions {
            emit_instruction(
                module,
                &mut builder,
                &func.name,
                inst,
                &reg_types,
                schemas,
                runtime_funcs,
                user_funcs,
                &mut str_counter,
            )?;
        }

        emit_terminator(&mut builder, &b.terminator, &block_map);
    }

    builder.seal_all_blocks();
    builder.finalize();

    module.define_function(func_id, ctx)?;

    Ok(())
}

fn emit_c_str_data<M: cranelift_module::Module>(
    module: &mut M,
    builder: &mut FunctionBuilder,
    func_name: &str,
    s: &str,
    str_counter: &mut usize,
) -> Result<cranelift::codegen::ir::Value, CodegenError> {
    let data_name = format!("{}_str_{}", func_name, *str_counter);
    *str_counter += 1;
    let mut data_desc = DataDescription::new();
    let mut bytes = s.as_bytes().to_vec();
    bytes.push(0);
    data_desc.define(bytes.into_boxed_slice());
    let data_id = module.declare_data(&data_name, Linkage::Local, false, false)?;
    module.define_data(data_id, &data_desc)?;
    let local_data = module.declare_data_in_func(data_id, builder.func);
    Ok(builder.ins().symbol_value(types::I64, local_data))
}

#[allow(clippy::too_many_arguments)]
fn emit_instruction<M: cranelift_module::Module>(
    module: &mut M,
    builder: &mut FunctionBuilder,
    func_name: &str,
    inst: &KnrInst,
    reg_types: &HashMap<RegId, Type>,
    schemas: &[Schema],
    runtime_funcs: &HashMap<&'static str, FuncId>,
    user_funcs: &HashMap<String, FuncId>,
    str_counter: &mut usize,
) -> Result<(), CodegenError> {
    let dst_var = inst.dst.map(var);

    match &inst.op {
        Op::ConstI64(val) => {
            let res = builder.ins().iconst(types::I64, *val);
            if let Some(dst) = dst_var {
                builder.def_var(dst, res);
            }
        }
        Op::ConstF64(val) => {
            let res = builder.ins().f64const(*val);
            if let Some(dst) = dst_var {
                builder.def_var(dst, res);
            }
        }
        Op::ConstBool(val) => {
            let res = builder.ins().iconst(types::I64, if *val { 1 } else { 0 });
            if let Some(dst) = dst_var {
                builder.def_var(dst, res);
            }
        }
        Op::ConstStr(s) => {
            let res = emit_c_str_data(module, builder, func_name, s, str_counter)?;
            if let Some(dst) = dst_var {
                builder.def_var(dst, res);
            }
        }
        Op::Add(a, b) => {
            let va = builder.use_var(var(*a));
            let vb = builder.use_var(var(*b));
            let res = if inst.ty == Type::F64 {
                builder.ins().fadd(va, vb)
            } else if inst.ty == Type::Str {
                let callee =
                    module.declare_func_in_func(runtime_funcs["rt_str_concat"], builder.func);
                let call = builder.ins().call(callee, &[va, vb]);
                builder.inst_results(call)[0]
            } else {
                builder.ins().iadd(va, vb)
            };
            if let Some(dst) = dst_var {
                builder.def_var(dst, res);
            }
        }
        Op::Sub(a, b) => {
            let va = builder.use_var(var(*a));
            let vb = builder.use_var(var(*b));
            let res = if inst.ty == Type::F64 {
                builder.ins().fsub(va, vb)
            } else {
                builder.ins().isub(va, vb)
            };
            if let Some(dst) = dst_var {
                builder.def_var(dst, res);
            }
        }
        Op::Mul(a, b) => {
            let va = builder.use_var(var(*a));
            let vb = builder.use_var(var(*b));
            let res = if inst.ty == Type::F64 {
                builder.ins().fmul(va, vb)
            } else {
                builder.ins().imul(va, vb)
            };
            if let Some(dst) = dst_var {
                builder.def_var(dst, res);
            }
        }
        Op::Div(a, b) => {
            let va = builder.use_var(var(*a));
            let vb = builder.use_var(var(*b));
            let res = if inst.ty == Type::F64 {
                builder.ins().fdiv(va, vb)
            } else {
                builder.ins().sdiv(va, vb)
            };
            if let Some(dst) = dst_var {
                builder.def_var(dst, res);
            }
        }
        Op::Rem(a, b) => {
            let va = builder.use_var(var(*a));
            let vb = builder.use_var(var(*b));
            let res = builder.ins().srem(va, vb);
            if let Some(dst) = dst_var {
                builder.def_var(dst, res);
            }
        }
        Op::CmpEq(a, b) => {
            let va = builder.use_var(var(*a));
            let vb = builder.use_var(var(*b));
            let ty_a = reg_types.get(a).unwrap_or(&Type::I64);
            let res = if *ty_a == Type::Str {
                let callee =
                    module.declare_func_in_func(runtime_funcs["rt_str_eq"], builder.func);
                let call = builder.ins().call(callee, &[va, vb]);
                builder.inst_results(call)[0]
            } else {
                let cmp = if *ty_a == Type::F64 {
                    builder.ins().fcmp(FloatCC::Equal, va, vb)
                } else {
                    builder.ins().icmp(IntCC::Equal, va, vb)
                };
                builder.ins().uextend(types::I64, cmp)
            };
            if let Some(dst) = dst_var {
                builder.def_var(dst, res);
            }
        }
        Op::CmpLt(a, b) => {
            let va = builder.use_var(var(*a));
            let vb = builder.use_var(var(*b));
            let ty_a = reg_types.get(a).unwrap_or(&Type::I64);
            let cmp = if *ty_a == Type::F64 {
                builder.ins().fcmp(FloatCC::LessThan, va, vb)
            } else {
                builder.ins().icmp(IntCC::SignedLessThan, va, vb)
            };
            let res = builder.ins().uextend(types::I64, cmp);
            if let Some(dst) = dst_var {
                builder.def_var(dst, res);
            }
        }
        Op::IntToStr(a) => {
            let va = builder.use_var(var(*a));
            let ty_a = reg_types.get(a).unwrap_or(&Type::I64);
            let res = match ty_a {
                Type::Str => va,
                Type::Bool => {
                    let callee =
                        module.declare_func_in_func(runtime_funcs["rt_bool_to_str"], builder.func);
                    let call = builder.ins().call(callee, &[va]);
                    builder.inst_results(call)[0]
                }
                Type::F64 => {
                    let callee =
                        module.declare_func_in_func(runtime_funcs["rt_f64_to_str"], builder.func);
                    let call = builder.ins().call(callee, &[va]);
                    builder.inst_results(call)[0]
                }
                _ => {
                    let callee =
                        module.declare_func_in_func(runtime_funcs["rt_int_to_str"], builder.func);
                    let call = builder.ins().call(callee, &[va]);
                    builder.inst_results(call)[0]
                }
            };
            if let Some(dst) = dst_var {
                builder.def_var(dst, res);
            }
        }
        Op::StrLen(a) => {
            let va = builder.use_var(var(*a));
            let callee = module.declare_func_in_func(runtime_funcs["rt_str_len"], builder.func);
            let call = builder.ins().call(callee, &[va]);
            let res = builder.inst_results(call)[0];
            if let Some(dst) = dst_var {
                builder.def_var(dst, res);
            }
        }
        Op::BufAlloc(size) => {
            let vs = builder.use_var(var(*size));
            let val_align = builder.ins().iconst(types::I64, 64);
            let local_callee = module.declare_func_in_func(
                runtime_funcs["rt_buf_alloc_aligned"],
                builder.func,
            );
            let call = builder.ins().call(local_callee, &[vs, val_align]);
            let res = builder.inst_results(call)[0];
            if let Some(dst) = dst_var {
                builder.def_var(dst, res);
            }
        }
        Op::BufReadU8(buf, idx) => {
            let vb = builder.use_var(var(*buf));
            let vi = builder.use_var(var(*idx));
            let addr = builder.ins().iadd(vb, vi);
            let byte = builder.ins().load(types::I8, MemFlags::trusted(), addr, 0);
            let res = builder.ins().uextend(types::I64, byte);
            if let Some(dst) = dst_var {
                builder.def_var(dst, res);
            }
        }
        Op::BufWriteU8(buf, idx, val) => {
            let vb = builder.use_var(var(*buf));
            let vi = builder.use_var(var(*idx));
            let vv = builder.use_var(var(*val));
            let addr = builder.ins().iadd(vb, vi);
            let byte = builder.ins().ireduce(types::I8, vv);
            builder.ins().store(MemFlags::trusted(), byte, addr, 0);
            if let Some(dst) = dst_var {
                builder.def_var(dst, vb);
            }
        }
        Op::BufSlice(buf, start, _len) => {
            let vb = builder.use_var(var(*buf));
            let vs = builder.use_var(var(*start));
            let res = builder.ins().iadd(vb, vs);
            if let Some(dst) = dst_var {
                builder.def_var(dst, res);
            }
        }
        Op::VecNew(_) => {
            let local_callee =
                module.declare_func_in_func(runtime_funcs["rt_vec_new"], builder.func);
            let call = builder.ins().call(local_callee, &[]);
            let res = builder.inst_results(call)[0];
            if let Some(dst) = dst_var {
                builder.def_var(dst, res);
            }
        }
        Op::VecPush(v, val) => {
            let vv = builder.use_var(var(*v));
            let mut v_val = builder.use_var(var(*val));
            if reg_types.get(val) == Some(&Type::F64) {
                v_val = builder.ins().bitcast(types::I64, MemFlags::new(), v_val);
            }
            let local_callee =
                module.declare_func_in_func(runtime_funcs["rt_vec_push"], builder.func);
            let call = builder.ins().call(local_callee, &[vv, v_val]);
            let res = builder.inst_results(call)[0];
            if let Some(dst) = dst_var {
                builder.def_var(dst, res);
            }
        }
        Op::VecGet(v, idx) => {
            let vv = builder.use_var(var(*v));
            let vi = builder.use_var(var(*idx));
            let local_callee =
                module.declare_func_in_func(runtime_funcs["rt_vec_get"], builder.func);
            let call = builder.ins().call(local_callee, &[vv, vi]);
            let mut res = builder.inst_results(call)[0];
            if inst.ty == Type::F64 {
                res = builder.ins().bitcast(types::F64, MemFlags::new(), res);
            }
            if let Some(dst) = dst_var {
                builder.def_var(dst, res);
            }
        }
        Op::VecLen(v) => {
            let vv = builder.use_var(var(*v));
            let local_callee =
                module.declare_func_in_func(runtime_funcs["rt_vec_len"], builder.func);
            let call = builder.ins().call(local_callee, &[vv]);
            let res = builder.inst_results(call)[0];
            if let Some(dst) = dst_var {
                builder.def_var(dst, res);
            }
        }
        Op::MapNew(_, _) => {
            let local_callee =
                module.declare_func_in_func(runtime_funcs["rt_map_new"], builder.func);
            let call = builder.ins().call(local_callee, &[]);
            let res = builder.inst_results(call)[0];
            if let Some(dst) = dst_var {
                builder.def_var(dst, res);
            }
        }
        Op::MapSet(m, k, v) => {
            let vm = builder.use_var(var(*m));
            let vk = builder.use_var(var(*k));
            let v_len = builder.ins().iconst(types::I64, 0);
            let mut vv = builder.use_var(var(*v));
            if reg_types.get(v) == Some(&Type::F64) {
                vv = builder.ins().bitcast(types::I64, MemFlags::new(), vv);
            }
            let local_callee =
                module.declare_func_in_func(runtime_funcs["rt_map_set"], builder.func);
            let call = builder.ins().call(local_callee, &[vm, vk, v_len, vv]);
            let res = builder.inst_results(call)[0];
            if let Some(dst) = dst_var {
                builder.def_var(dst, res);
            }
        }
        Op::MapGet(m, k) => {
            let vm = builder.use_var(var(*m));
            let vk = builder.use_var(var(*k));
            let v_len = builder.ins().iconst(types::I64, 0);
            let local_callee =
                module.declare_func_in_func(runtime_funcs["rt_map_get"], builder.func);
            let call = builder.ins().call(local_callee, &[vm, vk, v_len]);
            let mut res = builder.inst_results(call)[0];
            if inst.ty == Type::F64 {
                res = builder.ins().bitcast(types::F64, MemFlags::new(), res);
            }
            if let Some(dst) = dst_var {
                builder.def_var(dst, res);
            }
        }
        Op::MapDel(m, k) => {
            let vm = builder.use_var(var(*m));
            let vk = builder.use_var(var(*k));
            let v_len = builder.ins().iconst(types::I64, 0);
            let local_callee =
                module.declare_func_in_func(runtime_funcs["rt_map_del"], builder.func);
            let call = builder.ins().call(local_callee, &[vm, vk, v_len]);
            let res = builder.inst_results(call)[0];
            if let Some(dst) = dst_var {
                builder.def_var(dst, res);
            }
        }
        Op::ChanNew(_, cap) => {
            let vc = builder.use_var(var(*cap));
            let callee = module.declare_func_in_func(runtime_funcs["rt_chan_new"], builder.func);
            let call = builder.ins().call(callee, &[vc]);
            let res = builder.inst_results(call)[0];
            if let Some(dst) = dst_var {
                builder.def_var(dst, res);
            }
        }
        Op::ChanSend(c, val) => {
            let vc = builder.use_var(var(*c));
            let mut vv = builder.use_var(var(*val));
            if reg_types.get(val) == Some(&Type::F64) {
                vv = builder.ins().bitcast(types::I64, MemFlags::new(), vv);
            }
            let callee = module.declare_func_in_func(runtime_funcs["rt_chan_send"], builder.func);
            let call = builder.ins().call(callee, &[vc, vv]);
            let res = builder.inst_results(call)[0];
            if let Some(dst) = dst_var {
                builder.def_var(dst, res);
            }
        }
        Op::ChanRecv(c) => {
            let vc = builder.use_var(var(*c));
            let callee = module.declare_func_in_func(runtime_funcs["rt_chan_recv"], builder.func);
            let call = builder.ins().call(callee, &[vc]);
            let mut res = builder.inst_results(call)[0];
            if inst.ty == Type::F64 {
                res = builder.ins().bitcast(types::F64, MemFlags::new(), res);
            }
            if let Some(dst) = dst_var {
                builder.def_var(dst, res);
            }
        }
        Op::Spawn(_, _) => {
            let res = builder.ins().iconst(types::I64, 1);
            if let Some(dst) = dst_var {
                builder.def_var(dst, res);
            }
        }
        Op::Syscall(eff, op_name, payload) => {
            let eff_tag = match eff {
                Effect::Net => 0i64,
                Effect::Ui => 1i64,
                Effect::Hw => 2i64,
                Effect::Async => 3i64,
            };
            let v_eff = builder.ins().iconst(types::I64, eff_tag);
            let v_op = emit_c_str_data(module, builder, func_name, op_name, str_counter)?;
            let mut v_payload = builder.use_var(var(*payload));
            let p_ty = reg_types.get(payload).cloned().unwrap_or(Type::I64);
            if p_ty == Type::F64 {
                v_payload = builder.ins().bitcast(types::I64, MemFlags::new(), v_payload);
            }
            let payload_kind: i64 = match &p_ty {
                Type::Str => 1,
                Type::Bool => 4,
                Type::Struct(sname) => {
                    if let Some(sc) = schemas.iter().find(|s| &s.name == sname) {
                        if sname == "UiNode" || sc.fields.len() >= 11 {
                            5
                        } else if sc.get_field(2).is_some_and(|f| f.ty == Type::I64) {
                            3
                        } else {
                            2
                        }
                    } else if sname == "UiNode" {
                        5
                    } else {
                        2
                    }
                }
                _ => 0,
            };
            let v_kind = builder.ins().iconst(types::I64, payload_kind);
            let callee = module.declare_func_in_func(runtime_funcs["rt_syscall"], builder.func);
            let call = builder.ins().call(callee, &[v_eff, v_op, v_payload, v_kind]);
            let mut res = builder.inst_results(call)[0];
            if inst.ty == Type::F64 {
                res = builder.ins().bitcast(types::F64, MemFlags::new(), res);
            }
            if let Some(dst) = dst_var {
                builder.def_var(dst, res);
            }
        }
        Op::Free(r) => {
            let vr = builder.use_var(var(*r));
            match reg_types.get(r) {
                Some(Type::Vec(_)) => {
                    let local_callee =
                        module.declare_func_in_func(runtime_funcs["rt_vec_free"], builder.func);
                    builder.ins().call(local_callee, &[vr]);
                }
                Some(Type::Map(_, _)) => {
                    let local_callee =
                        module.declare_func_in_func(runtime_funcs["rt_map_free"], builder.func);
                    builder.ins().call(local_callee, &[vr]);
                }
                Some(Type::Struct(_)) => {
                    let local_callee =
                        module.declare_func_in_func(runtime_funcs["rt_struct_free"], builder.func);
                    builder.ins().call(local_callee, &[vr]);
                }
                Some(Type::Buf) => {
                    let v_size = builder.ins().iconst(types::I64, 0);
                    let v_align = builder.ins().iconst(types::I64, 64);
                    let local_callee =
                        module.declare_func_in_func(runtime_funcs["rt_buf_free"], builder.func);
                    builder.ins().call(local_callee, &[vr, v_size, v_align]);
                }
                _ => {}
            }
        }
        Op::StructNew(name) => {
            let field_count = schemas
                .iter()
                .find(|s| s.name == *name)
                .map(|sc| {
                    sc.fields
                        .iter()
                        .map(|f| f.index as i64 + 1)
                        .max()
                        .unwrap_or(sc.fields.len() as i64)
                })
                .unwrap_or(8)
                .max(1);
            let v_count = builder.ins().iconst(types::I64, field_count);
            let callee =
                module.declare_func_in_func(runtime_funcs["rt_struct_new"], builder.func);
            let call = builder.ins().call(callee, &[v_count]);
            let res = builder.inst_results(call)[0];
            if let Some(dst) = dst_var {
                builder.def_var(dst, res);
            }
        }
        Op::StructGet(s, field_idx) => {
            let vs = builder.use_var(var(*s));
            let v_idx = builder.ins().iconst(types::I64, *field_idx as i64);
            let callee =
                module.declare_func_in_func(runtime_funcs["rt_struct_get"], builder.func);
            let call = builder.ins().call(callee, &[vs, v_idx]);
            let mut res = builder.inst_results(call)[0];
            if inst.ty == Type::F64 {
                res = builder.ins().bitcast(types::F64, MemFlags::new(), res);
            }
            if let Some(dst) = dst_var {
                builder.def_var(dst, res);
            }
        }
        Op::StructSet(s, field_idx, val) => {
            let vs = builder.use_var(var(*s));
            let v_idx = builder.ins().iconst(types::I64, *field_idx as i64);
            let mut vv = builder.use_var(var(*val));
            if reg_types.get(val) == Some(&Type::F64) {
                vv = builder.ins().bitcast(types::I64, MemFlags::new(), vv);
            }
            let callee =
                module.declare_func_in_func(runtime_funcs["rt_struct_set"], builder.func);
            let call = builder.ins().call(callee, &[vs, v_idx, vv]);
            let res = builder.inst_results(call)[0];
            if let Some(dst) = dst_var {
                builder.def_var(dst, res);
            }
        }
        Op::Call(fn_name, args) => {
            let &target_id = user_funcs.get(fn_name).ok_or_else(|| {
                CodegenError::Cranelift(format!("Unknown function in call: {fn_name}"))
            })?;
            let local_callee = module.declare_func_in_func(target_id, builder.func);
            let arg_vals: Vec<_> = args.iter().map(|r| builder.use_var(var(*r))).collect();
            let call = builder.ins().call(local_callee, &arg_vals);
            if let Some(dst) = dst_var {
                let results = builder.inst_results(call);
                if !results.is_empty() {
                    builder.def_var(dst, results[0]);
                }
            }
        }
    }

    Ok(())
}

fn emit_terminator(
    builder: &mut FunctionBuilder,
    terminator: &Terminator,
    block_map: &HashMap<knr_core::BlockId, cranelift::prelude::Block>,
) {
    match terminator {
        Terminator::Ret(Some(r)) => {
            let vr = builder.use_var(var(*r));
            builder.ins().return_(&[vr]);
        }
        Terminator::Ret(None) => {
            builder.ins().return_(&[]);
        }
        Terminator::Br(target) => {
            builder.ins().jump(block_map[target], &[]);
        }
        Terminator::BrIf {
            cond,
            then_blk,
            else_blk,
        } => {
            let vc = builder.use_var(var(*cond));
            let bool_c = builder.ins().icmp_imm(IntCC::NotEqual, vc, 0);
            builder
                .ins()
                .brif(bool_c, block_map[then_blk], &[], block_map[else_blk], &[]);
        }
        Terminator::BrArgs { target, args } => {
            let arg_vals: Vec<_> = args.iter().map(|r| builder.use_var(var(*r))).collect();
            builder.ins().jump(block_map[target], &arg_vals);
        }
        Terminator::BrIfArgs {
            cond,
            then_blk,
            then_args,
            else_blk,
            else_args,
        } => {
            let vc = builder.use_var(var(*cond));
            let bool_c = builder.ins().icmp_imm(IntCC::NotEqual, vc, 0);
            let t_vals: Vec<_> = then_args.iter().map(|r| builder.use_var(var(*r))).collect();
            let e_vals: Vec<_> = else_args.iter().map(|r| builder.use_var(var(*r))).collect();
            builder.ins().brif(
                bool_c,
                block_map[then_blk],
                &t_vals,
                block_map[else_blk],
                &e_vals,
            );
        }
    }
}
