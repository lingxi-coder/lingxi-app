import { tr } from '../../../i18n/locale';
import { LingxiAccount } from './LingxiAccount';
import { Card, Row } from '../rows';
import { useT } from '../../../theme/ThemeContext';
import type { PageContentProps } from '../SettingsScreen';
import { ghostButtonStyle } from './ghostButton';
import { providerAccountHealthLabel } from './ProviderAccounts';
export function Account({ bridge, onNavigate }: PageContentProps) {
  const t = useT();
  return <><LingxiAccount /><Card title={tr('desktop_shell_credentials_overview')}>
    {(bridge.bootstrap?.providerAccountSnapshot?.accounts ?? []).map(entry => <Row key={entry.accountId} title={entry.displayName} desc={providerAccountHealthLabel(bridge.bootstrap?.providerAccountSnapshot?.health.find(health => health.accountId === entry.accountId && health.identityGeneration === entry.identityGeneration))} align="center">
      <button type="button" onClick={() => onNavigate('provider-credentials')} style={ghostButtonStyle(t)}>{tr('common_open')}</button>
    </Row>)}
  </Card></>;
}
