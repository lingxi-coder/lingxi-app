import type { BotUi } from './bots.js';
export const CH_BOTS_MARKET = 'lingxi:bots:market';
export interface BotPackageFile {
  path: string;
  sizeBytes: number;
  sha256: string;
}
export interface BotPackageManifest {
  schema: 1;
  kind: 'bot';
  name: string;
  version: string;
  license: string;
  licenseFile: string;
  entry: string;
  skills: { path: string }[];
  resources: { id: string; binding: 'installer'; required: boolean }[];
  capabilities: {
    tool: 'shared_resource.search' | 'chat.read_messages';
    operations: ['read'];
  }[];
  runtime: {
    minimumBotPackageVersion: 1;
    requiresTeamExecutionVersion: 2;
    hostingTargets: ('macos' | 'windows' | 'linux')[];
    memberClients: ('web' | 'desktop' | 'ios' | 'android')[];
  };
  examples: string[];
  files: BotPackageFile[];
}
export interface BotExportSelection {
  botId: string;
  expectedRevision: number;
  version: string;
  license: string;
  licenseText: string;
  skills: string[];
  resources: { id: string; required: boolean }[];
  examples: string[];
}
export interface BotPackagePreview {
  id: string;
  manifest: BotPackageManifest;
  files: { path: string; text: string }[];
  digest: string;
}
export interface BotMarketPublication {
  name: string;
  summary: string;
  description: string;
  category: string;
  notes: string;
}
export type BotMarketRequest =
  | { action: 'browse' | 'manage' }
  | { action: 'recover'; botId: string }
  | { action: 'prepare'; selection: BotExportSelection }
  | {
      action: 'publish';
      previewId: string;
      digest: string;
      confirmed: true;
      metadata: BotMarketPublication;
    }
  | { action: 'export'; previewId: string; digest: string; confirmed: true }
  | {
      action: 'install';
      listingId: string;
      versionId?: string;
      copyIntent?: string;
      intentId?: string;
    };
export type BotMarketResponse =
  | void
  | BotPackagePreview
  | BotUi
  | null
  | { botId: string; versionId: string; status: string };
export interface BotMarketOrigin {
  market: string;
  accountId: string;
  listingId: string;
  releaseId: string;
  digest: string;
  installIntent: string;
}
