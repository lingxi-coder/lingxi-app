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
  if (metadata?.storageError) return { kind: 'unavailable', label: '安全存储不可用' };
  if (metadata?.runtimeOnly) return { kind: 'runtime', label: '仅运行时' };
  if (metadata?.configured) return { kind: 'configured', label: '已配置' };
  return { kind: 'unconfigured', label: '未配置' };
}

/** Saving a new value replaces any existing credential; the action name stays stable. */
export function credentialSaveButtonLabel(state: { saving: boolean; modelApplying: boolean }): string {
  if (state.saving) return '保存中…';
  if (state.modelApplying) return '应用模型中…';
  return '保存';
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
export function ProviderCredentials({ bridge, initialProviderId, pendingModelReference: requestedModelReference, onClose }: PageContentProps) {
  const t = useT();
  const snapshot = bridge.bootstrap;
  const [key, setKey] = useState('');
  const [selectedProviderId, setSelectedProviderId] = useState<string | null>(() => (
    initialProviderId ? initialProviderSelection(initialProviderId) : null
  ));
  const [pendingModelReference, setPendingModelReference] = useState<string | null>(requestedModelReference ?? null);
  const [connecting, setConnecting] = useState(false);
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
        setApplyError(cause instanceof Error ? cause.message : '无法选择该模型。');
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
        setSaveError(cause instanceof Error ? cause.message : '无法保存该 Provider 凭据。');
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
        setSaveError(cause instanceof Error ? cause.message : 'ChatGPT 登录失败，请重试。');
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
      if (mountedRef.current) setSaveError(cause instanceof Error ? cause.message : '无法取消登录。');
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
        setConnectionTestError(cause instanceof Error ? cause.message : '连接测试失败。');
      }
    } finally {
      if (mountedRef.current && generation === connectionTestGenerationRef.current) {
        setTestingConnection(false);
      }
    }
  };

  const clearCredential = async () => {
    ++connectionTestGenerationRef.current;
    setConnectionTestResult(null);
    setConnectionTestError(null);
    setSaveError(null);
    try {
      await bridge.clearProviderCredential(selectedProvider.id);
    } catch (cause) {
      if (mountedRef.current) {
        setSaveError(cause instanceof Error ? cause.message : '无法删除该 Provider 凭据。');
      }
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
  const busy = connecting || modelApplying;
  const transactionLocked = busy || testingConnection;
  const credentialWriteDisabled = transactionLocked || bridge.running;
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
        <Card title="Provider">
          <Row title="选择 Provider" desc="凭据与 Desktop、CLI、TUI 共享；选择一项登录账号或修改 API Key。" align="center">
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
                    {provider.available ? provider.description : '通过 CLI / TUI 登录'}
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

          <Card title={selectedProvider.label}>
        {pendingModelReference && (
          <Row title="待应用的模型" desc={`${isCodexAuth ? '登录 ChatGPT 账号' : `保存 ${selectedProvider.label} API Key`}以使用 ${modelReference(pendingModelReference).label}。`} align="center">
            <Icon name="spark" size={15} color={t.accent} />
          </Row>
        )}
        {statusKind === 'runtime' && (
          <Row title="来源" desc="正在运行的引擎从外部运行时来源接收了该凭据；LingXi 未存储它。" align="center"><span /></Row>
        )}
        {statusKind === 'secure' && (
          <Row title="存储方式" desc="已安全保存在 macOS Data Protection Keychain 中，由签名凭据代理统一管理。" align="center"><span /></Row>
        )}
        {isCodexAuth && oauthStorageUnavailable && !selectedMetadata?.storageError && (
          <Row title="安全存储不可用" desc="ChatGPT 登录需要可用的安全凭据存储。请检查系统钥匙串或凭据代理后重试。" align="center">
            <Icon name="shieldAlert" size={15} color={t.danger} />
          </Row>
        )}
        {statusKind === 'unavailable' && (
          <Row title="安全存储不可用" desc={selectedMetadata?.storageError} align="center">
            <Icon name="shieldAlert" size={15} color={t.danger} />
          </Row>
        )}
        {!isCodexAuth && statusKind === 'fallback-configured' && (
          <Row title="存储方式" desc="当前凭据来自仅所有者可读的本地回退存储；引擎会在钥匙串可用时自动迁移。" align="center"><span /></Row>
        )}
        {!isCodexAuth && statusKind === 'fallback-unconfigured' && (
          <Row title="存储方式" desc="安全存储当前处于本地回退模式；保存时会优先尝试 macOS 登录钥匙串。" align="center"><span /></Row>
        )}
        {isCodexAuth ? (
          <Row title="ChatGPT 账号" desc={connecting
            ? '请在浏览器中完成 ChatGPT 登录。等待授权返回…'
            : '通过浏览器登录 ChatGPT 账号，授权凭据由安全存储管理。'} align="center">
            <div style={{ display: 'flex', gap: 7 }}>
              <button type="button" data-testid="codex-login" disabled={credentialWriteDisabled || oauthStorageUnavailable}
                onClick={() => void loginCodex()} style={ghostButtonStyle(t, credentialWriteDisabled || oauthStorageUnavailable)}>
                {connecting ? '等待登录…' : modelApplying ? '应用模型中…' : selectedMetadata?.configured ? '重新登录' : '登录'}
              </button>
              {connecting && !modelApplying && (
                <button type="button" data-testid="codex-login-cancel" onClick={() => void cancelCodexLogin()} style={ghostButtonStyle(t, false)}>取消登录</button>
              )}
              {selectedMetadata?.configured && !selectedMetadata.runtimeOnly && (
                <button type="button" disabled={credentialWriteDisabled} onClick={() => void clearCredential()} style={ghostButtonStyle(t, credentialWriteDisabled, true)}>退出登录</button>
              )}
            </div>
          </Row>
        ) : selectedProvider.available ? (
          <Row title={selectedProvider.keyLabel} desc={selectedMetadata?.configured ? '输入新的 API Key 并保存，即可更新现有凭据。' : undefined} align="center">
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
                获取或管理 API Key&nbsp;↗
              </a>
            </div>
          </Row>
        ) : (
          <Row title={selectedProvider.keyLabel} desc={`${selectedProvider.label} 目前只能通过 CLI/TUI 配置凭据。`} align="center"><span /></Row>
        )}
        {selectedProvider.available && !isCodexAuth && (
          <Row
            title="连接测试"
            desc={testMessage ? (
              <span role={connectionTestError ? 'alert' : 'status'} aria-live="polite" style={{ color: testMessageColor }}>
                {testMessage}
                {connectionTestResult && (
                  <span style={{ color: t.text4 }}>
                    {connectionTestResult.used_stored_credential ? ' · 已保存凭据' : ' · 当前输入'}
                  </span>
                )}
              </span>
            ) : '验证网络、API Key 与默认模型，不会发起推理请求。'}
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
              {testingConnection ? '测试中…' : '测试连接'}
            </button>
          </Row>
        )}
        {selectedProvider.available && (
          <Row
            title="对话模型列表"
            desc={providerCatalogEntry
              ? `${selectedModelIds.length} / ${providerCatalogEntry.models.length} 个模型可见`
              : '正在等待共享 Provider 模型目录。'}
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
              显示此 Provider
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
                在模型列表中显示
              </label>
            </Row>
          );
        })}
        {selectedProvider.available && visibility?.showInModelPicker !== false && providerCatalogEntry && selectedModelIds.length === 0 && (
          <Row title="提示" desc="当前没有可显示模型，这个 Provider 会从对话模型列表隐藏。" align="center">
            <Icon name="warning" size={15} color={t.warn} />
          </Row>
        )}
        {selectedProvider.available && !isCodexAuth && (
          <Row
            title="凭据操作"
            desc={selectedMetadata?.configured
              ? '保存新的 API Key，或删除已保存的凭据。'
              : '将输入的 API Key 保存到安全凭据存储。'}
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
                  删除 API Key
                </button>
              )}
            </div>
          </Row>
        )}
        {statusMessage && (
          <Row title="错误" align="center">
            <span role="alert" style={{ color: t.danger, fontSize: 12.5 }}>{statusMessage}</span>
          </Row>
        )}
        {applyError && pendingModelReference && (
          <Row title="应用模型" align="center">
            <button type="button" disabled={busy} onClick={() => void applyPendingModel()} style={ghostButtonStyle(t, busy)}>使用该模型并返回对话</button>
          </Row>
        )}
          </Card>
        </>
      )}
    </>
  );
}
