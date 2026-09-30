const STRONG_BOUNDARIES = new Set(['。', '！', '？', '!', '?', '；', ';', '\n']);
const SOFT_BOUNDARIES = new Set(['，', ',', '：', ':', '、']);
const FALLBACK_BOUNDARIES = new Set([...STRONG_BOUNDARIES, ...SOFT_BOUNDARIES, ' ', '\t']);

export class StreamingSpeechSegmenter {
  private readonly minimumSegmentLength: number;
  private readonly softBoundaryThreshold: number;
  private readonly hardBoundaryThreshold: number;

  private receivedText = '';
  private pendingText = '';
  private emittedText = '';
  private markdownCarry = '';
  private linkCarry = '';
  private insideFencedCode = false;
  private inlineCodeDelimiterLength: number | null = null;
  private finished = false;

  constructor(options: {
    minimumSegmentLength?: number;
    softBoundaryThreshold?: number;
    hardBoundaryThreshold?: number;
  } = {}) {
    this.minimumSegmentLength = options.minimumSegmentLength ?? 6;
    this.softBoundaryThreshold = options.softBoundaryThreshold ?? 24;
    this.hardBoundaryThreshold = options.hardBoundaryThreshold ?? 64;
  }

  append(delta: string): string[] {
    if (this.finished || !delta) return [];
    this.receivedText += delta;
    this.pendingText += this.consumeMarkdown(delta, false);
    return this.drainSegments();
  }

  finish(finalText: string): string[] {
    if (this.finished) return [];
    this.finished = true;
    const spokenFinal = sanitizeSpeakableText(finalText);
    // Compare the same spoken representation used for both streaming and final
    // output; raw Markdown delimiters must not make an emitted prefix differ.
    const spokenPrefix = sanitizeSpeakableText(this.emittedText);
    this.pendingText = spokenFinal.startsWith(spokenPrefix)
      ? spokenFinal.slice(spokenPrefix.length)
      : spokenFinal;
    this.markdownCarry = '';
    this.insideFencedCode = false;

    const segments = this.drainSegments();
    const tail = this.pendingText.trim();
    this.pendingText = '';
    if (tail) {
      segments.push(tail);
      this.emittedText += tail;
    }
    return segments;
  }

  private consumeMarkdown(delta: string, isFinal: boolean): string {
    const characters = [...this.markdownCarry, ...delta];
    this.markdownCarry = '';
    let output = '';

    for (let index = 0; index < characters.length; index += 1) {
      const character = characters[index]!;
      if (character === '`') {
        let runEnd = index;
        while (runEnd < characters.length && characters[runEnd] === '`') runEnd += 1;
        const count = runEnd - index;
        if (!isFinal && runEnd === characters.length && count < 3) {
          this.markdownCarry = '`'.repeat(count);
          break;
        }
        if (this.inlineCodeDelimiterLength !== null) {
          if (count === this.inlineCodeDelimiterLength) this.inlineCodeDelimiterLength = null;
        } else if (count >= 3) {
          this.insideFencedCode = !this.insideFencedCode;
        } else if (!this.insideFencedCode) {
          this.inlineCodeDelimiterLength = count;
          output += ' ';
        }
        index = runEnd - 1;
        continue;
      }
      if (!this.insideFencedCode && this.inlineCodeDelimiterLength === null) output += character;
    }

    if (isFinal && this.markdownCarry && !this.insideFencedCode) this.markdownCarry = '';
    return this.consumeLinks(output);
  }

  private consumeLinks(text: string): string {
    let remaining = this.linkCarry + text;
    this.linkCarry = '';
    let output = '';
    while (remaining) {
      const start = remaining.indexOf('[');
      if (start < 0) return output + remaining;
      output += remaining.slice(0, start);
      remaining = remaining.slice(start);
      const labelEnd = remaining.indexOf(']');
      if (labelEnd < 0 || labelEnd + 1 === remaining.length) break;
      if (remaining[labelEnd + 1] !== '(') {
        output += remaining.slice(0, labelEnd + 1);
        remaining = remaining.slice(labelEnd + 1);
        continue;
      }
      const urlEnd = remaining.indexOf(')', labelEnd + 2);
      if (urlEnd < 0) break;
      output += remaining.slice(1, labelEnd);
      remaining = remaining.slice(urlEnd + 1);
    }
    this.linkCarry = remaining;
    return output;
  }

  private drainSegments(): string[] {
    const segments: string[] = [];
    for (;;) {
      const length = this.nextBoundaryLength();
      if (length === null) return segments;
      const characters = [...this.pendingText];
      const rawSegment = characters.slice(0, length).join('');
      this.pendingText = characters.slice(length).join('');
      this.emittedText += rawSegment;
      const segment = sanitizeSpeakableText(rawSegment);
      if (segment) segments.push(segment);
    }
  }

  private nextBoundaryLength(): number | null {
    const characters = [...this.pendingText];
    if (characters.length < this.minimumSegmentLength) return null;
    const scanLimit = Math.min(characters.length, this.hardBoundaryThreshold);

    for (let index = 0; index < scanLimit; index += 1) {
      if (index + 1 >= this.minimumSegmentLength && STRONG_BOUNDARIES.has(characters[index]!)) {
        return index + 1;
      }
    }

    if (characters.length >= this.softBoundaryThreshold) {
      let softLength: number | null = null;
      for (let index = 0; index < scanLimit; index += 1) {
        if (index + 1 >= this.minimumSegmentLength && SOFT_BOUNDARIES.has(characters[index]!)) softLength = index + 1;
      }
      if (softLength !== null) return softLength;
    }

    if (characters.length < this.hardBoundaryThreshold) return null;
    let fallbackLength: number | null = null;
    for (let index = 0; index < scanLimit; index += 1) {
      if (index + 1 >= this.minimumSegmentLength && FALLBACK_BOUNDARIES.has(characters[index]!)) fallbackLength = index + 1;
    }
    return fallbackLength ?? this.hardBoundaryThreshold;
  }
}

export function sanitizeSpeakableText(markdown: string): string {
  return markdown
    .replace(/```[\s\S]*?```/g, ' ')
    .replace(/(`{1,2})(?!`)[\s\S]*?\1(?!`)/g, ' ')
    .replace(/\[([^\]]+)\]\(([^)]+)\)/g, '$1')
    .replace(/^[#>*-]+\s*/gm, '')
    .replace(/[*_~]/g, ' ')
    .replace(/\s+([，。！？；：、,.!?;:])/g, '$1')
    .replace(/([，。！？；：、,.!?;:])\s+/g, '$1')
    .replace(/\s+/g, ' ')
    .trim();
}
