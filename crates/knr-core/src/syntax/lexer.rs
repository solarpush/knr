use super::ast::Span;
use super::token::{Token, TokenKind};
use crate::error::ParseError;

pub struct Lexer<'a> {
    src: &'a str,
    chars: Vec<(usize, char)>,
    cursor: usize,
    line: usize,
    col: usize,
}

impl<'a> Lexer<'a> {
    pub fn new(src: &'a str) -> Self {
        let chars: Vec<(usize, char)> = src.char_indices().collect();
        Self {
            src,
            chars,
            cursor: 0,
            line: 1,
            col: 1,
        }
    }

    pub fn tokenize_all(&mut self) -> Result<Vec<Token>, ParseError> {
        let mut tokens = Vec::new();
        loop {
            let tok = self.next_token()?;
            let is_eof = tok.kind == TokenKind::Eof;
            tokens.push(tok);
            if is_eof {
                break;
            }
        }
        Ok(tokens)
    }

    pub fn next_token(&mut self) -> Result<Token, ParseError> {
        self.skip_whitespace_and_comments();

        let start_pos = self.current_byte_offset();
        let start_line = self.line;
        let start_col = self.col;

        let Some((_, ch)) = self.peek() else {
            return Ok(Token::new(
                TokenKind::Eof,
                Span::new(start_pos, start_pos, start_line, start_col),
            ));
        };

        // Identifiers and keywords
        if ch.is_ascii_alphabetic() || ch == '_' {
            return self.lex_ident(start_pos, start_line, start_col);
        }

        // Numbers
        if ch.is_ascii_digit() {
            return self.lex_number(start_pos, start_line, start_col);
        }

        // String literals
        if ch == '"' {
            return self.lex_string(start_pos, start_line, start_col);
        }

        // Hex color literals: #RGB, #RRGGBB, #RRGGBBAA
        if ch == '#' {
            self.advance();
            let mut hex = String::from("#");
            while let Some((_, c)) = self.peek() {
                if c.is_ascii_hexdigit() {
                    hex.push(c);
                    self.advance();
                } else {
                    break;
                }
            }
            let len = hex.len() - 1;
            if len != 3 && len != 6 && len != 8 {
                return Err(ParseError::InvalidSyntax {
                    line: start_line,
                    msg: format!("col {start_col}: invalid hex color literal '{hex}' (expected #RGB, #RRGGBB, or #RRGGBBAA)"),
                });
            }
            let end_pos = self.current_byte_offset();
            return Ok(Token::new(
                TokenKind::HexColor(hex),
                Span::new(start_pos, end_pos, start_line, start_col),
            ));
        }

        // Two-character operators or single characters
        self.advance();
        let kind = match ch {
            '{' => TokenKind::LBrace,
            '}' => TokenKind::RBrace,
            '(' => TokenKind::LParen,
            ')' => TokenKind::RParen,
            '[' => TokenKind::LBracket,
            ']' => TokenKind::RBracket,
            ';' => TokenKind::Semicolon,
            ':' => {
                if let Some((_, ':')) = self.peek() {
                    self.advance();
                    TokenKind::ColonColon
                } else {
                    TokenKind::Colon
                }
            }
            ',' => TokenKind::Comma,
            '.' => {
                if let Some((_, '.')) = self.peek() {
                    self.advance();
                    TokenKind::DotDot
                } else {
                    TokenKind::Dot
                }
            }
            '+' => TokenKind::Plus,
            '*' => TokenKind::Star,
            '/' => TokenKind::Slash,
            '%' => TokenKind::Percent,
            '-' => {
                if let Some((_, '>')) = self.peek() {
                    self.advance();
                    TokenKind::Arrow
                } else {
                    TokenKind::Minus
                }
            }
            '=' => {
                if let Some((_, '=')) = self.peek() {
                    self.advance();
                    TokenKind::EqEq
                } else if let Some((_, '>')) = self.peek() {
                    self.advance();
                    TokenKind::FatArrow
                } else {
                    TokenKind::Assign
                }
            }
            '!' => {
                if let Some((_, '=')) = self.peek() {
                    self.advance();
                    TokenKind::BangEq
                } else {
                    TokenKind::Bang
                }
            }
            '<' => {
                if let Some((_, '=')) = self.peek() {
                    self.advance();
                    TokenKind::LtEq
                } else {
                    TokenKind::Lt
                }
            }
            '>' => {
                if let Some((_, '=')) = self.peek() {
                    self.advance();
                    TokenKind::GtEq
                } else {
                    TokenKind::Gt
                }
            }
            '&' => {
                if let Some((_, '&')) = self.peek() {
                    self.advance();
                    TokenKind::AndAnd
                } else {
                    TokenKind::Amp
                }
            }
            '|' => {
                if let Some((_, '|')) = self.peek() {
                    self.advance();
                    TokenKind::OrOr
                } else {
                    return Err(ParseError::InvalidSyntax {
                        line: start_line,
                        msg: format!("col {start_col}: unexpected character '|'"),
                    });
                }
            }
            unknown => {
                return Err(ParseError::InvalidSyntax {
                    line: start_line,
                    msg: format!("col {start_col}: unexpected character '{unknown}'"),
                });
            }
        };

        let end_pos = self.current_byte_offset();
        Ok(Token::new(
            kind,
            Span::new(start_pos, end_pos, start_line, start_col),
        ))
    }

    fn peek(&self) -> Option<(usize, char)> {
        self.chars.get(self.cursor).copied()
    }

    fn peek_nth(&self, n: usize) -> Option<(usize, char)> {
        self.chars.get(self.cursor + n).copied()
    }

    fn advance(&mut self) -> Option<(usize, char)> {
        if let Some((_, ch)) = self.peek() {
            self.cursor += 1;
            if ch == '\n' {
                self.line += 1;
                self.col = 1;
            } else {
                self.col += 1;
            }
            Some((self.cursor - 1, ch))
        } else {
            None
        }
    }

    fn current_byte_offset(&self) -> usize {
        if let Some((pos, _)) = self.peek() {
            pos
        } else {
            self.src.len()
        }
    }

    fn skip_whitespace_and_comments(&mut self) {
        loop {
            match self.peek() {
                Some((_, ch)) if ch.is_whitespace() => {
                    self.advance();
                }
                Some((_, '/')) if self.peek_nth(1).map(|(_, c)| c) == Some('/') => {
                    // Skip line comment
                    self.advance();
                    self.advance();
                    while let Some((_, c)) = self.peek() {
                        self.advance();
                        if c == '\n' {
                            break;
                        }
                    }
                }
                _ => break,
            }
        }
    }

    fn lex_ident(
        &mut self,
        start_pos: usize,
        start_line: usize,
        start_col: usize,
    ) -> Result<Token, ParseError> {
        let mut name = String::new();
        while let Some((_, ch)) = self.peek() {
            if ch.is_ascii_alphanumeric() || ch == '_' {
                name.push(ch);
                self.advance();
            } else {
                break;
            }
        }

        let kind = match name.as_str() {
            "fn" => TokenKind::Fn,
            "shader" => TokenKind::Shader,
            "struct" => TokenKind::Struct,
            "schema" => TokenKind::Schema,
            "enum" => TokenKind::Enum,
            "match" => TokenKind::Match,
            "let" => TokenKind::Let,
            "return" => TokenKind::Return,
            "if" => TokenKind::If,
            "else" => TokenKind::Else,
            "while" => TokenKind::While,
            "break" => TokenKind::Break,
            "continue" => TokenKind::Continue,
            "effects" => TokenKind::Effects,
            "import" => TokenKind::Import,
            "true" => TokenKind::True,
            "false" => TokenKind::False,
            "syscall" => TokenKind::Syscall,
            _ => TokenKind::Ident(name),
        };

        let end_pos = self.current_byte_offset();
        Ok(Token::new(
            kind,
            Span::new(start_pos, end_pos, start_line, start_col),
        ))
    }

    fn lex_number(
        &mut self,
        start_pos: usize,
        start_line: usize,
        start_col: usize,
    ) -> Result<Token, ParseError> {
        let mut num_str = String::new();
        let mut is_float = false;

        while let Some((_, ch)) = self.peek() {
            if ch.is_ascii_digit() {
                num_str.push(ch);
                self.advance();
            } else if ch == '.' && !is_float && self.peek_nth(1).map(|(_, c)| c.is_ascii_digit()).unwrap_or(false) {
                is_float = true;
                num_str.push(ch);
                self.advance();
            } else {
                break;
            }
        }

        let end_pos = self.current_byte_offset();
        let span = Span::new(start_pos, end_pos, start_line, start_col);

        if is_float {
            match num_str.parse::<f64>() {
                Ok(val) => Ok(Token::new(TokenKind::FloatLit(val), span)),
                Err(_) => Err(ParseError::InvalidLiteral {
                    line: start_line,
                    token: num_str,
                    expected: "valid float".to_string(),
                }),
            }
        } else {
            match num_str.parse::<i64>() {
                Ok(val) => Ok(Token::new(TokenKind::IntLit(val), span)),
                Err(_) => Err(ParseError::InvalidLiteral {
                    line: start_line,
                    token: num_str,
                    expected: "valid integer".to_string(),
                }),
            }
        }
    }

    fn lex_string(
        &mut self,
        start_pos: usize,
        start_line: usize,
        start_col: usize,
    ) -> Result<Token, ParseError> {
        // Consume opening '"'
        self.advance();

        let mut s = String::new();
        loop {
            match self.peek() {
                None => {
                    return Err(ParseError::UnterminatedString { line: start_line });
                }
                Some((_, '"')) => {
                    self.advance();
                    break;
                }
                Some((_, '\\')) => {
                    self.advance();
                    match self.peek() {
                        None => {
                            return Err(ParseError::UnterminatedString { line: start_line });
                        }
                        Some((_, 'n')) => {
                            s.push('\n');
                            self.advance();
                        }
                        Some((_, 't')) => {
                            s.push('\t');
                            self.advance();
                        }
                        Some((_, 'r')) => {
                            s.push('\r');
                            self.advance();
                        }
                        Some((_, '\\')) => {
                            s.push('\\');
                            self.advance();
                        }
                        Some((_, '"')) => {
                            s.push('"');
                            self.advance();
                        }
                        Some((_, c)) => {
                            s.push(c);
                            self.advance();
                        }
                    }
                }
                Some((_, ch)) => {
                    s.push(ch);
                    self.advance();
                }
            }
        }

        let end_pos = self.current_byte_offset();
        Ok(Token::new(
            TokenKind::StrLit(s),
            Span::new(start_pos, end_pos, start_line, start_col),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_lex_basic() {
        let src = r#"
            // comment
            fn main() -> i64 effects(Ui) {
                let x = 42;
                let y = 3.25;
                return x + y;
            }
        "#;
        let mut lexer = Lexer::new(src);
        let tokens = lexer.tokenize_all().unwrap();
        assert!(tokens.iter().any(|t| t.kind == TokenKind::Fn));
        assert!(tokens.iter().any(|t| t.kind == TokenKind::IntLit(42)));
        assert!(tokens.iter().any(|t| matches!(t.kind, TokenKind::FloatLit(f) if (f - 3.25).abs() < 1e-6)));
    }

    #[test]
    fn test_lex_string_escapes() {
        let src = r#"let s = "hello\nworld \"quoted\"";"#;
        let mut lexer = Lexer::new(src);
        let tokens = lexer.tokenize_all().unwrap();
        let str_tok = tokens.iter().find(|t| matches!(t.kind, TokenKind::StrLit(_))).unwrap();
        if let TokenKind::StrLit(ref s) = str_tok.kind {
            assert_eq!(s, "hello\nworld \"quoted\"");
        } else {
            panic!("Expected string token");
        }
    }

    #[test]
    fn test_lex_operators() {
        let src = "== != <= >= && || -> =>";
        let mut lexer = Lexer::new(src);
        let tokens = lexer.tokenize_all().unwrap();
        let kinds: Vec<_> = tokens.into_iter().map(|t| t.kind).collect();
        assert_eq!(
            kinds,
            vec![
                TokenKind::EqEq,
                TokenKind::BangEq,
                TokenKind::LtEq,
                TokenKind::GtEq,
                TokenKind::AndAnd,
                TokenKind::OrOr,
                TokenKind::Arrow,
                TokenKind::FatArrow,
                TokenKind::Eof,
            ]
        );
    }
}
