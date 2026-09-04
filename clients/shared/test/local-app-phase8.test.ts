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
  assert.equal(hello.capabilities.client_protocol_version, '12.0.0');
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
    { type: 'start_local_app_mcp_authoring', app_id: 'habits-1a2b', user_goal: 'Add CRUD tools' },
    { type: 'set_local_app_mcp_enabled', app_id: 'habits-1a2b', enabled: true, expected_revision: 7 },
    { type: 'set_local_app_mcp_tool_enabled', app_id: 'habits-1a2b', tool_name: 'list_habits', enabled: false, expected_revision: 8 },
    { type: 'set_local_app_mcp_conversation_pinned', app_id: 'habits-1a2b', conversation_id: 'conv-1', pinned: true },
    { type: 'get_managed_mcp_inventory' },
  ] satisfies PluginCommandDto[];

  assert.equal(JSON.stringify(commands[0]), '{"type":"get_inventory","plugin_id":"lingxi-local-app"}');
  assert.equal(JSON.stringify(commands[1]), '{"type":"resolve_create_confirmation","request_id":"create-0001","approved":true}');
  assert.equal(JSON.stringify(commands[2]), '{"type":"resolve_mcp_proposal_approval","request_id":"proposal-0001","approved":false}');
  assert.equal(JSON.stringify(commands[3]), '{"type":"start_local_app_mcp_authoring","app_id":"habits-1a2b","user_goal":"Add CRUD tools"}');
  assert.equal(JSON.stringify(commands[4]), '{"type":"set_local_app_mcp_enabled","app_id":"habits-1a2b","enabled":true,"expected_revision":7}');
  assert.equal(JSON.stringify(commands[5]), '{"type":"set_local_app_mcp_tool_enabled","app_id":"habits-1a2b","tool_name":"list_habits","enabled":false,"expected_revision":8}');
  assert.equal(JSON.stringify(commands[6]), '{"type":"set_local_app_mcp_conversation_pinned","app_id":"habits-1a2b","conversation_id":"conv-1","pinned":true}');
  assert.equal(JSON.stringify(commands[7]), '{"type":"get_managed_mcp_inventory"}');
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

test('managed MCP inventory mirrors the current native DTO shape', () => {
  const event = validateClientEvent({
    type: 'app_event',
    event: {
      type: 'managed_mcp_inventory_changed',
      servers: [{
        serverName: 'lingxi-app-habits',
        appId: 'habits-1a2b',
        appName: 'Habits',
        enabled: true,
        status: 'enabled',
        settingsRevision: 7,
        enabledTools: ['track_habit'],
        pinnedToCurrentConversation: false,
        buildId: 'build-0001',
        catalogSha256: 'a'.repeat(64),
        toolSurfaceSha256: 'b'.repeat(64),
        toolCount: 1,
        authoringRevision: 3,
        publicationState: 'published_verified',
        mcpVerification: { status: 'passed', summary: 'MCP checks passed' },
        uiVerification: { status: 'unavailable', summary: 'UI runner unavailable on this host' },
        widget: {
          resourceUri: 'app://habits/widget',
          mimeType: 'text/html',
          resourceSha256: 'c'.repeat(64),
        },
        tools: [{
          name: 'track_habit',
          inputSchemaJson: '{"type":"object"}',
          semanticFlowJson: '{"flowId":"track-habit"}',
          permissionCeiling: 'ask',
        }],
      }],
    } satisfies AppEventDto,
  }) as Extract<ClientEvent, { type: 'app_event' }>;

  assert.equal(event.event.type, 'managed_mcp_inventory_changed');
  assert.equal(event.event.servers[0]?.status, 'enabled');
  assert.throws(
    () => validateClientEvent({
      type: 'app_event',
      event: {
        type: 'managed_mcp_inventory_changed',
        servers: [{
          serverName: 'lingxi-app-habits',
          appId: 'habits-1a2b',
          appName: 'Habits',
          enabled: true,
          status: 'enabled',
          settingsRevision: 7,
          enabledTools: ['track_habit'],
          pinnedToCurrentConversation: false,
          buildId: 'build-0001',
          catalogSha256: 'a'.repeat(64),
          toolSurfaceSha256: 'b'.repeat(64),
          toolCount: 1,
          authoringRevision: 3,
          publicationState: 'published_verified',
          mcpVerification: { status: 'passed', summary: 'ok' },
          uiVerification: { status: 'passed', summary: 'ok' },
          widget: { resourceUri: 'app://habits/widget', mimeType: 'text/html' },
        }],
      },
    }),
    /resourceSha256|unsupported fields/,
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
