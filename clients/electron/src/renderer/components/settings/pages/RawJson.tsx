import { useState } from 'react';
import { Card, ProvenanceBadge, Row, provenanceLabel, type Provenance } from '../rows';
import { useT } from '../../../theme/ThemeContext';
import type { PageContentProps } from '../SettingsScreen';
import type { SettingsFile, SettingsSnapshot } from '../useEngineSettings';
import { ghostButtonStyle, inputStyle } from './ghostButton';

/**
 * The escape hatch for every settings field no dedicated page covers, and
 * — per the task brief this page was built from — **the single sanctioned
 * exception to I1** (`apply_patch`'s refusal of the `permissions` top-level
 * key on the generic patch path, `bridge-server/src/settings_bridge.rs`'s
 * `RESERVED_KEYS`). Everywhere else two write paths to one key is the
 * defect I1 exists to prevent; here the user is replacing the whole layer
 * file as text, so refusing the key would make the escape hatch unable to
 * escape.
 *
 * **Investigated write path (do not re-derive this by re-reading the
 * engine — it was checked, not assumed):** there is NO engine-side command
 * today that overwrites a whole settings-layer file. The only write path is
 * `ClientCommand::UpdateSettings` (`bridge.updateEngineSettings`), which is
 * a SHALLOW, TOP-LEVEL PATCH — `apply_patch` reads the layer's file, applies
 * `(key, Some(value) | None)` entries to the in-memory map, then rewrites
 * the whole file — and it refuses the ENTIRE batch if `permissions` (or any
 * future `RESERVED_KEYS` entry) appears in the patch at all, whether the key
 * is being set, changed, or deleted. There is no whole-file-replace command,
 * and no variant of `UpdateSettings` that is allowed to name `permissions`.
 * `permission::persist_permission_rule_set`'s per-destination locked writer
 * is the only thing that ever touches `permissions` on disk; nothing routes
 * this page's save through it.
 *
 * This page therefore does the most honest thing possible with what exists:
 * it computes a MINIMAL patch — only the top-level keys the saved text
 * actually adds, changes, or removes relative to what this layer's file
 * currently holds (`computeLayerPatch`) — and sends that through the same
 * `update_settings` command every other layered page uses. An edit that
 * never touches `permissions` saves normally. An edit that DOES change
 * `permissions` is genuinely refused by the engine today (the same
 * `RESERVED_KEYS` message every other page's dedicated-command doc already
 * points at), and this page surfaces that refusal rather than silently
 * dropping the key from what it sends or pretending the save worked
 * (`layersDiverge`, mirroring `Permissions.tsx`'s `bypassRefused` — the
 * command's promise resolves either way; `apply_patch`'s failure is an
 * engine EVENT, not a rejected promise, so the only trustworthy signal is
 * comparing what was asked for against what the refreshed snapshot shows).
 * Making the brief's exception REAL — one write path that can actually
 * persist a changed `permissions` block as text — needs an engine change
 * (either lifting `RESERVED_KEYS` for a whole-layer-replace command, or a
 * new command altogether) that was not made here; this was reported instead
 * of invented, per this task's own instructions.
 *
 * **A second, related gap found during the same investigation**: a layer
 * whose file fails to parse contributes an EMPTY map to
 * `SettingsSnapshot.layers` (`build_snapshot`'s documented fallback) — the
 * wire carries `SettingsFile.parse_error` (a message) but never the file's
 * actual broken bytes, and no IPC channel exists on the Electron side to
 * read an arbitrary settings file's raw text either. So this page cannot
 * literally display a broken file's malformed content for in-place editing.
 * What it CAN do, and does, is let a broken layer still be fixed: the
 * editor starts EMPTY (not `"{}"`, which would misrepresent an empty object
 * as what is actually on disk) with the parse error shown alongside, and
 * saving valid JSON replaces the broken file — the "survive and become
 * fixable" property the brief asks for, just not a round-trip of the exact
 * broken bytes.
 */
export function validateRawLayer(text: string): string | null {
  let parsed: unknown;
  try {
    parsed = JSON.parse(text);
  } catch (error) {
    return `not valid JSON: ${(error as Error).message}`;
  }
  if (parsed === null || typeof parsed !== 'object' || Array.isArray(parsed)) {
    return 'a settings layer must be a JSON object';
  }
  return null;
}

/**
 * What the editor should start showing for `layer`. A layer whose file
 * parsed cleanly shows its own raw map, pretty-printed. A layer whose file
 * is BROKEN starts empty rather than `"{}"` — see this module's doc comment
 * for why the actual broken bytes are not available to show instead.
 */
export function initialLayerText(snapshot: SettingsSnapshot | null, layer: string, broken: boolean): string {
  if (broken) return '';
  return JSON.stringify(snapshot?.layers?.[layer] ?? {}, null, 2);
}

/**
 * The smallest `update_settings` patch (top-level key → new value, or
 * `null` to delete) that turns `oldLayer` (the layer's own current raw map)
 * into `newLayer` (the parsed text about to be saved).
 *
 * Diffing — rather than resending every key in `newLayer` verbatim — is not
 * an optimization, it is what makes this page usable at all on a layer that
 * also sets `permissions`: `apply_patch` refuses the WHOLE batch if any key
 * in it is `permissions`, regardless of whether that key's value actually
 * changed. Naming every top-level key on every save would mean a layer that
 * happens to contain `permissions` could never save ANYTHING through this
 * page, even a one-line unrelated edit. Only a key the saved text actually
 * added, changed, or removed belongs in the patch; an untouched
 * `permissions` block must never be re-asserted.
 */
export function computeLayerPatch(
  oldLayer: Record<string, unknown>,
  newLayer: Record<string, unknown>,
): Record<string, unknown | null> {
  const patch: Record<string, unknown | null> = {};
  for (const key of Object.keys(newLayer)) {
    if (JSON.stringify(newLayer[key]) !== JSON.stringify(oldLayer[key])) {
      patch[key] = newLayer[key];
    }
  }
  for (const key of Object.keys(oldLayer)) {
    if (!(key in newLayer)) patch[key] = null;
  }
  return patch;
}

/**
 * Whether `a` and `b` disagree on any top-level key's value. The grounded
 * (not promise-based) way to tell a save was actually refused: same
 * reasoning as `Permissions.tsx`'s `bypassRefused` — `apply_patch`'s
 * refusal surfaces as an engine `ClientEvent::Error`, not a rejected
 * `update_settings` promise, so comparing the refreshed snapshot against
 * what was attempted is the only signal that does not lie.
 */
export function layersDiverge(a: Record<string, unknown>, b: Record<string, unknown>): boolean {
  const keys = new Set([...Object.keys(a), ...Object.keys(b)]);
  for (const key of keys) {
    if (JSON.stringify(a[key]) !== JSON.stringify(b[key])) return true;
  }
  return false;
}

function FileRow({ file }: { file: SettingsFile }) {
  const t = useT();
  const status = !file.parsed ? '已损坏' : file.exists ? '可写' : '将新建';
  return (
    <Row
      align="center"
      title={<span className="mono" style={{ fontSize: 12 }}>{file.path}</span>}
      badge={<ProvenanceBadge destination={file.layer as Provenance} />}
      desc={file.parse_error ? `解析失败：${file.parse_error}` : file.exists ? '存在，已解析' : '文件不存在（尚未写入任何值）'}
    >
      <span style={{ fontSize: 12, color: file.parsed ? t.text3 : t.danger, fontWeight: 600 }}>{status}</span>
    </Row>
  );
}

export function RawJson({ bridge, snapshot, editingLayer }: PageContentProps) {
  const t = useT();
  const currentFile = snapshot?.files.find((file) => file.layer === editingLayer) ?? null;
  const broken = Boolean(currentFile?.parse_error);
  const oldLayer = snapshot?.layers?.[editingLayer] ?? {};

  // Seeded once per mount — the shell remounts this component on a layer
  // switch (`key={editingLayer}` in `SettingsScreen.tsx`), so there is no
  // stale-layer-text bug here to guard against with a reset effect the way
  // `ToolsAgent.tsx`/`Plugins.tsx` must.
  const [text, setText] = useState(() => initialLayerText(snapshot, editingLayer, broken));
  const [validationError, setValidationError] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);
  const [saveError, setSaveError] = useState<string | null>(null);
  // The exact object most recently sent for THIS layer, so a re-render once
  // the refreshed snapshot lands can tell whether it actually took —
  // `null` before any save attempt, so nothing here renders as a refusal on
  // first paint.
  const [lastAttempt, setLastAttempt] = useState<Record<string, unknown> | null>(null);

  const refused = lastAttempt !== null && !saving && layersDiverge(lastAttempt, oldLayer);

  const handleSave = () => {
    const error = validateRawLayer(text);
    if (error) {
      setValidationError(error);
      return;
    }
    setValidationError(null);
    const parsed = JSON.parse(text) as Record<string, unknown>;
    const patch = computeLayerPatch(oldLayer, parsed);
    setSaving(true);
    setSaveError(null);
    setLastAttempt(parsed);
    bridge.clearError();
    void bridge.updateEngineSettings(editingLayer, patch)
      .catch((cause) => setSaveError(cause instanceof Error ? cause.message : '无法保存设置。'))
      .finally(() => setSaving(false));
  };

  return (
    <>
      <Card title={`当前层原文 · ${provenanceLabel(editingLayer)}`}>
        {broken && (
          <div role="alert" style={{ padding: '12px 18px 0', color: t.danger, fontSize: 12, lineHeight: 1.6 }}>
            这一层的文件解析失败：{currentFile?.parse_error}。下方已留空——保存合法的
            JSON 会用新内容整体替换这个损坏的文件；在那之前不会写入任何东西。
          </div>
        )}
        <div style={{ padding: '14px 18px' }}>
          <textarea
            value={text}
            onChange={(event) => { setText(event.target.value); setValidationError(null); }}
            aria-label="原始 JSON"
            data-testid="raw-json-textarea"
            rows={18}
            className="mono"
            style={{ ...inputStyle(t), width: '100%', boxSizing: 'border-box', resize: 'vertical' }}
          />
          {validationError && (
            <div role="alert" style={{ marginTop: 8, color: t.danger, fontSize: 12 }}>{validationError}</div>
          )}
          {saveError && (
            <div role="alert" style={{ marginTop: 8, color: t.danger, fontSize: 12 }}>{saveError}</div>
          )}
          {refused && (
            <div role="alert" style={{ marginTop: 8, color: t.warn, fontSize: 12, lineHeight: 1.6 }}>
              保存未生效——{bridge.error ?? '这次改动可能触及了拥有专用写入命令的键（例如 permissions），请改用对应的设置页。'}
            </div>
          )}
          <div style={{ marginTop: 10 }}>
            <button type="button" disabled={saving} onClick={handleSave} style={ghostButtonStyle(t, saving)}>保存</button>
          </div>
        </div>
      </Card>

      <Card title="四个设置层">
        {snapshot?.files.map((file) => <FileRow key={file.layer} file={file} />)}
        <Row
          align="center"
          title="策略（managed）"
          badge={<ProvenanceBadge destination="managed" />}
          desc="来自管理员的托管设置层，只读；不对应单一本地文件，本页无法编辑它。"
        >
          <span style={{ fontSize: 12, color: t.text3, fontWeight: 600 }}>只读</span>
        </Row>
      </Card>
    </>
  );
}
