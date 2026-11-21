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

/// The top-level compilation unit of a `.knr` file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Program {
    pub imports: Vec<ImportDecl>,
    pub declarations: Vec<Declaration>,
    pub span: Span,
}

/// Import directive: `import "path/to/file";`
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ImportDecl {
    pub path: String,
    pub span: Span,
}

/// Top-level declaration.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Declaration {
    Struct(StructDef),
    Enum(EnumDef),
    Function(FunctionDef),
}

/// Tagged union / Enum definition: `enum Msg { Inc, Dec, Add(i64) }`
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EnumDef {
    pub name: Ident,
    pub variants: Vec<EnumVariantDef>,
    pub span: Span,
}

/// Variant inside an enum definition.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EnumVariantDef {
    pub name: Ident,
    pub fields: Vec<TypeExpr>,
    pub span: Span,
}

/// Struct / Schema definition.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StructDef {
    pub name: Ident,
    pub fields: Vec<FieldDef>,
    pub span: Span,
}

/// Field within a struct definition.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FieldDef {
    pub index: Option<u32>,
    pub name: Ident,
    pub ty: TypeExpr,
    pub span: Span,
}

/// Function definition.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FunctionDef {
    pub name: Ident,
    pub params: Vec<ParamDef>,
    pub ret_ty: TypeExpr,
    pub effects: Vec<Ident>,
    #[serde(default)]
    pub is_shader: bool,
    pub body: Block,
    pub span: Span,
}

/// Function parameter definition.
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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum StmtKind {
    Let {
        name: Ident,
        ty: Option<TypeExpr>,
        init: Expr,
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

    // Identifier / Variable
    Variable(Ident),

    // Composite Literals
    StructLit {
        name: Ident,
        fields: Vec<FieldInit>,
        spread: Option<Box<Expr>>,
    },
    EnumLit {
        enum_name: Ident,
        variant: Ident,
        args: Vec<Expr>,
    },
    ArrayLit(Vec<Expr>),
    Borrow(Box<Expr>),

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

/// A single arm inside a `match` expression.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MatchArm {
    pub pattern: MatchPattern,
    pub body: Expr,
    pub span: Span,
}

/// Pattern matched in a `match` arm.
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

/// Field initialization in a struct literal: `tag: "article"`
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
    Eq,    // ==
    NotEq, // !=
    Lt,    // <
    LtEq,  // <=
    Gt,    // >
    GtEq,  // >=

    // Logical
    And, // &&
    Or,  // ||
}

/// High-level type expression.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TypeExpr {
    pub kind: TypeExprKind,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum TypeExprKind {
    Named(Ident),                      // i64, f64, bool, str, buf, void, Point
    Vec(Box<TypeExpr>),                // vec[T] or [T]
    Map(Box<TypeExpr>, Box<TypeExpr>), // map[K, V]
    Chan(Box<TypeExpr>),               // chan[T]
    Struct(Ident),                     // struct[Card]
}

impl TypeExpr {
    pub fn new(kind: TypeExprKind, span: Span) -> Self {
        Self { kind, span }
    }

    /// Helper to convert high-level AST type expression into knr-core SSA IR Type.
    pub fn to_core_type(&self) -> Result<crate::ast::Type, String> {
        match &self.kind {
            TypeExprKind::Named(ident) => match ident.name.as_str() {
                "i64" => Ok(crate::ast::Type::I64),
                "f64" => Ok(crate::ast::Type::F64),
                "bool" => Ok(crate::ast::Type::Bool),
                "str" => Ok(crate::ast::Type::Str),
                "buf" => Ok(crate::ast::Type::Buf),
                "void" => Ok(crate::ast::Type::Void),
                custom => Ok(crate::ast::Type::Struct(custom.to_string())),
            },
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
            TypeExprKind::Struct(ident) => Ok(crate::ast::Type::Struct(ident.name.clone())),
        }
    }
}
