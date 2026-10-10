import type { ReactNode } from 'react';
import type { ToolRunItem } from '../model/runItem';
import { Disclosure } from './Disclosure';
import { Icon } from './Icon';
import { useT } from '../theme/ThemeContext';

export interface QuestionAnswerSummary { question: string; answer: string | null }

/** Preserve the tool's structured question/answer content without summarizing user text. */
export function questionAnswers(item: ToolRunItem): QuestionAnswerSummary[] | null {
  if (item.tool !== 'AskUserQuestion' || item.status !== 'done') return null;
  const output = item.nativeOutput;
  if (!output || typeof output !== 'object' || Array.isArray(output)) return null;
  const { questions, answers } = output;
  if (!Array.isArray(questions) || !questions.length || !answers || typeof answers !== 'object' || Array.isArray(answers)) return null;
  const rows: QuestionAnswerSummary[] = [];
  for (const question of questions) {
    if (!question || typeof question !== 'object' || Array.isArray(question) || typeof question.question !== 'string') return null;
    const answer = Object.prototype.hasOwnProperty.call(answers, question.question) ? answers[question.question] : undefined;
    if (answer !== undefined && typeof answer !== 'string') return null;
    rows.push({ question: question.question, answer: typeof answer === 'string' && answer.trim() ? answer : null });
  }
  return rows;
}

export function AskUserQuestionSummary({ id, rows, open, onToggle, keepMounted, renderBody = (body) => body }: {
  id: string; rows: QuestionAnswerSummary[]; open: boolean; onToggle(): void;
  keepMounted?: boolean; renderBody?(body: ReactNode): ReactNode;
}) {
  const t = useT();
  return <section className="ask-question-summary">
    <Disclosure id={id} open={open} onToggle={onToggle} keepMounted={keepMounted}
      buttonClassName="ask-question-summary-trigger"
      buttonStyle={{ color: t.text3, fontSize: 14, fontWeight: 400, opacity: .85, gap: 8, minHeight: 28 }}
      summary={<><Icon name="question" size={16} /><span>Asked {rows.length} {rows.length === 1 ? 'question' : 'questions'}</span></>}>
      {renderBody(<div className="ask-question-summary-list">
        {rows.map((row, index) => <div className="ask-question-summary-pair" key={index}>
          <p style={{ color: t.text3, opacity: .85 }}>{row.question}</p>
          <p style={{ color: t.text3, opacity: .5 }}>{row.answer ?? 'No answer provided'}</p>
        </div>)}
      </div>)}
    </Disclosure>
  </section>;
}
