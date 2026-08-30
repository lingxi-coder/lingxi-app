import { useT } from '../../theme/ThemeContext';

export function Toggle({ value, onChange }: { value: boolean; onChange: (v: boolean) => void }) {
  const t = useT();
  return (
    <button
      onClick={() => onChange(!value)}
      style={{
        width: 38, height: 22, borderRadius: 99, border: 'none', cursor: 'pointer', padding: 0,
        background: value ? t.accent : t.surfaceActive,
        position: 'relative', transition: 'background 0.15s', flexShrink: 0,
      }}
    >
      <span
        style={{
          position: 'absolute', top: 2, left: value ? 18 : 2,
          width: 18, height: 18, borderRadius: '50%', background: '#fff',
          transition: 'left 0.15s', boxShadow: '0 1px 3px rgba(0,0,0,0.2)',
        }}
      />
    </button>
  );
}
