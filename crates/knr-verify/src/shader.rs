//! Static GPU Shader Purity Verifier (`shader fn`).
//!
//! Ensures every `shader fn` (and any helper function it transitively calls) satisfies
//! WebGPU / WGSL / Vulkan / Metal execution constraints before codegen:
//! 1. Zero host effects (`Net`, `Ui`, `Hw`, `Async` forbidden)
//! 2. Zero dynamic heap allocations (`BufAlloc`, `VecNew`, `MapNew`, `ChanNew`, `ConstStr`, `Spawn`, `Syscall`)
//! 3. Zero recursion cycles in the shader call graph (GPUs forbid unbounded recursion)
//! 4. Only GPU-representable scalar or flat-struct types (no `str`, `buf`, `vec`, `map`, `chan`)

use std::collections::{HashMap, HashSet};

use knr_core::{format_instruction, Function, Op, Schema, Type};

use crate::diagnostic::Diagnostic;

pub fn verify_shader_function(
    func: &Function,
    schemas: &[Schema],
    functions: &[Function],
) -> Result<(), Diagnostic> {
    if !func.is_shader {
        return Ok(());
    }

    if !func.effects.is_empty() {
        return Err(Diagnostic::new(
            "ShaderEffectForbidden",
            &func.name,
            1,
            format!("shader fn {}", func.name),
        )
        .with_fix_hint("GPU shader functions must be pure and cannot declare host effects"));
    }

    let fn_map: HashMap<&str, &Function> = functions
        .iter()
        .map(|f| (f.name.as_str(), f))
        .collect();
    let schema_map: HashMap<&str, &Schema> = schemas
        .iter()
        .map(|s| (s.name.as_str(), s))
        .collect();

    // Check parameter and return types are GPU-compatible
    for (reg, ty) in &func.params {
        if !is_gpu_type(ty, &schema_map, &mut HashSet::new()) {
            return Err(Diagnostic::new(
                "ShaderUnsupportedType",
                &func.name,
                1,
                format!("param %{reg}: {ty}"),
            )
            .with_violated_register(format!("%{reg}"))
            .with_fix_hint(format!(
                "GPU shader parameter type '{ty}' is not GPU-representable (use f64, i64, bool, or flat structs like Vec2/Color)"
            )));
        }
    }
    if !is_gpu_type(&func.ret_ty, &schema_map, &mut HashSet::new()) {
        return Err(Diagnostic::new(
            "ShaderUnsupportedType",
            &func.name,
            1,
            format!("-> {}", func.ret_ty),
        )
        .with_fix_hint(format!(
            "GPU shader return type '{}' is not GPU-representable (use Color, f64, i64, bool, or flat structs)",
            func.ret_ty
        )));
    }

    let mut visited = HashSet::new();
    let mut stack = Vec::new();
    verify_shader_body_recursive(func, &fn_map, &schema_map, &mut visited, &mut stack)
}

fn verify_shader_body_recursive<'a>(
    func: &'a Function,
    fn_map: &HashMap<&'a str, &'a Function>,
    schema_map: &HashMap<&'a str, &'a Schema>,
    visited: &mut HashSet<&'a str>,
    stack: &mut Vec<&'a str>,
) -> Result<(), Diagnostic> {
    if stack.contains(&func.name.as_str()) {
        return Err(Diagnostic::new(
            "ShaderRecursionForbidden",
            &func.name,
            1,
            format!("shader fn {}", func.name),
        )
        .with_fix_hint("GPU shaders cannot contain direct or mutual recursion"));
    }
    if !visited.insert(func.name.as_str()) {
        return Ok(());
    }
    stack.push(func.name.as_str());

    for block in &func.blocks {
        for inst in &block.instructions {
            match &inst.op {
                Op::ConstStr(_)
                | Op::IntToStr(_)
                | Op::StrLen(_)
                | Op::BufAlloc(_)
                | Op::BufReadU8(_, _)
                | Op::BufWriteU8(_, _, _)
                | Op::BufSlice(_, _, _)
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
                    return Err(Diagnostic::new(
                        "ShaderImpureOp",
                        &func.name,
                        inst.line,
                        format_instruction(inst),
                    )
                    .with_fix_hint(
                        "Operation is not allowed inside a GPU shader (strings, dynamic containers, channels, and syscalls are forbidden on GPU)",
                    ));
                }
                Op::Call(callee_name, _) => {
                    if let Some(&callee) = fn_map.get(callee_name.as_str()) {
                        if !callee.effects.is_empty() {
                            return Err(Diagnostic::new(
                                "ShaderEffectForbidden",
                                &func.name,
                                inst.line,
                                format_instruction(inst),
                            )
                            .with_fix_hint(format!(
                                "GPU shader cannot call effectful function '{callee_name}'"
                            )));
                        }
                        verify_shader_body_recursive(callee, fn_map, schema_map, visited, stack)?;
                    }
                }
                _ => {
                    if inst.ty != Type::Void && !is_gpu_type(&inst.ty, schema_map, &mut HashSet::new()) {
                        return Err(Diagnostic::new(
                            "ShaderUnsupportedType",
                            &func.name,
                            inst.line,
                            format_instruction(inst),
                        )
                        .with_fix_hint(format!(
                            "Type '{}' is not supported inside GPU shaders",
                            inst.ty
                        )));
                    }
                }
            }
        }
    }

    stack.pop();
    Ok(())
}

fn is_gpu_type<'a>(
    ty: &'a Type,
    schemas: &HashMap<&'a str, &'a Schema>,
    seen: &mut HashSet<&'a str>,
) -> bool {
    match ty {
        Type::I64 | Type::F64 | Type::Bool | Type::Void => true,
        Type::Struct(name) => {
            if !seen.insert(name.as_str()) {
                return false;
            }
            let ok = schemas.get(name.as_str()).is_some_and(|sc| {
                sc.fields.iter().all(|f| is_gpu_type(&f.ty, schemas, seen))
            });
            seen.remove(name.as_str());
            ok
        }
        Type::Str | Type::Buf | Type::Vec(_) | Type::Map(_, _) | Type::Chan(_) => false,
    }
}
