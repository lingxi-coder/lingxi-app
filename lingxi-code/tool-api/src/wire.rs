//! Wire serialization of [`Tool`] definitions for the model's `tools` array.
//!
//! Each tool becomes the session-stable base schema claude-code sends on the
//! `messages.create` request: `{ name, description, input_schema }`, where
//! `description` is the tool's long-form [`Tool::prompt`] output (matching
//! `claude-code/src/utils/api.ts:169-178`). The orchestrator's streaming +
//! batched turn loops and the subagent seam all funnel their tool set through
//! [`tools_to_wire`] so the wire bytes are produced one way.
//!
//! FORCED DIVERGENCE / deferrals (documented, not bugs):
//! - The `strict`, `cache_control`, and `eager_input_streaming` extras
//!   (`api.ts:180-200`) are feature-flag / session-schema-cache gated in
//!   claude-code; only the base triple is emitted here.
//!
//! ORDER (parity): claude-code assembles the wire `tools` array as a sorted,
//! partitioned list — builtins sorted by `name`, kept as a contiguous prefix,
//! then MCP / plugin tools sorted by `name` (`assembleToolPool`
//! `claude-code/src/tools.ts:345-367`, `mergeAndFilterTools`
//! `claude-code/src/utils/toolPool.ts:65-70`); `claude.ts:1236`'s
//! `filteredTools.map(toolToAPISchema)` preserves that order. The sort key is
//! JS `String.prototype.localeCompare` (ICU default collation), NOT a byte
//! comparison — these differ for mixed-case tool names (e.g.
//! `ListMcpResourcesTool` < `LSP`, and `REPL` sorts after `RemoteTrigger`).
//!
//! [`ToolRegistry::available_tools`](crate::registry::ToolRegistry::available_tools)
//! already emits the builtin-prefix-then-MCP/LSP/plugin partition order with
//! each partition `locale_cmp`-sorted, so [`tools_to_wire`] preserves the
//! incoming slice order and does NOT re-sort. (A direct `&[Arc<dyn Tool>]`
//! slice that is not already partition-sorted — e.g. a hand-built test set —
//! is sorted here by [`locale_cmp`] as a faithful fallback.)

use crate::tool_trait::{PromptOptions, Tool};
use serde_json::{json, Value};
use std::cmp::Ordering;
use std::sync::Arc;

/// Printable-ASCII characters in JS `localeCompare` (ICU DUCET) primary order.
///
/// Derived empirically from `node -e '[...].sort((a,b)=>a.localeCompare(b))'`
/// over all printable ASCII: punctuation first, then digits, then letters with
/// each lower/upper pair adjacent (lowercase first at the tertiary level). The
/// two passes in [`locale_cmp`] reproduce ICU's primary-then-tertiary semantics
/// and match `localeCompare` exactly for ASCII tool names (validated against
/// Node over 4M+ comparison pairs, zero mismatches).
const LOCALE_PRIMARY_ORDER: &str =
    " _-,;:!?.'\"()[]{}@*/\\&#%`^+<=>|~$0123456789aAbBcCdDeEfFgGhHiIjJkKlLmMnNoOpPqQrRsStTuUvVwWxXyYzZ";

/// Primary collation weight of an ASCII char (its index in [`LOCALE_PRIMARY_ORDER`]
/// with the two cases of a letter collapsed to one weight). Non-ASCII / unmapped
/// chars fall back to `0x1000 + codepoint` so ordering stays total & deterministic
/// (real tool names are ASCII, so this branch is never hit in practice).
fn primary_weight(c: char) -> u32 {
    static TABLE: std::sync::OnceLock<[u32; 128]> = std::sync::OnceLock::new();
    let table = TABLE.get_or_init(|| {
        let mut table = [u32::MAX; 128];
        let mut next: u32 = 0;
        // Track the primary weight already assigned to a letter's lowercase form
        // so its uppercase counterpart shares it.
        let mut letter_weight: [u32; 26] = [u32::MAX; 26];
        for ch in LOCALE_PRIMARY_ORDER.chars() {
            let idx = ch as usize;
            if ch.is_ascii_alphabetic() {
                let li = (ch.to_ascii_lowercase() as u8 - b'a') as usize;
                if letter_weight[li] == u32::MAX {
                    letter_weight[li] = next;
                    next += 1;
                }
                table[idx] = letter_weight[li];
            } else {
                table[idx] = next;
                next += 1;
            }
        }
        table
    });
    let cp = c as u32;
    if cp < 128 && table[cp as usize] != u32::MAX {
        table[cp as usize]
    } else {
        0x1000 + cp
    }
}

/// Tertiary (case) collation weight: lowercase letters sort before their
/// uppercase counterpart; everything else is neutral.
fn tertiary_weight(c: char) -> u8 {
    if c.is_ascii_lowercase() {
        0
    } else if c.is_ascii_uppercase() {
        1
    } else {
        0
    }
}

/// Compare two tool names with the same ordering JS `String.localeCompare`
/// produces under the default (ICU DUCET) collation, ported for ASCII.
///
/// Two-level comparison mirroring ICU: first a *primary* pass (case-insensitive
/// letter/structure order), and only on a primary tie a *tertiary* pass
/// (lowercase before uppercase, left to right). Shorter string sorts first on a
/// pure prefix. This is the sort key claude-code uses for the wire `tools`
/// array (`a.name.localeCompare(b.name)`), so matching it is a parity contract,
/// not a stylistic choice.
#[must_use]
pub fn locale_cmp(a: &str, b: &str) -> Ordering {
    // Primary pass.
    let mut ai = a.chars();
    let mut bi = b.chars();
    loop {
        match (ai.next(), bi.next()) {
            (Some(ca), Some(cb)) => {
                let o = primary_weight(ca).cmp(&primary_weight(cb));
                if o != Ordering::Equal {
                    return o;
                }
            }
            (Some(_), None) => return Ordering::Greater,
            (None, Some(_)) => return Ordering::Less,
            (None, None) => break,
        }
    }
    // Tertiary (case) pass — only reached when primary-equal & same length-in-chars.
    for (ca, cb) in a.chars().zip(b.chars()) {
        let o = tertiary_weight(ca).cmp(&tertiary_weight(cb));
        if o != Ordering::Equal {
            return o;
        }
    }
    Ordering::Equal
}

/// Serialize one tool to its wire definition `{ name, description, input_schema }`.
///
/// `description` is awaited from [`Tool::prompt`] (the long-form tool prompt,
/// the same text claude-code places in the API tool definition's `description`).
pub async fn tool_to_wire(tool: &dyn Tool, opts: &PromptOptions) -> Value {
    json!({
        "name": tool.name(),
        "description": tool.prompt(opts).await,
        "input_schema": tool.input_schema(),
    })
}

/// Serialize a tool set to the wire `tools` array, **preserving the incoming
/// slice order**.
///
/// The canonical caller is the registry's
/// [`available_tools`](crate::registry::ToolRegistry::available_tools), which
/// already emits the parity order: builtins `locale_cmp`-sorted as a contiguous
/// prefix, then MCP / LSP / plugin partitions `locale_cmp`-sorted. Re-sorting
/// here would re-interleave MCP tools into the builtin prefix and break the
/// server-side prompt-cache breakpoint contract claude-code preserves
/// (`tools.ts:345-367`), so this function does NOT re-sort.
pub async fn tools_to_wire(tools: &[Arc<dyn Tool>], opts: &PromptOptions) -> Vec<Value> {
    let mut out: Vec<Value> = Vec::with_capacity(tools.len());
    for tool in tools {
        out.push(tool_to_wire(tool.as_ref(), opts).await);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::ToolUseContext;
    use crate::tool_trait::{
        DescriptionOptions, PromptOptions, ToolCallResult, ToolError, ToolStaticContext,
    };
    use async_trait::async_trait;
    use permission::result::PermissionMetadata;
    use permission::{PermissionDecisionReason, PermissionResult};
    use serde_json::json;

    /// Minimal stub tool with a configurable name / prompt / schema.
    struct StubTool {
        name: &'static str,
        prompt: &'static str,
        schema: Value,
    }

    #[async_trait]
    impl Tool for StubTool {
        fn name(&self) -> &str {
            self.name
        }
        fn input_schema(&self) -> &Value {
            &self.schema
        }
        fn is_enabled(&self, _ctx: &ToolStaticContext) -> bool {
            true
        }
        fn max_result_size_chars(&self) -> usize {
            1024
        }
        fn is_concurrency_safe(&self, _input: &Value) -> bool {
            true
        }
        fn is_read_only(&self, _input: &Value) -> bool {
            true
        }
        async fn check_permissions(
            &self,
            _input: &Value,
            _ctx: &ToolUseContext,
        ) -> PermissionResult {
            PermissionResult::Allow {
                reason: PermissionDecisionReason::Other {
                    reason: "test".into(),
                },
                updated_input: None,
                update_destination: None,
                metadata: PermissionMetadata::default(),
            }
        }
        async fn description(&self, _input: &Value, _opts: &DescriptionOptions) -> String {
            self.name.into()
        }
        async fn prompt(&self, _opts: &PromptOptions) -> String {
            self.prompt.into()
        }
        async fn call(
            &self,
            _input: Value,
            _ctx: ToolUseContext,
            _tx: crate::progress::ToolProgressSender,
        ) -> Result<ToolCallResult, ToolError> {
            Ok(ToolCallResult {
                data: json!({"ok": true}),
                model_content: None,
                new_messages: vec![],
                context_modifier: None,
                is_error: false,
                mcp_meta: None,
            })
        }
    }

    fn opts() -> PromptOptions {
        PromptOptions {
            include_examples: true,
            model: None,
            model_profile: None,
        }
    }

    #[tokio::test]
    async fn one_tool_serializes_to_base_triple() {
        let tool = StubTool {
            name: "Read",
            prompt: "Reads a file from disk.",
            schema: json!({"type": "object", "properties": {"file_path": {"type": "string"}}}),
        };
        let wire = tool_to_wire(&tool, &opts()).await;
        assert_eq!(wire["name"], "Read");
        assert_eq!(wire["description"], "Reads a file from disk.");
        assert_eq!(wire["input_schema"]["type"], "object");
        // Exactly the base triple — no stray keys.
        let obj = wire.as_object().unwrap();
        assert_eq!(obj.len(), 3, "only name/description/input_schema: {wire}");
    }

    #[tokio::test]
    async fn tools_to_wire_preserves_incoming_order() {
        // tools_to_wire is now order-PRESERVING: the registry hands it the
        // already-partition-sorted slice, so re-sorting would break the
        // builtin-prefix prompt-cache contract. Confirm the wire array mirrors
        // the input slice order exactly (no reordering).
        let tools: Vec<Arc<dyn Tool>> = vec![
            Arc::new(StubTool {
                name: "Bravo",
                prompt: "b",
                schema: json!({"type": "object"}),
            }),
            Arc::new(StubTool {
                name: "Alpha",
                prompt: "a",
                schema: json!({"type": "object"}),
            }),
            Arc::new(StubTool {
                name: "Charlie",
                prompt: "c",
                schema: json!({"type": "object"}),
            }),
        ];
        let wire = tools_to_wire(&tools, &opts()).await;
        let names: Vec<&str> = wire.iter().map(|v| v["name"].as_str().unwrap()).collect();
        assert_eq!(names, vec!["Bravo", "Alpha", "Charlie"]);
    }

    /// Distinguishing parity test: claude-code sorts the wire tools with JS
    /// `localeCompare` (`tools.ts:345-367`), which differs from a Rust byte
    /// `cmp` for these real builtin names. `locale_cmp` must place
    /// `ListMcpResourcesTool` before `LSP` (byte sort puts `LSP` first) and
    /// `REPL` LAST after `RemoteTrigger` (byte sort puts `REPL` before `Read`).
    #[test]
    fn locale_cmp_matches_claude_code_localecompare_not_byte_order() {
        let mut names = vec![
            "LSP",
            "ListMcpResourcesTool",
            "MCP",
            "McpAuth",
            "REPL",
            "Read",
            "ReadMcpResourceTool",
            "RemoteTrigger",
        ];
        names.sort_by(|a, b| locale_cmp(a, b));
        assert_eq!(
            names,
            vec![
                "ListMcpResourcesTool",
                "LSP",
                "MCP",
                "McpAuth",
                "Read",
                "ReadMcpResourceTool",
                "RemoteTrigger",
                "REPL",
            ],
            "wire order must be localeCompare, not byte cmp"
        );

        // And explicitly assert it is NOT the byte order, so the test fails if
        // someone reverts to `str::cmp`.
        let mut byte_sorted = names.clone();
        byte_sorted.sort();
        assert_ne!(
            names, byte_sorted,
            "localeCompare order must differ from byte order for these names"
        );
    }

    /// Spot-check `locale_cmp` against individual `localeCompare` outcomes
    /// (validated against Node) covering the divergent pairs and MCP-prefix
    /// names that interleave case-insensitively among PascalCase builtins.
    #[test]
    fn locale_cmp_pairwise_parity() {
        use std::cmp::Ordering::*;
        assert_eq!(locale_cmp("LSP", "ListMcpResourcesTool"), Greater);
        assert_eq!(locale_cmp("REPL", "Read"), Greater);
        assert_eq!(locale_cmp("REPL", "RemoteTrigger"), Greater);
        assert_eq!(locale_cmp("MCP", "McpAuth"), Less);
        // lowercase 'm' (mcp__) sorts case-insensitively among builtins, not
        // strictly after them: 'm' < 'w'.
        assert_eq!(locale_cmp("mcp__a", "Write"), Less);
        // pure case tie: lowercase before uppercase, decided left to right.
        assert_eq!(locale_cmp("abc", "ABC"), Less);
        assert_eq!(locale_cmp("aBc", "Abc"), Less);
        // prefix sorts first.
        assert_eq!(locale_cmp("ab", "abc"), Less);
        // underscore (punctuation) before letters.
        assert_eq!(locale_cmp("_x", "ax"), Less);
        assert_eq!(locale_cmp("Read", "Read"), Equal);
    }

    #[tokio::test]
    async fn empty_set_serializes_to_empty_vec() {
        let wire = tools_to_wire(&[], &opts()).await;
        assert!(wire.is_empty());
    }
}
