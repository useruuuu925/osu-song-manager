// errcode.rs — T1 多语言：后端错误消息改为「稳定错误码 + | 分隔参数」回传。
//
// 约定（与前端 src/i18n/trError 对齐）：
// - Err(String) 的内容形如 `err.<name>` 或 `err.<name>|param0|param1`；
// - `|` 在 Windows 文件名中非法，可安全作分隔符；
// - 参数原样嵌入（IO 错误文本、路径、计数等），由前端按 {0}{1}… 插值翻译；
// - 前端对未知码/纯文本（OS 错误等）原样透出，绝不丢信息。
// zh/en 文案见 src/i18n/zh.ts、en.ts —— 与本文件的常量一一对应（key = 错误码本体）。

/// 组合错误串：code 单独回传
pub fn ec(code: &str) -> String {
    code.to_string()
}

/// 组合错误串：code|p0
pub fn ec1(code: &str, p0: impl std::fmt::Display) -> String {
    format!("{code}|{p0}")
}

/// 组合错误串：code|p0|p1
pub fn ec2(code: &str, p0: impl std::fmt::Display, p1: impl std::fmt::Display) -> String {
    format!("{code}|{p0}|{p1}")
}

/// 组合错误串：code|p0|p1|p2
pub fn ec3(
    code: &str,
    p0: impl std::fmt::Display,
    p1: impl std::fmt::Display,
    p2: impl std::fmt::Display,
) -> String {
    format!("{code}|{p0}|{p1}|{p2}")
}

// ── 通用 / 检测（detect.rs） ─────────────────────────────────────────────────
pub const DIR_NOT_FOUND: &str = "err.dirNotFound";
pub const LAZER_NO_REALM: &str = "err.lazerNoRealm";
pub const STABLE_IS_LAZER: &str = "err.stableIsLazer";
pub const SCAN_INTERRUPTED: &str = "err.scanInterrupted";

// ── 扫描 / 曲库（library.rs / realm_db.rs） ──────────────────────────────────
pub const SONGS_DIR_NOT_FOUND: &str = "err.songsDirNotFound";
pub const READ_DIR_FAILED: &str = "err.readDirFailed";
pub const REALM_NOT_FOUND: &str = "err.realmNotFound";
pub const REALM_READ_FAILED: &str = "err.realmReadFailed";
pub const REALM_MISSING_TABLE: &str = "err.realmMissingTable";

// ── 库管理（manage.rs） ──────────────────────────────────────────────────────
pub const LAZER_READ_ONLY: &str = "err.lazerReadOnly";
pub const BAD_SHAPE: &str = "err.badShape";
pub const COPY_FAILED: &str = "err.copyFailed";
pub const COPIED_TRASH_FAILED: &str = "err.copiedButTrashFailed";
pub const PATH_MISSING: &str = "err.pathMissing";
pub const BAD_PATH_NAME: &str = "err.badPathName";
pub const TARGET_DIR_UNAVAILABLE: &str = "err.targetDirUnavailable";
pub const OUT_PARENT_MISSING: &str = "err.outParentMissing";
pub const PACK_TOO_BIG: &str = "err.packTooBig";
pub const CREATE_ARCHIVE_FAILED: &str = "err.createArchiveFailed";
pub const ARCHIVE_FINISH_FAILED: &str = "err.archiveFinishFailed";
pub const DETECT_INTERRUPTED: &str = "err.detectInterrupted";
pub const DELETE_INTERRUPTED: &str = "err.deleteInterrupted";
pub const MOVE_INTERRUPTED: &str = "err.moveInterrupted";
pub const PACK_INTERRUPTED: &str = "err.packInterrupted";

// ── 缩略图 / 导出（thumbs.rs） ───────────────────────────────────────────────
pub const THUMB_READ_FAILED: &str = "err.thumbReadFailed";
pub const THUMB_DECODE_FAILED: &str = "err.thumbDecodeFailed";
pub const THUMB_JPEG_FAILED: &str = "err.thumbJpegFailed";
pub const EXPORT_SOURCE_MISSING: &str = "err.exportSourceMissing";
pub const THUMB_BATCH_TOO_LARGE: &str = "err.thumbBatchTooLarge";
pub const THUMB_INTERRUPTED: &str = "err.thumbInterrupted";
pub const DIALOG_INTERRUPTED: &str = "err.dialogInterrupted";
pub const EXPORT_INTERRUPTED: &str = "err.exportInterrupted";

// ── 下载（downloader.rs） ────────────────────────────────────────────────────
pub const DOWNLOAD_TOO_MANY: &str = "err.downloadTooMany";
pub const CREATE_TARGET_DIR_FAILED: &str = "err.createTargetDirFailed";
pub const DOWNLOAD_BUSY: &str = "err.downloadBusy";
pub const DOWNLOAD_INTERRUPTED: &str = "err.downloadInterrupted";
pub const HTTP_CLIENT_FAILED: &str = "err.httpClientFailed";
pub const CANCELLED: &str = "err.cancelled";
pub const ALL_MIRRORS_COOLING: &str = "err.allMirrorsCooling";
pub const NOT_ZIP_RESPONSE: &str = "err.notZipResponse";
pub const HTTP_COOLING: &str = "err.httpCooling";
pub const ZIP_OPEN_FAILED: &str = "err.zipOpenFailed";
pub const ZIP_NO_OSU: &str = "err.zipNoOsu";
pub const READ_INTERRUPTED: &str = "err.readInterrupted";
pub const WRITE_FAILED: &str = "err.writeFailed";
pub const PART_CREATE_FAILED: &str = "err.partCreateFailed";
pub const RENAME_FAILED: &str = "err.renameFailed";

// ── 在线 / 登录（online.rs） ─────────────────────────────────────────────────
pub const NETWORK: &str = "err.network";
pub const TOKEN_FAILED: &str = "err.tokenFailed";
pub const TOKEN_PARSE_FAILED: &str = "err.tokenParseFailed";
pub const TOKEN_MISSING_ACCESS: &str = "err.tokenMissingAccess";
pub const TOKEN_MISSING_REFRESH: &str = "err.tokenMissingRefresh";
pub const OAUTH_CREDS_MISSING: &str = "err.oauthCredsMissing";
pub const STATE_LOCK_UNAVAILABLE: &str = "err.stateLockUnavailable";
pub const PORT_BUSY: &str = "err.portBusy";
pub const CODE_EMPTY: &str = "err.codeEmpty";
pub const CLIENT_ID_EMPTY: &str = "err.clientIdEmpty";
pub const CLIENT_SECRET_EMPTY: &str = "err.clientSecretEmpty";
pub const ONLINE_INTERRUPTED: &str = "err.onlineInterrupted";
pub const LOGIN_INTERRUPTED: &str = "err.loginInterrupted";
pub const STATE_MISMATCH: &str = "err.stateMismatch";
pub const CALLBACK_NO_CODE: &str = "err.callbackNoCode";

// ── CSV 导出（export_csv.rs） ────────────────────────────────────────────────
pub const CREATE_CSV_FAILED: &str = "err.createCsvFailed";
pub const WRITE_BOM_FAILED: &str = "err.writeBomFailed";
pub const CSV_INTERRUPTED: &str = "err.csvInterrupted";

// ── lazer→stable 转换（convert.rs） ──────────────────────────────────────────
pub const CONVERT_BUSY: &str = "err.convertBusy";
pub const CONVERT_NO_SETS: &str = "err.convertNoSets";
pub const CONVERT_TOO_MANY: &str = "err.convertTooMany";
pub const CONVERT_NO_LAZER_DIR: &str = "err.convertNoLazerDir";
pub const CONVERT_SCAN_FAILED: &str = "err.convertScanFailed";
pub const CONVERT_INTERRUPTED: &str = "err.convertInterrupted";
pub const CONVERT_LOCAL_UNSUPPORTED: &str = "err.convertLocalUnsupported";
pub const CONVERT_SET_NOT_FOUND: &str = "err.convertSetNotFound";
pub const CONVERT_SOURCE_MISSING: &str = "err.convertSourceMissing";
pub const CONVERT_WRITE_FAILED: &str = "err.convertWriteFailed";
pub const CONVERT_CREATE_DIR_FAILED: &str = "err.convertCreateDirFailed";
pub const CONVERT_ZIP_FAILED: &str = "err.convertZipFailed";
