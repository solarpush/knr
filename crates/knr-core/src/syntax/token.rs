use super::ast::Span;

#[derive(Debug, Clone, PartialEq)]
pub enum TokenKind {
    // Keywords
    Fn,
    Struct,
    Schema,
    Enum,
    Match,
    Let,
    Return,
    If,
    Else,
    While,
    Break,
    Continue,
    Effects,
    Import,
    True,
    False,
    Syscall,

    // Identifiers & Literals
    Ident(String),
    IntLit(i64),
    FloatLit(f64),
    StrLit(String),

    // Symbols & Delimiters
    LBrace,     // {
    RBrace,     // }
    LParen,     // (
    RParen,     // )
    LBracket,   // [
    RBracket,   // ]
    Semicolon,  // ;
    Colon,      // :
    ColonColon, // ::
    Comma,      // ,
    Dot,        // .
    Arrow,      // ->
    FatArrow,   // =>
    Assign,     // =
    Amp,        // &

    // Operators
    Plus,    // +
    Minus,   // -
    Star,    // *
    Slash,   // /
    Percent, // %
    EqEq,    // ==
    BangEq,  // !=
    Lt,      // <
    LtEq,    // <=
    Gt,      // >
    GtEq,    // >=
    AndAnd,  // &&
    OrOr,    // ||
    Bang,    // !

    Eof,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Token {
    pub kind: TokenKind,
    pub span: Span,
}

impl Token {
    pub fn new(kind: TokenKind, span: Span) -> Self {
        Self { kind, span }
    }
}
