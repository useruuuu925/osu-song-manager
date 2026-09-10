use crate::model::{BeatmapInfo, GameMode};

/// 一张 .osu 解析出的完整元数据
#[derive(Debug, Clone, Default)]
pub struct OsuFileMeta {
    pub audio_filename: String,
    pub mode: u8,
    pub title: String,
    pub title_unicode: String,
    pub artist: String,
    pub artist_unicode: String,
    pub creator: String,
    pub version: String,
    pub source: String,
    pub tags: String,
    pub beatmap_id: i64,
    pub beatmapset_id: i64,
    pub cs: f32,
    pub ar: f32,
    pub od: f32,
    pub hp: f32,
    pub bpm: f64,
    pub total_ms: i64,
    pub object_count: u32,
    pub background: Option<String>,
}

/// 解析 .osu 文本内容（UTF-8 已解码）
pub fn parse_osu(content: &str) -> OsuFileMeta {
    let mut meta = OsuFileMeta::default();
    let mut section = String::new();
    // 时间点：(offset, beatLength)，仅未继承点
    let mut timing: Vec<(f64, f64)> = Vec::new();

    for raw_line in content.lines() {
        let line = raw_line.trim_end_matches('\r').trim();
        if line.is_empty() || line.starts_with("//") {
            continue;
        }
        if line.starts_with('[') && line.ends_with(']') {
            section = line[1..line.len() - 1].to_string();
            continue;
        }

        match section.as_str() {
            "General" => {
                if let Some((k, v)) = split_kv(line) {
                    match k {
                        "AudioFilename" => meta.audio_filename = v.to_string(),
                        "Mode" => meta.mode = v.parse().unwrap_or(0),
                        _ => {}
                    }
                }
            }
            "Metadata" => {
                if let Some((k, v)) = split_kv(line) {
                    match k {
                        "Title" => meta.title = v.to_string(),
                        "TitleUnicode" => meta.title_unicode = v.to_string(),
                        "Artist" => meta.artist = v.to_string(),
                        "ArtistUnicode" => meta.artist_unicode = v.to_string(),
                        "Creator" => meta.creator = v.to_string(),
                        "Version" => meta.version = v.to_string(),
                        "Source" => meta.source = v.to_string(),
                        "Tags" => meta.tags = v.to_string(),
                        "BeatmapID" => meta.beatmap_id = v.parse().unwrap_or(0),
                        "BeatmapSetID" => meta.beatmapset_id = v.parse().unwrap_or(0),
                        _ => {}
                    }
                }
            }
            "Difficulty" => {
                if let Some((k, v)) = split_kv(line) {
                    match k {
                        "CircleSize" => meta.cs = v.parse().unwrap_or(0.0),
                        "ApproachRate" => meta.ar = v.parse().unwrap_or(0.0),
                        "OverallDifficulty" => meta.od = v.parse().unwrap_or(0.0),
                        "HPDrainRate" => meta.hp = v.parse().unwrap_or(0.0),
                        _ => {}
                    }
                }
            }
            "Events" => {
                // 背景行形如: 0,0,"bg.jpg",0,0
                if let Some(rest) = line.strip_prefix("0,0,\"") {
                    if let Some(end) = rest.find('"') {
                        meta.background = Some(rest[..end].to_string());
                    }
                }
            }
            "TimingPoints" => {
                // offset,beatLength,meter,sampleSet,sampleIndex,volume,uninherited,effects
                let parts: Vec<&str> = line.split(',').collect();
                if parts.len() >= 7 {
                    let offset: f64 = parts[0].parse().unwrap_or(0.0);
                    let beat_length: f64 = parts[1].parse().unwrap_or(0.0);
                    let uninherited: i32 = parts[6].parse().unwrap_or(0);
                    if uninherited == 1 && beat_length > 0.0 {
                        timing.push((offset, beat_length));
                    }
                }
            }
            "HitObjects" => {
                meta.object_count += 1;
                // x,y,time,type,...  time 是第三个字段
                let parts: Vec<&str> = line.split(',').collect();
                if parts.len() >= 3 {
                    if let Ok(t) = parts[2].parse::<i64>() {
                        meta.total_ms = meta.total_ms.max(t);
                    }
                }
            }
            _ => {}
        }
    }

    meta.bpm = dominant_bpm(&timing, meta.total_ms);
    // 补一个收尾余量，接近游戏内显示的时长
    meta.total_ms += 600;
    meta
}

fn split_kv(line: &str) -> Option<(&str, &str)> {
    let idx = line.find(':')?;
    let key = line[..idx].trim();
    let value = line[idx + 1..].trim();
    Some((key, value))
}

/// 按持续时间加权，取占主导的 BPM：每个未继承时间点统治到下一个时间点
/// （或谱面结束），相同 BPM 的段落权重累计，取最大者。
fn dominant_bpm(timing: &[(f64, f64)], end_ms: i64) -> f64 {
    if timing.is_empty() {
        return 0.0;
    }
    let end = end_ms as f64;
    let mut acc: Vec<(f64, f64)> = Vec::new(); // (bpm, 累计毫秒)
    for (i, &(offset, beat_length)) in timing.iter().enumerate() {
        let next_offset = timing
            .get(i + 1)
            .map(|&(n, _)| n)
            .unwrap_or(end.max(offset + beat_length));
        let dur = (next_offset - offset).max(0.0);
        if dur <= 0.0 {
            continue;
        }
        let bpm = (60000.0 / beat_length * 10.0).round() / 10.0;
        if let Some(slot) = acc.iter_mut().find(|(b, _)| (*b - bpm).abs() < 0.05) {
            slot.1 += dur;
        } else {
            acc.push((bpm, dur));
        }
    }
    acc.iter()
        .fold((0.0, f64::MIN), |a, b| if b.1 > a.1 { *b } else { a })
        .0
}

impl OsuFileMeta {
    pub fn to_beatmap_info(&self) -> BeatmapInfo {
        BeatmapInfo {
            beatmap_id: self.beatmap_id,
            mode: GameMode::from_int(self.mode),
            version: self.version.clone(),
            creator: self.creator.clone(),
            cs: self.cs,
            ar: self.ar,
            od: self.od,
            hp: self.hp,
            bpm: self.bpm,
            total_ms: self.total_ms,
            object_count: self.object_count,
            md5: None,
            star_rating: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "osu file format v14\n\
\n\
[General]\n\
AudioFilename: audio.mp3\n\
AudioLeadIn: 0\n\
Mode: 3\n\
\n\
[Metadata]\n\
Title:Sample Song\n\
TitleUnicode:サンプル曲\n\
Artist:Test Artist\n\
ArtistUnicode:テスト\n\
Creator: mapper_name\n\
Version:Another\n\
Source:Anime X\n\
Tags:tag1 tag2\n\
BeatmapID:111\n\
BeatmapSetID:2222\n\
\n\
[Difficulty]\n\
CircleSize:4\n\
OverallDifficulty:8\n\
ApproachRate:9\n\
HPDrainRate:6\n\
\n\
[Events]\n\
//Background and Video events\n\
0,0,\"bg image.jpg\",0,0\n\
//Break Periods\n\
\n\
[TimingPoints]\n\
1000,500,4,2,1,40,1,0\n\
3000,250,4,2,1,40,1,0\n\
3100,-100,4,2,1,40,0,0\n\
\n\
[HitObjects]\n\
256,192,1000,1,0\n\
256,192,2000,1,0\n\
256,192,4000,1,0\n";

    #[test]
    fn parses_metadata() {
        let m = parse_osu(SAMPLE);
        assert_eq!(m.audio_filename, "audio.mp3");
        assert_eq!(m.mode, 3);
        assert_eq!(m.title, "Sample Song");
        assert_eq!(m.title_unicode, "サンプル曲");
        assert_eq!(m.artist, "Test Artist");
        assert_eq!(m.creator, "mapper_name");
        assert_eq!(m.version, "Another");
        assert_eq!(m.beatmap_id, 111);
        assert_eq!(m.beatmapset_id, 2222);
        assert_eq!(m.cs, 4.0);
        assert_eq!(m.ar, 9.0);
        assert_eq!(m.od, 8.0);
        assert_eq!(m.hp, 6.0);
        assert_eq!(m.background.as_deref(), Some("bg image.jpg"));
        assert_eq!(m.object_count, 3);
    }

    #[test]
    fn computes_bpm_and_duration() {
        let m = parse_osu(SAMPLE);
        // 段1: 1000-3000ms @500ms/beat = 120bpm (2000ms)
        // 段2: 3000-4000ms @250ms/beat = 240bpm (1000ms)
        // 主导 BPM 应为 120
        assert!((m.bpm - 120.0).abs() < 0.01, "bpm was {}", m.bpm);
        // 最后对象 4000ms + 600 余量
        assert_eq!(m.total_ms, 4600);
    }

    #[test]
    fn handles_missing_sections_and_bom() {
        let content = "\u{feff}osu file format v4\n\n[Metadata]\nTitle:Old Map\nBeatmapSetID:0\n";
        let m = parse_osu(content);
        assert_eq!(m.title, "Old Map");
        assert_eq!(m.mode, 0);
        assert_eq!(m.bpm, 0.0);
        assert!(m.background.is_none());
    }

    #[test]
    fn ignores_inherited_points_in_bpm() {
        let content = "osu file format v14\n\n[TimingPoints]\n0,300,4,2,1,40,1,0\n5000,-200,4,2,1,40,0,0\n9000,300,4,2,1,40,1,0\n20000,300,4,2,1,40,1,0\n\n[HitObjects]\n256,192,25000,1,0\n";
        let m = parse_osu(content);
        // 全部 200bpm（300ms/beat），继承点不参与
        assert!((m.bpm - 200.0).abs() < 0.01, "bpm was {}", m.bpm);
    }
}
