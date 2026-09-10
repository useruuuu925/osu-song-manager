<script setup lang="ts">
import { computed } from "vue";
import { displayTitle, formatCount, modeClass, modeLabel, statusColor, statusLabel, type SetRow } from "../lib";
import { t } from "../i18n";

const props = defineProps<{
  rows: SetRow[];
  /** rowKey -> 缓存键；null = 生成失败（占位、不可选）；缺项 = 待生成 */
  thumbMap: Map<number, string | null>;
  selectedKeys: Set<number>;
  /** 最近一次导出的行 key（显示 ✓） */
  exportedKeys: Set<number>;
  /** 行是否有可用背景源（.osz 内条目不算） */
  eligible: (row: SetRow) => boolean;
}>();

const emit = defineEmits<{
  (e: "toggle", row: SetRow): void;
  (e: "open", row: SetRow): void;
  (e: "img-error", row: SetRow): void;
}>();

interface Card {
  row: SetRow;
  url: string | null;
  failed: boolean;
  pending: boolean;
  canPick: boolean;
  sel: boolean;
  exported: boolean;
  tip: string;
}

const cards = computed<Card[]>(() =>
  props.rows.map((row) => {
    const k = props.thumbMap.get(row.key);
    const ok = typeof k === "string";
    const failed = k === null;
    const pending = k === undefined && props.eligible(row);
    const hasBg = row.set.backgroundPath != null && row.set.backgroundPath !== "";
    return {
      row,
      url: ok ? `http://thumb.localhost/${k}` : null,
      failed,
      pending,
      canPick: props.eligible(row) && !failed,
      sel: props.selectedKeys.has(row.key),
      exported: props.exportedKeys.has(row.key),
      tip: failed
        ? t("gallery.bgFailed")
        : props.eligible(row)
          ? t("gallery.pickTip")
          : hasBg
            ? t("gallery.oszBg")
            : t("gallery.noBg"),
    };
  })
);
</script>

<template>
  <div class="grid">
    <div
      v-for="c in cards"
      :key="c.row.key"
      class="card"
      :class="{ sel: c.sel }"
      role="button"
      tabindex="0"
      @click="emit('open', c.row)"
      @keydown.enter.prevent="emit('open', c.row)"
      @keydown.space.prevent="emit('open', c.row)"
    >
      <div
        class="thumb"
        :class="{ dead: !c.canPick }"
        :title="c.tip"
        :aria-label="c.tip"
        @click.stop="c.canPick && emit('toggle', c.row)"
      >
        <img v-if="c.url" :src="c.url" loading="lazy" alt="" @error="emit('img-error', c.row)" />
        <span v-else class="ph">{{ c.pending ? "♪" : t("gallery.bgFailed") }}</span>
        <span
          v-if="c.canPick"
          class="pick"
          :class="{ on: c.sel }"
          role="checkbox"
          :aria-checked="c.sel"
          :aria-label="c.sel ? t('gallery.deselect') : t('gallery.select')"
          tabindex="0"
          @click.stop="emit('toggle', c.row)"
          @keydown.enter.prevent.stop="emit('toggle', c.row)"
          @keydown.space.prevent.stop="emit('toggle', c.row)"
        >{{ c.sel ? "✓" : "" }}</span>
        <span v-if="c.exported" class="done" :title="t('gallery.exportedTip')">✓</span>
      </div>

      <div class="card-info">
        <div class="card-title" :title="c.row.title">{{ displayTitle(c.row) }}</div>
        <div class="card-sub" :title="c.row.artist">{{ c.row.artist }}</div>
        <div class="card-foot">
          <span class="card-modes">
            <span v-for="m in c.row.modes" :key="m" class="mode-tag" :class="modeClass(m)">{{ modeLabel(m) }}</span>
          </span>
          <span class="st-tag" :style="{ color: statusColor(c.row.status), background: statusColor(c.row.status) + '22' }">
            {{ statusLabel(c.row.status) }}
          </span>
        </div>
        <div class="card-meta">
          {{ t("gallery.nDiffs", { 0: c.row.diffCount }) }}<template v-if="c.row.favs != null"> · {{ t("gallery.nFavs", { 0: formatCount(c.row.favs) }) }}</template>
        </div>
      </div>
    </div>
  </div>
</template>

<style scoped>
.grid {
  flex: 1;
  min-height: 0;
  overflow: auto;
  display: grid;
  /* 卡片宽钳制在 [236px, 320px]：窗口宽不无限拉伸，窗口窄过 236px 时跟随收缩不溢出 */
  grid-template-columns: repeat(auto-fill, minmax(min(100%, 236px), 320px));
  /* 行高必须显式按内容：Chrome 对带 overflow:hidden 的 aspect-ratio 网格项
     内在高度贡献为 0，缺这条会把行高压缩成容器高度平摊（卡片被裁成切片） */
  grid-auto-rows: max-content;
  gap: 14px;
  align-content: start;
  padding-bottom: 8px;
}

.card {
  background: var(--bg1);
  border: 1px solid var(--line);
  border-radius: 10px;
  overflow: hidden;
  cursor: pointer;
  transition: border-color 0.15s, transform 0.15s, box-shadow 0.15s;
}

.card:focus-visible,
.pick:focus-visible {
  outline: 2px solid var(--accent);
  outline-offset: 2px;
}

.card:hover {
  border-color: var(--tx2);
  transform: translateY(-2px);
}

.card.sel {
  border-color: var(--accent);
  box-shadow: 0 0 0 1px var(--accent), 0 6px 18px rgba(255, 95, 162, 0.15);
}

.thumb {
  position: relative;
  aspect-ratio: 16 / 9;
  background:
    radial-gradient(circle at 70% 20%, rgba(255, 95, 162, 0.12), transparent 60%),
    linear-gradient(160deg, var(--bg3), var(--bg2));
  overflow: hidden;
}

.thumb img {
  width: 100%;
  height: 100%;
  object-fit: cover;
  display: block;
}

.thumb .ph {
  position: absolute;
  inset: 0;
  display: flex;
  align-items: center;
  justify-content: center;
  font-size: 13px;
  color: var(--tx2);
}

.thumb.dead {
  background: repeating-linear-gradient(135deg, transparent 0 10px, rgba(255, 255, 255, 0.02) 10px 20px), var(--bg2);
  outline: 1px dashed var(--line);
  outline-offset: -5px;
  border-radius: 8px;
  cursor: default;
}

.pick {
  position: absolute;
  left: 8px;
  top: 8px;
  width: 22px;
  height: 22px;
  display: flex;
  align-items: center;
  justify-content: center;
  font-size: 13px;
  font-weight: 700;
  color: #1a0d14;
  background: rgba(10, 10, 14, 0.55);
  border: 1px solid rgba(255, 255, 255, 0.35);
  border-radius: 6px;
  cursor: pointer;
  transition: background 0.15s, border-color 0.15s;
}

.pick:hover {
  border-color: var(--accent-hi);
}

.pick.on {
  background: var(--accent);
  border-color: var(--accent);
}

.done {
  position: absolute;
  right: 8px;
  top: 8px;
  width: 20px;
  height: 20px;
  line-height: 20px;
  text-align: center;
  font-size: 12px;
  color: var(--ok);
  background: rgba(10, 10, 14, 0.6);
  border-radius: 50%;
}

.card-info {
  padding: 9px 10px 11px;
}

.card-title {
  font-size: 13px;
  font-weight: 600;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.card-sub {
  font-size: 12px;
  color: var(--tx1);
  margin-top: 2px;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.card-foot {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 6px;
  margin-top: 7px;
}

.card-modes {
  display: flex;
  gap: 4px;
  flex-wrap: wrap;
  min-width: 0;
}

.card-meta {
  margin-top: 6px;
  font-size: 11px;
  color: var(--tx2);
  font-variant-numeric: tabular-nums;
}

.mode-tag {
  font-size: 10.5px;
  line-height: 1;
  padding: 2.5px 5px;
  border-radius: 4px;
  white-space: nowrap;
}

.st-tag {
  font-size: 10.5px;
  line-height: 1;
  padding: 2.5px 6px;
  border-radius: 4px;
  white-space: nowrap;
  flex: 0 0 auto;
}
</style>
