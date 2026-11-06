import { useEffect, useState } from 'react';
import type { BotsApi, BotUi, BotSkillUi } from '../../shared/bots';
import type { BotPackagePreview } from '../../shared/botMarket';

/** Publication is an explicit snapshot review, independent of the Bot's private configuration. */
export function BotMarketPanel({
  api,
  bot,
  onInstalled,
}: {
  api: BotsApi;
  bot: BotUi;
  onInstalled(bot: BotUi): void;
}) {
  const [copyIntent, setCopyIntent] = useState(() => crypto.randomUUID());
  const [skills, setSkills] = useState<BotSkillUi[]>([]),
    [selected, setSelected] = useState<string[]>([]);
  const [version, setVersion] = useState('1.0.0'),
    [license, setLicense] = useState(''),
    [licenseText, setLicenseText] = useState('');
  const [resources, setResources] = useState(''),
    [examples, setExamples] = useState('');
  const [preview, setPreview] = useState<BotPackagePreview>(),
    [confirmed, setConfirmed] = useState(false);
  const [summary, setSummary] = useState(bot.description.slice(0, 160)),
    [busy, setBusy] = useState(false),
    [error, setError] = useState(''),
    [status, setStatus] = useState('');
  useEffect(() => {
    let live = true;
    void api.skills.list(bot.botId).then(
      (value) => {
        if (live) setSkills(value);
      },
      (cause) => {
        if (live) setError(String(cause));
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
    <section className="bots-panel" aria-label="Bot package and market">
      <div className="bots-detail-actions">
        <button
          type="button"
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
          onClick={() =>
            void perform(async () => {
              await api.market({ action: 'manage' });
            })
          }
        >
          My publications
        </button>
      </div>
      {bot.marketOrigin ? (
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
      ) : null}
      {bot.marketOrigin ? (
        <p className="bots-hint">
          Installed release {bot.marketOrigin.releaseId}. Configure your model
          in General and authorize your own read-only resources in the Team
          publication flow before sharing.
        </p>
      ) : null}
      <p className="bots-hint">
        Publish a reviewed role and selected text Skills. You choose the license
        and confirm the actual contents. Private memory, conversations, model
        credentials and resource bindings stay in your Bot.
      </p>
      {!preview ? (
        <button
          type="button"
          disabled={busy}
          onClick={() =>
            void perform(async () => {
              const recovered = await api.market({
                action: 'recover',
                botId: bot.botId,
              });
              if (recovered && 'manifest' in recovered) {
                setPreview(recovered);
                setConfirmed(false);
              } else
                setStatus(
                  'No prepared publication is available for this account.',
                );
            })
          }
        >
          Recover reviewed publication
        </button>
      ) : null}
      {!preview ? (
        <form
          onSubmit={(event) => {
            event.preventDefault();
            void prepare();
          }}
        >
          <label>
            Version
            <input
              required
              value={version}
              onChange={(event) => setVersion(event.target.value)}
              disabled={busy}
            />
          </label>
          <label>
            License identifier
            <input
              required
              placeholder="LicenseRef-Author-Defined"
              value={license}
              onChange={(event) => setLicense(event.target.value)}
              disabled={busy}
            />
          </label>
          <label>
            License text
            <textarea
              required
              value={licenseText}
              onChange={(event) => setLicenseText(event.target.value)}
              disabled={busy}
            />
          </label>
          <fieldset disabled={busy}>
            <legend>Explicitly included text Skills</legend>
            {skills.length ? (
              skills.map((skill) => (
                <label key={skill.name}>
                  <input
                    type="checkbox"
                    checked={selected.includes(skill.name)}
                    onChange={(event) =>
                      setSelected((value) =>
                        event.target.checked
                          ? [...value, skill.name]
                          : value.filter((name) => name !== skill.name),
                      )
                    }
                  />
                  {skill.name}
                </label>
              ))
            ) : (
              <p className="bots-muted">No Skills selected.</p>
            )}
          </fieldset>
          <label>
            Required read-only resources (public aliases, one per line)
            <textarea
              placeholder="travel-notes"
              value={resources}
              onChange={(event) => setResources(event.target.value)}
              disabled={busy}
            />
          </label>
          <label>
            Examples (one per line)
            <textarea
              value={examples}
              onChange={(event) => setExamples(event.target.value)}
              disabled={busy}
            />
          </label>
          <button type="submit" disabled={busy}>
            Review package contents
          </button>
        </form>
      ) : (
        <>
          <p>
            {preview.manifest.name} · {preview.manifest.version} ·{' '}
            {preview.manifest.license}
          </p>
          {preview.files.map((file) => (
            <details key={file.path}>
              <summary>{file.path}</summary>
              <pre style={{ whiteSpace: 'pre-wrap', overflowWrap: 'anywhere' }}>
                {file.text}
              </pre>
            </details>
          ))}
          <label>
            <input
              type="checkbox"
              checked={confirmed}
              onChange={(event) => setConfirmed(event.target.checked)}
              disabled={busy}
            />
            I reviewed every file, removed private information, and have
            permission to distribute these contents under the selected license.
          </label>
          <label>
            Market summary
            <input
              required
              value={summary}
              maxLength={160}
              onChange={(event) => setSummary(event.target.value)}
              disabled={busy}
            />
          </label>
          <div className="bots-detail-actions">
            <button
              type="button"
              disabled={busy}
              onClick={() => {
                setPreview(undefined);
                setConfirmed(false);
              }}
            >
              Change selection
            </button>
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
        </>
      )}
      {busy ? <p role="status">Preparing Bot package…</p> : null}
      {status ? <p role="status">{status}</p> : null}
      {error ? (
        <p role="alert" className="bots-problem">
          {error}
        </p>
      ) : null}
    </section>
  );
}
