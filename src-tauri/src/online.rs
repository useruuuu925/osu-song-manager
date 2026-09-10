// online.rs
//
// Purpose: M4c 在线元数据管线——谱面集收藏数/游玩数/评分/流派/语言。
// 数据源策略：已登录且有有效 token → 官方 osu API v2（per-set，1.2s 限速）；
// 未登录 / 官方失败（本机对 osu.ppy.sh 为 Cloudflare 403，属常态）→ 镜像
// mirror.hinamizawa.ai（免认证，返回 JSON 与官方逐字节同构，间隔 120ms，重试 2 次）。
// 所有缓存/凭据落在应用配置目录（与 config.json 同目录），client.realm 依旧只读。
//
// OAuth：authorization_code（无 PKCE），固定回环 redirect http://localhost:47821/callback；
// 支持浏览器回跳（后台 TcpListener 线程）与手动粘贴 code 两条路径；token 24h 自动刷新轮换。

use crate::model::{OnlineSetMeta, OnlineStatus};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tauri::Manager;

pub const REDIRECT_URI: &str = "http://localhost:47821/callback";
const AUTHORIZE_URL: &str = "https://osu.ppy.sh/oauth/authorize";
const TOKEN_URL: &str = "https://osu.ppy.sh/oauth/token";
const OFFICIAL_SET_URL: &str = "https://osu.ppy.sh/api/v2/beatmapsets";
const MIRROR_SET_URL: &str = "https://mirror.hinamizawa.ai/v3/osu/beatmaps/s";
/// 计数器/评分 7 天 TTL（genre/language 官方 90 天，但同乘一份 JSON → 以 7 天为准刷新）
pub const REFRESH_TTL_SECS: i64 = 7 * 24 * 3600;
/// 缓存条目上限，超出按 fetched_at 淘汰最旧
pub const MAX_CACHE_ENTRIES: usize = 50_000;
const HTTP_TIMEOUT: Duration = Duration::from_secs(15);
const USER_AGENT: &str = "osu-song-manager/0.1 (https://github.com/local/osu-song-manager)";

fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

// ── 路径 ──────────────────────────────────────────────────────────────────────

pub(crate) fn app_dir(app: &tauri::AppHandle) -> PathBuf {
    app.path()
        .app_data_dir()
        .ok()
        .or_else(|| app.path().app_config_dir().ok())
        .unwrap_or_else(|| {
            let appdata = std::env::var("APPDATA").unwrap_or_else(|_| ".".into());
            PathBuf::from(appdata).join("osu-song-manager")
        })
}

fn cache_file(dir: &Path) -> PathBuf {
    dir.join("online_cache.json")
}
fn oauth_file(dir: &Path) -> PathBuf {
    dir.join("oauth.json")
}
fn tokens_file(dir: &Path) -> PathBuf {
    dir.join("tokens.json")
}

/// 原子写：同目录临时文件 + 删除旧文件 + rename（Windows 下 rename 不覆盖已存在文件）。
fn write_atomic(path: &Path, content: &str) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let tmp = path.with_extension(format!("tmp.{}", rand_u32()));
    fs::write(&tmp, content).map_err(|e| e.to_string())?;
    let _ = fs::remove_file(path);
    fs::rename(&tmp, path).map_err(|e| e.to_string())
}

fn rand_u32() -> u32 {
    let t = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    static C: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let x = t
        ^ (std::process::id() as u64) << 32
        ^ C.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    splitmix64(x) as u32
}

// ── 凭据 / token ─────────────────────────────────────────────────────────────

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct OauthCreds {
    pub client_id: String,
    pub client_secret: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Tokens {
    pub access_token: String,
    pub refresh_token: String,
    pub token_type: String,
    /// Unix 秒；到期前 60s 即视为过期并尝试刷新
    pub expires_at: i64,
}

impl Tokens {
    fn needs_refresh(&self, now: i64) -> bool {
        now >= self.expires_at - 60
    }
}

fn load_creds(dir: &Path) -> OauthCreds {
    fs::read_to_string(oauth_file(dir))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

fn load_tokens(dir: &Path) -> Option<Tokens> {
    fs::read_to_string(tokens_file(dir))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .filter(|tk: &Tokens| !tk.refresh_token.is_empty())
}

fn save_tokens(dir: &Path, tk: &Tokens) -> Result<(), String> {
    write_atomic(
        &tokens_file(dir),
        &serde_json::to_string_pretty(tk).map_err(|e| e.to_string())?,
    )
}

#[derive(Debug, Default, Deserialize)]
struct TokenResp {
    access_token: Option<String>,
    refresh_token: Option<String>,
    token_type: Option<String>,
    expires_in: Option<i64>,
    #[serde(default)]
    error_description: Option<String>,
    #[serde(default)]
    error: Option<String>,
}

fn post_token(fields: &[(&str, String)]) -> Result<Tokens, String> {
    let client = http_client()?;
    let resp = client
        .post(TOKEN_URL)
        .form(fields)
        .send()
        .map_err(|e| crate::errcode::ec1(crate::errcode::NETWORK, e))?;
    let status = resp.status();
    let text = resp.text().unwrap_or_default();
    if !status.is_success() {
        let parsed: TokenResp = serde_json::from_str(&text).unwrap_or_default();
        let msg = parsed
            .error_description
            .or(parsed.error)
            .unwrap_or_else(|| format!("HTTP {status}"));
        return Err(crate::errcode::ec1(crate::errcode::TOKEN_FAILED, msg));
    }
    let parsed: TokenResp = serde_json::from_str(&text)
        .map_err(|e| crate::errcode::ec1(crate::errcode::TOKEN_PARSE_FAILED, e))?;
    let access = parsed
        .access_token
        .ok_or_else(|| crate::errcode::ec(crate::errcode::TOKEN_MISSING_ACCESS))?;
    let refresh = parsed
        .refresh_token
        .ok_or_else(|| crate::errcode::ec(crate::errcode::TOKEN_MISSING_REFRESH))?;
    Ok(Tokens {
        access_token: access,
        refresh_token: refresh,
        token_type: parsed.token_type.unwrap_or_else(|| "Bearer".into()),
        expires_at: now_secs() + parsed.expires_in.unwrap_or(24 * 3600),
    })
}

fn exchange_code(dir: &Path, code: &str) -> Result<(), String> {
    let creds = load_creds(dir);
    if creds.client_id.is_empty() || creds.client_secret.is_empty() {
        return Err(crate::errcode::ec(crate::errcode::OAUTH_CREDS_MISSING));
    }
    let tk = post_token(&[
        ("grant_type", "authorization_code".to_string()),
        ("client_id", creds.client_id.clone()),
        ("client_secret", creds.client_secret.clone()),
        ("code", code.to_string()),
        ("redirect_uri", REDIRECT_URI.to_string()),
    ])?;
    save_tokens(dir, &tk)
}

fn refresh_tokens(dir: &Path) -> Option<Tokens> {
    let creds = load_creds(dir);
    let old = load_tokens(dir)?;
    let tk = post_token(&[
        ("grant_type", "refresh_token".to_string()),
        ("client_id", creds.client_id),
        ("client_secret", creds.client_secret),
        ("refresh_token", old.refresh_token),
    ])
    .ok()?;
    // 成功即轮换保存
    save_tokens(dir, &tk).ok()?;
    Some(tk)
}

/// 当前可用的 access token；过期自动刷新（刷新失败 → None → 走镜像）。
fn valid_access(dir: &Path) -> Option<String> {
    let mut tk = load_tokens(dir)?;
    if tk.needs_refresh(now_secs()) {
        tk = refresh_tokens(dir)?;
    }
    Some(tk.access_token)
}

// ── HTTP ──────────────────────────────────────────────────────────────────────

fn http_client() -> Result<reqwest::blocking::Client, String> {
    reqwest::blocking::Client::builder()
        .timeout(HTTP_TIMEOUT)
        .connect_timeout(HTTP_TIMEOUT)
        .user_agent(USER_AGENT)
        .build()
        .map_err(|e| e.to_string())
}

/// GET JSON 文本。返回 Err 区分「传输/HTTP 状态」两类，便于官方失败判定。
fn get_json(url: &str, bearer: Option<&str>) -> Result<String, String> {
    let client = http_client()?;
    let mut req = client.get(url);
    if let Some(t) = bearer {
        req = req.header("Authorization", format!("Bearer {t}"));
    }
    let resp = req.send().map_err(|e| format!("transport: {e}"))?;
    let status = resp.status();
    if !status.is_success() {
        return Err(format!("status: {}", status.as_u16()));
    }
    resp.text().map_err(|e| format!("transport: {e}"))
}

// ── API JSON 解析（官方与镜像同构） ───────────────────────────────────────────

#[derive(Deserialize, Default)]
#[serde(default)]
struct ApiNamed {
    name: String,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct ApiSet {
    id: i64,
    favourite_count: Option<u64>,
    play_count: Option<u64>,
    rating: Option<f64>,
    genre: Option<ApiNamed>,
    language: Option<ApiNamed>,
}

/// 解析 osu API v2 beatmapset JSON（官方或镜像）→ OnlineSetMeta。
fn parse_set_json(text: &str, requested_id: i64) -> Result<OnlineSetMeta, String> {
    let raw: ApiSet =
        serde_json::from_str(text).map_err(|e| format!("beatmapset JSON 解析失败: {e}"))?;
    Ok(OnlineSetMeta {
        beatmapset_id: if raw.id != 0 { raw.id } else { requested_id },
        favourite_count: raw.favourite_count.unwrap_or(0),
        play_count: raw.play_count.unwrap_or(0),
        rating: raw.rating.filter(|r| r.is_finite()),
        genre: raw.genre.map(|g| g.name).filter(|s| !s.is_empty()),
        language: raw.language.map(|l| l.name).filter(|s| !s.is_empty()),
        fetched_at: now_secs(),
    })
}

// ── 缓存 ──────────────────────────────────────────────────────────────────────

type Cache = BTreeMap<i64, OnlineSetMeta>;

fn load_cache(dir: &Path) -> Cache {
    fs::read_to_string(cache_file(dir))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

/// M8 导出用：以 map 形式读取在线缓存（不触发任何网络）。
pub(crate) fn online_cache_map(
    dir: &Path,
) -> std::collections::HashMap<i64, crate::model::OnlineSetMeta> {
    load_cache(dir).into_iter().collect()
}

fn apply_cap(cache: &mut Cache, cap: usize) {
    if cache.len() <= cap {
        return;
    }
    let mut by_new: Vec<(i64, i64)> = cache.iter().map(|(k, v)| (*k, v.fetched_at)).collect();
    // 升序 fetched_at → 淘汰最旧
    by_new.sort_by_key(|(_, t)| *t);
    let drop_n = cache.len() - cap;
    for (k, _) in by_new.into_iter().take(drop_n) {
        cache.remove(&k);
    }
}

fn save_cache(dir: &Path, cache: &Cache) -> Result<(), String> {
    let mut c = cache.clone();
    apply_cap(&mut c, MAX_CACHE_ENTRIES);
    write_atomic(
        &cache_file(dir),
        &serde_json::to_string(&c).map_err(|e| e.to_string())?,
    )
}

fn is_fresh(meta: &OnlineSetMeta, now: i64) -> bool {
    now.saturating_sub(meta.fetched_at) < REFRESH_TTL_SECS
}

// ── 抓取编排 ──────────────────────────────────────────────────────────────────

const MAX_PER_CALL: usize = 200;
const MIRROR_RETRIES: usize = 3; // 首次 + 2 次重试

/// 抓取一批集（阻塞函数：在 spawn_blocking 线程 / 登录线程中调用，绝不在 async 上下文直接跑）。
/// 已缓存且新鲜的跳过；失败的单个集跳过（前端按缺省显示）。
fn fetch_sets_blocking(dir: &Path, set_ids: &[i64]) -> Vec<OnlineSetMeta> {
    let now = now_secs();
    let mut cache = load_cache(dir);
    // 去重 + 限量（输出按 id 升序，非请求顺序）
    let mut ids: Vec<i64> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for id in set_ids.iter().copied().filter(|id| *id > 0) {
        if seen.insert(id) {
            ids.push(id);
        }
    }
    ids.sort_unstable();
    ids.truncate(MAX_PER_CALL);

    let mut result: BTreeMap<i64, OnlineSetMeta> = BTreeMap::new();
    // 缓存命中（新鲜或过期都返回给 fetch，过期者重新拉）
    let mut need: Vec<i64> = Vec::new();
    for id in &ids {
        match cache.get(id) {
            Some(m) if is_fresh(m, now) => {
                result.insert(*id, m.clone());
            }
            _ => need.push(*id),
        }
    }

    let mut official_failed = valid_access(dir).is_none();
    for id in need {
        let mut meta: Option<OnlineSetMeta> = None;
        // 官方：仅登录且此前未判定失败时尝试；transport/401/403 视为环境不可达 → 后续全部走镜像
        if !official_failed {
            if let Some(tok) = valid_access(dir) {
                let url = format!("{OFFICIAL_SET_URL}/{id}");
                match get_json(&url, Some(&tok)) {
                    Ok(text) => match parse_set_json(&text, id) {
                        Ok(m) => meta = Some(m),
                        Err(e) => eprintln!("official parse set {id}: {e}"),
                    },
                    Err(e) => {
                        if e.starts_with("transport") || e.contains("401") || e.contains("403") {
                            eprintln!("官方接口不可用（{e}），后续切换镜像");
                            official_failed = true;
                        }
                        // 404 等状态错误：该集官方无数据，静默走镜像
                    }
                }
                std::thread::sleep(Duration::from_millis(1200)); // ~1.2 req/s
            }
        }
        // 镜像：无认证，重试 MIRROR_RETRIES-1 次
        if meta.is_none() {
            let url = format!("{MIRROR_SET_URL}/{id}");
            for attempt in 0..MIRROR_RETRIES {
                match get_json(&url, None) {
                    Ok(text) => {
                        match parse_set_json(&text, id) {
                            Ok(m) => {
                                meta = Some(m);
                                break;
                            }
                            Err(e) => {
                                eprintln!("mirror parse set {id}: {e}");
                                break; // JSON 坏了不必重试
                            }
                        }
                    }
                    Err(e) => {
                        // 404 是「镜像没有该集」，重试无意义
                        if e.contains("404") {
                            break;
                        }
                        if attempt + 1 == MIRROR_RETRIES {
                            eprintln!("mirror set {id} 最终失败: {e}");
                        }
                        std::thread::sleep(Duration::from_millis(120));
                    }
                }
            }
        }
        if let Some(m) = meta {
            cache.insert(id, m.clone());
            result.insert(id, m);
        }
        std::thread::sleep(Duration::from_millis(120)); // 镜像基础间隔
    }

    if let Err(e) = save_cache(dir, &cache) {
        eprintln!("online_cache 保存失败: {e}");
    }
    result.into_values().collect()
}

// ── 登录状态机 ────────────────────────────────────────────────────────────────

#[derive(Debug)]
struct LoginState {
    /// "idle" | "waiting" | "success" | "failed:<msg>"
    status: String,
    expected_state: Option<String>,
}

impl LoginState {
    const IDLE: LoginState = LoginState {
        status: String::new(),
        expected_state: None,
    };
}

static LOGIN: Mutex<LoginState> = Mutex::new(LoginState::IDLE);

fn set_login_status(s: &str) {
    if let Ok(mut g) = LOGIN.lock() {
        g.status = s.to_string();
    }
}

fn random_state() -> String {
    // 熵：时钟纳秒 ^ 进程 id ^ 全局计数器，经 splitmix64 混合（回环 OAuth CSRF state 足够）
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let t = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    let mut x = t
        ^ ((std::process::id() as u64) << 32)
        ^ COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let mut out = String::with_capacity(24);
    for _ in 0..3 {
        x = splitmix64(x);
        out.push_str(&format!("{:08x}", (x & 0xFFFF_FFFF) as u32));
    }
    out
}

fn splitmix64(x: u64) -> u64 {
    let mut z = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// 解析回环回调的 request target："GET /callback?code=…&state=… HTTP/1.1" 的 path 部分。
fn parse_callback_path(path: &str) -> (Option<String>, Option<String>) {
    let mut code = None;
    let mut state = None;
    let Some(query) = path.split_once('?').map(|(_, q)| q) else {
        return (None, None);
    };
    for pair in query.split('&') {
        let (k, v) = match pair.split_once('=') {
            Some(kv) => kv,
            None => continue,
        };
        match k {
            "code" => code = Some(percent_decode(v)),
            "state" => state = Some(v.to_string()),
            _ => {}
        }
    }
    (code, state)
}

fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 2 < bytes.len() => {
                let hi = (bytes[i + 1] as char).to_digit(16);
                let lo = (bytes[i + 2] as char).to_digit(16);
                match (hi, lo) {
                    (Some(h), Some(l)) => {
                        out.push((h * 16 + l) as u8);
                        i += 3;
                    }
                    _ => {
                        out.push(b'%');
                        i += 1;
                    }
                }
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// 纯函数：state 校验（拒绝缺失/不匹配）。
fn state_ok(expected: &str, got: Option<&str>) -> bool {
    got == Some(expected)
}

fn handle_callback_conn(stream: std::net::TcpStream, dir: &Path, expected: &str) {
    let respond = |status: &str, body: &str, stream: &std::net::TcpStream| {
        let html = format!(
            "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\n\
             Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        let _ = stream
            .try_clone()
            .and_then(|mut s| s.write_all(html.as_bytes()));
    };
    let Ok(reader) = stream.try_clone() else {
        return;
    };
    let mut first_line = String::new();
    if BufReader::new(reader).read_line(&mut first_line).is_err() || first_line.is_empty() {
        return;
    }
    // "GET /callback?code=...&state=... HTTP/1.1"
    let mut parts = first_line.split_whitespace();
    if parts.next() != Some("GET") {
        return;
    }
    let target = parts.next().unwrap_or("");
    let (code, state) = parse_callback_path(target);
    if !state_ok(expected, state.as_deref()) {
        set_login_status(&format!("failed:{}", crate::errcode::STATE_MISMATCH));
        respond("400 Bad Request", "<p>state mismatch</p>", &stream);
        return;
    }
    let Some(code) = code.filter(|c| !c.is_empty()) else {
        set_login_status(&format!("failed:{}", crate::errcode::CALLBACK_NO_CODE));
        respond("400 Bad Request", "<p>missing code</p>", &stream);
        return;
    };
    respond(
        "200 OK",
        "<html><body style='font-family:sans-serif;text-align:center;padding-top:20vh'>\
         <h2>登录完成，可关闭此页面</h2></body></html>",
        &stream,
    );
    match exchange_code(dir, &code) {
        Ok(()) => set_login_status("success"),
        Err(e) => set_login_status(&format!("failed:{e}")),
    }
}

// ── Tauri commands（冻结契约，AppHandle 由 Tauri 注入，前端只传契约参数）─────

#[tauri::command]
pub async fn online_fetch(
    app: tauri::AppHandle,
    set_ids: Vec<i64>,
) -> Result<Vec<OnlineSetMeta>, String> {
    let dir = app_dir(&app);
    let metas = tauri::async_runtime::spawn_blocking(move || fetch_sets_blocking(&dir, &set_ids))
        .await
        .map_err(|e| crate::errcode::ec1(crate::errcode::ONLINE_INTERRUPTED, e))?;
    Ok(metas)
}

#[tauri::command]
pub fn online_cache(app: tauri::AppHandle, set_ids: Vec<i64>) -> Vec<OnlineSetMeta> {
    let cache = load_cache(&app_dir(&app));
    set_ids
        .iter()
        .filter_map(|id| cache.get(id).cloned())
        .collect()
}

#[tauri::command]
pub fn online_status(app: tauri::AppHandle) -> OnlineStatus {
    let dir = app_dir(&app);
    let logged_in = load_tokens(&dir).is_some();
    OnlineStatus {
        logged_in,
        // 官方仅在登录时可能使用；未登录常态走镜像
        source: if logged_in { "official" } else { "mirror" }.to_string(),
        cached_count: load_cache(&dir).len() as u64,
    }
}

#[tauri::command]
pub fn set_oauth_credentials(
    app: tauri::AppHandle,
    client_id: String,
    client_secret: String,
) -> Result<(), String> {
    let creds = OauthCreds {
        client_id: client_id.trim().to_string(),
        client_secret: client_secret.trim().to_string(),
    };
    if creds.client_id.is_empty() {
        return Err(crate::errcode::ec(crate::errcode::CLIENT_ID_EMPTY));
    }
    if creds.client_secret.is_empty() {
        return Err(crate::errcode::ec(crate::errcode::CLIENT_SECRET_EMPTY));
    }
    let dir = app_dir(&app);
    write_atomic(
        &oauth_file(&dir),
        &serde_json::to_string_pretty(&creds).map_err(|e| e.to_string())?,
    )
}

#[tauri::command]
pub async fn osu_login_begin(app: tauri::AppHandle) -> Result<String, String> {
    let dir = app_dir(&app);
    let creds = load_creds(&dir);
    if creds.client_id.is_empty() || creds.client_secret.is_empty() {
        return Err(crate::errcode::ec(crate::errcode::OAUTH_CREDS_MISSING));
    }
    let state = random_state();
    let listener = TcpListener::bind("127.0.0.1:47821")
        .map_err(|e| crate::errcode::ec1(crate::errcode::PORT_BUSY, e))?;
    listener.set_nonblocking(true).map_err(|e| e.to_string())?;
    {
        let mut g = LOGIN
            .lock()
            .map_err(|_| crate::errcode::ec(crate::errcode::STATE_LOCK_UNAVAILABLE))?;
        g.status = "waiting".to_string();
        g.expected_state = Some(state.clone());
    }
    let url = format!(
        "{AUTHORIZE_URL}?response_type=code&client_id={}&redirect_uri={}&scope=public&state={}",
        urlencoding(&creds.client_id),
        urlencoding(REDIRECT_URI),
        urlencoding(&state),
    );
    // 后台监听一次回调，5 分钟超时
    std::thread::spawn(move || {
        let deadline = std::time::Instant::now() + Duration::from_secs(5 * 60);
        loop {
            match listener.accept() {
                Ok((stream, _)) => {
                    // listener 是非阻塞的；Unix 上 accept 出的连接继承非阻塞导致
                    // read_line 立即 WouldBlock，恢复阻塞模式（Windows 默认即阻塞）
                    let _ = stream.set_nonblocking(false);
                    handle_callback_conn(stream, &dir, &state);
                    return;
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    if std::time::Instant::now() >= deadline {
                        set_login_status("failed:timeout");
                        return;
                    }
                    std::thread::sleep(Duration::from_millis(100));
                }
                Err(e) => {
                    set_login_status(&format!("failed:{e}"));
                    return;
                }
            }
        }
    });
    Ok(url)
}

#[tauri::command]
pub fn osu_login_status() -> String {
    LOGIN
        .lock()
        .map(|g| {
            if g.status.is_empty() {
                "idle".to_string()
            } else {
                g.status.clone()
            }
        })
        .unwrap_or_else(|_| "idle".to_string())
}

#[tauri::command]
pub async fn osu_login_manual(app: tauri::AppHandle, code: String) -> Result<(), String> {
    let dir = app_dir(&app);
    let code = code.trim().to_string();
    if code.is_empty() {
        return Err(crate::errcode::ec(crate::errcode::CODE_EMPTY));
    }
    let result = tauri::async_runtime::spawn_blocking(move || exchange_code(&dir, &code))
        .await
        .map_err(|e| crate::errcode::ec1(crate::errcode::LOGIN_INTERRUPTED, e))?;
    match &result {
        Ok(()) => set_login_status("success"),
        Err(e) => set_login_status(&format!("failed:{e}")),
    }
    result
}

#[tauri::command]
pub fn osu_logout(app: tauri::AppHandle) -> Result<(), String> {
    let dir = app_dir(&app);
    let _ = fs::remove_file(tokens_file(&dir));
    set_login_status("idle");
    if let Ok(mut g) = LOGIN.lock() {
        g.expected_state = None;
    }
    Ok(())
}

#[tauri::command]
pub fn get_oauth_config(app: tauri::AppHandle) -> crate::model::OauthConfigView {
    let creds = load_creds(&app_dir(&app));
    crate::model::OauthConfigView {
        client_id: creds.client_id,
        has_secret: !creds.client_secret.is_empty(),
    }
}

fn urlencoding(s: &str) -> String {
    let mut out = String::new();
    for b in s.as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(*b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

// ── 测试 ──────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    struct TempDir(PathBuf);
    impl TempDir {
        fn new(tag: &str) -> Self {
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let root = std::env::temp_dir()
                .join(format!("osm_online_{tag}_{}_{nanos}", std::process::id()));
            fs::create_dir_all(&root).unwrap();
            Self(root)
        }
    }
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn meta(id: i64, fetched_at: i64) -> OnlineSetMeta {
        OnlineSetMeta {
            beatmapset_id: id,
            favourite_count: 10,
            play_count: 20,
            rating: Some(99.5),
            genre: Some("Animation".into()),
            language: Some("Japanese".into()),
            fetched_at,
        }
    }

    #[test]
    fn cache_roundtrip_and_ttl() {
        let tmp = TempDir::new("cache");
        let now = now_secs();
        let mut cache: Cache = BTreeMap::new();
        cache.insert(1, meta(1, now - 3600)); // 新鲜
        cache.insert(2, meta(2, now - REFRESH_TTL_SECS - 60)); // 过期
        save_cache(&tmp.0, &cache).unwrap();
        let loaded = load_cache(&tmp.0);
        assert_eq!(loaded.len(), 2);
        assert!(is_fresh(loaded.get(&1).unwrap(), now));
        assert!(!is_fresh(loaded.get(&2).unwrap(), now));
    }

    #[test]
    fn cache_cap_evicts_oldest() {
        let mut cache: Cache = BTreeMap::new();
        for i in 0..5 {
            cache.insert(i, meta(i, 1000 + i));
        }
        apply_cap(&mut cache, 3);
        assert_eq!(cache.len(), 3);
        assert!(!cache.contains_key(&0) && !cache.contains_key(&1));
        assert!(cache.contains_key(&4));
    }

    /// 镜像 v3 捕获样本（真实 37104 响应的代表子集；含未知字段验证忽略能力）
    #[test]
    fn parse_mirror_json_sample() {
        let sample = r#"{
            "id": 37104, "artist": "Inoue Marina , Kanae Itou", "title": "Egao no dessan (TV size)",
            "creator": "AkumaX", "favourite_count": 1, "nsfw": false, "offset": 2238,
            "play_count": 0, "rating": 0, "source": "", "status": "ranked", "tags": "osstama",
            "genre": {"id": 1, "name": "Unspecified"},
            "language": {"id": 1, "name": "Unspecified"},
            "video": false, "beatmaps": [{"id": 112807, "version": "Easy"}]
        }"#;
        let m = parse_set_json(sample, 37104).unwrap();
        assert_eq!(m.beatmapset_id, 37104);
        assert_eq!(m.favourite_count, 1);
        assert_eq!(m.play_count, 0);
        assert_eq!(m.rating, Some(0.0));
        assert_eq!(m.genre.as_deref(), Some("Unspecified"));
        assert_eq!(m.language.as_deref(), Some("Unspecified"));
        assert!(m.fetched_at > 0);
    }

    /// 官方空值形态：rating 可为 null、genre/language 可缺失
    #[test]
    fn parse_official_json_nulls() {
        let sample =
            r#"{"id": 999, "favourite_count": null, "play_count": 123456, "rating": null}"#;
        let m = parse_set_json(sample, 1).unwrap();
        assert_eq!(m.beatmapset_id, 999); // 响应 id 优先
        assert_eq!(m.favourite_count, 0);
        assert_eq!(m.play_count, 123456);
        assert_eq!(m.rating, None);
        assert_eq!(m.genre, None);
        assert_eq!(m.language, None);
    }

    #[test]
    fn callback_state_validation() {
        // 生产路径先按空白切出 target，这里模拟切好的结果
        let (code, state) = parse_callback_path("/callback?code=abc%2Bdef&state=xY123");
        assert_eq!(code.as_deref(), Some("abc+def"));
        assert_eq!(state.as_deref(), Some("xY123"));
        assert!(state_ok("xY123", state.as_deref()));
        assert!(!state_ok("other", state.as_deref()));
        assert!(!state_ok("xY123", None));
        let (c2, s2) = parse_callback_path("/callback");
        assert!(c2.is_none() && s2.is_none());
    }

    /// 真实镜像网络冒烟（文档化的 live check；网络不可用时优雅跳过，不失败）。
    /// 运行：cargo test -- --ignored --nocapture
    #[test]
    #[ignore = "requires network access to mirror.hinamizawa.ai"]
    fn live_mirror_check() {
        let tmp = TempDir::new("live");
        // 1) 单集直连：37104 应在；172800 视镜像覆盖而定
        for id in [37104i64, 172800] {
            let url = format!("{MIRROR_SET_URL}/{id}");
            match get_json(&url, None) {
                Ok(text) => {
                    let m = parse_set_json(&text, id).expect("mirror json parses");
                    println!(
                        "mirror {id}: fav={} play={} rating={:?} genre={:?} lang={:?}",
                        m.favourite_count, m.play_count, m.rating, m.genre, m.language
                    );
                }
                Err(e) if e.contains("404") => {
                    println!("mirror {id}: 404（镜像暂无该集，视为可用信号）");
                }
                Err(e) => {
                    println!("[skip] 网络不可用: {e}");
                    return;
                }
            }
        }
        // 2) 无 token → online_fetch 内部管线走镜像 + 生成缓存文件
        let metas = fetch_sets_blocking(&tmp.0, &[37104, 172800]);
        assert!(!metas.is_empty(), "expected ≥1 fetched meta");
        assert!(cache_file(&tmp.0).is_file(), "cache file must be created");
        let cache = load_cache(&tmp.0);
        for m in &metas {
            assert_eq!(
                cache.get(&m.beatmapset_id).map(|c| c.favourite_count),
                Some(m.favourite_count)
            );
        }
        println!(
            "live_mirror_check OK: fetched={} cached={}",
            metas.len(),
            cache.len()
        );
    }
}
