import { useT } from '../../theme/ThemeContext';

export function Toggle({ value, onChange, label, disabled = false }: { value: boolean; onChange: (v: boolean) => void; label: string; disabled?: boolean }) {
  const t = useT();
  return (
    <button
      type="button"
      role="switch"
      className="settings-toggle"
      disabled={disabled}
      aria-checked={value}
      aria-label={label}
      onClick={() => onChange(!value)}
      style={{
        width: 38, height: 22, borderRadius: 99, border: 'none', cursor: 'pointer', padding: 0,
        background: value ? t.accent : t.surfaceActive,
        position: 'relative', transition: 'background 0.15s', flexShrink: 0,
      }}
    >
      <span
        style={{
          position: 'absolute', top: 2, left: 2, transform: `translateX(${value ? 16 : 0}px)`,
          width: 18, height: 18, borderRadius: '50%', background: '#fff',
          transition: 'transform 0.15s', boxShadow: '0 1px 3px rgba(0,0,0,0.2)',
        }}
      />
    </button>
  );
}
