export type DocText = { en: string; zh: string };

export interface DocCode {
  label: string;
  language: string;
  code: string;
}

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
}

export interface DocPage {
  id: string;
  group: 'start' | 'harness' | 'llm' | 'mobile' | 'bridge';
  title: DocText;
  description: DocText;
  packageName?: string;
  sourceUrl: string;
  sections: DocSection[];
}
