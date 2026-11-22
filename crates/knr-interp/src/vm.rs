use crate::error::VmError;
use crate::trace::TraceRecord;
use crate::value::{Value, ValueExt};
use knr_core::{BasicBlock, BlockId, Effect, Function, Module, Op, RegId, Schema, Terminator, Type};
use knr_runtime::HostPlatform;
use std::collections::{BTreeMap, HashMap, VecDeque};

/// Deterministic VM execution profiling statistics.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ExecutionStats {
    pub total_instructions: u64,
    pub gas_used: u64,
    pub function_calls: BTreeMap<String, u64>,
    pub function_instructions: BTreeMap<String, u64>,
    pub allocations: u64,
    pub frees: u64,
}

const FLAT_REG_LIMIT: usize = 8192;
const FLAT_BLOCK_LIMIT: usize = 4096;

/// Fast register storage backed by a flat `Vec<Option<Value>>` for dense SSA registers
/// (`0..FLAT_REG_LIMIT`) with a sparse `HashMap` fallback for synthetic high register IDs.
#[derive(Debug, Default)]
struct RegisterFile {
    flat: Vec<Option<Value>>,
    overflow: HashMap<RegId, Value>,
}

impl RegisterFile {
    #[inline]
    fn with_capacity(cap: usize) -> Self {
        Self {
            flat: vec![None; cap.min(FLAT_REG_LIMIT)],
            overflow: HashMap::new(),
        }
    }

    #[inline]
    fn reset_for(&mut self, cap: usize) {
        let target = cap.min(FLAT_REG_LIMIT);
        self.flat.fill(None);
        if self.flat.len() < target {
            self.flat.resize(target, None);
        }
        if !self.overflow.is_empty() {
            self.overflow.clear();
        }
    }

    #[inline]
    fn get(&self, reg: RegId) -> Option<&Value> {
        let idx = reg as usize;
        if idx < self.flat.len() {
            self.flat[idx].as_ref()
        } else {
            self.overflow.get(&reg)
        }
    }

    #[inline]
    fn take(&mut self, reg: RegId) -> Option<Value> {
        let idx = reg as usize;
        if idx < self.flat.len() {
            self.flat[idx].take()
        } else {
            self.overflow.remove(&reg)
        }
    }

    #[inline]
    fn set(&mut self, reg: RegId, val: Value) {
        let idx = reg as usize;
        if idx < FLAT_REG_LIMIT {
            if idx >= self.flat.len() {
                self.flat.resize(idx + 1, None);
            }
            self.flat[idx] = Some(val);
        } else {
            self.overflow.insert(reg, val);
        }
    }

    #[inline]
    fn remove(&mut self, reg: RegId) -> Option<Value> {
        self.take(reg)
    }
}

/// Precomputed per-function execution metadata (block table, register capacity,
/// provably single-use same-block linear move mask, and pure-constant eligibility).
#[derive(Debug, Clone)]
struct FnMeta {
    reg_capacity: usize,
    block_table: Vec<Option<usize>>,
    sparse_blocks: HashMap<BlockId, usize>,
    can_move: Vec<bool>,
    is_pure_const: bool,
}

impl FnMeta {
    fn build(func: &Function) -> Self {
        let mut max_reg: usize = 0;
        let mut max_block_id: usize = 0;
        let mut sparse_blocks = HashMap::new();

        for (b_idx, blk) in func.blocks.iter().enumerate() {
            let bid = blk.id as usize;
            if bid < FLAT_BLOCK_LIMIT {
                max_block_id = max_block_id.max(bid + 1);
            } else {
                sparse_blocks.insert(blk.id, b_idx);
            }
            for &(r, _) in &blk.params {
                if (r as usize) < FLAT_REG_LIMIT {
                    max_reg = max_reg.max(r as usize + 1);
                }
            }
            for inst in &blk.instructions {
                if let Some(dst) = inst.dst {
                    if (dst as usize) < FLAT_REG_LIMIT {
                        max_reg = max_reg.max(dst as usize + 1);
                    }
                }
            }
        }
        for &(r, _) in &func.params {
            if (r as usize) < FLAT_REG_LIMIT {
                max_reg = max_reg.max(r as usize + 1);
            }
        }

        let mut block_table = vec![None; max_block_id];
        for (b_idx, blk) in func.blocks.iter().enumerate() {
            let bid = blk.id as usize;
            if bid < block_table.len() {
                block_table[bid] = Some(b_idx);
            }
        }

        // Compute provably single-use same-block registers for zero-clone in-place linear moves.
        // A register `r` is safe to move (take) on read iff:
        // 1. Defined exactly once by an instruction `inst.dst == Some(r)` in block `b_idx`
        //    (not a function param or block param).
        // 2. Read exactly once across the entire function.
        // 3. That single read occurs in the exact same block `b_idx`.
        let mut def_block: Vec<i32> = vec![-1; max_reg];
        let mut def_count: Vec<u16> = vec![0; max_reg];
        let mut read_count: Vec<u16> = vec![0; max_reg];
        let mut read_block: Vec<i32> = vec![-1; max_reg];

        for &(r, _) in &func.params {
            let idx = r as usize;
            if idx < max_reg {
                def_count[idx] = 2; // never treat params as single-block instruction defs
            }
        }

        let record_read =
            |r: RegId, b_idx: usize, rc: &mut [u16], rb: &mut [i32]| {
                let idx = r as usize;
                if idx < rc.len() {
                    rc[idx] = rc[idx].saturating_add(1);
                    if rb[idx] == -1 {
                        rb[idx] = b_idx as i32;
                    } else if rb[idx] != b_idx as i32 {
                        rb[idx] = -2;
                    }
                }
            };

        for (b_idx, blk) in func.blocks.iter().enumerate() {
            for &(r, _) in &blk.params {
                let idx = r as usize;
                if idx < max_reg {
                    def_count[idx] = 2;
                }
            }
            for inst in &blk.instructions {
                if let Some(dst) = inst.dst {
                    let idx = dst as usize;
                    if idx < max_reg {
                        def_count[idx] = def_count[idx].saturating_add(1);
                        def_block[idx] = b_idx as i32;
                    }
                }
                for_each_op_reg(&inst.op, |r| {
                    record_read(r, b_idx, &mut read_count, &mut read_block);
                });
            }
            for_each_term_reg(&blk.terminator, |r| {
                record_read(r, b_idx, &mut read_count, &mut read_block);
            });
        }

        let mut can_move = vec![false; max_reg];
        for idx in 0..max_reg {
            if def_count[idx] == 1
                && read_count[idx] == 1
                && def_block[idx] >= 0
                && def_block[idx] == read_block[idx]
            {
                can_move[idx] = true;
            }
        }

        // Pure 0-arg constant constructors (like `style_default()`, `theme_obsidian()`, etc.):
        // 0 params, 0 effects, single basic block, no syscalls/channels/spawns/calls.
        let is_pure_const = func.params.is_empty()
            && func.effects.is_empty()
            && func.blocks.len() == 1
            && matches!(func.blocks[0].terminator, Terminator::Ret(_))
            && func.blocks[0].instructions.iter().all(|inst| {
                !matches!(
                    inst.op,
                    Op::Syscall(_, _, _)
                        | Op::Spawn(_, _)
                        | Op::ChanNew(_, _)
                        | Op::ChanSend(_, _)
                        | Op::ChanRecv(_)
                        | Op::Call(_, _)
                )
            });

        Self {
            reg_capacity: max_reg,
            block_table,
            sparse_blocks,
            can_move,
            is_pure_const,
        }
    }

    #[inline]
    fn resolve_block(&self, id: BlockId) -> Option<usize> {
        let idx = id as usize;
        if idx < self.block_table.len() {
            self.block_table[idx]
        } else {
            self.sparse_blocks.get(&id).copied()
        }
    }

    #[inline]
    fn is_movable(&self, reg: RegId) -> bool {
        let idx = reg as usize;
        idx < self.can_move.len() && self.can_move[idx]
    }
}

#[inline]
fn for_each_op_reg(op: &Op, mut f: impl FnMut(RegId)) {
    match op {
        Op::ConstI64(_) | Op::ConstF64(_) | Op::ConstBool(_) | Op::ConstStr(_)
        | Op::VecNew(_) | Op::MapNew(_, _) | Op::StructNew(_) => {}
        Op::IntToStr(a)
        | Op::StrLen(a)
        | Op::Sin(a)
        | Op::Cos(a)
        | Op::Tan(a)
        | Op::Sqrt(a)
        | Op::Abs(a)
        | Op::Floor(a)
        | Op::Ceil(a)
        | Op::IntToFloat(a)
        | Op::FloatToInt(a)
        | Op::BufAlloc(a)
        | Op::VecLen(a)
        | Op::ChanNew(_, a)
        | Op::ChanRecv(a)
        | Op::Spawn(_, a)
        | Op::Syscall(_, _, a)
        | Op::Free(a)
        | Op::StructGet(a, _) => f(*a),
        Op::Add(a, b)
        | Op::Sub(a, b)
        | Op::Mul(a, b)
        | Op::Div(a, b)
        | Op::Rem(a, b)
        | Op::CmpEq(a, b)
        | Op::CmpLt(a, b)
        | Op::Pow(a, b)
        | Op::Atan2(a, b)
        | Op::Min(a, b)
        | Op::Max(a, b)
        | Op::BufReadU8(a, b)
        | Op::VecPush(a, b)
        | Op::VecGet(a, b)
        | Op::MapGet(a, b)
        | Op::MapDel(a, b)
        | Op::ChanSend(a, b)
        | Op::StructSet(a, _, b) => {
            f(*a);
            f(*b);
        }
        Op::BufWriteU8(a, b, c) | Op::BufSlice(a, b, c) | Op::MapSet(a, b, c) => {
            f(*a);
            f(*b);
            f(*c);
        }
        Op::Call(_, args) => {
            for &r in args {
                f(r);
            }
        }
    }
}

#[inline]
fn for_each_term_reg(term: &Terminator, mut f: impl FnMut(RegId)) {
    match term {
        Terminator::Ret(Some(r)) => f(*r),
        Terminator::Ret(None) | Terminator::Br(_) => {}
        Terminator::BrIf { cond, .. } => f(*cond),
        Terminator::BrArgs { args, .. } => {
            for &r in args {
                f(r);
            }
        }
        Terminator::BrIfArgs {
            cond,
            then_args,
            else_args,
            ..
        } => {
            f(*cond);
            for &r in then_args {
                f(r);
            }
            for &r in else_args {
                f(r);
            }
        }
    }
}

/// Precomputed schema lookup entry for O(1) `StructNew`, `StructGet`, and `StructSet`.
struct SchemaMeta<'a> {
    default_val: Value,
    field_types: Vec<Option<&'a Type>>,
    field_count: usize,
}

/// Virtual Machine execution engine for knr SSA programs.
pub struct Vm<'a> {
    registers: RegisterFile,
    frame_pool: Vec<RegisterFile>,
    channels: HashMap<usize, VecDeque<Value>>,
    next_chan_id: usize,
    platform: &'a mut dyn HostPlatform,
    module: Option<&'a Module>,
    fn_index: HashMap<&'a str, usize>,
    fn_metas: Vec<Option<FnMeta>>,
    pure_const_cache: HashMap<usize, (Value, u64)>,
    fallback_func: Option<(Function, FnMeta)>,
    schema_cache: HashMap<&'a str, SchemaMeta<'a>>,
    max_steps: u64,
    remaining_steps: u64,
    step_count: u64,
    trace_enabled: bool,
    trace_records: Vec<TraceRecord>,
    call_depth: usize,
    stats: Option<ExecutionStats>,
}

impl<'a> Vm<'a> {
    /// Creates a new VM instance with the given configuration.
    pub fn new(
        platform: &'a mut dyn HostPlatform,
        module: Option<&'a Module>,
        max_steps: u64,
        trace_enabled: bool,
    ) -> Self {
        let schemas = module.map(|m| m.schemas.as_slice()).unwrap_or(&[]);
        Self::new_with_schemas(platform, module, schemas, max_steps, trace_enabled)
    }

    /// Creates a new VM instance with explicit schemas.
    pub fn new_with_schemas(
        platform: &'a mut dyn HostPlatform,
        module: Option<&'a Module>,
        schemas: &'a [Schema],
        max_steps: u64,
        trace_enabled: bool,
    ) -> Self {
        let mut fn_index = HashMap::new();
        let mut fn_metas = Vec::new();
        if let Some(m) = module {
            fn_index.reserve(m.functions.len());
            fn_metas.resize_with(m.functions.len(), || None);
            for (idx, f) in m.functions.iter().enumerate() {
                fn_index.entry(f.name.as_str()).or_insert(idx);
            }
        }

        let mut schema_cache = HashMap::with_capacity(schemas.len());
        for s in schemas {
            let max_idx = s
                .fields
                .iter()
                .map(|f| f.index as usize)
                .max()
                .map(|m| m + 1)
                .unwrap_or(s.fields.len())
                .min(65536);
            let mut field_types = vec![None; max_idx];
            for f in &s.fields {
                let idx = f.index as usize;
                if idx < field_types.len() {
                    field_types[idx] = Some(&f.ty);
                }
            }
            let default_val = default_value_for_type_with_schemas(
                &Type::Struct(s.name.clone()),
                schemas,
            );
            schema_cache.insert(
                s.name.as_str(),
                SchemaMeta {
                    default_val,
                    field_types,
                    field_count: s.fields.len(),
                },
            );
        }

        Self {
            registers: RegisterFile::default(),
            frame_pool: Vec::with_capacity(8),
            channels: HashMap::new(),
            next_chan_id: 1,
            platform,
            module,
            fn_index,
            fn_metas,
            pure_const_cache: HashMap::new(),
            fallback_func: None,
            schema_cache,
            max_steps,
            remaining_steps: max_steps,
            step_count: 0,
            trace_enabled,
            trace_records: Vec::new(),
            call_depth: 0,
            stats: None,
        }
    }

    /// Enables deterministic per-function profiling stats collection on this VM instance.
    pub fn enable_profiling(&mut self) {
        self.stats = Some(ExecutionStats::default());
    }

    /// Returns collected profiling statistics if profiling is enabled.
    pub fn stats(&self) -> Option<&ExecutionStats> {
        self.stats.as_ref()
    }

    /// Extracts collected profiling statistics (or default if profiling was not enabled).
    pub fn take_stats(&mut self) -> ExecutionStats {
        let mut s = self.stats.take().unwrap_or_default();
        s.gas_used = self.step_count;
        s
    }

    /// Borrows a register's value without cloning, or returns `InvalidRegisterAccess`.
    #[inline]
    fn get_reg_ref(&self, reg: RegId) -> Result<&Value, VmError> {
        self.registers.get(reg).ok_or(VmError::InvalidRegisterAccess {
            reg: reg as usize,
        })
    }

    /// Moves a single-use linear register value out of the register file (zero clone),
    /// or clones it safely when it may be read again or when tracing is active.
    #[inline]
    fn take_or_clone_reg(&mut self, reg: RegId, meta: &FnMeta) -> Result<Value, VmError> {
        if !self.trace_enabled && meta.is_movable(reg) {
            self.registers
                .take(reg)
                .ok_or(VmError::InvalidRegisterAccess { reg: reg as usize })
        } else {
            self.registers
                .get(reg)
                .cloned()
                .ok_or(VmError::InvalidRegisterAccess { reg: reg as usize })
        }
    }

    /// Converts a `Value` into a string key for map operations.
    fn value_to_map_key(val: &Value) -> String {
        match val {
            Value::Str(s) => s.clone(),
            other => other.to_string(),
        }
    }

    /// Returns the per-request / per-tick gas limit.
    #[inline]
    pub fn max_steps(&self) -> u64 {
        self.max_steps
    }

    /// Returns the remaining steps in the current request budget.
    #[inline]
    pub fn remaining_steps(&self) -> u64 {
        self.remaining_steps
    }

    /// Returns the total lifetime steps executed.
    #[inline]
    pub fn step_count(&self) -> u64 {
        self.step_count
    }

    /// Decrements gas and increments step counter before an instruction or terminator.
    #[inline]
    fn consume_gas(&mut self) -> Result<(), VmError> {
        if self.max_steps == u64::MAX {
            self.step_count = self.step_count.wrapping_add(1);
            return Ok(());
        }
        if self.remaining_steps == 0 {
            return Err(VmError::GasExhausted {
                max_steps: self.max_steps,
            });
        }
        self.remaining_steps -= 1;
        self.step_count = self.step_count.wrapping_add(1);
        Ok(())
    }

    /// Charges `steps` gas atomically (used by pure 0-arg constant function memoization
    /// to preserve bit-identical gas accounting and exhaustion semantics).
    #[inline]
    fn consume_gas_n(&mut self, steps: u64) -> Result<(), VmError> {
        if self.max_steps == u64::MAX {
            self.step_count = self.step_count.wrapping_add(steps);
            return Ok(());
        }
        if self.remaining_steps < steps {
            self.step_count = self.step_count.wrapping_add(self.remaining_steps);
            self.remaining_steps = 0;
            return Err(VmError::GasExhausted {
                max_steps: self.max_steps,
            });
        }
        self.remaining_steps -= steps;
        self.step_count = self.step_count.wrapping_add(steps);
        Ok(())
    }

    /// Returns the human-readable opcode name.
    pub fn opcode_name(op: &Op) -> &'static str {
        match op {
            Op::ConstI64(_) => "const_i64",
            Op::ConstF64(_) => "const_f64",
            Op::ConstBool(_) => "const_bool",
            Op::ConstStr(_) => "const_str",
            Op::Add(_, _) => "add",
            Op::Sub(_, _) => "sub",
            Op::Mul(_, _) => "mul",
            Op::Div(_, _) => "div",
            Op::Rem(_, _) => "rem",
            Op::CmpEq(_, _) => "cmp_eq",
            Op::CmpLt(_, _) => "cmp_lt",
            Op::IntToStr(_) => "int_to_str",
            Op::StrLen(_) => "str_len",
            Op::Sin(_) => "sin",
            Op::Cos(_) => "cos",
            Op::Tan(_) => "tan",
            Op::Sqrt(_) => "sqrt",
            Op::Abs(_) => "abs",
            Op::Floor(_) => "floor",
            Op::Ceil(_) => "ceil",
            Op::Pow(_, _) => "pow",
            Op::Atan2(_, _) => "atan2",
            Op::Min(_, _) => "min",
            Op::Max(_, _) => "max",
            Op::IntToFloat(_) => "i64_to_f64",
            Op::FloatToInt(_) => "f64_to_i64",
            Op::BufAlloc(_) => "buf_alloc",
            Op::BufReadU8(_, _) => "buf_read_u8",
            Op::BufWriteU8(_, _, _) => "buf_write_u8",
            Op::BufSlice(_, _, _) => "buf_slice",
            Op::VecNew(_) => "vec_new",
            Op::VecPush(_, _) => "vec_push",
            Op::VecGet(_, _) => "vec_get",
            Op::VecLen(_) => "vec_len",
            Op::MapNew(_, _) => "map_new",
            Op::MapSet(_, _, _) => "map_set",
            Op::MapGet(_, _) => "map_get",
            Op::MapDel(_, _) => "map_del",
            Op::ChanNew(_, _) => "chan_new",
            Op::ChanSend(_, _) => "chan_send",
            Op::ChanRecv(_) => "chan_recv",
            Op::Spawn(_, _) => "spawn",
            Op::Syscall(_, _, _) => "syscall",
            Op::Free(_) => "free",
            Op::StructNew(_) => "struct_new",
            Op::StructGet(_, _) => "struct_get",
            Op::StructSet(_, _, _) => "struct_set",
            Op::Call(..) => "call",
        }
    }

    #[inline]
    fn borrow_i64(&self, reg: RegId, line: usize) -> Result<i64, VmError> {
        match self.get_reg_ref(reg)? {
            Value::I64(n) => Ok(*n),
            other => Err(VmError::TypeMismatch {
                expected: "i64".to_string(),
                actual: other.type_name().to_string(),
                line,
            }),
        }
    }

    /// Executes a single opcode, returning the produced value with zero redundant cloning.
    #[inline]
    fn exec_op(&mut self, op: &Op, line: usize, meta: &FnMeta) -> Result<Value, VmError> {
        match op {
            Op::ConstI64(v) => Ok(Value::I64(*v)),
            Op::ConstF64(v) => Ok(Value::F64(*v)),
            Op::ConstBool(v) => Ok(Value::Bool(*v)),
            Op::ConstStr(s) => Ok(Value::Str(s.clone())),

            Op::Add(a_reg, b_reg) => match (self.get_reg_ref(*a_reg)?, self.get_reg_ref(*b_reg)?) {
                (Value::I64(x), Value::I64(y)) => Ok(Value::I64(x.wrapping_add(*y))),
                (Value::F64(x), Value::F64(y)) => Ok(Value::F64(x + y)),
                (Value::Str(x), Value::Str(y)) => {
                    let mut out = String::with_capacity(x.len() + y.len());
                    out.push_str(x);
                    out.push_str(y);
                    Ok(Value::Str(out))
                }
                (x, y) => Err(VmError::TypeMismatch {
                    expected: "i64, f64, or str".to_string(),
                    actual: format!("{}, {}", x.type_name(), y.type_name()),
                    line,
                }),
            },

            Op::Sub(a_reg, b_reg) => match (self.get_reg_ref(*a_reg)?, self.get_reg_ref(*b_reg)?) {
                (Value::I64(x), Value::I64(y)) => Ok(Value::I64(x.wrapping_sub(*y))),
                (Value::F64(x), Value::F64(y)) => Ok(Value::F64(x - y)),
                (x, y) => Err(VmError::TypeMismatch {
                    expected: "i64 or f64".to_string(),
                    actual: format!("{}, {}", x.type_name(), y.type_name()),
                    line,
                }),
            },

            Op::Mul(a_reg, b_reg) => match (self.get_reg_ref(*a_reg)?, self.get_reg_ref(*b_reg)?) {
                (Value::I64(x), Value::I64(y)) => Ok(Value::I64(x.wrapping_mul(*y))),
                (Value::F64(x), Value::F64(y)) => Ok(Value::F64(x * y)),
                (x, y) => Err(VmError::TypeMismatch {
                    expected: "i64 or f64".to_string(),
                    actual: format!("{}, {}", x.type_name(), y.type_name()),
                    line,
                }),
            },

            Op::Div(a_reg, b_reg) => match (self.get_reg_ref(*a_reg)?, self.get_reg_ref(*b_reg)?) {
                (Value::I64(x), Value::I64(y)) => {
                    if *y == 0 {
                        return Err(VmError::DivisionByZero { line });
                    }
                    if *x == i64::MIN && *y == -1 {
                        Ok(Value::I64(i64::MIN))
                    } else {
                        Ok(Value::I64(*x / *y))
                    }
                }
                (Value::F64(x), Value::F64(y)) => Ok(Value::F64(x / y)),
                (x, y) => Err(VmError::TypeMismatch {
                    expected: "i64 or f64".to_string(),
                    actual: format!("{}, {}", x.type_name(), y.type_name()),
                    line,
                }),
            },

            Op::Rem(a_reg, b_reg) => match (self.get_reg_ref(*a_reg)?, self.get_reg_ref(*b_reg)?) {
                (Value::I64(x), Value::I64(y)) => {
                    if *y == 0 {
                        return Err(VmError::DivisionByZero { line });
                    }
                    if *x == i64::MIN && *y == -1 {
                        Ok(Value::I64(0))
                    } else {
                        Ok(Value::I64(*x % *y))
                    }
                }
                (Value::F64(x), Value::F64(y)) => Ok(Value::F64(x % y)),
                (x, y) => Err(VmError::TypeMismatch {
                    expected: "i64 or f64".to_string(),
                    actual: format!("{}, {}", x.type_name(), y.type_name()),
                    line,
                }),
            },

            Op::IntToStr(a_reg) => match self.get_reg_ref(*a_reg)? {
                Value::I64(n) => Ok(Value::Str(n.to_string())),
                Value::F64(f) => Ok(Value::Str(f.to_string())),
                Value::Bool(b) => Ok(Value::Str(b.to_string())),
                Value::Str(s) => Ok(Value::Str(s.clone())),
                other => Err(VmError::TypeMismatch {
                    expected: "i64, f64, bool, or str".to_string(),
                    actual: other.type_name().to_string(),
                    line,
                }),
            },

            Op::StrLen(a_reg) => match self.get_reg_ref(*a_reg)? {
                Value::Str(s) => Ok(Value::I64(s.len() as i64)),
                other => Err(VmError::TypeMismatch {
                    expected: "str".to_string(),
                    actual: other.type_name().to_string(),
                    line,
                }),
            },

            Op::Sin(r)
            | Op::Cos(r)
            | Op::Tan(r)
            | Op::Sqrt(r)
            | Op::Abs(r)
            | Op::Floor(r)
            | Op::Ceil(r) => match self.get_reg_ref(*r)? {
                Value::F64(x) => {
                    let out = match op {
                        Op::Sin(_) => x.sin(),
                        Op::Cos(_) => x.cos(),
                        Op::Tan(_) => x.tan(),
                        Op::Sqrt(_) => x.sqrt(),
                        Op::Abs(_) => x.abs(),
                        Op::Floor(_) => x.floor(),
                        _ => x.ceil(),
                    };
                    Ok(Value::F64(out))
                }
                other => Err(VmError::TypeMismatch {
                    expected: "f64".to_string(),
                    actual: other.type_name().to_string(),
                    line,
                }),
            },

            Op::Pow(a_reg, b_reg)
            | Op::Atan2(a_reg, b_reg)
            | Op::Min(a_reg, b_reg)
            | Op::Max(a_reg, b_reg) => {
                match (self.get_reg_ref(*a_reg)?, self.get_reg_ref(*b_reg)?) {
                    (Value::F64(x), Value::F64(y)) => {
                        let out = match op {
                            Op::Pow(_, _) => x.powf(*y),
                            Op::Atan2(_, _) => x.atan2(*y),
                            Op::Min(_, _) => x.min(*y),
                            _ => x.max(*y),
                        };
                        Ok(Value::F64(out))
                    }
                    (x, y) => Err(VmError::TypeMismatch {
                        expected: "f64, f64".to_string(),
                        actual: format!("{}, {}", x.type_name(), y.type_name()),
                        line,
                    }),
                }
            }

            Op::IntToFloat(r) => match self.get_reg_ref(*r)? {
                Value::I64(n) => Ok(Value::F64(*n as f64)),
                other => Err(VmError::TypeMismatch {
                    expected: "i64".to_string(),
                    actual: other.type_name().to_string(),
                    line,
                }),
            },

            Op::FloatToInt(r) => match self.get_reg_ref(*r)? {
                Value::F64(f) => Ok(Value::I64(*f as i64)),
                other => Err(VmError::TypeMismatch {
                    expected: "f64".to_string(),
                    actual: other.type_name().to_string(),
                    line,
                }),
            },

            Op::CmpEq(a_reg, b_reg) => {
                let a = self.get_reg_ref(*a_reg)?;
                let b = self.get_reg_ref(*b_reg)?;
                match (a, b) {
                    (Value::F64(x), Value::F64(y)) => {
                        if x.is_nan() || y.is_nan() {
                            Ok(Value::Bool(false))
                        } else {
                            Ok(Value::Bool(x == y))
                        }
                    }
                    _ => Ok(Value::Bool(a == b)),
                }
            }

            Op::CmpLt(a_reg, b_reg) => {
                match (self.get_reg_ref(*a_reg)?, self.get_reg_ref(*b_reg)?) {
                    (Value::I64(x), Value::I64(y)) => Ok(Value::Bool(x < y)),
                    (Value::F64(x), Value::F64(y)) => Ok(Value::Bool(x < y)),
                    (Value::Str(x), Value::Str(y)) => Ok(Value::Bool(x < y)),
                    (x, y) => Err(VmError::TypeMismatch {
                        expected: "comparable types (i64, f64, str)".to_string(),
                        actual: format!("{}, {}", x.type_name(), y.type_name()),
                        line,
                    }),
                }
            }

            Op::BufAlloc(size_reg) => {
                let size = self.borrow_i64(*size_reg, line)?;
                if size < 0 {
                    return Err(VmError::InvalidInstruction {
                        message: format!("Cannot allocate buffer with negative size: {size}"),
                        line,
                    });
                }
                Ok(Value::Buf(vec![0u8; size as usize]))
            }

            Op::BufReadU8(buf_reg, idx_reg) => {
                let idx = self.borrow_i64(*idx_reg, line)?;
                let buf = match self.get_reg_ref(*buf_reg)? {
                    Value::Buf(b) => b,
                    other => {
                        return Err(VmError::TypeMismatch {
                            expected: "buf".to_string(),
                            actual: other.type_name().to_string(),
                            line,
                        })
                    }
                };
                if idx < 0 || idx as usize >= buf.len() {
                    return Err(VmError::IndexOutOfBounds {
                        index: idx as usize,
                        len: buf.len(),
                        line,
                    });
                }
                Ok(Value::I64(buf[idx as usize] as i64))
            }

            Op::BufWriteU8(buf_reg, idx_reg, val_reg) => {
                let idx = self.borrow_i64(*idx_reg, line)?;
                let val = self.borrow_i64(*val_reg, line)?;
                let mut buf = self.take_or_clone_reg(*buf_reg, meta)?.try_into_buf(line)?;
                if idx < 0 || idx as usize >= buf.len() {
                    return Err(VmError::IndexOutOfBounds {
                        index: idx as usize,
                        len: buf.len(),
                        line,
                    });
                }
                buf[idx as usize] = (val & 0xFF) as u8;
                Ok(Value::Buf(buf))
            }

            Op::BufSlice(buf_reg, start_reg, len_reg) => {
                let start = self.borrow_i64(*start_reg, line)?;
                let len = self.borrow_i64(*len_reg, line)?;
                let buf = match self.get_reg_ref(*buf_reg)? {
                    Value::Buf(b) => b,
                    other => {
                        return Err(VmError::TypeMismatch {
                            expected: "buf".to_string(),
                            actual: other.type_name().to_string(),
                            line,
                        })
                    }
                };
                if start < 0 || len < 0 {
                    return Err(VmError::IndexOutOfBounds {
                        index: usize::MAX,
                        len: buf.len(),
                        line,
                    });
                }
                let end = match start.checked_add(len) {
                    Some(e) => e as usize,
                    None => {
                        return Err(VmError::IndexOutOfBounds {
                            index: (start as usize).saturating_add(len as usize),
                            len: buf.len(),
                            line,
                        });
                    }
                };
                if start as usize > buf.len() || end > buf.len() {
                    return Err(VmError::IndexOutOfBounds {
                        index: end,
                        len: buf.len(),
                        line,
                    });
                }
                Ok(Value::Buf(buf[start as usize..end].to_vec()))
            }

            Op::VecNew(_) => Ok(Value::Vec(Vec::new())),

            Op::VecPush(vec_reg, item_reg) => {
                let item = self.take_or_clone_reg(*item_reg, meta)?;
                let mut vec = self.take_or_clone_reg(*vec_reg, meta)?.try_into_vec(line)?;
                vec.push(item);
                Ok(Value::Vec(vec))
            }

            Op::VecGet(vec_reg, idx_reg) => {
                let idx = self.borrow_i64(*idx_reg, line)?;
                let vec = match self.get_reg_ref(*vec_reg)? {
                    Value::Vec(v) => v,
                    other => {
                        return Err(VmError::TypeMismatch {
                            expected: "vec".to_string(),
                            actual: other.type_name().to_string(),
                            line,
                        })
                    }
                };
                if idx < 0 || idx as usize >= vec.len() {
                    return Err(VmError::IndexOutOfBounds {
                        index: idx as usize,
                        len: vec.len(),
                        line,
                    });
                }
                Ok(vec[idx as usize].clone())
            }

            Op::VecLen(vec_reg) => match self.get_reg_ref(*vec_reg)? {
                Value::Vec(v) => Ok(Value::I64(v.len() as i64)),
                other => Err(VmError::TypeMismatch {
                    expected: "vec".to_string(),
                    actual: other.type_name().to_string(),
                    line,
                }),
            },

            Op::MapNew(_, _) => Ok(Value::Map(BTreeMap::new())),

            Op::MapSet(map_reg, key_reg, val_reg) => {
                let key = Self::value_to_map_key(self.get_reg_ref(*key_reg)?);
                let val = self.take_or_clone_reg(*val_reg, meta)?;
                let mut map = self.take_or_clone_reg(*map_reg, meta)?.try_into_map(line)?;
                map.insert(key, val);
                Ok(Value::Map(map))
            }

            Op::MapGet(map_reg, key_reg) => {
                let key = Self::value_to_map_key(self.get_reg_ref(*key_reg)?);
                let map = match self.get_reg_ref(*map_reg)? {
                    Value::Map(m) => m,
                    other => {
                        return Err(VmError::TypeMismatch {
                            expected: "map".to_string(),
                            actual: other.type_name().to_string(),
                            line,
                        })
                    }
                };
                Ok(map.get(&key).cloned().unwrap_or(Value::Void))
            }

            Op::MapDel(map_reg, key_reg) => {
                let key = Self::value_to_map_key(self.get_reg_ref(*key_reg)?);
                let mut map = self.take_or_clone_reg(*map_reg, meta)?.try_into_map(line)?;
                map.remove(&key);
                Ok(Value::Map(map))
            }

            Op::ChanNew(_, cap_reg) => {
                let cap = self.borrow_i64(*cap_reg, line)?;
                if cap < 0 {
                    return Err(VmError::InvalidInstruction {
                        message: format!("Cannot create channel with negative capacity: {cap}"),
                        line,
                    });
                }
                let chan_id = self.next_chan_id;
                self.next_chan_id += 1;
                self.channels
                    .insert(chan_id, VecDeque::with_capacity(cap as usize));
                Ok(Value::Chan(chan_id))
            }

            Op::ChanSend(chan_reg, val_reg) => {
                let chan_id = match self.get_reg_ref(*chan_reg)? {
                    Value::Chan(id) => *id,
                    other => {
                        return Err(VmError::TypeMismatch {
                            expected: "chan".to_string(),
                            actual: other.type_name().to_string(),
                            line,
                        })
                    }
                };
                let val = self.take_or_clone_reg(*val_reg, meta)?;
                let queue = self.channels.get_mut(&chan_id).ok_or_else(|| {
                    VmError::InvalidInstruction {
                        message: format!("Unknown channel ID: {chan_id}"),
                        line,
                    }
                })?;
                queue.push_back(val);
                Ok(Value::Bool(true))
            }

            Op::ChanRecv(chan_reg) => {
                let chan_id = match self.get_reg_ref(*chan_reg)? {
                    Value::Chan(id) => *id,
                    other => {
                        return Err(VmError::TypeMismatch {
                            expected: "chan".to_string(),
                            actual: other.type_name().to_string(),
                            line,
                        })
                    }
                };
                let queue = self.channels.get_mut(&chan_id).ok_or_else(|| {
                    VmError::InvalidInstruction {
                        message: format!("Unknown channel ID: {chan_id}"),
                        line,
                    }
                })?;
                Ok(queue.pop_front().unwrap_or(Value::Void))
            }

            Op::Spawn(name, arg_reg) => {
                let _ = self.get_reg_ref(*arg_reg)?;
                if self.module.is_some() && !self.fn_index.contains_key(name.as_str()) {
                    return Err(VmError::NoSuchFunction { name: name.clone() });
                }
                Ok(Value::Bool(true))
            }

            Op::Syscall(effect, op_name, payload_reg) => {
                let payload = self.take_or_clone_reg(*payload_reg, meta)?;
                let result = self.platform.host_call(*effect, op_name, payload)?;
                if (*effect == Effect::Net && op_name == "net.wait_req")
                    || (*effect == Effect::Ui && op_name == "ui.poll_event")
                {
                    self.remaining_steps = self.max_steps;
                }
                Ok(result)
            }

            Op::Free(reg) => {
                if self.registers.remove(*reg).is_none() {
                    return Err(VmError::InvalidRegisterAccess { reg: *reg as usize });
                }
                Ok(Value::Void)
            }

            Op::StructNew(name) => {
                if let Some(smeta) = self.schema_cache.get(name.as_str()) {
                    Ok(smeta.default_val.clone())
                } else {
                    Ok(Value::Struct {
                        schema: name.clone(),
                        fields: Vec::new(),
                    })
                }
            }

            Op::StructGet(s_reg, field_idx) => {
                let (schema, fields) = match self.get_reg_ref(*s_reg)? {
                    Value::Struct { schema, fields } => (schema.as_str(), fields),
                    other => {
                        return Err(VmError::TypeMismatch {
                            expected: "struct".to_string(),
                            actual: other.type_name().to_string(),
                            line,
                        })
                    }
                };
                let idx = *field_idx as usize;
                if idx >= fields.len() {
                    return Err(VmError::IndexOutOfBounds {
                        index: idx,
                        len: fields.len(),
                        line,
                    });
                }
                if let Some(smeta) = self.schema_cache.get(schema) {
                    if smeta.field_types.get(idx).and_then(|t| *t).is_none() {
                        return Err(VmError::IndexOutOfBounds {
                            index: idx,
                            len: smeta.field_count,
                            line,
                        });
                    }
                }
                Ok(fields[idx].clone())
            }

            Op::StructSet(s_reg, field_idx, val_reg) => {
                let val = self.take_or_clone_reg(*val_reg, meta)?;
                let (schema, mut fields) =
                    self.take_or_clone_reg(*s_reg, meta)?.try_into_struct(line)?;
                let idx = *field_idx as usize;
                if let Some(smeta) = self.schema_cache.get(schema.as_str()) {
                    match smeta.field_types.get(idx).and_then(|t| *t) {
                        Some(expected_ty) => {
                            if !value_matches_type(&val, expected_ty) {
                                return Err(VmError::TypeMismatch {
                                    expected: expected_ty.to_string(),
                                    actual: val.type_name().to_string(),
                                    line,
                                });
                            }
                        }
                        None => {
                            return Err(VmError::IndexOutOfBounds {
                                index: idx,
                                len: smeta.field_count,
                                line,
                            });
                        }
                    }
                    if idx >= fields.len() {
                        return Err(VmError::IndexOutOfBounds {
                            index: idx,
                            len: fields.len(),
                            line,
                        });
                    }
                } else if idx >= fields.len() {
                    if idx >= 65536 {
                        return Err(VmError::IndexOutOfBounds {
                            index: idx,
                            len: fields.len(),
                            line,
                        });
                    }
                    fields.resize(idx + 1, Value::Void);
                }
                fields[idx] = val;
                Ok(Value::Struct { schema, fields })
            }

            Op::Call(fn_name, args) => self.exec_call(fn_name, args, line, meta),
        }
    }

    fn get_or_build_module_fn_meta(&mut self, fn_idx: usize) -> FnMeta {
        if let Some(ref existing) = self.fn_metas[fn_idx] {
            return existing.clone();
        }
        let func = &self.module.unwrap().functions[fn_idx];
        let built = FnMeta::build(func);
        self.fn_metas[fn_idx] = Some(built.clone());
        built
    }

    fn exec_call(
        &mut self,
        fn_name: &str,
        args: &[RegId],
        line: usize,
        caller_meta: &FnMeta,
    ) -> Result<Value, VmError> {
        // Fast path: module function resolved by O(1) index, zero `Function::clone()`!
        if let (Some(module), Some(&fn_idx)) = (self.module, self.fn_index.get(fn_name)) {
            let target_fn = &module.functions[fn_idx];
            if target_fn.params.len() != args.len() {
                return Err(VmError::InvalidInstruction {
                    message: format!(
                        "Argument count mismatch calling '{}': expected {}, got {}",
                        fn_name,
                        target_fn.params.len(),
                        args.len()
                    ),
                    line,
                });
            }

            const MAX_CALL_DEPTH: usize = 64;
            if self.call_depth >= MAX_CALL_DEPTH {
                return Err(VmError::InvalidInstruction {
                    message: format!(
                        "Maximum call depth exceeded ({MAX_CALL_DEPTH}) calling '{}'",
                        fn_name
                    ),
                    line,
                });
            }

            // Pure 0-arg constant function memoization (e.g. `style_default()` called 141x per frame)
            // Skipped when tracing or profiling so traces & per-function stats remain 100% exact.
            let can_memoize = !self.trace_enabled && self.stats.is_none() && args.is_empty();
            if can_memoize {
                if let Some((cached_val, cached_steps)) = self.pure_const_cache.get(&fn_idx) {
                    let steps = *cached_steps;
                    let val = cached_val.clone();
                    self.consume_gas_n(steps)?;
                    return Ok(val);
                }
            }

            let mut arg_vals = Vec::with_capacity(args.len());
            for &r in args {
                arg_vals.push(self.take_or_clone_reg(r, caller_meta)?);
            }

            let callee_meta = self.get_or_build_module_fn_meta(fn_idx);
            let mut callee_frame = self
                .frame_pool
                .pop()
                .unwrap_or_else(|| RegisterFile::with_capacity(callee_meta.reg_capacity));
            callee_frame.reset_for(callee_meta.reg_capacity);
            for ((reg, _ty), arg) in target_fn.params.iter().zip(arg_vals) {
                callee_frame.set(*reg, arg);
            }

            let saved_registers = std::mem::replace(&mut self.registers, callee_frame);
            let steps_before = self.step_count;
            self.call_depth += 1;
            let result = self.execute_function_body(target_fn, &callee_meta);
            self.call_depth -= 1;

            let finished_frame = std::mem::replace(&mut self.registers, saved_registers);
            if self.frame_pool.len() < 16 {
                self.frame_pool.push(finished_frame);
            }

            if can_memoize && callee_meta.is_pure_const {
                if let Ok(ref val) = result {
                    let steps_delta = self.step_count.wrapping_sub(steps_before);
                    self.pure_const_cache
                        .insert(fn_idx, (val.clone(), steps_delta));
                }
            }

            return result;
        }

        // Fallback path for single standalone function (e.g. `execute(func, ...)`)
        let (target_fn, callee_meta) = self
            .fallback_func
            .as_ref()
            .filter(|(f, _)| f.name == fn_name)
            .cloned()
            .ok_or_else(|| VmError::InvalidInstruction {
                message: format!("Function '{}' not found in module", fn_name),
                line,
            })?;

        if target_fn.params.len() != args.len() {
            return Err(VmError::InvalidInstruction {
                message: format!(
                    "Argument count mismatch calling '{}': expected {}, got {}",
                    fn_name,
                    target_fn.params.len(),
                    args.len()
                ),
                line,
            });
        }

        const MAX_CALL_DEPTH: usize = 64;
        if self.call_depth >= MAX_CALL_DEPTH {
            return Err(VmError::InvalidInstruction {
                message: format!(
                    "Maximum call depth exceeded ({MAX_CALL_DEPTH}) calling '{}'",
                    fn_name
                ),
                line,
            });
        }

        let mut arg_vals = Vec::with_capacity(args.len());
        for &r in args {
            arg_vals.push(self.take_or_clone_reg(r, caller_meta)?);
        }

        let mut callee_frame = self
            .frame_pool
            .pop()
            .unwrap_or_else(|| RegisterFile::with_capacity(callee_meta.reg_capacity));
        callee_frame.reset_for(callee_meta.reg_capacity);
        for ((reg, _ty), arg) in target_fn.params.iter().zip(arg_vals) {
            callee_frame.set(*reg, arg);
        }

        let saved_registers = std::mem::replace(&mut self.registers, callee_frame);
        self.call_depth += 1;
        let result = self.execute_function_body(&target_fn, &callee_meta);
        self.call_depth -= 1;

        let finished_frame = std::mem::replace(&mut self.registers, saved_registers);
        if self.frame_pool.len() < 16 {
            self.frame_pool.push(finished_frame);
        }
        result
    }

    /// Runs the function to completion or until gas exhaustion / error.
    pub fn run(
        &mut self,
        func: &Function,
        args: Vec<Value>,
    ) -> Result<(Value, Vec<TraceRecord>), VmError> {
        let meta = if let Some(&fn_idx) = self.fn_index.get(func.name.as_str()) {
            self.get_or_build_module_fn_meta(fn_idx)
        } else {
            let built = FnMeta::build(func);
            if self.fallback_func.is_none() {
                self.fallback_func = Some((func.clone(), built.clone()));
            }
            built
        };

        if func.params.len() != args.len() {
            return Err(VmError::InvalidInstruction {
                message: format!(
                    "Argument count mismatch: expected {}, got {}",
                    func.params.len(),
                    args.len()
                ),
                line: 0,
            });
        }

        self.registers.reset_for(meta.reg_capacity);
        for ((reg, _ty), arg) in func.params.iter().zip(args) {
            self.registers.set(*reg, arg);
        }

        let ret_val = self.execute_function_body(func, &meta)?;
        if let Some(ref mut stats) = self.stats {
            stats.gas_used = self.step_count;
        }
        Ok((ret_val, std::mem::take(&mut self.trace_records)))
    }

    fn execute_function_body(
        &mut self,
        func: &Function,
        meta: &FnMeta,
    ) -> Result<Value, VmError> {
        let profiling = self.stats.is_some();
        if let Some(ref mut stats) = self.stats {
            *stats.function_calls.entry(func.name.clone()).or_insert(0) += 1;
            stats
                .function_instructions
                .entry(func.name.clone())
                .or_insert(0);
        }

        if func.blocks.is_empty() {
            return Ok(Value::Void);
        }

        let mut current_block_idx: usize = 0;

        loop {
            let block: &BasicBlock = &func.blocks[current_block_idx];

            // Execute regular instructions
            for inst in &block.instructions {
                self.consume_gas()?;

                if profiling {
                    if let Some(ref mut stats) = self.stats {
                        stats.total_instructions += 1;
                        *stats
                            .function_instructions
                            .entry(func.name.clone())
                            .or_insert(0) += 1;
                        match &inst.op {
                            Op::StructNew(_)
                            | Op::VecNew(_)
                            | Op::MapNew(_, _)
                            | Op::BufAlloc(_)
                            | Op::ChanNew(_, _) => {
                                stats.allocations += 1;
                            }
                            Op::Free(_) => {
                                stats.frees += 1;
                            }
                            _ => {}
                        }
                    }
                }

                let result = if let Op::Call(fn_name, args) = &inst.op {
                    self.exec_call(fn_name, args, inst.line, meta)?
                } else {
                    self.exec_op(&inst.op, inst.line, meta)?
                };

                if self.trace_enabled {
                    let json_val = inst.dst.map(|_| result.to_json());
                    if let Some(dst) = inst.dst {
                        self.registers.set(dst, result);
                    }
                    let record = TraceRecord::new(
                        self.step_count,
                        inst.line,
                        block.id as usize,
                        Self::opcode_name(&inst.op),
                        inst.dst.map(|r| format!("%{r}")),
                        json_val,
                    );
                    self.trace_records.push(record);
                } else if let Some(dst) = inst.dst {
                    self.registers.set(dst, result);
                }
            }

            // Execute block terminator
            self.consume_gas()?;

            let term_line = block
                .instructions
                .last()
                .map(|i| i.line + 1)
                .unwrap_or(1);

            match &block.terminator {
                Terminator::Ret(Some(reg)) => {
                    let ret_val = self.take_or_clone_reg(*reg, meta)?;
                    if self.trace_enabled {
                        self.trace_records.push(TraceRecord::new(
                            self.step_count,
                            term_line,
                            block.id as usize,
                            "ret",
                            None,
                            Some(ret_val.to_json()),
                        ));
                    }
                    return Ok(ret_val);
                }

                Terminator::Ret(None) => {
                    if self.trace_enabled {
                        self.trace_records.push(TraceRecord::new(
                            self.step_count,
                            term_line,
                            block.id as usize,
                            "ret",
                            None,
                            None,
                        ));
                    }
                    return Ok(Value::Void);
                }

                Terminator::Br(target) => {
                    if self.trace_enabled {
                        self.trace_records.push(TraceRecord::new(
                            self.step_count,
                            term_line,
                            block.id as usize,
                            "br",
                            None,
                            None,
                        ));
                    }
                    current_block_idx =
                        meta.resolve_block(*target).ok_or(VmError::NoSuchBlock {
                            block_id: *target as usize,
                        })?;
                }

                Terminator::BrIf {
                    cond,
                    then_blk,
                    else_blk,
                } => {
                    let cond_bool = match self.get_reg_ref(*cond)? {
                        Value::Bool(b) => *b,
                        other => {
                            return Err(VmError::TypeMismatch {
                                expected: "bool".to_string(),
                                actual: other.type_name().to_string(),
                                line: term_line,
                            })
                        }
                    };

                    if self.trace_enabled {
                        self.trace_records.push(TraceRecord::new(
                            self.step_count,
                            term_line,
                            block.id as usize,
                            "br_if",
                            None,
                            Some(serde_json::json!(cond_bool)),
                        ));
                    }

                    let target = if cond_bool { *then_blk } else { *else_blk };
                    current_block_idx = meta.resolve_block(target).ok_or(VmError::NoSuchBlock {
                        block_id: target as usize,
                    })?;
                }

                Terminator::BrArgs { target, args } => {
                    let mut arg_vals = Vec::with_capacity(args.len());
                    for &r in args {
                        arg_vals.push(self.take_or_clone_reg(r, meta)?);
                    }
                    if self.trace_enabled {
                        self.trace_records.push(TraceRecord::new(
                            self.step_count,
                            term_line,
                            block.id as usize,
                            "br",
                            None,
                            None,
                        ));
                    }
                    let target_idx = meta.resolve_block(*target).ok_or(VmError::NoSuchBlock {
                        block_id: *target as usize,
                    })?;
                    let target_blk = &func.blocks[target_idx];
                    for ((param_reg, _), val) in target_blk.params.iter().zip(arg_vals) {
                        self.registers.set(*param_reg, val);
                    }
                    current_block_idx = target_idx;
                }

                Terminator::BrIfArgs {
                    cond,
                    then_blk,
                    then_args,
                    else_blk,
                    else_args,
                } => {
                    let cond_bool = match self.get_reg_ref(*cond)? {
                        Value::Bool(b) => *b,
                        other => {
                            return Err(VmError::TypeMismatch {
                                expected: "bool".to_string(),
                                actual: other.type_name().to_string(),
                                line: term_line,
                            })
                        }
                    };
                    let (target, chosen_args) = if cond_bool {
                        (*then_blk, then_args)
                    } else {
                        (*else_blk, else_args)
                    };
                    let mut arg_vals = Vec::with_capacity(chosen_args.len());
                    for &r in chosen_args {
                        arg_vals.push(self.take_or_clone_reg(r, meta)?);
                    }
                    if self.trace_enabled {
                        self.trace_records.push(TraceRecord::new(
                            self.step_count,
                            term_line,
                            block.id as usize,
                            "br_if",
                            None,
                            Some(serde_json::json!(cond_bool)),
                        ));
                    }
                    let target_idx = meta.resolve_block(target).ok_or(VmError::NoSuchBlock {
                        block_id: target as usize,
                    })?;
                    let target_blk = &func.blocks[target_idx];
                    for ((param_reg, _), val) in target_blk.params.iter().zip(arg_vals) {
                        self.registers.set(*param_reg, val);
                    }
                    current_block_idx = target_idx;
                }
            }
        }
    }
}

/// Executes a single SSA function using the reference interpreter.
pub fn execute(
    func: &Function,
    args: Vec<Value>,
    platform: &mut dyn HostPlatform,
    max_steps: u64,
    trace: bool,
) -> Result<(Value, Vec<TraceRecord>), VmError> {
    let mut vm = Vm::new(platform, None, max_steps, trace);
    vm.run(func, args)
}

/// Executes a single SSA function with schemas using the reference interpreter.
pub fn execute_with_schemas(
    func: &Function,
    schemas: &[Schema],
    args: Vec<Value>,
    platform: &mut dyn HostPlatform,
    max_steps: u64,
    trace: bool,
) -> Result<(Value, Vec<TraceRecord>), VmError> {
    let mut vm = Vm::new_with_schemas(platform, None, schemas, max_steps, trace);
    vm.run(func, args)
}

/// Executes an entry-point function within an SSA module using the reference interpreter.
pub fn execute_module(
    module: &Module,
    entry: &str,
    args: Vec<Value>,
    platform: &mut dyn HostPlatform,
    max_steps: u64,
    trace: bool,
) -> Result<(Value, Vec<TraceRecord>), VmError> {
    let func = module
        .get_function(entry)
        .ok_or_else(|| VmError::NoSuchFunction {
            name: entry.to_string(),
        })?;
    let mut vm = Vm::new(platform, Some(module), max_steps, trace);
    vm.run(func, args)
}

/// Executes an entry-point function within an SSA module while collecting deterministic
/// execution statistics (instruction counts, gas used, per-function breakdown, allocations & frees).
pub fn execute_module_with_stats(
    module: &Module,
    entry: &str,
    args: Vec<Value>,
    platform: &mut dyn HostPlatform,
    max_steps: u64,
    trace: bool,
) -> Result<(Value, Vec<TraceRecord>, ExecutionStats), VmError> {
    let func = module
        .get_function(entry)
        .ok_or_else(|| VmError::NoSuchFunction {
            name: entry.to_string(),
        })?;
    let mut vm = Vm::new(platform, Some(module), max_steps, trace);
    vm.enable_profiling();
    let (val, traces) = vm.run(func, args)?;
    let stats = vm.take_stats();
    Ok((val, traces, stats))
}

/// Returns a default initial value for a given SSA type, taking module schemas into account
/// for properly-sized struct fields and nested structs.
pub fn default_value_for_type_with_schemas(ty: &knr_core::Type, schemas: &[Schema]) -> Value {
    fn inner(ty: &knr_core::Type, schemas: &[Schema], depth: usize) -> Value {
        if depth > 32 {
            return Value::Void;
        }
        match ty {
            knr_core::Type::I64 => Value::I64(0),
            knr_core::Type::F64 => Value::F64(0.0),
            knr_core::Type::Bool => Value::Bool(false),
            knr_core::Type::Str => Value::Str(String::new()),
            knr_core::Type::Buf => Value::Buf(Vec::new()),
            knr_core::Type::Vec(_) => Value::Vec(Vec::new()),
            knr_core::Type::Map(_, _) => Value::Map(BTreeMap::new()),
            knr_core::Type::Chan(_) => Value::Chan(0),
            knr_core::Type::Struct(name) => {
                if let Some(schema) = schemas.iter().find(|s| s.name == *name) {
                    let max_idx = schema
                        .fields
                        .iter()
                        .map(|f| f.index as usize)
                        .max()
                        .map(|m| m + 1)
                        .unwrap_or(schema.fields.len())
                        .min(65536);
                    let mut fields = vec![Value::Void; max_idx];
                    for f in &schema.fields {
                        let idx = f.index as usize;
                        if idx < fields.len() {
                            fields[idx] = inner(&f.ty, schemas, depth + 1);
                        }
                    }
                    Value::Struct {
                        schema: name.clone(),
                        fields,
                    }
                } else {
                    Value::Struct {
                        schema: name.clone(),
                        fields: Vec::new(),
                    }
                }
            }
            knr_core::Type::Void => Value::Void,
        }
    }
    inner(ty, schemas, 0)
}

pub fn default_value_for_type(ty: &knr_core::Type) -> Value {
    default_value_for_type_with_schemas(ty, &[])
}

/// Checks whether a runtime `Value` matches a static `Type`.
pub fn value_matches_type(val: &Value, ty: &knr_core::Type) -> bool {
    match (val, ty) {
        (Value::I64(_), knr_core::Type::I64) => true,
        (Value::F64(_), knr_core::Type::F64) => true,
        (Value::Bool(_), knr_core::Type::Bool) => true,
        (Value::Str(_), knr_core::Type::Str) => true,
        (Value::Buf(_), knr_core::Type::Buf) => true,
        (Value::Chan(_), knr_core::Type::Chan(_)) => true,
        (Value::Struct { schema, .. }, knr_core::Type::Struct(expected_schema)) => {
            schema == expected_schema
        }
        (Value::Vec(items), knr_core::Type::Vec(elem_ty)) => {
            items.iter().all(|item| value_matches_type(item, elem_ty))
        }
        (Value::Map(_), knr_core::Type::Map(_, _)) => true,
        (Value::Void, knr_core::Type::Void) => true,
        _ => false,
    }
}
