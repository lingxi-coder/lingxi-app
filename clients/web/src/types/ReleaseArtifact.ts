export type ReleaseStatus = 'stable' | 'beta' | 'coming-soon' | 'unavailable';

export type ReleaseSurface = 'desktop' | 'mobile' | 'cli';

export interface ReleaseArtifact {
  id: string;
  title: string;
  surface: ReleaseSurface;
  platform: string;
  arch: string;
  status: ReleaseStatus;
  version?: string;
  summary: string;
  note: string;
  installCommand?: string;
  recommendedPlatforms: string[];
}
