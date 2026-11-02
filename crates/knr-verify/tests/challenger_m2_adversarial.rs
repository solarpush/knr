use knr_core::parse_ssa;
use knr_verify::verify_module;

#[test]
fn test_linear_arg_consumed_cannot_use_after_move() {
    let ssa = r#"
fn consume(%0: buf) -> void {
bb0:
  free %0
  ret
}

fn main() -> void {
bb0:
  %0 = const_i64 128
  %1 = buf_alloc %0
  call consume %1
  free %1
  ret
}
"#;
    let module = parse_ssa(ssa).expect("Parse SSA");
    let errs = verify_module(&module).unwrap_err();
    assert!(
        errs.iter().any(|d| d.error_kind == "UseAfterMove"),
        "Expected UseAfterMove when buffer is used after being moved to call: {:?}",
        errs
    );
}

#[test]
fn test_linear_same_arg_passed_twice_rejected() {
    let ssa = r#"
fn take_two(%0: buf, %1: buf) -> void {
bb0:
  free %0
  free %1
  ret
}

fn main() -> void {
bb0:
  %0 = const_i64 64
  %1 = buf_alloc %0
  call take_two %1 %1
  ret
}
"#;
    let module = parse_ssa(ssa).expect("Parse SSA");
    let errs = verify_module(&module).unwrap_err();
    let diag = errs.iter().find(|d| d.error_kind == "UseAfterMove").expect("UseAfterMove expected");
    assert_eq!(diag.violated_register.as_deref(), Some("%1"));
}

#[test]
fn test_linear_struct_passed_to_call_consumed() {
    let ssa = r#"
schema Packet {
  0: id i64,
}

fn process_packet(%0: struct[Packet]) -> void {
bb0:
  free %0
  ret
}

fn main() -> i64 {
bb0:
  %0 = struct_new Packet
  %1 = const_i64 100
  %2 = struct_set %0 0 %1
  call process_packet %2
  %3 = struct_get %2 0
  ret %3
}
"#;
    let module = parse_ssa(ssa).expect("Parse SSA");
    let errs = verify_module(&module).unwrap_err();
    assert!(
        errs.iter().any(|d| d.error_kind == "UseAfterMove"),
        "Expected UseAfterMove when struct is accessed after passing to call: {:?}",
        errs
    );
}

#[test]
fn test_non_linear_same_arg_passed_twice_allowed() {
    let ssa = r#"
fn add_dup(%0: i64, %1: i64) -> i64 {
bb0:
  %2 = add %0 %1
  ret %2
}

fn main() -> i64 {
bb0:
  %0 = const_i64 21
  %1 = call add_dup %0 %0
  ret %1
}
"#;
    let module = parse_ssa(ssa).expect("Parse SSA");
    assert!(
        verify_module(&module).is_ok(),
        "Passing same non-linear register twice should be valid"
    );
}

#[test]
fn test_linear_return_marked_alive_unfreed_triggers_linear_leak() {
    let ssa = r#"
fn create_buffer() -> buf {
bb0:
  %0 = const_i64 32
  %1 = buf_alloc %0
  ret %1
}

fn main() -> void {
bb0:
  %0 = call create_buffer
  ret
}
"#;
    let module = parse_ssa(ssa).expect("Parse SSA");
    let errs = verify_module(&module).unwrap_err();
    assert!(
        errs.iter().any(|d| d.error_kind == "LinearLeak"),
        "Expected LinearLeak when linear return %0 is neither freed nor returned: {:?}",
        errs
    );
}

#[test]
fn test_linear_return_marked_alive_freed_succeeds() {
    let ssa = r#"
fn create_buffer() -> buf {
bb0:
  %0 = const_i64 32
  %1 = buf_alloc %0
  ret %1
}

fn main() -> void {
bb0:
  %0 = call create_buffer
  free %0
  ret
}
"#;
    let module = parse_ssa(ssa).expect("Parse SSA");
    assert!(
        verify_module(&module).is_ok(),
        "Freeing linear return %0 must pass linear verification"
    );
}

#[test]
fn test_discarding_linear_return_triggers_linear_leak_or_type_mismatch() {
    let ssa = r#"
fn create_buffer() -> buf {
bb0:
  %0 = const_i64 32
  %1 = buf_alloc %0
  ret %1
}

fn main() -> void {
bb0:
  call create_buffer
  ret
}
"#;
    let module = parse_ssa(ssa).expect("Parse SSA");
    let errs = verify_module(&module).unwrap_err();
    assert!(
        errs.iter().any(|d| d.error_kind == "LinearLeak" || d.error_kind == "TypeMismatch"),
        "Expected LinearLeak or TypeMismatch when linear return is discarded: {:?}",
        errs
    );
}

#[test]
fn test_discarding_non_linear_return_triggers_type_mismatch() {
    let ssa = r#"
fn get_num() -> i64 {
bb0:
  %0 = const_i64 42
  ret %0
}

fn main() -> void {
bb0:
  call get_num
  ret
}
"#;
    let module = parse_ssa(ssa).expect("Parse SSA");
    let errs = verify_module(&module).unwrap_err();
    assert!(
        errs.iter().any(|d| d.error_kind == "TypeMismatch"),
        "Expected TypeMismatch when non-void return is discarded: {:?}",
        errs
    );
}

#[test]
fn test_void_call_with_destination_triggers_type_mismatch() {
    let ssa = r#"
fn do_nothing() -> void {
bb0:
  ret
}

fn main() -> void {
bb0:
  %0 = call do_nothing
  ret
}
"#;
    let module = parse_ssa(ssa).expect("Parse SSA");
    let errs = verify_module(&module).unwrap_err();
    assert!(
        errs.iter().any(|d| d.error_kind == "TypeMismatch"),
        "Expected TypeMismatch when void function has destination: {:?}",
        errs
    );
}

#[test]
fn test_call_non_dominating_register_triggers_dominance_violation() {
    let ssa = r#"
fn callee(%0: i64) -> i64 {
bb0:
  ret %0
}

fn main() -> i64 {
bb0:
  %0 = const_bool false
  br_if %0 bb1 bb2

bb1:
  %1 = const_i64 99
  br bb3

bb2:
  %2 = const_i64 88
  br bb3

bb3:
  %3 = call callee %1
  ret %3
}
"#;
    let module = parse_ssa(ssa).expect("Parse SSA");
    let errs = verify_module(&module).unwrap_err();
    assert!(
        errs.iter().any(|d| d.error_kind == "DominanceViolation"),
        "Expected DominanceViolation for non-dominating call argument: {:?}",
        errs
    );
}

#[test]
fn test_call_effectful_callee_from_pure_caller_triggers_undeclared_effect() {
    let ssa = r#"
fn effectful() -> void effects(Net) {
bb0:
  %0 = const_i64 8080
  %1 = syscall Net "net.listen" %0
  ret
}

fn pure_caller() -> void {
bb0:
  call effectful
  ret
}
"#;
    let module = parse_ssa(ssa).expect("Parse SSA");
    let errs = verify_module(&module).unwrap_err();
    assert!(
        errs.iter().any(|d| d.error_kind == "UndeclaredEffect"),
        "Expected UndeclaredEffect when pure function calls effectful callee: {:?}",
        errs
    );
}

#[test]
fn test_call_effectful_callee_with_declared_effects_succeeds() {
    let ssa = r#"
fn effectful() -> void effects(Net) {
bb0:
  %0 = const_i64 8080
  %1 = syscall Net "net.listen" %0
  ret
}

fn caller_with_effects() -> void effects(Net, Ui) {
bb0:
  call effectful
  ret
}
"#;
    let module = parse_ssa(ssa).expect("Parse SSA");
    assert!(
        verify_module(&module).is_ok(),
        "Caller with superset effects must verify successfully"
    );
}
