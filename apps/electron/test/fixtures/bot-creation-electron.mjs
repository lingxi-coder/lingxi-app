import assert from 'node:assert/strict';
import { writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { app, BrowserWindow } from 'electron';
import { configureFixtureWindow, waitForFixture } from './electron-test-environment.mjs';

app.setPath('userData', process.env.LINGXI_TEST_USER_DATA);
const proposal = { schemaVersion: 1, name: 'Writing partner', description: 'Edits articles while preserving your voice.', persona: 'You are a writing partner.\nClarify the audience, preserve the author’s voice and explain significant edits.' };

async function main() {
  await app.whenReady();
  const win = new BrowserWindow({ show: false, width: 1280, height: 920, webPreferences: { sandbox: true, backgroundThrottling: false } });
  const errors = [];
  win.webContents.on('console-message', details => { if (details.level === 'error' || details.level === 3) errors.push(details.message); });
  const run = code => win.webContents.executeJavaScript(code);
  const wait = expression => waitForFixture(win.webContents, `Boolean(${expression})`);
  const click = selector => run(`document.querySelector(${JSON.stringify(selector)}).click()`);
  const clickText = (selector, text) => run(`Array.from(document.querySelectorAll(${JSON.stringify(selector)})).find(element => element.textContent.trim() === ${JSON.stringify(text)}).click()`);
  const calls = name => run(`window.fixtureCalls.filter(call => call[0] === ${JSON.stringify(name)})`);
  const setValue = (selector, text) => run(`(() => {
    const input = document.querySelector(${JSON.stringify(selector)});
    Object.getOwnPropertyDescriptor(Object.getPrototypeOf(input), 'value').set.call(input, ${JSON.stringify(text)});
    input.dispatchEvent(new Event('input', { bubbles: true }));
  })()`);
  const screenshot = async name => {
    if (!process.env.LINGXI_BOT_CREATION_SCREENSHOT_DIR) return;
    await run('new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve)))');
    await new Promise(resolve => setTimeout(resolve, 100));
    writeFileSync(join(process.env.LINGXI_BOT_CREATION_SCREENSHOT_DIR, name + '.png'), (await win.webContents.capturePage()).toPNG());
  };
  try {
    await win.loadURL(process.argv.find(arg => arg.startsWith('http://')));
    await configureFixtureWindow(win);
    await wait(`document.querySelector('.desktop-nav-rail [aria-label="Bots"]')`);
    await click('.desktop-nav-rail [aria-label="Bots"]');
    await clickText('.bots-page button', 'New Bot');
    await wait(`document.querySelector('.bot-create-intro')`);
    assert.equal(await run(`document.querySelector('.bot-create-intro button[type="submit"]').disabled`), true);
    await clickText('.bot-create-examples button', 'Writing partner');
    const request = await run(`document.querySelector('.bot-create-intro textarea').value`);
    assert.match(request, /writing partner/);
    assert.equal(await run(`document.activeElement === document.querySelector('.bot-create-intro textarea')`), true);
    await screenshot('bot-creation-intro');

    // Switching methods preserves both drafts; leaving asks before discarding.
    await clickText('.bot-create-mode button', 'Set up manually');
    await setValue('.bot-create-fields input', 'Manual draft');
    await clickText('.bot-create-mode button', 'Create with AI');
    assert.equal(await run(`document.querySelector('.bot-create-intro textarea').value`), request);
    await clickText('.bot-create-mode button', 'Set up manually');
    assert.equal(await run(`document.querySelector('.bot-create-fields input').value`), 'Manual draft');
    await clickText('.bot-create-mode button', 'Create with AI');
    await click('.bots-back');
    await wait(`document.querySelector('[role="dialog"]')`);
    await click('[role="dialog"] .desktop-dialog-action--cancel');
    await wait(`!document.querySelector('[role="dialog"]')`);
    assert.equal(await run(`document.querySelector('.bot-create-intro textarea').value`), request);

    // A failed host leaves the request intact. Two submit events open only one chat.
    const submitTwice = selector => run(`(() => { const form = document.querySelector(${JSON.stringify(selector)}); form.requestSubmit(); form.requestSubmit(); })()`);
    await submitTwice('.bot-create-intro');
    await wait(`document.querySelector('.bot-create-intro [role="alert"]')`);
    assert.equal((await calls('newSession')).length, 1);
    assert.equal(await run(`document.querySelector('.bot-create-intro textarea').value`), request);
    await submitTwice('.bot-create-intro');
    await wait(`document.querySelector('.beta-rich-prompt')?.textContent.startsWith('/create-bot ')`);
    assert.equal((await calls('newSession')).length, 2);
    assert.deepEqual((await calls('newSession')).at(-1), ['newSession', null]);
    assert.equal(await run(`document.querySelector('.beta-rich-prompt').textContent`), `/create-bot ${request}`);
    assert.equal(await run(`document.querySelector('.bots-page')?.getClientRects().length > 0`), false);
    assert.equal((await calls('command')).filter(call => ['run_slash_command', 'send_prompt'].includes(call[2]?.type)).length, 0);
    await click('.composer-send-presence .composer-submit-button');
    await wait(`window.fixtureCalls.some(call => call[0] === 'command' && call[2]?.type === 'run_slash_command')`);
    assert.equal((await calls('command')).find(call => call[2]?.type === 'run_slash_command')[2].raw, `/create-bot ${request}`);

    // The same runtime events as a streaming model response reveal the card only on closure.
    await run(`window.botCreationFixture.emit({type:'turn_started', turn_id:1}); window.botCreationFixture.emit({type:'text_delta',text:${JSON.stringify('```lingxi-bot\n' + JSON.stringify(proposal) + '\n')}})`);
    await wait(`document.querySelector('.markdown-content .code-card')`);
    assert.equal(await run(`Boolean(document.querySelector('.bot-proposal'))`), false);
    await run(`window.botCreationFixture.emit({type:'text_delta',text:'\x60\x60\x60'})`);
    await wait(`document.querySelector('.bot-proposal button')`);
    await run(`window.botCreationFixture.emit({type:'turn_ended',outcome:{type:'end_turn'},cost:{total_usd:0,input_tokens:0,output_tokens:0,api_calls:1,session_duration_secs:1,formatted:'1s'}})`);
    assert.equal((await calls('create')).length, 0);
    await screenshot('bot-creation-proposal');
    await click('.bot-proposal button');
    await wait(`document.querySelector('.bot-create-fields input')?.value === 'Writing partner'`);
    assert.equal((await calls('create')).length, 0, 'review only fills the form');
    assert.equal(await run(`document.querySelector('.bots-persona').value`), proposal.persona);
    assert.deepEqual(await run(`Array.from(document.querySelectorAll('.bot-create-fields select')).map(select => select.value)`), ['', 'default']);
    await setValue('.bot-create-fields input', 'My writing partner');
    await submitTwice('.bot-create-fields');
    await wait(`document.querySelector('.bot-create-fields [role="alert"]')`);
    assert.equal((await calls('create')).length, 1);
    assert.equal(await run(`document.querySelector('.bot-create-fields input').value`), 'My writing partner');
    assert.equal(await run(`document.querySelector('.bots-persona').value`), proposal.persona);
    await screenshot('bot-creation-review');
    await submitTwice('.bot-create-fields');
    await wait(`document.querySelector('.bots-page h1')?.textContent === 'My writing partner'`);
    assert.equal((await calls('create')).length, 2);
    assert.deepEqual((await calls('create')).at(-1)[1], { name: 'My writing partner', description: proposal.description, persona: proposal.persona, permissionMode: 'default' });
    assert.deepEqual(errors, []);
    console.log(JSON.stringify({ draftPreserved: true, chatPrepared: true, streamingPreview: true, reviewed: true, retried: true }));
    win.destroy(); app.exit(0);
  } catch (error) {
    console.error(error, '\nconsole:', errors, '\npage:', await run('document.body.innerText'));
    win.destroy(); app.exit(1);
  }
}
void main().catch(error => { console.error(error); app.exit(1); });
