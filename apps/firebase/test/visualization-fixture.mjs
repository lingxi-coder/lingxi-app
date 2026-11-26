import { createHash } from 'node:crypto';
import { packagePartBytes } from '../src/team-visualization-contract.mjs';

export function markupPackage(title, html) {
  const bytes = Buffer.isBuffer(html) ? html : Buffer.from(html);
  const chunks = [];
  for (let start = 0; start < bytes.length; start += packagePartBytes) chunks.push(bytes.subarray(start, start + packagePartBytes));
  return { package: { title, htmlBytes: bytes.length, assets: [],
    parts: chunks.map(chunk => createHash('sha256').update(chunk).digest('hex')) }, chunks };
}

export async function putFixtureParts(plan, chunks, request) {
  const { message, version, teamId, conversationId, generation } = plan;
  for (const index of plan.missing) {
    await request({ action: 'putPart', message, version, teamId, conversationId, generation,
      index, data: chunks[index].toString('base64') });
  }
}
