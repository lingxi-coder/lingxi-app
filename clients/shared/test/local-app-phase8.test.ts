import assert from 'node:assert/strict';
import { test } from 'node:test';

import type {
  AppEventDto,
  ClientEvent,
  PluginCommandDto,
  ServerHello,
} from '../src/protocol.js';
import { CLIENT_PROTOCOL_VERSION } from '../src/protocol.js';
import { validateClientEvent, validateServerHello } from '../src/validation.js';

test('ServerHello validation is exact and fail-closed', () => {
  const hello = {
    protocol_version: '0.2.0',
    server_name: 'LingXi Bridge',
    capabilities: {
      supports_streaming: true,
      supports_tools: true,
      supports_skills: true,
      supports_commands: true,
      client_protocol_version: CLIENT_PROTOCOL_VERSION,
    },
  } satisfies ServerHello;

  assert.deepEqual(validateServerHello(hello), hello);
  assert.equal(hello.capabilities.client_protocol_version, '10.0.0');
  assert.throws(
    () => validateServerHello({ ...hello, capabilities: { ...hello.capabilities, extra: true } }),
    /unsupported fields/,
  );
});

test('Phase8 Local App plugin commands stay nested and byte-stable', () => {
  const commands = [
    { type: 'get_inventory', plugin_id: 'lingxi-local-app' },
    { type: 'resolve_create_confirmation', request_id: 'create-0001', approved: true },
    { type: 'resolve_mcp_proposal_approval', request_id: 'proposal-0001', approved: false },
    { type: 'get_managed_mcp_inventory' },
  ] satisfies PluginCommandDto[];

  assert.equal(JSON.stringify(commands[0]), '{"type":"get_inventory","plugin_id":"lingxi-local-app"}');
  assert.equal(JSON.stringify(commands[1]), '{"type":"resolve_create_confirmation","request_id":"create-0001","approved":true}');
  assert.equal(JSON.stringify(commands[2]), '{"type":"resolve_mcp_proposal_approval","request_id":"proposal-0001","approved":false}');
  assert.equal(JSON.stringify(commands[3]), '{"type":"get_managed_mcp_inventory"}');
});

test('Phase8 Local App app_event payloads validate and reject unknown or incomplete variants', () => {
  const event = {
    type: 'app_event',
    event: {
      type: 'mcp_proposal_approval_requested',
      request: {
        requestId: 'proposal-0001',
        appId: 'habits-1a2b',
        workflowRunId: 'wf-0002',
        summary: 'Remove summarize_habits',
        proposalSha256: '3'.repeat(64),
        approvalContractSha256: '4'.repeat(64),
        toolSurfaceSha256: '5'.repeat(64),
        toolDiffs: [{
          kind: 'removed',
          name: 'summarize_habits',
          before: {
            name: 'summarize_habits',
            title: 'Track habits',
            description: 'Create or update one habit entry.',
            inputSchemaJson: '{"type":"object"}',
            semanticFlowJson: '{"flowId":"local-app-save"}',
            permissionCeiling: 'ask',
          },
        }],
        pendingGates: [{
          gateId: 'ui_runner',
          label: 'UI runner available',
          status: 'pending',
          available: true,
        }],
        receipt: {
          receiptId: 'receipt-0001',
          appId: 'habits-1a2b',
          workflowRunId: 'wf-0002',
          approvalContractSha256: '1'.repeat(64),
          candidateDigest: '2'.repeat(64),
          issuedAtMs: 1_750_000_000_000,
          expiresAtMs: 1_750_000_030_000,
          consumed: false,
          superseded: true,
        },
      },
    } satisfies AppEventDto,
  } satisfies ClientEvent;

  assert.equal((validateClientEvent(event) as Extract<ClientEvent, { type: 'app_event' }>).event.type, 'mcp_proposal_approval_requested');
  assert.throws(
    () => validateClientEvent({ type: 'app_event', event: { type: 'totally_new_local_app_event' } }),
    /unknown app event type/,
  );
  assert.throws(
    () => validateClientEvent({
      type: 'app_event',
      event: {
        type: 'plugin_inventory_changed',
        inventory: {
          pluginId: 'lingxi-local-app',
          displayName: 'LingXi Local App',
          source: 'builtin',
          version: '2.0.0-dev',
          bundleSha256: 'a'.repeat(64),
          state: 'loaded',
          manifestDefaultEnabled: true,
          counts: { skills: 27, agents: 1, workflows: 6 },
        },
      },
    }),
    /invalid templates|unsupported fields/,
  );
});

test('shared_local_app_validator_rejects_removed_selection_event', () => {
  assert.throws(
    () =>
      validateClientEvent({
        type: 'app_event',
        event: {
          type: 'app_runtime_profile_selection_requested',
          request: { requestId: 'removed-selector' },
        },
      }),
    /unknown app event type/,
  );
});
