import { useState } from 'react';
import type { AskUserQuestionRequestDto, PermissionRequest } from '@lingxi/bridge-client';
import type { RemotePending } from '@lingxi/bridge-client/remote';
import type { Locale } from '../utils/locale';
import { Markdown } from './markdown';
import { permissionFields } from './permissionDetail';
import { questionAnswers, type SessionController } from './remoteClient';
import { t } from './strings';
import { displayText } from './text';

interface PromptsProps {
  locale: Locale;
  controller: SessionController;
  pending: RemotePending;
  answeredElsewhere: boolean;
}

/** What the session is waiting on: permission requests, questions, and (display only) computer access. */
export function PendingPrompts({ locale, controller, pending, answeredElsewhere }: PromptsProps) {
  const any = pending.permissions.length + pending.questions.length + pending.computer_access.length > 0;
  if (!any && !answeredElsewhere) return null;
  return (
    <section className="rc-pending" aria-live="polite">
      {answeredElsewhere ? <p className="rc-note">{t(locale, 'answeredElsewhere')}</p> : null}
      {pending.permissions.map((request) => (
        <PermissionCard key={request.request_id} locale={locale} controller={controller} request={request} />
      ))}
      {pending.questions.map((request) => (
        <QuestionCard key={request.request_id} locale={locale} controller={controller} request={request} />
      ))}
      {pending.computer_access.length > 0 ? <p className="rc-note">{t(locale, 'computerAccessPending')}</p> : null}
    </section>
  );
}

/** Run an answer, keeping the buttons off while it is on its way and quietly noting a failure. */
function useAnswer(locale: Locale): { busy: boolean; error: string | null; run(action: () => Promise<void>): void } {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  return {
    busy,
    error,
    run(action) {
      setBusy(true);
      setError(null);
      action().then(
        () => setBusy(false),
        () => {
          setBusy(false);
          setError(t(locale, 'errGeneric'));
        },
      );
    },
  };
}

function PermissionCard({ locale, controller, request }: { locale: Locale; controller: SessionController; request: PermissionRequest }) {
  const answer = useAnswer(locale);
  const { kind } = request;
  // Only "allow once" and "deny" are offered here; an always-allow rule and the bypass mode stay on the computer.
  const respond = (type: 'allow_once' | 'deny') => answer.run(() => controller.respondPermission(request.request_id, { type }));
  return (
    <div className="rc-card rc-permission" role="group" aria-label={t(locale, 'permissionTitle')}>
      <h3>{t(locale, 'permissionTitle')}</h3>
      {kind.type === 'tool_use_confirm' ? (
        <>
          <p>{t(locale, 'permissionTool', { tool: displayText(kind.tool_name, 80) })}</p>
          <dl className="rc-fields">
            {permissionFields(kind.tool_input_json).map((field, index) => (
              <div key={index}>
                {field.key ? <dt>{displayText(field.key, 60)}</dt> : null}
                <dd><pre className="rc-code">{field.value}</pre></dd>
              </div>
            ))}
          </dl>
        </>
      ) : kind.type === 'exit_plan_mode' ? (
        <>
          <p>{t(locale, 'permissionPlan')}</p>
          <Markdown text={kind.plan} />
        </>
      ) : (
        <p>{t(locale, 'permissionBypass')}</p>
      )}
      <div className="rc-actions">
        {kind.type === 'bypass_permissions_mode' ? null : (
          <button type="button" className="button primary" disabled={answer.busy} onClick={() => respond('allow_once')}>
            {t(locale, 'allowOnce')}
          </button>
        )}
        <button type="button" className="button secondary" disabled={answer.busy} onClick={() => respond('deny')}>
          {t(locale, 'deny')}
        </button>
      </div>
      {answer.error ? <p className="rc-note rc-danger">{answer.error}</p> : null}
    </div>
  );
}

function QuestionCard({ locale, controller, request }: { locale: Locale; controller: SessionController; request: AskUserQuestionRequestDto }) {
  const answer = useAnswer(locale);
  const [chosen, setChosen] = useState<ReadonlyMap<string, readonly string[]>>(new Map());
  const [other, setOther] = useState<ReadonlyMap<string, string>>(new Map());
  const toggle = (question: string, label: string, multi: boolean): void => {
    const current = chosen.get(question) ?? [];
    const next = multi ? (current.includes(label) ? current.filter((entry) => entry !== label) : [...current, label]) : [label];
    setChosen(new Map(chosen).set(question, next));
    if (!multi) setOther(new Map(other).set(question, ''));
  };
  const answers = questionAnswers(request, chosen, other);
  return (
    <div className="rc-card rc-question" role="group" aria-label={t(locale, 'questionTitle')}>
      <h3>{t(locale, 'questionTitle')}</h3>
      {request.questions.map((question) => (
        <fieldset key={question.question}>
          <legend>{displayText(question.header, 40) ? <strong>{displayText(question.header, 40)} </strong> : null}{question.question}</legend>
          {question.options.map((option) => {
            const selected = chosen.get(question.question)?.includes(option.label) ?? false;
            return (
              <label key={option.label} className="rc-choice">
                <input
                  type={question.multi_select ? 'checkbox' : 'radio'}
                  name={question.question}
                  checked={selected}
                  onChange={() => toggle(question.question, option.label, question.multi_select)}
                />
                <span>
                  {option.label}
                  {option.description ? <small>{option.description}</small> : null}
                </span>
              </label>
            );
          })}
          <input
            className="rc-other"
            type="text"
            placeholder={t(locale, 'questionOther')}
            value={other.get(question.question) ?? ''}
            onChange={(event) => {
              setOther(new Map(other).set(question.question, event.target.value));
              if (!question.multi_select) setChosen(new Map(chosen).set(question.question, []));
            }}
          />
        </fieldset>
      ))}
      <div className="rc-actions">
        <button type="button" className="button primary" disabled={answer.busy || Object.keys(answers).length === 0} onClick={() => answer.run(() => controller.answerQuestion(request.request_id, answers))}>
          {t(locale, 'questionSubmit')}
        </button>
        <button type="button" className="button secondary" disabled={answer.busy} onClick={() => answer.run(() => controller.cancelQuestion(request.request_id))}>
          {t(locale, 'questionSkip')}
        </button>
      </div>
      {answer.error ? <p className="rc-note rc-danger">{answer.error}</p> : null}
    </div>
  );
}
