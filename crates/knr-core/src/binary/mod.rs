pub mod const_pool;
pub mod header;
pub mod instruction;

use crate::ast::{BasicBlock, BlockId, Effect, FieldDef, Function, Module, RegId, Schema};
use crate::error::BinaryError;
use const_pool::{ConstantPool, TypePool};
use header::{read_header, write_header};
use instruction::{
    pack_instruction, pack_terminator, unpack_instruction, unpack_terminator, ExtraArgsPool,
};
use std::io::Read;

pub fn encode_knrbin(module: &Module) -> Result<Vec<u8>, BinaryError> {
    let mut const_pool = ConstantPool::new();
    let mut type_pool = TypePool::new();
    let mut extra_pool = ExtraArgsPool::new();

    // First pass: pack all instructions and terminators while populating pools
    struct PrePackedBlock {
        id: BlockId,
        instructions: Vec<[u8; 8]>,
        terminator: [u8; 8],
    }

    struct PrePackedFunc {
        name: String,
        ret_ty_id: u8,
        effects_mask: u8,
        params: Vec<(u16, u8)>,
        blocks: Vec<PrePackedBlock>,
    }

    let mut pre_packed_funcs = Vec::with_capacity(module.functions.len());

    for func in &module.functions {
        let ret_ty_id = type_pool.insert(&func.ret_ty)?;

        let mut effects_mask: u8 = 0;
        if func.effects.contains(&Effect::Net) {
            effects_mask |= 1 << 0;
        }
        if func.effects.contains(&Effect::Ui) {
            effects_mask |= 1 << 1;
        }
        if func.effects.contains(&Effect::Hw) {
            effects_mask |= 1 << 2;
        }
        if func.effects.contains(&Effect::Async) {
            effects_mask |= 1 << 3;
        }

        let mut params = Vec::with_capacity(func.params.len());
        for (reg, ty) in &func.params {
            let reg_u16 = u16::try_from(*reg)
                .map_err(|_| BinaryError::RegisterOverflow(*reg))?;
            let ty_id = type_pool.insert(ty)?;
            params.push((reg_u16, ty_id));
        }

        let mut blocks = Vec::with_capacity(func.blocks.len());
        for block in &func.blocks {
            let mut inst_words = Vec::with_capacity(block.instructions.len());
            for inst in &block.instructions {
                let word = pack_instruction(inst, &mut const_pool, &mut type_pool, &mut extra_pool)?;
                inst_words.push(word);
            }
            let term_word = pack_terminator(&block.terminator, &mut extra_pool)?;
            blocks.push(PrePackedBlock {
                id: block.id,
                instructions: inst_words,
                terminator: term_word,
            });
        }

        // Insert func name in constant pool
        const_pool.insert_str(&func.name)?;

        pre_packed_funcs.push(PrePackedFunc {
            name: func.name.clone(),
            ret_ty_id,
            effects_mask,
            params,
            blocks,
        });
    }

    // Pre-insert schemas into pools
    for schema in &module.schemas {
        const_pool.insert_str(&schema.name)?;
        for field in &schema.fields {
            type_pool.insert(&field.ty)?;
            if let Some(ref name) = field.name {
                const_pool.insert_str(name)?;
            }
        }
    }

    // Now serialize to byte buffer
    let mut out = Vec::new();
    write_header(&mut out)?;
    const_pool.encode(&mut out)?;
    type_pool.encode(&mut out)?;
    extra_pool.encode(&mut out)?;

    // Serialize schemas
    let schema_count = module.schemas.len() as u32;
    out.extend_from_slice(&schema_count.to_le_bytes());
    for schema in &module.schemas {
        let name_idx = const_pool.insert_str(&schema.name)?;
        out.extend_from_slice(&(name_idx as u32).to_le_bytes());
        let field_count = schema.fields.len() as u32;
        out.extend_from_slice(&field_count.to_le_bytes());
        for field in &schema.fields {
            out.extend_from_slice(&field.index.to_le_bytes());
            let ty_id = type_pool.insert(&field.ty)?;
            out.push(ty_id);
            if let Some(ref name) = field.name {
                let fname_idx = const_pool.insert_str(name)?;
                out.extend_from_slice(&(fname_idx as u32).to_le_bytes());
            } else {
                out.extend_from_slice(&u32::MAX.to_le_bytes());
            }
        }
    }

    // Serialize functions
    let fn_count = pre_packed_funcs.len() as u32;
    out.extend_from_slice(&fn_count.to_le_bytes());

    for func in pre_packed_funcs {
        let name_str_idx = const_pool.insert_str(&func.name)?;
        out.extend_from_slice(&(name_str_idx as u32).to_le_bytes());
        out.push(func.ret_ty_id);
        out.push(func.effects_mask);

        let param_count = func.params.len() as u16;
        out.extend_from_slice(&param_count.to_le_bytes());
        for (reg, ty_id) in func.params {
            out.extend_from_slice(&reg.to_le_bytes());
            out.push(ty_id);
        }

        let block_count = func.blocks.len() as u32;
        out.extend_from_slice(&block_count.to_le_bytes());
        for block in func.blocks {
            out.extend_from_slice(&block.id.to_le_bytes());
            let inst_count = block.instructions.len() as u32;
            out.extend_from_slice(&inst_count.to_le_bytes());
            for word in block.instructions {
                out.extend_from_slice(&word);
            }
            out.extend_from_slice(&block.terminator);
        }
    }

    Ok(out)
}

pub fn decode_knrbin(bytes: &[u8]) -> Result<Module, BinaryError> {
    let mut cursor = bytes;
    let _version = read_header(&mut cursor)?;
    let const_pool = ConstantPool::decode(&mut cursor)?;
    let type_pool = TypePool::decode(&mut cursor)?;
    let extra_pool = ExtraArgsPool::decode(&mut cursor)?;

    let mut buf4 = [0u8; 4];
    cursor
        .read_exact(&mut buf4)
        .map_err(|_| BinaryError::UnexpectedEof {
            expected: 4,
            actual: 0,
        })?;
    let schema_count = u32::from_le_bytes(buf4) as usize;
    if schema_count > cursor.len() / 8 {
        return Err(BinaryError::UnexpectedEof {
            expected: schema_count * 8,
            actual: cursor.len(),
        });
    }

    let mut schemas = Vec::with_capacity(schema_count.min(cursor.len() / 8));
    for _ in 0..schema_count {
        cursor
            .read_exact(&mut buf4)
            .map_err(|_| BinaryError::UnexpectedEof {
                expected: 4,
                actual: 0,
            })?;
        let name_idx = u32::from_le_bytes(buf4) as usize;
        let name = const_pool
            .strings
            .get(name_idx)
            .ok_or(BinaryError::PoolIndexOutOfBounds {
                pool: "strings",
                index: name_idx,
                size: const_pool.strings.len(),
            })?
            .clone();

        cursor
            .read_exact(&mut buf4)
            .map_err(|_| BinaryError::UnexpectedEof {
                expected: 4,
                actual: 0,
            })?;
        let field_count = u32::from_le_bytes(buf4) as usize;
        if field_count > cursor.len() / 9 {
            return Err(BinaryError::UnexpectedEof {
                expected: field_count * 9,
                actual: cursor.len(),
            });
        }
        let mut fields = Vec::with_capacity(field_count.min(cursor.len() / 9));

        for _ in 0..field_count {
            cursor
                .read_exact(&mut buf4)
                .map_err(|_| BinaryError::UnexpectedEof {
                    expected: 4,
                    actual: 0,
                })?;
            let f_idx = u32::from_le_bytes(buf4);

            let mut ty_buf = [0u8; 1];
            cursor
                .read_exact(&mut ty_buf)
                .map_err(|_| BinaryError::UnexpectedEof {
                    expected: 1,
                    actual: 0,
                })?;
            let f_ty = type_pool.get(ty_buf[0])?;

            cursor
                .read_exact(&mut buf4)
                .map_err(|_| BinaryError::UnexpectedEof {
                    expected: 4,
                    actual: 0,
                })?;
            let fname_idx = u32::from_le_bytes(buf4);
            let fname = if fname_idx != u32::MAX {
                Some(
                    const_pool
                        .strings
                        .get(fname_idx as usize)
                        .ok_or(BinaryError::PoolIndexOutOfBounds {
                            pool: "strings",
                            index: fname_idx as usize,
                            size: const_pool.strings.len(),
                        })?
                        .clone(),
                )
            } else {
                None
            };

            fields.push(FieldDef {
                index: f_idx,
                name: fname,
                ty: f_ty,
            });
        }

        schemas.push(Schema { name, fields });
    }

    cursor
        .read_exact(&mut buf4)
        .map_err(|_| BinaryError::UnexpectedEof {
            expected: 4,
            actual: 0,
        })?;
    let fn_count = u32::from_le_bytes(buf4) as usize;

    let mut functions = Vec::with_capacity(fn_count.min(cursor.len() / 12));

    let mut line_counter = 1;

    for _ in 0..fn_count {
        cursor
            .read_exact(&mut buf4)
            .map_err(|_| BinaryError::UnexpectedEof {
                expected: 4,
                actual: 0,
            })?;
        let name_idx = u32::from_le_bytes(buf4) as usize;
        let name = const_pool
            .strings
            .get(name_idx)
            .ok_or(BinaryError::PoolIndexOutOfBounds {
                pool: "strings",
                index: name_idx,
                size: const_pool.strings.len(),
            })?
            .clone();

        let mut buf2_flags = [0u8; 2];
        cursor
            .read_exact(&mut buf2_flags)
            .map_err(|_| BinaryError::UnexpectedEof {
                expected: 2,
                actual: 0,
            })?;
        let ret_ty_id = buf2_flags[0];
        let effects_mask = buf2_flags[1];

        let ret_ty = type_pool.get(ret_ty_id)?;

        let mut effects = Vec::new();
        if (effects_mask & (1 << 0)) != 0 {
            effects.push(Effect::Net);
        }
        if (effects_mask & (1 << 1)) != 0 {
            effects.push(Effect::Ui);
        }
        if (effects_mask & (1 << 2)) != 0 {
            effects.push(Effect::Hw);
        }
        if (effects_mask & (1 << 3)) != 0 {
            effects.push(Effect::Async);
        }

        let mut buf2 = [0u8; 2];
        cursor
            .read_exact(&mut buf2)
            .map_err(|_| BinaryError::UnexpectedEof {
                expected: 2,
                actual: 0,
            })?;
        let param_count = u16::from_le_bytes(buf2) as usize;
        let mut params = Vec::with_capacity(param_count.min(cursor.len() / 3));

        for _ in 0..param_count {
            cursor
                .read_exact(&mut buf2)
                .map_err(|_| BinaryError::UnexpectedEof {
                    expected: 2,
                    actual: 0,
                })?;
            let reg = u16::from_le_bytes(buf2) as RegId;
            let mut ty_buf = [0u8; 1];
            cursor
                .read_exact(&mut ty_buf)
                .map_err(|_| BinaryError::UnexpectedEof {
                    expected: 1,
                    actual: 0,
                })?;
            let param_ty = type_pool.get(ty_buf[0])?;
            params.push((reg, param_ty));
        }

        cursor
            .read_exact(&mut buf4)
            .map_err(|_| BinaryError::UnexpectedEof {
                expected: 4,
                actual: 0,
            })?;
        let block_count = u32::from_le_bytes(buf4) as usize;
        let mut blocks = Vec::with_capacity(block_count.min(cursor.len() / 16));

        line_counter += 1; // fn header

        for _ in 0..block_count {
            cursor
                .read_exact(&mut buf4)
                .map_err(|_| BinaryError::UnexpectedEof {
                    expected: 4,
                    actual: 0,
                })?;
            let block_id = u32::from_le_bytes(buf4) as BlockId;

            cursor
                .read_exact(&mut buf4)
                .map_err(|_| BinaryError::UnexpectedEof {
                    expected: 4,
                    actual: 0,
                })?;
            let inst_count = u32::from_le_bytes(buf4) as usize;

            line_counter += 1; // block label line

            let mut instructions = Vec::with_capacity(inst_count.min(cursor.len() / 8));
            let mut word = [0u8; 8];

            for _ in 0..inst_count {
                cursor
                    .read_exact(&mut word)
                    .map_err(|_| BinaryError::UnexpectedEof {
                        expected: 8,
                        actual: 0,
                    })?;
                let inst = unpack_instruction(
                    word,
                    &const_pool,
                    &type_pool,
                    &extra_pool,
                    line_counter,
                )?;
                instructions.push(inst);
                line_counter += 1;
            }

            let mut term_word = [0u8; 8];
            cursor
                .read_exact(&mut term_word)
                .map_err(|_| BinaryError::UnexpectedEof {
                    expected: 8,
                    actual: 0,
                })?;
            let terminator = unpack_terminator(term_word, &extra_pool)?;
            line_counter += 1;

            blocks.push(BasicBlock {
                id: block_id,
                instructions,
                terminator,
            });
        }

        line_counter += 1; // closing brace

        functions.push(Function {
            name,
            params,
            ret_ty,
            effects,
            blocks,
        });
    }

    Ok(Module {
        imports: Vec::new(),
        schemas,
        functions,
    })
}
