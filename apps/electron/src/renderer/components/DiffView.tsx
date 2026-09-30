/**
 * Renders a {@link StructuredDiffDto} — the engine's pre-derived diff.
 *
 * Geometry is copied from the (now dead) `RightPanel` mock diff, which had the
 * proportions right: a right-aligned, unselectable line-number gutter, a narrow
 * sigil column, and `white-space: pre` content.
 *
 * ## Three things the wire deliberately does NOT carry
 *
 * 1. **String offsets.** Rows arrive PRE-SPLIT into segments. Rust indexes by
 *    UTF-8 byte, JS by UTF-16 code unit, Swift by grapheme — a `(start, end)`
 *    pair would mis-slice every CJK line. Concatenating `segments[].text`
 *    reproduces the row exactly, so we only ever APPEND segments in order and
 *    never index into a string.
 * 2. **Usable colors.** `CodeSegmentDto.rgb` is the terminal's resolved
 *    foreground, baked against ONE dark theme; on the light palette it is
 *    unreadable and it cannot follow the theme toggle. We map `class` through
 *    {@link Tokens.syntax} instead, and honor `rgb` only for `plain` runs in
 *    dark mode — the single case where the baked color sits on the background
 *    it was computed for.
 * 3. **Backgrounds.** The terminal's add/remove tints are alpha-over-BLACK
 *    blends, valid only over a black terminal. We derive ours from `kind` and
 *    `emph` against the live surface.
 *
 * ## Performance
 * A diff can be hundreds of rows inside a transcript that repaints on every
 * streaming delta. So: theme-dependent colors are written ONCE as CSS custom
 * properties on the root and every child references `var(...)`, and all
 * per-kind / per-class style objects are module-level frozen constants. A row
 * allocates a style object only for the rare bold/italic/underline segment.
 */

import { memo, useMemo, type CSSProperties } from 'react';
import type {
  CodeSegmentDto,
  DiffLineKindDto,
  DiffRowDto,
  StructuredDiffDto,
  SyntaxClassDto,
} from '@lingxi/bridge-client';

import { useT } from '../theme/ThemeContext';
import { ltrAnchored } from './bidi';

// ── Module-level frozen styles (allocated once, at import) ───────────────────

/**
 * A frozen table with NO prototype.
 *
 * Every table below is indexed by a WIRE value — `segment.class`, `row.kind` —
 * and `Object.freeze({ … })` keeps `Object.prototype`, so a class named
 * `constructor` or `toString` resolves to an inherited FUNCTION instead of
 * falling through to the `??` default. React would then be handed a function
 * as a style object. Null-prototype tables make the fallbacks real.
 */
function lookupTable<T extends object>(entries: T): Readonly<T> {
  return Object.freeze(Object.assign(Object.create(null) as T, entries));
}

const SYNTAX_CLASSES: readonly SyntaxClassDto[] = [
  'plain', 'keyword', 'type_name', 'function', 'string_lit', 'number',
  'comment', 'punctuation', 'operator', 'variable', 'constant', 'attribute',
];

/** One frozen, prototype-less class→style table from a per-class factory. */
function syntaxTable(style: (cls: SyntaxClassDto) => CSSProperties): Readonly<Record<SyntaxClassDto, CSSProperties>> {
  return lookupTable(
    Object.fromEntries(SYNTAX_CLASSES.map((cls) => [cls, Object.freeze(style(cls))])),
  ) as Readonly<Record<SyntaxClassDto, CSSProperties>>;
}

/** `{ color: var(--syn-keyword) }` for each class — one object per class. */
const SEGMENT_STYLE = syntaxTable((cls) => ({ color: `var(--syn-${cls})` }));

/** The same, plus the word-diff emphasis background for an ADDED row. */
const SEGMENT_STYLE_EMPH_ADD = syntaxTable((cls) => ({
  color: `var(--syn-${cls})`, background: 'var(--diff-emph-add)', borderRadius: 2,
}));

/** The same, for a REMOVED row. */
const SEGMENT_STYLE_EMPH_REMOVE = syntaxTable((cls) => ({
  color: `var(--syn-${cls})`, background: 'var(--diff-emph-del)', borderRadius: 2,
}));

const ROW_STYLE: Readonly<Record<DiffLineKindDto, CSSProperties>> = lookupTable({
  add: Object.freeze({ display: 'flex', background: 'var(--diff-add-bg)' }),
  remove: Object.freeze({ display: 'flex', background: 'var(--diff-del-bg)' }),
  context: Object.freeze({ display: 'flex', background: 'transparent' }),
});

const SIGIL_STYLE: Readonly<Record<DiffLineKindDto, CSSProperties>> = lookupTable({
  add: Object.freeze({ width: 16, flexShrink: 0, fontWeight: 600, color: 'var(--diff-add)', userSelect: 'none' }),
  remove: Object.freeze({ width: 16, flexShrink: 0, fontWeight: 600, color: 'var(--diff-del)', userSelect: 'none' }),
  context: Object.freeze({ width: 16, flexShrink: 0, fontWeight: 600, color: 'var(--diff-gutter)', userSelect: 'none' }),
});

const SIGIL: Readonly<Record<DiffLineKindDto, string>> = lookupTable({
  add: '+',
  remove: '−',
  context: ' ',
});

const CONTENT_STYLE: Readonly<Record<DiffLineKindDto, CSSProperties>> = lookupTable({
  add: Object.freeze({ whiteSpace: 'pre', paddingRight: 14, minWidth: 0 }),
  remove: Object.freeze({ whiteSpace: 'pre', paddingRight: 14, minWidth: 0 }),
  // Context rows recede so the changed rows read first.
  context: Object.freeze({ whiteSpace: 'pre', paddingRight: 14, minWidth: 0, opacity: 0.72 }),
});

/**
 * A row's kind, likewise off the wire. An unknown (or prototype-shaped) kind
 * renders as CONTEXT rather than crashing the whole transcript.
 */
function rowKind(kind: DiffLineKindDto): DiffLineKindDto {
  return ROW_STYLE[kind] === undefined ? 'context' : kind;
}

const HUNK_SEPARATOR_STYLE: CSSProperties = Object.freeze({
  color: 'var(--diff-gutter)',
  padding: '2px 0 2px 10px',
  userSelect: 'none',
  letterSpacing: 2,
});

const TRUNCATED_STYLE: CSSProperties = Object.freeze({
  color: 'var(--diff-gutter)',
  padding: '3px 0 1px 10px',
  fontStyle: 'italic',
});

const HEADER_STYLE: CSSProperties = Object.freeze({
  display: 'flex',
  alignItems: 'center',
  gap: 8,
  padding: '4px 12px 6px',
  fontSize: 11,
  fontWeight: 600,
});

const ROOT_STYLE: CSSProperties = Object.freeze({
  borderRadius: 8,
  overflow: 'hidden',
  fontSize: 11.5,
  lineHeight: 1.55,
});

const SCROLLER_STYLE: CSSProperties = Object.freeze({
  overflowX: 'auto',
  overflowY: 'auto',
  maxHeight: 420,
  padding: '4px 0 6px',
});

// ── Segment ─────────────────────────────────────────────────────────────────

/**
 * Pick the style for one segment.
 *
 * `rgb` is the LAST resort and only where it is honest: a `plain` run in dark
 * mode. Everything else goes through the class palette so it follows the theme.
 */
function segmentStyle(
  segment: CodeSegmentDto,
  kind: DiffLineKindDto,
  dark: boolean,
): CSSProperties {
  const table = segment.emph
    ? (kind === 'remove' ? SEGMENT_STYLE_EMPH_REMOVE : SEGMENT_STYLE_EMPH_ADD)
    : SEGMENT_STYLE;
  // `table` has no prototype, so an unknown class really is `undefined` here.
  const base = table[segment.class] ?? table.plain;

  const useRgb = dark && segment.class === 'plain' && typeof segment.rgb === 'number';
  const styled = segment.bold || segment.italic || segment.underline || useRgb;
  if (!styled) return base;

  const extra: CSSProperties = { ...base };
  if (useRgb) extra.color = packedRgbToCss(segment.rgb as number);
  if (segment.bold) extra.fontWeight = 700;
  if (segment.italic) extra.fontStyle = 'italic';
  if (segment.underline) extra.textDecoration = 'underline';
  return extra;
}

/** `0x00RRGGBB` → `#rrggbb`. */
export function packedRgbToCss(rgb: number): string {
  return `#${(rgb & 0xff_ff_ff).toString(16).padStart(6, '0')}`;
}

// ── Row ─────────────────────────────────────────────────────────────────────

interface DiffRowProps {
  row: DiffRowDto;
  gutterWidth: number;
  dark: boolean;
}

const DiffRow = memo(function DiffRow({ row, gutterWidth, dark }: DiffRowProps) {
  const kind = rowKind(row.kind);
  return (
    <div style={ROW_STYLE[kind]}>
      <span
        style={{
          // `gutter_width` is computed across ALL hunks so the column does not
          // jitter between them — use it rather than measuring per row.
          width: `calc(${Math.max(2, gutterWidth)}ch + 16px)`,
          minWidth: 38,
          textAlign: 'right',
          padding: '0 8px',
          flexShrink: 0,
          userSelect: 'none',
          color: 'var(--diff-gutter)',
        }}
      >
        {row.line_no}
      </span>
      <span style={SIGIL_STYLE[kind]}>{SIGIL[kind]}</span>
      <span style={CONTENT_STYLE[kind]}>
        {row.segments.map((segment, i) => (
          // Segments are pre-split and positional; the index IS their identity
          // and the array is replaced wholesale, so an index key is correct
          // here in a way it never is for the transcript's item list.
          <span key={i} style={segmentStyle(segment, kind, dark)}>
            {segment.text}
          </span>
        ))}
        {row.segments.length === 0 && ' '}
      </span>
    </div>
  );
});

// ── DiffView ────────────────────────────────────────────────────────────────

export interface DiffViewProps {
  diff: StructuredDiffDto;
}

export const DiffView = memo(function DiffView({ diff }: DiffViewProps) {
  const t = useT();

  // Every theme-dependent color is written ONCE here; the frozen child styles
  // above reference these variables and therefore never need re-creating.
  // Memoized on the palette so a diff repainting mid-stream does not rebuild
  // twenty custom properties per frame.
  const themeVars = useMemo(() => ({
    '--diff-add': t.add,
    '--diff-del': t.del,
    '--diff-add-bg': `color-mix(in oklab, ${t.add} 10%, transparent)`,
    '--diff-del-bg': `color-mix(in oklab, ${t.del} 11%, transparent)`,
    '--diff-emph-add': `color-mix(in oklab, ${t.add} 26%, transparent)`,
    '--diff-emph-del': `color-mix(in oklab, ${t.del} 26%, transparent)`,
    '--diff-gutter': t.text4,
    background: t.windowBg,
    border: `0.5px solid ${t.border}`,
    ...Object.fromEntries(
      (Object.keys(t.syntax) as SyntaxClassDto[]).map((cls) => [`--syn-${cls}`, t.syntax[cls]]),
    ),
    ...ROOT_STYLE,
  } as CSSProperties), [t]);

  return (
    <div className="mono-code" style={themeVars}>
      {(diff.file_path || diff.additions > 0 || diff.removals > 0) && (
        <div style={{ ...HEADER_STYLE, borderBottom: `0.5px solid ${t.border}`, color: t.text3 }}>
          {diff.file_path && (
            <span
              style={{
                flex: 1, minWidth: 0, overflow: 'hidden', textOverflow: 'ellipsis',
                whiteSpace: 'nowrap', direction: 'rtl', textAlign: 'left',
              }}
            >
              {/* `direction: rtl` clips the head of the path — and, unfenced,
                  also moves its leading `/` to the far right. See `bidi.ts`. */}
              {ltrAnchored(diff.file_path)}
            </span>
          )}
          {diff.additions > 0 && <span style={{ color: t.add }}>+{diff.additions}</span>}
          {diff.removals > 0 && <span style={{ color: t.del }}>−{diff.removals}</span>}
        </div>
      )}
      <div style={SCROLLER_STYLE}>
        {diff.rows.map((row, i) => {
          const previous = i > 0 ? diff.rows[i - 1] : undefined;
          // A hunk index CHANGE is where the elision marker belongs. There is
          // no separator row kind on the wire, by design.
          const separator = previous !== undefined && previous.hunk !== row.hunk;
          return (
            <div key={`${row.hunk}:${row.kind}:${row.line_no}:${i}`}>
              {separator && <div style={HUNK_SEPARATOR_STYLE}>⋯</div>}
              <DiffRow row={row} gutterWidth={diff.gutter_width} dark={t.dark} />
            </div>
          );
        })}
        {diff.truncated_rows > 0 && (
          <div style={TRUNCATED_STYLE}>
            … {diff.truncated_rows} more {diff.truncated_rows === 1 ? 'line' : 'lines'}
          </div>
        )}
      </div>
    </div>
  );
});
