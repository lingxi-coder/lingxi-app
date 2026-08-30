import { useRef, useState } from 'react';
import { Card, ProvenanceBadge, Row, provenanceLabel, type Provenance } from '../rows';
import { useT } from '../../../theme/ThemeContext';
import type { PageContentProps } from '../SettingsScreen';
import {
  ATTEMPT_NOT_DISPATCHED,
  snapshotLandedAfter,
  type SettingsFile,
  type SettingsSnapshot,
  type SnapshotBoundAttempt,
} from '../useEngineSettings';
import { parseJsonObjectInput } from '../jsonInput';
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
 * `permissions` is genuinely refused by the engine today, and this page
 * surfaces that refusal rather than silently dropping the key from what it
 * sends or pretending the save worked (`layersDiverge`, mirroring
 * `Permissions.tsx`'s `bypassRefused` — the command's promise resolves
 * either way; `apply_patch`'s failure is an engine EVENT, not a rejected
 * promise, so the only trustworthy signal is comparing what was asked for
 * against what the refreshed snapshot shows). Making the brief's exception
 * REAL — one write path that can actually persist a changed `permissions`
 * block as text — needs an engine change (either lifting `RESERVED_KEYS`
 * for a whole-layer-replace command, or a new command altogether) that was
 * not made here; this was reported instead of invented, per this task's own
 * instructions.
 *
 * **A layer whose file is currently broken cannot be repaired from this
 * page, full stop — this is NOT a scope gap, it is how the engine is
 * supposed to behave.** `apply_patch` (`settings_bridge.rs`) begins with
 * `read_settings_map`, and that function returns `Err` for any non-empty
 * content that fails to parse — the exact condition a `parse_error` on
 * `SettingsFile` reports. So for a broken layer, EVERY save is refused
 * before the submitted patch is even looked at, no matter how clean the
 * JSON the user typed is. There is no way to distinguish "the user's new
 * text is valid" from "the old file was broken" at that point, because the
 * refusal happens first. (Fix round 1: an earlier draft of this page
 * claimed the opposite — that saving valid JSON would replace a broken
 * file — inferred from the shape of the design rather than checked against
 * `read_settings_map`'s actual behaviour. That claim was wrong and reached
 * user-facing copy.) This refusal is a deliberate safety property: a
 * corrupt file must never be silently overwritten. A real fix (a
 * whole-file-replace path that is allowed to proceed from an unparseable
 * starting file) needs its own design pass — concurrent writers, where
 * `permission::mark_internal_write` fits, whether it warrants its own
 * confirmation step — and is a known, recorded gap, not something
 * improvised in a fix round. `rawJsonView` below returns a `'broken'`
 * variant with no `initialText`/save affordance at all for exactly this
 * reason: the type itself makes "present an editor that implies saving can
 * fix this" unrepresentable, the same way `Hooks.tsx`'s `editable: false`
 * literal does for its own read-only claim.
 */
export function validateRawLayer(text: string): string | null {
  const result = parseJsonObjectInput(text, '设置层');
  return 'error' in result ? result.error : null;
}

/** One top-level settings key with its own dedicated writer, refused on the generic patch path — see this module's doc comment. Currently just `permissions`; kept as a list (not a single constant) so a future engine-side addition to `RESERVED_KEYS` only needs one more entry here, not a new code shape. */
export const RESERVED_SETTINGS_KEYS = ['permissions'] as const;

/** Whether `patch` (as `computeLayerPatch` would produce) names a key with its own dedicated writer — the one case this page's save is known to be refused for today, distinct from a layer that is unparseable to begin with (`rawJsonView`'s `'broken'` variant handles that case before a save is ever possible). */
export function patchTouchesReservedKey(patch: Record<string, unknown | null>): boolean {
  return RESERVED_SETTINGS_KEYS.some((key) => key in patch);
}

/**
 * What `RawJson` should render for `layer`: either a `'broken'` view (no
 * text to edit, no save affordance — see this module's doc comment for why
 * a broken layer can never be fixed from here) or an `'editable'` view
 * seeded from the layer's OWN raw map (never `snapshot.effective`).
 *
 * This is a discriminated union rather than a plain string, on purpose: the
 * `'broken'` variant carries no `initialText` field at all, so a future
 * change that tries to show an editable box for a broken layer has to
 * change the TYPE first, which a test can catch — the same reasoning
 * `Hooks.tsx`'s `HooksPageModel.editable: false` literal already uses for
 * its own read-only claim.
 */
export type RawJsonView =
  | { mode: 'broken'; file: SettingsFile }
  | { mode: 'editable'; initialText: string };

export function rawJsonView(snapshot: SettingsSnapshot | null, layer: string): RawJsonView {
  const file = snapshot?.files.find((candidate) => candidate.layer === layer) ?? null;
  if (file?.parse_error) return { mode: 'broken', file };
  return { mode: 'editable', initialText: JSON.stringify(snapshot?.layers?.[layer] ?? {}, null, 2) };
}

/**
 * The guidance shown in place of an editor for a `'broken'` `RawJsonView`.
 * Pinned as its own function so a test can assert what it does NOT say
 * (that saving repairs the file) as directly as what it does say (the
 * refusal is deliberate, and where to fix the file instead) — see fix
 * round 1: the previous copy asserted the false claim this function exists
 * to replace.
 */
export function brokenLayerGuidance(file: SettingsFile): string {
  return `这一层的文件当前无法解析（${file.parse_error}），本页无法从这里修复它。`
    + `引擎会拒绝覆盖任何它读不懂的现有文件——这是有意的安全设计，用来防止一次写入`
    + `静默吞掉磁盘上已经存在、只是格式有误的内容。请用文本编辑器直接打开 ${file.path}`
    + `，修正其中的 JSON 语法，改好之后回到这个页面确认已经恢复正常。`;
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
 *
 * An ABSENT key and a key whose value is `null` compare EQUAL, which is not
 * a convenience — it is what makes this comparison agree with the write it
 * is checking. `computeLayerPatch` turns a `null` in the saved text into
 * `patch[key] = null`, and `apply_patch` reads that as DELETE. So a save of
 * `{"model": null}` lands on disk as a layer with no `model` at all: the
 * attempted value and the refreshed layer are the same state spelled two
 * ways. Comparing them by raw `JSON.stringify` (`'null'` vs `undefined`)
 * made them differ FOREVER, and unlike the promise-timing bug this one
 * never self-cleared — the refusal banner stayed up on a save that did
 * exactly what it was asked to.
 */
export function layersDiverge(a: Record<string, unknown>, b: Record<string, unknown>): boolean {
  const keys = new Set([...Object.keys(a), ...Object.keys(b)]);
  for (const key of keys) {
    if (settledValue(a[key]) !== settledValue(b[key])) return true;
  }
  return false;
}

/** `undefined` (key absent) and `null` (key explicitly nulled, i.e. deleted by `apply_patch`) are one state — see {@link layersDiverge}. */
function settledValue(value: unknown): string {
  return value === undefined || value === null ? 'null' : JSON.stringify(value);
}

/**
 * Whether the engine REFUSED `attempt` — the page's one refusal signal,
 * kept as a pure function so its two failure modes (see below) are
 * testable without mounting anything.
 *
 * Two things must both be true before a divergence means anything:
 *
 * 1. **A snapshot newer than the attempt has landed** ({@link
 *    snapshotLandedAfter}). Without this the comparison is against the
 *    PRE-save layer map and every successful save renders a refusal alert
 *    for one frame — invisible to the eye, announced by assistive tech
 *    every single time, and directly contradicting this module's own claim
 *    that the comparison "is the only signal that does not lie".
 * 2. **The attempted value still disagrees with what the layer now holds**
 *    ({@link layersDiverge}), with an explicit `null` counted as the
 *    deletion it actually performs.
 */
export function saveRefused(
  attempt: (SnapshotBoundAttempt & { value: Record<string, unknown> }) | null,
  currentSnapshot: unknown,
  layerNow: Record<string, unknown>,
): boolean {
  if (!snapshotLandedAfter(attempt, currentSnapshot)) return false;
  return layersDiverge((attempt as { value: Record<string, unknown> }).value, layerNow);
}

/**
 * The message to show for a refused save. `bridgeError` (from
 * `bridge.error`, the engine's own `ClientEvent::Error` text) is always
 * authoritative when present. When it is not — the event has not arrived
 * yet, or this connection predates one being wired for some future refusal
 * reason — this falls back to a message shaped by what was actually
 * attempted rather than guessing: naming `permissions` is only accurate
 * when the patch actually touched a `RESERVED_SETTINGS_KEYS` entry (fix
 * round 1, Important — the previous fallback named `permissions`
 * unconditionally, which misdirects a user whose refusal has nothing to do
 * with it, e.g. a file that went unparseable from another process between
 * this page loading it and the save being attempted).
 */
export function saveRefusalMessage(bridgeError: string | null, touchedReservedKey: boolean): string {
  if (bridgeError) return bridgeError;
  if (touchedReservedKey) {
    return '这次改动触及了拥有专用写入命令的键（permissions），请改用对应的设置页保存这部分改动。';
  }
  return '这次保存被引擎拒绝，具体原因未知——常见原因是这层文件在编辑期间从别处被改成了引擎读不懂的内容。刷新后重试；如果仍然失败，请用文本编辑器直接检查这个文件。';
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
  const view = rawJsonView(snapshot, editingLayer);
  const oldLayer = snapshot?.layers?.[editingLayer] ?? {};

  return (
    <>
      <Card title={`当前层原文 · ${provenanceLabel(editingLayer)}`}>
        {view.mode === 'broken'
          ? (
            <div role="alert" style={{ padding: '14px 18px', color: t.danger, fontSize: 12.5, lineHeight: 1.6 }}>
              {brokenLayerGuidance(view.file)}
            </div>
          )
          : <EditableLayer bridge={bridge} editingLayer={editingLayer} oldLayer={oldLayer} initialText={view.initialText} />}
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

function EditableLayer({ bridge, editingLayer, oldLayer, initialText }: {
  bridge: PageContentProps['bridge'];
  editingLayer: PageContentProps['editingLayer'];
  oldLayer: Record<string, unknown>;
  initialText: string;
}) {
  const t = useT();

  // Seeded once per mount — the shell remounts this component on a layer
  // switch (`key={editingLayer}` in `SettingsScreen.tsx`), so there is no
  // stale-layer-text bug here to guard against with a reset effect the way
  // `ToolsAgent.tsx`/`Plugins.tsx` must.
  const [text, setText] = useState(initialText);
  const [validationError, setValidationError] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);
  const [saveError, setSaveError] = useState<string | null>(null);
  // The exact object most recently sent for THIS layer, whether that attempt
  // touched a reserved key (so a refusal can name the right cause), and WHICH
  // snapshot the interface was holding when the attempt finished dispatching.
  // That last field is what makes the refusal signal honest: see
  // `snapshotLandedAfter`. `null` before any save attempt, so nothing here
  // renders as a refusal on first paint.
  const [lastAttempt, setLastAttempt] = useState<
    (SnapshotBoundAttempt & { value: Record<string, unknown>; touchedReservedKey: boolean }) | null
  >(null);

  // Read inside `.finally` below, where the closure's own render-time value
  // would be the pre-save snapshot and would hand `snapshotLandedAfter` a
  // stale "before" reference. Assigning a ref during render is the same
  // pattern `useBridge.ts` uses for `activeSessionIdRef`.
  const latestSnapshotRef = useRef<unknown>(bridge.settingsSnapshotEvent);
  latestSnapshotRef.current = bridge.settingsSnapshotEvent;

  // No `!saving` term here on purpose: "the save is still in flight" is
  // already expressed by `ATTEMPT_NOT_DISPATCHED`, and expressing it twice
  // is how the previous version ended up trusting `saving`'s wall-clock
  // timing instead of the snapshot's arrival.
  const refused = saveRefused(lastAttempt, bridge.settingsSnapshotEvent, oldLayer);

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
    setLastAttempt({
      value: parsed,
      touchedReservedKey: patchTouchesReservedKey(patch),
      snapshotAtDispatch: ATTEMPT_NOT_DISPATCHED,
    });
    bridge.clearError();
    void bridge.updateEngineSettings(editingLayer, patch)
      .catch((cause) => setSaveError(cause instanceof Error ? cause.message : '无法保存设置。'))
      .finally(() => {
        // Stamp the attempt with the snapshot that is current NOW — both
        // commands are on the wire, so any snapshot arriving after this
        // point is the engine's answer. The save button is `disabled` while
        // `saving`, so there is never a second attempt to stamp by mistake.
        setLastAttempt((attempt) => (
          attempt === null ? attempt : { ...attempt, snapshotAtDispatch: latestSnapshotRef.current }
        ));
        setSaving(false);
      });
  };

  return (
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
          保存未生效——{saveRefusalMessage(bridge.error, lastAttempt?.touchedReservedKey ?? false)}
        </div>
      )}
      <div style={{ marginTop: 10 }}>
        <button type="button" disabled={saving} onClick={handleSave} style={ghostButtonStyle(t, saving)}>保存</button>
      </div>
    </div>
  );
}
