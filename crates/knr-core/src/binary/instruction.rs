use crate::ast::{BlockId, Effect, Instruction, Op, RegId, Terminator};
use crate::binary::const_pool::{ConstantPool, TypePool};
use crate::error::BinaryError;
use std::io::{Read, Write};

pub const NO_REG: u16 = 0xFFFF;

// Opcodes: 0x01 to 0x1C (Instructions), 0x1D to 0x1F (Terminators)
pub const OP_CONST_I64: u8 = 0x01;
pub const OP_CONST_F64: u8 = 0x02;
pub const OP_CONST_BOOL: u8 = 0x03;
pub const OP_CONST_STR: u8 = 0x04;
pub const OP_ADD: u8 = 0x05;
pub const OP_SUB: u8 = 0x06;
pub const OP_MUL: u8 = 0x07;
pub const OP_DIV: u8 = 0x08;
pub const OP_CMP_EQ: u8 = 0x09;
pub const OP_CMP_LT: u8 = 0x0A;
pub const OP_BUF_ALLOC: u8 = 0x0B;
pub const OP_BUF_READ_U8: u8 = 0x0C;
pub const OP_BUF_WRITE_U8: u8 = 0x0D;
pub const OP_BUF_SLICE: u8 = 0x0E;
pub const OP_VEC_NEW: u8 = 0x0F;
pub const OP_VEC_PUSH: u8 = 0x10;
pub const OP_VEC_GET: u8 = 0x11;
pub const OP_VEC_LEN: u8 = 0x12;
pub const OP_MAP_NEW: u8 = 0x13;
pub const OP_MAP_SET: u8 = 0x14;
pub const OP_MAP_GET: u8 = 0x15;
pub const OP_MAP_DEL: u8 = 0x16;
pub const OP_CHAN_NEW: u8 = 0x17;
pub const OP_CHAN_SEND: u8 = 0x18;
pub const OP_CHAN_RECV: u8 = 0x19;
pub const OP_SPAWN: u8 = 0x1A;
pub const OP_SYSCALL: u8 = 0x1B;
pub const OP_FREE: u8 = 0x1C;
pub const OP_RET: u8 = 0x1D;
pub const OP_BR: u8 = 0x1E;
pub const OP_BR_IF: u8 = 0x1F;
pub const OP_STRUCT_NEW: u8 = 0x21;
pub const OP_STRUCT_GET: u8 = 0x22;
pub const OP_STRUCT_SET: u8 = 0x23;


#[derive(Debug, Default, Clone, PartialEq)]
pub struct ExtraArgsPool {
    pub entries: Vec<[u16; 2]>,
}

impl ExtraArgsPool {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&mut self, arg1: u16, arg2: u16) -> Result<u16, BinaryError> {
        let idx = u16::try_from(self.entries.len())
            .map_err(|_| BinaryError::RegisterOverflow(self.entries.len() as u32))?;
        self.entries.push([arg1, arg2]);
        Ok(idx)
    }

    pub fn get(&self, idx: u16) -> Result<[u16; 2], BinaryError> {
        self.entries
            .get(idx as usize)
            .copied()
            .ok_or(BinaryError::PoolIndexOutOfBounds {
                pool: "ExtraArgsPool",
                index: idx as usize,
                size: self.entries.len(),
            })
    }

    pub fn encode(&self, w: &mut impl Write) -> Result<(), BinaryError> {
        w.write_all(&(self.entries.len() as u32).to_le_bytes())
            .map_err(|e| BinaryError::IoError(e.to_string()))?;
        for entry in &self.entries {
            w.write_all(&entry[0].to_le_bytes())
                .map_err(|e| BinaryError::IoError(e.to_string()))?;
            w.write_all(&entry[1].to_le_bytes())
                .map_err(|e| BinaryError::IoError(e.to_string()))?;
        }
        Ok(())
    }

    pub fn decode(cursor: &mut &[u8]) -> Result<Self, BinaryError> {
        let mut buf4 = [0u8; 4];
        cursor
            .read_exact(&mut buf4)
            .map_err(|_| BinaryError::UnexpectedEof {
                expected: 4,
                actual: 0,
            })?;
        let count = u32::from_le_bytes(buf4) as usize;
        let mut entries = Vec::with_capacity(count.min(cursor.len() / 4));
        let mut buf2 = [0u8; 2];
        for _ in 0..count {
            cursor
                .read_exact(&mut buf2)
                .map_err(|_| BinaryError::UnexpectedEof {
                    expected: 2,
                    actual: 0,
                })?;
            let a1 = u16::from_le_bytes(buf2);
            cursor
                .read_exact(&mut buf2)
                .map_err(|_| BinaryError::UnexpectedEof {
                    expected: 2,
                    actual: 0,
                })?;
            let a2 = u16::from_le_bytes(buf2);
            entries.push([a1, a2]);
        }
        Ok(Self { entries })
    }
}

fn reg_to_u16(r: RegId) -> Result<u16, BinaryError> {
    if r >= (NO_REG as u32) {
        Err(BinaryError::RegisterOverflow(r))
    } else {
        Ok(r as u16)
    }
}

pub fn pack_instruction(
    inst: &Instruction,
    const_pool: &mut ConstantPool,
    type_pool: &mut TypePool,
    extra_pool: &mut ExtraArgsPool,
) -> Result<[u8; 8], BinaryError> {
    let ty_id = type_pool.insert(&inst.ty)?;
    let dst = match inst.dst {
        Some(d) => reg_to_u16(d)?,
        None => NO_REG,
    };

    let (opcode, src1, src2) = match &inst.op {
        Op::ConstI64(v) => (OP_CONST_I64, const_pool.insert_int(*v)?, 0),
        Op::ConstF64(v) => (OP_CONST_F64, const_pool.insert_float(*v)?, 0),
        Op::ConstBool(b) => (OP_CONST_BOOL, if *b { 1 } else { 0 }, 0),
        Op::ConstStr(s) => (OP_CONST_STR, const_pool.insert_str(s)?, 0),
        Op::Add(r1, r2) => (OP_ADD, reg_to_u16(*r1)?, reg_to_u16(*r2)?),
        Op::Sub(r1, r2) => (OP_SUB, reg_to_u16(*r1)?, reg_to_u16(*r2)?),
        Op::Mul(r1, r2) => (OP_MUL, reg_to_u16(*r1)?, reg_to_u16(*r2)?),
        Op::Div(r1, r2) => (OP_DIV, reg_to_u16(*r1)?, reg_to_u16(*r2)?),
        Op::CmpEq(r1, r2) => (OP_CMP_EQ, reg_to_u16(*r1)?, reg_to_u16(*r2)?),
        Op::CmpLt(r1, r2) => (OP_CMP_LT, reg_to_u16(*r1)?, reg_to_u16(*r2)?),
        Op::BufAlloc(size) => (OP_BUF_ALLOC, reg_to_u16(*size)?, NO_REG),
        Op::BufReadU8(b, idx) => (OP_BUF_READ_U8, reg_to_u16(*b)?, reg_to_u16(*idx)?),
        Op::BufWriteU8(b, idx, v) => {
            let extra = extra_pool.insert(reg_to_u16(*idx)?, reg_to_u16(*v)?)?;
            (OP_BUF_WRITE_U8, reg_to_u16(*b)?, extra)
        }
        Op::BufSlice(b, start, len) => {
            let extra = extra_pool.insert(reg_to_u16(*start)?, reg_to_u16(*len)?)?;
            (OP_BUF_SLICE, reg_to_u16(*b)?, extra)
        }
        Op::VecNew(elem_ty) => (OP_VEC_NEW, type_pool.insert(elem_ty)? as u16, NO_REG),
        Op::VecPush(v, item) => (OP_VEC_PUSH, reg_to_u16(*v)?, reg_to_u16(*item)?),
        Op::VecGet(v, idx) => (OP_VEC_GET, reg_to_u16(*v)?, reg_to_u16(*idx)?),
        Op::VecLen(v) => (OP_VEC_LEN, reg_to_u16(*v)?, NO_REG),
        Op::MapNew(k, v) => (
            OP_MAP_NEW,
            type_pool.insert(k)? as u16,
            type_pool.insert(v)? as u16,
        ),
        Op::MapSet(m, k, v) => {
            let extra = extra_pool.insert(reg_to_u16(*k)?, reg_to_u16(*v)?)?;
            (OP_MAP_SET, reg_to_u16(*m)?, extra)
        }
        Op::MapGet(m, k) => (OP_MAP_GET, reg_to_u16(*m)?, reg_to_u16(*k)?),
        Op::MapDel(m, k) => (OP_MAP_DEL, reg_to_u16(*m)?, reg_to_u16(*k)?),
        Op::ChanNew(elem_ty, cap) => (
            OP_CHAN_NEW,
            type_pool.insert(elem_ty)? as u16,
            reg_to_u16(*cap)?,
        ),
        Op::ChanSend(c, val) => (OP_CHAN_SEND, reg_to_u16(*c)?, reg_to_u16(*val)?),
        Op::ChanRecv(c) => (OP_CHAN_RECV, reg_to_u16(*c)?, NO_REG),
        Op::Spawn(fn_name, arg) => (OP_SPAWN, const_pool.insert_str(fn_name)?, reg_to_u16(*arg)?),
        Op::Syscall(effect, op_name, payload) => {
            let eff_tag = match effect {
                Effect::Net => 0,
                Effect::Ui => 1,
                Effect::Hw => 2,
                Effect::Async => 3,
            };
            let extra = extra_pool.insert(eff_tag, reg_to_u16(*payload)?)?;
            (OP_SYSCALL, const_pool.insert_str(op_name)?, extra)
        }
        Op::Free(r) => (OP_FREE, reg_to_u16(*r)?, NO_REG),
        Op::StructNew(name) => (OP_STRUCT_NEW, const_pool.insert_str(name)?, NO_REG),
        Op::StructGet(s, field_idx) => {
            let idx_u16 = u16::try_from(*field_idx)
                .map_err(|_| BinaryError::RegisterOverflow(*field_idx))?;
            (OP_STRUCT_GET, reg_to_u16(*s)?, idx_u16)
        }
        Op::StructSet(s, field_idx, v) => {
            let idx_u16 = u16::try_from(*field_idx)
                .map_err(|_| BinaryError::RegisterOverflow(*field_idx))?;
            let extra = extra_pool.insert(idx_u16, reg_to_u16(*v)?)?;
            (OP_STRUCT_SET, reg_to_u16(*s)?, extra)
        }
    };

    let mut word = [0u8; 8];
    word[0] = opcode;
    word[1] = ty_id;
    word[2..4].copy_from_slice(&dst.to_le_bytes());
    word[4..6].copy_from_slice(&src1.to_le_bytes());
    word[6..8].copy_from_slice(&src2.to_le_bytes());
    Ok(word)
}

pub fn unpack_instruction(
    word: [u8; 8],
    const_pool: &ConstantPool,
    type_pool: &TypePool,
    extra_pool: &ExtraArgsPool,
    line: usize,
) -> Result<Instruction, BinaryError> {
    let opcode = word[0];
    let ty_id = word[1];
    let dst_raw = u16::from_le_bytes([word[2], word[3]]);
    let src1 = u16::from_le_bytes([word[4], word[5]]);
    let src2 = u16::from_le_bytes([word[6], word[7]]);

    let ty = type_pool.get(ty_id)?;
    let dst = if dst_raw == NO_REG {
        None
    } else {
        Some(dst_raw as RegId)
    };

    let op = match opcode {
        OP_CONST_I64 => {
            let v = *const_pool
                .ints
                .get(src1 as usize)
                .ok_or(BinaryError::PoolIndexOutOfBounds {
                    pool: "int_pool",
                    index: src1 as usize,
                    size: const_pool.ints.len(),
                })?;
            Op::ConstI64(v)
        }
        OP_CONST_F64 => {
            let v = *const_pool
                .floats
                .get(src1 as usize)
                .ok_or(BinaryError::PoolIndexOutOfBounds {
                    pool: "float_pool",
                    index: src1 as usize,
                    size: const_pool.floats.len(),
                })?;
            Op::ConstF64(v)
        }
        OP_CONST_BOOL => Op::ConstBool(src1 != 0),
        OP_CONST_STR => {
            let s = const_pool
                .strings
                .get(src1 as usize)
                .ok_or(BinaryError::PoolIndexOutOfBounds {
                    pool: "str_pool",
                    index: src1 as usize,
                    size: const_pool.strings.len(),
                })?;
            Op::ConstStr(s.clone())
        }
        OP_ADD => Op::Add(src1 as RegId, src2 as RegId),
        OP_SUB => Op::Sub(src1 as RegId, src2 as RegId),
        OP_MUL => Op::Mul(src1 as RegId, src2 as RegId),
        OP_DIV => Op::Div(src1 as RegId, src2 as RegId),
        OP_CMP_EQ => Op::CmpEq(src1 as RegId, src2 as RegId),
        OP_CMP_LT => Op::CmpLt(src1 as RegId, src2 as RegId),
        OP_BUF_ALLOC => Op::BufAlloc(src1 as RegId),
        OP_BUF_READ_U8 => Op::BufReadU8(src1 as RegId, src2 as RegId),
        OP_BUF_WRITE_U8 => {
            let extra = extra_pool.get(src2)?;
            Op::BufWriteU8(src1 as RegId, extra[0] as RegId, extra[1] as RegId)
        }
        OP_BUF_SLICE => {
            let extra = extra_pool.get(src2)?;
            Op::BufSlice(src1 as RegId, extra[0] as RegId, extra[1] as RegId)
        }
        OP_VEC_NEW => Op::VecNew(type_pool.get(src1 as u8)?),
        OP_VEC_PUSH => Op::VecPush(src1 as RegId, src2 as RegId),
        OP_VEC_GET => Op::VecGet(src1 as RegId, src2 as RegId),
        OP_VEC_LEN => Op::VecLen(src1 as RegId),
        OP_MAP_NEW => Op::MapNew(type_pool.get(src1 as u8)?, type_pool.get(src2 as u8)?),
        OP_MAP_SET => {
            let extra = extra_pool.get(src2)?;
            Op::MapSet(src1 as RegId, extra[0] as RegId, extra[1] as RegId)
        }
        OP_MAP_GET => Op::MapGet(src1 as RegId, src2 as RegId),
        OP_MAP_DEL => Op::MapDel(src1 as RegId, src2 as RegId),
        OP_CHAN_NEW => Op::ChanNew(type_pool.get(src1 as u8)?, src2 as RegId),
        OP_CHAN_SEND => Op::ChanSend(src1 as RegId, src2 as RegId),
        OP_CHAN_RECV => Op::ChanRecv(src1 as RegId),
        OP_SPAWN => {
            let fn_name = const_pool
                .strings
                .get(src1 as usize)
                .ok_or(BinaryError::PoolIndexOutOfBounds {
                    pool: "str_pool",
                    index: src1 as usize,
                    size: const_pool.strings.len(),
                })?;
            Op::Spawn(fn_name.clone(), src2 as RegId)
        }
        OP_SYSCALL => {
            let op_name = const_pool
                .strings
                .get(src1 as usize)
                .ok_or(BinaryError::PoolIndexOutOfBounds {
                    pool: "str_pool",
                    index: src1 as usize,
                    size: const_pool.strings.len(),
                })?;
            let extra = extra_pool.get(src2)?;
            let effect = match extra[0] {
                0 => Effect::Net,
                1 => Effect::Ui,
                2 => Effect::Hw,
                3 => Effect::Async,
                other => return Err(BinaryError::InvalidOpcode(other as u8)),
            };
            Op::Syscall(effect, op_name.clone(), extra[1] as RegId)
        }
        OP_FREE => Op::Free(src1 as RegId),
        OP_STRUCT_NEW => {
            let name = const_pool
                .strings
                .get(src1 as usize)
                .ok_or(BinaryError::PoolIndexOutOfBounds {
                    pool: "str_pool",
                    index: src1 as usize,
                    size: const_pool.strings.len(),
                })?;
            Op::StructNew(name.clone())
        }
        OP_STRUCT_GET => Op::StructGet(src1 as RegId, src2 as u32),
        OP_STRUCT_SET => {
            let extra = extra_pool.get(src2)?;
            Op::StructSet(src1 as RegId, extra[0] as u32, extra[1] as RegId)
        }
        other => return Err(BinaryError::InvalidOpcode(other)),
    };

    Ok(Instruction {
        dst,
        ty,
        op,
        line,
    })
}

pub fn pack_terminator(
    term: &Terminator,
    extra_pool: &mut ExtraArgsPool,
) -> Result<[u8; 8], BinaryError> {
    let mut word = [0u8; 8];
    match term {
        Terminator::Ret(reg) => {
            word[0] = OP_RET;
            word[1] = 0;
            word[2..4].copy_from_slice(&NO_REG.to_le_bytes());
            let src1 = match reg {
                Some(r) => reg_to_u16(*r)?,
                None => NO_REG,
            };
            word[4..6].copy_from_slice(&src1.to_le_bytes());
            word[6..8].copy_from_slice(&NO_REG.to_le_bytes());
        }
        Terminator::Br(target) => {
            word[0] = OP_BR;
            word[1] = 0;
            word[2..4].copy_from_slice(&NO_REG.to_le_bytes());
            let src1 = reg_to_u16(*target)?;
            word[4..6].copy_from_slice(&src1.to_le_bytes());
            word[6..8].copy_from_slice(&NO_REG.to_le_bytes());
        }
        Terminator::BrIf {
            cond,
            then_blk,
            else_blk,
        } => {
            word[0] = OP_BR_IF;
            word[1] = 0;
            word[2..4].copy_from_slice(&NO_REG.to_le_bytes());
            let extra = extra_pool.insert(reg_to_u16(*then_blk)?, reg_to_u16(*else_blk)?)?;
            let src1 = reg_to_u16(*cond)?;
            word[4..6].copy_from_slice(&src1.to_le_bytes());
            word[6..8].copy_from_slice(&extra.to_le_bytes());
        }
    }
    Ok(word)
}

pub fn unpack_terminator(
    word: [u8; 8],
    extra_pool: &ExtraArgsPool,
) -> Result<Terminator, BinaryError> {
    let opcode = word[0];
    let src1 = u16::from_le_bytes([word[4], word[5]]);
    let src2 = u16::from_le_bytes([word[6], word[7]]);

    match opcode {
        OP_RET => {
            let reg = if src1 == NO_REG {
                None
            } else {
                Some(src1 as RegId)
            };
            Ok(Terminator::Ret(reg))
        }
        OP_BR => Ok(Terminator::Br(src1 as BlockId)),
        OP_BR_IF => {
            let extra = extra_pool.get(src2)?;
            Ok(Terminator::BrIf {
                cond: src1 as RegId,
                then_blk: extra[0] as BlockId,
                else_blk: extra[1] as BlockId,
            })
        }
        other => Err(BinaryError::InvalidOpcode(other)),
    }
}
