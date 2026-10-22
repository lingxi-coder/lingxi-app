import assert from 'node:assert/strict';
import { test } from 'node:test';
import { ManualClock, randomId } from '@lingxi/bridge-client/remote';

import {
  ApprovalDialog,
  formatPairingCode,
  type ApprovalMessageBox,
  type ApprovalOutcome,
  type ApprovalRequest,
} from '../src/main/remoteControl/approvalDialog.js';
import { START, settle } from './remote-helpers.js';

/** A message box the test answers by hand. */
class FakeShow {
  readonly boxes: ApprovalMessageBox[] = [];
  private readonly open: Array<{ resolve(result: { response: number }): void; reject(error: Error): void }> = [];

  readonly show = (options: ApprovalMessageBox): Promise<{ response: number }> => {
    this.boxes.push(options);
    return new Promise((resolve, reject) => { this.open.push({ resolve, reject }); });
  };

  /** Answer the dialog shown `index`-th (in order of showing). */
  answer(response: number, index = this.boxes.length - 1): void {
    this.open[index]!.resolve({ response });
  }

  fail(error: Error, index = this.boxes.length - 1): void {
    this.open[index]!.reject(error);
  }
}

function setup() {
  const clock = new ManualClock(START);
  const fake = new FakeShow();
  const dialog = new ApprovalDialog({ show: fake.show, clock });
  return { clock, fake, dialog };
}

function request(overrides: Partial<ApprovalRequest> = {}): { request: ApprovalRequest; abort(): void } {
  const controller = new AbortController();
  return {
    request: {
      deviceId: randomId(),
      deviceName: 'Lingfeng iPhone',
      platform: 'ios',
      pairingCode: '123456',
      signal: controller.signal,
      ...overrides,
    },
    abort: () => controller.abort(),
  };
}

const denied = (reason: string): ApprovalOutcome => ({ allowed: false, reason } as ApprovalOutcome);

test('the dialog names the device, shows the pairing code and defaults to Deny', async () => {
  const { fake, dialog } = setup();
  const pending = dialog.request(request({ deviceName: 'Lingfeng iPhone', platform: 'android', pairingCode: '042713' }).request);
  assert.equal(fake.boxes.length, 1);
  const box = fake.boxes[0]!;
  assert.equal(box.type, 'question');
  assert.ok(box.message.includes('Lingfeng iPhone'));
  assert.ok(box.detail.includes('Lingfeng iPhone'));
  assert.ok(box.detail.includes('Android'));
  assert.ok(box.detail.includes('配对码：042 713'));
  assert.deepEqual(box.buttons, ['允许', '拒绝']);
  assert.equal(box.defaultId, 1, 'Return must not allow');
  assert.equal(box.cancelId, 1, 'Escape and closing the box must not allow');
  assert.equal(box.noLink, true);
  assert.ok(box.signal instanceof AbortSignal);
  fake.answer(0);
  assert.deepEqual(await pending, { allowed: true });
});

test('the pairing code is shown in groups of three, as the viewer shows it', async () => {
  assert.equal(formatPairingCode('123456'), '123 456');
  assert.equal(formatPairingCode('12345'), '12345');
  const { fake, dialog } = setup();
  const pending = dialog.request(request({ pairingCode: '918273' }).request);
  assert.ok(fake.boxes[0]!.detail.includes('配对码：918 273'));
  fake.answer(0);
  await pending;
});

test('each platform has its own label', async () => {
  for (const [platform, label] of [['web', '网页浏览器'], ['android', 'Android'], ['ios', 'iOS']] as const) {
    const { fake, dialog } = setup();
    const pending = dialog.request(request({ platform }).request);
    assert.ok(fake.boxes[0]!.detail.includes(label), platform);
    fake.answer(0);
    await pending;
  }
});

test('a long device name is shortened by characters, not cut inside one', async () => {
  const { fake, dialog } = setup();
  const name = '😀'.repeat(100);
  const pending = dialog.request(request({ deviceName: name }).request);
  const shown = fake.boxes[0]!.message;
  assert.ok(shown.includes(`${'😀'.repeat(64)}…`));
  assert.ok(!shown.includes('😀'.repeat(65)));
  fake.answer(0);
  await pending;
});

test('anything but the Allow button denies, and a denied device is not asked again for ten minutes', async () => {
  const { clock, fake, dialog } = setup();
  const phone = request();
  const first = dialog.request(phone.request);
  fake.answer(1);
  assert.deepEqual(await first, denied('denied'));
  await settle();

  const again = await dialog.request(request({ deviceId: phone.request.deviceId }).request);
  assert.deepEqual(again, denied('denied'));
  assert.equal(fake.boxes.length, 1, 'no second dialog');

  const other = dialog.request(request().request);
  assert.equal(fake.boxes.length, 2, 'other devices are still asked');
  fake.answer(-1);
  assert.deepEqual(await other, denied('denied'));
  await settle();

  clock.advance(10 * 60_000);
  const later = dialog.request(request({ deviceId: phone.request.deviceId }).request);
  assert.equal(fake.boxes.length, 3);
  fake.answer(0);
  assert.deepEqual(await later, { allowed: true });
});

test('one dialog at a time: two more requests wait their turn, the next is told busy', async () => {
  const { fake, dialog } = setup();
  const one = dialog.request(request().request);
  const two = dialog.request(request().request);
  const three = dialog.request(request().request);
  assert.equal(fake.boxes.length, 1);
  assert.deepEqual(await dialog.request(request().request), denied('busy'));

  fake.answer(0, 0);
  assert.deepEqual(await one, { allowed: true });
  await settle();
  assert.equal(fake.boxes.length, 2, 'the next one is shown once the first is answered');
  fake.answer(1, 1);
  assert.deepEqual(await two, denied('denied'));
  await settle();
  assert.equal(fake.boxes.length, 3);
  fake.answer(0, 2);
  assert.deepEqual(await three, { allowed: true });
});

test('a request whose connection ends while it waits in line is dropped without a dialog', async () => {
  const { fake, dialog } = setup();
  const first = dialog.request(request().request);
  const waiting = request();
  const second = dialog.request(waiting.request);
  waiting.abort();
  assert.deepEqual(await second, denied('cancelled'));
  fake.answer(0, 0);
  await first;
  await settle();
  assert.equal(fake.boxes.length, 1, 'the dropped request never reached the screen');
});

test('a dialog whose connection ends is closed, and an answer that raced the closing is not believed', async () => {
  const { fake, dialog } = setup();
  const connection = request();
  const pending = dialog.request(connection.request);
  const { signal } = fake.boxes[0]!;
  assert.equal(signal.aborted, false);
  connection.abort();
  assert.equal(signal.aborted, true, 'the box is told to close');
  fake.answer(0);
  assert.deepEqual(await pending, denied('cancelled'));
});

test('a request that is already aborted is not shown at all', async () => {
  const { fake, dialog } = setup();
  const gone = request();
  gone.abort();
  assert.deepEqual(await dialog.request(gone.request), denied('cancelled'));
  assert.equal(fake.boxes.length, 0);
});

test('an unanswered dialog closes itself after two minutes', async () => {
  const { clock, fake, dialog } = setup();
  const phone = request();
  const pending = dialog.request(phone.request);
  clock.advance(2 * 60_000 - 1);
  assert.equal(fake.boxes[0]!.signal.aborted, false);
  clock.advance(1);
  assert.equal(fake.boxes[0]!.signal.aborted, true);
  fake.answer(1);
  assert.deepEqual(await pending, denied('timeout'));
  await settle();

  // The timeout is not the user saying no: the same device may try again at once.
  const retry = dialog.request(request({ deviceId: phone.request.deviceId }).request);
  assert.equal(fake.boxes.length, 2);
  fake.answer(0);
  assert.deepEqual(await retry, { allowed: true });
});

test('a box that fails to show, or rejects when closed, ends the request', async () => {
  const { clock, fake, dialog } = setup();
  const broken = dialog.request(request().request);
  fake.fail(new Error('no window server'));
  assert.deepEqual(await broken, denied('cancelled'));
  await settle();

  const slow = dialog.request(request().request);
  clock.advance(2 * 60_000);
  fake.fail(new Error('aborted'));
  assert.deepEqual(await slow, denied('timeout'));
});

test('at most five dialogs per ten minutes', async () => {
  const { clock, fake, dialog } = setup();
  for (let i = 0; i < 5; i++) {
    const pending = dialog.request(request().request);
    fake.answer(0);
    assert.deepEqual(await pending, { allowed: true });
    await settle();
    clock.advance(60_000);
  }
  assert.equal(fake.boxes.length, 5);
  assert.deepEqual(await dialog.request(request().request), denied('rate_limited'));
  assert.equal(fake.boxes.length, 5, 'no sixth dialog');
  await settle();

  // The first dialog was shown at START; ten minutes after it the window has room again.
  clock.advance(10 * 60_000 - 5 * 60_000);
  const next = dialog.request(request().request);
  assert.equal(fake.boxes.length, 6);
  fake.answer(0);
  assert.deepEqual(await next, { allowed: true });
});

test('a device approved while its request waited is not asked about, and costs no dialog', async () => {
  const { fake, dialog } = setup();
  const first = dialog.request(request().request);
  let approved = false;
  const second = dialog.request(request({ alreadyApproved: () => approved }).request);
  approved = true;
  fake.answer(0, 0);
  await first;
  assert.deepEqual(await second, { allowed: true });
  assert.equal(fake.boxes.length, 1);
});

test('dispose closes the open dialog, turns the line away and refuses later requests', async () => {
  const { fake, dialog } = setup();
  const shown = dialog.request(request().request);
  const waiting = dialog.request(request().request);
  dialog.dispose();
  assert.equal(fake.boxes[0]!.signal.aborted, true);
  assert.deepEqual(await waiting, denied('cancelled'));
  fake.answer(0);
  assert.deepEqual(await shown, denied('cancelled'));
  assert.deepEqual(await dialog.request(request().request), denied('cancelled'));
  assert.equal(fake.boxes.length, 1);
});
