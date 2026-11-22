pub mod ast;
pub mod format;
pub mod lexer;
pub mod parser;
pub mod stdlib;
pub mod token;

use std::path::Path;

pub use ast::*;
pub use format::format_tsk;
pub use lexer::Lexer;
pub use parser::Parser;
pub use stdlib::resolve_builtin_std;
pub use token::{Token, TokenKind};

use crate::error::ParseError;

/// Returns `true` if `path` is a high-level Strict TypeScript/TSX source file (`.tsk` or legacy `.knr`).
pub fn is_source_file(path: &Path) -> bool {
    let s = path.to_string_lossy();
    s.ends_with(".tsk") || s.ends_with(".knr")
}

/// Parses high-level Strict TypeScript/TSX source text into a typed `Program` AST.
pub fn parse_tsx(src: &str) -> Result<Program, ParseError> {
    let mut parser = Parser::new(src);
    parser.parse_program()
}

/// Alias for `parse_tsx` (parses Strict TypeScript/TSX source into a typed `Program` AST).
pub fn parse_knr(src: &str) -> Result<Program, ParseError> {
    parse_tsx(src)
}
