import { useI18n } from '../../../i18n/useI18n';
import { tr } from '../../../i18n/locale';
import { useEffect, useRef, useState, type CSSProperties } from 'react';
import { Card, Row } from '../rows';
import { useT } from '../../../theme/ThemeContext';
import { Icon } from '../../Icon';
import type { PageContentProps } from '../SettingsScreen';
import { PROVIDERS, providerById } from '../../../../shared/providers';
import type { ProviderCredentialMetadata } from '../../../bridge/lingxi';
import type { ProviderConnectionTestResult } from '../../../bridge/lingxi';
import { isCurrentCredentialTransaction, persistProviderCredentialAndApplyModel } from '../../../bridge/providerCredentials';
import {
  modelCapabilitySummary,
  modelReference,
  selectedModelIdsForPickerSettings,
  waitForModelSelection,
} from '../../../bridge/modelCatalog';
import { ghostButtonStyle } from './ghostButton';
import { ProviderProtocol } from './ProviderProtocol';

/**
 * Which provider is pre-selected on mount. Pure so it's testable without
 * mounting anything — same shape `Projects.tsx`'s `projectRows` uses.
 * Mirrors the old settings modal's own
 * `providerById(initialProviderId ?? '') ? initialProviderId! : 'anthropic'`
 * (`BetaDesktop.tsx`, retired in Task 20), which is how the composer's model-picker deep link
 * (`SettingsRoute.providerId`) lands this page on the provider actually
 * blocking the chosen model instead of always opening on Anthropic.
 */
export function initialProviderSelection(initialProviderId?: string): string {
  return providerById(initialProviderId ?? '') ? initialProviderId! : 'anthropic';
}

export type CredentialStatusKind = 'runtime' | 'secure' | 'fallback-configured' | 'fallback-unconfigured' | 'unavailable' | 'none';
export type ProviderCredentialStatusKind = 'configured' | 'runtime' | 'unconfigured' | 'unavailable';

export interface ProviderCredentialStatus {
  kind: ProviderCredentialStatusKind;
  label: string;
}

/**
 * Selected-provider storage status. Broker/signing failures win so the user
 * gets an actionable configuration error; runtime-only and legacy non-macOS
 * fallback states remain distinguishable from Data Protection Keychain.
 */
export function credentialStatusKind(metadata?: Pick<ProviderCredentialMetadata, 'configured' | 'encryptionAvailable' | 'runtimeOnly' | 'storageError'>): CredentialStatusKind {
  if (!metadata) return 'none';
  if (metadata.storageError) return 'unavailable';
  if (metadata.runtimeOnly) return 'runtime';
  if (metadata.configured && metadata.encryptionAvailable) return 'secure';
  if (metadata.configured && !metadata.encryptionAvailable) return 'fallback-configured';
  if (!metadata.configured && metadata.encryptionAvailable === false) return 'fallback-unconfigured';
  return 'none';
}

/** The list reports authoritative credential configuration, not a persistent network connection. */
export function providerCredentialStatus(
  metadata: Pick<ProviderCredentialMetadata, 'configured' | 'runtimeOnly' | 'storageError'> | undefined,
  providerAvailable: boolean,
): ProviderCredentialStatus {
  if (!providerAvailable) return { kind: 'unavailable', label: 'CLI / TUI' };
  if (metadata?.storageError) return { kind: 'unavailable', label: tr('settings_provider_secure_storage_unavailable') };
  if (metadata?.runtimeOnly) return { kind: 'runtime', label: tr('desktop_settings_59f46d1810') };
  if (metadata?.configured) return { kind: 'configured', label: tr('settings_provider_status_configured') };
  return { kind: 'unconfigured', label: tr('settings_provider_unconfigured') };
}

/** Saving a new value replaces any existing credential; the action name stays stable. */
export function credentialSaveButtonLabel(state: { saving: boolean; modelApplying: boolean }): string {
  if (state.saving) return tr('desktop_settings_ff509c9ba0');
  if (state.modelApplying) return tr('desktop_settings_de5336c85d');
  return tr('voice_save_button');
}

export function shouldRequestCredentialPreview(
  providerId: string | null,
  metadata: Pick<ProviderCredentialMetadata, 'configured' | 'credentialPreview' | 'storageError'> | undefined,
  requestedProviderIds: ReadonlySet<string>,
  credentialSourceAvailable: boolean,
): providerId is string {
  return providerId !== null
    && credentialSourceAvailable
    && metadata?.configured === true
    && !metadata.credentialPreview
    && !metadata.storageError
    && !requestedProviderIds.has(providerId);
}

/** Built-in Provider credentials use their declared official API endpoints. */
export function ProviderCredentials({ bridge, initialProviderId, pendingModelReference: requestedModelReference, onClose, snapshot: engineSnapshot, editingLayer, onJumpToLayer, onLayerLockChange }: PageContentProps) {
  useI18n();
  const t = useT();
  const snapshot = bridge.bootstrap;
  const [key, setKey] = useState('');
  const [selectedProviderId, setSelectedProviderId] = useState<string | null>(() => (
    initialProviderId ? initialProviderSelection(initialProviderId) : null
  ));
  const [pendingModelReference, setPendingModelReference] = useState<string | null>(requestedModelReference ?? null);
  const [connecting, setConnecting] = useState(false);
  const [clearing, setClearing] = useState(false);
  const [saveError, setSaveError] = useState<string | null>(null);
  const [applyError, setApplyError] = useState<string | null>(null);
  const [modelApplying, setModelApplying] = useState(false);
  const [testingConnection, setTestingConnection] = useState(false);
  const [connectionTestResult, setConnectionTestResult] = useState<ProviderConnectionTestResult | null>(null);
  const [connectionTestError, setConnectionTestError] = useState<string | null>(null);

  const selectedProvider = providerById(selectedProviderId ?? '') ?? PROVIDERS[0];
  const selectedMetadata = snapshot?.providerCredentials?.find((entry) => entry.providerId === selectedProvider.id);
  const statusKind = credentialStatusKind(selectedMetadata);
  // Key the OAuth branches on the capability, not on one provider id: this is a
  // multi-provider product, and `authMethod` is declared required on every
  // PROVIDERS entry precisely so a new OAuth provider cannot land without
  // answering. Keying on the id gave any future OAuth provider the API-key
  // field and no sign-in button.
  const isCodexAuth = selectedProvider.authMethod === 'oauth';
  const oauthStorageUnavailable = Boolean(selectedMetadata?.storageError) || selectedMetadata?.encryptionAvailable === false;
  const providerModelCatalog = bridge.desktop.providerModelCatalog ?? [];
  const providerCatalogEntry = providerModelCatalog.find((entry) => entry.provider_id === selectedProvider.id);
  const visibility = snapshot?.settings.modelPickerVisibility?.[selectedProvider.id];
  const selectedModelIds = selectedModelIdsForPickerSettings(
    providerCatalogEntry?.models.map((model) => model.model_id) ?? [],
    visibility,
  );

  const mountedRef = useRef(true);
  const transactionGenerationRef = useRef(0);
  const applyGenerationRef = useRef(0);
  const applyAbortRef = useRef<AbortController | null>(null);
  const currentModelRef = useRef<string | null>(bridge.desktop.currentModel);
  const credentialRef = useRef<HTMLInputElement>(null);
  const requestedPreviewProvidersRef = useRef(new Set<string>());
  const connectionTestGenerationRef = useRef(0);
  const oauthActiveRef = useRef(false);
  const cancelCodexRef = useRef(bridge.cancelCodexLogin);
  cancelCodexRef.current = bridge.cancelCodexLogin;
  currentModelRef.current = bridge.desktop.currentModel;

  useEffect(() => {
    mountedRef.current = true;
    return () => {
      mountedRef.current = false;
      ++transactionGenerationRef.current;
      ++applyGenerationRef.current;
      ++connectionTestGenerationRef.current;
      applyAbortRef.current?.abort();
      if (oauthActiveRef.current) void cancelCodexRef.current().catch(() => {});
    };
  }, []);

  // Fix round 1: this focus move was dropped in the lift. The old settings
  // modal (`BetaDesktop.tsx:1923-1926`, retired in Task 20) moved focus into the credential input when
  // the composer's model picker deep-links here (both `initialProviderId`
  // and the requested model are set) — arriving from that flow means the
  // person is here specifically to type a key, so the cursor should already
  // be waiting there instead of leaving them to find the field themselves.
  useEffect(() => {
    if (!requestedModelReference || !initialProviderId) return;
    window.requestAnimationFrame(() => credentialRef.current?.focus());
  }, [initialProviderId, requestedModelReference]);

  // The provider list asks only for account attributes. Fetch one masked
  // preview lazily after selection; masking happens inside the native broker,
  // so the renderer never receives the complete credential.
  useEffect(() => {
    if (isCodexAuth || !shouldRequestCredentialPreview(
      selectedProviderId,
      selectedMetadata,
      requestedPreviewProvidersRef.current,
      bridge.connected || snapshot?.credentialBrokerAvailable === true,
    )) return;
    requestedPreviewProvidersRef.current.add(selectedProviderId);
    void bridge.refreshProviderCredential(selectedProviderId);
  }, [bridge, isCodexAuth, selectedMetadata, selectedProviderId, snapshot?.credentialBrokerAvailable]);

  const applyPendingModel = async (
    allowWhileConnecting = false,
    transactionGeneration = transactionGenerationRef.current,
  ): Promise<boolean> => {
    const requestedModel = pendingModelReference;
    if (!isCurrentCredentialTransaction(mountedRef.current, transactionGeneration, transactionGenerationRef.current)
      || !requestedModel || (connecting && !allowWhileConnecting) || modelApplying) return false;
    const generation = ++applyGenerationRef.current;
    applyAbortRef.current?.abort();
    const abortController = new AbortController();
    applyAbortRef.current = abortController;
    setModelApplying(true);
    setApplyError(null);
    try {
      // setModel only confirms command delivery. Wait for the authoritative
      // model_changed/model_list state before returning to chat.
      await bridge.setModel(requestedModel);
      await waitForModelSelection(requestedModel, () => currentModelRef.current, { signal: abortController.signal });
      if (generation !== applyGenerationRef.current
        || !isCurrentCredentialTransaction(mountedRef.current, transactionGeneration, transactionGenerationRef.current)) return false;
      setPendingModelReference(null);
      onClose();
      return true;
    } catch (cause) {
      if (generation === applyGenerationRef.current
        && isCurrentCredentialTransaction(mountedRef.current, transactionGeneration, transactionGenerationRef.current)) {
        setApplyError(cause instanceof Error ? cause.message : tr('desktop_settings_1bf27e20c7'));
      }
      return false;
    } finally {
      if (generation === applyGenerationRef.current
        && isCurrentCredentialTransaction(mountedRef.current, transactionGeneration, transactionGenerationRef.current)) {
        setModelApplying(false);
        applyAbortRef.current = null;
      }
    }
  };

  const save = async () => {
    const submitted = key;
    if (!submitted.trim() || connecting || !mountedRef.current) return;
    const transactionGeneration = ++transactionGenerationRef.current;
    setConnecting(true);
    setSaveError(null);
    setApplyError(null);
    setConnectionTestResult(null);
    setConnectionTestError(null);
    try {
      await persistProviderCredentialAndApplyModel(
        selectedProvider.id,
        submitted,
        bridge.setProviderCredential,
        () => {
          if (isCurrentCredentialTransaction(mountedRef.current, transactionGeneration, transactionGenerationRef.current)) {
            setKey((current) => current === submitted ? '' : current);
          }
        },
        pendingModelReference,
        () => applyPendingModel(true, transactionGeneration),
      );
    } catch (cause) {
      if (isCurrentCredentialTransaction(mountedRef.current, transactionGeneration, transactionGenerationRef.current)) {
        setSaveError(cause instanceof Error ? cause.message : tr('desktop_settings_bc6770887a'));
      }
    } finally {
      if (isCurrentCredentialTransaction(mountedRef.current, transactionGeneration, transactionGenerationRef.current)) {
        setConnecting(false);
      }
    }
  };

  const loginCodex = async () => {
    if (connecting || modelApplying || bridge.running || oauthStorageUnavailable || oauthActiveRef.current) return;
    const generation = ++transactionGenerationRef.current;
    oauthActiveRef.current = true;
    setConnecting(true);
    setSaveError(null);
    setApplyError(null);
    try {
      await bridge.loginCodex();
      if (isCurrentCredentialTransaction(mountedRef.current, generation, transactionGenerationRef.current)
        && pendingModelReference) await applyPendingModel(true, generation);
    } catch (cause) {
      if (isCurrentCredentialTransaction(mountedRef.current, generation, transactionGenerationRef.current)) {
        setSaveError(cause instanceof Error ? cause.message : tr('desktop_settings_07d73325e7'));
      }
    } finally {
      oauthActiveRef.current = false;
      if (isCurrentCredentialTransaction(mountedRef.current, generation, transactionGenerationRef.current)) setConnecting(false);
    }
  };

  const cancelCodexLogin = async () => {
    if (!oauthActiveRef.current) return;
    try {
      await bridge.cancelCodexLogin();
    } catch (cause) {
      if (mountedRef.current) setSaveError(cause instanceof Error ? cause.message : tr('desktop_settings_02d84a144f'));
    }
  };

  const selectProvider = (providerId: string) => {
    if (connecting || modelApplying || testingConnection) return;
    setSelectedProviderId(providerId);
    setKey('');
    // A manual provider change cancels the model intent from the picker.
    setPendingModelReference(null);
    setSaveError(null);
    setApplyError(null);
    setConnectionTestResult(null);
    setConnectionTestError(null);
    ++applyGenerationRef.current;
    applyAbortRef.current?.abort();
  };

  const showProviderList = () => {
    if (transactionLocked) return;
    setSelectedProviderId(null);
    setKey('');
    setSaveError(null);
    setApplyError(null);
    setConnectionTestResult(null);
    setConnectionTestError(null);
  };

  const testConnection = async () => {
    if (testingConnection || bridge.running || !bridge.connected) return;
    const providerId = selectedProvider.id;
    const generation = ++connectionTestGenerationRef.current;
    setTestingConnection(true);
    setConnectionTestResult(null);
    setConnectionTestError(null);
    try {
      const result = await bridge.testProviderConnection(providerId, key.trim() ? key : undefined);
      if (mountedRef.current
        && generation === connectionTestGenerationRef.current
        && selectedProviderId === providerId) {
        setConnectionTestResult(result);
      }
    } catch (cause) {
      if (mountedRef.current
        && generation === connectionTestGenerationRef.current
        && selectedProviderId === providerId) {
        setConnectionTestError(cause instanceof Error ? cause.message : tr('desktop_settings_abadf257cb'));
      }
    } finally {
      if (mountedRef.current && generation === connectionTestGenerationRef.current) {
        setTestingConnection(false);
      }
    }
  };

  const clearCredential = async () => {
    if (connecting || clearing || modelApplying || testingConnection) return;
    const generation = ++transactionGenerationRef.current;
    setClearing(true);
    ++connectionTestGenerationRef.current;
    setConnectionTestResult(null);
    setConnectionTestError(null);
    setSaveError(null);
    try {
      await bridge.clearProviderCredential(selectedProvider.id);
    } catch (cause) {
      if (isCurrentCredentialTransaction(mountedRef.current, generation, transactionGenerationRef.current)) {
        setSaveError(cause instanceof Error ? cause.message : tr('desktop_settings_58cd253c17'));
      }
    } finally {
      if (isCurrentCredentialTransaction(mountedRef.current, generation, transactionGenerationRef.current)) setClearing(false);
    }
  };

  const writeVisibility = (next: { showInModelPicker?: boolean; visibleModelIds?: string[] }) => {
    const current = snapshot?.settings.modelPickerVisibility ?? {};
    void bridge.setModelPickerVisibility({
      ...current,
      [selectedProvider.id]: next,
    });
  };

  const statusMessage = saveError ?? applyError;
  const busy = connecting || clearing || modelApplying;
  const transactionLocked = busy || testingConnection;
  const credentialWriteDisabled = transactionLocked || (isCodexAuth && bridge.running);
  const canTestConnection = selectedProvider.available
    && bridge.connected
    && !bridge.running
    && !transactionLocked
    && Boolean(key.trim() || selectedMetadata?.configured || selectedMetadata?.runtimeOnly);
  const testMessage = connectionTestError ?? connectionTestResult?.message;
  const testMessageColor = connectionTestError
    ? t.danger
    : connectionTestResult?.connected
      ? t.ok
      : connectionTestResult?.reachable
        ? t.warn
        : t.danger;
  return (
    <>
      {selectedProviderId === null ? (
        <Card title={tr('model_detail_provider')}>
          <Row title={tr('desktop_settings_295f21535b')} desc={tr('desktop_settings_c9b4966c44')} align="center">
            <span />
          </Row>
          {PROVIDERS.map((provider) => {
            const metadata = snapshot?.providerCredentials?.find((entry) => entry.providerId === provider.id);
            const status = providerCredentialStatus(metadata, provider.available);
            const configured = status.kind === 'configured' || status.kind === 'runtime';
            const statusColor = configured
              ? t.ok
              : metadata?.storageError
                ? t.danger
                : t.text3;
            return (
              <button
                key={provider.id}
                type="button"
                data-testid={`provider-list-item-${provider.id}`}
                disabled={transactionLocked}
                onClick={() => selectProvider(provider.id)}
                className="settings-provider-row"
                aria-label={`${provider.label}，${status.label}`}
                style={{
                  '--settings-provider-row-hover': t.surfaceHover,
                  width: '100%', minHeight: 76, padding: '15px 18px',
                  border: 0, borderTop: `0.5px solid ${t.border}`,
                  background: 'transparent', color: t.text, font: 'inherit',
                  display: 'flex', alignItems: 'center', gap: 16,
                  textAlign: 'left', cursor: transactionLocked ? 'default' : 'pointer',
                } as CSSProperties}
              >
                <span style={{ flex: 1, minWidth: 0 }}>
                  <span style={{ display: 'block', fontSize: 14, fontWeight: 500, color: t.text }}>
                    {provider.label}
                  </span>
                  <span style={{ display: 'block', marginTop: 4, fontSize: 12.5, lineHeight: 1.45, color: t.text3 }}>
                    {provider.available ? provider.description : tr('desktop_settings_ff2973f7ec')}
                  </span>
                </span>
                <span style={{ display: 'inline-flex', alignItems: 'center', justifyContent: 'flex-end', gap: 7, minWidth: 96, color: statusColor, fontSize: 12.5, fontWeight: 500 }}>
                  <Icon name="dot" size={10} color={statusColor} />
                  {status.label}
                </span>
                <Icon name="chevronR" size={15} color={t.text4} stroke={1.8} />
              </button>
            );
          })}
        </Card>
      ) : (
        <>
          <div style={{ display: 'flex', alignItems: 'center', gap: 10, minHeight: 40, margin: '-8px 0 16px' }}>
            <button
              type="button"
              data-testid="provider-list-back"
              disabled={transactionLocked}
              onClick={showProviderList}
              className="settings-provider-back"
              style={{
                '--settings-provider-row-hover': t.surfaceHover,
                minHeight: 40, padding: '0 10px 0 7px', border: 0, borderRadius: 8,
                background: 'transparent', color: t.text2, font: 'inherit', fontSize: 13,
                fontWeight: 500, display: 'inline-flex', alignItems: 'center', gap: 5,
                cursor: transactionLocked ? 'default' : 'pointer',
              } as CSSProperties}
            >
              <Icon name="chevronL" size={15} color="currentColor" stroke={1.9} />
              Provider
            </button>
            <span aria-hidden="true" style={{ width: 1, height: 16, background: t.border }} />
            <span style={{ color: t.text, fontSize: 13, fontWeight: 600 }}>{selectedProvider.label}</span>
          </div>

          <ProviderProtocol providerId={selectedProvider.id} bridge={bridge} snapshot={engineSnapshot}
            editingLayer={editingLayer} onJumpToLayer={onJumpToLayer} onLayerLockChange={onLayerLockChange} />
          <Card title={selectedProvider.label}>
        {pendingModelReference && (
          <Row title={tr('desktop_settings_148ae692c1')} desc={tr('desktop_settings_d4c606e559', isCodexAuth ? tr('desktop_settings_1567161eda') : tr('desktop_settings_f30a4dcfe1', selectedProvider.label), modelReference(pendingModelReference).label)} align="center">
            <Icon name="spark" size={15} color={t.accent} />
          </Row>
        )}
        {statusKind === 'runtime' && (
          <Row title={tr('audio_source_title')} desc={tr('desktop_settings_22d84d77c9')} align="center"><span /></Row>
        )}
        {selectedMetadata?.environmentOverride && (
          <Row title={tr('desktop_settings_3af9fa5cc6')} desc={tr('desktop_settings_5f688b5996')} align="center"><span /></Row>
        )}
        {statusKind === 'secure' && (
          <Row title={tr('desktop_settings_0326a778db')} desc={tr('desktop_settings_d7b0a92ce6')} align="center"><span /></Row>
        )}
        {isCodexAuth && oauthStorageUnavailable && !selectedMetadata?.storageError && (
          <Row title={tr('settings_provider_secure_storage_unavailable')} desc={tr('desktop_settings_b7207229c4')} align="center">
            <Icon name="shieldAlert" size={15} color={t.danger} />
          </Row>
        )}
        {statusKind === 'unavailable' && (
          <Row title={tr('settings_provider_secure_storage_unavailable')} desc={selectedMetadata?.storageError} align="center">
            <Icon name="shieldAlert" size={15} color={t.danger} />
          </Row>
        )}
        {!isCodexAuth && statusKind === 'fallback-configured' && (
          <Row title={tr('desktop_settings_0326a778db')} desc={tr('desktop_settings_2fe8aeba2d')} align="center"><span /></Row>
        )}
        {!isCodexAuth && statusKind === 'fallback-unconfigured' && (
          <Row title={tr('desktop_settings_0326a778db')} desc={tr('desktop_settings_092a6dff70')} align="center"><span /></Row>
        )}
        {isCodexAuth ? (
          <Row title={tr('desktop_settings_1dcf32a354')} desc={connecting
            ? tr('desktop_settings_15706d2ff9')
            : selectedMetadata?.codexAccountEmail
              ? tr('desktop_settings_9e3cec77a5', selectedMetadata.codexAccountEmail)
              : tr('desktop_settings_785d9d7e44')} align="center">
            <div style={{ display: 'flex', gap: 7 }}>
              <button type="button" data-testid="codex-login" disabled={credentialWriteDisabled || oauthStorageUnavailable}
                onClick={() => void loginCodex()} style={ghostButtonStyle(t, credentialWriteDisabled || oauthStorageUnavailable)}>
                {connecting ? tr('desktop_settings_8e8e57279f') : modelApplying ? tr('desktop_settings_de5336c85d') : selectedMetadata?.configured ? tr('desktop_settings_957244cdb9') : tr('settings_provider_login')}
              </button>
              {connecting && !modelApplying && (
                <button type="button" data-testid="codex-login-cancel" onClick={() => void cancelCodexLogin()} style={ghostButtonStyle(t, false)}>{tr('desktop_settings_531c764f9b')}</button>
              )}
              {selectedMetadata?.configured && !selectedMetadata.runtimeOnly && (
                <button type="button" disabled={credentialWriteDisabled} onClick={() => void clearCredential()} style={ghostButtonStyle(t, credentialWriteDisabled, true)}>{tr('settings_logout')}</button>
              )}
            </div>
          </Row>
        ) : selectedProvider.available ? (
          <Row title={selectedProvider.keyLabel} desc={selectedMetadata?.configured ? tr('desktop_settings_1085f7e2c0') : undefined} align="center">
            <div style={{ display: 'flex', flexDirection: 'column', alignItems: 'flex-end', gap: 7 }}>
              <input
                ref={credentialRef}
                type="password"
                autoComplete="off"
                disabled={credentialWriteDisabled}
                value={key}
                onChange={(event) => {
                  setKey(event.target.value);
                  setConnectionTestResult(null);
                  setConnectionTestError(null);
                }}
                onKeyDown={(event) => { if (event.key === 'Enter') { event.preventDefault(); void save(); } }}
                placeholder={selectedMetadata?.configured ? (selectedMetadata.credentialPreview ?? '••••••••') : selectedProvider.keyPlaceholder}
                aria-label={selectedProvider.keyLabel}
                data-testid="provider-credential-preview"
                style={{ padding: '6px 10px', borderRadius: 7, border: `0.5px solid ${t.border}`, background: t.surface, color: t.text, fontSize: 12.5, minWidth: 220 }}
              />
              <a
                href={selectedProvider.credentialManagementUrl}
                target="_blank"
                rel="noreferrer"
                style={{ color: t.link ?? t.accent, fontSize: 12, fontWeight: 500, textDecoration: 'none' }}
              >
                {tr('desktop_settings_aab39b9ba5')}</a>
            </div>
          </Row>
        ) : (
          <Row title={selectedProvider.keyLabel} desc={tr('desktop_settings_93dfefffab', selectedProvider.label)} align="center"><span /></Row>
        )}
        {selectedProvider.available && !isCodexAuth && (
          <Row
            title={tr('desktop_settings_200f465659')}
            desc={testMessage ? (
              <span role={connectionTestError ? 'alert' : 'status'} aria-live="polite" style={{ color: testMessageColor }}>
                {testMessage}
                {connectionTestResult && (
                  <span style={{ color: t.text4 }}>
                    {connectionTestResult.used_stored_credential ? tr('desktop_settings_71fa27d672') : tr('desktop_settings_e20d24b6e0')}
                  </span>
                )}
              </span>
            ) : tr('desktop_settings_544b542fc9')}
            align="center"
          >
            <button
              type="button"
              data-testid="provider-connection-test"
              disabled={!canTestConnection}
              onClick={() => void testConnection()}
              style={ghostButtonStyle(t, !canTestConnection)}
            >
              <Icon name="activity" size={13} color="currentColor" stroke={1.8} />
              {testingConnection ? tr('settings_provider_testing') : tr('settings_provider_test_connection')}
            </button>
          </Row>
        )}
        {selectedProvider.available && (
          <Row
            title={tr('desktop_settings_a148f7060d')}
            desc={providerCatalogEntry
              ? tr('desktop_settings_28e11f6cd1', selectedModelIds.length, providerCatalogEntry.models.length)
              : tr('desktop_settings_71a4b21d4a')}
            align="center"
          >
            <label style={{ display: 'inline-flex', alignItems: 'center', gap: 8, color: t.text2, fontSize: 12.5 }}>
              <input
                type="checkbox"
                checked={visibility?.showInModelPicker !== false}
                onChange={(event) => writeVisibility({
                  showInModelPicker: event.currentTarget.checked,
                  visibleModelIds: visibility?.visibleModelIds,
                })}
              />
              {tr('desktop_settings_6a04127f16')}</label>
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
              <label style={{ display: 'inline-flex', alignItems: 'center', gap: 8, color: t.text2, fontSize: 12.5 }}>
                <input
                  type="checkbox"
                  checked={selected}
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
                {tr('desktop_settings_1d237711e4')}</label>
            </Row>
          );
        })}
        {selectedProvider.available && visibility?.showInModelPicker !== false && providerCatalogEntry && selectedModelIds.length === 0 && (
          <Row title={tr('desktop_settings_f56c6c8220')} desc={tr('settings_provider_picker_hidden_empty')} align="center">
            <Icon name="warning" size={15} color={t.warn} />
          </Row>
        )}
        {selectedProvider.available && !isCodexAuth && (
          <Row
            title={tr('desktop_settings_23f2e626ca')}
            desc={selectedMetadata?.configured
              ? tr('desktop_settings_e105bf21f5')
              : tr('desktop_settings_13f024018d')}
            align="center"
          >
            <div style={{ display: 'flex', justifyContent: 'flex-end', gap: 7 }}>
              <button
                type="button"
                disabled={!key.trim() || credentialWriteDisabled}
                onClick={() => void save()}
                style={{
                  ...ghostButtonStyle(t, !key.trim() || credentialWriteDisabled),
                  width: 88,
                  boxSizing: 'border-box',
                  justifyContent: 'center',
                  whiteSpace: 'nowrap',
                }}
              >
                {credentialSaveButtonLabel({ saving: connecting, modelApplying })}
              </button>
              {selectedMetadata?.configured && !selectedMetadata.runtimeOnly && (
                <button type="button" disabled={credentialWriteDisabled} onClick={() => void clearCredential()} style={ghostButtonStyle(t, credentialWriteDisabled, true)}>
                  {tr('desktop_settings_fc150034cb')}</button>
              )}
            </div>
          </Row>
        )}
        {statusMessage && (
          <Row title={tr('cron_error_section')} align="center">
            <span role="alert" style={{ color: t.danger, fontSize: 12.5 }}>{statusMessage}</span>
          </Row>
        )}
        {applyError && pendingModelReference && (
          <Row title={tr('desktop_settings_d1b40bcf37')} align="center">
            <button type="button" disabled={busy} onClick={() => void applyPendingModel()} style={ghostButtonStyle(t, busy)}>{tr('desktop_settings_00a58a5b98')}</button>
          </Row>
        )}
          </Card>
        </>
      )}
    </>
  );
}
