use knr_core::binary::const_pool::{ConstantPool, TypePool};
use knr_core::binary::instruction::{pack_instruction, unpack_instruction, ExtraArgsPool, NO_REG};
use knr_core::{decode_knrbin, encode_knrbin, format_ssa, parse_ssa, Instruction, Op, Type};

#[test]
fn test_extra_args_pool_direct_call_args_roundtrip_0_1_2_5_10() {
    let cases: Vec<Vec<u32>> = vec![
        vec![],
        vec![42],
        vec![10, 20],
        vec![1, 2, 3, 4, 5],
        vec![100, 101, 102, 103, 104, 105, 106, 107, 108, 109],
    ];

    for (case_idx, args) in cases.iter().enumerate() {
        let mut pool = ExtraArgsPool::new();
        let idx = if args.is_empty() {
            NO_REG
        } else {
            pool.insert_call_args(args).expect("insert call args")
        };

        let decoded = pool.get_call_args(idx).expect("get call args");
        assert_eq!(
            &decoded, args,
            "ExtraArgsPool roundtrip mismatch for case {} (argc={})",
            case_idx,
            args.len()
        );
    }
}

#[test]
fn test_pack_unpack_call_instruction_0_1_2_5_10_args() {
    let arg_counts = [0, 1, 2, 5, 10];

    for &argc in &arg_counts {
        let args: Vec<u32> = (0..argc as u32).map(|i| i * 2 + 1).collect();
        let inst = Instruction {
            dst: Some(99),
            ty: Type::I64,
            op: Op::Call("target_func".to_string(), args.clone()),
            line: 42,
        };

        let mut const_pool = ConstantPool::new();
        let mut type_pool = TypePool::new();
        let mut extra_pool = ExtraArgsPool::new();

        let packed = pack_instruction(&inst, &mut const_pool, &mut type_pool, &mut extra_pool)
            .expect("pack call instruction");

        let unpacked = unpack_instruction(packed, &const_pool, &type_pool, &extra_pool, 42)
            .expect("unpack call instruction");

        assert_eq!(unpacked.dst, inst.dst);
        assert_eq!(unpacked.ty, inst.ty);
        assert_eq!(unpacked.line, inst.line);
        match unpacked.op {
            Op::Call(fn_name, unpacked_args) => {
                assert_eq!(fn_name, "target_func");
                assert_eq!(
                    unpacked_args, args,
                    "Arg mismatch for argc = {}",
                    argc
                );
            }
            other => panic!("Expected Op::Call, got {:?}", other),
        }
    }
}

#[test]
fn test_binary_roundtrip_module_with_0_1_2_5_10_args() {
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

fn target5(%0: i64, %1: i64, %2: i64, %3: i64, %4: i64) -> i64 {
bb0:
    ret %0
}

fn target10(%0: i64, %1: i64, %2: i64, %3: i64, %4: i64, %5: i64, %6: i64, %7: i64, %8: i64, %9: i64) -> i64 {
bb0:
    ret %9
}

fn test_suite() -> i64 {
bb0:
    call target0
    %0 = const_i64 1
    %1 = call target1 %0
    %2 = const_i64 2
    %3 = call target2 %0 %2
    %4 = const_i64 3
    %5 = const_i64 4
    %6 = const_i64 5
    %7 = call target5 %0 %2 %4 %5 %6
    %8 = const_i64 6
    %9 = const_i64 7
    %10 = const_i64 8
    %11 = const_i64 9
    %12 = const_i64 10
    %13 = call target10 %0 %2 %4 %5 %6 %8 %9 %10 %11 %12
    ret %13
}
"#;

    let module = parse_ssa(ssa).expect("Parse SSA with 0, 1, 2, 5, 10 args");
    assert_eq!(module.functions.len(), 6);

    let bytes = encode_knrbin(&module).expect("Encode knrbin");
    let decoded = decode_knrbin(&bytes).expect("Decode knrbin");

    assert_eq!(module.functions.len(), decoded.functions.len());
    let suite_orig = module.get_function("test_suite").unwrap();
    let suite_dec = decoded.get_function("test_suite").unwrap();

    let orig_ops: Vec<_> = suite_orig.blocks[0]
        .instructions
        .iter()
        .map(|i| (i.dst, &i.ty, &i.op))
        .collect();
    let dec_ops: Vec<_> = suite_dec.blocks[0]
        .instructions
        .iter()
        .map(|i| (i.dst, &i.ty, &i.op))
        .collect();

    assert_eq!(orig_ops, dec_ops);
    assert_eq!(format_ssa(&module), format_ssa(&decoded));
}
