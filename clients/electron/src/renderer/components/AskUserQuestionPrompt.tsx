import { useEffect, useMemo, useRef, useState } from 'react';
import type { AskQuestionDto, AskUserQuestionRequestDto } from '@lingxi/bridge-client';

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
          <span className="ask-dialog-progress">{progress}</span>
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
      <fieldset className="ask-dialog-options">
          <legend className="ask-dialog-legend">
            {question.multi_select ? 'Select one or more options' : 'Select one option'}
          </legend>
          {question.options.map((option) => (
            <label
              key={option.label}
              className="ask-dialog-option"
            >
              <input
                type={question.multi_select ? 'checkbox' : 'radio'}
                name={`ask-${request.request_id}-${current}`}
                checked={answer.selected.has(option.label)}
                onChange={() => toggleOption(option.label)}
              />
              <span>
                <span className="ask-dialog-option-title">{option.label}</span>
                <span className="ask-dialog-option-description">{option.description}</span>
                {option.preview && (
                  <span className="ask-dialog-option-preview">
                    {option.preview}
                  </span>
                )}
              </span>
            </label>
          ))}
          <div className="ask-dialog-option ask-dialog-other">
            <label className="ask-dialog-other-choice">
            <input
              type={question.multi_select ? 'checkbox' : 'radio'}
              name={`ask-${request.request_id}-${current}`}
              checked={answer.useOther}
              onChange={toggleOther}
            />
            <span className="ask-dialog-option-title">Other</span>
            </label>
              <input
                aria-label="Other answer"
                value={answer.other}
                onFocus={() => {
                  if (!answer.useOther) toggleOther();
                }}
                onChange={(event) => updateAnswer({ ...answer, other: event.target.value, useOther: true })}
                className="ask-dialog-other-input"
                placeholder="Write your own answer…"
              />
          </div>
          {error && <div role="alert" className="ask-dialog-error">{error}</div>}
      </fieldset>
    </DesktopDialog>
  );
}
