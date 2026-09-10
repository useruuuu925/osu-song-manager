export enum SourceKind {
  Lazer = "lazer",
  Stable = "stable",
  // rust 端 #[serde(rename_all = "lowercase")] → "oszfolder"（model.rs 为准）
  OszFolder = "oszfolder",
}

export enum GameMode {
  Osu = "osu",
  Taiko = "taiko",
  Catch = "catch",
  Mania = "mania",
}

/// 谱面集在线状态（serde 小写，与 model.rs BeatmapStatus 一致）
export enum BeatmapStatus {
  Graveyard = "graveyard",
  Wip = "wip",
  Pending = "pending",
  Ranked = "ranked",
  Approved = "approved",
  Qualified = "qualified",
  Loved = "loved",
  Unknown = "unknown",
}

export interface BeatmapInfo {
  beatmapId: number;
  mode: GameMode;
  version: string;
  creator: string;
  cs: number;
  ar: number;
  od: number;
  hp: number;
  bpm: number;
  totalMs: number;
  objectCount: number;
  md5: string | null;
  starRating: number | null;
}

export interface BeatmapSetInfo {
  beatmapsetId: number;
  title: string;
  titleUnicode: string;
  artist: string;
  artistUnicode: string;
  creator: string;
  source: string;
  tags: string;
  difficulties: BeatmapInfo[];
  background: string | null;
  backgroundPath: string | null;
  audioFilename: string | null;
  sourceKind: SourceKind;
  location: string;
  status: BeatmapStatus;
  /** Unix 秒；stable/.osz 无此信息 → null */
  dateAdded: number | null;
}

export interface LibraryCandidate {
  kind: SourceKind;
  path: string;
  detail: string;
}

export interface AppConfig {
  mode?: SourceKind;
  lazerDir?: string;
  stableSongsDir?: string;
  oszDir?: string;
  lastExportDir?: string;
  lastDownloadDir?: string;
}

/** 谱面集在线元数据（online.rs 冻结契约；fetchedAt 为 Unix 秒，7 天保鲜） */
export interface OnlineSetMeta {
  beatmapsetId: number;
  favouriteCount: number;
  playCount: number;
  /** 0~100；官方对冷门集可能为 null 或 0（0 按"无值"处理） */
  rating: number | null;
  genre: string | null;
  language: string | null;
  fetchedAt: number;
}

/** onlineStatus() 返回 */
export interface OnlineStatusInfo {
  loggedIn: boolean;
  source: string; // "official" | "mirror" | "none"
  cachedCount: number;
}

/** getOauthConfig() 返回（secret 永不回传） */
export interface OauthConfigView {
  clientId: string;
  hasSecret: boolean;
}

// ---------- M5：缩略图 / 批量导出（thumbs.rs 冻结契约） ----------

export interface ThumbRequest {
  setId: number;
  /** 背景图绝对路径（lazer files\<hash> 或 stable 文件）；.osz 内条目名不算 */
  sourcePath: string;
  size: number;
}

export interface ThumbResult {
  setId: number;
  /** 缓存键 <64hex>_<size>.jpg；失败为 null */
  key: string | null;
  ok: boolean;
}

export interface ExportSetItem {
  setId: number;
  artist: string;
  title: string;
  creator: string;
  sourcePath: string;
}

export interface ExportReport {
  exported: number;
  renamed: number;
  failed: number;
  errors: string[];
}

/** export-progress 事件负载 */
export interface ExportProgressInfo {
  done: number;
  total: number;
  current: string;
}

// ---------- M6：库管理（manage.rs 冻结契约） ----------

export interface DupMember {
  setId: number;
  title: string;
  artist: string;
  creator: string;
  difficultyCount: number;
  /** stable=集文件夹绝对路径；osz=.osz 文件路径；lazer=数字 ID 字符串（非路径） */
  location: string;
  /** lazer 恒为 0（只读不触碰） */
  sizeBytes: number;
  /** 组内最老成员的 dateAdded（组级值，成员无独立日期） */
  firstAdded: number | null;
}

/** reason: "setOnlineId" | "md5" | "identicalSet" */
export interface DuplicateGroup {
  reason: string;
  label: string;
  members: DupMember[];
}

export interface FailedPath {
  path: string;
  error: string;
}

export interface TrashReport {
  deleted: string[];
  failed: FailedPath[];
}

/** moved 记录目标端最终路径（含自动改名） */
export interface MoveReport {
  moved: string[];
  failed: FailedPath[];
}

export interface PackReport {
  packed: number;
  bytes: number;
  skipped: string[];
}

// ---------- M7：镜像链下载（downloader.rs 冻结契约） ----------

/** download-progress 事件状态（serde 小写） */
export type DownloadState = "queued" | "downloading" | "validating" | "done" | "failed" | "cancelled";

export interface DownloadProgressInfo {
  setId: number;
  state: DownloadState;
  /** 当前使用的镜像；未知为 null */
  mirror: string | null;
  received: number;
  /** Content-Length；未知为 0 */
  total: number;
  error: string | null;
}

export interface DownloadResult {
  setId: number;
  ok: boolean;
  /** 落地文件完整路径 */
  path: string | null;
  mirror: string | null;
  bytes: number;
  error: string | null;
}

// ---------- M8：曲库 CSV 导出（后端冻结契约） ----------

export interface ExportCsvResult {
  /** 生成的 CSV 完整路径 */
  filePath: string;
  /** 导出的谱面集数 */
  sets: number;
  /** 难度行数 */
  rows: number;
}

// ---------- T2：lazer→stable 转换（convert.rs 冻结契约） ----------

/** convert-progress 事件状态（serde 小写） */
export type ConvertState = "queued" | "converting" | "done" | "failed" | "cancelled";

export interface ConvertProgressInfo {
  done: number;
  total: number;
  setId: number;
  state: ConvertState;
  current: string;
  error: string | null;
}

export interface ConvertSetResult {
  setId: number;
  ok: boolean;
  /** 产物路径（集文件夹或 .osz 文件） */
  output: string | null;
  files: number;
  bytes: number;
  error: string | null;
  /** 非致命警告（err.* 码） */
  warnings?: string[];
}

export interface ConvertReport {
  total: number;
  converted: number;
  failed: number;
  cancelled: boolean;
  outputDir: string;
  results: ConvertSetResult[];
}
