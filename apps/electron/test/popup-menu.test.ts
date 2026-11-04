import assert from 'node:assert/strict';
import test from 'node:test';

import { attachPopupMenu, MENU_ITEMS } from '../src/renderer/bridge/popupMenu';

/** Just enough of a document and a menu for the keys and presses to be sent to. */
function scene(itemCount = 3) {
  const body = { name: 'body' };
  const doc: any = {
    body,
    activeElement: body as unknown,
    listeners: new Map<string, { listener: (event: any) => void; capture: unknown }>(),
    addEventListener(type: string, listener: (event: any) => void, capture?: unknown) { this.listeners.set(type, { listener, capture }); },
    removeEventListener(type: string) { this.listeners.delete(type); },
  };
  const focusOn = (element: object) => () => { doc.activeElement = element; };
  const items = Array.from({ length: itemCount }, (_, index) => {
    const item: any = { name: `item ${index}` };
    item.focus = focusOn(item);
    return item;
  });
  const inside = new Set<unknown>(items);
  const asked: string[] = [];
  const menu = {
    contains: (node: unknown) => inside.has(node),
    querySelectorAll: (selector: string) => { asked.push(selector); return items; },
  };
  const opener: any = { name: 'opener', isConnected: true };
  opener.focus = focusOn(opener);
  const trigger = { contains: (node: unknown) => node === opener };
  let closed = 0;
  const attach = (extra: { trigger?: typeof trigger } = {}) => attachPopupMenu({ menu: menu as any, opener, onClose: () => { closed++; }, doc, ...extra });
  const key = (name: string) => {
    const event = { key: name, prevented: false, preventDefault() { this.prevented = true; } };
    doc.listeners.get('keydown')?.listener(event);
    return event;
  };
  const press = (target: unknown) => doc.listeners.get('pointerdown')?.listener({ target });
  return { doc, items, opener, trigger, asked, attach, key, press, closed: () => closed };
}

test('the first item that can be used takes the focus when the menu comes up, and only items that can be used are reached', () => {
  const s = scene();
  s.attach();
  assert.equal(s.doc.activeElement, s.items[0]);
  assert.ok(s.asked.every((selector) => selector === MENU_ITEMS));
  assert.match(MENU_ITEMS, /\[role="menuitem"\]:not\(:disabled\)/);
});

test('the arrow keys move between the items and wrap round; Home and End go to the ends', () => {
  const s = scene();
  s.attach();
  assert.equal(s.key('ArrowDown').prevented, true);
  assert.equal(s.doc.activeElement, s.items[1]);
  s.key('ArrowDown');
  s.key('ArrowDown');
  assert.equal(s.doc.activeElement, s.items[0], 'past the last is the first');
  s.key('ArrowUp');
  assert.equal(s.doc.activeElement, s.items[2], 'before the first is the last');
  s.key('Home');
  assert.equal(s.doc.activeElement, s.items[0]);
  s.key('End');
  assert.equal(s.doc.activeElement, s.items[2]);
  assert.equal(s.key('a').prevented, false, 'other keys are not taken');
  assert.equal(s.closed(), 0);
});

test('with the focus outside the items, Down reaches the first and Up the last', () => {
  const s = scene();
  s.attach();
  s.doc.activeElement = s.doc.body;
  s.key('ArrowDown');
  assert.equal(s.doc.activeElement, s.items[0]);
  s.doc.activeElement = s.doc.body;
  s.key('ArrowUp');
  assert.equal(s.doc.activeElement, s.items[2]);
});

test('Escape closes the menu and the page does not also act on it; Tab closes it and lets the focus move on', () => {
  const s = scene();
  s.attach();
  const escape = s.key('Escape');
  assert.equal(escape.prevented, true);
  assert.equal(s.closed(), 1);
  const tab = s.key('Tab');
  assert.equal(tab.prevented, false);
  assert.equal(s.closed(), 2);
});

test('a press outside the menu closes it; a press on an item, or on the button that opened it, does not', () => {
  const s = scene();
  s.attach({ trigger: s.trigger });
  s.press(s.items[1]);
  assert.equal(s.closed(), 0, 'on an item');
  s.press(s.opener);
  assert.equal(s.closed(), 0, 'on the trigger, which closes the menu itself');
  s.press({ name: 'somewhere else' });
  assert.equal(s.closed(), 1);
  assert.equal(s.doc.listeners.get('pointerdown')?.capture, true, 'seen before the page can stop it');
});

test('stopping gives the focus back to what had it, unless the owner has put it somewhere else', () => {
  const closing = scene();
  const stop = closing.attach();
  assert.equal(closing.doc.activeElement, closing.items[0]);
  closing.doc.activeElement = closing.doc.body; // the item is gone with the menu, which leaves the focus on the page
  stop();
  assert.equal(closing.doc.activeElement, closing.opener);
  assert.equal(closing.doc.listeners.size, 0, 'nothing is left listening');

  const moved = scene();
  const elsewhere = { name: 'elsewhere' };
  const stopMoved = moved.attach();
  moved.doc.activeElement = elsewhere;
  stopMoved();
  assert.equal(moved.doc.activeElement, elsewhere);

  const gone = scene();
  gone.opener.isConnected = false;
  const stopGone = gone.attach();
  gone.doc.activeElement = gone.doc.body;
  stopGone();
  assert.equal(gone.doc.activeElement, gone.doc.body, 'an opener that left the page is not focused');
});

test('a menu with nothing to choose takes no keys', () => {
  const s = scene(0);
  s.attach();
  assert.equal(s.key('ArrowDown').prevented, false);
  assert.equal(s.key('End').prevented, false);
  assert.equal(s.key('Escape').prevented, true, 'it can still be closed');
});
