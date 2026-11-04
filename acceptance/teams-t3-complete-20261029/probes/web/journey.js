async (page) => {
  const fs = await import('node:fs/promises');
  const path = await import('node:path');
  const base = 'http://127.0.0.1:18784';
  const output = process.env.TEAMS_T3_OUTPUT_DIR || '/private/tmp/teams-t3-web-acceptance/output/playwright';
  await fs.mkdir(output, { recursive: true });
  const checks = {}, errors = [], commands = [], observations = [];
  const assert = (condition, message) => { if (!condition) throw new Error(message); };
  const TEAM = 'Teams T3 live acceptance', THREAD = 'Web live Bot question', LIVE_BOT = 'T3 acceptance Bot', OFFLINE_BOT = 'T3 offline acceptance Bot';
  const exactly = text => new RegExp(`^${text.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')}$`);
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
      if ((command.action === 'botState' || command.action === 'botStates') && response.ok()) {
        // The workspace asks for the runs of the open conversation in one `botStates`; `botState` is the single-run check before an answer.
        const body = await response.json();
        for (const state of command.action === 'botStates' ? Object.values(body.states ?? {}) : [body]) {
          if (state) observations.push({ conversationId: command.conversationId, state: state.state, questionId: state.questionId, answered: state.answered });
        }
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

  // ---- The workspace: sidebar (team switcher, conversations by kind), open conversation, details drawer ----
  const sidebarOf = current => current.getByRole('navigation', { name: 'Teams', exact: true });
  const sectionOf = (current, heading) => sidebarOf(current).locator('section.tw-section').filter({ has: current.getByRole('heading', { level: 3, name: heading, exact: true }) });
  const titled = (current, title) => current.locator('li.tw-row').filter({ has: current.locator('.tw-row-title', { hasText: exactly(title) }) });
  const conversationOf = (current, title) => current.getByRole('region', { name: title, exact: true });
  const drawerOf = current => current.getByRole('complementary', { name: 'Team details', exact: true });
  // Open the probe as one synthetic account and pick the acceptance team in the switcher (a menu of radio items).
  const openWorkspace = async (current, uid) => {
    await current.goto(`${base}/?uid=${uid}`);
    await current.getByTestId('probe-ready').waitFor({ timeout: 30000 });
    assert(await current.getByTestId('probe-ready').getAttribute('data-uid') === uid, 'Wrong synthetic account');
    const switcher = sidebarOf(current).locator('button.tw-switcher');
    await switcher.waitFor({ timeout: 30000 });
    await switcher.click();
    await current.getByRole('menu', { name: 'Teams', exact: true }).getByRole('menuitemradio', { name: TEAM }).click({ timeout: 30000 });
    await sidebarOf(current).locator('.tw-switcher-name').filter({ hasText: exactly(TEAM) }).waitFor({ timeout: 30000 });
  };
  const chooseThread = async (current, title) => {
    const row = titled(current, title).locator('button.tw-row-main');
    await row.click({ timeout: 30000 });
    await conversationOf(current, title).getByRole('heading', { level: 2, name: title, exact: true }).waitFor();
    assert(await row.getAttribute('aria-current') === 'page', `${title} is not the open conversation`);
  };
  // The drawer opens from the team menu (works with no conversation open) or from the conversation header's button.
  const openDrawer = async (current, tab, { fromMenu = false } = {}) => {
    const drawer = drawerOf(current);
    if (!await drawer.count()) {
      if (fromMenu) {
        await sidebarOf(current).locator('button.tw-switcher').click();
        await current.getByRole('menu', { name: 'Teams', exact: true }).getByRole('menuitem', { name: 'Team members & settings', exact: true }).click();
      } else await current.getByRole('button', { name: 'Team details', exact: true }).click();
    }
    await drawer.waitFor();
    const handle = drawer.getByRole('tab', { name: tab, exact: true });
    await handle.click();
    assert(await handle.getAttribute('aria-selected') === 'true', `The ${tab} tab did not open`);
    return drawer;
  };
  const closeDrawer = async () => {
    await drawerOf(page).getByRole('button', { name: 'Close details', exact: true }).click();
    await drawerOf(page).waitFor({ state: 'detached' });
  };
  const delivered = text => page.locator('.tw-messages > li.tw-msg[data-message-id] .tw-msg-body', { hasText: exactly(text) });
  const send = async text => {
    const composer = page.locator('form.tw-composer');
    await composer.getByLabel('Message', { exact: true }).fill(text);
    const [response] = await Promise.all([messageResponse('mutate', command => command.text === text), composer.getByRole('button', { name: 'Send', exact: true }).click()]);
    assert(response.ok(), `Message write rejected: ${response.status()}`);
    await delivered(text).waitFor();
    return response.request().postDataJSON();
  };
  const answerCurrent = async () => {
    const question = page.locator('.team-bot-question').last();
    await question.getByLabel('Which acceptance result should I use?', { exact: true }).fill('Current', { timeout: 60000 });
    await question.getByRole('button', { name: 'Send answer', exact: true }).click();
    await question.getByText('Answer received.', { exact: true }).waitFor({ timeout: 30000 });
  };
  // "Message a Bot" in the sidebar: a dialog listing the Bots the account may use, each with Start chat.
  const startBotChat = async name => {
    await sidebarOf(page).getByRole('button', { name: 'Message a Bot', exact: true }).click();
    const dialog = page.getByRole('dialog', { name: 'Message a Bot', exact: true });
    const entry = dialog.getByRole('list', { name: 'Bots you can message', exact: true }).getByRole('listitem').filter({ has: page.getByText(name, { exact: true }) });
    const [response] = await Promise.all([messageResponse('create', command => command.kind === 'bot_dm'), entry.getByRole('button', { name: 'Start chat', exact: true }).click()]);
    assert(response.ok(), `Bot DM creation rejected: ${response.status()}`);
    await dialog.waitFor({ state: 'detached' });
    await conversationOf(page, name).getByRole('heading', { level: 2, name, exact: true }).waitFor();
    return response.request().postDataJSON().conversationId;
  };
  try {
    await page.setViewportSize({ width: 1440, height: 1080 });
    await openWorkspace(observer, 'web-observer');
    await chooseThread(observer, THREAD);
    await observer.locator('.team-bot-question').first().waitFor({ timeout: 60000 });
    assert(await observer.locator('.team-bot-question textarea').count() === 0, 'Observer received answer controls');
    await observer.getByText('Only the original requester in the same membership can answer.', { exact: true }).waitFor();
    await screenshot(observer, '01-observer-question-readonly.png'); checks.originalHumanOnly = true;

    await openWorkspace(page, 'web-human');
    for (const heading of ['Channels', 'Direct messages', 'Bot DMs']) await sectionOf(page, heading).waitFor();
    // The team's details: its members, the Invites tab only for those who may create invitations, and the Bots with their owners.
    let drawer = await openDrawer(page, 'Members', { fromMenu: true });
    const people = drawer.getByRole('list', { name: 'Members', exact: true });
    await people.waitFor({ timeout: 30000 });
    assert(await people.getByRole('listitem').count() >= 2, 'The members tab must list both Web accounts');
    assert(await people.locator('small', { hasText: 'You' }).count() === 1, 'The members tab must mark the signed-in account');
    const teamRole = (await drawer.locator('.tw-drawer-title p').innerText()).split(' · ')[0];
    const invitesTab = drawer.getByRole('tab', { name: 'Invites', exact: true });
    assert(await invitesTab.count() === (teamRole === 'Member' ? 0 : 1), `The Invites tab must be offered to owners and admins only (role: ${teamRole})`);
    if (teamRole !== 'Member') {
      await invitesTab.click();
      await drawer.getByRole('button', { name: 'Create invite link', exact: true }).waitFor();
    }
    drawer = await openDrawer(page, 'Bots');
    const cards = drawer.locator('.team-bot-grid');
    await cards.getByText(LIVE_BOT, { exact: true }).waitFor();
    await drawer.getByText('Open Teams in LingXi Desktop to share a local Bot or change its permissions.', { exact: true }).waitFor();
    assert(await cards.getByText('Owner', { exact: false }).count() > 0, 'Bot owner disclosure missing');
    await screenshot(page, '02-bot-cards-and-privacy.png'); checks.cardsAndWebBindingGuidance = true;
    await closeDrawer();
    await chooseThread(page, THREAD);
    await answerCurrent(); await screenshot(page, '03-original-human-answer-acknowledged.png'); checks.realQuestionAnswer = true;

    const stamp = Date.now();
    const dmId = await startBotChat(LIVE_BOT);
    assert(await titled(page, LIVE_BOT).getAttribute('data-kind') === 'bot_dm' && await sectionOf(page, 'Bot DMs').locator('li.tw-row').filter({ has: page.locator('.tw-row-title', { hasText: exactly(LIVE_BOT) }) }).count() === 1,
      'The Bot DM must be listed once, under Bot DMs');
    await send(`Web Bot DM acceptance ${stamp}`);
    let state = await fixtureState();
    assert(state.conversations.some(value => value.id === dmId && value.triggerCount === 1), 'Bot DM did not create exactly one real trigger');
    await answerCurrent(); await screenshot(page, '04-bot-dm.png'); checks.permittedBotDM = true;

    await sidebarOf(page).getByRole('button', { name: 'New message', exact: true }).click();
    const dialog = page.getByRole('dialog', { name: 'New message', exact: true });
    const groupTitle = `Web explicit targets ${stamp}`;
    const kinds = dialog.getByRole('radiogroup', { name: 'Type', exact: true });
    assert(await kinds.getByRole('radio').count() === 2, 'New message must offer exactly a direct message and a group');
    await kinds.locator('label').filter({ hasText: 'Group' }).click();
    assert(await kinds.getByRole('radio', { name: 'Group', exact: true }).isChecked(), 'Conversation type did not change to group');
    await dialog.getByLabel('Group name (optional)', { exact: true }).fill(groupTitle);
    await dialog.getByRole('group', { name: 'Participating Bots', exact: true }).getByRole('checkbox', { name: /T3 acceptance Bot/ }).check();
    const [created] = await Promise.all([messageResponse('create', command => command.kind === 'group' && command.title === groupTitle), dialog.getByRole('button', { name: 'Create group', exact: true }).click()]);
    assert(created.ok(), `Group creation rejected: ${created.status()}`);
    const groupId = created.request().postDataJSON().conversationId;
    await dialog.waitFor({ state: 'detached' });
    await conversationOf(page, groupTitle).getByRole('heading', { level: 2, name: groupTitle, exact: true }).waitFor();
    assert(await titled(page, groupTitle).getAttribute('data-kind') === 'group' && await sectionOf(page, 'Direct messages').locator('li.tw-row').filter({ has: page.locator('.tw-row-title', { hasText: exactly(groupTitle) }) }).count() === 1,
      'The group must be listed once, under Direct messages');
    const plain = await send(`@T3 acceptance Bot display only ${stamp}`);
    assert(!plain.targets || plain.targets.length === 0, 'Plain @ text received execution targets');
    state = await fixtureState();
    assert(state.conversations.some(value => value.id === groupId && value.triggerCount === 0), 'Plain @ text enqueued a Bot trigger');
    await screenshot(page, '05-plain-mention-no-execution.png'); checks.plainMentionNeverExecutes = true;

    // The composer's @ button offers the Bots that take part in the group; only the one chosen there is asked.
    const composer = page.locator('form.tw-composer');
    await composer.getByRole('button', { name: 'Ask a Bot (@)', exact: true }).click();
    await composer.getByRole('listbox', { name: 'Bots you can ask', exact: true }).getByRole('option', { name: /T3 acceptance Bot/ }).click();
    await composer.getByRole('list', { name: 'Bots that will be asked', exact: true }).getByText(`@${LIVE_BOT}`, { exact: true }).waitFor();
    const explicit = await send(`Explicit Web Bot request ${stamp}`);
    assert(explicit.targets?.length === 1 && explicit.targets[0] === state.botKey, 'Explicit target command did not preserve the registered Bot key');
    state = await fixtureState();
    assert(state.conversations.some(value => value.id === groupId && value.triggerCount === 1), 'Explicit target did not create exactly one real trigger');
    await answerCurrent(); await screenshot(page, '06-explicit-group-target-and-answer.png'); checks.explicitGroupTarget = true;

    // This second card belongs to a deliberately unstarted synthetic host. It
    // proves actual backend queue state without racing or mocking the live worker.
    drawer = await openDrawer(page, 'Bots');
    const offlineCard = drawer.locator('.team-bot-grid > li').filter({ has: page.getByText(OFFLINE_BOT, { exact: true }) });
    await offlineCard.waitFor({ timeout: 30000 });
    const [queuedDm] = await Promise.all([messageResponse('create', command => command.kind === 'bot_dm'), offlineCard.getByRole('button', { name: 'Message Bot', exact: true }).click()]);
    assert(queuedDm.ok(), `Bot DM creation rejected: ${queuedDm.status()}`);
    const queuedId = queuedDm.request().postDataJSON().conversationId;
    await conversationOf(page, OFFLINE_BOT).getByRole('heading', { level: 2, name: OFFLINE_BOT, exact: true }).waitFor();
    await send(`Queued on offline host ${stamp}`);
    await page.locator('.team-bot-run').getByText('Bot · Queued', { exact: true }).waitFor({ timeout: 30000 });
    await screenshot(page, '07-real-offline-queued-status.png');
    assert(observations.some(value => value.conversationId === queuedId && value.state === 'queued'), 'No real queued botState observation');
    checks.realQueuedStatus = true;
    assert(!commands.some(value => ['claimBot', 'startBot', 'finishBot', 'botPoll', 'botWait', 'botQuestion'].includes(value.action)), 'Member UI called a host command');
    checks.noExecutorCapabilities = true;
    assert(errors.length === 0, `Browser runtime errors: ${errors.join('; ')}`);
    const result = { passed: true, checks, commands, observations, runtimeErrors: errors, scope: 'Shared Teams workspace (TeamSidebar, TeamConversationView, Composer, TeamDetailsDrawer, BotDirectory, BotRunCards) in the Web TeamsWorkspaceView + product Web adapter + real local Auth/Firestore/HTTP fixture; Desktop owner tool approval and production deployment excluded.' };
    await fs.writeFile(path.join(output, 'result.json'), JSON.stringify(result, null, 2)); return { ...result, output };
  } catch (error) {
    await screenshot(page, 'failure.png').catch(() => {});
    await fs.writeFile(path.join(output, 'result.json'), JSON.stringify({ passed: false, checks, commands, observations, runtimeErrors: errors, error: String(error) }, null, 2));
    throw error;
  } finally { await observerContext.close(); }
}
