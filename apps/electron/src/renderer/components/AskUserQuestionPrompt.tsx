import { useEffect, useRef, useState, type CSSProperties } from 'react';
import type { AskQuestionDto, AskUserQuestionRequestDto } from '@lingxi/bridge-client';

import { DesktopDialogActions, DesktopDialogButton } from './DesktopDialog';
import { Icon } from './Icon';
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

const EMPTY_ANSWER: QuestionAnswer = { selected: new Set(), other: '', useOther: false };

function emptyAnswers(request: AskUserQuestionRequestDto | null): QuestionAnswer[] {
  return request?.questions.map(() => ({ ...EMPTY_ANSWER, selected: new Set<string>() })) ?? [];
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

export function AskUserQuestionPrompt({ request, onSubmit, onCancel }: AskUserQuestionPromptProps) {
  const t = useT();
  const [expandedQuestion, setExpandedQuestion] = useState<number | null>(0);
  const [answers, setAnswers] = useState<QuestionAnswer[]>(() => emptyAnswers(request));
  const [error, setError] = useState<string | null>(null);
  const questionTriggers = useRef<Array<HTMLButtonElement | null>>([]);
  const submitButton = useRef<HTMLButtonElement>(null);

  useEffect(() => {
    setExpandedQuestion(0);
    setAnswers(emptyAnswers(request));
    setError(null);
  }, [request?.request_id]);

  if (!request || request.questions.length === 0) return null;

  const commitAnswer = (questionIndex: number, next: QuestionAnswer, advance = false) => {
    const nextAnswers = answers.map((entry, index) => index === questionIndex ? next : entry);
    setAnswers(nextAnswers);
    setError(null);
    if (advance) {
      const firstUnanswered = request.questions.findIndex((question, index) =>
        !answerText(question, nextAnswers[index] ?? EMPTY_ANSWER),
      );
      setExpandedQuestion(firstUnanswered >= 0 ? firstUnanswered : null);
      if (firstUnanswered >= 0) questionTriggers.current[firstUnanswered]?.focus();
      else submitButton.current?.focus();
    }
  };

  const toggleOption = (questionIndex: number, label: string) => {
    const question = request.questions[questionIndex];
    const answer = answers[questionIndex] ?? EMPTY_ANSWER;
    if (!question) return;
    if (!question.multi_select) {
      commitAnswer(questionIndex, { ...answer, selected: new Set([label]), useOther: false }, true);
      return;
    }
    const selected = new Set(answer.selected);
    if (selected.has(label)) selected.delete(label);
    else selected.add(label);
    commitAnswer(questionIndex, { ...answer, selected });
  };

  const toggleOther = (questionIndex: number) => {
    const question = request.questions[questionIndex];
    const answer = answers[questionIndex] ?? EMPTY_ANSWER;
    if (!question) return;
    commitAnswer(questionIndex, {
      ...answer,
      selected: question.multi_select ? answer.selected : new Set(),
      useOther: !answer.useOther,
    });
  };

  const submit = () => {
    const missingIndex = request.questions.findIndex((question, index) =>
      !answerText(question, answers[index] ?? EMPTY_ANSWER),
    );
    if (missingIndex >= 0) {
      setExpandedQuestion(missingIndex);
      questionTriggers.current[missingIndex]?.focus();
      setError('Answer each question before sending.');
      return;
    }
    const result: Record<string, string> = {};
    for (const [index, question] of request.questions.entries()) {
      const value = answerText(question, answers[index] ?? EMPTY_ANSWER);
      if (value) result[question.question] = value;
    }
    onSubmit(request.request_id, result);
  };

  const answeredCount = request.questions.reduce(
    (count, question, index) => count + Number(Boolean(answerText(question, answers[index] ?? EMPTY_ANSWER))),
    0,
  );
  const dialogVariables = {
    '--dialog-window': t.windowBg,
    '--dialog-surface': t.surface,
    '--dialog-surface-hover': t.surfaceHover,
    '--dialog-border': t.border,
    '--dialog-border-strong': t.borderStrong,
    '--dialog-text': t.text,
    '--dialog-text-2': t.text2,
    '--dialog-text-3': t.text3,
    '--dialog-accent': t.text2,
    '--dialog-accent-border': t.borderStrong,
    '--dialog-danger': t.danger,
  } as CSSProperties;

  return (
    <section
      className="inline-interaction-card ask-user-question-inline"
      style={dialogVariables}
      role="region"
      aria-labelledby="lingxi-ask-heading"
      aria-describedby="lingxi-ask-instructions"
      onKeyDown={(event) => {
        if (event.key === 'Escape') {
          event.preventDefault();
          onCancel(request.request_id);
        }
      }}
    >
      <header className="inline-interaction-header">
        <span className="inline-interaction-heading-icon" aria-hidden="true">
          <Icon name="question" size={19} stroke={1.7} />
        </span>
        <h2 id="lingxi-ask-heading">{request.questions.length} {request.questions.length === 1 ? 'question' : 'questions'}</h2>
        <span className="ask-dialog-progress">{answeredCount} of {request.questions.length} answered</span>
        <button
          type="button"
          className="inline-interaction-close"
          aria-label="Skip question request"
          title="Skip"
          onClick={() => onCancel(request.request_id)}
        >
          <Icon name="x" size={17} stroke={1.8} />
        </button>
      </header>

      <div className="inline-interaction-body ask-dialog-body">
        <p id="lingxi-ask-instructions" className="ask-dialog-instructions">
          {request.questions.length === 1
            ? (request.questions[0]?.multi_select ? 'Choose one or more options, or write your own response.' : 'Choose an option, or write your own response.')
            : 'Answer each question below. You can reopen a question to change your answer.'}
        </p>
        {request.timeout_secs !== undefined && (
          <p className="ask-dialog-timeout">
            <Icon name="clock" size={14} stroke={1.8} />
            <span>{`Auto-continue in ${request.timeout_secs}s if unanswered`}</span>
          </p>
        )}
        {error && <div role="alert" className="ask-dialog-error">{error}</div>}

        <div className="ask-dialog-question-list">
          {request.questions.map((question, questionIndex) => {
            const answer = answers[questionIndex] ?? EMPTY_ANSWER;
            const currentAnswer = answerText(question, answer);
            const expanded = expandedQuestion === questionIndex;
            const optionListId = `ask-question-${request.request_id}-${questionIndex}-options`;
            return (
              <section
                key={`${questionIndex}-${question.question}`}
                className="ask-dialog-question-card"
                data-expanded={expanded}
                data-answered={Boolean(currentAnswer)}
              >
                <button
                  type="button"
                  className="ask-dialog-question-trigger"
                  ref={(node) => { questionTriggers.current[questionIndex] = node; }}
                  aria-expanded={expanded}
                  aria-controls={optionListId}
                  onClick={() => setExpandedQuestion(expanded ? null : questionIndex)}
                >
                  <span className="ask-dialog-question-index" aria-hidden="true">{questionIndex + 1}</span>
                  <span className="ask-dialog-question-copy">
                    <span className="ask-dialog-question-header">{question.header}</span>
                    <span className="ask-dialog-question-title">{question.question}</span>
                    {!expanded && (
                      <span className="ask-dialog-answer-summary" data-empty={!currentAnswer}>
                        {currentAnswer ?? 'Choose an answer'}
                      </span>
                    )}
                  </span>
                  <span className="ask-dialog-question-trailing" aria-hidden="true">
                    {currentAnswer && <Icon name="check" size={15} stroke={2} />}
                    <Icon name={expanded ? 'chevron' : 'chevronR'} size={15} stroke={1.8} />
                  </span>
                </button>

                {expanded && (
                  <fieldset id={optionListId} className="ask-dialog-options">
                    <legend className="ask-dialog-legend">
                      {question.multi_select ? 'Select one or more options' : 'Select one option'}
                    </legend>
                    {question.options.map((option, optionIndex) => (
                      <label key={option.label} className="ask-dialog-option">
                        <input
                          type={question.multi_select ? 'checkbox' : 'radio'}
                          name={`ask-${request.request_id}-${questionIndex}`}
                          checked={answer.selected.has(option.label)}
                          onChange={() => toggleOption(questionIndex, option.label)}
                        />
                        <span className="ask-dialog-option-index" aria-hidden="true">{optionIndex + 1}</span>
                        <span className="ask-dialog-option-copy">
                          <span className="ask-dialog-option-title">{option.label}</span>
                          <span className="ask-dialog-option-description">{option.description}</span>
                          {option.preview && <span className="ask-dialog-option-preview">{option.preview}</span>}
                        </span>
                      </label>
                    ))}
                    <div className="ask-dialog-other" data-selected={answer.useOther}>
                      <label className="ask-dialog-other-choice">
                        <input
                          type={question.multi_select ? 'checkbox' : 'radio'}
                          name={`ask-${request.request_id}-${questionIndex}`}
                          checked={answer.useOther}
                          onChange={() => toggleOther(questionIndex)}
                        />
                        <span className="ask-dialog-other-icon" aria-hidden="true">
                          <Icon name="compose" size={15} stroke={1.8} />
                        </span>
                        <span className="ask-dialog-option-title">Other</span>
                      </label>
                      <input
                        aria-label={`Your own answer for question ${questionIndex + 1}`}
                        value={answer.other}
                        onFocus={() => { if (!answer.useOther) toggleOther(questionIndex); }}
                        onChange={(event) => commitAnswer(questionIndex, { ...answer, other: event.target.value, useOther: true })}
                        className="ask-dialog-other-input"
                        placeholder="Or write your own response…"
                      />
                    </div>
                  </fieldset>
                )}
              </section>
            );
          })}
        </div>
      </div>

      <footer className="inline-interaction-footer ask-dialog-footer">
        <DesktopDialogActions>
          <DesktopDialogButton variant="cancel" onClick={() => onCancel(request.request_id)}>
            Skip
          </DesktopDialogButton>
          <DesktopDialogButton ref={submitButton} variant="primary" onClick={submit}>
            Send
          </DesktopDialogButton>
        </DesktopDialogActions>
      </footer>
    </section>
  );
}
