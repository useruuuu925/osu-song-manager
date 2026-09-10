import { invoke } from "@tauri-apps/api/core";
import type {
  AppConfig,
  BeatmapSetInfo,
  ConvertReport,
  DownloadResult,
  DuplicateGroup,
  ExportCsvResult,
  ExportReport,
  ExportSetItem,
  LibraryCandidate,
  MoveReport,
  OauthConfigView,
  OnlineSetMeta,
  OnlineStatusInfo,
  PackReport,
  SourceKind,
  ThumbRequest,
  ThumbResult,
  TrashReport,
} from "./types";

export async function detectLibraries(): Promise<LibraryCandidate[]> {
  return invoke("detect_libraries");
}

export async function validateLibrary(kind: SourceKind, path: string): Promise<void> {
  return invoke("validate_library", { kind, path });
}

export async function getConfig(): Promise<AppConfig> {
  return invoke("get_config");
}

export async function setConfig(config: AppConfig): Promise<void> {
  return invoke("set_config", { config });
}

export async function scanLibrary(kind: SourceKind, path: string): Promise<BeatmapSetInfo[]> {
  return invoke("scan_library", { kind, path });
}

// ---------- M4c：在线元数据 / osu! 登录 ----------

/** 网络+缓存混合抓取；已新鲜缓存会被后端跳过，只回传结果 */
export async function onlineFetch(setIds: number[]): Promise<OnlineSetMeta[]> {
  return invoke("online_fetch", { setIds });
}

/** 纯缓存同步读取（无网络，瞬间返回） */
export async function onlineCache(setIds: number[]): Promise<OnlineSetMeta[]> {
  return invoke("online_cache", { setIds });
}

export async function onlineStatus(): Promise<OnlineStatusInfo> {
  return invoke("online_status");
}

export async function setOauthCredentials(clientId: string, clientSecret: string): Promise<void> {
  return invoke("set_oauth_credentials", { clientId, clientSecret });
}

/** 返回授权 URL（调用方负责打开浏览器） */
export async function osuLoginBegin(): Promise<string> {
  return invoke("osu_login_begin");
}

/** "idle" | "waiting" | "success" | "failed:<msg>" */
export async function osuLoginStatus(): Promise<string> {
  return invoke("osu_login_status");
}

export async function osuLoginManual(code: string): Promise<void> {
  return invoke("osu_login_manual", { code });
}

export async function osuLogout(): Promise<void> {
  return invoke("osu_logout");
}

/** secret 永不回传，只有 hasSecret 标记 */
export async function getOauthConfig(): Promise<OauthConfigView> {
  return invoke("get_oauth_config");
}

// ---------- M5：缩略图 / 批量导出 ----------

/** 单批 ≤500，超出后端直接报错；结果顺序与请求一致 */
export async function prepareThumbnails(requests: ThumbRequest[]): Promise<ThumbResult[]> {
  return invoke("prepare_thumbnails", { requests });
}

/** 从 osu! 官方 CDN 拉取缺失背景并入缓存；null = 下载/解码失败 */
export async function fetchOnlineBackground(setId: number, size: number): Promise<ThumbResult | null> {
  return invoke("fetch_online_background", { setId, size });
}

/** 系统文件夹选择框；null = 用户取消 */
export async function pickExportFolder(): Promise<string | null> {
  return invoke("pick_export_folder");
}

export async function exportBackgrounds(sets: ExportSetItem[], targetDir: string): Promise<ExportReport> {
  return invoke("export_backgrounds", { sets, targetDir });
}

// ---------- M6：库管理（查重 / 回收站 / 移动 / 打包 / 空文件夹） ----------

/** 对 kind+path 全量扫描并分组（lazer 也可用，只读） */
export async function detectDuplicates(kind: SourceKind, path: string): Promise<DuplicateGroup[]> {
  return invoke("detect_duplicates", { kind, path });
}

/** 仅 stable 集文件夹 / *.osz；lazer 内路径后端直接拒绝 */
export async function deleteToTrash(paths: string[]): Promise<TrashReport> {
  return invoke("delete_to_trash", { paths });
}

/** 复制成功后才把原件送回收站；目标冲突自动加后缀；moved 为最终目标路径 */
export async function moveSets(paths: string[], targetDir: string): Promise<MoveReport> {
  return invoke("move_sets", { paths, targetDir });
}

export async function packArchives(paths: string[], outFile: string): Promise<PackReport> {
  return invoke("pack_archives", { paths, outFile });
}

/** 列出 root 下一层不含任何 .osu 的子文件夹；lazer 根会被后端拒绝 */
export async function scanEmptyFolders(root: string): Promise<string[]> {
  return invoke("scan_empty_folders", { root });
}

// ---------- M7：镜像链下载 ----------

/** ≤200/批（前端已去重，后端还会再 dedupe）；并发 3；实时进度走 download-progress 事件 */
export async function downloadOsz(
  setIds: number[],
  targetDir: string,
  noVideo: boolean
): Promise<DownloadResult[]> {
  return invoke("download_osz", { setIds, targetDir, noVideo });
}

/** 请求取消当前批次：进行中的任务中断、剩余队列流转为 cancelled */
export async function cancelDownloads(): Promise<void> {
  return invoke("cancel_downloads");
}

// ---------- M8：曲库 CSV 导出 ----------

/** 对当前 kind+path 导出曲库 CSV（集级+难度级行）；targetDir 由用户选择 */
export async function exportLibraryCsv(
  kind: SourceKind,
  path: string,
  targetDir: string
): Promise<ExportCsvResult> {
  return invoke("export_library_csv", { kind, path, targetDir });
}

// ---------- T2：lazer→stable 转换 ----------

/** 从 lazer 只读复制整集到目标（Songs 文件夹 或 .osz）；逐集容错；进度走 convert-progress */
export async function convertLazerToStable(
  setIds: number[],
  mode: "songs" | "osz",
  targetDir: string
): Promise<ConvertReport> {
  return invoke("convert_lazer_to_stable", { setIds, mode, targetDir });
}

/** 请求取消当前转换批次 */
export async function cancelConvert(): Promise<void> {
  return invoke("cancel_convert");
}
