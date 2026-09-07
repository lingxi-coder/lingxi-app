import { app, BrowserWindow } from 'electron';
const url = process.argv.find((argument) => argument.startsWith('http://'));
if (!url || !process.env.LINGXI_TEST_USER_DATA) throw new Error('fixture URL and isolated user data required');
app.setPath('userData', process.env.LINGXI_TEST_USER_DATA);
const delay = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
async function main() {
  await app.whenReady();
  const window = new BrowserWindow({ show: false, width: 1440, height: 1000, webPreferences: { sandbox: true } });
  try {
    await window.loadURL(url);
    const run = (script) => window.webContents.executeJavaScript(script);
    const wait = async (expression) => {
      const deadline = Date.now() + 6000;
      while (Date.now() < deadline) { if (await run(expression)) return; await delay(20); }
      throw new Error(`Timed out: ${expression}\n${await run('document.body.innerText')}`);
    };
    const click = async (text) => {
      const expression = `[...document.querySelectorAll('button')].find(b => b.textContent.trim().replace(/→$/, '').trim() === ${JSON.stringify(text)})`;
      await wait(`Boolean(${expression})`);
      await run(`${expression}.click()`);
      await delay(30);
    };
    const state = () => run('window.scheduledCronFixture.state()');
    const setTitle = async (value) => {
      await run(`(() => { const input = document.querySelector('.scheduled-setup-title'); Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value').set.call(input, ${JSON.stringify(value)}); input.dispatchEvent(new Event('input', {bubbles:true})); })()`);
      await delay(30);
    };
    await wait("Boolean(document.querySelector('.scheduled-suggestion'))");
    await run("[...document.querySelectorAll('.scheduled-suggestion')].find(b => b.textContent.includes('Weekly review')).click()");
    for (let step = 1; step <= 4; step++) {
      await wait(`document.querySelector('.scheduled-setup-progress')?.textContent === 'Step ${step} of 4'`);
      await click(step === 4 ? 'Review task details' : 'Continue');
    }
    await wait("Boolean(document.querySelector('[aria-label=\"Editable task details\"]'))");
    await click('Create task');
    await wait("document.querySelectorAll('.scheduled-job-row').length === 1");
    const result = { created: await state() };
    await run("document.querySelector('.scheduled-job-row .scheduled-suggestion').click()");
    await wait("Boolean(document.querySelector('.scheduled-setup-title'))");
    await setTitle('Edited weekly review');
    await click('Save changes');
    await wait("Boolean(document.querySelector('.scheduled-job-row'))");
    await run("document.querySelector('.scheduled-job-row .scheduled-suggestion').click()");
    await wait("Boolean(document.querySelector('.scheduled-setup-title'))");
    result.reopenedTitle = await run("document.querySelector('.scheduled-setup-title').value");
    await setTitle('Unsaved title');
    await run("window.scheduledCronFixture.rejectNext('Fixture backend rejected save')");
    await click('Save changes');
    await wait("Boolean(document.querySelector('[role=alert]'))");
    result.rejected = await run("({error:document.querySelector('[role=alert]').textContent,title:document.querySelector('.scheduled-setup-title').value,state:window.scheduledCronFixture.state()})");
    await run("document.querySelector('.scheduled-setup-back').click()");
    await wait("Boolean(document.querySelector('.scheduled-job-row'))");
    await click('Delete');
    if ((await state()).jobs.length !== 1) throw new Error('Delete must require confirmation');
    await click('Confirm delete');
    await wait("document.querySelectorAll('.scheduled-job-row').length === 0");
    result.deleted = await state();
    await run('window.scheduledCronFixture.addExternal()');
    await click('Refresh');
    await wait("document.querySelectorAll('.scheduled-job-row').length === 1");
    result.refreshed = await state();
    await run("document.querySelector('.scheduled-job-row .scheduled-suggestion').click()");
    await wait("Boolean(document.querySelector('.scheduled-setup-title'))");
    await setTitle('Saved during navigation');
    const offset = (await state()).requests.length;
    await run('window.scheduledCronFixture.holdMutation()');
    await click('Save changes');
    await run('window.scheduledCronFixture.setVisible(false)');
    await run('window.scheduledCronFixture.setVisible(true)');
    await delay(50);
    result.duringMutation = (await state()).requests.slice(offset).map((request) => request.action);
    await run('window.scheduledCronFixture.releaseMutation()');
    await wait("document.querySelector('.scheduled-job-row')?.textContent.includes('Saved during navigation')");
    process.stdout.write(`${JSON.stringify(result)}\n`);
  } finally { window.destroy(); app.quit(); }
}
main().catch((error) => { process.stderr.write(`${error.stack ?? error}\n`); app.exit(1); });
