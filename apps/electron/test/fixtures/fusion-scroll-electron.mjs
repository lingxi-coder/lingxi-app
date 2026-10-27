import { app, BrowserWindow } from 'electron';
import { writeFileSync } from 'node:fs';
const url = process.argv.find((argument) => argument.startsWith('http://'));
if (!url || !process.env.LINGXI_TEST_USER_DATA) throw new Error('fixture URL and isolated user data required');
app.setPath('userData', process.env.LINGXI_TEST_USER_DATA);
const delay = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
async function main() {
  await app.whenReady();
  const window = new BrowserWindow({ show: false, width: 1100, height: 800, webPreferences: { sandbox: true } });
  try {
    await window.loadURL(url);
    const run = (script) => window.webContents.executeJavaScript(script);
    const wait = async (expression) => {
      const deadline = Date.now() + 6000;
      while (Date.now() < deadline) { if (await run(expression)) return; await delay(20); }
      throw new Error(`Timed out: ${expression}\n${await run('document.body.innerText')}`);
    };
    const trigger = `document.querySelector('button[aria-label="选择模型"]')`;
    const key = async (keyCode) => {
      window.webContents.sendInputEvent({ type: 'keyDown', keyCode });
      window.webContents.sendInputEvent({ type: 'keyUp', keyCode });
      await delay(40);
    };
    await wait(`Boolean([...document.querySelectorAll('button')].find(button => button.textContent.includes('新建策略')))`);
    const excludedLabel = await run(`[...document.querySelectorAll('.extension-hub-row')].find(row => row.textContent.includes('排除的策略'))?.textContent.includes('当前会话未启用此作用域')`);
    await run(`[...document.querySelectorAll('.extension-hub-row-open')].find(button => button.textContent.includes('排除的策略')).click()`); await delay(60);
    const excludedEditable = await run(`document.querySelector('[aria-label="策略名称"]').value === '排除的策略' && !document.querySelector('[aria-label="策略名称"]').disabled`);
    await run(`[...document.querySelectorAll('button')].find(button => button.textContent.trim() === '取消').click()`); await delay(60);
    await run(`[...document.querySelectorAll('button')].find(button => button.textContent.includes('新建策略')).click()`);
    await wait(`Boolean(${trigger})`);
    await run(`${trigger}.scrollIntoView({block: 'center'})`);
    await delay(100);
    await run(`${trigger}.click()`);
    await wait('document.querySelectorAll("[role=option]").length === 73');
    const result = await run(`(() => { const list = document.querySelector('[role=listbox]'); return {
      options: list.querySelectorAll('[role=option]').length,
      height: list.closest('[role=dialog]').getBoundingClientRect().height,
      overflow: getComputedStyle(list).overflowY, scrollable: list.scrollHeight > list.clientHeight,
    }; })()`);
    result.excludedLabel = excludedLabel; result.excludedEditable = excludedEditable;
    await delay(100);
    writeFileSync('/tmp/fusion-scroll.png', (await window.webContents.capturePage()).toPNG());
    await run(`document.querySelector('[role=listbox]').scrollTop = 99999`);
    await delay(100);
    await run(`document.querySelector('[role=option]:last-child').click()`);
    await wait('!document.querySelector("[role=listbox]")');
    result.clicked = await run(`${trigger}.textContent`);
    await run(`${trigger}.click()`);
    await wait('Boolean(document.querySelector("[role=listbox]"))');
    const search = async (text) => {
      await run(`(() => { const input = document.querySelector('input[aria-label="搜索模型"]'); Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value').set.call(input, ${JSON.stringify(text)}); input.dispatchEvent(new Event('input', {bubbles:true})); })()`);
      await delay(70);
    };
    await search('Model 71');
    result.searchName = await run(`document.querySelector('[role=listbox]').textContent.includes('Model 71') && !document.querySelector('[role=listbox]').textContent.includes('Model 70')`);
    await search('model-69');
    result.searchId = await run(`document.querySelector('[role=listbox]').textContent.includes('Model 69') && !document.querySelector('[role=listbox]').textContent.includes('Model 70')`);
    await search('Configured provider');
    result.searchProvider = await run(`document.querySelector('[role=listbox]').textContent.includes('Model 00') && document.querySelector('[role=listbox]').textContent.includes('Model 71')`);
    await search('does-not-exist');
    result.empty = await run(`!document.querySelector('[role=listbox]').textContent.includes('Model 00')`);
    await search('');
    await key('Down');
    await key('End');
    await key('Enter');
    await wait('!document.querySelector("[role=listbox]")');
    result.keyboard = await run(`${trigger}.textContent`);
    await run(`${trigger}.click()`);
    await wait('Boolean(document.querySelector("[role=listbox]"))');
    await key('Escape');
    result.escapeClosed = await run('!document.querySelector("[role=listbox]") && Boolean(document.querySelector("[data-nav-page=fusion]"))');
    result.focusReturned = await run(`document.activeElement === ${trigger}`);
    const analyst = `document.querySelector('button[aria-label="选择 analyst…"]')`;
    await run(`${analyst}.scrollIntoView({block:'center', behavior:'instant'})`);
    const analystBottom = await run(`${analyst}.getBoundingClientRect().bottom`);
    window.setContentSize(1100, Math.ceil(analystBottom + 40));
    await run(`${analyst}.scrollIntoView({block:'end', behavior:'instant'})`);
    await delay(100);
    await run(`document.querySelector('button[aria-label="选择 analyst…"]').click()`);
    await wait('Boolean(document.querySelector("[role=listbox]"))');
    result.upwardPlacement = await run(`(() => {
      const trigger = ${analyst}.getBoundingClientRect();
      const menu = document.querySelector('[role=listbox]').closest('[role=dialog]').getBoundingClientRect();
      return { spaceBelow: innerHeight - trigger.bottom, triggerTop: trigger.top,
        menuTop: menu.top, menuBottom: menu.bottom, menuHeight: menu.height };
    })()`);
    await key('Escape');
    window.setContentSize(1100, 800);
    const clickText = async (text) => {
      const expression = `[...document.querySelectorAll('button')].find(button => button.textContent.trim() === ${JSON.stringify(text)})`;
      await wait(`Boolean(${expression})`); await run(`${expression}.click()`); await delay(60);
    };
    const input = async (selector, text) => {
      await run(`(() => { const input = document.querySelector(${JSON.stringify(selector)}); Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value').set.call(input, ${JSON.stringify(text)}); input.dispatchEvent(new Event('input', {bubbles:true})); })()`); await delay(60);
    };
    const select = async (selector, value) => {
      await run(`(() => { const element = document.querySelector(${JSON.stringify(selector)}); element.value = ${JSON.stringify(value)}; element.dispatchEvent(new Event('change', { bubbles: true })); })()`); await delay(60);
    };
    process.stderr.write('fixture: CRUD\n');
    // A complete named strategy: preserve configured-provider model selection.
    await clickText('添加');
    await run(`${trigger}.click()`); await wait('Boolean(document.querySelector("[role=listbox]"))');
    await run(`[...document.querySelectorAll('[role=option]')].find(option => option.textContent.includes('Model 70')).click()`);
    await clickText('添加');
    await run(`${analyst}.click()`); await wait('Boolean(document.querySelector("[role=listbox]"))');
    await run(`[...document.querySelectorAll('[role=option]')].find(option => option.textContent.includes('Model 69')).click()`);
    await input('[aria-label="策略名称"]', '深度审查');
    await run(`document.querySelector('[aria-label="返回应用"]').click()`); await delay(60);
    process.stderr.write('fixture: guard\n');
    result.closeGuard = await run(`window.fusionFixture.closes === 0 && document.body.innerText.includes('当前修改尚未保存')`);
    await clickText('继续编辑');
    await run(`document.querySelector('[data-nav-page="general"]').click()`); await delay(60);
    result.navigationGuard = await run(`Boolean(document.querySelector('[aria-label="策略名称"]')) && document.body.innerText.includes('当前修改尚未保存')`);
    await clickText('继续编辑');
    await clickText('保存');
    await wait(`Boolean(document.querySelector('[aria-label="Fusion 策略列表"]'))`);
    process.stderr.write('fixture: created\n');
    result.created = await run(`window.fusionFixture.catalog.scopes[0].strategies.find(strategy => strategy.name === '深度审查')`);
    const openNamed = async (name) => { await run(`[...document.querySelectorAll('.extension-hub-row-open')].find(button => button.textContent.includes(${JSON.stringify(name)})).click()`); await delay(80); };
    await openNamed('深度审查');
    result.scopeImmutable = await run(`document.querySelector('[aria-label="策略作用域"]').disabled`);
    await input('[aria-label="策略名称"]', 'Review renamed'); await clickText('保存');
    await wait(`Boolean(document.querySelector('[aria-label="Fusion 策略列表"]'))`);
    result.renamed = await run(`window.fusionFixture.catalog.scopes[0].strategies.find(strategy => strategy.name === 'Review renamed')`);
    await run(`document.querySelector('[aria-label="设为默认 Review renamed 用户"]').click()`);
    await wait(`window.fusionFixture.catalog.default_strategy !== null`);
    result.defaultSet = await run(`window.fusionFixture.catalog.default_strategy`);
    await openNamed('Review renamed');
    await select('[aria-label="复制目标作用域"]', 'project'); await clickText('复制策略');
    result.copyNewScope = await run(`document.querySelector('[aria-label="策略作用域"]').value === 'project' && !document.querySelector('[aria-label="策略作用域"]').disabled`);
    await input('[aria-label="策略名称"]', 'Review renamed'); await clickText('保存');
    await wait(`Boolean(document.querySelector('[aria-label="Fusion 策略列表"]'))`);
    process.stderr.write('fixture: copied\n');
    result.copied = await run(`window.fusionFixture.catalog.scopes[1].strategies.find(strategy => strategy.name === 'Review renamed')`);
    await input('[aria-label="搜索 Fusion 策略"]', 'Review renamed');
    result.crossScopeNames = await run(`document.querySelectorAll('.extension-hub-row-open').length`);
    await clickText('项目'); result.scopeFilter = await run(`document.querySelectorAll('.extension-hub-row-open').length === 1`);
    await input('[aria-label="搜索 Fusion 策略"]', ''); await clickText('全部');
    await run(`document.querySelector('.settings-content-scroll').scrollTop = 0`); await delay(70);
    writeFileSync('/tmp/fusion-strategies-light.png', (await window.webContents.capturePage()).toPNG());
    await run(`window.fusionFixture.setDark(true)`); await delay(70);
    window.setContentSize(760, 800); await delay(100);
    result.narrowOverflow = await run(`document.documentElement.scrollWidth > innerWidth`);
    writeFileSync('/tmp/fusion-strategies-dark-narrow.png', (await window.webContents.capturePage()).toPNG());
    process.stderr.write('fixture: narrow\n');
    result.scopeLabelsVisible = await run(`[...document.querySelectorAll('.extension-hub-row-scope')].every(element => getComputedStyle(element).display !== 'none')`);
    await run('window.fusionFixture.invalidateNamedStrategy()'); await delay(60);
    await openNamed('Review renamed');
    result.namedRepairNoReplacement = await run(`(() => {
      const diagnostic = document.querySelector('[role="alert"]')?.textContent ?? '';
      return diagnostic.includes('fusion.maxPanel must be 2..=8')
        && !document.body.innerText.includes('我确认用此新策略替换');
    })()`);
    await run(`[...document.querySelectorAll('summary')].find(summary => summary.textContent.includes('高级执行参数')).click()`);
    await input('[aria-label="模型数量上限"]', '4');
    await clickText('保存');
    await wait(`Boolean(document.querySelector('[aria-label="Fusion 策略列表"]'))`);
    result.namedRepairPreservedScope = await run(`(() => {
      const scope = window.fusionFixture.catalog.scopes[0];
      const write = window.fusionFixture.commands.filter(command => command.action === 'save').at(-1);
      return scope.strategies.length === 2 && scope.default_strategy_id === scope.strategies.find(strategy => strategy.name === 'Review renamed').id
        && scope.strategies.find(strategy => strategy.name === 'Review renamed').config.maxPanel === 4
        && !Object.hasOwn(JSON.parse(write.payload_json), 'replace_legacy');
    })()`);
    await openNamed('Review renamed');
    await run(`document.querySelector('.settings-content-scroll').scrollTop = 0`); await delay(70);
    result.narrowEditorOverflow = await run(`document.querySelector('.settings-content-scroll').scrollWidth > document.querySelector('.settings-content-scroll').clientWidth`);
    writeFileSync('/tmp/fusion-editor-dark-narrow.png', (await window.webContents.capturePage()).toPNG());
    window.setContentSize(1100, 800);
    await input('[aria-label="策略名称"]', 'Uncommitted rename');
    await run(`window.fusionFixture.concurrentEdit()`); await delay(60); await clickText('保存');
    await wait(`document.body.innerText.includes('配置版本冲突')`);
    process.stderr.write('fixture: conflict\n');
    result.conflictKeepsDraft = await run(`document.querySelector('[aria-label="策略名称"]').value === 'Uncommitted rename'`);
    await clickText('取消'); await clickText('丢弃并继续');
    await openNamed('Review renamed'); await clickText('删除策略'); await clickText('确认删除');
    await wait(`Boolean(document.querySelector('[aria-label="Fusion 策略列表"]'))`);
    result.deletedDefault = await run(`window.fusionFixture.catalog.scopes[0].default_strategy_id === null && !window.fusionFixture.catalog.scopes[0].strategies.some(strategy => strategy.name === 'Review renamed')`);
    // A changed catalog file must not strand its initiating operation when
    // that old operation is no longer exposed in the current-domain receipt.
    await openNamed('Review renamed');
    await input('[aria-label="策略名称"]', 'Pending project rename');
    await run('window.fusionFixture.delayNextWrite()');
    await clickText('保存');
    await wait(`document.querySelector('[aria-label="策略名称"]').disabled`);
    await run('window.fusionFixture.switchProjectPath()'); await delay(60);
    await run('window.fusionFixture.completeDelayedWrite()');
    await wait(`!document.querySelector('[aria-label="策略名称"]').disabled`);
    result.oldScopeWriteSettled = await run(`document.querySelector('[aria-label="策略名称"]').value === 'Pending project rename' && document.body.innerText.includes('原会话的 Fusion 配置已保存')`);
    const writesBeforeStaleSave = await run(`window.fusionFixture.commands.filter(command => command.action === 'save').length`);
    await clickText('保存');
    result.staleProjectWriteBlocked = await run(`document.body.innerText.includes('当前项目已切换') && window.fusionFixture.commands.filter(command => command.action === 'save').length === ${writesBeforeStaleSave}`);
    await clickText('取消'); await clickText('丢弃并继续');
    result.oldScopeWriteCanExit = await run(`Boolean(document.querySelector('[aria-label="Fusion 策略列表"]'))`);
    await openNamed('Review renamed');
    await input('[aria-label="策略名称"]', 'Prior session draft');
    await run('window.fusionFixture.switchSession()'); await delay(60);
    result.sessionRemountsEditor = await run(`!document.querySelector('[aria-label="策略名称"]') && Boolean(document.querySelector('[aria-label="Fusion 策略列表"]'))`);
    result.closeCount = await run('window.fusionFixture.closes');
    process.stderr.write('fixture: complete\n');
    process.stdout.write(`${JSON.stringify(result)}\n`);
  } finally { window.destroy(); app.quit(); }
}
main().catch((error) => { process.stderr.write(`${error.stack ?? error}\n`); app.exit(1); });
