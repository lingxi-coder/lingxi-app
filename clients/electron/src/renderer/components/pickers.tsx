import { useT } from '../theme/ThemeContext';
import {
  EFFORTS, PLAN_USAGE, PERM_MODES, PERM_DEFAULT, type Model,
} from '../data';
import { groupModelReferences } from '../bridge/modelCatalog';
import { Icon } from './Icon';
import { Kbd } from './primitives';

// ─── PERMISSION MODE PICKER ──────────────────────────────
export function PermissionPicker({
  mode, setMode, open, setOpen,
}: {
  mode: string;
  setMode: (m: string) => void;
  open: boolean;
  setOpen: (v: boolean) => void;
}) {
  const t = useT();
  const current = PERM_MODES.find((m) => m.id === mode) || PERM_MODES[4];
  const isWarn = mode === 'bypass';
  return (
    <div style={{ position: 'relative' }}>
      <button
        onClick={() => setOpen(!open)}
        style={{
          display: 'flex', alignItems: 'center', gap: 6,
          padding: '4px 9px', borderRadius: 7, border: 'none', cursor: 'pointer',
          background: open ? t.surfaceHover : 'transparent',
          color: isWarn ? t.warn : t.text2,
          fontSize: 12, fontFamily: 'inherit', fontWeight: 500,
        }}
        onMouseEnter={(e) => {
          if (!open) e.currentTarget.style.background = t.surfaceHover;
        }}
        onMouseLeave={(e) => {
          if (!open) e.currentTarget.style.background = 'transparent';
        }}
      >
        <span>{current.label}</span>
      </button>
      {open && (
        <>
          <div onClick={() => setOpen(false)} style={{ position: 'fixed', inset: 0, zIndex: 49 }} />
          <div
            style={{
              position: 'absolute', bottom: 'calc(100% + 8px)', left: 0, zIndex: 50,
              background: t.surface, border: `0.5px solid ${t.borderStrong}`,
              borderRadius: 12, padding: 6, minWidth: 290,
              boxShadow: '0 16px 40px rgba(0,0,0,0.22)',
              animation: 'fade-in 0.15s ease',
            }}
          >
            <div style={{ padding: '8px 10px 4px', display: 'flex', alignItems: 'center', gap: 6 }}>
              <span style={{ flex: 1, fontSize: 12, color: t.text3, fontWeight: 500 }}>Mode</span>
              <Kbd>⇧</Kbd>
              <Kbd>⌘</Kbd>
              <Kbd>M</Kbd>
            </div>
            <div style={{ display: 'flex', alignItems: 'center', gap: 6, padding: '6px 10px 10px' }}>
              <span style={{ flex: 1, fontSize: 13.5, color: t.text, fontWeight: 500 }}>
                {current.label}
                {mode === PERM_DEFAULT && <span style={{ color: t.text4, fontWeight: 500 }}> · Default</span>}
              </span>
              <Icon name="check" size={13} color={t.text2} stroke={2.2} />
            </div>
            <div style={{ height: 0.5, background: t.border, margin: '0 4px 4px' }} />
            {PERM_MODES.map((m, i) => {
              const active = m.id === mode;
              return (
                <div
                  key={m.id}
                  onClick={() => {
                    setMode(m.id);
                    setOpen(false);
                  }}
                  style={{
                    display: 'flex', alignItems: 'center', gap: 8,
                    padding: '7px 10px', borderRadius: 7, cursor: 'pointer',
                  }}
                  onMouseEnter={(e) => (e.currentTarget.style.background = t.surfaceHover)}
                  onMouseLeave={(e) => (e.currentTarget.style.background = 'transparent')}
                >
                  <span style={{ flex: 1, fontSize: 13.5, color: t.text, fontWeight: 500 }}>{m.label}</span>
                  {active && <Icon name="check" size={13} color={t.text2} stroke={2.2} />}
                  <span style={{ fontSize: 11.5, color: t.text4, fontFamily: 'inherit', minWidth: 10, textAlign: 'right' }}>{i + 1}</span>
                </div>
              );
            })}
          </div>
        </>
      )}
    </div>
  );
}

// ─── MODEL PICKER (with Effort + Fast mode) ──────────────
export function ModelPicker({
  model, models, setModel, open, setOpen, effort, setEffort, fastMode, setFastMode,
}: {
  model: Model;
  models: readonly Model[];
  setModel: (m: Model) => void;
  open: boolean;
  setOpen: (v: boolean) => void;
  effort: string;
  setEffort: (e: string) => void;
  fastMode: boolean;
  setFastMode: (v: boolean) => void;
}) {
  const t = useT();
  const modelsById = new Map(models.map((entry) => [entry.id, entry]));
  const modelGroups = groupModelReferences(models.map((entry) => entry.id));
  return (
    <div style={{ position: 'relative' }}>
      <button
        onClick={() => setOpen(!open)}
        style={{
          display: 'flex', alignItems: 'center', gap: 6,
          padding: '4px 10px', borderRadius: 7, border: 'none', cursor: 'pointer',
          background: open ? t.surfaceHover : 'transparent',
          color: t.text2, fontSize: 11.5, fontFamily: 'inherit',
        }}
        onMouseEnter={(e) => {
          if (!open) e.currentTarget.style.background = t.surfaceHover;
        }}
        onMouseLeave={(e) => {
          if (!open) e.currentTarget.style.background = 'transparent';
        }}
      >
        <span style={{ fontWeight: 600, color: t.text }}>{model.name}</span>
        {model.variant && <span style={{ color: t.text3, fontWeight: 500 }}>{model.variant}</span>}
        <span style={{ color: t.text4 }}>·</span>
        <span>{EFFORTS.find((e) => e.id === effort)?.label}</span>
        {fastMode && (
          <>
            <span style={{ color: t.text4 }}>·</span>
            <span>Fast</span>
          </>
        )}
      </button>
      {open && (
        <>
          <div onClick={() => setOpen(false)} style={{ position: 'fixed', inset: 0, zIndex: 49 }} />
          <div
            style={{
              position: 'absolute', bottom: 'calc(100% + 8px)', right: 0, zIndex: 50,
              background: t.surface, border: `0.5px solid ${t.borderStrong}`,
              borderRadius: 12, padding: 6, minWidth: 320,
              boxShadow: '0 16px 40px rgba(0,0,0,0.28)',
              animation: 'fade-in 0.15s ease',
            }}
          >
            <div style={{ padding: '8px 10px 4px', display: 'flex', alignItems: 'center', gap: 6 }}>
              <span style={{ flex: 1, fontSize: 11.5, color: t.text3, fontWeight: 500 }}>Models</span>
              <Kbd>⇧</Kbd>
              <Kbd>⌘</Kbd>
              <Kbd>I</Kbd>
            </div>
            {modelGroups.map((group, groupIndex) => (
              <section
                key={group.providerId ?? 'unqualified'}
                aria-labelledby={`model-provider-${group.providerId ?? 'other'}`}
                style={groupIndex === 0 ? undefined : { marginTop: 5, paddingTop: 5, borderTop: `0.5px solid ${t.border}` }}
              >
                <div id={`model-provider-${group.providerId ?? 'other'}`} style={{ padding: '7px 10px 4px', fontSize: 10.5, color: t.text3, fontWeight: 650, letterSpacing: '.06em', textTransform: 'uppercase' }}>{group.providerLabel}</div>
                {group.models.map((entry) => {
                  const m = modelsById.get(entry.reference);
                  if (!m) return null;
                  const active = m.id === model.id;
                  return (
                    <div
                      key={m.id}
                      onClick={() => setModel(m)}
                      style={{
                        display: 'flex', alignItems: 'center', gap: 10,
                        padding: '6px 10px', borderRadius: 7, cursor: 'pointer',
                        background: active ? t.surfaceHover : 'transparent',
                      }}
                      onMouseEnter={(e) => {
                        if (!active) e.currentTarget.style.background = t.surfaceHover;
                      }}
                      onMouseLeave={(e) => {
                        if (!active) e.currentTarget.style.background = 'transparent';
                      }}
                    >
                      <span style={{ flex: 1, display: 'flex', alignItems: 'baseline', gap: 6, fontSize: 13.5, color: t.text }}>
                        <span style={{ fontWeight: 500 }}>{m.name}</span>
                        {m.variant && <span style={{ color: t.text3, fontWeight: 500, fontSize: 13 }}>{m.variant}</span>}
                      </span>
                      {active ? <Icon name="check" size={13} color={t.accent} stroke={2.4} /> : <span style={{ width: 13 }} />}
                    </div>
                  );
                })}
              </section>
            ))}

            <div style={{ height: 0.5, background: t.border, margin: '6px 10px' }} />

            <div style={{ padding: '4px 10px 4px', display: 'flex', alignItems: 'center', gap: 6 }}>
              <span style={{ flex: 1, fontSize: 11.5, color: t.text3, fontWeight: 500 }}>Effort</span>
              <Kbd>⇧</Kbd>
              <Kbd>⌘</Kbd>
              <Kbd>E</Kbd>
            </div>
            {EFFORTS.map((e) => {
              const active = e.id === effort;
              return (
                <div
                  key={e.id}
                  onClick={() => setEffort(e.id)}
                  style={{
                    display: 'flex', alignItems: 'center', gap: 10,
                    padding: '6px 10px', borderRadius: 7, cursor: 'pointer',
                    background: active ? t.surfaceHover : 'transparent',
                  }}
                  onMouseEnter={(ev) => {
                    if (!active) ev.currentTarget.style.background = t.surfaceHover;
                  }}
                  onMouseLeave={(ev) => {
                    if (!active) ev.currentTarget.style.background = 'transparent';
                  }}
                >
                  <span style={{ flex: 1, fontSize: 13.5, color: t.text, fontWeight: 500 }}>{e.label}</span>
                  {active && <Icon name="check" size={13} color={t.accent} stroke={2.4} />}
                </div>
              );
            })}

            <div style={{ height: 0.5, background: t.border, margin: '6px 10px' }} />

            <div style={{ padding: '4px 10px 4px' }}>
              <span style={{ fontSize: 11.5, color: t.text3, fontWeight: 500 }}>Fast mode</span>
            </div>
            <div style={{ display: 'flex', alignItems: 'center', gap: 10, padding: '8px 10px', borderRadius: 7 }}>
              <span style={{ flex: 1, fontSize: 13.5, color: t.text }}>Enable fast mode</span>
              <button
                onClick={() => setFastMode(!fastMode)}
                style={{
                  width: 34, height: 20, borderRadius: 99, border: 'none', cursor: 'pointer', padding: 0,
                  background: fastMode ? t.accent : t.surfaceActive,
                  position: 'relative', transition: 'background 0.15s',
                }}
              >
                <span
                  style={{
                    position: 'absolute', top: 2, left: fastMode ? 16 : 2,
                    width: 16, height: 16, borderRadius: '50%', background: '#fff',
                    transition: 'left 0.15s', boxShadow: '0 1px 3px rgba(0,0,0,0.2)',
                  }}
                />
              </button>
            </div>
          </div>
        </>
      )}
    </div>
  );
}

// ─── CONTEXT PICKER (window usage + plan usage) ────────────
export function ContextPicker({ open, setOpen }: { open: boolean; setOpen: (v: boolean) => void }) {
  const t = useT();
  const colorMap: Record<string, string> = { accent: t.accent, accent2: t.accent2, accent3: t.accent3 };
  return (
    <div style={{ position: 'relative' }}>
      <button
        onClick={() => setOpen(!open)}
        title="上下文与额度"
        style={{
          marginLeft: 4, width: 22, height: 22, borderRadius: 99,
          border: 'none', cursor: 'pointer', padding: 0,
          background: 'transparent', position: 'relative',
          display: 'flex', alignItems: 'center', justifyContent: 'center',
        }}
        onMouseEnter={(e) => (e.currentTarget.style.background = t.surfaceHover)}
        onMouseLeave={(e) => (e.currentTarget.style.background = 'transparent')}
      >
        {/* donut showing context fill */}
        <svg width="14" height="14" viewBox="0 0 14 14">
          <circle cx="7" cy="7" r="5.4" fill="none" stroke={t.text4} strokeWidth="1.4" opacity="0.5" />
          <circle
            cx="7"
            cy="7"
            r="5.4"
            fill="none"
            stroke={t.accent}
            strokeWidth="1.6"
            strokeDasharray={`${(7 / 100) * 33.93} 33.93`}
            strokeLinecap="round"
            transform="rotate(-90 7 7)"
          />
        </svg>
      </button>
      {open && (
        <>
          <div onClick={() => setOpen(false)} style={{ position: 'fixed', inset: 0, zIndex: 49 }} />
          <div
            style={{
              position: 'absolute', bottom: 'calc(100% + 8px)', right: -4, zIndex: 50,
              background: t.surface, border: `0.5px solid ${t.borderStrong}`,
              borderRadius: 12, padding: '14px 16px', minWidth: 380,
              boxShadow: '0 16px 40px rgba(0,0,0,0.28)',
              animation: 'fade-in 0.15s ease',
            }}
          >
            <div style={{ display: 'flex', alignItems: 'center', cursor: 'pointer', marginBottom: 6 }}>
              <span style={{ flex: 1, fontSize: 13, color: t.text3, fontWeight: 500 }}>Context window</span>
              <span className="mono" style={{ fontSize: 12.5, color: t.text2 }}>
                67.9k / 1.0M <span style={{ color: t.text4 }}>(7%)</span>
              </span>
              <Icon name="chevronR" size={13} color={t.text4} stroke={2} />
            </div>
            <div style={{ height: 5, borderRadius: 99, background: t.surfaceActive, overflow: 'hidden', marginBottom: 14 }}>
              <div style={{ width: '7%', height: '100%', background: t.accent, borderRadius: 99 }} />
            </div>

            <div style={{ height: 0.5, background: t.border, margin: '0 -16px 12px' }} />

            <div style={{ display: 'flex', alignItems: 'center', marginBottom: 10, cursor: 'pointer' }}>
              <span style={{ flex: 1, fontSize: 13, color: t.text3, fontWeight: 500 }}>Plan usage</span>
              <span style={{ display: 'inline-flex', color: t.text4 }}>
                <Icon name="chevronR" size={13} stroke={2} />
              </span>
            </div>

            {PLAN_USAGE.map((u) => (
              <div key={u.label} style={{ marginBottom: 10 }}>
                <div style={{ display: 'flex', alignItems: 'baseline', marginBottom: 5 }}>
                  <span style={{ flex: 1, fontSize: 13.5, color: t.text, fontWeight: 500 }}>{u.label}</span>
                  <span style={{ fontSize: 12, color: t.text3 }}>
                    <span style={{ color: t.text2, fontWeight: 500 }}>{u.used}%</span>
                    <span style={{ margin: '0 4px', color: t.text4 }}>·</span>
                    <span>{u.reset}</span>
                  </span>
                </div>
                <div style={{ height: 4, borderRadius: 99, background: t.surfaceActive, overflow: 'hidden' }}>
                  <div style={{ width: u.used + '%', height: '100%', background: colorMap[u.color], borderRadius: 99 }} />
                </div>
              </div>
            ))}
          </div>
        </>
      )}
    </div>
  );
}
