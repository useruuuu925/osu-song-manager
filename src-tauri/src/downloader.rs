// downloader.rs
//
// Purpose: M7 镜像链批量 .osz 下载器。
//
// 镜像链（2026-09-03 实测验证的顺序与方言；按序尝试、失败换源）：
//   1. hinai     https://mirror.hinamizawa.ai/api/v1/hinai/d/{id}      必须带描述性 UA；
//                noVideo 用 ?noVideo=1（catboy 的 id+n 后缀方言弃用，实测 ?noVideo=1 可用）
//   2. osu.direct https://osu.direct/api/d/{id}    Content-Disposition 带真实文件名
//   3. nerinyan  https://api.nerinyan.moe/d/{id}    302 → dl.nerinyan.moe，reqwest 默认自动跟随
//   4. sayobot   https://txy1.sayobot.cn/beatmaps/download/{full|novideo}/{id}  不稳定；连接 10s 硬超时
//
// 无任何源支持 Range 续传（实测）→ 重试即整文件重下，换源前删除 .part。
// 仅接受 2xx；首 2 字节必须为 PK(0x50 0x4B)，否则视为伪装错误页 → 该源判失败换源。
// 429/503：按 Retry-After（下限 60s）挂起该 lane（全局暂停表），后续任务跳过冷却中的源；
// 若所有 lane 均处于冷却 → 任务报「所有镜像暂时冷却，稍后重试」。
// 完成 → 打开 zip 校验存在 *.osu 条目 → 原子改名 `<id> <标题>.osz`（标题来自可解析的
// 明文 Content-Disposition；冲突加 (2)(3)…）。
//
// 超时说明（与简报的偏差，如实记录）：reqwest 0.13 的 blocking 构建器没有 read_timeout，
// 无法实现逐块 30s 空闲超时；以 connect_timeout 10s + 整请求 300s 超时替代，
// 卡死的读会在 300s 处中断并走换源逻辑。并发度 3（作用域线程轮转分片，零新增依赖）。

use crate::manage;
use crate::model::{DownloadProgress, DownloadResult, DownloadState};
use std::collections::HashMap;
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

pub const HINAI_UA: &str = "osu-song-manager/0.1 (desktop beatmap manager)";
pub const MAX_IDS_PER_CALL: usize = 200;
pub const PROGRESS_INTERVAL: Duration = Duration::from_millis(150);
pub const LANE_PAUSE_FLOOR: Duration = Duration::from_secs(60);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Mirror {
    pub id: &'static str,
    pub label: &'static str,
}

pub const CHAIN: [Mirror; 4] = [
    Mirror {
        id: "hinai",
        label: "hinai",
    },
    Mirror {
        id: "osudirect",
        label: "osu.direct",
    },
    Mirror {
        id: "nerinyan",
        label: "nerinyan",
    },
    Mirror {
        id: "sayobot",
        label: "sayobot",
    },
];

/// 纯函数：按镜像与 noVideo 方言生成下载 URL（契约顺序与形式）。
pub fn build_url(m: Mirror, id: u64, no_video: bool) -> String {
    let novideo_qs = if no_video { "?noVideo=1" } else { "" };
    match m.id {
        "hinai" => format!("https://mirror.hinamizawa.ai/api/v1/hinai/d/{id}{novideo_qs}"),
        "osudirect" => format!("https://osu.direct/api/d/{id}{novideo_qs}"),
        "nerinyan" => format!("https://api.nerinyan.moe/d/{id}{novideo_qs}"),
        // sayobot 用路径段区分全量/无视频
        _ => format!(
            "https://txy1.sayobot.cn/beatmaps/download/{}/{}",
            if no_video { "novideo" } else { "full" },
            id
        ),
    }
}

/// 首 2 字节 PK 魔数嗅探。
pub fn is_zip_magic(first: &[u8]) -> bool {
    first.len() >= 2 && first[0] == 0x50 && first[1] == 0x4B
}

/// 解析 Content-Disposition 的明文文件名并去掉 .osz 后缀。
/// RFC2047 编码（=?UTF-8?…?=）一律不解析（契约要求）。
pub fn parse_content_disposition(header: &str) -> Option<String> {
    if header.contains("=?") {
        return None;
    }
    let idx = header.find("filename=")?;
    let rest = header[idx + "filename=".len()..].trim();
    let raw = if let Some(after_quote) = rest.strip_prefix('"') {
        after_quote.split('"').next().unwrap_or("")
    } else {
        // 裸值：到分号/空白截止
        rest.split([';', ' ']).next().unwrap_or("")
    };
    if raw.is_empty() {
        return None;
    }
    let stem = match raw.rfind('.') {
        Some(dot) if raw[dot..].eq_ignore_ascii_case(".osz") => &raw[..dot],
        _ => raw,
    };
    let stem = stem.trim();
    if stem.is_empty() {
        None
    } else {
        Some(stem.to_string())
    }
}

// ── lane 冷却表 ───────────────────────────────────────────────────────────────

// lane 冷却表（懒初始化，std 内实现，无新增依赖）。
fn pauses() -> &'static Mutex<HashMap<&'static str, Instant>> {
    static PAUSES: OnceLock<Mutex<HashMap<&'static str, Instant>>> = OnceLock::new();
    PAUSES.get_or_init(|| Mutex::new(HashMap::new()))
}

fn lane_paused_at(map: &HashMap<&'static str, Instant>, id: &'static str, now: Instant) -> bool {
    map.get(id).is_some_and(|until| *until > now)
}

pub fn lane_paused(id: &'static str) -> bool {
    pauses()
        .lock()
        .map(|m| lane_paused_at(&m, id, Instant::now()))
        .unwrap_or(false)
}

fn pause_lane_at(
    map: &mut HashMap<&'static str, Instant>,
    id: &'static str,
    secs: u64,
    now: Instant,
) {
    let d = Duration::from_secs(secs).max(LANE_PAUSE_FLOOR);
    map.insert(id, now + d);
}

pub fn pause_lane(id: &'static str, secs: u64) {
    if let Ok(mut m) = pauses().lock() {
        pause_lane_at(&mut m, id, secs, Instant::now());
    }
}

/// Retry-After 头：整数秒，下限 60s；缺失按 60s。
fn retry_after_secs(hv: Option<&str>) -> u64 {
    hv.and_then(|v| v.trim().parse::<u64>().ok())
        .unwrap_or(60)
        .max(60)
}

// ── 取消 ──────────────────────────────────────────────────────────────────────

pub struct CancelFlag {
    running: AtomicBool,
    cancel: AtomicBool,
}

impl CancelFlag {
    const fn new() -> Self {
        CancelFlag {
            running: AtomicBool::new(false),
            cancel: AtomicBool::new(false),
        }
    }
}

/// 一次运行的 RAII 句柄：begin 时重置取消位；Drop 时结束运行态。
pub struct RunHandle;

static GLOBAL: CancelFlag = CancelFlag::new();

impl RunHandle {
    /// 无运行中的任务时开始；返回 None 表示已有任务（并发 download_osz 罕见，直接串行等待语义过于复杂，选择报错）。
    fn begin() -> Option<RunHandle> {
        // cancel 重置必须在 CAS 之前（同 convert.rs，防丢取消窗口）
        GLOBAL.cancel.store(false, Ordering::SeqCst);
        if GLOBAL
            .running
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            return None;
        }
        Some(RunHandle)
    }

    fn is_cancelled() -> bool {
        GLOBAL.cancel.load(Ordering::SeqCst)
    }

    /// 请求取消当前运行；未在运行时为 no-op。
    pub fn request() {
        if GLOBAL.running.load(Ordering::SeqCst) {
            GLOBAL.cancel.store(true, Ordering::SeqCst);
        }
    }
}

impl Drop for RunHandle {
    fn drop(&mut self) {
        GLOBAL.running.store(false, Ordering::SeqCst);
    }
}

// ── 下载核心（阻塞函数；在 spawn_blocking + 作用域线程中运行）──────────────────

fn http_error(err: &str, mirror: &str) -> String {
    format!("{mirror}: {err}")
}

/// 单个源的一次尝试：流式写入 .part（PK 嗅探 + 取消检查 + 节流进度），
/// 成功后 zip 校验。返回 (最终路径候选标题, 实际镜像)。
fn attempt_download(
    client: &reqwest::blocking::Client,
    m: Mirror,
    id: u64,
    no_video: bool,
    part: &Path,
    emit: &dyn Fn(&mut DownloadProgress),
) -> Result<Option<String>, String> {
    let url = build_url(m, id, no_video);
    let mut req = client.get(&url);
    if m.id == "hinai" {
        req = req.header(reqwest::header::USER_AGENT, HINAI_UA);
    }
    let mut resp = req.send().map_err(|e| format!("send: {e}"))?;
    let status = resp.status();
    if status.as_u16() == 404 {
        return Err("HTTP 404".into()); // 换源，不重试、不冷却
    }
    if status.as_u16() == 429 || status.as_u16() == 503 {
        let ra = resp
            .headers()
            .get("retry-after")
            .and_then(|v| v.to_str().ok());
        let secs = retry_after_secs(ra);
        pause_lane(m.id, secs);
        return Err(crate::errcode::ec2(
            crate::errcode::HTTP_COOLING,
            status.as_u16(),
            secs,
        ));
    }
    if !status.is_success() {
        return Err(format!("HTTP {}", status.as_u16()));
    }

    // Content-Disposition 明文标题（可解析才用）
    let title: Option<String> = resp
        .headers()
        .get(reqwest::header::CONTENT_DISPOSITION)
        .and_then(|v| v.to_str().ok())
        .and_then(parse_content_disposition);

    let total = resp.content_length().unwrap_or(0);
    let _ = fs::remove_file(part); // 换源前清 .part（无续传）
    let mut file = fs::File::create(part)
        .map_err(|e| crate::errcode::ec1(crate::errcode::PART_CREATE_FAILED, e))?;

    let mut received = 0u64;
    // PK 魔数嗅探只需前 2 字节：必须截断，否则整个文件会随 chunks 无谓驻留内存
    // （3 并发 lane 最坏数百 MB RSS）。
    let mut magic: Vec<u8> = Vec::with_capacity(2);
    let mut last_emit = Instant::now() - PROGRESS_INTERVAL;
    let mut buf = vec![0u8; 128 * 1024];
    loop {
        if RunHandle::is_cancelled() {
            drop(file);
            let _ = fs::remove_file(part);
            return Err("__cancelled__".into());
        }
        match resp.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                if magic.len() < 2 {
                    let need = 2 - magic.len();
                    magic.extend_from_slice(&buf[..need.min(n)]);
                }
                if magic.len() >= 2 && !is_zip_magic(&magic[..2]) {
                    drop(file);
                    let _ = fs::remove_file(part);
                    return Err(crate::errcode::ec(crate::errcode::NOT_ZIP_RESPONSE));
                }
                file.write_all(&buf[..n])
                    .map_err(|e| crate::errcode::ec1(crate::errcode::WRITE_FAILED, e))?;
                received += n as u64;
                if last_emit.elapsed() >= PROGRESS_INTERVAL || received == total {
                    let mut p = base_progress(id);
                    p.state = DownloadState::Downloading;
                    p.mirror = Some(m.label.to_string());
                    p.received = received;
                    p.total = total;
                    emit(&mut p);
                    last_emit = Instant::now();
                }
            }
            Err(e) => {
                drop(file);
                let _ = fs::remove_file(part);
                return Err(http_error(
                    &crate::errcode::ec1(crate::errcode::READ_INTERRUPTED, e),
                    m.label,
                ));
            }
        }
    }
    file.flush().ok();
    drop(file);

    // 校验：zip 可开且含 ≥1 个 .osu 条目
    let mut p = base_progress(id);
    p.state = DownloadState::Validating;
    p.mirror = Some(m.label.to_string());
    p.received = received;
    p.total = total;
    emit(&mut p);
    validate_osz(part).map_err(|e| http_error(&e, m.label))?;
    Ok(title)
}

fn validate_osz(path: &Path) -> Result<(), String> {
    let f = fs::File::open(path).map_err(|e| e.to_string())?;
    let za = zip::ZipArchive::new(f)
        .map_err(|e| crate::errcode::ec1(crate::errcode::ZIP_OPEN_FAILED, e))?;
    let has_osu = za.file_names().any(|n| n.to_lowercase().ends_with(".osu"));
    if !has_osu {
        return Err(crate::errcode::ec(crate::errcode::ZIP_NO_OSU));
    }
    Ok(())
}

fn base_progress(set_id: u64) -> DownloadProgress {
    DownloadProgress {
        set_id,
        state: DownloadState::Queued,
        mirror: None,
        received: 0,
        total: 0,
        error: None,
    }
}

fn cancelled_result(id: u64) -> DownloadResult {
    DownloadResult {
        set_id: id,
        ok: false,
        path: None,
        mirror: None,
        bytes: 0,
        error: Some(crate::errcode::ec(crate::errcode::CANCELLED)),
    }
}

fn cancelled_progress(id: u64) -> DownloadProgress {
    let mut p = base_progress(id);
    p.state = DownloadState::Cancelled;
    p.error = Some(crate::errcode::ec(crate::errcode::CANCELLED));
    p
}

fn download_one(
    client: &reqwest::blocking::Client,
    target_dir: &Path,
    id: u64,
    no_video: bool,
    emit: &(dyn Fn(&mut DownloadProgress) + Send + Sync),
) -> DownloadResult {
    let part = target_dir.join(format!(".{id}.part"));
    let mut errors: Vec<String> = Vec::new();
    let mut any_ran = false;
    for m in CHAIN {
        if RunHandle::is_cancelled() {
            let _ = fs::remove_file(&part);
            let mut p = cancelled_progress(id);
            emit(&mut p);
            return cancelled_result(id);
        }
        if lane_paused(m.id) {
            continue; // 冷却中的 lane 直接跳过
        }
        any_ran = true;
        let mut p = base_progress(id);
        p.state = DownloadState::Downloading;
        p.mirror = Some(m.label.to_string());
        emit(&mut p);
        match attempt_download(client, m, id, no_video, &part, &|prog| emit(prog)) {
            Ok(title) => {
                // 落地名 `<id> <标题>.osz`；标题等于 id 时（如 hinai 的裸 `{id}.osz` 响应）不重复拼接
                let final_name = match title {
                    Some(t) if t != id.to_string() => {
                        format!("{} {}", id, crate::thumbs::sanitize_filename(&t))
                    }
                    _ => id.to_string(),
                } + ".osz";
                let (dst, _n) = manage::unique_name(target_dir, &final_name, true);
                if let Err(e) = fs::rename(&part, &dst) {
                    let _ = fs::remove_file(&part);
                    errors.push(http_error(
                        &crate::errcode::ec1(crate::errcode::RENAME_FAILED, e),
                        m.label,
                    ));
                    continue;
                }
                let bytes = fs::metadata(&dst).map(|m| m.len()).unwrap_or(0);
                let mut p = base_progress(id);
                p.state = DownloadState::Done;
                p.mirror = Some(m.label.to_string());
                p.received = bytes;
                p.total = bytes;
                emit(&mut p);
                return DownloadResult {
                    set_id: id,
                    ok: true,
                    path: Some(dst.to_string_lossy().into_owned()),
                    mirror: Some(m.label.to_string()),
                    bytes,
                    error: None,
                };
            }
            Err(e) => {
                if e == "__cancelled__" {
                    let mut p = cancelled_progress(id);
                    emit(&mut p);
                    return cancelled_result(id);
                }
                errors.push(e);
                let _ = fs::remove_file(&part); // 任何失败 → 清 .part 换下一源
            }
        }
    }
    let err = if !any_ran {
        crate::errcode::ec(crate::errcode::ALL_MIRRORS_COOLING)
    } else {
        let joined = errors.join("; ");
        if joined.chars().count() > 300 {
            format!("{}…", joined.chars().take(300).collect::<String>())
        } else {
            joined
        }
    };
    let mut p = base_progress(id);
    p.state = DownloadState::Failed;
    p.error = Some(err.clone());
    emit(&mut p);
    DownloadResult {
        set_id: id,
        ok: false,
        path: None,
        mirror: None,
        bytes: 0,
        error: Some(err),
    }
}

fn new_client() -> Result<reqwest::blocking::Client, String> {
    reqwest::blocking::Client::builder()
        .connect_timeout(Duration::from_secs(10)) // sayobot 硬超时要求（全局无害）
        .timeout(Duration::from_secs(300)) // blocking 无 read_timeout → 整请求兜底（见模块头偏差说明）
        .redirect(reqwest::redirect::Policy::default()) // nerinyan 302 自动跟随
        .build()
        .map_err(|e| e.to_string())
}

/// 队列执行（并发 3，作用域线程轮转分片；结果按去重后输入顺序返回）。
/// emit 在每次状态变更时被调用（含 queued）。
pub fn run_queue(
    target_dir: &Path,
    ids: &[u64],
    no_video: bool,
    emit: &(dyn Fn(&mut DownloadProgress) + Send + Sync),
) -> Vec<DownloadResult> {
    for id in ids {
        let mut p = base_progress(*id);
        p.state = DownloadState::Queued;
        emit(&mut p);
    }
    // 轮转分片为 3 份，保持各分片内部原始顺序
    const WORKERS: usize = 3;
    let mut shards: Vec<Vec<(usize, u64)>> = vec![Vec::new(); WORKERS];
    for (i, id) in ids.iter().enumerate() {
        shards[i % WORKERS].push((i, *id));
    }
    let mut slots: Vec<Option<DownloadResult>> = vec![None; ids.len()];
    std::thread::scope(|s| {
        let mut handles = Vec::new();
        for shard in shards {
            handles.push(
                std::thread::Builder::new()
                    .spawn_scoped(s, move || {
                        let mut out = Vec::new();
                        let client = match new_client() {
                            Ok(c) => c,
                            Err(e) => {
                                let err =
                                    crate::errcode::ec1(crate::errcode::HTTP_CLIENT_FAILED, e);
                                for (i, id) in shard {
                                    let mut p = base_progress(id);
                                    p.state = DownloadState::Failed;
                                    p.error = Some(err.clone());
                                    emit(&mut p);
                                    out.push((
                                        i,
                                        DownloadResult {
                                            set_id: id,
                                            ok: false,
                                            path: None,
                                            mirror: None,
                                            bytes: 0,
                                            error: Some(err.clone()),
                                        },
                                    ));
                                }
                                return out;
                            }
                        };
                        for (i, id) in shard {
                            // 已取消：剩余任务直接标记 cancelled（不触碰网络）
                            if RunHandle::is_cancelled() {
                                let mut p = cancelled_progress(id);
                                emit(&mut p);
                                out.push((i, cancelled_result(id)));
                                continue;
                            }
                            out.push((i, download_one(&client, target_dir, id, no_video, emit)));
                        }
                        out
                    })
                    .expect("scoped spawn"),
            );
        }
        for h in handles {
            for (i, r) in h.join().expect("worker panicked") {
                slots[i] = Some(r);
            }
        }
    });
    slots
        .into_iter()
        .map(|r| r.expect("worker missed slot"))
        .collect()
}

// ── Tauri commands ────────────────────────────────────────────────────────────

#[tauri::command]
pub async fn download_osz(
    app: tauri::AppHandle,
    set_ids: Vec<u64>,
    target_dir: String,
    no_video: bool,
) -> Result<Vec<DownloadResult>, String> {
    // 去重保序 + 上限
    let mut ids: Vec<u64> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for id in set_ids {
        if seen.insert(id) {
            ids.push(id);
        }
    }
    if ids.len() > MAX_IDS_PER_CALL {
        return Err(crate::errcode::ec2(
            crate::errcode::DOWNLOAD_TOO_MANY,
            MAX_IDS_PER_CALL,
            ids.len(),
        ));
    }
    let target = PathBuf::from(&target_dir);
    let roots = manage::lazer_roots(&app);
    if let Some(msg) = manage::lazer_guard(&target, &roots) {
        return Err(msg);
    }
    if !target.exists() {
        fs::create_dir_all(&target)
            .map_err(|e| crate::errcode::ec1(crate::errcode::CREATE_TARGET_DIR_FAILED, e))?;
    }
    let results = tauri::async_runtime::spawn_blocking(move || {
        let _guard = match RunHandle::begin() {
            Some(g) => g,
            None => {
                return ids
                    .into_iter()
                    .map(|id| DownloadResult {
                        set_id: id,
                        ok: false,
                        path: None,
                        mirror: None,
                        bytes: 0,
                        error: Some(crate::errcode::ec(crate::errcode::DOWNLOAD_BUSY)),
                    })
                    .collect();
            }
        };
        use tauri::Emitter;
        run_queue(&target, &ids, no_video, &|p| {
            let _ = app.emit("download-progress", p.clone());
        })
    })
    .await
    .map_err(|e| crate::errcode::ec1(crate::errcode::DOWNLOAD_INTERRUPTED, e))?;
    Ok(results)
}

#[tauri::command]
pub fn cancel_downloads() {
    RunHandle::request();
}

// ── 测试 ──────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chain_order_and_dialects() {
        let urls_full: Vec<String> = CHAIN.iter().map(|m| build_url(*m, 42, false)).collect();
        assert_eq!(
            urls_full[0],
            "https://mirror.hinamizawa.ai/api/v1/hinai/d/42"
        );
        assert_eq!(urls_full[1], "https://osu.direct/api/d/42");
        assert_eq!(urls_full[2], "https://api.nerinyan.moe/d/42");
        assert_eq!(
            urls_full[3],
            "https://txy1.sayobot.cn/beatmaps/download/full/42"
        );
        let urls_nv: Vec<String> = CHAIN.iter().map(|m| build_url(*m, 42, true)).collect();
        assert!(urls_nv[0].ends_with("/42?noVideo=1")); // hinai 用 query，而非 catboy id+n 方言
        assert!(urls_nv[1].ends_with("/42?noVideo=1"));
        assert!(urls_nv[2].ends_with("/42?noVideo=1"));
        assert!(urls_nv[3].contains("/novideo/42")); // sayobot 用路径段
        assert!(!urls_nv[0].contains("+n"), "catboy 方言不得出现");
    }

    #[test]
    fn disposition_plain_names_only() {
        assert_eq!(
            parse_content_disposition(r#"attachment; filename="MEGALOVANIA [Kyshiro].osz""#)
                .as_deref(),
            Some("MEGALOVANIA [Kyshiro]")
        );
        assert_eq!(
            parse_content_disposition(r#"inline; filename=x.osz"#).as_deref(),
            Some("x")
        );
        // RFC2047 编码一律跳过
        assert!(
            parse_content_disposition("attachment; filename=\"=?UTF-8?B?5a6d?=.osz\"").is_none()
        );
        // 空白/缺失
        assert!(parse_content_disposition("attachment").is_none());
        assert_eq!(
            parse_content_disposition(r#"attachment; filename="a:b/c?.osz""#).as_deref(),
            Some("a:b/c?"), // 仅取值；消毒在下载器落地时进行
        );
        // 消毒与下载器组合：sanitize 后无非法字符
        let raw = parse_content_disposition(r#"attachment; filename="a:b<c>.osz""#).unwrap();
        assert_eq!(crate::thumbs::sanitize_filename(&raw), "a_b_c_");
    }

    #[test]
    fn paused_lane_skipping_and_floor() {
        let mut map: HashMap<&'static str, Instant> = HashMap::new();
        let now = Instant::now();
        pause_lane_at(&mut map, "hinai", 5, now); // 5s → 下限 60s
        assert!(lane_paused_at(&map, "hinai", now));
        assert!(lane_paused_at(&map, "hinai", now + Duration::from_secs(59))); // 仍在冷却
        assert!(!lane_paused_at(
            &map,
            "hinai",
            now + Duration::from_secs(61)
        )); // 60s 后释放
        assert!(!lane_paused_at(&map, "osudirect", now)); // 未挂起的 lane 正常
        pause_lane_at(&mut map, "sayobot", 9999, now);
        assert!(lane_paused_at(
            &map,
            "sayobot",
            now + Duration::from_secs(3600)
        ));
    }

    #[test]
    fn retry_after_floor_and_parse() {
        assert_eq!(retry_after_secs(Some("120")), 120);
        assert_eq!(retry_after_secs(Some("5")), 60); // 下限
        assert_eq!(retry_after_secs(Some("garbage")), 60);
        assert_eq!(retry_after_secs(None), 60);
    }

    #[test]
    fn cancel_flag_semantics() {
        // 串行访问全局单例（其余测试不触发下载，无竞争）
        assert!(!RunHandle::is_cancelled());
        let g = RunHandle::begin().expect("idle begin");
        assert!(!RunHandle::is_cancelled());
        RunHandle::request();
        assert!(RunHandle::is_cancelled());
        drop(g);
        // 运行结束后 request() 是 no-op；新 run 的 begin 重置取消位
        RunHandle::request();
        let g2 = RunHandle::begin().expect("re-begin resets");
        assert!(!RunHandle::is_cancelled());
        drop(g2);
    }

    #[test]
    fn pk_sniff() {
        assert!(is_zip_magic(b"PK\x03\x04"));
        assert!(!is_zip_magic(b"{\""));
        assert!(!is_zip_magic(b"P"));
        assert!(!is_zip_magic(b"pk")); // 区分大小写
    }

    #[test]
    fn collision_numbering_for_final_names() {
        let tmp = std::env::temp_dir().join(format!(
            "osm_m7_col_{}_{:?}",
            std::process::id(),
            Instant::now().elapsed().as_nanos()
        ));
        fs::create_dir_all(&tmp).unwrap();
        let (p0, _) = manage::unique_name(&tmp, "42 Title.osz", true);
        assert_eq!(p0.file_name().unwrap().to_string_lossy(), "42 Title.osz");
        fs::write(&p0, b"PK").unwrap();
        let (p1, _) = manage::unique_name(&tmp, "42 Title.osz", true);
        assert_eq!(
            p1.file_name().unwrap().to_string_lossy(),
            "42 Title (2).osz"
        );
        fs::remove_dir_all(&tmp).ok();
    }

    /// LIVE 链路测试：真实下载 283897（osu.direct 已知小图）+ 320118（hinai 已知 200）
    /// 到临时目录；断言结果、zip 内含 .osu、字节数与文件一致。
    /// 网络不可用时优雅跳过。运行：cargo test -- --ignored --nocapture
    #[test]
    #[ignore = "live mirror-chain download"]
    fn live_download_chain() {
        let start = Instant::now();
        let dir = std::env::temp_dir().join(format!("osm_m7_live_{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let events: Mutex<Vec<DownloadProgress>> = Mutex::new(Vec::new());
        let results = run_queue(&dir, &[283897, 320118], false, &|p| {
            events.lock().unwrap().push(p.clone());
        });
        let ev = events.lock().unwrap();
        let chain_trace: Vec<String> = ev
            .iter()
            .filter(|p| p.state == DownloadState::Downloading)
            .map(|p| format!("{}→{}", p.set_id, p.mirror.clone().unwrap_or_default()))
            .collect();
        println!(
            "elapsed: {:.1}s chain-attempts: {:?}",
            start.elapsed().as_secs_f64(),
            chain_trace
        );
        for p in ev
            .iter()
            .filter(|p| matches!(p.state, DownloadState::Failed | DownloadState::Done))
        {
            println!("  set {} {:?} {:?}", p.set_id, p.state, p.error);
        }
        assert_eq!(results.len(), 2);
        let mut ok_any = 0;
        for r in &results {
            match (r.ok, &r.path) {
                (true, Some(path)) => {
                    ok_any += 1;
                    let size = fs::metadata(path).unwrap().len();
                    assert_eq!(size, r.bytes, "reported bytes == file size");
                    let f = fs::File::open(path).unwrap();
                    let za = zip::ZipArchive::new(f).unwrap();
                    assert!(za.file_names().any(|n| n.to_lowercase().ends_with(".osu")));
                    println!(
                        "OK set {} via {} : {} bytes → {}",
                        r.set_id,
                        r.mirror.clone().unwrap_or_default(),
                        r.bytes,
                        Path::new(path).file_name().unwrap().to_string_lossy()
                    );
                }
                _ => println!("FAIL set {}: {:?}", r.set_id, r.error),
            }
        }
        // 允许单源夜间故障，但链路必须至少成功其一；两个都挂通常是断网 → 跳过
        if ok_any == 0 {
            let transport_only = results.iter().all(|r| {
                r.error
                    .as_deref()
                    .map(|e| {
                        e.contains("send:")
                            || e.contains("err.readInterrupted")
                            || e.contains("err.allMirrorsCooling")
                    })
                    .unwrap_or(true)
            });
            if transport_only {
                println!("[skip] 网络不可用，链路全部传输失败");
                fs::remove_dir_all(&dir).ok();
                return;
            }
        }
        assert_eq!(ok_any, 2, "两个测试集均应成功（含换源兜底）: {results:?}");
        fs::remove_dir_all(&dir).ok();
    }
}
