<script setup lang="ts">
import { onBeforeUnmount, onMounted, ref } from "vue";
import { openUrl } from "@tauri-apps/plugin-opener";
import {
  getOauthConfig,
  osuLoginBegin,
  osuLoginManual,
  osuLoginStatus,
  osuLogout,
  setOauthCredentials,
} from "../api";
import { mk, sm, t, type Slot } from "../i18n";

defineProps<{
  loggedIn: boolean;
}>();

const emit = defineEmits<{
  (e: "change"): void;
}>();

// ---------- 凭据 ----------
const clientId = ref("");
const clientSecret = ref("");
const hasSecret = ref(false);
const credMsg = ref<Slot | null>(null);
const credKind = ref<"info" | "error">("info");

async function loadCreds() {
  try {
    const cfg = await getOauthConfig();
    clientId.value = cfg.clientId;
    hasSecret.value = cfg.hasSecret;
    // 后端每次保存都要求两个字段非空，恢复 clientId 方便只改 secret 的保存
    applyServerStatus(await osuLoginStatus());
  } catch {
    // 非 Tauri 环境（纯浏览器调试）
  }
}

async function saveCreds() {
  credMsg.value = null;
  if (!clientId.value.trim()) {
    credKind.value = "error";
    credMsg.value = mk("auth.errClientId");
    return;
  }
  if (!clientSecret.value.trim()) {
    credKind.value = "error";
    credMsg.value = hasSecret.value ? mk("auth.errSecretAgain") : mk("auth.errSecretEmpty");
    return;
  }
  try {
    await setOauthCredentials(clientId.value, clientSecret.value);
    hasSecret.value = true;
    clientSecret.value = "";
    credKind.value = "info";
    credMsg.value = mk("auth.saved");
    emit("change");
  } catch (e) {
    credKind.value = "error";
    credMsg.value = { err: e };
  }
}

// ---------- 登录状态机 ----------
type LoginPhase = "idle" | "waiting" | "success" | "failed";
const phase = ref<LoginPhase>("idle");
const failMsg = ref<Slot | null>(null);
let pollTimer: ReturnType<typeof setInterval> | null = null;

function stopPoll() {
  if (pollTimer != null) {
    clearInterval(pollTimer);
    pollTimer = null;
  }
}

function applyServerStatus(s: string) {
  if (s === "idle") {
    if (phase.value === "waiting") phase.value = "idle";
    stopPoll();
  } else if (s === "waiting") {
    phase.value = "waiting";
    startPoll();
  } else if (s === "success") {
    phase.value = "success";
    stopPoll();
    emit("change");
  } else if (s.startsWith("failed:")) {
    phase.value = "failed";
    failMsg.value = failSlot(s.slice(7));
    stopPoll();
  }
}

/** 后端 failed: 后缀 → 消息槽（timeout 有专属文案；错误码渲染期经 trError 随语言映射） */
function failSlot(msg: string): Slot {
  return msg === "timeout" ? mk("auth.timeout") : { err: msg };
}

function startPoll() {
  stopPoll();
  pollTimer = setInterval(async () => {
    try {
      applyServerStatus(await osuLoginStatus());
    } catch {
      stopPoll();
    }
  }, 1500);
}

function cancelWait() {
  stopPoll();
  phase.value = "idle";
}

async function beginLogin() {
  failMsg.value = null;
  try {
    const url = await osuLoginBegin();
    phase.value = "waiting";
    startPoll();
    try {
      await openUrl(url);
    } catch {
      credHint.value = mk("auth.browserFail");
      authorizeUrl.value = url;
    }
  } catch (e) {
    phase.value = "failed";
    failMsg.value = { err: e };
  }
}

const credHint = ref<Slot | null>(null);
const authorizeUrl = ref("");

const manualCode = ref("");
const manualMsg = ref<Slot | null>(null);

/** 支持粘贴纯 code 或完整回调 URL */
function extractCode(input: string): string {
  const s = input.trim();
  const m = /[?&]code=([^&#]+)/.exec(s);
  if (m) {
    try {
      return decodeURIComponent(m[1]);
    } catch {
      return m[1];
    }
  }
  return s;
}

async function manualLogin() {
  manualMsg.value = null;
  const code = extractCode(manualCode.value);
  if (!code) {
    manualMsg.value = mk("auth.pasteEmpty");
    return;
  }
  try {
    await osuLoginManual(code);
    manualCode.value = "";
    phase.value = "success";
    failMsg.value = null;
    stopPoll();
    emit("change");
  } catch (e) {
    manualMsg.value = { err: e };
    phase.value = "failed";
    failMsg.value = { err: e };
  }
}

async function logout() {
  try {
    await osuLogout();
    phase.value = "idle";
    failMsg.value = null;
    emit("change");
  } catch (e) {
    failMsg.value = { err: e };
  }
}

onMounted(loadCreds);
onBeforeUnmount(stopPoll);
</script>

<template>
  <div class="auth">
    <h4>{{ t("auth.title") }}</h4>
    <p class="help">
      {{ t("auth.help", { 0: "http://localhost:47821/callback" }) }}
    </p>

    <div class="row">
      <span class="lbl">client_id</span>
      <input v-model="clientId" class="txt" :placeholder="t('auth.phClientId')" />
    </div>
    <div class="row">
      <span class="lbl">client_secret</span>
      <input
        v-model="clientSecret"
        class="txt"
        type="password"
        :placeholder="hasSecret ? t('auth.phSecretSaved') : t('auth.phSecret')"
      />
    </div>
    <div class="row">
      <span class="lbl"></span>
      <button class="btn sm" @click="saveCreds()">{{ t("auth.saveCreds") }}</button>
      <span v-if="credMsg" class="mini" :class="credKind">{{ sm(credMsg) }}</span>
    </div>

    <div class="sep"></div>

    <div class="row">
      <span class="lbl">{{ t("auth.loginState") }}</span>
      <span class="state" :class="{ on: loggedIn }">{{ loggedIn ? t("auth.loggedIn") : t("auth.loggedOut") }}</span>
    </div>
    <div class="row">
      <button v-if="!loggedIn" class="btn sm primary" :disabled="phase === 'waiting'" @click="beginLogin()">
        {{ phase === "waiting" ? t("auth.waiting") : t("auth.login") }}
      </button>
      <button v-if="phase === 'waiting'" class="btn sm" @click="cancelWait()">{{ t("auth.cancelWait") }}</button>
      <button v-if="loggedIn" class="btn sm" @click="logout()">{{ t("auth.logout") }}</button>
      <span v-if="loggedIn && phase === 'success'" class="mini info">{{ t("auth.success") }}</span>
    </div>
    <p v-if="phase === 'waiting'" class="tip">{{ t("auth.waitingTip") }}</p>
    <div v-if="authorizeUrl" class="row">
      <input class="txt" :value="authorizeUrl" readonly />
    </div>
    <p v-if="credHint" class="tip">{{ sm(credHint) }}</p>
    <p v-if="phase === 'failed'" class="mini error">{{ t("auth.failed", { 0: sm(failMsg) }) }}</p>

    <div class="row">
      <span class="lbl">{{ t("auth.pasteLabel") }}</span>
      <input
        v-model="manualCode"
        class="txt"
        :placeholder="t('auth.pastePh')"
        @keyup.enter="manualLogin()"
      />
      <button class="btn sm" @click="manualLogin()">{{ t("auth.submit") }}</button>
    </div>
    <p v-if="manualMsg" class="mini error">{{ sm(manualMsg) }}</p>
  </div>
</template>

<style scoped>
.auth {
  display: flex;
  flex-direction: column;
  gap: 8px;
  font-size: 12.5px;
  color: var(--tx0);
}

.auth h4 {
  margin: 0;
  font-size: 13px;
  color: var(--tx0);
}

.help {
  margin: 0;
  font-size: 11.5px;
  line-height: 1.55;
  color: var(--tx1);
}

.help code {
  color: var(--accent-hi);
  background: var(--bg3);
  padding: 1px 4px;
  border-radius: 4px;
}

.row {
  display: flex;
  align-items: center;
  gap: 8px;
}

.lbl {
  flex: 0 0 76px;
  color: var(--tx2);
  font-size: 11.5px;
}

.txt {
  flex: 1;
  min-width: 0;
  background: var(--bg2);
  border: 1px solid var(--line);
  border-radius: 6px;
  color: var(--tx0);
  font: inherit;
  font-size: 12px;
  padding: 5px 8px;
}

.txt:focus {
  outline: none;
  border-color: var(--accent);
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
}

.btn.sm {
  padding: 5px 11px;
  font-size: 12px;
}

.btn:hover:not(:disabled) {
  border-color: var(--tx2);
  background: var(--bg3);
}

.btn.primary {
  background: var(--accent);
  border-color: var(--accent);
  color: #1a0d14;
  font-weight: 700;
}

.btn:disabled {
  opacity: 0.6;
  cursor: default;
}

.mini {
  font-size: 11.5px;
  color: var(--tx1);
}

.mini.info {
  color: var(--ok);
}

.mini.error {
  color: var(--err);
}

.tip {
  margin: 0;
  font-size: 11.5px;
  color: var(--tx2);
}

.sep {
  border-top: 1px solid var(--line-soft);
  margin: 2px 0;
}

.state {
  color: var(--tx2);
}

.state.on {
  color: var(--ok);
  font-weight: 600;
}
</style>
