import { useI18n } from '../../../i18n/useI18n';
import { tr } from '../../../i18n/locale';
import { useEffect, useState } from 'react';
import type { ClientCommand, PermissionBehaviorDto, WritableScopeDto } from '@lingxi/bridge-client';
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
  destination?: WritableScopeDto;
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
 * (`apps/electron/src/main/settings.ts`), not in any engine settings
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
  { id: 'default', label: "desktop_settings_07f8a52190" },
  { id: 'plan', label: "desktop_settings_79505cb43f" },
  { id: 'acceptEdits', label: "settings_permission_mode_accept_edits" },
  { id: 'dontAsk', label: "desktop_settings_91c1cf103a" },
  { id: 'auto', label: "desktop_settings_ebd1340f07" },
  { id: 'bypassPermissions', label: "desktop_settings_ec6040b0a6", danger: true },
];

const BEHAVIOR_SECTIONS: { key: 'allow' | 'deny' | 'ask'; title: string; desc: string }[] = [
  { key: 'allow', title: "desktop_settings_c7f00c6c78", desc: "desktop_settings_000ad51c44" },
  { key: 'deny', title: "desktop_settings_93453f0e6a", desc: "desktop_settings_9cdc6bb497" },
  { key: 'ask', title: "desktop_settings_1735f05ca0", desc: "desktop_settings_a0333cee43" },
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
        <div style={{ padding: '10px 18px 14px', color: t.text4, fontSize: 12.5 }}>{tr('desktop_settings_69b6ed2dbd')}</div>
      )}
      {rules.map((rule) => (
        <Row key={rule} align="center" title={<span className="mono" style={{ fontSize: 12.5 }}>{rule}</span>}>
          <button type="button" disabled={saving} onClick={() => onRemove(rule)} style={ghostButtonStyle(t, saving, true)}>{tr('settings_parity_remove')}</button>
        </Row>
      ))}
      <Row title={tr('desktop_settings_83009fcfe6')} align="center">
        <div style={{ display: 'flex', gap: 7 }}>
          <input
            value={draft}
            onChange={(e) => setDraft(e.target.value)}
            placeholder={tr('desktop_settings_9c02f9652c')}
            aria-label={tr('desktop_settings_8df39a2f56', title)}
            style={{ ...inputStyle(t), width: 220 }}
          />
          <button
            type="button"
            disabled={saving || !draft.trim()}
            onClick={() => { onAdd(draft.trim()); setDraft(''); }}
            style={ghostButtonStyle(t, saving || !draft.trim())}
          >
            {tr('workspace_confirm_add')}
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
  useI18n();
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
      .catch((cause) => setRuleError(cause instanceof Error ? cause.message : tr('desktop_settings_4231766e96')))
      .finally(() => setRuleSaving(false));
  };

  const handleRemoveRule = (behavior: PermissionBehaviorDto, rule: string) => {
    const command = capturePermissionEdit({ destination: editingLayer, behavior, remove: [rule] });
    setRuleSaving(true);
    setRuleError(null);
    void bridge.updatePermissionRules(command.destination, command.behavior, command.add, command.remove)
      .catch((cause) => setRuleError(cause instanceof Error ? cause.message : tr('desktop_settings_45ebc4f38b')))
      .finally(() => setRuleSaving(false));
  };

  const handleSetMode = (mode: string) => {
    setModeSaving(true);
    setModeError(null);
    setAttemptedMode(mode);
    void bridge.setDefaultPermissionMode(editingLayer, mode)
      .catch((cause) => setModeError(cause instanceof Error ? cause.message : tr('desktop_settings_70b6b18a11')))
      .finally(() => setModeSaving(false));
  };

  const handleAddDirectory = () => {
    const dir = dirDraft.trim();
    if (!dir) return;
    setDirSaving(true);
    setDirError(null);
    void bridge.updateWorkspaceDirectories(editingLayer, [dir], [])
      .then(() => setDirDraft(''))
      .catch((cause) => setDirError(cause instanceof Error ? cause.message : tr('desktop_settings_037376edba')))
      .finally(() => setDirSaving(false));
  };

  const handleRemoveDirectory = (dir: string) => {
    setDirSaving(true);
    setDirError(null);
    void bridge.updateWorkspaceDirectories(editingLayer, [], [dir])
      .catch((cause) => setDirError(cause instanceof Error ? cause.message : tr('desktop_settings_93cde86488')))
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
        <Card title={tr('desktop_settings_ad25986d5b')}>
          <FieldProvenanceNotice snapshot={snapshot} fieldKey="permissions" editingLayer={editingLayer} onJumpToLayer={onJumpToLayer} label="permissions" />
        </Card>
      )}

      {BEHAVIOR_SECTIONS.map((section) => (
        <RuleSection
          key={section.key}
          title={tr(section.title)}
          desc={tr(section.desc)}
          rules={draft[section.key]}
          saving={ruleSaving}
          onAdd={(rule) => handleAddRule(section.key, rule)}
          onRemove={(rule) => handleRemoveRule(section.key, rule)}
        />
      ))}
      {ruleError && <div role="alert" style={{ color: t.danger, fontSize: 12, marginTop: -18, marginBottom: 18 }}>{ruleError}</div>}

      <Card title={tr('settings_parity_default_permission')}>
        <Row title="defaultMode" desc={tr('desktop_settings_d47c2477c5')} align="center">
          <select
            value={draft.defaultMode ?? 'default'}
            disabled={modeSaving}
            onChange={(e) => handleSetMode(e.target.value)}
            aria-label={tr('settings_parity_default_permission')}
            style={inputStyle(t)}
          >
            {DEFAULT_MODE_OPTIONS.map((opt) => <option key={opt.id} value={opt.id}>{tr(opt.label)}</option>)}
          </select>
        </Row>
        {bypassRefused && (
          <Row title={tr('desktop_settings_6a1be012c9')} align="center">
            <span role="alert" style={{ color: t.warn, fontSize: 12 }}>
              {tr('desktop_settings_7892426a18')}</span>
          </Row>
        )}
        {modeError && <Row title={tr('cron_error_section')} align="center"><span role="alert" style={{ color: t.danger, fontSize: 12 }}>{modeError}</span></Row>}
      </Card>

      <Card title={tr('desktop_settings_0c5726f11b')}>
        {draft.additionalDirectories.length === 0 && (
          <div style={{ padding: '14px 18px', color: t.text4, fontSize: 12.5 }}>{tr('desktop_settings_5b087a2d1b')}</div>
        )}
        {draft.additionalDirectories.map((dir) => (
          <Row key={dir} align="center" title={<span className="mono" style={{ fontSize: 12.5 }}>{dir}</span>}>
            <button type="button" disabled={dirSaving} onClick={() => handleRemoveDirectory(dir)} style={ghostButtonStyle(t, dirSaving, true)}>{tr('settings_parity_remove')}</button>
          </Row>
        ))}
        <Row title={tr('desktop_settings_7f55269f30')} align="center">
          <div style={{ display: 'flex', gap: 7 }}>
            <input
              value={dirDraft}
              onChange={(e) => setDirDraft(e.target.value)}
              placeholder={tr('desktop_settings_77e1ea5c56')}
              aria-label={tr('desktop_settings_dbe9f32480')}
              style={{ ...inputStyle(t), width: 260 }}
            />
            <button type="button" disabled={dirSaving || !dirDraft.trim()} onClick={handleAddDirectory} style={ghostButtonStyle(t, dirSaving || !dirDraft.trim())}>{tr('workspace_confirm_add')}</button>
          </div>
        </Row>
        {dirError && <Row title={tr('cron_error_section')} align="center"><span role="alert" style={{ color: t.danger, fontSize: 12 }}>{dirError}</span></Row>}
      </Card>

      <Card title={tr('desktop_settings_240cf246ac')}>
        <Row
          title={tr('desktop_settings_e387c90938')}
          badge={<ProvenanceBadge destination={bypassRowProvenance()} />}
          desc={tr('desktop_settings_6e5e2fce96')}
          align="center"
        >
          <span style={{ fontSize: 12.5, color: t.text2 }}>
            {bridge.bootstrap?.settings.bypassPermissionsModeAccepted ? tr('common_yes') : tr('common_no')}
          </span>
        </Row>
      </Card>
    </>
  );
}
