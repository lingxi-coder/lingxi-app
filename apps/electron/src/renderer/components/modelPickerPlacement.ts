/**
 * Where the composer's model-picker submenus are allowed to sit.
 *
 * The picker menu is right-aligned to the model pill, which lives at the right
 * end of the composer toolbar, and each submenu flies out to its LEFT. Nothing
 * in CSS knows how much room that leaves: the submenu's containing block is the
 * menu itself, so `calc(100vw - …)` clamps against the window while the real
 * limit is the left edge of `.desktop-workspace-upper` — an `overflow: hidden`
 * box that starts where the sidebar ends. A flyout wider than that gap is not
 * merely tucked under the sidebar, it is CLIPPED AWAY, which is how a 390px
 * model list rendered as a 200px sliver with every model name cut off.
 *
 * So the placement is measured instead. These helpers are pure so the geometry
 * can be reasoned about without a DOM; the honest check that the numbers land
 * where they should is the Electron interaction test, which opens the real
 * picker in a real window and compares rectangles.
 */

/** Width of the picker menu the submenus fly out from. */
export const MODEL_PICKER_MENU_WIDTH = 340;

/** Gap between the flown-out submenu and the picker menu. */
export const MODEL_PICKER_SUBMENU_GAP = 12;

/**
 * The horizontal band a submenu may occupy, in viewport pixels.
 *
 * `anchorRight` is the right edge of the picker menu (it shares the model
 * control's right edge); `boundaryLeft` is the leftmost pixel that survives
 * the clipping ancestor.
 */
export type ModelPickerSubmenuBounds = { anchorRight: number; boundaryLeft: number };

/**
 * A resolved submenu box. `right` is the offset from the model control's right
 * edge — which the menu is flush against — so `right: 0` means the submenu
 * covers the menu and anything larger means it has flown out beside it.
 */
export type ModelPickerSubmenuPlacement = { width: number; right: number };

/**
 * Place a submenu of `preferredWidth` inside `bounds`.
 *
 * It flies out to the left whenever the full preferred width fits there; when
 * it does not, it drills down OVER the menu instead of hanging off the side,
 * because a sideways panel that does not fit is the clipped sliver this exists
 * to prevent. Overlaying shares the menu's bottom edge so the menu does not
 * peek out beneath it. `bounds` is null before the first measurement, where the
 * uncorrected flyout is the right guess — the menu is usually far from its
 * limit, and the layout effect that measures runs before paint.
 */
export function modelPickerSubmenuPlacement(
  preferredWidth: number,
  bounds: ModelPickerSubmenuBounds | null,
): ModelPickerSubmenuPlacement {
  const flyout = MODEL_PICKER_MENU_WIDTH + MODEL_PICKER_SUBMENU_GAP;
  if (!bounds) return { width: preferredWidth, right: flyout };
  const span = Math.max(0, Math.floor(bounds.anchorRight - bounds.boundaryLeft));
  if (span - flyout >= preferredWidth) return { width: preferredWidth, right: flyout };
  return { width: Math.min(preferredWidth, span), right: 0 };
}
