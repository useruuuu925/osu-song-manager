// thumbs.rs
//
// Purpose: M5 缩略图缓存管线 + 背景图批量导出。
// - 缩略图：源文件是内容寻址的无扩展名文件（lazer files\ 或 stable 图），按魔数自动嗅探；
//   解码 → RGB8 → thumbnail → JPEG q80 → 原子写入 <app_data>/thumbs/<key>.jpg。
//   前端用自定义协议 thumb 取图：http://thumb.localhost/<key>（或 thumb://<key>）。
//   源文件全程只读，任何写入都限定在 app_data_dir/thumbs 与用户选择的导出目录。
// - 缓存键：sha256(source_path 小写) 的十六进制（64 hex）拼 `_{size}.jpg`，跨进程稳定。
// - 导出：文件名 "{artist} - {title} [{creator}]" 经 Windows 安全化，扩展名按源文件魔数，
//   同名冲突追加 " (2)"、" (3)"…（保证 100 次导出不覆盖）。

use crate::model::{ExportReport, ExportSet, ThumbRequest, ThumbResult};
use crate::online::app_dir;
use image::ImageEncoder as _;
use sha2::{Digest, Sha256};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

pub const MAX_THUMB_BATCH: usize = 500;
pub const DEFAULT_JPEG_QUALITY: u8 = 80;

/// app_data_dir/thumbs —— 所有缩略图与协议读文件的唯一根目录。
pub fn cache_dir(app: &tauri::AppHandle) -> PathBuf {
    app_dir(app).join("thumbs")
}

/// 稳定缓存键：sha256(小写路径) hex64 + "_{w}x{h}.jpg"。
/// 缩略图为 16:9 cover 裁剪（w 由请求方给定，h = w*9/16）。
pub fn thumb_key(source_path: &str, size: u32) -> String {
    let mut h = Sha256::new();
    h.update(source_path.to_lowercase().as_bytes());
    let digest = h.finalize();
    let mut hex = String::with_capacity(64);
    for b in digest.iter() {
        hex.push_str(&format!("{b:02x}"));
    }
    format!("{hex}_{}x{}.jpg", size, size * 9 / 16)
}

/// 在线背景的缓存键：web{setId}_{w}x{h}.jpg（与本地源键空间互不相交）。
pub fn online_thumb_key(set_id: i64, size: u32) -> String {
    format!("web{set_id}_{}x{}.jpg", size, size * 9 / 16)
}

/// 严格校验缓存键：^[0-9a-f]{16,64}|web[0-9]{1,12}_[0-9]{1,5}x[0-9]{1,5}\.jpg$
/// （兼容旧键形态 `_{size}.jpg`）。任何路径分隔符 / 目录穿越 / 非法字符都必须为 false。
pub fn is_valid_thumb_key(key: &str) -> bool {
    // 快速拒绝任何分隔符或上级目录（双保险）
    if key.contains('/') || key.contains('\\') || key.contains('.') && !key.ends_with(".jpg") {
        return false;
    }
    let Some(dot) = key.rfind('.') else {
        return false;
    };
    if &key[dot..] != ".jpg" {
        return false;
    }
    let stem = &key[..dot]; // 需为 hex_size 或 web{id}_size
    let Some(under) = stem.rfind('_') else {
        return false;
    };
    let (id_part, size) = (&stem[..under], &stem[under + 1..]);
    let hex_ok = if let Some(digits) = id_part.strip_prefix("web") {
        // 在线键：web{setId}，1-12 位数字
        !digits.is_empty() && digits.len() <= 12 && digits.bytes().all(|b| b.is_ascii_digit())
    } else {
        (16..=64).contains(&id_part.len())
            && id_part
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    };
    let size_ok = match size.split_once('x') {
        // 新形态 {w}x{h}
        Some((w, h)) => {
            (1..=5).contains(&w.len())
                && (1..=5).contains(&h.len())
                && w.bytes().all(|b| b.is_ascii_digit())
                && h.bytes().all(|b| b.is_ascii_digit())
        }
        // 旧形态 {size}
        None => (1..=5).contains(&size.len()) && size.bytes().all(|b| b.is_ascii_digit()),
    };
    hex_ok && size_ok
}

/// 从自定义协议 URI 提取键名。支持 http://thumb.localhost/<key> 与 thumb://<key> 两种形态。
pub fn key_from_uri(uri: &str) -> Option<String> {
    // 去掉查询串与片段
    let s = uri.split(['?', '#']).next().unwrap_or("");
    // 取 scheme:// 之后的整体
    let after = if let Some(pos) = s.find("://") {
        &s[pos + 3..]
    } else {
        s
    };
    // host 段与 key 之间以最后一个 '/' 分隔；key 本身不含 '/'
    let candidate = match after.rfind('/') {
        Some(p) => &after[p + 1..],
        None => after,
    };
    let candidate = percent_decode(candidate);
    if is_valid_thumb_key(&candidate) {
        Some(candidate)
    } else {
        None
    }
}

/// 仅对 key 字符集（[0-9a-f_.]）恒等的轻量百分号解码（防御性，非法编码原样返回）。
fn percent_decode(input: &str) -> String {
    let b = input.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let (Some(h), Some(l)) = (
                (b[i + 1] as char).to_digit(16),
                (b[i + 2] as char).to_digit(16),
            ) {
                out.push((h * 16 + l) as u8);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// 自定义协议处理器：校验 key → 仅从 thumbs 目录读文件 → 200 + image/jpeg，否则 404。
/// 由 lib.rs 的 register_uri_scheme_protocol("thumb", …) 调用。
pub fn serve(app: &tauri::AppHandle, uri: &str) -> tauri::http::Response<Vec<u8>> {
    let not_found = || {
        tauri::http::Response::builder()
            .status(404)
            .body(Vec::new())
            .unwrap()
    };
    let Some(key) = key_from_uri(uri) else {
        return not_found();
    };
    let path = cache_dir(app).join(key);
    match fs::read(&path) {
        Ok(bytes) if !bytes.is_empty() => tauri::http::Response::builder()
            .status(200)
            .header("content-type", "image/jpeg")
            .header("cache-control", "public, max-age=31536000, immutable")
            .body(bytes)
            .unwrap(),
        _ => not_found(),
    }
}

/// 把 RGB 图做 16:9 cover 裁剪（等比缩放至覆盖 w×h，再中心裁剪），返回 w×h 图。
/// 背景图以 16:9 展示，源方形/竖图裁上下比左右损失小。
fn cover_16_9(rgb: &image::RgbImage, w: u32, h: u32) -> image::RgbImage {
    let (iw, ih) = (rgb.width().max(1), rgb.height().max(1));
    let scale = (w as f64 / iw as f64).max(h as f64 / ih as f64);
    let nw = ((iw as f64 * scale).ceil() as u32).max(w).max(1);
    let nh = ((ih as f64 * scale).ceil() as u32).max(h).max(1);
    let scaled = image::imageops::resize(rgb, nw, nh, image::imageops::FilterType::Triangle);
    let x = (nw - w) / 2;
    let y = (nh - h) / 2;
    image::imageops::crop_imm(&scaled, x, y, w, h).to_image()
}

/// 由源字节生成 16:9 缩略图并原子写入 dest。失败返回 Err（调用方逐项跳过，绝不 panic）。
fn generate_thumb(source_path: &str, size: u32, dest: &Path) -> Result<(), String> {
    use crate::errcode;
    let bytes = fs::read(source_path).map_err(|e| errcode::ec1(errcode::THUMB_READ_FAILED, e))?;
    let img = image::load_from_memory(&bytes)
        .map_err(|e| errcode::ec1(errcode::THUMB_DECODE_FAILED, e))?;
    encode_thumb(&img.into_rgb8(), size, dest)
}

/// RGB 图 → w×(w*9/16) JPEG → 原子写入 dest。
fn encode_thumb(rgb: &image::RgbImage, size: u32, dest: &Path) -> Result<(), String> {
    use crate::errcode;
    let size = size.clamp(1, 4096);
    let h = size * 9 / 16;
    let thumb = cover_16_9(rgb, size, h.max(1));
    let mut buf: Vec<u8> = Vec::new();
    let enc = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buf, DEFAULT_JPEG_QUALITY);
    enc.write_image(
        thumb.as_raw(),
        thumb.width(),
        thumb.height(),
        image::ExtendedColorType::Rgb8,
    )
    .map_err(|e| errcode::ec1(errcode::THUMB_JPEG_FAILED, e))?;
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let tmp = dest.with_extension("jpg.tmpwrite");
    {
        let mut f = fs::File::create(&tmp).map_err(|e| e.to_string())?;
        f.write_all(&buf).map_err(|e| e.to_string())?;
        f.flush().ok();
    }
    let _ = fs::remove_file(dest);
    fs::rename(&tmp, dest).map_err(|e| e.to_string())
}

/// 纯函数核心（无 AppHandle 依赖，便于测试）。输入顺序保留，按 key 去重，命中缓存即跳过。
/// emit_progress(done,total) 每处理一项回调一次。
pub fn prepare_blocking(
    thumbs: &Path,
    requests: &[ThumbRequest],
    mut emit_progress: impl FnMut(u32, u32),
) -> Vec<ThumbResult> {
    let total = requests.len() as u32;
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut out = Vec::with_capacity(requests.len());
    for (i, r) in requests.iter().enumerate() {
        let size = r.size.clamp(1, 4096);
        let key = thumb_key(&r.source_path, size);
        let dest = thumbs.join(&key);
        let done = i as u32 + 1;
        // 已缓存（文件存在且非空）→ 直接命中
        let cached_ok = fs::metadata(&dest).map(|m| m.len() > 0).unwrap_or(false);
        let ok = if cached_ok {
            true
        } else if !seen.insert(key.clone()) {
            // 同批次重复项，前面已生成
            fs::metadata(&dest).map(|m| m.len() > 0).unwrap_or(false)
        } else {
            generate_thumb(&r.source_path, size, &dest).is_ok()
        };
        out.push(ThumbResult {
            set_id: r.set_id,
            key: if ok { Some(key) } else { None },
            ok,
        });
        emit_progress(done, total);
    }
    out
}

#[tauri::command]
pub async fn prepare_thumbnails(
    app: tauri::AppHandle,
    requests: Vec<ThumbRequest>,
) -> Result<Vec<ThumbResult>, String> {
    if requests.len() > MAX_THUMB_BATCH {
        return Err(crate::errcode::ec2(
            crate::errcode::THUMB_BATCH_TOO_LARGE,
            MAX_THUMB_BATCH,
            requests.len(),
        ));
    }
    let thumbs = cache_dir(&app);
    let results = tauri::async_runtime::spawn_blocking(move || {
        use tauri::Emitter;
        prepare_blocking(&thumbs, &requests, |done, total| {
            let _ = app.emit("thumb-progress", (done, total));
        })
    })
    .await
    .map_err(|e| crate::errcode::ec1(crate::errcode::THUMB_INTERRUPTED, e))?;
    Ok(results)
}

/// osu! 官方素材 CDN：谱面集原始背景图（公开、免登录）。
const ONLINE_BG_URL: &str = "https://assets.ppy.sh/beatmaps/{id}/covers/raw.jpg";
/// 单图下载上限（raw.jpg 实测多在 1 MB 内，留足余量）。
const ONLINE_BG_MAX_BYTES: usize = 30 * 1024 * 1024;
/// 共享 HTTP 客户端：连接复用免每次 TLS 握手。
static ONLINE_CLIENT: std::sync::OnceLock<reqwest::blocking::Client> = std::sync::OnceLock::new();

fn online_client() -> &'static reqwest::blocking::Client {
    ONLINE_CLIENT.get_or_init(|| {
        reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(20))
            .user_agent("osu-song-manager/0.1 (background thumbnail fetch)")
            .build()
            .expect("http client")
    })
}

/// 启动清扫：删除「键形态已废弃」的旧缩略图缓存（256 方形时代的 _{size}.jpg，
/// 新键为 _{w}x{h}.jpg）与残留的 .tmpwrite 临时文件。仅在启动时跑一次。
pub fn prune_stale_thumbs(dir: &Path) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        let stale_tmp = name.ends_with(".tmpwrite");
        let stale_square = {
            // 旧形态：{hex}_{digits}.jpg（尺寸段无 'x'）；只清 hex 键，不碰 web{setId} 键
            name.ends_with(".jpg")
                && !name.contains('x')
                && name.split('_').count() == 2
                && name.split('_').next().is_some_and(|hex| {
                    (16..=64).contains(&hex.len())
                        && hex
                            .bytes()
                            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
                })
                && name
                    .split('_')
                    .nth(1)
                    .and_then(|s| s.strip_suffix(".jpg"))
                    .is_some_and(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()))
        };
        if stale_tmp || stale_square {
            let _ = fs::remove_file(entry.path());
        }
    }
}

/// 从 osu! 官方 CDN 拉取谱面集背景图并按本地缩略图管线入缓存。
/// 命中缓存直接返回；下载/解码失败一律 Ok(None)（由前端决定占位呈现）。
#[tauri::command]
pub async fn fetch_online_background(
    app: tauri::AppHandle,
    set_id: i64,
    size: u32,
) -> Result<Option<ThumbResult>, String> {
    if set_id <= 0 {
        return Ok(None);
    }
    let size = size.clamp(64, 1024);
    let key = online_thumb_key(set_id, size);
    let thumbs = cache_dir(&app);
    let cached_ok = fs::metadata(thumbs.join(&key))
        .map(|m| m.len() > 0)
        .unwrap_or(false);
    if cached_ok {
        return Ok(Some(ThumbResult {
            set_id,
            key: Some(key),
            ok: true,
        }));
    }
    let key_task = key.clone();
    let result = tauri::async_runtime::spawn_blocking(move || -> Option<()> {
        let url = ONLINE_BG_URL.replace("{id}", &set_id.to_string());
        let resp = online_client().get(url).send().ok()?;
        if !resp.status().is_success() {
            return None;
        }
        let ctype = resp
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();
        if !ctype.starts_with("image/") {
            return None;
        }
        let bytes = resp.bytes().ok()?;
        if bytes.len() > ONLINE_BG_MAX_BYTES || bytes.is_empty() {
            return None;
        }
        let img = image::load_from_memory(&bytes).ok()?;
        let dest = thumbs.join(&key_task);
        encode_thumb(&img.into_rgb8(), size, &dest).ok()
    })
    .await
    .unwrap_or(None);
    Ok(result.map(|_| ThumbResult {
        set_id,
        key: Some(key),
        ok: true,
    }))
}

// ── 导出 ──────────────────────────────────────────────────────────────────────

/// Windows 安全文件名：剥离 <>:"/\|?* 与控制字符，裁剪结尾点/空格，空→untitled。
pub fn sanitize_filename(name: &str) -> String {
    let mut s: String = name
        .chars()
        .filter(|c| !c.is_control())
        .map(|c| match c {
            '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*' => '_',
            other => other,
        })
        .collect();
    while s.ends_with('.') || s.ends_with(' ') {
        s.pop();
    }
    let s = s.trim_matches(' ').to_string();
    if s.is_empty() {
        "untitled".to_string()
    } else {
        s
    }
}

/// 按源文件魔数决定扩展名（无点）。
pub fn ext_from_magic(bytes: &[u8]) -> &'static str {
    if bytes.len() >= 3 && bytes[0] == 0xFF && bytes[1] == 0xD8 && bytes[2] == 0xFF {
        "jpg"
    } else if bytes.len() >= 4
        && bytes[0] == 0x89
        && bytes[1] == 0x50
        && bytes[2] == 0x4E
        && bytes[3] == 0x47
    {
        "png"
    } else if bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        "webp"
    } else {
        "img"
    }
}

/// 冲突改名：base.ext → base (2).ext → base (3).ext …（磁盘上已存在则递增；从 (2) 起，与 M5 契约一致）。
pub fn unique_path(dir: &Path, base: &str, ext: &str) -> (PathBuf, u32) {
    let mut n = 0u32;
    loop {
        let filename = if n == 0 {
            format!("{base}.{ext}")
        } else {
            format!("{base} ({}).{ext}", n + 1)
        };
        let p = dir.join(filename);
        if !p.exists() {
            return (p, n);
        }
        n += 1;
    }
}

/// 纯导出核心（便于测试）。emit_progress(done,total,current)。
pub fn export_blocking(
    target: &Path,
    sets: &[ExportSet],
    mut emit_progress: impl FnMut(u32, u32, &str),
) -> ExportReport {
    let total = sets.len() as u32;
    let mut report = ExportReport::default();
    let _ = fs::create_dir_all(target);
    for (i, s) in sets.iter().enumerate() {
        let base = sanitize_filename(&format!("{} - {} [{}]", s.artist, s.title, s.creator));
        emit_progress(i as u32, total, &base);
        let bytes = match fs::read(&s.source_path) {
            Ok(b) => b,
            Err(_) => {
                report.failed += 1;
                report.errors.push(crate::errcode::ec1(
                    crate::errcode::EXPORT_SOURCE_MISSING,
                    s.set_id,
                ));
                continue;
            }
        };
        let ext = ext_from_magic(&bytes);
        let (path, renamed) = unique_path(target, &base, ext);
        match fs::write(&path, &bytes) {
            Ok(()) => {
                report.exported += 1;
                if renamed > 0 {
                    report.renamed += 1;
                }
            }
            Err(e) => {
                report.failed += 1;
                report.errors.push(format!("{}: {e}", s.set_id));
            }
        }
        emit_progress(i as u32 + 1, total, &base);
    }
    report
}

#[tauri::command]
pub async fn pick_export_folder() -> Result<Option<String>, String> {
    let picked = tauri::async_runtime::spawn_blocking(|| {
        rfd::FileDialog::new()
            .set_title("osu-song-manager")
            .pick_folder()
    })
    .await
    .map_err(|e| crate::errcode::ec1(crate::errcode::DIALOG_INTERRUPTED, e))?;
    Ok(picked.map(|p| p.to_string_lossy().into_owned()))
}

#[tauri::command]
pub async fn export_backgrounds(
    app: tauri::AppHandle,
    sets: Vec<ExportSet>,
    target_dir: String,
) -> Result<ExportReport, String> {
    let target = PathBuf::from(&target_dir);
    // 与 convert/download 一致：写入目标位于 lazer 数据目录内时整体拒绝
    if let Some(msg) = crate::manage::lazer_guard(&target, &crate::manage::lazer_roots(&app)) {
        return Err(msg);
    }
    let target = PathBuf::from(target_dir);
    use tauri::Emitter;
    let report = tauri::async_runtime::spawn_blocking(move || {
        export_blocking(&target, &sets, |done, total, current| {
            let _ = app.emit(
                "export-progress",
                crate::model::ExportProgress {
                    done,
                    total,
                    current: current.to_string(),
                },
            );
        })
    })
    .await
    .map_err(|e| crate::errcode::ec1(crate::errcode::EXPORT_INTERRUPTED, e))?;
    Ok(report)
}

// ── 测试 ──────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::ThumbRequest;
    use std::time::{SystemTime, UNIX_EPOCH};

    struct TempDir(PathBuf);
    impl TempDir {
        fn new(tag: &str) -> Self {
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let root = std::env::temp_dir()
                .join(format!("osm_thumb_{tag}_{}_{nanos}", std::process::id()));
            fs::create_dir_all(&root).unwrap();
            Self(root)
        }
    }
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn key_valid_rejects_traversal_and_accepts_good() {
        assert!(!is_valid_thumb_key("../../etc/passwd"));
        assert!(!is_valid_thumb_key("deadbeef_deadbeef_256.jpg/../x"));
        assert!(!is_valid_thumb_key("DEADBEEF_deadbeef_256.jpg")); // 大写十六进制
        assert!(!is_valid_thumb_key("abc_256.jpg")); // hex 太短
        assert!(!is_valid_thumb_key("deadbeefdeadbeef_256.png")); // 扩展名
        assert!(!is_valid_thumb_key("deadbeefdeadbeef_.jpg")); // 无尺寸
                                                               // 合规键：sha256 → 64 hex，16:9 尺寸对
        let k = thumb_key("C:\\osu\\files\\a\\ab\\abc", 256);
        assert!(is_valid_thumb_key(&k), "generated key should be valid: {k}");
        assert!(k.ends_with("_256x144.jpg"));
        // 在线键形态
        let wk = online_thumb_key(387700, 256);
        assert!(is_valid_thumb_key(&wk), "online key should be valid: {wk}");
        assert!(wk.starts_with("web387700_"));
        assert!(!is_valid_thumb_key("webX_256x144.jpg")); // id 非数字
    }

    #[test]
    fn key_from_uri_both_forms() {
        let k = thumb_key("C:/x/y", 128);
        assert_eq!(
            key_from_uri(&format!("http://thumb.localhost/{k}")).map(|s| s.to_string()),
            Some(k.clone())
        );
        assert_eq!(
            key_from_uri(&format!("thumb://{k}")).map(|s| s.to_string()),
            Some(k)
        );
        assert!(key_from_uri("http://thumb.localhost/../../secret").is_none());
        assert!(key_from_uri("").is_none());
    }

    #[test]
    fn generator_produces_decodable_jpeg() {
        let tmp = TempDir::new("gen");
        // 造一个 4x4 RGB 源（无扩展名），存成 png 内容
        let src = tmp.0.join("srcfile");
        let img = image::RgbImage::from_pixel(4, 4, image::Rgb([10, 20, 30]));
        img.save_with_format(&src, image::ImageFormat::Png).unwrap(); // 无扩展名 → 显式格式（模拟内容寻址文件）
        let thumbs = tmp.0.join("thumbs");
        let reqs = vec![ThumbRequest {
            set_id: 7,
            source_path: src.to_string_lossy().into_owned(),
            size: 2,
        }];
        let res = prepare_blocking(&thumbs, &reqs, |_, _| {});
        assert_eq!(res.len(), 1);
        assert!(res[0].ok && res[0].key.is_some(), "{res:?}");
        let out = thumbs.join(res[0].key.as_ref().unwrap());
        assert!(out.is_file());
        let back = image::load_from_memory(&fs::read(&out).unwrap()).unwrap();
        assert!(back.width() <= 2 && back.height() <= 2);
        assert_eq!(back.width(), 2); // 4→2 缩放
    }

    #[test]
    fn cache_hit_skips_regen() {
        let tmp = TempDir::new("hit");
        let src = tmp.0.join("s");
        image::RgbImage::from_pixel(3, 3, image::Rgb([1, 2, 3]))
            .save_with_format(&src, image::ImageFormat::Png)
            .unwrap();
        let thumbs = tmp.0.join("t");
        let reqs = vec![ThumbRequest {
            set_id: 1,
            source_path: src.to_string_lossy().into_owned(),
            size: 16,
        }];
        let first = prepare_blocking(&thumbs, &reqs, |_, _| {});
        let key = first[0].key.clone().unwrap();
        // 篡改文件内容，第二次应命中缓存（读到已存在文件，不重新生成 → 内容不变）
        let path = thumbs.join(&key);
        fs::write(&path, b"sentinel").unwrap();
        let second = prepare_blocking(&thumbs, &reqs, |_, _| {});
        assert!(second[0].ok);
        assert_eq!(fs::read(&path).unwrap(), b"sentinel");
    }

    #[test]
    fn sanitizer_windows_cases() {
        assert_eq!(sanitize_filename("a:b/c?*"), "a_b_c__");
        assert_eq!(sanitize_filename("trailing..."), "trailing");
        assert_eq!(sanitize_filename("  spaced  "), "spaced");
        assert_eq!(sanitize_filename(""), "untitled");
        assert_eq!(sanitize_filename("..."), "untitled");
        assert_eq!(sanitize_filename("ok name [x]"), "ok name [x]");
    }

    #[test]
    fn magic_and_collision() {
        assert_eq!(ext_from_magic(&[0xFF, 0xD8, 0xFF, 0x00]), "jpg");
        assert_eq!(ext_from_magic(&[0x89, 0x50, 0x4E, 0x47, 0, 0, 0, 0]), "png");
        let mut webp = b"RIFFxxxxWEBP".to_vec();
        assert_eq!(ext_from_magic(&webp), "webp");
        webp[4] = b'y';
        assert_eq!(ext_from_magic(&[1, 2, 3]), "img");

        let tmp = TempDir::new("col");
        let (p0, n0) = unique_path(&tmp.0, "art - ttl [cr]", "jpg");
        assert_eq!(n0, 0);
        fs::write(&p0, b"x").unwrap();
        let (p1, n1) = unique_path(&tmp.0, "art - ttl [cr]", "jpg");
        assert_eq!(n1, 1);
        assert!(p1.to_string_lossy().ends_with("art - ttl [cr] (2).jpg"));
    }

    #[test]
    fn collision_hundred_exports_never_overwrite() {
        let tmp = TempDir::new("col100");
        let src = tmp.0.join("s100");
        image::RgbImage::from_pixel(2, 2, image::Rgb([9, 9, 9]))
            .save_with_format(&src, image::ImageFormat::Png)
            .unwrap();
        let target = tmp.0.join("out");
        let sets: Vec<ExportSet> = (0..100)
            .map(|i| ExportSet {
                set_id: i,
                artist: "Same".into(),
                title: "Artist".into(),
                creator: "One".into(),
                source_path: src.to_string_lossy().into_owned(),
            })
            .collect();
        let rep = export_blocking(&target, &sets, |_, _, _| {});
        assert_eq!(rep.exported, 100);
        assert_eq!(rep.renamed, 99);
        assert_eq!(rep.failed, 0);
        let files: Vec<_> = fs::read_dir(&target).unwrap().flatten().collect();
        assert_eq!(files.len(), 100, "no overwrite: 100 distinct files");
    }

    #[test]
    fn export_writes_and_reports() {
        let tmp = TempDir::new("exp");
        let src = tmp.0.join("bgsrc");
        image::RgbImage::from_pixel(2, 2, image::Rgb([5, 5, 5]))
            .save_with_format(&src, image::ImageFormat::Png)
            .unwrap();
        let target = tmp.0.join("out");
        let sets = vec![
            ExportSet {
                set_id: 1,
                artist: "A".into(),
                title: "T".into(),
                creator: "C".into(),
                source_path: src.to_string_lossy().into_owned(),
            },
            ExportSet {
                set_id: 2,
                artist: "A".into(),
                title: "T".into(),
                creator: "C".into(),
                source_path: src.to_string_lossy().into_owned(), // 同名冲突
            },
            ExportSet {
                set_id: 3,
                artist: "X".into(),
                title: "Y".into(),
                creator: "Z".into(),
                source_path: tmp.0.join("missing").to_string_lossy().into_owned(),
            },
        ];
        let rep = export_blocking(&target, &sets, |_, _, _| {});
        assert_eq!(rep.exported, 2);
        assert_eq!(rep.renamed, 1);
        assert_eq!(rep.failed, 1);
        assert_eq!(rep.errors.len(), 1);
        assert!(target.join("A - T [C].png").is_file());
        assert!(target.join("A - T [C] (2).png").is_file());
    }

    /// 真实曲库冒烟（只读背景源；缩略图与导出均写入临时目录）。
    /// 运行：cargo test -- --ignored --nocapture
    #[test]
    #[ignore = "requires local osu!lazer installation"]
    fn live_thumb_and_export_check() {
        use crate::realm_db::scan_lazer;
        // 复用 realm 测试的数据目录探测
        let data_dir = {
            let appdata = std::path::PathBuf::from(std::env::var("APPDATA").unwrap());
            ["osu!", "osu"]
                .iter()
                .map(|n| appdata.join(n))
                .find(|p| p.join("client.realm").is_file())
                .expect("lazer data dir")
        };
        let sets = scan_lazer(&data_dir, &|_, _| {}).unwrap();
        let with_bg: Vec<_> = sets
            .iter()
            .filter(|s| {
                s.background_path
                    .as_ref()
                    .map(|p| Path::new(p).is_file())
                    .unwrap_or(false)
            })
            .take(3)
            .collect();
        assert!(!with_bg.is_empty(), "需要至少一个可解析背景的真实集");

        let tmp = TempDir::new("live");
        let thumbs = tmp.0.join("thumbs");
        let reqs: Vec<ThumbRequest> = with_bg
            .iter()
            .map(|s| ThumbRequest {
                set_id: s.beatmapset_id,
                source_path: s.background_path.clone().unwrap(),
                size: 256,
            })
            .collect();
        let res = prepare_blocking(&thumbs, &reqs, |_, _| {});
        for r in &res {
            assert!(r.ok, "thumb 生成失败: {r:?}");
        }
        let sizes: Vec<u64> = res
            .iter()
            .map(|r| {
                fs::metadata(thumbs.join(r.key.as_ref().unwrap()))
                    .unwrap()
                    .len()
            })
            .collect();
        println!("thumb cache sizes (bytes): {sizes:?}");
        assert!(sizes.iter().all(|&s| s > 0));

        let export_sets: Vec<ExportSet> = with_bg
            .iter()
            .map(|s| ExportSet {
                set_id: s.beatmapset_id,
                artist: s.display_artist(),
                title: s.display_title(),
                creator: s.creator.clone(),
                source_path: s.background_path.clone().unwrap(),
            })
            .collect();
        let out = tmp.0.join("export");
        let rep = export_blocking(&out, &export_sets, |_, _, _| {});
        println!("export report: {rep:?}");
        assert_eq!(rep.exported as usize, export_sets.len());
        let exported: Vec<_> = fs::read_dir(&out).unwrap().flatten().collect();
        assert!(!exported.is_empty());
        println!(
            "exported sample file: {:?}",
            exported[0].file_name().to_string_lossy()
        );
    }
}
