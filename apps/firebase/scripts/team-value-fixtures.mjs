import { createHash } from 'node:crypto';
import { isDeepStrictEqual } from 'node:util';

export const digest = value => createHash('sha256').update(JSON.stringify(value) ?? 'undefined').digest('hex');
const item = (text, done = false) => ({ text, done });
const familyInitial = [item('牛奶：库存0盒；目标2盒；采购2盒'), item('鸡蛋：库存2个；目标6个；采购4个')];
const familyUpdated = [familyInitial[0], item('鸡蛋：库存2个；目标4个；采购2个')];
const familyFinal = [item(familyInitial[0].text, true), familyUpdated[1]];
const teamInitial = [item('A｜提交需求清单｜负责人甲｜截止2026-11-20｜无依赖'),
  item('B｜审核方案｜负责人乙｜截止2026-11-21｜无依赖'),
  item('C｜发布通知｜负责人丙｜截止2026-11-23｜依赖B')];
const teamCompleted = [item(teamInitial[0].text, true), ...teamInitial.slice(1)];
const teamFinal = [teamCompleted[0], item('B｜审核方案｜负责人乙｜截止2026-11-22｜无依赖'), teamInitial[2]];
export const FIXTURES = {
  schemaVersion: 1, fixtureId: 'assistant-value-p0-20261116', date: '2026-11-16',
  source: '.omx/plans/2026-11-16-assistant-value-improvement-plan-v2.md',
  participants: { family: ['家庭成员甲', '家庭成员乙'], team: ['团队成员一', '团队成员二'], owners: ['甲', '乙', '丙'] },
  familySource: '2026-11-16家庭库存：牛奶0盒，鸡蛋2个，大米1袋。目标：牛奶2盒，鸡蛋6个，大米1袋。只列需采购的项目。',
  teamSource: 'A提交需求清单，负责人甲，截止2026-11-20；B审核方案，负责人乙，截止2026-11-21；C发布通知，负责人丙，截止2026-11-23，依赖B。',
  family: [
    { id: 'F01', actor: 'first', action: 'submit', prompt: '请读取共享的家庭库存与目标，计算并生成采购清单，只列需要采购的物品。每项text采用“品名：库存N单位；目标N单位；采购N单位”的格式，初始done=false。', expected: familyInitial },
    { id: 'F02', actor: 'second', action: 'submit', prompt: '鸡蛋目标改为4个，库存不变。请重新计算并更新原采购清单，保留其他物品与完成状态，不得增加重复项。', expected: familyUpdated },
    { id: 'F03', actor: 'first', action: 'submit', prompt: '牛奶已购齐。仅将原牛奶项done标记true，保留所有text和鸡蛋done=false。', expected: familyFinal },
    { id: 'F04', actor: 'first', action: 'replay', replayOf: 'F03', expected: familyFinal },
    { id: 'F05', actor: 'secondDevice', action: 'reopen', expected: familyFinal },
  ],
  team: [
    { id: 'T01', actor: 'first', action: 'submit', prompt: '从共享项目资料生成A、B、C三项清单。text严格采用“标识｜事项｜负责人姓名｜截止YYYY-MM-DD｜无依赖/依赖B”；done均为false。保留全部负责人、日期和C依赖B。', expected: teamInitial },
    { id: 'T02', actor: 'second', action: 'submit', prompt: 'A提交需求清单已完成。只改原A项done=true，不改其余项。', expected: teamCompleted },
    { id: 'T03', actor: 'second', action: 'submit', prompt: 'B审核方案截止改为2026-11-22。更新原B项text，保留负责人乙、done=false；其余不改。', expected: teamFinal },
    { id: 'T04', actor: 'first', action: 'submit', prompt: '按最新清单生成共享说明文档，每项一行，逐字复制text并添加“｜已完成”或“｜未完成”。只用最新截止日期。不得改清单。', expected: teamFinal,
      expectedDocument: teamFinal.map(row => `${row.text}｜${row.done ? '已完成' : '未完成'}`).join('\n') },
    { id: 'T05', actor: 'secondDevice', action: 'reopenAndAsk', prompt: '重新打开后，请读取最新清单和共享Chat历史，回答：A是否完成、B最新截止日期、C负责人和依赖，并识别最后修改B截止日期的成员。用“A完成状态；B截止日期；C负责人姓名，依赖标识；B最后修改者成员显示名或稳定作者标识”的格式。', expected: teamFinal,
      expectedAnswer: 'A已完成；B截止2026-11-22；C负责人丙，依赖B' },
  ],
};
// Frozen fixtures are synthetic test inputs; importing this module never contacts a host.
export const FIXTURE_DIGEST = digest(FIXTURES);
export const STABLE_SOURCE = { fixtureId: FIXTURES.fixtureId, checksumValue: 'lingxi-p0-shared-source-20261116-v1' };
export const SHARED_CHECKSUM = digest(STABLE_SOURCE);

/** Deterministic ids allow the existing QA runner to replay the exact command.
 * F05 is a member read; T05 uses the same submit after the operator reopens UI. */
export function workflowCommand(caseId, { runId, scope, binding }) {
  if (!/^[A-Za-z0-9_-]{8,40}$/u.test(runId)) throw new Error('Valid frozen runId required');
  const fixture = FIXTURES.family.concat(FIXTURES.team).find(f => f.id === caseId);
  if (!fixture) throw new Error('Only F01–F05 and T01–T05 are supported');
  if (!['team', 'service'].includes(scope?.kind)) throw new Error('Explicit existing scope required');
  const sourceId = fixture.replayOf ?? caseId;
  const source = FIXTURES.family.concat(FIXTURES.team).find(f => f.id === sourceId);
  return { actor: source.actor, fixtureCaseId: caseId, command: {
    schemaVersion: 1, commandId: digest([runId, sourceId, 'command']),
    [scope.kind === 'service' ? 'serviceId' : 'teamId']: scope.id,
    generation: binding[source.actor].generation,
    action: sourceId === 'F05' ? 'readList' : 'submit',
    payload: sourceId === 'F05' ? { sharedBotId: binding.sharedBotId } : {
      conversationId: binding.conversationId, messageId: digest([runId, sourceId, 'message']), changeId: digest([runId, sourceId, 'change']),
      targets: [binding.sharedBotId], text: source.prompt,
    },
  } };
}

const same = isDeepStrictEqual;
export function gradeWorkflows(rows) {
  const suites = {};
  for (const name of ['family', 'team']) {
    const cases = FIXTURES[name].map(fixture => {
      const matches = rows.filter(row => row.caseId === fixture.id);
      const row = matches[0], failures = [];
      if (matches.length !== 1) failures.push('missing_or_duplicate_case');
      if (!same(row?.items?.map(({ text, done }) => ({ text, done })), fixture.expected)) failures.push('incorrect_list');
      const itemIds = row?.items?.map(i => i.itemId);
      if (!itemIds?.every(id => typeof id === 'string' && id.length > 0) || new Set(itemIds).size !== fixture.expected.length) failures.push('item_identity_required');
      const first = rows.find(r => r.caseId === FIXTURES[name][0].id);
      if (fixture.id !== FIXTURES[name][0].id && !same(itemIds, first?.items?.map(i => i.itemId))) failures.push('original_items_required');
      if (!row?.taskId || row?.state !== 'completed') failures.push('completed_task_required');
      if (!row?.sourceRef || !row?.participantRef || !Number.isFinite(Date.parse(row?.observedAt))) failures.push('evidence_required');
      if (fixture.expectedDocument && row?.documentBody?.trim() !== fixture.expectedDocument) failures.push('incorrect_document');
      if (fixture.expectedAnswer && fixture.id !== 'T05' && row?.answer?.trim() !== fixture.expectedAnswer) failures.push('incorrect_answer');
      if (fixture.id === 'T05') {
        const edit = rows.find(r => r.caseId === 'T03'), author = edit?.requestMessage?.author, change = edit?.requestChange;
        const member = edit?.participantIdentity, command = edit?.command;
        const validSource = edit?.participantRef === 'second' && typeof member?.uid === 'string' && member.uid.length > 0 && Number.isSafeInteger(member.generation) && member.generation > 0
          && command?.action === 'submit' && command.generation === member.generation
          && command.payload?.text === FIXTURES.team[2].prompt
          && command.payload?.messageId === edit.requestMessage?.id && command.payload?.changeId === change?.id
          && change?.messageId === edit.requestMessage?.id && change?.actorUid === member.uid && change?.generation === member.generation
          && author?.kind === 'user' && author.uid === member.uid
          && Date.parse(edit.requestMessage?.createdAt) === Date.parse(change?.at)
          && Number.isFinite(Date.parse(change?.at)) && Date.parse(change.at) <= Date.parse(edit.observedAt)
          && Date.parse(change.at) < Date.parse(row?.observedAt);
        if (!validSource) failures.push('T03_actual_modifier_source_required');
        const requiredSource = change ? { messageId: change.messageId, changeId: change.id, actorUid: change.actorUid, generation: change.generation, at: change.at } : undefined;
        if (!requiredSource || !same(row?.modifierSource, requiredSource)) failures.push('latest_modifier_history_reference_required');
        const names = validSource ? [author.displayName, author.uid].filter(v => typeof v === 'string' && v.length > 0) : [];
        if (!names.some(name => row?.answer?.trim() === `${fixture.expectedAnswer}；B最后修改者${name}`)) failures.push('incorrect_answer_or_modifier');
      }
      if (fixture.action.includes('reopen') && (!row?.differentDevice || !row?.historyVerified || !row?.sameArtifact)) failures.push('reopen_evidence_required');
      if (fixture.id === 'F04') {
        const original = rows.find(r => r.caseId === 'F03');
        if (!same(row?.command, original?.command) || row?.command?.action !== 'submit'
          || !row?.command?.commandId || !row?.command?.payload?.messageId || !row?.command?.payload?.changeId
          || row?.taskId !== original?.taskId) failures.push('original_F03_replay_required');
        if (!row?.before || !row?.after || !same(row.before, row.after)
          || !['taskIds', 'quotaCounts', 'usage', 'operationIds', 'listDataRevision', 'historyDigest'].every(key => row.before[key] !== undefined)) failures.push('duplicate_side_effect_or_missing_snapshot');
        if (!row?.before?.taskIds?.includes(original?.taskId)
          || !Array.isArray(row?.before?.operationIds)
          || !['team', 'member', 'bot'].every(k => Number.isSafeInteger(row?.before?.quotaCounts?.[k]) && row.before.quotaCounts[k] >= 0)
          || !['complete', 'partial', 'missing'].includes(row?.before?.usage?.status)) failures.push('replay_snapshot_incomplete');
      }
      return { caseId: fixture.id, passed: failures.length === 0, failures };
    });
    suites[name] = { passed: cases.every(c => c.passed), count: cases.filter(c => c.passed).length, denominator: 5, cases };
  }
  return { ...suites, passed: suites.family.passed && suites.team.passed,
    legacy: { denominator: 30, minimum: 27, status: 'not_graded_here' },
    unexpectedCases: rows.filter(row => !FIXTURES.family.concat(FIXTURES.team).some(f => f.id === row.caseId)).map(r => r.caseId) };
}
