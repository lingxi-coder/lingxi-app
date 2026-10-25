import baseline from './baseline.json';
import type { DocPage, DocRepository } from './types';

export interface DocRepositoryBaseline {
  repo: DocRepository;
  revision: string;
  sourceUrl: string;
  dirty: boolean;
  published?: boolean;
  treeFingerprint: string;
  productPinnedRevision?: string;
}

export interface ResolvedDocSource {
  repo: DocRepository;
  path: string;
  revision: string;
  sourceState: 'committed' | 'unpublished' | 'working-tree';
  url?: string;
}

export const docsBaseline = baseline as {
  schemaVersion: number;
  sourceMode: string;
  repositories: DocRepositoryBaseline[];
  pages: Record<string, ResolvedDocSource[]>;
};

export function resolveDocSources(page: DocPage): ResolvedDocSource[] {
  const saved = docsBaseline.pages[page.id] ?? [];
  if (JSON.stringify(saved.map(({ repo, path }) => ({ repo, path }))) !== JSON.stringify(page.sources ?? [])) return [];
  return saved.map((source) => source.sourceState !== 'committed' ? { ...source, url: undefined } : source);
}

export function validateDocBaseline(pages: DocPage[], baseline = docsBaseline): void {
  if (JSON.stringify(Object.keys(baseline.pages).sort()) !== JSON.stringify(pages.map((page) => page.id).sort())) {
    throw new Error('Handbook baseline page set is stale; run npm run docs:sync');
  }
  for (const page of pages) {
    const saved = baseline.pages[page.id];
    if (JSON.stringify(saved.map(({ repo, path }) => ({ repo, path }))) !== JSON.stringify(page.sources ?? [])) {
      throw new Error(`Handbook sources for ${page.id} are stale; run npm run docs:sync`);
    }
  }
}

export const verificationLabels = {
  compile: { en: 'Compile check', zh: '编译检查' },
  mock: { en: 'Mock run', zh: '模拟运行' },
  manual: { en: 'Environment validation', zh: '需环境验证' },
  excerpt: { en: 'API excerpt', zh: '接口摘录' },
} as const;

export function codeLabel(label: string | { en: string; zh: string }, locale: 'en' | 'zh'): string {
  return typeof label === 'string' ? label : label[locale];
}
