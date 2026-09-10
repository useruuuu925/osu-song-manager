use crate::model::{BeatmapSetInfo, SourceKind};
use crate::osu_parser::parse_osu;
use std::fs;
use std::path::Path;

/// .osu 文件内容 MD5（osu! 定义：文件字节流哈希）
fn bytes_md5(bytes: &[u8]) -> String {
    use md5::{Digest, Md5};
    let mut h = Md5::new();
    h.update(bytes);
    format!("{:x}", h.finalize())
}

/// 扫描 stable Songs 目录
pub fn scan_stable(
    songs_dir: &str,
    progress: &dyn Fn(u32, u32),
) -> Result<Vec<BeatmapSetInfo>, String> {
    use crate::errcode;
    let root = Path::new(songs_dir);
    if !root.is_dir() {
        return Err(errcode::ec1(errcode::SONGS_DIR_NOT_FOUND, songs_dir));
    }

    let mut dirs: Vec<_> = fs::read_dir(root)
        .map_err(|e| errcode::ec1(errcode::READ_DIR_FAILED, e))?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    dirs.sort();

    let total = dirs.len() as u32;
    let mut out = Vec::with_capacity(dirs.len());

    for (i, dir) in dirs.iter().enumerate() {
        if i % 25 == 0 {
            progress(i as u32, total);
        }
        if let Some(set) = scan_stable_one(dir) {
            out.push(set);
        }
    }
    progress(total, total);
    Ok(out)
}

/// 解析单个 Songs 子文件夹；没有任何 .osu 时返回 None
fn scan_stable_one(dir: &Path) -> Option<BeatmapSetInfo> {
    let mut osu_files: Vec<_> = fs::read_dir(dir)
        .ok()?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| {
            p.extension()
                .map(|x| x.eq_ignore_ascii_case("osu"))
                .unwrap_or(false)
        })
        .collect();
    osu_files.sort();

    let mut metas = Vec::with_capacity(osu_files.len());
    let mut md5s: Vec<Option<String>> = Vec::with_capacity(osu_files.len());
    for f in &osu_files {
        if let Ok(bytes) = fs::read(f) {
            md5s.push(Some(bytes_md5(&bytes)));
            let text = String::from_utf8_lossy(&bytes);
            metas.push(parse_osu(&text));
        }
    }
    if metas.is_empty() {
        return None;
    }

    let first = &metas[0];
    // 目录内实际文件名列表，用于大小写不敏感地定位背景/音频
    let actual_files: Vec<String> = fs::read_dir(dir)
        .map(|rd| {
            rd.filter_map(|e| e.ok())
                .filter(|e| e.file_type().map(|t| t.is_file()).unwrap_or(false))
                .filter_map(|e| e.file_name().into_string().ok())
                .collect()
        })
        .unwrap_or_default();

    let bg_name = metas.iter().find_map(|m| m.background.clone());
    let bg_resolved = bg_name.as_ref().and_then(|name| {
        actual_files
            .iter()
            .find(|f| f.eq_ignore_ascii_case(name))
            .map(|f| dir.join(f).to_string_lossy().into_owned())
    });
    let audio = first.audio_filename.clone();

    Some(BeatmapSetInfo {
        beatmapset_id: first.beatmapset_id,
        title: first.title.clone(),
        title_unicode: first.title_unicode.clone(),
        artist: first.artist.clone(),
        artist_unicode: first.artist_unicode.clone(),
        creator: first.creator.clone(),
        source: first.source.clone(),
        tags: first.tags.clone(),
        difficulties: metas
            .iter()
            .zip(md5s.iter())
            .map(|(m, h)| {
                let mut bi = m.to_beatmap_info();
                bi.md5 = h.clone();
                bi
            })
            .collect(),
        background: bg_name,
        background_path: bg_resolved,
        audio_filename: if audio.is_empty() { None } else { Some(audio) },
        source_kind: SourceKind::Stable,
        location: dir.to_string_lossy().into_owned(),
        status: crate::model::BeatmapStatus::Unknown,
        date_added: None,
    })
}

/// 扫描存放 .osz 的文件夹（zip 内存解析，不落盘）
pub fn scan_osz_folder(
    folder: &str,
    progress: &dyn Fn(u32, u32),
) -> Result<Vec<BeatmapSetInfo>, String> {
    use crate::errcode;
    let root = Path::new(folder);
    if !root.is_dir() {
        return Err(errcode::ec1(errcode::DIR_NOT_FOUND, folder));
    }

    let mut files: Vec<_> = fs::read_dir(root)
        .map_err(|e| errcode::ec1(errcode::READ_DIR_FAILED, e))?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| {
            p.is_file()
                && p.extension()
                    .map(|x| x.eq_ignore_ascii_case("osz"))
                    .unwrap_or(false)
        })
        .collect();
    files.sort();

    let total = files.len() as u32;
    let mut out = Vec::with_capacity(files.len());

    for (i, f) in files.iter().enumerate() {
        progress(i as u32, total);
        match scan_osz_one(f) {
            Ok(Some(set)) => out.push(set),
            Ok(None) => {}
            Err(e) => eprintln!("skip bad osz {}: {e}", f.display()),
        }
    }
    progress(total, total);
    Ok(out)
}

/// 解析单个 .osz；zip 损坏时返回 Err
fn scan_osz_one(osz_path: &Path) -> Result<Option<BeatmapSetInfo>, String> {
    let file = fs::File::open(osz_path).map_err(|e| e.to_string())?;
    let mut zip = zip::ZipArchive::new(file).map_err(|e| e.to_string())?;

    // 先收集全部条目名，zip 内路径分隔符统一成 /
    // 不能用闭包 map：ZipFile 借用 zip，引用不能逃出 FnMut 闭包
    let mut entry_names: Vec<String> = Vec::new();
    for i in 0..zip.len() {
        if let Ok(e) = zip.by_index(i) {
            if !e.is_dir() {
                entry_names.push(e.name().replace('\\', "/"));
            }
        }
    }

    let mut metas = Vec::new();
    let mut md5s: Vec<Option<String>> = Vec::new();
    for name in &entry_names {
        // 只要根目录下的 .osu（谱面打包约定如此）
        if name.to_lowercase().ends_with(".osu") && !name.contains('/') {
            if let Ok(mut entry) = zip.by_name(name) {
                let mut buf = Vec::new();
                if std::io::Read::read_to_end(&mut entry, &mut buf).is_ok() {
                    md5s.push(Some(bytes_md5(&buf)));
                    metas.push(parse_osu(&String::from_utf8_lossy(&buf)));
                }
            }
        }
    }
    if metas.is_empty() {
        return Ok(None);
    }

    let first = &metas[0];
    let bg_name = metas.iter().find_map(|m| m.background.clone());
    let bg_entry = bg_name.as_ref().and_then(|name| {
        let norm = name.replace('\\', "/");
        entry_names
            .iter()
            .find(|f| f.to_lowercase() == norm.to_lowercase())
            .cloned()
    });

    let audio = first.audio_filename.clone();
    Ok(Some(BeatmapSetInfo {
        beatmapset_id: first.beatmapset_id,
        title: first.title.clone(),
        title_unicode: first.title_unicode.clone(),
        artist: first.artist.clone(),
        artist_unicode: first.artist_unicode.clone(),
        creator: first.creator.clone(),
        source: first.source.clone(),
        tags: first.tags.clone(),
        difficulties: metas
            .iter()
            .zip(md5s.iter())
            .map(|(m, h)| {
                let mut bi = m.to_beatmap_info();
                bi.md5 = h.clone();
                bi
            })
            .collect(),
        background: bg_name,
        // 压缩包内条目名；实际取图时由 location（.osz 路径）+ 此名解出
        background_path: bg_entry,
        audio_filename: if audio.is_empty() { None } else { Some(audio) },
        source_kind: SourceKind::OszFolder,
        location: osz_path.to_string_lossy().into_owned(),
        status: crate::model::BeatmapStatus::Unknown,
        date_added: None,
    }))
}

/// 统一入口：按模式扫描
pub fn scan_library(
    kind: SourceKind,
    path: &str,
    progress: &dyn Fn(u32, u32),
) -> Result<Vec<BeatmapSetInfo>, String> {
    match kind {
        SourceKind::Stable => scan_stable(path, progress),
        SourceKind::OszFolder => scan_osz_folder(path, progress),
        SourceKind::Lazer => {
            // 安装目录输入先透明解析为数据目录，再做只读 realm 扫描
            let resolved = crate::detect::resolve_lazer_data_dir(Path::new(path));
            crate::realm_db::scan_lazer(&resolved, progress)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    /// 唯一 temp fixture 目录，Drop 时 best-effort 清理（容忍失败）。
    struct TempFixture(PathBuf);

    impl TempFixture {
        fn new(tag: &str) -> TempFixture {
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let dir = std::env::temp_dir()
                .join(format!("osu-sm-lib-{tag}-{}-{nanos}", std::process::id()));
            fs::create_dir_all(&dir).expect("fixture root");
            Self(dir)
        }
    }

    impl Drop for TempFixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    /// .osu 文件内容，基于 osu_parser.rs 测试样例改写；title/artist 不同，
    /// BPM 由首条未继承 TimingPoint 的 beatLength(ms) 推导：bpm = 60000 / beat_length。
    fn osu_file(title: &str, artist: &str, beat_length_ms: f64) -> String {
        format!(
            "osu file format v14

[General]
AudioFilename: audio.mp3
Mode: 0

[Metadata]
Title:{title}
TitleUnicode:{title}
Artist:{artist}
ArtistUnicode:{artist}
Creator:tester
Version:Insane
Tags:fixture test

[Difficulty]
HPDrainRate:5
CircleSize:4
OverallDifficulty:8
ApproachRate:5

[TimingPoints]
0,{beat_length_ms},4,2,1,40,1,0

[HitObjects]
256,192,1000,1,0,0
256,192,2000,1,0,0
256,192,3000,5,2,0:0:0:0,1,0
"
        )
    }

    #[test]
    fn scan_stable_finds_two_song_subfolders() {
        let fixture = TempFixture::new("songs");
        let songs = fixture.0.join("Songs");
        fs::create_dir_all(&songs).unwrap();

        let first = songs.join("1001 - Alpha Song [Insane]");
        fs::create_dir_all(&first).unwrap();
        fs::write(
            first.join("Alpha One [Insane].osu"),
            osu_file("Alpha One", "Artist A", 500.0),
        )
        .unwrap();
        fs::write(
            first.join("Alpha One [Another].osu"),
            osu_file("Alpha One", "Artist A", 500.0),
        )
        .unwrap();

        let second = songs.join("1002 - Beta Song [Hard]");
        fs::create_dir_all(&second).unwrap();
        fs::write(
            second.join("Beta Song [Hard].osu"),
            osu_file("Beta Song", "Artist B", 300.0),
        )
        .unwrap();

        let empty = songs.join("not a beatmap");
        fs::create_dir_all(&empty).unwrap();
        fs::write(empty.join("readme.txt"), "no osu here").unwrap();

        let progress_calls = std::cell::Cell::new(0u32);
        let sets = scan_stable(&songs.to_string_lossy(), &|done, total| {
            let _ = (done, total);
            progress_calls.set(progress_calls.get() + 1);
        })
        .expect("scan_stable");

        assert_eq!(sets.len(), 2, "two beatmapsets found");
        assert!(progress_calls.get() > 0, "progress callback fired");

        // dirs 按路径排序：Alpha 在前
        let first_set = &sets[0];
        assert_eq!(first_set.title, "Alpha One");
        assert_eq!(first_set.artist, "Artist A");
        assert_eq!(first_set.difficulties.len(), 2);
        assert!(
            (first_set.difficulties[0].bpm - 120.0).abs() < 0.01,
            "first BPM should be 120, got {}",
            first_set.difficulties[0].bpm
        );

        let second_set = &sets[1];
        assert_eq!(second_set.title, "Beta Song");
        assert_eq!(second_set.artist, "Artist B");
        assert_eq!(second_set.difficulties.len(), 1);
        assert!(
            (second_set.difficulties[0].bpm - 200.0).abs() < 0.01,
            "second BPM should be 200, got {}",
            second_set.difficulties[0].bpm
        );
    }

    #[test]
    fn scan_osz_folder_reads_zip_fixture() {
        use std::io::Write;

        let fixture = TempFixture::new("osz");
        let file = fs::File::create(fixture.0.join("beatmap.osz")).unwrap();
        {
            // zip 2.x + deflate（与 Cargo.toml 依赖一致）
            let opts = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated);
            let mut writer = zip::ZipWriter::new(file);
            writer.start_file("Zipped Tune [Insane].osu", opts).unwrap();
            writer
                .write_all(osu_file("Zipped Tune", "Artist C", 400.0).as_bytes())
                .unwrap();
            writer.finish().unwrap();
        }

        let sets = scan_osz_folder(&fixture.0.to_string_lossy(), &|_, _| {}).unwrap();
        assert_eq!(sets.len(), 1, "one set parsed");
        let set = &sets[0];
        assert_eq!(set.title, "Zipped Tune");
        assert_eq!(set.artist, "Artist C");
        assert_eq!(set.difficulties.len(), 1);
        assert!(
            (set.difficulties[0].bpm - 150.0).abs() < 0.01,
            "osz BPM should be 150, got {}",
            set.difficulties[0].bpm
        );
    }
}
