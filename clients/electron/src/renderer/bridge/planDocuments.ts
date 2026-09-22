import type { RunItem } from '../model/runItem';
import type { SubmittedPlan } from './submittedPlan';
/** Only assistant-authored, explicitly identified plan documents become cards. */
function isTitledPlan(text: string): boolean {
  // Ignore examples inside fenced code; a document needs a plan heading and
  // multiple real sections, not a passing mention or a short "# Plan" reply.
  const prose = text.replace(/^(```|~~~)[\s\S]*?^\1[^\n]*$/gm, '');
  // Plans arrive as "# Proposal" AND as "## 计划" whose sections are bold-led
  // lines ("**目标**：…"), so the title may sit on either of the top two heading
  // levels and a section may be a heading below the title or a bold-led line.
  // The title may not be the FIRST heading in the document: a preamble section
  // ("## Overview") can precede it. Take the first level-1/2 heading whose text
  // actually names a plan, so a leading non-plan heading cannot hide the card.
  const title = [...prose.matchAll(/^(#{1,2})\s+(.+)$/gm)]
    .find(candidate => /(?:计划|方案|\bplan\b|\bproposal\b)/i.test(candidate[2]));
  if (!title) return false;
  const titleLevel = title[1].length;
  const sections = prose.slice(title.index + title[0].length).split('\n').filter(line => {
    const heading = /^(#{1,6})\s+\S/.exec(line.trimStart());
    return heading ? heading[1].length > titleLevel : /^\*\*[^*\n]+\*\*/.test(line.trim());
  });
  return sections.length >= 2;
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
