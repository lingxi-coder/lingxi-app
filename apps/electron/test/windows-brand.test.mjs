import test from 'node:test';
import assert from 'node:assert/strict';
import { mkdtempSync, readFileSync, writeFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { Data, NtExecutable, NtExecutableResource, Resource } from 'resedit';
import { applyWindowsBrand } from '../scripts/windows-brand.mjs';

test('Windows branding writes a readable PE icon and product label without changing its manifest', () => {
  const folder = mkdtempSync(join(tmpdir(), 'lingxi-brand-'));
  try {
    const path = join(folder, 'app.exe');
    const iconPath = new URL('../assets/icons/icon.ico', import.meta.url);
    const icons = Data.IconFile.from(readFileSync(iconPath));
    const exe = NtExecutable.createEmpty(false, false);
    const resource = NtExecutableResource.from(exe);
    Resource.IconGroupEntry.replaceIconsForResource(resource.entries, 1, 1033, [icons.icons[0].data]);
    Resource.VersionInfo.create({lang:1033,fixedInfo:{},strings:[{lang:1033,codepage:1200,values:{ProductName:'Electron',LegalCopyright:'Original copyright'}}]}).outputToResourceEntries(resource.entries);
    const manifest = new TextEncoder().encode('<assembly><trustInfo>unchanged</trustInfo></assembly>');
    resource.entries.push({type:24,id:1,lang:1033,codepage:0,bin:manifest.buffer});
    resource.outputResource(exe);
    writeFileSync(path,Buffer.from(exe.generate()));
    applyWindowsBrand(path,iconPath,'LingXi Code');
    const updated=NtExecutableResource.from(NtExecutable.from(readFileSync(path)));
    const group=Resource.IconGroupEntry.fromEntries(updated.entries)[0];
    assert.equal(group.icons.length,icons.icons.length);
    const strings=Resource.VersionInfo.fromEntries(updated.entries)[0].getStringValues({lang:1033,codepage:1200});
    assert.equal(strings.ProductName,'LingXi Code');
    assert.equal(strings.LegalCopyright,'Original copyright');
    assert.deepEqual(Buffer.from(updated.entries.find(entry=>entry.type===24).bin),Buffer.from(manifest));
  } finally { rmSync(folder,{recursive:true,force:true}); }
});
