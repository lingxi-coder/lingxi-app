/**
 * A Bot's skills, and the owner's own that can be copied in. A Bot has only the
 * skills it was given; "import" copies one once, and nothing follows the
 * original afterwards.
 */

import type { BotSkillUi, BotsApi } from '../../shared/bots';
import { ipcErrorMessage } from './botForm';
import { ObservableState } from './observableState';

/** What the owner could still copy in: their skills, less the names the Bot already has. */
export function importCandidates(importable: readonly BotSkillUi[], present: readonly BotSkillUi[]): BotSkillUi[] {
  const names = new Set(present.map((skill) => skill.name));
  return importable.filter((skill) => !names.has(skill.name));
}

export interface BotSkillsState {
  /** Unknown until the first read. */
  skills: readonly BotSkillUi[] | undefined;
  loading: boolean;
  error: string | undefined;
  /** Set while the owner is choosing a skill to copy in. */
  importing: { candidates: readonly BotSkillUi[] | undefined; busy: string | undefined; error: string | undefined } | undefined;
  /** The skill the owner was asked whether to delete. */
  deleting: { name: string; busy: boolean; error: string | undefined } | undefined;
}

export class BotSkillsController extends ObservableState<BotSkillsState> {
  private request = 0;

  constructor(private readonly api: BotsApi['skills'], private readonly botId: string) {
    super({ skills: undefined, loading: false, error: undefined, importing: undefined, deleting: undefined });
  }

  async load(): Promise<void> {
    const request = ++this.request;
    this.update({ loading: true });
    try {
      const skills = await this.api.list(this.botId);
      if (request === this.request) this.update({ skills, error: undefined, loading: false });
    } catch (error) {
      if (request === this.request) this.update({ error: ipcErrorMessage(error), loading: false });
    }
  }

  async openImport(): Promise<void> {
    this.update({ importing: { candidates: undefined, busy: undefined, error: undefined } });
    try {
      const [importable, present] = await Promise.all([this.api.listImportable(), this.api.list(this.botId)]);
      if (this.getSnapshot().importing) this.update({ skills: present, importing: { candidates: importCandidates(importable, present), busy: undefined, error: undefined } });
    } catch (error) {
      if (this.getSnapshot().importing) this.update({ importing: { candidates: [], busy: undefined, error: ipcErrorMessage(error) } });
    }
  }

  closeImport(): void {
    if (!this.getSnapshot().importing?.busy) this.update({ importing: undefined });
  }

  async importSkill(name: string): Promise<void> {
    const { importing } = this.getSnapshot();
    if (!importing?.candidates || importing.busy) return;
    this.update({ importing: { ...importing, busy: name, error: undefined } });
    try {
      await this.api.importFromUser(this.botId, name);
    } catch (error) {
      this.update({ importing: { ...importing, busy: undefined, error: ipcErrorMessage(error) } });
      return;
    }
    this.update({ importing: { candidates: importing.candidates.filter((skill) => skill.name !== name), busy: undefined, error: undefined } });
    await this.load();
  }

  askDelete(name: string): void {
    this.update({ deleting: { name, busy: false, error: undefined } });
  }

  cancelDelete(): void {
    if (!this.getSnapshot().deleting?.busy) this.update({ deleting: undefined });
  }

  async confirmDelete(): Promise<void> {
    const { deleting } = this.getSnapshot();
    if (!deleting || deleting.busy) return;
    this.update({ deleting: { ...deleting, busy: true, error: undefined } });
    try {
      await this.api.delete(this.botId, deleting.name);
    } catch (error) {
      this.update({ deleting: { ...deleting, busy: false, error: ipcErrorMessage(error) } });
      return;
    }
    this.update({ deleting: undefined });
    await this.load();
  }
}
