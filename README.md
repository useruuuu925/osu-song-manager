# osu 曲库管理（osu-song-manager / osu! Song Manager）

> [中文](#软件简介) · [English](#english)

---

## 软件简介

面向 osu! 玩家的桌面曲库管理工具，支持三种曲库来源：**osu!lazer 数据库**、**osu!stable Songs 目录**、**.osz 文件夹**。

主要功能：

- 曲目/难度两级浏览，标题/艺术家/谱师/BPM/星级等列排序与关键词搜索
- 收藏数排序：联网同步各谱面集收藏数、播放数、评分、流派、语言后按收藏数排序
- 背景画廊：以缩略图网格浏览曲库背景图，支持批量导出原始背景图（自动重命名、防覆盖）
- 清理：按 OnlineID / MD5 / 指纹查重，回收站删除、移动到指定目录、打包为 zip、扫描空文件夹
- 下载：通过镜像链批量下载 .osz 文件（免登录可用）
- 转换：把 lazer 曲库的谱面集转换为 stable Songs 文件夹或 .osz 文件（lazer 侧只读）
- 导出：生成兼容社区工具的曲库 CSV（UTF-8 带 BOM）
- 多语言：中文 / English 界面切换（设置中切换，自动检测系统语言，无刷新生效）

## 下载与运行

- 从 Releases 下载便携版 exe，双击即可运行，无需安装；支持 Windows 10 / 11（64 位）。
- Windows 11 自带 WebView2 运行时，开箱即用。
- Windows 10 若提示缺少 WebView2，请安装微软官方运行时：
  https://developer.microsoft.com/microsoft-edge/webview2/

## 使用指南

### 曲目

首次使用请在顶栏选择曲库模式并指定目录（lazer 可留空自动检测）。列表每行一个谱面集，
点击行展开难度。表头点击可排序；搜索框跨标题/艺术家/谱师匹配。
带在线数据的列（收藏、播放、评分、流派、语言）在未同步时显示「—」。

### 详情

选中谱面集后右侧显示详情面板：难度参数（AR/CS/HP/OD、BPM、时长、星数、对象数）、
标签、来源与在线信息。面板宽度可拖拽分隔条调整，字号在设置中可调。

### 画廊

以 16:9 背景缩略图网格浏览曲库（缩略图按 256×144 生成，带歌曲信息：标题 / 艺术家 /
模式 / 状态 / 难度数）。首次进入会按需生成本地缩略图缓存（app 数据目录 thumbs\），
进度以事件推送。本地提取失败的背景可自动从 osu! 官方素材库
（assets.ppy.sh/beatmaps/{曲号}/covers/raw.jpg）拉取补齐，可在设置中关闭。
勾选后可「导出背景图」：选择文件夹后批量导出原图，
文件名为「艺术家 - 标题 [谱师]」，重名自动追加 (2)(3)。

### 清理

- 查重：按相同 OnlineID、相同难度 MD5（跨集）、相同难度指纹（本地导入集）三种依据分组；
  勾选后可移入回收站或移动到其他目录。
- 空文件夹：扫描 Songs 下不含任何 .osu 的子目录并列出。
- 打包：将选中的集目录 / .osz 压缩为一个 zip（源文件只读，不改动）。

### 下载

输入 beatmapset ID 列表，沿 hinai → osu.direct → nerinyan → sayobot
镜像链下载，每集自动校验 zip 内含 .osu。最多 3 路并发礼貌限速。

### 转换（lazer → stable）

把 lazer 曲库中的谱面集转换输出到用户指定目录，两种产物形态：

- **Songs 文件夹**：按 stable 惯例命名（`<ID> <艺术家> - <标题>`）的集文件夹，
  把目标目录指向 stable 的 Songs 即可直接入库；
- **.osz 文件**：可直接双击导入 osu! 的压缩包。

转换从 lazer 内容寻址存储（`files\`）**只读**复制全部文件（含音频、背景、故事板、样本包），
并对 .osu 做 stable 兼容降级（stable 格式上限为 v14，lazer 编辑器改写过的图为 v128）：

- 头行 `osu file format v128` → `v14`；
- `[Events]` 浮点 break 行（如 `2,48577.86,…`）截断为整数——stable 按整数解析，
  浮点直接报「无法解析谱面头部」（ppy/osu#28609）；
- `[TimingPoints]` 时间 floor + 同区间物件整体平移对齐（ppy/osu#30607，防 snap/SV
  错位）；负 beatLength 的 uninherited 点按语义改回 inherited（#37583）；
- `[HitObjects]` 坐标 round、时间截断（v128 允许全精度，v14 侧按整数解析，#31305）；
- **slider 多段混合曲线 → 单 `B|` bezier 锚点**（stable 只认最后出现的曲线类型，
  #31713）：LINEAR/CATMULL/PERFECT 圆弧（三点定圆 + 单位圆弧模板 + de Casteljau
  子弧收敛）全部转锚点，段边界 double-up，末控制点裸 type 清除（#24570）；
- **stable 不支持的媒体格式逐集警告**：音频 flac/opus/m4a/aac/wma（导入后可能无声）、
  视频 webm/mov/mkv（可能不显示），转换报告中标 ⚠。

未被 lazer 编辑器改写过的集（绝大多数）本来就是 v14 原始字节，降级路径不触发、逐字节直通。
输出文件夹名清洗非法字符并截断超长名（路径长度防御）。
Files 列表数据错位混入的他集 .osu 会按内嵌 BeatmapSetID 过滤，缺失的本集背景按
`Metadata.BackgroundFile` 全局回退补齐。本地导入集（无在线 ID）暂不支持按 ID 转换，
会被明确跳过并提示。
支持批量、进度事件、随时取消，单集失败不影响其余集。

### 多语言

设置 → 语言：中文 / English。首次启动按系统语言自动选择，切换后立即生效（含窗口标题），
记忆在本地 `osu-mgr.lang`。英文文案与中文同标准：直白朴素，无营销腔。

## 数据与隐私

- **lazer 曲库绝对只读**：删除/移动/打包/转换等一切写操作仅作用于用户选择的目标目录与
  stable Songs / .osz；lazer 数据库目录内的任何写入路径一律拒绝，只读取不修改。
- 在线元数据来自 hinai 镜像（免登录）或 osu! 官方 API（需登录）。
- osu! 登录是可选的：仅在设置中填入你自己申请的 OAuth client_id/secret 后才需要；
  token 仅保存在本机。
- 本地缓存文件（位于应用数据目录，与 config.json 同级）：
  online_cache.json（在线元数据缓存）、oauth.json、tokens.json（登录凭据）、
  thumbs\（缩略图缓存）。删除这些文件或退出登录即可清理。
- 删除文件走系统回收站，可随时恢复。

## 常见问题

- **扫描结果为空**：确认选择的目录正确——stable 应选 Songs 目录本身，
  lazer 可填安装目录（如 C:\Users\<你>\AppData\Local\osulazer）会自动解析到数据目录。
- **收藏列全是「—」**：点击同步等待缓存写入；若长时间无结果可尝试登录 osu! 走官方通道。
- **下载 404**：该谱面集已被 osu! 删除，换下一个源也不会有，属正常现象。
- **下载偏慢**：并发固定 3 路并带节流与冷却（429/503 后该镜像暂停 ≥60 秒），
  对镜像站礼貌限速，属设计行为。
- **转换提示「本地导入集不支持转换」**：无在线 ID 的集无法按 ID 寻址，属预期行为。
- **Win10 打不开**：安装 WebView2 Runtime（见上）。

## 开发简述

技术栈：Tauri 2 + Rust（后端含 Realm v24 只读解析、reqwest/trash/image/csv 等）+ Vue 3 前端。
前端零运行时依赖（自研 i18n）；数据链路 serde camelCase，类型以 `src-tauri/src/model.rs` 为准。

```bash
# 后端测试（含真库/网络探针：cargo test -- --ignored）
cd src-tauri && cargo test
# 开发运行
npm install && npm run tauri dev
# 打包发布（npm run build 会先校验 zh/en 文案 key 集合全等）
npm run tauri build
```

---

## English

A desktop library manager for osu! players with three library sources: the **osu!lazer
database**, the **osu!stable Songs directory**, and **.osz folders**.

### Features

- Two-level browsing (sets / difficulties) with sortable columns and keyword search
- Online metadata (favourites, play count, rating, genre, language) synced per page from a
  login-free mirror, or the official API after optional osu! login
- Background gallery: 16:9 thumbnails (256×144) with song info, local cache, bulk export
  (collision-safe naming), and an optional online fallback from the osu! asset CDN for
  backgrounds that fail local extraction (toggle in Settings)
- Clean-up: duplicate detection by OnlineID / difficulty MD5 / set fingerprint; recycle-bin
  delete, move, zip packing, empty-folder scan
- Downloads: batch .osz downloads across a mirror chain (hinai → osu.direct → nerinyan → sayobot)
- **Convert**: turn lazer library sets into stable Songs folders or .osz files (read-only on lazer)
- CSV export compatible with community tools (UTF-8 with BOM)
- **i18n**: Chinese / English switching in Settings (auto-detected on first launch, instant,
  remembered locally)

### Convert (lazer → stable)

Copies every file of each set read-only from the lazer content-addressed store (`files\`)
into your chosen target — either a stable-convention folder (`<ID> <Artist> - <Title>`) or a
double-click-installable .osz. `.osu` files get a stable-compatibility downgrade (stable caps
at format v14; maps re-saved by lazer's editor are v128):

- the `osu file format v128` header line becomes `v14`;
- float-precision break lines in `[Events]` (e.g. `2,48577.86,…`) are truncated to integers —
  stable parses them as ints and otherwise fails with "failed to parse beatmap header" (ppy/osu#28609);
- timing-point times are floored with same-interval object shifting (ppy/osu#30607, prevents
  snap/SV drift); uninherited points with negative beat length fall back to inherited (#37583);
- hit-object coordinates are rounded and times truncated (v128 allows full precision, v14
  does not, #31305);
- **multi-segment slider curves are rewritten to a single `B|` bezier anchor path** (stable
  only honours the last curve type, #31713): LINEAR/CATMULL/PERFECT arcs (circumcircle +
  unit-arc templates + de Casteljau convergence) all converted, segment boundaries doubled
  up, trailing bare type token dropped (#24570);
- media formats stable cannot play (audio flac/opus/m4a, video webm/mov) are reported per
  set as ⚠ warnings in the conversion report.

Sets never touched by lazer's editor (the vast majority) are original v14 bytes and pass
through byte-for-byte. Foreign .osu files mixed into the set's Files list (a lazer database
quirk) are filtered by their embedded BeatmapSetID; a missing set background is re-added via a
global fallback on `Metadata.BackgroundFile`. Local imports (no online ID) are skipped with a
clear notice. Batch, progress events, cancel anytime; one failing set never blocks the rest.

### Privacy & safety

- **The lazer library is strictly read-only.** Every write (delete/move/pack/convert) targets
  only user-selected destinations and stable Songs / .osz paths; anything inside the lazer data
  directory is refused.
- Deletions go to the system recycle bin. Login is optional; tokens stay on this machine.
- Local caches live next to config.json in the app data directory (online_cache.json,
  oauth.json, tokens.json, thumbs\).

### Development

Stack: Tauri 2 + Rust backend (read-only Realm v24 parser, reqwest/trash/image/csv) + Vue 3
frontend with zero runtime dependencies (hand-rolled i18n). Data contracts are serde camelCase
with `src-tauri/src/model.rs` as the single source of truth.

```bash
cd src-tauri && cargo test   # unit tests; add -- --ignored for real-library/network probes
npm install && npm run tauri dev
npm run tauri build          # npm run build first verifies zh/en locale key parity
```
