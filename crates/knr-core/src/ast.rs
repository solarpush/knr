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
    Rem(RegId, RegId),
    CmpEq(RegId, RegId),
    CmpLt(RegId, RegId),
    IntToStr(RegId),
    StrLen(RegId),
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
    Call(String, Vec<RegId>),
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
    BrArgs {
        target: BlockId,
        args: Vec<RegId>,
    },
    BrIfArgs {
        cond: RegId,
        then_blk: BlockId,
        then_args: Vec<RegId>,
        else_blk: BlockId,
        else_args: Vec<RegId>,
    },
}

impl Terminator {
    /// Returns all outgoing (target_block, branch_args) edges for this terminator.
    pub fn targets(&self) -> Vec<(BlockId, &[RegId])> {
        match self {
            Terminator::Ret(_) => Vec::new(),
            Terminator::Br(t) => vec![(*t, &[])],
            Terminator::BrIf {
                then_blk,
                else_blk,
                ..
            } => vec![(*then_blk, &[]), (*else_blk, &[])],
            Terminator::BrArgs { target, args } => vec![(*target, args.as_slice())],
            Terminator::BrIfArgs {
                then_blk,
                then_args,
                else_blk,
                else_args,
                ..
            } => vec![
                (*then_blk, then_args.as_slice()),
                (*else_blk, else_args.as_slice()),
            ],
        }
    }

    /// Returns all registers read by this terminator (condition, return value, branch args).
    pub fn used_regs(&self) -> Vec<RegId> {
        match self {
            Terminator::Ret(Some(r)) => vec![*r],
            Terminator::Ret(None) | Terminator::Br(_) => Vec::new(),
            Terminator::BrIf { cond, .. } => vec![*cond],
            Terminator::BrArgs { args, .. } => args.clone(),
            Terminator::BrIfArgs {
                cond,
                then_args,
                else_args,
                ..
            } => {
                let mut regs = Vec::with_capacity(1 + then_args.len() + else_args.len());
                regs.push(*cond);
                regs.extend_from_slice(then_args);
                regs.extend_from_slice(else_args);
                regs
            }
        }
    }
}

/// An SSA Basic Block comprising optional block parameters, instructions, and a single terminator.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BasicBlock {
    pub id: BlockId,
    #[serde(default)]
    pub params: Vec<(RegId, Type)>,
    pub instructions: Vec<Instruction>,
    pub terminator: Terminator,
}

impl BasicBlock {
    pub fn new(id: BlockId, terminator: Terminator) -> Self {
        Self {
            id,
            params: Vec::new(),
            instructions: Vec::new(),
            terminator,
        }
    }

    pub fn with_params(id: BlockId, params: Vec<(RegId, Type)>, terminator: Terminator) -> Self {
        Self {
            id,
            params,
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
    /// Marks whether parameter `i` is a second-class borrow (`&T`), meaning it is read-only
    /// at call sites (not consumed by the caller) and cannot escape inside the callee.
    #[serde(default)]
    pub param_borrows: Vec<bool>,
    pub ret_ty: Type,
    pub effects: Vec<Effect>,
    pub blocks: Vec<BasicBlock>,
}

impl Function {
    pub fn new(name: impl Into<String>, ret_ty: Type) -> Self {
        Self {
            name: name.into(),
            params: Vec::new(),
            param_borrows: Vec::new(),
            ret_ty,
            effects: Vec::new(),
            blocks: Vec::new(),
        }
    }

    pub fn add_param(&mut self, reg: RegId, ty: Type) {
        self.params.push((reg, ty));
        if !self.param_borrows.is_empty() {
            self.param_borrows.push(false);
        }
    }

    pub fn add_borrowed_param(&mut self, reg: RegId, ty: Type) {
        while self.param_borrows.len() < self.params.len() {
            self.param_borrows.push(false);
        }
        self.params.push((reg, ty));
        self.param_borrows.push(true);
    }

    #[inline]
    pub fn is_param_borrowed(&self, idx: usize) -> bool {
        self.param_borrows.get(idx).copied().unwrap_or(false)
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
    pub imports: Vec<String>,
    pub schemas: Vec<Schema>,
    pub functions: Vec<Function>,
}

impl Module {
    pub fn new() -> Self {
        Self {
            imports: Vec::new(),
            schemas: Vec::new(),
            functions: Vec::new(),
        }
    }

    pub fn add_import(&mut self, import: impl Into<String>) {
        let imp = import.into();
        if !self.imports.contains(&imp) {
            self.imports.push(imp);
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

    /// Aligns any `Op::Call` instruction's return type `inst.ty` with the declared `ret_ty`
    /// of the target function found within the module.
    pub fn resolve_call_types(&mut self) {
        let fn_map: std::collections::HashMap<String, Type> = self
            .functions
            .iter()
            .map(|f| (f.name.clone(), f.ret_ty.clone()))
            .collect();
        for func in &mut self.functions {
            for block in &mut func.blocks {
                for inst in &mut block.instructions {
                    if let Op::Call(ref name, _) = inst.op {
                        if let Some(target_ret) = fn_map.get(name) {
                            inst.ty = target_ret.clone();
                        }
                    }
                }
            }
        }
    }
}
