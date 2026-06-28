# Parity Loop Prompt — Design

**Date:** 2026-06-28
**Purpose:** A reusable `/loop` driving prompt for sustaining byte-level 1:1 parity
between the LingXi Rust port and the latest claude-code, while never touching this
project's intentional divergences.

## Goal

Drive an autonomous `/loop` that checks and fixes every divergence from latest
claude-code, module by module, until each target module is **byte-level identical**
on byte-lockable surfaces — except for code that is intentionally project-specific.

## Design decisions

- **Per-tick preflight.** Every tick first locates the real claude binary and records
  `claude --version`, the binary path, and the strings-dump path. If the real binary
  is unavailable, the tick **stops and reports a blocker** — it must not fall back to
  `claude-code/src` (possibly-stale TS) as the byte oracle. This folds in the
  highest-value correctness lesson (judge against the binary, never stale TS).
- **State file `.omx/state/parity-loop.json`.** Each tick maintains the current
  module, oracle version, completed evidence, remaining divergences, and the next
  tick, so the rotation survives restarts and hand-offs.
- **Iteration model — rotating single-module deep-dive.** Each tick focuses on
  exactly one module, takes it to full parity via adversarial verification, then
  advances to the next. Modules already at 1:1 drop to periodic regression checks;
  do not rework aligned parts.
- **Byte-level boundary.** Pursue byte-level only on byte-lockable surfaces — CLI,
  protocol, error strings, telemetry, file formats. For explicitly carved-out
  branding/paths/provider-routing and for non-deterministic model output, pursue only
  observable parity (behavior-equivalent), not byte equality.
- **Prompt content — goals + constraints only.** The prompt states the goal, the
  rotation list, and the project-specific carve-outs. The fuller methodology
  (Workflow/Ultracode adversarial verify, `.worktree` + ff-merge flow, package-name
  pitfalls, ScheduleWakeup re-arm) lives in CLAUDE.md and memory, not the prompt.
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
> **每 tick 先做 preflight**：定位真实 claude binary，记录 `claude --version`、binary 路径、strings dump 路径。若真实 binary 不可用，**停止并报告 blocker**，不得用 `claude-code/src` 作为 byte oracle（TS 可能过时）。
>
> **状态文件 `.omx/state/parity-loop.json`**：每 tick 维护当前模块、oracle 版本、已完成证据、剩余差异、下一 tick。
>
> **范围（轮转单模块深挖）**：每个 tick 只聚焦一个模块，用对抗式验证把它做到与 latest claude 完全一致后，再轮到下一个。优先顺序：
> session → turn-loop → agent/subagent → memory → compact → permission → hook → workflow → tasks → mcp → plugin → skills → message queue → orchestrator → lsp → cost → file → web → coordinate。
> 完成 1:1 的模块降级为定期回归检查，不要反复重做已对齐的部分。
>
> **byte-level 的边界**：只在可字节锁定的表面追求 byte-level —— CLI、协议、错误字符串、遥测、文件格式。对明确 carve-out 的品牌/路径/provider routing，以及非确定性的模型输出，只追求 observable parity（行为可观察一致即可），不追字节。
>
> **不得改动的项目特有代码（永远不要"修复"成 claude 原样）**：
> - 多 LLM provider 路由 —— 不能改成只支持 Anthropic；
> - 项目命名 rebrand（Claude Code → LingXi，符号/类名等）；
> - 配置与存储路径（`.claude` → `.lingxi`）、`LINGXI_` 环境变量；
> - 故意的反-parity 决策：auto-mode classifier（关闭）、ContextCollapse / +500k token budget；
> - 已保留的 Anthropic 专有标识（BedrockClaude*、ClaudeAi*、tengu_*）。
