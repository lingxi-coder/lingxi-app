import { createHash } from 'node:crypto';
import { mkdirSync } from 'node:fs';
import { join } from 'node:path';
import { DatabaseSync } from 'node:sqlite';

export const DELIVERY_RETENTION_MS = 7 * 24 * 60 * 60 * 1000;
export interface DeliveryLedger {
  has(key: string, now: number): boolean;
  remember(keys: readonly string[], now: number): void;
}

/** The user-data directory belongs to one installation. Only salted-by-account event hashes are stored. */
export class NotificationDeliveryLedger implements DeliveryLedger {
  private db?: DatabaseSync;
  private nextCleanup = 0;
  constructor(
    private readonly userData: string,
    private readonly account: () => string | null,
  ) {}
  private database(): DatabaseSync {
    if (!this.db) {
      const dir = join(this.userData, 'inbox');
      mkdirSync(dir, { recursive: true, mode: 0o700 });
      this.db = new DatabaseSync(join(dir, 'delivery.sqlite'));
      this.db.exec(
        'CREATE TABLE IF NOT EXISTS delivered (event TEXT PRIMARY KEY, time INTEGER NOT NULL); CREATE INDEX IF NOT EXISTS by_time ON delivered(time)',
      );
    }
    return this.db;
  }
  private hash(key: string): string {
    return createHash('sha256')
      .update(JSON.stringify([this.account(), key]))
      .digest('hex');
  }
  has(key: string, now: number): boolean {
    return !!this.database()
      .prepare('SELECT 1 FROM delivered WHERE event=? AND time>?')
      .get(this.hash(key), now - DELIVERY_RETENTION_MS);
  }
  remember(keys: readonly string[], now: number): void {
    const db = this.database();
    db.exec('BEGIN');
    try {
      if (now >= this.nextCleanup) db.prepare('DELETE FROM delivered WHERE time<=?').run(now - DELIVERY_RETENTION_MS);
      const insert = db.prepare('INSERT OR REPLACE INTO delivered VALUES(?,?)');
      for (const key of keys) insert.run(this.hash(key), now);
      db.exec('COMMIT');
      if (now >= this.nextCleanup) this.nextCleanup = now + 60 * 60 * 1000;
    } catch (error) {
      db.exec('ROLLBACK');
      throw error;
    }
  }
  close(): void {
    this.db?.close();
    this.db = undefined;
  }
}
