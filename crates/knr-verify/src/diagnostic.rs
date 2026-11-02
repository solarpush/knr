use knr_core::{Instruction, Op, Terminator};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Diagnostic {
    pub status: String,
    pub error_kind: String,
    pub function: String,
    pub line: usize,
    pub instruction: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub violated_register: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub consumed_at_line: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alive_registers: Option<BTreeMap<String, String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fix_hint: Option<String>,
}

impl Diagnostic {
    pub fn new(
        error_kind: impl Into<String>,
        function: impl Into<String>,
        line: usize,
        instruction: impl Into<String>,
    ) -> Self {
        Self {
            status: "error".to_string(),
            error_kind: error_kind.into(),
            function: function.into(),
            line,
            instruction: instruction.into(),
            violated_register: None,
            consumed_at_line: None,
            alive_registers: None,
            fix_hint: None,
        }
    }

    pub fn with_violated_register(mut self, reg: impl Into<String>) -> Self {
        self.violated_register = Some(reg.into());
        self
    }

    pub fn with_consumed_at(mut self, line: usize) -> Self {
        self.consumed_at_line = Some(line);
        self
    }

    pub fn with_alive_registers(mut self, regs: BTreeMap<String, String>) -> Self {
        self.alive_registers = Some(regs);
        self
    }

    pub fn with_fix_hint(mut self, hint: impl Into<String>) -> Self {
        self.fix_hint = Some(hint.into());
        self
    }

    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string_pretty(self)
    }
}

impl fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "[{}] {} at {}:{} `{}`",
            self.status, self.error_kind, self.function, self.line, self.instruction
        )?;
        if let Some(ref reg) = self.violated_register {
            write!(f, " (register {})", reg)?;
        }
        if let Some(consumed) = self.consumed_at_line {
            write!(f, " (consumed at line {})", consumed)?;
        }
        if let Some(ref hint) = self.fix_hint {
            write!(f, " - hint: {}", hint)?;
        }
        Ok(())
    }
}

impl std::error::Error for Diagnostic {}

pub fn format_instruction(inst: &Instruction) -> String {
    let op_str = format_op(&inst.op);
    match inst.dst {
        Some(dst) => format!("%{dst} = {op_str}"),
        None => op_str,
    }
}

pub fn format_terminator(term: &Terminator) -> String {
    match term {
        Terminator::Ret(Some(reg)) => format!("ret %{reg}"),
        Terminator::Ret(None) => "ret".to_string(),
        Terminator::Br(blk) => format!("br bb{blk}"),
        Terminator::BrIf {
            cond,
            then_blk,
            else_blk,
        } => format!("br_if %{cond} bb{then_blk} bb{else_blk}"),
    }
}

fn format_op(op: &Op) -> String {
    match op {
        Op::ConstI64(v) => format!("const_i64 {v}"),
        Op::ConstF64(v) => {
            let s = v.to_string();
            if !s.contains('.') && !s.contains('e') && !s.contains('E') && v.is_finite() {
                format!("const_f64 {s}.0")
            } else {
                format!("const_f64 {s}")
            }
        }
        Op::ConstBool(v) => format!("const_bool {v}"),
        Op::ConstStr(s) => format!("const_str \"{}\"", escape_str(s)),
        Op::Add(a, b) => format!("add %{a} %{b}"),
        Op::Sub(a, b) => format!("sub %{a} %{b}"),
        Op::Mul(a, b) => format!("mul %{a} %{b}"),
        Op::Div(a, b) => format!("div %{a} %{b}"),
        Op::CmpEq(a, b) => format!("cmp_eq %{a} %{b}"),
        Op::CmpLt(a, b) => format!("cmp_lt %{a} %{b}"),
        Op::BufAlloc(s) => format!("buf_alloc %{s}"),
        Op::BufReadU8(b, i) => format!("buf_read_u8 %{b} %{i}"),
        Op::BufWriteU8(b, i, v) => format!("buf_write_u8 %{b} %{i} %{v}"),
        Op::BufSlice(b, s, l) => format!("buf_slice %{b} %{s} %{l}"),
        Op::VecNew(t) => format!("vec_new {t}"),
        Op::VecPush(v, item) => format!("vec_push %{v} %{item}"),
        Op::VecGet(v, i) => format!("vec_get %{v} %{i}"),
        Op::VecLen(v) => format!("vec_len %{v}"),
        Op::MapNew(k, v) => format!("map_new {k} {v}"),
        Op::MapSet(m, k, v) => format!("map_set %{m} %{k} %{v}"),
        Op::MapGet(m, k) => format!("map_get %{m} %{k}"),
        Op::MapDel(m, k) => format!("map_del %{m} %{k}"),
        Op::ChanNew(t, cap) => format!("chan_new {t} %{cap}"),
        Op::ChanSend(c, val) => format!("chan_send %{c} %{val}"),
        Op::ChanRecv(c) => format!("chan_recv %{c}"),
        Op::Spawn(fn_name, arg) => format!("spawn \"{}\" %{arg}", escape_str(fn_name)),
        Op::Syscall(eff, op_name, payload) => {
            format!("syscall {eff} \"{}\" %{payload}", escape_str(op_name))
        }
        Op::Free(r) => format!("free %{r}"),
        Op::StructNew(name) => format!("struct_new {name}"),
        Op::StructGet(s, idx) => format!("struct_get %{s} {idx}"),
        Op::StructSet(s, idx, val) => format!("struct_set %{s} {idx} %{val}"),
    }
}

fn escape_str(s: &str) -> String {
    let mut out = String::new();
    for ch in s.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            c => out.push(c),
        }
    }
    out
}
