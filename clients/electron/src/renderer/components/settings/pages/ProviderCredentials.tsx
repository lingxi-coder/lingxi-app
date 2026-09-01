import { useEffect, useRef, useState } from 'react';
import { Card, Row } from '../rows';
import { useT } from '../../../theme/ThemeContext';
import { Icon } from '../../Icon';
import type { PageContentProps } from '../SettingsScreen';
import { PROVIDERS, providerById } from '../../../../shared/providers';
import type { ProviderCredentialMetadata } from '../../../bridge/lingxi';
import { isCurrentCredentialTransaction, persistProviderCredentialAndApplyModel } from '../../../bridge/providerCredentials';
import { modelReference, waitForModelSelection } from '../../../bridge/modelCatalog';
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

export type CredentialStatusKind = 'runtime' | 'secure' | 'fallback-configured' | 'fallback-unconfigured' | 'none';

/**
 * Which of the four Keychain-availability/runtime-source status paragraphs
 * the old settings modal showed for the selected provider — pure so each of the four
 * states (plus "none of the above") can be asserted by name without
 * mounting anything. The four conditions and their order are copied
 * verbatim from `BetaDesktop.tsx`'s `beta-provider-form` block: `runtimeOnly`
 * wins over everything else (an external runtime source pre-configured this
 * provider — LingXi never persisted it), then configured+encrypted, then
 * configured+fallback, then not-configured+fallback-would-be-used.
 */
export function credentialStatusKind(metadata?: Pick<ProviderCredentialMetadata, 'configured' | 'encryptionAvailable' | 'runtimeOnly'>): CredentialStatusKind {
  if (!metadata) return 'none';
  if (metadata.runtimeOnly) return 'runtime';
  if (metadata.configured && metadata.encryptionAvailable) return 'secure';
  if (metadata.configured && !metadata.encryptionAvailable) return 'fallback-configured';
  if (!metadata.configured && metadata.encryptionAvailable === false) return 'fallback-unconfigured';
  return 'none';
}

/**
 * The connect/replace/use-entered-key button's label, in the same priority
 * order the old settings modal used: an in-flight connect or model-apply wins over
 * anything else so the button never invites a second click mid-transaction;
 * otherwise `runtimeOnly` offers "use entered key" (there's a live value to
 * override), and plain `configured` offers "replace" instead of "connect".
 */
export function connectButtonLabel(state: { connecting: boolean; modelApplying: boolean; runtimeOnly?: boolean; configured?: boolean }): string {
  if (state.connecting) return '连接中…';
  if (state.modelApplying) return '应用模型中…';
  if (state.runtimeOnly) return '使用输入的密钥';
  if (state.configured) return '替换';
  return '连接';
}

/** An empty/whitespace-only base URL means "clear the override", not "set it to empty string" — `main/settings.ts` treats `''`/`null` identically, this just picks one before it reaches the wire. */
export function apiBaseUrlPatch(value: string): string | null {
  const trimmed = value.trim();
  return trimmed.length > 0 ? trimmed : null;
}

function invoke(action: () => Promise<unknown>): void {
  void action().catch(() => undefined);
}

/**
 * Lifted from the old settings modal's "Providers" section in
 * `BetaDesktop.tsx` (retired in Task 20; it lived at ~lines 1852-2119):
 * connecting/replacing/disconnecting a credential, the
 * four Keychain-availability/runtime-source status states
 * (`credentialStatusKind` above), the pending-model banner and its "use
 * model and return to chat" recovery, the post-persist recovery UI for when
 * a credential saves but the engine restart doesn't finish, and — added in
 * fix round 1, dropped in the first pass — the deep-link autofocus: moving
 * focus into the credential input via `requestAnimationFrame` when both
 * `initialProviderId` and the requested model are set
 * (that modal's own lines 1923-1926). None of that logic was rewritten —
 * `isCurrentCredentialTransaction` and `persistProviderCredentialAndApplyModel`
 * (`bridge/providerCredentials.ts`) are the same already-tested functions
 * that modal called.
 *
 * One thing this page adds that the old modal's Providers section did NOT
 * have: an editor for `apiBaseUrl`. The brief for this task listed
 * `apiBaseUrl` among the state lifted from that modal, but grepping
 * `BetaDesktop.tsx` turned up no such UI — `apiBaseUrl` exists only as a
 * `PublicSettings` field and a `host.updateSettings` patch key
 * (`lingxi.d.ts`, `main/settings.ts`, `main/host.ts`), wired end-to-end on
 * the main-process side but never exposed to a person. `nav.ts` lists
 * `apiBaseUrl` as this page's own search keyword, so rather than silently
 * dropping it (the exact failure mode this task's brief warns about), this
 * builds the minimal new editor on top of that already-working, already-
 * tested plumbing — see this task's report for the full correction.
 */
export function ProviderCredentials({ bridge, initialProviderId, pendingModelReference: requestedModelReference, onClose }: PageContentProps) {
  const t = useT();
  const snapshot = bridge.bootstrap;
  const [key, setKey] = useState('');
  const [selectedProviderId, setSelectedProviderId] = useState(() => initialProviderSelection(initialProviderId));
  const [pendingModelReference, setPendingModelReference] = useState<string | null>(requestedModelReference ?? null);
  const [connecting, setConnecting] = useState(false);
  const [saveError, setSaveError] = useState<string | null>(null);
  const [applyError, setApplyError] = useState<string | null>(null);
  const [modelApplying, setModelApplying] = useState(false);
  const [postPersistRecovery, setPostPersistRecovery] = useState(false);
  const [apiBaseUrlInput, setApiBaseUrlInput] = useState(bridge.bootstrap?.settings.apiBaseUrl ?? '');
  const [apiBaseUrlSaving, setApiBaseUrlSaving] = useState(false);
  const [apiBaseUrlError, setApiBaseUrlError] = useState<string | null>(null);

  const selectedProvider = providerById(selectedProviderId) ?? PROVIDERS[0];
  const selectedMetadata = snapshot?.providerCredentials?.find((entry) => entry.providerId === selectedProvider.id);
  const statusKind = credentialStatusKind(selectedMetadata);

  const mountedRef = useRef(true);
  const transactionGenerationRef = useRef(0);
  const applyGenerationRef = useRef(0);
  const applyAbortRef = useRef<AbortController | null>(null);
  const recoverySessionIdRef = useRef<string | null>(null);
  const currentModelRef = useRef<string | null>(bridge.desktop.currentModel);
  const credentialRef = useRef<HTMLInputElement>(null);
  currentModelRef.current = bridge.desktop.currentModel;

  useEffect(() => {
    mountedRef.current = true;
    return () => {
      mountedRef.current = false;
      ++transactionGenerationRef.current;
      ++applyGenerationRef.current;
      applyAbortRef.current?.abort();
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

  // A person may navigate away from this page and back without a fresh
  // deep link; only sync the local base-URL draft from bootstrap when it
  // has not been hand-edited into a different pending value already.
  useEffect(() => {
    setApiBaseUrlInput((current) => (
      apiBaseUrlSaving ? current : (bridge.bootstrap?.settings.apiBaseUrl ?? '')
    ));
  }, [apiBaseUrlSaving, bridge.bootstrap?.settings.apiBaseUrl]);

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
    // Capture the session before persistence yields — it can change while
    // the secure-store write is pending, and a late completion must never
    // restart whichever session happens to be active then.
    const restartSessionId = bridge.activeSession?.sessionId ?? bridge.bootstrap?.activeSession?.sessionId;
    if (!restartSessionId) {
      setSaveError('打开一个会话后才能连接 Provider。');
      return;
    }
    const transactionGeneration = ++transactionGenerationRef.current;
    let restartCompleted = false;
    let persisted = false;
    setConnecting(true);
    setSaveError(null);
    setApplyError(null);
    setPostPersistRecovery(false);
    try {
      recoverySessionIdRef.current = restartSessionId;
      await persistProviderCredentialAndApplyModel(
        selectedProvider.id,
        submitted,
        bridge.setProviderCredential,
        () => {
          persisted = true;
          if (isCurrentCredentialTransaction(mountedRef.current, transactionGeneration, transactionGenerationRef.current)) {
            setKey((current) => current === submitted ? '' : current);
          }
        },
        async (sessionId) => {
          await bridge.restartBridge(sessionId);
          restartCompleted = true;
        },
        restartSessionId,
        pendingModelReference,
        () => applyPendingModel(true, transactionGeneration),
      );
      recoverySessionIdRef.current = null;
    } catch (cause) {
      if (isCurrentCredentialTransaction(mountedRef.current, transactionGeneration, transactionGenerationRef.current)) {
        setSaveError(cause instanceof Error ? cause.message : '无法连接该 Provider。');
        setPostPersistRecovery(persisted && !restartCompleted);
        if (!persisted || restartCompleted) recoverySessionIdRef.current = null;
      }
    } finally {
      if (isCurrentCredentialTransaction(mountedRef.current, transactionGeneration, transactionGenerationRef.current)) {
        setConnecting(false);
      }
    }
  };

  const selectProvider = (providerId: string) => {
    if (connecting || modelApplying || postPersistRecovery) return;
    setSelectedProviderId(providerId);
    setKey('');
    // A manual provider change cancels the model intent from the picker.
    setPendingModelReference(null);
    setSaveError(null);
    setApplyError(null);
    ++applyGenerationRef.current;
    applyAbortRef.current?.abort();
  };

  const retryPostPersistRecovery = async () => {
    if (!postPersistRecovery || connecting || modelApplying) return;
    const transactionGeneration = transactionGenerationRef.current;
    const recoverySessionId = recoverySessionIdRef.current;
    if (!recoverySessionId) {
      setSaveError('原会话已不可用，无法重启。');
      setPostPersistRecovery(false);
      return;
    }
    setConnecting(true);
    setSaveError(null);
    try {
      await bridge.restartBridge(recoverySessionId);
      if (!isCurrentCredentialTransaction(mountedRef.current, transactionGeneration, transactionGenerationRef.current)) return;
      recoverySessionIdRef.current = null;
      setPostPersistRecovery(false);
      if (pendingModelReference) await applyPendingModel(true, transactionGeneration);
    } catch (cause) {
      if (isCurrentCredentialTransaction(mountedRef.current, transactionGeneration, transactionGenerationRef.current)) {
        setSaveError(cause instanceof Error ? cause.message : '引擎无法重启。');
      }
    } finally {
      if (isCurrentCredentialTransaction(mountedRef.current, transactionGeneration, transactionGenerationRef.current)) {
        setConnecting(false);
      }
    }
  };

  const saveApiBaseUrl = () => {
    setApiBaseUrlSaving(true);
    setApiBaseUrlError(null);
    void bridge.setApiBaseUrl(apiBaseUrlPatch(apiBaseUrlInput))
      .catch((cause) => setApiBaseUrlError(cause instanceof Error ? cause.message : '无法保存自定义 API 地址。'))
      .finally(() => setApiBaseUrlSaving(false));
  };

  const statusMessage = saveError ?? applyError;
  const busy = connecting || modelApplying;
  const transactionLocked = busy || postPersistRecovery;

  return (
    <>
      <Card title="Provider">
        <Row title="选择 Provider" desc="与 Desktop、CLI、TUI 共享；密钥优先使用 macOS 登录钥匙串。" align="center">
          <span />
        </Row>
        {PROVIDERS.map((provider) => {
          const metadata = snapshot?.providerCredentials?.find((entry) => entry.providerId === provider.id);
          const selected = provider.id === selectedProvider.id;
          return (
            <Row
              key={provider.id}
              align="center"
              title={
                <span style={{ display: 'flex', alignItems: 'center', gap: 8 }}>
                  {metadata?.configured ? <Icon name="check" size={13} color={t.ok} stroke={2.5} /> : <span style={{ width: 13, display: 'inline-block' }} />}
                  {provider.label}
                  {selected && (
                    <span style={{ fontSize: 10.5, padding: '2px 7px', borderRadius: 5, background: t.surfaceHover, color: t.text3, fontWeight: 600 }}>
                      正在编辑
                    </span>
                  )}
                </span>
              }
              desc={provider.available ? provider.description : 'CLI/TUI 登录'}
            >
              <button
                type="button"
                disabled={transactionLocked}
                onClick={() => selectProvider(provider.id)}
                style={ghostButtonStyle(t, transactionLocked || selected)}
              >
                {selected ? '当前' : '选择'}
              </button>
            </Row>
          );
        })}
      </Card>

      <Card title={selectedProvider.label}>
        {pendingModelReference && (
          <Row title="待应用的模型" desc={`连接 ${selectedProvider.label} 以使用 ${modelReference(pendingModelReference).label}。`} align="center">
            <Icon name="spark" size={15} color={t.accent} />
          </Row>
        )}
        {statusKind === 'runtime' && (
          <Row title="来源" desc="正在运行的引擎从外部运行时来源接收了该凭据；LingXi 未存储它。" align="center"><span /></Row>
        )}
        {statusKind === 'secure' && (
          <Row title="存储方式" desc="已安全保存在本机；出于设计，通用 API 密钥不会出现在“密码”App 中。" align="center"><span /></Row>
        )}
        {statusKind === 'fallback-configured' && (
          <Row title="存储方式" desc="macOS 钥匙串不可用；当前使用共享的仅所有者本地回退存储。" align="center"><span /></Row>
        )}
        {statusKind === 'fallback-unconfigured' && (
          <Row title="存储方式" desc="macOS 钥匙串不可用；连接后将使用共享的仅所有者本地回退存储。" align="center"><span /></Row>
        )}
        {selectedProvider.available ? (
          <Row title={selectedProvider.keyLabel} desc={selectedMetadata?.configured ? '输入新密钥以替换现有凭据。' : undefined} align="center">
            <div style={{ display: 'flex', gap: 7 }}>
              <input
                ref={credentialRef}
                type="password"
                autoComplete="off"
                disabled={transactionLocked || bridge.running}
                value={key}
                onChange={(event) => setKey(event.target.value)}
                onKeyDown={(event) => { if (event.key === 'Enter') { event.preventDefault(); void save(); } }}
                placeholder={selectedMetadata?.configured ? '输入新密钥' : selectedProvider.keyPlaceholder}
                aria-label={selectedProvider.keyLabel}
                style={{ padding: '6px 10px', borderRadius: 7, border: `0.5px solid ${t.border}`, background: t.surface, color: t.text, fontSize: 12.5, minWidth: 220 }}
              />
              <button type="button" disabled={!key.trim() || transactionLocked || bridge.running} onClick={() => void save()} style={ghostButtonStyle(t, !key.trim() || transactionLocked || bridge.running)}>
                {connectButtonLabel({ connecting, modelApplying, runtimeOnly: selectedMetadata?.runtimeOnly, configured: selectedMetadata?.configured })}
              </button>
              {selectedMetadata?.configured && !selectedMetadata.runtimeOnly && (
                <button type="button" disabled={transactionLocked || bridge.running} onClick={() => invoke(() => bridge.clearProviderCredential(selectedProvider.id))} style={ghostButtonStyle(t, transactionLocked || bridge.running, true)}>
                  断开连接
                </button>
              )}
            </div>
          </Row>
        ) : (
          <Row title={selectedProvider.keyLabel} desc={`${selectedProvider.label} 目前只能通过 CLI/TUI 的登录流程连接。`} align="center"><span /></Row>
        )}
        {statusMessage && (
          <Row title="错误" align="center">
            <span role="alert" style={{ color: t.danger, fontSize: 12.5 }}>{statusMessage}</span>
          </Row>
        )}
        {postPersistRecovery && (
          <Row title="凭据已保存" desc="但引擎重启未完成。重试连接，或保留已保存的凭据并离开此页。" align="center">
            <div style={{ display: 'flex', gap: 7 }}>
              <button type="button" disabled={busy} onClick={() => void retryPostPersistRecovery()} style={ghostButtonStyle(t, busy)}>重试引擎连接</button>
              <button type="button" disabled={busy} onClick={onClose} style={ghostButtonStyle(t, busy)}>保留凭据并关闭</button>
            </div>
          </Row>
        )}
        {applyError && pendingModelReference && (
          <Row title="应用模型" align="center">
            <button type="button" disabled={busy} onClick={() => void applyPendingModel()} style={ghostButtonStyle(t, busy)}>使用该模型并返回对话</button>
          </Row>
        )}
      </Card>

      <Card title="高级">
        <Row title="自定义 API 地址" desc="覆盖引擎连接的默认 API 端点；留空以恢复默认值。修改后引擎会自动重启。" align="center">
          <div style={{ display: 'flex', gap: 7 }}>
            <input
              value={apiBaseUrlInput}
              onChange={(event) => setApiBaseUrlInput(event.target.value)}
              placeholder="https://…"
              aria-label="自定义 API 地址"
              style={{ padding: '6px 10px', borderRadius: 7, border: `0.5px solid ${t.border}`, background: t.surface, color: t.text, fontSize: 12.5, minWidth: 220 }}
            />
            <button type="button" disabled={apiBaseUrlSaving || bridge.running} onClick={saveApiBaseUrl} style={ghostButtonStyle(t, apiBaseUrlSaving || bridge.running)}>
              {apiBaseUrlSaving ? '保存中…' : '保存'}
            </button>
          </div>
        </Row>
        {apiBaseUrlError && (
          <Row title="错误" align="center">
            <span role="alert" style={{ color: t.danger, fontSize: 12.5 }}>{apiBaseUrlError}</span>
          </Row>
        )}
      </Card>
    </>
  );
}
