// bgExport.ts — 单集背景图导出共享 store（详情面板用；画廊导出流程独立不受影响）。
// export-progress 事件监听只注册一次；导出在后台持续，面板关闭不中断。

import { ref } from "vue";
import { exportBackgrounds } from "./api";
import type { ExportReport, ExportSetItem } from "./types";

const DIR_KEY = "osu-mgr.export-dir";

function readStoredDir(): string {
  try {
    return localStorage.getItem(DIR_KEY) ?? "";
  } catch {
    return "";
  }
}

export const bgExportDir = ref<string>(readStoredDir());
export const bgExportRunning = ref(false);
export const bgExportReport = ref<ExportReport | null>(null);

export function setBgExportDir(dir: string) {
  bgExportDir.value = dir;
  try {
    localStorage.setItem(DIR_KEY, dir);
  } catch {
    // 存储失败不影响本次会话
  }
}

/** 导出单集背景图；true = 成功（至少导出 1 张） */
export async function exportOneBg(item: ExportSetItem): Promise<boolean> {
  if (bgExportRunning.value || !bgExportDir.value) return false;
  bgExportRunning.value = true;
  bgExportReport.value = null;
  try {
    const rep = await exportBackgrounds([item], bgExportDir.value);
    bgExportReport.value = rep;
    return rep.exported > 0;
  } catch (e) {
    bgExportReport.value = {
      exported: 0,
      renamed: 0,
      failed: 1,
      errors: [e instanceof Error ? e.message : String(e)],
    };
    return false;
  } finally {
    bgExportRunning.value = false;
  }
}
