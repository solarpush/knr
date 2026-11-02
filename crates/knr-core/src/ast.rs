use serde::{Deserialize, Serialize};
use std::fmt;

pub type RegId = u32;
pub type BlockId = u32;

/// Scalar (Copy) and Linear (Move) types in knr SSA.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Type {
    I64,
    F64,
    Bool,
    Str,
    Buf,
    Vec(Box<Type>),
    Map(Box<Type>, Box<Type>),
    Chan(Box<Type>),
    Struct(String),
    Void,
}

impl Type {
    /// Returns true if this type has linear ownership semantics (Move-only).
    #[inline]
    pub fn is_linear(&self) -> bool {
        !matches!(self, Type::I64 | Type::F64 | Type::Bool | Type::Void)
    }
}

impl fmt::Display for Type {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Type::I64 => write!(f, "i64"),
            Type::F64 => write!(f, "f64"),
            Type::Bool => write!(f, "bool"),
            Type::Str => write!(f, "str"),
            Type::Buf => write!(f, "buf"),
            Type::Vec(inner) => write!(f, "vec[{inner}]"),
            Type::Map(k, v) => write!(f, "map[{k}, {v}]"),
            Type::Chan(inner) => write!(f, "chan[{inner}]"),
            Type::Struct(name) => write!(f, "struct[{name}]"),
            Type::Void => write!(f, "void"),
        }
    }
}

/// Host platform capability effects.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Effect {
    Net,
    Ui,
    Hw,
    Async,
}

impl fmt::Display for Effect {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Effect::Net => write!(f, "Net"),
            Effect::Ui => write!(f, "Ui"),
            Effect::Hw => write!(f, "Hw"),
            Effect::Async => write!(f, "Async"),
        }
    }
}

/// A field declaration within a struct schema.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct FieldDef {
    pub index: u32,
    pub name: Option<String>,
    pub ty: Type,
}

impl FieldDef {
    pub fn new(index: u32, ty: Type) -> Self {
        Self {
            index,
            name: None,
            ty,
        }
    }

    pub fn with_name(index: u32, name: impl Into<String>, ty: Type) -> Self {
        Self {
            index,
            name: Some(name.into()),
            ty,
        }
    }
}

/// A structured data type schema definition.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Schema {
    pub name: String,
    pub fields: Vec<FieldDef>,
}

impl Schema {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            fields: Vec::new(),
        }
    }

    pub fn add_field(&mut self, index: u32, name: Option<String>, ty: Type) {
        self.fields.push(FieldDef { index, name, ty });
    }

    pub fn get_field(&self, index: u32) -> Option<&FieldDef> {
        self.fields.iter().find(|f| f.index == index)
    }

    pub fn get_field_by_name(&self, name: &str) -> Option<&FieldDef> {
        self.fields.iter().find(|f| f.name.as_deref() == Some(name))
    }
}

/// All SSA Opcodes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Op {
    ConstI64(i64),
    ConstF64(f64),
    ConstBool(bool),
    ConstStr(String),
    Add(RegId, RegId),
    Sub(RegId, RegId),
    Mul(RegId, RegId),
    Div(RegId, RegId),
    CmpEq(RegId, RegId),
    CmpLt(RegId, RegId),
    BufAlloc(RegId),
    BufReadU8(RegId, RegId),
    BufWriteU8(RegId, RegId, RegId),
    BufSlice(RegId, RegId, RegId),
    VecNew(Type),
    VecPush(RegId, RegId),
    VecGet(RegId, RegId),
    VecLen(RegId),
    MapNew(Type, Type),
    MapSet(RegId, RegId, RegId),
    MapGet(RegId, RegId),
    MapDel(RegId, RegId),
    ChanNew(Type, RegId),
    ChanSend(RegId, RegId),
    ChanRecv(RegId),
    Spawn(String, RegId),
    Syscall(Effect, String, RegId),
    Free(RegId),
    StructNew(String),
    StructGet(RegId, u32),
    StructSet(RegId, u32, RegId),
}

/// A single instruction within a basic block.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Instruction {
    pub dst: Option<RegId>,
    pub ty: Type,
    pub op: Op,
    pub line: usize,
}

impl Instruction {
    pub fn new(dst: Option<RegId>, ty: Type, op: Op, line: usize) -> Self {
        Self { dst, ty, op, line }
    }
}

/// Control-flow terminator ending each basic block.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Terminator {
    Ret(Option<RegId>),
    Br(BlockId),
    BrIf {
        cond: RegId,
        then_blk: BlockId,
        else_blk: BlockId,
    },
}

/// An SSA Basic Block comprising instructions followed by a single terminator.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BasicBlock {
    pub id: BlockId,
    pub instructions: Vec<Instruction>,
    pub terminator: Terminator,
}

impl BasicBlock {
    pub fn new(id: BlockId, terminator: Terminator) -> Self {
        Self {
            id,
            instructions: Vec::new(),
            terminator,
        }
    }

    pub fn push(&mut self, inst: Instruction) {
        self.instructions.push(inst);
    }
}

/// A complete function definition.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Function {
    pub name: String,
    pub params: Vec<(RegId, Type)>,
    pub ret_ty: Type,
    pub effects: Vec<Effect>,
    pub blocks: Vec<BasicBlock>,
}

impl Function {
    pub fn new(name: impl Into<String>, ret_ty: Type) -> Self {
        Self {
            name: name.into(),
            params: Vec::new(),
            ret_ty,
            effects: Vec::new(),
            blocks: Vec::new(),
        }
    }

    pub fn add_param(&mut self, reg: RegId, ty: Type) {
        self.params.push((reg, ty));
    }

    pub fn add_effect(&mut self, effect: Effect) {
        if !self.effects.contains(&effect) {
            self.effects.push(effect);
        }
    }

    pub fn add_block(&mut self, block: BasicBlock) {
        self.blocks.push(block);
    }
}

/// A collection of schemas and functions representing an SSA compilation unit.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Module {
    pub schemas: Vec<Schema>,
    pub functions: Vec<Function>,
}

impl Module {
    pub fn new() -> Self {
        Self {
            schemas: Vec::new(),
            functions: Vec::new(),
        }
    }

    pub fn add_schema(&mut self, schema: Schema) {
        self.schemas.push(schema);
    }

    pub fn get_schema(&self, name: &str) -> Option<&Schema> {
        self.schemas.iter().find(|s| s.name == name)
    }

    pub fn add_function(&mut self, func: Function) {
        self.functions.push(func);
    }

    pub fn get_function(&self, name: &str) -> Option<&Function> {
        self.functions.iter().find(|f| f.name == name)
    }
}
