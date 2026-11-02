use knr_core::{
    decode_knrbin, encode_knrbin, format_ssa, parse_ssa, Op, Type,
};

#[test]
fn test_parse_call_void_zero_args() {
    let ssa = r#"
fn no_op() -> void {
bb0:
  ret
}

fn main() -> void {
bb0:
  call no_op
  ret
}
"#;
    let module = parse_ssa(ssa).expect("Failed to parse void zero-arg call");
    assert_eq!(module.functions.len(), 2);
    let main_fn = module.get_function("main").unwrap();
    assert_eq!(main_fn.blocks[0].instructions.len(), 1);
    let inst = &main_fn.blocks[0].instructions[0];
    assert_eq!(inst.dst, None);
    assert_eq!(inst.ty, Type::Void);
    assert_eq!(inst.op, Op::Call("no_op".to_string(), vec![]));
}

#[test]
fn test_parse_call_with_return_and_args() {
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
    let module = parse_ssa(ssa).expect("Failed to parse call with args and return");
    let main_fn = module.get_function("main").unwrap();
    let inst = &main_fn.blocks[0].instructions[2];
    assert_eq!(inst.dst, Some(2));
    assert_eq!(inst.ty, Type::I64);
    assert_eq!(inst.op, Op::Call("add".to_string(), vec![0, 1]));
}

#[test]
fn test_parse_call_forward_reference() {
    let ssa = r#"
fn main() -> i64 {
bb0:
  %0 = call helper
  ret %0
}

fn helper() -> i64 {
bb0:
  %0 = const_i64 42
  ret %0
}
"#;
    let module = parse_ssa(ssa).expect("Failed to parse forward reference call");
    let main_fn = module.get_function("main").unwrap();
    let inst = &main_fn.blocks[0].instructions[0];
    assert_eq!(inst.dst, Some(0));
    assert_eq!(inst.ty, Type::I64);
    assert_eq!(inst.op, Op::Call("helper".to_string(), vec![]));
}

#[test]
fn test_parse_call_quoted_function_name() {
    let ssa = r#"
fn "service.compute"(%0: i64) -> i64 {
bb0:
  ret %0
}

fn main() -> i64 {
bb0:
  %0 = const_i64 100
  %1 = call "service.compute" %0
  ret %1
}
"#;
    let module = parse_ssa(ssa).expect("Failed to parse call with quoted function name");
    let main_fn = module.get_function("main").unwrap();
    let inst = &main_fn.blocks[0].instructions[1];
    assert_eq!(inst.dst, Some(1));
    assert_eq!(inst.ty, Type::I64);
    assert_eq!(inst.op, Op::Call("service.compute".to_string(), vec![0]));
}

#[test]
fn test_call_textual_format_roundtrip() {
    let ssa = r#"fn compute(%0: i64, %1: i64, %2: i64) -> i64 {
bb0:
    %3 = add %0 %1
    %4 = add %3 %2
    ret %4
}

fn main() -> i64 {
bb0:
    %0 = const_i64 1
    %1 = const_i64 2
    %2 = const_i64 3
    %3 = call compute %0 %1 %2
    ret %3
}
"#;
    let module = parse_ssa(ssa).expect("Failed to parse SSA");
    let formatted = format_ssa(&module);
    let roundtripped = parse_ssa(&formatted).expect("Failed to re-parse formatted SSA");
    assert_eq!(module, roundtripped);
}

#[test]
fn test_call_binary_roundtrip_various_argc() {
    let ssa = r#"fn target0() -> void {
bb0:
    ret
}

fn target1(%0: i64) -> i64 {
bb0:
    ret %0
}

fn target2(%0: i64, %1: i64) -> i64 {
bb0:
    %2 = add %0 %1
    ret %2
}

fn target3(%0: i64, %1: i64, %2: i64) -> i64 {
bb0:
    ret %0
}

fn target5(%0: i64, %1: i64, %2: i64, %3: i64, %4: i64) -> i64 {
bb0:
    ret %0
}

fn caller() -> i64 {
bb0:
    call target0
    %0 = const_i64 10
    %1 = call target1 %0
    %2 = const_i64 20
    %3 = call target2 %0 %2
    %4 = const_i64 30
    %5 = call target3 %0 %2 %4
    %6 = const_i64 40
    %7 = const_i64 50
    %8 = call target5 %0 %2 %4 %6 %7
    ret %8
}
"#;
    let module = parse_ssa(ssa).expect("Parse multi-arg caller");
    let bytes = encode_knrbin(&module).expect("Encode knrbin");
    let decoded = decode_knrbin(&bytes).expect("Decode knrbin");

    assert_eq!(module.functions.len(), decoded.functions.len());
    let caller_orig = module.get_function("caller").unwrap();
    let caller_dec = decoded.get_function("caller").unwrap();

    let orig_ops: Vec<_> = caller_orig.blocks[0]
        .instructions
        .iter()
        .map(|i| (i.dst, &i.ty, &i.op))
        .collect();
    let dec_ops: Vec<_> = caller_dec.blocks[0]
        .instructions
        .iter()
        .map(|i| (i.dst, &i.ty, &i.op))
        .collect();
    assert_eq!(orig_ops, dec_ops);
    assert_eq!(format_ssa(&module), format_ssa(&decoded));
}
