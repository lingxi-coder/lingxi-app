import { memo, type CSSProperties, type ReactNode } from 'react';

import {
  commandDiagnosticTone,
  commandName,
  commandPresentation,
  commandShouldCollapse,
  parseCommandHelp,
  parseCommandMetrics,
  type CommandRunItem,
} from '../model/runItem';
import { useT } from '../theme/ThemeContext';
import { Icon } from './Icon';

function PlainBody({ item }: { item: CommandRunItem }) {
  return <pre className="command-result-pre mono">{item.output}</pre>;
}

function HelpBody({ item }: { item: CommandRunItem }) {
  const entries = parseCommandHelp(item.output);
  if (entries.length === 0) return <PlainBody item={item} />;
  return (
    <div className="command-help-grid" role="list" aria-label="Available slash commands">
      {entries.map((entry) => (
        <div className="command-help-entry" role="listitem" key={entry.name}>
          <code className="mono">{entry.name}</code>
          <span>{entry.description}</span>
        </div>
      ))}
    </div>
  );
}

function MetricsBody({ item }: { item: CommandRunItem }) {
  const entries = parseCommandMetrics(item.output);
  if (entries.length < 2) return <PlainBody item={item} />;
  return (
    <dl className="command-metric-grid">
      {entries.map((entry, index) => (
        <div className="command-metric-entry" key={`${entry.label}-${index}`}>
          <dt>{entry.label}</dt>
          <dd className="mono">{entry.value}</dd>
        </div>
      ))}
    </dl>
  );
}

function DiagnosticsBody({ item }: { item: CommandRunItem }) {
  const lines = item.output.replace(/\r\n?/g, '\n').split('\n').filter((line) => line.trim());
  return (
    <div className="command-diagnostic-list" role="list">
      {lines.map((line, index) => {
        const tone = commandDiagnosticTone(line);
        return (
          <div className="command-diagnostic-entry" data-tone={tone} role="listitem" key={`${line}-${index}`}>
            <Icon name={tone === 'warning' ? 'shieldAlert' : tone === 'ok' ? 'check' : 'info'} size={14} stroke={1.8} />
            <span className="mono">{line}</span>
          </div>
        );
      })}
    </div>
  );
}

function CatalogBody({ item }: { item: CommandRunItem }) {
  const lines = item.output.replace(/\r\n?/g, '\n').split('\n').filter((line) => line.trim());
  return (
    <div className="command-catalog-list" role="list">
      {lines.map((line, index) => (
        <div className="command-catalog-entry" role="listitem" key={`${line}-${index}`}>
          <span className="command-catalog-index mono">{String(index + 1).padStart(2, '0')}</span>
          <span className="mono">{line}</span>
        </div>
      ))}
    </div>
  );
}

function ActionBody({ item }: { item: CommandRunItem }) {
  return <p className="command-action-copy">{item.output}</p>;
}

function CommandBody({ item }: { item: CommandRunItem }): ReactNode {
  switch (commandPresentation(item).kind) {
    case 'help': return <HelpBody item={item} />;
    case 'metrics': return <MetricsBody item={item} />;
    case 'diagnostics': return <DiagnosticsBody item={item} />;
    case 'catalog': return <CatalogBody item={item} />;
    case 'action': return <ActionBody item={item} />;
    case 'error':
    case 'plain':
    default: return <PlainBody item={item} />;
  }
}

export const CommandOutput = memo(function CommandOutput({ item, open, onSetOpen }: {
  item: CommandRunItem;
  open: boolean;
  onSetOpen(id: string, next: boolean): void;
}) {
  const t = useT();
  const presentation = commandPresentation(item);
  const collapsible = commandShouldCollapse(item);
  const expanded = !collapsible || open;
  const accent = presentation.tone === 'danger'
    ? t.danger
    : presentation.tone === 'warning'
      ? t.warn
      : presentation.tone === 'success'
        ? t.ok
        : presentation.tone === 'accent'
          ? t.accent
          : t.text3;
  const style = {
    '--command-accent': accent,
    '--command-surface': t.surface,
    '--command-surface-muted': t.surfaceHover,
    '--command-ring': t.border,
    '--command-text': t.text,
    '--command-text-muted': t.text3,
    '--command-danger': t.danger,
  } as CSSProperties;
  const label = item.name || (commandName(item) ? `/${commandName(item)}` : 'Slash command');
  return (
    <section
      className="command-result-card"
      data-command-kind={presentation.kind}
      data-command-name={commandName(item)}
      role={item.isError ? 'alert' : undefined}
      style={style}
    >
      <div className="command-result-header">
        <span className="command-result-icon" aria-hidden="true">
          <Icon name={presentation.icon} size={15} stroke={1.75} />
        </span>
        <div className="command-result-heading">
          <strong>{presentation.title}</strong>
          <span className="mono">{label}</span>
        </div>
        {collapsible && (
          <button
            type="button"
            className="command-result-toggle"
            aria-label={expanded ? `Collapse ${label} output` : `Expand ${label} output`}
            aria-expanded={expanded}
            aria-controls={`command-result-body-${item.id}`}
            onClick={() => onSetOpen(item.id, !expanded)}
          >
            <Icon name={expanded ? 'chevron' : 'chevronR'} size={14} stroke={1.9} />
          </button>
        )}
      </div>
      {expanded && (
        <div className="command-result-body" id={`command-result-body-${item.id}`}>
          <CommandBody item={item} />
        </div>
      )}
    </section>
  );
});
