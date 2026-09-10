// M4b/M4c 曲库 UI 共享层：行模型（曲目 / 难度）、在线 meta 补丁、筛选谓词、排序比较、格式化
import {
  BeatmapStatus,
  GameMode,
  SourceKind,
  type BeatmapInfo,
  type BeatmapSetInfo,
  type OnlineSetMeta,
} from "./types";
import { t } from "./i18n";

// ---------------------------------------------------------------------------
// 行模型
// ---------------------------------------------------------------------------

/** 单条难度（扁平视图的一行），数值全部预计算 */
export interface DiffRow {
  key: number;
  setKey: number;
  set: BeatmapSetInfo;
  beatmapId: number;
  title: string;
  artist: string;
  version: string;
  creator: string;
  mode: GameMode;
  star: number | null;
  ar: number;
  od: number;
  hp: number;
  cs: number;
  bpm: number;
  totalMs: number;
  objects: number;
  /** objects / 分钟；totalMs <= 0 时为 null（不参与数值筛选，排序垫底） */
  nps: number | null;
  status: BeatmapStatus;
  dateAdded: number | null;
  // ---- 在线字段（取父集值，patch 层填充；null = 未同步/本地导入） ----
  favs: number | null;
  plays: number | null;
  rating: number | null;
  genre: string | null;
  language: string | null;
  /** 父集无有效在线 ID（-1/0/负数）：不参与同步 */
  isLocal: boolean;
  hay: string;
}

/** 谱面集行（曲目视图） */
/**
 * 展示曲名：非英文曲名后附括号内的罗马音/英文译名（与游戏内显示形态一致，
 * 如「哀の隙間 (ai no sukima)」）。两者相同（忽略大小写）或译名缺失时返回原文。
 */
export function displayTitle(row: { title: string; set: { title: string } }): string {
  const uni = row.title.trim();
  const rom = (row.set.title || "").trim();
  if (!rom || rom.toLowerCase() === uni.toLowerCase()) return uni;
  return `${uni} (${rom})`;
}

export interface SetRow {
  key: number;
  set: BeatmapSetInfo;
  title: string;
  artist: string;
  hay: string;
  diffCount: number;
  bpmMin: number;
  bpmMax: number;
  durationMs: number;
  modes: GameMode[];
  status: BeatmapStatus;
  dateAdded: number | null;
  diffs: DiffRow[];
  /** 集级在线 ID；<= 0 表示本地导入（OnlineID 缺失/-1），不参与同步 */
  onlineId: number;
  isLocal: boolean;
  // ---- 在线字段（patch 层填充；null = 未同步） ----
  favs: number | null;
  plays: number | null;
  rating: number | null;
  genre: string | null;
  language: string | null;
}

export type SortKey =
  | "title"
  | "artist"
  | "creator"
  | "diffCount"
  | "bpm"
  | "durationMs"
  | "favs"
  | "plays"
  | "rating";
export type DiffSortKey =
  | "title"
  | "version"
  | "star"
  | "ar"
  | "od"
  | "hp"
  | "cs"
  | "bpm"
  | "durationMs"
  | "objects"
  | "nps"
  | "favs"
  | "plays"
  | "rating";
export type SortDir = 1 | -1;

const collator = new Intl.Collator("zh-Hans-CN", { numeric: true, sensitivity: "base" });

/** nps = 物件数 / 分钟；守卫除零 */
export function npsOf(d: BeatmapInfo): number | null {
  if (!Number.isFinite(d.totalMs) || d.totalMs <= 0) return null;
  return d.objectCount / (d.totalMs / 60000);
}

export function buildRows(sets: BeatmapSetInfo[]): SetRow[] {
  let diffKey = 0;
  return sets.map((set, i) => {
    let bpmMin = Infinity;
    let bpmMax = 0;
    let durationMs = 0;
    const modes: GameMode[] = [];
    const title = set.titleUnicode || set.title || t("common.unknownTitle");
    const artist = set.artistUnicode || set.artist || "-";
    const onlineId = set.beatmapsetId;
    const isLocal = !(onlineId > 0); // -1 / 0 / 负数 = 本地导入，无在线数据
    const baseHay = [set.title, set.titleUnicode, set.artist, set.artistUnicode, set.creator, set.tags]
      .join(" ")
      .toLowerCase();

    const diffs: DiffRow[] = set.difficulties.map((d) => {
      if (Number.isFinite(d.bpm)) {
        if (d.bpm < bpmMin) bpmMin = d.bpm;
        if (d.bpm > bpmMax) bpmMax = d.bpm;
      }
      if (d.totalMs > durationMs) durationMs = d.totalMs;
      if (!modes.includes(d.mode)) modes.push(d.mode);
      return {
        key: diffKey++,
        setKey: i,
        set,
        beatmapId: d.beatmapId,
        title,
        artist,
        version: d.version,
        creator: d.creator,
        mode: d.mode,
        star: d.starRating != null && Number.isFinite(d.starRating) ? d.starRating : null,
        ar: d.ar,
        od: d.od,
        hp: d.hp,
        cs: d.cs,
        bpm: d.bpm,
        totalMs: d.totalMs,
        objects: d.objectCount,
        nps: npsOf(d),
        status: set.status,
        dateAdded: set.dateAdded,
        favs: null,
        plays: null,
        rating: null,
        genre: null,
        language: null,
        isLocal,
        hay: `${baseHay} ${d.version.toLowerCase()} ${d.creator.toLowerCase()}`,
      };
    });

    if (!Number.isFinite(bpmMin)) bpmMin = 0;
    return {
      key: i,
      set,
      title,
      artist,
      // 曲目搜索额外覆盖难度名（曲目视图按"任一难度命中"精神）
      hay: `${baseHay} ${diffs.map((d) => d.version.toLowerCase()).join(" ")}`,
      diffCount: set.difficulties.length,
      bpmMin,
      bpmMax,
      durationMs,
      modes,
      status: set.status,
      dateAdded: set.dateAdded,
      diffs,
      onlineId,
      isLocal,
      favs: null,
      plays: null,
      rating: null,
      genre: null,
      language: null,
    };
  });
}

// ---------------------------------------------------------------------------
// 在线 meta 补丁层（rowsBase computed 依赖 metaMap 引用，Map 替换会触发整库行重建；
// 大库可感知——优化方向：渲染期 join meta，行对象保持稳定）
// ---------------------------------------------------------------------------

export const ONLINE_FRESH_SECS = 7 * 24 * 3600;

export function isMetaFresh(meta: OnlineSetMeta | undefined, nowSec = Date.now() / 1000): boolean {
  return !!meta && nowSec - meta.fetchedAt < ONLINE_FRESH_SECS;
}

/** rating 官方对冷门集可能给 null 或 0，统一归一为 null */
function normRating(v: number | null): number | null {
  return v != null && v > 0 ? v : null;
}

export function patchRowsOnline(rows: SetRow[], metas: ReadonlyMap<number, OnlineSetMeta>): void {
  if (metas.size === 0) return;
  for (const r of rows) {
    if (r.isLocal) continue;
    const m = metas.get(r.onlineId);
    if (!m) continue;
    r.favs = m.favouriteCount;
    r.plays = m.playCount;
    r.rating = normRating(m.rating);
    r.genre = m.genre || null;
    r.language = m.language || null;
    for (const d of r.diffs) {
      d.favs = r.favs;
      d.plays = r.plays;
      d.rating = r.rating;
      d.genre = r.genre;
      d.language = r.language;
    }
  }
}

/** 已加载 metas 里去重排序的下拉选项（genre / language） */
export function distinctValues(rows: SetRow[], pick: (r: SetRow) => string | null): string[] {
  const seen = new Set<string>();
  for (const r of rows) {
    const v = pick(r);
    if (v) seen.add(v);
  }
  return [...seen].sort((a, b) => collator.compare(a, b));
}

// ---------------------------------------------------------------------------
// 筛选
// ---------------------------------------------------------------------------

export type NumField = "star" | "ar" | "od" | "hp" | "cs" | "bpm" | "dur" | "objects" | "nps" | "favs";

export interface Range {
  lo: number | null;
  hi: number | null;
}

export type DatePreset = "" | "7" | "30" | "90" | "custom";

export interface FilterState {
  modes: GameMode[];
  statuses: BeatmapStatus[];
  /** 已统一小写 */
  sources: string[];
  ranges: Record<NumField, { min: string; max: string }>;
  date: { preset: DatePreset; from: string; to: string };
  /** 在线 meta 下拉（"" = 不限；选项来自已加载 metas 去重） */
  genre: string;
  language: string;
}

const RANGE_KEYS: NumField[] = ["star", "ar", "od", "hp", "cs", "bpm", "dur", "objects", "nps", "favs"];

export const NUM_FIELDS = RANGE_KEYS;

/** 区间字段标签（筛选面板 + 生效 chip 共用）；带文案的字段走 i18n，其余为通用符号 */
export function rangeFieldLabel(f: NumField): string {
  switch (f) {
    case "dur":
      return t("range.dur");
    case "objects":
      return t("range.objects");
    case "favs":
      return t("range.favs");
    default:
      return { star: "★", ar: "AR", od: "OD", hp: "HP", cs: "CS", bpm: "BPM", nps: "nps" }[f];
  }
}

export function makeFilterState(): FilterState {
  const ranges = {} as FilterState["ranges"];
  for (const k of RANGE_KEYS) ranges[k] = { min: "", max: "" };
  return {
    modes: [],
    statuses: [],
    sources: [],
    ranges,
    date: { preset: "", from: "", to: "" },
    genre: "",
    language: "",
  };
}

export function clearFilterState(fs: FilterState) {
  fs.modes = [];
  fs.statuses = [];
  fs.sources = [];
  for (const k of RANGE_KEYS) {
    fs.ranges[k].min = "";
    fs.ranges[k].max = "";
  }
  fs.date.preset = "";
  fs.date.from = "";
  fs.date.to = "";
  fs.genre = "";
  fs.language = "";
}

export interface ParsedFilters {
  q: string;
  modes: GameMode[];
  statuses: BeatmapStatus[];
  sources: string[];
  ranges: Partial<Record<NumField, Range>>;
  dateLo: number | null;
  dateHi: number | null;
  genre: string;
  language: string;
}

export function parseFilters(fs: FilterState, q: string): ParsedFilters {
  const ranges: Partial<Record<NumField, Range>> = {};
  for (const k of RANGE_KEYS) {
    const lo = parseNum(fs.ranges[k].min);
    const hi = parseNum(fs.ranges[k].max);
    if (lo != null || hi != null) ranges[k] = { lo, hi };
  }

  let dateLo: number | null = null;
  let dateHi: number | null = null;
  if (fs.date.preset === "7" || fs.date.preset === "30" || fs.date.preset === "90") {
    dateLo = Math.floor(Date.now() / 1000) - Number(fs.date.preset) * 86400;
  } else if (fs.date.preset === "custom") {
    if (fs.date.from) {
      const t = Date.parse(`${fs.date.from}T00:00:00`);
      if (Number.isFinite(t)) dateLo = Math.floor(t / 1000);
    }
    if (fs.date.to) {
      const t = Date.parse(`${fs.date.to}T23:59:59`);
      if (Number.isFinite(t)) dateHi = Math.floor(t / 1000);
    }
  }

  return {
    q,
    modes: fs.modes,
    statuses: fs.statuses,
    sources: fs.sources,
    ranges,
    dateLo,
    dateHi,
    genre: fs.genre,
    language: fs.language,
  };
}

/** 取某难度在指定字段上的数值（null = 不可比较，直接不命中） */
export function numVal(d: DiffRow, f: NumField): number | null {
  switch (f) {
    case "star":
      return d.star;
    case "ar":
      return d.ar;
    case "od":
      return d.od;
    case "hp":
      return d.hp;
    case "cs":
      return d.cs;
    case "bpm":
      return Number.isFinite(d.bpm) ? d.bpm : null;
    case "dur":
      return d.totalMs / 1000;
    case "objects":
      return d.objects;
    case "nps":
      return d.nps;
    case "favs":
      return d.favs;
  }
}

function inRange(v: number | null, rg: Range): boolean {
  if (v == null || !Number.isFinite(v)) return false;
  if (rg.lo != null && v < rg.lo) return false;
  if (rg.hi != null && v > rg.hi) return false;
  return true;
}

function dateOk(ts: number | null, f: ParsedFilters): boolean {
  if (f.dateLo == null && f.dateHi == null) return true;
  if (ts == null) return false;
  if (f.dateLo != null && ts < f.dateLo) return false;
  if (f.dateHi != null && ts > f.dateHi) return false;
  return true;
}

/** 难度视图：逐条精确匹配 */
export function diffMatches(d: DiffRow, f: ParsedFilters): boolean {
  if (f.q && !d.hay.includes(f.q)) return false;
  if (f.modes.length && !f.modes.includes(d.mode)) return false;
  if (f.statuses.length && !f.statuses.includes(d.status)) return false;
  if (f.sources.length && !f.sources.includes(d.set.sourceKind.toLowerCase())) return false;
  if (f.genre && d.genre !== f.genre) return false;
  if (f.language && d.language !== f.language) return false;
  if (!dateOk(d.dateAdded, f)) return false;
  for (const k in f.ranges) {
    const rg = f.ranges[k as NumField];
    if (rg && !inRange(numVal(d, k as NumField), rg)) return false;
  }
  return true;
}

/** 曲目视图：集级字段直接匹配；数值区间"任一难度满足"（favs 是集级值，父集共享同一值） */
export function setMatches(r: SetRow, f: ParsedFilters): boolean {
  if (f.q && !r.hay.includes(f.q)) return false;
  if (f.modes.length && !r.modes.some((m) => f.modes.includes(m))) return false;
  if (f.statuses.length && !f.statuses.includes(r.status)) return false;
  if (f.sources.length && !f.sources.includes(r.set.sourceKind.toLowerCase())) return false;
  if (f.genre && r.genre !== f.genre) return false;
  if (f.language && r.language !== f.language) return false;
  if (!dateOk(r.dateAdded, f)) return false;
  for (const k in f.ranges) {
    const rg = f.ranges[k as NumField];
    if (rg && !r.diffs.some((d) => inRange(numVal(d, k as NumField), rg))) return false;
  }
  return true;
}

// ---------------------------------------------------------------------------
// 排序
// ---------------------------------------------------------------------------

export function compareRows(a: SetRow, b: SetRow, key: SortKey, dir: SortDir): number {
  // 在线列（可能为 null）：null 恒垫底，不受升降序影响
  if (key === "favs" || key === "plays" || key === "rating") {
    const av = key === "favs" ? a.favs : key === "plays" ? a.plays : a.rating;
    const bv = key === "favs" ? b.favs : key === "plays" ? b.plays : b.rating;
    if (av == null || bv == null) {
      return av == null && bv == null ? a.key - b.key : av == null ? 1 : -1;
    }
    const r = av - bv;
    return r !== 0 ? r * dir : a.key - b.key;
  }
  let r = 0;
  switch (key) {
    case "title":
      r = collator.compare(a.title, b.title);
      break;
    case "artist":
      r = collator.compare(a.artist, b.artist);
      break;
    case "creator":
      r = collator.compare(a.set.creator, b.set.creator);
      break;
    case "diffCount":
      r = a.diffCount - b.diffCount;
      break;
    case "bpm":
      r = a.bpmMax - b.bpmMax;
      break;
    case "durationMs":
      r = a.durationMs - b.durationMs;
      break;
  }
  return r !== 0 ? r * dir : a.key - b.key;
}

/** null 值恒排最后（不受升降序影响） */
export function sortDiffs(rows: DiffRow[], key: DiffSortKey, dir: SortDir): DiffRow[] {
  const out = [...rows];
  out.sort((a, b) => {
    let r = 0;
    switch (key) {
      case "title":
        r = collator.compare(a.title, b.title);
        break;
      case "version":
        r = collator.compare(a.version, b.version);
        break;
      case "star":
      case "nps":
      case "favs":
      case "plays":
      case "rating": {
        const av =
          key === "star" ? a.star : key === "nps" ? a.nps : key === "favs" ? a.favs : key === "plays" ? a.plays : a.rating;
        const bv =
          key === "star" ? b.star : key === "nps" ? b.nps : key === "favs" ? b.favs : key === "plays" ? b.plays : b.rating;
        if (av == null || bv == null) {
          r = av == null && bv == null ? a.key - b.key : av == null ? 1 : -1;
          return r; // null 恒垫底，不乘 dir
        }
        r = av - bv;
        break;
      }
      case "ar":
        r = a.ar - b.ar;
        break;
      case "od":
        r = a.od - b.od;
        break;
      case "hp":
        r = a.hp - b.hp;
        break;
      case "cs":
        r = a.cs - b.cs;
        break;
      case "bpm":
        r = a.bpm - b.bpm;
        break;
      case "durationMs":
        r = a.totalMs - b.totalMs;
        break;
      case "objects":
        r = a.objects - b.objects;
        break;
    }
    return r !== 0 ? r * dir : a.key - b.key;
  });
  return out;
}

// ---------------------------------------------------------------------------
// 预设档位 / 常量
// ---------------------------------------------------------------------------

export interface Band {
  label: string;
  min: string;
  max: string;
}

export const STAR_BANDS: Band[] = [
  { label: "<2", min: "", max: "2" },
  { label: "2~3", min: "2", max: "3" },
  { label: "3~4", min: "3", max: "4" },
  { label: "4~5", min: "4", max: "5" },
  { label: "5~6", min: "5", max: "6" },
  { label: "≥6", min: "6", max: "" },
];

export const AR_BANDS: Band[] = [
  { label: "≤9", min: "", max: "9" },
  { label: "9~9.5", min: "9", max: "9.5" },
  { label: "9.5~10", min: "9.5", max: "10" },
  { label: ">10", min: "10.01", max: "" },
];

export const BPM_BANDS: Band[] = [
  { label: "<120", min: "", max: "119.99" },
  { label: "120~150", min: "120", max: "150" },
  { label: "150~180", min: "150", max: "180" },
  { label: "180~210", min: "180", max: "210" },
  { label: ">210", min: "210.01", max: "" },
];

export const ALL_MODES: GameMode[] = [GameMode.Osu, GameMode.Taiko, GameMode.Catch, GameMode.Mania];
export const ALL_SOURCES: SourceKind[] = [SourceKind.Lazer, SourceKind.Stable, SourceKind.OszFolder];
export const ALL_STATUSES: BeatmapStatus[] = [
  BeatmapStatus.Ranked,
  BeatmapStatus.Approved,
  BeatmapStatus.Qualified,
  BeatmapStatus.Loved,
  BeatmapStatus.Pending,
  BeatmapStatus.Wip,
  BeatmapStatus.Graveyard,
  BeatmapStatus.Unknown,
];

// ---------------------------------------------------------------------------
// 格式化
// ---------------------------------------------------------------------------

/** 毫秒 -> "3:27" / "1:02:15" */
export function formatTime(ms: number): string {
  const s = Math.max(0, Math.floor(ms / 1000));
  const h = Math.floor(s / 3600);
  const m = Math.floor((s % 3600) / 60);
  const sec = s % 60;
  const mm = h > 0 ? String(m).padStart(2, "0") : String(m);
  return h > 0 ? `${h}:${mm}:${String(sec).padStart(2, "0")}` : `${mm}:${String(sec).padStart(2, "0")}`;
}

export function bpmText(row: SetRow): string {
  const lo = Math.round(row.bpmMin);
  const hi = Math.round(row.bpmMax);
  return lo === hi ? String(lo) : `${lo}~${hi}`;
}

/** Unix 秒 -> "2025-03-12" */
export function formatDate(ts: number | null): string {
  if (ts == null || !Number.isFinite(ts)) return "—";
  const d = new Date(ts * 1000);
  const p = (n: number) => String(n).padStart(2, "0");
  return `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())}`;
}

export function modeLabel(mode: GameMode): string {
  switch (mode) {
    case GameMode.Osu:
      return "osu!";
    case GameMode.Taiko:
      return "taiko";
    case GameMode.Catch:
      return "catch";
    case GameMode.Mania:
      return "mania";
  }
}

export function modeClass(mode: GameMode): string {
  switch (mode) {
    case GameMode.Osu:
      return "m-osu";
    case GameMode.Taiko:
      return "m-taiko";
    case GameMode.Catch:
      return "m-catch";
    case GameMode.Mania:
      return "m-mania";
  }
}

export function statusLabel(status: BeatmapStatus): string {
  switch (status) {
    case BeatmapStatus.Graveyard:
      return t("status.graveyard");
    case BeatmapStatus.Wip:
      return t("status.wip");
    case BeatmapStatus.Pending:
      return t("status.pending");
    case BeatmapStatus.Ranked:
      return t("status.ranked");
    case BeatmapStatus.Approved:
      return t("status.approved");
    case BeatmapStatus.Qualified:
      return t("status.qualified");
    case BeatmapStatus.Loved:
      return t("status.loved");
    case BeatmapStatus.Unknown:
      return t("status.unknown");
  }
}

/** 状态色（内联 style 用，避开 scoped 特异性问题）；6 位十六进制 */
const STATUS_COLORS: Record<string, string> = {
  graveyard: "#6e6c83",
  wip: "#a78bfa",
  pending: "#e8b45a",
  ranked: "#8b95ff",
  approved: "#5fc98a",
  qualified: "#e6c85f",
  loved: "#ff5fa2",
  unknown: "#565466",
};

export function statusColor(status: BeatmapStatus): string {
  return STATUS_COLORS[status] ?? STATUS_COLORS[BeatmapStatus.Unknown];
}

/** sourceKind 的 JSON 序列化大小写不稳定（rust 端为 lowercase），统一小写比较 */
export function sourceLabel(kind: string): string {
  switch (kind.toLowerCase()) {
    case "lazer":
      return "Lazer";
    case "stable":
      return "Stable";
    case "oszfolder":
      return ".osz";
    default:
      return kind;
  }
}

/** 表格/详情里的数字：整数不补零，其余保留两位 */
export function fmtNum(v: number | undefined | null, digits = 2): string {
  if (v === undefined || v === null || !Number.isFinite(v)) return "—";
  return Number.isInteger(v) ? String(v) : v.toFixed(digits);
}

export function parseNum(v: string): number | null {
  const n = parseFloat(v);
  return Number.isFinite(n) ? n : null;
}

/** 收藏/游玩等在线计数：null → "—"，大数千分位 */
export function formatCount(v: number | null): string {
  if (v == null || !Number.isFinite(v)) return "—";
  return v.toLocaleString("zh-CN");
}

/** 字节数 → 人类可读（MB/GB 保留一位） */
export function formatBytes(b: number): string {
  if (!Number.isFinite(b) || b <= 0) return "0 B";
  const units = ["B", "KB", "MB", "GB", "TB"];
  let n = b;
  let i = 0;
  while (n >= 1024 && i < units.length - 1) {
    n /= 1024;
    i++;
  }
  return `${i <= 1 ? Math.round(n) : n.toFixed(1)} ${units[i]}`;
}

// ---------------------------------------------------------------------------
// 多行 ID 输入解析（下载 / 转换工作区共用）
// ---------------------------------------------------------------------------

const ID_LINE_RE = /(^\d{1,8}$)|(beatmapsets\/(\d{1,8}))/i;

/** 每行取首个匹配：裸 ID 或 beatmapsets/<id>（自动剥 #taiko、查询串等尾部）；再兜底 CSV 第一列。
 *  去重保序，仅保留正整数。 */
export function parseIdList(text: string): number[] {
  const out: number[] = [];
  const seen = new Set<number>();
  for (const raw of text.split(/\r?\n/)) {
    const line = raw.trim();
    if (!line) continue;
    let idStr: string | null = null;
    const m = ID_LINE_RE.exec(line);
    if (m) {
      idStr = m[1] ?? m[3] ?? null;
    } else {
      // CSV / 空格分隔行：取第一列（去掉可能的引号）
      const first = line.split(/[,;\t ]/)[0]?.replace(/^"|"$/g, "") ?? "";
      if (/^\d{1,8}$/.test(first)) idStr = first;
    }
    if (!idStr) continue;
    const id = Number(idStr);
    if (id > 0 && !seen.has(id)) {
      seen.add(id);
      out.push(id);
    }
  }
  return out;
}

// ---------------------------------------------------------------------------
// 进度事件合批（下载 / 转换工作区共用）
// ---------------------------------------------------------------------------

/**
 * 把高频进度事件（后端每集每状态一条，批启动可瞬间上千条）按 key 合并为
 * 每 intervalMs 一次的批量 flush：同 key 只保留最新值。
 * Tauri 每条事件是独立 JS 回调，各自触发一次 Vue 渲染；合批后渲染次数
 * 从「事件数」降为「时间片数」，最终状态仍由 Promise 结果兜底，不丢状态。
 */
export function createProgressBatcher<K, V>(flush: (values: V[]) => void, intervalMs = 100) {
  let pending = new Map<K, V>();
  let timer: ReturnType<typeof setTimeout> | null = null;

  function drain() {
    timer = null;
    if (!pending.size) return;
    const items = [...pending.values()];
    pending = new Map();
    flush(items);
  }

  return {
    push(key: K, value: V) {
      pending.set(key, value);
      if (timer == null) {
        timer = setTimeout(drain, intervalMs);
      }
    },
    /** 丢弃未 flush 的挂起事件（批次结束/组件卸载时） */
    reset() {
      if (timer != null) {
        clearTimeout(timer);
        timer = null;
      }
      pending = new Map();
    },
  };
}
