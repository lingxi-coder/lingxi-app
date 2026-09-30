import { test } from 'node:test';
import assert from 'node:assert/strict';

import {
  activeFileMention,
  fileMentionsFromPrompt,
  fileMentionToken,
  promptWithFileMentions,
} from '../src/renderer/bridge/fileMentions';

test('active file mention follows the whitespace-delimited TUI semantics', () => {
  assert.deepEqual(activeFileMention('inspect @src/ma please', 15), {
    start: 8,
    end: 15,
    query: 'src/ma',
  });
  assert.equal(activeFileMention('email@example.com', 9), null);
  assert.equal(activeFileMention('plain text', 5), null);
});

test('serialized file mentions can be reconstructed without parsing the prompt body', () => {
  assert.deepEqual(
    fileMentionsFromPrompt('@src/app.ts @"My Files/read\\ me.md"\n\nreview this'),
    ['src/app.ts', 'My Files/read me.md'],
  );
  assert.deepEqual(fileMentionsFromPrompt('@src/app.ts'), ['src/app.ts']);
  assert.deepEqual(fileMentionsFromPrompt('email@example.com\n\nbody'), []);
  assert.deepEqual(fileMentionsFromPrompt('@"unterminated\n\nbody'), []);
  assert.deepEqual(fileMentionsFromPrompt('@src/app.ts extra\n\nbody'), []);
});

test('file mentions quote whitespace paths', () => {
  assert.equal(fileMentionToken('My Files/read me.md'), '@"My Files/read me.md"');
  const text = 'review @"My Files/read me.md" now';
  assert.deepEqual(activeFileMention(text, 20), {
    start: 7,
    end: 29,
    query: 'My Files/re',
  });
});

test('rich file tokens serialize into the engine prompt once without changing the visible body', () => {
  assert.equal(promptWithFileMentions('review this', ['src/app.ts']), '@src/app.ts\n\nreview this');
  assert.equal(promptWithFileMentions('', ['src/app.ts']), '@src/app.ts');
  assert.equal(
    promptWithFileMentions('review', ['src/app.ts', 'src/app.ts', 'My Files/read me.md']),
    '@src/app.ts @"My Files/read me.md"\n\nreview',
  );
});
