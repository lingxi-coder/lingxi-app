import { execFile } from 'node:child_process';
import { promisify } from 'node:util';
import { validBundleId } from '../shared/applicationIcon.js';

const run = promisify(execFile);
const cache = new Map<string, Promise<string | null>>();
const resolveScript = `ObjC.import('AppKit');
function run(argv) {
  const url = $.NSWorkspace.sharedWorkspace.URLForApplicationWithBundleIdentifier(argv[0]);
  if (!url || url.isNil()) return '';
  const image = $.NSWorkspace.sharedWorkspace.iconForFile(ObjC.unwrap(url.path));
  const small = $.NSImage.alloc.initWithSize($.NSMakeSize(40, 40));
  small.lockFocus;
  image.drawInRectFromRectOperationFraction($.NSMakeRect(0, 0, 40, 40), $.NSZeroRect, $.NSCompositingOperationCopy, 1);
  small.unlockFocus;
  const bitmap = $.NSBitmapImageRep.imageRepWithData(small.TIFFRepresentation);
  const png = bitmap.representationUsingTypeProperties($.NSPNGFileType, $({}));
  return ObjC.unwrap(png.base64EncodedStringWithOptions(0));
}`;

export async function applicationIcon(bundleId: unknown): Promise<string | null> {
  if (!validBundleId(bundleId) || process.platform !== 'darwin') return null;
  const existing = cache.get(bundleId);
  if (existing) return existing;
  const pending = (async () => {
    try {
      const { stdout } = await run('/usr/bin/osascript', ['-l', 'JavaScript', '-e', resolveScript, bundleId], { timeout: 3000, maxBuffer: 128 * 1024 });
      const png = stdout.trim();
      return png.startsWith('iVBORw0KGgo') && /^[A-Za-z0-9+/]+=*$/.test(png) ? `data:image/png;base64,${png}` : null;
    } catch { return null; }
  })();
  // Bound memory while coalescing concurrent transcript requests.
  if (cache.size >= 128) cache.delete(cache.keys().next().value!);
  cache.set(bundleId, pending);
  return pending;
}
