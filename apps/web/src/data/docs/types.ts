export type DocText = { en: string; zh: string };

export interface DocCode {
  label: string | DocText;
  language: string;
  code: string;
  verification?: 'compile' | 'mock' | 'manual' | 'excerpt';
  exampleId?: string;
}

export type DocRepository = 'lingxi-app' | 'harness-runtime' | 'llm-client' | 'mobile-linux-runtime';

export interface DocSource { repo: DocRepository; path: string }
export interface DocLink { label: DocText; href: string }
export interface DocTable { headers: DocText[]; rows: DocText[][] }
export interface DocDiagram { src: string; alt: DocText; caption?: DocText }

export interface DocApi {
  name: string;
  signature: string;
  description: DocText;
}

export interface DocSection {
  id: string;
  title: DocText;
  paragraphs?: DocText[];
  bullets?: DocText[];
  code?: DocCode[];
  apis?: DocApi[];
  note?: DocText;
  tables?: DocTable[];
  steps?: DocText[];
  links?: DocLink[];
  diagram?: DocDiagram;
}

export interface DocPage {
  id: string;
  group: 'start' | 'system' | 'harness' | 'llm' | 'mobile' | 'bridge' | 'operations';
  title: DocText;
  description: DocText;
  packageName?: string;
  sourceUrl: string;
  sources?: DocSource[];
  sections: DocSection[];
}
