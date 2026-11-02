use crate::ast::Type;
use crate::error::BinaryError;
use std::collections::HashMap;
use std::io::{Read, Write};

#[derive(Debug, Default, Clone, PartialEq)]
pub struct ConstantPool {
    pub strings: Vec<String>,
    pub floats: Vec<f64>,
    pub ints: Vec<i64>,
    str_map: HashMap<String, u16>,
    float_map: HashMap<u64, u16>,
    int_map: HashMap<i64, u16>,
}

impl ConstantPool {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert_str(&mut self, s: &str) -> Result<u16, BinaryError> {
        if let Some(&idx) = self.str_map.get(s) {
            return Ok(idx);
        }
        let idx = u16::try_from(self.strings.len())
            .map_err(|_| BinaryError::RegisterOverflow(self.strings.len() as u32))?;
        self.strings.push(s.to_string());
        self.str_map.insert(s.to_string(), idx);
        Ok(idx)
    }

    pub fn insert_float(&mut self, f: f64) -> Result<u16, BinaryError> {
        let bits = f.to_bits();
        if let Some(&idx) = self.float_map.get(&bits) {
            return Ok(idx);
        }
        let idx = u16::try_from(self.floats.len())
            .map_err(|_| BinaryError::RegisterOverflow(self.floats.len() as u32))?;
        self.floats.push(f);
        self.float_map.insert(bits, idx);
        Ok(idx)
    }

    pub fn insert_int(&mut self, i: i64) -> Result<u16, BinaryError> {
        if let Some(&idx) = self.int_map.get(&i) {
            return Ok(idx);
        }
        let idx = u16::try_from(self.ints.len())
            .map_err(|_| BinaryError::RegisterOverflow(self.ints.len() as u32))?;
        self.ints.push(i);
        self.int_map.insert(i, idx);
        Ok(idx)
    }

    pub fn encode(&self, w: &mut impl Write) -> Result<(), BinaryError> {
        w.write_all(&(self.strings.len() as u32).to_le_bytes())
            .map_err(|e| BinaryError::IoError(e.to_string()))?;
        for s in &self.strings {
            let bytes = s.as_bytes();
            w.write_all(&(bytes.len() as u32).to_le_bytes())
                .map_err(|e| BinaryError::IoError(e.to_string()))?;
            w.write_all(bytes)
                .map_err(|e| BinaryError::IoError(e.to_string()))?;
        }

        w.write_all(&(self.floats.len() as u32).to_le_bytes())
            .map_err(|e| BinaryError::IoError(e.to_string()))?;
        for f in &self.floats {
            w.write_all(&f.to_bits().to_le_bytes())
                .map_err(|e| BinaryError::IoError(e.to_string()))?;
        }

        w.write_all(&(self.ints.len() as u32).to_le_bytes())
            .map_err(|e| BinaryError::IoError(e.to_string()))?;
        for i in &self.ints {
            w.write_all(&i.to_le_bytes())
                .map_err(|e| BinaryError::IoError(e.to_string()))?;
        }
        Ok(())
    }

    pub fn decode(cursor: &mut &[u8]) -> Result<Self, BinaryError> {
        let mut pool = ConstantPool::new();

        let mut buf4 = [0u8; 4];
        cursor
            .read_exact(&mut buf4)
            .map_err(|_| BinaryError::UnexpectedEof {
                expected: 4,
                actual: 0,
            })?;
        let str_count = u32::from_le_bytes(buf4) as usize;
        for _ in 0..str_count {
            cursor
                .read_exact(&mut buf4)
                .map_err(|_| BinaryError::UnexpectedEof {
                    expected: 4,
                    actual: 0,
                })?;
            let len = u32::from_le_bytes(buf4) as usize;
            if len > cursor.len() {
                return Err(BinaryError::UnexpectedEof {
                    expected: len,
                    actual: cursor.len(),
                });
            }
            let mut str_bytes = vec![0u8; len];
            cursor
                .read_exact(&mut str_bytes)
                .map_err(|_| BinaryError::UnexpectedEof {
                    expected: len,
                    actual: 0,
                })?;
            let s = String::from_utf8(str_bytes)?;
            pool.insert_str(&s)?;
        }

        cursor
            .read_exact(&mut buf4)
            .map_err(|_| BinaryError::UnexpectedEof {
                expected: 4,
                actual: 0,
            })?;
        let float_count = u32::from_le_bytes(buf4) as usize;
        let mut buf8 = [0u8; 8];
        for _ in 0..float_count {
            cursor
                .read_exact(&mut buf8)
                .map_err(|_| BinaryError::UnexpectedEof {
                    expected: 8,
                    actual: 0,
                })?;
            let bits = u64::from_le_bytes(buf8);
            pool.insert_float(f64::from_bits(bits))?;
        }

        cursor
            .read_exact(&mut buf4)
            .map_err(|_| BinaryError::UnexpectedEof {
                expected: 4,
                actual: 0,
            })?;
        let int_count = u32::from_le_bytes(buf4) as usize;
        for _ in 0..int_count {
            cursor
                .read_exact(&mut buf8)
                .map_err(|_| BinaryError::UnexpectedEof {
                    expected: 8,
                    actual: 0,
                })?;
            let val = i64::from_le_bytes(buf8);
            pool.insert_int(val)?;
        }

        Ok(pool)
    }
}

#[derive(Debug, Default, Clone, PartialEq)]
pub struct TypePool {
    pub types: Vec<Type>,
    type_map: HashMap<Type, u8>,
}

impl TypePool {
    pub const TYPE_VOID: u8 = 0;
    pub const TYPE_I64: u8 = 1;
    pub const TYPE_F64: u8 = 2;
    pub const TYPE_BOOL: u8 = 3;
    pub const TYPE_STR: u8 = 4;
    pub const TYPE_BUF: u8 = 5;

    pub fn new() -> Self {
        let mut pool = Self::default();
        pool.types.push(Type::Void);
        pool.types.push(Type::I64);
        pool.types.push(Type::F64);
        pool.types.push(Type::Bool);
        pool.types.push(Type::Str);
        pool.types.push(Type::Buf);
        pool.type_map.insert(Type::Void, Self::TYPE_VOID);
        pool.type_map.insert(Type::I64, Self::TYPE_I64);
        pool.type_map.insert(Type::F64, Self::TYPE_F64);
        pool.type_map.insert(Type::Bool, Self::TYPE_BOOL);
        pool.type_map.insert(Type::Str, Self::TYPE_STR);
        pool.type_map.insert(Type::Buf, Self::TYPE_BUF);
        pool
    }

    pub fn insert(&mut self, ty: &Type) -> Result<u8, BinaryError> {
        if let Some(&id) = self.type_map.get(ty) {
            return Ok(id);
        }
        match ty {
            Type::Void => Ok(Self::TYPE_VOID),
            Type::I64 => Ok(Self::TYPE_I64),
            Type::F64 => Ok(Self::TYPE_F64),
            Type::Bool => Ok(Self::TYPE_BOOL),
            Type::Str => Ok(Self::TYPE_STR),
            Type::Buf => Ok(Self::TYPE_BUF),
            Type::Vec(inner) => {
                let _ = self.insert(inner)?;
                let id = u8::try_from(self.types.len())
                    .map_err(|_| BinaryError::RegisterOverflow(self.types.len() as u32))?;
                self.types.push(ty.clone());
                self.type_map.insert(ty.clone(), id);
                Ok(id)
            }
            Type::Map(k, v) => {
                let _ = self.insert(k)?;
                let _ = self.insert(v)?;
                let id = u8::try_from(self.types.len())
                    .map_err(|_| BinaryError::RegisterOverflow(self.types.len() as u32))?;
                self.types.push(ty.clone());
                self.type_map.insert(ty.clone(), id);
                Ok(id)
            }
            Type::Chan(inner) => {
                let _ = self.insert(inner)?;
                let id = u8::try_from(self.types.len())
                    .map_err(|_| BinaryError::RegisterOverflow(self.types.len() as u32))?;
                self.types.push(ty.clone());
                self.type_map.insert(ty.clone(), id);
                Ok(id)
            }
            Type::Struct(_) => {
                let id = u8::try_from(self.types.len())
                    .map_err(|_| BinaryError::RegisterOverflow(self.types.len() as u32))?;
                self.types.push(ty.clone());
                self.type_map.insert(ty.clone(), id);
                Ok(id)
            }
        }
    }

    pub fn get(&self, id: u8) -> Result<Type, BinaryError> {
        self.types
            .get(id as usize)
            .cloned()
            .ok_or(BinaryError::PoolIndexOutOfBounds {
                pool: "TypePool",
                index: id as usize,
                size: self.types.len(),
            })
    }

    pub fn encode(&self, w: &mut impl Write) -> Result<(), BinaryError> {
        let param_count = self.types.len().saturating_sub(6) as u16;
        w.write_all(&param_count.to_le_bytes())
            .map_err(|e| BinaryError::IoError(e.to_string()))?;
        for ty in &self.types[6..] {
            match ty {
                Type::Vec(elem) => {
                    w.write_all(&[0x10, self.type_map[elem]])
                        .map_err(|e| BinaryError::IoError(e.to_string()))?;
                }
                Type::Map(k, v) => {
                    w.write_all(&[0x11, self.type_map[k], self.type_map[v]])
                        .map_err(|e| BinaryError::IoError(e.to_string()))?;
                }
                Type::Chan(elem) => {
                    w.write_all(&[0x12, self.type_map[elem]])
                        .map_err(|e| BinaryError::IoError(e.to_string()))?;
                }
                Type::Struct(name) => {
                    let bytes = name.as_bytes();
                    let len = u16::try_from(bytes.len())
                        .map_err(|_| BinaryError::IoError("Struct name too long".to_string()))?;
                    w.write_all(&[0x13])
                        .map_err(|e| BinaryError::IoError(e.to_string()))?;
                    w.write_all(&len.to_le_bytes())
                        .map_err(|e| BinaryError::IoError(e.to_string()))?;
                    w.write_all(bytes)
                        .map_err(|e| BinaryError::IoError(e.to_string()))?;
                }
                _ => unreachable!(),
            }
        }
        Ok(())
    }

    pub fn decode(r: &mut impl Read) -> Result<Self, BinaryError> {
        let mut pool = TypePool::new();
        let mut buf2 = [0u8; 2];
        r.read_exact(&mut buf2)
            .map_err(|_| BinaryError::UnexpectedEof {
                expected: 2,
                actual: 0,
            })?;
        let count = u16::from_le_bytes(buf2) as usize;
        for _ in 0..count {
            let mut tag = [0u8; 1];
            r.read_exact(&mut tag)
                .map_err(|_| BinaryError::UnexpectedEof {
                    expected: 1,
                    actual: 0,
                })?;
            match tag[0] {
                0x10 => {
                    let mut elem = [0u8; 1];
                    r.read_exact(&mut elem)
                        .map_err(|_| BinaryError::UnexpectedEof {
                            expected: 1,
                            actual: 0,
                        })?;
                    let elem_ty = pool.get(elem[0])?;
                    let ty = Type::Vec(Box::new(elem_ty));
                    pool.insert(&ty)?;
                }
                0x11 => {
                    let mut kv = [0u8; 2];
                    r.read_exact(&mut kv)
                        .map_err(|_| BinaryError::UnexpectedEof {
                            expected: 2,
                            actual: 0,
                        })?;
                    let k_ty = pool.get(kv[0])?;
                    let v_ty = pool.get(kv[1])?;
                    let ty = Type::Map(Box::new(k_ty), Box::new(v_ty));
                    pool.insert(&ty)?;
                }
                0x12 => {
                    let mut elem = [0u8; 1];
                    r.read_exact(&mut elem)
                        .map_err(|_| BinaryError::UnexpectedEof {
                            expected: 1,
                            actual: 0,
                        })?;
                    let elem_ty = pool.get(elem[0])?;
                    let ty = Type::Chan(Box::new(elem_ty));
                    pool.insert(&ty)?;
                }
                0x13 => {
                    let mut len_buf = [0u8; 2];
                    r.read_exact(&mut len_buf)
                        .map_err(|_| BinaryError::UnexpectedEof {
                            expected: 2,
                            actual: 0,
                        })?;
                    let len = u16::from_le_bytes(len_buf) as usize;
                    let mut name_bytes = vec![0u8; len];
                    r.read_exact(&mut name_bytes)
                        .map_err(|_| BinaryError::UnexpectedEof {
                            expected: len,
                            actual: 0,
                        })?;
                    let name = String::from_utf8(name_bytes)?;
                    if name.is_empty() {
                        return Err(BinaryError::IoError("Empty struct name in type pool".to_string()));
                    }
                    let ty = Type::Struct(name);
                    pool.insert(&ty)?;
                }
                other => return Err(BinaryError::InvalidTypeTag(other)),
            }
        }
        Ok(pool)
    }
}
