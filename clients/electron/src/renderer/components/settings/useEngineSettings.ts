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

/** 已解析好的分层设置快照。解析 wire 上的 JSON 字符串是桥接层的工作，不在这里做。 */
export interface SettingsSnapshot {
  files: SettingsFile[];
  effective: Record<string, unknown>;
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
 * 引擎放项目级配置的目录名（Rust 侧的 `branding::DOT_DIR`）。客户端没有共享常量：
 * `main/host-utils.ts` 与 `main/file-search.ts` 也各自写死同一个字面量。
 */
const SETTINGS_DOT_DIR = '.lingxi';

/**
 * 引擎解析 `project` / `local` 两层时实际用的那个项目目录，或 `null`。
 *
 * 唯一可信的来源是引擎自己回传的 `files_json`。`SettingsPaths.project_dir` 在
 * bridge-server 启动时就由 `--cwd` 定死（`apps/bridge-server/src/boot.rs`），而桌面端
 * 每个会话各起一个引擎进程 —— 所以「项目层这次写到哪个目录」只有正在答题的那个引擎
 * 知道。渲染端的 `bootstrap.workspace.path` / `settings.activeProject` 是**界面**的当前
 * 项目，两者可以不是同一个（切换项目只改元数据，不重开引擎），拿它们去标注层切换器，
 * 就会在指着项目 B 的同时把值写进项目 A 的文件。
 *
 * 反推只做一件事：把路径末尾的 `<DOT_DIR>/settings.json` 去掉。后缀对不上就返回
 * `null` —— 宁可不显示项目，也不猜一个可能是错的。
 */
export function projectDirFromSnapshot(snapshot: SettingsSnapshot | null): string | null {
  const path = snapshot?.files.find((file) => file.layer === 'project')?.path;
  if (!path) return null;
  const segments = path.split(/[\\/]/);
  if (segments.length < 3) return null;
  if (segments[segments.length - 1] !== 'settings.json') return null;
  if (segments[segments.length - 2] !== SETTINGS_DOT_DIR) return null;
  // Windows 路径（`C:\proj\.lingxi\settings.json`）要拼回反斜杠；只有在路径里确实
  // 只出现反斜杠时才这么判断，混用分隔符的路径按 POSIX 处理。
  const separator = path.includes('\\') && !path.includes('/') ? '\\' : '/';
  const dir = segments.slice(0, -2).join(separator);
  return dir.length > 0 ? dir : separator;
}

/**
 * 项目目录的末段，用作人读的项目名 —— 全应用一致的约定（顶栏
 * `BetaDesktop.tsx` 与「项目与信任」页都是「末段加粗、完整路径在下」）。
 *
 * 路径本身永远要同时显示：末段会重名（两个不同项目都叫 `app`），只给名字等于
 * 又造一个「到底是哪一个」的问题，而那正是这次要消除的东西。
 */
export function projectDisplayName(dir: string): string {
  const segments = dir.split(/[\\/]/).filter(Boolean);
  return segments[segments.length - 1] ?? dir;
}

/**
 * 「这次写入还没被引擎回答」的哨兵值。
 *
 * 一次写入是从「点了保存」开始的，但那一刻还没有任何东西可以和「引擎回答之后
 * 的快照」比较 —— 命令甚至还没发出去。用一个绝不会等于任何快照事件的哨兵占住
 * 这个位置，`snapshotLandedAfter` 在命令真正发完之前就恒为 `false`，页面因此
 * 不会在自己的写入还在路上的时候，拿写入之前的状态去判定「写入没生效」。
 */
export const ATTEMPT_NOT_DISPATCHED: unique symbol = Symbol('settings-attempt-not-dispatched');

/** 一次已经发出、正在等待引擎权威回答的设置写入。 */
export interface SnapshotBoundAttempt {
  /**
   * 这次写入的命令**发完**的那一刻，界面手里的那个快照事件对象（
   * `UseBridge.settingsSnapshotEvent`，每来一个新事件就是一个新对象），或者
   * 命令还没发完时的 {@link ATTEMPT_NOT_DISPATCHED}。
   */
  readonly snapshotAtDispatch: unknown;
}

/**
 * 「比这次写入更新的快照到了吗？」—— 判定一次设置写入是否被引擎拒绝时，唯一
 * 可信的**时间**依据。
 *
 * 为什么不能用 promise：`bridge.updateEngineSettings` 只等到 `update_settings`
 * 和 `refresh_listings` 两条命令**发出去**为止（`useBridge.ts` 的
 * `updateEngineSettings`），而 `SettingsSnapshot` 是稍后才到的事件。所以
 * promise 落地的那一瞬间，页面手里的快照仍然是写入**之前**那一份 —— 任何在这
 * 一瞬间「拿请求值和快照对比」的判据，都会在一次完全成功的保存上报出「保存未
 * 生效」。它下一帧就自己消失，但屏幕阅读器每次都会念出来。
 *
 * 判据因此落在快照事件的**身份**上，而不是墙上时钟：只有当界面手里的快照对象
 * 已经不是发命令时的那一个，我们才拥有一份「引擎已经回答过」的状态，可以拿它
 * 和请求值比较。
 *
 * 残余竞态（诚实记录，不是可以靠这个函数关掉的）：协议上没有把一份快照关联回
 * 某条命令的东西（相关请求 id 需要改协议），所以一份在 `refresh_listings` 发出
 * 之前就已经在路上的快照，仍然可能被当成「更新的那一份」。把身份取在命令**发
 * 完**之后（而不是点击那一刻）已经把这个窗口压到最小。
 */
export function snapshotLandedAfter(
  attempt: SnapshotBoundAttempt | null,
  currentSnapshot: unknown,
): boolean {
  if (attempt === null) return false;
  if (attempt.snapshotAtDispatch === ATTEMPT_NOT_DISPATCHED) return false;
  return currentSnapshot !== attempt.snapshotAtDispatch;
}
