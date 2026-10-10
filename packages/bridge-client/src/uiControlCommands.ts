import type { ClientCommand, NativeUiControlRequest } from './protocol.js';

export type NativeUiControlCommand = Extract<ClientCommand, {
  type:
    | 'ui_render'
    | 'ui_client_module'
    | 'ui_message'
    | 'ui_client_fault'
    | 'ui_client_press'
    | 'ui_press'
    | 'ui_input'
    | 'ui_select';
}>;

/** Builds the correlated bridge command from an already normalized Native UI request. */
export function buildNativeUiControlCommand(
  request: NativeUiControlRequest,
  requestId: string,
): NativeUiControlCommand {
  switch (request.subtype) {
    case 'ui_client_module':
      return { type: 'ui_client_module', request_id: requestId, plugin: request.plugin };
    case 'ui_render':
      return { type: 'ui_render', request_id: requestId, request_json: JSON.stringify(request) };
    case 'ui_client_press':
      return { type: 'ui_client_press', request_id: requestId, request_json: JSON.stringify(request) };
    case 'ui_message':
      return { type: 'ui_message', request_id: requestId, request_json: JSON.stringify(request) };
    case 'ui_client_fault':
      return { type: 'ui_client_fault', request_id: requestId, request_json: JSON.stringify(request) };
    case 'ui_press':
      return { type: 'ui_press', request_id: requestId, request_json: JSON.stringify(request) };
    case 'ui_input':
      return { type: 'ui_input', request_id: requestId, request_json: JSON.stringify(request) };
    case 'ui_select':
      return { type: 'ui_select', request_id: requestId, request_json: JSON.stringify(request) };
  }
}
