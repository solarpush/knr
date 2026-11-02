use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DiagnosticJson {
    pub status: String,
    pub error_kind: String,
    pub function: Option<String>,
    pub line: Option<usize>,
    pub instruction: Option<String>,
    pub violated_register: Option<String>,
    pub consumed_at_line: Option<usize>,
    #[serde(default)]
    pub alive_registers: Option<BTreeMap<String, String>>,
    pub fix_hint: Option<String>,
}

pub fn parse_and_validate_diagnostic(json_str: &str) -> Result<DiagnosticJson, serde_json::Error> {
    let diag: DiagnosticJson = serde_json::from_str(json_str)?;
    assert_eq!(diag.status, "error", "Diagnostic status must be 'error'");
    assert!(
        !diag.error_kind.is_empty(),
        "Diagnostic error_kind cannot be empty"
    );
    Ok(diag)
}
