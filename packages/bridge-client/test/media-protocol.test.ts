import assert from 'node:assert/strict';
import { test } from 'node:test';

import { BridgeClient } from '../src/client.js';
import {
  validateMediaServiceRequest,
  validateMediaServiceResponse,
} from '../src/mediaProtocol.js';
import type { ClientCommand, MediaServiceResponseDto } from '../src/protocol.js';

const operationId = { id: 'operation-1', generation: 1, service_epoch: 2 };
const incarnation = '00000000-0000-4000-8000-000000000001';
const operationResource = { type: 'operation', operation_id: operationId, incarnation };

class RecordingBridgeClient extends BridgeClient {
  readonly commands: ClientCommand[] = [];

  override sendCommand(command: ClientCommand): void {
    this.commands.push(command);
  }

  respondToMediaService(response: MediaServiceResponseDto): void {
    (this as unknown as {
      acceptMediaServiceResponse(response: MediaServiceResponseDto): void;
    }).acceptMediaServiceResponse(response);
  }
}

function serviceRequest(action: unknown) {
  return validateMediaServiceRequest({ request_id: 'request-1', action });
}

function operationResponse(result: unknown, handle = incarnation) {
  return validateMediaServiceResponse({
    request_id: 'response-1',
    result: {
      type: 'operation',
      operation_id: operationId,
      incarnation: handle,
      result,
    },
    error: null,
  });
}

test('cancel and operation results require a host-issued UUID v4 incarnation', () => {
  const cancel = {
    request_id: 'request-1',
    action: { type: 'cancel', operation_id: operationId, incarnation },
  };
  assert.deepEqual(validateMediaServiceRequest(cancel), cancel);
  assert.throws(() => serviceRequest({ type: 'cancel', operation_id: operationId }), /incarnation/);
  assert.throws(() => serviceRequest({
    type: 'cancel', operation_id: operationId, incarnation: '00000000-0000-1000-8000-000000000001',
  }), /incarnation/);
  assert.throws(() => serviceRequest({
    type: 'cancel', operation_id: operationId, incarnation: 'not-a-uuid',
  }), /incarnation/);

  assert.equal(operationResponse({ type: 'sources', sources: [] }).result?.type, 'operation');
  assert.throws(() => operationResponse({ type: 'sources', sources: [] }, 'invalid'), /incarnation/);
  assert.throws(() => validateMediaServiceResponse({
    request_id: 'response-1',
    result: { type: 'operation', operation_id: operationId, result: { type: 'sources', sources: [] } },
    error: null,
  }), /incarnation/);
});

test('BridgeClient returns the operation incarnation and requires it for cancellation', async () => {
  const client = new RecordingBridgeClient();
  const operationPromise = client.executeMedia({ type: 'sources' });
  const executeCommand = client.commands[0];
  assert.equal(executeCommand?.type, 'media_service_request');
  if (!executeCommand || executeCommand.type !== 'media_service_request') throw new Error('missing media execute command');
  client.respondToMediaService({
    request_id: executeCommand.request.request_id,
    result: {
      type: 'operation', operation_id: operationId, incarnation,
      result: { type: 'sources', sources: [] },
    },
    error: null,
  });
  assert.deepEqual(await operationPromise, {
    operation_id: operationId,
    incarnation,
    result: { type: 'sources', sources: [] },
  });

  const cancelPromise = client.cancelMedia(operationId, incarnation);
  const cancelCommand = client.commands[1];
  assert.equal(cancelCommand?.type, 'media_service_request');
  if (!cancelCommand || cancelCommand.type !== 'media_service_request') throw new Error('missing media cancel command');
  assert.deepEqual(cancelCommand.request.action, {
    type: 'cancel', operation_id: operationId, incarnation,
  });
  client.respondToMediaService({
    request_id: cancelCommand.request.request_id,
    result: { type: 'cancelled' },
    error: null,
  });
  await cancelPromise;
});

test('all operation target forms validate resource refs and operation incarnations', () => {
  const nonOperationResources = [
    { type: 'source', source: { id: 'source-1' } },
    { type: 'recording', recording_id: 'recording-1' },
    { type: 'session', session_id: 'session-1' },
    { type: 'media', media_ref: 'media-1' },
  ];
  const targetOperations = (target: unknown) => [
    { type: 'status', target },
    { type: 'observe', target, limit: 1 },
    { type: 'wait', target, timeout_ms: 1, limit: 1 },
    { type: 'stop', target },
  ];

  for (const target of [...nonOperationResources, operationResource]) {
    for (const operation of targetOperations(target)) {
      const validated = serviceRequest({ type: 'execute', operation });
      assert.deepEqual(validated.action, { type: 'execute', operation });
    }
  }

  const missingIncarnation = { type: 'operation', operation_id: operationId };
  const invalidIncarnation = { ...operationResource, incarnation: '00000000-0000-1000-8000-000000000001' };
  for (const target of [missingIncarnation, invalidIncarnation]) {
    for (const operation of targetOperations(target)) {
      assert.throws(() => serviceRequest({ type: 'execute', operation }), /incarnation/);
    }
  }

  assert.doesNotThrow(() => serviceRequest({
    type: 'execute', operation: { type: 'status', target: null },
  }));
});

test('every resource ref in returned status, event and stopped results keeps its handle scope', () => {
  const validEvents = {
    type: 'events',
    batch: {
      target: operationResource,
      events: [{
        sequence: 1,
        occurred_at_unix_ms: 10,
        target: operationResource,
        operation_id: operationId,
        kind: { type: 'state_changed', resource: operationResource, state: 'active' },
      }],
      next_sequence: 1,
      dropped_before_cursor: false,
    },
  };
  assert.doesNotThrow(() => operationResponse({
    type: 'status', resources: [{ resource: operationResource, state: 'active' }],
  }));
  assert.doesNotThrow(() => operationResponse(validEvents));
  assert.doesNotThrow(() => operationResponse({ type: 'stopped', resource: operationResource }));

  const missingIncarnation = { type: 'operation', operation_id: operationId };
  const badStatus = { type: 'status', resources: [{ resource: missingIncarnation, state: 'active' }] };
  const badBatchTarget = { type: 'events', batch: { ...validEvents.batch, target: missingIncarnation } };
  const badEventTarget = {
    type: 'events',
    batch: { ...validEvents.batch, events: [{ ...validEvents.batch.events[0], target: missingIncarnation }] },
  };
  const badStateResource = {
    type: 'events',
    batch: {
      ...validEvents.batch,
      events: [{
        ...validEvents.batch.events[0],
        kind: { type: 'state_changed', resource: missingIncarnation, state: 'active' },
      }],
    },
  };
  const badStopped = { type: 'stopped', resource: missingIncarnation };
  for (const result of [badStatus, badBatchTarget, badEventTarget, badStateResource, badStopped]) {
    assert.throws(() => operationResponse(result), /incarnation/);
  }

  // Event operation_id is only the correlation DTO in Harness; it has no incarnation field.
  assert.doesNotThrow(() => operationResponse({
    type: 'events',
    batch: {
      target: { type: 'session', session_id: 'session-1' },
      events: [{
        sequence: 1,
        occurred_at_unix_ms: 10,
        target: { type: 'session', session_id: 'session-1' },
        operation_id: operationId,
        kind: { type: 'transcript', text: 'hello' },
      }],
      next_sequence: 1,
      dropped_before_cursor: false,
    },
  }));
});
