<script setup lang="ts">
import { computed, nextTick, ref, watch } from "vue";
import {
  bpmText,
  fmtNum,
  formatCount,
  formatDate,
  formatTime,
  modeClass,
  modeLabel,
  sourceLabel,
  statusColor,
  statusLabel,
  displayTitle,
  type SetRow,
} from "../lib";
import { t, trError } from "../i18n";
import { BeatmapStatus, SourceKind } from "../types";
import { pickExportFolder } from "../api";
import {
  bgExportDir,
  bgExportReport,
  bgExportRunning,
  exportOneBg,
  setBgExportDir,
} from "../bgExport";
import {
  cancelConvertTask,
  convertCancelRequested,
  convertDir,
  convertFinished,
  convertMode,
  convertRunning,
  convertTasks,
  ensureConvertListener,
  setConvertDir,
  startConvert,
} from "../convertTasks";

const props = defineProps<{
  row: SetRow;
  /** 从难度视图点入时，滚动并高亮该难度行 */
  focusBeatmapId?: number | null;
  /** 缩略图缓存键对应的展示地址（App 按需生成后传入）；null = 暂不可用 */
  thumbUrl?: string | null;
}>();

const emit = defineEmits<{
  (e: "close"): void;
}>();

const diffWrapRef = ref<HTMLElement | null>(null);

function scrollToFocus() {
  const id = props.focusBeatmapId;
  if (id == null) return;
  void nextTick(() => {
    diffWrapRef.value
      ?.querySelector(`[data-bid="${id}"]`)
      ?.scrollIntoView({ block: "center", behavior: "smooth" });
  });
}

watch(
  () => [props.row.key, props.focusBeatmapId] as const,
  scrollToFocus,
  { immediate: true }
);

// ---------- 内嵌转换卡（仅 lazer 集；状态在共享 store，关面板不中断） ----------
const cvTask = computed(() => convertTasks.value.find((x) => x.setId === props.row.onlineId) ?? null);
const cvSummary = computed(() => {
  if (!convertFinished.value || !cvTask.value) return null;
  return cvTask.value.state === "done"
    ? t("cv.summary", { 0: 1, 1: 0, 2: "" })
    : cvTask.value.state === "cancelled"
      ? t("dl.cancelledN", { 0: 1 })
      : null;
});

async function browseDir() {
  try {
    const dir = await pickExportFolder();
    if (dir) setConvertDir(dir);
  } catch {
    // 目录选择失败保持原值
  }
}

async function startThis() {
  await ensureConvertListener();
  await startConvert([props.row.onlineId]);
}

// ---------- 内嵌导出卡（单集背景图） ----------
async function browseExportDir() {
  try {
    const dir = await pickExportFolder();
    if (dir) setBgExportDir(dir);
  } catch {
    // 目录选择失败保持原值
  }
}

async function exportThisBg() {
  if (!props.row.set.backgroundPath) return;
  await exportOneBg({
    setId: props.row.onlineId,
    artist: props.row.artist,
    title: props.row.title,
    creator: props.row.set.creator,
    sourcePath: props.row.set.backgroundPath,
  });
}
</script>

<template>
  <aside class="panel">
    <header class="p-head">
      <div class="p-titles">
        <h2 :title="displayTitle(row)">{{ displayTitle(row) }}</h2>
        <p>{{ row.artist }}</p>
      </div>
      <button class="close" :aria-label="t('detail.closeAria')" @click="emit('close')">✕</button>
    </header>

    <div class="p-body">
      <!-- 背景预览：thumb 协议按需生成（含在线兜底），无图时占位 -->
      <div class="bg-box" :class="{ dim: !thumbUrl }">
        <img v-if="thumbUrl" :src="thumbUrl" :alt="t('detail.bgPreview')" loading="lazy" />
        <span v-else>{{ row.set.backgroundPath ? t("detail.bgPreview") : t("detail.noBg") }}</span>
      </div>

      <dl class="meta">
        <div><dt>{{ t("detail.creator") }}</dt><dd :title="row.set.creator">{{ row.set.creator || "—" }}</dd></div>
        <div><dt>{{ t("detail.libType") }}</dt><dd>{{ sourceLabel(row.set.sourceKind) }}</dd></div>
        <div>
          <dt>{{ t("detail.status") }}</dt>
          <dd>
            <span
              v-if="row.status !== BeatmapStatus.Unknown"
              class="st-tag"
              :style="{ color: statusColor(row.status), background: statusColor(row.status) + '22' }"
            >
              {{ statusLabel(row.status) }}
            </span>
            <span v-else>—</span>
          </dd>
        </div>
        <div><dt>{{ t("detail.added") }}</dt><dd>{{ formatDate(row.dateAdded) }}</dd></div>
        <div><dt>{{ t("detail.diffCount") }}</dt><dd>{{ row.diffCount }}</dd></div>
        <div><dt>BPM</dt><dd>{{ bpmText(row) }}</dd></div>
        <div><dt>{{ t("detail.longest") }}</dt><dd>{{ formatTime(row.durationMs) }}</dd></div>
        <template v-if="row.isLocal">
          <div class="wide"><dt>{{ t("detail.online") }}</dt><dd>{{ t("detail.onlineLocal") }}</dd></div>
        </template>
        <template v-else>
          <div><dt>{{ t("detail.favs") }}</dt><dd>{{ formatCount(row.favs) }}</dd></div>
          <div><dt>{{ t("detail.plays") }}</dt><dd>{{ formatCount(row.plays) }}</dd></div>
          <div><dt>{{ t("detail.rating") }}</dt><dd>{{ row.rating != null ? row.rating.toFixed(1) : "—" }}</dd></div>
          <div><dt>{{ t("detail.genre") }}</dt><dd :title="row.genre ?? ''">{{ row.genre || "—" }}</dd></div>
          <div><dt>{{ t("detail.language") }}</dt><dd :title="row.language ?? ''">{{ row.language || "—" }}</dd></div>
        </template>
      </dl>

      <div class="tags" v-if="row.set.tags">
        <span class="tags-label">{{ t("detail.tags") }}</span>
        <p :title="row.set.tags">{{ row.set.tags }}</p>
      </div>

      <div class="loc" :title="row.set.location">
        <span class="loc-label">{{ t("detail.location") }}</span>{{ row.set.location }}
      </div>

            <!-- 内嵌转换卡：仅 lazer 集；本地导入集（无在线 ID）置灰说明 -->
      <div v-if="row.set.sourceKind === SourceKind.Lazer" class="cv-card" :class="{ dim: row.onlineId <= 0 }">
        <div class="cv-card-head">
          <span class="cv-card-title">{{ t("cv.cardTitle") }}</span>
          <span v-if="convertRunning && cvTask" class="cv-card-state">{{ t("cv.progress", { 0: convertTasks.filter((x) => x.state === "done").length, 1: convertTasks.length }) }}</span>
        </div>
        <p v-if="row.onlineId <= 0" class="cv-card-note">{{ t("cv.notConvertible") }}</p>
        <template v-else>
          <div class="cv-card-row">
            <button
              class="cbtn"
              :class="{ on: convertMode === 'songs' }"
              :disabled="convertRunning"
              @click="convertMode = 'songs'"
            >
              {{ t("cv.modeSongs") }}
            </button>
            <button
              class="cbtn"
              :class="{ on: convertMode === 'osz' }"
              :disabled="convertRunning"
              @click="convertMode = 'osz'"
            >
              {{ t("cv.modeOsz") }}
            </button>
          </div>
          <div class="cv-card-row dirrow">
            <input class="cdir" :value="convertDir" readonly :placeholder="t('dl.unset')" :title="convertDir" />
            <button class="cbtn" :disabled="convertRunning" @click="browseDir">{{ t("dl.browse") }}</button>
          </div>
          <div class="cv-card-row actions">
            <button v-if="!convertRunning" class="cbtn primary" @click="startThis">
              {{ t("cv.start", { 0: 1 }) }}
            </button>
            <button v-else class="cbtn" @click="cancelConvertTask()">
              {{ convertCancelRequested ? t("dl.cancelling") : t("action.cancel") }}
            </button>
            <span v-if="convertRunning" class="cv-cur">{{ t("cv.currentSet", { 0: convertTasks.find((x) => x.state === "converting")?.current ?? "" }) }}</span>
          </div>
          <p v-if="cvSummary" class="cv-card-note ok">{{ cvSummary }}</p>
          <p v-if="cvTask && cvTask.state === 'failed'" class="cv-card-note err">{{ trError(cvTask.error ?? "") }}</p>
          <p v-for="w in cvTask?.warnings ?? []" :key="w" class="cv-card-note warn">{{ trError(w) }}</p>
        </template>
      </div>

            <!-- 内嵌导出卡：单集背景图导出（有背景源才显示） -->
      <div v-if="row.set.backgroundPath" class="cv-card">
        <div class="cv-card-head">
          <span class="cv-card-title">{{ t("detail.exportBgTitle") }}</span>
        </div>
        <div class="cv-card-row dirrow">
          <input class="cdir" :value="bgExportDir" readonly :placeholder="t('dl.unset')" :title="bgExportDir" />
          <button class="cbtn" :disabled="bgExportRunning" @click="browseExportDir">{{ t("dl.browse") }}</button>
        </div>
        <div class="cv-card-row actions">
          <button v-if="!bgExportRunning" class="cbtn primary" @click="exportThisBg">
            {{ t("detail.exportBg") }}
          </button>
          <span v-else class="cv-cur">{{ t("detail.exportBgRunning") }}</span>
        </div>
        <p
          v-if="bgExportReport"
          class="cv-card-note"
          :class="bgExportReport.exported > 0 ? 'ok' : 'err'"
        >
          {{ t("detail.exportBgDone", { 0: bgExportReport.exported, 1: bgExportReport.failed }) }}
        </p>
      </div>

      <h3>{{ t("detail.diffList") }}</h3>
      <div class="diff-wrap" ref="diffWrapRef">
        <table>
          <thead>
            <tr>
              <th>{{ t("col.version") }}</th>
              <th>{{ t("col.mode") }}</th>
              <th class="num">★</th>
              <th class="num">AR</th>
              <th class="num">CS</th>
              <th class="num">HP</th>
              <th class="num">OD</th>
              <th class="num">{{ t("col.dur") }}</th>
              <th class="num">BPM</th>
              <th class="num">{{ t("col.objects") }}</th>
              <th class="num">nps</th>
            </tr>
          </thead>
          <tbody>
            <tr
              v-for="d in row.diffs"
              :key="d.key"
              :data-bid="d.beatmapId"
              :class="{ hl: focusBeatmapId != null && d.beatmapId === focusBeatmapId }"
            >
              <td class="dv" :title="d.version">{{ d.version || "—" }}</td>
              <td><span class="mode-tag" :class="modeClass(d.mode)">{{ modeLabel(d.mode) }}</span></td>
              <td class="num">{{ d.star != null ? d.star.toFixed(2) : "—" }}</td>
              <td class="num">{{ fmtNum(d.ar) }}</td>
              <td class="num">{{ fmtNum(d.cs) }}</td>
              <td class="num">{{ fmtNum(d.hp) }}</td>
              <td class="num">{{ fmtNum(d.od) }}</td>
              <td class="num">{{ formatTime(d.totalMs) }}</td>
              <td class="num">{{ Math.round(d.bpm) }}</td>
              <td class="num">{{ d.objects.toLocaleString("zh-CN") }}</td>
              <td class="num">{{ d.nps != null ? d.nps.toFixed(1) : "—" }}</td>
            </tr>
          </tbody>
        </table>
      </div>
    </div>
  </aside>
</template>

<style scoped>
.panel {
  width: 100%;
  flex: 1;
  min-width: 0;
  height: 100%;
  display: flex;
  flex-direction: column;
  background: var(--bg1);
  border-left: 1px solid var(--line);
}

.p-head {
  display: flex;
  align-items: flex-start;
  gap: 10px;
  padding: 14px 16px 12px;
  border-bottom: 1px solid var(--line);
}

.p-titles {
  flex: 1;
  min-width: 0;
}

.p-titles h2 {
  margin: 0;
  font-size: 15px;
  line-height: 1.35;
  color: var(--tx0);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.p-titles p {
  margin: 3px 0 0;
  font-size: 12.5px;
  color: var(--tx1);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.close {
  border: none;
  background: transparent;
  color: var(--tx2);
  font-size: 14px;
  cursor: pointer;
  padding: 4px 6px;
  border-radius: 6px;
}

.close:hover {
  color: var(--tx0);
  background: var(--bg3);
}

.p-body {
  flex: 1;
  overflow: auto;
  padding: 14px 16px 20px;
}

.bg-box {
  height: 96px;
  border-radius: 8px;
  border: 1px dashed var(--line);
  background:
    repeating-linear-gradient(135deg, transparent 0 10px, rgba(255, 255, 255, 0.02) 10px 20px),
    var(--bg2);
  display: flex;
  align-items: center;
  justify-content: center;
  margin-bottom: 12px;
  overflow: hidden;
}

.bg-box img {
  width: 100%;
  height: 100%;
  object-fit: cover;
  display: block;
}

.bg-box span {
  font-size: 12px;
  color: var(--tx2);
}

.bg-box.dim {
  opacity: 0.6;
}

/* ── 内嵌转换卡 ── */
.cv-card {
  border: 1px solid var(--line);
  border-radius: 8px;
  padding: 10px 12px;
  margin-bottom: 12px;
  background: var(--bg1);
}

.cv-card.dim {
  opacity: 0.6;
}

.cv-card-head {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 8px;
  margin-bottom: 8px;
}

.cv-card-title {
  font-size: 12.5px;
  font-weight: 600;
}

.cv-card-state {
  font-size: 11px;
  color: var(--tx2);
  font-variant-numeric: tabular-nums;
}

.cv-card-note {
  margin: 6px 0 0;
  font-size: 11.5px;
  color: var(--tx2);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.cv-card-note.ok {
  color: var(--ok);
}

.cv-card-note.err {
  color: var(--err);
}

.cv-card-note.warn {
  color: #e0a24a;
}

.cv-card-row {
  display: flex;
  align-items: center;
  gap: 6px;
  margin-top: 8px;
  min-width: 0;
}

.cv-card-row.dirrow {
  flex: 1 1 auto;
}

.cbtn {
  font-size: 11.5px;
  line-height: 1;
  padding: 6px 10px;
  border-radius: 6px;
  border: 1px solid var(--line);
  background: var(--bg2);
  color: var(--tx1);
  cursor: pointer;
  white-space: nowrap;
  transition: border-color 0.15s, background 0.15s;
}

.cbtn:hover {
  border-color: var(--tx2);
}

.cbtn.on,
.cbtn.primary {
  border-color: var(--accent);
  background: var(--accent-soft);
  color: var(--accent-hi);
}

.cbtn:disabled {
  opacity: 0.55;
  cursor: default;
}

.cdir {
  flex: 1 1 auto;
  min-width: 0;
  font-size: 11.5px;
  padding: 6px 8px;
  border: 1px solid var(--line);
  border-radius: 6px;
  background: var(--bg2);
  color: var(--tx1);
}

.cv-cur {
  font-size: 11px;
  color: var(--tx2);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.bg-box:has(img) {
  border-style: solid;
  background: var(--bg2);
}

.meta {
  display: grid;
  grid-template-columns: 1fr 1fr;
  gap: 6px 14px;
  margin: 0 0 12px;
}

.meta > div {
  min-width: 0;
}

.meta > div.wide {
  grid-column: 1 / -1;
}

.meta dt {
  font-size: 11px;
  color: var(--tx2);
}

.meta dd {
  margin: 1px 0 0;
  font-size: 13px;
  color: var(--tx0);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.st-tag {
  font-size: 11px;
  line-height: 1;
  padding: 3px 7px;
  border-radius: 4px;
  white-space: nowrap;
  display: inline-block;
}

.tags {
  margin-bottom: 10px;
}

.tags p {
  margin: 2px 0 0;
  font-size: 12.5px;
  color: var(--tx1);
  display: -webkit-box;
  -webkit-line-clamp: 3;
  -webkit-box-orient: vertical;
  overflow: hidden;
  word-break: break-all;
}

.tags-label,
.loc-label {
  font-size: 11px;
  color: var(--tx2);
}

.loc {
  font-size: 11.5px;
  color: var(--tx2);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  margin-bottom: 14px;
}

.p-body h3 {
  font-size: 12px;
  color: var(--tx1);
  margin: 0 0 8px;
  font-weight: 600;
}

.diff-wrap {
  border: 1px solid var(--line);
  border-radius: 8px;
  overflow: auto;
}

table {
  width: 100%;
  min-width: 560px; /* 面板拖窄时保持横向滚动，而不是压坏列 */
  border-collapse: collapse;
  font-size: 12px;
  font-variant-numeric: tabular-nums;
}

th {
  position: sticky;
  top: 0;
  background: var(--bg2);
  color: var(--tx1);
  font-weight: 600;
  text-align: left;
  padding: 6px 8px;
  white-space: nowrap;
  border-bottom: 1px solid var(--line);
}

th.num,
td.num {
  text-align: right;
}

td {
  padding: 5px 8px;
  border-bottom: 1px solid var(--line-soft);
  color: var(--tx0);
  white-space: nowrap;
}

tr:last-child td {
  border-bottom: none;
}

tr.hl td {
  background: var(--accent-soft);
  box-shadow: inset 2px 0 0 var(--accent);
}

.dv {
  max-width: 96px;
  overflow: hidden;
  text-overflow: ellipsis;
}

.mode-tag {
  font-size: 10.5px;
  line-height: 1;
  padding: 2.5px 5px;
  border-radius: 4px;
}
</style>
