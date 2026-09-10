// manage.rs
//
// Purpose: M6 库管理——查重、回收站删除、移动、打包导出。
//
// IRON RULE：lazer 曲库只读。delete/move 在执行前对每条路径做「是否位于任一
// lazer 根目录下」检查（lazer 根 = 配置里的 lazer_dir 及其解析结果 + 自动检测
// 候选中所有 Lazer 项）；命中即拒绝并回读只读提示。pack（zip）只读取源内容、
// 写入用户选择的归档文件，属只读访问，因此不拒绝 lazer 路径，但同样绝不修改源。
// 一切删除走回收站（trash），且仅针对「含 .osu 的 stable 集文件夹」或「*.osz
// 文件」这两种形态，其余路径一律拒绝，绝不触碰任意文件。

use crate::config::{config_path, AppConfig};
use crate::detect;
use crate::library;
use crate::model::{
    DupMember, DuplicateGroup, FailedPath, MoveReport, PackReport, SourceKind, TrashReport,
};
use crate::online::app_dir;
use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

/// 只读拒绝文案（契约要求；T1 起为稳定错误码，前端翻译）
pub const LAZER_READONLY_MSG: &str = crate::errcode::LAZER_READ_ONLY;
const SHAPE_MSG: &str = crate::errcode::BAD_SHAPE;
pub const PACK_LIMIT_BYTES: u64 = 2_000_000_000; // 2 GB

// ── 只读守卫 ──────────────────────────────────────────────────────────────────

/// 收集全部 lazer 数据根目录（去重；含配置项、其安装目录解析结果、自动检测候选）。
pub fn lazer_roots(app: &tauri::AppHandle) -> Vec<PathBuf> {
    let mut raw: Vec<PathBuf> = Vec::new();
    let cfg = AppConfig::load(&config_path(Some(&app_dir(app))));
    if let Some(dir) = cfg.lazer_dir {
        let p = PathBuf::from(&dir);
        let resolved = detect::resolve_lazer_data_dir(&p);
        raw.push(p);
        if resolved != dir {
            raw.push(resolved);
        }
    }
    for c in detect::detect_libraries() {
        if c.kind == SourceKind::Lazer {
            raw.push(PathBuf::from(c.path));
        }
    }
    let mut seen = std::collections::HashSet::new();
    raw.into_iter()
        .filter(|p| seen.insert(p.to_string_lossy().to_lowercase()))
        .collect()
}

/// 规范化比较键：沿最深存在的父目录做 canonicalize（去掉 `\\?\` 前缀、解析临时目录
/// 短名/junction），再拼回不存在的尾部；整体小写、去结尾分隔符。
/// 这样"存在的路径"与"其下不存在的路径"拥有可比较的同一前缀。
fn norm_key(p: &Path) -> String {
    // 找到最深的存在祖先
    let mut cursor = p;
    let mut suffix: Vec<std::ffi::OsString> = Vec::new();
    while !cursor.exists() {
        match (cursor.parent(), cursor.file_name()) {
            (Some(par), Some(name)) => {
                suffix.push(name.to_os_string());
                cursor = par;
            }
            _ => break,
        }
    }
    let base = cursor
        .canonicalize()
        .unwrap_or_else(|_| cursor.to_path_buf());
    let mut full = base;
    for s in suffix.iter().rev() {
        full = full.join(s);
    }
    let str_full = full.to_string_lossy().to_lowercase();
    let str_full = str_full
        .strip_prefix("\\\\?\\")
        .map(str::to_string)
        .unwrap_or(str_full);
    str_full.trim_end_matches(['\\', '/']).to_string()
}

/// 路径本身等于或位于任一 lazer 根之下 → Some(拒绝理由)。大小写不敏感。
pub fn lazer_guard(path: &Path, roots: &[PathBuf]) -> Option<String> {
    let target = norm_key(path);
    for r in roots {
        let rk = norm_key(r);
        if rk.is_empty() {
            continue;
        }
        if target == rk
            || target.starts_with(format!("{rk}\\").as_str())
            || target.starts_with(format!("{rk}/").as_str())
        {
            return Some(LAZER_READONLY_MSG.to_string());
        }
    }
    None
}

/// 操作形态校验：目录且直接含 *.osu，或是 *.osz 文件。其余一律拒绝。
fn shape_ok(path: &Path) -> bool {
    if path.is_file() {
        return path
            .extension()
            .map(|e| e.eq_ignore_ascii_case("osz"))
            .unwrap_or(false);
    }
    if path.is_dir() {
        return fs::read_dir(path)
            .map(|rd| {
                rd.flatten().any(|e| {
                    e.path().is_file()
                        && e.path()
                            .extension()
                            .map(|x| x.eq_ignore_ascii_case("osu"))
                            .unwrap_or(false)
                })
            })
            .unwrap_or(false);
    }
    false
}

// ── 大小 / 复制工具 ───────────────────────────────────────────────────────────

/// 目录树（或单文件）字节总量；读取错误按 0 计。
pub fn walk_size(path: &Path) -> u64 {
    if path.is_file() {
        return fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    }
    let mut total = 0u64;
    let Ok(rd) = fs::read_dir(path) else {
        return 0;
    };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            total += walk_size(&p);
        } else if let Ok(m) = fs::metadata(&p) {
            total += m.len();
        }
    }
    total
}

fn copy_tree(src: &Path, dst: &Path) -> Result<(), String> {
    if src.is_file() {
        if let Some(parent) = dst.parent() {
            fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        return fs::copy(src, dst).map(|_| ()).map_err(|e| e.to_string());
    }
    fs::create_dir_all(dst).map_err(|e| e.to_string())?;
    for entry in fs::read_dir(src).map_err(|e| e.to_string())?.flatten() {
        let from = entry.path();
        let to = dst.join(entry.file_name());
        if from.is_dir() {
            copy_tree(&from, &to)?;
        } else if fs::copy(&from, &to).is_err() {
            return Err(crate::errcode::ec1(
                crate::errcode::COPY_FAILED,
                from.display(),
            ));
        }
    }
    Ok(())
}

/// 目标目录内防冲突：name → name (2) → name (3)…
/// is_file=true 时后缀插在扩展名前（x.osz → x (2).osz）；文件夹名整体追加。
pub(crate) fn unique_name(dir: &Path, name: &str, is_file: bool) -> (PathBuf, u32) {
    let mut n = 0u32;
    loop {
        let candidate = if n == 0 {
            name.to_string()
        } else if is_file {
            split_ext(name)
                .map(|(stem, ext)| format!("{stem} ({}).{ext}", n + 1))
                .unwrap_or_else(|| format!("{name} ({})", n + 1))
        } else {
            format!("{name} ({})", n + 1)
        };
        let p = dir.join(&candidate);
        if !p.exists() {
            return (p, n);
        }
        n += 1;
    }
}

fn split_ext(name: &str) -> Option<(&str, &str)> {
    let dot = name.rfind('.')?;
    if dot == 0 || dot + 1 >= name.len() {
        return None;
    }
    Some((&name[..dot], &name[dot + 1..]))
}

// ── 查重 ──────────────────────────────────────────────────────────────────────

fn fingerprint(set: &crate::model::BeatmapSetInfo) -> Option<String> {
    let mut parts: Vec<&str> = set
        .difficulties
        .iter()
        .filter_map(|d| d.md5.as_deref())
        .collect();
    if parts.is_empty() {
        return None;
    }
    parts.sort_unstable();
    Some(parts.join("|"))
}

/// 纯核心：对扫描结果分组。firstAdded = 组内最老成员 dateAdded（无则 null）。
pub fn duplicate_groups(
    sets: &[crate::model::BeatmapSetInfo],
    size_of: &dyn Fn(&Path) -> u64,
) -> Vec<DuplicateGroup> {
    let mut groups: Vec<DuplicateGroup> = Vec::new();

    let member = |s: &crate::model::BeatmapSetInfo, first: Option<i64>| DupMember {
        set_id: s.beatmapset_id,
        title: s.display_title(),
        artist: s.display_artist(),
        creator: s.creator.clone(),
        difficulty_count: s.difficulties.len() as u32,
        location: s.location.clone(),
        size_bytes: if s.source_kind == SourceKind::Lazer {
            0
        } else {
            size_of(Path::new(&s.location))
        },
        first_added: first.or(s.date_added),
    };

    // 1) setOnlineId：相同 OnlineID(>0) 被 ≥2 条集记录持有
    let mut by_online: BTreeMap<i64, Vec<usize>> = BTreeMap::new();
    for (i, s) in sets.iter().enumerate() {
        if s.beatmapset_id > 0 {
            by_online.entry(s.beatmapset_id).or_default().push(i);
        }
    }
    for (id, idxs) in by_online {
        if idxs.len() < 2 {
            continue;
        }
        let first = oldest_added(sets, &idxs);
        groups.push(DuplicateGroup {
            reason: "setOnlineId".into(),
            label: format!("Online ID {id}"),
            members: idxs.iter().map(|&i| member(&sets[i], first)).collect(),
        });
    }

    // 2) md5：同一难度 md5 出现在 ≥2 个不同 set（同集内重复不算组）
    let mut by_md5: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    for (i, s) in sets.iter().enumerate() {
        let mut uniq: Vec<&str> = s
            .difficulties
            .iter()
            .filter_map(|d| d.md5.as_deref())
            .collect();
        uniq.sort_unstable();
        uniq.dedup();
        for h in uniq {
            by_md5.entry(h.to_string()).or_default().push(i);
        }
    }
    for (md5, mut idxs) in by_md5 {
        idxs.sort_unstable();
        idxs.dedup();
        if idxs.len() < 2 {
            continue;
        }
        let first = oldest_added(sets, &idxs);
        groups.push(DuplicateGroup {
            reason: "md5".into(),
            label: format!("MD5 {}", md5.chars().take(8).collect::<String>()),
            members: idxs.iter().map(|&i| member(&sets[i], first)).collect(),
        });
    }

    // 3) identicalSet：OnlineID<=0 的本地导入克隆，难度 md5 指纹一致
    let mut by_fp: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    for (i, s) in sets.iter().enumerate() {
        if s.beatmapset_id > 0 {
            continue;
        }
        if let Some(fp) = fingerprint(s) {
            by_fp.entry(fp).or_default().push(i);
        }
    }
    for (fp, idxs) in by_fp {
        if idxs.len() < 2 {
            continue;
        }
        let first = oldest_added(sets, &idxs);
        let label = sets[idxs[0]].display_title();
        let _ = fp;
        groups.push(DuplicateGroup {
            reason: "identicalSet".into(),
            label,
            members: idxs.iter().map(|&i| member(&sets[i], first)).collect(),
        });
    }

    groups.sort_by(|a, b| {
        b.members
            .len()
            .cmp(&a.members.len())
            .then_with(|| a.label.cmp(&b.label))
    });
    groups
}

fn oldest_added(sets: &[crate::model::BeatmapSetInfo], idxs: &[usize]) -> Option<i64> {
    idxs.iter().filter_map(|&i| sets[i].date_added).min()
}

/// 不触发进度事件的扫描（复用 scan_library 的 no-op progress）。
pub fn scan_quiet(
    kind: SourceKind,
    path: &str,
) -> Result<Vec<crate::model::BeatmapSetInfo>, String> {
    library::scan_library(kind, path, &|_done, _total| {})
}

// ── 命令 ──────────────────────────────────────────────────────────────────────

#[tauri::command]
pub async fn detect_duplicates(
    app: tauri::AppHandle,
    kind: SourceKind,
    path: String,
) -> Result<Vec<DuplicateGroup>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let _ = &app; // 命令签名要求 app；扫描本身不需要
        let sets = scan_quiet(kind, &path)?;
        Ok(duplicate_groups(&sets, &walk_size))
    })
    .await
    .map_err(|e| crate::errcode::ec1(crate::errcode::DETECT_INTERRUPTED, e))?
}

#[tauri::command]
pub async fn delete_to_trash(
    app: tauri::AppHandle,
    paths: Vec<String>,
) -> Result<TrashReport, String> {
    let roots = lazer_roots(&app);
    let report = tauri::async_runtime::spawn_blocking(move || delete_paths(&roots, &paths))
        .await
        .map_err(|e| crate::errcode::ec1(crate::errcode::DELETE_INTERRUPTED, e))?;
    Ok(report)
}

/// 纯核心（可测）：逐路径守卫 → 形态校验 → 回收站删除；单项失败不中断。
pub fn delete_paths(roots: &[PathBuf], paths: &[String]) -> TrashReport {
    let mut report = TrashReport::default();
    for p in paths {
        let path = Path::new(p);
        if let Some(msg) = lazer_guard(path, roots) {
            report.failed.push(FailedPath {
                path: p.clone(),
                error: msg,
            });
            continue;
        }
        if !path.exists() {
            report.failed.push(FailedPath {
                path: p.clone(),
                error: crate::errcode::PATH_MISSING.into(),
            });
            continue;
        }
        if !shape_ok(path) {
            report.failed.push(FailedPath {
                path: p.clone(),
                error: SHAPE_MSG.into(),
            });
            continue;
        }
        match trash::delete(path) {
            Ok(()) => report.deleted.push(p.clone()),
            Err(e) => report.failed.push(FailedPath {
                path: p.clone(),
                error: e.to_string(),
            }),
        }
    }
    report
}

#[tauri::command]
pub async fn move_sets(
    app: tauri::AppHandle,
    paths: Vec<String>,
    target_dir: String,
) -> Result<MoveReport, String> {
    let roots = lazer_roots(&app);
    let report =
        tauri::async_runtime::spawn_blocking(move || move_paths(&roots, &paths, &target_dir))
            .await
            .map_err(|e| crate::errcode::ec1(crate::errcode::MOVE_INTERRUPTED, e))?;
    Ok(report)
}

/// 纯核心：复制成功后才回收原件；目标名冲突自动加后缀。
pub fn move_paths(roots: &[PathBuf], paths: &[String], target_dir: &str) -> MoveReport {
    let mut report = MoveReport::default();
    let target = Path::new(target_dir);
    if let Some(msg) = lazer_guard(target, roots) {
        // 目标位于 lazer 根内：整体拒绝
        for p in paths {
            report.failed.push(FailedPath {
                path: p.clone(),
                error: msg.clone(),
            });
        }
        return report;
    }
    if let Err(e) = fs::create_dir_all(target) {
        for p in paths {
            report.failed.push(FailedPath {
                path: p.clone(),
                error: crate::errcode::ec1(crate::errcode::TARGET_DIR_UNAVAILABLE, &e),
            });
        }
        return report;
    }
    for p in paths {
        let src = Path::new(p);
        if let Some(msg) = lazer_guard(src, roots) {
            report.failed.push(FailedPath {
                path: p.clone(),
                error: msg,
            });
            continue;
        }
        if !src.exists() {
            report.failed.push(FailedPath {
                path: p.clone(),
                error: crate::errcode::PATH_MISSING.into(),
            });
            continue;
        }
        if !shape_ok(src) {
            report.failed.push(FailedPath {
                path: p.clone(),
                error: SHAPE_MSG.into(),
            });
            continue;
        }
        let name = match src.file_name() {
            Some(n) => n.to_string_lossy().into_owned(),
            None => {
                report.failed.push(FailedPath {
                    path: p.clone(),
                    error: crate::errcode::BAD_PATH_NAME.into(),
                });
                continue;
            }
        };
        let is_file = src.is_file();
        let (dst, _renamed) = unique_name(target, &name, is_file);
        if let Err(e) = copy_tree(src, &dst) {
            report.failed.push(FailedPath {
                path: p.clone(),
                error: e,
            });
            continue; // 复制未完全成功 → 原件保留
        }
        match trash::delete(src) {
            Ok(()) => report.moved.push(dst.to_string_lossy().into_owned()),
            Err(e) => report.failed.push(FailedPath {
                path: p.clone(),
                error: crate::errcode::ec2(crate::errcode::COPIED_TRASH_FAILED, dst.display(), e),
            }),
        }
    }
    report
}

#[tauri::command]
pub async fn pack_archives(
    app: tauri::AppHandle,
    paths: Vec<String>,
    out_file: String,
) -> Result<PackReport, String> {
    let roots = lazer_roots(&app);
    let result =
        tauri::async_runtime::spawn_blocking(move || pack_paths(&roots, &paths, &out_file))
            .await
            .map_err(|e| crate::errcode::ec1(crate::errcode::PACK_INTERRUPTED, e))?;
    result
}

/// 纯核心：把集文件夹/.osz 打包为 zip。lazer 路径允许（只读源）。
/// 单文件读错 → skipped；源不存在 → skipped；2GB 上限 → 整体 Err。
pub fn pack_paths(
    _roots: &[PathBuf],
    paths: &[String],
    out_file: &str,
) -> Result<PackReport, String> {
    let out = Path::new(out_file);
    out.parent()
        .filter(|p| p.is_dir())
        .ok_or_else(|| crate::errcode::ec(crate::errcode::OUT_PARENT_MISSING))?;

    // 预聚合大小（2GB 上限）
    let mut plan: Vec<(PathBuf, String /*顶层名*/, u64)> = Vec::new();
    let mut used_names: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut total_bytes = 0u64;
    for p in paths {
        let src = Path::new(p);
        if !src.exists() || !shape_ok(src) {
            continue; // 打包不报错，只跳过
        }
        let size = walk_size(src);
        if total_bytes.saturating_add(size) > PACK_LIMIT_BYTES {
            return Err(crate::errcode::ec1(
                crate::errcode::PACK_TOO_BIG,
                total_bytes,
            ));
        }
        total_bytes += size;
        let raw_name = src
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "archive".into());
        let mut final_name = raw_name.clone();
        let mut n = 1u32;
        while !used_names.insert(final_name.clone()) {
            n += 1;
            final_name = split_ext(&raw_name)
                .map(|(stem, ext)| format!("{stem} ({n}).{ext}"))
                .unwrap_or_else(|| format!("{raw_name} ({n})"));
        }
        plan.push((src.to_path_buf(), final_name, size));
    }

    let mut report = PackReport::default();
    let file = fs::File::create(out)
        .map_err(|e| crate::errcode::ec1(crate::errcode::CREATE_ARCHIVE_FAILED, e))?;
    let mut zw = zip::ZipWriter::new(file);
    let opts: zip::write::SimpleFileOptions = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated)
        .compression_level(Some(6));

    for (src, entry_root, _size) in &plan {
        // 输出文件选在源目录内时会自我吞入（读正在写的 zip 自身），跳过
        if src == out {
            continue;
        }
        if src.is_file() {
            // .osz 原样收纳（文件名字节为顶层条目）
            match (fs::read(src), zw.start_file(entry_root.clone(), opts)) {
                (Ok(bytes), Ok(())) => {
                    if zw.write_all(&bytes).is_ok() {
                        report.packed += 1;
                    } else {
                        report.skipped.push(src.to_string_lossy().into_owned());
                    }
                }
                _ => report.skipped.push(src.to_string_lossy().into_owned()),
            }
        } else {
            let mut files: Vec<PathBuf> = Vec::new();
            collect_files(src, &mut files);
            files.sort();
            files.retain(|f| f != out);
            let mut counted_any = false;
            for f in files {
                let rel = match f.strip_prefix(src) {
                    Ok(r) => r.to_string_lossy().replace('\\', "/"),
                    Err(_) => continue,
                };
                let full = format!("{entry_root}/{rel}");
                match (fs::read(&f), zw.start_file(full, opts)) {
                    (Ok(bytes), Ok(())) => {
                        if zw.write_all(&bytes).is_ok() {
                            counted_any = true;
                        } else {
                            report.skipped.push(f.to_string_lossy().into_owned());
                        }
                    }
                    _ => report.skipped.push(f.to_string_lossy().into_owned()),
                }
            }
            if counted_any {
                report.packed += 1;
            }
        }
    }
    zw.finish()
        .map_err(|e| crate::errcode::ec1(crate::errcode::ARCHIVE_FINISH_FAILED, e))?;
    report.bytes = fs::metadata(out).map(|m| m.len()).unwrap_or(0);
    Ok(report)
}

fn collect_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            collect_files(&p, out);
        } else {
            out.push(p);
        }
    }
}

#[tauri::command]
pub fn scan_empty_folders(root: String) -> Result<Vec<String>, String> {
    let base = Path::new(&root);
    if !base.is_dir() {
        return Err(crate::errcode::ec1(crate::errcode::DIR_NOT_FOUND, root));
    }
    if base.join("client.realm").is_file() {
        return Err(LAZER_READONLY_MSG.to_string());
    }
    let mut out = Vec::new();
    let rd = fs::read_dir(base).map_err(|e| e.to_string())?;
    let mut subs: Vec<PathBuf> = rd
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    subs.sort();
    for sub in subs {
        if !has_osu_recursive(&sub) {
            out.push(sub.to_string_lossy().into_owned());
        }
    }
    Ok(out)
}

fn has_osu_recursive(dir: &Path) -> bool {
    let Ok(rd) = fs::read_dir(dir) else {
        return false;
    };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_file()
            && p.extension()
                .map(|x| x.eq_ignore_ascii_case("osu"))
                .unwrap_or(false)
        {
            return true;
        }
        if p.is_dir() && has_osu_recursive(&p) {
            return true;
        }
    }
    false
}

// ── 测试 ──────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;
    use std::time::{SystemTime, UNIX_EPOCH};

    struct TempDir(PathBuf);
    impl TempDir {
        fn new(tag: &str) -> Self {
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let root =
                std::env::temp_dir().join(format!("osm_m6_{tag}_{}_{nanos}", std::process::id()));
            fs::create_dir_all(&root).unwrap();
            Self(root)
        }
    }
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn osu(title: &str, set_id: i64, version: &str) -> String {
        format!(
            "osu file format v14\n\n[General]\nAudioFilename: audio.mp3\n\n[Metadata]\nTitle:{title}\nArtist:A\nCreator:C\nVersion:{version}\nBeatmapSetID:{set_id}\n\n[Difficulty]\nCircleSize:4\n\n[TimingPoints]\n0,500,4,2,1,40,1,0\n\n[HitObjects]\n256,192,1000,1,0\n"
        )
    }

    fn make_set(songs: &Path, folder: &str, title: &str, set_id: i64, diffs: &[&str]) -> PathBuf {
        let dir = songs.join(folder);
        fs::create_dir_all(&dir).unwrap();
        for v in diffs {
            fs::write(
                dir.join(format!("{title} [{v}].osu")),
                osu(title, set_id, v),
            )
            .unwrap();
        }
        dir
    }

    fn fake_lazer_root() -> TempDir {
        let t = TempDir::new("lazer");
        fs::write(t.0.join("client.realm"), b"realm").unwrap();
        fs::create_dir_all(t.0.join("files")).unwrap();
        t
    }

    #[test]
    fn lazer_guard_rejects_inside_and_allows_outside() {
        let lz = fake_lazer_root();
        let roots = vec![lz.0.clone()];
        assert!(lazer_guard(&lz.0.join("Songs").join("x"), &roots).is_some());
        assert!(lazer_guard(&lz.0, &roots).is_some());
        let other = TempDir::new("outside");
        assert!(lazer_guard(&other.0, &roots).is_none());
    }

    #[test]
    fn delete_rejects_lazer_missing_and_wrong_shape() {
        let lz = fake_lazer_root();
        let inside = lz.0.join("beatmap");
        fs::create_dir_all(&inside).unwrap();
        fs::write(inside.join("a.osu"), osu("A", 1, "x")).unwrap();
        let tmp = TempDir::new("del");

        let roots = vec![lz.0.clone()];
        // lazer 内路径 → 拒绝且原样保留
        let rep = delete_paths(&roots, &[inside.to_string_lossy().into_owned()]);
        assert!(rep.deleted.is_empty());
        assert_eq!(rep.failed[0].error, LAZER_READONLY_MSG);
        assert!(inside.exists());

        // 不存在 → 拒绝
        let rep = delete_paths(
            &roots,
            &[tmp.0.join("ghost").to_string_lossy().into_owned()],
        );
        assert_eq!(rep.failed[0].error, crate::errcode::PATH_MISSING);

        // 随机 .txt → 形态拒绝
        let txt = tmp.0.join("note.txt");
        fs::write(&txt, b"hi").unwrap();
        let rep = delete_paths(&roots, &[txt.to_string_lossy().into_owned()]);
        assert_eq!(rep.failed[0].error, SHAPE_MSG);

        // 合法 stable 集文件夹 → 回收站删除
        let dir = make_set(&tmp.0, "Real Set [1]", "Real", 1, &["Insane"]);
        let rep = delete_paths(&roots, &[dir.to_string_lossy().into_owned()]);
        assert_eq!(rep.deleted.len(), 1, "{rep:?}");
        assert!(!dir.exists(), "已移入回收站后路径不应存在");
    }

    #[test]
    fn duplicates_group_all_three_reasons() {
        let songs = TempDir::new("dups");
        // 两份相同 OnlineID + 相同内容的克隆 → setOnlineId + md5 + identicalSet 都触发条件
        make_set(&songs.0, "Hollow A", "Hollow", 55, &["Easy", "Hard"]);
        make_set(&songs.0, "Hollow B", "Hollow", 55, &["Easy", "Hard"]);
        // 独立集
        make_set(&songs.0, "Other", "Other", 9, &["Normal"]);
        // 本地导入克隆（OnlineID=-1 → scan 得 -1？stable 用 BeatmapSetID；给 -1 两目录）
        make_set(&songs.0, "Local A", "Local", -1, &["Extra"]);
        make_set(&songs.0, "Local B", "Local", -1, &["Extra"]);

        let sets = scan_quiet(SourceKind::Stable, &songs.0.to_string_lossy()).unwrap();
        assert_eq!(sets.len(), 5);
        let groups = duplicate_groups(&sets, &|_| 128);

        let online = groups
            .iter()
            .find(|g| g.reason == "setOnlineId" && g.label.contains("55"));
        assert!(online.is_some(), "setOnlineId 组缺失: {groups:?}");
        assert_eq!(online.unwrap().members.len(), 2);
        assert!(online.unwrap().members.iter().all(|m| m.size_bytes == 128));

        // md5 跨集：Hollow Easy/Hard 各成一组（内容相同 → 同 md5，两集）
        let md5_groups: Vec<_> = groups.iter().filter(|g| g.reason == "md5").collect();
        assert!(!md5_groups.is_empty(), "md5 组缺失");
        assert!(md5_groups.iter().all(|g| g.members.len() >= 2));

        // identicalSet：本地 -1 克隆
        let ident = groups.iter().find(|g| g.reason == "identicalSet");
        assert!(ident.is_some(), "identicalSet 组缺失");
        assert_eq!(ident.unwrap().members.len(), 2);

        // 排序：成员数降序 + label
        for w in groups.windows(2) {
            assert!(
                w[0].members.len() > w[1].members.len()
                    || (w[0].members.len() == w[1].members.len() && w[0].label <= w[1].label)
            );
        }
    }

    #[test]
    fn move_copy_then_trash_with_collision() {
        let lz = fake_lazer_root();
        let roots = vec![lz.0.clone()];
        let songs = TempDir::new("movesrc");
        let target = TempDir::new("movetgt");
        let a = make_set(&songs.0, "Move Me", "MoveMe", 7, &["Hard"]);
        // 先放一个同名目标制造冲突
        make_set(&target.0, "Move Me", "Placeholder", 7, &["Hard"]);

        let rep = move_paths(
            &roots,
            &[a.to_string_lossy().into_owned()],
            &target.0.to_string_lossy(),
        );
        assert_eq!(rep.moved.len(), 1, "{rep:?}");
        assert!(rep.moved[0].contains("Move Me (2)"), "{}", rep.moved[0]);
        assert!(!a.exists(), "复制成功后原件应已入回收站");
        assert!(target
            .0
            .join("Move Me (2)")
            .join("MoveMe [Hard].osu")
            .is_file());
        assert!(
            target
                .0
                .join("Move Me")
                .join("Placeholder [Hard].osu")
                .is_file(),
            "原目标不被覆盖"
        );

        // lazer 目标目录 → 整体拒绝
        let rep = move_paths(
            &roots,
            &[a.to_string_lossy().into_owned()],
            &lz.0.to_string_lossy(),
        );
        assert!(rep.moved.is_empty());
        assert_eq!(rep.failed[0].error, LAZER_READONLY_MSG);
    }

    #[test]
    fn pack_zip_contains_all_sources() {
        let songs = TempDir::new("packsrc");
        let make_osz = |dir: &Path, name: &str| {
            let f = fs::File::create(dir.join(name)).unwrap();
            let mut zw = zip::ZipWriter::new(f);
            let opts = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated);
            zw.start_file("Inner Song [Single].osu", opts).unwrap();
            zw.write_all(osu("Inner", 42, "Single").as_bytes()).unwrap();
            zw.finish().unwrap();
        };
        let s1 = make_set(&songs.0, "Pack One", "PackOne", 1, &["A"]);
        let s2 = make_set(&songs.0, "Pack Two", "PackTwo", 2, &["B"]);
        make_osz(&songs.0, "inner.osz");
        let out = TempDir::new("packout");
        let outfile = out.0.join("sets.zip");

        let paths: Vec<String> = [s1, s2, songs.0.join("inner.osz")]
            .iter()
            .map(|p| p.to_string_lossy().into_owned())
            .collect();
        let rep = pack_paths(&[], &paths, &outfile.to_string_lossy()).unwrap();
        assert_eq!(rep.packed, 3, "{rep:?}");
        assert!(rep.skipped.is_empty());
        assert!(rep.bytes > 0);

        let zf = fs::File::open(&outfile).unwrap();
        let mut za = zip::ZipArchive::new(zf).unwrap();
        let names: Vec<String> = (0..za.len())
            .map(|i| za.by_index(i).unwrap().name().to_string())
            .collect();
        assert!(
            names.iter().any(|n| n == "Pack One/PackOne [A].osu"),
            "{names:?}"
        );
        assert!(names.iter().any(|n| n == "Pack Two/PackTwo [B].osu"));
        assert!(names.iter().any(|n| n == "inner.osz"));
        // 内容可读回
        let mut f = za.by_name("Pack One/PackOne [A].osu").unwrap();
        let mut s = String::new();
        f.read_to_string(&mut s).unwrap();
        assert!(s.contains("[HitObjects]"));
    }

    #[test]
    fn pack_dedupes_colliding_entry_names() {
        let songs = TempDir::new("packdup");
        let a = make_set(&songs.0, "Same Name", "First", 1, &["X"]);
        fs::create_dir_all(songs.0.join("nested").join("Same Name")).unwrap();
        let b_dir = songs.0.join("nested").join("Same Name");
        fs::write(b_dir.join("Y.osu"), osu("Second", 2, "Y")).unwrap();
        let out = TempDir::new("packdupout");
        let rep = pack_paths(
            &[],
            &[
                a.to_string_lossy().into_owned(),
                b_dir.to_string_lossy().into_owned(),
            ],
            &out.0.join("o.zip").to_string_lossy(),
        )
        .unwrap();
        assert_eq!(rep.packed, 2);
        let zf = fs::File::open(out.0.join("o.zip")).unwrap();
        let mut za = zip::ZipArchive::new(zf).unwrap();
        let names: Vec<String> = (0..za.len())
            .map(|i| za.by_index(i).unwrap().name().to_string())
            .collect();
        assert!(names.iter().any(|n| n.starts_with("Same Name/")));
        assert!(
            names.iter().any(|n| n.starts_with("Same Name (2)/")),
            "{names:?}"
        );
    }

    #[test]
    fn empty_folders_scan() {
        let root = TempDir::new("empty");
        fs::create_dir_all(root.0.join("no map here")).unwrap();
        fs::write(root.0.join("no map here").join("readme.txt"), b"x").unwrap();
        let good = make_set(&root.0, "has map", "T", 5, &["D"]);
        let deep = root.0.join("deep one").join("inner");
        fs::create_dir_all(&deep).unwrap();
        fs::write(deep.join("Z.osu"), osu("Z", 6, "z")).unwrap();

        let empties = scan_empty_folders(root.0.to_string_lossy().into_owned()).unwrap();
        assert_eq!(empties.len(), 1);
        assert!(empties[0].ends_with("no map here"));
        assert!(good.exists());

        // lazer 根（含 client.realm）→ 拒绝
        let lz = fake_lazer_root();
        let err = scan_empty_folders(lz.0.to_string_lossy().into_owned()).unwrap_err();
        assert_eq!(err, LAZER_READONLY_MSG);
    }

    #[test]
    fn stable_scan_now_populates_md5() {
        let songs = TempDir::new("md5fill");
        make_set(&songs.0, "Md5 Set", "Md5", 3, &["One"]);
        let sets = scan_quiet(SourceKind::Stable, &songs.0.to_string_lossy()).unwrap();
        let md5 = sets[0].difficulties[0].md5.clone();
        assert!(
            md5.as_deref()
                .map(|h| h.len() == 32 && h.bytes().all(|b| b.is_ascii_hexdigit()))
                .unwrap_or(false),
            "{md5:?}"
        );
    }
}
