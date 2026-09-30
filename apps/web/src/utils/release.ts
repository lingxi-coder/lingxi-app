import { ReleaseArtifact, ReleaseStatus } from '../types/ReleaseArtifact';
import { Locale, LocalizedText, pickLocaleText } from './locale';

export interface ReleaseStatusMeta {
  label: string;
  description: string;
  tone: 'success' | 'accent' | 'muted' | 'danger';
}

const releaseStatusCopy: Record<ReleaseStatus, { label: LocalizedText; description: LocalizedText; tone: ReleaseStatusMeta['tone'] }> = {
  stable: {
    label: { en: 'Stable', zh: '稳定版' },
    description: { en: 'Ready for production work.', zh: '适合稳定日常使用。' },
    tone: 'success',
  },
  beta: {
    label: { en: 'Beta', zh: '测试版' },
    description: { en: 'Shipping fast with active polish.', zh: '快速迭代中，适合尝鲜。' },
    tone: 'accent',
  },
  'coming-soon': {
    label: { en: 'Coming Soon', zh: '即将推出' },
    description: { en: 'Planned, but not yet downloadable.', zh: '已规划，暂未开放下载。' },
    tone: 'muted',
  },
  unavailable: {
    label: { en: 'Unavailable', zh: '暂不可用' },
    description: { en: 'Not supported in the current release wave.', zh: '当前发布阶段暂不支持。' },
    tone: 'danger',
  },
};

export function getReleaseStatusMeta(status: ReleaseStatus, locale: Locale): ReleaseStatusMeta {
  const copy = releaseStatusCopy[status];
  return {
    label: pickLocaleText(locale, copy.label),
    description: pickLocaleText(locale, copy.description),
    tone: copy.tone,
  };
}

export function formatReleaseBadge(artifact: ReleaseArtifact, locale: Locale): string {
  const meta = getReleaseStatusMeta(artifact.status, locale);
  const version = artifact.version ? ` · ${artifact.version}` : '';
  return `${artifact.platform} · ${artifact.arch} · ${meta.label}${version}`;
}

export function pickRecommendedArtifact(artifacts: ReleaseArtifact[], platformKey: string): ReleaseArtifact {
  return (
    artifacts.find((artifact) => artifact.recommendedPlatforms.includes(platformKey)) ??
    artifacts.find((artifact) => artifact.status === 'stable') ??
    artifacts[0]
  );
}
