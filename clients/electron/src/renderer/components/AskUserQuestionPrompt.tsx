import { useEffect, useMemo, useRef, useState } from 'react';
import type { AskQuestionDto, AskUserQuestionRequestDto } from '@lingxi/bridge-client';

import { useT } from '../theme/ThemeContext';

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
    <div
      role="dialog"
      aria-labelledby="lingxi-ask-title"
      onFocusCapture={() => { promptHasFocus.current = true; }}
      onBlurCapture={(event) => { promptHasFocus.current = event.currentTarget.contains(event.relatedTarget as Node | null); }}
      onKeyDown={(event) => {
        if (event.key !== 'Escape') return;
        event.preventDefault();
        onCancel(request.request_id);
      }}
      style={{
        position: 'absolute', inset: 0, zIndex: 70,
        display: 'flex', alignItems: 'center', justifyContent: 'center',
        background: 'rgba(0,0,0,0.32)',
      }}
    >
      <div
        ref={dialogRef}
        tabIndex={-1}
        style={{
          width: 520, maxWidth: '92%', maxHeight: '85%', overflow: 'auto',
          borderRadius: 14, background: t.windowBg, border: `0.5px solid ${t.border}`,
          boxShadow: '0 18px 48px rgba(0,0,0,0.34)',
        }}
      >
        <div style={{ padding: '18px 20px 12px', borderBottom: `0.5px solid ${t.border}` }}>
          <div style={{ display: 'flex', justifyContent: 'space-between', gap: 12 }}>
            <span style={{ color: t.accent, fontSize: 12, fontWeight: 700 }}>{question.header}</span>
            <span style={{ color: t.text3, fontSize: 12 }}>{progress}</span>
          </div>
          <div id="lingxi-ask-title" style={{ marginTop: 8, color: t.text, fontSize: 15, fontWeight: 600 }}>
            {question.question}
          </div>
          {request.timeout_secs !== undefined && (
            <div style={{ marginTop: 6, color: t.text3, fontSize: 11 }}>
              {`Auto-continue in ${request.timeout_secs}s if unanswered`}
            </div>
          )}
        </div>

        <fieldset style={{ border: 0, margin: 0, padding: '12px 20px' }}>
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

        <div style={{ display: 'flex', gap: 8, padding: '12px 16px', borderTop: `0.5px solid ${t.border}`, background: t.surface }}>
          <button type="button" onClick={() => onCancel(request.request_id)} style={{ flex: 1, padding: 8 }}>
            Cancel
          </button>
          {current > 0 && (
            <button type="button" onClick={() => { setCurrent(current - 1); setError(null); }} style={{ flex: 1, padding: 8 }}>
              Back
            </button>
          )}
          <button type="button" onClick={advance} style={{ flex: 1, padding: 8, color: '#fff', background: t.accent }}>
            {current + 1 === request.questions.length ? 'Submit' : 'Next'}
          </button>
        </div>
      </div>
    </div>
  );
}
