// Separate JSONL helper process. Test controls hold terminal responses so the
// manager must join a late open and its exact stop before releasing a flight.
const readline = require('node:readline');
const leases = new Map();
let heldOpen;
let heldStop;
let holdStops = false;

function send(envelope, result, closeReceipt) {
  process.stdout.write(`${JSON.stringify({ id: envelope.id, type: 'media_response',
    response: { request_id: envelope.request.request_id, result },
    ...(closeReceipt ? { close_receipt: closeReceipt } : {}),
  })}\n`);
}
function opened(envelope) {
  const open = envelope.request.request.request;
  const leaseId = `lease-${open.owner.attachment_generation}`;
  leases.set(leaseId, { lease_id: leaseId, open_request_id: envelope.request.request_id,
    owner: open.owner, identity: open.identity });
  send(envelope, { type: 'opened', lease: { lease_id: leaseId } });
}
function stopped(envelope, wrongReceipt = false) {
  const receipt = leases.get(envelope.request.request.lease_id);
  const wrong = wrongReceipt === 'owner' ? { ...receipt, owner: { ...receipt.owner, logical_owner_id: 'wrong-owner' } }
    : wrongReceipt === 'identity' ? { ...receipt, identity: { ...receipt.identity, generation: 99 } }
    : wrongReceipt === 'open_request' ? { ...receipt, open_request_id: 'wrong-open' }
    : wrongReceipt ? { ...receipt, lease_id: 'wrong-lease' } : receipt;
  send(envelope, { type: 'stopped' }, wrong);
}
readline.createInterface({ input: process.stdin }).on('line', (line) => {
  const envelope = JSON.parse(line);
  if (envelope.kind === 'test_control') {
    if (envelope.action === 'hold_stops') holdStops = true;
    if (envelope.action === 'release_open' && heldOpen) { const value = heldOpen; heldOpen = undefined; opened(value); }
    if (envelope.action === 'release_stop' && heldStop) {
      const value = heldStop; heldStop = undefined; holdStops = false; stopped(value, envelope.wrong_receipt);
    }
    return;
  }
  if (envelope.kind !== 'media_request') return;
  const request = envelope.request;
  process.stderr.write(`${JSON.stringify(request)}\n`);
  if (request.request.type === 'open') {
    if (request.request_id.startsWith('held-open')) { heldOpen = envelope; return; }
    if (request.request_id === 'preflight-fail') {
      send(envelope, { type: 'open_failed_without_resource', error: { kind: 'permission_denied', message: 'no admission' } }); return;
    }
    if (request.request_id === 'unknown-fail') {
      send(envelope, { type: 'error', error: { kind: 'permission_denied', message: 'unknown' } }); return;
    }
    opened(envelope);
  } else if (holdStops) {
    heldStop = envelope;
  } else {
    stopped(envelope);
  }
});
