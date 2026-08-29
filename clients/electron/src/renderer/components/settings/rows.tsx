import type { ReactNode } from 'react';
import { useT } from '../../theme/ThemeContext';

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
