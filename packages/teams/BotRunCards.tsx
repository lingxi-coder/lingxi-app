import { useState, type ReactElement, type ReactNode } from 'react';
import { canRecoverBot, isQuestionRecipient, type TeamBotState } from './bots.js';
import { recoveryKey, type TeamMessage, type TeamMessaging } from './messages.js';

// The two cards a Bot's message carries in the conversation: the question it asked and the run it started.
// Both render inside a `.tw-embed` wrapper (workspace.css), which styles the `team-*` classes used here.

export function BotQuestion({ message, uid, generation, state, model, busy, zh, attempt }: {
  message: TeamMessage; uid: string; generation: number; state?: TeamBotState; model: TeamMessaging; busy: boolean; zh: boolean;
  attempt(action: () => Promise<unknown>): Promise<void>;
}): ReactElement {
  const t = (en: string, cn: string) => zh ? cn : en;
  const [answers, setAnswers] = useState<Record<string, string>>({});
  const pending = model.state.answerOutbox?.find(value => value.conversationId === model.selected && value.questionId === message.question!.id);
  const answered = state?.questionId === message.question!.id && state.answered || model.state.answerReceipts?.includes(JSON.stringify([model.selected, message.question!.id]));
  const recipient = isQuestionRecipient(message, uid, generation);
  const active = state?.state === 'running' && state.questionId === message.question!.id && state.origin.uid === uid && state.origin.generation === generation && (state.leaseUntil ?? 0) > Date.now();
  let content: ReactNode;
  if (answered) {
    content = <p role="status">{t('Answer received.', '答案已接收。')}</p>;
  } else if (pending) {
    content = <>
      <p role="status">{pending.error ? t('Answer was rejected. It was not changed or resent with a new identity.', '答案被拒绝，未更改内容或使用新身份重发。') : t('Answer saved. Waiting for confirmation; reconnect retries this exact answer.', '答案已保存，等待确认；重新连接后将重试相同答案。')}</p>
      {message.question!.questions.map(question => <p key={question}><b>{question}</b><br />{pending.answers[question]}</p>)}
      {pending.error ? <button disabled={busy} onClick={() => void attempt(() => model.discardAnswer(pending.answerId))}>{t('Discard rejected answer', '丢弃被拒绝的答案')}</button> : null}
    </>;
  } else if (recipient) {
    content = <form onSubmit={event => { event.preventDefault(); void attempt(() => model.answer(message, answers)); }}>
      {message.question!.questions.map(question => <label key={question}>{question}<textarea maxLength={2000} required disabled={busy || !active} value={answers[question] ?? ''} onChange={event => setAnswers(old => ({ ...old, [question]: event.target.value }))} /></label>)}
      <button disabled={busy || !active || message.question!.questions.some(question => !answers[question]?.trim())}>{t('Send answer', '发送答案')}</button>
      {!active ? <p className="teams-hint">{state ? t('This question is no longer active.', '此问题已无法回答。') : t('Waiting to verify this question is still active.', '正在确认此问题是否仍可回答。')}</p> : null}
    </form>;
  } else {
    content = <>
      <ul>{message.question!.questions.map(question => <li key={question}>{question}</li>)}</ul>
      <p className="teams-hint">{t('Only the original requester in the same membership can answer.', '仅同一次成员身份的原始请求人可回答。')}</p>
    </>;
  }
  return <section className="team-bot-question"><h4>{t('Bot question', 'Bot 提问')}</h4>{content}</section>;
}

export function BotRun({ state, triggerId, conversationId, model, uid, zh, busy, attempt }: {
  state: TeamBotState; triggerId: string; conversationId: string; model: TeamMessaging; uid: string; zh: boolean; busy: boolean;
  attempt(action: () => Promise<unknown>): Promise<void>;
}): ReactElement {
  const t = (en: string, cn: string) => zh ? cn : en;
  const [text, setText] = useState(''), [confirmed, setConfirmed] = useState(false);
  const key = recoveryKey(conversationId, triggerId);
  const pending = model.state.recoveryOutbox?.find(item => recoveryKey(item.conversationId, item.triggerId) === key);
  const done = model.state.recoveryReceipts?.includes(key);
  const labels: Record<string, [string, string]> = { queued: ['Queued', '已排队'], pending: ['Queued', '已排队'], claimed: ['Preparing', '准备中'], running: ['Processing', '处理中'], recovery: ['Owner recovery required', '需要所有者恢复'], recovered: ['Recovered by owner', '所有者已恢复'], completed: ['Completed', '已完成'], failed: ['Failed', '失败'], missed: ['Expired while offline', '离线期间已过期'], cancelled: ['Cancelled', '已取消'], revoked: ['Access revoked', '授权已撤销'] };
  const hostLabels: Record<NonNullable<TeamBotState['hostStatus']>, [string, string]> = {
    retrying: ['Desktop is reconnecting. The saved request will be retried.', 'Desktop 正在重新连接，将重试已保存的请求。'],
    result_pending: ['The exact result is saved on Desktop. Reconnecting retries delivery without running tools again.', '原始结果已保存在 Desktop，重新连接后将重试交付，不会再次运行工具。'],
    recovery_required: ['Desktop stopped this interrupted run. Review it before settling the request.', 'Desktop 已停止此中断运行，请检查后结束请求。'],
    model_unavailable: ['The configured model or credential is unavailable on Desktop. Check provider settings.', 'Desktop 上配置的模型或凭证不可用，请检查供应商设置。'],
    runtime_unavailable: ['The isolated Teams runtime is unavailable on Desktop. Check the installed Desktop build.', 'Desktop 的团队隔离运行时不可用，请检查已安装版本。'],
    storage_error: ['Desktop could not save the request state. Check local storage before retrying.', 'Desktop 无法保存请求状态，请检查本地存储后重试。'],
  };
  let recovery: ReactNode = null;
  if (pending) {
    recovery = <div className="team-pending">
      <p role="status">{pending.error ? t('Settlement was rejected. Review the run before creating a new settlement.', '处理结果被拒绝，请检查运行状态后再创建新的处理结果。') : t('Settlement saved. Reconnecting retries this exact result without running tools.', '处理结果已保存，重新连接后将重试交付原始结果，不会运行工具。')}</p>
      <p>{pending.text}</p>
      {pending.error ? <button disabled={busy} onClick={() => void attempt(() => model.discardRejectedRecovery(conversationId, triggerId))}>{t('Discard rejected settlement', '丢弃被拒绝的处理结果')}</button> : null}
    </div>;
  } else if (!done && canRecoverBot(state, uid)) {
    recovery = <details>
      <summary>{t('Resolve interrupted request', '处理已中断请求')}</summary>
      <p>{t('This settles the request without running its tools again. Check any external effects before writing the result.', '此操作会结束该请求，不会重新执行工具。请先检查外部操作结果。')}</p>
      <form onSubmit={event => { event.preventDefault(); void attempt(() => model.recover(conversationId, triggerId, text)); }}>
        <label>{t('Result for the conversation', '发送到会话的处理结果')}<textarea required value={text} maxLength={8192} onChange={event => setText(event.target.value)} /></label>
        <label className="team-party"><input type="checkbox" checked={confirmed} onChange={event => setConfirmed(event.target.checked)} />{t('I checked the interrupted request and want to settle it.', '我已检查中断请求并确认结束。')}</label>
        <button disabled={busy || !confirmed || !text.trim() || new TextEncoder().encode(text).length > 8192}>{t('Settle request', '结束请求')}</button>
      </form>
    </details>;
  }
  return <div className="team-bot-run"><p role="status">Bot · {done ? t('Recovered by owner', '所有者已恢复') : labels[state.state]?.[zh ? 1 : 0] ?? state.state}</p>{uid === state.ownerUid && state.hostStatus ? <p role="status" className="teams-hint">{hostLabels[state.hostStatus][zh ? 1 : 0]}</p> : null}{recovery}</div>;
}
