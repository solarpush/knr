use crate::error::ParseError;
use super::ast::Span;
use super::token::{Token, TokenKind};

/// Snapshot of lexer cursor position for clean parser lookahead & JSX mode switching.
#[derive(Debug, Clone, Copy)]
pub struct LexerState {
    pub pos: usize,
    pub line: usize,
    pub col: usize,
}

/// Pull-based Strict TypeScript / TSX Lexer with exact span tracking,
/// template literal scanning, and raw JSX child text reading.
pub struct Lexer<'a> {
    src: &'a str,
    pos: usize,
    line: usize,
    col: usize,
}

impl<'a> Lexer<'a> {
    pub fn new(src: &'a str) -> Self {
        Self {
            src,
            pos: 0,
            line: 1,
            col: 1,
        }
    }

    pub fn checkpoint(&self) -> LexerState {
        LexerState {
            pos: self.pos,
            line: self.line,
            col: self.col,
        }
    }

    pub fn restore(&mut self, st: LexerState) {
        self.pos = st.pos;
        self.line = st.line;
        self.col = st.col;
    }

    pub fn current_span(&self) -> Span {
        Span::new(self.pos, self.pos, self.line, self.col)
    }

    pub fn is_eof(&self) -> bool {
        self.pos >= self.src.len()
    }

    /// Reads raw JSX child text starting at the exact cursor until `<`, `{`, or EOF.
    /// Does NOT skip leading whitespace before capturing so inline spacing is preserved.
    pub fn read_jsx_text(&mut self) -> (String, Span) {
        let start = self.pos;
        let start_line = self.line;
        let start_col = self.col;
        let mut text = String::new();
        while let Some(ch) = self.peek_char() {
            if ch == '<' || ch == '{' {
                break;
            }
            self.advance_char();
            text.push(ch);
        }
        let span = Span::new(start, self.pos, start_line, start_col);
        (text, span)
    }

    pub fn peek_raw_char(&self) -> Option<char> {
        self.peek_char()
    }

    /// Peeks the next token without advancing the cursor.
    pub fn peek_token(&mut self) -> Result<Token, ParseError> {
        let saved = self.checkpoint();
        let tok = self.next_token();
        self.restore(saved);
        tok
    }

    /// Peeks the N-th token ahead (0 = next token, 1 = token after next).
    pub fn peek_nth_token(&mut self, n: usize) -> Result<Token, ParseError> {
        let saved = self.checkpoint();
        let mut tok = Token::new(TokenKind::Eof, self.current_span());
        for _ in 0..=n {
            match self.next_token() {
                Ok(t) => tok = t,
                Err(e) => {
                    self.restore(saved);
                    return Err(e);
                }
            }
        }
        self.restore(saved);
        Ok(tok)
    }

    /// Reads all tokens until `TokenKind::Eof` (inclusive).
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

    /// Reads the next token from the source stream.
    pub fn next_token(&mut self) -> Result<Token, ParseError> {
        self.skip_whitespace_and_comments()?;

        let start_pos = self.pos;
        let start_line = self.line;
        let start_col = self.col;

        let Some(ch) = self.advance_char() else {
            return Ok(Token::new(
                TokenKind::Eof,
                Span::new(start_pos, start_pos, start_line, start_col),
            ));
        };

        let kind = match ch {
            '@' => TokenKind::At,
            '{' => TokenKind::LBrace,
            '}' => TokenKind::RBrace,
            '(' => TokenKind::LParen,
            ')' => TokenKind::RParen,
            '[' => TokenKind::LBracket,
            ']' => TokenKind::RBracket,
            ';' => TokenKind::Semicolon,
            ':' => TokenKind::Colon,
            ',' => TokenKind::Comma,
            '?' => {
                if self.peek_char() == Some('?') {
                    self.advance_char();
                    TokenKind::QuestionQuestion
                } else {
                    TokenKind::Question
                }
            }
            '%' => TokenKind::Percent,

            '.' => {
                if self.peek_char() == Some('.') && self.peek_second_char() == Some('.') {
                    self.advance_char();
                    self.advance_char();
                    TokenKind::DotDotDot
                } else {
                    TokenKind::Dot
                }
            }

            '+' => {
                if self.peek_char() == Some('+') {
                    self.advance_char();
                    TokenKind::PlusPlus
                } else if self.peek_char() == Some('=') {
                    self.advance_char();
                    TokenKind::PlusEq
                } else {
                    TokenKind::Plus
                }
            }

            '-' => {
                if self.peek_char() == Some('-') {
                    self.advance_char();
                    TokenKind::MinusMinus
                } else if self.peek_char() == Some('=') {
                    self.advance_char();
                    TokenKind::MinusEq
                } else {
                    TokenKind::Minus
                }
            }

            '*' => {
                if self.peek_char() == Some('=') {
                    self.advance_char();
                    TokenKind::StarEq
                } else {
                    TokenKind::Star
                }
            }

            '/' => {
                if self.peek_char() == Some('=') {
                    self.advance_char();
                    TokenKind::SlashEq
                } else {
                    TokenKind::Slash
                }
            }

            '=' => {
                if self.peek_char() == Some('=') {
                    self.advance_char();
                    if self.peek_char() == Some('=') {
                        self.advance_char();
                    }
                    TokenKind::EqEq
                } else if self.peek_char() == Some('>') {
                    self.advance_char();
                    TokenKind::FatArrow
                } else {
                    TokenKind::Assign
                }
            }

            '!' => {
                if self.peek_char() == Some('=') {
                    self.advance_char();
                    if self.peek_char() == Some('=') {
                        self.advance_char();
                    }
                    TokenKind::BangEq
                } else {
                    TokenKind::Bang
                }
            }

            '<' => {
                if self.peek_char() == Some('=') {
                    self.advance_char();
                    TokenKind::LtEq
                } else {
                    TokenKind::Lt
                }
            }

            '>' => {
                if self.peek_char() == Some('=') {
                    self.advance_char();
                    TokenKind::GtEq
                } else {
                    TokenKind::Gt
                }
            }

            '&' => {
                if self.peek_char() == Some('&') {
                    self.advance_char();
                    TokenKind::AndAnd
                } else {
                    TokenKind::Amp
                }
            }

            '|' => {
                if self.peek_char() == Some('|') {
                    self.advance_char();
                    TokenKind::OrOr
                } else {
                    TokenKind::Pipe
                }
            }

            '"' | '\'' => self.lex_string(ch, start_line)?,
            '`' => self.lex_template_string(start_line)?,
            '#' => self.lex_hex_color(start_line)?,

            c if c.is_ascii_digit() => self.lex_number(start_pos, start_line)?,
            c if is_ident_start(c) => self.lex_ident(start_pos),

            other => {
                return Err(ParseError::InvalidSyntax {
                    line: start_line,
                    msg: format!(
                        "Unexpected character '{}' at line {}, column {}",
                        other, start_line, start_col
                    ),
                });
            }
        };

        Ok(Token::new(
            kind,
            Span::new(start_pos, self.pos, start_line, start_col),
        ))
    }

    fn peek_char(&self) -> Option<char> {
        self.src[self.pos..].chars().next()
    }

    fn peek_second_char(&self) -> Option<char> {
        let mut it = self.src[self.pos..].chars();
        it.next()?;
        it.next()
    }

    fn advance_char(&mut self) -> Option<char> {
        let ch = self.src[self.pos..].chars().next()?;
        self.pos += ch.len_utf8();
        if ch == '\n' {
            self.line += 1;
            self.col = 1;
        } else {
            self.col += 1;
        }
        Some(ch)
    }

    fn skip_whitespace_and_comments(&mut self) -> Result<(), ParseError> {
        loop {
            match self.peek_char() {
                Some(ch) if ch.is_whitespace() => {
                    self.advance_char();
                }
                Some('/') if self.peek_second_char() == Some('/') => {
                    self.advance_char();
                    self.advance_char();
                    while let Some(c) = self.advance_char() {
                        if c == '\n' {
                            break;
                        }
                    }
                }
                Some('/') if self.peek_second_char() == Some('*') => {
                    let start_line = self.line;
                    self.advance_char();
                    self.advance_char();
                    let mut closed = false;
                    while let Some(c) = self.advance_char() {
                        if c == '*' && self.peek_char() == Some('/') {
                            self.advance_char();
                            closed = true;
                            break;
                        }
                    }
                    if !closed {
                        return Err(ParseError::InvalidSyntax {
                            line: start_line,
                            msg: "Unterminated block comment '/* ... */'".to_string(),
                        });
                    }
                }
                _ => break,
            }
        }
        Ok(())
    }

    fn lex_ident(&mut self, start_pos: usize) -> TokenKind {
        while let Some(ch) = self.peek_char() {
            if is_ident_continue(ch) {
                self.advance_char();
            } else {
                break;
            }
        }
        let text = &self.src[start_pos..self.pos];
        match text {
            "import" => TokenKind::Import,
            "from" => TokenKind::From,
            "export" => TokenKind::Export,
            "type" => TokenKind::Type,
            "interface" => TokenKind::Interface,
            "enum" => TokenKind::Enum,
            "function" => TokenKind::Function,
            "const" => TokenKind::Const,
            "let" => TokenKind::Let,
            "return" => TokenKind::Return,
            "if" => TokenKind::If,
            "else" => TokenKind::Else,
            "while" => TokenKind::While,
            "for" => TokenKind::For,
            "switch" => TokenKind::Switch,
            "case" => TokenKind::Case,
            "default" => TokenKind::Default,
            "break" => TokenKind::Break,
            "continue" => TokenKind::Continue,
            "readonly" => TokenKind::Readonly,
            "true" => TokenKind::True,
            "false" => TokenKind::False,
            "as" => TokenKind::As,
            _ => TokenKind::Ident(text.to_string()),
        }
    }

    fn lex_number(&mut self, start_pos: usize, start_line: usize) -> Result<TokenKind, ParseError> {
        let mut is_float = false;
        while let Some(ch) = self.peek_char() {
            if ch.is_ascii_digit() || ch == '_' {
                self.advance_char();
            } else {
                break;
            }
        }
        if self.peek_char() == Some('.') && self.peek_second_char().is_some_and(|c| c.is_ascii_digit()) {
            is_float = true;
            self.advance_char();
            while let Some(ch) = self.peek_char() {
                if ch.is_ascii_digit() || ch == '_' {
                    self.advance_char();
                } else {
                    break;
                }
            }
        }
        if matches!(self.peek_char(), Some('e') | Some('E')) {
            is_float = true;
            self.advance_char();
            if matches!(self.peek_char(), Some('+') | Some('-')) {
                self.advance_char();
            }
            while let Some(ch) = self.peek_char() {
                if ch.is_ascii_digit() {
                    self.advance_char();
                } else {
                    break;
                }
            }
        }
        let raw = self.src[start_pos..self.pos].replace('_', "");
        if is_float {
            let val = raw.parse::<f64>().map_err(|_| ParseError::InvalidSyntax {
                line: start_line,
                msg: format!("Invalid float literal '{raw}'"),
            })?;
            Ok(TokenKind::FloatLit(val))
        } else {
            let val = raw.parse::<i64>().map_err(|_| ParseError::InvalidSyntax {
                line: start_line,
                msg: format!("Invalid integer literal '{raw}'"),
            })?;
            Ok(TokenKind::IntLit(val))
        }
    }

    fn lex_hex_color(&mut self, start_line: usize) -> Result<TokenKind, ParseError> {
        let mut hex = String::from("#");
        while let Some(ch) = self.peek_char() {
            if ch.is_ascii_hexdigit() {
                hex.push(ch);
                self.advance_char();
            } else {
                break;
            }
        }
        let digits = hex.len() - 1;
        if !matches!(digits, 3 | 6 | 8) {
            return Err(ParseError::InvalidSyntax {
                line: start_line,
                msg: format!("Invalid hex color literal '{hex}': expected #RGB, #RRGGBB, or #RRGGBBAA"),
            });
        }
        Ok(TokenKind::HexColor(hex))
    }

    fn lex_string(&mut self, quote: char, start_line: usize) -> Result<TokenKind, ParseError> {
        let mut s = String::new();
        while let Some(ch) = self.advance_char() {
            if ch == quote {
                return Ok(TokenKind::StrLit(s));
            }
            if ch == '\\' {
                match self.advance_char() {
                    Some('n') => s.push('\n'),
                    Some('r') => s.push('\r'),
                    Some('t') => s.push('\t'),
                    Some('\\') => s.push('\\'),
                    Some('"') => s.push('"'),
                    Some('\'') => s.push('\''),
                    Some('0') => s.push('\0'),
                    Some(other) => {
                        s.push('\\');
                        s.push(other);
                    }
                    None => {
                        return Err(ParseError::InvalidSyntax {
                            line: start_line,
                            msg: "Unterminated escape sequence in string literal".to_string(),
                        });
                    }
                }
            } else {
                s.push(ch);
            }
        }
        Err(ParseError::InvalidSyntax {
            line: start_line,
            msg: "Unterminated string literal".to_string(),
        })
    }

    fn lex_template_string(&mut self, start_line: usize) -> Result<TokenKind, ParseError> {
        let mut raw = String::new();
        let mut expr_depth: usize = 0;
        while let Some(ch) = self.advance_char() {
            if expr_depth == 0 {
                if ch == '`' {
                    return Ok(TokenKind::TemplateLit(raw));
                }
                if ch == '\\' {
                    raw.push('\\');
                    if let Some(esc) = self.advance_char() {
                        raw.push(esc);
                    }
                    continue;
                }
                if ch == '$' && self.peek_char() == Some('{') {
                    self.advance_char();
                    raw.push_str("${");
                    expr_depth = 1;
                    continue;
                }
                raw.push(ch);
            } else {
                if ch == '{' {
                    expr_depth += 1;
                    raw.push(ch);
                } else if ch == '}' {
                    expr_depth -= 1;
                    raw.push(ch);
                } else if ch == '"' || ch == '\'' {
                    raw.push(ch);
                    while let Some(sc) = self.advance_char() {
                        raw.push(sc);
                        if sc == '\\' {
                            if let Some(esc) = self.advance_char() {
                                raw.push(esc);
                            }
                        } else if sc == ch {
                            break;
                        }
                    }
                } else {
                    raw.push(ch);
                }
            }
        }
        Err(ParseError::InvalidSyntax {
            line: start_line,
            msg: "Unterminated template literal '`...`'".to_string(),
        })
    }
}

fn is_ident_start(c: char) -> bool {
    c.is_ascii_alphabetic() || c == '_' || c == '$'
}

fn is_ident_continue(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_' || c == '$'
}
