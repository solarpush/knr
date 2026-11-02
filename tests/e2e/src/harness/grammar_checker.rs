use thiserror::Error;

#[derive(Error, Debug, PartialEq)]
pub enum GrammarError {
    #[error("Line {line}: missing function declaration")]
    MissingFunctionDeclaration { line: usize },
    #[error("Line {line}: invalid function signature: {detail}")]
    InvalidSignature { line: usize, detail: String },
    #[error("Line {line}: invalid block label: {label}")]
    InvalidBlockLabel { line: usize, label: String },
    #[error("Line {line}: invalid register identifier: {reg}")]
    InvalidRegister { line: usize, reg: String },
    #[error("Line {line}: unknown opcode: {opcode}")]
    UnknownOpcode { line: usize, opcode: String },
    #[error("Line {line}: unterminated string literal")]
    UnterminatedString { line: usize },
    #[error("Line {line}: block {block} missing terminator")]
    MissingTerminator { line: usize, block: String },
    #[error("Line {line}: invalid type expression: {ty}")]
    InvalidType { line: usize, ty: String },
}

pub const KNOWN_OPCODES: &[&str] = &[
    "const_i64", "const_f64", "const_bool", "const_str",
    "add", "sub", "mul", "div",
    "cmp_eq", "cmp_lt",
    "buf_alloc", "buf_read_u8", "buf_write_u8", "buf_slice",
    "vec_new", "vec_push", "vec_get", "vec_len",
    "map_new", "map_set", "map_get", "map_del",
    "chan_new", "chan_send", "chan_recv",
    "spawn", "syscall", "free",
];

pub const KNOWN_TERMINATORS: &[&str] = &["ret", "br", "br_if"];

pub const KNOWN_EFFECTS: &[&str] = &["Net", "Ui", "Hw", "Async"];

pub fn is_valid_register(reg: &str) -> bool {
    if !reg.starts_with('%') {
        return false;
    }
    let num_part = &reg[1..];
    !num_part.is_empty() && num_part.chars().all(|c| c.is_ascii_digit())
}

pub fn is_valid_block_label(lbl: &str) -> bool {
    if !lbl.starts_with("bb") {
        return false;
    }
    let num_part = &lbl[2..];
    !num_part.is_empty() && num_part.chars().all(|c| c.is_ascii_digit())
}

pub fn is_valid_scalar_type(ty: &str) -> bool {
    matches!(ty, "i64" | "f64" | "bool" | "str" | "buf" | "void")
}

pub fn validate_ssa_grammar(source: &str) -> Result<(), GrammarError> {
    let mut in_function = false;
    let mut current_block: Option<String> = None;
    let mut block_has_terminator = false;
    let mut last_line_num = 0;

    for (line_idx, raw_line) in source.lines().enumerate() {
        let line_num = line_idx + 1;
        last_line_num = line_num;
        let line = raw_line.trim();

        // Skip comments and empty lines
        if line.is_empty() || line.starts_with("//") || line.starts_with('#') {
            continue;
        }

        // Check for unterminated string quotes on the line
        let quote_count = line.chars().filter(|&c| c == '"').count();
        if quote_count % 2 != 0 {
            return Err(GrammarError::UnterminatedString { line: line_num });
        }

        if line.starts_with("fn ") {
            in_function = true;
            if !line.ends_with('{') {
                return Err(GrammarError::InvalidSignature {
                    line: line_num,
                    detail: "function header must end with '{'".to_string(),
                });
            }
            continue;
        }

        if line == "}" {
            if let Some(block) = current_block.take() {
                if !block_has_terminator {
                    return Err(GrammarError::MissingTerminator {
                        line: line_num,
                        block,
                    });
                }
            }
            in_function = false;
            continue;
        }

        if !in_function {
            continue;
        }

        // Check for basic block label
        if line.ends_with(':') {
            if let Some(block) = current_block.take() {
                if !block_has_terminator {
                    return Err(GrammarError::MissingTerminator {
                        line: line_num,
                        block,
                    });
                }
            }
            let label = line.trim_end_matches(':');
            if !is_valid_block_label(label) {
                return Err(GrammarError::InvalidBlockLabel {
                    line: line_num,
                    label: label.to_string(),
                });
            }
            current_block = Some(label.to_string());
            block_has_terminator = false;
            continue;
        }

        // Instructions and terminators
        let tokens: Vec<&str> = line.split_whitespace().collect();
        if tokens.is_empty() {
            continue;
        }

        // Check if terminator
        if KNOWN_TERMINATORS.contains(&tokens[0]) {
            block_has_terminator = true;
            match tokens[0] {
                "ret" => {
                    if tokens.len() > 1 && !is_valid_register(tokens[1]) {
                        return Err(GrammarError::InvalidRegister {
                            line: line_num,
                            reg: tokens[1].to_string(),
                        });
                    }
                }
                "br" => {
                    if tokens.len() > 1 && !is_valid_block_label(tokens[1]) {
                        return Err(GrammarError::InvalidBlockLabel {
                            line: line_num,
                            label: tokens[1].to_string(),
                        });
                    }
                }
                "br_if" => {
                    if tokens.len() > 1 && !is_valid_register(tokens[1]) {
                        return Err(GrammarError::InvalidRegister {
                            line: line_num,
                            reg: tokens[1].to_string(),
                        });
                    }
                    if tokens.len() > 2 && !is_valid_block_label(tokens[2]) {
                        return Err(GrammarError::InvalidBlockLabel {
                            line: line_num,
                            label: tokens[2].to_string(),
                        });
                    }
                    if tokens.len() > 3 && !is_valid_block_label(tokens[3]) {
                        return Err(GrammarError::InvalidBlockLabel {
                            line: line_num,
                            label: tokens[3].to_string(),
                        });
                    }
                }
                _ => {}
            }
            continue;
        }

        // Regular instruction: %dst = opcode ... OR free %reg
        if tokens[0].starts_with('%') {
            if !is_valid_register(tokens[0]) {
                return Err(GrammarError::InvalidRegister {
                    line: line_num,
                    reg: tokens[0].to_string(),
                });
            }
            if tokens.len() >= 3 && tokens[1] == "=" {
                let opcode = tokens[2];
                if !KNOWN_OPCODES.contains(&opcode) {
                    return Err(GrammarError::UnknownOpcode {
                        line: line_num,
                        opcode: opcode.to_string(),
                    });
                }
            }
        } else if tokens[0] == "free" {
            if tokens.len() > 1 && !is_valid_register(tokens[1]) {
                return Err(GrammarError::InvalidRegister {
                    line: line_num,
                    reg: tokens[1].to_string(),
                });
            }
        }
    }

    if let Some(block) = current_block {
        if !block_has_terminator {
            return Err(GrammarError::MissingTerminator {
                line: last_line_num,
                block,
            });
        }
    }

    Ok(())
}
