import { test } from 'node:test';
import assert from 'node:assert/strict';

import type { ClientCommand, ClientEvent } from '../src/protocol';

test('AskUserQuestion event and answer commands mirror the Rust wire contract', () => {
  const event = {
    type: 'ask_user_question',
    request: {
      request_id: 9,
      questions: [{
        question: 'Choose targets',
        header: 'Targets',
        options: [{
          label: 'Tests',
          description: 'Run tests',
          preview: 'cargo test',
        }],
        multi_select: true,
      }],
      timeout_secs: 60,
    },
  } satisfies ClientEvent;
  const answer = {
    type: 'answer_ask_user_question',
    request_id: 9,
    answers: { 'Choose targets': 'Tests, docs' },
  } satisfies ClientCommand;
  const cancel = {
    type: 'cancel_ask_user_question',
    request_id: 9,
  } satisfies ClientCommand;
  const resolved = {
    type: 'ask_user_question_resolved',
    request_id: 9,
  } satisfies ClientEvent;

  assert.equal(JSON.stringify(event), '{"type":"ask_user_question","request":{"request_id":9,"questions":[{"question":"Choose targets","header":"Targets","options":[{"label":"Tests","description":"Run tests","preview":"cargo test"}],"multi_select":true}],"timeout_secs":60}}');
  assert.equal(JSON.stringify(answer), '{"type":"answer_ask_user_question","request_id":9,"answers":{"Choose targets":"Tests, docs"}}');
  assert.equal(JSON.stringify(cancel), '{"type":"cancel_ask_user_question","request_id":9}');
  assert.equal(JSON.stringify(resolved), '{"type":"ask_user_question_resolved","request_id":9}');
});
