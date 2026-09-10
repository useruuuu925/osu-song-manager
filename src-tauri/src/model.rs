use serde::{Deserialize, Serialize};

/// osu! 游戏模式
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GameMode {
    Osu,
    Taiko,
    Catch,
    Mania,
}

impl GameMode {
    pub fn from_int(v: u8) -> GameMode {
        match v {
            1 => GameMode::Taiko,
            2 => GameMode::Catch,
            3 => GameMode::Mania,
            _ => GameMode::Osu,
        }
    }

    /// 仅诊断测试使用
    #[cfg(test)]
    pub fn display(&self) -> &'static str {
        match self {
            GameMode::Osu => "osu!",
            GameMode::Taiko => "taiko",
            GameMode::Catch => "catch",
            GameMode::Mania => "mania",
        }
    }
}

/// 单个难度（一张 .osu）
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BeatmapInfo {
    pub beatmap_id: i64,
    pub mode: GameMode,
    /// 难度名，如 "Another"
    pub version: String,
    /// 谱师（单难度级别）
    pub creator: String,
    pub cs: f32,
    pub ar: f32,
    pub od: f32,
    pub hp: f32,
    /// 主 BPM
    pub bpm: f64,
    /// 总时长（毫秒）
    pub total_ms: i64,
    /// 对象数
    pub object_count: u32,
    /// .osu 文件自身 MD5（stable 模式计算；lazer 由数据库提供）
    pub md5: Option<String>,
    /// lazer 已算好的星数
    pub star_rating: Option<f64>,
}

/// 谱面集在线状态（lazer BeatmapOnlineStatus 持久化数值的映射结果）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BeatmapStatus {
    Graveyard,
    Wip,
    Pending,
    Ranked,
    Approved,
    Qualified,
    Loved,
    Unknown,
}

impl BeatmapStatus {
    /// 对应 ppy/osu `BeatmapOnlineStatus`（与 osu-web API 数值一致）：
    /// -3 graveyard / -2 wip / -1 pending / 0 none(未上架) / 1 ranked /
    /// 2 approved / 3 qualified / 4 loved；其余 → Unknown。
    pub fn from_lazer_numeric(v: i64) -> BeatmapStatus {
        match v {
            -3 => BeatmapStatus::Graveyard,
            -2 => BeatmapStatus::Wip,
            -1 => BeatmapStatus::Pending,
            1 => BeatmapStatus::Ranked,
            2 => BeatmapStatus::Approved,
            3 => BeatmapStatus::Qualified,
            4 => BeatmapStatus::Loved,
            _ => BeatmapStatus::Unknown,
        }
    }
}

/// 谱面集（一个文件夹 / 一个 .osz / 一条 BeatmapSet 记录）
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BeatmapSetInfo {
    pub beatmapset_id: i64,
    pub title: String,
    pub title_unicode: String,
    pub artist: String,
    pub artist_unicode: String,
    /// 谱师（取首个难度的 Creator）
    pub creator: String,
    pub source: String,
    pub tags: String,
    pub difficulties: Vec<BeatmapInfo>,
    /// 背景图相对文件名（如 "bg.jpg"）
    pub background: Option<String>,
    /// 背景图的绝对可用路径（stable=文件系统路径；lazer=files/xx/hash；osz=压缩包内条目名）
    pub background_path: Option<String>,
    /// 音频文件名
    pub audio_filename: Option<String>,
    /// 来源模式标识
    pub source_kind: SourceKind,
    /// stable：Songs 下的文件夹绝对路径；osz：.osz 文件绝对路径；lazer：数字 ID 文件夹名
    pub location: String,
    /// 集级在线状态（lazer 才有；stable/.osz 为 Unknown）
    pub status: BeatmapStatus,
    /// lazer：DateAdded（Unix 秒）；stable/.osz 无此信息 → None
    pub date_added: Option<i64>,
}

impl BeatmapSetInfo {
    pub fn display_title(&self) -> String {
        if !self.title.is_empty() {
            self.title.clone()
        } else {
            self.title_unicode.clone()
        }
    }

    pub fn display_artist(&self) -> String {
        if !self.artist.is_empty() {
            self.artist.clone()
        } else {
            self.artist_unicode.clone()
        }
    }

    /// 仅诊断测试使用
    #[cfg(test)]
    pub fn max_bpm(&self) -> f64 {
        self.difficulties.iter().map(|d| d.bpm).fold(0.0, f64::max)
    }

    /// 仅诊断测试使用
    #[cfg(test)]
    pub fn max_total_ms(&self) -> i64 {
        self.difficulties
            .iter()
            .map(|d| d.total_ms)
            .fold(0, i64::max)
    }
}

/// 曲库来源模式
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SourceKind {
    /// osu!lazer 数据目录（只读解析 client.realm）
    Lazer,
    /// osu!stable Songs 目录
    Stable,
    /// 存放 .osz 文件的文件夹
    OszFolder,
}

/// 检测到的候选曲库
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LibraryCandidate {
    pub kind: SourceKind,
    pub path: String,
    /// 简短说明（为什么认为是这个）
    pub detail: String,
}

/// 谱面集的在线元数据（osu API v2 / 镜像；冻结契约 M4c）
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OnlineSetMeta {
    pub beatmapset_id: i64,
    pub favourite_count: u64,
    pub play_count: u64,
    /// 0~100，官方对冷门集可能为 null/0
    pub rating: Option<f64>,
    pub genre: Option<String>,
    pub language: Option<String>,
    /// 抓取时间（Unix 秒）——前端据此自行判断陈旧度
    pub fetched_at: i64,
}

/// 在线管线状态（冻结契约 M4c）
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OnlineStatus {
    pub logged_in: bool,
    /// "official" | "mirror" | "none"
    pub source: String,
    pub cached_count: u64,
}

/// OAuth 客户端配置视图（secret 永不回传；冻结契约 M4c 末项的既有风格适配）
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OauthConfigView {
    pub client_id: String,
    pub has_secret: bool,
}

// ── M5 缩略图 / 导出 ─────────────────────────────────────────────────────────

/// 缩略图生成请求（冻结契约 M5）
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThumbRequest {
    pub set_id: i64,
    /// 背景图绝对路径（stable 文件 / lazer files\<hash>，只读）
    pub source_path: String,
    pub size: u32,
}

/// 缩略图生成结果：key 为缓存文件名（供 thumb 协议取用）；失败时 key=null, ok=false
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThumbResult {
    pub set_id: i64,
    pub key: Option<String>,
    pub ok: bool,
}

/// 背景图批量导出条目
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportSet {
    pub set_id: i64,
    pub artist: String,
    pub title: String,
    pub creator: String,
    pub source_path: String,
}

/// 导出结果报告
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportReport {
    pub exported: u32,
    pub renamed: u32,
    pub failed: u32,
    pub errors: Vec<String>,
}

/// M8：曲库 CSV 导出结果（export_library_csv 返回值；camelCase 与前端约定一致）
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportCsvReport {
    pub file_path: String,
    pub sets: u32,
    pub rows: u32,
}

/// export-progress 事件负载
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportProgress {
    pub done: u32,
    pub total: u32,
    pub current: String,
}

// ── M6 库管理（查重 / 回收站 / 移动 / 打包） ─────────────────────────────────

/// 重复项分组内的一条成员
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DupMember {
    pub set_id: i64,
    pub title: String,
    pub artist: String,
    pub creator: String,
    pub difficulty_count: u32,
    /// 扫描结果原样：stable=集文件夹路径；osz=.osz 文件路径；lazer=数字 ID 字符串
    pub location: String,
    /// 目录遍历总大小 / .osz 文件大小；lazer 为 0（只读不触碰）
    pub size_bytes: u64,
    /// 组内最老成员的 dateAdded（无则 null）
    pub first_added: Option<i64>,
}

/// 一组重复（reason: "setOnlineId" | "md5" | "identicalSet"）
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DuplicateGroup {
    pub reason: String,
    pub label: String,
    pub members: Vec<DupMember>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FailedPath {
    pub path: String,
    pub error: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TrashReport {
    pub deleted: Vec<String>,
    pub failed: Vec<FailedPath>,
}

/// moved 记录的是目标端最终路径（含改名后的名字）
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MoveReport {
    pub moved: Vec<String>,
    pub failed: Vec<FailedPath>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PackReport {
    pub packed: u32,
    pub bytes: u64,
    pub skipped: Vec<String>,
}

// ── M7 镜像链下载 ─────────────────────────────────────────────────────────────

/// download-progress 事件状态（serde 小写）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DownloadState {
    Queued,
    Downloading,
    Validating,
    Done,
    Failed,
    Cancelled,
}

/// 进度事件负载（每次状态变更必发；字节进度 ≥150ms 节流）
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadProgress {
    pub set_id: u64,
    pub state: DownloadState,
    pub mirror: Option<String>,
    pub received: u64,
    /// Content-Length；未知为 0
    pub total: u64,
    pub error: Option<String>,
}

/// 每个 set 的最终下载结果（download_osz 返回，按去重后的输入顺序）
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadResult {
    pub set_id: u64,
    pub ok: bool,
    pub path: Option<String>,
    pub mirror: Option<String>,
    pub bytes: u64,
    pub error: Option<String>,
}

// ── T2 lazer→stable 转换 ─────────────────────────────────────────────────────

/// convert-progress 事件状态（serde 小写）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ConvertState {
    Queued,
    Converting,
    Done,
    Failed,
    Cancelled,
}

/// convert-progress 事件负载（每集状态变更必发）
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConvertProgress {
    pub done: u32,
    pub total: u32,
    pub set_id: i64,
    pub state: ConvertState,
    pub current: String,
    pub error: Option<String>,
}

/// 单集转换结果（convert_lazer_to_stable 返回的逐集明细）
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConvertSetResult {
    pub set_id: i64,
    pub ok: bool,
    /// 产物路径（集文件夹或 .osz 文件）
    pub output: Option<String>,
    pub files: u32,
    pub bytes: u64,
    pub error: Option<String>,
    /// 非致命警告（err.* 码 + 参数，如 stable 不支持的音频/视频格式）
    #[serde(default)]
    pub warnings: Vec<String>,
}

/// 转换批次总报告
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConvertReport {
    pub total: u32,
    pub converted: u32,
    pub failed: u32,
    pub cancelled: bool,
    pub output_dir: String,
    pub results: Vec<ConvertSetResult>,
}
