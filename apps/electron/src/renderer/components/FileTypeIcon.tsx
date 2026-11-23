import { fileTypeIcon } from '../model/fileTypeIcon';

export function FileTypeIcon({ path }: { path: string }) {
  return <img src={fileTypeIcon(path)} alt="" aria-hidden="true" width={18} height={18} style={{ flex: 'none' }} />;
}
