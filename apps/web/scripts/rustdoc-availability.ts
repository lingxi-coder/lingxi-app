import { readFile, stat } from 'node:fs/promises';
import path from 'node:path';

interface Profile { id: string; state: string; url?: string; reason?: string; [key: string]: unknown }
interface Manifest { profiles: Profile[]; [key: string]: unknown }

/** Saved build evidence is not proof that ignored artifacts exist in this bundle. */
export async function availableRustdocManifest(directory: string): Promise<Manifest> {
  let manifest: Manifest;
  try { manifest = JSON.parse(await readFile(path.join(directory, 'rustdoc-manifest.json'), 'utf8')); }
  catch { return { profiles: [] }; }
  if (!Array.isArray(manifest.profiles)) return { profiles: [] };
  const profiles = await Promise.all(manifest.profiles.map(async (profile) => {
    if (profile.state !== 'ready') return profile;
    const url = profile.url;
    const valid = typeof url === 'string' && /^\/rustdoc\/[a-z0-9-]+\/[a-zA-Z0-9_]+\/index\.html$/.test(url);
    if (valid) {
      try { if ((await stat(path.join(directory, url.slice(1)))).isFile()) return profile; } catch { /* absent compiler artifact */ }
    }
    const { url: _url, ...saved } = profile;
    return { ...saved, state: 'unavailable', reason: 'Compiler documentation is absent from this bundle. Run npm run docs:rustdoc before building.' };
  }));
  return { ...manifest, profiles };
}
