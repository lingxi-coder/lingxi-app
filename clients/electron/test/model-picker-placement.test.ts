/**
 * The model picker's width budget.
 *
 * Whether a submenu is READABLE is measured in a real window by
 * `model-picker-interaction.test.mjs`; what is pinned here is the decision
 * itself — the threshold it switches on, and the guess it makes before it has
 * measured anything.
 */

import { test } from 'node:test';
import assert from 'node:assert/strict';

import {
  modelPickerSubmenuPlacement,
  MODEL_PICKER_MENU_WIDTH,
  MODEL_PICKER_SUBMENU_GAP,
} from '../src/renderer/components/modelPickerPlacement';

const FLYOUT = MODEL_PICKER_MENU_WIDTH + MODEL_PICKER_SUBMENU_GAP;

/** Bounds whose span is exactly `span` pixels wide. */
const span = (width: number) => ({ anchorRight: 1000, boundaryLeft: 1000 - width });

test('a submenu flies out as soon as the full width fits beside the menu', () => {
  assert.deepEqual(
    modelPickerSubmenuPlacement(390, span(FLYOUT + 390)),
    { width: 390, right: FLYOUT },
  );
  // One pixel short is short: half a panel beside the menu is the clipped
  // sliver this exists to prevent, not a near miss worth rendering.
  assert.deepEqual(
    modelPickerSubmenuPlacement(390, span(FLYOUT + 389)),
    { width: 390, right: 0 },
  );
});

test('a submenu that cannot fit beside the menu drills down over it', () => {
  // The reported window: a 900px app with the default sidebar leaves the
  // picker roughly 530px, far short of the 742px a 390px flyout needs.
  assert.deepEqual(modelPickerSubmenuPlacement(390, span(530)), { width: 390, right: 0 });
  assert.deepEqual(modelPickerSubmenuPlacement(300, span(530)), { width: 300, right: 0 });
});

test('a submenu never renders wider than the space it has', () => {
  assert.deepEqual(modelPickerSubmenuPlacement(390, span(240)), { width: 240, right: 0 });
  // A negative span is a control that has been laid out to the left of its own
  // boundary — nonsense geometry, and a negative width would throw off layout
  // rather than degrade.
  assert.deepEqual(
    modelPickerSubmenuPlacement(390, { anchorRight: 100, boundaryLeft: 400 }),
    { width: 0, right: 0 },
  );
});

test('an unmeasured submenu flies out, which is where it belongs in a normal window', () => {
  assert.deepEqual(modelPickerSubmenuPlacement(390, null), { width: 390, right: FLYOUT });
});
