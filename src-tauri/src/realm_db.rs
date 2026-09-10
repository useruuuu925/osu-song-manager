// realm_db.rs
//
// Purpose: osu!lazer 只读曲库扫描（M3）。解析数据目录下的 client.realm（Realm 文件
// 格式 v24），组装与 stable 扫描一致的 BeatmapSetInfo。全程只读：仅 std::fs::read，
// 绝不写入任何 lazer 数据。
//
// 二进制布局经对真实 client.realm 的多轮探测校准（要点；已用磁盘文件存在性锚定）：
// - v24 簇树表：table_arr[2] = cluster root；inner root 的子叶簇在 root[3..]；叶根表
//   （小表）root 本身即叶簇。
// - 叶子簇 entries：[0] 行数标记（奇数 → v>>1 行；偶数 → 键数组 ref）、[1] 键列
//   （Guid 主键字节串 / 字符串主键 / 整数键 / 隐藏键）、[2..] 数据列按 spec 顺序、
//   末尾若干扩展数组（null 位图等，本扫描不使用）。
//   因此：spec[0] 即主键的表（class_Beatmap / class_BeatmapSet 的 ID）spec 列 k →
//   entry[1+k]；无主键列的表（Metadata/NRU/Difficulty/User）spec 列 k → entry[2+k]；
//   class_File 的 [1] 是字符串主键（= Hash），spec 列 0 → entry[1]。
// - 链接列（type 12）：标量链接存目标表 1 基行号 L → 展平行序 rows[L-1]（已锚定：
//   triangles 的 .osu 哈希 a1556d… 同时满足 Beatmap.Hash 与 NRU File 链接）；
//   BeatmapSet.Files 列表列存 **NRU 名字表 ID（1 基）**——2026-09-05 逐环 dump 推翻旧的
//   "0 基 File 行号"解读：早期数据两表行号恰好重合导致旧解读"碰巧能用"，File 表删除
//   空洞（ID≠行号）后整体漂移。名字可信；内容行号按 probe_file_row（名字引导 + 魔数
//   校验，FileID ±12 行窗口）定位。
// - 字符串列三种形态自动识别：[offsets,blob] 紧凑串 / per-row wtype2 引用 / multiply
//   短串。
//
// 数组原语复用 vendor/realm-codec 的 read_array_for_debug / read_string_array_for_debug，
// 未改动 vendored reader 代码（链接按行序解析即可，无需暴露键数组）。

use crate::model::{BeatmapInfo, BeatmapSetInfo, BeatmapStatus, GameMode, SourceKind};
use std::path::{Path, PathBuf};

const NODE_HEADER_SIZE: usize = 8;

// ── 底层原语 ──────────────────────────────────────────────────────────────────

fn read_arr(data: &[u8], off: usize) -> Vec<u64> {
    if off == 0 || !off.is_multiple_of(8) || off + NODE_HEADER_SIZE > data.len() {
        return vec![];
    }
    realm_codec::reader::read_array_for_debug(data, off).unwrap_or_default()
}

fn node_wtype(data: &[u8], off: usize) -> Option<(u8, u8, usize)> {
    if off + NODE_HEADER_SIZE > data.len() {
        return None;
    }
    let h = &data[off..off + 8];
    let enc = h[4] & 0x07;
    let width = if enc == 0 { 0 } else { 1u8 << (enc - 1) };
    let wtype = (h[4] & 0x18) >> 3;
    let size = ((h[5] as usize) << 16) | ((h[6] as usize) << 8) | h[7] as usize;
    Some((wtype, width, size))
}

fn is_ref(off: u64, data: &[u8]) -> bool {
    off != 0 && off.is_multiple_of(8) && (off as usize) + NODE_HEADER_SIZE <= data.len()
}

/// 读 wtype=2（raw bytes）节点内的字符串（去掉尾 null）。
fn read_raw_string(data: &[u8], off: usize) -> Option<String> {
    let (wtype, _w, size) = node_wtype(data, off)?;
    if wtype != 2 {
        return None;
    }
    let payload = &data[off + NODE_HEADER_SIZE..];
    let len = size.min(payload.len());
    let end = payload[..len].iter().position(|&b| b == 0).unwrap_or(len);
    Some(String::from_utf8_lossy(&payload[..end]).into_owned())
}

// ── 表遍历 ────────────────────────────────────────────────────────────────────

fn locate_table(data: &[u8], want: &str) -> Option<usize> {
    let top_ref = u64::from_le_bytes(data.get(0..8)?.try_into().ok()?) as usize;
    let group = read_arr(data, top_ref);
    if group.len() < 2 {
        return None;
    }
    let names =
        realm_codec::reader::read_string_array_for_debug(data, group[0] as usize, 64).ok()?;
    let table_refs = read_arr(data, group[1] as usize);
    let i = names.iter().position(|n| n == want)?;
    let tarr = read_arr(data, *table_refs.get(i)? as usize);
    let root = *tarr.get(2)?;
    if root == 0 {
        return None;
    }
    Some(root as usize)
}

struct Leaf {
    entries: Vec<u64>,
    rows: usize,
}

/// 收集一张表的全部叶簇（B+ 树子节点顺序 = 键升序 = 行序）。
fn collect_leaves(data: &[u8], root_ref: usize) -> Vec<Leaf> {
    let root = read_arr(data, root_ref);
    if root.is_empty() {
        return vec![];
    }
    let inner = data
        .get(root_ref + 4)
        .map(|b| b & 0x80 != 0)
        .unwrap_or(false);
    if !inner {
        let rows = leaf_rows(data, &root);
        return if rows > 0 {
            vec![Leaf {
                entries: root,
                rows,
            }]
        } else {
            vec![]
        };
    }
    let mut out = Vec::new();
    for &child in root.iter().skip(3) {
        if !is_ref(child, data) {
            continue;
        }
        let leaf = read_arr(data, child as usize);
        let rows = leaf_rows(data, &leaf);
        if rows > 0 {
            out.push(Leaf {
                entries: leaf,
                rows,
            });
        }
    }
    out
}

fn leaf_rows(data: &[u8], leaf: &[u64]) -> usize {
    let Some(&tag) = leaf.first() else {
        return 0;
    };
    if tag & 1 != 0 {
        // 畸形/损坏数据的 sanity clamp：行数不可能超过文件字节数，
        // 否则 vec![…; rows] 会尝试 TB 级分配直接 abort
        ((tag >> 1) as usize).min(data.len())
    } else {
        read_arr(data, tag as usize).len()
    }
}

// ── 列解码 ────────────────────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Int,
    F64,
    F32,
    Str,
}

enum Col {
    Int(Vec<u64>),
    F64(Vec<f64>),
    Str(Vec<String>),
}

impl Col {
    fn new(kind: Kind, cap: usize) -> Self {
        match kind {
            Kind::Int => Col::Int(Vec::with_capacity(cap)),
            Kind::F64 | Kind::F32 => Col::F64(Vec::with_capacity(cap)),
            Kind::Str => Col::Str(Vec::with_capacity(cap)),
        }
    }
    fn push_leaf(&mut self, other: Col, rows: usize) {
        match (self, other) {
            (Col::Int(a), Col::Int(v)) => a.extend(v),
            (Col::F64(a), Col::F64(v)) => a.extend(v),
            (Col::Str(a), Col::Str(v)) => a.extend(v),
            (Col::Int(a), _) => a.resize(a.len() + rows, 0),
            (Col::F64(a), _) => a.resize(a.len() + rows, 0.0),
            (Col::Str(a), _) => a.resize(a.len() + rows, String::new()),
        }
    }
    fn len(&self) -> usize {
        match self {
            Col::Int(v) => v.len(),
            Col::F64(v) => v.len(),
            Col::Str(v) => v.len(),
        }
    }
    fn int_at(&self, i: usize) -> Option<u64> {
        if let Col::Int(v) = self {
            v.get(i).copied()
        } else {
            None
        }
    }
    fn f64_at(&self, i: usize) -> Option<f64> {
        if let Col::F64(v) = self {
            v.get(i).copied()
        } else {
            None
        }
    }
    fn str_at(&self, i: usize) -> Option<&str> {
        if let Col::Str(v) = self {
            v.get(i).map(|s| s.as_str())
        } else {
            None
        }
    }
}

/// 读取某物理 entry 列并展平全部叶簇。entry 越界/解码失败 → 默认值，绝不 panic。
fn read_col(data: &[u8], leaves: &[Leaf], entry: usize, kind: Kind) -> Col {
    let total: usize = leaves.iter().map(|l| l.rows).sum();
    let mut acc = Col::new(kind, total);
    for leaf in leaves {
        let e = leaf.entries.get(entry).copied().unwrap_or(0);
        let part = decode_col(data, e, leaf.rows, kind);
        acc.push_leaf(part, leaf.rows);
    }
    acc
}

fn decode_col(data: &[u8], e: u64, rows: usize, kind: Kind) -> Col {
    if !is_ref(e, data) {
        // 小表内联标量列（值直接放在 entry 槽位）或空列。
        // 注意：非零内联 Int 仅返回单元素列，多行表会与其它列错位——
        // 真实曲库未观察到触发（内联标量仅见于单行小表），改动前需先验证。
        let one = [e];
        return match kind {
            Kind::Int => Col::Int(if e == 0 {
                vec![0; rows]
            } else {
                one[..1].to_vec()
            }),
            Kind::F64 | Kind::F32 => Col::F64(vec![0.0; rows]),
            Kind::Str => Col::Str(vec![String::new(); rows]),
        };
    }
    let off = e as usize;
    match kind {
        Kind::Int => {
            let v = read_arr(data, off);
            Col::Int(v)
        }
        Kind::F64 => {
            let v = read_arr(data, off);
            Col::F64(v.iter().map(|&b| f64::from_bits(b)).collect())
        }
        Kind::F32 => {
            let v = read_arr(data, off);
            Col::F64(v.iter().map(|&b| f32::from_bits(b as u32) as f64).collect())
        }
        Kind::Str => Col::Str(decode_strings(data, off, rows)),
    }
}

/// 字符串列：自动识别 紧凑串 [offsets,blob] / per-row 引用 / multiply 短串。
fn decode_strings(data: &[u8], off: usize, rows: usize) -> Vec<String> {
    let mut out = vec![String::new(); rows];
    let elems = read_arr(data, off);
    if elems.is_empty() {
        return out;
    }
    let (wtype, _w, _size) = node_wtype(data, off).unwrap_or((0, 0, 0));

    // 歧义判定：per-row 引用数组长度 == rows 且每个元素都能解出 raw 字符串；
    // 紧凑串 [offsets,blob] 的 offsets 元素不是 raw 字符串节点，可据此区分小表。
    let looks_per_row = elems.len() >= rows
        && rows > 0
        && (0..rows).all(|i| {
            let v = *elems.get(i).unwrap_or(&0);
            v != 0 && read_raw_string(data, v as usize).is_some()
        });

    // (1) 紧凑串：[offsets, blob, (nullmap?)]，blob 为 raw 节点
    if !looks_per_row && (elems.len() == 2 || elems.len() == 3) && wtype == 0 {
        let (offs_ref, blob_ref) = (elems[0] as usize, elems[1] as usize);
        if is_ref(elems[0], data) && read_raw_string(data, blob_ref).is_some() {
            let offsets = read_arr(data, offs_ref);
            if let Some((_t, _w, bsize)) = node_wtype(data, blob_ref) {
                let blob_start = blob_ref + NODE_HEADER_SIZE;
                let blob_end = (blob_start + bsize).min(data.len());
                let blob = &data[blob_start..blob_end];
                for (r, dst) in out.iter_mut().enumerate().take(rows) {
                    let start = if r == 0 {
                        0
                    } else {
                        offsets.get(r - 1).copied().unwrap_or(0) as usize
                    };
                    let end = offsets
                        .get(r)
                        .map(|&o| (o as usize).saturating_sub(1))
                        .unwrap_or(start);
                    let start = start.min(end).min(blob.len());
                    let end = end.min(blob.len());
                    *dst = String::from_utf8_lossy(&blob[start..end]).into_owned();
                }
                return out;
            }
        }
    }

    // (2) per-row 引用数组（元素均为 8 对齐 ref）
    if elems.len() >= rows && elems.iter().all(|&v| v == 0 || is_ref(v, data)) {
        let mut hits = 0usize;
        let mut tmp: Vec<String> = Vec::with_capacity(rows);
        for &v in elems.iter().take(rows) {
            match read_raw_string(data, v as usize) {
                Some(s) => {
                    hits += 1;
                    tmp.push(s);
                }
                None => tmp.push(String::new()),
            }
        }
        if hits > 0 && hits * 2 >= rows {
            return tmp;
        }
    }

    // (3) multiply 短串槽位
    if let Some((wt, width, size)) = node_wtype(data, off) {
        if wt == 1 && width > 0 {
            let payload = &data[off + NODE_HEADER_SIZE..];
            let count = size.min(rows);
            for (i, dst) in out.iter_mut().enumerate().take(count) {
                let so = i * width as usize;
                if so >= payload.len() {
                    break;
                }
                let se = (so + width as usize).min(payload.len());
                let slot = &payload[so..se];
                let tail = slot[slot.len() - 1] as usize;
                let len = if tail < slot.len() {
                    slot.len() - 1 - tail
                } else {
                    0
                };
                *dst = String::from_utf8_lossy(&slot[..len]).into_owned();
            }
        }
    }
    out
}

/// 列表列（Files / Beatmaps）：每行一个 ref → 值数组。
fn read_list_col(data: &[u8], leaves: &[Leaf], entry: usize) -> Vec<Vec<u64>> {
    let total: usize = leaves.iter().map(|l| l.rows).sum();
    let mut out: Vec<Vec<u64>> = Vec::with_capacity(total);
    for leaf in leaves {
        let e = leaf.entries.get(entry).copied().unwrap_or(0);
        let per_row = if is_ref(e, data) {
            read_arr(data, e as usize)
        } else {
            vec![0; leaf.rows]
        };
        for &row_ref in per_row.iter().take(leaf.rows) {
            if is_ref(row_ref, data) {
                out.push(read_arr(data, row_ref as usize));
            } else {
                out.push(vec![]);
            }
        }
        for _ in per_row.len()..leaf.rows {
            out.push(vec![]);
        }
    }
    out.resize(total, vec![]);
    out
}

/// 双槽时间戳列：每个叶簇的 entry 指向一个 [秒数组ref, 纳秒数组ref] 对。
/// 展平为每行 Option<i64>（epoch 秒）。0 / 哨兵值视为 None。
fn read_timestamp_secs(data: &[u8], leaves: &[Leaf], entry: usize) -> Vec<Option<i64>> {
    let mut out = Vec::new();
    for leaf in leaves {
        let e = leaf.entries.get(entry).copied().unwrap_or(0);
        let pair = read_arr(data, e as usize);
        // pair[0] = 秒数组 ref；pair[1] = 纳秒数组 ref
        let secs = if !pair.is_empty() && is_ref(pair[0], data) {
            read_arr(data, pair[0] as usize)
        } else {
            vec![0; leaf.rows]
        };
        for i in 0..leaf.rows {
            let raw = secs.get(i).copied().unwrap_or(0);
            let secs_i = sign_extend_i64(raw);
            // realm 可空时间戳哨兵：0 / 0x7FFFFFFF / 0xFFFFFFFF / 负数 → None
            let valid = matches!(secs_i, s if s > 0 && s != 0x7FFF_FFFF && s != 0xFFFF_FFFF);
            out.push(if valid { Some(secs_i) } else { None });
        }
    }
    out
}

/// 64 位补码还原：realm 的时间戳/状态列以 u64 存储，直接按 i64 解释即得到
/// 补码语义（负值 = null/特殊哨兵，由调用方的 valid 过滤器处理）。
fn sign_extend_i64(raw: u64) -> i64 {
    raw as i64
}

/// Status 列以 8-bit 存储：>127 视作负（253 → -3）。
fn sign_extend_i8(raw: u64) -> i64 {
    let b = (raw & 0xFF) as u8;
    b as i8 as i64
}

/// 32 位列补码还原：4294967295 → -1（lazer 用 -1 表示本地导入、无在线 ID）。
fn sign_extend_i32(raw: u64) -> i64 {
    (raw as u32) as i32 as i64
}

// ── 组装 ──────────────────────────────────────────────────────────────────────

fn mode_from_shortname(s: &str) -> GameMode {
    if s.eq_ignore_ascii_case("taiko") {
        GameMode::Taiko
    } else if s.eq_ignore_ascii_case("fruits") || s.eq_ignore_ascii_case("catch") {
        GameMode::Catch
    } else if s.eq_ignore_ascii_case("mania") {
        GameMode::Mania
    } else {
        GameMode::Osu
    }
}

fn sha256_hex(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit())
}

/// osu! 支持的音频扩展名（仅按文件名判断，不做体积等启发式猜测）
fn is_audio_filename(name: &str) -> bool {
    let lower = name.to_lowercase();
    [".mp3", ".ogg", ".wav", ".mp4", ".m4a"]
        .iter()
        .any(|ext| lower.ends_with(ext))
}

/// 数据目录（含 client.realm 与 files\）→ 谱面集列表。只读解析。
/// progress 采用与本模块其它扫描函数一致的 `&dyn Fn(u32, u32)` 形态
/// （scan_library 在 spawn_blocking 内部同步调用，回调 Send 性由调用点保证）。
pub fn scan_lazer(
    data_dir: &Path,
    progress: &dyn Fn(u32, u32),
) -> Result<Vec<BeatmapSetInfo>, String> {
    let realm_path = data_dir.join("client.realm");
    if !realm_path.is_file() {
        return Err(crate::errcode::ec1(
            crate::errcode::REALM_NOT_FOUND,
            realm_path.display(),
        ));
    }
    // 只读方式加载文件内容（不做任何写操作）
    let data = std::fs::read(&realm_path)
        .map_err(|e| crate::errcode::ec1(crate::errcode::REALM_READ_FAILED, e))?;
    let total = 10u32;
    progress(1, total);

    let bm_root = locate_table(&data, "class_Beatmap")
        .ok_or_else(|| crate::errcode::ec1(crate::errcode::REALM_MISSING_TABLE, "class_Beatmap"))?;
    let set_root = locate_table(&data, "class_BeatmapSet").ok_or_else(|| {
        crate::errcode::ec1(crate::errcode::REALM_MISSING_TABLE, "class_BeatmapSet")
    })?;
    let leaves_bm = collect_leaves(&data, bm_root);
    let leaves_set = collect_leaves(&data, set_root);
    progress(2, total);

    // Beatmap：spec[0]=ID(键) → spec 列 k → entry[1+k]
    let bm_version = read_col(&data, &leaves_bm, 2, Kind::Str); // DifficultyName
    let bm_ruleset = read_col(&data, &leaves_bm, 3, Kind::Int);
    let bm_diff = read_col(&data, &leaves_bm, 4, Kind::Int);
    let bm_meta = read_col(&data, &leaves_bm, 5, Kind::Int);
    let bm_set = read_col(&data, &leaves_bm, 7, Kind::Int);
    let bm_online = read_col(&data, &leaves_bm, 9, Kind::Int);
    let bm_len = read_col(&data, &leaves_bm, 10, Kind::F64);
    let bm_bpm = read_col(&data, &leaves_bm, 11, Kind::F64);
    let bm_sr = read_col(&data, &leaves_bm, 13, Kind::F64);
    let bm_md5 = read_col(&data, &leaves_bm, 14, Kind::Str);
    let bm_objects = read_col(&data, &leaves_bm, 20, Kind::Int); // TotalObjectCount
    let bm_count = bm_set.len();
    progress(3, total);

    // BeatmapSet：[1]=ID 键、[2]=隐藏列(行序号)、[3]=OnlineID（经 mirror.hinizawa 对拍证实：
    // entry[3] 的值 158023/320118 与线上真实集 "Everything will freeze"/"No title" 一致，
    // 而 entry[2] 是密集序号 1,37104,60520…并非线上 ID）、时间戳对 @4..6、
    // Beatmaps 列表 @7、Files 列表 @8、Status @9
    let set_online = read_col(&data, &leaves_set, 3, Kind::Int);
    let set_files = read_list_col(&data, &leaves_set, 8);
    let set_status = read_col(&data, &leaves_set, 9, Kind::Int);
    let set_date_added = read_timestamp_secs(&data, &leaves_set, 4);
    let set_count = set_online.len();
    progress(4, total);

    // BeatmapMetadata：无 ID 列 → spec k → entry[2+k]
    // 已证实列位：Title@2 TitleUnicode@3 Artist@4 ArtistUnicode@5 Author@6 AudioFile@11
    // （音频 529/529 验证过）；BackgroundFile@12 为 AudioFile 的相邻声明（曲名→作者→来源→
    // 标签→预览点→音频→背景 的属性声明序），由 live 测试中"与 NRU 命中名的一致性"交叉验证。
    let leaves_md = opt_leaves(&data, "class_BeatmapMetadata");
    let (md_title, md_title_u, md_artist, md_artist_u, md_author, md_audio, md_bg) =
        match &leaves_md {
            Some(lv) => (
                read_col(&data, lv, 2, Kind::Str),
                read_col(&data, lv, 3, Kind::Str),
                read_col(&data, lv, 4, Kind::Str),
                read_col(&data, lv, 5, Kind::Str),
                read_col(&data, lv, 6, Kind::Int),
                read_col(&data, lv, 11, Kind::Str),
                read_col(&data, lv, 12, Kind::Str),
            ),
            None => (
                Col::Str(vec![]),
                Col::Str(vec![]),
                Col::Str(vec![]),
                Col::Str(vec![]),
                Col::Int(vec![]),
                Col::Str(vec![]),
                Col::Str(vec![]),
            ),
        };
    // class_File：[1] = 字符串主键（即 Hash，per-row 引用）
    let file_hashes = match opt_leaves(&data, "class_File") {
        Some(lv) => read_col(&data, &lv, 1, Kind::Str),
        None => Col::Str(vec![]),
    };
    // NRU：隐藏键列占 [1] → File 链接 @2（1 基）、Filename @3
    let leaves_nru = opt_leaves(&data, "class_RealmNamedFileUsage");
    let (nru_file, nru_name) = match &leaves_nru {
        Some(lv) => (
            read_col(&data, lv, 2, Kind::Int),
            read_col(&data, lv, 3, Kind::Str),
        ),
        None => (Col::Int(vec![]), Col::Str(vec![])),
    };
    progress(5, total);

    // BeatmapDifficulty：隐藏键 @1 → DrainRate@2 CircleSize@3 OD@4 AR@5
    let leaves_bd = opt_leaves(&data, "class_BeatmapDifficulty");
    let (d_dr, d_cs, d_od, d_ar) = match &leaves_bd {
        Some(lv) => (
            read_col(&data, lv, 2, Kind::F32),
            read_col(&data, lv, 3, Kind::F32),
            read_col(&data, lv, 4, Kind::F32),
            read_col(&data, lv, 5, Kind::F32),
        ),
        None => (
            Col::F64(vec![]),
            Col::F64(vec![]),
            Col::F64(vec![]),
            Col::F64(vec![]),
        ),
    };
    // RealmUser：spec [OnlineID, Username, CountryCode] 无 ID → k → [2+k]
    let user_name = match opt_leaves(&data, "class_RealmUser") {
        Some(lv) => read_col(&data, &lv, 3, Kind::Str),
        None => Col::Str(vec![]),
    };
    // Ruleset：叶子根小表（4 行）。诊断确认列物理布局：entry[1]=ShortName
    // （multiply 短串 ["fruits","mania","osu","taiko"]），行序与链接 1 基行号一致。
    let ruleset_short = match opt_leaves(&data, "class_Ruleset") {
        Some(lv) => read_col(&data, &lv, 1, Kind::Str),
        None => Col::Str(vec![]),
    };
    progress(6, total);

    // 分组：难度按 BeatmapSet 链接归组
    let mut per_set: Vec<Vec<usize>> = vec![Vec::new(); set_count.max(1)];
    let mut skipped = 0u32;
    for i in 0..bm_count {
        match bm_set.int_at(i) {
            Some(l) if l >= 1 && (l as usize) <= set_count => per_set[l as usize - 1].push(i),
            _ => skipped += 1,
        }
    }

    let mut out: Vec<BeatmapSetInfo> = Vec::new();
    for (sj, diffs) in per_set.iter().enumerate() {
        if diffs.is_empty() {
            continue;
        }
        let first = diffs[0];
        let mi = match bm_meta.int_at(first) {
            Some(l) if l >= 1 => (l - 1) as usize,
            _ => {
                skipped += 1;
                continue;
            }
        };
        // ── 每集文件与背景/音频解析 ──
        // 实测（2026-09-05 逐环 dump）：Files 列表值 = NRU 名字表 ID（1 基），
        // 并非 File 表行号——按行号解读会把 .osu 当成背景图（早期数据两表行号
        // 恰好重合而"碰巧能用"，即旧版 335/529 覆盖率的来历）。NRU 名字可靠；
        // File 表删除空洞使内容行号 = FileID-1-漂移（同集文件块内漂移一致，≤8），
        // 用音频文件校准本集漂移后精确落位（calibrate_drift + resolve_at）。
        // ⚠️ 不做全局同名回退：BG.jpg 类同名跨集极常见，跨集捞图正是串图根因；
        //    也不向前（d<0）搜索：行号 > FileID-1 的行必然属于更高 FileID 的他集文件。
        let named = set_named_files(
            set_files.get(sj).map(|v| &v[..]).unwrap_or(&[]),
            &nru_name,
            &nru_file,
        );
        let mut background: Option<String> = None;
        let mut background_path: Option<String> = None;
        let mut audio_filename: Option<String> = {
            let a = md_audio.str_at(mi).unwrap_or("");
            is_audio_filename(a).then(|| a.to_string())
        };
        if audio_filename.is_none() {
            if let Some((name, _)) = named.iter().find(|(n, _)| is_audio_filename(n)) {
                audio_filename = Some(name.clone());
            }
        }
        let online_id = sign_extend_i32(set_online.int_at(sj).unwrap_or(0));
        // 音频锚点校准本集漂移（优先 md_audio 名，其次列表内首个音频名）
        let d0 = {
            let want_audio = audio_filename.as_deref().unwrap_or("");
            let anchor = named
                .iter()
                .find(|(n, _)| !want_audio.is_empty() && n.eq_ignore_ascii_case(want_audio))
                .or_else(|| named.iter().find(|(n, _)| is_audio_filename(n)));
            match anchor {
                Some((_, fid)) => calibrate_drift(&file_hashes, data_dir, *fid),
                None => 0,
            }
        };
        let resolve = |name: &str, fid: u64| -> Option<String> {
            resolve_at(&file_hashes, data_dir, fid, name, d0, 12).map(|(_, p)| p)
        };
        // ① 规范名 background.(png|jpg|jpeg|webp)
        for (name, fid) in &named {
            if background.is_none()
                && matches!(
                    name.to_lowercase().as_str(),
                    "background.png" | "background.jpg" | "background.jpeg" | "background.webp"
                )
            {
                if let Some(p) = resolve(name, *fid) {
                    background = Some(name.clone());
                    background_path = Some(p);
                }
            }
        }
        // ② Metadata.BackgroundFile 精确名（Importer 记录名最权威）
        if background.is_none() {
            let want = md_bg.str_at(mi).unwrap_or("");
            if !want.is_empty() {
                for (name, fid) in &named {
                    if name.eq_ignore_ascii_case(want) {
                        if let Some(p) = resolve(name, *fid) {
                            background = Some(name.clone());
                            background_path = Some(p);
                        }
                        break;
                    }
                }
            }
        }
        // ③ 任意图片后缀名（含 webp）
        if background.is_none() {
            for (name, fid) in &named {
                let lower = name.to_lowercase();
                if matches!(
                    lower.rsplit('.').next(),
                    Some("png" | "jpg" | "jpeg" | "webp")
                ) {
                    if let Some(p) = resolve(name, *fid) {
                        background = Some(name.clone());
                        background_path = Some(p);
                        break;
                    }
                }
            }
        }
        // ④ 强归属兜底：列表残缺（背景名不在本集列表）的少数集，用本集 .osu
        // 内嵌 BeatmapSetID 校验归属、锚定文件块后按背景扩展名找图。
        if background.is_none() {
            let want = md_bg.str_at(mi).unwrap_or("");
            if let Some((name, p)) =
                recover_bg_by_osu_anchor(&file_hashes, data_dir, &named, d0, want, online_id)
            {
                background = Some(name);
                background_path = Some(p);
            }
        }
        let mut difficulties = Vec::with_capacity(diffs.len());
        for &bi in diffs {
            let version = bm_version.str_at(bi).unwrap_or("");
            if version.is_empty() {
                skipped += 1;
                continue;
            }
            let di = match bm_diff.int_at(bi) {
                Some(l) if l >= 1 => (l - 1) as usize,
                _ => usize::MAX,
            };
            let mode = match bm_ruleset.int_at(bi) {
                Some(l) if l >= 1 && (l as usize) <= ruleset_short.len() => {
                    mode_from_shortname(ruleset_short.str_at(l as usize - 1).unwrap_or("osu!"))
                }
                _ => GameMode::Osu,
            };
            let md5 = bm_md5.str_at(bi).unwrap_or("");
            difficulties.push(BeatmapInfo {
                beatmap_id: sign_extend_i32(bm_online.int_at(bi).unwrap_or(0)),
                mode,
                version: version.to_string(),
                creator: match md_author.int_at(mi) {
                    Some(l) if l >= 1 => {
                        user_name.str_at((l - 1) as usize).unwrap_or("").to_string()
                    }
                    _ => String::new(),
                },
                cs: d_cs.f64_at(di).unwrap_or(0.0) as f32,
                ar: d_ar.f64_at(di).unwrap_or(0.0) as f32,
                od: d_od.f64_at(di).unwrap_or(0.0) as f32,
                hp: d_dr.f64_at(di).unwrap_or(0.0) as f32,
                bpm: bm_bpm.f64_at(bi).unwrap_or(0.0),
                total_ms: bm_len.f64_at(bi).unwrap_or(0.0).round() as i64,
                object_count: bm_objects.int_at(bi).unwrap_or(0) as u32,
                md5: if md5.is_empty() {
                    None
                } else {
                    Some(md5.to_string())
                },
                star_rating: {
                    let v = bm_sr.f64_at(bi).unwrap_or(0.0);
                    if v > 0.0 && v.is_finite() {
                        Some(v)
                    } else {
                        None
                    }
                },
            });
        }
        if difficulties.is_empty() {
            skipped += 1;
            continue;
        }
        let status = set_status
            .int_at(sj)
            .map(sign_extend_i8)
            .map(BeatmapStatus::from_lazer_numeric)
            .unwrap_or(BeatmapStatus::Unknown);
        let date_added = set_date_added.get(sj).copied().flatten();
        out.push(BeatmapSetInfo {
            beatmapset_id: online_id,
            title: md_title.str_at(mi).unwrap_or("").to_string(),
            title_unicode: md_title_u.str_at(mi).unwrap_or("").to_string(),
            artist: md_artist.str_at(mi).unwrap_or("").to_string(),
            artist_unicode: md_artist_u.str_at(mi).unwrap_or("").to_string(),
            creator: difficulties[0].creator.clone(),
            source: String::new(),
            tags: String::new(),
            difficulties,
            background,
            background_path,
            audio_filename,
            source_kind: SourceKind::Lazer,
            location: online_id.to_string(),
            status,
            date_added,
        });
        if sj % 128 == 0 {
            let p = 6 + ((sj as u32 * 3) / (set_count.max(1) as u32)).min(3);
            progress(p, total);
        }
    }
    if skipped > 0 {
        eprintln!("scan_lazer: 跳过 {skipped} 条异常/缺链接记录");
    }
    progress(total, total);
    Ok(out)
}

fn opt_leaves(data: &[u8], table: &str) -> Option<Vec<Leaf>> {
    locate_table(data, table).map(|r| collect_leaves(data, r))
}

// ── T2 转换器数据源：每集完整文件清单 ────────────────────────────────────────

/// 一集的文件索引（convert.rs 的输入）。
#[derive(Debug, Clone)]
pub struct LazerSetFileIndex {
    pub online_id: i64,
    pub title: String,
    pub artist: String,
    /// (原始文件名, 源绝对路径)。同名去重（首个优先）。
    pub files: Vec<(String, PathBuf)>,
}

/// 索引出每集的完整文件清单（文件名 → files/<h[0]>/<h[0..2]>/<hash> 物理路径）+
/// 命名用元数据（首个难度的 Title/Artist）。
/// 列布局与 scan_lazer 完全一致（BeatmapSet OnlineID@3 / Files@8 / Beatmaps@7、
/// Beatmap Meta@5、Metadata Title@2 Artist@4、File 哈希@1、NRU File@2 Name@3），
/// 独立成函数以免触碰 scan_lazer 的既有行为。只读。
/// only_ids = Some 时仅索引这些集（转换路径按请求集工作，避免大曲库全库探测的固定等待）；
/// None = 全量（测试/诊断）。
pub fn collect_set_files(
    data_dir: &Path,
    only_ids: Option<&std::collections::HashSet<i64>>,
) -> Result<Vec<LazerSetFileIndex>, String> {
    use std::path::PathBuf;
    let realm_path = data_dir.join("client.realm");
    if !realm_path.is_file() {
        return Err(crate::errcode::ec1(
            crate::errcode::REALM_NOT_FOUND,
            realm_path.display(),
        ));
    }
    let data = std::fs::read(&realm_path)
        .map_err(|e| crate::errcode::ec1(crate::errcode::REALM_READ_FAILED, e))?;

    let set_root = locate_table(&data, "class_BeatmapSet").ok_or_else(|| {
        crate::errcode::ec1(crate::errcode::REALM_MISSING_TABLE, "class_BeatmapSet")
    })?;
    let bm_root = locate_table(&data, "class_Beatmap")
        .ok_or_else(|| crate::errcode::ec1(crate::errcode::REALM_MISSING_TABLE, "class_Beatmap"))?;
    let leaves_set = collect_leaves(&data, set_root);
    let leaves_bm = collect_leaves(&data, bm_root);
    let set_online = read_col(&data, &leaves_set, 3, Kind::Int);
    let set_files = read_list_col(&data, &leaves_set, 8);
    // 每集首个难度 → 元数据行（scan_lazer 同款链接链）
    let bm_set = read_col(&data, &leaves_bm, 7, Kind::Int);
    let bm_meta = read_col(&data, &leaves_bm, 5, Kind::Int);
    let leaves_md = opt_leaves(&data, "class_BeatmapMetadata");
    let (md_title, md_artist, md_bg) = match &leaves_md {
        Some(lv) => (
            read_col(&data, lv, 2, Kind::Str),
            read_col(&data, lv, 4, Kind::Str),
            read_col(&data, lv, 12, Kind::Str),
        ),
        None => (Col::Str(vec![]), Col::Str(vec![]), Col::Str(vec![])),
    };
    let file_hashes = match opt_leaves(&data, "class_File") {
        Some(lv) => read_col(&data, &lv, 1, Kind::Str),
        None => Col::Str(vec![]),
    };
    let leaves_nru = opt_leaves(&data, "class_RealmNamedFileUsage");
    let (nru_file, nru_name) = match &leaves_nru {
        Some(lv) => (
            read_col(&data, lv, 2, Kind::Int),
            read_col(&data, lv, 3, Kind::Str),
        ),
        None => (Col::Int(vec![]), Col::Str(vec![])),
    };

    // 每集首个难度的元数据行号（1 基 → 0 基）
    let mut first_meta: Vec<Option<usize>> = vec![None; set_online.len().max(1)];
    for i in 0..bm_set.len() {
        if let (Some(l), Some(ml)) = (bm_set.int_at(i), bm_meta.int_at(i)) {
            if l >= 1
                && (l as usize) <= first_meta.len()
                && ml >= 1
                && first_meta[l as usize - 1].is_none()
            {
                first_meta[l as usize - 1] = Some((ml - 1) as usize);
            }
        }
    }

    let mut out: Vec<LazerSetFileIndex> = Vec::new();
    for (sj, first_meta_cell) in first_meta.iter().enumerate() {
        let online_id = sign_extend_i32(set_online.int_at(sj).unwrap_or(0));
        if online_id <= 0 {
            continue; // 本地导入（无在线 ID）：按 ID 转换不可寻址，跳过
        }
        if let Some(only) = only_ids {
            if !only.contains(&online_id) {
                continue;
            }
        }
        let mut files: Vec<(String, PathBuf)> = Vec::new();
        let mut used: std::collections::HashSet<String> = std::collections::HashSet::new();
        // Files 列表值 = NRU 名字表 ID（1 基，见 scan_lazer 注释）：名字可靠，
        // 内容行号按本集音频锚点校准漂移后精确落位。
        let named = set_named_files(
            set_files.get(sj).map(|v| &v[..]).unwrap_or(&[]),
            &nru_name,
            &nru_file,
        );
        let d0 = named
            .iter()
            .find(|(n, _)| is_audio_filename(n))
            .map(|(_, fid)| calibrate_drift(&file_hashes, data_dir, *fid))
            .unwrap_or(0);
        for (name, fid) in &named {
            let Some((_, p)) = resolve_at(&file_hashes, data_dir, *fid, name, d0, 12) else {
                continue;
            };
            // 异集 .osu 过滤：Files 列表与链接在部分数据段仍有残余歧义，直接复制会让
            // 产物带进加载失败的陌生难度。以 .osu 内嵌 [Metadata] BeatmapSetID 为准
            // （读文件文本，权威且与 stable 行为一致）；ID 缺失/≤0 的旧文件不丢弃。
            if name.to_lowercase().ends_with(".osu") {
                let embedded = osu_embedded_set_id(Path::new(&p));
                if embedded > 0 && embedded != online_id {
                    continue;
                }
            }
            if !used.insert(name.to_lowercase()) {
                continue; // 同名多 File：首个优先
            }
            files.push((name.clone(), PathBuf::from(p)));
        }
        // 强归属背景兜底（scan_lazer ④ 同款）：列表残缺的集在块邻域补背景，
        // 否则转换产物缺图。
        if let Some(mi) = first_meta_cell {
            let want = md_bg.str_at(*mi).unwrap_or("");
            if let Some((name, p)) =
                recover_bg_by_osu_anchor(&file_hashes, data_dir, &named, d0, want, online_id)
            {
                if used.insert(name.to_lowercase()) {
                    files.push((name, PathBuf::from(p)));
                }
            }
        }
        if files.is_empty() {
            continue;
        }
        let mi = first_meta[sj];
        out.push(LazerSetFileIndex {
            online_id,
            title: mi
                .and_then(|i| md_title.str_at(i))
                .unwrap_or("")
                .to_string(),
            artist: mi
                .and_then(|i| md_artist.str_at(i))
                .unwrap_or("")
                .to_string(),
            files,
        });
    }
    Ok(out)
}

/// 读取 .osu 文件内嵌的 [Metadata] BeatmapSetID（collect_set_files 的异集过滤用）。
/// 逐行读、命中即返（.osu 的 Metadata 在文件头部，通常几行内命中，避免整文件读入）。
/// 文件缺失/非 UTF-8/无该字段时返回 0（调用方按"未知"放行，不误删旧图）。
fn osu_embedded_set_id(path: &Path) -> i64 {
    use std::io::BufRead;
    let Ok(f) = std::fs::File::open(path) else {
        return 0;
    };
    let mut section = String::new();
    for line in std::io::BufReader::new(f).lines() {
        let Ok(line) = line else { return 0 };
        let line = line.trim();
        if line.starts_with('[') && line.ends_with(']') {
            section = line[1..line.len() - 1].to_string();
            continue;
        }
        if section == "Metadata" {
            if let Some(v) = line.strip_prefix("BeatmapSetID:") {
                return v.trim().parse().unwrap_or(0);
            }
        }
    }
    0
}

/// 文件行号（0 基）→ 该行的哈希 → 磁盘路径 files/<h[0]>/<h[0..2]>/<h>。
/// 哈希非法时返回 None（不检查存在性——probe_file_row 负责校验内容）。
fn file_row_path(hashes: &Col, row: usize, data_dir: &Path) -> Option<std::path::PathBuf> {
    let h = hashes.str_at(row)?;
    if !sha256_hex(h) {
        return None;
    }
    Some(data_dir.join("files").join(&h[..1]).join(&h[..2]).join(h))
}

/// 文件名扩展名 → 内容魔数匹配（probe_file_row 的名字引导校验，只看前 16 字节）。
/// 未知扩展名一律 false（严格：宁缺毋错）。.osu/.osb 容忍 BOM 前缀。
fn magic_matches(name: &str, bytes: &[u8]) -> bool {
    let ext = name.rsplit('.').next().unwrap_or("").to_lowercase();
    let b = bytes;
    match ext.as_str() {
        "jpg" | "jpeg" => b.starts_with(&[0xFF, 0xD8]),
        "png" => b.starts_with(&[0x89, b'P', b'N', b'G']),
        "webp" => b.len() >= 12 && &b[0..4] == b"RIFF" && &b[8..12] == b"WEBP",
        "gif" => b.starts_with(b"GIF8"),
        "bmp" => b.starts_with(b"BM"),
        "mp3" => b.starts_with(b"ID3") || (b.len() >= 2 && b[0] == 0xFF && (b[1] & 0xE0) == 0xE0),
        "ogg" | "oga" | "opus" => b.starts_with(b"OggS"),
        "wav" => b.starts_with(b"RIFF") && b.len() >= 12 && &b[8..12] == b"WAVE",
        "avi" => b.starts_with(b"RIFF"),
        "mp4" | "m4a" => b.len() >= 8 && &b[4..8] == b"ftyp",
        "flv" => b.starts_with(b"FLV"),
        "osu" | "osb" => {
            let t = if b.starts_with(&[0xEF, 0xBB, 0xBF]) {
                &b[3..]
            } else {
                b
            };
            t.starts_with(b"osu file format")
                || t.starts_with(b"[Events]")
                || t.starts_with(b"[General]")
        }
        _ => false,
    }
}

/// 通用音频魔数（校准用，不限扩展名）：MP3 (ID3/裸帧)、Ogg、WAV/AVI (RIFF)、M4A (ftyp)。
fn is_audio_magic(b: &[u8]) -> bool {
    b.starts_with(b"ID3")
        || (b.len() >= 2 && b[0] == 0xFF && (b[1] & 0xE0) == 0xE0)
        || b.starts_with(b"OggS")
        || (b.starts_with(b"RIFF") && b.len() >= 12 && &b[8..12] == b"WAVE")
        || (b.len() >= 8 && &b[4..8] == b"ftyp")
}

/// 集内局部漂移校准：File 表的删除空洞使 FileID ≠ 行号，且漂移在同一集的文件块内
/// 一致（实测 545 集漂移 ≤8）。用音频文件锚定：从 FileID-1 起向后（d 增大）找音频
/// 魔数行，d = FileID-1-行号 即本集漂移量；找不到返回 0。
fn calibrate_drift(hashes: &Col, data_dir: &Path, fid_audio: u64) -> i64 {
    use std::io::Read;
    let base = match fid_audio.checked_sub(1) {
        Some(b) => b as i64,
        None => return 0,
    };
    for d in 0..=12i64 {
        let r = base - d;
        if r < 0 {
            break;
        }
        let Some(p) = file_row_path(hashes, r as usize, data_dir) else {
            continue;
        };
        let Ok(mut f) = std::fs::File::open(&p) else {
            continue;
        };
        let mut buf = [0u8; 16];
        let n = f.read(&mut buf).unwrap_or(0);
        if n > 0 && is_audio_magic(&buf[..n]) {
            return d;
        }
    }
    0
}

/// 按校准漂移 d0 精确落位 + 邻近校验：行号 = FileID-1-d，先试 d0，再按 |d-d0|
/// 递增向两侧扩（d 恒 ≥0：行号不可能大于 FileID-1，向前搜索必然落到他集文件）。
/// 只读文件前 16 字节做扩展名魔数校验。返回 (File 行号, 磁盘路径)。
fn resolve_at(
    hashes: &Col,
    data_dir: &Path,
    file_id: u64,
    name: &str,
    d0: i64,
    window: i64,
) -> Option<(u64, String)> {
    use std::io::Read;
    let base = file_id.checked_sub(1)? as i64;
    let len = hashes.len() as i64;
    let d0 = d0.clamp(0, window);
    for off in 0..=window {
        for &d in [d0 + off, d0 - off].iter() {
            if d < 0 || d > window {
                continue;
            }
            let r = base - d;
            if r < 0 || r >= len {
                continue;
            }
            let Some(p) = file_row_path(hashes, r as usize, data_dir) else {
                continue;
            };
            let Ok(mut f) = std::fs::File::open(&p) else {
                continue;
            };
            let mut buf = [0u8; 16];
            let n = f.read(&mut buf).unwrap_or(0);
            if n > 0 && magic_matches(name, &buf[..n]) {
                return Some((r as u64, p.to_string_lossy().into_owned()));
            }
        }
    }
    None
}

/// 强归属背景兜底（少数 Files 列表数据残缺的集：背景名根本不在本集列表）。
/// 用本集 .osu 内容内嵌的 BeatmapSetID（内容级证据）校验归属并锚定本集文件块，
/// 再在块邻域（锚点 ±8 行）内按背景扩展名找图。只读。
fn recover_bg_by_osu_anchor(
    hashes: &Col,
    data_dir: &Path,
    named: &[(String, u64)],
    d0: i64,
    want: &str,
    set_id: i64,
) -> Option<(String, String)> {
    use std::io::Read;
    if want.is_empty() {
        return None;
    }
    let mut anchors: Vec<i64> = Vec::new();
    for (name, fid) in named
        .iter()
        .filter(|(n, _)| n.to_lowercase().ends_with(".osu"))
        .take(4)
    {
        if let Some((row, p)) = resolve_at(hashes, data_dir, *fid, name, d0, 12) {
            if osu_embedded_set_id(Path::new(&p)) == set_id {
                anchors.push(row as i64);
            }
        }
    }
    if anchors.is_empty() {
        return None;
    }
    let len = hashes.len() as i64;
    let lo = (*anchors.iter().min().unwrap() - 8).max(0);
    let hi = (*anchors.iter().max().unwrap() + 8).min(len - 1);
    for r in lo..=hi {
        let Some(p) = file_row_path(hashes, r as usize, data_dir) else {
            continue;
        };
        let Ok(mut f) = std::fs::File::open(&p) else {
            continue;
        };
        let mut buf = [0u8; 16];
        let n = f.read(&mut buf).unwrap_or(0);
        if n > 0 && magic_matches(want, &buf[..n]) {
            return Some((want.to_string(), p.to_string_lossy().into_owned()));
        }
    }
    None
}

/// 一集的命名文件：Files 列表值 = NRU 名字表 ID（1 基）→ (名字, FileID)。
/// 同名去重（首个优先）；列表值越界/名字为空/链接非法的条目跳过。
fn set_named_files(list: &[u64], nru_name: &Col, nru_file: &Col) -> Vec<(String, u64)> {
    let mut out: Vec<(String, u64)> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    for &v in list {
        if v < 1 || (v as usize) > nru_name.len() {
            continue;
        }
        let row = v as usize - 1;
        let Some(name) = nru_name.str_at(row) else {
            continue;
        };
        if name.is_empty() {
            continue;
        }
        let Some(fid) = nru_file.int_at(row).filter(|&l| l >= 1) else {
            continue;
        };
        if seen.insert(name.to_lowercase()) {
            out.push((name.to_string(), fid));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn magic_matches_by_extension() {
        let jpg = [0xFF, 0xD8, 0xFF, 0xE0, 0, 0];
        let png = [0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];
        let webp = *b"RIFF\x10\x00\x00\x00WEBPVP8 ";
        let ogg = *b"OggS\x00\x02\x00\x00";
        let osu = *b"osu file format v14";
        let osu_bom = *b"\xEF\xBB\xBFosu file format";
        assert!(magic_matches("BG.jpg", &jpg));
        assert!(magic_matches("BG.jpeg", &jpg));
        assert!(!magic_matches("BG.jpg", &png), "jpg 名不接受 PNG 内容");
        assert!(magic_matches("a.png", &png));
        assert!(magic_matches("a.webp", &webp));
        assert!(magic_matches("audio.ogg", &ogg));
        assert!(magic_matches("a.osu", &osu));
        assert!(magic_matches("a.osu", &osu_bom), "osu 容忍 UTF-8 BOM");
        assert!(magic_matches("a.osb", &osu));
        assert!(!magic_matches("a.unknown", &jpg), "未知扩展名不通过");
        assert!(!magic_matches("noext", &jpg));
    }

    #[test]
    fn resolve_at_calibrated_drift() {
        use std::io::Write;
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("osm_resolve_{}_{nanos}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mk = |bytes: &[u8]| {
            use sha2::{Digest, Sha256};
            let mut h = Sha256::new();
            h.update(bytes);
            let hex: String = h.finalize().iter().map(|b| format!("{b:02x}")).collect();
            let p = dir.join("files").join(&hex[..1]).join(&hex[..2]);
            std::fs::create_dir_all(&p).unwrap();
            let mut f = std::fs::File::create(p.join(&hex)).unwrap();
            f.write_all(bytes).unwrap();
            hex
        };
        // 模拟删除空洞：FileID-1-2 处才是真内容（局部漂移 d=2）
        let jpg_hex = mk(&[0xFF, 0xD8, 0xFF, 0xE0, 1, 2, 3]);
        let ogg_hex = mk(b"OggSdata");
        let osu_hex = mk(b"osu file format v14");
        // 行 0=ogg、行 1=osu、行 2=jpg；某集音频 FileID=1 → 反向校准 d：行 0 是 OggS → d=1
        let hashes = Col::Str(vec![ogg_hex.clone(), osu_hex.clone(), jpg_hex.clone()]);
        let d = calibrate_drift(&hashes, &dir, 1);
        assert_eq!(d, 0, "音频 FileID=1 → 行 0 = OggS 命中，d=0");
        // 音频 FileID=3（行 2 应为 jpg，非音频）→ 向后校准到…行 1 osu 非、行 0 ogg 是 → d=2
        let d3 = calibrate_drift(&hashes, &dir, 3);
        assert_eq!(
            d3, 2,
            "音频 FileID=3 → 行 2 jpg 非、行 1 osu 非、行 0 ogg 是，d=2"
        );
        // bg FileID=3 名 BG.jpg：d0=2 → 行 2 = jpg 精确命中
        let hit = resolve_at(&hashes, &dir, 3, "BG.jpg", d3, 12);
        assert_eq!(hit.as_ref().map(|(r, _)| *r), Some(2));
        assert!(hit.unwrap().1.ends_with(&jpg_hex));
        // 名字与内容类型全不匹配 → None（video.mp4 不接受 jpg/ogg/osu）
        assert!(resolve_at(&hashes, &dir, 3, "video.mp4", d3, 12).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn set_named_files_dedups_and_skips_invalid() {
        let nru_name = Col::Str(vec![
            "BG.jpg".into(),
            "audio.mp3".into(),
            "".into(),
            "BG.jpg".into(),
        ]);
        let nru_file = Col::Int(vec![11, 12, 13, 14]);
        // 列表值 = NRU ID（1 基）：1→BG.jpg(11)、2→audio.mp3(12)、3→空名跳过、4→BG.jpg 重名去重
        let named = set_named_files(&[1, 2, 3, 4, 99], &nru_name, &nru_file);
        assert_eq!(named, vec![("BG.jpg".into(), 11), ("audio.mp3".into(), 12)]);
    }

    /// 本机真实数据目录探测：OSU_LAZER_DATA 优先，其次 %APPDATA%\osu! 与 %APPDATA%\osu。
    fn real_data_dir() -> std::path::PathBuf {
        if let Ok(p) = std::env::var("OSU_LAZER_DATA") {
            return std::path::PathBuf::from(p);
        }
        let appdata = std::path::PathBuf::from(std::env::var("APPDATA").expect("APPDATA"));
        for name in ["osu!", "osu"] {
            let cand = appdata.join(name);
            if cand.join("client.realm").is_file() {
                return cand;
            }
        }
        panic!("未找到本地 lazer 数据目录（设置 OSU_LAZER_DATA 覆盖）");
    }

    #[test]
    #[ignore = "requires local osu!lazer installation"]
    fn real_collect_set_files_index() {
        // T2 转换器数据源冒烟：每集文件清单可索引、物理路径在磁盘上真实存在
        let data_dir = real_data_dir();
        let index = collect_set_files(&data_dir, None).expect("collect_set_files ok");
        assert!(!index.is_empty(), "expected >0 indexed sets");
        let with_existing = index
            .iter()
            .filter(|s| s.files.iter().any(|(_, p)| p.is_file()))
            .count();
        assert!(
            with_existing > 0,
            "expected at least one set whose files resolve on disk"
        );
        let total_files: usize = index.iter().map(|s| s.files.len()).sum();
        println!(
            "collect_set_files: sets={} total_files={} sets_with_existing_files={with_existing}",
            index.len(),
            total_files
        );
        for s in index.iter().take(3) {
            println!(
                "  sample set {} | {} - {} | files={}",
                s.online_id,
                s.artist,
                s.title,
                s.files.len()
            );
        }
        // 全部 online_id > 0（本地导入被跳过）
        assert!(index.iter().all(|s| s.online_id > 0));
    }

    #[test]
    #[ignore = "requires local osu!lazer installation"]
    fn real_client_realm_scan() {
        let sets = scan_lazer(&real_data_dir(), &|_done, _total| {}).expect("scan_lazer ok");
        assert!(
            !sets.is_empty(),
            "expected >0 beatmapsets from real client.realm"
        );
        let total_diffs: usize = sets.iter().map(|s| s.difficulties.len()).sum();
        let with_bg = sets.iter().filter(|s| s.background_path.is_some()).count();
        let with_audio = sets.iter().filter(|s| s.audio_filename.is_some()).count();
        let mut existing_bg = 0;
        for s in &sets {
            if let Some(p) = &s.background_path {
                if std::path::Path::new(p).is_file() {
                    existing_bg += 1;
                }
            }
        }
        println!(
            "real client.realm: sets={} difficulties={} sets_with_bg_path={with_bg} sets_with_audio={with_audio} bg_exists_on_disk={existing_bg}",
            sets.len(),
            total_diffs
        );
        for s in sets.iter().take(3) {
            println!(
                "  sample: set {} | {} | {} - {} | diffs={} bpm={:.1} len={}ms status={:?} added={:?}",
                s.beatmapset_id,
                s.title,
                s.artist,
                s.creator,
                s.difficulties.len(),
                s.max_bpm(),
                s.max_total_ms(),
                s.status,
                s.date_added
            );
        }
        // ── M4b 分布统计 ──
        let mut modes: std::collections::BTreeMap<String, usize> = Default::default();
        for s in &sets {
            for d in &s.difficulties {
                *modes.entry(d.mode.display().to_string()).or_insert(0) += 1;
            }
        }
        let mut statuses: std::collections::BTreeMap<String, usize> = Default::default();
        for s in &sets {
            let key = serde_json::to_value(s.status)
                .unwrap()
                .as_str()
                .unwrap_or("?")
                .to_string();
            *statuses.entry(key).or_insert(0) += 1;
        }
        let date_added_count = sets.iter().filter(|s| s.date_added.is_some()).count();
        let object_count_total: usize = sets
            .iter()
            .flat_map(|s| &s.difficulties)
            .filter(|d| d.object_count > 0)
            .count();
        println!(
            "  modes: {:?} statuses: {:?} dateAdded_some={}/{} audio_some={}/{} objectCount>0={}",
            modes,
            statuses,
            date_added_count,
            sets.len(),
            with_audio,
            sets.len(),
            object_count_total
        );
        // 模式分布非平凡：该用户曲库含 mania 图（已由难度参数分布证实）
        assert!(
            *modes.get("mania").unwrap_or(&0) > 0,
            "expected mania difficulties > 0, got {:?}",
            modes
        );
        let with_sr = sets
            .iter()
            .flat_map(|s| &s.difficulties)
            .filter(|d| d.star_rating.is_some())
            .count();
        println!("  difficulties with star_rating={with_sr}/{total_diffs}");
        // 每个集至少 1 个难度
        assert!(sets.iter().all(|s| !s.difficulties.is_empty()));
        // 至少一个背景图能在磁盘上找到实际文件
        assert!(
            existing_bg > 0,
            "expected at least one background file resolving to disk"
        );
    }
}
