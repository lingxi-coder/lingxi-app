import { useEffect, useRef, useState, type ReactNode } from 'react';
import { publicExtensionText, type PublicExtensionItem } from '../../../../shared/publicExtensionDirectory';
import { useI18n } from '../../../i18n/useI18n';
import { tr } from '../../../i18n/locale';
import { ExtensionIcon, useExtensionHubDialogFocus } from './ExtensionHub';
import { ExtensionGlyph } from './ExtensionHubIcons';

export function ExtensionDetails({ item, breadcrumb, onBack, actions, status, children, kind = item.kind }: {
  item: PublicExtensionItem;
  breadcrumb: string;
  onBack: () => void;
  actions: ReactNode;
  status?: string;
  children?: ReactNode;
  kind?: 'plugin' | 'skill' | 'mcp';
}) {
  const { locale } = useI18n();
  const heading = useRef<HTMLHeadingElement>(null);
  const [copied, setCopied] = useState<number | null>(null);
  const [copyError, setCopyError] = useState(false);
  useEffect(() => {
    heading.current?.closest('.settings-content-scroll')?.scrollTo({ top: 0 });
    heading.current?.focus({ preventScroll: true });
    setCopied(null);
    setCopyError(false);
  }, [item.id]);
  const copyExample = async (text: string, index: number) => {
    setCopyError(false);
    try {
      if (typeof window.lingxi?.copyText === 'function') await window.lingxi.copyText(text);
      else await navigator.clipboard.writeText(text);
      setCopied(index);
    } catch {
      setCopyError(true);
    }
  };
  const category = tr(`desktop_extension_category_${item.category}`);
  const examples = item.examples ?? [];
  return <section className="extension-overview" aria-labelledby="extension-overview-title" data-extension-id={item.id}>
    <nav className="extension-breadcrumb" aria-label={tr('desktop_extension_details_breadcrumb')}>
      <button type="button" onClick={onBack}>{breadcrumb}</button>
      <ExtensionGlyph name="chevronR" size={14} />
      <span aria-current="page">{item.name}</span>
    </nav>
    <header className="extension-overview-header">
      <ExtensionIcon kind={kind} large />
      <div className="extension-overview-heading">
        <h1 id="extension-overview-title" ref={heading} tabIndex={-1}>{item.name}</h1>
        <p>{item.publisher} · {publicExtensionText(item.description, locale)}</p>
        {status && <span className="extension-overview-status"><ExtensionGlyph name="check" size={14} />{status}</span>}
      </div>
      <div className="extension-overview-actions">{actions}</div>
    </header>
    {children}
    {examples.length > 0 && <section className="extension-overview-showcase" aria-label={tr('desktop_extension_examples')}>
      <div className="extension-overview-example-heading"><ExtensionIcon kind={kind} /><span>{tr('desktop_extension_examples')}</span></div>
      {examples.map((example, index) => {
        const text = publicExtensionText(example, locale);
        return <button key={index} type="button" className="extension-overview-example" onClick={() => { void copyExample(text, index); }} aria-label={tr('desktop_extension_copy_example_named', text)}>
          <span><strong>{item.name}</strong> {text}</span>
          <span className="extension-overview-example-arrow"><ExtensionGlyph name={copied === index ? 'check' : 'chevronR'} size={18} /></span>
        </button>;
      })}
      <span className="extension-overview-copy-status" role="status">{copyError ? tr('desktop_extension_copy_failed') : copied !== null ? tr('desktop_extension_example_copied') : tr('desktop_extension_copy_example_hint')}</span>
    </section>}
    <p className="extension-overview-description">{publicExtensionText(item.description, locale)}</p>
    {item.requirements && <p className="extension-overview-requirements">{publicExtensionText(item.requirements, locale)}</p>}
    {item.skills && item.skills.length > 0 && <section className="extension-overview-section">
      <h2>{tr('desktop_extension_included_skills')} <span>{item.skills.length}</span></h2>
      <div className="extension-overview-components">{item.skills.map((name) => <div key={name} className="extension-overview-component"><ExtensionIcon kind="skill" /><span>{name}</span></div>)}</div>
    </section>}
    <section className="extension-overview-section">
      <h2>{tr('desktop_extension_information')}</h2>
      <dl className="extension-overview-information">
        <dt>{tr('desktop_extension_developer')}</dt><dd>{item.publisher}</dd>
        <dt>{tr('desktop_extension_category')}</dt><dd>{category}</dd>
        <dt>{tr('desktop_extension_type')}</dt><dd>{kind === 'skill' ? tr('settings_title_skills') : kind === 'mcp' ? 'MCP' : tr('desktop_extension_plugins')}</dd>
        {item.version && <><dt>{tr('settings_linux_version')}</dt><dd>{item.version}</dd></>}
        <dt>{tr('desktop_extension_open_documentation')}</dt><dd><a href={item.documentationUrl} target="_blank" rel="noopener noreferrer">{tr('desktop_extension_open_documentation')} ↗</a></dd>
        <dt>{tr('desktop_extension_open_source')}</dt><dd><a href={item.sourceUrl} target="_blank" rel="noopener noreferrer">{tr('desktop_extension_open_source')} ↗</a></dd>
      </dl>
    </section>
  </section>;
}

export function ExtensionRemovalConfirmation({ name, busy, error, onConfirm, onCancel }: {
  name: string; busy: boolean; error?: string | null; onConfirm: () => void; onCancel: () => void;
}) {
  const dialogRef = useExtensionHubDialogFocus(true, () => { if (!busy) onCancel(); });
  return <div className="extension-detail-backdrop"><section ref={dialogRef} tabIndex={-1} role="alertdialog" aria-modal="true" aria-labelledby="extension-remove-title" className="extension-detail-dialog extension-remove-confirmation">
    <div className="extension-detail-head"><h2 id="extension-remove-title">{tr('desktop_extension_remove_named', name)}</h2></div>
    <p className="extension-detail-content">{tr('desktop_extension_remove_configuration_note')}</p>
    {error && <p role="alert" className="extension-detail-content">{error}</p>}
    <footer className="extension-detail-footer"><button type="button" className="extension-hub-icon-text" data-dialog-initial-focus disabled={busy} onClick={onCancel}>{tr('common_cancel')}</button><button type="button" className="extension-hub-danger" disabled={busy} onClick={onConfirm}>{tr('desktop_extension_remove')}</button></footer>
  </section></div>;
}
