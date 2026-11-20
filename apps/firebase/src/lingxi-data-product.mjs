import { LingxiDataError } from './lingxi-data.mjs';

const path = '/v1/product-data';
const maxBody = 131072;
const liveUid = /^[A-Za-z0-9_-]{1,64}$/u;

export function createProductDataTransport(endpoint, fetchRequest = fetch) {
  if (!endpoint) return null;
  const url = new URL(endpoint);
  if (url.protocol !== 'https:' || url.username || url.password || url.pathname !== '/' || url.search || url.hash) {
    throw new Error('Expected an HTTPS product Data adapter origin');
  }
  url.pathname = path;
  return async function forwardProductData(_uid, token, body) {
    let response, value;
    try {
      response = await fetchRequest(url, {
        method: 'POST', redirect: 'error', signal: AbortSignal.timeout(45_000),
        headers: { Authorization: 'Bearer ' + token, 'Content-Type': 'application/json' },
        body: JSON.stringify(body),
      });
      value = await response.json();
    } catch {
      throw new LingxiDataError(503, 'DATA_UNAVAILABLE');
    }
    if (!response.ok) {
      const codes = new Set(['INVALID_DATA_REQUEST', 'DATA_ACCESS_REQUIRED', 'STALE_MEMBERSHIP',
        'DATA_UNAVAILABLE', 'DATA_NOT_FOUND', 'DATA_CONFLICT', 'DATA_CAPACITY', 'OPERATION_ID_REQUIRED']);
      if (!codes.has(value?.error?.code)) throw new LingxiDataError(503, 'DATA_UNAVAILABLE');
      throw new LingxiDataError(response.status, value.error.code);
    }
    return value;
  };
}

export function createProductDataListener({ verifyToken, gateway, preview, botRecords, rules, sources, templates }) {
  return async function handleProductData(req, res) {
    function reply(status, value) {
      res.writeHead(status, { 'Content-Type': 'application/json', 'Cache-Control': 'no-store' });
      res.end(JSON.stringify(value));
    }

    try {
      const signaling = req.url === '/v1/data/signaling';
      if (req.method !== 'POST' || (req.url !== path && !(signaling && preview))) return reply(404, { error: { code: 'DATA_NOT_FOUND' } });
      const header = /^Bearer (\S{1,8000})$/u.exec(req.headers.authorization ?? '');
      if (!header) return reply(401, { error: { code: 'DATA_ACCESS_REQUIRED' } });
      let user;
      try { user = await verifyToken(header[1]); }
      catch { return reply(401, { error: { code: 'DATA_ACCESS_REQUIRED' } }); }
      if (!liveUid.test(user?.uid ?? '') || user.uid.startsWith('lx_data_bot_')) return reply(403, { error: { code: 'DATA_ACCESS_REQUIRED' } });

      const chunks = [];
      let size = 0;
      for await (const chunk of req.iterator({ destroyOnReturn: false })) {
        size += chunk.length;
        if (size > maxBody) {
          res.setHeader('Connection', 'close');
          return reply(413, { error: { code: 'INVALID_DATA_REQUEST' } });
        }
        chunks.push(chunk);
      }
      let body;
      try { body = JSON.parse(Buffer.concat(chunks).toString('utf8')); }
      catch { return reply(400, { error: { code: 'INVALID_DATA_REQUEST' } }); }
      if (signaling) {
        const taskToken = req.headers['x-lingxi-task-token'];
        if (taskToken === undefined && sources) {
          const result = await sources.signalOwner(user.uid, header[1], body);
          if (result !== undefined) return reply(200, result);
        }
        return reply(200, await preview.signal(user.uid, header[1], taskToken, body));
      }
      if (body?.action === 'privatePreviewPermit') {
        if (!preview) throw new LingxiDataError(503, 'DATA_UNAVAILABLE');
        return reply(200, await preview.permit(user.uid, body));
      }
      if (['setBotDataAccess', 'botDataOffers', 'botDataOperation', 'botDataHandoff'].includes(body?.action)) {
        if (!botRecords) throw new LingxiDataError(503, 'DATA_UNAVAILABLE');
        if (body.action === 'setBotDataAccess') return reply(200, await botRecords.setAccess(user.uid, header[1], body));
        if (body.action === 'botDataOffers') return reply(200, await botRecords.offers(user.uid, body));
        if (body.action === 'botDataHandoff') return reply(200, await botRecords.handoff(user.uid, body));
        return reply(200, await botRecords.operate(user.uid, body));
      }
      if (body?.action === 'setDataRule') {
        if (!rules) throw new LingxiDataError(503, 'DATA_UNAVAILABLE');
        return reply(200, await rules.setRule(user.uid, body));
      }
      if (body?.command?.action === 'dataAutomation') {
        if (!botRecords) throw new LingxiDataError(503, 'DATA_UNAVAILABLE');
        return reply(200, await botRecords.settings(user.uid, body));
      }
      if (body?.command?.action === 'templateViews' || body?.command?.action === 'setTemplateView') {
        if (!templates) throw new LingxiDataError(503, 'DATA_UNAVAILABLE');
        return reply(200, await templates(user.uid, header[1], body));
      }
      if (body?.action === 'dataSources' || body?.action === 'configureDataSource' || body?.command?.action === 'privateSourceOffers') {
        if (!sources) throw new LingxiDataError(503, 'DATA_UNAVAILABLE');
        return reply(200, await sources.manage(user.uid, header[1], body));
      }
      return reply(200, await gateway(user.uid, header[1], body));
    } catch (error) {
      if (error instanceof LingxiDataError) return reply(error.status, { error: { code: error.code } });
      return reply(503, { error: { code: 'DATA_UNAVAILABLE' } });
    }
  };
}
