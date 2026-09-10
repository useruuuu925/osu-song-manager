// export_csv.rs — 曲库导出 CSV（M8）
//
// 列名与社区工具（如 linnzero00/Osu-Beatmap-Seekman 的导入器）按列名定位兼容：
// 本导出为其格式子集 + 尾部扩展列。每行 = 一个难度。
// 规则：
// - seekman_export_version 固定 "2"；exported_at = 导出时刻 RFC3339 UTC（全行同值）。
// - beatmapset_id/beatmap_id ≤0（本地导入哨兵/0）→ 空单元格，绝不输出假值。
// - stars：null 或 0（未评）→ 空；bpm 3 位；AR/CS/HP/OD 2 位；total_time 整数秒。
// - ranked_status：按 ppy `BeatmapOnlineStatus` 数值（graveyard -3 / wip -2 / pending -1 /
//   ranked 1 / approved 2 / qualified 3 / loved 4）；Unknown（含 none/未映射）→ 空串，
//   不写 0 —— 0 在 osu-web 语义中是 none，不是未知。beatmapset_status 为同一状态文本。
// - 排序：beatmapset_id 升序（无 ID 本地者殿后）→ 星级降序 → version 升序。
// - 在线列（favourite_count/play_count/rating/genre/language/online_synced_at）从本应用
//   缓存 online_cache.json 按 beatmapset_id 合并；无数据则空；online_synced_at=fetchedAt。
// - UTF-8 带 BOM（Excel 中文不乱码）；LF 行尾；RFC4180 最小引号；单元格全部预格式化字符串。
// - 不引入 chrono：epoch→UTC 日期用 Hinnant civil 算法 + 闰规则（测试含 2000/2024/2100）。

use crate::library;
use crate::model::{BeatmapSetInfo, BeatmapStatus, ExportCsvReport, GameMode, SourceKind};
use std::fs;
use std::io::Write;
use std::path::Path;

pub const CSV_HEADER: &[&str] = &[
    "seekman_export_version",
    "exported_at",
    "beatmapset_id",
    "beatmap_id",
    "artist",
    "artist_unicode",
    "title",
    "title_unicode",
    "creator",
    "version",
    "mode",
    "md5",
    "ranked_status",
    "stars",
    "ar",
    "cs",
    "hp",
    "od",
    "bpm",
    "total_time",
    "object_count",
    "source",
    "tags",
    "beatmap_creator",
    "beatmapset_status",
    "favourite_count",
    "play_count",
    "rating",
    "genre",
    "language",
    "online_synced_at",
    "date_added",
    "library_source",
];

// ── 日期时间（无 chrono） ─────────────────────────────────────────────────────

const fn is_leap(y: i64) -> bool {
    (y % 4 == 0 && y % 100 != 0) || y % 400 == 0
}

/// 1970-01-01 之前（或当年）任意天数 → (年,月,日)。纯定义式回退扫描，逻辑即日历。
fn date_from_epoch_day(day: i64) -> (i64, u32, u32) {
    let mut y = 1970 + if day >= 0 { day / 365 } else { day / 366 - 1 };
    while year_start_day(y) > day {
        y -= 1;
    }
    while year_start_day(y + 1) <= day {
        y += 1;
    }
    let mut doy = (day - year_start_day(y)) as u32;
    let months: &[u32; 12] = if is_leap(y) {
        &[31, 29, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    } else {
        &[31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    };
    let mut m = 0usize;
    while m < 11 && doy >= months[m] {
        doy -= months[m];
        m += 1;
    }
    (y, (m + 1) as u32, doy + 1)
}

/// 从 1970-01-01 到 Y-01-01 的天数（可为负，整型除法向零截断，故对负数修正）。
fn year_start_day(y: i64) -> i64 {
    let count = |yy: i64| {
        let p = yy - 1;
        p * 365 + p.div_euclid(4) - p.div_euclid(100) + p.div_euclid(400)
    };
    count(y) - count(1970)
}

/// epoch 秒 → (y, mo, d, h, mi, s) UTC
pub fn datetime_parts(secs: i64) -> (i64, u32, u32, u32, u32, u32) {
    let day = secs.div_euclid(86_400);
    let tod = secs.rem_euclid(86_400) as u32;
    let (y, mo, d) = date_from_epoch_day(day);
    (y, mo, d, tod / 3600, (tod % 3600) / 60, tod % 60)
}

pub fn iso_rfc3339_utc(secs: Option<i64>) -> String {
    match secs {
        None => String::new(),
        Some(s) => {
            let (y, mo, d, h, mi, se) = datetime_parts(s);
            format!("{y:04}-{mo:02}-{d:02}T{h:02}:{mi:02}:{se:02}Z")
        }
    }
}

/// 文件名时间戳 yyyyMMdd-HHmmss
pub fn stamp_from_unix(secs: i64) -> String {
    let (y, mo, d, h, mi, s) = datetime_parts(secs);
    format!("{y:04}{mo:02}{d:02}-{h:02}{mi:02}{s:02}")
}

// ── 单元格规则 ────────────────────────────────────────────────────────────────

fn id_cell(v: i64) -> String {
    if v <= 0 {
        String::new()
    } else {
        v.to_string()
    }
}

/// ranked_status 数值（ppy 枚举）。Unknown → 空串（决策见模块头注释）。
fn ranked_status_code(s: BeatmapStatus) -> String {
    match s {
        BeatmapStatus::Graveyard => "-3".into(),
        BeatmapStatus::Wip => "-2".into(),
        BeatmapStatus::Pending => "-1".into(),
        BeatmapStatus::Ranked => "1".into(),
        BeatmapStatus::Approved => "2".into(),
        BeatmapStatus::Qualified => "3".into(),
        BeatmapStatus::Loved => "4".into(),
        BeatmapStatus::Unknown => String::new(),
    }
}

fn status_text(s: BeatmapStatus) -> &'static str {
    match s {
        BeatmapStatus::Graveyard => "graveyard",
        BeatmapStatus::Wip => "wip",
        BeatmapStatus::Pending => "pending",
        BeatmapStatus::Ranked => "ranked",
        BeatmapStatus::Approved => "approved",
        BeatmapStatus::Qualified => "qualified",
        BeatmapStatus::Loved => "loved",
        BeatmapStatus::Unknown => "unknown",
    }
}

fn mode_text(m: GameMode) -> &'static str {
    match m {
        GameMode::Osu => "osu",
        GameMode::Taiko => "taiko",
        GameMode::Catch => "catch",
        GameMode::Mania => "mania",
    }
}

fn source_text(s: SourceKind) -> &'static str {
    match s {
        SourceKind::Lazer => "lazer",
        SourceKind::Stable => "stable",
        SourceKind::OszFolder => "oszfolder",
    }
}

// ── 组行 ──────────────────────────────────────────────────────────────────────

struct FlatRow {
    sort_set: i64, // beatmapset_id（本地 ≤0 → 排到后面）
    sort_stars: f64,
    cells: Vec<String>,
}

fn build_rows(
    sets: &[BeatmapSetInfo],
    online: &std::collections::HashMap<i64, crate::model::OnlineSetMeta>,
    exported_at: i64,
) -> Vec<FlatRow> {
    let ts = iso_rfc3339_utc(Some(exported_at));
    let mut rows: Vec<FlatRow> = Vec::new();
    for set in sets {
        let o = if set.beatmapset_id > 0 {
            online.get(&set.beatmapset_id)
        } else {
            None
        };
        let ranked = ranked_status_code(set.status);
        let status_t = status_text(set.status).to_string();
        let date_added = iso_rfc3339_utc(set.date_added);
        let online_synced_at = o
            .map(|m| iso_rfc3339_utc(Some(m.fetched_at)))
            .unwrap_or_default();
        for d in &set.difficulties {
            let mut c: Vec<String> = Vec::with_capacity(CSV_HEADER.len());
            c.push("2".into());
            c.push(ts.clone());
            c.push(id_cell(set.beatmapset_id));
            c.push(id_cell(d.beatmap_id));
            c.push(set.artist.clone());
            c.push(set.artist_unicode.clone());
            c.push(set.title.clone());
            c.push(set.title_unicode.clone());
            c.push(set.creator.clone());
            c.push(d.version.clone());
            c.push(mode_text(d.mode).to_string());
            c.push(d.md5.clone().unwrap_or_default());
            c.push(ranked.clone());
            c.push(match d.star_rating {
                Some(v) if v > 0.0 && v.is_finite() => format!("{v:.4}"),
                _ => String::new(),
            });
            c.push(format!("{:.2}", d.ar));
            c.push(format!("{:.2}", d.cs));
            c.push(format!("{:.2}", d.hp));
            c.push(format!("{:.2}", d.od));
            c.push(format!("{:.3}", d.bpm));
            c.push((d.total_ms.div_euclid(1000)).to_string());
            c.push(d.object_count.to_string());
            c.push(set.source.clone());
            c.push(set.tags.clone());
            c.push(d.creator.clone());
            c.push(status_t.clone());
            c.push(o.map(|m| m.favourite_count.to_string()).unwrap_or_default());
            c.push(o.map(|m| m.play_count.to_string()).unwrap_or_default());
            c.push(
                o.and_then(|m| m.rating)
                    .filter(|r| r.is_finite())
                    .map(|r| format!("{r:.2}"))
                    .unwrap_or_default(),
            );
            c.push(o.and_then(|m| m.genre.clone()).unwrap_or_default());
            c.push(o.and_then(|m| m.language.clone()).unwrap_or_default());
            c.push(online_synced_at.clone());
            c.push(date_added.clone());
            c.push(source_text(set.source_kind).to_string());
            let sort_set = if set.beatmapset_id > 0 {
                set.beatmapset_id
            } else {
                i64::MAX
            };
            let sort_stars = d.star_rating.unwrap_or(0.0);
            rows.push(FlatRow {
                sort_set,
                sort_stars,
                cells: c,
            });
        }
    }
    rows.sort_by(|a, b| {
        a.sort_set
            .cmp(&b.sort_set)
            .then(b.sort_stars.total_cmp(&a.sort_stars))
            .then_with(|| a.cells[9].cmp(&b.cells[9])) // version 升序
    });
    rows
}

/// 纯核心：构建并写入 CSV（BOM + LF + 最小引号）。文件名按导出时刻（now_unix 注入，可测）。
pub fn write_library_csv(
    sets: &[BeatmapSetInfo],
    online: &std::collections::HashMap<i64, crate::model::OnlineSetMeta>,
    target_dir: &Path,
    now_unix: i64,
) -> Result<ExportCsvReport, String> {
    use crate::errcode;
    fs::create_dir_all(target_dir)
        .map_err(|e| errcode::ec1(errcode::CREATE_TARGET_DIR_FAILED, e))?;
    let fname = format!("osu-library-export-{}.csv", stamp_from_unix(now_unix));
    let path = target_dir.join(fname);
    let mut f = fs::File::create(&path).map_err(|e| errcode::ec1(errcode::CREATE_CSV_FAILED, e))?;
    f.write_all(&[0xEF, 0xBB, 0xBF])
        .map_err(|e| errcode::ec1(errcode::WRITE_BOM_FAILED, e))?;
    let mut w = csv::WriterBuilder::new()
        .terminator(csv::Terminator::Any(b'\n'))
        .flexible(true)
        .from_writer(f);
    w.write_record(CSV_HEADER).map_err(|e| e.to_string())?;
    let rows = build_rows(sets, online, now_unix);
    for r in &rows {
        w.write_record(&r.cells).map_err(|e| e.to_string())?;
    }
    w.flush().map_err(|e| e.to_string())?;
    Ok(ExportCsvReport {
        file_path: path.to_string_lossy().into_owned(),
        sets: sets.len() as u32,
        rows: rows.len() as u32,
    })
}

// ── 命令 ──────────────────────────────────────────────────────────────────────

#[tauri::command]
pub async fn export_library_csv(
    app: tauri::AppHandle,
    kind: SourceKind,
    path: String,
    target_dir: String,
) -> Result<ExportCsvReport, String> {
    let dir = crate::online::app_dir(&app);
    let report = tauri::async_runtime::spawn_blocking(move || {
        let sets = library::scan_library(kind, &path, &|_done, _total| {})?;
        let online_map: std::collections::HashMap<i64, crate::model::OnlineSetMeta> =
            crate::online::online_cache_map(&dir).into_iter().collect();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        write_library_csv(&sets, &online_map, Path::new(&target_dir), now)
    })
    .await
    .map_err(|e| crate::errcode::ec1(crate::errcode::CSV_INTERRUPTED, e))??;
    Ok(report)
}

// ── 测试 ──────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{BeatmapInfo, OnlineSetMeta};

    fn tmpdir(tag: &str) -> std::path::PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let d = std::env::temp_dir().join(format!("osm_m8_{tag}_{}_{nanos}", std::process::id()));
        fs::create_dir_all(&d).unwrap();
        d
    }

    fn diff(id: i64, version: &str, stars: Option<f64>, md5: Option<&str>) -> BeatmapInfo {
        BeatmapInfo {
            beatmap_id: id,
            mode: GameMode::Osu,
            version: version.into(),
            creator: "nm".into(),
            cs: 4.0,
            ar: 9.5,
            od: 8.25,
            hp: 5.0,
            bpm: 180.0,
            total_ms: 134_500,
            object_count: 456,
            md5: md5.map(str::to_string),
            star_rating: stars,
        }
    }

    fn set(
        sid: i64,
        title: &str,
        artist: &str,
        status: BeatmapStatus,
        date_added: Option<i64>,
        diffs: Vec<BeatmapInfo>,
        src: SourceKind,
    ) -> BeatmapSetInfo {
        BeatmapSetInfo {
            beatmapset_id: sid,
            title: title.into(),
            title_unicode: title.into(),
            artist: artist.into(),
            artist_unicode: artist.into(),
            creator: "cr".into(),
            source: "anime".into(),
            tags: "t1 t2".into(),
            difficulties: diffs,
            background: None,
            background_path: None,
            audio_filename: None,
            source_kind: src,
            location: "loc".into(),
            status,
            date_added,
        }
    }

    #[test]
    fn header_is_33_cols_in_exact_order() {
        assert_eq!(CSV_HEADER.len(), 33);
        assert_eq!(CSV_HEADER[0], "seekman_export_version");
        assert_eq!(CSV_HEADER[1], "exported_at");
        assert_eq!(CSV_HEADER[2], "beatmapset_id");
        assert_eq!(CSV_HEADER[32], "library_source");
        assert!(CSV_HEADER.contains(&"online_synced_at"));
    }

    #[test]
    fn datetime_parts_known_epochs_incl_leap_rules() {
        assert_eq!(datetime_parts(0), (1970, 1, 1, 0, 0, 0), "unix 0 = epoch");
        // 2000-02-29（世纪闰年）12:00:00Z
        assert_eq!(datetime_parts(951_825_600), (2000, 2, 29, 12, 0, 0));
        // 2024-02-29T13:45:30Z
        assert_eq!(datetime_parts(1_709_214_330), (2024, 2, 29, 13, 45, 30));
        // 2100 非闰：00:00:00Z 应为 03-01 前一天 = 02-28 之后直接跳 03-01
        assert_eq!(datetime_parts(4_107_542_400), (2100, 3, 1, 0, 0, 0));
        // 负 epoch（1900-01-01T00:00:00Z = -2208988800）
        assert_eq!(datetime_parts(-2_208_988_800), (1900, 1, 1, 0, 0, 0));
    }

    #[test]
    fn cells_bom_quoting_sentinels_and_zero_rules() {
        let dir = tmpdir("cells");
        let sets = vec![set(
            -1,
            "本地,图\"名",
            "艺术家A",
            BeatmapStatus::Unknown,
            None,
            vec![diff(-1, "Hard", Some(0.0), None)],
            SourceKind::Stable,
        )];
        let rep = write_library_csv(
            &sets,
            &std::collections::HashMap::new(),
            &dir,
            1_700_000_000,
        )
        .unwrap();
        let bytes = fs::read(&rep.file_path).unwrap();
        assert_eq!(&bytes[..3], &[0xEF, 0xBB, 0xBF], "UTF-8 BOM");
        assert!(!bytes[3..].contains(&b'\r'), "LF only");
        let text = String::from_utf8(bytes[3..].to_vec()).unwrap();
        let line = text.lines().nth(1).unwrap();
        // 逗号+引号字段被最小引号包裹，内部引号双写
        assert!(line.contains("\"本地,图\"\"名\""), "got: {line}");
        let rec = csv::ReaderBuilder::new()
            .has_headers(false)
            .flexible(true)
            .from_reader(line.as_bytes())
            .records()
            .next()
            .unwrap()
            .unwrap();
        let cells: Vec<String> = rec.iter().map(|s| s.to_string()).collect();
        // beatmapset_id / beatmap_id 本地哨兵 → 空
        assert_eq!(cells[2], "", "local set id empty");
        assert_eq!(cells[3], "", "local beatmap id empty");
        // stars 0（未评）→ 空；md5 null → 空；ranked_status Unknown → 空
        assert_eq!(cells[13], "", "unrated 0 stars empty");
        assert_eq!(cells[11], "", "null md5 empty");
        assert_eq!(cells[12], "", "unknown status empty, not 0");
        // online 未同步 → 6 列空
        for (i, cell) in cells.iter().enumerate().skip(25).take(6) {
            assert_eq!(cell, "", "online col {i} empty");
        }
        // library_source / 版本 / 时间
        assert_eq!(cells[0], "2");
        assert_eq!(cells[1], "2023-11-14T22:13:20Z");
        assert_eq!(cells[32], "stable");
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn golden_three_sets_five_difficulties() {
        let dir = tmpdir("golden");
        let sets = vec![
            set(
                5001,
                "Zeta",
                "AA",
                BeatmapStatus::Loved,
                Some(1_600_000_000),
                vec![
                    diff(7002, "B", Some(3.25), Some("bbb")),
                    diff(7001, "A", Some(5.5), Some("aaa")),
                ],
                SourceKind::Lazer,
            ),
            set(
                -1,
                "Local",
                "ZZ",
                BeatmapStatus::Unknown,
                None,
                vec![diff(-1, "Insane", None, Some("ccc"))],
                SourceKind::Stable,
            ),
            set(
                1001,
                "Alpha",
                "BB",
                BeatmapStatus::Ranked,
                Some(1_500_000_000),
                vec![
                    diff(9002, "Expert", Some(6.75), Some("eee")),
                    diff(9001, "Easy", Some(2.0), Some("ddd")),
                ],
                SourceKind::OszFolder,
            ),
        ];
        let rep = write_library_csv(
            &sets,
            &std::collections::HashMap::new(),
            &dir,
            1_720_000_000,
        )
        .unwrap();
        assert_eq!(rep.sets, 3);
        assert_eq!(rep.rows, 5);
        let bytes = fs::read(&rep.file_path).unwrap();
        assert_eq!(&bytes[..3], &[0xEF, 0xBB, 0xBF]);
        let text = String::from_utf8_lossy(&bytes[3..]).replace("\r\n", "\n");
        let mut lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 6); // header + 5
        let header = lines.remove(0);
        assert_eq!(
            header,
            "seekman_export_version,exported_at,beatmapset_id,beatmap_id,artist,artist_unicode,title,title_unicode,creator,version,mode,md5,ranked_status,stars,ar,cs,hp,od,bpm,total_time,object_count,source,tags,beatmap_creator,beatmapset_status,favourite_count,play_count,rating,genre,language,online_synced_at,date_added,library_source"
        );
        // 行序：set id 升序（本地最后）→ 星降序 → version 升序
        let versions: Vec<String> = lines
            .iter()
            .map(|l| {
                let c: Vec<String> = csv::ReaderBuilder::new()
                    .has_headers(false)
                    .flexible(true)
                    .from_reader(l.as_bytes())
                    .records()
                    .next()
                    .unwrap()
                    .unwrap()
                    .iter()
                    .map(|s| s.to_string())
                    .collect();
                format!("{}|{}|{}", c[2], c[13], c[9])
            })
            .collect();
        assert_eq!(
            versions,
            vec![
                "1001|6.7500|Expert",
                "1001|2.0000|Easy",
                "5001|5.5000|A",
                "5001|3.2500|B",
                "||Insane",
            ]
        );
        // 冻结一行全文（lines[2] = 5001 首个难度 A，星 5.5 降序在前）验证单元格格式
        assert_eq!(
            lines[2],
            "2,2024-07-03T09:46:40Z,5001,7001,AA,AA,Zeta,Zeta,cr,A,osu,aaa,4,5.5000,9.50,4.00,5.00,8.25,180.000,134,456,anime,t1 t2,nm,loved,,,,,,,2020-09-13T12:26:40Z,lazer"
        );
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn online_cache_merge_fills_synced_only() {
        let dir = tmpdir("merge");
        let mut map = std::collections::HashMap::new();
        map.insert(
            1001i64,
            OnlineSetMeta {
                beatmapset_id: 1001,
                favourite_count: 42,
                play_count: 1000,
                rating: Some(97.5),
                genre: Some("Games".into()),
                language: Some("English".into()),
                fetched_at: 1_700_000_000,
            },
        );
        let sets = vec![
            set(
                1001,
                "Synced",
                "S",
                BeatmapStatus::Ranked,
                None,
                vec![diff(9, "D", Some(1.5), Some("m"))],
                SourceKind::Lazer,
            ),
            set(
                2002,
                "Unsynced",
                "U",
                BeatmapStatus::Pending,
                None,
                vec![diff(8, "D", Some(1.5), Some("m2"))],
                SourceKind::Lazer,
            ),
        ];
        let rep = write_library_csv(&sets, &map, &dir, 1_720_000_000).unwrap();
        let bytes = fs::read(&rep.file_path).unwrap();
        let text = String::from_utf8_lossy(&bytes[3..]).replace("\r\n", "\n");
        let lines: Vec<&str> = text.lines().skip(1).collect();
        let parse = |l: &str| -> Vec<String> {
            csv::ReaderBuilder::new()
                .has_headers(false)
                .flexible(true)
                .from_reader(l.as_bytes())
                .records()
                .next()
                .unwrap()
                .unwrap()
                .iter()
                .map(|s| s.to_string())
                .collect()
        };
        let a = parse(lines[0]); // 1001 synced → 先排序在前
        assert_eq!(&a[2], "1001");
        assert_eq!(&a[25], "42");
        assert_eq!(&a[26], "1000");
        assert_eq!(&a[27], "97.50");
        assert_eq!(&a[28], "Games");
        assert_eq!(&a[29], "English");
        assert_eq!(&a[30], "2023-11-14T22:13:20Z");
        let b = parse(lines[1]);
        assert_eq!(&b[2], "2002");
        for (i, cell) in b.iter().enumerate().skip(25).take(6) {
            assert_eq!(cell, "", "unsynced cell {i} empty");
        }
        fs::remove_dir_all(&dir).ok();
    }
}
