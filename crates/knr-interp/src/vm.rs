use crate::error::VmError;
use crate::trace::TraceRecord;
use crate::value::{Value, ValueExt};
use knr_core::{BasicBlock, BlockId, Function, Module, Op, RegId, Schema, Terminator};
use knr_runtime::HostPlatform;
use std::collections::{BTreeMap, HashMap, VecDeque};

/// Virtual Machine execution engine for knr SSA programs.
pub struct Vm<'a> {
    registers: HashMap<RegId, Value>,
    channels: HashMap<usize, VecDeque<Value>>,
    next_chan_id: usize,
    platform: &'a mut dyn HostPlatform,
    module: Option<&'a Module>,
    schemas: &'a [Schema],
    max_steps: u64,
    remaining_steps: u64,
    step_count: u64,
    trace_enabled: bool,
    trace_records: Vec<TraceRecord>,
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
        Self {
            registers: HashMap::new(),
            channels: HashMap::new(),
            next_chan_id: 1,
            platform,
            module,
            schemas,
            max_steps,
            remaining_steps: max_steps,
            step_count: 0,
            trace_enabled,
            trace_records: Vec::new(),
        }
    }

    /// Fetches a register's value or returns `InvalidRegisterAccess`.
    #[inline]
    fn get_reg(&self, reg: RegId, _line: usize) -> Result<Value, VmError> {
        self.registers
            .get(&reg)
            .cloned()
            .ok_or(VmError::InvalidRegisterAccess {
                reg: reg as usize,
            })
    }

    /// Converts a `Value` into a string key for map operations.
    fn value_to_map_key(val: &Value) -> String {
        match val {
            Value::Str(s) => s.clone(),
            other => other.to_string(),
        }
    }

    /// Decrements gas and increments step counter before an instruction or terminator.
    #[inline]
    fn consume_gas(&mut self) -> Result<(), VmError> {
        if self.remaining_steps == 0 {
            return Err(VmError::GasExhausted {
                max_steps: self.max_steps,
            });
        }
        self.remaining_steps -= 1;
        self.step_count += 1;
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
            Op::CmpEq(_, _) => "cmp_eq",
            Op::CmpLt(_, _) => "cmp_lt",
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
        }
    }

    /// Executes a single opcode, returning the produced value.
    fn exec_op(&mut self, op: &Op, line: usize) -> Result<Value, VmError> {
        match op {
            Op::ConstI64(v) => Ok(Value::I64(*v)),
            Op::ConstF64(v) => Ok(Value::F64(*v)),
            Op::ConstBool(v) => Ok(Value::Bool(*v)),
            Op::ConstStr(s) => Ok(Value::Str(s.clone())),

            Op::Add(a_reg, b_reg) => {
                let a = self.get_reg(*a_reg, line)?;
                let b = self.get_reg(*b_reg, line)?;
                match (a, b) {
                    (Value::I64(x), Value::I64(y)) => Ok(Value::I64(x.wrapping_add(y))),
                    (Value::F64(x), Value::F64(y)) => Ok(Value::F64(x + y)),
                    (Value::Str(x), Value::Str(y)) => Ok(Value::Str(format!("{x}{y}"))),
                    (x, y) => Err(VmError::TypeMismatch {
                        expected: "i64, f64, or str".to_string(),
                        actual: format!("{}, {}", x.type_name(), y.type_name()),
                        line,
                    }),
                }
            }

            Op::Sub(a_reg, b_reg) => {
                let a = self.get_reg(*a_reg, line)?;
                let b = self.get_reg(*b_reg, line)?;
                match (a, b) {
                    (Value::I64(x), Value::I64(y)) => Ok(Value::I64(x.wrapping_sub(y))),
                    (Value::F64(x), Value::F64(y)) => Ok(Value::F64(x - y)),
                    (x, y) => Err(VmError::TypeMismatch {
                        expected: "i64 or f64".to_string(),
                        actual: format!("{}, {}", x.type_name(), y.type_name()),
                        line,
                    }),
                }
            }

            Op::Mul(a_reg, b_reg) => {
                let a = self.get_reg(*a_reg, line)?;
                let b = self.get_reg(*b_reg, line)?;
                match (a, b) {
                    (Value::I64(x), Value::I64(y)) => Ok(Value::I64(x.wrapping_mul(y))),
                    (Value::F64(x), Value::F64(y)) => Ok(Value::F64(x * y)),
                    (x, y) => Err(VmError::TypeMismatch {
                        expected: "i64 or f64".to_string(),
                        actual: format!("{}, {}", x.type_name(), y.type_name()),
                        line,
                    }),
                }
            }

            Op::Div(a_reg, b_reg) => {
                let a = self.get_reg(*a_reg, line)?;
                let b = self.get_reg(*b_reg, line)?;
                match (a, b) {
                    (Value::I64(x), Value::I64(y)) => {
                        if y == 0 {
                            return Err(VmError::DivisionByZero { line });
                        }
                        if x == i64::MIN && y == -1 {
                            Ok(Value::I64(i64::MIN))
                        } else {
                            Ok(Value::I64(x / y))
                        }
                    }
                    (Value::F64(x), Value::F64(y)) => Ok(Value::F64(x / y)),
                    (x, y) => Err(VmError::TypeMismatch {
                        expected: "i64 or f64".to_string(),
                        actual: format!("{}, {}", x.type_name(), y.type_name()),
                        line,
                    }),
                }
            }

            Op::CmpEq(a_reg, b_reg) => {
                let a = self.get_reg(*a_reg, line)?;
                let b = self.get_reg(*b_reg, line)?;
                match (&a, &b) {
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
                let a = self.get_reg(*a_reg, line)?;
                let b = self.get_reg(*b_reg, line)?;
                match (a, b) {
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
                let size = self.get_reg(*size_reg, line)?.try_into_i64(line)?;
                if size < 0 {
                    return Err(VmError::InvalidInstruction {
                        message: format!("Cannot allocate buffer with negative size: {size}"),
                        line,
                    });
                }
                Ok(Value::Buf(vec![0u8; size as usize]))
            }

            Op::BufReadU8(buf_reg, idx_reg) => {
                let buf = self.get_reg(*buf_reg, line)?.try_into_buf(line)?;
                let idx = self.get_reg(*idx_reg, line)?.try_into_i64(line)?;
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
                let mut buf = self.get_reg(*buf_reg, line)?.try_into_buf(line)?;
                let idx = self.get_reg(*idx_reg, line)?.try_into_i64(line)?;
                let val = self.get_reg(*val_reg, line)?.try_into_i64(line)?;
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
                let buf = self.get_reg(*buf_reg, line)?.try_into_buf(line)?;
                let start = self.get_reg(*start_reg, line)?.try_into_i64(line)?;
                let len = self.get_reg(*len_reg, line)?.try_into_i64(line)?;
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
                let slice = buf[start as usize..end].to_vec();
                Ok(Value::Buf(slice))
            }

            Op::VecNew(_) => Ok(Value::Vec(Vec::new())),

            Op::VecPush(vec_reg, item_reg) => {
                let mut vec = self.get_reg(*vec_reg, line)?.try_into_vec(line)?;
                let item = self.get_reg(*item_reg, line)?;
                vec.push(item);
                Ok(Value::Vec(vec))
            }

            Op::VecGet(vec_reg, idx_reg) => {
                let vec = self.get_reg(*vec_reg, line)?.try_into_vec(line)?;
                let idx = self.get_reg(*idx_reg, line)?.try_into_i64(line)?;
                if idx < 0 || idx as usize >= vec.len() {
                    return Err(VmError::IndexOutOfBounds {
                        index: idx as usize,
                        len: vec.len(),
                        line,
                    });
                }
                Ok(vec[idx as usize].clone())
            }

            Op::VecLen(vec_reg) => {
                let vec = self.get_reg(*vec_reg, line)?.try_into_vec(line)?;
                Ok(Value::I64(vec.len() as i64))
            }

            Op::MapNew(_, _) => Ok(Value::Map(BTreeMap::new())),

            Op::MapSet(map_reg, key_reg, val_reg) => {
                let mut map = self.get_reg(*map_reg, line)?.try_into_map(line)?;
                let key_val = self.get_reg(*key_reg, line)?;
                let key = Self::value_to_map_key(&key_val);
                let val = self.get_reg(*val_reg, line)?;
                map.insert(key, val);
                Ok(Value::Map(map))
            }

            Op::MapGet(map_reg, key_reg) => {
                let map = self.get_reg(*map_reg, line)?.try_into_map(line)?;
                let key_val = self.get_reg(*key_reg, line)?;
                let key = Self::value_to_map_key(&key_val);
                Ok(map.get(&key).cloned().unwrap_or(Value::Void))
            }

            Op::MapDel(map_reg, key_reg) => {
                let mut map = self.get_reg(*map_reg, line)?.try_into_map(line)?;
                let key_val = self.get_reg(*key_reg, line)?;
                let key = Self::value_to_map_key(&key_val);
                map.remove(&key);
                Ok(Value::Map(map))
            }

            Op::ChanNew(_, cap_reg) => {
                let cap = self.get_reg(*cap_reg, line)?.try_into_i64(line)?;
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
                let chan_val = self.get_reg(*chan_reg, line)?;
                let chan_id = match chan_val {
                    Value::Chan(id) => id,
                    other => {
                        return Err(VmError::TypeMismatch {
                            expected: "chan".to_string(),
                            actual: other.type_name().to_string(),
                            line,
                        })
                    }
                };
                let val = self.get_reg(*val_reg, line)?;
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
                let chan_val = self.get_reg(*chan_reg, line)?;
                let chan_id = match chan_val {
                    Value::Chan(id) => id,
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
                let _arg = self.get_reg(*arg_reg, line)?;
                if let Some(module) = self.module {
                    if module.get_function(name).is_none() {
                        return Err(VmError::NoSuchFunction { name: name.clone() });
                    }
                }
                Ok(Value::Bool(true))
            }

            Op::Syscall(effect, op_name, payload_reg) => {
                let payload = self.get_reg(*payload_reg, line)?;
                let result = self.platform.host_call(*effect, op_name, payload)?;
                Ok(result)
            }

            Op::Free(reg) => {
                let _ = self.get_reg(*reg, line)?;
                self.registers.remove(reg);
                Ok(Value::Void)
            }

            Op::StructNew(name) => {
                let val = default_value_for_type_with_schemas(
                    &knr_core::Type::Struct(name.clone()),
                    self.schemas,
                );
                Ok(val)
            }

            Op::StructGet(s_reg, field_idx) => {
                let (schema, fields) = self.get_reg(*s_reg, line)?.try_into_struct(line)?;
                let idx = *field_idx as usize;
                if idx >= fields.len() {
                    return Err(VmError::IndexOutOfBounds {
                        index: idx,
                        len: fields.len(),
                        line,
                    });
                }
                if let Some(schema_def) = self.schemas.iter().find(|s| s.name == schema) {
                    if !schema_def.fields.iter().any(|f| f.index == *field_idx) {
                        return Err(VmError::IndexOutOfBounds {
                            index: idx,
                            len: schema_def.fields.len(),
                            line,
                        });
                    }
                }
                Ok(fields[idx].clone())
            }

            Op::StructSet(s_reg, field_idx, val_reg) => {
                let (schema, mut fields) = self.get_reg(*s_reg, line)?.try_into_struct(line)?;
                let val = self.get_reg(*val_reg, line)?;
                let idx = *field_idx as usize;
                if let Some(schema_def) = self.schemas.iter().find(|s| s.name == schema) {
                    if let Some(field_def) = schema_def.fields.iter().find(|f| f.index == *field_idx) {
                        if !value_matches_type(&val, &field_def.ty) {
                            return Err(VmError::TypeMismatch {
                                expected: field_def.ty.to_string(),
                                actual: val.type_name().to_string(),
                                line,
                            });
                        }
                    } else {
                        return Err(VmError::IndexOutOfBounds {
                            index: idx,
                            len: schema_def.fields.len(),
                            line,
                        });
                    }
                }
                if idx >= fields.len() {
                    if self.schemas.iter().any(|s| s.name == schema) || idx >= 65536 {
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
        }
    }

    /// Runs the function to completion or until gas exhaustion / error.
    pub fn run(
        &mut self,
        func: &Function,
        args: Vec<Value>,
    ) -> Result<(Value, Vec<TraceRecord>), VmError> {
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

        for ((reg, _ty), arg) in func.params.iter().zip(args) {
            self.registers.insert(*reg, arg);
        }

        if func.blocks.is_empty() {
            return Ok((Value::Void, std::mem::take(&mut self.trace_records)));
        }

        let block_map: HashMap<BlockId, &BasicBlock> =
            func.blocks.iter().map(|b| (b.id, b)).collect();
        let mut current_block_id = func.blocks[0].id;

        loop {
            let block = block_map
                .get(&current_block_id)
                .ok_or(VmError::NoSuchBlock {
                    block_id: current_block_id as usize,
                })?;

            // Execute regular instructions
            for inst in &block.instructions {
                self.consume_gas()?;

                let result = self.exec_op(&inst.op, inst.line)?;
                if let Some(dst) = inst.dst {
                    self.registers.insert(dst, result.clone());
                }

                if self.trace_enabled {
                    let record = TraceRecord::new(
                        self.step_count,
                        inst.line,
                        block.id as usize,
                        Self::opcode_name(&inst.op),
                        inst.dst.map(|r| format!("%{r}")),
                        inst.dst.map(|_| result.to_json()),
                    );
                    self.trace_records.push(record);
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
                    let ret_val = self.get_reg(*reg, term_line)?;
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
                    return Ok((ret_val, std::mem::take(&mut self.trace_records)));
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
                    return Ok((Value::Void, std::mem::take(&mut self.trace_records)));
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
                    if !block_map.contains_key(target) {
                        return Err(VmError::NoSuchBlock {
                            block_id: *target as usize,
                        });
                    }
                    current_block_id = *target;
                }

                Terminator::BrIf {
                    cond,
                    then_blk,
                    else_blk,
                } => {
                    let cond_val = self.get_reg(*cond, term_line)?;
                    let cond_bool = match cond_val {
                        Value::Bool(b) => b,
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

                    let target = if cond_bool { then_blk } else { else_blk };
                    if !block_map.contains_key(target) {
                        return Err(VmError::NoSuchBlock {
                            block_id: *target as usize,
                        });
                    }
                    current_block_id = *target;
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
