<script setup lang="ts">
import { computed, ref } from "vue";
import { pickExportFolder } from "../api";
import { formatBytes, parseIdList } from "../lib";
import { mk, sm, t, trError, type Slot } from "../i18n";
import type { BeatmapSetInfo, ConvertState } from "../types";
import {
  cancelConvertTask,
  convertBatchError,
  convertDoneCount,
  convertCancelRequested,
  convertDir,
  convertFailedCount,
  convertFinished,
  convertMode,
  convertReport,
  convertRunning,
  convertTasks,
  setConvertDir,
  startConvert,
} from "../convertTasks";

const props = defineProps<{
  /** 当前曲库扫描结果（lazer 模式下每行 location = 在线 ID 字符串） */
  library: BeatmapSetInfo[];
  /** 当前模式是否为 lazer（非 lazer 时展示提示并禁用） */
  isLazer: boolean;
}>();

// ---------- 输入 ----------
/** 与后端 convert.rs 的 MAX_SETS_PER_CALL 一致，超限前端先拦截 */
const MAX_CONVERT = 2000;
const PREVIEW_CHIPS = 20;

const sourceMode = ref<"all" | "ids">("all");
const inputText = ref("");
// mode / targetDir 为共享 store 状态（详情面板/状态条同源）
const mode = convertMode;
const targetDir = convertDir;
/** 时点消息槽：渲染期 sm() 取词，语言切换即时回溯 */
const dirHint = ref<Slot | null>(null);

/** 当前曲库中可转换的集（有在线 ID）与本地导入集数量 */
const convertible = computed(() => props.library.filter((s) => s.beatmapsetId > 0));
const localCount = computed(() => props.library.length - convertible.value.length);

const parsedIds = computed(() => parseIdList(inputText.value));
const willRun = computed<number[]>(() => {
  if (!props.isLazer) return [];
  if (sourceMode.value === "ids") return parsedIds.value.slice(0, MAX_CONVERT);
  return convertible.value.map((s) => s.beatmapsetId).slice(0, MAX_CONVERT);
});
const truncated = computed(() => {
  if (!props.isLazer) return false;
  const total = sourceMode.value === "ids" ? parsedIds.value.length : convertible.value.length;
  return total > MAX_CONVERT;
});
const previewChips = computed(() => willRun.value.slice(0, PREVIEW_CHIPS));

// ---------- 任务队列（共享 store：任务状态模块级，切页/关面板不丢） ----------
const tasks = convertTasks;
const running = convertRunning;
const finished = convertFinished;
const report = convertReport;
const batchError = convertBatchError;
const cancelRequested = convertCancelRequested;
const doneCount = convertDoneCount;
const failedCount = convertFailedCount;
const cancelledCount = computed(() => tasks.value.filter((x) => x.state === "cancelled").length);

function stateText(s: ConvertState): string {
  switch (s) {
    case "queued":
      return t("cv.state.queued");
    case "converting":
      return t("cv.state.converting");
    case "done":
      return t("cv.state.done");
    case "failed":
      return t("cv.state.failed");
    case "cancelled":
      return t("cv.state.cancelled");
  }
}

async function browse() {
  try {
    const dir = await pickExportFolder();
    if (dir) {
      setConvertDir(dir);
      dirHint.value = null;
    }
    // 取消：保持原值
  } catch (e) {
    dirHint.value = { err: e };
  }
}

async function start(ids: number[]) {
  if (running.value || !ids.length || !props.isLazer) return;
  if (!convertDir.value) {
    dirHint.value = mk("dl.needDir");
    return;
  }
  dirHint.value = null;
  await startConvert(ids);
}

function startAll() {
  void start(willRun.value);
}

function retryFailed() {
  const ids = tasks.value.filter((x) => x.state === "failed").map((x) => x.setId);
  void start(ids);
}

async function cancel() {
  await cancelConvertTask();
}
</script>

<template>
  <div class="cv">
    <p v-if="!isLazer" class="cv-guard">{{ t("cv.onlyLazer") }}</p>

    <template v-else>
      <!-- 未扫描指引 -->
      <div v-if="!library.length" class="ws-card cv-empty">
        <p>{{ t("empty.notScanned") }}</p>
        <p class="hint">{{ t("empty.notScannedSub") }}</p>
      </div>

      <template v-else>
        <!-- 输入 -->
        <div class="ws-card">
          <div class="row">
            <label class="opt">
              <input v-model="sourceMode" type="radio" value="all" :disabled="running" />
              {{ t("cv.allLabel") }}
            </label>
            <label class="opt">
              <input v-model="sourceMode" type="radio" value="ids" :disabled="running" />
              {{ t("cv.idsTitle") }}
            </label>
          </div>
          <textarea
            v-if="sourceMode === 'ids'"
            v-model="inputText"
            class="ta"
            rows="4"
            spellcheck="false"
            :placeholder="t('cv.idsPh')"
          ></textarea>
          <div v-if="sourceMode === 'ids' && previewChips.length" class="row preview">
            <span class="parse-n">{{ t("dl.parsed", { 0: willRun.length }) }}</span>
            <span v-for="id in previewChips" :key="id" class="id-chip">{{ id }}</span>
            <span v-if="willRun.length > PREVIEW_CHIPS" class="hint">{{ t("dl.more", { 0: willRun.length }) }}</span>
          </div>
          <p class="hint">
            {{ sourceMode === "ids" ? t("cv.idsHint") : t("cv.willConvertAll", { 0: willRun.length }) }}
            <template v-if="localCount > 0">　{{ t("cv.localSkip", { 0: localCount }) }}</template>
          </p>
          <p v-if="truncated" class="err-line">{{ t("cv.truncated", { 0: MAX_CONVERT }) }}</p>
        </div>

        <!-- 输出格式与目标 -->
        <div class="ws-card">
          <div class="row">
            <span class="sec-title">{{ t("cv.modeTitle") }}</span>
            <label class="opt">
              <input v-model="mode" type="radio" value="songs" :disabled="running" />
              {{ t("cv.modeSongs") }}
            </label>
            <label class="opt">
              <input v-model="mode" type="radio" value="osz" :disabled="running" />
              {{ t("cv.modeOsz") }}
            </label>
          </div>
          <div class="row">
            <span class="sec-title">{{ t("dl.saveTo") }}</span>
            <input class="dir" :value="targetDir" readonly :placeholder="t('dl.unset')" :title="targetDir" />
            <button class="btn sm" :disabled="running" @click="browse()">{{ t("dl.browse") }}</button>
          </div>
          <p v-if="dirHint" class="err-line">{{ sm(dirHint) }}</p>
          <div class="row">
            <button class="btn primary sm" :disabled="running || !willRun.length" @click="startAll()">
              {{ running ? t("cv.btnConverting") : t("cv.start", { 0: willRun.length }) }}
            </button>
            <button v-if="running" class="btn sm danger" :disabled="cancelRequested" @click="cancel()">
              {{ cancelRequested ? t("dl.cancelling") : t("action.cancel") }}
            </button>
          </div>
        </div>

        <!-- 队列 + 结果 -->
        <div v-if="tasks.length" class="ws-card">
          <div class="row agg">
            <span class="parse-n">{{ t("cv.progress", { 0: doneCount, 1: tasks.length }) }}</span>
            <span v-if="failedCount" class="err-line">{{ t("cv.failedN", { 0: failedCount }) }}</span>
            <span v-if="cancelledCount" class="hint">{{ t("dl.cancelledN", { 0: cancelledCount }) }}</span>
            <button v-if="finished && failedCount" class="btn xs retry" @click="retryFailed()">{{ t("cv.retry") }}</button>
          </div>
          <p v-if="batchError" class="err-line">{{ sm(batchError) }}</p>
          <div class="queue">
            <div v-for="task in tasks" :key="task.setId" class="q-row">
              <span class="q-id">{{ task.setId }}</span>
              <span class="q-state" :class="`s-${task.state}`">{{ stateText(task.state) }}</span>
              <span class="q-name" :title="task.current">{{ task.current }}</span>
              <span
                v-if="task.state === 'done' && task.result"
                class="q-file"
                :title="task.result.output ?? ''"
              >
                {{ t("cv.files", { 0: task.result.files, 1: formatBytes(task.result.bytes) }) }}
              </span>
              <span
                v-if="task.warnings.length"
                class="q-warn"
                :title="task.warnings.map(trError).join('；')"
              >
                ⚠ {{ task.warnings.length }}
              </span>
              <span v-else-if="task.error" class="q-err" :title="task.error">{{ trError(task.error) }}</span>
            </div>
          </div>
          <div v-if="finished && report" class="final">
            <p class="final-note">
              {{ t("cv.summary", { 0: report.converted, 1: report.failed, 2: report.cancelled ? t("cv.cancelledSuffix") : "" }) }}
            </p>
            <div class="row">
              <span class="sec-title">{{ t("cv.output") }}</span>
              <input class="dir" :value="report.outputDir" readonly :title="report.outputDir" />
            </div>
            <p class="final-note">{{ t("cv.finalNote") }}</p>
          </div>
        </div>
      </template>
    </template>
  </div>
</template>

<style scoped>
.cv {
  position: relative;
  flex: 1;
  min-height: 0;
  overflow: auto;
  display: flex;
  flex-direction: column;
  gap: 10px;
  padding-bottom: 12px;
}

.cv-guard {
  margin: 0;
  font-size: 12.5px;
  color: var(--warn);
}

.cv-empty p {
  margin: 0;
  font-size: 13px;
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

.sec-title {
  font-size: 13px;
  font-weight: 600;
  white-space: nowrap;
}

.hint {
  font-size: 11.5px;
  color: var(--tx2);
  margin: 0;
}

.err-line {
  margin: 0;
  font-size: 12px;
  color: var(--err);
  word-break: break-all;
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

.parse-n {
  font-size: 12.5px;
  color: var(--tx1);
  font-variant-numeric: tabular-nums;
  white-space: nowrap;
}

.agg {
  gap: 12px;
  padding-bottom: 8px;
  border-bottom: 1px solid var(--line-soft);
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

.s-converting {
  color: var(--c-mania);
  background: rgba(139, 149, 255, 0.14);
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

.q-name {
  flex: 1 1 auto;
  min-width: 0;
  color: var(--tx1);
  overflow: hidden;
  text-overflow: ellipsis;
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
.q-warn {
  color: #e0a24a;
  font-size: 11.5px;
  cursor: help;
}

.final {
  display: flex;
  flex-direction: column;
  gap: 8px;
}

.final-note {
  margin: 0;
  font-size: 12.5px;
  color: var(--tx1);
  line-height: 1.6;
}
</style>
