<script setup lang="ts">
import { computed } from "vue";
import {
  fmtNum,
  formatCount,
  formatTime,
  modeClass,
  modeLabel,
  type DiffRow,
  type DiffSortKey,
  type SortDir,
} from "../lib";
import { t } from "../i18n";

defineProps<{
  rows: DiffRow[];
  sortKey: DiffSortKey;
  sortDir: SortDir;
  selectedKey: number | null;
}>();

const emit = defineEmits<{
  (e: "sort", key: DiffSortKey): void;
  (e: "select", row: DiffRow): void;
}>();

const cols = computed<{ key?: DiffSortKey; label: string; cls?: string }[]>(() => [
  { key: "title", label: t("col.songTitle") },
  { key: "version", label: t("col.version") },
  { label: t("col.mode") },
  { key: "star", label: "★", cls: "num" },
  { key: "ar", label: "AR", cls: "num" },
  { key: "od", label: "OD", cls: "num" },
  { key: "hp", label: "HP", cls: "num" },
  { key: "cs", label: "CS", cls: "num" },
  { key: "bpm", label: "BPM", cls: "num" },
  { key: "durationMs", label: t("col.dur"), cls: "num" },
  { key: "objects", label: t("col.objects"), cls: "num" },
  { key: "nps", label: "nps", cls: "num" },
  { key: "favs", label: t("col.favs"), cls: "num" },
  { key: "plays", label: t("col.plays"), cls: "num" },
  { key: "rating", label: t("col.rating"), cls: "num" },
]);
</script>

<template>
  <div class="table-wrap">
    <table>
      <colgroup>
        <col style="width: 17%" />
        <col style="width: 9%" />
        <col style="width: 6%" />
        <col style="width: 5.5%" />
        <col style="width: 5%" />
        <col style="width: 5%" />
        <col style="width: 5%" />
        <col style="width: 5%" />
        <col style="width: 6%" />
        <col style="width: 7%" />
        <col style="width: 6.5%" />
        <col style="width: 5.5%" />
        <col style="width: 6%" />
        <col style="width: 6%" />
        <col style="width: 5.5%" />
      </colgroup>
      <thead>
        <tr>
          <template v-for="(c, ci) in cols" :key="ci">
            <th
              v-if="c.key"
              :class="[c.cls, { sorted: sortKey === c.key }]"
              tabindex="0"
              @click="emit('sort', c.key)"
              @keydown.enter.prevent="emit('sort', c.key)"
              @keydown.space.prevent="emit('sort', c.key)"
            >
              <span class="th-inner"
                >{{ c.label }}<span class="arrow">{{ sortKey === c.key ? (sortDir === 1 ? "↑" : "↓") : "↕" }}</span></span
              >
            </th>
            <th v-else class="nosort">{{ c.label }}</th>
          </template>
        </tr>
      </thead>
      <tbody>
        <tr
          v-for="row in rows"
          :key="row.key"
          :class="{ sel: row.key === selectedKey }"
          @click="emit('select', row)"
        >
          <td class="cell-title" :title="`${row.artist} - ${row.title}`">{{ row.title }}</td>
          <td class="cell-artist" :title="row.version">{{ row.version || "—" }}</td>
          <td><span class="mode-tag" :class="modeClass(row.mode)">{{ modeLabel(row.mode) }}</span></td>
          <td class="num">{{ row.star != null ? row.star.toFixed(2) : "—" }}</td>
          <td class="num">{{ fmtNum(row.ar, 1) }}</td>
          <td class="num">{{ fmtNum(row.od, 1) }}</td>
          <td class="num">{{ fmtNum(row.hp, 1) }}</td>
          <td class="num">{{ fmtNum(row.cs, 1) }}</td>
          <td class="num">{{ Math.round(row.bpm) }}</td>
          <td class="num">{{ formatTime(row.totalMs) }}</td>
          <td class="num">{{ row.objects.toLocaleString("zh-CN") }}</td>
          <td class="num">{{ row.nps != null ? row.nps.toFixed(1) : "—" }}</td>
          <td class="num">
            <span v-if="row.isLocal" class="local-tag" :title="t('local.tagTitle')">{{ t("local.tag") }}</span>
            <template v-else>{{ formatCount(row.favs) }}</template>
          </td>
          <td class="num">{{ formatCount(row.plays) }}</td>
          <td class="num">{{ row.rating != null ? row.rating.toFixed(1) : "—" }}</td>
        </tr>
      </tbody>
    </table>
  </div>
</template>

<style scoped>
.table-wrap {
  overflow: auto;
  height: 100%;
  border: 1px solid var(--line);
  border-radius: 10px;
  background: var(--bg1);
}

table {
  width: 100%;
  min-width: 1420px;
  table-layout: fixed;
  border-collapse: separate;
  border-spacing: 0;
  font-size: 12.5px;
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
  padding: 7px 9px;
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
  gap: 3px;
  align-items: baseline;
}

.arrow {
  font-size: 10.5px;
  opacity: 0.55;
}

th.sorted .arrow {
  opacity: 1;
}

tbody td {
  padding: 6px 9px;
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

tbody tr.sel td {
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

.mode-tag {
  font-size: 10.5px;
  line-height: 1;
  padding: 2.5px 5px;
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
