use super::ast::Span;

#[derive(Debug, Clone, PartialEq)]
pub enum TokenKind {
    // Strict TypeScript / TSX Keywords
    Import,
    From,
    Export,
    Type,
    Interface,
    Enum,
    Function,
    Const,
    Let,
    Return,
    If,
    Else,
    While,
    For,
    Switch,
    Case,
    Default,
    Break,
    Continue,
    Readonly,
    True,
    False,
    As,

    // Identifiers & Literals
    Ident(String),
    IntLit(i64),
    FloatLit(f64),
    StrLit(String),
    TemplateLit(String),
    HexColor(String),

    // Decorators & Symbols
    At,        // @
    LBrace,    // {
    RBrace,    // }
    LParen,    // (
    RParen,    // )
    LBracket,  // [
    RBracket,  // ]
    Semicolon, // ;
    Colon,     // :
    Comma,     // ,
    Dot,              // .
    DotDotDot,        // ...
    Question,         // ?
    QuestionQuestion, // ??
    Pipe,             // |
    Amp,       // &
    FatArrow,  // =>
    Assign,    // =
    PlusEq,    // +=
    MinusEq,   // -=
    StarEq,    // *=
    SlashEq,   // /=
    PlusPlus,  // ++
    MinusMinus,// --

    // Operators
    Plus,    // +
    Minus,   // -
    Star,    // *
    Slash,   // /
    Percent, // %
    EqEq,    // == / ===
    BangEq,  // != / !==
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
