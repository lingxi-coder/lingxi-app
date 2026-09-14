import type { RunItem } from '../model/runItem';
import type { SubmittedPlan } from './submittedPlan';
/** Only assistant-authored, explicitly identified plan documents become cards. */
function isTitledPlan(text: string): boolean {
  // Ignore examples inside fenced code; a document needs a plan heading and
  // multiple real sections, not a passing mention or a short "# Plan" reply.
  const prose = text.replace(/^(```|~~~)[\s\S]*?^\1[^\n]*$/gm, '');
  const title = /^#\s+(.+)$/m.exec(prose);
  return Boolean(title && /(?:计划|方案|\bplan\b|\bproposal\b)/i.test(title[1])
    && (prose.match(/^#{2,3}\s+\S/gm)?.length ?? 0) >= 2);
}
export function narrationPlans(items: readonly RunItem[]): SubmittedPlan[] {
 return items.flatMap(item => {
  if(item.type !== 'narration' || item.role !== 'assistant') return [];
  const match = /^\s*<proposed_plan>\s*([\s\S]*?)(?:\s*<\/proposed_plan>\s*|$)$/.exec(item.text);
  return match ? [{id:item.id,content:match[1],status:'submitted' as const}]
    : isTitledPlan(item.text) ? [{id:item.id,content:item.text,status:'submitted' as const}] : [];
 });
}

/** Keep different plan sources in transcript order. */
export function conversationPlans(items: readonly RunItem[], tools: readonly SubmittedPlan[]): SubmittedPlan[] {
  const plans = new Map([...tools, ...narrationPlans(items)].map(plan => [plan.id, plan]));
  const ordered: SubmittedPlan[] = [];
  for (const item of items) {
    const plan = plans.get(item.id);
    if (plan) { ordered.push(plan); plans.delete(item.id); }
  }
  return [...ordered, ...plans.values()];
}
