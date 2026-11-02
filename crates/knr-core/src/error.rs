use thiserror::Error;

#[derive(Error, Debug, Clone, PartialEq)]
pub enum ParseError {
    #[error("Parse error at line {line}: unexpected end of input")]
    UnexpectedEof { line: usize },

    #[error("Parse error at line {line}: {msg}")]
    InvalidSyntax { line: usize, msg: String },

    #[error("Parse error at line {line}: unknown opcode '{opcode}'")]
    UnknownOpcode { line: usize, opcode: String },

    #[error("Parse error at line {line}: invalid register token '{token}', expected %<digits>")]
    InvalidRegister { line: usize, token: String },

    #[error("Parse error at line {line}: invalid block label '{token}', expected bb<digits>:")]
    InvalidBlockLabel { line: usize, token: String },

    #[error("Parse error at line {line}: invalid type '{token}'")]
    InvalidType { line: usize, token: String },

    #[error("Parse error at line {line}: invalid effect '{token}'")]
    InvalidEffect { line: usize, token: String },

    #[error("Parse error at line {line}: unterminated string literal")]
    UnterminatedString { line: usize },

    #[error("Parse error at line {line}: invalid literal '{token}': {expected}")]
    InvalidLiteral { line: usize, token: String, expected: String },

    #[error("Parse error at line {line}: block 'bb{block_id}' is missing a terminator (ret, br, br_if)")]
    MissingTerminator { line: usize, block_id: u32 },

    #[error("Parse error at line {line}: missing closing brace '}}'")]
    MissingClosingBrace { line: usize },

    #[error("Parse error at line {line}: inline schema '{name}' is forbidden in code file: {fix_hint}")]
    InlineSchemaInCodeFile {
        line: usize,
        name: String,
        fix_hint: String,
    },

    #[error("Parse error at line {line}: invalid construct in schema file: {msg}")]
    InvalidConstructInSchemaFile { line: usize, msg: String },

    #[error("Parse error at line {line}: import directive must appear at the top of the file before declarations")]
    ImportAfterDeclaration { line: usize },

    #[error("Circular import detected: {}", .cycle.join(" -> "))]
    CircularImport { cycle: Vec<String> },

    #[error("Duplicate symbol '{symbol}' defined in '{first_file}' and '{second_file}'")]
    DuplicateSymbol {
        symbol: String,
        first_file: String,
        second_file: String,
    },

    #[error("File not found at line {line}: '{path}'")]
    FileNotFound { line: usize, path: String },

    #[error("I/O error for '{path}': {msg}")]
    IoError { path: String, msg: String },
}

#[derive(Error, Debug, Clone, PartialEq)]
pub enum BinaryError {
    #[error("Invalid magic header: {0:?}")]
    BadMagic([u8; 4]),

    #[error("Unsupported binary version: {0}")]
    UnsupportedVersion(u16),

    #[error("Unexpected end of binary stream: expected {expected} bytes, got {actual}")]
    UnexpectedEof { expected: usize, actual: usize },

    #[error("Corrupted pool {pool}: index {index} out of bounds (size {size})")]
    PoolIndexOutOfBounds {
        pool: &'static str,
        index: usize,
        size: usize,
    },

    #[error("Invalid opcode tag: {0:#04x}")]
    InvalidOpcode(u8),

    #[error("Invalid type tag: {0:#04x}")]
    InvalidTypeTag(u8),

    #[error("Register overflow: register ID {0} exceeds 16-bit encoding limit")]
    RegisterOverflow(u32),

    #[error("String decode error: {0}")]
    Utf8Error(String),

    #[error("IO error: {0}")]
    IoError(String),
}

impl From<std::string::FromUtf8Error> for BinaryError {
    fn from(e: std::string::FromUtf8Error) -> Self {
        BinaryError::Utf8Error(e.to_string())
    }
}

impl From<std::io::Error> for BinaryError {
    fn from(e: std::io::Error) -> Self {
        BinaryError::IoError(e.to_string())
    }
}

#[derive(Error, Debug)]
pub enum CoreError {
    #[error("Parse error: {0}")]
    Parse(#[from] ParseError),
    #[error("Binary error: {0}")]
    Binary(#[from] BinaryError),
}
