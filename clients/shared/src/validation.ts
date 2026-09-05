import type {
  AppEventDto,
  AppRuntimeProfileDto,
  ClientEvent,
  LocalAppCreateConfirmationRequestDto,
  LocalAppGateStatusDto,
  LocalAppMcpProposalApprovalRequestDto,
  LocalAppMcpToolSurfaceDto,
  LocalAppPluginInventoryDto,
  LocalAppVerificationStatusDto,
  ManagedLocalAppMcpServerDto,
  PluginStatusDto,
  ServerHello,
} from './protocol.js';

function object(value: unknown, name = 'payload'): Record<string, unknown> {
  if (typeof value !== 'object' || value === null || Array.isArray(value)) throw new Error(`invalid ${name}`);
  return value as Record<string, unknown>;
}

function exactKeys(value: Record<string, unknown>, allowed: readonly string[], name = 'payload'): void {
  if (Object.keys(value).some((key) => !allowed.includes(key))) {
    throw new Error(`${name} contains unsupported fields`);
  }
}

function string(value: unknown, name: string): string {
  if (typeof value !== 'string' || value.length === 0) throw new Error(`invalid ${name}`);
  return value;
}

function boolean(value: unknown, name: string): boolean {
  if (typeof value !== 'boolean') throw new Error(`invalid ${name}`);
  return value;
}

function integer(value: unknown, name: string): number {
  if (!Number.isSafeInteger(value) || (value as number) < 0) throw new Error(`invalid ${name}`);
  return value as number;
}

function optionalString(value: unknown, name: string): string | undefined {
  return value === undefined ? undefined : string(value, name);
}

function stringArray(value: unknown, name: string): string[] {
  if (!Array.isArray(value)) throw new Error(`invalid ${name}`);
  return value.map((entry, index) => string(entry, `${name}[${index}]`));
}

function verificationStatus(value: unknown, name: string): LocalAppVerificationStatusDto {
  const status = string(value, name);
  if (!['pending', 'passed', 'failed', 'unverified', 'unavailable'].includes(status)) {
    throw new Error(`invalid ${name}`);
  }
  return status as LocalAppVerificationStatusDto;
}

function runtimeProfileFamily(value: unknown, name: string): AppRuntimeProfileDto {
  const family = string(value, name);
  if (!['react_dom', 'canvas_2d', 'three_3d', 'phaser_2d', 'babylon_3d'].includes(family)) {
    throw new Error(`invalid ${name}`);
  }
  return family as AppRuntimeProfileDto;
}

function sessionMode(value: unknown, name: string): 'chat' | 'code' {
  const mode = string(value, name);
  if (mode !== 'chat' && mode !== 'code') {
    throw new Error(`invalid ${name}`);
  }
  return mode;
}

function validatePluginStatus(value: unknown): PluginStatusDto {
  const input = object(value, 'plugin status');
  exactKeys(input, ['plugin_id', 'state', 'manifest_default_enabled'], 'plugin status');
  if (input['state'] !== 'loaded' && input['state'] !== 'disabled') throw new Error('invalid plugin status state');
  return {
    plugin_id: string(input['plugin_id'], 'plugin status plugin_id'),
    state: input['state'],
    manifest_default_enabled: boolean(input['manifest_default_enabled'], 'plugin status manifest_default_enabled'),
  };
}

function validateGate(value: unknown): LocalAppGateStatusDto {
  const input = object(value, 'local app gate');
  exactKeys(input, ['gateId', 'label', 'status', 'available', 'detail'], 'local app gate');
  return {
    gateId: string(input['gateId'], 'gateId'),
    label: string(input['label'], 'label'),
    status: verificationStatus(input['status'], 'gate status'),
    available: boolean(input['available'], 'gate available'),
    ...(input['detail'] === undefined ? {} : { detail: string(input['detail'], 'gate detail') }),
  };
}

function validateToolSurface(value: unknown): LocalAppMcpToolSurfaceDto {
  const input = object(value, 'local app MCP tool');
  exactKeys(
    input,
    ['name', 'title', 'description', 'inputSchemaJson', 'outputSchemaJson', 'annotationsJson', 'executionJson', 'visibleMetaJson', 'semanticFlowJson', 'permissionCeiling'],
    'local app MCP tool',
  );
  return {
    name: string(input['name'], 'tool name'),
    ...(input['title'] === undefined ? {} : { title: string(input['title'], 'tool title') }),
    ...(input['description'] === undefined ? {} : { description: string(input['description'], 'tool description') }),
    inputSchemaJson: string(input['inputSchemaJson'], 'inputSchemaJson'),
    ...(input['outputSchemaJson'] === undefined ? {} : { outputSchemaJson: string(input['outputSchemaJson'], 'outputSchemaJson') }),
    ...(input['annotationsJson'] === undefined ? {} : { annotationsJson: string(input['annotationsJson'], 'annotationsJson') }),
    ...(input['executionJson'] === undefined ? {} : { executionJson: string(input['executionJson'], 'executionJson') }),
    ...(input['visibleMetaJson'] === undefined ? {} : { visibleMetaJson: string(input['visibleMetaJson'], 'visibleMetaJson') }),
    semanticFlowJson: string(input['semanticFlowJson'], 'semanticFlowJson'),
    permissionCeiling: string(input['permissionCeiling'], 'permissionCeiling'),
  };
}

function validateManagedMcpStatus(value: unknown): ManagedLocalAppMcpServerDto['status'] {
  const status = string(value, 'managed MCP status');
  if (!['disabled', 'needs_setup', 'authoring', 'enabled', 'needs_revalidation', 'error'].includes(status)) {
    throw new Error('invalid managed MCP status');
  }
  return status as ManagedLocalAppMcpServerDto['status'];
}

function validateMcpAppWidget(value: unknown): NonNullable<ManagedLocalAppMcpServerDto['widget']> {
  const input = object(value, 'managed MCP widget');
  exactKeys(input, ['resourceUri', 'mimeType', 'resourceSha256'], 'managed MCP widget');
  return {
    resourceUri: string(input['resourceUri'], 'resourceUri'),
    mimeType: string(input['mimeType'], 'mimeType'),
    resourceSha256: string(input['resourceSha256'], 'resourceSha256'),
  };
}

function validatePluginInventory(value: unknown): LocalAppPluginInventoryDto {
  const input = object(value, 'local app plugin inventory');
  exactKeys(
    input,
    ['pluginId', 'displayName', 'source', 'version', 'bundleSha256', 'state', 'manifestDefaultEnabled', 'counts', 'validationError'],
    'local app plugin inventory',
  );
  const counts = object(input['counts'], 'local app plugin inventory counts');
  exactKeys(counts, ['skills', 'agents', 'workflows', 'templates'], 'local app plugin inventory counts');
  if (input['state'] !== 'loaded' && input['state'] !== 'disabled') throw new Error('invalid local app plugin state');
  return {
    pluginId: string(input['pluginId'], 'pluginId'),
    displayName: string(input['displayName'], 'displayName'),
    source: string(input['source'], 'source'),
    version: string(input['version'], 'version'),
    bundleSha256: string(input['bundleSha256'], 'bundleSha256'),
    state: input['state'],
    manifestDefaultEnabled: boolean(input['manifestDefaultEnabled'], 'manifestDefaultEnabled'),
    counts: {
      skills: integer(counts['skills'], 'skills'),
      agents: integer(counts['agents'], 'agents'),
      workflows: integer(counts['workflows'], 'workflows'),
      templates: integer(counts['templates'], 'templates'),
    },
    ...(input['validationError'] === undefined ? {} : { validationError: string(input['validationError'], 'validationError') }),
  };
}

function validateCreateConfirmationRequest(value: unknown): LocalAppCreateConfirmationRequestDto {
  const input = object(value, 'create confirmation request');
  exactKeys(
    input,
    ['requestId', 'appId', 'name', 'brief', 'selectedTemplate', 'runtimeProfile', 'reason', 'rejected', 'initialTools', 'requiredGates'],
    'create confirmation request',
  );
  const template = object(input['selectedTemplate'], 'selected template');
  exactKeys(template, ['templateId', 'surface', 'summary'], 'selected template');
  const runtimeProfile = object(input['runtimeProfile'], 'runtime profile');
  exactKeys(runtimeProfile, ['family', 'revision', 'contractSha256', 'surface', 'corePackages', 'cacheStatus', 'downloadStatus', 'available', 'reason'], 'runtime profile');
  const corePackages = Array.isArray(runtimeProfile['corePackages']) ? runtimeProfile['corePackages'] : (() => { throw new Error('invalid corePackages'); })();
  return {
    requestId: string(input['requestId'], 'requestId'),
    appId: string(input['appId'], 'appId'),
    name: string(input['name'], 'name'),
    brief: string(input['brief'], 'brief'),
    selectedTemplate: {
      templateId: string(template['templateId'], 'templateId'),
      surface: string(template['surface'], 'surface') as LocalAppCreateConfirmationRequestDto['selectedTemplate']['surface'],
      summary: string(template['summary'], 'summary'),
    },
    runtimeProfile: {
      family: runtimeProfileFamily(runtimeProfile['family'], 'runtimeProfile.family'),
      revision: integer(runtimeProfile['revision'], 'runtimeProfile.revision'),
      contractSha256: string(runtimeProfile['contractSha256'], 'runtimeProfile.contractSha256'),
      surface: string(runtimeProfile['surface'], 'runtimeProfile.surface') as LocalAppCreateConfirmationRequestDto['runtimeProfile']['surface'],
      corePackages: corePackages.map((entry, index) => {
        const pkg = object(entry, `corePackage[${index}]`);
        exactKeys(pkg, ['name', 'version'], `corePackage[${index}]`);
        return { name: string(pkg['name'], `corePackage[${index}].name`), version: string(pkg['version'], `corePackage[${index}].version`) };
      }),
      cacheStatus: string(runtimeProfile['cacheStatus'], 'runtimeProfile.cacheStatus'),
      downloadStatus: string(runtimeProfile['downloadStatus'], 'runtimeProfile.downloadStatus'),
      available: boolean(runtimeProfile['available'], 'runtimeProfile.available'),
      ...(runtimeProfile['reason'] === undefined ? {} : { reason: string(runtimeProfile['reason'], 'runtimeProfile.reason') }),
    },
    reason: string(input['reason'], 'reason'),
    ...(input['rejected'] === undefined ? {} : {
      rejected: (Array.isArray(input['rejected']) ? input['rejected'] : (() => { throw new Error('invalid rejected'); })()).map((entry, index) => {
        const rejected = object(entry, `rejected[${index}]`);
        exactKeys(rejected, ['templateId', 'reason'], `rejected[${index}]`);
        return { templateId: string(rejected['templateId'], `rejected[${index}].templateId`), reason: string(rejected['reason'], `rejected[${index}].reason`) };
      }),
    }),
    ...(input['initialTools'] === undefined ? {} : {
      initialTools: (Array.isArray(input['initialTools']) ? input['initialTools'] : (() => { throw new Error('invalid initialTools'); })()).map(validateToolSurface),
    }),
    ...(input['requiredGates'] === undefined ? {} : {
      requiredGates: (Array.isArray(input['requiredGates']) ? input['requiredGates'] : (() => { throw new Error('invalid requiredGates'); })()).map(validateGate),
    }),
  };
}

function validateProposalApprovalRequest(value: unknown): LocalAppMcpProposalApprovalRequestDto {
  const input = object(value, 'proposal approval request');
  exactKeys(
    input,
    ['requestId', 'appId', 'workflowRunId', 'summary', 'proposalSha256', 'approvalContractSha256', 'toolSurfaceSha256', 'toolDiffs', 'requiredFlowChanges', 'excludedCapabilities', 'pendingGates'],
    'proposal approval request',
  );
  return {
    requestId: string(input['requestId'], 'requestId'),
    appId: string(input['appId'], 'appId'),
    workflowRunId: string(input['workflowRunId'], 'workflowRunId'),
    summary: string(input['summary'], 'summary'),
    proposalSha256: string(input['proposalSha256'], 'proposalSha256'),
    approvalContractSha256: string(input['approvalContractSha256'], 'approvalContractSha256'),
    toolSurfaceSha256: string(input['toolSurfaceSha256'], 'toolSurfaceSha256'),
    ...(input['toolDiffs'] === undefined ? {} : {
      toolDiffs: (Array.isArray(input['toolDiffs']) ? input['toolDiffs'] : (() => { throw new Error('invalid toolDiffs'); })()).map((entry, index) => {
        const diff = object(entry, `toolDiffs[${index}]`);
        exactKeys(diff, ['kind', 'name', 'before', 'after', 'changedFields'], `toolDiffs[${index}]`);
        const kind = string(diff['kind'], `toolDiffs[${index}].kind`);
        if (!['added', 'removed', 'changed'].includes(kind)) throw new Error(`invalid toolDiffs[${index}].kind`);
        return {
          kind: kind as NonNullable<LocalAppMcpProposalApprovalRequestDto['toolDiffs']>[number]['kind'],
          name: string(diff['name'], `toolDiffs[${index}].name`),
          ...(diff['before'] === undefined ? {} : { before: validateToolSurface(diff['before']) }),
          ...(diff['after'] === undefined ? {} : { after: validateToolSurface(diff['after']) }),
          ...(diff['changedFields'] === undefined ? {} : { changedFields: stringArray(diff['changedFields'], `toolDiffs[${index}].changedFields`) as NonNullable<LocalAppMcpProposalApprovalRequestDto['toolDiffs']>[number]['changedFields'] }),
        };
      }),
    }),
    ...(input['requiredFlowChanges'] === undefined ? {} : { requiredFlowChanges: stringArray(input['requiredFlowChanges'], 'requiredFlowChanges') }),
    ...(input['excludedCapabilities'] === undefined ? {} : { excludedCapabilities: stringArray(input['excludedCapabilities'], 'excludedCapabilities') }),
    ...(input['pendingGates'] === undefined ? {} : {
      pendingGates: (Array.isArray(input['pendingGates']) ? input['pendingGates'] : (() => { throw new Error('invalid pendingGates'); })()).map(validateGate),
    }),
  };
}

function validateVerificationSummary(value: unknown, name: string) {
  const input = object(value, name);
  exactKeys(input, ['status', 'summary', 'code'], name);
  return {
    status: verificationStatus(input['status'], `${name}.status`),
    summary: string(input['summary'], `${name}.summary`),
    ...(input['code'] === undefined ? {} : { code: string(input['code'], `${name}.code`) }),
  };
}

function validateManagedMcpServer(value: unknown): ManagedLocalAppMcpServerDto {
  const input = object(value, 'managed MCP server');
  exactKeys(
    input,
    [
      'serverName',
      'appId',
      'appName',
      'enabled',
      'status',
      'settingsRevision',
      'enabledTools',
      'pinnedToCurrentConversation',
      'buildId',
      'catalogSha256',
      'toolSurfaceSha256',
      'toolCount',
      'authoringRevision',
      'publicationState',
      'mcpVerification',
      'uiVerification',
      'widget',
      'tools',
    ],
    'managed MCP server',
  );
  return {
    serverName: string(input['serverName'], 'serverName'),
    appId: string(input['appId'], 'appId'),
    appName: string(input['appName'], 'appName'),
    enabled: boolean(input['enabled'], 'enabled'),
    status: validateManagedMcpStatus(input['status']),
    settingsRevision: integer(input['settingsRevision'], 'settingsRevision'),
    ...(input['enabledTools'] === undefined ? {} : { enabledTools: stringArray(input['enabledTools'], 'enabledTools') }),
    pinnedToCurrentConversation: boolean(input['pinnedToCurrentConversation'], 'pinnedToCurrentConversation'),
    buildId: string(input['buildId'], 'buildId'),
    catalogSha256: string(input['catalogSha256'], 'catalogSha256'),
    toolSurfaceSha256: string(input['toolSurfaceSha256'], 'toolSurfaceSha256'),
    toolCount: integer(input['toolCount'], 'toolCount'),
    authoringRevision: integer(input['authoringRevision'], 'authoringRevision'),
    publicationState: string(input['publicationState'], 'publicationState') as ManagedLocalAppMcpServerDto['publicationState'],
    mcpVerification: validateVerificationSummary(input['mcpVerification'], 'mcpVerification'),
    uiVerification: validateVerificationSummary(input['uiVerification'], 'uiVerification'),
    ...(input['widget'] === undefined ? {} : { widget: validateMcpAppWidget(input['widget']) }),
    ...(input['tools'] === undefined ? {} : { tools: (Array.isArray(input['tools']) ? input['tools'] : (() => { throw new Error('invalid tools'); })()).map(validateToolSurface) }),
  };
}

function validateAppEvent(value: unknown): AppEventDto {
  const input = object(value, 'app event');
  const type = string(input['type'], 'app event type');
  switch (type) {
    case 'app_details_changed':
      exactKeys(input, ['type', 'details'], 'app event');
      object(input['details'], 'app details');
      return input as AppEventDto;
    case 'app_created':
      exactKeys(input, ['type', 'record', 'request_id'], 'app event');
      object(input['record'], 'app record');
      optionalString(input['request_id'], 'request_id');
      return input as AppEventDto;
    case 'app_record_changed':
      exactKeys(input, ['type', 'record'], 'app event');
      object(input['record'], 'app record');
      return input as AppEventDto;
    case 'app_profile_proposal':
      exactKeys(input, ['type', 'proposal'], 'app event');
      object(input['proposal'], 'app profile proposal');
      return input as AppEventDto;
    case 'app_bridge_response':
      exactKeys(input, ['type', 'response'], 'app event');
      object(input['response'], 'app bridge response');
      return input as AppEventDto;
    case 'app_ui_request':
      exactKeys(input, ['type', 'request'], 'app event');
      object(input['request'], 'app UI request');
      return input as AppEventDto;
    case 'app_capability_requested':
      exactKeys(input, ['type', 'request'], 'app event');
      object(input['request'], 'app capability request');
      return input as AppEventDto;
    case 'app_dependency_change_confirmation_requested':
      exactKeys(input, ['type', 'request'], 'app event');
      object(input['request'], 'dependency confirmation request');
      return input as AppEventDto;
    case 'app_checkpoints_changed':
      exactKeys(input, ['type', 'app_id', 'checkpoints'], 'app event');
      string(input['app_id'], 'app_id');
      if (!Array.isArray(input['checkpoints'])) throw new Error('invalid checkpoints');
      return input as AppEventDto;
    case 'app_llm_activity_changed':
      exactKeys(input, ['type', 'app_id', 'active'], 'app event');
      string(input['app_id'], 'app_id');
      boolean(input['active'], 'active');
      return input as AppEventDto;
    case 'app_agent_event_posted':
      exactKeys(input, ['type', 'app_id', 'seq', 'topic', 'created_at_ms'], 'app event');
      string(input['app_id'], 'app_id');
      integer(input['seq'], 'seq');
      string(input['topic'], 'topic');
      integer(input['created_at_ms'], 'created_at_ms');
      return input as AppEventDto;
    case 'app_background_task_changed':
      exactKeys(input, ['type', 'app_id', 'task_id', 'status', 'result_json', 'error', 'retryable'], 'app event');
      string(input['app_id'], 'app_id');
      string(input['task_id'], 'task_id');
      string(input['status'], 'status');
      optionalString(input['result_json'], 'result_json');
      optionalString(input['error'], 'error');
      boolean(input['retryable'], 'retryable');
      return input as AppEventDto;
    case 'app_bridge_stream_frame':
      exactKeys(input, ['type', 'frame', 'frameJson'], 'app event');
      object(input['frame'], 'bridge stream frame');
      string(input['frameJson'], 'frameJson');
      return input as AppEventDto;
    case 'plugin_status_changed':
      exactKeys(input, ['type', 'status'], 'app event');
      return { type, status: validatePluginStatus(input['status']) };
    case 'plugin_inventory_changed':
      exactKeys(input, ['type', 'inventory'], 'app event');
      return { type, inventory: validatePluginInventory(input['inventory']) };
    case 'create_confirmation_requested':
      exactKeys(input, ['type', 'request'], 'app event');
      return { type, request: validateCreateConfirmationRequest(input['request']) };
    case 'mcp_proposal_approval_requested':
      exactKeys(input, ['type', 'request'], 'app event');
      return { type, request: validateProposalApprovalRequest(input['request']) };
    case 'managed_mcp_inventory_changed':
      exactKeys(input, ['type', 'servers'], 'app event');
      return {
        type,
        servers: (Array.isArray(input['servers']) ? input['servers'] : (() => { throw new Error('invalid servers'); })()).map(validateManagedMcpServer),
      };
    case 'verification_summary_changed':
      exactKeys(input, ['type', 'app_id', 'publication_state', 'mcp_verification', 'ui_verification'], 'app event');
      return {
        type,
        app_id: string(input['app_id'], 'app_id'),
        publication_state: string(input['publication_state'], 'publication_state') as AppEventDto & { publication_state: string }['publication_state'],
        mcp_verification: validateVerificationSummary(input['mcp_verification'], 'mcp_verification'),
        ui_verification: validateVerificationSummary(input['ui_verification'], 'ui_verification'),
      } as AppEventDto;
    case 'local_app_operation_failed': {
      exactKeys(input, ['type', 'app_id', 'code', 'message', 'request_id'], 'app event');
      const code = string(input['code'], 'code');
      if (![
        'plugin_disabled',
        'builtin_bundle_unavailable',
        'template_unavailable',
        'proposal_invalid',
        'catalog_stale',
        'active_state_corrupt',
        'revision_conflict',
        'invalid_mcp_settings',
        'mcp_authoring_required',
        'repair_budget_exhausted',
        'exposure_capacity_reached',
      ].includes(code)) {
        throw new Error('invalid local app operation error code');
      }
      return {
        type,
        ...(input['app_id'] === undefined ? {} : { app_id: string(input['app_id'], 'app_id') }),
        code: code as AppEventDto & { code: string }['code'],
        message: string(input['message'], 'message'),
        ...(input['request_id'] === undefined ? {} : { request_id: string(input['request_id'], 'request_id') }),
      } as AppEventDto;
    }
    default:
      throw new Error(`unknown app event type: ${type}`);
  }
}

export function validateClientEvent(value: unknown): ClientEvent {
  const input = object(value, 'client event');
  const type = string(input['type'], 'client event type');
  if (type === 'app_event') {
    exactKeys(input, ['type', 'event'], 'client event');
    return { type, event: validateAppEvent(input['event']) } as ClientEvent;
  }
  switch (type) {
    case 'session_started':
      exactKeys(input, ['type', 'session_id', 'mode'], 'client event');
      return {
        type,
        session_id: string(input['session_id'], 'session_id'),
        mode: sessionMode(input['mode'], 'mode'),
      } as ClientEvent;
    case 'session_resumed':
      exactKeys(input, ['type', 'session_id', 'mode', 'messages'], 'client event');
      if (!Array.isArray(input['messages'])) throw new Error('invalid messages');
      return {
        type,
        session_id: string(input['session_id'], 'session_id'),
        mode: sessionMode(input['mode'], 'mode'),
        messages: input['messages'] as ClientEvent & { messages: unknown[] }['messages'],
      } as ClientEvent;
    case 'session_forked':
      exactKeys(input, ['type', 'source_session_id', 'session_id', 'mode'], 'client event');
      return {
        type,
        source_session_id: string(input['source_session_id'], 'source_session_id'),
        session_id: string(input['session_id'], 'session_id'),
        mode: sessionMode(input['mode'], 'mode'),
      } as ClientEvent;
    case 'session_list':
      exactKeys(input, ['type', 'sessions'], 'client event');
      if (!Array.isArray(input['sessions'])) throw new Error('invalid sessions');
      for (const row of input['sessions']) {
        const item = object(row, 'session row');
        exactKeys(item, ['uuid', 'mode', 'title', 'modified_rfc3339', 'message_count', 'path'], 'session row');
        string(item['uuid'], 'session row uuid');
        sessionMode(item['mode'], 'session row mode');
        string(item['title'], 'session row title');
        string(item['modified_rfc3339'], 'session row modified_rfc3339');
        integer(item['message_count'], 'session row message_count');
        string(item['path'], 'session row path');
      }
      return input as ClientEvent;
    case 'app_sessions_changed':
      exactKeys(input, ['type', 'app_id', 'sessions', 'next_offset'], 'client event');
      string(input['app_id'], 'app_id');
      if (!Array.isArray(input['sessions'])) throw new Error('invalid sessions');
      for (const row of input['sessions']) {
        const item = object(row, 'app session row');
        exactKeys(item, ['uuid', 'mode', 'title', 'modified_rfc3339', 'message_count', 'kind'], 'app session row');
        string(item['uuid'], 'app session row uuid');
        sessionMode(item['mode'], 'app session row mode');
        string(item['title'], 'app session row title');
        string(item['modified_rfc3339'], 'app session row modified_rfc3339');
        integer(item['message_count'], 'app session row message_count');
        string(item['kind'], 'app session row kind');
      }
      if (input['next_offset'] !== undefined) integer(input['next_offset'], 'next_offset');
      return input as ClientEvent;
  }
  return input as ClientEvent;
}

export function validateServerHello(value: unknown): ServerHello {
  const input = object(value, 'ServerHello');
  exactKeys(input, ['protocol_version', 'server_name', 'capabilities'], 'ServerHello');
  const capabilities = object(input['capabilities'], 'ServerHello capabilities');
  exactKeys(
    capabilities,
    ['supports_streaming', 'supports_tools', 'supports_skills', 'supports_commands', 'client_protocol_version'],
    'ServerHello capabilities',
  );
  return {
    protocol_version: string(input['protocol_version'], 'protocol_version'),
    server_name: string(input['server_name'], 'server_name'),
    capabilities: {
      supports_streaming: boolean(capabilities['supports_streaming'], 'supports_streaming'),
      supports_tools: boolean(capabilities['supports_tools'], 'supports_tools'),
      supports_skills: boolean(capabilities['supports_skills'], 'supports_skills'),
      supports_commands: boolean(capabilities['supports_commands'], 'supports_commands'),
      client_protocol_version: string(capabilities['client_protocol_version'], 'client_protocol_version'),
    },
  };
}
