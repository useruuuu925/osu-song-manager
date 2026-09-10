// convertTasks.ts — 转换任务共享 store（模块级单例）。
//
// 背景：转换入口从单一 ConvertPanel 扩展为「详情面板单集卡 / 曲目多选确认弹层 /
// 转换 tab（转换全部）/ 全局状态条」四处，任务状态必须唯一且跨 UI 同步——
// 状态放模块级 ref，所有消费方引用同一份；convert-progress 监听 + 合批器只注册一次。
// 切页/关面板不中断转换（状态在模块级，UI 卸载无影响）。

import { computed, ref } from "vue";
import { listen } from "@tauri-apps/api/event";
import { cancelConvert, convertLazerToStable } from "./api";
import { createProgressBatcher } from "./lib";
import type { ConvertProgressInfo, ConvertReport, ConvertSetResult, ConvertState } from "./types";
import type { Slot } from "./i18n";

export interface ConvertTask {
  setId: number;
  state: ConvertState;
  current: string;
  error: string | null;
  warnings: string[];
  result: ConvertSetResult | null;
}

const DIR_KEY = "osu-mgr.convert-dir";

function readStoredDir(): string {
  try {
    return localStorage.getItem(DIR_KEY) ?? "";
  } catch {
    return "";
  }
}

// ── 共享状态 ──────────────────────────────────────────────────────────────────
export const convertMode = ref<"songs" | "osz">("songs");
export const convertDir = ref<string>(readStoredDir());
export const convertTasks = ref<ConvertTask[]>([]);
export const convertRunning = ref(false);
export const convertFinished = ref(false);
export const convertCancelRequested = ref(false);
export const convertReport = ref<ConvertReport | null>(null);
export const convertBatchError = ref<Slot | null>(null);

export const convertDoneCount = computed(() => convertTasks.value.filter((x) => x.state === "done").length);
export const convertFailedCount = computed(() => convertTasks.value.filter((x) => x.state === "failed").length);
export const convertTotalCount = computed(() => convertTasks.value.length);
export const convertCurrent = computed(() => convertTasks.value.find((x) => x.state === "converting")?.current ?? "");

/** setId → Task 索引（O(1) 查找；非响应式，任务对象本身是响应式的） */
let taskIndex = new Map<number, ConvertTask>();
/** 代次守卫：理论上 startConvert 无并发入口，防御性保留 */
let gen = 0;

export function setConvertDir(dir: string) {
  convertDir.value = dir;
  try {
    localStorage.setItem(DIR_KEY, dir);
  } catch {
    // 存储失败不影响本次会话
  }
}

/** 完成态 dismiss（全局状态条的 ✕） */
export function dismissConvertFinished() {
  convertFinished.value = false;
  convertReport.value = null;
}

// 进度事件合批：后端批启动会瞬时广播全部 queued 事件（≤2000 条），
// 每条独立 IPC 回调各触发一次渲染会卡死 UI；合批为每 100ms 一次批量落账。
const progressBatcher = createProgressBatcher<number, ConvertProgressInfo>((items) => {
  let changed = false;
  for (const p of items) {
    const task = taskIndex.get(p.setId);
    if (!task) continue;
    // no-op 守卫：值未变不触发响应式写入
    if (task.state !== p.state || task.current !== p.current || task.error !== p.error) {
      task.state = p.state;
      task.current = p.current;
      task.error = p.error;
      changed = true;
    }
  }
  if (changed) convertTasks.value = [...convertTasks.value];
});

let listenerReady = false;

/** 消费方 setup 时调用一次；实际只注册一个全局监听 */
export async function ensureConvertListener() {
  if (listenerReady) return;
  listenerReady = true;
  try {
    await listen<ConvertProgressInfo>("convert-progress", (ev) => {
      progressBatcher.push(ev.payload.setId, ev.payload);
    });
  } catch {
    // 非 Tauri 环境（纯浏览器调试）下忽略进度事件
  }
}

/** 发起转换；false = 被运行守卫/空列表/未选目录拒绝 */
export async function startConvert(ids: number[]): Promise<boolean> {
  if (convertRunning.value || !ids.length || !convertDir.value) return false;
  await ensureConvertListener();
  const myGen = ++gen;
  convertBatchError.value = null;
  convertReport.value = null;
  convertFinished.value = false;
  convertCancelRequested.value = false;
  convertTasks.value = ids.map((id) => ({
    setId: id,
    state: "queued" as ConvertState,
    current: String(id),
    error: null,
    warnings: [] as string[],
    result: null,
  }));
  taskIndex = new Map(convertTasks.value.map((task) => [task.setId, task]));
  convertRunning.value = true;
  try {
    const rep = await convertLazerToStable(ids, convertMode.value, convertDir.value);
    if (myGen !== gen) return true;
    convertReport.value = rep;
    const byId = new Map(rep.results.map((r) => [r.setId, r]));
    for (const task of convertTasks.value) {
      const r = byId.get(task.setId);
      if (!r) continue;
      task.result = r;
      task.warnings = r.warnings ?? [];
      // 事件丢失兜底：Promise 结果为准
      if (task.state !== "done" && task.state !== "cancelled") {
        task.state = r.ok ? "done" : "failed";
        task.error = r.error;
      }
    }
    convertTasks.value = [...convertTasks.value];
  } catch (e) {
    // 整批失败（如超限/目标目录拒绝）：不留一列假"排队中"
    convertBatchError.value = { err: e };
    convertTasks.value = [];
    taskIndex = new Map();
  } finally {
    progressBatcher.reset();
    if (myGen === gen) {
      convertRunning.value = false;
      convertFinished.value = true;
    }
  }
  return true;
}

export async function cancelConvertTask() {
  if (!convertRunning.value || convertCancelRequested.value) return;
  convertCancelRequested.value = true;
  try {
    await cancelConvert();
  } catch (e) {
    convertBatchError.value = { err: e };
    convertCancelRequested.value = false;
  }
}
