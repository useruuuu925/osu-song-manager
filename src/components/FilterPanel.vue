<script setup lang="ts">
import { computed } from "vue";
import {
  ALL_MODES,
  ALL_SOURCES,
  ALL_STATUSES,
  AR_BANDS,
  BPM_BANDS,
  STAR_BANDS,
  modeClass,
  modeLabel,
  rangeFieldLabel,
  sourceLabel,
  statusColor,
  statusLabel,
  type Band,
  type FilterState,
  type NumField,
} from "../lib";
import { t } from "../i18n";
import { GameMode, SourceKind, type BeatmapStatus } from "../types";

/** fs 为 App 里 reactive 的筛选状态，本组件直接读写（内部工具，简化双向） */
const props = defineProps<{
  fs: FilterState;
  /** 选项来自已加载 metas 去重（App 计算） */
  genreOptions: string[];
  languageOptions: string[];
}>();

const RANGE_FIELDS = computed<{ key: NumField; label: string }[]>(() =>
  (["star", "ar", "od", "hp", "cs", "bpm", "dur", "objects", "nps", "favs"] as NumField[]).map((k) => ({
    key: k,
    label: rangeFieldLabel(k),
  }))
);

const BAND_GROUPS = computed<{ field: NumField; label: string; bands: Band[] }[]>(() => [
  { field: "star", label: t("band.star"), bands: STAR_BANDS },
  { field: "ar", label: t("band.ar"), bands: AR_BANDS },
  { field: "bpm", label: t("band.bpm"), bands: BPM_BANDS },
]);

function toggleMode(m: GameMode) {
  const i = props.fs.modes.indexOf(m);
  if (i >= 0) props.fs.modes.splice(i, 1);
  else props.fs.modes.push(m);
}

function toggleStatus(s: BeatmapStatus) {
  const i = props.fs.statuses.indexOf(s);
  if (i >= 0) props.fs.statuses.splice(i, 1);
  else props.fs.statuses.push(s);
}

function toggleSource(s: SourceKind) {
  const key = s.toLowerCase();
  const i = props.fs.sources.indexOf(key);
  if (i >= 0) props.fs.sources.splice(i, 1);
  else props.fs.sources.push(key);
}

function bandActive(field: NumField, band: Band): boolean {
  const r = props.fs.ranges[field];
  return r.min === band.min && r.max === band.max;
}

function toggleBand(field: NumField, band: Band) {
  const r = props.fs.ranges[field];
  if (bandActive(field, band)) {
    r.min = "";
    r.max = "";
  } else {
    r.min = band.min;
    r.max = band.max;
  }
}

function toggleDatePreset(p: "7" | "30" | "90") {
  if (props.fs.date.preset === p) {
    props.fs.date.preset = "";
  } else {
    props.fs.date.preset = p;
    props.fs.date.from = "";
    props.fs.date.to = "";
  }
}

function onCustomDate() {
  props.fs.date.preset = "custom";
}

function clearDate() {
  props.fs.date.preset = "";
  props.fs.date.from = "";
  props.fs.date.to = "";
}
</script>

<template>
  <div class="fp">
    <div class="fp-row">
      <div class="f-group">
        <span class="f-label">{{ t("f.mode") }}</span>
        <button
          v-for="m in ALL_MODES"
          :key="m"
          class="f-chip"
          :class="[modeClass(m), { on: fs.modes.includes(m) }]"
          @click="toggleMode(m)"
        >
          {{ modeLabel(m) }}
        </button>
      </div>
      <div class="f-group">
        <span class="f-label">{{ t("f.status") }}</span>
        <button
          v-for="s in ALL_STATUSES"
          :key="s"
          class="f-chip st-chip"
          :style="
            fs.statuses.includes(s)
              ? { background: statusColor(s), borderColor: statusColor(s), color: '#17171f' }
              : { color: statusColor(s) }
          "
          @click="toggleStatus(s)"
        >
          {{ statusLabel(s) }}
        </button>
      </div>
      <div class="f-group">
        <span class="f-label">{{ t("f.lib") }}</span>
        <button
          v-for="s in ALL_SOURCES"
          :key="s"
          class="f-chip"
          :class="{ on: fs.sources.includes(s.toLowerCase()) }"
          @click="toggleSource(s)"
        >
          {{ sourceLabel(s) }}
        </button>
      </div>
    </div>

    <p class="fp-hint">{{ t("f.hint") }}</p>

    <div class="fp-row">
      <div class="f-group">
        <span class="f-label">{{ t("f.genre") }}</span>
        <select v-model="fs.genre" class="f-select">
          <option value="">{{ t("f.unlimited") }}</option>
          <option v-for="g in genreOptions" :key="g" :value="g">{{ g }}</option>
        </select>
      </div>
      <div class="f-group">
        <span class="f-label">{{ t("f.language") }}</span>
        <select v-model="fs.language" class="f-select">
          <option value="">{{ t("f.unlimited") }}</option>
          <option v-for="l in languageOptions" :key="l" :value="l">{{ l }}</option>
        </select>
      </div>
    </div>

    <div class="fp-grid">
      <div v-for="rf in RANGE_FIELDS" :key="rf.key" class="f-group">
        <span class="f-label">{{ rf.label }}</span>
        <input v-model="fs.ranges[rf.key].min" class="f-num" type="number" min="0" placeholder="min" />
        <span class="f-tilde">~</span>
        <input v-model="fs.ranges[rf.key].max" class="f-num" type="number" min="0" placeholder="max" />
      </div>
    </div>

    <div v-for="g in BAND_GROUPS" :key="g.field" class="f-group fp-bandrow">
      <span class="f-label">{{ g.label }}</span>
      <button
        v-for="b in g.bands"
        :key="b.label"
        class="f-chip band"
        :class="{ on: bandActive(g.field, b) }"
        @click="toggleBand(g.field, b)"
      >
        {{ b.label }}
      </button>
    </div>

    <div class="f-group fp-bandrow">
      <span class="f-label">{{ t("f.added") }}</span>
      <button
        v-for="d in (['7', '30', '90'] as const)"
        :key="d"
        class="f-chip band"
        :class="{ on: fs.date.preset === d }"
        @click="toggleDatePreset(d)"
      >
        {{ t("f.lastDays", { 0: d }) }}
      </button>
      <input v-model="fs.date.from" class="f-date" type="date" @input="onCustomDate" />
      <span class="f-tilde">~</span>
      <input v-model="fs.date.to" class="f-date" type="date" @input="onCustomDate" />
      <button v-if="fs.date.preset" class="link-btn" @click="clearDate()">{{ t("action.clear") }}</button>
    </div>
  </div>
</template>

<style scoped>
.fp {
  display: flex;
  flex-direction: column;
  gap: 8px;
  padding: 10px 18px 12px;
  border-bottom: 1px solid var(--line-soft);
  background: var(--bg1);
}

.fp-row {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  gap: 8px 22px;
}

.f-group {
  display: flex;
  align-items: center;
  gap: 6px;
  flex-wrap: wrap;
}

.f-label {
  font-size: 12px;
  color: var(--tx2);
  white-space: nowrap;
}

.fp-hint {
  margin: 0;
  font-size: 11.5px;
  color: var(--tx2);
}

.fp-grid {
  display: grid;
  grid-template-columns: repeat(auto-fill, minmax(min(100%, 212px), 320px));
  gap: 8px 18px;
}

.f-chip {
  border: 1px solid var(--line);
  background: transparent;
  color: var(--tx1);
  font: inherit;
  font-size: 12px;
  line-height: 1;
  padding: 5px 10px;
  border-radius: 20px;
  cursor: pointer;
  transition: all 0.15s;
}

.f-chip:hover {
  border-color: var(--tx2);
  color: var(--tx0);
}

.f-chip.st-chip:hover {
  color: inherit;
  filter: brightness(1.25);
}

.f-chip.band {
  border-radius: 6px;
  padding: 5px 8px;
  font-variant-numeric: tabular-nums;
}

.f-chip.on {
  color: #fff;
  font-weight: 600;
}

.f-chip.band.on {
  background: var(--accent);
  border-color: var(--accent);
  color: #2b0f1d;
}

.f-chip.on.m-osu {
  background: var(--c-osu);
  border-color: var(--c-osu);
  color: #2b0f1d;
}

.f-chip.on.m-taiko {
  background: var(--c-taiko);
  border-color: var(--c-taiko);
  color: #2b1210;
}

.f-chip.on.m-catch {
  background: var(--c-catch);
  border-color: var(--c-catch);
  color: #0e2417;
}

.f-chip.on.m-mania {
  background: var(--c-mania);
  border-color: var(--c-mania);
  color: #13152e;
}

.f-chip.on:not(.m-osu):not(.m-taiko):not(.m-catch):not(.m-mania):not(.band):not(.st-chip) {
  background: var(--accent);
  border-color: var(--accent);
  color: #2b0f1d;
}

.f-num {
  width: 64px;
  background: var(--bg2);
  border: 1px solid var(--line);
  border-radius: 6px;
  color: var(--tx0);
  font: inherit;
  font-size: 12.5px;
  padding: 5px 8px;
}

.f-num:focus,
.f-date:focus {
  outline: none;
  border-color: var(--accent);
}

.f-date {
  background: var(--bg2);
  border: 1px solid var(--line);
  border-radius: 6px;
  color: var(--tx0);
  font: inherit;
  font-size: 12px;
  padding: 4px 6px;
  color-scheme: dark;
}

.f-tilde {
  color: var(--tx2);
  font-size: 12px;
}

.f-select {
  max-width: 180px;
  background: var(--bg2);
  border: 1px solid var(--line);
  border-radius: 6px;
  color: var(--tx0);
  font: inherit;
  font-size: 12px;
  padding: 4px 6px;
}

.f-select:focus {
  outline: none;
  border-color: var(--accent);
}

.fp-bandrow {
  gap: 6px;
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
  padding: 2px 4px;
}

.link-btn:hover {
  color: var(--tx0);
}
</style>
