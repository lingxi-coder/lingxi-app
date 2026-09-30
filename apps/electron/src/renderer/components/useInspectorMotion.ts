import { useEffect, useLayoutEffect, useRef, useState } from 'react';

/** Native animation playback owns completion/cancellation; React owns presence. */
export function useInspectorMotion(open: boolean, contentKey: string, scope: string) {
  const ref = useRef<HTMLElement>(null);
  const animation = useRef<Animation | null>(null);
  const contentAnimation = useRef<Animation | null>(null);
  const previousScope = useRef(scope);
  const previousContent = useRef(contentKey);
  const first = useRef(true);
  const [present, setPresent] = useState(open);
  if (open && !present) setPresent(true);

  useLayoutEffect(() => {
    const node = ref.current;
    const initial = first.current;
    const changedSession = previousScope.current !== scope;
    first.current = false;
    previousScope.current = scope;
    if (!node) {
      animation.current?.cancel();
      animation.current = null;
      return;
    }
    node.inert = !open;
    const media = window.matchMedia('(prefers-reduced-motion: reduce)');
    const finish = () => {
      if (!open) {
        setPresent(false);
      } else {
        animation.current?.cancel();
        animation.current = null;
      }
    };
    if (media.matches || changedSession || (initial && open)) {
      finish();
      return;
    }
    const style = getComputedStyle(node);
    const overlay = style.position === 'absolute' || style.position === 'fixed';
    const resting = { opacity: '1', transform: 'translateX(0px)', marginRight: '0px' };
    const hidden = { opacity: '0', transform: 'translateX(20px)', marginRight: overlay ? '0px' : `${-node.offsetWidth}px` };
    // Read the interpolated frame BEFORE cancelling, so rapid toggles retarget
    // from what the user actually sees, without jumping to an endpoint.
    const from = animation.current
      ? { opacity: style.opacity, transform: style.transform, marginRight: style.marginRight }
      : open ? hidden : resting;
    animation.current?.cancel();
    const playback = node.animate([from, open ? resting : hidden], {
      duration: open ? 220 : 160, easing: 'cubic-bezier(0.2, 0, 0, 1)', fill: 'both',
    });
    animation.current = playback;
    void playback.finished.then(() => {
      if (animation.current !== playback) return;
      finish();
    }, () => { /* Cancellation is expected when the user changes direction. */ });
    const reduce = () => {
      if (media.matches) { contentAnimation.current?.cancel(); finish(); }
    };
    media.addEventListener('change', reduce);
    return () => media.removeEventListener('change', reduce);
  }, [open, present, scope]);

  useLayoutEffect(() => {
    const changed = previousContent.current !== contentKey;
    previousContent.current = contentKey;
    contentAnimation.current?.cancel();
    if (!changed || !open || window.matchMedia('(prefers-reduced-motion: reduce)').matches) return;
    const content = ref.current?.querySelector<HTMLElement>('#runtime-inspector-panel');
    if (!content) return;
    const playback = content.animate([{ opacity: 0.65 }, { opacity: 1 }], {
      duration: 120, easing: 'ease-out',
    });
    contentAnimation.current = playback;
    return () => playback.cancel();
  }, [contentKey, open]);

  useEffect(() => () => {
    animation.current?.cancel();
    contentAnimation.current?.cancel();
  }, []);
  return { ref, present };
}
