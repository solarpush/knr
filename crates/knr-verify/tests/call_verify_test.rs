use knr_core::parse_ssa;
use knr_verify::verify_module;

#[test]
fn test_verify_call_valid() {
    let ssa = r#"
fn add(%0: i64, %1: i64) -> i64 {
bb0:
  %2 = add %0 %1
  ret %2
}

fn main() -> i64 {
bb0:
  %0 = const_i64 10
  %1 = const_i64 20
  %2 = call add %0 %1
  ret %2
}
"#;
    let module = parse_ssa(ssa).expect("Parse SSA");
    let result = verify_module(&module);
    assert!(result.is_ok(), "Expected valid verification: {:?}", result);
}

#[test]
fn test_verify_call_undefined_function() {
    let ssa = r#"
fn main() -> i64 {
bb0:
  %0 = const_i64 10
  %1 = call undefined_func %0
  ret %1
}
"#;
    let module = parse_ssa(ssa).expect("Parse SSA");
    let errs = verify_module(&module).unwrap_err();
    assert!(
        errs.iter().any(|d| d.error_kind == "UndefinedFunction"),
        "Expected UndefinedFunction diagnostic, got: {:?}",
        errs
    );
}

#[test]
fn test_verify_call_arity_mismatch() {
    let ssa = r#"
fn take_two(%0: i64, %1: i64) -> i64 {
bb0:
  %2 = add %0 %1
  ret %2
}

fn main() -> i64 {
bb0:
  %0 = const_i64 10
  %1 = call take_two %0
  ret %1
}
"#;
    let module = parse_ssa(ssa).expect("Parse SSA");
    let errs = verify_module(&module).unwrap_err();
    assert!(
        errs.iter().any(|d| d.error_kind == "TypeMismatch"),
        "Expected TypeMismatch for arity, got: {:?}",
        errs
    );
}

#[test]
fn test_verify_call_argument_type_mismatch() {
    let ssa = r#"
fn takes_bool(%0: bool) -> i64 {
bb0:
  %1 = const_i64 1
  ret %1
}

fn main() -> i64 {
bb0:
  %0 = const_i64 42
  %1 = call takes_bool %0
  ret %1
}
"#;
    let module = parse_ssa(ssa).expect("Parse SSA");
    let errs = verify_module(&module).unwrap_err();
    let diag = errs.iter().find(|d| d.error_kind == "TypeMismatch").expect("TypeMismatch expected");
    assert_eq!(diag.violated_register.as_deref(), Some("%0"));
}

#[test]
fn test_verify_call_void_with_destination() {
    let ssa = r#"
fn void_target() -> void {
bb0:
  ret
}

fn main() -> i64 {
bb0:
  %0 = call void_target
  %1 = const_i64 0
  ret %1
}
"#;
    let module = parse_ssa(ssa).expect("Parse SSA");
    let errs = verify_module(&module).unwrap_err();
    assert!(
        errs.iter().any(|d| d.error_kind == "TypeMismatch"),
        "Expected TypeMismatch when void function has dst, got: {:?}",
        errs
    );
}

#[test]
fn test_verify_call_non_void_without_destination() {
    let ssa = r#"
fn returns_int() -> i64 {
bb0:
  %0 = const_i64 42
  ret %0
}

fn main() -> void {
bb0:
  call returns_int
  ret
}
"#;
    let module = parse_ssa(ssa).expect("Parse SSA");
    let errs = verify_module(&module).unwrap_err();
    assert!(
        errs.iter().any(|d| d.error_kind == "TypeMismatch"),
        "Expected TypeMismatch when non-void function called without dst, got: {:?}",
        errs
    );
}

#[test]
fn test_verify_call_ssa_dominance() {
    let ssa = r#"
fn target(%0: i64) -> i64 {
bb0:
  ret %0
}

fn main() -> i64 {
bb0:
  %0 = const_bool true
  br_if %0 bb1 bb2

bb1:
  %1 = const_i64 100
  br bb3

bb2:
  %2 = const_i64 200
  br bb3

bb3:
  %3 = call target %1
  ret %3
}
"#;
    let module = parse_ssa(ssa).expect("Parse SSA");
    let errs = verify_module(&module).unwrap_err();
    assert!(
        errs.iter().any(|d| d.error_kind == "DominanceViolation"),
        "Expected DominanceViolation for call argument %1 in bb3, got: {:?}",
        errs
    );
}

#[test]
fn test_verify_call_linear_argument_ownership_transfer() {
    let ssa = r#"
fn consume_buf(%0: buf) -> void {
bb0:
  free %0
  ret
}

fn main() -> void {
bb0:
  %0 = const_i64 64
  %1 = buf_alloc %0
  call consume_buf %1
  free %1
  ret
}
"#;
    let module = parse_ssa(ssa).expect("Parse SSA");
    let errs = verify_module(&module).unwrap_err();
    assert!(
        errs.iter().any(|d| d.error_kind == "UseAfterMove"),
        "Expected UseAfterMove when using buffer after passing to call, got: {:?}",
        errs
    );
}

#[test]
fn test_verify_call_linear_argument_double_pass() {
    let ssa = r#"
fn take_two_bufs(%0: buf, %1: buf) -> void {
bb0:
  free %0
  free %1
  ret
}

fn main() -> void {
bb0:
  %0 = const_i64 64
  %1 = buf_alloc %0
  call take_two_bufs %1 %1
  ret
}
"#;
    let module = parse_ssa(ssa).expect("Parse SSA");
    let errs = verify_module(&module).unwrap_err();
    assert!(
        errs.iter().any(|d| d.error_kind == "UseAfterMove"),
        "Expected UseAfterMove when passing same linear buffer twice, got: {:?}",
        errs
    );
}

#[test]
fn test_verify_call_linear_leak_when_discarded() {
    let ssa = r#"
fn make_buf() -> buf {
bb0:
  %0 = const_i64 64
  %1 = buf_alloc %0
  ret %1
}

fn main() -> void {
bb0:
  call make_buf
  ret
}
"#;
    let module = parse_ssa(ssa).expect("Parse SSA");
    let errs = verify_module(&module).unwrap_err();
    assert!(
        errs.iter().any(|d| d.error_kind == "LinearLeak" || d.error_kind == "TypeMismatch"),
        "Expected LinearLeak or TypeMismatch when linear return discarded, got: {:?}",
        errs
    );
}

#[test]
fn test_verify_call_transitive_effects_undeclared() {
    let ssa = r#"
fn effectful_callee() -> str effects(Ui) {
bb0:
  %0 = const_str "page"
  ret %0
}

fn pure_caller() -> str {
bb0:
  %0 = call effectful_callee
  ret %0
}
"#;
    let module = parse_ssa(ssa).expect("Parse SSA");
    let errs = verify_module(&module).unwrap_err();
    assert!(
        errs.iter().any(|d| d.error_kind == "UndeclaredEffect"),
        "Expected UndeclaredEffect when calling effectful function from pure caller, got: {:?}",
        errs
    );
}

#[test]
fn test_verify_call_transitive_effects_declared() {
    let ssa = r#"
fn effectful_callee() -> str effects(Ui) {
bb0:
  %0 = const_str "page"
  ret %0
}

fn effectful_caller() -> str effects(Ui) {
bb0:
  %0 = call effectful_callee
  ret %0
}
"#;
    let module = parse_ssa(ssa).expect("Parse SSA");
    let result = verify_module(&module);
    assert!(
        result.is_ok(),
        "Expected verification to pass when effects match: {:?}",
        result
    );
}
