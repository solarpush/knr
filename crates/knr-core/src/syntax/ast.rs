use serde::{Deserialize, Serialize};

/// Source location tracking with 1-based line/col and 0-based byte offsets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub struct Span {
    pub start: usize,
    pub end: usize,
    pub line: usize,
    pub col: usize,
}

impl Span {
    pub fn new(start: usize, end: usize, line: usize, col: usize) -> Self {
        Self {
            start,
            end,
            line,
            col,
        }
    }

    pub fn dummy() -> Self {
        Self {
            start: 0,
            end: 0,
            line: 1,
            col: 1,
        }
    }

    pub fn merge(start: Span, end: Span) -> Self {
        Self {
            start: start.start,
            end: end.end,
            line: start.line,
            col: start.col,
        }
    }
}

/// An identifier with its source span.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Ident {
    pub name: String,
    pub span: Span,
}

impl Ident {
    pub fn new(name: impl Into<String>, span: Span) -> Self {
        Self {
            name: name.into(),
            span,
        }
    }
}

/// Top-level compilation unit of a Strict TypeScript/TSX `.tsx` / `.ts` / `.knr` file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Program {
    pub imports: Vec<ImportDecl>,
    pub declarations: Vec<Declaration>,
    pub span: Span,
}

/// Import specifier form for preserving exact import syntax during formatting.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub enum ImportKind {
    #[default]
    SideEffect,
    Named(Vec<(Ident, Option<Ident>)>),
    Namespace(Ident),
    Default {
        name: Ident,
        named: Vec<(Ident, Option<Ident>)>,
    },
}

/// Import directive: `import { A, B } from "std/ui";`, `import "./helper.tsk";`, or `export { A } from "./a.tsk";`
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ImportDecl {
    pub path: String,
    #[serde(default)]
    pub kind: ImportKind,
    #[serde(default)]
    pub is_reexport: bool,
    pub span: Span,
}

/// Top-level declaration.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Declaration {
    Struct(StructDef),
    Enum(EnumDef),
    Function(FunctionDef),
    Const(ConstDef),
}

/// Top-level constant declaration: `export const ACCENT: string = "#38bdf8";`
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConstDef {
    pub name: Ident,
    pub ty: Option<TypeExpr>,
    pub value: Expr,
    #[serde(default)]
    pub is_exported: bool,
    pub span: Span,
}

/// Tagged union / Enum definition: `export enum Action { Inc(i64), Dec(i64), Reset }`
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EnumDef {
    pub name: Ident,
    #[serde(default)]
    pub type_params: Vec<Ident>,
    pub variants: Vec<EnumVariantDef>,
    #[serde(default)]
    pub is_exported: bool,
    pub span: Span,
}

/// Variant inside an enum definition.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EnumVariantDef {
    pub name: Ident,
    pub fields: Vec<TypeExpr>,
    pub span: Span,
}

/// Type / Interface / Struct definition: `export type State = { count: i64; name: string; };`
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StructDef {
    pub name: Ident,
    #[serde(default)]
    pub type_params: Vec<Ident>,
    pub fields: Vec<FieldDef>,
    #[serde(default)]
    pub is_exported: bool,
    pub span: Span,
}

/// Field within a type/interface definition.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FieldDef {
    pub index: Option<u32>,
    pub name: Ident,
    pub ty: TypeExpr,
    pub span: Span,
}

/// Function definition (`export function ...`, `@shader`, `@effects("net")`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FunctionDef {
    pub name: Ident,
    #[serde(default)]
    pub type_params: Vec<Ident>,
    pub params: Vec<ParamDef>,
    pub ret_ty: TypeExpr,
    pub effects: Vec<Ident>,
    #[serde(default)]
    pub is_shader: bool,
    #[serde(default)]
    pub is_exported: bool,
    pub body: Block,
    pub span: Span,
}

/// Function parameter definition (`s: readonly State` -> `is_borrowed: true`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ParamDef {
    pub name: Ident,
    pub ty: TypeExpr,
    #[serde(default)]
    pub is_borrowed: bool,
    pub span: Span,
}

/// A braced block containing statements and an optional tail expression.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Block {
    pub stmts: Vec<Stmt>,
    pub tail_expr: Option<Box<Expr>>,
    pub span: Span,
}

/// Statement within a block.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Stmt {
    pub kind: StmtKind,
    pub span: Span,
}

/// Destructuring pattern for `const { a, b: localB } = obj;` and `const [first, second] = arr;`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum DestructPattern {
    /// Object destructuring: `{ a, b: localB }` -> `Vec<(field_name, optional_local_alias)>`
    Object(Vec<(Ident, Option<Ident>)>),
    /// Array destructuring: `[first, second]`
    Array(Vec<Ident>),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum StmtKind {
    Let {
        name: Ident,
        ty: Option<TypeExpr>,
        init: Expr,
        #[serde(default)]
        is_const: bool,
        #[serde(default)]
        destruct: Option<DestructPattern>,
    },
    Assign {
        target: Expr,
        value: Expr,
    },
    Return {
        value: Option<Expr>,
    },
    If {
        cond: Expr,
        then_block: Block,
        else_block: Option<Block>,
    },
    While {
        cond: Expr,
        body: Block,
    },
    Break,
    Continue,
    Syscall {
        effect: Ident,
        op: String,
        payload: Expr,
    },
    Expr(Expr),
    Block(Block),
}

/// A part inside a template string literal: `` `Count: ${s.count}` ``
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum TemplatePart {
    Literal(String),
    Expr(Expr),
}

/// A prop on a JSX element: `gap={16}`, `bg="#0a0f1e"`, or boolean shorthand `checked`
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JsxProp {
    pub name: Ident,
    pub value: Option<Expr>,
    pub span: Span,
}

/// A child inside a JSX element: raw text, `{expr}`, or nested `<Child />`
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum JsxChild {
    Text(String, Span),
    Expr(Expr),
    Element(Expr),
}

/// Body of an arrow function: `(x) => expr` or `(x) => { ... }`
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ArrowBody {
    Expr(Box<Expr>),
    Block(Block),
}

/// Expression with span.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Expr {
    pub kind: ExprKind,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ExprKind {
    // Literals
    Int(i64),
    Float(f64),
    Bool(bool),
    Str(String),
    ColorHex(String),
    TemplateStr(Vec<TemplatePart>),

    // Identifier / Variable
    Variable(Ident),

    // Composite Literals
    StructLit {
        name: Option<Ident>,
        fields: Vec<FieldInit>,
        spread: Option<Box<Expr>>,
    },
    EnumLit {
        enum_name: Ident,
        #[serde(default)]
        type_args: Vec<TypeExpr>,
        variant: Ident,
        args: Vec<Expr>,
    },
    ArrayLit(Vec<Expr>),
    Borrow(Box<Expr>),

    // Arrow Function (for `.map(...)` / `.filter(...)`)
    Arrow {
        params: Vec<Ident>,
        body: ArrowBody,
    },

    // Declarative TSX/JSX Element
    Jsx {
        tag: Ident,
        props: Vec<JsxProp>,
        children: Vec<JsxChild>,
    },

    // Operations
    Unary {
        op: UnaryOp,
        operand: Box<Expr>,
    },
    Binary {
        op: BinaryOp,
        lhs: Box<Expr>,
        rhs: Box<Expr>,
    },

    // Invocations & Access
    Call {
        callee: Ident,
        #[serde(default)]
        type_args: Vec<TypeExpr>,
        args: Vec<Expr>,
    },
    MethodCall {
        receiver: Box<Expr>,
        method: Ident,
        args: Vec<Expr>,
    },
    Syscall {
        effect: Ident,
        op: String,
        payload: Box<Expr>,
    },
    FieldAccess {
        target: Box<Expr>,
        field: Ident,
    },
    Index {
        target: Box<Expr>,
        index: Box<Expr>,
    },

    // Expressions with Control Flow
    If {
        cond: Box<Expr>,
        then_branch: Block,
        else_branch: Block,
    },
    Match {
        scrutinee: Box<Expr>,
        arms: Vec<MatchArm>,
    },
    Block(Block),
}

/// A single arm inside a `switch`/`match` expression.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MatchArm {
    pub pattern: MatchPattern,
    pub body: Expr,
    pub span: Span,
}

/// Pattern matched in a `switch`/`match` arm.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum MatchPattern {
    Variant {
        enum_name: Option<Ident>,
        variant: Ident,
        bindings: Vec<Ident>,
    },
    Int(i64),
    Bool(bool),
    Str(String),
    Wildcard,
}

/// Field initialization in an object/struct literal: `count: s.count + 1`
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FieldInit {
    pub name: Ident,
    pub value: Expr,
    pub span: Span,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum UnaryOp {
    Neg, // -
    Not, // !
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum BinaryOp {
    // Arithmetic
    Add, // +
    Sub, // -
    Mul, // *
    Div, // /
    Rem, // %

    // Comparison
    Eq,    // === / ==
    NotEq, // !== / !=
    Lt,    // <
    LtEq,  // <=
    Gt,    // >
    GtEq,  // >=

    // Logical & Nullish Coalescing
    And,             // &&
    Or,              // ||
    NullishCoalesce, // ??
}

/// High-level TypeScript type expression.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TypeExpr {
    pub kind: TypeExprKind,
    #[serde(default)]
    pub type_args: Vec<TypeExpr>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum TypeExprKind {
    Named(Ident),                      // i64, f64, number, bool, boolean, string, Buffer, void, State
    Vec(Box<TypeExpr>),                // T[] or Array<T>
    Map(Box<TypeExpr>, Box<TypeExpr>), // Map<K, V>
    Chan(Box<TypeExpr>),               // Channel<T>
    Struct(Ident),                     // explicit struct
}

/// Canonical deterministic name fragment for monomorphized generic SSA types/functions.
pub fn mangle_type_name(ty: &crate::ast::Type) -> String {
    match ty {
        crate::ast::Type::I64 => "i64".to_string(),
        crate::ast::Type::F64 => "f64".to_string(),
        crate::ast::Type::Bool => "bool".to_string(),
        crate::ast::Type::Str => "str".to_string(),
        crate::ast::Type::Buf => "buf".to_string(),
        crate::ast::Type::Void => "void".to_string(),
        crate::ast::Type::Struct(name) => name.clone(),
        crate::ast::Type::Vec(inner) => format!("vec_{}", mangle_type_name(inner)),
        crate::ast::Type::Map(k, v) => {
            format!("map_{}_{}", mangle_type_name(k), mangle_type_name(v))
        }
        crate::ast::Type::Chan(inner) => format!("chan_{}", mangle_type_name(inner)),
    }
}

/// Deterministic mangled symbol name for a generic instantiation (e.g. `Option<i64>` -> `Option__i64`).
pub fn mangle_generic_name(base: &str, args: &[crate::ast::Type]) -> String {
    if args.is_empty() {
        base.to_string()
    } else {
        let parts: Vec<String> = args.iter().map(mangle_type_name).collect();
        format!("{base}__{}", parts.join("__"))
    }
}

impl TypeExpr {
    pub fn new(kind: TypeExprKind, span: Span) -> Self {
        Self {
            kind,
            type_args: Vec::new(),
            span,
        }
    }

    pub fn with_type_args(kind: TypeExprKind, type_args: Vec<TypeExpr>, span: Span) -> Self {
        Self {
            kind,
            type_args,
            span,
        }
    }

    /// Converts high-level Strict TypeScript type expression into knr-core SSA IR Type.
    pub fn to_core_type(&self) -> Result<crate::ast::Type, String> {
        match &self.kind {
            TypeExprKind::Named(ident) => {
                if !self.type_args.is_empty() {
                    let mut concrete = Vec::with_capacity(self.type_args.len());
                    for arg in &self.type_args {
                        concrete.push(arg.to_core_type()?);
                    }
                    return Ok(crate::ast::Type::Struct(mangle_generic_name(
                        &ident.name,
                        &concrete,
                    )));
                }
                match ident.name.as_str() {
                    "i64" | "int" => Ok(crate::ast::Type::I64),
                    "f64" | "number" => Ok(crate::ast::Type::F64),
                    "bool" | "boolean" => Ok(crate::ast::Type::Bool),
                    "str" | "string" => Ok(crate::ast::Type::Str),
                    "buf" | "Buffer" => Ok(crate::ast::Type::Buf),
                    "void" => Ok(crate::ast::Type::Void),
                    custom => Ok(crate::ast::Type::Struct(custom.to_string())),
                }
            }
            TypeExprKind::Vec(inner) => {
                let inner_ty = inner.to_core_type()?;
                Ok(crate::ast::Type::Vec(Box::new(inner_ty)))
            }
            TypeExprKind::Map(k, v) => {
                let k_ty = k.to_core_type()?;
                let v_ty = v.to_core_type()?;
                Ok(crate::ast::Type::Map(Box::new(k_ty), Box::new(v_ty)))
            }
            TypeExprKind::Chan(inner) => {
                let inner_ty = inner.to_core_type()?;
                Ok(crate::ast::Type::Chan(Box::new(inner_ty)))
            }
            TypeExprKind::Struct(ident) => {
                if !self.type_args.is_empty() {
                    let mut concrete = Vec::with_capacity(self.type_args.len());
                    for arg in &self.type_args {
                        concrete.push(arg.to_core_type()?);
                    }
                    return Ok(crate::ast::Type::Struct(mangle_generic_name(
                        &ident.name,
                        &concrete,
                    )));
                }
                Ok(crate::ast::Type::Struct(ident.name.clone()))
            }
        }
    }
}
