<script setup lang="ts">
import { computed } from "vue";
import {
  bpmText,
  fmtNum,
  formatCount,
  formatTime,
  modeClass,
  modeLabel,
  statusColor,
  statusLabel,
  type SetRow,
  displayTitle,
  type SortDir,
  type SortKey,
} from "../lib";
import { t } from "../i18n";

defineProps<{
  rows: SetRow[];
  sortKey: SortKey;
  sortDir: SortDir;
  selectedKey: number | null;
  expandedKeys: Set<number>;
  /** lazer 模式下显示多选列（转换所选） */
  showCheckbox?: boolean;
  selectedIds?: Set<number>;
}>();

const emit = defineEmits<{
  (e: "sort", key: SortKey): void;
  (e: "select", row: SetRow): void;
  (e: "toggle-expand", key: number): void;
  (e: "toggle-select", row: SetRow): void;
  (e: "toggle-page"): void;
}>();

function pageAllSelected(selectedIds: Set<number> | undefined, rows: SetRow[]): boolean {
  if (!selectedIds || !rows.length) return false;
  return rows.every((r) => selectedIds.has(r.key));
}

const cols = computed<{ key: SortKey; label: string; cls?: string }[]>(() => [
  { key: "title", label: t("col.title") },
  { key: "artist", label: t("col.artist") },
  { key: "creator", label: t("col.creator") },
  { key: "diffCount", label: t("col.diffCount"), cls: "num" },
  { key: "bpm", label: "BPM", cls: "num" },
  { key: "durationMs", label: t("col.dur"), cls: "num" },
  { key: "favs", label: t("col.favs"), cls: "num" },
  { key: "plays", label: t("col.plays"), cls: "num" },
  { key: "rating", label: t("col.rating"), cls: "num" },
]);
</script>

<template>
  <div class="table-wrap">
    <table>
      <colgroup>
        <col v-if="showCheckbox" style="width: 30px" />
        <col style="width: 22%" />
        <col style="width: 13%" />
        <col style="width: 9%" />
        <col style="width: 5.5%" />
        <col style="width: 7%" />
        <col style="width: 6.5%" />
        <col style="width: 8%" />
        <col style="width: 6.5%" />
        <col style="width: 7%" />
        <col style="width: 7%" />
        <col style="width: 8%" />
      </colgroup>
      <thead>
        <tr>
          <th v-if="showCheckbox" class="nosort sel-col">
            <input
              type="checkbox"
              :checked="pageAllSelected(selectedIds, rows)"
              :aria-label="t('action.selectAllPage')"
              @change="emit('toggle-page')"
            />
          </th>
          <th
            v-for="c in cols"
            :key="c.key"
            :class="[c.cls, { sorted: sortKey === c.key }]"
            tabindex="0"
            @click="emit('sort', c.key)"
            @keydown.enter.prevent="emit('sort', c.key)"
            @keydown.space.prevent="emit('sort', c.key)"
          >
            <span class="th-inner">{{ c.label }}<span class="arrow">{{ sortKey === c.key ? (sortDir === 1 ? "↑" : "↓") : "↕" }}</span></span>
          </th>
          <th class="nosort">{{ t("col.mode") }}</th>
          <th class="nosort">{{ t("col.status") }}</th>
        </tr>
      </thead>
      <tbody>
        <template v-for="row in rows" :key="row.key">
          <tr :class="{ sel: row.key === selectedKey }" @click="emit('select', row)">
            <td v-if="showCheckbox" class="sel-col">
              <input
                type="checkbox"
                :checked="selectedIds?.has(row.key)"
                :aria-label="t('gallery.select')"
                @click.stop
                @change="emit('toggle-select', row)"
              />
            </td>
            <td class="cell-title" :title="`${row.artist} - ${displayTitle(row)}`">
              <button
                class="chev"
                :class="{ open: expandedKeys.has(row.key) }"
                :aria-label="t('aria.expand')"
                @click.stop="emit('toggle-expand', row.key)"
              >
                ▸
              </button>
              {{ displayTitle(row) }}
            </td>
            <td class="cell-artist" :title="row.artist">{{ row.artist }}</td>
            <td class="cell-artist" :title="row.set.creator">{{ row.set.creator || "-" }}</td>
            <td class="num">{{ row.diffCount }}</td>
            <td class="num">{{ bpmText(row) }}</td>
            <td class="num">{{ formatTime(row.durationMs) }}</td>
            <td class="num">
              <span v-if="row.isLocal" class="local-tag" :title="t('local.tagTitle')">{{ t("local.tag") }}</span>
              <template v-else>{{ formatCount(row.favs) }}</template>
            </td>
            <td class="num">{{ formatCount(row.plays) }}</td>
            <td class="num">{{ row.rating != null ? row.rating.toFixed(1) : "—" }}</td>
            <td>
              <span class="modes">
                <span v-for="m in row.modes" :key="m" class="mode-tag" :class="modeClass(m)">{{ modeLabel(m) }}</span>
              </span>
            </td>
            <td>
              <span class="st-tag" :style="{ color: statusColor(row.status), background: statusColor(row.status) + '22' }">
                {{ statusLabel(row.status) }}
              </span>
            </td>
          </tr>
          <tr v-if="expandedKeys.has(row.key)" class="subrow">
            <td colspan="11">
              <div class="sub-inner">
                <table class="sub-table">
                  <thead>
                    <tr>
                      <th>{{ t("col.mode") }}</th>
                      <th>{{ t("col.version") }}</th>
                      <th class="num">★</th>
                      <th class="num">AR</th>
                      <th class="num">OD</th>
                      <th class="num">HP</th>
                      <th class="num">CS</th>
                      <th class="num">BPM</th>
                      <th class="num">{{ t("col.dur") }}</th>
                      <th class="num">{{ t("col.objects") }}</th>
                      <th class="num">nps</th>
                    </tr>
                  </thead>
                  <tbody>
                    <tr v-for="d in row.diffs" :key="d.key">
                      <td><span class="mode-tag" :class="modeClass(d.mode)">{{ modeLabel(d.mode) }}</span></td>
                      <td class="sv" :title="d.version">{{ d.version || "—" }}</td>
                      <td class="num">{{ d.star != null ? d.star.toFixed(2) : "—" }}</td>
                      <td class="num">{{ fmtNum(d.ar, 1) }}</td>
                      <td class="num">{{ fmtNum(d.od, 1) }}</td>
                      <td class="num">{{ fmtNum(d.hp, 1) }}</td>
                      <td class="num">{{ fmtNum(d.cs, 1) }}</td>
                      <td class="num">{{ Math.round(d.bpm) }}</td>
                      <td class="num">{{ formatTime(d.totalMs) }}</td>
                      <td class="num">{{ d.objects.toLocaleString("zh-CN") }}</td>
                      <td class="num">{{ d.nps != null ? d.nps.toFixed(1) : "—" }}</td>
                    </tr>
                  </tbody>
                </table>
              </div>
            </td>
          </tr>
        </template>
      </tbody>
    </table>
  </div>
</template>

<style scoped>
.sel-col {
  width: 30px;
  text-align: center;
}

.sel-col input {
  cursor: pointer;
  accent-color: var(--accent);
}

.table-wrap {


  overflow: auto;
  height: 100%;
  border: 1px solid var(--line);
  border-radius: 10px;
  background: var(--bg1);
}

table {
  width: 100%;
  min-width: 1080px;
  table-layout: fixed;
  border-collapse: separate;
  border-spacing: 0;
  font-size: 13px;
  font-variant-numeric: tabular-nums;
}

thead th {
  position: sticky;
  top: 0;
  z-index: 1;
  background: var(--bg2);
  color: var(--tx1);
  font-weight: 600;
  text-align: left;
  padding: 8px 12px;
  border-bottom: 1px solid var(--line);
  white-space: nowrap;
  cursor: pointer;
  user-select: none;
}

thead th:hover {
  color: var(--tx0);
}

thead th.nosort {
  cursor: default;
}

thead th.nosort:hover {
  color: var(--tx1);
}

thead th.sorted {
  color: var(--accent);
}

.th-inner {
  display: inline-flex;
  gap: 4px;
  align-items: baseline;
}

.arrow {
  font-size: 11px;
  opacity: 0.55;
}

th.sorted .arrow {
  opacity: 1;
}

tbody td {
  padding: 7px 12px;
  border-bottom: 1px solid var(--line-soft);
  color: var(--tx0);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

tbody tr {
  cursor: pointer;
}

tbody tr:hover td {
  background: var(--bg3);
}

tbody tr.sel > td {
  background: var(--accent-soft);
  box-shadow: inset 2px 0 0 var(--accent);
}

th.num,
td.num {
  text-align: right;
}

.cell-title {
  font-weight: 600;
}

.cell-artist {
  color: var(--tx1);
}

/* ---------- 展开箭头 ---------- */
.chev {
  border: none;
  background: transparent;
  color: var(--tx2);
  font-size: 11px;
  line-height: 1;
  padding: 2px 4px 2px 0;
  cursor: pointer;
  transition: transform 0.15s ease, color 0.15s;
  transform-origin: center;
}

.chev:hover {
  color: var(--accent-hi);
}

.chev.open {
  transform: rotate(90deg);
  color: var(--accent-hi);
}

/* ---------- 难度子表 ---------- */
.subrow > td {
  padding: 0;
  background: var(--bg0);
  cursor: default;
}

.sub-inner {
  padding: 8px 14px 10px 34px;
  overflow-x: auto;
}

.sub-table {
  width: auto;
  min-width: 640px;
  table-layout: fixed;
  font-size: 12px;
  border: 1px solid var(--line-soft);
  border-radius: 8px;
  overflow: hidden;
}

.sub-table th {
  position: static;
  background: var(--bg2);
  color: var(--tx2);
  font-weight: 600;
  text-align: left;
  padding: 5px 10px;
  white-space: nowrap;
  cursor: default;
  border-bottom: 1px solid var(--line-soft);
}

.sub-table th.num,
.sub-table td.num {
  text-align: right;
}

.sub-table td {
  padding: 5px 10px;
  border-bottom: 1px solid var(--line-soft);
  color: var(--tx0);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
  max-width: 180px;
}

.sub-table tbody tr:last-child td {
  border-bottom: none;
}

.sub-table tbody tr:hover td {
  background: var(--bg2);
}

.sv {
  max-width: 160px;
}

.modes {
  display: inline-flex;
  gap: 4px;
  flex-wrap: nowrap;
  overflow: hidden;
}

.mode-tag {
  font-size: 11px;
  line-height: 1;
  padding: 3px 6px;
  border-radius: 4px;
  white-space: nowrap;
}

.st-tag {
  font-size: 11px;
  line-height: 1;
  padding: 3px 7px;
  border-radius: 4px;
  white-space: nowrap;
}

.local-tag {
  font-size: 10.5px;
  color: var(--tx2);
  border: 1px solid var(--line);
  border-radius: 4px;
  padding: 2px 5px;
  white-space: nowrap;
}
</style>
