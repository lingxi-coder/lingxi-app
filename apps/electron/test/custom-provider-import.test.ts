import assert from 'node:assert/strict';
import test from 'node:test';
import { parseProviderImport, mergeProviderImport, validateCustomProvider, validateImportEntry, validateProfileName } from '../src/renderer/components/settings/pages/customProviderImport';
const native = { type: 'openai', baseUrl: 'https://example.com/v1', apiKeyEnv: 'EXAMPLE_KEY', models: [{ id: 'm', aliases: ['alias'], capabilities: { vision: true } }], supportsWebsockets: false };
test('LingXi wrappers and native maps preserve supported advanced fields', () => {
  for (const input of [{ providers: { custom: native } }, { custom: native }]) {
    const result = parseProviderImport(JSON.stringify(input));
    assert.deepEqual(result.entries[0].draft, native);
    assert.equal(validateImportEntry(result.entries[0]), null);
  }
});
test('four OpenCode SDKs map with separate credentials and explicit model aliases', () => {
  for (const [npm, type] of [['@ai-sdk/openai-compatible','openai'],['@ai-sdk/openai','openai-responses'],['@ai-sdk/anthropic','anthropic'],['@ai-sdk/google','gemini']]) {
    const { entries } = parseProviderImport(JSON.stringify({ provider: { custom: { npm, options: { baseURL: native.baseUrl, apiKey: 'super-secret' }, models: { alias: { id: 'actual', name: 'Friendly' } } } } }));
    assert.equal(entries[0].draft.type, type);
    assert.equal(entries[0].apiKey, 'super-secret');
    assert.deepEqual(entries[0].draft.models, [{ id: 'actual', aliases: ['alias'] }]);
    assert.ok(!JSON.stringify(entries[0].draft).includes('super-secret'));
    assert.ok(!JSON.stringify(entries[0].diagnostics).includes('super-secret'));
  }
});
test('environment references convert; file references and request options block', () => {
  const make = (options: unknown) => parseProviderImport(JSON.stringify({ provider: { custom: { npm: '@ai-sdk/openai', options, models: { m: {} } } } })).entries[0];
  assert.equal(make({ apiKey: '{env:MY_KEY}', baseURL: native.baseUrl }).draft.apiKeyEnv, 'MY_KEY');
  for (const options of [{ apiKey: '{file:secret}' }, { headers: { Authorization: 'super-secret' } }, { timeout: 5000 }]) {
    const entry = make(options);
    assert.ok(entry.diagnostics.some(d => d.severity === 'error'));
    assert.ok(!JSON.stringify(entry.diagnostics).includes('super-secret'));
    assert.ok(validateImportEntry(entry));
  }
});
test('invalid JSON never quotes sensitive input; malformed shapes cannot crash', () => {
  for (const value of ['{"apiKey":"super-secret",', 'null', '[]', '{"providers":null}', '{"providers":{"custom":null}}', '{"providers":{"custom":{"models":[null]}}}']) {
    const result = parseProviderImport(value);
    assert.ok(result.diagnostics.length || result.entries.some(e => validateImportEntry(e)));
    assert.ok(!JSON.stringify(result.diagnostics).includes('super-secret'));
  }
});
test('conflicts default to skip; selected replacements merge only into supplied layer', () => {
  const current = { custom: native, untouched: native };
  const entries = parseProviderImport(JSON.stringify({ providers: { custom: { ...native, apiKey: 'secret' }, added: native } }), current).entries;
  assert.equal(entries[0].selected, false);
  assert.equal(entries[0].conflict, true);
  let merged = mergeProviderImport(current, entries);
  assert.equal(merged.providers.custom, native);
  assert.equal(merged.providers.untouched, native);
  entries[0].selected = true;
  merged = mergeProviderImport(current, entries);
  assert.equal(merged.credentials.custom, 'secret');
  assert.ok(!JSON.stringify(merged.providers).includes('secret'));
  assert.deepEqual(Object.keys(current), ['custom', 'untouched']);
});
test('unknown packages can be corrected, missing fields and selected errors prevent merge', () => {
  const entry = parseProviderImport('{"provider":{"custom":{"npm":"unknown","models":{"m":{}}}}}').entries[0];
  assert.ok(validateImportEntry(entry));
  entry.draft.type = 'openai'; entry.draft.baseUrl = native.baseUrl; entry.apiKey = 'key';
  assert.equal(validateImportEntry(entry), null);
  entry.draft.models = [];
  assert.throws(() => mergeProviderImport({}, [entry]));
});
test('profile IDs and prototype pollution are rejected', () => {
  for (const name of ['__proto__','constructor','prototype','Bad Name','UPPER']) assert.ok(validateProfileName(name));
  assert.equal(validateProfileName('custom-1.test'), null);
  const parsed = parseProviderImport('{"providers":{"__proto__":{"type":"openai","models":["m"]}}}');
  assert.ok(validateImportEntry(parsed.entries[0]));
  assert.equal(({} as Record<string, unknown>).polluted, undefined);
});
test('strict write validation follows engine requirements and handles malformed models', () => {
  assert.match(validateCustomProvider({ ...native, models: [null] } as never) ?? '', /id|models/);
  assert.ok(validateCustomProvider({ ...native, baseUrl: '' }));
  assert.ok(validateCustomProvider({ ...native, baseUrl: 'https://user:secret@example.com' }));
  assert.ok(validateCustomProvider({ ...native, type: 'azure-openai' }));
  assert.equal(validateCustomProvider({ ...native, type: 'azure-openai', apiVersion: '2024-06-01' }), null);
  assert.equal(validateCustomProvider({ type: 'bedrock-claude', region: 'us-east-1', models: [{id:'m'}] }), null);
});
test('native advanced metadata and pricing survive while unsafe nested properties block', () => {
  const draft = { ...native, models: [{ id: 'm', metadata: { contextWindowTokens: 10000, maxOutputTokens: 1000, pricing: { inputPerMillion: 1, tiers: [{ contextThresholdTokens: 10000, inputPerMillion: 2 }] } } }], pricing: { m: { inputPerMtok: 1, outputPerMtok: 2 } } };
  const entry = parseProviderImport(JSON.stringify({ providers: { custom: draft } })).entries[0];
  assert.deepEqual(mergeProviderImport({}, [entry]).providers.custom, draft);
  const unsafe = parseProviderImport(JSON.stringify({ providers: { custom: { ...draft, models: [{ id: 'm', metadata: { accessToken: 'hidden-secret' } }] } } })).entries[0];
  assert.ok(validateImportEntry(unsafe));
  assert.ok(!JSON.stringify(unsafe.draft).includes('hidden-secret'));
  assert.ok(!JSON.stringify(unsafe.diagnostics).includes('hidden-secret'));
});
test('invalid advanced fields and duplicate models are rejected', () => {
  for (const change of [{ models: [{id:'m'}, {id:'m'}] }, { models: [{id:'m', aliases:[2]}] }, { models: [{id:'m',metadata:{inputModalities:null}}] }, { pricing: { m: { inputPerMtok: -1 } } }, { supportsWebsockets:true }, { websocketConnectTimeoutMs:1.2 }]) {
    assert.ok(validateCustomProvider({ ...native, ...change } as never));
  }
});
test('batch merge is all-or-nothing and metadata omissions are nonblocking warnings', () => {
  const current = { custom: native };
  const entries = parseProviderImport(JSON.stringify({ providers: { good:native, broken:{...native,models:[]} } })).entries;
  assert.throws(() => mergeProviderImport(current, entries));
  assert.deepEqual(current, { custom:native });
  const result = parseProviderImport(JSON.stringify({ plugin:['something'], provider: { other: { npm:'@ai-sdk/openai', name:'Other', options:{baseURL:native.baseUrl,apiKey:'key'}, models:{m:{name:'M'}} } } }));
  assert.ok(result.diagnostics.some(d => d.severity === 'warning'));
  assert.equal(validateImportEntry(result.entries[0]), null);
});
test('missing OpenCode fields remain editable in preview', () => {
  const entry = parseProviderImport('{"provider":{"custom":{"npm":"@ai-sdk/openai"}}}').entries[0];
  assert.ok(validateImportEntry(entry));
  entry.draft.baseUrl = native.baseUrl;
  entry.draft.models = [{id:'m'}];
  entry.apiKey = 'key';
  assert.equal(validateImportEntry(entry), null);
});
test('existing stored credentials permit import without overwriting device secrets', () => {
  const entry = parseProviderImport(JSON.stringify({providers:{custom:{...native,apiKeyEnv:undefined}}})).entries[0];
  assert.ok(validateImportEntry(entry));
  assert.equal(validateImportEntry(entry, {credentialConfigured:true}), null);
  assert.throws(() => mergeProviderImport({}, [entry]));
  const merged = mergeProviderImport({}, [entry], {credentialConfigured: name => name === 'custom'});
  assert.deepEqual(merged.credentials, {});
  assert.equal(merged.providers.custom.apiKeyEnv, undefined);
});
test('native apiKey references take precedence over fallback apiKeyEnv independent of property order', () => {
  for (const raw of [{...native,apiKeyEnv:'FALLBACK',apiKey:'{env:PRIMARY}'}, {apiKey:'{env:PRIMARY}',...native,apiKeyEnv:'FALLBACK'}]) {
    const entry = parseProviderImport(JSON.stringify({providers:{custom:raw}})).entries[0];
    assert.equal(entry.draft.apiKeyEnv, 'PRIMARY');
    assert.ok(entry.diagnostics.some(d => d.message.includes('apiKeyEnv')));
  }
  const entry = parseProviderImport(JSON.stringify({providers:{custom:{...native,apiKey:'literal-secret'}}})).entries[0];
  assert.equal(entry.apiKey,'literal-secret');
  assert.equal(entry.draft.apiKeyEnv,native.apiKeyEnv);
});
test('unsupported diagnostics identify field names but never field values', () => {
  const result = parseProviderImport(JSON.stringify({plugin:['private-value'],provider:{custom:{npm:'@ai-sdk/openai',options:{baseURL:native.baseUrl,apiKey:'key',timeout:'private-value'},models:{m:{name:'private-value',variants:{value:'private-value'}}},blacklist:['private-value']}}}));
  const messages = JSON.stringify([...result.diagnostics,...result.entries[0].diagnostics]);
  for (const field of ['plugin','timeout','name','variants','blacklist']) assert.ok(messages.includes(field));
  assert.ok(!messages.includes('private-value'));
});
test('native string model entries normalize; malformed drafts refuse safely', () => {
  const entry = parseProviderImport(JSON.stringify({providers:{custom:{...native,models:['m']}}})).entries[0];
  assert.deepEqual(entry.draft.models,[{id:'m'}]);
  for (const models of ['m', ['m'], [null], [{id:4}], [{id:'m',aliases:'alias'}]]) assert.ok(validateCustomProvider({...native,models} as never));
});
test('arbitrary property names are not echoed into diagnostics', () => {
  const result = parseProviderImport(JSON.stringify({providers:{custom:{...native,'secret-value-as-field':'private-value'}}}));
  assert.ok(!JSON.stringify(result.entries[0].diagnostics).includes('secret-value-as-field'));
  assert.ok(!JSON.stringify(result.entries[0].diagnostics).includes('private-value'));
});
test('OpenCode request id override becomes model id and retains the map key alias', () => {
  const result = parseProviderImport(JSON.stringify({provider:{custom:{npm:'@ai-sdk/openai', options:{baseURL:native.baseUrl,apiKey:'key'}, models:{'gpt-5-mini':{id:'gpt-production'},same:{id:'same'},implicit:{}}}}}));
  assert.deepEqual(result.entries[0].draft.models,[{id:'gpt-production',aliases:['gpt-5-mini']},{id:'same'},{id:'implicit'}]);
});
test('builtin and claude profile IDs cannot shadow credential routing aliases', () => {
  for (const name of ['builtin','claude']) {
    assert.ok(validateProfileName(name));
    const entry = parseProviderImport(JSON.stringify({providers:{[name]:native}})).entries[0];
    assert.ok(validateImportEntry(entry));
    assert.throws(() => mergeProviderImport({},[entry]));
  }
  assert.equal(validateProfileName('anthropic'),null);
});
test('provider price overrides require both input and output prices before import', () => {
  for (const prices of [{}, { inputPerMtok: 1 }, { outputPerMtok: 2 }, { cacheReadPerMtok: 0 }]) {
    const draft = { ...native, pricing: { m: prices } };
    assert.match(validateCustomProvider(draft) ?? '', /inputPerMtok.*outputPerMtok/);
    const entry = parseProviderImport(JSON.stringify({ providers: { custom: draft } })).entries[0];
    assert.ok(validateImportEntry(entry));
    assert.throws(() => mergeProviderImport({}, [entry]));
  }
  assert.equal(validateCustomProvider({ ...native, pricing: {} }), null);
  assert.equal(validateCustomProvider({ ...native, pricing: { m: { inputPerMtok: 0, outputPerMtok: 0 } } }), null);
});
test('advanced string fields reject shapes that the engine cannot parse', () => {
  for (const change of [{visionDelegate:''}, {billingMode:['perToken']}, {models:[{id:'m',metadata:{pricing:{billingMode:['perToken']}}}]}]) {
    const draft = {...native,...change};
    assert.ok(validateCustomProvider(draft));
    const entry = parseProviderImport(JSON.stringify({providers:{custom:draft}})).entries[0];
    assert.ok(validateImportEntry(entry));
    assert.throws(() => mergeProviderImport({},[entry]));
  }
  assert.equal(validateCustomProvider({...native,visionDelegate:'other/model',billingMode:'perToken',models:[{id:'m',metadata:{pricing:{billingMode:'unknown'}}}]}),null);
});
