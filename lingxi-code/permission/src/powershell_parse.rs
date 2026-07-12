//! PowerShell command parsing via `pwsh` — byte-faithful port of claude-code's
//! parse layer for [`crate::powershell_containment`].
//!
//! claude-code does NOT hand-parse PowerShell. It base64-encodes the command,
//! prepends it to an embedded PowerShell script that calls
//! `[System.Management.Automation.Language.Parser]::ParseInput`, runs that
//! through `pwsh -NoProfile -NonInteractive -NoLogo -EncodedCommand <utf16le-b64>`,
//! and consumes the emitted JSON AST — then transforms it into the node model the
//! containment validator (`xgg`/`Z_u`) walks. When `pwsh` is unavailable or the
//! command does not parse, `valid` is false and the caller passes through (no
//! containment) — so this is inert on hosts without PowerShell.
//!
//! This module owns: the embedded parse script ([`PS_PARSE_SCRIPT`]), the
//! `pwsh` argument vector ([`PWSH_ARGS`]), the script/command wrapping
//! ([`build_pwsh_script`]), and the pure JSON-AST → [`PsStatement`] transform
//! (claude-code `ufg`/`Nhu`/`cfg`/`yBr`/`_Br`). The actual subprocess spawn is
//! injected by the caller (the permission gate wiring), keeping the transform
//! fully testable.

use crate::powershell_containment::{PsCommand, PsElement, PsRedirection, PsStatement};
use serde_json::Value;

/// The embedded PowerShell parse script (claude-code `Fhu`) — verbatim. It reads
/// `$EncodedCommand` (base64 of the command), parses via `ParseInput`, and emits
/// a compact JSON AST.
pub const PS_PARSE_SCRIPT: &str = include_str!("powershell_parse.ps1");

/// The `pwsh` argument vector preceding the encoded command (claude-code
/// `["-NoProfile","-NonInteractive","-NoLogo","-EncodedCommand", …]`).
pub const PWSH_ARGS: [&str; 4] = ["-NoProfile", "-NonInteractive", "-NoLogo", "-EncodedCommand"];

/// Build the PowerShell script to run for `command` (claude-code `afg`):
/// `$EncodedCommand = '<base64-of-command>'` followed by [`PS_PARSE_SCRIPT`].
/// The result is what the caller passes (UTF-16LE-base64-encoded) to
/// `pwsh -EncodedCommand`.
#[must_use]
pub fn build_pwsh_script(command: &str) -> String {
    format!("$EncodedCommand = '{}'\n{PS_PARSE_SCRIPT}", base64_std(command.as_bytes()))
}

/// Standard base64 (claude-code `Buffer.from(e,"utf8").toString("base64")`), for
/// the `$EncodedCommand` literal inside the script.
#[must_use]
pub fn base64_std(input: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        out.push(T[((n >> 18) & 63) as usize] as char);
        out.push(T[((n >> 12) & 63) as usize] as char);
        out.push(if chunk.len() > 1 { T[((n >> 6) & 63) as usize] as char } else { '=' });
        out.push(if chunk.len() > 2 { T[(n & 63) as usize] as char } else { '=' });
    }
    out
}

/// UTF-16LE base64 for `pwsh -EncodedCommand` (claude-code `sfg`): encode the
/// script as little-endian UTF-16 bytes, then standard base64.
#[must_use]
pub fn encode_for_pwsh(script: &str) -> String {
    let mut utf16le = Vec::with_capacity(script.len() * 2);
    for u in script.encode_utf16() {
        utf16le.push((u & 0xFF) as u8);
        utf16le.push((u >> 8) as u8);
    }
    base64_std(&utf16le)
}

/// A parsed PowerShell command (claude-code parse result). `valid` is false when
/// `pwsh` was unavailable or the command did not parse — the caller then passes
/// through with NO containment.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ParseResult {
    /// Whether the command parsed with no errors.
    pub valid: bool,
    /// The transformed statements to validate.
    pub statements: Vec<PsStatement>,
}

// ───────────────────────────────────────────────────────────────────────────
// JSON-AST → node model transform (ufg / Nhu / cfg / yBr / _Br)
// ───────────────────────────────────────────────────────────────────────────

/// `Cae` — normalize a JSON value that may be absent / a single object / an
/// array (PowerShell `ConvertTo-Json` renders a one-element `@(x)` as a bare
/// object and `@()` as null) into a slice of elements.
fn cae(v: Option<&Value>) -> Vec<&Value> {
    match v {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Array(a)) => a.iter().collect(),
        Some(x) => vec![x],
    }
}

fn str_field<'a>(v: &'a Value, key: &str) -> Option<&'a str> {
    v.get(key).and_then(Value::as_str)
}

/// `Yve` — normalize the en/em/horizontal-bar dashes to ASCII `-`.
fn yve(s: &str) -> String {
    s.replace(['\u{2013}', '\u{2014}', '\u{2015}'], "-")
}

/// Strip ONE leading and ONE trailing straight quote (claude-code
/// `/^['"]|['"]$/g` applied to a command name).
fn strip_one_quote(s: &str) -> &str {
    let s = s.strip_prefix(['\'', '"']).unwrap_or(s);
    s.strip_suffix(['\'', '"']).unwrap_or(s)
}

/// `xzn` — strip a `Module\Cmdlet` module-qualifier prefix (but not a drive /
/// UNC / `.\`/`..\` path).
fn strip_module_path(e: &str) -> &str {
    let Some(t) = e.rfind('\\') else {
        return e;
    };
    let is_drive = e.len() >= 2 && e.as_bytes()[0].is_ascii_alphabetic() && e.as_bytes()[1] == b':';
    if is_drive || e.starts_with("\\\\") || e.starts_with(".\\") || e.starts_with("..\\") {
        return e;
    }
    let r = &e[t + 1..];
    if r.is_empty() {
        e
    } else {
        r
    }
}

/// `_Br` — map a raw AST node type (and, for a `CommandExpressionAst`, its inner
/// expression type) to the simplified element type the extractor uses.
fn br_map(node_type: &str, expr_type: Option<&str>) -> String {
    match node_type {
        "ScriptBlockExpressionAst" => "ScriptBlock",
        "SubExpressionAst" | "ArrayExpressionAst" | "ParenExpressionAst" => "SubExpression",
        "ExpandableStringExpressionAst" => "ExpandableString",
        "InvokeMemberExpressionAst" | "MemberExpressionAst" => "MemberInvocation",
        "VariableExpressionAst" => "Variable",
        "StringConstantExpressionAst" | "ConstantExpressionAst" => "StringConstant",
        "CommandParameterAst" => "Parameter",
        "CommandExpressionAst" => return expr_type.map_or("Other".to_string(), |t| br_map(t, None)),
        _ => "Other",
    }
    .to_string()
}

/// The internal (operator, target, is_merging) of a transformed redirection —
/// `operator` is used only for `ufg`'s dedup, then dropped.
struct Redir {
    operator: String,
    target: String,
    is_merging: bool,
}

/// `yBr` — transform a raw redirection JSON node.
fn transform_redirection(raw: &Value) -> Redir {
    if str_field(raw, "type") == Some("MergingRedirectionAst") {
        return Redir { operator: "2>&1".to_string(), target: String::new(), is_merging: true };
    }
    let append = raw.get("append").and_then(Value::as_bool).unwrap_or(false);
    let from = str_field(raw, "fromStream").unwrap_or("Output");
    let operator = match (append, from) {
        (true, "Error") => "2>>",
        (true, "All") => "*>>",
        (true, _) => ">>",
        (false, "Error") => "2>",
        (false, "All") => "*>",
        (false, _) => ">",
    };
    Redir {
        operator: operator.to_string(),
        target: str_field(raw, "locationText").unwrap_or("").to_string(),
        is_merging: false,
    }
}

/// `Nhu` — transform a raw `CommandAst` element into a [`PsCommand`].
fn transform_command(raw: &Value) -> PsCommand {
    let elems = cae(raw.get("commandElements"));
    let mut name = String::new();
    let mut args: Vec<String> = Vec::new();
    let mut element_types: Vec<String> = Vec::new();

    if let Some((first, rest)) = elems.split_first() {
        let ftype = str_field(first, "type").unwrap_or("");
        let is_string =
            ftype == "StringConstantExpressionAst" || ftype == "ExpandableStringExpressionAst";
        let raw_name = if is_string {
            first.get("value").and_then(Value::as_str)
        } else {
            None
        }
        .unwrap_or_else(|| str_field(first, "text").unwrap_or(""));
        let f = strip_one_quote(raw_name);
        name = yve(strip_module_path(f));
        element_types.push(br_map(ftype, str_field(first, "expressionType")));

        for g in rest {
            let gtype = str_field(g, "type").unwrap_or("");
            let is_str =
                gtype == "StringConstantExpressionAst" || gtype == "ExpandableStringExpressionAst";
            let text = if is_str {
                g.get("value").and_then(Value::as_str)
            } else {
                None
            }
            .unwrap_or_else(|| str_field(g, "text").unwrap_or(""));
            args.push(yve(text));
            element_types.push(br_map(gtype, str_field(g, "expressionType")));
        }
    }

    let redirections = cae(raw.get("redirections"))
        .iter()
        .map(|r| {
            let t = transform_redirection(r);
            PsRedirection { target: t.target, is_merging: t.is_merging }
        })
        .collect();

    PsCommand { name, args, element_types, redirections }
}

/// `ufg` — transform a raw statement into a [`PsStatement`].
fn transform_statement(raw: &Value) -> PsStatement {
    let mut commands: Vec<PsElement> = Vec::new();
    let mut redirs: Vec<Redir> = Vec::new();

    let elements = cae(raw.get("elements"));
    if elements.is_empty() {
        // No pipeline elements → a single CommandExpression node (claude-code's
        // `else` arm) + the statement's redirections.
        commands.push(PsElement::Expression {
            text: yve(str_field(raw, "text").unwrap_or("")),
        });
        for r in cae(raw.get("redirections")) {
            redirs.push(transform_redirection(r));
        }
    } else {
        for l in &elements {
            if str_field(l, "type") == Some("CommandAst") {
                commands.push(PsElement::Command(transform_command(l)));
            } else {
                // `cfg` — a non-CommandAst element (expression / paren): a pipeline
                // source, kept as an Expression carrying its text.
                commands.push(PsElement::Expression {
                    text: yve(str_field(l, "text").unwrap_or("")),
                });
            }
            for c in cae(l.get("redirections")) {
                redirs.push(transform_redirection(c));
            }
        }
        // Append statement-level redirections, deduped on (operator, target).
        let mut seen: std::collections::HashSet<(String, String)> = redirs
            .iter()
            .map(|r| (r.operator.clone(), r.target.clone()))
            .collect();
        for r in cae(raw.get("redirections")) {
            let c = transform_redirection(r);
            let key = (c.operator.clone(), c.target.clone());
            if seen.insert(key) {
                redirs.push(c);
            }
        }
    }

    let nested_commands = cae(raw.get("nestedCommands"))
        .iter()
        .map(|c| transform_command(c))
        .collect();

    PsStatement {
        commands,
        nested_commands,
        redirections: redirs
            .into_iter()
            .map(|r| PsRedirection { target: r.target, is_merging: r.is_merging })
            .collect(),
    }
}

/// Parse the `pwsh` JSON output into a [`ParseResult`] (the pure half). Returns
/// `valid=false` (→ passthrough) when the JSON is malformed or reports parse
/// errors. `statements` are the transformed [`PsStatement`]s.
#[must_use]
pub fn parse_ps_ast_json(json: &str) -> ParseResult {
    let Ok(root) = serde_json::from_str::<Value>(json) else {
        return ParseResult::default();
    };
    let valid = root.get("valid").and_then(Value::as_bool).unwrap_or(false);
    if !valid {
        return ParseResult { valid: false, statements: Vec::new() };
    }
    let statements = cae(root.get("statements"))
        .iter()
        .map(|s| transform_statement(s))
        .collect();
    ParseResult { valid: true, statements }
}

#[cfg(test)]
#[path = "powershell_parse_test.rs"]
mod powershell_parse_test;
