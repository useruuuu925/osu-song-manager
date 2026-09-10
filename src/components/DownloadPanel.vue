<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref } from "vue";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { cancelDownloads, downloadOsz, pickExportFolder } from "../api";
import { createProgressBatcher, formatBytes, parseIdList } from "../lib";
import { mk, sm, t, trError, type Slot } from "../i18n";
import type { DownloadProgressInfo, DownloadResult, DownloadState } from "../types";

const props = defineProps<{
  /** 当前曲库（已扫描）的 beatmapsetId，用于「已在库」标记；未扫描时为空集合 */
  libraryIds: Set<number>;
}>();

// ---------- 输入解析 ----------
const MAX_IDS = 200;

const inputText = ref("");

const parsedIds = computed(() => parseIdList(inputText.value));
const willRun = computed(() => parsedIds.value.slice(0, MAX_IDS));
const truncated = computed(() => parsedIds.value.length > MAX_IDS);
const previewChips = computed(() => willRun.value.slice(0, 20));
const inLibraryCount = computed(() => {
  if (!props.libraryIds.size) return -1; // 未扫描：跳过该功能
  return willRun.value.filter((id) => props.libraryIds.has(id)).length;
});

// ---------- 目标与选项 ----------
const targetDir = ref("");
const noVideo = ref(true);
const dirHint = ref<Slot | null>(null);

async function browse() {
  try {
    const dir = await pickExportFolder();
    if (dir) {
      targetDir.value = dir;
      dirHint.value = null;
    }
    // 取消：保持原值
  } catch (e) {
    dirHint.value = { err: e };
  }
}

// ---------- 任务队列 ----------
interface Task {
  setId: number;
  state: DownloadState;
  mirror: string | null;
  received: number;
  total: number;
  error: string | null;
  result: DownloadResult | null;
}

const tasks = ref<Task[]>([]);
const running = ref(false);
const finished = ref(false);
const cancelRequested = ref(false);
const batchError = ref<Slot | null>(null);

/** setId → Task 索引（O(1) 查找，submit() 时重建；任务对象本身是响应式的） */
let taskIndex = new Map<number, Task>();

function stateText(s: DownloadState): string {
  switch (s) {
    case "queued":
      return t("dl.state.queued");
    case "downloading":
      return t("dl.state.downloading");
    case "validating":
      return t("dl.state.validating");
    case "done":
      return t("dl.state.done");
    case "failed":
      return t("dl.state.failed");
    case "cancelled":
      return t("dl.state.cancelled");
  }
}

function pct(task: Task): number {
  return task.total > 0 ? Math.min(100, Math.round((task.received / task.total) * 100)) : 0;
}

function baseName(path: string): string {
  const parts = path.split(/[\\/]/);
  return parts[parts.length - 1] || path;
}

const doneCount = computed(() => tasks.value.filter((t) => t.state === "done").length);
const failedCount = computed(() => tasks.value.filter((t) => t.state === "failed").length);
const cancelledCount = computed(() => tasks.value.filter((t) => t.state === "cancelled").length);

// 速度：滑动窗口（累计 received 差值 / 时间）
const speedText = ref("");
let samples: { at: number; bytes: number }[] = [];
let ticker: ReturnType<typeof setInterval> | null = null;

function stopTicker() {
  if (ticker != null) {
    clearInterval(ticker);
    ticker = null;
  }
}

function startTicker() {
  stopTicker();
  samples = [];
  speedText.value = "";
  ticker = setInterval(() => {
    const now = performance.now();
    const bytes = tasks.value.reduce((s, t) => s + t.received, 0);
    samples.push({ at: now, bytes });
    while (samples.length > 14) samples.shift();
    const first = samples[0];
    const dt = (now - first.at) / 1000;
    if (dt >= 0.5 && bytes >= first.bytes) {
      speedText.value = `${formatBytes((bytes - first.bytes) / dt)}/s`;
    }
  }, 500);
}

/** 只更新本批次清单里的 id，忽略无关事件；事件经合批后批量落账（O(1) 索引 + no-op 守卫） */
const progressBatcher = createProgressBatcher<number, DownloadProgressInfo>((items) => {
  let changed = false;
  for (const p of items) {
    const task = taskIndex.get(p.setId);
    if (!task) continue;
    if (
      task.state !== p.state ||
      task.received !== p.received ||
      task.total !== p.total ||
      task.error !== p.error
    ) {
      task.state = p.state;
      if (p.mirror) task.mirror = p.mirror; // 状态切换时 mirror 可能为 null，保留最近值
      task.received = p.received;
      task.total = p.total;
      task.error = p.error;
      changed = true;
    }
  }
  if (changed) tasks.value = [...tasks.value];
});

function onProgress(p: DownloadProgressInfo) {
  progressBatcher.push(p.setId, p);
}

// 组件常驻（v-show），监听生命周期与 App 一致
let unlistenDl: UnlistenFn | null = null;

onMounted(async () => {
  try {
    unlistenDl = await listen<DownloadProgressInfo>("download-progress", (ev) => onProgress(ev.payload));
  } catch {
    // 非 Tauri 环境（纯浏览器调试）下忽略进度事件
  }
});

onBeforeUnmount(() => {
  unlistenDl?.();
  stopTicker();
  progressBatcher.reset();
});

async function submit(ids: number[]) {
  if (running.value || !ids.length) return;
  if (!targetDir.value) {
    dirHint.value = mk("dl.needDir");
    return;
  }
  dirHint.value = null;
  batchError.value = null;
  finished.value = false;
  cancelRequested.value = false;
  tasks.value = ids.map((id) => ({
    setId: id,
    state: "queued" as DownloadState,
    mirror: null,
    received: 0,
    total: 0,
    error: null,
    result: null,
  }));
  taskIndex = new Map(tasks.value.map((task) => [task.setId, task]));
  running.value = true;
  startTicker();
  try {
    const results = await downloadOsz(ids, targetDir.value, noVideo.value);
    const byId = new Map(results.map((r) => [r.setId, r]));
    for (const task of tasks.value) {
      const r = byId.get(task.setId);
      if (!r) continue;
      task.result = r;
      // 事件丢失兜底：Promise 结果为准
      if (task.state !== "done" && task.state !== "cancelled") {
        task.state = r.ok ? "done" : "failed";
        if (!r.ok && r.error) task.error = r.error;
      }
    }
    tasks.value = [...tasks.value];
  } catch (e) {
    batchError.value = { err: e };
  } finally {
    progressBatcher.reset();
    stopTicker();
    speedText.value = "";
    running.value = false;
    finished.value = true;
  }
}

function startDownload() {
  void submit(willRun.value);
}

function retryFailed() {
  const ids = tasks.value.filter((t) => t.state === "failed").map((t) => t.setId);
  void submit(ids);
}

async function cancel() {
  if (!running.value || cancelRequested.value) return;
  cancelRequested.value = true;
  try {
    await cancelDownloads();
  } catch (e) {
    batchError.value = { err: e };
    cancelRequested.value = false;
  }
}

onBeforeUnmount(stopTicker);
</script>

<template>
  <div class="dl">
    <!-- 输入区 -->
    <div class="ws-card">
      <div class="row">
        <span class="sec-title">{{ t("dl.setIdTitle") }}</span>
        <span class="hint">{{ t("dl.inputHint") }}</span>
      </div>
      <textarea
        v-model="inputText"
        class="ta"
        rows="5"
        spellcheck="false"
        placeholder="1234567&#10;https://osu.ppy.sh/beatmapsets/720244#taiko/1529490&#10;184039,Artist,Title"
      ></textarea>
      <div class="row preview">
        <span class="parse-n">{{ t("dl.parsed", { 0: parsedIds.length }) }}</span>
        <span v-for="id in previewChips" :key="id" class="id-chip" :class="{ inlib: libraryIds.has(id) }" :title="libraryIds.has(id) ? t('dl.inLib') : String(id)">
          {{ id }}
        </span>
        <span v-if="willRun.length > 20" class="hint">{{ t("dl.more", { 0: willRun.length }) }}</span>
      </div>
      <p v-if="truncated" class="warn-line">{{ t("dl.truncated", { 0: MAX_IDS, 1: MAX_IDS }) }}</p>
      <p v-if="inLibraryCount > 0" class="hint">{{ t("dl.someInLib", { 0: inLibraryCount }) }}</p>
    </div>

    <!-- 目标与选项 -->
    <div class="ws-card">
      <div class="row">
        <span class="sec-title">{{ t("dl.saveTo") }}</span>
        <input class="dir" :value="targetDir" readonly :placeholder="t('dl.unset')" :title="targetDir" />
        <button class="btn sm" :disabled="running" @click="browse()">{{ t("dl.browse") }}</button>
      </div>
      <div class="row">
        <label class="opt">
          <input v-model="noVideo" type="checkbox" :disabled="running" />
          {{ t("dl.noVideo") }}
        </label>
        <span class="hint">{{ t("dl.noVideoHint") }}</span>
      </div>
      <p v-if="dirHint" class="err-line">{{ sm(dirHint) }}</p>
      <div class="row">
        <button class="btn primary sm" :disabled="running || !willRun.length" @click="startDownload()">
          {{ running ? t("dl.btnDownloading") : t("dl.start", { 0: willRun.length }) }}
        </button>
        <button v-if="running" class="btn sm danger" :disabled="cancelRequested" @click="cancel()">
          {{ cancelRequested ? t("dl.cancelling") : t("action.cancel") }}
        </button>
      </div>
    </div>

    <!-- 聚合 + 队列 -->
    <div v-if="tasks.length" class="ws-card">
      <div class="row agg">
        <span class="parse-n">{{ t("dl.progress", { 0: doneCount, 1: tasks.length }) }}</span>
        <span v-if="running && speedText" class="speed">{{ speedText }}</span>
        <span v-if="failedCount" class="err-line">{{ t("dl.failedN", { 0: failedCount }) }}</span>
        <span v-if="cancelledCount" class="hint">{{ t("dl.cancelledN", { 0: cancelledCount }) }}</span>
        <button v-if="finished && failedCount" class="btn xs retry" @click="retryFailed()">{{ t("dl.retry") }}</button>
      </div>
      <p v-if="batchError" class="err-line">{{ sm(batchError) }}</p>
      <div class="queue">
        <div v-for="task in tasks" :key="task.setId" class="q-row">
          <span class="q-id">{{ task.setId }}</span>
          <span class="q-state" :class="`s-${task.state}`">{{ stateText(task.state) }}</span>
          <span class="q-bar">
            <span
              class="q-fill"
              :class="{ indet: task.state === 'downloading' && task.total === 0 }"
              :style="task.total > 0 ? { width: pct(task) + '%' } : undefined"
            ></span>
          </span>
          <span class="q-bytes">{{ task.total > 0 ? `${formatBytes(task.received)} / ${formatBytes(task.total)}` : formatBytes(task.received) }}</span>
          <span v-if="task.mirror" class="q-mirror" :title="t('dl.mirrorTitle')">{{ task.mirror }}</span>
          <span v-if="task.state === 'done' && (task.result?.path || task.result)" class="q-file" :title="task.result?.path ?? ''">
            {{ task.result ? `${baseName(task.result.path ?? "")} · ${formatBytes(task.result.bytes)}` : t("dl.saved") }}
          </span>
          <span v-else-if="task.error" class="q-err" :title="task.error">{{ trError(task.error) }}</span>
        </div>
      </div>
      <p v-if="finished" class="final-note">
        {{ t("dl.final", { 0: doneCount, 1: failedCount, 2: cancelledCount }) }}
      </p>
    </div>
  </div>
</template>

<style scoped>
.dl {
  position: relative;
  flex: 1;
  min-height: 0;
  overflow: auto;
  display: flex;
  flex-direction: column;
  gap: 10px;
  padding-bottom: 12px;
}

.ws-card {
  background: var(--bg1);
  border: 1px solid var(--line);
  border-radius: 10px;
  padding: 10px 12px;
  flex: 0 0 auto;
  display: flex;
  flex-direction: column;
  gap: 8px;
}

.row {
  display: flex;
  align-items: center;
  gap: 10px;
  flex-wrap: wrap;
}

.sec-title {
  font-size: 13px;
  font-weight: 600;
  white-space: nowrap;
}

.hint {
  font-size: 11.5px;
  color: var(--tx2);
}

.warn-line {
  margin: 0;
  font-size: 12px;
  color: var(--warn);
}

.err-line {
  margin: 0;
  font-size: 12px;
  color: var(--err);
  word-break: break-all;
}

.parse-n {
  font-size: 12.5px;
  color: var(--tx1);
  font-variant-numeric: tabular-nums;
  white-space: nowrap;
}

.ta {
  width: 100%;
  box-sizing: border-box;
  resize: vertical;
  background: var(--bg2);
  border: 1px solid var(--line);
  border-radius: 7px;
  color: var(--tx0);
  font: 12.5px/1.6 Consolas, "Cascadia Mono", Menlo, monospace;
  padding: 8px 10px;
}

.ta:focus {
  outline: none;
  border-color: var(--accent);
}

.preview {
  gap: 6px;
}

.id-chip {
  font-size: 11px;
  line-height: 1;
  padding: 4px 8px;
  border-radius: 20px;
  background: rgba(139, 149, 255, 0.14);
  color: var(--c-mania);
  font-variant-numeric: tabular-nums;
}

.id-chip.inlib {
  background: var(--bg3);
  color: var(--tx2);
  text-decoration: line-through;
}

.dir {
  flex: 1;
  min-width: 220px;
  background: var(--bg2);
  border: 1px solid var(--line);
  border-radius: 7px;
  color: var(--tx0);
  font: inherit;
  font-size: 12px;
  padding: 6px 10px;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.opt {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  font-size: 12.5px;
  cursor: pointer;
  white-space: nowrap;
}

.opt input {
  accent-color: var(--accent);
  margin: 0;
}

.btn {
  border: 1px solid var(--line);
  background: var(--bg2);
  color: var(--tx0);
  font: inherit;
  padding: 6px 13px;
  border-radius: 7px;
  cursor: pointer;
  white-space: nowrap;
  transition: border-color 0.15s, background 0.15s;
}

.btn.sm {
  font-size: 12.5px;
}

.btn.xs {
  font-size: 12px;
  padding: 4px 10px;
  border-radius: 6px;
}

.btn:hover:not(:disabled) {
  border-color: var(--tx2);
  background: var(--bg3);
}

.btn:disabled {
  opacity: 0.45;
  cursor: default;
}

.btn.primary {
  background: var(--accent);
  border-color: var(--accent);
  color: #1a0d14;
  font-weight: 700;
}

.btn.danger {
  color: var(--err);
  border-color: rgba(242, 114, 114, 0.45);
}

.btn.retry {
  margin-left: auto;
  color: var(--accent-hi);
  border-color: rgba(255, 95, 162, 0.45);
}

/* ---------- 队列 ---------- */
.agg {
  gap: 12px;
  padding-bottom: 8px;
  border-bottom: 1px solid var(--line-soft);
}

.speed {
  font-size: 12px;
  color: var(--accent-hi);
  font-variant-numeric: tabular-nums;
}

.queue {
  display: flex;
  flex-direction: column;
  max-height: 420px;
  overflow: auto;
}

.q-row {
  display: flex;
  align-items: center;
  gap: 10px;
  padding: 6px 2px;
  border-bottom: 1px solid var(--line-soft);
  font-size: 12.5px;
  min-width: 0;
}

.q-row:last-child {
  border-bottom: none;
}

.q-id {
  flex: 0 0 72px;
  font-variant-numeric: tabular-nums;
  color: var(--tx0);
}

.q-state {
  flex: 0 0 58px;
  font-size: 11px;
  line-height: 1;
  padding: 4px 7px;
  border-radius: 20px;
  text-align: center;
  white-space: nowrap;
}

.s-queued {
  color: var(--tx2);
  background: var(--bg3);
}

.s-downloading {
  color: var(--c-mania);
  background: rgba(139, 149, 255, 0.14);
}

.s-validating {
  color: var(--warn);
  background: rgba(232, 180, 90, 0.12);
}

.s-done {
  color: var(--ok);
  background: rgba(95, 201, 138, 0.12);
}

.s-failed {
  color: var(--err);
  background: rgba(242, 114, 114, 0.12);
}

.s-cancelled {
  color: var(--tx2);
  background: rgba(110, 108, 131, 0.12);
}

.q-bar {
  flex: 1 1 120px;
  min-width: 90px;
  height: 5px;
  border-radius: 3px;
  background: var(--bg3);
  overflow: hidden;
  position: relative;
}

.q-fill {
  display: block;
  height: 100%;
  width: 0;
  border-radius: 3px;
  background: linear-gradient(90deg, var(--accent), var(--accent-hi));
  transition: width 0.2s ease;
}

.q-fill.indet {
  width: 35%;
  animation: indet 1.1s ease-in-out infinite alternate;
}

@keyframes indet {
  from {
    margin-left: 0;
  }
  to {
    margin-left: 65%;
  }
}

.q-bytes {
  flex: 0 0 auto;
  font-size: 11.5px;
  color: var(--tx1);
  font-variant-numeric: tabular-nums;
  white-space: nowrap;
}

.q-mirror {
  flex: 0 0 auto;
  font-size: 11px;
  color: var(--tx2);
  border: 1px solid var(--line);
  border-radius: 4px;
  padding: 2px 6px;
  white-space: nowrap;
}

.q-file {
  flex: 0 1 auto;
  min-width: 0;
  font-size: 11.5px;
  color: var(--ok);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.q-err {
  flex: 0 1 auto;
  min-width: 0;
  font-size: 11.5px;
  color: var(--err);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  max-width: 320px;
}

.final-note {
  margin: 0;
  font-size: 12.5px;
  color: var(--tx1);
  line-height: 1.6;
}
</style>
