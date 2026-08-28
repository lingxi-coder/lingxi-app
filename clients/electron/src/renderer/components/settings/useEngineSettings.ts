import type { Provenance } from './rows';

/**
 * 层优先级，高者覆盖低者。与引擎的合并顺序一致：
 * env → managed → cli → local → project → user → defaults（此处从高到低）。
 * `user` 是文件层里最低的一层 —— 这不是边缘情况，是常态：多数用户在 user
 * 层写了值，但 project 或 local 层赢了。
 */
const LAYER_RANK: Record<string, number> = {
  defaults: 0, user: 1, project: 2, local: 3, cli: 4, managed: 5, env: 6,
};

/**
 * 单个设置文件层在磁盘上的状态。字段名 `writable` 是历史命名，语义其实是
 * 「这一层的文件解析成功」（对应 wire 上的 `parsed`）—— 一个 chmod 444 的
 * 文件依然会解析成功，这个字段与 OS 写权限无关。保留这个名字是因为它是
 * 本地类型的既定契约，不是因为它准确。
 */
export interface SettingsFile {
  destination: string;
  path: string;
  exists: boolean;
  writable: boolean;
  parse_error?: string;
}

/** 已解析好的引擎设置快照。解析 wire 上的 JSON 字符串是桥接层的工作，不在这里做。 */
export interface SettingsSnapshot {
  files: SettingsFile[];
  effective: Record<string, unknown>;
  active: Record<string, unknown>;
  provenance: Record<string, string>;
  locked: string[];
}

/**
 * 一行设置相对于「正在编辑的层」处于什么状态。
 *
 * - `unset`：没有任何层定义这个键。
 * - `set-here`：正在编辑的层就是生效值的来源。
 * - `overridden`：正在编辑的层写了值，但更高的层赢了 —— 常态，不是边缘情况。
 * - `inherited`：正在编辑的层比当前赢家层级更高，但自己还没有写值。
 * - `locked`：managed 层钉住了这个键，不可编辑。
 * - `layer-broken`：正在编辑的层的文件解析失败。
 *
 * `device` 层（Electron store 里的设备级设置）从不流经这个函数 —— 它们不参与
 * 引擎层合并，UI 对它们直接渲染固定的「设备」徽标。
 */
export type RowState =
  | { kind: 'unset' }
  | { kind: 'set-here' }
  | { kind: 'overridden'; by: Provenance }
  | { kind: 'inherited'; from: Provenance }
  | { kind: 'locked' }
  | { kind: 'layer-broken'; error: string };

export function rowState(
  snapshot: SettingsSnapshot, key: string, editingLayer: Provenance,
): RowState {
  const file = snapshot.files.find((f) => f.destination === editingLayer);
  if (file?.parse_error) return { kind: 'layer-broken', error: file.parse_error };
  if (snapshot.locked.includes(key)) return { kind: 'locked' };

  const winner = snapshot.provenance[key];
  if (winner === undefined) return { kind: 'unset' };
  if (winner === editingLayer) return { kind: 'set-here' };

  const winnerRank = LAYER_RANK[winner] ?? 0;
  const editingRank = LAYER_RANK[editingLayer] ?? 0;
  return winnerRank > editingRank
    ? { kind: 'overridden', by: winner as Provenance }
    : { kind: 'inherited', from: winner as Provenance };
}

/**
 * 盘上生效值（`effective`）与运行中会话实际加载值（`active`）的差集。
 *
 * 这是「需要重启才能生效」提示的唯一诚实依据 —— 不是前端记账「用户刚点过
 * 保存」，那种记账在引擎重启后、或用户在终端里直接改了文件之后会撒谎。
 */
export function pendingKeys(snapshot: SettingsSnapshot): string[] {
  const keys = new Set([...Object.keys(snapshot.effective), ...Object.keys(snapshot.active)]);
  return [...keys].filter(
    (k) => JSON.stringify(snapshot.effective[k]) !== JSON.stringify(snapshot.active[k]),
  ).sort();
}
