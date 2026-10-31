import { createRequire } from 'node:module';
import { readFile, writeFile } from 'node:fs/promises';
import { homedir } from 'node:os';
import path from 'node:path';

const require = createRequire(import.meta.url);
const modulePath = process.env.TEAMS_PLAYWRIGHT_MODULE || path.join(homedir(), '.cache/codex-runtimes/codex-primary-runtime/dependencies/node/node_modules/playwright');
const { chromium } = require(modulePath);
const source = await readFile(new URL('./journey.js', import.meta.url), 'utf8');
const journey = new Function(`return (${source});`)();
if (process.argv.includes('--check')) {
  console.log('TEAMS_T3_WEB_JOURNEY_SOURCE_READY: existing Playwright resolved; journey syntax valid; no browser or network started.');
} else {
  const browser = await chromium.launch({ headless: !process.argv.includes('--headed'), ...(process.env.CHROME_EXECUTABLE ? { executablePath: process.env.CHROME_EXECUTABLE } : {}) });
  let result;
  const finalReplies = [];
  try {
    const page = await browser.newPage();
    const answeredRuns = new Map();
    page.on('response', response => {
      if (!response.ok() || !response.url().endsWith('/v1/teams/messages')) return;
      const request = response.request();
      const command = request.postDataJSON();
      if (command?.action !== 'botAnswer') return;
      // Keep the real UI token only in memory for authenticated read commands.
      // Neither it nor the answer map is included in the acceptance artifacts.
      answeredRuns.set(JSON.stringify([command.teamId, command.conversationId, command.triggerId]), {
        teamId: command.teamId, generation: command.generation, conversationId: command.conversationId,
        triggerId: command.triggerId, authorization: request.headers().authorization,
      });
    });
    result = await journey(page);
    result.checks.liveQuestionsCompleteWithExactReplies = false;
    if (answeredRuns.size !== 3) throw new Error(`Expected three distinct answered live runs; observed ${answeredRuns.size}`);
    const expectedFinalText = 'Current acceptance result completed once.';
    const deadline = Date.now() + 60000;
    for (const run of answeredRuns.values()) {
      const { authorization, triggerId, ...scope } = run;
      if (!authorization?.startsWith('Bearer ')) throw new Error('Missing authenticated UI request for completion reads');
      const post = async command => {
        const response = await page.request.post('http://127.0.0.1:18784/v1/teams/messages', {
          headers: { Authorization: authorization }, data: { ...scope, ...command }, timeout: 5000,
        });
        if (!response.ok()) throw new Error(`Completion ${command.action} read failed (${response.status()})`);
        return response.json();
      };
      let state;
      while (Date.now() < deadline) {
        state = await post({ action: 'botState', triggerId });
        if (state.state === 'completed') break;
        if (['failed', 'missed', 'cancelled', 'recovered', 'revoked', 'recovery'].includes(state.state)) {
          throw new Error(`Answered run ${triggerId} ended as ${state.state}`);
        }
        await new Promise(resolve => setTimeout(resolve, 250));
      }
      if (state?.state !== 'completed') throw new Error(`Answered run ${triggerId} did not complete before the deadline`);
      const messages = new Map();
      let after = 0, hasMore;
      do {
        const batch = await post({ action: 'sync', after });
        const previous = after;
        for (const message of batch.messages) messages.set(message.id, message);
        for (const change of batch.changes) after = Math.max(after, change.seq);
        hasMore = batch.hasMore;
        if (hasMore && after <= previous) throw new Error('Completion sync cursor did not advance');
      } while (hasMore);
      const replies = [...messages.values()].filter(message => message.triggerId === triggerId && message.author.kind === 'bot' && message.kind === 'text');
      if (replies.length !== 1 || replies[0].recalled || replies[0].author.botKey !== state.botKey || replies[0].text !== expectedFinalText) {
        throw new Error(`Answered run ${triggerId} must have exactly one unchanged final Bot text reply with the fixture result`);
      }
      finalReplies.push({ teamId: scope.teamId, conversationId: scope.conversationId, triggerId,
        state: state.state, replyId: replies[0].id, replyCount: replies.length, text: replies[0].text });
    }
    result.checks.liveQuestionsCompleteWithExactReplies = true;
    result.finalReplies = finalReplies;
    await writeFile(path.join(result.output, 'result.json'), JSON.stringify(result, null, 2));
    console.log(JSON.stringify({ passed: result.passed, checks: result.checks, output: result.output }, null, 2));
  } catch (error) {
    if (result) {
      await writeFile(path.join(result.output, 'result.json'), JSON.stringify({ ...result, passed: false, finalReplies, completionError: String(error) }, null, 2));
    }
    throw error;
  } finally { await browser.close(); }
}
