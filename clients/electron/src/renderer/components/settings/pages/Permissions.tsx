import { useEffect, useState } from 'react';
import type { ClientCommand, PermissionBehaviorDto, SettingsDestinationDto } from '@lingxi/bridge-client';
import { Card, FieldProvenanceNotice, ProvenanceBadge, Row, type Provenance } from '../rows';
import { useT } from '../../../theme/ThemeContext';
import type { PageContentProps } from '../SettingsScreen';
import { rowState, type SettingsSnapshot } from '../useEngineSettings';
import { ghostButtonStyle, inputStyle } from './ghostButton';

/**
 * `permissions.{allow,deny,ask,defaultMode,additionalDirectories}` as
 * `editingLayer`'s OWN raw value — never `snapshot.effective`. Same fix as
 * `CustomProviders.providersFromLayer`: `permissions` is a `DeepMerge` key
 * (`core/src/settings/schema.rs`'s `MERGE_STRATEGIES`), so `effective` is
 * a cross-layer merge that would fork other layers' rules into whichever
 * layer this page saves to if it were used as the write-time base.
 *
 * This page never actually needs to WRITE the whole object back, though —
 * see `capturePermissionEdit` below — but the DISPLAY of "what does this
 * layer itself currently say" still has to come from here, not `effective`,
 * or a rule struck from `project` would still visually appear to be in
 * `user`'s list.
 */
export interface PermissionsDraft {
  allow: string[];
  deny: string[];
  ask: string[];
  defaultMode?: string;
  additionalDirectories: string[];
}

function stringArray(value: unknown): string[] {
  return Array.isArray(value) ? value.filter((entry): entry is string => typeof entry === 'string') : [];
}

export function permissionsFromLayer(snapshot: SettingsSnapshot | null, layer: string): PermissionsDraft {
  const value = snapshot?.layers?.[layer]?.['permissions'];
  const obj = value && typeof value === 'object' && !Array.isArray(value) ? (value as Record<string, unknown>) : {};
  return {
    allow: stringArray(obj['allow']),
    deny: stringArray(obj['deny']),
    ask: stringArray(obj['ask']),
    defaultMode: typeof obj['defaultMode'] === 'string' ? (obj['defaultMode'] as string) : undefined,
    additionalDirectories: stringArray(obj['additionalDirectories']),
  };
}

export interface PermissionRuleEdit {
  destination?: SettingsDestinationDto;
  behavior: PermissionBehaviorDto;
  add?: string[];
  remove?: string[];
}

/** The exact wire shape `capturePermissionEdit` returns — narrowed (not the full `ClientCommand` union) so the component can destructure `destination`/`behavior`/`add`/`remove` off it without a cast. */
export type PermissionRuleCommand = Extract<ClientCommand, { type: 'update_permission_rules' }>;

/**
 * The routing decision this task exists to pin (Task 18 brief's Step 1
 * test, verbatim shape). A permission-rule edit is ALWAYS an
 * `update_permission_rules` command, never `update_settings` —
 * `apply_patch` refuses the `permissions` key outright
 * (`settings_bridge.rs`'s `RESERVED_KEYS`) and points at this dedicated
 * command instead, so there is no generic-patch fallback to accidentally
 * reach for. Parsing on the engine side is INFALLIBLE
 * (`PermissionRuleValue::from_rule_string` degrades malformed input to a
 * bare tool name, matching claude-code) — this function does not validate
 * or reject the rule string either; it only shapes the command.
 *
 * **Task 18 fix round 1, Important**: `handleAddRule`/`handleRemoveRule`
 * below now build their command through THIS function instead of calling
 * `bridge.updatePermissionRules` with inline arguments — before this fix
 * the function existed only for the test to call, so a regression that
 * routed a rule edit through `updateEngineSettings` instead would have left
 * `settings-coding-pages.test.ts` green. Routing the real write path
 * through it makes the pin load-bearing.
 */
export function capturePermissionEdit(edit: PermissionRuleEdit): PermissionRuleCommand {
  return {
    type: 'update_permission_rules',
    destination: edit.destination ?? 'user',
    behavior: edit.behavior,
    add: edit.add ?? [],
    remove: edit.remove ?? [],
  };
}

/**
 * `bypassPermissionsModeAccepted` lives in the Electron store
 * (`clients/electron/src/main/settings.ts`), not in any engine settings
 * file — it never flows through `snapshot.layers`/`rowState`. Pinned as its
 * own function (rather than inlined at the call site) so a future edit that
 * tries to make the bypass row follow `editingLayer` breaks a test instead
 * of shipping quietly.
 */
export function bypassRowProvenance(): Provenance {
  return 'device';
}

/**
 * `persist_permission_mode` (`permission/src/persist.rs`) accepts exactly
 * `"default" | "acceptEdits" | "plan" | "dontAsk" | "auto"` and always
 * refuses `"bypassPermissions"` (`Ok(false)`, a deliberate security
 * property — see `bypassRefused` below). `auto` was missing from this list
 * in the first cut of this page (Task 18 fix round 1, Minor) — a user with
 * `auto` already in their file saw a blank `<select>`, and had no way to
 * pick it from here either.
 */
const DEFAULT_MODE_OPTIONS: { id: string; label: string; danger?: boolean }[] = [
  { id: 'default', label: '默认（每次询问）' },
  { id: 'plan', label: 'Plan（只读探索）' },
  { id: 'acceptEdits', label: '自动接受编辑' },
  { id: 'dontAsk', label: "Don't Ask（拒绝未显式允许的操作）" },
  { id: 'auto', label: 'Auto（分类器驱动的自动批准，遇到风险时询问）' },
  { id: 'bypassPermissions', label: 'Bypass Permissions（完全放行）', danger: true },
];

const BEHAVIOR_SECTIONS: { key: 'allow' | 'deny' | 'ask'; title: string; desc: string }[] = [
  { key: 'allow', title: '允许 (allow)', desc: '匹配的工具调用无需询问，直接放行。' },
  { key: 'deny', title: '拒绝 (deny)', desc: '匹配的工具调用直接拒绝，不会询问。' },
  { key: 'ask', title: '询问 (ask)', desc: '匹配的工具调用总是询问，即使默认模式会自动放行。' },
];

function RuleSection({
  title, desc, rules, saving, onAdd, onRemove,
}: {
  title: string; desc: string; rules: string[]; saving: boolean;
  onAdd(rule: string): void; onRemove(rule: string): void;
}) {
  const t = useT();
  const [draft, setDraft] = useState('');
  return (
    <Card title={title}>
      <div style={{ padding: '10px 18px 0', fontSize: 12, color: t.text3 }}>{desc}</div>
      {rules.length === 0 && (
        <div style={{ padding: '10px 18px 14px', color: t.text4, fontSize: 12.5 }}>还没有规则。</div>
      )}
      {rules.map((rule) => (
        <Row key={rule} align="center" title={<span className="mono" style={{ fontSize: 12.5 }}>{rule}</span>}>
          <button type="button" disabled={saving} onClick={() => onRemove(rule)} style={ghostButtonStyle(t, saving, true)}>移除</button>
        </Row>
      ))}
      <Row title="新增规则" align="center">
        <div style={{ display: 'flex', gap: 7 }}>
          <input
            value={draft}
            onChange={(e) => setDraft(e.target.value)}
            placeholder="例如 Bash(ls:*)"
            aria-label={`新增${title}规则`}
            style={{ ...inputStyle(t), width: 220 }}
          />
          <button
            type="button"
            disabled={saving || !draft.trim()}
            onClick={() => { onAdd(draft.trim()); setDraft(''); }}
            style={ghostButtonStyle(t, saving || !draft.trim())}
          >
            添加
          </button>
        </div>
      </Row>
    </Card>
  );
}

/**
 * Permissions never goes through the generic settings patch — see
 * `capturePermissionEdit` above and `bridge.updatePermissionRules` /
 * `setDefaultPermissionMode` / `updateWorkspaceDirectories`
 * (`useBridge.ts`), which wrap the three dedicated commands
 * (`client-protocol/src/commands.rs`, Task 5). Each write targets
 * `editingLayer` explicitly as its `destination` and is an ADD/REMOVE
 * DELTA against that layer's own file — `permission::persist_permission_rule_set`
 * does the locking/atomic-replace/alias-de-duplication, so this page never
 * needs to read-modify-write a whole array itself (unlike
 * `CustomProviders`' `settings.providers`).
 */
export function Permissions({ bridge, snapshot, editingLayer, onJumpToLayer }: PageContentProps) {
  const t = useT();
  const draft = permissionsFromLayer(snapshot, editingLayer);
  const permissionsRowState = snapshot ? rowState(snapshot, 'permissions', editingLayer) : null;

  const [ruleSaving, setRuleSaving] = useState(false);
  const [ruleError, setRuleError] = useState<string | null>(null);
  const [modeSaving, setModeSaving] = useState(false);
  const [modeError, setModeError] = useState<string | null>(null);
  const [attemptedMode, setAttemptedMode] = useState<string | null>(null);
  const [dirDraft, setDirDraft] = useState('');
  const [dirSaving, setDirSaving] = useState(false);
  const [dirError, setDirError] = useState<string | null>(null);

  // A write that lands in a DIFFERENT layer than the one now selected must
  // not be echoed as if it just landed here — reset the "did the mode
  // change take" tracker whenever the edited layer changes.
  useEffect(() => { setAttemptedMode(null); }, [editingLayer]);

  const handleAddRule = (behavior: PermissionBehaviorDto, rule: string) => {
    const command = capturePermissionEdit({ destination: editingLayer, behavior, add: [rule] });
    setRuleSaving(true);
    setRuleError(null);
    void bridge.updatePermissionRules(command.destination, command.behavior, command.add, command.remove)
      .catch((cause) => setRuleError(cause instanceof Error ? cause.message : '无法保存权限规则。'))
      .finally(() => setRuleSaving(false));
  };

  const handleRemoveRule = (behavior: PermissionBehaviorDto, rule: string) => {
    const command = capturePermissionEdit({ destination: editingLayer, behavior, remove: [rule] });
    setRuleSaving(true);
    setRuleError(null);
    void bridge.updatePermissionRules(command.destination, command.behavior, command.add, command.remove)
      .catch((cause) => setRuleError(cause instanceof Error ? cause.message : '无法移除权限规则。'))
      .finally(() => setRuleSaving(false));
  };

  const handleSetMode = (mode: string) => {
    setModeSaving(true);
    setModeError(null);
    setAttemptedMode(mode);
    void bridge.setDefaultPermissionMode(editingLayer, mode)
      .catch((cause) => setModeError(cause instanceof Error ? cause.message : '无法保存默认权限模式。'))
      .finally(() => setModeSaving(false));
  };

  const handleAddDirectory = () => {
    const dir = dirDraft.trim();
    if (!dir) return;
    setDirSaving(true);
    setDirError(null);
    void bridge.updateWorkspaceDirectories(editingLayer, [dir], [])
      .then(() => setDirDraft(''))
      .catch((cause) => setDirError(cause instanceof Error ? cause.message : '无法保存工作目录。'))
      .finally(() => setDirSaving(false));
  };

  const handleRemoveDirectory = (dir: string) => {
    setDirSaving(true);
    setDirError(null);
    void bridge.updateWorkspaceDirectories(editingLayer, [], [dir])
      .catch((cause) => setDirError(cause instanceof Error ? cause.message : '无法移除工作目录。'))
      .finally(() => setDirSaving(false));
  };

  // Grounded refusal notice for the `bypassPermissions` special case
  // (constraint #5): rather than guessing from promise resolution (the
  // command's promise resolves either way — the refusal surfaces as an
  // engine EVENT, not a rejected promise), compare what was actually
  // requested against what the refreshed snapshot now shows. If they still
  // differ after a save attempt, the write did not take — which is exactly
  // what `persist_permission_mode` promises for this one mode.
  const bypassRefused = attemptedMode === 'bypassPermissions' && !modeSaving && draft.defaultMode !== 'bypassPermissions';

  return (
    <>
      {(permissionsRowState?.kind === 'merged' || permissionsRowState?.kind === 'overridden') && (
        <Card title="生效层">
          <FieldProvenanceNotice snapshot={snapshot} fieldKey="permissions" editingLayer={editingLayer} onJumpToLayer={onJumpToLayer} label="permissions" />
        </Card>
      )}

      {BEHAVIOR_SECTIONS.map((section) => (
        <RuleSection
          key={section.key}
          title={section.title}
          desc={section.desc}
          rules={draft[section.key]}
          saving={ruleSaving}
          onAdd={(rule) => handleAddRule(section.key, rule)}
          onRemove={(rule) => handleRemoveRule(section.key, rule)}
        />
      ))}
      {ruleError && <div role="alert" style={{ color: t.danger, fontSize: 12, marginTop: -18, marginBottom: 18 }}>{ruleError}</div>}

      <Card title="默认权限模式">
        <Row title="defaultMode" desc="没有任何规则匹配时，如何处理工具调用。" align="center">
          <select
            value={draft.defaultMode ?? 'default'}
            disabled={modeSaving}
            onChange={(e) => handleSetMode(e.target.value)}
            aria-label="默认权限模式"
            style={inputStyle(t)}
          >
            {DEFAULT_MODE_OPTIONS.map((opt) => <option key={opt.id} value={opt.id}>{opt.label}</option>)}
          </select>
        </Row>
        {bypassRefused && (
          <Row title="未保存" align="center">
            <span role="alert" style={{ color: t.warn, fontSize: 12 }}>
              Bypass Permissions 无法从设置页持久化——这是安全设计，不是 bug；每次激活都需要单独在弹窗中确认。
            </span>
          </Row>
        )}
        {modeError && <Row title="错误" align="center"><span role="alert" style={{ color: t.danger, fontSize: 12 }}>{modeError}</span></Row>}
      </Card>

      <Card title="附加工作目录 (additionalDirectories)">
        {draft.additionalDirectories.length === 0 && (
          <div style={{ padding: '14px 18px', color: t.text4, fontSize: 12.5 }}>还没有附加目录。</div>
        )}
        {draft.additionalDirectories.map((dir) => (
          <Row key={dir} align="center" title={<span className="mono" style={{ fontSize: 12.5 }}>{dir}</span>}>
            <button type="button" disabled={dirSaving} onClick={() => handleRemoveDirectory(dir)} style={ghostButtonStyle(t, dirSaving, true)}>移除</button>
          </Row>
        ))}
        <Row title="新增目录" align="center">
          <div style={{ display: 'flex', gap: 7 }}>
            <input
              value={dirDraft}
              onChange={(e) => setDirDraft(e.target.value)}
              placeholder="路径"
              aria-label="新增附加工作目录"
              style={{ ...inputStyle(t), width: 260 }}
            />
            <button type="button" disabled={dirSaving || !dirDraft.trim()} onClick={handleAddDirectory} style={ghostButtonStyle(t, dirSaving || !dirDraft.trim())}>添加</button>
          </div>
        </Row>
        {dirError && <Row title="错误" align="center"><span role="alert" style={{ color: t.danger, fontSize: 12 }}>{dirError}</span></Row>}
      </Card>

      <Card title="设备状态">
        <Row
          title="Bypass Permissions 已确认"
          badge={<ProvenanceBadge destination={bypassRowProvenance()} />}
          desc="这条状态存在 Electron 本地存储中，与设置层无关——切换上方的层不会改变它。"
          align="center"
        >
          <span style={{ fontSize: 12.5, color: t.text2 }}>
            {bridge.bootstrap?.settings.bypassPermissionsModeAccepted ? '是' : '否'}
          </span>
        </Row>
      </Card>
    </>
  );
}
