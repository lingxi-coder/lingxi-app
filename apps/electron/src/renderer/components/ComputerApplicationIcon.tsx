import { useEffect, useState, type ReactNode } from 'react';
import { desktopHost } from '../bridge/bridgeConnection';
import type { ToolRunItem } from '../model/runItem';
import { computerApplication } from './computerApplication';

export function ComputerApplicationIcon({ item, fallback }: { item?: ToolRunItem; fallback: ReactNode }) {
  const application = item ? computerApplication(item) : null;
  const bundleId = application?.bundleId;
  const [icon, setIcon] = useState<{ bundleId: string; url: string } | null>(null);
  const [failed, setFailed] = useState<string | null>(null);
  useEffect(() => {
    let current = true;
    if (bundleId && typeof window !== 'undefined') {
      void desktopHost()?.applicationIcon?.get(bundleId).then(url => {
        if (current && url?.startsWith('data:image/png;base64,')) setIcon({ bundleId, url });
      }).catch(() => undefined);
    }
    return () => { current = false; };
  }, [bundleId]);
  return icon && icon.bundleId === bundleId && failed !== icon.url
    ? <img className="tool-application-icon" src={icon.url} width={20} height={20} alt="" aria-hidden="true" title={application?.name} draggable={false} onError={() => setFailed(icon.url)} style={{ display: 'block', flexShrink: 0, objectFit: 'contain' }} />
    : <>{fallback}</>;
}
