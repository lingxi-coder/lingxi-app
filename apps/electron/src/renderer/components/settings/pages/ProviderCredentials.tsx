import { logicalProviderId, providerAccountSummary, type ProviderAccountSnapshot } from '../../../../shared/providerAccounts';
import { ProviderAccounts } from './ProviderAccounts';
import { CloudProviders } from './CloudProviders';
import { VAULT_PRESET_IDS } from '../../../../shared/providerVault';
import { ProviderProtocol } from './ProviderProtocol';
import { useI18n } from '../../../i18n/useI18n';
import { tr } from '../../../i18n/locale';
import { useEffect, useRef, useState, type CSSProperties } from 'react';
import { Card, Row } from '../rows';
import { useT } from '../../../theme/ThemeContext';
import { Icon } from '../../Icon';
import type { PageContentProps } from '../SettingsScreen';
import { PROVIDERS, providerById, providersFromCatalog } from '../../../../shared/providers';
import { modelCapabilitySummary, modelReference, selectedModelIdsForPickerSettings, waitForModelSelection } from '../../../bridge/modelCatalog';

export function initialProviderSelection(initialProviderId?: string): string {
  return providerById(logicalProviderId(initialProviderId ?? ''))?.id ?? 'anthropic';
}

export function ProviderCredentials({ bridge, onLayerLockChange, initialProviderId, pendingModelReference: requestedModelReference, onClose }: PageContentProps) {
  useI18n();
  const t = useT();
  const snapshot = bridge.bootstrap;
  const [selectedProviderId, setSelectedProviderId] = useState<string | null>(() => initialProviderId ? initialProviderSelection(initialProviderId) : null);
  const [pendingModelReference, setPendingModelReference] = useState(requestedModelReference);
  const [applyError, setApplyError] = useState<string>();
  const [applying, setApplying] = useState(false);
  const [search, setSearch] = useState('');
  const searchInput = useRef<HTMLInputElement>(null);
  const directoryList = useRef<HTMLUListElement>(null);
  const backButton = useRef<HTMLButtonElement>(null);
  const previousProviderId = useRef(selectedProviderId);
  useEffect(() => {
    const previousId = previousProviderId.current;
    if (selectedProviderId === previousId) return;
    previousProviderId.current = selectedProviderId;
    if (selectedProviderId !== null) {
      backButton.current?.focus();
    } else {
      const row = previousId && directoryList.current?.querySelector<HTMLButtonElement>(
        `[data-testid="provider-list-item-${CSS.escape(previousId)}"]`,
      );
      (row || searchInput.current)?.focus();
    }
  }, [selectedProviderId]);
  const [accountSnapshot, setAccountSnapshot] = useState<ProviderAccountSnapshot | undefined>(snapshot?.providerAccountSnapshot);
  const [accountStatusError, setAccountStatusError] = useState(false);
  const [refreshingAccounts, setRefreshingAccounts] = useState(false);
  const accountRefresh = useRef(0);
  useEffect(() => {
    setAccountSnapshot(snapshot?.providerAccountSnapshot);
  }, [snapshot?.providerAccountSnapshot]);
  const refreshAccounts = async () => {
    const generation = ++accountRefresh.current;
    setRefreshingAccounts(true);
    try {
      if (!bridge.providerAccounts) throw new Error('Account status unavailable');
      const next = await bridge.providerAccounts.snapshot();
      if (accountRefresh.current === generation) { setAccountSnapshot(next); setAccountStatusError(false); }
    } catch {
      if (accountRefresh.current === generation) setAccountStatusError(true);
    } finally {
      if (accountRefresh.current === generation) setRefreshingAccounts(false);
    }
  };
  useEffect(() => {
    if (selectedProviderId !== null) return;
    void refreshAccounts();
    const timer = setInterval(() => { void refreshAccounts(); }, 15_000);
    return () => { ++accountRefresh.current; clearInterval(timer); };
  }, [selectedProviderId, bridge.providerAccounts]);
  const currentModel = useRef(bridge.desktop.currentModel);
  currentModel.current = bridge.desktop.currentModel;
  const abort = useRef<AbortController>();
  useEffect(() => () => abort.current?.abort(), []);
  useEffect(() => { void bridge.refreshModelPicker().catch(() => undefined); }, [bridge.refreshModelPicker]);
  const catalogProviders = providersFromCatalog(bridge.desktop.providerModelCatalog ?? []);
  // Presets make credential management reachable without an engine. They do
  // not assert that any account or model is available for inference.
  const usingPresets = catalogProviders.length === 0;
  const officialProviders = (usingPresets ? PROVIDERS : catalogProviders).filter(provider => provider.id !== 'openai-chatgpt');
  const query = search.trim().toLocaleLowerCase();
  const visibleProviders = officialProviders.filter(provider => `${provider.label} ${provider.id}`.toLocaleLowerCase().includes(query));
  const activeModel = modelReference(bridge.desktop.currentModel ?? '');
  const activeProviderId = logicalProviderId(activeModel.providerId ?? '');
  const activeModelLabel = bridge.desktop.providerModelCatalog
    ?.find(entry => entry.provider_id === activeModel.providerId)
    ?.models.find(model => model.reference === activeModel.reference)?.display_name || activeModel.label;
  const selectedProvider = officialProviders.find(provider => provider.id === selectedProviderId) ?? providerById(selectedProviderId ?? '') ?? PROVIDERS[0];
  const providerCatalogEntry = bridge.desktop.providerModelCatalog?.find(entry => entry.provider_id === selectedProvider.id);
  const visibility = snapshot?.settings.modelPickerVisibility?.[selectedProvider.id];
  const selectedModelIds = selectedModelIdsForPickerSettings(providerCatalogEntry?.models.map(model => model.model_id) ?? [], visibility);
  const theme = { '--pa-bg': t.surface, '--pa-text': t.text, '--pa-secondary': t.text2, '--pa-border': t.border, '--pa-accent': t.accent, '--pa-ok': t.ok, '--pa-danger': t.danger, '--pa-warn': t.warn } as CSSProperties;
  const writeVisibility = (next: { showInModelPicker?: boolean; visibleModelIds?: string[] }) => {
    void bridge.setModelPickerVisibility({ ...snapshot?.settings.modelPickerVisibility, [selectedProvider.id]: next });
  };
  const configured = async () => {
    await bridge.refreshModelPicker('manual');
    if (!pendingModelReference || applying) return;
    abort.current?.abort(); const controller = new AbortController(); abort.current = controller;
    setApplying(true); setApplyError(undefined);
    try {
      await bridge.setModel(pendingModelReference);
      await waitForModelSelection(pendingModelReference, () => currentModel.current, { signal: controller.signal });
      if (!controller.signal.aborted) onClose();
    } catch (cause) { if (!controller.signal.aborted) setApplyError(cause instanceof Error ? cause.message : tr('provider_accounts_error')); }
    finally { if (!controller.signal.aborted) setApplying(false); }
  };
  if (selectedProviderId === null) return <div className="provider-settings-content provider-directory" style={theme}>
    <header className="provider-directory-intro">
      <h2>{tr('provider_accounts_all_providers')}</h2>
      <p>{tr('provider_accounts_directory_desc')}</p>
    </header>
    <div className="provider-directory-tools">
      <label className="provider-directory-search">
        <Icon name="search" size={16} />
        <input
          ref={searchInput}
          type="search"
          aria-label={tr('provider_accounts_search')}
          placeholder={tr('provider_accounts_search')}
          value={search}
          onChange={event => setSearch(event.target.value)}
          onKeyDown={event => {
            if (event.key !== 'Escape' || !search) return;
            event.preventDefault();
            event.stopPropagation();
            setSearch('');
          }}
        />
      </label>
      <button type="button" className="pa-icon-button" aria-label={tr('provider_accounts_refresh')} title={tr('provider_accounts_refresh')} aria-disabled={refreshingAccounts} onClick={() => {
        if (refreshingAccounts) return;
        void refreshAccounts();
        void bridge.refreshModelPicker('manual').catch(() => undefined);
      }}>
        {refreshingAccounts ? <span className="pa-spinner" /> : <Icon name="refresh" />}
      </button>
    </div>
    {visibleProviders.length > 0 ? <ul ref={directoryList} className="provider-directory-list" role="list">
      {visibleProviders.map(provider => {
        const summary = accountSnapshot ? providerAccountSummary(accountSnapshot, provider.id) : undefined;
        const status = accountStatusError ? 'unknown' : summary?.status ?? 'loading';
        const isActive = provider.id === activeProviderId;
        const statusLabel = tr(`provider_accounts_configuration_${status}`);
        const showStatusLabel = status !== 'configured' && status !== 'unconfigured';
        return <li key={provider.id}>
          <button type="button" className="settings-provider-row" data-current={isActive} data-testid={`provider-list-item-${provider.id}`} onClick={() => { setPendingModelReference(undefined); setApplyError(undefined); setSelectedProviderId(provider.id); }}>
            <span className="provider-monogram" aria-hidden="true">{provider.label.slice(0, 1)}</span>
            <span className="provider-directory-copy">
              <strong>{provider.label}</strong>
              {isActive
                ? <span className="provider-current-model">{tr('composer_current_model')} · {activeModelLabel}</span>
                : provider.description && <span>{provider.description}</span>}
            </span>
            <span className="provider-directory-status" data-status={status}>
              <span className="provider-status-label">
                {showStatusLabel
                  ? <><span className="provider-status-dot" aria-hidden="true" />{statusLabel}</>
                  : <span className="provider-status-dot" role="img" aria-label={statusLabel} title={statusLabel} />}
              </span>
              {!accountStatusError && summary && summary.total > 0 && <small>{tr('provider_accounts_configuration_count', summary.enabled, summary.total)}</small>}
            </span>
            <Icon name="chevronR" size={16} />
          </button>
        </li>;
      })}
    </ul> : <div className="provider-directory-empty" role="status">
      <Icon name="search" size={20} />
      <p>{tr('provider_accounts_no_matches')}</p>
    </div>}
    {usingPresets && <p className="provider-directory-note">{tr('provider_accounts_presets_only')}</p>}
    <CloudProviders onPulled={async () => { await bridge.reloadProviderSettings?.().catch(() => undefined); await refreshAccounts(); await bridge.refreshModelPicker('manual').catch(() => undefined); }} />
  </div>;
  return <div className="provider-settings-content" style={theme}>
    <button ref={backButton} type="button" className="settings-provider-back" data-testid="provider-list-back" disabled={applying} onClick={() => setSelectedProviderId(null)}>
      <Icon name="chevronL" size={15} />{tr('provider_accounts_all_providers')}
    </button>
    <header className="provider-detail-heading">
      <span className="provider-monogram" aria-hidden="true">{selectedProvider.label.slice(0, 1)}</span>
      <div><h2>{selectedProvider.label}</h2>{selectedProvider.description && <p>{selectedProvider.description}</p>}</div>
    </header>
    {bridge.providerSettingsApplyPending && <div className="pa-feedback pa-error" role="alert"><span>{bridge.providerSettingsError}</span><button className="pa-button" disabled={applying} onClick={() => { setApplying(true); void bridge.updateProviderSettings({}).catch(cause => setApplyError(cause instanceof Error ? cause.message : tr('provider_accounts_error'))).finally(() => setApplying(false)); }}>{tr('common_retry')}</button></div>}
    {bridge.providerAccounts ? <ProviderAccounts key={selectedProvider.id} api={bridge.providerAccounts} provider={selectedProvider} running={bridge.running || applying} onConfigured={configured} usageRows={bridge.cost?.by_account} cloudSyncable={VAULT_PRESET_IDS.has(logicalProviderId(selectedProvider.id))} /> : <p role="alert">{tr('provider_accounts_status_error')}</p>}
    {applyError && <p role="alert" style={{ color: t.danger }}>{applyError}<button type="button" disabled={applying} onClick={() => void configured()} className="pa-button">{tr('desktop_settings_00a58a5b98')}</button></p>}
    <details className="provider-settings-disclosure">
      <summary>
        <span className="provider-disclosure-icon" aria-hidden="true"><Icon name="spark" size={16} /></span>
        <span className="provider-disclosure-copy">
          <strong>{tr('provider_accounts_models_title')}</strong>
          {providerCatalogEntry && <small>{tr('desktop_settings_28e11f6cd1', selectedModelIds.length, providerCatalogEntry.models.length)}</small>}
        </span>
        <Icon name="chevron" size={15} />
      </summary>
      <Card>
        {!providerCatalogEntry && <Row title={tr('desktop_settings_71a4b21d4a')}><span /></Row>}
        {selectedProvider.available && providerCatalogEntry && (
          <Row
            title={tr('desktop_settings_6a04127f16')}
            align="center"
          >
            <label className="pa-switch provider-visibility-switch">
              <input
                type="checkbox"
                role="switch"
                aria-label={tr('desktop_settings_6a04127f16')}
                checked={visibility?.showInModelPicker !== false}
                onChange={(event) => writeVisibility({
                  showInModelPicker: event.currentTarget.checked,
                  visibleModelIds: visibility?.visibleModelIds,
                })}
              />
              <span aria-hidden="true" />
            </label>
          </Row>
        )}
        {selectedProvider.available && providerCatalogEntry?.models.map((entry) => {
          const selected = selectedModelIds.includes(entry.model_id);
          return (
            <Row
              key={entry.reference}
              title={entry.display_name || entry.model_id}
              desc={[entry.model_id, modelCapabilitySummary(entry)].filter(Boolean).join(' · ')}
              align="center"
            >
              <label className="pa-switch provider-visibility-switch">
                <input
                  type="checkbox"
                  role="switch"
                  checked={selected}
                  aria-label={`${tr('desktop_settings_1d237711e4')} · ${entry.display_name || entry.model_id}`}
                  onChange={() => {
                    const base = visibility?.visibleModelIds ?? providerCatalogEntry.models.map((model) => model.model_id);
                    const next = selected
                      ? base.filter((modelId) => modelId !== entry.model_id)
                      : [...base, entry.model_id];
                    writeVisibility({
                      showInModelPicker: visibility?.showInModelPicker,
                      visibleModelIds: [...new Set(next)],
                    });
                  }}
                />
                <span aria-hidden="true" />
              </label>
            </Row>
          );
        })}
        {selectedProvider.available && visibility?.showInModelPicker !== false && providerCatalogEntry && selectedModelIds.length === 0 && (
          <Row title={tr('desktop_settings_f56c6c8220')} desc={tr('settings_provider_picker_hidden_empty')} align="center">
            <Icon name="warning" size={15} color={t.warn} />
          </Row>
        )}
      </Card>
    </details>
    <ProviderProtocol providerId={selectedProvider.id} bridge={bridge} onLayerLockChange={onLayerLockChange} />
    <p className="provider-directory-note">{tr('provider_accounts_global')}</p>
  </div>;
}
