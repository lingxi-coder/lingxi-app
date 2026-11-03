import { tr } from '../i18n/locale';
import { useT } from '../theme/ThemeContext';

/** The sidebar in Teams mode. It names the mode and nothing more; the Teams page is what the main area shows. */
export function TeamsSidebar() {
  const t = useT();
  return (
    <nav className="no-drag desktop-sidebar-list" aria-label={tr('teams_title')} style={{ flex: 1, minHeight: 0, overflowY: 'auto', padding: '0 10px 12px' }}>
      <section aria-labelledby="teams-sidebar-heading">
        <div className="projects-sidebar-heading" style={{ minHeight: 35, padding: '3px 4px 5px 8px', display: 'flex', alignItems: 'center' }}>
          <h2 id="teams-sidebar-heading" style={{ flex: 1, color: t.text4, fontSize: 12.5, fontWeight: 500, letterSpacing: '.01em' }}>{tr('teams_title')}</h2>
        </div>
      </section>
    </nav>
  );
}
