use crate::CodegenError;
use cranelift::codegen::ir::{types, AbiParam, InstBuilder, MemFlags, UserFuncName};
use cranelift::codegen::isa::TargetIsa;
use cranelift::codegen::settings::{self, Configurable, Flags};
use cranelift::codegen::Context;
use cranelift::prelude::{FloatCC, FunctionBuilder, FunctionBuilderContext, IntCC, Variable};
use cranelift_jit::{JITBuilder, JITModule};
use cranelift_module::{default_libcall_names, DataDescription, FuncId, Linkage, Module as _};
use cranelift_object::{ObjectBuilder, ObjectModule};
use knr_core::{Function, Instruction as KnrInst, Module, Op, RegId, Terminator, Type};
use std::collections::HashMap;
use std::sync::Arc;

#[inline]
fn var(reg: RegId) -> Variable {
    Variable::from_u32(reg)
}

/// Native code generator lowering SSA `Module` / `Function` using Cranelift.
/// Supports both in-memory JIT execution (`compile_jit`) and native object emission (`compile_object`).
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

    /// Compiles an SSA `Function` to executable machine code via Cranelift JIT.
    /// Returns a raw function pointer to the compiled machine code.
    pub fn compile_jit(&mut self, func: &Function) -> Result<*const u8, CodegenError> {
        let mut jit_builder = JITBuilder::with_isa(self.isa.clone(), default_libcall_names());

        // Register runtime symbols from knr-runtime
        jit_builder.symbol("rt_map_new", knr_runtime::rt_map_new as *const u8);
        jit_builder.symbol("rt_map_set", knr_runtime::rt_map_set as *const u8);
        jit_builder.symbol("rt_map_get", knr_runtime::rt_map_get as *const u8);
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

        let mut module = JITModule::new(jit_builder);
        let runtime_funcs = declare_runtime_symbols(&mut module)?;

        let mut ctx = module.make_context();
        let func_id = compile_function_into_module(&mut module, &mut ctx, func, &runtime_funcs)?;

        module
            .finalize_definitions()
            .map_err(|e| CodegenError::Cranelift(e.to_string()))?;

        let code_ptr = module.get_finalized_function(func_id);
        self.jit_module = Some(module);

        Ok(code_ptr)
    }

    /// Compiles an SSA `Module` into an AOT native object file (`.o`) via `cranelift-object`.
    pub fn compile_object(&mut self, module: &Module) -> Result<Vec<u8>, CodegenError> {
        let obj_builder =
            ObjectBuilder::new(self.isa.clone(), "knr_module", default_libcall_names())?;
        let mut obj_module = ObjectModule::new(obj_builder);
        let runtime_funcs = declare_runtime_symbols(&mut obj_module)?;

        let mut ctx = obj_module.make_context();
        for func in &module.functions {
            compile_function_into_module(&mut obj_module, &mut ctx, func, &runtime_funcs)?;
            ctx.clear();
        }

        let product = obj_module.finish();
        let bytes = product
            .emit()
            .map_err(|e| CodegenError::Cranelift(e.to_string()))?;
        Ok(bytes)
    }
}

/// Declares external C runtime symbols in the Cranelift module.
fn declare_runtime_symbols<M: cranelift_module::Module>(
    module: &mut M,
) -> Result<HashMap<&'static str, FuncId>, CodegenError> {
    let mut funcs = HashMap::new();

    // rt_map_new: () -> i64
    let mut sig = module.make_signature();
    sig.returns.push(AbiParam::new(types::I64));
    let id = module.declare_function("rt_map_new", Linkage::Import, &sig)?;
    funcs.insert("rt_map_new", id);

    // rt_map_set: (i64, i64, i64, i64) -> i64
    let mut sig = module.make_signature();
    sig.params.push(AbiParam::new(types::I64));
    sig.params.push(AbiParam::new(types::I64));
    sig.params.push(AbiParam::new(types::I64));
    sig.params.push(AbiParam::new(types::I64));
    sig.returns.push(AbiParam::new(types::I64));
    let id = module.declare_function("rt_map_set", Linkage::Import, &sig)?;
    funcs.insert("rt_map_set", id);

    // rt_map_get: (i64, i64, i64) -> i64
    let mut sig = module.make_signature();
    sig.params.push(AbiParam::new(types::I64));
    sig.params.push(AbiParam::new(types::I64));
    sig.params.push(AbiParam::new(types::I64));
    sig.returns.push(AbiParam::new(types::I64));
    let id = module.declare_function("rt_map_get", Linkage::Import, &sig)?;
    funcs.insert("rt_map_get", id);

    // rt_map_free: (i64) -> ()
    let mut sig = module.make_signature();
    sig.params.push(AbiParam::new(types::I64));
    let id = module.declare_function("rt_map_free", Linkage::Import, &sig)?;
    funcs.insert("rt_map_free", id);

    // rt_vec_new: () -> i64
    let mut sig = module.make_signature();
    sig.returns.push(AbiParam::new(types::I64));
    let id = module.declare_function("rt_vec_new", Linkage::Import, &sig)?;
    funcs.insert("rt_vec_new", id);

    // rt_vec_push: (i64, i64) -> i64
    let mut sig = module.make_signature();
    sig.params.push(AbiParam::new(types::I64));
    sig.params.push(AbiParam::new(types::I64));
    sig.returns.push(AbiParam::new(types::I64));
    let id = module.declare_function("rt_vec_push", Linkage::Import, &sig)?;
    funcs.insert("rt_vec_push", id);

    // rt_vec_get: (i64, i64) -> i64
    let mut sig = module.make_signature();
    sig.params.push(AbiParam::new(types::I64));
    sig.params.push(AbiParam::new(types::I64));
    sig.returns.push(AbiParam::new(types::I64));
    let id = module.declare_function("rt_vec_get", Linkage::Import, &sig)?;
    funcs.insert("rt_vec_get", id);

    // rt_vec_len: (i64) -> i64
    let mut sig = module.make_signature();
    sig.params.push(AbiParam::new(types::I64));
    sig.returns.push(AbiParam::new(types::I64));
    let id = module.declare_function("rt_vec_len", Linkage::Import, &sig)?;
    funcs.insert("rt_vec_len", id);

    // rt_vec_free: (i64) -> ()
    let mut sig = module.make_signature();
    sig.params.push(AbiParam::new(types::I64));
    let id = module.declare_function("rt_vec_free", Linkage::Import, &sig)?;
    funcs.insert("rt_vec_free", id);

    // rt_buf_alloc_aligned: (i64, i64) -> i64
    let mut sig = module.make_signature();
    sig.params.push(AbiParam::new(types::I64));
    sig.params.push(AbiParam::new(types::I64));
    sig.returns.push(AbiParam::new(types::I64));
    let id = module.declare_function("rt_buf_alloc_aligned", Linkage::Import, &sig)?;
    funcs.insert("rt_buf_alloc_aligned", id);

    // rt_buf_free: (i64, i64, i64) -> ()
    let mut sig = module.make_signature();
    sig.params.push(AbiParam::new(types::I64));
    sig.params.push(AbiParam::new(types::I64));
    sig.params.push(AbiParam::new(types::I64));
    let id = module.declare_function("rt_buf_free", Linkage::Import, &sig)?;
    funcs.insert("rt_buf_free", id);

    Ok(funcs)
}

/// Translates a knr SSA Type to a Cranelift IR Type.
fn clif_type(ty: &Type) -> types::Type {
    match ty {
        Type::I64 | Type::Bool => types::I64,
        Type::F64 => types::F64,
        Type::Void => types::I64,
        // Linear types (Buf, Str, Vec, Map, Chan, Struct) mapped to 64-bit pointers
        Type::Buf | Type::Str | Type::Vec(_) | Type::Map(_, _) | Type::Chan(_) | Type::Struct(_) => types::I64,
    }
}

/// Compiles a single SSA Function into a Cranelift module.
fn compile_function_into_module<M: cranelift_module::Module>(
    module: &mut M,
    ctx: &mut Context,
    func: &Function,
    runtime_funcs: &HashMap<&'static str, FuncId>,
) -> Result<FuncId, CodegenError> {
    let mut sig = module.make_signature();
    for (_, ty) in &func.params {
        sig.params.push(AbiParam::new(clif_type(ty)));
    }
    if func.ret_ty != Type::Void {
        sig.returns.push(AbiParam::new(clif_type(&func.ret_ty)));
    }

    let func_id = module.declare_function(&func.name, Linkage::Export, &sig)?;

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
        return Ok(func_id);
    }

    let entry_block = block_map[&func.blocks[0].id];
    builder.append_block_params_for_function_params(entry_block);

    // Record types for all registers
    let mut reg_types: HashMap<RegId, Type> = HashMap::new();
    for (reg, ty) in &func.params {
        reg_types.insert(*reg, ty.clone());
    }
    for b in &func.blocks {
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

    // Static string counter for data segments
    let mut str_counter = 0usize;

    // Translate all basic blocks
    for b in &func.blocks {
        let clif_block = block_map[&b.id];
        builder.switch_to_block(clif_block);

        for inst in &b.instructions {
            emit_instruction(
                module,
                &mut builder,
                &func.name,
                inst,
                &reg_types,
                runtime_funcs,
                &mut str_counter,
            )?;
        }

        emit_terminator(&mut builder, &b.terminator, &block_map);
    }

    builder.seal_all_blocks();
    builder.finalize();

    module.define_function(func_id, ctx)?;

    Ok(func_id)
}

fn emit_instruction<M: cranelift_module::Module>(
    module: &mut M,
    builder: &mut FunctionBuilder,
    func_name: &str,
    inst: &KnrInst,
    reg_types: &HashMap<RegId, Type>,
    runtime_funcs: &HashMap<&'static str, FuncId>,
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
            let data_name = format!("{}_str_{}", func_name, *str_counter);
            *str_counter += 1;
            let mut data_desc = DataDescription::new();
            let mut bytes = s.as_bytes().to_vec();
            bytes.push(0);
            data_desc.define(bytes.into_boxed_slice());
            let data_id =
                module.declare_data(&data_name, Linkage::Local, false, false)?;
            module.define_data(data_id, &data_desc)?;

            let local_data = module.declare_data_in_func(data_id, builder.func);
            let res = builder.ins().symbol_value(types::I64, local_data);
            if let Some(dst) = dst_var {
                builder.def_var(dst, res);
            }
        }
        Op::Add(a, b) => {
            let va = builder.use_var(var(*a));
            let vb = builder.use_var(var(*b));
            let res = if inst.ty == Type::F64 {
                builder.ins().fadd(va, vb)
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
        Op::CmpEq(a, b) => {
            let va = builder.use_var(var(*a));
            let vb = builder.use_var(var(*b));
            let ty_a = reg_types.get(a).unwrap_or(&Type::I64);
            let cmp = if *ty_a == Type::F64 {
                builder.ins().fcmp(FloatCC::Equal, va, vb)
            } else {
                builder.ins().icmp(IntCC::Equal, va, vb)
            };
            let res = builder.ins().uextend(types::I64, cmp);
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
            // Returns consumed buffer
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
            let v_val = builder.use_var(var(*val));
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
            let res = builder.inst_results(call)[0];
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
            let vv = builder.use_var(var(*v));
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
            let res = builder.inst_results(call)[0];
            if let Some(dst) = dst_var {
                builder.def_var(dst, res);
            }
        }
        Op::MapDel(m, _k) => {
            // map_del returns consumed map
            let vm = builder.use_var(var(*m));
            if let Some(dst) = dst_var {
                builder.def_var(dst, vm);
            }
        }
        Op::ChanNew(_, _) => {
            let res = builder.ins().iconst(types::I64, 1);
            if let Some(dst) = dst_var {
                builder.def_var(dst, res);
            }
        }
        Op::ChanSend(_, _) => {
            let res = builder.ins().iconst(types::I64, 1);
            if let Some(dst) = dst_var {
                builder.def_var(dst, res);
            }
        }
        Op::ChanRecv(_) => {
            let res = builder.ins().iconst(types::I64, 0);
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
        Op::Syscall(_, _, _) => {
            let res = match inst.ty {
                Type::F64 => builder.ins().f64const(0.0),
                _ => builder.ins().iconst(types::I64, 0),
            };
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
                _ => {
                    let v_size = builder.ins().iconst(types::I64, 0);
                    let v_align = builder.ins().iconst(types::I64, 64);
                    let local_callee =
                        module.declare_func_in_func(runtime_funcs["rt_buf_free"], builder.func);
                    builder.ins().call(local_callee, &[vr, v_size, v_align]);
                }
            }
        }
        Op::StructNew(_) | Op::StructGet(_, _) | Op::StructSet(_, _, _) => {
            return Err(CodegenError::Cranelift(format!("Unsupported struct opcode: {:?}", inst.op)));
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
    }
}
