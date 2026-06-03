import { useState } from 'react';
import { useT } from '../../theme/ThemeContext';
import {
  Segmented, Toggle, SettingsSelect, SettingsRow, SectionTitle, CodePreview, CODE_THEMES,
} from './primitives';

export function SettingsCodePage() {
  const t = useT();
  const [darkVariant, setDarkVariant] = useState('dark');
  const [interfaceFont, setInterfaceFont] = useState('lingxi');
  const [textSize, setTextSize] = useState('medium');
  const [codeFont, setCodeFont] = useState('');
  const [lightCodeTheme, setLightCodeTheme] = useState('lingxi-light');
  const [darkCodeTheme, setDarkCodeTheme] = useState('lingxi-dark');
  const [classify, setClassify] = useState(true);
  const [bypass, setBypass] = useState(true);

  return (
    <div>
      <SectionTitle>Appearance</SectionTitle>
      <SettingsRow title="Dark mode" desc="How dark the interface looks when using dark mode.">
        <Segmented
          value={darkVariant}
          onChange={setDarkVariant}
          options={[
            { id: 'dark', label: 'Dark' },
            { id: 'midnight', label: 'Midnight' },
          ]}
        />
      </SettingsRow>
      <SettingsRow title="Interface font" desc="Font for the Lingxi Code interface — menus, sidebar, and chat.">
        <Segmented
          value={interfaceFont}
          onChange={setInterfaceFont}
          options={[
            { id: 'lingxi', label: 'Lingxi Sans' },
            { id: 'system', label: 'System' },
          ]}
        />
      </SettingsRow>
      <SettingsRow title="Transcript text size" desc="Size of the conversation transcript text.">
        <Segmented
          value={textSize}
          onChange={setTextSize}
          options={[
            { id: 'small', label: 'Small' },
            { id: 'medium', label: 'Medium' },
            { id: 'large', label: 'Large' },
          ]}
        />
      </SettingsRow>

      <SectionTitle>Code appearance</SectionTitle>
      <SettingsRow title="Code font" desc="Set a custom monospace font for code and terminal.">
        <input
          value={codeFont}
          onChange={(e) => setCodeFont(e.target.value)}
          placeholder="e.g. JetBrains Mono"
          style={{
            width: 280, padding: '8px 12px', borderRadius: 8,
            background: t.surface, border: `0.5px solid ${t.border}`,
            color: t.text, fontSize: 13, fontFamily: 'inherit', outline: 'none',
          }}
        />
      </SettingsRow>
      <div style={{ display: 'flex', gap: 14, padding: '14px 0 22px', borderBottom: `0.5px solid ${t.border}` }}>
        <div style={{ flex: 1, minWidth: 0 }}>
          <SettingsSelect value={lightCodeTheme} onChange={setLightCodeTheme} options={CODE_THEMES.light} />
          <div style={{ marginTop: 12 }}>
            <CodePreview theme="light" />
          </div>
        </div>
        <div style={{ flex: 1, minWidth: 0 }}>
          <SettingsSelect value={darkCodeTheme} onChange={setDarkCodeTheme} options={CODE_THEMES.dark} />
          <div style={{ marginTop: 12 }}>
            <CodePreview theme="dark" />
          </div>
        </div>
      </div>

      <SectionTitle>General</SectionTitle>
      <SettingsRow
        title="Classify session states"
        desc="Allow Lingxi to automatically classify sessions as blocked, ready for review, or done. Classifying sessions counts towards your plan usage. Applies to new sessions."
      >
        <Toggle value={classify} onChange={setClassify} />
      </SettingsRow>

      <SectionTitle>Local sessions</SectionTitle>
      <SettingsRow
        title="Allow bypass permissions mode"
        desc="Bypass all permission checks and let Lingxi work uninterrupted. This works well for workflows like fixing lint errors or generating boilerplate code. Letting Lingxi run arbitrary commands is risky and can result in data loss, system corruption, or data exfiltration (e.g. prompt injection)."
      >
        <Toggle value={bypass} onChange={setBypass} />
      </SettingsRow>
      <div style={{ height: 60 }} />
    </div>
  );
}
