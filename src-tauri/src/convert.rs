// convert.rs — T2 lazer → stable 谱面集转换导出。
//
// 数据流：client.realm 索引出「每集文件清单（原始文件名 → files\<h>\<hh>\<hash> 物理路径）」
// → 逐集复制到用户目标（stable Songs 文件夹 或 .osz zip），.osu 内容做 stable 兼容降级。
//
// .osu 降级规则（2026-09-05 调研结论，证据链）：
// - stable 的格式上限是 v14（osu! wiki .osu file format）；lazer 编辑器/导出改写过的
//   图是 v128 头（LegacyBeatmapEncoder FIRST_LAZER_VERSION=128），直接导入 stable 报
//   「无法解析谱面头部/谱面可能已损坏」（ppy/osu#28607、#33931）。
// - v128→v14 必须项（参考 JPK314/LazerToStable convert.py + 官方 PR #28609）：
//   ① 头行 `osu file format v128` → v14；
//   ② [Events] break 行浮点时间截断为整数（stable 按整数解析，浮点直接崩，#28609）；
//     ②b timing point floor + 同区间物件整体平移（#30607 防 missnap）；
//     ②c 负 beatLength 的 uninherited 点改回 inherited（#37583）；
//     ②d 坐标 round 而非截断（#31305）；
//     ②e slider 多段混合曲线 → 单 B| bezier 锚点（#31713，见 slider_path.rs）；
//     ②f stable 不支持的音频/视频格式（flac/opus/webm 等）逐集警告（见 media_warnings）；
//   ③ [TimingPoints] 首字段时间截断；[HitObjects] 的 x/y/time 截断（slider 追加
//      slides/pixelLength，spinner/hold 追加 endTime）——v128 允许全精度数值，
//      v14 侧按整数解析（#29340）。
// - slider 多段混合曲线路径（stable 不支持）暂不改写：lazer encoder 对绝大多数
//   滑条输出单段类型；该项留待实测出现显示异常再迭代。
// - hitsounds/.osb/音频/背景文件名引用无需处理（v14 原生支持，文件为导入时原始字节）。
// - 未被编辑器改写过的集（绝大多数，含全部从 stable/官网导入的图）本来就是 v14
//   原始字节，降级路径不触发，清洗为幂等无操作。
//
// 铁律：只读 lazer（仅 std::fs::read），写入只发生在用户选择的目标目录；
// 目标位于 lazer 数据目录内时整体拒绝（lazer_guard）。逐集失败不中断；可取消。
//
// 进度事件 convert-progress {done,total,setId,state,current,error}（对象形，与
// export-progress/download-progress 同族）；最终 ConvertReport 逐集回传。

use crate::manage;
use crate::model::{ConvertProgress, ConvertReport, ConvertSetResult, ConvertState};
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};

pub const MAX_SETS_PER_CALL: usize = 2000;

/// 输出形态：stable Songs 目录下的集文件夹，或可直接双击导入的 .osz。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ConvertMode {
    Songs,
    Osz,
}

// ── 取消（与 downloader.RunHandle 同构但独立的开关，避免与下载互相关停） ─────

static CONVERT_RUNNING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static CONVERT_CANCEL: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

struct ConvertRunGuard;

impl ConvertRunGuard {
    fn begin() -> Option<ConvertRunGuard> {
        use std::sync::atomic::Ordering;
        // cancel 重置必须在 CAS 之前：request_cancel 仅在 RUNNING=true 时生效，
        // 放在 CAS 之后会有「用户点取消被新一轮启动覆盖」的丢取消窗口
        CONVERT_CANCEL.store(false, Ordering::SeqCst);
        if CONVERT_RUNNING
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            return None;
        }
        Some(ConvertRunGuard)
    }

    fn cancelled() -> bool {
        use std::sync::atomic::Ordering;
        CONVERT_CANCEL.load(Ordering::SeqCst)
    }
}

impl Drop for ConvertRunGuard {
    fn drop(&mut self) {
        use std::sync::atomic::Ordering;
        CONVERT_RUNNING.store(false, Ordering::SeqCst);
    }
}

/// 请求取消当前转换批次；未在运行时为 no-op。
pub fn request_cancel() {
    use std::sync::atomic::Ordering;
    if CONVERT_RUNNING.load(Ordering::SeqCst) {
        CONVERT_CANCEL.store(true, Ordering::SeqCst);
    }
}

// ── .osu stable 兼容降级（纯函数，幂等） ─────────────────────────────────────

/// 解析 .osu 首行的格式版本号（容忍 BOM）；无版本行或无法解析返回 None。
pub fn osu_format_version(content: &str) -> Option<i64> {
    let first = content.split('\n').next()?;
    let t = first.trim().trim_start_matches('\u{feff}');
    t.strip_prefix("osu file format v")?.parse::<i64>().ok()
}

/// C# `(int)` 语义的截断：把浮点字段变为整数文本（v14 侧按整数解析）。
/// 非/非法浮点或本就是整数文本时返回 None（无需改写）。
fn trunc_field(fields: &[&str], idx: usize) -> Option<String> {
    let raw = fields.get(idx)?.trim();
    let v = raw.parse::<f64>().ok()?;
    if v.is_finite() && (v.fract() != 0.0 || raw.contains('.')) {
        Some((v.trunc() as i64).to_string())
    } else {
        None
    }
}

/// v128→v14 降级的行级改写（仅 >v14 的文件触发）：
/// - TimingPoints：首字段（时间）截断；
/// - HitObjects：x/y/time 恒截断（x/y 上限钳制 131072 防御异常值）；slider 追加
///   slides/pixelLength；spinner/hold 追加 endTime（hold 的 `endTime:hitSample`
///   尾巴只截断冒号前的时间部分，样本组原样保留）。
///
/// 行无需改写时返回 None（调用方原样透传，避免无谓分配）。
/// 时间平移预计算（ppy/osu#30607 语义）：floor 每个 timing point 时间，
/// 其偏移累计平移到同区间的时间点与物件（起始+结束），保持相对节奏间隔。
///
/// 返回 行号 → (新起始时间, 新结束时间可选)。
fn precompute_time_shifts(content: &str) -> std::collections::HashMap<usize, (i64, Option<i64>)> {
    let mut map = std::collections::HashMap::new();
    let mut section = String::new();
    let mut tps: Vec<(usize, f64)> = Vec::new();
    let mut objs: Vec<(usize, f64, Option<f64>)> = Vec::new();
    for (ln, line) in content.split_inclusive('\n').enumerate() {
        let bare = line.trim_end_matches(['\n', '\r']).trim();
        if bare.starts_with('[') && bare.ends_with(']') {
            section = bare[1..bare.len() - 1].to_string();
            continue;
        }
        if section == "TimingPoints" {
            if let Some(t) = bare
                .split(',')
                .next()
                .and_then(|s| s.trim().parse::<f64>().ok())
            {
                tps.push((ln, t));
            }
        } else if section == "HitObjects" {
            let f: Vec<&str> = bare.split(',').collect();
            if f.len() < 3 {
                continue;
            }
            let Ok(t) = f[2].trim().parse::<f64>() else {
                continue;
            };
            let obj_type: u32 = f.get(3).and_then(|s| s.trim().parse().ok()).unwrap_or(0);
            let mut end = None;
            if obj_type & (8 | 128) != 0 {
                if let Some(f5) = f.get(5) {
                    if let Some(et) = f5
                        .split(':')
                        .next()
                        .and_then(|s| s.trim().parse::<f64>().ok())
                    {
                        end = Some(et);
                    }
                }
            }
            objs.push((ln, t, end));
        }
    }
    tps.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
    let mut cum = 0f64;
    let mut deltas: Vec<(f64, f64)> = Vec::new();
    for (ln, t) in &tps {
        let new_t = (*t + cum).trunc();
        let delta = new_t - *t;
        cum += delta;
        map.insert(*ln, (new_t as i64, None));
        deltas.push((*t, cum));
    }
    for (ln, t, end) in &objs {
        let mut cum_at = 0f64;
        for (old, cum_after) in &deltas {
            if *old <= *t {
                cum_at = *cum_after;
            } else {
                break;
            }
        }
        let new_end = end.map(|e| (e + cum_at).trunc() as i64);
        map.insert(*ln, ((*t + cum_at).trunc() as i64, new_end));
    }
    map
}

fn downgrade_line(
    bare: &str,
    section: &str,
    orig: &str,
    ln: usize,
    shifts: &std::collections::HashMap<usize, (i64, Option<i64>)>,
) -> Option<String> {
    let fields: Vec<&str> = bare.split(',').collect();
    if fields.len() < 3 {
        return None;
    }
    let mut fixed: Vec<Option<String>> = vec![None; fields.len()];
    if section == "TimingPoints" {
        // 时间：优先用平移预计算值（ppy/osu#30607 防 missnap），否则截断
        fixed[0] = Some(
            shifts
                .get(&ln)
                .map(|(t, _)| t.to_string())
                .unwrap_or_else(|| {
                    trunc_field(&fields, 0).unwrap_or_else(|| fields[0].trim().to_string())
                }),
        );
        // #37583：lazer 编辑器可产生负 beatLength 的 uninherited 点，stable 直接
        // 无法加载该 diff → 按语义改回 inherited（legacy 格式负 beatLength 即 SV 点）
        if let (Some(bl), Some(flag)) = (fields.get(1), fields.get(6)) {
            if flag.trim() == "1" {
                if let Ok(v) = bl.trim().parse::<f64>() {
                    if v < 0.0 {
                        fixed[6] = Some("0".to_string());
                    }
                }
            }
        }
    } else if section == "HitObjects" {
        // x / y：坐标 round 而非截断（ppy/osu#31305），上限钳制 ±131072
        for (i, slot) in fixed.iter_mut().enumerate().take(2) {
            let raw = fields.get(i)?.trim();
            if let Ok(v) = raw.parse::<f64>() {
                if v.is_finite() && (v.fract() != 0.0 || raw.contains('.') || v.abs() > 131072.0) {
                    *slot = Some((v.round().clamp(-131072.0, 131072.0) as i64).to_string());
                }
            }
        }
        // 时间：平移预计算值（同上），无则截断
        fixed[2] = Some(
            shifts
                .get(&ln)
                .map(|(t, _)| t.to_string())
                .unwrap_or_else(|| {
                    trunc_field(&fields, 2).unwrap_or_else(|| fields[2].trim().to_string())
                }),
        );
        let obj_type: u32 = fields
            .get(3)
            .and_then(|s| s.trim().parse().ok())
            .unwrap_or(0);
        if obj_type & 2 != 0 {
            // slider: ...,curve,slides,pixelLength[,hitSample]
            fixed[6] = trunc_field(&fields, 6);
            fixed[7] = trunc_field(&fields, 7);
            // 多段混合曲线 → 单 B| bezier（stable 只认最后出现的类型，ppy/osu#31713）
            if let Some(curve) = fields.get(5) {
                if let Some(new_curve) = crate::slider_path::downgrade_curve(curve) {
                    fixed[5] = Some(new_curve);
                }
            }
        } else if obj_type & (8 | 128) != 0 {
            // spinner: endTime 为第 5 列；hold: `endTime:hitSample` 同列冒号分隔
            // endTime 同样随本集时间平移
            let shifted_end = shifts.get(&ln).and_then(|(_, e)| *e);
            if let Some(f5) = fields.get(5) {
                let mut parts = f5.splitn(2, ':');
                let end = parts.next().unwrap_or("");
                let tail = parts.next();
                if let Some(ne) = shifted_end {
                    fixed[5] = Some(match tail {
                        Some(tail) if !tail.is_empty() => format!("{ne}:{tail}"),
                        _ => ne.to_string(),
                    });
                } else if let Ok(t) = end.trim().parse::<f64>() {
                    if t.is_finite() && (t.fract() != 0.0 || end.trim().contains('.')) {
                        fixed[5] = Some(match tail {
                            Some(tail) => format!("{}:{}", t.trunc() as i64, tail),
                            None => (t.trunc() as i64).to_string(),
                        });
                    }
                }
            }
        }
    } else {
        return None;
    }
    if fixed.iter().all(Option::is_none) {
        return None;
    }
    let newline = if orig.contains("\r\n") { "\r\n" } else { "\n" };
    let mut out = fields
        .iter()
        .zip(fixed.iter())
        .map(|(f, fx)| fx.as_deref().unwrap_or(f))
        .collect::<Vec<_>>()
        .join(",");
    out.push_str(newline);
    Some(out)
}

/// stable 兼容降级：
/// - 任意版本：[Events] 小节内 break 行 `2,<start>,<end>` 的浮点时间截断为整数
///   （无法解析数字的 break 行整行丢弃，stable 同样解析不了）；
/// - 版本 > v14（lazer 编码产物，如 v128）：头行改写为 v14，并对 [TimingPoints] /
///   [HitObjects] 做整数化降级；
/// - v14 及以下：除 break 修复外逐字节保留（直通快路径）。
///
/// 返回 (降级后内容, 是否有改动)。
pub fn sanitize_osu_for_stable(content: &str) -> (String, bool) {
    let mut changed = false;
    let version = osu_format_version(content);
    let downgrade = matches!(version, Some(v) if v > 14);
    // 时间平移预计算（ppy/osu#30607）：floor 每个 timing point 并把同区间的
    // 物件整体平移同一偏移，保持相对间隔（防 stable 端 snap/SV 错位）
    let shifts: std::collections::HashMap<usize, (i64, Option<i64>)> = if downgrade {
        precompute_time_shifts(content)
    } else {
        std::collections::HashMap::new()
    };
    let mut out = String::with_capacity(content.len() + 64);
    let mut section = String::new();
    for (ln, line) in content.split_inclusive('\n').enumerate() {
        // split_inclusive 保留原始行尾（\n 或 \r\n），未命中降级的行原样输出
        let trimmed = line.trim_end_matches(['\n', '\r']);
        let bare = trimmed.trim();
        // 头行降级：lazer 版本（v128）→ v14（stable 已知上限，保留 BOM）
        if ln == 0 && downgrade {
            changed = true;
            let newline = if line.contains("\r\n") { "\r\n" } else { "\n" };
            if trimmed.starts_with('\u{feff}') {
                out.push('\u{feff}');
            }
            out.push_str("osu file format v14");
            out.push_str(newline);
            continue;
        }
        if bare.starts_with('[') && bare.ends_with(']') {
            section = bare[1..bare.len() - 1].to_string();
            out.push_str(line);
            continue;
        }
        let is_events = section == "Events";
        let is_break = is_events && (bare.starts_with("2,") || bare.starts_with("2, "));
        if !is_break || bare.starts_with("//") {
            // 降级路径：TimingPoints / HitObjects 行级整数化
            if downgrade
                && !bare.is_empty()
                && !bare.starts_with("//")
                && (section == "TimingPoints" || section == "HitObjects")
            {
                if let Some(rewritten) = downgrade_line(bare, &section, line, ln, &shifts) {
                    changed = true;
                    out.push_str(&rewritten);
                    continue;
                }
            }
            out.push_str(line);
            continue;
        }
        let fields: Vec<&str> = bare.split(',').collect();
        if fields.len() < 3 {
            changed = true; // 残缺 break 行：丢弃
            continue;
        }
        let parse_t = |s: &str| -> Option<i64> {
            let t = s.trim();
            if let Ok(v) = t.parse::<i64>() {
                return Some(v);
            }
            t.parse::<f64>()
                .ok()
                .filter(|f| f.is_finite())
                .map(|f| f.trunc() as i64)
        };
        match (parse_t(fields[1]), parse_t(fields[2])) {
            (Some(a), Some(b)) => {
                let tail: Vec<&str> = fields[3..].to_vec();
                let newline = if line.contains("\r\n") { "\r\n" } else { "\n" };
                out.push_str(&format!("2,{a},{b}"));
                for f in tail {
                    out.push(',');
                    out.push_str(f);
                }
                out.push_str(newline);
                if fields[1].trim().parse::<i64>().is_err()
                    || fields[2].trim().parse::<i64>().is_err()
                {
                    changed = true;
                }
            }
            _ => {
                changed = true; // 时间字段非数字：整行丢弃
            }
        }
    }
    (out, changed)
}

// ── 命名 ─────────────────────────────────────────────────────────────────────

/// 输出基名：`{id} {artist} - {title}`（stable Songs 文件夹惯例），逐段消毒；
/// 空段优雅省略（极端情况退化为裸 id）。
pub fn output_base_name(set_id: i64, artist: &str, title: &str) -> String {
    // 先判空再消毒：sanitize_filename 对空串返回 "untitled"，会把空段误判为非空
    let sanitize = |s: &str| {
        if s.is_empty() {
            String::new()
        } else {
            crate::thumbs::sanitize_filename(s)
        }
    };
    let artist = sanitize(artist);
    let title = sanitize(title);
    let id = if set_id > 0 {
        set_id.to_string()
    } else {
        String::new()
    };
    let mut name = id.clone();
    if !artist.is_empty() {
        if !name.is_empty() {
            name.push(' ');
        }
        name.push_str(&artist);
        if !title.is_empty() {
            name.push_str(" - ");
            name.push_str(&title);
        }
    } else if !title.is_empty() {
        if !name.is_empty() {
            name.push(' ');
        }
        name.push_str(&title);
    }
    if name.is_empty() {
        name.push_str("untitled");
    }
    // Windows 路径长度防御：集文件夹名上限 100 字符（Songs\folder\file 全长 <260）
    if name.chars().count() > 100 {
        name = name.chars().take(100).collect();
    }
    name
}

/// 文件名路径分段消毒：保留子目录结构（'/' 与 '\\' 视为分隔符），仅消毒每段。
fn sanitize_rel_path(name: &str) -> Vec<String> {
    name.split(['/', '\\'])
        .filter(|s| !s.is_empty() && *s != "." && *s != "..")
        .map(crate::thumbs::sanitize_filename)
        .filter(|s| !s.is_empty())
        .collect()
}

// ── 纯核心转换 ───────────────────────────────────────────────────────────────

/// 一集的转换输入：realm 索引出的文件清单（转换核心与 realm 解耦，便于测试）。
#[derive(Clone)]
pub struct SetSource {
    pub set_id: i64,
    pub title: String,
    pub artist: String,
    /// (原始文件名, 源绝对路径)
    pub files: Vec<(String, PathBuf)>,
}

fn progress(
    done: u32,
    total: u32,
    set_id: i64,
    state: ConvertState,
    current: &str,
    error: Option<String>,
) -> ConvertProgress {
    ConvertProgress {
        done,
        total,
        set_id,
        state,
        current: current.to_string(),
        error,
    }
}

/// stable 不支持的媒体格式（osu! wiki Compressing files 口径：音频仅 mp3/ogg）。
/// 检测文件名扩展名，返回 err.* 警告串（trError 可译）。
fn media_warnings(files: &[(String, PathBuf)]) -> Vec<String> {
    const BAD: &[(&str, &str)] = &[
        ("flac", "err.convertWarnAudio"),
        ("opus", "err.convertWarnAudio"),
        ("m4a", "err.convertWarnAudio"),
        ("aac", "err.convertWarnAudio"),
        ("wma", "err.convertWarnAudio"),
        ("webm", "err.convertWarnVideo"),
        ("mov", "err.convertWarnVideo"),
        ("mkv", "err.convertWarnVideo"),
    ];
    let mut out = Vec::new();
    for (name, _) in files {
        let ext = name.rsplit('.').next().unwrap_or("").to_lowercase();
        if let Some((_, key)) = BAD.iter().find(|(e, _)| *e == ext) {
            if out.len() < 8 {
                out.push(format!("{key}|{name}"));
            }
        }
    }
    out
}

/// 单集转换：Songs → 建目录写文件；Osz → 打 zip。失败回 Err（错误码形态）。
fn convert_one(
    mode: ConvertMode,
    target_dir: &Path,
    src: &SetSource,
) -> Result<(PathBuf, u32, u64, Vec<String>), String> {
    // 预检：任何源文件缺失 → 整集失败（部分集会让 stable 里出现残缺图，宁缺毋滥）
    let mut missing = 0usize;
    let mut first_missing = String::new();
    for (name, path) in &src.files {
        if !path.is_file() {
            missing += 1;
            if first_missing.is_empty() {
                first_missing = name.clone();
            }
        }
    }
    if missing > 0 {
        return Err(crate::errcode::ec3(
            crate::errcode::CONVERT_SOURCE_MISSING,
            missing,
            src.files.len(),
            first_missing,
        ));
    }

    let warnings = media_warnings(&src.files);
    let base = output_base_name(src.set_id, &src.artist, &src.title);
    match mode {
        ConvertMode::Songs => {
            let (dst_dir, _) = manage::unique_name(target_dir, &base, false);
            fs::create_dir_all(&dst_dir)
                .map_err(|e| crate::errcode::ec1(crate::errcode::CONVERT_CREATE_DIR_FAILED, e))?;
            let res: Result<(u32, u64), String> = (|| {
                let mut count = 0u32;
                let mut bytes = 0u64;
                for (name, path) in &src.files {
                    let parts = sanitize_rel_path(name);
                    let mut dst = dst_dir.clone();
                    for p in &parts {
                        dst.push(p);
                    }
                    if let Some(parent) = dst.parent() {
                        fs::create_dir_all(parent).map_err(|e| {
                            crate::errcode::ec1(crate::errcode::CONVERT_CREATE_DIR_FAILED, e)
                        })?;
                    }
                    let n = write_converted(path, &dst, name)?;
                    count += 1;
                    bytes += n;
                }
                Ok((count, bytes))
            })();
            // 中途失败清理半成品目录：stable 扫到残缺文件夹正是要避免的「宁缺毋滥」反例
            res.inspect_err(|_| {
                let _ = fs::remove_dir_all(&dst_dir);
            })
            .map(|(count, bytes)| (dst_dir.clone(), count, bytes, warnings))
        }
        ConvertMode::Osz => {
            let (dst_file, _) = manage::unique_name(target_dir, &format!("{base}.osz"), true);
            let file = fs::File::create(&dst_file)
                .map_err(|e| crate::errcode::ec1(crate::errcode::CONVERT_CREATE_DIR_FAILED, e))?;
            let mut zw = zip::ZipWriter::new(file);
            let deflate: zip::write::SimpleFileOptions = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated)
                .compression_level(Some(6));
            let stored: zip::write::SimpleFileOptions = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Stored);
            let mut count = 0u32;
            let mut bytes = 0u64;
            for (name, path) in &src.files {
                let entry = sanitize_rel_path(name).join("/");
                match sanitized_osu_bytes(path, name)? {
                    Some(content) => {
                        zw.start_file(&entry, deflate).map_err(|e| {
                            crate::errcode::ec1(crate::errcode::CONVERT_ZIP_FAILED, e)
                        })?;
                        zw.write_all(&content).map_err(|e| {
                            crate::errcode::ec1(crate::errcode::CONVERT_ZIP_FAILED, e)
                        })?;
                        bytes += content.len() as u64;
                    }
                    None => {
                        // 非 .osu（常为上百 MB 音视频）：流式写入 zip，不整读进内存；
                        // 已压缩媒体用 Stored 免白烧 CPU。
                        let mut src_file = fs::File::open(path).map_err(|e| {
                            crate::errcode::ec1(crate::errcode::CONVERT_WRITE_FAILED, e)
                        })?;
                        zw.start_file(
                            &entry,
                            if is_precompressed(name) {
                                stored
                            } else {
                                deflate
                            },
                        )
                        .map_err(|e| crate::errcode::ec1(crate::errcode::CONVERT_ZIP_FAILED, e))?;
                        let n = std::io::copy(&mut src_file, &mut zw).map_err(|e| {
                            crate::errcode::ec1(crate::errcode::CONVERT_ZIP_FAILED, e)
                        })?;
                        bytes += n;
                    }
                }
                count += 1;
            }
            zw.finish()
                .map_err(|e| crate::errcode::ec1(crate::errcode::CONVERT_ZIP_FAILED, e))
                .inspect_err(|_| {
                    let _ = fs::remove_file(&dst_file);
                })
                .map(|_| (dst_file.clone(), count, bytes, warnings))
        }
    }
}

/// 已压缩媒体扩展名（小写含点）：deflate 收益 ≈0–2% 却全额烧 CPU，Osz 模式对
/// 这些文件改用 Stored（产物仍是合法 zip，osu! 稳定支持）；其余（.osu/.osb/.wav
/// 等文本/PCM）保持 Deflated 6。
fn is_precompressed(entry_name: &str) -> bool {
    let lower = entry_name.to_lowercase();
    [
        ".mp3", ".ogg", ".m4a", ".mp4", ".jpg", ".jpeg", ".png", ".webp",
    ]
    .iter()
    .any(|ext| lower.ends_with(ext))
}

/// 条目名为 .osu 的内容做 stable 清洗，返回清洗后字节；其余返回 None（走流式复制）。
fn sanitized_osu_bytes(path: &Path, entry_name: &str) -> Result<Option<Vec<u8>>, String> {
    let is_osu = entry_name
        .rsplit('.')
        .next()
        .map(|ext| ext.eq_ignore_ascii_case("osu"))
        .unwrap_or(false);
    if !is_osu {
        return Ok(None);
    }
    let raw =
        fs::read(path).map_err(|e| crate::errcode::ec1(crate::errcode::CONVERT_WRITE_FAILED, e))?;
    let text = String::from_utf8_lossy(&raw);
    let (cleaned, _) = sanitize_osu_for_stable(&text);
    Ok(Some(cleaned.into_bytes()))
}

/// Songs 模式落盘：.osu 清洗后写出；其余直接 fs::copy（Windows 走 CopyFileEx，
/// 免用户态整块缓冲——此前整读进内存再写出，大视频场景峰值内存 = 文件大小）。
/// 返回写入字节数。
fn write_converted(src: &Path, dst: &Path, entry_name: &str) -> Result<u64, String> {
    if let Some(bytes) = sanitized_osu_bytes(src, entry_name)? {
        fs::write(dst, &bytes)
            .map_err(|e| crate::errcode::ec1(crate::errcode::CONVERT_WRITE_FAILED, e))?;
        return Ok(bytes.len() as u64);
    }
    fs::copy(src, dst).map_err(|e| crate::errcode::ec1(crate::errcode::CONVERT_WRITE_FAILED, e))
}

/// 批量转换纯核心：逐集转换，单集失败不中断；cancelled() 返回 true 时剩余集标记取消。
/// emit 在每集排队/开始/结束时调用（与下载面板相同的节奏）。
pub fn convert_core(
    mode: ConvertMode,
    target_dir: &Path,
    sources: &[SetSource],
    emit: &(dyn Fn(ConvertProgress) + Send + Sync),
    cancelled: &dyn Fn() -> bool,
) -> ConvertReport {
    let total = sources.len() as u32;
    for s in sources {
        emit(progress(
            0,
            total,
            s.set_id,
            ConvertState::Queued,
            &output_base_name(s.set_id, &s.artist, &s.title),
            None,
        ));
    }
    let mut report = ConvertReport {
        total,
        converted: 0,
        failed: 0,
        cancelled: false,
        output_dir: target_dir.to_string_lossy().into_owned(),
        results: Vec::with_capacity(sources.len()),
    };
    let _ = fs::create_dir_all(target_dir);
    for (i, s) in sources.iter().enumerate() {
        let current = output_base_name(s.set_id, &s.artist, &s.title);
        if cancelled() {
            report.cancelled = true;
            emit(progress(
                i as u32,
                total,
                s.set_id,
                ConvertState::Cancelled,
                &current,
                None,
            ));
            report.results.push(ConvertSetResult {
                set_id: s.set_id,
                ok: false,
                output: None,
                files: 0,
                bytes: 0,
                error: Some(crate::errcode::ec(crate::errcode::CANCELLED)),
                warnings: Vec::new(),
            });
            continue;
        }
        emit(progress(
            i as u32,
            total,
            s.set_id,
            ConvertState::Converting,
            &current,
            None,
        ));
        match convert_one(mode, target_dir, s) {
            Ok((path, files, bytes, warnings)) => {
                report.converted += 1;
                emit(progress(
                    i as u32 + 1,
                    total,
                    s.set_id,
                    ConvertState::Done,
                    &current,
                    None,
                ));
                report.results.push(ConvertSetResult {
                    set_id: s.set_id,
                    ok: true,
                    output: Some(path.to_string_lossy().into_owned()),
                    files,
                    bytes,
                    error: None,
                    warnings,
                });
            }
            Err(e) => {
                report.failed += 1;
                emit(progress(
                    i as u32 + 1,
                    total,
                    s.set_id,
                    ConvertState::Failed,
                    &current,
                    Some(e.clone()),
                ));
                report.results.push(ConvertSetResult {
                    set_id: s.set_id,
                    ok: false,
                    output: None,
                    files: 0,
                    bytes: 0,
                    error: Some(e),
                    warnings: Vec::new(),
                });
            }
        }
    }
    report
}

// ── Tauri 命令 ───────────────────────────────────────────────────────────────

/// 解析 lazer 数据目录：优先配置（含安装目录解析），退化到自动检测候选。
fn resolve_data_dir(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    let cfg = crate::config::AppConfig::load(&crate::config::config_path(Some(
        &crate::online::app_dir(app),
    )));
    if let Some(d) = cfg.lazer_dir {
        let p = PathBuf::from(&d);
        let resolved = crate::detect::resolve_lazer_data_dir(&p);
        if resolved.join("client.realm").is_file() {
            return Ok(resolved);
        }
    }
    for c in crate::detect::detect_libraries() {
        if c.kind == crate::model::SourceKind::Lazer {
            let p = PathBuf::from(c.path);
            if p.join("client.realm").is_file() {
                return Ok(p);
            }
        }
    }
    Err(crate::errcode::ec(crate::errcode::CONVERT_NO_LAZER_DIR))
}

#[tauri::command]
pub async fn convert_lazer_to_stable(
    app: tauri::AppHandle,
    set_ids: Vec<i64>,
    mode: ConvertMode,
    target_dir: String,
) -> Result<ConvertReport, String> {
    // 去重保序 + 上限
    let mut ids: Vec<i64> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for id in set_ids {
        if seen.insert(id) {
            ids.push(id);
        }
    }
    if ids.is_empty() {
        return Err(crate::errcode::ec(crate::errcode::CONVERT_NO_SETS));
    }
    if ids.len() > MAX_SETS_PER_CALL {
        return Err(crate::errcode::ec2(
            crate::errcode::CONVERT_TOO_MANY,
            MAX_SETS_PER_CALL,
            ids.len(),
        ));
    }
    let target = PathBuf::from(&target_dir);
    let roots = manage::lazer_roots(&app);
    if let Some(msg) = manage::lazer_guard(&target, &roots) {
        return Err(msg);
    }
    // 运行锁：持有到命令结束（Drop 复位）。此前 begin() 从未被调用，
    // RUNNING 恒为 false → cancel_convert 的取消检查形同虚设（取消按钮失效根因）
    let _run_guard =
        ConvertRunGuard::begin().ok_or_else(|| crate::errcode::ec(crate::errcode::CONVERT_BUSY))?;

    let data_dir = resolve_data_dir(&app)?;
    use tauri::Emitter;
    let report = tauri::async_runtime::spawn_blocking(move || {
        // 只索引请求的集：全库探测是转换的固定前置成本，按请求集过滤后与曲库体积无关
        let wanted: std::collections::HashSet<i64> = ids.iter().copied().collect();
        let index = crate::realm_db::collect_set_files(&data_dir, Some(&wanted))
            .map_err(|e| crate::errcode::ec1(crate::errcode::CONVERT_SCAN_FAILED, e))?;
        let mut by_id: std::collections::HashMap<i64, &crate::realm_db::LazerSetFileIndex> =
            std::collections::HashMap::new();
        for entry in &index {
            // 同 ID 重复记录取首个（realm 中不应出现；防御性）
            by_id.entry(entry.online_id).or_insert(entry);
        }
        // 组装 SetSource：请求顺序；本地 ID(≤0)/库中缺失 → 预置失败结果
        let mut sources: Vec<SetSource> = Vec::new();
        let mut presized_failures: Vec<ConvertSetResult> = Vec::new();
        for id in &ids {
            if *id <= 0 {
                presized_failures.push(ConvertSetResult {
                    set_id: *id,
                    ok: false,
                    output: None,
                    files: 0,
                    bytes: 0,
                    error: Some(crate::errcode::ec(
                        crate::errcode::CONVERT_LOCAL_UNSUPPORTED,
                    )),
                    warnings: Vec::new(),
                });
                continue;
            }
            match by_id.get(id) {
                Some(entry) => sources.push(SetSource {
                    set_id: entry.online_id,
                    title: entry.title.clone(),
                    artist: entry.artist.clone(),
                    files: entry.files.clone(),
                }),
                None => presized_failures.push(ConvertSetResult {
                    set_id: *id,
                    ok: false,
                    output: None,
                    files: 0,
                    bytes: 0,
                    error: Some(crate::errcode::ec(crate::errcode::CONVERT_SET_NOT_FOUND)),
                    warnings: Vec::new(),
                }),
            }
        }
        let mut report = convert_core(
            mode,
            &target,
            &sources,
            &|p| {
                let _ = app.emit("convert-progress", p);
            },
            &ConvertRunGuard::cancelled,
        );
        report.results.extend(presized_failures);
        Ok::<ConvertReport, String>(report)
    })
    .await
    .map_err(|e| crate::errcode::ec1(crate::errcode::CONVERT_INTERRUPTED, e))??;
    Ok(report)
}

#[tauri::command]
pub fn cancel_convert() {
    request_cancel();
}

// ── 测试 ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    struct TempDir(PathBuf);
    impl TempDir {
        fn new(tag: &str) -> Self {
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let root = std::env::temp_dir()
                .join(format!("osm_convert_{tag}_{}_{nanos}", std::process::id()));
            fs::create_dir_all(&root).unwrap();
            Self(root)
        }
    }
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    /// lazer 编辑器形态的 .osu：v128 版本行 + 浮点 break + 内嵌 storyboard 命令
    const LAZER_EDITED_OSU: &str = "osu file format v128\n\
\n\
[General]\n\
AudioFilename: audio.mp3\n\
Mode: 0\n\
\n\
[Events]\n\
0,0,\"bg.jpg\",0,0\n\
2,48577.86047984592,49772.32558139535\n\
Sprite,Foreground,Centre,\"sb\\dot.png\",320,240\n\
 M,0,1000,2000,0,0,100,100\n\
2,95845.30233467814,100005.32558139534\n\
\n\
[TimingPoints]\n\
0,500,4,2,1,40,1,0\n\
\n\
[HitObjects]\n\
256,192,1000,1,0\n";

    #[test]
    fn sanitize_rewrites_float_breaks_keeps_everything_else() {
        let (out, changed) = sanitize_osu_for_stable(LAZER_EDITED_OSU);
        assert!(changed);
        // 浮点 break → 整数（截断，C# (int) 语义，同官方 legacy 导出）
        assert!(out.contains("2,48577,49772\n"), "{out}");
        assert!(out.contains("2,95845,100005\n"), "{out}");
        // v128 头降级为 v14
        assert!(out.starts_with("osu file format v14\n"), "{out}");
        // 其余内容逐字节保留（注意：Rust 字符串续行会吞行首空格，常量里 storyboard
        // 缩进命令行实际无前导空格）
        assert!(out.contains("0,0,\"bg.jpg\",0,0\n"));
        assert!(out.contains("Sprite,Foreground,Centre,\"sb\\dot.png\",320,240\n"));
        assert!(out.contains("M,0,1000,2000,0,0,100,100\n"));
        assert!(out.contains("[HitObjects]\n256,192,1000,1,0\n"));
    }

    #[test]
    fn sanitize_downgrades_v128_numeric_fields() {
        let v128 = "osu file format v128\n\
\n\
[TimingPoints]\n\
1234.75,500.5,4,2,1,40,1,0\n\
2000,-100.0,4,3,1,40,0,0\n\
\n\
[HitObjects]\n\
108.6,192.2,1000.9,1,0\n\
100,200,2000.0,2,0,B|300:300|400:100,2.5,150.75,0:1:0:0:\n\
256,192,3000.5,12,0,5000.7,0:1:0:0:\n\
256,192,4000.5,128,0,6000.7:0:0:0:0:small.wav\n";
        let (out, changed) = sanitize_osu_for_stable(v128);
        assert!(changed);
        assert!(out.starts_with("osu file format v14\n"), "{out}");
        // TimingPoints 首字段 floor；beatLength 等其余字段原样（v14 允许浮点）。
        // #30607：第一个 TP floor 产生 -0.75 偏移，累计平移后续时间点与物件
        assert!(out.contains("\n1234,500.5,4,2,1,40,1,0\n"), "{out}");
        assert!(out.contains("\n1999,-100.0,4,3,1,40,0,0\n"), "{out}");
        // circle: x/y round、time 随区间平移
        assert!(out.contains("\n109,192,1000,1,0\n"), "{out}");
        // slider: 曲线路径与 hitSample 尾原样；时间平移
        assert!(
            out.contains("\n100,200,1998,2,0,B|300:300|400:100,2,150,0:1:0:0:\n"),
            "{out}"
        );
        // spinner: endTime 截断 + 平移
        assert!(out.contains("\n256,192,2998,12,0,4998,0:1:0:0:\n"), "{out}");
        // hold: endTime 截断 + 平移、hitSample 尾巴原样保留
        assert!(
            out.contains("\n256,192,3998,128,0,5998:0:0:0:0:small.wav\n"),
            "{out}"
        );
        // 幂等：降级产物二次处理为 no-op
        let (twice, changed2) = sanitize_osu_for_stable(&out);
        assert!(!changed2);
        assert_eq!(out, twice);
    }

    #[test]
    fn v14_passthrough_is_byte_identical() {
        let v14 = "osu file format v14\n\
\n\
[TimingPoints]\n\
1234,500.5,4,2,1,40,1,0\n\
\n\
[HitObjects]\n\
108,192,1000,1,0\n\
100,200,2000,2,0,B|300:300|400:100,2,150,0:1:0:0:\n";
        let (out, changed) = sanitize_osu_for_stable(v14);
        assert!(!changed);
        assert_eq!(out, v14);
    }

    #[test]
    fn version_line_tolerates_bom() {
        let bom_v128 = "\u{feff}osu file format v128\r\n\r\n[Events]\r\n2,10.5,20.7\r\n";
        let (out, changed) = sanitize_osu_for_stable(bom_v128);
        assert!(changed);
        assert!(out.starts_with("\u{feff}osu file format v14\r\n"), "{out}");
        assert!(out.contains("2,10,20\r\n"), "{out}");
    }

    #[test]
    fn sanitize_is_idempotent_and_noop_for_clean_files() {
        let (once, changed) = sanitize_osu_for_stable(LAZER_EDITED_OSU);
        assert!(changed);
        let (twice, changed2) = sanitize_osu_for_stable(&once);
        assert!(!changed2, "二次清洗应为 no-op");
        assert_eq!(once, twice);
        // 原始整数 break 的普通谱面不动
        let stable = "osu file format v14\n\n[Events]\n0,0,\"bg.jpg\",0,0\n2,1000,2000\n\n[HitObjects]\n1,1,1,1,1\n";
        let (out, changed) = sanitize_osu_for_stable(stable);
        assert!(!changed);
        assert_eq!(out, stable);
    }

    #[test]
    fn sanitize_drops_unparseable_break_lines() {
        let bad = "[Events]\n0,0,\"b.jpg\",0,0\n2,abc,def\n[HitObjects]\n";
        let (out, changed) = sanitize_osu_for_stable(bad);
        assert!(changed);
        assert!(!out.contains("2,abc"));
        assert!(out.contains("0,0,\"b.jpg\",0,0"));
    }

    #[test]
    fn base_name_conventions() {
        assert_eq!(
            output_base_name(387700, "Artist", "Title"),
            "387700 Artist - Title"
        );
        assert_eq!(output_base_name(1, "", "Only Title"), "1 Only Title");
        assert_eq!(output_base_name(2, "Only Artist", ""), "2 Only Artist");
        // 非法字符消毒
        assert_eq!(output_base_name(3, "a:b<c>", "d?e"), "3 a_b_c_ - d_e");
        assert_eq!(output_base_name(-1, "", ""), "untitled");
    }

    fn fixture_source(root: &Path, name: &str, id: i64) -> SetSource {
        let files_dir = root.join("files");
        fs::create_dir_all(&files_dir).unwrap();
        let audio = files_dir.join("audiohash");
        fs::write(&audio, b"ID3 fake mp3").unwrap();
        let osu = files_dir.join("osuhash");
        fs::write(&osu, LAZER_EDITED_OSU).unwrap();
        let bg = files_dir.join("bghash");
        fs::write(&bg, b"\xFF\xD8\xFF fake jpg").unwrap();
        SetSource {
            set_id: id,
            title: name.to_string(),
            artist: "Artist".into(),
            files: vec![
                ("audio.mp3".into(), audio),
                ("map [Insane].osu".into(), osu),
                ("bg.jpg".into(), bg),
            ],
        }
    }

    #[test]
    fn songs_mode_copies_sanitizes_and_renames_on_collision() {
        let tmp = TempDir::new("songs");
        let target = tmp.0.join("out");
        let src = fixture_source(&tmp.0, "Song", 387700);

        let events: std::sync::Mutex<Vec<ConvertProgress>> = std::sync::Mutex::new(Vec::new());
        let rep = convert_core(
            ConvertMode::Songs,
            &target,
            std::slice::from_ref(&src),
            &|p| events.lock().unwrap().push(p),
            &|| false,
        );
        assert_eq!(rep.converted, 1);
        assert_eq!(rep.failed, 0);
        assert!(!rep.cancelled);
        let dir = PathBuf::from(rep.results[0].output.as_ref().unwrap());
        assert!(dir.is_dir());
        assert!(dir.file_name().unwrap().to_string_lossy() == "387700 Artist - Song");
        // .osu 被降级（v128 头 → v14）+ break 截断
        let cleaned = fs::read_to_string(dir.join("map [Insane].osu")).unwrap();
        assert!(cleaned.contains("osu file format v14"));
        assert!(cleaned.contains("2,48577,49772"));
        // 其他文件原样
        assert_eq!(fs::read(dir.join("audio.mp3")).unwrap(), b"ID3 fake mp3");
        assert_eq!(rep.results[0].files, 3);
        assert!(rep.results[0].bytes > 0);

        // 同名冲突 → (2)
        let rep2 = convert_core(ConvertMode::Songs, &target, &[src], &|_| {}, &|| false);
        let dir2 = PathBuf::from(rep2.results[0].output.as_ref().unwrap());
        assert!(dir2.file_name().unwrap().to_string_lossy() == "387700 Artist - Song (2)");
    }

    #[test]
    fn osz_mode_packs_zip_with_sanitized_entries() {
        let tmp = TempDir::new("osz");
        let target = tmp.0.join("out");
        let src = fixture_source(&tmp.0, "Packed", 158023);
        let rep = convert_core(ConvertMode::Osz, &target, &[src], &|_| {}, &|| false);
        assert_eq!(rep.converted, 1);
        let path = PathBuf::from(rep.results[0].output.as_ref().unwrap());
        assert!(path.file_name().unwrap().to_string_lossy() == "158023 Artist - Packed.osz");
        let f = fs::File::open(&path).unwrap();
        let mut za = zip::ZipArchive::new(f).unwrap();
        let names: Vec<String> = (0..za.len())
            .map(|i| za.by_index(i).unwrap().name().to_string())
            .collect();
        assert!(names.contains(&"map [Insane].osu".to_string()), "{names:?}");
        assert!(names.contains(&"audio.mp3".to_string()));
        let mut content = String::new();
        std::io::Read::read_to_string(&mut za.by_name("map [Insane].osu").unwrap(), &mut content)
            .unwrap();
        assert!(content.contains("2,48577,49772"), "zip 内 .osu 应已清洗");
        assert!(
            content.contains("osu file format v14"),
            "zip 内 .osu 头应已降级"
        );
        // zip 可被扫描器读回（有 .osu 条目即可被识别为谱面包）
    }

    #[test]
    fn missing_source_fails_that_set_only() {
        let tmp = TempDir::new("missing");
        let target = tmp.0.join("out");
        let mut good = fixture_source(&tmp.0, "Good", 1);
        good.set_id = 1;
        let bad = SetSource {
            set_id: 2,
            title: "Bad".into(),
            artist: "A".into(),
            files: vec![
                ("audio.mp3".into(), tmp.0.join("files").join("audiohash")),
                ("ghost.osu".into(), tmp.0.join("files").join("nonexistent")),
            ],
        };
        let rep = convert_core(ConvertMode::Songs, &target, &[bad, good], &|_| {}, &|| {
            false
        });
        assert_eq!(rep.converted, 1, "好集不受坏集影响");
        assert_eq!(rep.failed, 1);
        let err = rep.results[0].error.as_deref().unwrap();
        assert!(err.starts_with("err.convertSourceMissing|"), "{err}");
        assert!(!PathBuf::from(rep.results[1].output.as_ref().unwrap()).is_file());
        assert!(PathBuf::from(rep.results[1].output.as_ref().unwrap()).is_dir());
    }

    #[test]
    fn cancel_marks_remaining_as_cancelled() {
        let tmp = TempDir::new("cancel");
        let target = tmp.0.join("out");
        let sources: Vec<SetSource> = (1..=3)
            .map(|i| {
                let mut s = fixture_source(&tmp.0, "S", i);
                s.set_id = i;
                s
            })
            .collect();
        let rep = convert_core(ConvertMode::Songs, &target, &sources, &|_| {}, &|| true);
        assert!(rep.cancelled);
        assert_eq!(rep.converted, 0);
        assert_eq!(rep.failed, 0);
        assert!(rep
            .results
            .iter()
            .all(|r| r.error.as_deref() == Some(crate::errcode::CANCELLED)));
    }

    /// 真机端到端冒烟（临时测试性长期保留：转换链路依赖本机 lazer 数据）：
    /// 取库前若干集转 Songs，断言产物 .osu 头版本 ≤ v14（v128 已降级）。
    #[test]
    #[ignore = "requires local osu!lazer installation"]
    fn real_convert_smoke() {
        let lazer = crate::detect::detect_libraries()
            .into_iter()
            .find(|c| c.kind == crate::model::SourceKind::Lazer)
            .expect("lazer library detected");
        let data_dir = std::path::PathBuf::from(lazer.path);
        let index = crate::realm_db::collect_set_files(&data_dir, None).expect("collect_set_files");
        assert!(!index.is_empty());
        // 全库 .osu 头版本画像：v128（lazer 编辑过）的集数量
        let mut sets_with_v128 = 0u32;
        for e in &index {
            let has = e.files.iter().any(|(n, p)| {
                n.to_lowercase().ends_with(".osu")
                    && std::fs::read_to_string(p)
                        .ok()
                        .and_then(|t| osu_format_version(&t))
                        .map(|v| v > 14)
                        .unwrap_or(false)
            });
            if has {
                sets_with_v128 += 1;
            }
        }
        println!("sets total={} with_v128_osu={sets_with_v128}", index.len());

        let sources: Vec<SetSource> = index
            .iter()
            .take(5)
            .map(|e| SetSource {
                set_id: e.online_id,
                title: e.title.clone(),
                artist: e.artist.clone(),
                files: e.files.clone(),
            })
            .collect();
        let tmp = std::env::temp_dir().join(format!("osm_real_convert_{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        let rep = convert_core(ConvertMode::Songs, &tmp, &sources, &|_| {}, &|| false);
        println!(
            "real convert: converted={} failed={}",
            rep.converted, rep.failed
        );
        assert!(rep.converted > 0, "at least one set converted");
        for r in &rep.results {
            if r.ok {
                let dir = PathBuf::from(r.output.as_ref().unwrap());
                for entry in fs::read_dir(&dir).unwrap().flatten() {
                    let p = entry.path();
                    let is_osu = p
                        .extension()
                        .map(|e| e.eq_ignore_ascii_case("osu"))
                        .unwrap_or(false);
                    if !is_osu {
                        continue;
                    }
                    let text = fs::read_to_string(&p).unwrap_or_default();
                    if let Some(v) = osu_format_version(&text) {
                        assert!(v <= 14, "{} header v{v} must be ≤14", p.display());
                    }
                }
            }
        }
        let _ = fs::remove_dir_all(&tmp);
    }
}
