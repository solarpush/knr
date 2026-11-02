use crate::diagnostic::{format_instruction, Diagnostic};
use knr_core::{Effect, Function, Module, Op};

/// Validates effect capabilities and isolation:
/// 1. Pure functions cannot execute instructions with unlisted effects (Net, Ui, Hw, Async).
/// 2. Syscall operations (`syscall <Effect> <op>`) must match declared function effect annotations.
/// 3. Async operations (`chan_new`, `spawn`) require `Async` effect.
pub fn verify_effects(func: &Function) -> Result<(), Diagnostic> {
    for block in &func.blocks {
        for inst in &block.instructions {
            match &inst.op {
                Op::Syscall(effect, _op_name, _payload) => {
                    if !func.effects.contains(effect) {
                        return Err(Diagnostic::new(
                            "UndeclaredEffect",
                            &func.name,
                            inst.line,
                            format_instruction(inst),
                        )
                        .with_fix_hint(format!(
                            "Function '{}' calls syscall with effect {effect} but does not declare effects({effect})",
                            func.name
                        )));
                    }
                }
                Op::ChanNew(_, _) if !func.effects.contains(&Effect::Async) => {
                    return Err(Diagnostic::new(
                        "UndeclaredEffect",
                        &func.name,
                        inst.line,
                        format_instruction(inst),
                    )
                    .with_fix_hint(format!(
                        "Function '{}' creates a channel with chan_new but does not declare effects(Async)",
                        func.name
                    )));
                }
                Op::Spawn(target_fn, _) if !func.effects.contains(&Effect::Async) => {
                    return Err(Diagnostic::new(
                        "UndeclaredEffect",
                        &func.name,
                        inst.line,
                        format_instruction(inst),
                    )
                    .with_fix_hint(format!(
                        "Function '{}' spawns task '{target_fn}' but does not declare effects(Async)",
                        func.name
                    )));
                }
                _ => {}
            }
        }
    }

    Ok(())
}

/// Verifies inter-procedural effects across a Module.
/// If function A spawns or calls function B, A must have a superset of B's effects.
pub fn verify_module_effects(module: &Module) -> Result<(), Diagnostic> {
    for func in &module.functions {
        for block in &func.blocks {
            for inst in &block.instructions {
                if let Op::Spawn(target_fn, _) | Op::Call(target_fn, _) = &inst.op {
                    if let Some(target) = module.get_function(target_fn) {
                        for eff in &target.effects {
                            if !func.effects.contains(eff) {
                                let action = if matches!(inst.op, Op::Spawn(..)) {
                                    "spawns"
                                } else {
                                    "calls"
                                };
                                return Err(Diagnostic::new(
                                    "UndeclaredEffect",
                                    &func.name,
                                    inst.line,
                                    format_instruction(inst),
                                )
                                .with_fix_hint(format!(
                                    "Function '{}' {action} effectful function '{}' requiring effect {eff}",
                                    func.name, target_fn
                                )));
                            }
                        }
                    }
                }
            }
        }
    }
    Ok(())
}
