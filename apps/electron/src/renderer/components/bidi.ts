/**
 * Bidi glue for the head-clipping trick, kept out of the component files so it
 * runs under `node --test` with no DOM.
 *
 * ## The bug this exists to prevent
 *
 * A file path is clipped at its HEAD — `…/to/host.rs` — by laying the text out
 * `direction: rtl`, which is the only way CSS puts `text-overflow: ellipsis` at
 * the START of a line. But `direction` also sets the BIDI PARAGRAPH direction,
 * and the Unicode bidi algorithm then resolves the neutral characters at the
 * edges of the string to that paragraph direction. A leading `/` is bidi class
 * CS with no strong character before it, so rule N2 hands it the RTL paragraph
 * level and it renders rightmost:
 *
 *     logical            : /Users/luo/host.rs
 *     display (base RTL) : Users/luo/host.rs/
 *
 * The file tools REQUIRE absolute paths, so that was every `Update(…)` /
 * `Read(…)` / `Write(…)` header and every diff header in the app.
 *
 * ## Why U+200E and not `unicode-bidi: plaintext`
 *
 * `plaintext` derives the paragraph direction from the first STRONG character
 * (P2/P3) instead of from `direction`. For a path that direction is LTR — which
 * fixes the reordering but also moves the ellipsis back to the tail, throwing
 * away the filename. Head-clipping is the whole point of the RTL layout, so
 * that cure is worse than the disease.
 *
 * U+200E LEFT-TO-RIGHT MARK is instead a zero-width STRONG L character. With
 * one on each side, a leading or trailing separator now sits between two L runs
 * and rule N1 resolves it to L along with them, while the line itself stays
 * RTL-based and keeps clipping at the head. Interior strong RTL text (an Arabic
 * filename) still forms its own RTL run, as it should.
 */

/** U+200E LEFT-TO-RIGHT MARK — zero-width, strong L. Spelled escaped: it is
 * invisible in an editor and a literal one would be silently deletable. */
export const LRM = '\u200E';

/**
 * Fence `text` with LRMs so its edge separators cannot be reordered by an
 * RTL-based line. Idempotent, and `''` stays `''` (an empty span needs no
 * fence, and fencing it would defeat a falsy check).
 */
export function ltrAnchored(text: string): string {
  if (!text) return text;
  const head = text.startsWith(LRM) ? '' : LRM;
  const tail = text.endsWith(LRM) ? '' : LRM;
  return `${head}${text}${tail}`;
}
