import { useEffect, useId, useRef, useState } from 'react';
import type { BotsApi, BotUi, BotSkillUi } from '../../shared/bots';
import type {
  BotPackagePreview,
  BotUpgradePlan,
} from '../../shared/botMarket';
import { BotUpgradeReview } from './BotUpgradeReview';

/** Publication is an explicit snapshot review, independent of the Bot's private configuration. */
export function BotMarketPanel({
  api,
  bot,
  title,
  onInstalled,
}: {
  api: BotsApi;
  bot: BotUi;
  title: string;
  onInstalled(bot: BotUi): void;
}) {
  const [copyIntent, setCopyIntent] = useState(() => crypto.randomUUID());
  const [skills, setSkills] = useState<BotSkillUi[]>(),
    [selected, setSelected] = useState<string[]>([]);
  const [version, setVersion] = useState('1.0.0'),
    [license, setLicense] = useState(''),
    [licenseText, setLicenseText] = useState('');
  const [resources, setResources] = useState(''),
    [examples, setExamples] = useState('');
  const [preview, setPreview] = useState<BotPackagePreview>(),
    [confirmed, setConfirmed] = useState(false);
  const [upgrade, setUpgrade] = useState<BotUpgradePlan>();
  const [summary, setSummary] = useState(bot.description.slice(0, 160)),
    [busy, setBusy] = useState(false),
    [error, setError] = useState(''),
    [status, setStatus] = useState('');
  const id = useId();
  const stepHeading = useRef<HTMLHeadingElement>(null);
  const reviewing = preview !== undefined;
  const wasReviewing = useRef(reviewing);
  useEffect(() => {
    if (wasReviewing.current !== reviewing) stepHeading.current?.focus();
    wasReviewing.current = reviewing;
  }, [reviewing]);
  useEffect(() => {
    let live = true;
    void api.skills.list(bot.botId).then(
      (value) => {
        if (live) setSkills(value);
      },
      (cause) => {
        if (live) {
          setSkills([]);
          setError(String(cause));
        }
      },
    );
    return () => {
      live = false;
    };
  }, [api, bot.botId]);
  const perform = async (operation: () => Promise<void>) => {
    if (busy) return;
    setBusy(true);
    setError('');
    setStatus('');
    try {
      await operation();
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setBusy(false);
    }
  };
  const prepare = () =>
    perform(async () => {
      const result = await api.market({
        action: 'prepare',
        selection: {
          botId: bot.botId,
          expectedRevision: bot.revision,
          version,
          license,
          licenseText,
          skills: selected,
          resources: resources
            .split(/\r?\n/)
            .map((s) => s.trim())
            .filter(Boolean)
            .map((id) => ({ id, required: true })),
          examples: examples
            .split(/\r?\n/)
            .map((s) => s.trim())
            .filter(Boolean),
        },
      });
      if (!result || !('manifest' in result))
        throw new Error('Bot package preview unavailable');
      setPreview(result);
      setConfirmed(false);
    });
  const complete = (action: 'publish' | 'export') =>
    perform(async () => {
      if (!preview || !confirmed) return;
      const result = await api.market(
        action === 'export'
          ? {
              action,
              previewId: preview.id,
              digest: preview.digest,
              confirmed: true,
            }
          : {
              action,
              previewId: preview.id,
              digest: preview.digest,
              confirmed: true,
              metadata: {
                name: preview.manifest.name,
                summary,
                description: summary,
                category: 'tools',
                notes: `Bot ${preview.manifest.version}`,
              },
            },
      );
      setStatus(
        action === 'publish' && result && 'status' in result
          ? `Submitted: ${result.status}. Your release stays in your library until its audience is explicitly enabled.`
          : 'Reviewed Bot package export completed.',
      );
    });
  return (
    <section className="bots-market" aria-labelledby={`${id}-title`}>
      <header className="bots-market-header">
        <h2 className="bots-detail-section-title" id={`${id}-title`}>{title}</h2>
        <div className="bots-market-actions">
          <button
            type="button"
            disabled={busy}
            onClick={() =>
              void perform(async () => {
                await api.market({ action: 'browse' });
              })
            }
          >
            Browse Bot market
          </button>
          <button
            type="button"
            disabled={busy}
            onClick={() =>
              void perform(async () => {
                await api.market({ action: 'manage' });
              })
            }
          >
            My publications
          </button>
        </div>
        <p className="bots-detail-description">
          Package your Bot’s role and selected text Skills. Private memory,
          conversations, model credentials and resource bindings stay in your Bot.
        </p>
      </header>
      {bot.marketOrigin ? <section className="bots-market-card" aria-labelledby={`${id}-installed`}>
        <h3 id={`${id}-installed`}>Installed release</h3>
        <p className="bots-hint">
          {bot.marketOrigin.releaseId}. Configure your model in General and authorize
          your read-only resources in the Team publication flow before sharing.
        </p>
        <div className="bots-market-actions">
          <button
            type="button"
            disabled={busy}
            onClick={() =>
              void perform(async () => {
                const origin = bot.marketOrigin!;
                const result = await api.market({
                  action: 'install',
                  listingId: origin.listingId,
                  versionId: origin.releaseId,
                  copyIntent,
                });
                if (result && 'revision' in result) {
                  onInstalled(result);
                  setCopyIntent(crypto.randomUUID());
                  setStatus(
                    `Created ${result.name}. Choose its own bindings before sharing.`,
                  );
                }
              })
            }
          >
            Download as another personal Bot
          </button>
          {!upgrade ? (
            <button
              type="button"
              disabled={busy}
              onClick={() =>
                void perform(async () => {
                  const plan = await api.market({
                    action: 'checkUpgrade',
                    botId: bot.botId,
                  });
                  if (plan && 'kind' in plan && plan.kind === 'upgrade')
                    setUpgrade(plan);
                  else setStatus('This Bot is on the newest release.');
                })
              }
            >
              Check for a new release
            </button>
          ) : null}
        </div>
        {upgrade ? (
          <BotUpgradeReview
            plan={upgrade}
            busy={busy}
            onDecline={() => {
              setUpgrade(undefined);
              setStatus(
                `This Bot stays on version ${upgrade.from.version}. Nothing was changed.`,
              );
            }}
            onUpgrade={(choices, approveAccess) =>
              void perform(async () => {
                try {
                  const result = await api.market({
                    action: 'upgrade',
                    botId: upgrade.botId,
                    expectedRevision: upgrade.expectedRevision,
                    releaseId: upgrade.to.releaseId,
                    plan: upgrade.digest,
                    choices,
                    approveAccess,
                  });
                  if (result && 'revision' in result) onInstalled(result);
                  setUpgrade(undefined);
                  setStatus(`Updated to version ${upgrade.to.version}.`);
                } catch (cause) {
                  // The Bot or the release changed since the review: it is shown again, and nothing was written.
                  if (/BOT_UPGRADE_CHANGED/.test(String(cause))) {
                    setUpgrade(undefined);
                    throw new Error(
                      'This Bot or the release changed after you looked. Check for a new release again.',
                    );
                  }
                  throw cause;
                }
              })
            }
          />
        ) : null}
      </section> : null}
      {!preview ? (
        <form
          className="bots-market-form"
          onSubmit={(event) => {
            event.preventDefault();
            void prepare();
          }}
        >
          <div className="bots-market-grid">
            <section className="bots-market-card" aria-labelledby={`${id}-details`}>
              <h3 ref={stepHeading} tabIndex={-1} id={`${id}-details`}>Version &amp; license</h3>
              <label className="bots-field">
                <span className="bots-label">Version</span>
                <input name="version" required spellCheck={false} value={version} onChange={(event) => setVersion(event.target.value)} disabled={busy} />
              </label>
              <label className="bots-field">
                <span className="bots-label">License identifier</span>
                <input name="license" required spellCheck={false} placeholder="LicenseRef-Author-Defined" value={license} onChange={(event) => setLicense(event.target.value)} disabled={busy} />
              </label>
              <label className="bots-field">
                <span className="bots-label">License text</span>
                <textarea name="licenseText" rows={4} required value={licenseText} onChange={(event) => setLicenseText(event.target.value)} disabled={busy} />
              </label>
            </section>
            <section className="bots-market-card" aria-labelledby={`${id}-contents`}>
              <h3 id={`${id}-contents`}>Package contents</h3>
              <fieldset className="bots-market-skills" disabled={busy}>
                <legend className="bots-label">Text Skills to include</legend>
                {skills?.length ? (
                  <div className="bots-market-skill-list">
                    {skills.map((skill) => (
                      <label className="bots-market-option" key={skill.name}>
                        <input
                          type="checkbox"
                          name="skills"
                          value={skill.name}
                          checked={selected.includes(skill.name)}
                          onChange={(event) => {
                            const checked = event.target.checked;
                            setSelected((value) => checked ? [...value, skill.name] : value.filter((name) => name !== skill.name));
                          }}
                        />
                        <span>{skill.name}</span>
                      </label>
                    ))}
                  </div>
                ) : (
                  <p className="bots-muted">{skills ? 'No text Skills available.' : 'Loading Skills…'}</p>
                )}
              </fieldset>
              <div className="bots-field">
                <label className="bots-label" htmlFor={`${id}-resources`}>Required read-only resources</label>
                <textarea id={`${id}-resources`} name="resources" aria-describedby={`${id}-resources-hint`} rows={2} spellCheck={false} placeholder="travel-notes" value={resources} onChange={(event) => setResources(event.target.value)} disabled={busy} />
                <p className="bots-hint" id={`${id}-resources-hint`}>Enter one public alias per line.</p>
              </div>
              <div className="bots-field">
                <label className="bots-label" htmlFor={`${id}-examples`}>Examples</label>
                <textarea id={`${id}-examples`} name="examples" aria-describedby={`${id}-examples-hint`} rows={2} value={examples} onChange={(event) => setExamples(event.target.value)} disabled={busy} />
                <p className="bots-hint" id={`${id}-examples-hint`}>Enter one example per line.</p>
              </div>
            </section>
          </div>
          <footer className="bots-market-footer">
            <button
              type="button"
              className="bots-market-secondary"
              disabled={busy}
              onClick={() =>
                void perform(async () => {
                  const recovered = await api.market({ action: 'recover', botId: bot.botId });
                  if (recovered && 'manifest' in recovered) {
                    setPreview(recovered);
                    setConfirmed(false);
                  } else setStatus('No prepared publication is available for this account.');
                })
              }
            >
              Recover reviewed publication
            </button>
            <button type="submit" data-variant="primary" disabled={busy}>Review package contents</button>
          </footer>
        </form>
      ) : (
        <>
          <section className="bots-market-card" aria-labelledby={`${id}-review`}>
            <h3 ref={stepHeading} tabIndex={-1} id={`${id}-review`}>Review package contents</h3>
            <p className="bots-detail-description">
              {preview.manifest.name} · {preview.manifest.version} · {preview.manifest.license}
            </p>
            <div className="bots-market-files">
              {preview.files.map((file) => (
                <details key={file.path}>
                  <summary>{file.path}</summary>
                  <pre className="bots-memory-content">{file.text}</pre>
                </details>
              ))}
            </div>
            <label className="bots-market-option bots-market-confirm">
              <input
                type="checkbox"
                checked={confirmed}
                onChange={(event) => setConfirmed(event.target.checked)}
                disabled={busy}
              />
              <span>
                I reviewed every file, removed private information, and have permission
                to distribute these contents under the selected license.
              </span>
            </label>
            <label className="bots-field">
              <span className="bots-label">Market summary</span>
              <input
                required
                value={summary}
                maxLength={160}
                onChange={(event) => setSummary(event.target.value)}
                disabled={busy}
              />
            </label>
          </section>
          <footer className="bots-market-footer">
            <button
              type="button"
              className="bots-market-secondary"
              disabled={busy}
              onClick={() => {
                setPreview(undefined);
                setConfirmed(false);
              }}
            >
              Change selection
            </button>
            <div className="bots-market-actions">
              <button
                type="button"
                disabled={busy || !confirmed}
                onClick={() => void complete('export')}
              >
                Export ZIP
              </button>
              <button
                type="button"
                data-variant="primary"
                disabled={busy || !confirmed || !summary.trim()}
                onClick={() => void complete('publish')}
              >
                Publish to my library
              </button>
            </div>
          </footer>
        </>
      )}
      {busy ? <p role="status" className="bots-muted">Working…</p> : null}
      {status ? <p role="status" className="bots-muted">{status}</p> : null}
      {error ? (
        <p role="alert" className="bots-problem">
          {error}
        </p>
      ) : null}
    </section>
  );
}
