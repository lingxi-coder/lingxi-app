const MAX_UTF16 = 100_000;
const MAX_VALUES = 20_000;
const MAX_DEPTH = 32;

/** Host plain-data bounds (hooks/mod_ui_client_host.rs). Count the JSON that
 * crosses the wire without allocating another copy of an oversized payload.
 */
export function validateModUiPlainData(value: unknown, name: string): void {
  let values = 0;
  let units = 0;
  const ancestors = new Set<object>();
  const add = (length: number) => {
    units += length;
    if (units > MAX_UTF16) throw new TypeError(`${name} exceeds the Native serialized-data limit`);
  };
  const string = (text: string) => {
    // Every source UTF-16 unit needs at least one serialized unit.
    if (units + text.length + 2 > MAX_UTF16) throw new TypeError(`${name} exceeds the Native serialized-data limit`);
    add(2);
    for (let i = 0; i < text.length; i++) {
      const unit = text.charCodeAt(i);
      if (unit === 34 || unit === 92 || (unit === 8 || unit === 9 || unit === 10 || unit === 12 || unit === 13)) add(2);
      else if (unit < 32) add(6);
      else if (unit >= 0xd800 && unit <= 0xdbff) {
        const next = text.charCodeAt(i + 1);
        if (next >= 0xdc00 && next <= 0xdfff) { add(2); i++; }
        else add(6);
      } else if (unit >= 0xdc00 && unit <= 0xdfff) add(6);
      else add(1);
    }
  };
  const visit = (current: unknown, depth: number): void => {
    if (++values > MAX_VALUES || depth > MAX_DEPTH) throw new TypeError(`${name} exceeds the Native plain-data bounds`);
    if (current === null) { add(4); return; }
    if (typeof current === 'string') { string(current); return; }
    if (typeof current === 'boolean') { add(current ? 4 : 5); return; }
    if (typeof current === 'number') {
      if (!Number.isFinite(current)) throw new TypeError(`${name} contains a number that is not finite`);
      add(JSON.stringify(current).length);
      return;
    }
    if (typeof current !== 'object' || ancestors.has(current)) throw new TypeError(`${name} must be acyclic plain JSON data`);
    const toJSON = Object.getOwnPropertyDescriptor(current, 'toJSON');
    if (toJSON && (!('value' in toJSON) || typeof toJSON.value === 'function')) {
      throw new TypeError(`${name} must be plain JSON data`);
    }
    ancestors.add(current);
    add(2);
    if (Array.isArray(current)) {
      if (current.length > MAX_VALUES - values) throw new TypeError(`${name} exceeds the Native plain-data bounds`);
      for (let index = 0; index < current.length; index++) {
        const property = Object.getOwnPropertyDescriptor(current, index);
        if (!property) throw new TypeError(`${name} cannot contain sparse arrays`);
        if (!('value' in property)) throw new TypeError(`${name} must be plain JSON data`);
        if (index) add(1);
        visit(property.value, depth + 1);
      }
    } else {
      const prototype = Object.getPrototypeOf(current);
      if (prototype !== null && prototype !== Object.prototype) throw new TypeError(`${name} must be plain JSON data`);
      const keys = Object.keys(current);
      if (keys.length > MAX_VALUES - values) throw new TypeError(`${name} exceeds the Native plain-data bounds`);
      for (const [index, key] of keys.entries()) {
        if (index) add(1);
        string(key);
        add(1);
        const property = Object.getOwnPropertyDescriptor(current, key);
        if (!property || !('value' in property)) throw new TypeError(`${name} must be plain JSON data`);
        visit(property.value, depth + 1);
      }
    }
    ancestors.delete(current);
  };
  visit(value, 0);
}

export function modUiRenderPropsWithinBounds(value: unknown): boolean {
  if (value === null || typeof value !== 'object' || Array.isArray(value)) return false;
  try { validateModUiPlainData(value, 'Mod UI render props'); return true; }
  catch { return false; }
}
