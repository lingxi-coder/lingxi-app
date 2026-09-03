import { useEffect, useMemo, useRef, useState } from 'react';
import type { AskQuestionDto, AskUserQuestionRequestDto } from '@lingxi/bridge-client';

import { useT } from '../theme/ThemeContext';
import { DesktopDialog, DesktopDialogActions, DesktopDialogButton } from './DesktopDialog';

export interface AskUserQuestionPromptProps {
  request: AskUserQuestionRequestDto | null;
  onSubmit(requestId: number, answers: Record<string, string>): void;
  onCancel(requestId: number): void;
}

interface QuestionAnswer {
  selected: Set<string>;
  other: string;
  useOther: boolean;
}

function emptyAnswers(request: AskUserQuestionRequestDto | null): QuestionAnswer[] {
  return request?.questions.map(() => ({
    selected: new Set<string>(),
    other: '',
    useOther: false,
  })) ?? [];
}

function answerText(question: AskQuestionDto, answer: QuestionAnswer): string | null {
  const other = answer.other.trim();
  if (!question.multi_select) {
    if (answer.useOther) return other || null;
    return question.options.find((option) => answer.selected.has(option.label))?.label ?? null;
  }
  const labels = question.options
    .filter((option) => answer.selected.has(option.label))
    .map((option) => option.label);
  if (answer.useOther && other) labels.push(other);
  return labels.length > 0 ? labels.join(', ') : null;
}

export function AskUserQuestionPrompt({
  request,
  onSubmit,
  onCancel,
}: AskUserQuestionPromptProps) {
  const t = useT();
  const dialogRef = useRef<HTMLDivElement>(null);
  const promptHasFocus = useRef(false);
  const [current, setCurrent] = useState(0);
  const [answers, setAnswers] = useState<QuestionAnswer[]>(() => emptyAnswers(request));
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    setCurrent(0);
    setAnswers(emptyAnswers(request));
    setError(null);
  }, [request?.request_id]);

  useEffect(() => {
    if (!request) return;
    const previous = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    dialogRef.current?.focus();
    return () => {
      if (promptHasFocus.current) previous?.focus();
    };
  }, [request]);

  const question = request?.questions[current];
  const answer = answers[current];
  const progress = useMemo(
    () => request ? `${Math.min(current + 1, request.questions.length)} / ${request.questions.length}` : '',
    [current, request],
  );

  if (!request || !question || !answer) return null;

  const updateAnswer = (next: QuestionAnswer) => {
    setError(null);
    setAnswers((previous) => previous.map((entry, index) => index === current ? next : entry));
  };
  const toggleOption = (label: string) => {
    if (!question.multi_select) {
      updateAnswer({ ...answer, selected: new Set([label]), useOther: false });
      return;
    }
    const selected = new Set(answer.selected);
    if (selected.has(label)) selected.delete(label);
    else selected.add(label);
    updateAnswer({ ...answer, selected });
  };
  const toggleOther = () => {
    updateAnswer({
      ...answer,
      selected: question.multi_select ? answer.selected : new Set(),
      useOther: !answer.useOther,
    });
  };
  const advance = () => {
    const text = answerText(question, answer);
    if (!text) {
      setError('Select an option or enter an Other answer.');
      return;
    }
    const nextAnswers = answers.map((entry, index) => index === current ? answer : entry);
    if (current + 1 < request.questions.length) {
      setAnswers(nextAnswers);
      setCurrent(current + 1);
      setError(null);
      return;
    }
    const result: Record<string, string> = {};
    for (const [index, item] of request.questions.entries()) {
      const value = answerText(item, nextAnswers[index]);
      if (!value) {
        setCurrent(index);
        setError('Answer every question before submitting.');
        return;
      }
      result[item.question] = value;
    }
    onSubmit(request.request_id, result);
  };

  return (
    <DesktopDialog
      ref={dialogRef}
      title={question.question}
      summary={request.timeout_secs !== undefined
        ? `Auto-continue in ${request.timeout_secs}s if unanswered`
        : question.multi_select ? 'Select one or more options.' : 'Select one option.'}
      eyebrow={(
        <span className="ask-dialog-eyebrow-copy">
          <span>{question.header}</span>
          <span>{progress}</span>
        </span>
      )}
      icon="info"
      size="regular"
      zIndex={70}
      titleId="lingxi-ask-title"
      summaryId="lingxi-ask-summary"
      ariaLabelledBy="lingxi-ask-title"
      ariaDescribedBy="lingxi-ask-summary"
      className="ask-user-dialog"
      tabIndex={-1}
      onFocusCapture={() => { promptHasFocus.current = true; }}
      onBlurCapture={(event) => { promptHasFocus.current = event.currentTarget.contains(event.relatedTarget as Node | null); }}
      onEscape={() => onCancel(request.request_id)}
      footer={(
        <DesktopDialogActions>
          <DesktopDialogButton variant="cancel" onClick={() => onCancel(request.request_id)}>
            Cancel
          </DesktopDialogButton>
          {current > 0 && (
            <DesktopDialogButton
              variant="secondary"
              onClick={() => { setCurrent(current - 1); setError(null); }}
            >
              Back
            </DesktopDialogButton>
          )}
          <DesktopDialogButton variant="primary" onClick={advance}>
            {current + 1 === request.questions.length ? 'Submit' : 'Next'}
          </DesktopDialogButton>
        </DesktopDialogActions>
      )}
    >
      <fieldset style={{ border: 0, margin: 0, padding: 0 }}>
          <legend style={{ position: 'absolute', width: 1, height: 1, overflow: 'hidden' }}>
            {question.multi_select ? 'Select one or more options' : 'Select one option'}
          </legend>
          {question.options.map((option) => (
            <label
              key={option.label}
              style={{
                display: 'grid', gridTemplateColumns: '20px 1fr', gap: 8,
                padding: '9px 0', color: t.text, cursor: 'pointer',
              }}
            >
              <input
                type={question.multi_select ? 'checkbox' : 'radio'}
                name={`ask-${request.request_id}-${current}`}
                checked={answer.selected.has(option.label)}
                onChange={() => toggleOption(option.label)}
              />
              <span>
                <span style={{ display: 'block', fontSize: 13, fontWeight: 600 }}>{option.label}</span>
                <span style={{ display: 'block', marginTop: 2, color: t.text2, fontSize: 12 }}>{option.description}</span>
                {option.preview && (
                  <span style={{ display: 'block', marginTop: 5, color: t.text3, fontSize: 11, whiteSpace: 'pre-wrap' }}>
                    {option.preview}
                  </span>
                )}
              </span>
            </label>
          ))}
          <label style={{ display: 'grid', gridTemplateColumns: '20px 1fr', gap: 8, padding: '9px 0', color: t.text }}>
            <input
              type={question.multi_select ? 'checkbox' : 'radio'}
              name={`ask-${request.request_id}-${current}`}
              checked={answer.useOther}
              onChange={toggleOther}
            />
            <span>
              <span style={{ display: 'block', fontSize: 13, fontWeight: 600 }}>Other</span>
              <input
                aria-label="Other answer"
                value={answer.other}
                onFocus={() => {
                  if (!answer.useOther) toggleOther();
                }}
                onChange={(event) => updateAnswer({ ...answer, other: event.target.value, useOther: true })}
                style={{
                  boxSizing: 'border-box', width: '100%', marginTop: 6, padding: '8px 10px',
                  borderRadius: 7, border: `0.5px solid ${t.border}`,
                  background: t.surface, color: t.text, fontFamily: 'inherit',
                }}
              />
            </span>
          </label>
          {error && <div role="alert" style={{ color: t.danger, fontSize: 12, marginTop: 6 }}>{error}</div>}
      </fieldset>
    </DesktopDialog>
  );
}
