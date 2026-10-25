import { tr } from '../../i18n/locale';
import { useEffect, useRef, useState } from 'react';
import type { PageContentProps } from './SettingsScreen';

/** The settings shell delegates close/navigation to the current draft owner. */
export function useSettingsDraftGuard(props: Pick<PageContentProps, 'onClose' | 'onNavigate' | 'onRegisterCloseRequest' | 'onRegisterNavigationRequest'>, dirty: boolean, guardNavigation = false) {
  const [pending, setPending] = useState<(() => void) | null>(null);
  const current = useRef({ props, dirty }); current.current = { props, dirty };
  useEffect(() => {
    const request = (action: () => void) => { if (current.current.dirty) setPending(() => action); else action(); };
    props.onRegisterCloseRequest?.(() => request(() => current.current.props.onClose()));
    if (guardNavigation) props.onRegisterNavigationRequest?.((pageId, pluginId) => request(() => current.current.props.onNavigate(pageId, pluginId)));
    return () => { props.onRegisterCloseRequest?.(null); if (guardNavigation) props.onRegisterNavigationRequest?.(null); };
  }, [props.onRegisterCloseRequest, props.onRegisterNavigationRequest, guardNavigation]);
  return pending ? <div role="alertdialog" aria-modal="true" aria-label={tr('desktop_shell_unsaved_title')} style={{ position: 'fixed', zIndex: 120, top: 80, left: '30%', maxWidth: 520, background: 'Canvas', color: 'CanvasText', padding: 16, border: '1px solid currentColor', borderRadius: 8, marginBottom: 16 }}>
    <p>{tr('desktop_shell_unsaved_question')}</p>
    <button type="button" onClick={() => { const action = pending; setPending(null); action(); }}>{tr('desktop_shell_discard_leave')}</button>{' '}
    <button type="button" onClick={() => setPending(null)}>{tr('desktop_shell_continue_editing')}</button>
  </div> : null;
}
