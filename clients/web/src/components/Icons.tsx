import { SVGProps } from 'react';

function createIcon(path: React.ReactNode, props: SVGProps<SVGSVGElement>) {
  return (
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true" {...props}>
      {path}
    </svg>
  );
}

export function IconSpark(props: SVGProps<SVGSVGElement>) {
  return createIcon(
    <>
      <path d="M12 3l1.6 5.4L19 10l-5.4 1.6L12 17l-1.6-5.4L5 10l5.4-1.6L12 3Z" />
      <path d="M19 3v4" />
      <path d="M21 5h-4" />
    </>,
    props,
  );
}

export function IconPhone(props: SVGProps<SVGSVGElement>) {
  return createIcon(
    <>
      <rect x="7" y="2.5" width="10" height="19" rx="2.5" />
      <path d="M10 5.5h4" />
      <path d="M11 18.5h2" />
    </>,
    props,
  );
}

export function IconDesktop(props: SVGProps<SVGSVGElement>) {
  return createIcon(
    <>
      <rect x="3" y="4" width="18" height="12" rx="2" />
      <path d="M8 20h8" />
      <path d="M12 16v4" />
    </>,
    props,
  );
}

export function IconTerminal(props: SVGProps<SVGSVGElement>) {
  return createIcon(
    <>
      <rect x="3" y="4" width="18" height="15" rx="2" />
      <path d="m7 9 3 3-3 3" />
      <path d="M13 15h4" />
    </>,
    props,
  );
}

export function IconArrowRight(props: SVGProps<SVGSVGElement>) {
  return createIcon(
    <>
      <path d="M5 12h13" />
      <path d="m14 7 5 5-5 5" />
    </>,
    props,
  );
}

export function IconDownload(props: SVGProps<SVGSVGElement>) {
  return createIcon(
    <>
      <path d="M12 4v10" />
      <path d="m8 10 4 4 4-4" />
      <path d="M5 19h14" />
    </>,
    props,
  );
}

export function IconChart(props: SVGProps<SVGSVGElement>) {
  return createIcon(
    <>
      <path d="M4 19V5" />
      <path d="M4 19h16" />
      <path d="m7 14 3-3 3 2 4-5" />
    </>,
    props,
  );
}

export function IconKey(props: SVGProps<SVGSVGElement>) {
  return createIcon(
    <>
      <path d="M14 8a4 4 0 1 0 0 8 4 4 0 0 0 0-8Z" />
      <path d="M10.5 13H3v3h3v2h2v-2h2l.5-.5" />
    </>,
    props,
  );
}

export function IconWallet(props: SVGProps<SVGSVGElement>) {
  return createIcon(
    <>
      <path d="M3 7.5A2.5 2.5 0 0 1 5.5 5H18a3 3 0 0 1 3 3v8a3 3 0 0 1-3 3H6a3 3 0 0 1-3-3v-8Z" />
      <path d="M16 12h3" />
      <path d="M3 9h18" />
    </>,
    props,
  );
}

export function IconDocs(props: SVGProps<SVGSVGElement>) {
  return createIcon(
    <>
      <path d="M7 4h9l3 3v13H7a3 3 0 0 0-3 3V7a3 3 0 0 1 3-3Z" />
      <path d="M16 4v4h4" />
      <path d="M9 12h6" />
      <path d="M9 16h6" />
    </>,
    props,
  );
}

export function IconSun(props: SVGProps<SVGSVGElement>) {
  return createIcon(
    <>
      <circle cx="12" cy="12" r="4" />
      <path d="M12 2v2.5M12 19.5V22M4.9 4.9l1.8 1.8M17.3 17.3l1.8 1.8M2 12h2.5M19.5 12H22M4.9 19.1l1.8-1.8M17.3 6.7l1.8-1.8" />
    </>,
    props,
  );
}

export function IconMoon(props: SVGProps<SVGSVGElement>) {
  return createIcon(<path d="M19 14.5A7.5 7.5 0 0 1 9.5 5a8.5 8.5 0 1 0 9.5 9.5Z" />, props);
}

export function IconGlobe(props: SVGProps<SVGSVGElement>) {
  return createIcon(
    <>
      <circle cx="12" cy="12" r="9" />
      <path d="M3 12h18" />
      <path d="M12 3a15 15 0 0 1 0 18" />
      <path d="M12 3a15 15 0 0 0 0 18" />
    </>,
    props,
  );
}

export function IconCheck(props: SVGProps<SVGSVGElement>) {
  return createIcon(
    <>
      <path d="m5 12 4 4L19 6" />
    </>,
    props,
  );
}

export function IconSearch(props: SVGProps<SVGSVGElement>) {
  return createIcon(
    <>
      <circle cx="11" cy="11" r="6.5" />
      <path d="m16 16 4 4" />
    </>,
    props,
  );
}

export function IconCopy(props: SVGProps<SVGSVGElement>) {
  return createIcon(
    <>
      <rect x="9" y="9" width="10" height="10" rx="2" />
      <path d="M6 15H5a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h8a2 2 0 0 1 2 2v1" />
    </>,
    props,
  );
}

export function IconClose(props: SVGProps<SVGSVGElement>) {
  return createIcon(
    <>
      <path d="m6 6 12 12M18 6 6 18" />
    </>,
    props,
  );
}
