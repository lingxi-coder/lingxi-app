import { formatTags, messageFormatParts, messageStyles, validMessageFormats, type MessageFormat } from './message-format.js';

export type EditorDraft = { text: string; formats: MessageFormat[] };
export type EditorSelection = { start: number; end: number };
type Point = { node: Node; offset: number };
const blockTags = new Set(['DIV', 'P']);

export function editorRange(editor: HTMLElement): Range | undefined {
  const selection = editor.ownerDocument.getSelection();
  const range = selection?.rangeCount ? selection.getRangeAt(0) : undefined;
  return range && editor.contains(range.commonAncestorContainer) ? range : undefined;
}

export function restoreEditorRange(editor: HTMLElement, range: Range): void {
  const selection = editor.ownerDocument.getSelection();
  selection?.removeAllRanges();
  selection?.addRange(range);
}

function nodeStyle(node: HTMLElement, style: number): number {
  const { fontWeight, fontStyle, textDecoration, textDecorationLine } = node.style;
  const decorations = (textDecorationLine || textDecoration).split(' ');
  if (node.matches('b,strong') || fontWeight === 'bold' || Number(fontWeight) >= 600) style |= messageStyles.bold;
  if (fontWeight === 'normal' || (fontWeight && Number(fontWeight) === 400)) style &= ~messageStyles.bold;
  if (node.matches('i,em') || fontStyle === 'italic') style |= messageStyles.italic;
  if (fontStyle === 'normal') style &= ~messageStyles.italic;
  if (node.matches('u') || decorations.includes('underline')) style |= messageStyles.underline;
  if (node.matches('s,strike,del') || decorations.includes('line-through')) style |= messageStyles.strikeThrough;
  if (decorations.includes('none')) style &= ~(messageStyles.underline | messageStyles.strikeThrough);
  return style;
}

/** Read browser-generated lines and styles without storing the editable DOM. */
export function readMessageEditor(editor: HTMLElement, range: Range | StaticRange | undefined = editorRange(editor)): EditorDraft & EditorSelection {
  return readEditableContents(editor, range);
}

function readEditableContents(editor: HTMLElement | DocumentFragment, range?: Range | StaticRange): EditorDraft & EditorSelection {
  const formats: MessageFormat[] = [];
  let text = '', start: number | undefined, end: number | undefined;
  const locate = (node: Node, offset: number) => {
    if (range?.startContainer === node && range.startOffset === offset) start = text.length;
    if (range?.endContainer === node && range.endOffset === offset) end = text.length;
  };
  const append = (value: string, style: number) => {
    const last = formats[formats.length - 1];
    if (value && style) {
      if (last && last.offset + last.length === text.length && last.style === style) last.length += value.length;
      else formats.push({ offset: text.length, length: value.length, style });
    }
    // Browsers use nonbreaking spaces to preserve runs of spaces during editing.
    text += value.replace(/\u00a0/gu, ' ');
  };
  const walk = (node: Node, style: number) => {
    if (node.nodeType === 3) {
      const value = node.nodeValue ?? '';
      if (range?.startContainer === node) start = text.length + range.startOffset;
      if (range?.endContainer === node) end = text.length + range.endOffset;
      append(value, style);
      return;
    }
    if (node.nodeType !== 1 && node.nodeType !== 11) return;
    let previousBlock = false;
    for (const [index, child] of Array.from(node.childNodes).entries()) {
      locate(node, index);
      const block = child instanceof HTMLElement && blockTags.has(child.tagName);
      if (index && (block || previousBlock)) append('\n', 0);
      if (child.nodeName === 'BR') {
        // A last BR is the browser's caret placeholder for the final line.
        if (child.nextSibling) append('\n', 0);
      } else {
        walk(child, child instanceof HTMLElement ? nodeStyle(child, style) : style);
      }
      previousBlock = block;
    }
    locate(node, node.childNodes.length);
  };
  walk(editor, 0);
  return { text, formats, start: start ?? text.length, end: end ?? start ?? text.length };
}

export function writeMessageEditor(editor: HTMLElement, { text, formats }: EditorDraft): void {
  const document = editor.ownerDocument;
  const content = document.createDocumentFragment();
  for (const part of messageFormatParts(text, formats)) {
    let node: Node = document.createTextNode(part.text);
    for (const [style, tag] of formatTags) {
      if (!(part.style & style)) continue;
      const wrapper = document.createElement(tag);
      wrapper.append(node);
      node = wrapper;
    }
    content.append(node);
  }
  if (text.endsWith('\n')) content.append(document.createElement('br'));
  editor.replaceChildren(content);
}

export function pasteMessageEditor(editor: HTMLElement, clipboard: DataTransfer): void {
  const text = clipboard.getData('text/plain').replace(/\r\n?/gu, '\n');
  if (!text) return;
  const document = editor.ownerDocument;
  const html = clipboard.getData('text/html');
  if (html) {
    const template = document.createElement('template');
    template.innerHTML = html;
    const draft = readEditableContents(template.content);
    if (draft.formats.length && draft.text === text && validMessageFormats(text, draft.formats)) {
      // Rebuild only the supported inline tags and escaped text. Clipboard HTML stays inert.
      const safe = document.createElement('div');
      writeMessageEditor(safe, draft);
      if (document.execCommand('insertHTML', false, safe.innerHTML)) return;
    }
  }
  document.execCommand('insertText', false, text);
}

/** Used after writing known runs. Native edits keep their original Range. */
export function placeEditorCaret(editor: HTMLElement, offset: number): void {
  const document = editor.ownerDocument;
  const walker = document.createTreeWalker(editor, document.defaultView?.NodeFilter.SHOW_TEXT);
  let point: Point = { node: editor, offset: editor.childNodes.length };
  for (let node = walker.nextNode(); node; node = walker.nextNode()) {
    const length = node.nodeValue?.length ?? 0;
    if (offset <= length) { point = { node, offset }; break; }
    offset -= length;
  }
  const range = document.createRange();
  range.setStart(point.node, point.offset);
  range.collapse(true);
  restoreEditorRange(editor, range);
}

export function replaceEditorFormats(formats: readonly MessageFormat[], start: number, end: number, length: number): MessageFormat[] {
  const next: MessageFormat[] = [];
  for (const run of formats) {
    const runEnd = run.offset + run.length;
    if (run.offset < start) next.push({ ...run, length: Math.min(runEnd, start) - run.offset });
    if (runEnd > end) {
      const offset = Math.max(end, run.offset);
      next.push({ ...run, offset: offset + length - (end - start), length: runEnd - offset });
    }
  }
  return next;
}
