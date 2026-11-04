async (page) => {
  const fs = await import('node:fs/promises');
  const path = await import('node:path');
  const base = 'http://127.0.0.1:18784';
  const output = process.env.TEAMS_T3_OUTPUT_DIR || '/private/tmp/teams-t3-web-acceptance/output/playwright';
  await fs.mkdir(output, { recursive: true });
  const checks = {}, errors = [], commands = [], observations = [];
  const assert = (condition, message) => { if (!condition) throw new Error(message); };
  const browser = page.context().browser();
  assert(browser, 'A connected browser is required');
  const observerContext = await browser.newContext({ viewport: { width: 1440, height: 1080 } });
  const observer = await observerContext.newPage();
  for (const current of [page, observer]) current.on('pageerror', error => errors.push(String(error)));
  page.on('response', async response => {
    if (!response.url().endsWith('/v1/teams/messages')) return;
    try {
      const command = response.request().postDataJSON();
      // Persist only public command metadata. Never tokens, answers, tool details or traces.
      commands.push({ action: command.action, conversationId: command.conversationId, kind: command.kind,
        targets: command.targets, questionId: command.questionId, status: response.status() });
      if (command.action === 'botState' && response.ok()) {
        const state = await response.json(); observations.push({ conversationId: command.conversationId, state: state.state, questionId: state.questionId, answered: state.answered });
      }
    } catch { /* HTTP failure is checked by the operation that issued it. */ }
  });
  const screenshot = (current, name) => current.screenshot({ path: path.join(output, name), fullPage: true });
  const fixtureState = async () => {
    const response = await page.request.get(`${base}/_acceptance/state?uid=web-human`);
    assert(response.ok(), `Fixture state failed: ${response.status()}`); return response.json();
  };
  const messageResponse = (action, predicate = () => true) => page.waitForResponse(response => {
    if (!response.url().endsWith('/v1/teams/messages')) return false;
    try { const command = response.request().postDataJSON(); return command.action === action && predicate(command); } catch { return false; }
  }, { timeout: 30000 });
  const openMessages = async (current, uid) => {
    await current.goto(`${base}/?uid=${uid}`);
    await current.getByTestId('probe-ready').waitFor({ timeout: 30000 });
    assert(await current.getByTestId('probe-ready').getAttribute('data-uid') === uid, 'Wrong synthetic account');
    await current.locator('.teams-list > button.teams-card').filter({ hasText: 'Teams T3 live acceptance' }).click();
    await current.getByRole('button', { name: 'Conversations', exact: true }).click();
    await current.locator('.team-bots').waitFor();
  };
  const chooseThread = async (current, title) => current.locator('button.team-thread').filter({ hasText: title }).click();
  const send = async text => {
    const composer = page.locator('.team-composer');
    await composer.getByLabel('Message', { exact: true }).fill(text);
    const [response] = await Promise.all([messageResponse('mutate', command => command.text === text), composer.getByRole('button', { name: 'Send', exact: true }).click()]);
    assert(response.ok(), `Message write rejected: ${response.status()}`);
    await page.locator('.team-messages').getByText(text, { exact: true }).waitFor();
    return response.request().postDataJSON();
  };
  const answerCurrent = async () => {
    const question = page.locator('.team-bot-question').last();
    await question.getByLabel('Which acceptance result should I use?', { exact: true }).fill('Current', { timeout: 60000 });
    await question.getByRole('button', { name: 'Send answer', exact: true }).click();
    await question.getByText('Answer received.', { exact: true }).waitFor({ timeout: 30000 });
  };
  const openBotDM = async name => {
    const card = page.locator('.team-bot-grid > li').filter({ has: page.getByText(name, { exact: true }) });
    const [response] = await Promise.all([messageResponse('create', command => command.kind === 'bot_dm'), card.getByRole('button', { name: 'Message Bot', exact: true }).click()]);
    assert(response.ok(), `Bot DM creation rejected: ${response.status()}`);
    await page.getByRole('heading', { level: 3, name, exact: true }).waitFor();
    return response.request().postDataJSON().conversationId;
  };
  try {
    await page.setViewportSize({ width: 1440, height: 1080 });
    await openMessages(observer, 'web-observer');
    await chooseThread(observer, 'Web live Bot question');
    await observer.locator('.team-bot-question').first().waitFor({ timeout: 60000 });
    assert(await observer.locator('.team-bot-question textarea').count() === 0, 'Observer received answer controls');
    await observer.getByText('Only the original requester in the same membership can answer.', { exact: true }).waitFor();
    await screenshot(observer, '01-observer-question-readonly.png'); checks.originalHumanOnly = true;

    await openMessages(page, 'web-human');
    const cards = page.locator('.team-bot-grid');
    await cards.getByText('T3 acceptance Bot', { exact: true }).waitFor();
    await page.getByText('Open Teams in LingXi Desktop to share a local Bot or change its permissions.', { exact: true }).waitFor();
    assert(await cards.getByText('Owner', { exact: false }).count() > 0, 'Bot owner disclosure missing');
    await screenshot(page, '02-bot-cards-and-privacy.png'); checks.cardsAndWebBindingGuidance = true;
    await chooseThread(page, 'Web live Bot question');
    await answerCurrent(); await screenshot(page, '03-original-human-answer-acknowledged.png'); checks.realQuestionAnswer = true;

    const stamp = Date.now();
    const dmId = await openBotDM('T3 acceptance Bot');
    await send(`Web Bot DM acceptance ${stamp}`);
    let state = await fixtureState();
    assert(state.conversations.some(value => value.id === dmId && value.triggerCount === 1), 'Bot DM did not create exactly one real trigger');
    await answerCurrent(); await screenshot(page, '04-bot-dm.png'); checks.permittedBotDM = true;

    const form = page.locator('.team-message-layout > aside details form');
    await page.locator('.team-message-layout > aside details summary').click();
    const groupTitle = `Web explicit targets ${stamp}`;
    await form.getByLabel('Title', { exact: true }).fill(groupTitle);
    const conversationType = form.locator('select');
    assert(await conversationType.count() === 1, 'New conversation form must have one type selector');
    await conversationType.selectOption('group');
    assert(await conversationType.inputValue() === 'group', 'Conversation type did not change to group');
    await form.getByRole('group', { name: 'Participating Bots', exact: true }).getByRole('checkbox', { name: /T3 acceptance Bot/ }).check();
    const [created] = await Promise.all([messageResponse('create', command => command.kind === 'group' && command.title === groupTitle), form.getByRole('button', { name: 'Create conversation', exact: true }).click()]);
    assert(created.ok(), `Group creation rejected: ${created.status()}`);
    const groupId = created.request().postDataJSON().conversationId;
    await page.getByRole('heading', { level: 3, name: groupTitle, exact: true }).waitFor();
    const plain = await send(`@T3 acceptance Bot display only ${stamp}`);
    assert(!plain.targets || plain.targets.length === 0, 'Plain @ text received execution targets');
    state = await fixtureState();
    assert(state.conversations.some(value => value.id === groupId && value.triggerCount === 0), 'Plain @ text enqueued a Bot trigger');
    await screenshot(page, '05-plain-mention-no-execution.png'); checks.plainMentionNeverExecutes = true;

    await page.locator('.team-composer').getByRole('group', { name: 'Ask Bots explicitly', exact: true }).getByRole('checkbox', { name: /T3 acceptance Bot/ }).check();
    const explicit = await send(`Explicit Web Bot request ${stamp}`);
    assert(explicit.targets?.length === 1 && explicit.targets[0] === state.botKey, 'Explicit target command did not preserve the registered Bot key');
    state = await fixtureState();
    assert(state.conversations.some(value => value.id === groupId && value.triggerCount === 1), 'Explicit target did not create exactly one real trigger');
    await answerCurrent(); await screenshot(page, '06-explicit-group-target-and-answer.png'); checks.explicitGroupTarget = true;

    // This second card belongs to a deliberately unstarted synthetic host. It
    // proves actual backend queue state without racing or mocking the live worker.
    const offlineCard = page.locator('.team-bot-grid > li').filter({ hasText: 'T3 offline acceptance Bot' });
    await offlineCard.waitFor({ timeout: 30000 });
    const queuedId = await openBotDM('T3 offline acceptance Bot');
    await send(`Queued on offline host ${stamp}`);
    await page.locator('.team-bot-run').getByText('Bot · Queued', { exact: true }).waitFor({ timeout: 30000 });
    await screenshot(page, '07-real-offline-queued-status.png');
    assert(observations.some(value => value.conversationId === queuedId && value.state === 'queued'), 'No real queued botState observation');
    checks.realQueuedStatus = true;
    assert(!commands.some(value => ['claimBot', 'startBot', 'finishBot', 'botPoll', 'botWait', 'botQuestion'].includes(value.action)), 'Member UI called a host command');
    checks.noExecutorCapabilities = true;
    assert(errors.length === 0, `Browser runtime errors: ${errors.join('; ')}`);
    const result = { passed: true, checks, commands, observations, runtimeErrors: errors, scope: 'Shared TeamsPanel/MessagesPanel + product Web adapter + real local Auth/Firestore/HTTP fixture; Desktop owner tool approval and production deployment excluded.' };
    await fs.writeFile(path.join(output, 'result.json'), JSON.stringify(result, null, 2)); return { ...result, output };
  } catch (error) {
    await screenshot(page, 'failure.png').catch(() => {});
    await fs.writeFile(path.join(output, 'result.json'), JSON.stringify({ passed: false, checks, commands, observations, runtimeErrors: errors, error: String(error) }, null, 2));
    throw error;
  } finally { await observerContext.close(); }
}
