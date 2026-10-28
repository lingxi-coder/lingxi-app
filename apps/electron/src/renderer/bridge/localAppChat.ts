import type { DesktopAuthoringRef, LocalAppEditRequest } from '../../shared/localApps';
import { isDesktopAuthoringRef } from '../../shared/localApps';
import type { SessionRef } from '../../shared/settings';
import type { ProjectCreationPurpose } from '../../shared/projectCreation';

export interface LocalAppDraft {
  id: string;
  name: string;
  nonce: number;
  target: SessionRef;
}
export interface LocalAppChatNavigation {
  hasProject(path: string): boolean;
  openSession(projectPath: string, sessionId: string): Promise<SessionRef | false | undefined>;
  addProject(purpose?: ProjectCreationPurpose): Promise<{ path?: string } | null | undefined | void>;
  newSession(projectPath: string): Promise<SessionRef | undefined | void>;
  bindAuthoring(appId: string, session: SessionRef): Promise<DesktopAuthoringRef>;
  prepareAuthoring(session: SessionRef): Promise<DesktopAuthoringRef>;
}

const agentRecoveryGuidance = '如果应用使用 Agent，请在提交前持久保存可恢复的应用状态及待处理的 runId、threadId；重新打开时读取宿主任务状态和事件或聊天历史，以只读方式重连同一 run 和 thread。请求结果不确定或任务中断时，不要自动提交、重发或重放工具；仅在宿主确认时显示正在运行；明确显示中断或未知状态和用户需要采取的操作。用户检查可能已发生的副作用后，新的尝试必须由用户明确发起并使用新的 runId；只有宿主终止结果支持时才显示完成。iOS 可能暂停或终止后台工作，不要承诺无限期运行。';

/** Draft for either authoring entry; it is presented for review and never sent automatically. */
export function localAppAuthoringPrompt(draft: Pick<LocalAppDraft, 'id' | 'name'>): string {
  const introduction = draft.id
    ? `修改本地应用「${draft.name}」（LocalApp id: ${draft.id}）。使用 local-app skill，先用 LocalApp read 查看应用及来源，在已绑定的源项目中修改后发布到同一个 id。`
    : '创建一个本地应用，使用 local-app skill 并发布到应用库。';
  return `${introduction}\n${agentRecoveryGuidance}\n需求：\n`;
}

function assertAuthoringSession(binding: DesktopAuthoringRef, session: SessionRef): void {
  if (binding.sourceDirectory !== session.projectPath || binding.projectRef.kind !== 'desktopPath'
    || binding.projectRef.path !== session.projectPath || binding.authoringSessionId !== session.sessionId) {
    throw new Error('Desktop did not verify the requested Local App source Code session');
  }
}

/** Navigate to the host-verified Code workspace before creating the draft. */
export async function localAppChatTarget(
  app: LocalAppEditRequest | undefined,
  navigation: LocalAppChatNavigation,
  isCurrent: () => boolean,
): Promise<SessionRef | undefined> {
  if (!isCurrent()) return;
  const sourceDirectory = app?.authoring?.sourceDirectory;
  const authoring = isDesktopAuthoringRef(app?.authoring) ? app.authoring : undefined;
  let project = authoring?.projectRef.path ?? sourceDirectory;
  if (app?.authoringSourceMissing) {
    // The old source is gone. Let the user locate a replacement or explicitly
    // restore it, then bind the new Code session before creating a draft.
    const restored = await navigation.addProject('local-app');
    if (!restored?.path || !isCurrent()) return;
    project = restored.path;
  } else if (project && !navigation.hasProject(project)) {
    const restored = await navigation.addProject('local-app');
    if (!restored?.path || !isCurrent()) return;
    if (restored.path !== project) throw new Error(`Add the saved app workspace at ${project} before editing this application.`);
    project = restored.path;
  } else if (!project) {
    const selected = await navigation.addProject('local-app');
    if (!selected?.path || !isCurrent()) return;
    project = selected.path;
  }

  if (authoring && !app?.authoringSourceMissing && app?.authoringAction === 'resumeSession' && navigation.hasProject(project)) {
    const opened = await navigation.openSession(project, authoring.authoringSessionId);
    if (!isCurrent()) return;
    if (opened && opened.projectPath === project) return opened;
    throw new Error(`Could not resume the saved app authoring session ${authoring.authoringSessionId} in ${project}. Refresh the Apps library and try again.`);
  }

  // An unavailable legacy/recovery session is replaced inside its saved source
  // project. A chat session or an unrelated active repository is never used.
  const failure = `Could not open the app authoring chat. The project is saved at ${project}. Choose that source folder to retry.`;
  let target: SessionRef | undefined | void;
  try { target = await navigation.newSession(project); }
  catch (cause) {
    if (!isCurrent()) return;
    throw Object.assign(new Error(`${failure}\n${cause instanceof Error ? cause.message : String(cause)}`), { cause });
  }
  if (!isCurrent()) return;
  if (!target) throw new Error(failure);
  if (app?.id) {
    assertAuthoringSession(await navigation.bindAuthoring(app.id, target), target);
    if (!isCurrent()) return;
  } else {
    // The native Create flow must durably register this exact Code session
    // before the composer exposes its first Local App draft.
    assertAuthoringSession(await navigation.prepareAuthoring(target), target);
    if (!isCurrent()) return;
  }
  return target;
}

/** Discard a request once another settled chat wins, rather than reviving it later. */
export function localAppDraftDisposition(
  draft: LocalAppDraft,
  activeSession: SessionRef | null | undefined,
  loading: boolean,
): 'wait' | 'apply' | 'discard' {
  if (loading) return 'wait';
  return activeSession?.sessionId === draft.target.sessionId
    && activeSession.projectPath === draft.target.projectPath ? 'apply' : 'discard';
}
