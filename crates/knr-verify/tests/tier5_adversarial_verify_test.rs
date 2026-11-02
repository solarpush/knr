use knr_core::parser::{parse_function, parse_ssa};
use knr_verify::{verify_function, verify_module};

#[test]
fn test_tier5_ssa_missing_entry_block() {
    let ssa = "fn test() -> void {
bb1:
    ret
}";
    let func = parse_function(ssa).unwrap();
    let diag = verify_function(&func).unwrap_err();
    assert_eq!(diag.error_kind, "MissingEntryBlock");
}

#[test]
fn test_tier5_ssa_duplicate_and_non_monotonic_parameters() {
    // Duplicate parameter
    let ssa_dup = "fn test(%0: i64, %0: i64) -> void {
bb0:
    ret
}";
    let func_dup = parse_function(ssa_dup).unwrap();
    let diag_dup = verify_function(&func_dup).unwrap_err();
    assert_eq!(diag_dup.error_kind, "MultipleAssignment");
    assert_eq!(diag_dup.violated_register.as_deref(), Some("%0"));

    // Non-monotonic parameters
    let ssa_non_mono = "fn test(%2: i64, %1: i64) -> void {
bb0:
    ret
}";
    let func_non_mono = parse_function(ssa_non_mono).unwrap();
    let diag_non_mono = verify_function(&func_non_mono).unwrap_err();
    assert_eq!(diag_non_mono.error_kind, "NonMonotonicRegisters");
    assert_eq!(diag_non_mono.violated_register.as_deref(), Some("%1"));

    // Parameter reassigned in body
    let ssa_reassign = "fn test(%0: i64) -> void {
bb0:
    %0 = const_i64 42
    ret
}";
    let func_reassign = parse_function(ssa_reassign).unwrap();
    let diag_reassign = verify_function(&func_reassign).unwrap_err();
    assert_eq!(diag_reassign.error_kind, "MultipleAssignment");
}

#[test]
fn test_tier5_ssa_monotonicity_and_duplicate_destinations() {
    // Duplicate destination in same block
    let ssa_dup = "fn test() -> void {
bb0:
    %0 = const_i64 1
    %0 = const_i64 2
    ret
}";
    let func_dup = parse_function(ssa_dup).unwrap();
    let diag_dup = verify_function(&func_dup).unwrap_err();
    assert_eq!(diag_dup.error_kind, "MultipleAssignment");

    // Duplicate destination across blocks
    let ssa_dup_cross = "fn test() -> void {
bb0:
    %0 = const_i64 1
    br bb1
bb1:
    %0 = const_i64 2
    ret
}";
    let func_dup_cross = parse_function(ssa_dup_cross).unwrap();
    let diag_dup_cross = verify_function(&func_dup_cross).unwrap_err();
    assert_eq!(diag_dup_cross.error_kind, "MultipleAssignment");

    // Non-monotonic register numbering: %0, %2, %1
    let ssa_non_mono = "fn test() -> void {
bb0:
    %0 = const_i64 1
    %2 = const_i64 2
    %1 = const_i64 3
    ret
}";
    let func_non_mono = parse_function(ssa_non_mono).unwrap();
    let diag_non_mono = verify_function(&func_non_mono).unwrap_err();
    assert_eq!(diag_non_mono.error_kind, "NonMonotonicRegisters");
}

#[test]
fn test_tier5_ssa_invalid_branch_targets() {
    // Br to non-existent block bb99
    let ssa_br = "fn test() -> void {
bb0:
    br bb99
}";
    let func_br = parse_function(ssa_br).unwrap();
    let diag_br = verify_function(&func_br).unwrap_err();
    assert_eq!(diag_br.error_kind, "InvalidBranchTarget");

    // BrIf with non-existent then_blk
    let ssa_brif_then = "fn test(%0: bool) -> void {
bb0:
    br_if %0 bb99 bb1
bb1:
    ret
}";
    let func_brif_then = parse_function(ssa_brif_then).unwrap();
    let diag_then = verify_function(&func_brif_then).unwrap_err();
    assert_eq!(diag_then.error_kind, "InvalidBranchTarget");

    // BrIf with non-existent else_blk
    let ssa_brif_else = "fn test(%0: bool) -> void {
bb0:
    br_if %0 bb1 bb99
bb1:
    ret
}";
    let func_brif_else = parse_function(ssa_brif_else).unwrap();
    let diag_else = verify_function(&func_brif_else).unwrap_err();
    assert_eq!(diag_else.error_kind, "InvalidBranchTarget");
}

#[test]
fn test_tier5_ssa_dominance_violations() {
    // Straight-line: register used before definition
    let ssa_straight = "fn test() -> i64 {
bb0:
    %0 = add %1 %1
    %1 = const_i64 10
    ret %0
}";
    let func_straight = parse_function(ssa_straight).unwrap();
    let diag_s = verify_function(&func_straight).unwrap_err();
    assert_eq!(diag_s.error_kind, "DominanceViolation");
    assert_eq!(diag_s.violated_register.as_deref(), Some("%1"));

    // Cross-branch diamond CFG: %1 defined in bb1, used in join bb3
    let ssa_diamond = "fn test(%0: bool) -> i64 {
bb0:
    br_if %0 bb1 bb2
bb1:
    %1 = const_i64 42
    br bb3
bb2:
    br bb3
bb3:
    %2 = add %1 %1
    ret %2
}";
    let func_diamond = parse_function(ssa_diamond).unwrap();
    let diag_d = verify_function(&func_diamond).unwrap_err();
    assert_eq!(diag_d.error_kind, "DominanceViolation");
    assert_eq!(diag_d.violated_register.as_deref(), Some("%1"));

    // Loop body register used in loop exit
    let ssa_loop = "fn test(%0: bool) -> i64 {
bb0:
    br bb1
bb1:
    br_if %0 bb2 bb3
bb2:
    %1 = const_i64 99
    br bb1
bb3:
    ret %1
}";
    let func_loop = parse_function(ssa_loop).unwrap();
    let diag_l = verify_function(&func_loop).unwrap_err();
    assert_eq!(diag_l.error_kind, "DominanceViolation");
    assert_eq!(diag_l.violated_register.as_deref(), Some("%1"));

    // Terminator dominance: ret %99 where %99 is never defined
    let ssa_ret_undef = "fn test() -> i64 {
bb0:
    ret %99
}";
    let func_ret = parse_function(ssa_ret_undef).unwrap();
    let diag_r = verify_function(&func_ret).unwrap_err();
    assert_eq!(diag_r.error_kind, "DominanceViolation");
    assert_eq!(diag_r.violated_register.as_deref(), Some("%99"));
}

#[test]
fn test_tier5_linear_use_after_move() {
    // MapSet consumes old map: using old map afterwards is UseAfterMove
    let ssa_map = "fn test() -> void {
bb0:
    %0 = map_new str i64
    %1 = const_str \"key\"
    %2 = const_i64 42
    %3 = map_set %0 %1 %2
    %4 = map_get %0 %1
    free %3
    ret
}";
    let func_map = parse_function(ssa_map).unwrap();
    let diag_map = verify_function(&func_map).unwrap_err();
    assert_eq!(diag_map.error_kind, "UseAfterMove");
    assert_eq!(diag_map.violated_register.as_deref(), Some("%0"));

    // MapSet with linear value (buf): value is consumed into map
    let ssa_map_val = "fn test() -> void {
bb0:
    %0 = map_new str buf
    %1 = const_str \"buf_key\"
    %2 = const_i64 64
    %3 = buf_alloc %2
    %4 = map_set %0 %1 %3
    free %3
    free %4
    ret
}";
    let func_map_val = parse_function(ssa_map_val).unwrap();
    let diag_mv = verify_function(&func_map_val).unwrap_err();
    assert_eq!(diag_mv.error_kind, "UseAfterMove");
    assert_eq!(diag_mv.violated_register.as_deref(), Some("%3"));

    // VecPush with linear value (buf): value is consumed into vector
    let ssa_vec_val = "fn test() -> void {
bb0:
    %0 = vec_new buf
    %1 = const_i64 64
    %2 = buf_alloc %1
    %3 = vec_push %0 %2
    free %2
    free %3
    ret
}";
    let func_vec_val = parse_function(ssa_vec_val).unwrap();
    let diag_vv = verify_function(&func_vec_val).unwrap_err();
    assert_eq!(diag_vv.error_kind, "UseAfterMove");
    assert_eq!(diag_vv.violated_register.as_deref(), Some("%2"));

    // BufWriteU8 consumes old buffer: reading old buffer is UseAfterMove
    let ssa_buf = "fn test() -> void {
bb0:
    %0 = const_i64 16
    %1 = buf_alloc %0
    %2 = const_i64 0
    %3 = const_i64 255
    %4 = buf_write_u8 %1 %2 %3
    %5 = buf_read_u8 %1 %2
    free %4
    ret
}";
    let func_buf = parse_function(ssa_buf).unwrap();
    let diag_buf = verify_function(&func_buf).unwrap_err();
    assert_eq!(diag_buf.error_kind, "UseAfterMove");
    assert_eq!(diag_buf.violated_register.as_deref(), Some("%1"));

    // Double free
    let ssa_dbl_free = "fn test() -> void {
bb0:
    %0 = const_i64 16
    %1 = buf_alloc %0
    free %1
    free %1
    ret
}";
    let func_df = parse_function(ssa_dbl_free).unwrap();
    let diag_df = verify_function(&func_df).unwrap_err();
    assert_eq!(diag_df.error_kind, "UseAfterMove");
    assert_eq!(diag_df.violated_register.as_deref(), Some("%1"));
}

#[test]
fn test_tier5_linear_branch_imbalance() {
    // Diamond CFG: bb1 frees %1, bb2 does not. bb3 returns void without freeing %1.
    // Path through bb2 leaks %1!
    let ssa_leak = "fn test(%0: bool) -> void {
bb0:
    %1 = const_i64 32
    %2 = buf_alloc %1
    br_if %0 bb1 bb2
bb1:
    free %2
    br bb3
bb2:
    br bb3
bb3:
    ret
}";
    let func_leak = parse_function(ssa_leak).unwrap();
    let diag_l = verify_function(&func_leak).unwrap_err();
    assert_eq!(diag_l.error_kind, "LinearLeak");
    assert_eq!(diag_l.violated_register.as_deref(), Some("%2"));

    // Diamond CFG: bb1 moves %2 into free, bb2 does not. bb3 calls free %2.
    // Path through bb1 encounters UseAfterMove!
    let ssa_uam = "fn test(%0: bool) -> void {
bb0:
    %1 = const_i64 32
    %2 = buf_alloc %1
    br_if %0 bb1 bb2
bb1:
    free %2
    br bb3
bb2:
    br bb3
bb3:
    free %2
    ret
}";
    let func_uam = parse_function(ssa_uam).unwrap();
    let diag_uam = verify_function(&func_uam).unwrap_err();
    assert_eq!(diag_uam.error_kind, "UseAfterMove");
    assert_eq!(diag_uam.violated_register.as_deref(), Some("%2"));
}

#[test]
fn test_tier5_linear_leak_unfreed_containers() {
    // Unfreed buf_alloc
    let ssa_buf = "fn test() -> void {\nbb0:\n    %0 = const_i64 16\n    %1 = buf_alloc %0\n    ret\n}";
    assert_eq!(
        verify_function(&parse_function(ssa_buf).unwrap()).unwrap_err().error_kind,
        "LinearLeak"
    );

    // Unfreed buf_slice
    let ssa_slice = "fn test(%0: buf) -> void {\nbb0:\n    %1 = const_i64 0\n    %2 = const_i64 8\n    %3 = buf_slice %0 %1 %2\n    free %0\n    ret\n}";
    assert_eq!(
        verify_function(&parse_function(ssa_slice).unwrap()).unwrap_err().error_kind,
        "LinearLeak"
    );

    // Unfreed vec_new
    let ssa_vec = "fn test() -> void {\nbb0:\n    %0 = vec_new i64\n    ret\n}";
    assert_eq!(
        verify_function(&parse_function(ssa_vec).unwrap()).unwrap_err().error_kind,
        "LinearLeak"
    );

    // Unfreed map_new
    let ssa_map = "fn test() -> void {\nbb0:\n    %0 = map_new str i64\n    ret\n}";
    assert_eq!(
        verify_function(&parse_function(ssa_map).unwrap()).unwrap_err().error_kind,
        "LinearLeak"
    );

    // Unfreed chan_new
    let ssa_chan = "fn test() -> void effects(Async) {\nbb0:\n    %0 = const_i64 4\n    %1 = chan_new i64 %0\n    ret\n}";
    assert_eq!(
        verify_function(&parse_function(ssa_chan).unwrap()).unwrap_err().error_kind,
        "LinearLeak"
    );

    // const_str does NOT leak
    let ssa_str = "fn test() -> void {\nbb0:\n    %0 = const_str \"hello\"\n    ret\n}";
    assert!(verify_function(&parse_function(ssa_str).unwrap()).is_ok());

    // Returned linear resource is NOT leaked
    let ssa_ret = "fn test() -> buf {\nbb0:\n    %0 = const_i64 16\n    %1 = buf_alloc %0\n    ret %1\n}";
    assert!(verify_function(&parse_function(ssa_ret).unwrap()).is_ok());
}

#[test]
fn test_tier5_static_type_checking_rejections() {
    // Arithmetic on booleans
    let ssa_arith_bool = "fn test(%0: bool, %1: bool) -> bool {\nbb0:\n    %2 = add %0 %1\n    ret %2\n}";
    assert_eq!(
        verify_function(&parse_function(ssa_arith_bool).unwrap()).unwrap_err().error_kind,
        "TypeMismatch"
    );

    // Arithmetic with mismatched types: i64 and f64
    let ssa_arith_mismatch = "fn test(%0: i64, %1: f64) -> i64 {\nbb0:\n    %2 = add %0 %1\n    ret %2\n}";
    assert_eq!(
        verify_function(&parse_function(ssa_arith_mismatch).unwrap()).unwrap_err().error_kind,
        "TypeMismatch"
    );

    // cmp_lt on booleans
    let ssa_cmp_bool = "fn test(%0: bool, %1: bool) -> bool {\nbb0:\n    %2 = cmp_lt %0 %1\n    ret %2\n}";
    assert_eq!(
        verify_function(&parse_function(ssa_cmp_bool).unwrap()).unwrap_err().error_kind,
        "TypeMismatch"
    );

    // cmp_eq with mismatched types: i64 and bool
    let ssa_cmpeq_diff = "fn test(%0: i64, %1: bool) -> bool {\nbb0:\n    %2 = cmp_eq %0 %1\n    ret %2\n}";
    assert_eq!(
        verify_function(&parse_function(ssa_cmpeq_diff).unwrap()).unwrap_err().error_kind,
        "TypeMismatch"
    );

    // vec_push with mismatched element type: str pushed to vec[i64]
    let ssa_vec_bad = "fn test(%0: vec[i64], %1: str) -> vec[i64] {\nbb0:\n    %2 = vec_push %0 %1\n    ret %2\n}";
    assert_eq!(
        verify_function(&parse_function(ssa_vec_bad).unwrap()).unwrap_err().error_kind,
        "TypeMismatch"
    );

    // map_set with mismatched key type: i64 key for map[str, i64]
    let ssa_map_bad_key = "fn test(%0: map[str, i64], %1: i64, %2: i64) -> map[str, i64] {\nbb0:\n    %3 = map_set %0 %1 %2\n    ret %3\n}";
    assert_eq!(
        verify_function(&parse_function(ssa_map_bad_key).unwrap()).unwrap_err().error_kind,
        "TypeMismatch"
    );

    // chan_send with mismatched value type: f64 value for chan[i64]
    let ssa_chan_bad = "fn test(%0: chan[i64], %1: f64) -> bool effects(Async) {\nbb0:\n    %2 = chan_send %0 %1\n    ret %2\n}";
    assert_eq!(
        verify_function(&parse_function(ssa_chan_bad).unwrap()).unwrap_err().error_kind,
        "TypeMismatch"
    );

    // br_if with non-bool condition
    let ssa_brif_bad = "fn test(%0: i64) -> void {\nbb0:\n    br_if %0 bb1 bb2\nbb1:\n    ret\nbb2:\n    ret\n}";
    assert_eq!(
        verify_function(&parse_function(ssa_brif_bad).unwrap()).unwrap_err().error_kind,
        "TypeMismatch"
    );

    // Function return type mismatch: declares i64, returns bool
    let ssa_ret_bad = "fn test(%0: bool) -> i64 {\nbb0:\n    ret %0\n}";
    assert_eq!(
        verify_function(&parse_function(ssa_ret_bad).unwrap()).unwrap_err().error_kind,
        "TypeMismatch"
    );
}

#[test]
fn test_tier5_effects_checking_rejections() {
    // Syscall Net without effects(Net)
    let ssa_net = "fn test(%0: i64) -> i64 {\nbb0:\n    %1 = syscall Net \"net.listen\" %0\n    ret %1\n}";
    assert_eq!(
        verify_function(&parse_function(ssa_net).unwrap()).unwrap_err().error_kind,
        "UndeclaredEffect"
    );

    // Syscall Hw without effects(Hw)
    let ssa_hw = "fn test(%0: i64) -> buf {\nbb0:\n    %1 = syscall Hw \"hw.mmio_map\" %0\n    ret %1\n}";
    assert_eq!(
        verify_function(&parse_function(ssa_hw).unwrap()).unwrap_err().error_kind,
        "UndeclaredEffect"
    );

    // chan_new without effects(Async)
    let ssa_chan = "fn test(%0: i64) -> chan[i64] {\nbb0:\n    %1 = chan_new i64 %0\n    ret %1\n}";
    assert_eq!(
        verify_function(&parse_function(ssa_chan).unwrap()).unwrap_err().error_kind,
        "UndeclaredEffect"
    );

    // spawn without effects(Async)
    let ssa_spawn = "fn test(%0: chan[i64]) -> bool {\nbb0:\n    %1 = spawn \"worker\" %0\n    ret %1\n}";
    assert_eq!(
        verify_function(&parse_function(ssa_spawn).unwrap()).unwrap_err().error_kind,
        "UndeclaredEffect"
    );

    // Transitive module effect: caller with effects(Async) spawns worker with effects(Net, Async), but caller lacks Net
    let ssa_module = r#"
fn worker(%0: chan[i64]) -> void effects(Net, Async) {
bb0:
    %1 = const_i64 8080
    %2 = syscall Net "net.listen" %1
    free %2
    ret
}

fn caller(%0: chan[i64]) -> bool effects(Async) {
bb0:
    %1 = spawn "worker" %0
    ret %1
}
"#;
    let module = parse_ssa(ssa_module).unwrap();
    let diags = verify_module(&module).unwrap_err();
    assert!(diags.iter().any(|d| d.error_kind == "UndeclaredEffect"));
}
