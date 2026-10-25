import { readFileSync, writeFileSync } from 'node:fs';
import { Data, NtExecutable, NtExecutableResource, Resource } from 'resedit';

// Runs before signing/archiving on Windows. Upstream signatures cannot survive
// resource changes; generate() strips them so the final app can be signed anew.
export function applyWindowsBrand(executablePath, iconPath, name) {
  const executable = NtExecutable.from(readFileSync(executablePath), { ignoreCert: true });
  const resources = NtExecutableResource.from(executable);
  const groups = Resource.IconGroupEntry.fromEntries(resources.entries);
  if (groups.length !== 1) throw new Error('Expected exactly one Electron executable icon group');
  const icon = Data.IconFile.from(readFileSync(iconPath));
  Resource.IconGroupEntry.replaceIconsForResource(resources.entries, groups[0].id, groups[0].lang, icon.icons.map(item => item.data));
  const versions = Resource.VersionInfo.fromEntries(resources.entries);
  if (versions.length !== 1) throw new Error('Expected exactly one executable version resource');
  for (const language of versions[0].getAllLanguagesForStringValues()) {
    versions[0].setStringValues(language, { ProductName: name, FileDescription: name, OriginalFilename: `${name}.exe` });
  }
  versions[0].outputToResourceEntries(resources.entries);
  resources.outputResource(executable);
  writeFileSync(executablePath, Buffer.from(executable.generate()));
}
