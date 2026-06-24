# Stream-JSON Control Protocol Phase 3b — set_permission_mode (live mode mutation)

The net-new runtime mode-mutation surface (§2.2 #4 / §5.6). The active mode was
baked into `PolicyPermissionGate` at boot behind an immutable `Arc`; this makes
it live-settable, mirroring claude-code reading `toolPermissionContext.mode` on
every check.

## Vertical slice (4 crates)
- **traits** (`permission_gate.rs`): new `PermissionGate::set_permission_mode(&self, mode: &str) -> Result<(), String>`, default no-op `Ok(())` (additive-defaulted, frozen-trait safe). `&str` not `PermissionMode` because `traits` is below `lingxi-permission` in the dep graph.
- **permission** (`policy_gate.rs`): `PolicyPermissionGate` gains `mode_override: RwLock<Option<PermissionMode>>` + `effective_authorize` (override mode when set, else boot mode) — used by `check`/`check_with_worker`/`check_after_hook_allow`/`resolve_detailed`. `check_in_plan_mode` still force-uses Plan (the EnterPlanMode flag wins). Override impl parses via `parse_settable_mode` (six modes incl. `auto`; unknown ⇒ `Err`), rejects `bypassPermissions` when `policy.bypass_killswitch_active`.
- **orchestrator** (`conversation.rs`): `ConversationOrchestrator::set_permission_mode(mode) → self.perms.set_permission_mode(mode)`.
- **cli** (`run.rs`): `set_permission_mode` arm → `orchestrator.set_permission_mode(mode)`; success echoes `{mode}`, error ⇒ error frame.

## Tests — 4 (policy_gate): **30 policy_gate tests pass**
override flips outcome (Default Ask→delegate vs bypassPermissions→Allow, inner
not consulted); unknown mode → `Invalid permission mode: <x>`; bypass+killswitch
→ `Bypass permissions mode was disabled by settings`; `parse_settable_mode` table.

## Notes / faithful approximations
- Error strings (`Invalid permission mode: …`) are reasonable, NOT byte-dumped
  from the binary (the spec listed the error *families*, not literals); the
  bypass-disabled string reuses the byte-confirmed CLI notice
  (`initial_permission_mode_from_cli`).
- `set_permission_mode` and the session `plan_mode` flag remain separate cells;
  when both are active the plan_mode flag wins via `check_in_plan_mode` (the
  binary unifies them under one `toolPermissionContext.mode`). Documented.
- The `ultraplan` request field is ignored (first pass).
