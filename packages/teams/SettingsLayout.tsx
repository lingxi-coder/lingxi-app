import type { ReactNode, Ref } from 'react';
import { Icon, type IconName } from './ui';

export interface SettingsSection<Id extends string> { id: Id; text: string; icon: IconName }

export function SettingsLayout<Id extends string>({ label, sections, active, onSelect, heading, children }: {
  label: string;
  sections: readonly SettingsSection<Id>[];
  active: SettingsSection<Id>;
  onSelect(id: Id): void;
  heading?: Ref<HTMLHeadingElement>;
  children: ReactNode;
}) {
  return <div className="tw-settings-layout">
    <nav className="tw-settings-nav" aria-label={label}>
      <p>{label}</p>
      {sections.map(item => <button key={item.id} type="button" aria-current={active.id === item.id ? 'page' : undefined}
        onClick={() => onSelect(item.id)}><Icon name={item.icon} />{item.text}</button>)}
    </nav>
    <div className="tw-settings-section">
      <h2 ref={heading} tabIndex={-1}>{active.text}</h2>
      {children}
    </div>
  </div>;
}
