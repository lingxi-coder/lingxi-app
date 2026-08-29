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
 * 单个设置文件层在磁盘上的状态。字段名与 wire 上的 `files_json` 条目一致
 * （`client-protocol/src/events.rs` 里 `[{layer, path, exists, parsed,
 * parse_error?}]`），这样桥接层解析 JSON 之后可以直接做类型断言，而不需要
 * 有人记得改字段名。`parsed` 只报告 JSON 是否解析成功，与 OS 写权限无关 ——
 * 一个 chmod 444 的文件依然会解析成功。
 */
export interface SettingsFile {
  layer: string;
  path: string;
  exists: boolean;
  parsed: boolean;
  parse_error?: string;
}

/** 已解析好的引擎设置快照。解析 wire 上的 JSON 字符串是桥接层的工作，不在这里做。 */
export interface SettingsSnapshot {
  files: SettingsFile[];
  effective: Record<string, unknown>;
  active: Record<string, unknown>;
  provenance: Record<string, string>;
  locked: string[];
  /**
   * `{layer: {key: value}}` —— 每一个文件层「自己」的原始设置 map，未经合并。
   * 对应 wire 上的 `layers_json`（`client-protocol/src/events.rs`）。`effective`
   * 是跨层合并后的值，`active` 是不含 managed 覆盖的文件层合并值 —— 两者都不能
   * 回答「L 层的文件本身写了什么」，而分层编辑器在写回某一层之前恰恰需要这个
   * 答案：`update_settings` 是整键替换、不做深度合并，所以拿 `effective` 去
   * 预合并一次写入，会把「碰巧在 effective 里赢了」的其它层数据悄悄叉进正在
   * 保存的这一层（Task 17 fix round 1 的真实 bug）。旧生产者省略这个字段时
   * 默认为 `{}`，而不是让每一层都读到 undefined。
   */
  layers: Record<string, Record<string, unknown>>;
  /**
   * 生效值是「跨层合并」而非任何单独一层的键。引擎对一组特定的键做深合并
   * 或数组并集去重（`hooks`、`permissions`、`providers`、`enabledPlugins`、
   * `trustedDirectories`… —— 见引擎的 `settings::schema::MERGE_STRATEGIES`），
   * 这类键一旦有多层同时贡献，生效值就不属于任何一层，`provenance` 里那一层
   * 只是「优先级最高的贡献者」。所以对这些键必须停止渲染单层来源徽标，改说
   * 「多层合并」—— 否则徽标本身就是假话。
   *
   * 只列出真正被合并的键：高优先层把下层条目全覆盖掉的深合并键不在其中，
   * 因为那种情况下生效值确实就是那一层的值，指名它是诚实的。旧生产者省略
   * 这个字段时默认为 `[]`（「我们不知道有任何合并」），与 `layers` 的缺省
   * 理由相同。
   */
  mergedKeys: string[];
}

/**
 * 一行设置相对于「正在编辑的层」处于什么状态。
 *
 * - `unset`：没有任何层定义这个键。
 * - `set-here`：正在编辑的层就是生效值的来源。
 * - `overridden`：正在编辑的层写了值，但更高的层赢了 —— 常态，不是边缘情况。
 * - `inherited`：正在编辑的层比当前赢家层级更高，但自己还没有写值。
 * - `locked`：managed 层钉住了这个键，不可编辑。
 * - `merged`：生效值是多层合并出来的，不属于任何一层。`locked` 一并带出来，
 *   因为一个被策略钉住的键也可能同时是合并值（引擎把 managed 层和文件层
 *   走的是同一个 merger），这时既不能画单层徽标、又必须禁用控件。
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
  | { kind: 'merged'; locked: boolean }
  | { kind: 'layer-broken'; error: string };

export function rowState(
  snapshot: SettingsSnapshot, key: string, editingLayer: Provenance,
): RowState {
  const file = snapshot.files.find((f) => f.layer === editingLayer);
  if (file?.parse_error) return { kind: 'layer-broken', error: file.parse_error };

  // 合并判定排在 `locked` 之前，是刻意的：`locked` 渲染的是 managed 单层
  // 徽标，而一个既被策略钉住、又跨层合并的键，它的生效值并不全是策略的 ——
  // 画那个徽标就是把 Task 17b 要消除的谎话换个字段说一遍。`merged` 自己带
  // `locked` 标志，所以「不可编辑」这个事实一点没丢。
  if (snapshot.mergedKeys.includes(key)) {
    return { kind: 'merged', locked: snapshot.locked.includes(key) };
  }
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
