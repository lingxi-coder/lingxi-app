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
    }
  | { action: 'checkUpgrade'; botId: string }
  | {
      action: 'upgrade';
      botId: string;
      expectedRevision: number;
      releaseId: string;
      /** The reviewed plan's `digest`: the upgrade is refused if anything it was made from has changed since. */
      plan: string;
      /** One choice per file in conflict, by its `key`. */
      choices: Record<string, 'mine' | 'theirs'>;
      /** The owner agreed to the tools and resources the new release adds. */
      approveAccess: boolean;
    };
/**
 * What a newer release of an installed Bot's listing would do to it:
 * `update`/`add`/`remove` — the release changed a file the owner left as installed, and the Bot follows;
 * `kept` — the owner changed a file the release did not, and it stays as it is;
 * `conflict` — both changed it differently. Nothing happens until the owner chooses, file by file.
 */
export interface BotUpgradeFile {
  key: string;
  kind: 'role' | 'skill';
  name: string;
  state: 'update' | 'add' | 'remove' | 'kept' | 'conflict';
  /** For a conflict: the Bot's text now and the release's. `null`: the file is not there. */
  mine?: string | null;
  theirs?: string | null;
}
export interface BotUpgradePlan {
  kind: 'upgrade';
  botId: string;
  expectedRevision: number;
  from: { releaseId: string; version: string };
  to: { releaseId: string; version: string; name: string };
  /** Identifies everything the plan was made from: both releases, the Bot's files now, and the outcome per file. */
  digest: string;
  files: BotUpgradeFile[];
  license?: { from: string; to: string };
  tools: { added: string[]; removed: string[] };
  resources: { added: string[]; removed: string[]; nowRequired: string[] };
  /** The new release reads more than the installed one (a tool or a resource was added): the owner must agree to it. */
  expands: boolean;
}
export type BotMarketResponse =
  | void
  | BotPackagePreview
  | BotUpgradePlan
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
