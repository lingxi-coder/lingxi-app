# Parity Loop Prompt — Design

**Date:** 2026-06-28
**Purpose:** A reusable `/loop` driving prompt for sustaining byte-level 1:1 parity
between the LingXi Rust port and the latest claude-code, while never touching this
project's intentional divergences.

## Goal

Drive an autonomous `/loop` that checks and fixes every divergence from latest
claude-code, module by module, until each target module is **byte-level identical** —
except for code that is intentionally project-specific.

## Design decisions

- **Iteration model — rotating single-module deep-dive.** Each tick focuses on
  exactly one module, takes it to full parity via adversarial verification, then
  advances to the next. Modules already at 1:1 drop to periodic regression checks;
  do not rework aligned parts.
- **Prompt content — goals + constraints only.** The prompt states the goal, the
  rotation list, and the project-specific carve-outs. The full methodology
  (Workflow/Ultracode adversarial verify, `.worktree` + ff-merge flow, package-name
  pitfalls, ScheduleWakeup re-arm) lives in CLAUDE.md and memory, not the prompt.
- **One guardrail kept inline.** The single highest-value correctness lesson — judge
  divergences against the real binary (`strings` dump oracle), not the possibly-stale
  `claude-code/src` TS — is stated as a constraint because mis-trusting stale TS has
  already caused wrong refutations.
- **Language — Chinese.**

## Module rotation order (priority)

session → turn-loop → agent/subagent → memory → compact → permission → hook →
workflow → tasks → mcp → plugin → skills → message queue → orchestrator → lsp →
cost → file → web → coordinate

## Project-specific carve-outs (never "fix" to match claude)

- Multi-LLM provider routing — must not be narrowed to Anthropic-only.
- Project naming rebrand (Claude Code → LingXi; symbols/class names).
- Config & storage paths (`.claude` → `.lingxi`), `LINGXI_` env vars.
- Intentional anti-parity decisions: auto-mode classifier (off), ContextCollapse /
  +500k token budget.
- Retained Anthropic-specific identifiers (BedrockClaude*, ClaudeAi*, tengu_*).

## The prompt (verbatim)

> 检查并修复本项目 Rust 移植与 latest claude code 之间的所有差异，目标是 **byte-level 1:1**。
>
> **范围（轮转单模块深挖）**：每个 tick 只聚焦一个模块，用对抗式验证把它做到与 latest claude 完全一致后，再轮到下一个。优先顺序：
> session → turn-loop → agent/subagent → memory → compact → permission → hook → workflow → tasks → mcp → plugin → skills → message queue → orchestrator → lsp → cost → file → web → coordinate。
> 完成 1:1 的模块降级为定期回归检查，不要反复重做已对齐的部分。
>
> **不得改动的项目特有代码（这些是 1:1 的例外，永远不要"修复"成 claude 原样）**：
> - 多 LLM provider 路由 —— 不能改成只支持 Anthropic；
> - 项目命名 rebrand（Claude Code → LingXi，符号/类名等）；
> - 配置与存储路径（`.claude` → `.lingxi`）、`LINGXI_` 环境变量；
> - 故意的反-parity 决策：auto-mode classifier（关闭）、ContextCollapse / +500k token budget；
> - 已保留的 Anthropic 专有标识（BedrockClaude*、ClaudeAi*、tengu_*）。
>
> **护栏**：判定差异时以真实二进制为准（oracle = `strings` dump），不要相信可能过时的 `claude-code/src` TS 源码。
