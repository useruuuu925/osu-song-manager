<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, ref, watch } from "vue";
import { deleteToTrash, detectDuplicates, moveSets, packArchives, pickExportFolder, scanEmptyFolders } from "../api";
import { formatDate, formatBytes } from "../lib";
import { lang, mk, sm, t, type Slot } from "../i18n";
import { SourceKind, type DupMember, type DuplicateGroup } from "../types";

const props = defineProps<{
  kind: SourceKind;
  path: string;
}>();

const isLazer = computed(() => props.kind.toLowerCase() === SourceKind.Lazer.toLowerCase());
const isStable = computed(() => props.kind.toLowerCase() === SourceKind.Stable.toLowerCase());
const kindLabel = computed(() =>
  isLazer.value ? t("manage.kind.lazer") : isStable.value ? t("manage.kind.stable") : t("manage.kind.osz")
);

// ---------- 查重 ----------
const dupScanning = ref(false);
const dupError = ref<Slot | null>(null);
const groups = ref<DuplicateGroup[] | null>(null);
const scannedTarget = ref(""); // 上次查重针对的 kind+path，变了就提示重新查重

const targetKey = computed(() => `${props.kind}|${props.path}`);
const staleResult = computed(
  () => !!groups.value && scannedTarget.value !== "" && scannedTarget.value !== targetKey.value
);

/** 跨组共享选择（按 location） */
const sel = ref(new Set<string>());
/** 已删除/已移动成功的 location，标记后不再参与操作 */
const doneLocs = ref(new Set<string>());

function toggleLoc(loc: string) {
  const next = new Set(sel.value);
  if (next.has(loc)) next.delete(loc);
  else next.add(loc);
  sel.value = next;
}

function selectable(m: DupMember): boolean {
  return !doneLocs.value.has(m.location);
}

/** 每组建议保留 = 第一个成员（后端契约里 firstAdded 是组级值，无法逐成员比较日期） */
function suggestKeepIndex(_g: DuplicateGroup): number {
  return 0;
}

function selectGroupRedundant(g: DuplicateGroup) {
  const keep = suggestKeepIndex(g);
  const next = new Set(sel.value);
  g.members.forEach((m, i) => {
    if (i !== keep && selectable(m)) next.add(m.location);
  });
  sel.value = next;
}

function selectAllRedundant() {
  for (const g of groups.value ?? []) selectGroupRedundant(g);
}

function clearSelection() {
  sel.value = new Set();
}

const selectedPaths = computed(() => [...sel.value]);
const selectedCount = computed(() => sel.value.size);

function reasonLabel(reason: string): string {
  switch (reason) {
    case "setOnlineId":
      return t("reason.setOnlineId");
    case "md5":
      return t("reason.md5");
    case "identicalSet":
      return t("reason.identicalSet");
    default:
      return reason;
  }
}

function reasonClass(reason: string): string {
  switch (reason) {
    case "setOnlineId":
      return "r-id";
    case "md5":
      return "r-md5";
    case "identicalSet":
      return "r-same";
    default:
      return "";
  }
}

function memberSize(m: DupMember): string {
  return m.sizeBytes > 0 ? formatBytes(m.sizeBytes) : "—";
}

async function runDetect() {
  if (!props.path) {
    dupError.value = mk("manage.needPathFirst");
    return;
  }
  dupScanning.value = true;
  dupError.value = null;
  try {
    const keyAtStart = targetKey.value;
    const g = await detectDuplicates(props.kind, props.path);
    groups.value = g;
    scannedTarget.value = keyAtStart;
    // 清理不再存在的选中项
    const alive = new Set<string>();
    for (const grp of g) for (const m of grp.members) alive.add(m.location);
    const next = new Set([...sel.value].filter((l) => alive.has(l)));
    sel.value = next;
  } catch (e) {
    dupError.value = { err: e };
    groups.value = null;
  } finally {
    dupScanning.value = false;
  }
}

// ---------- 空文件夹（stable） ----------
const emptyOpen = ref(false);
const emptyScanning = ref(false);
const emptyResult = ref<string[] | null>(null);
const emptyError = ref<Slot | null>(null);

async function runEmptyScan() {
  if (!props.path) return;
  emptyScanning.value = true;
  emptyError.value = null;
  try {
    emptyResult.value = await scanEmptyFolders(props.path);
  } catch (e) {
    emptyError.value = { err: e };
    emptyResult.value = null;
  } finally {
    emptyScanning.value = false;
  }
}

// ---------- 两步确认 + 执行 ----------
type OpKind = "delete" | "move" | "pack";
interface Plan {
  op: OpKind;
  paths: string[];
  targetDir?: string;
  outFile?: string;
}

const plan = ref<Plan | null>(null);
const planExpanded = ref(false);
const busy = ref(false);

interface OpResult {
  op: OpKind;
  /** 时点消息槽：渲染期 sm() 取词，语言切换即时回溯 */
  summary: Slot;
  /** 逐条失败/跳过明细：{key,params} 槽（params 支持嵌套 {err}），同样随语言回溯 */
  errors: Slot[];
  errorsTitle: Slot;
  tone: "ok" | "warn";
}

const lastResult = ref<OpResult | null>(null);
const resultOpen = ref(false);

function askDelete() {
  if (!selectedCount.value || isLazer.value) return;
  plan.value = { op: "delete", paths: selectedPaths.value };
  planExpanded.value = false;
}

async function askMove() {
  if (!selectedCount.value || isLazer.value || busy.value) return;
  busy.value = true;
  try {
    const dir = await pickExportFolder(); // 复用现有文件夹选择命令（rfd）
    if (!dir) return;
    plan.value = { op: "move", paths: selectedPaths.value, targetDir: dir };
    planExpanded.value = false;
  } catch (e) {
    showFailure(t("manage.pickFailTitle"), e);
  } finally {
    busy.value = false;
  }
}

function askPack() {
  if (!selectedCount.value || busy.value) return;
  if (!outFile.value.trim()) {
    dupToast.value = t("manage.toastPackFirst");
    scheduleToastHide();
    return;
  }
  plan.value = { op: "pack", paths: selectedPaths.value, outFile: outFile.value.trim() };
  planExpanded.value = false;
}

function showFailure(title: Slot, e: unknown) {
  lastResult.value = {
    op: "delete",
    summary: title,
    errors: [{ err: e }],
    errorsTitle: mk("manage.errorList"),
    tone: "warn",
  };
  resultOpen.value = true;
}

async function execPlan() {
  const p = plan.value;
  if (!p || busy.value) return;
  plan.value = null;
  busy.value = true;
  try {
    if (p.op === "delete") {
      const rep = await deleteToTrash(p.paths);
      const done = new Set(doneLocs.value);
      for (const loc of rep.deleted) done.add(loc);
      doneLocs.value = done;
      lastResult.value = {
        op: "delete",
        summary: mk("manage.barMoved", { 0: rep.deleted.length, 1: rep.failed.length }),
        errors: rep.failed.map((f) => mk("manage.failedPath", { 0: f.path, 1: { err: f.error } })),
        errorsTitle: mk("manage.failList"),
        tone: rep.failed.length ? "warn" : "ok",
      };
      if (rep.deleted.length) showToast(t("manage.toastMoved"));
    } else if (p.op === "move") {
      const rep = await moveSets(p.paths, p.targetDir ?? "");
      const done = new Set(doneLocs.value);
      // moved 记录的是目标端路径，用源选择里"未出现在失败清单"的部分来标记
      const failedSrc = new Set(rep.failed.map((f) => f.path));
      for (const loc of p.paths) if (!failedSrc.has(loc)) done.add(loc);
      doneLocs.value = done;
      lastResult.value = {
        op: "move",
        summary: mk("manage.barMovedDone", { 0: rep.moved.length, 1: rep.failed.length }),
        errors: rep.failed.map((f) => mk("manage.failedPath", { 0: f.path, 1: { err: f.error } })),
        errorsTitle: mk("manage.failList"),
        tone: rep.failed.length ? "warn" : "ok",
      };
      if (rep.moved.length) showToast(t("manage.toastMovedDone"));
    } else {
      const rep = await packArchives(p.paths, p.outFile ?? "");
      lastResult.value = {
        op: "pack",
        summary: mk("manage.barPacked", { 0: rep.packed, 1: formatBytes(rep.bytes), 2: rep.skipped.length }),
        errors: rep.skipped,
        errorsTitle: mk("manage.skipList"),
        tone: rep.skipped.length ? "warn" : "ok",
      };
    }
    resultOpen.value = lastResult.value.errors.length > 0;
    clearSelection();
  } catch (e) {
    lastResult.value = {
      op: p.op,
      summary: mk(
        p.op === "pack" ? "manage.incompletePack" : p.op === "delete" ? "manage.incompleteDelete" : "manage.incompleteMove"
      ),
      errors: [{ err: e }],
      errorsTitle: mk("manage.errorList"),
      tone: "warn",
    };
    resultOpen.value = true;
  } finally {
    busy.value = false;
  }
}

// ---------- 两步确认弹层：Escape 关闭 + 打开时聚焦「取消」 ----------
const planCancelButton = ref<HTMLButtonElement | null>(null);

function onPlanKeydown(e: KeyboardEvent) {
  if (e.key === "Escape") {
    e.stopPropagation();
    plan.value = null;
  }
}

watch(plan, (p) => {
  if (p) {
    window.addEventListener("keydown", onPlanKeydown, true);
    void nextTick(() => planCancelButton.value?.focus());
  } else {
    window.removeEventListener("keydown", onPlanKeydown, true);
  }
});

onBeforeUnmount(() => window.removeEventListener("keydown", onPlanKeydown, true));

// ---------- 打包输出路径 ----------
function defaultOut(): string {
  const root = props.path.replace(/[\\/]+$/, "");
  const d = new Date();
  const p = (n: number) => String(n).padStart(2, "0");
  const stamp = `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())}`;
  const name = t("manage.archiveName", { 0: stamp });
  return root ? `${root}\\${name}` : name;
}

const outFile = ref(defaultOut());
let outFileTouched = false;
// 归档默认文件名随语言重生成（用户手改过后不再覆盖）
watch(lang, () => {
  if (!outFileTouched) outFile.value = defaultOut();
});
watch(
  () => props.path,
  () => {
    if (!outFileTouched) outFile.value = defaultOut();
  }
);
function onOutFileInput() {
  outFileTouched = true;
}

// ---------- toast ----------
const dupToast = ref("");
let toastTimer: ReturnType<typeof setTimeout> | undefined;

function showToast(msg: string) {
  dupToast.value = msg;
  scheduleToastHide();
}

function scheduleToastHide() {
  clearTimeout(toastTimer);
  toastTimer = setTimeout(() => {
    dupToast.value = "";
  }, 3000);
}

onBeforeUnmount(() => clearTimeout(toastTimer));
</script>

<template>
  <div class="ws">
    <!-- 目标条 -->
    <div class="ws-target">
      <span class="ws-kind">{{ kindLabel }}</span>
      <span class="ws-path" :title="path || ''">{{ path || t("manage.pathUnset") }}</span>
      <button class="btn sm" :disabled="dupScanning || !path" @click="runDetect()">
        {{ groups && !staleResult ? t("manage.rescan") : t("manage.startScan") }}
      </button>
    </div>
    <p v-if="isLazer" class="ws-guard">{{ t("manage.guard") }}</p>
    <p v-if="staleResult" class="ws-warn">{{ t("manage.stale") }}</p>
    <p v-if="dupError" class="ws-err">{{ sm(dupError) }}</p>
    <p v-if="dupScanning" class="ws-note">{{ t("manage.scanning") }}</p>

    <!-- 空文件夹（stable 专属，默认收起） -->
    <div v-if="isStable && path" class="ws-card">
      <button class="fold" @click="emptyOpen = !emptyOpen">
        <span class="fold-chev" :class="{ open: emptyOpen }">▸</span>
        {{ t("manage.emptyTitle") }}
        <span v-if="emptyResult" class="fold-n">{{ t("manage.emptyN", { 0: emptyResult.length }) }}</span>
      </button>
      <div v-if="emptyOpen" class="fold-body">
        <div class="row">
          <button class="btn sm" :disabled="emptyScanning" @click="runEmptyScan()">
            {{ emptyScanning ? t("action.scanning") : t("manage.scanRoot") }}
          </button>
          <span class="hint">{{ t("manage.scanRootHint", { 0: path }) }}</span>
        </div>
        <p v-if="emptyError" class="ws-err">{{ sm(emptyError) }}</p>
        <ul v-if="emptyResult" class="path-list">
          <li v-for="p in emptyResult" :key="p" :title="p">{{ p }}</li>
          <li v-if="!emptyResult.length" class="li-empty">{{ t("manage.noEmpty") }}</li>
        </ul>
        <p v-if="emptyResult?.length" class="hint">{{ t("manage.emptyNote") }}</p>
      </div>
    </div>

    <!-- 查重结果 -->
    <div v-if="groups && !groups.length" class="ws-ok-card">{{ t("manage.noDup") }}</div>

    <div v-for="(g, gi) in groups ?? []" :key="gi" class="ws-card">
      <div class="g-head">
        <span class="g-reason" :class="reasonClass(g.reason)">{{ reasonLabel(g.reason) }}</span>
        <span class="g-label" :title="g.label">{{ g.label }}</span>
        <span v-if="g.members[0]?.firstAdded != null" class="g-date">{{ t("manage.oldest", { 0: formatDate(g.members[0].firstAdded) }) }}</span>
        <span class="g-n">{{ t("manage.nItems", { 0: g.members.length }) }}</span>
        <button class="btn xs" @click="selectGroupRedundant(g)">{{ t("manage.selectRedundant") }}</button>
      </div>
      <div
        v-for="(m, mi) in g.members"
        :key="m.location"
        class="m-row"
        :class="{ done: !selectable(m) }"
      >
        <input
          type="checkbox"
          class="m-check"
          :disabled="!selectable(m)"
          :checked="sel.has(m.location)"
          @change="toggleLoc(m.location)"
        />
        <span class="m-name" :title="m.location">
          {{ m.artist }} - {{ m.title }}<span class="m-creator"> [{{ m.creator }}]</span>
        </span>
        <span class="m-meta">{{ t("manage.nDiffs", { 0: m.difficultyCount }) }}</span>
        <span class="m-meta">{{ memberSize(m) }}</span>
        <span v-if="mi === suggestKeepIndex(g)" class="m-keep" :title="t('manage.keepTitle')">{{ t("manage.keep") }}</span>
        <span v-else-if="!selectable(m)" class="m-done">{{ t("manage.done") }}</span>
        <span class="m-loc" :title="m.location">{{ m.location }}</span>
      </div>
    </div>

    <p v-if="!groups && !dupScanning && !dupError" class="ws-note">
      {{ t("manage.scanHint") }}
    </p>

    <!-- 批量工具条 -->
    <div class="ws-bar" :class="{ on: selectedCount > 0 }">
      <span class="bar-n">
        {{ t("manage.selected", { 0: selectedCount }) }}
        <template v-if="selectedCount > 0 && groups">{{ t("manage.crossGroup") }}</template>
      </span>
      <button class="btn xs" :disabled="!groups?.length" @click="selectAllRedundant()">{{ t("manage.selectRedundantAll") }}</button>
      <button class="btn xs" :disabled="!selectedCount" @click="clearSelection()">{{ t("manage.clearSel") }}</button>
      <span class="bar-spacer"></span>
      <button
        class="btn xs danger"
        :disabled="!selectedCount || isLazer || busy"
        :title="isLazer ? t('manage.deleteTitleLazer') : t('manage.deleteTitle')"
        @click="askDelete()"
      >
        {{ t("manage.delete") }}
      </button>
      <button
        class="btn xs"
        :disabled="!selectedCount || isLazer || busy"
        :title="isLazer ? t('manage.moveTitleLazer') : t('manage.moveTitle')"
        @click="askMove()"
      >
        {{ t("manage.move") }}
      </button>
      <input v-model="outFile" class="bar-out" :class="{ dim: !selectedCount }" :placeholder="t('manage.packPlaceholder')" @input="onOutFileInput" />
      <button class="btn xs primary" :disabled="!selectedCount || busy" :title="t('manage.packTitle')" @click="askPack()">
        {{ t("manage.pack") }}
      </button>
    </div>

    <!-- 执行结果 -->
    <div v-if="lastResult" class="ws-result" :class="lastResult.tone">
      <span>{{ sm(lastResult.summary) }}</span>
      <button v-if="lastResult.errors.length" class="link-btn" @click="resultOpen = !resultOpen">
        {{ resultOpen ? t("manage.collapse") : t("manage.viewN", { 0: lastResult.errors.length, 1: sm(lastResult.errorsTitle) }) }}
      </button>
      <ul v-if="resultOpen && lastResult.errors.length" class="err-list">
        <li v-for="(e, i) in lastResult.errors" :key="i">{{ sm(e) }}</li>
      </ul>
    </div>

    <!-- 两步确认弹层 -->
    <div v-if="plan" class="plan-back" @click.self="plan = null">
      <div class="plan-card">
        <h4>
          {{ plan.op === "delete" ? t("manage.confirmDelete") : plan.op === "move" ? t("manage.confirmMove") : t("manage.confirmPack") }}
        </h4>
        <p class="plan-sub">
          {{ t("manage.planTotal", { 0: plan.paths.length }) }}<template v-if="plan.targetDir">{{ t("manage.planTarget", { 0: plan.targetDir }) }}</template>
          <template v-if="plan.outFile">{{ t("manage.planOut", { 0: plan.outFile }) }}</template>
        </p>
        <ul class="plan-list">
          <li v-for="(p, i) in planExpanded ? plan.paths : plan.paths.slice(0, 10)" :key="i" :title="p">{{ p }}</li>
        </ul>
        <button
          v-if="plan.paths.length > 10 && !planExpanded"
          class="link-btn"
          @click="planExpanded = true"
        >
          {{ t("manage.expandAll", { 0: plan.paths.length }) }}
        </button>
        <p v-if="plan.op === 'delete'" class="plan-note">{{ t("manage.noteDelete") }}</p>
        <p v-if="plan.op === 'pack'" class="plan-note">{{ t("manage.notePack") }}</p>
        <p v-if="plan.op === 'move'" class="plan-note">{{ t("manage.noteMove") }}</p>
        <div class="plan-btns">
          <button ref="planCancelButton" class="btn sm" @click="plan = null">{{ t("action.cancel") }}</button>
          <button class="btn sm" :class="plan.op === 'delete' ? 'danger' : 'primary'" @click="execPlan()">
            {{ t("action.execute") }}
          </button>
        </div>
      </div>
    </div>

    <transition name="fade">
      <div v-if="dupToast" class="ws-toast">{{ dupToast }}</div>
    </transition>
  </div>
</template>

<style scoped>
.ws {
  position: relative;
  flex: 1;
  min-height: 0;
  overflow: auto;
  display: flex;
  flex-direction: column;
  gap: 10px;
  padding-bottom: 12px;
}

/* ---------- 目标条 ---------- */
.ws-target {
  display: flex;
  align-items: center;
  gap: 10px;
  flex: 0 0 auto;
}

.ws-kind {
  font-size: 11.5px;
  font-weight: 700;
  color: var(--accent-hi);
  background: var(--accent-soft);
  border: 1px solid rgba(255, 95, 162, 0.35);
  padding: 3px 9px;
  border-radius: 20px;
  white-space: nowrap;
}

.ws-path {
  flex: 1;
  min-width: 0;
  font-size: 12px;
  color: var(--tx1);
  background: var(--bg2);
  border: 1px solid var(--line);
  border-radius: 7px;
  padding: 6px 10px;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.ws-guard {
  margin: 0;
  font-size: 12px;
  color: var(--warn);
  flex: 0 0 auto;
}

.ws-warn {
  margin: 0;
  font-size: 12px;
  color: var(--warn);
  flex: 0 0 auto;
}

.ws-err {
  margin: 0;
  font-size: 12px;
  color: var(--err);
  word-break: break-all;
  flex: 0 0 auto;
}

.ws-note {
  margin: 0;
  font-size: 12.5px;
  color: var(--tx2);
  flex: 0 0 auto;
}

/* ---------- 按钮 ---------- */
.btn {
  border: 1px solid var(--line);
  background: var(--bg2);
  color: var(--tx0);
  font: inherit;
  padding: 6px 13px;
  border-radius: 7px;
  cursor: pointer;
  white-space: nowrap;
  transition: border-color 0.15s, background 0.15s, color 0.15s;
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

.btn.danger.primary {
  background: var(--err);
  border-color: var(--err);
  color: #1a0d0d;
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
  align-self: flex-start;
}

.link-btn:hover {
  color: var(--tx0);
}

/* ---------- 卡片 ---------- */
.ws-card {
  background: var(--bg1);
  border: 1px solid var(--line);
  border-radius: 10px;
  padding: 10px 12px;
  flex: 0 0 auto;
}

.ws-ok-card {
  background: var(--bg1);
  border: 1px solid var(--line);
  border-radius: 10px;
  padding: 14px;
  font-size: 13px;
  color: var(--ok);
  flex: 0 0 auto;
}

.fold {
  display: flex;
  align-items: center;
  gap: 8px;
  width: 100%;
  border: none;
  background: transparent;
  color: var(--tx0);
  font: inherit;
  font-size: 13px;
  font-weight: 600;
  cursor: pointer;
  padding: 2px;
  text-align: left;
}

.fold-chev {
  display: inline-block;
  transition: transform 0.15s ease;
  color: var(--tx2);
  font-size: 11px;
}

.fold-chev.open {
  transform: rotate(90deg);
}

.fold-n {
  font-size: 11.5px;
  color: var(--tx2);
  font-weight: 400;
}

.fold-body {
  display: flex;
  flex-direction: column;
  gap: 8px;
  margin-top: 10px;
}

.row {
  display: flex;
  align-items: center;
  gap: 10px;
  flex-wrap: wrap;
}

.hint {
  font-size: 11.5px;
  color: var(--tx2);
  margin: 0;
  word-break: break-all;
}

/* ---------- 组成员行 ---------- */
.g-head {
  display: flex;
  align-items: center;
  gap: 8px;
  flex-wrap: wrap;
  padding-bottom: 8px;
  border-bottom: 1px solid var(--line-soft);
}

.g-reason {
  font-size: 11px;
  line-height: 1;
  padding: 4px 8px;
  border-radius: 20px;
  font-weight: 700;
}

.r-id {
  color: var(--c-mania);
  background: rgba(139, 149, 255, 0.14);
}

.r-md5 {
  color: var(--c-catch);
  background: rgba(95, 201, 138, 0.12);
}

.r-same {
  color: var(--warn);
  background: rgba(232, 180, 90, 0.12);
}

.g-label {
  font-size: 13px;
  font-weight: 600;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  max-width: 420px;
}

.g-date,
.g-n {
  font-size: 11.5px;
  color: var(--tx2);
}

.g-n {
  margin-left: auto;
}

.m-row {
  display: flex;
  align-items: center;
  gap: 10px;
  padding: 6px 2px;
  border-bottom: 1px solid var(--line-soft);
  font-size: 12.5px;
  min-width: 0;
}

.m-row:last-child {
  border-bottom: none;
}

.m-row.done {
  opacity: 0.45;
}

.m-row.done .m-name {
  text-decoration: line-through;
}

.m-check {
  accent-color: var(--accent);
  cursor: pointer;
  flex: 0 0 auto;
  margin: 0;
}

.m-check:disabled {
  cursor: default;
}

.m-name {
  flex: 1 1 auto;
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  font-weight: 600;
}

.m-creator {
  color: var(--tx1);
  font-weight: 400;
}

.m-meta {
  flex: 0 0 auto;
  color: var(--tx1);
  font-size: 11.5px;
  font-variant-numeric: tabular-nums;
}

.m-keep {
  flex: 0 0 auto;
  font-size: 11.5px;
  color: var(--ok);
  white-space: nowrap;
}

.m-done {
  flex: 0 0 auto;
  font-size: 11.5px;
  color: var(--tx2);
}

.m-loc {
  flex: 0 1 30%;
  min-width: 0;
  color: var(--tx2);
  font-size: 11px;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  direction: rtl;
  text-align: left;
}

/* ---------- 批量条 ---------- */
.ws-bar {
  position: sticky;
  bottom: 0;
  display: flex;
  align-items: center;
  gap: 8px;
  flex-wrap: wrap;
  background: var(--bg1);
  border: 1px solid var(--line);
  border-radius: 10px;
  padding: 8px 12px;
  flex: 0 0 auto;
  opacity: 0.65;
  transition: opacity 0.15s, border-color 0.15s;
}

.ws-bar.on {
  opacity: 1;
}

.ws-bar.on:has(.btn.danger:not(:disabled)) {
  border-color: rgba(255, 95, 162, 0.4);
}

.bar-n {
  font-size: 12px;
  color: var(--tx1);
  font-variant-numeric: tabular-nums;
  white-space: nowrap;
}

.bar-spacer {
  flex: 1;
}

.bar-out {
  width: 300px;
  max-width: 40vw;
  background: var(--bg2);
  border: 1px solid var(--line);
  border-radius: 6px;
  color: var(--tx0);
  font: inherit;
  font-size: 12px;
  padding: 5px 8px;
}

.bar-out:focus {
  outline: none;
  border-color: var(--accent);
}

.bar-out.dim {
  opacity: 0.6;
}

/* ---------- 结果 ---------- */
.ws-result {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  gap: 10px;
  padding: 8px 12px;
  background: var(--bg1);
  border: 1px solid var(--line);
  border-radius: 10px;
  font-size: 12.5px;
  color: var(--ok);
  flex: 0 0 auto;
}

.ws-result.warn {
  color: var(--warn);
}

.err-list {
  flex: 1 1 100%;
  margin: 2px 0 0;
  padding-left: 18px;
  max-height: 160px;
  overflow: auto;
  font-size: 11.5px;
  color: var(--err);
  word-break: break-all;
}

.path-list {
  margin: 0;
  padding-left: 18px;
  max-height: 200px;
  overflow: auto;
  font-size: 12px;
  color: var(--tx1);
  word-break: break-all;
}

.li-empty {
  color: var(--tx2);
}

/* ---------- 确认弹层 ---------- */
.plan-back {
  position: absolute;
  inset: 0;
  background: rgba(6, 6, 10, 0.62);
  display: flex;
  align-items: center;
  justify-content: center;
  z-index: 40;
  padding: 20px;
}

.plan-card {
  width: 560px;
  max-width: 100%;
  max-height: 82%;
  overflow: auto;
  background: var(--bg1);
  border: 1px solid var(--line);
  border-radius: 12px;
  box-shadow: 0 18px 48px rgba(0, 0, 0, 0.55);
  padding: 16px 18px;
  display: flex;
  flex-direction: column;
  gap: 10px;
}

.plan-card h4 {
  margin: 0;
  font-size: 14px;
}

.plan-sub {
  margin: 0;
  font-size: 12.5px;
  color: var(--tx1);
  word-break: break-all;
}

.plan-list {
  margin: 0;
  padding: 8px 10px 8px 26px;
  background: var(--bg0);
  border: 1px solid var(--line-soft);
  border-radius: 8px;
  font-size: 11.5px;
  color: var(--tx1);
  max-height: 240px;
  overflow: auto;
  word-break: break-all;
}

.plan-note {
  margin: 0;
  font-size: 11.5px;
  color: var(--warn);
}

.plan-btns {
  display: flex;
  justify-content: flex-end;
  gap: 8px;
}

/* ---------- toast ---------- */
.ws-toast {
  position: sticky;
  bottom: 12px;
  align-self: center;
  background: var(--bg3);
  border: 1px solid var(--line);
  color: var(--tx0);
  font-size: 12.5px;
  padding: 8px 16px;
  border-radius: 20px;
  box-shadow: 0 8px 24px rgba(0, 0, 0, 0.45);
  z-index: 50;
}

.fade-enter-active,
.fade-leave-active {
  transition: opacity 0.25s ease;
}

.fade-enter-from,
.fade-leave-to {
  opacity: 0;
}
</style>
