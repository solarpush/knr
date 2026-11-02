use crate::ast::*;
use crate::error::ParseError;
use std::collections::HashMap;

/// Parses complete textual SSA into a Module.
pub fn parse_ssa(input: &str) -> Result<Module, ParseError> {
    let mut module = Module::new();
    let lines: Vec<&str> = input.lines().collect();
    let mut idx = 0;

    while idx < lines.len() {
        let line_num = idx + 1;
        let stripped = strip_comment(lines[idx]);
        let trimmed = stripped.trim();

        if trimmed.is_empty() {
            idx += 1;
            continue;
        }

        if trimmed.starts_with("schema ") {
            let (schema, next_idx) = parse_schema_from_lines(&lines, idx)?;
            module.add_schema(schema);
            idx = next_idx;
        } else if trimmed.starts_with("fn ") {
            let (func, next_idx) =
                parse_function_from_lines_with_schemas(&lines, idx, &module.schemas)?;
            module.add_function(func);
            idx = next_idx;
        } else {
            return Err(ParseError::InvalidSyntax {
                line: line_num,
                msg: format!(
                    "Expected schema or function definition, found '{}'",
                    trimmed
                ),
            });
        }
    }

    Ok(module)
}

/// Parses a single function from a textual SSA string.
pub fn parse_function(input: &str) -> Result<Function, ParseError> {
    let lines: Vec<&str> = input.lines().collect();
    let mut idx = 0;
    let mut schemas = Vec::new();
    while idx < lines.len() {
        let stripped = strip_comment(lines[idx]);
        let trimmed = stripped.trim();
        if !trimmed.is_empty() {
            if trimmed.starts_with("schema ") {
                let (schema, next_idx) = parse_schema_from_lines(&lines, idx)?;
                schemas.push(schema);
                idx = next_idx;
                continue;
            } else if trimmed.starts_with("fn ") {
                let (func, _) = parse_function_from_lines_with_schemas(&lines, idx, &schemas)?;
                return Ok(func);
            } else {
                return Err(ParseError::InvalidSyntax {
                    line: idx + 1,
                    msg: format!("Expected 'fn <name>...', found '{}'", trimmed),
                });
            }
        }
        idx += 1;
    }
    Err(ParseError::UnexpectedEof { line: lines.len() })
}

fn strip_comment(line: &str) -> &str {
    let mut in_quote = false;
    let mut escape = false;
    let bytes = line.as_bytes();
    for i in 0..bytes.len() {
        if escape {
            escape = false;
            continue;
        }
        if bytes[i] == b'\\' && in_quote {
            escape = true;
            continue;
        }
        if bytes[i] == b'"' {
            in_quote = !in_quote;
            continue;
        }
        if !in_quote {
            if bytes[i] == b'#' {
                return &line[..i];
            }
            if bytes[i] == b'/' && i + 1 < bytes.len() && bytes[i + 1] == b'/' {
                return &line[..i];
            }
        }
    }
    line
}

fn is_valid_ident(s: &str) -> bool {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {
            chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
        }
        _ => false,
    }
}

fn parse_schema_from_lines(
    lines: &[&str],
    mut idx: usize,
) -> Result<(Schema, usize), ParseError> {
    let header_line_num = idx + 1;
    let header_line = strip_comment(lines[idx]).trim();
    idx += 1;

    if !header_line.starts_with("schema ") {
        return Err(ParseError::InvalidSyntax {
            line: header_line_num,
            msg: format!("Expected 'schema <name>...', got '{}'", header_line),
        });
    }

    let without_schema = header_line[7..].trim();
    let name_part = if let Some(brace_pos) = without_schema.find('{') {
        without_schema[..brace_pos].trim()
    } else {
        return Err(ParseError::InvalidSyntax {
            line: header_line_num,
            msg: "Schema header must end with '{'".into(),
        });
    };

    if name_part.is_empty() {
        return Err(ParseError::InvalidSyntax {
            line: header_line_num,
            msg: "Missing schema name".into(),
        });
    }
    if !is_valid_ident(name_part) {
        return Err(ParseError::InvalidSyntax {
            line: header_line_num,
            msg: format!("Invalid schema name '{}': must be a valid identifier", name_part),
        });
    }
    let schema_name = name_part.to_string();
    let mut schema = Schema::new(schema_name);
    let mut next_field_idx = 0u32;

    while idx < lines.len() {
        let line_num = idx + 1;
        let raw = strip_comment(lines[idx]);
        let trimmed = raw.trim();
        idx += 1;

        if trimmed.is_empty() {
            continue;
        }

        if trimmed == "}" {
            return Ok((schema, idx));
        }

        let field_line = trimmed.trim_end_matches(',').trim();
        if field_line.is_empty() {
            continue;
        }

        let first_colon = field_line.find(':').ok_or_else(|| ParseError::InvalidSyntax {
            line: line_num,
            msg: format!("Expected ':' in schema field definition, got '{}'", field_line),
        })?;
        let part1 = field_line[..first_colon].trim();
        let after_first = field_line[first_colon + 1..].trim();

        if let Ok(explicit_idx) = part1.parse::<u32>() {
            if explicit_idx > u16::MAX as u32 {
                return Err(ParseError::InvalidSyntax {
                    line: line_num,
                    msg: format!("Field index {} exceeds maximum allowed index (65535)", explicit_idx),
                });
            }
            // Case 1: 0: x: i64
            if let Some(second_colon) = after_first.find(':') {
                let candidate_name = after_first[..second_colon].trim();
                let after_second = after_first[second_colon + 1..].trim();
                if !candidate_name.is_empty() && is_valid_ident(candidate_name) {
                    let ty = parse_full_type(after_second, line_num)?;
                    schema.add_field(explicit_idx, Some(candidate_name.to_string()), ty);
                    next_field_idx = explicit_idx.saturating_add(1);
                    continue;
                }
            }
            // Case 2: 0: x i64 or 0: i64
            let mut parts = after_first.split_whitespace();
            let first_token = parts.next().unwrap_or("");
            let rest_tokens = after_first[first_token.len()..].trim();
            if !rest_tokens.is_empty()
                && is_valid_ident(first_token)
                && !first_token.starts_with("struct[")
                && !first_token.starts_with("vec[")
                && !first_token.starts_with("map[")
                && !first_token.starts_with("chan[")
            {
                if let Ok(ty) = parse_full_type(rest_tokens, line_num) {
                    schema.add_field(explicit_idx, Some(first_token.to_string()), ty);
                    next_field_idx = explicit_idx.saturating_add(1);
                    continue;
                }
            }
            let ty = parse_full_type(after_first, line_num)?;
            schema.add_field(explicit_idx, None, ty);
            next_field_idx = explicit_idx.saturating_add(1);
        } else {
            if !is_valid_ident(part1) {
                return Err(ParseError::InvalidSyntax {
                    line: line_num,
                    msg: format!("Invalid field identifier '{}' in schema field definition", part1),
                });
            }
            if next_field_idx > u16::MAX as u32 {
                return Err(ParseError::InvalidSyntax {
                    line: line_num,
                    msg: format!("Field index {} exceeds maximum allowed index (65535)", next_field_idx),
                });
            }
            let ty = parse_full_type(after_first, line_num)?;
            schema.add_field(next_field_idx, Some(part1.to_string()), ty);
            next_field_idx = next_field_idx.saturating_add(1);
        }
    }

    Err(ParseError::MissingClosingBrace {
        line: lines.len(),
    })
}

#[allow(dead_code)]
fn parse_function_from_lines(
    lines: &[&str],
    idx: usize,
) -> Result<(Function, usize), ParseError> {
    parse_function_from_lines_with_schemas(lines, idx, &[])
}

fn parse_function_from_lines_with_schemas(
    lines: &[&str],
    mut idx: usize,
    schemas: &[Schema],
) -> Result<(Function, usize), ParseError> {
    let header_line_num = idx + 1;
    let header_line = strip_comment(lines[idx]).trim();
    idx += 1;

    // Syntax: fn <name>(<params>) -> <ret_ty> [effects(<effects>)] {
    if !header_line.starts_with("fn ") {
        return Err(ParseError::InvalidSyntax {
            line: header_line_num,
            msg: format!("Expected 'fn <name>...', got '{}'", header_line),
        });
    }

    let without_fn = &header_line[3..].trim();
    let paren_open = without_fn.find('(').ok_or_else(|| ParseError::InvalidSyntax {
        line: header_line_num,
        msg: "Missing '(' in function declaration".into(),
    })?;
    let name = without_fn[..paren_open].trim().to_string();

    let after_paren_open = &without_fn[paren_open + 1..];
    let paren_close = after_paren_open.find(')').ok_or_else(|| ParseError::InvalidSyntax {
        line: header_line_num,
        msg: "Missing ')' in function parameter list".into(),
    })?;
    let params_part = &after_paren_open[..paren_close].trim();
    let rest = after_paren_open[paren_close + 1..].trim();

    let mut params = Vec::new();
    let mut reg_types: HashMap<RegId, Type> = HashMap::new();

    if !params_part.is_empty() {
        let param_tokens = split_top_level_comma(params_part);
        for p in param_tokens {
            let p = p.trim();
            if p.is_empty() {
                continue;
            }
            let colon = p.find(':').ok_or_else(|| ParseError::InvalidSyntax {
                line: header_line_num,
                msg: format!("Expected '%<reg>: <Type>' in param, got '{}'", p),
            })?;
            let reg_str = p[..colon].trim();
            let ty_str = p[colon + 1..].trim();
            let reg = parse_reg(reg_str, header_line_num)?;
            let ty = parse_full_type(ty_str, header_line_num)?;
            reg_types.insert(reg, ty.clone());
            params.push((reg, ty));
        }
    }

    // Expect "->"
    let arrow_pos = rest.find("->").ok_or_else(|| ParseError::InvalidSyntax {
        line: header_line_num,
        msg: "Missing '->' return type in function declaration".into(),
    })?;
    let mut after_arrow = rest[arrow_pos + 2..].trim();

    // Check for trailing '{'
    if after_arrow.ends_with('{') {
        after_arrow = after_arrow[..after_arrow.len() - 1].trim();
    } else {
        return Err(ParseError::InvalidSyntax {
            line: header_line_num,
            msg: "Function header must end with '{'".into(),
        });
    }

    // Parse ret_ty and optional effects
    let mut effects = Vec::new();
    let ret_ty: Type;

    if let Some(eff_pos) = after_arrow.find("effects") {
        let ty_part = after_arrow[..eff_pos].trim();
        ret_ty = parse_full_type(ty_part, header_line_num)?;

        let eff_part = after_arrow[eff_pos + 7..].trim();
        let eff_paren_open = eff_part.find('(').ok_or_else(|| ParseError::InvalidSyntax {
            line: header_line_num,
            msg: "Missing '(' in effects list".into(),
        })?;
        let eff_paren_close = eff_part.find(')').ok_or_else(|| ParseError::InvalidSyntax {
            line: header_line_num,
            msg: "Missing ')' in effects list".into(),
        })?;
        let eff_tokens = &eff_part[eff_paren_open + 1..eff_paren_close];
        for eff_str in split_top_level_comma(eff_tokens) {
            let eff_str = eff_str.trim();
            if eff_str.is_empty() {
                continue;
            }
            let eff = match eff_str {
                "Net" => Effect::Net,
                "Ui" => Effect::Ui,
                "Hw" => Effect::Hw,
                "Async" => Effect::Async,
                other => {
                    return Err(ParseError::InvalidEffect {
                        line: header_line_num,
                        token: other.to_string(),
                    })
                }
            };
            if !effects.contains(&eff) {
                effects.push(eff);
            }
        }
    } else {
        ret_ty = parse_full_type(after_arrow, header_line_num)?;
    }

    let mut func = Function::new(name, ret_ty);
    func.params = params;
    func.effects = effects;

    // Parse basic blocks until closing brace '}'
    let mut current_block: Option<BasicBlock> = None;
    let mut block_has_terminator = false;

    while idx < lines.len() {
        let line_num = idx + 1;
        let raw = strip_comment(lines[idx]);
        let trimmed = raw.trim();
        idx += 1;

        if trimmed.is_empty() {
            continue;
        }

        if trimmed == "}" {
            if let Some(block) = current_block.take() {
                if !block_has_terminator {
                    return Err(ParseError::MissingTerminator {
                        line: line_num,
                        block_id: block.id,
                    });
                }
                func.add_block(block);
            }
            return Ok((func, idx));
        }

        // Check if label "bb<digits>:"
        if trimmed.starts_with("bb") && trimmed.ends_with(':') {
            let label = &trimmed[2..trimmed.len() - 1];
            let blk_id: u32 = label.parse().map_err(|_| ParseError::InvalidBlockLabel {
                line: line_num,
                token: trimmed.to_string(),
            })?;

            if let Some(b) = current_block.take() {
                if !block_has_terminator {
                    return Err(ParseError::MissingTerminator {
                        line: line_num,
                        block_id: b.id,
                    });
                }
                func.add_block(b);
            }

            // Create new block with temporary dummy terminator (replaced when terminator line is parsed)
            current_block = Some(BasicBlock {
                id: blk_id,
                instructions: Vec::new(),
                terminator: Terminator::Ret(None),
            });
            block_has_terminator = false;
            continue;
        }

        // Must be inside a basic block
        let block = current_block.as_mut().ok_or_else(|| ParseError::InvalidSyntax {
            line: line_num,
            msg: format!("Instruction outside basic block: '{}'", trimmed),
        })?;

        if block_has_terminator {
            return Err(ParseError::InvalidSyntax {
                line: line_num,
                msg: format!(
                    "Unreachable statement after terminator in block 'bb{}': '{}'",
                    block.id, trimmed
                ),
            });
        }

        // Check if terminator
        if trimmed == "ret"
            || trimmed.starts_with("ret ")
            || trimmed.starts_with("br ")
            || trimmed.starts_with("br_if ")
        {
            let term = parse_terminator(trimmed, line_num)?;
            block.terminator = term;
            block_has_terminator = true;
            continue;
        }

        // Instruction: either "%<dst> = <op>" or "<op>" (free %<reg>)
        let (dst, op_str) = if let Some(eq_pos) = trimmed.find('=') {
            let dst_str = trimmed[..eq_pos].trim();
            let dst_reg = parse_reg(dst_str, line_num)?;
            let rest_op = trimmed[eq_pos + 1..].trim();
            (Some(dst_reg), rest_op)
        } else {
            (None, trimmed)
        };

        let (op, inferred_ty) = parse_op(op_str, line_num, &reg_types, schemas)?;
        let ty = inferred_ty;
        if let Some(d) = dst {
            reg_types.insert(d, ty.clone());
        }

        block.instructions.push(Instruction {
            dst,
            ty,
            op,
            line: line_num,
        });
    }

    Err(ParseError::MissingClosingBrace {
        line: lines.len(),
    })
}

fn parse_terminator(s: &str, line: usize) -> Result<Terminator, ParseError> {
    let tokens = tokenize_whitespace(s);
    if tokens.is_empty() {
        return Err(ParseError::InvalidSyntax {
            line,
            msg: "Empty terminator".into(),
        });
    }

    match tokens[0] {
        "ret" => {
            if tokens.len() == 1 {
                Ok(Terminator::Ret(None))
            } else if tokens.len() == 2 {
                let reg = parse_reg(tokens[1], line)?;
                Ok(Terminator::Ret(Some(reg)))
            } else {
                Err(ParseError::InvalidSyntax {
                    line,
                    msg: format!("Malformed 'ret' terminator: '{}'", s),
                })
            }
        }
        "br" => {
            if tokens.len() == 2 {
                let target = parse_block_target(tokens[1], line)?;
                Ok(Terminator::Br(target))
            } else {
                Err(ParseError::InvalidSyntax {
                    line,
                    msg: format!("Malformed 'br' terminator: '{}'", s),
                })
            }
        }
        "br_if" => {
            if tokens.len() == 4 {
                let cond = parse_reg(tokens[1], line)?;
                let then_blk = parse_block_target(tokens[2], line)?;
                let else_blk = parse_block_target(tokens[3], line)?;
                Ok(Terminator::BrIf {
                    cond,
                    then_blk,
                    else_blk,
                })
            } else {
                Err(ParseError::InvalidSyntax {
                    line,
                    msg: format!("Malformed 'br_if' terminator: '{}'", s),
                })
            }
        }
        other => Err(ParseError::InvalidSyntax {
            line,
            msg: format!("Unknown terminator '{}'", other),
        }),
    }
}

fn parse_op(
    s: &str,
    line: usize,
    reg_types: &HashMap<RegId, Type>,
    schemas: &[Schema],
) -> Result<(Op, Type), ParseError> {
    let mut rest = s.trim();
    let opcode = extract_ident(&mut rest);

    match opcode {
        "const_i64" => {
            let val: i64 = rest.trim().parse().map_err(|_| ParseError::InvalidLiteral {
                line,
                token: rest.to_string(),
                expected: "i64 integer".into(),
            })?;
            Ok((Op::ConstI64(val), Type::I64))
        }
        "const_f64" => {
            let val: f64 = rest.trim().parse().map_err(|_| ParseError::InvalidLiteral {
                line,
                token: rest.to_string(),
                expected: "f64 float".into(),
            })?;
            Ok((Op::ConstF64(val), Type::F64))
        }
        "const_bool" => {
            let val: bool = rest.trim().parse().map_err(|_| ParseError::InvalidLiteral {
                line,
                token: rest.to_string(),
                expected: "bool (true/false)".into(),
            })?;
            Ok((Op::ConstBool(val), Type::Bool))
        }
        "const_str" => {
            let s_val = parse_quoted_string(&mut rest, line)?;
            Ok((Op::ConstStr(s_val), Type::Str))
        }
        "add" => {
            let (r1, r2) = parse_two_regs(rest, line)?;
            let ty = reg_types.get(&r1).cloned().unwrap_or(Type::I64);
            Ok((Op::Add(r1, r2), ty))
        }
        "sub" => {
            let (r1, r2) = parse_two_regs(rest, line)?;
            let ty = reg_types.get(&r1).cloned().unwrap_or(Type::I64);
            Ok((Op::Sub(r1, r2), ty))
        }
        "mul" => {
            let (r1, r2) = parse_two_regs(rest, line)?;
            let ty = reg_types.get(&r1).cloned().unwrap_or(Type::I64);
            Ok((Op::Mul(r1, r2), ty))
        }
        "div" => {
            let (r1, r2) = parse_two_regs(rest, line)?;
            let ty = reg_types.get(&r1).cloned().unwrap_or(Type::I64);
            Ok((Op::Div(r1, r2), ty))
        }
        "cmp_eq" => {
            let (r1, r2) = parse_two_regs(rest, line)?;
            Ok((Op::CmpEq(r1, r2), Type::Bool))
        }
        "cmp_lt" => {
            let (r1, r2) = parse_two_regs(rest, line)?;
            Ok((Op::CmpLt(r1, r2), Type::Bool))
        }
        "buf_alloc" => {
            let r = parse_one_reg(rest, line)?;
            Ok((Op::BufAlloc(r), Type::Buf))
        }
        "buf_read_u8" => {
            let (b, idx) = parse_two_regs(rest, line)?;
            Ok((Op::BufReadU8(b, idx), Type::I64))
        }
        "buf_write_u8" => {
            let (b, idx, v) = parse_three_regs(rest, line)?;
            Ok((Op::BufWriteU8(b, idx, v), Type::Buf))
        }
        "buf_slice" => {
            let (b, s, l) = parse_three_regs(rest, line)?;
            Ok((Op::BufSlice(b, s, l), Type::Buf))
        }
        "vec_new" => {
            let elem_ty = parse_full_type(rest.trim(), line)?;
            Ok((Op::VecNew(elem_ty.clone()), Type::Vec(Box::new(elem_ty))))
        }
        "vec_push" => {
            let (v, item) = parse_two_regs(rest, line)?;
            let ty = reg_types
                .get(&v)
                .cloned()
                .unwrap_or_else(|| Type::Vec(Box::new(Type::I64)));
            Ok((Op::VecPush(v, item), ty))
        }
        "vec_get" => {
            let (v, idx) = parse_two_regs(rest, line)?;
            let ty = if let Some(Type::Vec(elem)) = reg_types.get(&v) {
                *elem.clone()
            } else {
                Type::I64
            };
            Ok((Op::VecGet(v, idx), ty))
        }
        "vec_len" => {
            let v = parse_one_reg(rest, line)?;
            Ok((Op::VecLen(v), Type::I64))
        }
        "map_new" => {
            let (k_ty, remaining) = parse_type_prefix(rest.trim(), line)?;
            let v_ty = parse_full_type(remaining.trim(), line)?;
            Ok((
                Op::MapNew(k_ty.clone(), v_ty.clone()),
                Type::Map(Box::new(k_ty), Box::new(v_ty)),
            ))
        }
        "map_set" => {
            let (m, k, v) = parse_three_regs(rest, line)?;
            let ty = reg_types
                .get(&m)
                .cloned()
                .unwrap_or_else(|| Type::Map(Box::new(Type::Str), Box::new(Type::I64)));
            Ok((Op::MapSet(m, k, v), ty))
        }
        "map_get" => {
            let (m, k) = parse_two_regs(rest, line)?;
            let ty = if let Some(Type::Map(_, val_ty)) = reg_types.get(&m) {
                *val_ty.clone()
            } else {
                Type::I64
            };
            Ok((Op::MapGet(m, k), ty))
        }
        "map_del" => {
            let (m, k) = parse_two_regs(rest, line)?;
            let ty = reg_types
                .get(&m)
                .cloned()
                .unwrap_or_else(|| Type::Map(Box::new(Type::Str), Box::new(Type::I64)));
            Ok((Op::MapDel(m, k), ty))
        }
        "chan_new" => {
            let (elem_ty, remaining) = parse_type_prefix(rest.trim(), line)?;
            let cap = parse_one_reg(remaining.trim(), line)?;
            Ok((
                Op::ChanNew(elem_ty.clone(), cap),
                Type::Chan(Box::new(elem_ty)),
            ))
        }
        "chan_send" => {
            let (c, val) = parse_two_regs(rest, line)?;
            Ok((Op::ChanSend(c, val), Type::Bool))
        }
        "chan_recv" => {
            let c = parse_one_reg(rest, line)?;
            let ty = if let Some(Type::Chan(elem)) = reg_types.get(&c) {
                *elem.clone()
            } else {
                Type::I64
            };
            Ok((Op::ChanRecv(c), ty))
        }
        "spawn" => {
            let fn_name = parse_quoted_string_or_ident(&mut rest, line)?;
            let arg = parse_one_reg(rest.trim(), line)?;
            Ok((Op::Spawn(fn_name, arg), Type::Bool))
        }
        "syscall" => {
            let eff_ident = extract_ident(&mut rest);
            let eff = match eff_ident {
                "Net" => Effect::Net,
                "Ui" => Effect::Ui,
                "Hw" => Effect::Hw,
                "Async" => Effect::Async,
                other => {
                    return Err(ParseError::InvalidEffect {
                        line,
                        token: other.to_string(),
                    })
                }
            };
            let op_name = parse_quoted_string_or_ident(&mut rest, line)?;
            let payload = parse_one_reg(rest.trim(), line)?;
            let ret_ty = if eff == Effect::Ui && op_name.contains("render") {
                Type::Str
            } else {
                Type::I64
            };
            Ok((Op::Syscall(eff, op_name, payload), ret_ty))
        }
        "free" => {
            let r = parse_one_reg(rest, line)?;
            Ok((Op::Free(r), Type::Void))
        }
        "struct_new" => {
            let trimmed = rest.trim_start();
            let name = if let Some(after) = trimmed.strip_prefix("struct[") {
                let close = after.find(']').ok_or_else(|| ParseError::InvalidSyntax {
                    line,
                    msg: "Missing closing ']' in struct_new".into(),
                })?;
                rest = &after[close + 1..];
                after[..close].trim().to_string()
            } else if let Some(after) = trimmed.strip_prefix("struct<") {
                let close = after.find('>').ok_or_else(|| ParseError::InvalidSyntax {
                    line,
                    msg: "Missing closing '>' in struct_new".into(),
                })?;
                rest = &after[close + 1..];
                after[..close].trim().to_string()
            } else {
                let mut n = parse_quoted_string_or_ident(&mut rest, line)?;
                if (n.starts_with("struct[") && n.ends_with(']'))
                    || (n.starts_with("struct<") && n.ends_with('>'))
                {
                    n = n[7..n.len() - 1].to_string();
                }
                n
            };
            if name.is_empty() {
                return Err(ParseError::InvalidSyntax {
                    line,
                    msg: "Schema name in struct_new cannot be empty".into(),
                });
            }
            if !is_valid_ident(&name) {
                return Err(ParseError::InvalidSyntax {
                    line,
                    msg: format!("Invalid schema identifier '{}' in struct_new", name),
                });
            }
            if !rest.trim().is_empty() {
                return Err(ParseError::InvalidSyntax {
                    line,
                    msg: format!("Unexpected trailing arguments for struct_new: '{}'", rest.trim()),
                });
            }
            Ok((Op::StructNew(name.clone()), Type::Struct(name)))
        }
        "struct_get" => {
            let tokens = tokenize_whitespace(rest);
            if tokens.len() != 2 {
                return Err(ParseError::InvalidSyntax {
                    line,
                    msg: format!(
                        "Expected 2 arguments for struct_get: '<struct_reg> <field>', found '{}'",
                        rest
                    ),
                });
            }
            let s = parse_reg(tokens[0], line)?;
            let field_tok = tokens[1];
            let (field_idx, inferred_ty) = if let Ok(idx) = field_tok.parse::<u32>() {
                if idx > u16::MAX as u32 {
                    return Err(ParseError::InvalidSyntax {
                        line,
                        msg: format!("Field index {} exceeds maximum allowed index (65535)", idx),
                    });
                }
                let ty = if let Some(Type::Struct(sname)) = reg_types.get(&s) {
                    if let Some(schema) = schemas.iter().find(|sc| sc.name == *sname) {
                        schema
                            .get_field(idx)
                            .map(|f| f.ty.clone())
                            .unwrap_or(Type::I64)
                    } else {
                        Type::I64
                    }
                } else {
                    Type::I64
                };
                (idx, ty)
            } else if let Some(Type::Struct(sname)) = reg_types.get(&s) {
                if let Some(schema) = schemas.iter().find(|sc| sc.name == *sname) {
                    if let Some(f) = schema.get_field_by_name(field_tok) {
                        (f.index, f.ty.clone())
                    } else {
                        return Err(ParseError::InvalidSyntax {
                            line,
                            msg: format!("Field '{field_tok}' is not declared in schema '{sname}'"),
                        });
                    }
                } else {
                    return Err(ParseError::InvalidSyntax {
                        line,
                        msg: format!("Schema '{sname}' is not declared in module"),
                    });
                }
            } else {
                return Err(ParseError::InvalidSyntax {
                    line,
                    msg: format!("Invalid field identifier '{}'", field_tok),
                });
            };
            Ok((Op::StructGet(s, field_idx), inferred_ty))
        }
        "struct_set" => {
            let tokens = tokenize_whitespace(rest);
            if tokens.len() != 3 {
                return Err(ParseError::InvalidSyntax {
                    line,
                    msg: format!(
                        "Expected 3 arguments for struct_set: '<struct_reg> <field> <val_reg>', found '{}'",
                        rest
                    ),
                });
            }
            let s = parse_reg(tokens[0], line)?;
            let field_tok = tokens[1];
            let v = parse_reg(tokens[2], line)?;
            let field_idx = if let Ok(idx) = field_tok.parse::<u32>() {
                if idx > u16::MAX as u32 {
                    return Err(ParseError::InvalidSyntax {
                        line,
                        msg: format!("Field index {} exceeds maximum allowed index (65535)", idx),
                    });
                }
                idx
            } else if let Some(Type::Struct(sname)) = reg_types.get(&s) {
                if let Some(schema) = schemas.iter().find(|sc| sc.name == *sname) {
                    if let Some(f) = schema.get_field_by_name(field_tok) {
                        f.index
                    } else {
                        return Err(ParseError::InvalidSyntax {
                            line,
                            msg: format!("Field '{field_tok}' is not declared in schema '{sname}'"),
                        });
                    }
                } else {
                    return Err(ParseError::InvalidSyntax {
                        line,
                        msg: format!("Schema '{sname}' is not declared in module"),
                    });
                }
            } else {
                return Err(ParseError::InvalidSyntax {
                    line,
                    msg: format!("Invalid field identifier '{}'", field_tok),
                });
            };
            let ret_ty = reg_types
                .get(&s)
                .cloned()
                .unwrap_or_else(|| Type::Struct("Unknown".to_string()));
            Ok((Op::StructSet(s, field_idx, v), ret_ty))
        }
        other => Err(ParseError::UnknownOpcode {
            line,
            opcode: other.to_string(),
        }),
    }
}

fn parse_one_reg(s: &str, line: usize) -> Result<RegId, ParseError> {
    let tokens = tokenize_whitespace(s);
    if tokens.len() != 1 {
        return Err(ParseError::InvalidSyntax {
            line,
            msg: format!("Expected 1 register argument, found '{}'", s),
        });
    }
    parse_reg(tokens[0], line)
}

fn parse_two_regs(s: &str, line: usize) -> Result<(RegId, RegId), ParseError> {
    let tokens = tokenize_whitespace(s);
    if tokens.len() != 2 {
        return Err(ParseError::InvalidSyntax {
            line,
            msg: format!("Expected 2 register arguments, found '{}'", s),
        });
    }
    let r1 = parse_reg(tokens[0], line)?;
    let r2 = parse_reg(tokens[1], line)?;
    Ok((r1, r2))
}

fn parse_three_regs(s: &str, line: usize) -> Result<(RegId, RegId, RegId), ParseError> {
    let tokens = tokenize_whitespace(s);
    if tokens.len() != 3 {
        return Err(ParseError::InvalidSyntax {
            line,
            msg: format!("Expected 3 register arguments, found '{}'", s),
        });
    }
    let r1 = parse_reg(tokens[0], line)?;
    let r2 = parse_reg(tokens[1], line)?;
    let r3 = parse_reg(tokens[2], line)?;
    Ok((r1, r2, r3))
}

fn parse_reg(s: &str, line: usize) -> Result<RegId, ParseError> {
    let s = s.trim();
    if !s.starts_with('%') {
        return Err(ParseError::InvalidRegister {
            line,
            token: s.to_string(),
        });
    }
    let digits = &s[1..];
    digits.parse::<u32>().map_err(|_| ParseError::InvalidRegister {
        line,
        token: s.to_string(),
    })
}

fn parse_block_target(s: &str, line: usize) -> Result<BlockId, ParseError> {
    let s = s.trim();
    if !s.starts_with("bb") {
        return Err(ParseError::InvalidBlockLabel {
            line,
            token: s.to_string(),
        });
    }
    let digits = &s[2..];
    digits.parse::<u32>().map_err(|_| ParseError::InvalidBlockLabel {
        line,
        token: s.to_string(),
    })
}

fn extract_ident<'a>(s: &mut &'a str) -> &'a str {
    let trimmed = s.trim_start();
    let end = trimmed
        .find(|c: char| !c.is_alphanumeric() && c != '_' && c != '.')
        .unwrap_or(trimmed.len());
    let ident = &trimmed[..end];
    *s = &trimmed[end..];
    ident
}

fn parse_quoted_string(s: &mut &str, line: usize) -> Result<String, ParseError> {
    let trimmed = s.trim_start();
    if !trimmed.starts_with('"') {
        return Err(ParseError::InvalidSyntax {
            line,
            msg: format!("Expected '\"', found '{}'", trimmed),
        });
    }

    let mut out = String::new();
    let inner = &trimmed[1..];
    let mut escape = false;

    for (idx, c) in inner.char_indices() {
        if escape {
            match c {
                '"' => out.push('"'),
                '\\' => out.push('\\'),
                'n' => out.push('\n'),
                't' => out.push('\t'),
                'r' => out.push('\r'),
                '0' => out.push('\0'),
                other => out.push(other),
            }
            escape = false;
        } else if c == '\\' {
            escape = true;
        } else if c == '"' {
            *s = &inner[idx + 1..];
            return Ok(out);
        } else {
            out.push(c);
        }
    }

    Err(ParseError::UnterminatedString { line })
}

fn parse_quoted_string_or_ident(s: &mut &str, line: usize) -> Result<String, ParseError> {
    let trimmed = s.trim_start();
    if trimmed.starts_with('"') {
        parse_quoted_string(s, line)
    } else if trimmed.starts_with('%') {
        let end = trimmed
            .find(|c: char| c.is_whitespace())
            .unwrap_or(trimmed.len());
        let reg = &trimmed[..end];
        *s = &trimmed[end..];
        Ok(reg.to_string())
    } else {
        let ident = extract_ident(s);
        if ident.is_empty() {
            Err(ParseError::InvalidSyntax {
                line,
                msg: "Expected identifier or string".into(),
            })
        } else {
            Ok(ident.to_string())
        }
    }
}

/// Parses a full Type string, ensuring all input is consumed.
pub fn parse_full_type(s: &str, line: usize) -> Result<Type, ParseError> {
    let (ty, rem) = parse_type_prefix(s, line)?;
    if !rem.trim().is_empty() {
        return Err(ParseError::InvalidType {
            line,
            token: s.to_string(),
        });
    }
    Ok(ty)
}

/// Parses a Type prefix from a string and returns (Type, remaining_str).
pub fn parse_type_prefix(s: &str, line: usize) -> Result<(Type, &str), ParseError> {
    let s = s.trim_start();
    if let Some(rem) = s.strip_prefix("i64") {
        Ok((Type::I64, rem))
    } else if let Some(rem) = s.strip_prefix("f64") {
        Ok((Type::F64, rem))
    } else if let Some(rem) = s.strip_prefix("bool") {
        Ok((Type::Bool, rem))
    } else if let Some(rem) = s.strip_prefix("struct") {
        let rem = rem.trim_start();
        let (_open_b, close_b) = if rem.starts_with('[') {
            ('[', ']')
        } else if rem.starts_with('<') {
            ('<', '>')
        } else {
            return Err(ParseError::InvalidType {
                line,
                token: s.to_string(),
            });
        };
        let after_open = &rem[1..];
        let close_pos = after_open.find(close_b).ok_or_else(|| ParseError::InvalidType {
            line,
            token: format!("Missing closing '{}' in struct type: '{}'", close_b, s),
        })?;
        let name = after_open[..close_pos].trim();
        if name.is_empty() {
            return Err(ParseError::InvalidType {
                line,
                token: format!("Empty struct name in '{}'", s),
            });
        }
        Ok((Type::Struct(name.to_string()), &after_open[close_pos + 1..]))
    } else if let Some(rem) = s.strip_prefix("str") {
        if rem.starts_with(|c: char| c.is_alphanumeric() || c == '_') {
            return Err(ParseError::InvalidType {
                line,
                token: s.to_string(),
            });
        }
        Ok((Type::Str, rem))
    } else if let Some(rem) = s.strip_prefix("buf") {
        Ok((Type::Buf, rem))
    } else if let Some(rem) = s.strip_prefix("void") {
        Ok((Type::Void, rem))
    } else if let Some(rem) = s.strip_prefix("vec") {
        let rem = rem.trim_start();
        let (_open_b, close_b) = if rem.starts_with('[') {
            ('[', ']')
        } else if rem.starts_with('<') {
            ('<', '>')
        } else {
            return Err(ParseError::InvalidType {
                line,
                token: s.to_string(),
            });
        };
        let after_open = &rem[1..];
        let (inner, after_inner) = parse_type_prefix(after_open, line)?;
        let trimmed_after = after_inner.trim_start();
        if !trimmed_after.starts_with(close_b) {
            return Err(ParseError::InvalidType {
                line,
                token: format!("Missing closing '{}' in vec type: '{}'", close_b, s),
            });
        }
        Ok((Type::Vec(Box::new(inner)), &trimmed_after[1..]))
    } else if let Some(rem) = s.strip_prefix("map") {
        let rem = rem.trim_start();
        let (_open_b, close_b) = if rem.starts_with('[') {
            ('[', ']')
        } else if rem.starts_with('<') {
            ('<', '>')
        } else {
            return Err(ParseError::InvalidType {
                line,
                token: s.to_string(),
            });
        };
        let after_open = &rem[1..];
        let (k_ty, after_k) = parse_type_prefix(after_open, line)?;
        let trimmed_k = after_k.trim_start();
        if !trimmed_k.starts_with(',') {
            return Err(ParseError::InvalidType {
                line,
                token: format!("Missing ',' between map types in '{}'", s),
            });
        }
        let after_comma = &trimmed_k[1..];
        let (v_ty, after_v) = parse_type_prefix(after_comma, line)?;
        let trimmed_v = after_v.trim_start();
        if !trimmed_v.starts_with(close_b) {
            return Err(ParseError::InvalidType {
                line,
                token: format!("Missing closing '{}' in map type: '{}'", close_b, s),
            });
        }
        Ok((
            Type::Map(Box::new(k_ty), Box::new(v_ty)),
            &trimmed_v[1..],
        ))
    } else if let Some(rem) = s.strip_prefix("chan") {
        let rem = rem.trim_start();
        let (_open_b, close_b) = if rem.starts_with('[') {
            ('[', ']')
        } else if rem.starts_with('<') {
            ('<', '>')
        } else {
            return Err(ParseError::InvalidType {
                line,
                token: s.to_string(),
            });
        };
        let after_open = &rem[1..];
        let (inner, after_inner) = parse_type_prefix(after_open, line)?;
        let trimmed_after = after_inner.trim_start();
        if !trimmed_after.starts_with(close_b) {
            return Err(ParseError::InvalidType {
                line,
                token: format!("Missing closing '{}' in chan type: '{}'", close_b, s),
            });
        }
        Ok((Type::Chan(Box::new(inner)), &trimmed_after[1..]))
    } else {
        let trimmed = s.trim_start();
        if let Some(first_char) = trimmed.chars().next() {
            if first_char.is_ascii_uppercase() {
                let end = trimmed
                    .find(|c: char| !c.is_ascii_alphanumeric() && c != '_')
                    .unwrap_or(trimmed.len());
                let ident = &trimmed[..end];
                return Ok((Type::Struct(ident.to_string()), &trimmed[end..]));
            }
        }
        Err(ParseError::InvalidType {
            line,
            token: s.to_string(),
        })
    }
}

fn tokenize_whitespace(s: &str) -> Vec<&str> {
    s.split_whitespace().collect()
}

fn split_top_level_comma(s: &str) -> Vec<&str> {
    let mut result = Vec::new();
    let mut depth: usize = 0;
    let mut last = 0;
    let bytes = s.as_bytes();

    for i in 0..bytes.len() {
        match bytes[i] {
            b'[' | b'<' | b'(' => depth += 1,
            b']' | b'>' | b')' => depth = depth.saturating_sub(1),
            b',' if depth == 0 => {
                result.push(&s[last..i]);
                last = i + 1;
            }
            _ => {}
        }
    }
    if last <= s.len() {
        result.push(&s[last..]);
    }
    result
}
