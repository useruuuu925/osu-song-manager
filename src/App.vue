<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, reactive, ref, watch } from "vue";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import {
  detectLibraries,
  exportBackgrounds,
  exportLibraryCsv,
  getConfig,
  onlineCache,
  onlineFetch,
  onlineStatus,
  pickExportFolder,
  fetchOnlineBackground,
  prepareThumbnails,
  scanLibrary,
  setConfig,
  validateLibrary,
} from "./api";
import {
  cancelConvertTask,
  convertBatchError,
  convertCancelRequested,
  convertCurrent,
  convertDir,
  convertDoneCount,
  convertFinished,
  convertMode,
  convertReport,
  convertRunning,
  convertTasks,
  convertTotalCount,
  dismissConvertFinished,
  ensureConvertListener,
  setConvertDir,
  startConvert,
} from "./convertTasks";
import {
  type AppConfig,
  type BeatmapSetInfo,
  type ExportProgressInfo,
  type ExportReport,
  type ExportSetItem,
  type LibraryCandidate,
  type OnlineSetMeta,
  type OnlineStatusInfo,
  SourceKind,
  type ThumbRequest,
} from "./types";
import {
  buildRows,
  clearFilterState,
  compareRows,
  diffMatches,
  distinctValues,
  isMetaFresh,
  makeFilterState,
  modeLabel,
  NUM_FIELDS,
  parseFilters,
  patchRowsOnline,
  rangeFieldLabel,
  setMatches,
  sortDiffs,
  sourceLabel,
  statusLabel,
  type DiffRow,
  type DiffSortKey,
  type SetRow,
  type SortDir,
  type SortKey,
} from "./lib";
import { applyWindowTitle, lang, mk, setLang, sm, t, trError, type Slot, type TKey } from "./i18n";
import BeatmapTable from "./components/BeatmapTable.vue";
import DifficultyTable from "./components/DifficultyTable.vue";
import DetailPanel from "./components/DetailPanel.vue";
import FilterPanel from "./components/FilterPanel.vue";
import AuthPanel from "./components/AuthPanel.vue";
import GalleryView from "./components/GalleryView.vue";
import ManagePanel from "./components/ManagePanel.vue";
import DownloadPanel from "./components/DownloadPanel.vue";
import ConvertPanel from "./components/ConvertPanel.vue";

// ---------- 配置 / 扫描 ----------
const config = ref<AppConfig>({});
const candidates = ref<LibraryCandidate[]>([]);
const library = ref<BeatmapSetInfo[]>([]);
const loading = ref(false);
const hasScanned = ref(false);
/** 时点消息槽：存 {key,params}/{err}/null，渲染期 sm() 取词，语言切换即时回溯 */
const scanMsg = ref<Slot | null>(null);
const scanMsgKind = ref<"info" | "warn" | "error">("info");
const scanDone = ref(0);
const scanTotal = ref(0);

let unlistenProgress: UnlistenFn | null = null;

const sourceOptions = computed<{ label: string; value: SourceKind }[]>(() => [
  { label: "osu!lazer", value: SourceKind.Lazer },
  { label: "osu!stable Songs", value: SourceKind.Stable },
  { label: t("mode.oszFolder"), value: SourceKind.OszFolder },
]);

const currentMode = computed<SourceKind>(() => config.value.mode ?? SourceKind.Lazer);

const currentPath = computed<string>(() => pathForMode(currentMode.value));

function pathForMode(m: SourceKind): string {
  switch (m) {
    case SourceKind.Lazer:
      return config.value.lazerDir ?? "";
    case SourceKind.Stable:
      return config.value.stableSongsDir ?? "";
    case SourceKind.OszFolder:
      return config.value.oszDir ?? "";
  }
}

function setPathForMode(m: SourceKind, v: string) {
  switch (m) {
    case SourceKind.Lazer:
      config.value.lazerDir = v;
      break;
    case SourceKind.Stable:
      config.value.stableSongsDir = v;
      break;
    case SourceKind.OszFolder:
      config.value.oszDir = v;
      break;
  }
}

function setMode(m: SourceKind) {
  if (loading.value) return; // 扫描进行中禁止切换（模式 tab 已禁用，此处兜底）
  config.value = { ...config.value, mode: m };
  library.value = [];
  hasScanned.value = false;
  scanMsg.value = null;
  closePanel();
  expanded.value = new Set();
  resetGallery();
}

function onPathInput(e: Event) {
  setPathForMode(currentMode.value, (e.target as HTMLInputElement).value);
}

async function saveConfig() {
  await setConfig(config.value);
}

async function handleDetect() {
  try {
    candidates.value = await detectLibraries();
    // rust 端枚举序列化为小写，这里统一不区分大小写匹配
    const hit = candidates.value.find((c) => c.kind.toLowerCase() === currentMode.value.toLowerCase());
    if (hit) {
      setPathForMode(currentMode.value, hit.path);
      await saveConfig();
      // hit.detail 为后端下发的稳定 key（dt.*），渲染期经 sm→translateParams 翻译
      scanMsg.value = mk("msg.located", { 0: hit.detail });
      scanMsgKind.value = "info";
    } else {
      scanMsg.value = mk("msg.noLibrary");
      scanMsgKind.value = "warn";
    }
  } catch (e) {
    scanMsg.value = mk("msg.detectFailed", { 0: e instanceof Error ? e.message : String(e) });
    scanMsgKind.value = "error";
  }
}

async function handleScan() {
  const p = currentPath.value;
  const kindAtStart = currentMode.value;
  if (!p) {
    scanMsg.value = mk("msg.needPath");
    scanMsgKind.value = "warn";
    return;
  }
  try {
    loading.value = true;
    scanDone.value = 0;
    scanTotal.value = 0;
    scanMsg.value = mk("msg.scanning");
    scanMsgKind.value = "info";
    await validateLibrary(kindAtStart, p);
    const result = await scanLibrary(kindAtStart, p);
    // 竞态守卫：扫描期间用户改了模式或路径 → 结果已错位，丢弃
    if (currentMode.value !== kindAtStart || currentPath.value !== p) {
      scanMsg.value = mk("msg.scanStale");
      scanMsgKind.value = "warn";
      return;
    }
    library.value = result;
    hasScanned.value = true;
    page.value = 1;
    closePanel();
    expanded.value = new Set();
    resetGallery();
    scanMsg.value = mk("msg.scanDone", { 0: library.value.length });
    scanMsgKind.value = "info";
    await saveConfig();
  } catch (e) {
    scanMsg.value = { err: e };
    scanMsgKind.value = "error";
  } finally {
    loading.value = false;
  }
}

const progressPct = computed(() => {
  if (!loading.value || scanTotal.value <= 0) return "0%";
  return `${Math.min(100, Math.round((scanDone.value / scanTotal.value) * 100))}%`;
});

// ---------- M8：曲库 CSV 导出 ----------
const csvBusy = ref(false);

async function handleExportCsv() {
  if (csvBusy.value || !library.value.length) return;
  const p = currentPath.value;
  const kindAtStart = currentMode.value;
  if (!p) {
    scanMsg.value = mk("msg.needPath");
    scanMsgKind.value = "warn";
    return;
  }
  // busy 在对话框 await 前置位：防双击叠开对话框（与 ManagePanel 同款守卫）
  csvBusy.value = true;
  let dir: string | null = null;
  try {
    dir = await pickExportFolder();
  } catch (e) {
    scanMsg.value = { err: e };
    scanMsgKind.value = "error";
    return;
  }
  if (!dir) {
    csvBusy.value = false;
    return; // 取消：静默
  }
  // 对话框期间模式可能被切换：kind 与 path 必须同源
  if (currentMode.value !== kindAtStart) {
    csvBusy.value = false;
    return;
  }
  scanMsg.value = mk("msg.exportingCsv");
  scanMsgKind.value = "info";
  try {
    const r = await exportLibraryCsv(currentMode.value, p, dir);
    scanMsg.value = mk("msg.csvDone", { 0: r.sets, 1: r.rows, 2: r.filePath });
    scanMsgKind.value = "info";
  } catch (e) {
    scanMsg.value = mk("msg.csvFailed", { 0: trError(e) });
    scanMsgKind.value = "error";
  } finally {
    csvBusy.value = false;
  }
}

// ---------- 搜索（防抖 150ms） ----------
const searchInput = ref("");
const query = ref("");
let debounceTimer: ReturnType<typeof setTimeout> | undefined;

watch(searchInput, () => {
  clearTimeout(debounceTimer);
  debounceTimer = setTimeout(() => {
    query.value = searchInput.value.trim().toLowerCase();
  }, 150);
});

// ---------- 视图 / 展开 / 分页 ----------
const tableMode = ref<"set" | "diff" | "grid" | "manage" | "download" | "convert">("set");
/** 工作区（清理 / 下载 / 转换）：隐藏库视图的搜索、筛选、分页与详情侧栏 */
const isWorkspace = computed(
  () => tableMode.value === "manage" || tableMode.value === "download" || tableMode.value === "convert"
);
const showFilters = ref(false);
const expanded = ref<Set<number>>(new Set());
const page = ref(1);
const pageSize = ref(50);

function toggleExpand(key: number) {
  const next = new Set(expanded.value);
  if (next.has(key)) next.delete(key);
  else next.add(key);
  expanded.value = next;
}

// ---------- 筛选状态 ----------
const fs = reactive(makeFilterState());

function clearFilters() {
  clearFilterState(fs);
}

// ---------- 排序（曲目 / 难度 各记一份） ----------
const sortKey = ref<SortKey>("title");
const sortDir = ref<SortDir>(1);
const dSortKey = ref<DiffSortKey>("star");
const dSortDir = ref<SortDir>(-1);

const TEXT_SORT_KEYS: SortKey[] = ["title", "artist", "creator"];

function onSort(key: SortKey) {
  if (sortKey.value === key) {
    sortDir.value = sortDir.value === 1 ? -1 : 1;
  } else {
    sortKey.value = key;
    sortDir.value = TEXT_SORT_KEYS.includes(key) ? 1 : -1;
  }
}

function onDiffSort(key: DiffSortKey) {
  if (dSortKey.value === key) {
    dSortDir.value = dSortDir.value === 1 ? -1 : 1;
  } else {
    dSortKey.value = key;
    dSortDir.value = key === "title" || key === "version" ? 1 : -1;
  }
}

// ---------- 数据管线（只在状态变化时算一次） ----------
/** 在线 meta 补丁层：meta 合并后替换 Map 引用 → rowsBase 重算 → 下游自动排序/重渲染 */
const onlineMap = ref(new Map<number, OnlineSetMeta>());

const rowsBase = computed<SetRow[]>(() => {
  const rows = buildRows(library.value);
  patchRowsOnline(rows, onlineMap.value);
  return rows;
});
const allDiffs = computed<DiffRow[]>(() => rowsBase.value.flatMap((r) => r.diffs));

/** 当前曲库在线 ID 集合（下载工作区「已在库」标记；未扫描时为空集，自动跳过该功能） */
const libIds = computed(() => new Set(library.value.map((s) => s.beatmapsetId)));

const parsed = computed(() => parseFilters(fs, query.value));

const filteredSets = computed<SetRow[]>(() => {
  const f = parsed.value;
  return rowsBase.value.filter((r) => setMatches(r, f));
});

const sortedSets = computed<SetRow[]>(() => {
  const k = sortKey.value;
  const dir = sortDir.value;
  return [...filteredSets.value].sort((a, b) => compareRows(a, b, k, dir));
});

const filteredDiffs = computed<DiffRow[]>(() => {
  const f = parsed.value;
  return allDiffs.value.filter((d) => diffMatches(d, f));
});

const sortedDiffs = computed<DiffRow[]>(() => sortDiffs(filteredDiffs.value, dSortKey.value, dSortDir.value));

const totalRows = computed(() =>
  tableMode.value === "diff" ? filteredDiffs.value.length : filteredSets.value.length
);
const pageCount = computed(() => Math.max(1, Math.ceil(totalRows.value / pageSize.value)));
const pageSets = computed(() => {
  const start = (page.value - 1) * pageSize.value;
  return sortedSets.value.slice(start, start + pageSize.value);
});
const pageDiffs = computed(() => {
  const start = (page.value - 1) * pageSize.value;
  return sortedDiffs.value.slice(start, start + pageSize.value);
});

// 搜索/筛选/排序/视图切换时回到第一页
const filterSig = computed(
  () =>
    [
      query.value,
      JSON.stringify(fs),
      tableMode.value,
      sortKey.value,
      sortDir.value,
      dSortKey.value,
      dSortDir.value,
      pageSize.value,
    ].join("|")
);

watch(filterSig, () => {
  page.value = 1;
});

// ---------- 在线数据（按页惰性同步，"部分数据直接排"） ----------
const online = ref<OnlineStatusInfo | null>(null);
const syncBusy = ref(false);
const showSettings = ref(false);
let syncing = false;
let pendingSync: { ids: number[]; force: boolean } | null = null;
let syncTimer: ReturnType<typeof setTimeout> | undefined;

async function refreshOnlineStatus() {
  try {
    online.value = await onlineStatus();
  } catch {
    // 非 Tauri 环境（纯浏览器调试）
  }
}

function mergeMetas(metas: OnlineSetMeta[]) {
  if (!metas.length) return;
  const next = new Map(onlineMap.value);
  for (const m of metas) next.set(m.beatmapsetId, m);
  onlineMap.value = next;
}

function missingIds(ids: number[]): number[] {
  const now = Date.now() / 1000;
  return [...new Set(ids)].filter((id) => id > 0 && !isMetaFresh(onlineMap.value.get(id), now));
}

async function syncPage(ids: number[], force: boolean) {
  const valid = [...new Set(ids.filter((id) => id > 0))]; // 本地导入（-1/0/负）一律跳过
  if (!valid.length) return;
  if (syncing) {
    pendingSync = { ids: valid, force };
    return;
  }
  syncing = true;
  try {
    if (force) {
      syncBusy.value = true;
      mergeMetas(await onlineFetch(valid));
    } else {
      mergeMetas(await onlineCache(valid)); // 纯缓存，先即时渲染
      const need = missingIds(valid);
      if (need.length) {
        syncBusy.value = true; // 仅缺失/过期才走网络
        mergeMetas(await onlineFetch(need));
      }
    }
  } catch (e) {
    console.warn("在线同步失败", e);
  } finally {
    syncing = false;
    syncBusy.value = false;
    void refreshOnlineStatus();
    if (pendingSync) {
      const p = pendingSync;
      pendingSync = null;
      void syncPage(p.ids, p.force);
    }
  }
}

/** 当前页（曲目/画廊 = 本页集；难度 = 本页难度的父集，去重）的在线 ID */
const visibleIds = computed<number[]>(() =>
  tableMode.value === "diff"
    ? [...new Set(pageDiffs.value.map((d) => d.set.beatmapsetId))]
    : pageSets.value.map((r) => r.onlineId)
);
// 字符串签名：merge 后值不变则不再触发，避免自激循环
const visibleSig = computed(() => visibleIds.value.join(","));

watch(visibleSig, () => {
  clearTimeout(syncTimer);
  syncTimer = setTimeout(() => void syncPage(visibleIds.value, false), 250);
});

const libValid = computed(() => rowsBase.value.reduce((n, r) => n + (r.isLocal ? 0 : 1), 0));
const libFresh = computed(() => {
  const now = Date.now() / 1000;
  let n = 0;
  for (const r of rowsBase.value) {
    if (!r.isLocal && isMetaFresh(onlineMap.value.get(r.onlineId), now)) n++;
  }
  return n;
});

const sourceLabelCn = computed(() => {
  const src = online.value?.source ?? "";
  if (src === "official") return t("channel.official");
  if (src === "none") return t("channel.none");
  return t("channel.mirror");
});

const syncTip = computed(
  () =>
    t("sync.tip", {
      0: sourceLabelCn.value,
      1: online.value?.cachedCount ?? 0,
    })
);

const genreOptions = computed(() => distinctValues(rowsBase.value, (r) => r.genre));
const languageOptions = computed(() => distinctValues(rowsBase.value, (r) => r.language));

// ---------- M5：缩略图生成（画廊） ----------
const THUMB_SIZE = 256;
/** rowKey -> 缓存键；null = 生成失败（占位、不可选）；缺项 = 待生成 */
const thumbMap = ref(new Map<number, string | null>());
const thumbBusy = ref(false);
const thumbDone = ref(0);
const thumbTotal = ref(0);
const thumbRetried = new Map<number, number>(); // img 加载失败重试计数（非响应式）
let thumbTimer: ReturnType<typeof setTimeout> | undefined;
let unlistenThumb: UnlistenFn | null = null;
let unlistenExportProg: UnlistenFn | null = null;

/** lazer/stable 且有绝对背景路径才可同步/导出；.osz 条目名解包前不可用 */
function isEligible(row: SetRow): boolean {
  if (row.set.sourceKind.toLowerCase() === SourceKind.OszFolder.toLowerCase()) return false;
  const p = row.set.backgroundPath;
  return p != null && p !== "";
}

/** 可勾选：有背景源且尚未被判定失败（待生成也允许选） */
function isSelectable(row: SetRow): boolean {
  return isEligible(row) && thumbMap.value.get(row.key) !== null;
}

function applyThumbResults(targets: SetRow[], results: { setId: number; key: string | null; ok: boolean }[]) {
  const next = new Map(thumbMap.value);
  for (let i = 0; i < targets.length; i++) {
    const r = results[i];
    next.set(targets[i].key, r && r.ok && r.key ? r.key : null);
  }
  thumbMap.value = next;
  // 判失败的行退出选择
  const sel = new Set(selKeys.value);
  let changed = false;
  for (const t of targets) {
    if (next.get(t.key) === null && sel.delete(t.key)) changed = true;
  }
  if (changed) selKeys.value = sel;
}

/** 代次守卫：翻页/重扫后旧轮询直接失效，防叠加与脏写 */
let thumbGen = 0;
/** 本地+在线都失败的行：会话内重试计数（限 1 次，防死循环） */
const nullRetry = new Map<number, number>();

async function requestThumbs(only?: SetRow[]) {
  const pending = (only ?? pageSets.value).filter((r) => {
    if (!isEligible(r)) return false;
    if (!thumbMap.value.has(r.key)) return true;
    // 曾失败的行给一次重试机会（限次，防死循环）
    return thumbMap.value.get(r.key) === null && (nullRetry.get(r.key) ?? 0) < 1;
  });
  if (!pending.length) return;
  const gen = ++thumbGen;
  thumbBusy.value = true;
  thumbDone.value = 0;
  thumbTotal.value = pending.length;
  try {
    for (let i = 0; i < pending.length; i += 500) {
      if (gen !== thumbGen) return;
      const chunk = pending.slice(i, i + 500);
      const reqs: ThumbRequest[] = [];
      for (const r of chunk) {
        if (thumbMap.value.get(r.key) === null) {
          nullRetry.set(r.key, (nullRetry.get(r.key) ?? 0) + 1);
        }
        const p = r.set.backgroundPath;
        if (p) reqs.push({ setId: r.onlineId, sourcePath: p, size: THUMB_SIZE });
      }
      const results = await prepareThumbnails(reqs);
      if (gen !== thumbGen) return;
      // 成功写键；失败行按设置分流——在线兜底期间**保持 ♪ 待生成态**（不先写 null，
      // 避免"假失败"闪现），兜底失败才落最终 null
      const next = new Map(thumbMap.value);
      const onlineRetry: SetRow[] = [];
      for (let k = 0; k < chunk.length; k++) {
        const res = results[k];
        if (res && res.ok && res.key) {
          next.set(chunk[k].key, res.key);
        } else if (onlineBg.value) {
          onlineRetry.push(chunk[k]);
          next.delete(chunk[k].key);
        } else {
          next.set(chunk[k].key, null);
        }
      }
      thumbMap.value = next;
      // 在线兜底：3 路并发；成功写键，失败落最终 null（并退出选择）
      for (let j = 0; j < onlineRetry.length; j += 3) {
        if (gen !== thumbGen) return;
        const batch = onlineRetry.slice(j, j + 3);
        await Promise.all(
          batch.map(async (r) => {
            try {
              const res = await fetchOnlineBackground(r.onlineId, THUMB_SIZE);
              if (gen !== thumbGen) return;
              const m = new Map(thumbMap.value);
              if (res && res.ok && res.key) {
                m.set(r.key, res.key);
              } else {
                m.set(r.key, null);
                const sel = new Set(selKeys.value);
                if (sel.delete(r.key)) selKeys.value = sel;
              }
              thumbMap.value = m;
            } catch {
              if (gen !== thumbGen) return;
              const m = new Map(thumbMap.value);
              m.set(r.key, null);
              thumbMap.value = m;
            }
          })
        );
      }
      if (gen !== thumbGen) return;
      thumbDone.value = i + chunk.length;
    }
  } catch (e) {
    console.warn("缩略图生成失败", e);
  } finally {
    if (gen === thumbGen) thumbBusy.value = false;
  }
}

/** 404 → 重新生成一次并重挂 <img>（先删条目渲染占位，再写回强制重新拉取） */
async function onImgError(row: SetRow) {
  // 代次守卫：重扫/切模式后已在飞的网络回写不得污染新库的 thumbMap
  const gen = thumbGen;
  const n = (thumbRetried.get(row.key) ?? 0) + 1;
  thumbRetried.set(row.key, n);
  const p = row.set.backgroundPath;
  const strip = new Map(thumbMap.value);
  strip.delete(row.key);
  thumbMap.value = strip;
  if (gen !== thumbGen) return;
  if (n > 1 || !p) {
    // 本地重试仍失败 → 按设置从官方 CDN 兜底
    if (p && onlineBg.value) {
      try {
        const res = await fetchOnlineBackground(row.onlineId, THUMB_SIZE);
        if (gen !== thumbGen) return;
        if (res && res.ok && res.key) {
          const next = new Map(thumbMap.value);
          next.set(row.key, res.key);
          thumbMap.value = next;
          return;
        }
      } catch {
        // 网络失败继续落失败占位
      }
    }
    const next = new Map(thumbMap.value);
    next.set(row.key, null);
    thumbMap.value = next;
    const sel = new Set(selKeys.value);
    sel.delete(row.key);
    selKeys.value = sel;
    return;
  }
  if (gen !== thumbGen) return;
  try {
    const results = await prepareThumbnails([{ setId: row.onlineId, sourcePath: p, size: THUMB_SIZE }]);
    if (gen !== thumbGen) return;
    applyThumbResults([row], results);
  } catch {
    const next = new Map(thumbMap.value);
    next.set(row.key, null);
    thumbMap.value = next;
  }
}

// 进入画廊 / 翻页 / 换筛选后 250ms，为本页缺键的行生成缩略图
const gridSig = computed(() =>
  tableMode.value === "grid" && !loading.value
    ? pageSets.value.filter(isEligible).map((r) => r.key).join(",")
    : ""
);

watch(gridSig, (sig) => {
  if (!sig) return;
  clearTimeout(thumbTimer);
  thumbTimer = setTimeout(() => void requestThumbs(), 250);
});

function resetGallery() {
  thumbMap.value = new Map();
  thumbRetried.clear();
  nullRetry.clear();
  songSel.value = new Set();
  selKeys.value = new Set();
  lastExported.value = new Set();
  exportReport.value = null;
  exportProg.value = null;
  clearTimeout(thumbTimer);
}

// ---------- M5：批量导出背景图 ----------
const selKeys = ref(new Set<number>());
// 曲目视图多选（转换所选；与画廊 selKeys 平行，互不影响）
const songSel = ref(new Set<number>());
const songSelCount = computed(() => songSel.value.size);
function toggleSongSelect(row: SetRow) {
  const next = new Set(songSel.value);
  if (next.has(row.key)) next.delete(row.key);
  else next.add(row.key);
  songSel.value = next;
}
function toggleSongPage() {
  const allSelected = pageSets.value.every((r) => songSel.value.has(r.key));
  const next = new Set(songSel.value);
  for (const r of pageSets.value) {
    if (allSelected) next.delete(r.key);
    else next.add(r.key);
  }
  songSel.value = next;
}
const showConvertConfirm = ref(false);
const showConvDetail = ref(false);
const convertConfirmIds = computed<number[]>(() =>
  rowsBase.value
    .filter((r) => songSel.value.has(r.key) && r.onlineId > 0)
    .map((r) => r.onlineId)
    .slice(0, 2000)
);
async function startConvertSelected() {
  await ensureConvertListener();
  const ok = await startConvert(convertConfirmIds.value);
  if (ok) showConvertConfirm.value = false;
}
// 切模式/重扫清空曲目多选
watch(currentMode, () => {
  songSel.value = new Set();
});
const selectedRows = computed(() => rowsBase.value.filter((r) => selKeys.value.has(r.key)));
const selOffPage = computed(() => {
  const pk = new Set(pageSets.value.map((r) => r.key));
  return selectedRows.value.filter((r) => !pk.has(r.key)).length;
});

function toggleSelect(row: SetRow) {
  const next = new Set(selKeys.value);
  if (next.has(row.key)) next.delete(row.key);
  else next.add(row.key);
  selKeys.value = next;
}

function selectAllPage() {
  const next = new Set(selKeys.value);
  for (const r of pageSets.value) {
    if (isSelectable(r)) next.add(r.key);
  }
  selKeys.value = next;
}

const exporting = ref(false);
const exportProg = ref<ExportProgressInfo | null>(null);
const exportReport = ref<ExportReport | null>(null);
const errorsOpen = ref(false);
const lastExported = ref(new Set<number>());

const exportPct = computed(() => {
  const p = exportProg.value;
  if (!p || p.total <= 0) return "0%";
  return `${Math.min(100, Math.round((p.done / p.total) * 100))}%`;
});

async function handleExport() {
  if (exporting.value || !selKeys.value.size) return;
  // busy 在对话框 await 前置位：防双击叠开对话框
  exporting.value = true;
  let dir: string | null = null;
  try {
    dir = await pickExportFolder();
  } catch (e) {
    exportReport.value = { exported: 0, renamed: 0, failed: 0, errors: [e instanceof Error ? e.message : String(e)] };
    exporting.value = false;
    return;
  }
  if (!dir) {
    exporting.value = false;
    return; // 用户取消，静默
  }
  const items: ExportSetItem[] = [];
  const targets: SetRow[] = [];
  for (const r of selectedRows.value) {
    const p = r.set.backgroundPath;
    if (!p) continue;
    items.push({ setId: r.onlineId, artist: r.artist, title: r.title, creator: r.set.creator, sourcePath: p });
    targets.push(r);
  }
  if (!items.length) return;
  exporting.value = true;
  exportReport.value = null;
  exportProg.value = { done: 0, total: items.length, current: "" };
  try {
    const rep = await exportBackgrounds(items, dir);
    exportReport.value = rep;
    // 后端只回计数，逐卡 ✓ 按批次标记（失败条目见错误列表）
    lastExported.value = rep.exported > 0 ? new Set(targets.map((t) => t.key)) : new Set();
    errorsOpen.value = rep.errors.length > 0;
  } catch (e) {
    exportReport.value = {
      exported: 0,
      renamed: 0,
      failed: items.length,
      errors: [e instanceof Error ? e.message : String(e)],
    };
  } finally {
    exporting.value = false;
    exportProg.value = null;
  }
}

// ---------- 生效筛选 chip ----------
interface ActiveChip {
  label: string;
  clear: () => void;
}

const activeChips = computed<ActiveChip[]>(() => {
  const chips: ActiveChip[] = [];
  for (const m of fs.modes) {
    chips.push({
      label: t("chip.mode", { 0: modeLabel(m) }),
      clear: () => {
        const i = fs.modes.indexOf(m);
        if (i >= 0) fs.modes.splice(i, 1);
      },
    });
  }
  for (const s of fs.statuses) {
    chips.push({
      label: t("chip.status", { 0: statusLabel(s) }),
      clear: () => {
        const i = fs.statuses.indexOf(s);
        if (i >= 0) fs.statuses.splice(i, 1);
      },
    });
  }
  for (const s of fs.sources) {
    chips.push({
      label: t("chip.source", { 0: sourceLabel(s) }),
      clear: () => {
        const i = fs.sources.indexOf(s);
        if (i >= 0) fs.sources.splice(i, 1);
      },
    });
  }
  for (const k of NUM_FIELDS) {
    const r = fs.ranges[k];
    if (r.min || r.max) {
      chips.push({
        label: t("chip.range", { 0: rangeFieldLabel(k), 1: r.min || "…", 2: r.max || "…" }),
        clear: () => {
          r.min = "";
          r.max = "";
        },
      });
    }
  }
  if (fs.genre) {
    chips.push({
      label: t("chip.genre", { 0: fs.genre }),
      clear: () => {
        fs.genre = "";
      },
    });
  }
  if (fs.language) {
    chips.push({
      label: t("chip.language", { 0: fs.language }),
      clear: () => {
        fs.language = "";
      },
    });
  }
  if (fs.date.preset === "7" || fs.date.preset === "30" || fs.date.preset === "90") {
    chips.push({
      label: t("chip.dateRecent", { 0: fs.date.preset }),
      clear: () => {
        fs.date.preset = "";
      },
    });
  } else if (fs.date.preset === "custom") {
    chips.push({
      label: t("chip.dateCustom", { 0: fs.date.from || "…", 1: fs.date.to || "…" }),
      clear: () => {
        fs.date.preset = "";
        fs.date.from = "";
        fs.date.to = "";
      },
    });
  }
  return chips;
});

// ---------- 详情面板 ----------
const selectedKey = ref<number | null>(null);
const selectedDiffKey = ref<number | null>(null);
const focusBeatmapId = ref<number | null>(null);

/** 详情面板背景预览地址：复用画廊缩略图缓存 */
const detailThumbUrl = computed(() => {
  const r = selectedRow.value;
  if (!r) return null;
  const k = thumbMap.value.get(r.key);
  return typeof k === "string" ? `http://thumb.localhost/${k}` : null;
});

// 面板打开/换行时确保该集缩略图已生成（只生成这一行，避免整页背景风暴）
watch(
  () => selectedRow.value?.key,
  (key) => {
    if (key == null || !panelOpen.value || !selectedRow.value) return;
    if (!isEligible(selectedRow.value)) return;
    void requestThumbs([selectedRow.value]);
  }
);

const selectedRow = computed<SetRow | null>(() => {
  if (selectedKey.value == null) return null;
  return rowsBase.value.find((r) => r.key === selectedKey.value) ?? null;
});

const panelOpen = computed(() => selectedKey.value != null);

function selectSet(row: SetRow) {
  selectedKey.value = row.key;
  selectedDiffKey.value = null;
  focusBeatmapId.value = null;
}

function selectDiff(row: DiffRow) {
  selectedKey.value = row.setKey;
  selectedDiffKey.value = row.key;
  focusBeatmapId.value = row.beatmapId;
}

function closePanel() {
  selectedKey.value = null;
  selectedDiffKey.value = null;
  focusBeatmapId.value = null;
}

// ---------- 界面字号（CSS zoom 整体缩放，WebView2=Chromium 原生支持） ----------
// zoom 挂在根节点上，px 样式等比缩放；vh/vw 在缩放子树内按规范已除以缩放比，
// 故 .app 的 height:100vh 仍恰好铺满视口，sticky 表头与 fixed 弹窗不受影响。
const ZOOM_STEPS = [0.85, 0.925, 1, 1.1, 1.225, 1.35];
const ZOOM_KEY = "osu-mgr.ui-zoom";
// 画廊缺失背景从 osu! 官方 CDN 兜底拉取（用户可关）
const ONLINE_BG_KEY = "osu-mgr.onlineBg";
const onlineBg = ref(true);
try {
  onlineBg.value = localStorage.getItem(ONLINE_BG_KEY) !== "0";
} catch {
  // 存储不可用时保持默认开
}
function setOnlineBg(v: boolean) {
  onlineBg.value = v;
  try {
    localStorage.setItem(ONLINE_BG_KEY, v ? "1" : "0");
  } catch {
    // 存储失败不影响本次会话
  }
}
const zoomIdx = ref(2);
const uiZoom = computed(() => ZOOM_STEPS[zoomIdx.value]);
const zoomLabel = computed(() => t(`zoom.${zoomIdx.value}` as TKey));

function stepZoom(dir: number) {
  const next = Math.min(ZOOM_STEPS.length - 1, Math.max(0, zoomIdx.value + dir));
  if (next === zoomIdx.value) return;
  zoomIdx.value = next;
  try {
    localStorage.setItem(ZOOM_KEY, String(next));
  } catch {
    // 存储失败不影响本次会话
  }
  // 缩放比变了，"窗口 70%" 折算成 CSS px 的上限随之变化 → 重新夹紧
  panelW.value = clampPanelW(panelW.value);
}

// ---------- 详情面板宽度拖拽（类分屏；表格区靠 flex 自适应剩余宽度，纯 CSS 重排） ----------
const PANEL_DEFAULT_W = 420;
const PANEL_MIN_W = 300;
const PANEL_W_KEY = "osu-mgr.detail-width";
const panelW = ref(PANEL_DEFAULT_W);
const resizing = ref(false);
let dragStartX = 0; // clientX 为视口屏幕像素，不受 zoom 影响
let dragStartW = 0; // 拖拽起始宽度（CSS px）
let dragZoom = 1; // 拖拽期间锁定缩放比
let resizeRaf = 0;
let pendingX = 0;

/**
 * 面板宽度上限（CSS px）：面板屏幕宽 = w × zoom，不得超过 innerWidth 的 70%，
 * 故换算为 CSS px 时除以缩放比；窗口极窄时保底最小宽，保证 min ≤ max。
 */
function panelMaxW(): number {
  return Math.max(PANEL_MIN_W, (window.innerWidth * 0.7) / uiZoom.value);
}

function clampPanelW(w: number): number {
  if (!Number.isFinite(w)) return PANEL_DEFAULT_W;
  return Math.min(Math.max(w, PANEL_MIN_W), panelMaxW());
}

function savePanelW() {
  try {
    localStorage.setItem(PANEL_W_KEY, String(Math.round(panelW.value)));
  } catch {
    // 忽略
  }
}

/** 每帧最多一次：把最新 clientX 换成一个宽度数字写入 ref，其余全部交给浏览器布局 */
function onResizeMove(ev: PointerEvent) {
  pendingX = ev.clientX;
  if (resizeRaf) return;
  resizeRaf = requestAnimationFrame(() => {
    resizeRaf = 0;
    const screenW = dragStartW * dragZoom + (dragStartX - pendingX); // 向左拖 = 变宽（屏幕 px）
    panelW.value = clampPanelW(screenW / dragZoom); // 换回 zoom 内的 CSS px
  });
}

function endResize() {
  resizing.value = false;
  document.body.classList.remove("ui-resizing");
  window.removeEventListener("pointermove", onResizeMove);
  window.removeEventListener("pointerup", endResize);
  if (resizeRaf) {
    cancelAnimationFrame(resizeRaf);
    resizeRaf = 0;
  }
  savePanelW();
}

function startResize(e: PointerEvent) {
  if (e.button !== 0) return;
  e.preventDefault();
  resizing.value = true;
  document.body.classList.add("ui-resizing");
  dragStartX = e.clientX;
  dragStartW = panelW.value;
  dragZoom = uiZoom.value;
  window.addEventListener("pointermove", onResizeMove);
  window.addEventListener("pointerup", endResize);
}

function resetPanelW() {
  panelW.value = clampPanelW(PANEL_DEFAULT_W);
  savePanelW();
}

function onWindowResize() {
  panelW.value = clampPanelW(panelW.value);
}

function onKeydown(e: KeyboardEvent) {
  if (e.key === "Escape") {
    if (showConvertConfirm.value) {
      showConvertConfirm.value = false;
    } else if (showSettings.value) {
      showSettings.value = false;
    } else if (panelOpen.value) {
      closePanel();
    }
  }
}

// ---------- 生命周期 ----------
onMounted(async () => {
  window.addEventListener("keydown", onKeydown);
  window.addEventListener("resize", onWindowResize);
  void applyWindowTitle();
  // 恢复字号与面板宽度（宽度按当前 zoom 下重新夹紧）
  try {
    const z = Number.parseInt(localStorage.getItem(ZOOM_KEY) ?? "", 10);
    if (Number.isInteger(z) && z >= 0 && z < ZOOM_STEPS.length) zoomIdx.value = z;
    const w = Number.parseFloat(localStorage.getItem(PANEL_W_KEY) ?? "");
    if (Number.isFinite(w)) panelW.value = clampPanelW(w);
  } catch {
    // 存储不可用时用默认值
  }
  try {
    unlistenProgress = await listen<[number, number]>("scan-progress", (ev) => {
      scanDone.value = ev.payload[0];
      scanTotal.value = ev.payload[1];
    });
    unlistenThumb = await listen<[number, number]>("thumb-progress", (ev) => {
      thumbDone.value = ev.payload[0];
      thumbTotal.value = ev.payload[1];
    });
    unlistenExportProg = await listen<ExportProgressInfo>("export-progress", (ev) => {
      exportProg.value = ev.payload;
    });
  } catch {
    // 非 Tauri 环境（纯浏览器调试）下忽略进度事件
  }
  config.value = await getConfig();
  candidates.value = await detectLibraries();
  if (!config.value.mode && candidates.value.length) {
    const first = candidates.value[0];
    config.value.mode = first.kind;
    setPathForMode(first.kind, first.path);
  }
  void refreshOnlineStatus();
});

onBeforeUnmount(() => {
  window.removeEventListener("keydown", onKeydown);
  window.removeEventListener("resize", onWindowResize);
  window.removeEventListener("pointermove", onResizeMove);
  window.removeEventListener("pointerup", endResize);
  document.body.classList.remove("ui-resizing");
  unlistenProgress?.();
  unlistenThumb?.();
  unlistenExportProg?.();
  clearTimeout(syncTimer);
  clearTimeout(thumbTimer);
});
</script>

<template>
  <div class="app" :style="{ zoom: uiZoom }">
    <header class="topbar">
      <div class="brand"><span class="brand-dot"></span>{{ t("app.title") }}</div>
      <nav class="mode-tabs">
        <button
          v-for="opt in sourceOptions"
          :key="opt.value"
          :class="{ active: currentMode === opt.value }"
          :disabled="loading"
          :title="loading ? t('action.scanning') : undefined"
          @click="setMode(opt.value)"
        >
          {{ opt.label }}
        </button>
      </nav>
    </header>

    <section class="pathbar">
      <button class="btn" @click="handleDetect">{{ t("action.autoDetect") }}</button>
      <button
        v-show="!isWorkspace"
        class="btn"
        :disabled="!library.length || csvBusy"
        :title="!library.length ? t('title.needScan') : t('title.exportCsv')"
        @click="handleExportCsv()"
      >
        {{ csvBusy ? t("action.exporting") : t("action.exportCsv") }}
      </button>
      <input
        class="path-input"
        :value="currentPath"
        :placeholder="
          currentMode === SourceKind.Lazer
            ? t('ph.lazer')
            : currentMode === SourceKind.Stable
              ? t('ph.stable')
              : t('ph.osz')
        "
        @input="onPathInput"
        @blur="saveConfig"
      />
      <button class="btn primary" :disabled="loading" @click="handleScan">
        {{ loading ? t("action.scanning") : t("action.scan") }}
      </button>
      <div class="scan-status">
        <div v-if="loading" class="pbar" :title="`${scanDone} / ${scanTotal}`">
          <div class="pfill" :style="{ width: progressPct }"></div>
        </div>
        <span v-if="loading" class="msg info">{{ scanDone }} / {{ scanTotal }}</span>
        <span v-else-if="scanMsg" class="msg" :class="scanMsgKind" :title="sm(scanMsg)">{{ sm(scanMsg) }}</span>
      </div>
    </section>

    <section class="toolbar">
      <div class="tb-left">
        <input
          v-show="!isWorkspace"
          v-model="searchInput"
          class="search"
          :placeholder="t('ph.search')"
        />
        <div class="zoom-ctl" :title="t('ui.zoomTitle', { 0: Math.round(uiZoom * 100) })">
          <span class="zc-label">{{ t("ui.fontSize") }}</span>
          <button class="zc-btn" :disabled="zoomIdx === 0" :aria-label="t('ui.zoomOut')" @click="stepZoom(-1)">−</button>
          <span class="zc-step">{{ zoomLabel }}</span>
          <button class="zc-btn" :disabled="zoomIdx === ZOOM_STEPS.length - 1" :aria-label="t('ui.zoomIn')" @click="stepZoom(1)">+</button>
        </div>
      </div>
      <div class="tb-right">
        <span class="sync-chip" :title="syncTip">
          <span
            class="dot"
            :class="online?.source === 'official' ? 'dot-official' : online?.source === 'none' ? 'dot-none' : 'dot-mirror'"
          ></span>
          {{ t("sync.synced", { 0: libFresh, 1: libValid }) }}
          <span v-if="syncBusy" class="spin" :title="t('sync.fetching')">⟳</span>
        </span>
        <button
          v-show="!isWorkspace"
          class="btn"
          :class="{ active: showFilters }"
          :aria-expanded="showFilters"
          @click="showFilters = !showFilters"
        >
          {{ t("action.filter") }}<span v-if="activeChips.length" class="badge">{{ activeChips.length }}</span>
          <span class="caret">{{ showFilters ? "▲" : "▼" }}</span>
        </button>
        <div class="seg">
          <button :class="{ active: tableMode === 'set' }" @click="tableMode = 'set'">{{ t("view.sets") }}</button>
          <button :class="{ active: tableMode === 'diff' }" @click="tableMode = 'diff'">{{ t("view.diffs") }}</button>
          <button :class="{ active: tableMode === 'grid' }" @click="tableMode = 'grid'">{{ t("view.gallery") }}</button>
          <button :class="{ active: tableMode === 'manage' }" @click="tableMode = 'manage'">{{ t("view.manage") }}</button>
          <button :class="{ active: tableMode === 'download' }" @click="tableMode = 'download'">{{ t("view.download") }}</button>
          <button :class="{ active: tableMode === 'convert' }" @click="tableMode = 'convert'">{{ t("view.convert") }}</button>
        </div>
        <div class="settings-wrap">
          <button
            class="btn"
            :class="{ active: showSettings }"
            :aria-expanded="showSettings"
            @click="showSettings = !showSettings"
          >
            {{ t("action.settings") }}
          </button>
          <div v-if="showSettings" class="pop-backdrop" @click="showSettings = false"></div>
          <div v-if="showSettings" class="settings-pop">
            <div class="lang-row">
              <span class="lang-label">{{ t("settings.language") }}</span>
              <button class="btn sm lang-btn" :class="{ active: lang === 'zh' }" @click="setLang('zh')">
                {{ t("lang.zh") }}
              </button>
              <button class="btn sm lang-btn" :class="{ active: lang === 'en' }" @click="setLang('en')">
                {{ t("lang.en") }}
              </button>
            </div>
            <div class="online-row">
              <div class="lang-row">
                <span class="lang-label">{{ t("settings.onlineBg") }}</span>
                <button
                  class="btn sm lang-btn"
                  :class="{ active: onlineBg }"
                  :aria-pressed="onlineBg"
                  @click="setOnlineBg(!onlineBg)"
                >
                  {{ onlineBg ? t("common.on") : t("common.off") }}
                </button>
              </div>
              <p class="online-hint">{{ t("settings.onlineBgHint") }}</p>
            </div>
            <AuthPanel :logged-in="online?.loggedIn ?? false" @change="refreshOnlineStatus()" />
          </div>
        </div>
      </div>
    </section>

    <!-- 全局转换状态条：所有视图可见，转换中/完成态常驻（✕ 关闭完成态） -->
    <div v-if="convertRunning || convertFinished" class="conv-bar">
      <template v-if="convertRunning">
        <span class="conv-bar-text">
          {{ t("cv.runningBar", { 0: convertDoneCount, 1: convertTotalCount }) }}
          <span class="dim">{{ t("cv.currentSet", { 0: convertCurrent }) }}</span>
        </span>
        <button class="link-btn" :disabled="convertCancelRequested" @click="cancelConvertTask()">
          {{ convertCancelRequested ? t("dl.cancelling") : t("action.cancel") }}
        </button>
      </template>
      <template v-else>
        <span class="conv-bar-text">
          {{
            convertReport
              ? t("cv.summary", {
                  0: convertReport.converted,
                  1: convertReport.failed,
                  2: convertReport.cancelled ? t("cv.cancelledSuffix") : "",
                })
              : ""
          }}
        </span>
        <button class="link-btn" :aria-label="t('detail.closeAria')" @click="dismissConvertFinished()">✕</button>
      </template>
      <button class="link-btn" @click="showConvDetail = !showConvDetail">
        {{ showConvDetail ? t("cv.hideDetail") : t("cv.showDetail") }}
      </button>
      <span v-if="convertBatchError" class="conv-err">{{ sm(convertBatchError) }}</span>
    </div>
    <div v-if="showConvDetail && convertTasks.length" class="conv-detail">
      <div v-for="task in convertTasks" :key="task.setId" class="conv-task">
        <span class="ct-id">{{ task.setId }}</span>
        <span class="ct-state" :class="`s-${task.state}`">{{ task.state }}</span>
        <span class="ct-name" :title="task.current">{{ task.current }}</span>
        <span v-if="task.error" class="ct-err">{{ trError(task.error) }}</span>
      </div>
    </div>

    <FilterPanel
      v-if="showFilters && !isWorkspace"
      :fs="fs"
      :genre-options="genreOptions"
      :language-options="languageOptions"
    />

    <section v-if="activeChips.length && !isWorkspace" class="chips-bar">
      <button
        v-for="(chip, i) in activeChips"
        :key="i"
        type="button"
        class="chip"
        @click="chip.clear()"
      >
        {{ chip.label }}<span class="chip-x" aria-hidden="true">✕</span>
      </button>
      <button class="chip-clear" @click="clearFilters()">{{ t("action.clearFilters") }}</button>
    </section>

    <div class="body">
      <main class="content">
        <div v-show="!isWorkspace" class="count-line">
          <span>
            <template v-if="tableMode === 'diff'">{{ t("count.diffs", { 0: filteredDiffs.length, 1: allDiffs.length }) }}</template>
            <template v-else>{{ t("count.sets", { 0: filteredSets.length, 1: library.length }) }}</template>
          </span>
          <button
            v-if="library.length"
            class="refetch"
            :disabled="syncBusy"
            :title="t('title.refetch')"
            @click="syncPage(visibleIds, true)"
          >
            {{ t("action.refetch") }}
          </button>
          <button
            v-if="currentMode === SourceKind.Lazer && tableMode === 'set' && songSelCount > 0"
            class="refetch conv-pick"
            :disabled="convertRunning"
            @click="showConvertConfirm = true"
          >
            {{ t("cv.convertSelected", { 0: songSelCount }) }}
          </button>
        </div>

        <!-- 转换所选确认弹层 -->
        <div v-if="showConvertConfirm" class="pop-backdrop" @click="showConvertConfirm = false"></div>
        <div v-if="showConvertConfirm" class="conv-confirm">
          <p class="conv-title">{{ t("cv.confirmTitle") }}</p>
          <p class="conv-body">{{ t("cv.confirmBody", { 0: convertConfirmIds.length }) }}</p>
          <div class="conv-row">
            <button class="refetch" :class="{ on: convertMode === 'songs' }" :disabled="convertRunning" @click="convertMode = 'songs'">
              {{ t("cv.modeSongs") }}
            </button>
            <button class="refetch" :class="{ on: convertMode === 'osz' }" :disabled="convertRunning" @click="convertMode = 'osz'">
              {{ t("cv.modeOsz") }}
            </button>
          </div>
          <div class="conv-row">
            <input class="dir" :value="convertDir" readonly :placeholder="t('dl.unset')" :title="convertDir" />
            <button
              class="refetch"
              :disabled="convertRunning"
              @click="
                pickExportFolder().then((d) => {
                  if (d) setConvertDir(d);
                })
              "
            >
              {{ t("dl.browse") }}
            </button>
          </div>
          <p v-if="!convertDir" class="conv-hint">{{ t("dl.needDir") }}</p>
          <div class="conv-row actions">
            <button
              class="refetch primary"
              :disabled="convertRunning || !convertDir || !convertConfirmIds.length"
              @click="startConvertSelected"
            >
              {{ t("cv.start", { 0: convertConfirmIds.length }) }}
            </button>
            <button class="refetch" @click="showConvertConfirm = false">{{ t("action.cancel") }}</button>
          </div>
        </div>

        <!-- 清理工作区（v-show：切走标签不丢选择/结果，保留本次会话） -->
        <ManagePanel v-show="tableMode === 'manage'" :kind="currentMode" :path="currentPath" />

        <!-- 下载工作区（同样常驻，下载中切走不断事件） -->
        <DownloadPanel v-show="tableMode === 'download'" :library-ids="libIds" />

        <!-- 转换工作区（T2：lazer → stable） -->
        <ConvertPanel v-show="tableMode === 'convert'" :library="library" :is-lazer="currentMode === SourceKind.Lazer" />

        <BeatmapTable
          v-if="tableMode === 'set' && pageSets.length"
          :rows="pageSets"
          :sort-key="sortKey"
          :sort-dir="sortDir"
          :selected-key="selectedKey"
          :expanded-keys="expanded"
          :show-checkbox="currentMode === SourceKind.Lazer"
          :selected-ids="songSel"
          @sort="onSort"
          @select="selectSet"
          @toggle-expand="toggleExpand"
          @toggle-select="toggleSongSelect"
          @toggle-page="toggleSongPage"
        />

        <DifficultyTable
          v-else-if="tableMode === 'diff' && pageDiffs.length"
          :rows="pageDiffs"
          :sort-key="dSortKey"
          :sort-dir="dSortDir"
          :selected-key="selectedDiffKey"
          @sort="onDiffSort"
          @select="selectDiff"
        />

        <template v-else-if="tableMode === 'grid' && !loading">
          <div class="g-bar">
            <div class="g-left">
              <button class="btn sm" :disabled="!pageSets.length" @click="selectAllPage()">{{ t("action.selectAllPage") }}</button>
              <button class="btn sm" :disabled="!selKeys.size" @click="selKeys = new Set()">{{ t("action.deselectAll") }}</button>
              <span v-if="selKeys.size" class="g-count">
                {{ t("gallery.selected", { 0: selKeys.size }) }}<template v-if="selOffPage">{{ t("gallery.offPage", { 0: selOffPage }) }}</template>
              </span>
              <span v-if="thumbBusy" class="g-chip" :title="t('title.thumbGenerating')">
                <span class="spin">⟳</span> {{ t("gallery.generating", { 0: thumbDone, 1: thumbTotal }) }}
              </span>
            </div>
            <button class="btn primary sm" :disabled="!selKeys.size || exporting" @click="handleExport()">
              {{ exporting ? t("action.exporting") : t("action.exportSelected") }}
            </button>
          </div>

          <div v-if="exporting && exportProg" class="export-bar">
            <div class="pbar">
              <div class="pfill" :style="{ width: exportPct }"></div>
            </div>
            <span class="exp-cur" :title="exportProg.current">
              {{ exportProg.done }} / {{ exportProg.total }} · {{ exportProg.current }}
            </span>
          </div>

          <div v-if="exportReport" class="export-result">
            <span class="er-sum">
              {{ t("gallery.exportSummary", { 0: exportReport.exported, 1: exportReport.renamed, 2: exportReport.failed }) }}
            </span>
            <button
              v-if="exportReport.errors.length"
              class="link-btn"
              @click="errorsOpen = !errorsOpen"
            >
              {{ errorsOpen ? t("action.hideErrors") : t("action.viewErrors", { 0: exportReport.errors.length }) }}
            </button>
            <button class="link-btn" @click="exportReport = null">{{ t("action.close") }}</button>
            <ul v-if="errorsOpen && exportReport.errors.length" class="err-list">
              <li v-for="(e, i) in exportReport.errors" :key="i">{{ trError(e) }}</li>
            </ul>
          </div>

          <GalleryView
            v-if="pageSets.length"
            :rows="pageSets"
            :thumb-map="thumbMap"
            :selected-keys="selKeys"
            :exported-keys="lastExported"
            :eligible="isEligible"
            @toggle="toggleSelect"
            @open="selectSet"
            @img-error="onImgError"
          />
          <div v-else class="empty">
            <template v-if="!hasScanned">
              <p>{{ t("empty.notScanned") }}</p>
              <p class="sub">{{ t("empty.notScannedSub") }}</p>
            </template>
            <template v-else-if="!library.length">
              <p>{{ t("empty.noSets") }}</p>
              <p class="sub">{{ t("empty.noSetsSub") }}</p>
            </template>
            <template v-else>
              <p>{{ t("empty.noMatch") }}</p>
              <p class="sub">{{ t("empty.noMatchSub") }}</p>
            </template>
          </div>
        </template>

        <div v-else-if="!isWorkspace" class="empty">
          <template v-if="loading">{{ t("empty.scanning") }}</template>
          <template v-else-if="!hasScanned">
            <p>{{ t("empty.notScanned") }}</p>
            <p class="sub">{{ t("empty.notScannedSub") }}</p>
          </template>
          <template v-else-if="!library.length">
            <p>{{ t("empty.noSets") }}</p>
            <p class="sub">{{ t("empty.noSetsSub") }}</p>
          </template>
          <template v-else>
            <p>{{ tableMode === "diff" ? t("empty.noMatchDiff") : t("empty.noMatch") }}</p>
            <p class="sub">{{ t("empty.noMatchSub") }}</p>
          </template>
        </div>
      </main>

      <!-- 详情面板：点击行/卡片打开（固定式，占布局）；拖左缘调宽，双击复位 -->
      <div
        v-if="panelOpen && selectedRow && !isWorkspace"
        class="panel-host"
        :style="{ width: panelW + 'px', flexBasis: panelW + 'px' }"
      >
        <div
          class="resizer"
          :class="{ dragging: resizing }"
          :title="t('title.resizer')"
          @pointerdown="startResize"
          @dblclick="resetPanelW"
        ></div>
        <DetailPanel
          :row="selectedRow"
          :focus-beatmap-id="focusBeatmapId"
          :thumb-url="detailThumbUrl"
          @close="closePanel()"
        />
      </div>

    </div>

    <footer v-show="!isWorkspace" class="pager">
      <label class="size">
        {{ t("pager.prefix") }}
        <select v-model.number="pageSize">
          <option :value="50">50</option>
          <option :value="100">100</option>
          <option :value="200">200</option>
        </select>
        {{ t("pager.suffix") }}
      </label>
      <div class="pager-btns">
        <button class="btn" :disabled="page <= 1" @click="page = 1">{{ t("pager.first") }}</button>
        <button class="btn" :disabled="page <= 1" @click="page--">{{ t("pager.prev") }}</button>
        <span class="page-info">{{ page }} / {{ pageCount }}</span>
        <button class="btn" :disabled="page >= pageCount" @click="page++">{{ t("pager.next") }}</button>
        <button class="btn" :disabled="page >= pageCount" @click="page = pageCount">{{ t("pager.last") }}</button>
      </div>
    </footer>
  </div>
</template>

<style>
:root {
  --bg0: #0f0f14;
  --bg1: #16161e;
  --bg2: #1c1c26;
  --bg3: #242430;
  --line: #2b2b3a;
  --line-soft: #20202b;
  --tx0: #eceaf3;
  --tx1: #a3a1b5;
  /* 辅助文字：对 bg1 ≈ 4.6:1，满足 WCAG AA（4.5:1）；暗色小字场景大量使用 */
  --tx2: #8a889f;
  --accent: #ff5fa2;
  --accent-hi: #ff7cb5;
  --accent-soft: rgba(255, 95, 162, 0.13);
  --c-osu: #ff5fa2;
  --c-taiko: #f27d72;
  --c-catch: #5fc98a;
  --c-mania: #8b95ff;
  --ok: #5fc98a;
  --warn: #e8b45a;
  --err: #f27272;
}

html,
body,
#app {
  height: 100%;
  margin: 0;
  background: var(--bg0);
}

body {
  font-family: "Segoe UI", "Microsoft YaHei UI", "Microsoft YaHei", system-ui, sans-serif;
  color: var(--tx0);
  -webkit-font-smoothing: antialiased;
  overflow: hidden;
}

::-webkit-scrollbar {
  width: 10px;
  height: 10px;
}

::-webkit-scrollbar-thumb {
  background: #34344a;
  border-radius: 5px;
  border: 2px solid transparent;
  background-clip: content-box;
}

::-webkit-scrollbar-thumb:hover {
  background: #43435c;
  background-clip: content-box;
}

::-webkit-scrollbar-track {
  background: transparent;
}

/* 拖拽调宽期间：全局统一光标、禁选文本 */
body.ui-resizing,
body.ui-resizing * {
  cursor: col-resize !important;
  user-select: none !important;
}

select {
  background: var(--bg2);
  color: var(--tx0);
  border: 1px solid var(--line);
  border-radius: 6px;
  padding: 3px 6px;
  font: inherit;
}

/* 模式配色（设计令牌）：表格 / 筛选 / 画廊 / 详情多个组件共用，放全局 */
.mode-tag {
  font-size: 11px;
  line-height: 1;
  padding: 3px 6px;
  border-radius: 4px;
  white-space: nowrap;
}

.m-osu {
  color: var(--c-osu);
  background: rgba(255, 95, 162, 0.12);
}

.m-taiko {
  color: var(--c-taiko);
  background: rgba(242, 125, 114, 0.12);
}

.m-catch {
  color: var(--c-catch);
  background: rgba(95, 201, 138, 0.12);
}

.m-mania {
  color: var(--c-mania);
  background: rgba(139, 149, 255, 0.14);
}
</style>

<style scoped>
.app {
  display: flex;
  flex-direction: column;
  height: 100vh;
}

/* ---------- 顶栏 ---------- */
.topbar {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 16px;
  padding: 10px 18px;
  background: var(--bg1);
  border-bottom: 1px solid var(--line);
  flex: 0 0 auto;
}

.brand {
  display: flex;
  align-items: center;
  gap: 9px;
  font-size: 15px;
  font-weight: 700;
  letter-spacing: 0.2px;
}

.brand-dot {
  width: 12px;
  height: 12px;
  border-radius: 50%;
  background: var(--accent);
  box-shadow: 0 0 10px rgba(255, 95, 162, 0.6);
}

.mode-tabs {
  display: flex;
  gap: 6px;
  background: var(--bg2);
  padding: 3px;
  border-radius: 9px;
  border: 1px solid var(--line);
}

.mode-tabs button {
  border: none;
  background: transparent;
  color: var(--tx1);
  font: inherit;
  font-size: 12.5px;
  padding: 5px 13px;
  border-radius: 6px;
  cursor: pointer;
  transition: color 0.15s, background 0.15s;
}

.mode-tabs button:hover {
  color: var(--tx0);
}

.mode-tabs button.active {
  background: var(--accent-soft);
  color: var(--accent-hi);
  font-weight: 600;
}

/* ---------- 路径栏 ---------- */
.pathbar {
  display: flex;
  align-items: center;
  gap: 10px;
  padding: 10px 18px;
  border-bottom: 1px solid var(--line-soft);
  flex: 0 0 auto;
}

.btn {
  border: 1px solid var(--line);
  background: var(--bg2);
  color: var(--tx0);
  font: inherit;
  font-size: 12.5px;
  padding: 6px 13px;
  border-radius: 7px;
  cursor: pointer;
  white-space: nowrap;
  transition: border-color 0.15s, background 0.15s, color 0.15s;
}

.btn:hover:not(:disabled) {
  border-color: var(--tx2);
  background: var(--bg3);
}

.btn:disabled {
  opacity: 0.5;
  cursor: default;
}

.btn.primary {
  background: var(--accent);
  border-color: var(--accent);
  color: #1a0d14;
  font-weight: 700;
}

.btn.primary:hover:not(:disabled) {
  background: var(--accent-hi);
  border-color: var(--accent-hi);
}

.btn.active {
  border-color: var(--accent);
  color: var(--accent-hi);
}

.path-input {
  flex: 1;
  min-width: 240px;
  background: var(--bg2);
  border: 1px solid var(--line);
  border-radius: 7px;
  color: var(--tx0);
  font: inherit;
  font-size: 12.5px;
  padding: 7px 11px;
}

.path-input:focus,
.search:focus {
  outline: none;
  border-color: var(--accent);
}

.path-input::placeholder,
.search::placeholder {
  color: var(--tx2);
}

.scan-status {
  display: flex;
  align-items: center;
  gap: 8px;
  min-width: 200px;
  max-width: 340px;
}

.pbar {
  flex: 1;
  height: 5px;
  border-radius: 3px;
  background: var(--bg3);
  overflow: hidden;
}

.pfill {
  height: 100%;
  background: linear-gradient(90deg, var(--accent), var(--accent-hi));
  border-radius: 3px;
  transition: width 0.25s ease;
}

.msg {
  font-size: 12px;
  color: var(--tx1);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.msg.info {
  color: var(--ok);
}

.msg.warn {
  color: var(--warn);
}

.msg.error {
  color: var(--err);
}

/* ---------- 工具栏 ---------- */
.toolbar {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
  padding: 9px 18px;
  flex: 0 0 auto;
}

.search {
  width: 340px;
  background: var(--bg2);
  border: 1px solid var(--line);
  border-radius: 7px;
  color: var(--tx0);
  font: inherit;
  font-size: 13px;
  padding: 7px 12px;
}

.tb-right {
  display: flex;
  align-items: center;
  gap: 10px;
}

/* 左侧组：搜索 + 字号控件 */
.tb-left {
  display: flex;
  align-items: center;
  gap: 10px;
  min-width: 0;
}

.zoom-ctl {
  display: inline-flex;
  align-items: center;
  gap: 2px;
  flex: 0 0 auto;
  background: var(--bg2);
  border: 1px solid var(--line);
  border-radius: 8px;
  padding: 2px 4px;
}

.zc-label {
  font-size: 11.5px;
  color: var(--tx2);
  padding: 0 4px 0 6px;
  white-space: nowrap;
}

.zc-btn {
  border: none;
  background: transparent;
  color: var(--tx1);
  font: inherit;
  font-size: 14px;
  line-height: 1;
  width: 22px;
  height: 22px;
  border-radius: 5px;
  cursor: pointer;
  transition: color 0.15s, background 0.15s;
}

.zc-btn:hover:not(:disabled) {
  color: var(--accent-hi);
  background: var(--bg3);
}

.zc-btn:disabled {
  opacity: 0.35;
  cursor: default;
}

.zc-step {
  font-size: 11.5px;
  color: var(--tx1);
  min-width: 32px;
  text-align: center;
  white-space: nowrap;
  font-variant-numeric: tabular-nums;
}

.badge {
  display: inline-block;
  min-width: 16px;
  text-align: center;
  background: var(--accent);
  color: #1a0d14;
  font-size: 10.5px;
  font-weight: 700;
  line-height: 1;
  padding: 3px 4px;
  border-radius: 8px;
  margin-left: 5px;
}

.caret {
  font-size: 9px;
  color: var(--tx2);
  margin-left: 5px;
}

.seg {
  display: flex;
  background: var(--bg2);
  border: 1px solid var(--line);
  border-radius: 8px;
  padding: 2px;
}

.seg button {
  border: none;
  background: transparent;
  color: var(--tx1);
  font: inherit;
  font-size: 12.5px;
  padding: 4px 14px;
  border-radius: 6px;
  cursor: pointer;
}

.seg button.active {
  background: var(--bg3);
  color: var(--accent-hi);
  font-weight: 600;
}

/* ---------- 在线同步状态 ---------- */
.sync-chip {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  font-size: 12px;
  color: var(--tx1);
  background: var(--bg2);
  border: 1px solid var(--line);
  border-radius: 20px;
  padding: 5px 11px;
  cursor: default;
  font-variant-numeric: tabular-nums;
  white-space: nowrap;
}

.dot {
  width: 7px;
  height: 7px;
  border-radius: 50%;
  flex: 0 0 auto;
}

.dot-mirror {
  background: var(--ok);
  box-shadow: 0 0 6px rgba(95, 201, 138, 0.7);
}

.dot-official {
  background: var(--c-mania);
  box-shadow: 0 0 6px rgba(139, 149, 255, 0.7);
}

.dot-none {
  background: var(--tx2);
}

.spin {
  display: inline-block;
  color: var(--accent-hi);
  animation: spin 1.1s linear infinite;
}

@keyframes spin {
  to {
    transform: rotate(360deg);
  }
}

.settings-wrap {
  position: relative;
}

.pop-backdrop {
  position: fixed;
  inset: 0;
  z-index: 25;
}

.settings-pop {
  position: absolute;
  right: 0;
  top: calc(100% + 6px);
  z-index: 30;
  width: 430px;
  background: var(--bg1);
  border: 1px solid var(--line);
  border-radius: 10px;
  box-shadow: 0 12px 32px rgba(0, 0, 0, 0.5);
  padding: 12px 14px 14px;
}

/* ---------- 语言切换（T1） ---------- */
.online-row {
  margin-top: 8px;
}

.online-hint {
  margin: 4px 0 0;
  font-size: 11px;
  line-height: 1.45;
  color: var(--tx2);
}

.lang-row {
  display: flex;
  align-items: center;
  gap: 8px;
  padding-bottom: 10px;
  margin-bottom: 10px;
  border-bottom: 1px solid var(--line-soft);
}

.lang-label {
  flex: 0 0 76px;
  color: var(--tx2);
  font-size: 11.5px;
}

.lang-btn {
  font-size: 12px;
  padding: 4px 12px;
  border-radius: 6px;
}

.lang-btn.active {
  border-color: var(--accent);
  color: var(--accent-hi);
  font-weight: 600;
}

/* ---------- 本页强制刷新 ---------- */
.count-line {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
}

/* ── 全局转换状态条 ── */
.conv-bar {
  display: flex;
  align-items: center;
  gap: 10px;
  padding: 6px 14px;
  font-size: 12px;
  color: var(--tx1);
  background: var(--bg1);
  border-bottom: 1px solid var(--line);
}

.conv-bar-text {
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.conv-bar .dim,
.dim {
  color: var(--tx2);
}

.conv-err {
  color: var(--err);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.conv-detail {
  max-height: 220px;
  overflow: auto;
  padding: 6px 14px;
  border-bottom: 1px solid var(--line);
  background: var(--bg1);
}

.conv-task {
  display: flex;
  align-items: center;
  gap: 10px;
  font-size: 11.5px;
  padding: 3px 0;
  color: var(--tx1);
}

.conv-task .ct-id {
  font-variant-numeric: tabular-nums;
  color: var(--tx2);
}

.conv-task .ct-state {
  flex: 0 0 auto;
}

.conv-task .ct-name {
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.conv-task .ct-err {
  color: var(--err);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.conv-task .s-done {
  color: var(--ok);
}

.conv-task .s-failed {
  color: var(--err);
}

/* ── 转换所选确认弹层 ── */
.conv-confirm {
  position: fixed;
  inset: 0;
  margin: auto;
  width: min(460px, calc(100vw - 40px));
  height: fit-content;
  z-index: 60;
  background: var(--bg1);
  border: 1px solid var(--line);
  border-radius: 10px;
  padding: 14px 16px;
  box-shadow: 0 10px 32px rgba(0, 0, 0, 0.45);
}

.conv-title {
  margin: 0 0 6px;
  font-size: 14px;
  font-weight: 600;
}

.conv-body {
  margin: 0 0 10px;
  font-size: 12px;
  color: var(--tx1);
}

.conv-row {
  display: flex;
  align-items: center;
  gap: 8px;
  margin-bottom: 10px;
}

.conv-row .dir {
  flex: 1 1 auto;
  min-width: 0;
}

.conv-hint {
  margin: 0 0 10px;
  font-size: 11.5px;
  color: var(--err);
}

.conv-row.actions {
  margin-bottom: 0;
}

.refetch.on {
  border-color: var(--accent);
  color: var(--accent-hi);
}

.refetch.conv-pick {
  border-color: var(--accent);
  color: var(--accent-hi);
}

.refetch {
  border: none;
  background: transparent;
  color: var(--tx2);
  font: inherit;
  font-size: 12px;
  cursor: pointer;
  padding: 2px 4px;
  white-space: nowrap;
}

.refetch:hover:not(:disabled) {
  color: var(--accent-hi);
}

.refetch:disabled {
  opacity: 0.5;
  cursor: default;
}

/* ---------- 生效筛选 chip ---------- */
.chips-bar {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  gap: 8px;
  padding: 8px 18px;
  border-bottom: 1px solid var(--line-soft);
  flex: 0 0 auto;
}

.chip {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  background: var(--accent-soft);
  color: var(--accent-hi);
  border: 1px solid rgba(255, 95, 162, 0.35);
  font: inherit;
  font-size: 12px;
  line-height: 1;
  padding: 5px 9px;
  border-radius: 20px;
  cursor: pointer;
}

.chip:hover .chip-x {
  opacity: 1;
}

.chip-x {
  font-size: 10px;
  opacity: 0.55;
}

.chip-clear {
  border: none;
  background: transparent;
  color: var(--tx1);
  font: inherit;
  font-size: 12px;
  cursor: pointer;
  text-decoration: underline;
  text-underline-offset: 3px;
}

.chip-clear:hover {
  color: var(--tx0);
}

/* ---------- 主体 ---------- */
.body {
  flex: 1;
  display: flex;
  min-height: 0;
  position: relative;
}

.content {
  flex: 1;
  min-width: 0;
  display: flex;
  flex-direction: column;
  padding: 10px 18px 12px;
}

.count-line {
  font-size: 12px;
  color: var(--tx2);
  margin-bottom: 8px;
  flex: 0 0 auto;
  font-variant-numeric: tabular-nums;
}

.content :deep(.table-wrap) {
  flex: 1;
  min-height: 0;
}

/* ---------- 画廊（卡片样式在 GalleryView 内部） ---------- */
.g-bar {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
  flex-wrap: wrap;
  margin-bottom: 10px;
  flex: 0 0 auto;
}

.g-left {
  display: flex;
  align-items: center;
  gap: 8px;
  flex-wrap: wrap;
}

.btn.sm {
  padding: 5px 11px;
  font-size: 12px;
}

.g-count {
  font-size: 12px;
  color: var(--tx1);
  font-variant-numeric: tabular-nums;
}

.g-chip {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  font-size: 12px;
  color: var(--accent-hi);
  background: var(--accent-soft);
  border: 1px solid rgba(255, 95, 162, 0.35);
  padding: 4px 10px;
  border-radius: 20px;
  font-variant-numeric: tabular-nums;
}

.content :deep(.grid) {
  flex: 1;
  min-height: 0;
}

.export-bar {
  display: flex;
  align-items: center;
  gap: 10px;
  margin-bottom: 8px;
  flex: 0 0 auto;
}

.export-bar .pbar {
  max-width: 420px;
}

.exp-cur {
  font-size: 11.5px;
  color: var(--tx1);
  max-width: 340px;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  font-variant-numeric: tabular-nums;
}

.export-result {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  gap: 10px;
  margin-bottom: 8px;
  padding: 7px 10px;
  background: var(--bg1);
  border: 1px solid var(--line);
  border-radius: 8px;
  flex: 0 0 auto;
}

.er-sum {
  font-size: 12.5px;
  color: var(--ok);
  font-variant-numeric: tabular-nums;
}

.link-btn {
  border: none;
  background: transparent;
  color: var(--tx1);
  font: inherit;
  font-size: 12px;
  cursor: pointer;
  text-decoration: underline;
  text-underline-offset: 3px;
}

.link-btn:hover {
  color: var(--tx0);
}

.err-list {
  flex: 1 1 100%;
  margin: 2px 0 0;
  padding-left: 18px;
  max-height: 140px;
  overflow: auto;
  font-size: 11.5px;
  color: var(--err);
}

/* ---------- 空状态 ---------- */
.empty {
  flex: 1;
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: 4px;
  border: 1px dashed var(--line);
  border-radius: 10px;
  color: var(--tx1);
  font-size: 14px;
}

.empty p {
  margin: 0;
}

.empty .sub {
  font-size: 12.5px;
  color: var(--tx2);
}

/* ---------- 详情面板（host 控制宽度；左缘拖拽调宽，表格区 flex 自适应） ---------- */
.panel-host {
  flex: 0 0 420px; /* 实际值由内联 width / flexBasis（panelW）覆盖 */
  height: 100%;
  min-height: 0;
  position: relative;
}

/* 拖拽条：宽 9px 的命中区（含向表格侧外扩 8px，易抓不挡路），
   面板边框处常态为 1px 浅线（继承 .panel 的 border-left），悬停/拖拽变粉色高亮 */
.resizer {
  position: absolute;
  left: 0;
  top: 0;
  bottom: 0;
  width: 9px;
  margin-left: -8px;
  cursor: col-resize;
  z-index: 6;
  touch-action: none;
}

.resizer::after {
  content: "";
  position: absolute;
  left: 8px;
  top: 0;
  bottom: 0;
  width: 1px;
  background: transparent;
  transition: background 0.15s, box-shadow 0.15s;
}

.resizer:hover::after,
.resizer.dragging::after {
  background: var(--accent);
  box-shadow: 0 0 8px rgba(255, 95, 162, 0.55);
}

/* ---------- 分页 ---------- */
.pager {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 9px 18px;
  background: var(--bg1);
  border-top: 1px solid var(--line);
  flex: 0 0 auto;
  font-size: 12.5px;
  color: var(--tx1);
}

.size {
  display: inline-flex;
  align-items: center;
  gap: 6px;
}

.pager-btns {
  display: flex;
  align-items: center;
  gap: 6px;
}

.pager-btns .btn {
  padding: 5px 11px;
}

.page-info {
  min-width: 68px;
  text-align: center;
  font-variant-numeric: tabular-nums;
  color: var(--tx0);
}
</style>
