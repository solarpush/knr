use knr_core::{Instruction, Terminator};
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub column: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub span: Option<(usize, usize)>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_variable: Option<String>,
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
            column: None,
            span: None,
            source_variable: None,
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

    pub fn with_column(mut self, col: usize) -> Self {
        self.column = Some(col);
        self
    }

    pub fn with_span(mut self, start: usize, end: usize) -> Self {
        self.span = Some((start, end));
        self
    }

    pub fn with_source_variable(mut self, var: impl Into<String>) -> Self {
        self.source_variable = Some(var.into());
        self
    }

    pub fn from_lower_error(err: knr_core::lower::LowerError) -> Self {
        let line = err.line();
        let var_name = err.variable_name();
        Self {
            status: "error".to_string(),
            error_kind: err.kind_str().to_string(),
            function: "<program>".to_string(),
            line,
            instruction: "".to_string(),
            violated_register: None,
            consumed_at_line: None,
            alive_registers: None,
            fix_hint: Some(err.message()),
            column: None,
            span: None,
            source_variable: var_name,
        }
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
    knr_core::format_instruction(inst)
}

pub fn format_terminator(term: &Terminator) -> String {
    knr_core::format_terminator(term)
}
