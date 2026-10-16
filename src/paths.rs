//! 跨平台路径与应用元信息（对应参考实现 `Services/AppPaths.cs`、`Services/AppInfo.cs`）。
//!
//! 数据目录的解析顺序与环境变量与原实现完全一致，见
//! `docs/reference/backend-data.md` §1.4 / §6 与 `docs/reference/ffmpeg-core.md` §1。

use std::path::{Component, Path, PathBuf};

use uuid::Uuid;

/// 应用显示名（日志、通知、窗口标题、数据目录名共用）
pub const APP_DISPLAY_NAME: &str = "FFmpeg WebUI (Rust)";

/// 应用版本
pub const APP_VERSION: &str = env!("CARGO_PKG_VERSION");

/// 数据目录子目录名。**注意**：只有未设置 `FFMPEGWEBRUST_DATA_DIR` 时才追加这一层。
const DATA_DIR_NAME: &str = "FFmpegWebRust";

/// 数据库文件名。
///
/// 刻意不复用参考实现的 `data.db`（那是 LiteDB 格式），避免把旧库当 SQLite 打开而损坏。
pub const DATABASE_FILE_NAME: &str = "data.sqlite3";

// ────────────────────────────────────────────────────────────────────────────
// 平台判定
// ────────────────────────────────────────────────────────────────────────────

/// 平台标识，用于 API、编码器画像过滤：`"windows"` / `"macos"` / `"linux"`
pub fn platform() -> &'static str {
    if cfg!(target_os = "windows") {
        "windows"
    } else if cfg!(target_os = "macos") {
        "macos"
    } else {
        // 其它 Unix（FreeBSD 等）与原实现一样归入 Linux 分支
        "linux"
    }
}

/// 平台显示名（界面与 `/api/health` 使用）
pub fn platform_display_name() -> &'static str {
    if cfg!(target_os = "windows") {
        "Windows"
    } else if cfg!(target_os = "macos") {
        "macOS"
    } else if cfg!(target_os = "linux") {
        "Linux"
    } else {
        "未知系统"
    }
}

pub fn is_windows() -> bool {
    cfg!(target_os = "windows")
}

pub fn is_macos() -> bool {
    cfg!(target_os = "macos")
}

pub fn is_linux() -> bool {
    cfg!(target_os = "linux")
}

/// 是否运行在容器里（Linux 下探测 `/.dockerenv` 或 `container` 环境变量）
pub fn is_container() -> bool {
    if !is_linux() {
        return false;
    }
    Path::new("/.dockerenv").exists()
        || std::env::var("DOTNET_RUNNING_IN_CONTAINER").map(|v| v == "true").unwrap_or(false)
        || std::env::var("container").is_ok()
}

/// 可执行文件后缀（Windows 为 `.exe`）
pub fn exe_suffix() -> &'static str {
    if is_windows() { ".exe" } else { "" }
}

// ────────────────────────────────────────────────────────────────────────────
// 环境变量
// ────────────────────────────────────────────────────────────────────────────

/// 覆盖数据目录（设置后**不再**追加 `FFmpegWebRust` 子目录）
pub const ENV_DATA_DIR: &str = "FFMPEGWEBRUST_DATA_DIR";
/// 覆盖监听地址；未设置 = 仅回环
pub const ENV_BIND: &str = "FFMPEGWEBRUST_BIND";
/// 覆盖监听端口
pub const ENV_PORT: &str = "FFMPEGWEBRUST_PORT";
/// 设为 `1` 则不自动打开浏览器
pub const ENV_NO_BROWSER: &str = "FFMPEGWEBRUST_NO_BROWSER";
/// 覆盖数据库文件位置
pub const ENV_DATABASE_PATH: &str = "FFMPEGWEBRUST_DATABASE_PATH";

/// 读取环境变量（空白视为未设置）
pub fn env_var(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.trim().is_empty())
}

// ────────────────────────────────────────────────────────────────────────────
// 数据目录
// ────────────────────────────────────────────────────────────────────────────

/// 应用数据目录（数据库、缓存所在）
pub fn data_directory() -> PathBuf {
    if let Some(dir) = env_var(ENV_DATA_DIR) {
        // 直接使用，不再追加应用名子目录（与原实现一致）
        let path = expand_home(dir.trim());
        ensure_directory(&path);
        return path;
    }

    let root = if is_windows() {
        std::env::var("LOCALAPPDATA").ok().map(PathBuf::from)
    } else if is_macos() {
        dirs::home_dir().map(|h| h.join("Library").join("Application Support"))
    } else {
        std::env::var("XDG_DATA_HOME")
            .ok()
            .filter(|v| !v.trim().is_empty())
            .map(PathBuf::from)
            .or_else(|| dirs::home_dir().map(|h| h.join(".local").join("share")))
    };

    let dir = match root {
        Some(root) => root.join(DATA_DIR_NAME),
        // 极少见：拿不到任何数据根目录时退回可执行文件所在目录
        None => executable_directory().join(DATA_DIR_NAME),
    };

    ensure_directory(&dir);
    dir
}

/// 数据库文件路径（`FFMPEGWEBRUST_DATABASE_PATH` > 数据目录/data.sqlite3）
pub fn database_path() -> PathBuf {
    if let Some(custom) = env_var(ENV_DATABASE_PATH) {
        let path = to_absolute(&custom);
        if let Some(parent) = path.parent() {
            ensure_directory(parent);
        }
        return path;
    }

    let path = data_directory().join(DATABASE_FILE_NAME);
    if let Some(parent) = path.parent() {
        ensure_directory(parent);
    }
    path
}

/// 可执行文件所在目录（`AppContext.BaseDirectory` 的等价物）
pub fn executable_directory() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(Path::to_path_buf))
        .unwrap_or_else(|| PathBuf::from("."))
}

/// 默认输出目录：系统「视频」目录 → 主目录 → 临时目录
pub fn default_output_directory() -> PathBuf {
    if let Some(videos) = dirs::video_dir() {
        if videos.is_dir() {
            return videos;
        }
    }
    if let Some(home) = dirs::home_dir() {
        if home.is_dir() {
            return home;
        }
    }
    std::env::temp_dir()
}

// ────────────────────────────────────────────────────────────────────────────
// 路径规整
// ────────────────────────────────────────────────────────────────────────────

/// 展开 `~`、`~/…`、`~\…`；拿不到主目录时原样返回
pub fn expand_home(path: &str) -> PathBuf {
    let trimmed = path.trim();
    if trimmed == "~" {
        return dirs::home_dir().unwrap_or_else(|| PathBuf::from("~"));
    }

    let rest = trimmed
        .strip_prefix("~/")
        .or_else(|| trimmed.strip_prefix("~\\"));
    if let Some(rest) = rest {
        if let Some(home) = dirs::home_dir() {
            return home.join(rest);
        }
    }

    PathBuf::from(trimmed)
}

/// 规整成绝对路径（**纯词法**，不要求目标存在、不解析符号链接）。
///
/// 原实现用 `Path.GetFullPath`：相对路径基于进程 CWD，且对不存在的输出文件也成立。
/// `fs::canonicalize` 语义不同（要求存在且解析软链），会破坏「输出路径尚未创建」的场景。
pub fn to_absolute(path: &str) -> PathBuf {
    let trimmed = path.trim();
    if trimmed.is_empty() {
        return PathBuf::new();
    }

    let unquoted = trimmed
        .strip_prefix('"')
        .and_then(|v| v.strip_suffix('"'))
        .unwrap_or(trimmed)
        .trim();

    let expanded = expand_home(unquoted);
    match std::path::absolute(&expanded) {
        // `std::path::absolute` 是纯词法规整（不访问文件系统），与 GetFullPath 语义最接近
        Ok(abs) => lexical_normalize(&abs),
        Err(_) => lexical_normalize(&expanded),
    }
}

/// 词法规整：消除 `.` 与多余的 `..`（不访问文件系统）
pub fn lexical_normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                // 根目录之上的 `..` 直接忽略（`/../a` == `/a`）；相对路径才保留
                if !out.pop() && !out.is_absolute() {
                    out.push("..");
                }
            }
            other => out.push(other.as_os_str()),
        }
    }
    if out.as_os_str().is_empty() {
        PathBuf::from(".")
    } else {
        out
    }
}

/// 递归创建目录，失败静默（与原实现一致：目录不可用时留到使用处报错）
pub fn ensure_directory(path: &Path) -> bool {
    if path.as_os_str().is_empty() {
        return false;
    }
    if path.is_dir() {
        return true;
    }
    std::fs::create_dir_all(path).is_ok()
}

/// 判断两个路径是否指向同一个文件（词法规整后按字符串比较；
/// 与原实现的 `PathsEqual` 一致，见 `backend-data.md` §4.3）
pub fn paths_equal(a: &str, b: &str) -> bool {
    if a.trim().is_empty() || b.trim().is_empty() {
        return false;
    }
    let pa = to_absolute(a);
    let pb = to_absolute(b);
    if is_windows() {
        pa.to_string_lossy().to_lowercase() == pb.to_string_lossy().to_lowercase()
    } else {
        pa == pb
    }
}

/// 生成一个临时目录路径（测试素材、试运行输出用）
pub fn temp_path(prefix: &str, extension: &str) -> PathBuf {
    let name = format!("{prefix}-{}{}", Uuid::new_v4().simple(), if extension.is_empty() {
        String::new()
    } else {
        format!(".{extension}")
    });
    std::env::temp_dir().join(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn platform_is_known() {
        assert!(matches!(platform(), "windows" | "macos" | "linux"));
        assert!(!platform_display_name().is_empty());
    }

    #[test]
    fn to_absolute_removes_quotes_and_dots() {
        let path = to_absolute("\"./foo/../bar/baz.mp4\"");
        assert!(path.is_absolute(), "{path:?}");
        assert!(path.ends_with("bar/baz.mp4"), "{path:?}");
    }

    #[test]
    fn to_absolute_expands_home() {
        let Some(home) = dirs::home_dir() else { return };
        assert_eq!(to_absolute("~/Movies"), home.join("Movies"));
        assert_eq!(to_absolute("~"), home);
    }

    #[test]
    fn to_absolute_handles_empty() {
        assert_eq!(to_absolute("   "), PathBuf::new());
    }

    #[test]
    fn to_absolute_keeps_relative_parent_beyond_root() {
        // 纯词法规整不应把根目录之上的 .. 吞掉
        let path = lexical_normalize(Path::new("/../a"));
        assert_eq!(path, PathBuf::from("/a"));
    }

    #[test]
    fn data_directory_is_absolute() {
        let dir = data_directory();
        assert!(dir.is_absolute(), "{dir:?}");
    }
}
