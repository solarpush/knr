use knr_core::{BasicBlock, Effect, Function, Instruction, Op, Terminator, Type};
use knr_interp::{execute, Value, VmError};
use knr_runtime::{MockPlatform, RuntimeError};

#[test]
fn test_mock_platform_host_call() {
    let mut platform = MockPlatform::new();
    platform.set_response(Effect::Net, "net.listen", Value::I64(200));

    // %0 = const_i64 8080
    // %1 = syscall Net "net.listen" %0
    // ret %1
    let mut func = Function::new("syscall_test", Type::I64);
    func.add_effect(Effect::Net);
    let mut bb0 = BasicBlock::new(0, Terminator::Ret(Some(1)));
    bb0.push(Instruction::new(Some(0), Type::I64, Op::ConstI64(8080), 1));
    bb0.push(Instruction::new(
        Some(1),
        Type::I64,
        Op::Syscall(Effect::Net, "net.listen".into(), 0),
        2,
    ));
    func.add_block(bb0);

    let (res, _) = execute(&func, vec![], &mut platform, 100, false).expect("exec ok");
    assert_eq!(res, Value::I64(200));

    // Verify call recorded on platform
    assert_eq!(platform.call_count(), 1);
    let last = platform.last_call().unwrap();
    assert_eq!(last.effect, Effect::Net);
    assert_eq!(last.op, "net.listen");
    assert_eq!(last.payload, Value::I64(8080));
}

struct FailingPlatform;

impl knr_runtime::HostPlatform for FailingPlatform {
    fn host_call(
        &mut self,
        _effect: Effect,
        _op: &str,
        _payload: Value,
    ) -> Result<Value, RuntimeError> {
        Err(RuntimeError::Platform("Hardware failure".into()))
    }
}

#[test]
fn test_host_call_error_propagation() {
    let mut platform = FailingPlatform;

    let mut func = Function::new("failing_syscall", Type::Void);
    func.add_effect(Effect::Hw);
    let mut bb0 = BasicBlock::new(0, Terminator::Ret(None));
    bb0.push(Instruction::new(Some(0), Type::I64, Op::ConstI64(0), 1));
    bb0.push(Instruction::new(
        Some(1),
        Type::Void,
        Op::Syscall(Effect::Hw, "hw.reset".into(), 0),
        2,
    ));
    func.add_block(bb0);

    let err = execute(&func, vec![], &mut platform, 100, false).unwrap_err();
    match err {
        VmError::HostCallError(RuntimeError::Platform(msg)) => {
            assert_eq!(msg, "Hardware failure");
        }
        other => panic!("Expected HostCallError, got {other:?}"),
    }
}
