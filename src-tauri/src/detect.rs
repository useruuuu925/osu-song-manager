use crate::model::{LibraryCandidate, SourceKind};
use std::fs;
use std::path::{Path, PathBuf};

/// 自动检测本机可能的 osu! 曲库位置。
/// lazer：包含 client.realm 的目录；stable：包含 Songs 的安装目录。
pub fn detect_libraries() -> Vec<LibraryCandidate> {
    let mut out = Vec::new();

    // 1. lazer / stable 共用的默认数据目录 %LOCALAPPDATA%\osu!
    //    detail 为稳定 key（dt.*），由前端按当前语言翻译；未命中词表的串原样透出。
    if let Some(local) = std::env::var_os("LOCALAPPDATA") {
        let base = PathBuf::from(&local).join("osu!");
        if base.join("client.realm").is_file() {
            out.push(LibraryCandidate {
                kind: SourceKind::Lazer,
                path: base.to_string_lossy().into_owned(),
                detail: "dt.lazerDefault".into(),
            });
        }
        // stable 默认也装在这里：存在 Songs 子目录但没有 client.realm
        let songs = base.join("Songs");
        if songs.is_dir() && !base.join("client.realm").is_file() {
            out.push(LibraryCandidate {
                kind: SourceKind::Stable,
                path: songs.to_string_lossy().into_owned(),
                detail: "dt.stableDefault".into(),
            });
        }
    }

    // 1b. 真实 Windows 安装中 lazer 的数据目录常位于 %APPDATA%\osu!（Roaming，
    //     新版部分安装为 %APPDATA%\osu），
    //     内含 client.realm / client.realm.lock / files\ / online.db 等。
    if let Some(roaming) = std::env::var_os("APPDATA") {
        for name in ["osu!", "osu"] {
            let base = PathBuf::from(&roaming).join(name);
            if looks_like_lazer_data(&base) && !already(&out, SourceKind::Lazer, &base) {
                out.push(LibraryCandidate {
                    kind: SourceKind::Lazer,
                    path: base.to_string_lossy().into_owned(),
                    detail: "dt.lazerRoaming".into(),
                });
            }
        }
    }

    // 2. 常见自定义盘符位置
    for drive in ["C", "D", "E", "F"] {
        for name in ["osu!", "osu"] {
            let base = PathBuf::from(format!("{drive}:\\{name}"));
            if base.join("client.realm").is_file() && !already(&out, SourceKind::Lazer, &base) {
                out.push(LibraryCandidate {
                    kind: SourceKind::Lazer,
                    path: base.to_string_lossy().into_owned(),
                    detail: "dt.lazerDrive".into(),
                });
            }
            let songs = base.join("Songs");
            if songs.is_dir() && !already(&out, SourceKind::Stable, &songs) {
                out.push(LibraryCandidate {
                    kind: SourceKind::Stable,
                    path: songs.to_string_lossy().into_owned(),
                    detail: "dt.stableDrive".into(),
                });
            }
        }
    }

    out
}

fn already(list: &[LibraryCandidate], kind: SourceKind, path: &Path) -> bool {
    list.iter()
        .any(|c| c.kind == kind && Path::new(&c.path) == path)
}

/// 目录是否看起来是 lazer 数据目录（含 client.realm 文件或 files\ 子目录）
fn looks_like_lazer_data(p: &Path) -> bool {
    p.join("client.realm").is_file() || p.join("files").is_dir()
}

/// 是否看起来是 Velopack 安装目录（含 Update.exe 或 current\ 子目录，
/// 大小写不敏感；或目录名本身就叫 osulazer），此类目录不含谱面数据。
fn is_lazer_install_dir(p: &Path) -> bool {
    if p.file_name()
        .map(|n| n.to_string_lossy().eq_ignore_ascii_case("osulazer"))
        .unwrap_or(false)
    {
        return true;
    }
    let Ok(rd) = fs::read_dir(p) else {
        return false;
    };
    rd.flatten().any(|e| {
        let name = e.file_name();
        let name = name.to_string_lossy();
        name.eq_ignore_ascii_case("Update.exe")
            || (name.eq_ignore_ascii_case("current") && e.path().is_dir())
    })
}

/// 把用户输入的路径透明解析为真正的 lazer 数据目录：
/// - 已是数据目录（含 client.realm 或 files\）→ 原样返回；
/// - 像安装目录 → 尝试 Roaming 数据目录 %APPDATA%\osu!（确认其确实是数据目录才重定向）；
/// - 其他情况 → 原样返回，让后续校验报出原始错误。
pub fn resolve_lazer_data_dir(p: &Path) -> PathBuf {
    resolve_lazer_data_dir_with(p, std::env::var_os("APPDATA").map(PathBuf::from).as_deref())
}

/// APPDATA 注入版本，便于测试（与 detect_libraries 使用同一环境变量来源）。
fn resolve_lazer_data_dir_with(p: &Path, appdata: Option<&Path>) -> PathBuf {
    if looks_like_lazer_data(p) {
        return p.to_path_buf();
    }
    if is_lazer_install_dir(p) {
        if let Some(appdata) = appdata {
            for name in ["osu!", "osu"] {
                let candidate = appdata.join(name);
                if looks_like_lazer_data(&candidate) {
                    return candidate;
                }
            }
        }
    }
    p.to_path_buf()
}

/// 校验用户手动选择的目录是否为合法曲库路径
pub fn validate_library(kind: SourceKind, path: &str) -> Result<(), String> {
    use crate::errcode;
    let p = Path::new(path);
    if !p.is_dir() {
        return Err(errcode::ec1(errcode::DIR_NOT_FOUND, path));
    }
    match kind {
        SourceKind::Lazer => {
            // 先尝试把安装目录（Velopack 布局）解析为 Roaming 数据目录，再校验
            let resolved = resolve_lazer_data_dir(p);
            if looks_like_lazer_data(&resolved) {
                Ok(())
            } else {
                Err(errcode::ec(errcode::LAZER_NO_REALM))
            }
        }
        SourceKind::Stable => {
            // 允许直接选 Songs，也允许选包含 Songs 的安装根目录
            if p.join("client.realm").is_file() {
                Err(errcode::ec(errcode::STABLE_IS_LAZER))
            } else {
                Ok(())
            }
        }
        SourceKind::OszFolder => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    /// 临时夹具目录，Drop 时尽力删除（容忍失败），不触碰用户 AppData 下真实数据
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(tag: &str) -> Self {
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let root = std::env::temp_dir().join(format!(
                "osu_sm_detect_{tag}_{}_{nanos}",
                std::process::id()
            ));
            fs::create_dir_all(&root).expect("create temp fixture dir");
            Self(root)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn resolves_velopack_install_dir_to_roaming_data_dir() {
        // 安装目录形状：含 Update.exe + current\，无任何谱面数据
        let install = TempDir::new("install");
        fs::write(install.0.join("Update.exe"), b"").expect("write Update.exe marker");
        fs::create_dir_all(install.0.join("current")).expect("create current dir");
        // 假 Roaming：%APPDATA% 指向临时目录，其 osu! 子目录含 client.realm
        let fake_appdata = TempDir::new("appdata");
        let data = fake_appdata.0.join("osu!");
        fs::create_dir_all(&data).expect("create fake data dir");
        fs::write(data.join("client.realm"), b"").expect("write client.realm");

        let resolved = resolve_lazer_data_dir_with(&install.0, Some(&fake_appdata.0));
        assert_eq!(resolved, data);
    }

    #[test]
    fn data_dir_is_returned_unchanged() {
        let dir = TempDir::new("data");
        fs::write(dir.0.join("client.realm"), b"").expect("write client.realm");
        // 即使注入了指向别处的 APPDATA，也应原样返回
        let other = TempDir::new("other");
        let resolved = resolve_lazer_data_dir_with(&dir.0, Some(&other.0));
        assert_eq!(resolved, dir.0);
    }
}
