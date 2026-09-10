// i18n — 自研零依赖双语文案层（T1）。
// 用法：组件内 import { t } from "../i18n"; 模板里 {{ t("key") }}；
// t() 在渲染期读取 lang ref，语言切换自动触发所有用到它的组件重渲染。
// zh 是完整兜底：en 缺 key 回退 zh，再缺回退 key 本身。
// en 侧类型为 Record<keyof typeof zh, string> → vue-tsc 在构建期强制两语言 key 集合全等
// （scripts/check-locales.mjs 再做一次独立校验，双保险）。
// 后端错误码约定："err.xxx|param0|param1"（| 在 Windows 路径中非法，可安全作分隔符），
// trError() 负责切分、找首个已知错误码并翻译，前缀（如镜像标签 "hinai: "）保留原样。
import { ref } from "vue";
import { zh } from "./zh";
import { en } from "./en";

export type Lang = "zh" | "en";
export type TKey = keyof typeof zh;

const LANG_KEY = "osu-mgr.lang";

function detectLang(): Lang {
  try {
    const saved = localStorage.getItem(LANG_KEY);
    if (saved === "zh" || saved === "en") return saved;
  } catch {
    // 存储不可用：落到 navigator 检测
  }
  const nav = typeof navigator !== "undefined" ? navigator.language : "";
  return nav.toLowerCase().startsWith("zh") ? "zh" : "en";
}

/** 当前语言（响应式） */
export const lang = ref<Lang>(detectLang());

/** 参数值：字符串 / 数字 / 嵌套错误槽（渲染期经 trError 随语言映射） */
export type ParamValue = string | number | { err: unknown };
export type Params = Record<string, ParamValue>;

function interpolate(template: string, params?: Params): string {
  if (!params) return template;
  return template.replace(/\{(\w+)\}/g, (m, k) => {
    const v = params[k];
    // {err} 型参数必须经 sm/translateParams 预翻译；直达 t() 视为占位缺失
    if (v == null || typeof v === "object") return m;
    return String(v);
  });
}

/** 取文案；en 缺 key 回退 zh，再缺回退 key 本身（zh 为完整兜底） */
export function t(key: TKey, params?: Params): string {
  const dict = lang.value === "en" ? (en as Record<string, string>) : (zh as Record<string, string>);
  const s = dict[key] ?? zh[key] ?? key;
  return interpolate(s, params);
}

export function hasKey(key: string): key is TKey {
  return Object.prototype.hasOwnProperty.call(zh, key);
}

/**
 * 时点消息槽：解决「消息在事件发生时定稿，语言切换后不回溯」的问题。
 * - { key, params }：渲染期取词（params 里值为词表 key 的字符串也会被翻译，
 *   如后端 detect detail 下发的 dt.* 稳定码）；
 * - { err }：渲染期 trError（错误码随当前语言重新映射）；
 * - string：原样透出（纯文本/无需回溯的内容）。
 */
export type Slot = { key: TKey; params?: Params } | { err: unknown } | string;

/** 构造 key 型消息槽 */
export function mk(key: TKey, params?: Params): Slot {
  return { key, params };
}

function translateParams(params?: Params): Params | undefined {
  if (!params) return undefined;
  const out: Params = {};
  for (const [k, v] of Object.entries(params)) {
    if (v != null && typeof v === "object" && "err" in v) {
      out[k] = trError(v.err);
    } else if (typeof v === "string" && hasKey(v)) {
      out[k] = t(v);
    } else {
      out[k] = v;
    }
  }
  return out;
}

/** 渲染消息槽（模板/computed 内调用；内部读 lang，语言切换自动重渲染） */
export function sm(slot: Slot | null | undefined): string {
  if (slot == null) return "";
  if (typeof slot === "string") return slot;
  if ("err" in slot) return trError(slot.err);
  return t(slot.key, translateParams(slot.params));
}

/** 命中词表则翻译，否则原样透出（用于后端下发的稳定 key，如 detect detail） */


function applyHtmlLang() {
  try {
    document.documentElement.lang = lang.value === "zh" ? "zh-CN" : "en";
  } catch {
    // 非 DOM 环境
  }
}
applyHtmlLang();

/** 切换语言并持久化；同时同步窗口标题与 <html lang> */
export function setLang(l: Lang) {
  lang.value = l;
  try {
    localStorage.setItem(LANG_KEY, l);
  } catch {
    // 存储不可用：本次会话内仍生效
  }
  applyHtmlLang();
  void applyWindowTitle();
}

/**
 * 后端错误串 → 当前语言文案。
 * 在整串里找首个 err.* 形态的错误码（前面允许 "hinai: " 这类前缀），
 * 其后按 | 切参数；找不到已知码则原样返回（OS/IO 纯英文错误直接透出）。
 */
export function trError(e: unknown): string {
  const raw = e instanceof Error ? e.message : String(e);
  const m = /(^|[^A-Za-z0-9_.])(err\.[A-Za-z0-9_.]+)/.exec(raw);
  if (m && hasKey(m[2])) {
    const prefix = raw.slice(0, m.index + m[1].length);
    const rest = raw.slice(m.index + m[1].length + m[2].length);
    const params: Params = {};
    rest
      .split("|")
      .forEach((p, i) => (params[String(i)] = p));
    return prefix + t(m[2], params);
  }
  return raw;
}

/** 窗口标题跟随语言（非 Tauri 环境静默失败） */
export async function applyWindowTitle() {
  try {
    const { getCurrentWindow } = await import("@tauri-apps/api/window");
    await getCurrentWindow().setTitle(t("app.title"));
  } catch {
    // 非 Tauri 环境（纯浏览器调试）
  }
}
