import { z } from 'zod';
import { authorizeDataBinding, LingxiDataError } from './lingxi-data.mjs';
import { dataId, generation } from './lingxi-data-sources.mjs';

const scope = z.discriminatedUnion('kind', [
  z.object({ kind: z.literal('personal') }).strict(),
  z.object({ kind: z.literal('team'), teamId: dataId, conversationId: dataId, generation }).strict(),
]);
const listRequest = z.object({ scope, command: z.object({ action: z.literal('templateViews'), dataset: dataId }).strict() }).strict();
const length = z.string().regex(/^(?:0|[0-9]{1,2}(?:px|rem))$/u);
const color = z.string().regex(/^#[a-f0-9]{6}$/iu);
const style = z.object({
  display: z.enum(['grid', 'flex', 'block']).optional(), flexDirection: z.enum(['row', 'column']).optional(),
  gap: length.optional(), padding: length.optional(), borderRadius: length.optional(), fontSize: length.optional(),
  fontWeight: z.enum(['400', '500', '600', '700']).optional(), color: color.optional(), backgroundColor: color.optional(),
}).strict();
const node = z.lazy(() => z.discriminatedUnion('kind', [
  z.object({ kind: z.literal('text'), text: z.string().max(1024) }).strict(),
  z.object({ kind: z.literal('field'), field: dataId }).strict(),
  z.object({ kind: z.literal('element'), tag: z.enum(['section', 'div', 'h3', 'p', 'span', 'strong', 'small', 'ul', 'li']),
    style: style.optional(), children: z.array(node).max(32) }).strict(),
]));
const setRequest = z.object({ scope, command: z.object({
  action: z.literal('setTemplateView'), dataset: dataId, view: dataId, schemaVersion: generation,
  expectedVersion: z.number().int().min(0).max(Number.MAX_SAFE_INTEGER - 1),
  definition: z.object({ version: z.literal(1), nodes: z.array(z.unknown()).max(16) }).strict().nullable(),
}).strict() }).strict();

function invalid() { return new LingxiDataError(400, 'INVALID_DATA_REQUEST'); }

function validateTree(nodes, properties) {
  let count = 0;
  function check(values, depth) {
    if (depth > 10) throw invalid();
    for (const value of values) {
      if (!value || typeof value !== 'object' || ++count > 128) throw invalid();
      if (value.kind === 'field' && !Object.hasOwn(properties, value.field)) throw invalid();
      if (Array.isArray(value.children)) check(value.children, depth + 1);
    }
  }
  check(nodes, 0);
  const parsed = z.array(node).safeParse(nodes);
  if (!parsed.success) throw invalid();
  return parsed.data;
}

export function createDataTemplates({ db, gateway }) {
  return async function templates(uid, token, raw) {
    const list = listRequest.safeParse(raw);
    if (list.success) {
      const { scope, command: { dataset } } = list.data;
      return db.runTransaction(async tx => {
        await authorizeDataBinding(tx, db, uid, scope, dataset, false);
        const rows = await tx.get(db.collection('lingxiDataTemplateViews').doc(dataset).collection('views').limit(20));
        return { views: rows.docs.flatMap(doc => {
          const entry = doc.data();
          return entry.definition ? [{ view: doc.id, ...entry }] : [];
        }) };
      }, { readOnly: true });
    }
    const set = setRequest.safeParse(raw);
    if (!set.success) throw invalid();
    const { scope, command: { dataset, view, schemaVersion, expectedVersion, definition } } = set.data;
    const schema = await gateway(uid, token, { scope, command: { action: 'describe', dataset } });
    if (schema.schemaVersion !== schemaVersion) throw new LingxiDataError(409, 'DATA_CONFLICT');
    const viewRef = db.collection('lingxiDataTemplateViews').doc(dataset).collection('views');
    const doc = viewRef.doc(view);
    const normalized = definition ? { version: 1, nodes: validateTree(definition.nodes, schema.schema.properties ?? {}) } : null;
    return db.runTransaction(async tx => {
      await authorizeDataBinding(tx, db, uid, scope, dataset, true, true);
      const previous = (await tx.get(doc)).data();
      if ((previous?.version ?? 0) !== expectedVersion) throw new LingxiDataError(409, 'DATA_CONFLICT');
      if (!previous && (await tx.get(viewRef.limit(20))).size >= 20) throw new LingxiDataError(409, 'DATA_CAPACITY');
      tx.set(doc, { version: expectedVersion + 1, schemaVersion, definition: normalized });
      return { version: expectedVersion + 1 };
    });
  };
}
