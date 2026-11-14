import { useState } from 'react';
import type { BotUpgradeFile, BotUpgradePlan } from '../../shared/botMarket';

export type BotUpgradeChoices = Record<string, 'mine' | 'theirs'>;

/** What a file of the plan is called and what happens to it, as the owner reads it. */
export function upgradeFileLine(file: BotUpgradeFile): string {
  const what = file.kind === 'role' ? 'Role' : `Skill “${file.name}”`;
  switch (file.state) {
    case 'update':
      return `${what}: updated to the new release`;
    case 'add':
      return `${what}: added by the new release`;
    case 'remove':
      return `${what}: removed by the new release`;
    case 'kept':
      return `${what}: you changed it and the release did not. Yours stays`;
    case 'conflict':
      return `${what}: you and the new release both changed it`;
  }
}

/** Whether the plan can be carried out with these answers: a choice for every conflict, and agreement where the release reads more. */
export function upgradeReady(
  plan: BotUpgradePlan,
  choices: BotUpgradeChoices,
  approved: boolean,
): boolean {
  return (
    plan.files.every((f) => f.state !== 'conflict' || choices[f.key] !== undefined) &&
    (!plan.expands || approved)
  );
}

/**
 * The review of a newer release of an installed Bot. Nothing is changed by looking. The owner chooses for each file both
 * sides changed, agrees to anything the release would read in addition, and then updates, or closes the review and the
 * Bot stays as it is.
 */
export function BotUpgradeReview({
  plan,
  busy,
  onUpgrade,
  onDecline,
}: {
  plan: BotUpgradePlan;
  busy: boolean;
  onUpgrade(choices: BotUpgradeChoices, approveAccess: boolean): void;
  onDecline(): void;
}) {
  const [choices, setChoices] = useState<BotUpgradeChoices>({});
  const [approved, setApproved] = useState(false);
  const text = (value: string | null | undefined) =>
    value === null || value === undefined ? '(not in this version)' : value;
  return (
    <section className="bots-panel" aria-label="New release">
      <h3>
        Version {plan.to.version} is available (this Bot is on {plan.from.version})
      </h3>
      {plan.files.length === 0 ? (
        <p className="bots-hint">No role or Skill of this Bot changes.</p>
      ) : (
        <ul className="bots-hint-list">
          {plan.files.map((file) => (
            <li key={file.key}>
              {upgradeFileLine(file)}
              {file.state === 'conflict' ? (
                <fieldset className="bots-fields" disabled={busy}>
                  <label>
                    <input
                      type="radio"
                      name={`upgrade-${file.key}`}
                      checked={choices[file.key] === 'mine'}
                      onChange={() => setChoices({ ...choices, [file.key]: 'mine' })}
                    />{' '}
                    Keep mine
                  </label>
                  <pre className="bots-hint">{text(file.mine)}</pre>
                  <label>
                    <input
                      type="radio"
                      name={`upgrade-${file.key}`}
                      checked={choices[file.key] === 'theirs'}
                      onChange={() => setChoices({ ...choices, [file.key]: 'theirs' })}
                    />{' '}
                    Use the new release’s
                  </label>
                  <pre className="bots-hint">{text(file.theirs)}</pre>
                </fieldset>
              ) : null}
            </li>
          ))}
        </ul>
      )}
      {plan.license ? (
        <p className="bots-hint">
          License changes from {plan.license.from} to {plan.license.to}.
        </p>
      ) : null}
      {plan.tools.removed.length || plan.resources.removed.length ? (
        <p className="bots-hint">
          No longer used: {[...plan.tools.removed, ...plan.resources.removed].join(', ')}.
        </p>
      ) : null}
      {plan.resources.nowRequired.length ? (
        <p className="bots-hint">
          Now required before sharing: {plan.resources.nowRequired.join(', ')}.
        </p>
      ) : null}
      {plan.expands ? (
        <label className="bots-field">
          <span>
            The new release reads more than this Bot does now:{' '}
            {[...plan.tools.added, ...plan.resources.added].join(', ')}. Nothing is authorized by updating; you still
            choose your own resources when you share the Bot.
          </span>
          <span>
            <input
              type="checkbox"
              checked={approved}
              disabled={busy}
              onChange={(event) => setApproved(event.target.checked)}
            />{' '}
            I agree to this Bot using them
          </span>
        </label>
      ) : null}
      <p className="bots-hint">
        Conversations, memory, your model, your own Skills and what you already shared with a Team stay as they are. A
        Team keeps the version it was shared with until you share the Bot again.
      </p>
      <div className="bots-detail-actions">
        <button
          type="button"
          disabled={busy || !upgradeReady(plan, choices, approved)}
          onClick={() => onUpgrade(choices, approved)}
        >
          Update to {plan.to.version}
        </button>
        <button type="button" disabled={busy} onClick={onDecline}>
          Stay on {plan.from.version}
        </button>
      </div>
    </section>
  );
}
