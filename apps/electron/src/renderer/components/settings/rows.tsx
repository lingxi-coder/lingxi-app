import type { ReactNode } from 'react';
import { useT } from '../../theme/ThemeContext';
import { rowState, type SettingsSnapshot } from './useEngineSettings';
// Type-only: `EditableLayer` is a subtype of `Provenance` computed in
// `SettingsScreen.tsx`. `import type` is fully erased at compile time, so
// this does NOT create a runtime edge back to `SettingsScreen.tsx` (which
// imports page components that import THIS module at runtime) — only a
// real (non-type) import here would risk that cycle.
import type { EditableLayer } from './SettingsScreen';

/**
 * Where a setting's effective value lives. This is presentation vocabulary
 * only — these primitives render a badge, they do not decide which layer
 * wins. `managed` is not one more layer alongside the others: it means an
 * administrator pinned the value, so a row showing it must disable its
 * control. `device` values never go through engine-layer merging at all —
 * they live in the Electron store and always render as a fixed badge.
 */
export type Provenance = 'device' | 'user' | 'project' | 'local' | 'managed';

const PROVENANCE_LABELS: Record<Provenance, string> = {
  device: '设备',
  user: '用户',
  project: '项目',
  local: '本地',
  managed: '策略',
};

/** Total over `Provenance` — every value has a badge label, so callers can never get `undefined`. */
export function provenanceLabel(d: Provenance): string {
  return PROVENANCE_LABELS[d];
}

/**
 * 每一层「值落到哪、影响谁」的一句话。这是整套设置界面里唯一解释层含义的地方——
 * 三个裸标签（用户 / 项目 / 本地）本身什么都不说明，而它们的差别有真实后果：
 * `project` 层的文件随仓库提交，团队每个人都会拿到；`local` 层的同名文件被
 * gitignore（`.gitignore`），只在本机生效。一个人在「项目」层加一条
 * 权限规则，是在替整个团队做决定——今天界面上没有任何东西说过这件事。
 *
 * 刻意不写死具体路径：真实路径由引擎在 `files_json` 里逐层回传，界面显示的必须是
 * 那一个（见 `useEngineSettings.ts` 的 `projectDirFromSnapshot`）。这里只讲含义，
 * 于是这张表不会因为引擎换了配置目录而变成谎话。
 */
const PROVENANCE_DESCRIPTIONS: Record<Provenance, string> = {
  device: '存在这台设备的应用里，不经引擎合并，与项目无关。',
  user: '写进你的用户设置文件，本机所有项目都生效。',
  project: '写进项目内的设置文件，只对这个项目生效；该文件会随仓库提交，团队成员都会拿到。',
  local: '写进项目内的本地设置文件，只对这个项目、只在本机生效；该文件不会提交。',
  managed: '管理员托管的策略，只读，优先级高于以上所有层。',
};

/** Total over `Provenance`, for the same reason {@link provenanceLabel} is. */
export function provenanceDescription(d: Provenance): string {
  return PROVENANCE_DESCRIPTIONS[d];
}

/** A group of settings rows: a rounded card whose rows are separated by hairlines inside it. */
export function Card({ title, children }: { title?: string; children: ReactNode }) {
  const t = useT();
  return (
    <div style={{ marginBottom: 28 }}>
      {title && (
        <div style={{ fontSize: 13, fontWeight: 600, color: t.text2, marginBottom: 10 }}>{title}</div>
      )}
      <div style={{
        border: `0.5px solid ${t.border}`, borderRadius: 12,
        background: t.surface, overflow: 'hidden',
      }}>
        {children}
      </div>
    </div>
  );
}

export function Row({
  title, desc, badge, align = 'start', children,
}: {
  title: ReactNode; desc?: ReactNode; badge?: ReactNode;
  align?: 'start' | 'center'; children: ReactNode;
}) {
  const t = useT();
  return (
    <div style={{
      display: 'flex', gap: 24, padding: '16px 18px',
      alignItems: align === 'center' ? 'center' : 'flex-start',
      borderTop: `0.5px solid ${t.border}`,
    }}>
      <div style={{ flex: 1, minWidth: 0 }}>
        <div style={{ display: 'flex', alignItems: 'center', gap: 8 }}>
          <span style={{ fontSize: 14, fontWeight: 500, color: t.text }}>{title}</span>
          {badge}
        </div>
        {desc && (
          <div style={{ fontSize: 12.5, color: t.text3, marginTop: 4, lineHeight: 1.5, maxWidth: 620 }}>
            {desc}
          </div>
        )}
      </div>
      <div style={{ flexShrink: 0 }}>{children}</div>
    </div>
  );
}

/** Names the layer a row's effective value lives in. */
export function ProvenanceBadge({ destination }: { destination: Provenance }) {
  const t = useT();
  return (
    <span style={{
      fontSize: 10.5, padding: '2px 7px', borderRadius: 5,
      background: t.surfaceHover, color: t.text3, fontWeight: 600,
    }}>
      {provenanceLabel(destination)}
    </span>
  );
}

/** Shorthand for a policy-locked row: always the `managed` badge, never a plain layer name. */
export function LockedBadge() {
  return <ProvenanceBadge destination="managed" />;
}

/**
 * For a key whose effective value the engine merged across layers, there is
 * no single layer to name — `hooks` written in `user` and in `project`
 * resolves to both at once. `ProvenanceBadge` would have to pick one, and
 * picking one is false, so this badge says what actually happened instead.
 *
 * It is deliberately NOT a `Provenance` value: `merged` is not one more
 * layer, it is the absence of a single one. An expandable per-layer
 * breakdown would be a nicer answer; a badge that does not lie is the bar.
 */
export function MergedBadge() {
  const t = useT();
  return (
    <span
      title="这个值由多个设置层合并而成，不属于任何单独一层"
      style={{
        fontSize: 10.5, padding: '2px 7px', borderRadius: 5,
        background: t.surfaceHover, color: t.text3, fontWeight: 600,
      }}
    >
      多层合并
    </span>
  );
}

/**
 * The row-level counterpart to [`MergedBadge`], for the same reason
 * `OverriddenNotice` exists: on a merged key, "你写在 X 层的值被 Y 层盖掉了"
 * is not what happened — nothing was overridden, the layers were combined —
 * so that notice must not render, and this says the true thing in its place.
 */
export function MergedNotice({ editingLayer }: { editingLayer: Provenance }) {
  const t = useT();
  return (
    <div style={{ marginTop: 6, fontSize: 12, color: t.text3 }}>
      当前生效值由多个设置层合并而成，不属于任何单独一层；
      在{provenanceLabel(editingLayer)}层的编辑只改动这一层自己的条目。
    </div>
  );
}

/**
 * The user layer is the lowest of the file layers, so "I set this here but
 * a higher layer's value wins" is the common case, not an edge case. This
 * reads as an explanation with a way to act, not as an error.
 */
export function OverriddenNotice({
  editingLayer, effectiveLayer, onJump,
}: { editingLayer: Provenance; effectiveLayer: Provenance; onJump(): void }) {
  const t = useT();
  return (
    <div style={{ marginTop: 6, fontSize: 12, color: t.warn }}>
      已写入{provenanceLabel(editingLayer)}层；当前生效值来自{provenanceLabel(effectiveLayer)}层。
      <button
        type="button"
        onClick={onJump}
        style={{
          marginLeft: 6, background: 'none', border: 'none', padding: 0,
          color: t.link ?? t.accent, cursor: 'pointer', font: 'inherit', textDecoration: 'underline',
        }}
      >
        前往该层
      </button>
    </div>
  );
}

/**
 * Whether `layer` is one of the three tabs the layer switcher can actually
 * jump to — `OverriddenNotice.onJump`/`FieldProvenanceNotice` must not offer
 * to jump to `cli`/`managed`/`env`/`defaults`, which the shell has no tab
 * for. Moved here from `CustomProviders.tsx` (Task 18 fix round 1, Minor)
 * so every page that needs it — not just the one that happened to define it
 * first — imports one copy; `CustomProviders.tsx` re-exports it so its own
 * existing import sites (including the test file) keep working.
 */
export function isEditableLayer(layer: Provenance): layer is EditableLayer {
  return layer === 'user' || layer === 'project' || layer === 'local';
}

/**
 * The "does a write to `editingLayer` actually take effect" banner, factored
 * out of five near-identical copies (`CustomProviders`, `ToolsAgent`,
 * `Plugins`, `Hooks`, `Permissions` — Task 18 fix round 1, Minor). Renders
 * nothing for `unset`/`set-here`/`inherited`/`locked` — only `merged` and
 * `overridden` have anything to say here; `locked` is surfaced by the
 * caller disabling its own control, not by this notice.
 */
export function FieldProvenanceNotice({
  snapshot, fieldKey, editingLayer, onJumpToLayer, label = '生效层',
}: {
  snapshot: SettingsSnapshot | null;
  fieldKey: string;
  editingLayer: Provenance;
  onJumpToLayer(layer: EditableLayer): void;
  label?: string;
}) {
  const state = snapshot ? rowState(snapshot, fieldKey, editingLayer) : null;
  if (state?.kind === 'merged') {
    return <Row title={label} badge={<MergedBadge />} align="center"><MergedNotice editingLayer={editingLayer} /></Row>;
  }
  if (state?.kind === 'overridden') {
    return (
      <Row title={label} align="center">
        <OverriddenNotice
          editingLayer={editingLayer}
          effectiveLayer={state.by}
          onJump={() => { if (isEditableLayer(state.by)) onJumpToLayer(state.by); }}
        />
      </Row>
    );
  }
  return null;
}
