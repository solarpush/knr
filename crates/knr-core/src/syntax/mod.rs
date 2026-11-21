pub mod ast;
pub mod lexer;
pub mod parser;
pub mod stdlib;
pub mod token;

pub use ast::*;
pub use lexer::Lexer;
pub use parser::Parser;
pub use stdlib::resolve_builtin_std;
pub use token::{Token, TokenKind};

use crate::error::ParseError;

/// Parses high-level `.knr` source text into a typed `Program` AST.
pub fn parse_knr(src: &str) -> Result<Program, ParseError> {
    let mut parser = Parser::new(src)?;
    parser.parse_program()
}
