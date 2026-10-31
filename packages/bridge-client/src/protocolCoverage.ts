/** Inventories are generated from the same pinned Rust declarations as the
 * TypeScript unions. Snapshot tests independently compare these with goldens. */
export {
  ALL_CLIENT_COMMAND_TYPES,
  ALL_CLIENT_EVENT_TYPES,
} from './protocol.generated.js';
