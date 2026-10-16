//! FFmpeg / FFprobe 可执行文件定位（规格 `ffmpeg-core.md` §2，源 `Services/FFmpegLocator.cs`）。
//!
//! 关键语义（规格 §2.1、§8.2）：`ffmpeg` / `ffprobe` 字段**永远是可执行字符串，不是 `Option`**；
//! 找不到时返回裸名 `"ffmpeg"` / `"ffprobe"`，交给 `Command::new()` 继承 PATH 解析，
//! 让「后来才装好 FFmpeg」的场景仍有救。真正的「找不到」信号是 `*_resolved == false`。
//!
//! 候选顺序（规格 §2.4）：内置目录 → PATH → 平台常见安装位置；
//! 而 ffprobe 专用候选序列是**反过来的**（平台常见位置 → 内置目录，且不含 PATH，见 §2.8）。

use std::path::{Path, PathBuf};

/// ffprobe 的可执行名（规格 §2.8 `ProbeName()`）。
pub const PROBE_NAME: &str = "ffprobe";
/// ffmpeg 的可执行名。
pub const FFMPEG_NAME: &str = "ffmpeg";

/// 命中用户配置的 ffmpeg 路径时的提示文案（规格 §2.3，界面会直接显示）。
pub const HINT_USER_PATH: &str = "使用设置中指定的路径";
/// 自动探测成功时的提示文案。
pub const HINT_AUTO_DETECTED: &str = "自动探测到 FFmpeg";
/// 自动探测失败时的提示文案。
pub const HINT_NOT_FOUND: &str = "未找到 FFmpeg，请在设置中指定路径";

/// 可执行文件的命中来源，用于 `FFmpegPaths::source` 的中文描述。
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub enum Location {
    /// 用户显式配置的路径
    UserConfigured,
    /// 用户配置的 ffmpeg 同目录下的 ffprobe
    Sibling,
    /// 随应用内置的目录
    Bundled,
    /// 系统 PATH
    Path,
    /// 平台常见安装位置
    WellKnown,
}

impl Location {
    /// 中文描述。
    pub fn label(self) -> &'static str {
        match self {
            Location::UserConfigured => "用户指定",
            Location::Sibling => "ffmpeg 同目录",
            Location::Bundled => "内置目录",
            Location::Path => "PATH",
            Location::WellKnown => "平台固定路径",
        }
    }
}

/// ffmpeg / ffprobe 的解析结果（对应 C# `FFmpegPaths`）。
#[derive(Debug, Clone, serde::Serialize)]
pub struct FFmpegPaths {
    /// ffmpeg 可执行文件（绝对路径，或裸名 `"ffmpeg"`）
    pub ffmpeg: String,
    /// 是否成功解析到实际存在的 ffmpeg
    pub ffmpeg_resolved: bool,
    /// ffprobe 可执行文件（绝对路径，或裸名 `"ffprobe"`）
    pub ffprobe: String,
    /// 是否成功解析到实际存在的 ffprobe
    pub ffprobe_resolved: bool,
    /// 数据来源描述（内置目录 / PATH / 平台固定路径 / 用户指定 / 未找到）
    pub source: String,
    /// 未找到时的可读提示（文案见规格 §2.3）
    pub hint: String,
}

impl Default for FFmpegPaths {
    fn default() -> Self {
        Self {
            ffmpeg: FFMPEG_NAME.to_string(),
            ffmpeg_resolved: false,
            ffprobe: PROBE_NAME.to_string(),
            ffprobe_resolved: false,
            source: "未找到".to_string(),
            hint: HINT_NOT_FOUND.to_string(),
        }
    }
}

/// 解析可执行文件名（`name` + 平台后缀；macOS/Linux 不加后缀，见规格 §1.1）。
pub fn executable_name(name: &str) -> String {
    format!("{name}{}", crate::paths::exe_suffix())
}

/// 判断某个路径是否是可用的可执行文件（C# `File.Exists`）。
pub fn is_executable_file(path: &str) -> bool {
    !path.trim().is_empty() && Path::new(path).is_file()
}

/// 当前可执行文件所在目录（`AppContext.BaseDirectory` 的等价物，规格 §2.4）。
///
/// PORT NOTE：C# 的 `AppContext.BaseDirectory` 在单文件发布/符号链接下与
/// `argv[0]` 的行为不同；这里统一采用 `std::env::current_exe()` 的父目录
/// （对 `cargo run`、`cargo test` 与普通发布都稳定）。
pub fn base_directory() -> Option<PathBuf> {
    std::env::current_exe().ok().and_then(|p| p.parent().map(Path::to_path_buf))
}

/// 值里不含任何目录分隔符时，视为「交给系统 PATH 解析的可执行名」（规格 §2.6 `IsOnPath`）。
pub fn is_on_path(value: &str) -> bool {
    !value.contains('/') && !value.contains('\\')
}

/// 内置候选（相对可执行文件目录），顺序即优先级（规格 §2.4 `BundledCandidates`）。
pub fn bundled_candidates(name: &str) -> Vec<String> {
    let exe = executable_name(name);
    vec![
        exe.clone(),
        format!("ffmpeg/{exe}"),
        format!("tools/{exe}"),
        format!("tools/ffmpeg/{exe}"),
        format!("runtimes/ffmpeg/{exe}"),
    ]
}

/// 系统 PATH 查找（规格 §2.5）。
///
/// 只读 `PATH`；每段仅去掉首尾双引号（兼容 Windows `"C:\Program Files\..."` 式 PATH 项），
/// 不做 `~` 展开；非法 PATH 项直接忽略。
pub fn find_on_path(name: &str) -> Option<String> {
    let path_var = std::env::var_os("PATH")?;
    let separator = if cfg!(windows) { ';' } else { ':' };
    let exe = executable_name(name);

    for dir in path_var.to_string_lossy().split(separator) {
        if dir.is_empty() {
            continue;
        }
        let trimmed = dir.trim_matches('"');
        if trimmed.is_empty() {
            continue;
        }
        let candidate = Path::new(trimmed).join(&exe);
        if candidate.is_file() {
            return Some(candidate.to_string_lossy().into_owned());
        }
    }
    None
}

/// 平台常见安装位置，顺序即优先级（规格 §2.7，逐条对应源码 `FFmpegLocator.cs:147-214`）。
pub fn well_known_candidates(name: &str) -> Vec<String> {
    let exe = executable_name(name);
    let home = dirs::home_dir();
    let mut out: Vec<String> = Vec::new();

    if crate::paths::is_windows() {
        // 1/2/6 是硬编码字符串（不随系统盘符变化），与源码一致。
        out.push(join_dir("C:\\ffmpeg\\bin", &exe));
        out.push(join_dir("C:\\ffmpeg", &exe));
        if let Some(dir) = env_dir("ProgramFiles") {
            out.push(join_dir(&join_dir(&join_dir(&dir, "ffmpeg"), "bin"), &exe));
        }
        if let Some(dir) = env_dir("ProgramFiles(x86)") {
            out.push(join_dir(&join_dir(&join_dir(&dir, "ffmpeg"), "bin"), &exe));
        }
        if let Some(dir) = env_dir("LOCALAPPDATA") {
            out.push(join_dir(&join_dir(&join_dir(&dir, "Microsoft"), "WinGet"), &join_dir("Links", &exe)));
        }
        out.push(join_dir("C:\\ProgramData\\chocolatey\\bin", &exe));
        if let Some(dir) = env_dir("LOCALAPPDATA") {
            out.push(join_dir(&join_dir(&join_dir(&dir, "Microsoft"), "WindowsApps"), &exe));
        }
        if let Some(dir) = env_dir("APPDATA") {
            out.push(join_dir(&join_dir(&dir, "ffmpeg"), &join_dir("bin", &exe)));
        }
        if let Some(home) = home.as_ref() {
            out.push(join_dir(&home.join("scoop").join("shims").to_string_lossy(), &exe));
            out.push(join_dir(
                &home.join("scoop").join("apps").join("ffmpeg").join("current").join("bin").to_string_lossy(),
                &exe,
            ));
        }
    } else if crate::paths::is_macos() {
        out.push(join_dir("/opt/homebrew/bin", &exe)); // Apple Silicon Homebrew
        out.push(join_dir("/usr/local/bin", &exe)); // Intel Homebrew
        out.push(join_dir("/opt/local/bin", &exe)); // MacPorts
        out.push(join_dir("/usr/bin", &exe));
        out.push(join_dir("/nix/var/nix/profiles/default/bin", &exe));
        out.push(join_dir("/run/current-system/sw/bin", &exe));
        if let Some(home) = home.as_ref() {
            out.push(join_dir(&home.join(".local").join("bin").to_string_lossy(), &exe));
            out.push(join_dir(&home.join("bin").to_string_lossy(), &exe));
        }
    } else {
        // 一切非 Windows / 非 macOS 的平台都走 Linux 分支（含未知系统）。
        // 1 是 jellyfin-ffmpeg：注释明确「优先它，因为 QSV/VAAPI 支持更完整」。
        out.push(join_dir("/usr/lib/jellyfin-ffmpeg", &exe));
        out.push(join_dir("/usr/local/bin", &exe));
        out.push(join_dir("/usr/bin", &exe));
        out.push(join_dir("/bin", &exe));
        out.push(join_dir("/snap/bin", &exe));
        out.push(join_dir("/var/lib/flatpak/exports/bin", &exe));
        out.push(join_dir("/nix/var/nix/profiles/default/bin", &exe));
        out.push(join_dir("/run/current-system/sw/bin", &exe));
        if let Some(home) = home.as_ref() {
            out.push(join_dir(&home.join(".local").join("bin").to_string_lossy(), &exe));
            out.push(join_dir(&home.join("bin").to_string_lossy(), &exe));
            out.push(join_dir(&home.join(".nix-profile").join("bin").to_string_lossy(), &exe));
        }
    }
    out
}

/// 通用查找：内置目录 → PATH → 平台常见位置（规格 §2.4）。
pub fn find_executable(name: &str) -> Option<String> {
    find_executable_from(name).map(|(path, _)| path)
}

/// 同 [`find_executable`]，但额外返回命中来源。
pub fn find_executable_from(name: &str) -> Option<(String, Location)> {
    if let Some(base) = base_directory() {
        for rel in bundled_candidates(name) {
            let candidate = base.join(&rel);
            if candidate.is_file() {
                return Some((candidate.to_string_lossy().into_owned(), Location::Bundled));
            }
        }
    }
    if let Some(found) = find_on_path(name) {
        return Some((found, Location::Path));
    }
    for candidate in well_known_candidates(name) {
        if is_executable_file(&candidate) {
            return Some((candidate, Location::WellKnown));
        }
    }
    None
}

/// ffprobe 专用候选序列：**先平台常见位置，再内置目录**，且不含 PATH 查找（规格 §2.8）。
pub fn probe_candidates_with_location() -> Vec<(String, Location)> {
    let mut out: Vec<(String, Location)> =
        well_known_candidates(PROBE_NAME).into_iter().map(|p| (p, Location::WellKnown)).collect();
    if let Some(base) = base_directory() {
        for rel in bundled_candidates(PROBE_NAME) {
            out.push((base.join(&rel).to_string_lossy().into_owned(), Location::Bundled));
        }
    }
    out
}

/// ffprobe 专用候选序列（只要路径）。
pub fn probe_candidates() -> Vec<String> {
    probe_candidates_with_location().into_iter().map(|(path, _)| path).collect()
}

/// 找 ffmpeg 同目录下的 ffprobe（规格 §2.8 `FindSiblingProbe`）。
pub fn find_sibling_probe(ffmpeg_path: &str) -> Option<String> {
    let parent = Path::new(ffmpeg_path).parent()?;
    if parent.as_os_str().is_empty() {
        return None;
    }
    let candidate = parent.join(executable_name(PROBE_NAME));
    if candidate.is_file() {
        Some(candidate.to_string_lossy().into_owned())
    } else {
        None
    }
}

/// 解析可执行文件路径（规格 §2.3 `ResolveCore()` 决策树）。
///
/// * `custom_ffmpeg` / `custom_ffprobe` 对应设置里的 `FFmpegPath` / `FFprobePath`；
///   `None`、空串、全空白都视为「未配置」。
///
/// PORT NOTE：分支 1（用户配置了 ffmpeg）里 `ffmpeg_resolved` **硬编码为 `true`**，
/// 即使配置的只是一个裸名（规格 §8.8 的有意复刻）。
pub fn resolve(custom_ffmpeg: Option<&str>, custom_ffprobe: Option<&str>) -> FFmpegPaths {
    let custom_ffmpeg = non_blank(custom_ffmpeg);
    let custom_ffprobe = non_blank(custom_ffprobe);

    // ── 分支 1：用户显式配置了 ffmpeg 路径 ──────────────────────────────
    if let Some(raw) = custom_ffmpeg {
        let configured = crate::paths::to_absolute(raw).to_string_lossy().into_owned();
        if is_executable_file(&configured) || is_on_path(&configured) {
            let (probe, probe_location) = match custom_ffprobe {
                Some(raw_probe) => (
                    crate::paths::to_absolute(raw_probe).to_string_lossy().into_owned(),
                    Some(Location::UserConfigured),
                ),
                None => match find_sibling_probe(&configured) {
                    Some(sibling) => (sibling, Some(Location::Sibling)),
                    None => probe_candidates_with_location()
                        .into_iter()
                        .find(|(candidate, _)| is_executable_file(candidate))
                        .map(|(candidate, location)| (candidate, Some(location)))
                        .unwrap_or_else(|| (PROBE_NAME.to_string(), None)),
                },
            };
            let probe_resolved = is_executable_file(&probe) || is_on_path(&probe);
            return FFmpegPaths {
                ffmpeg: configured,
                // 规格 §2.3：用户配置分支里 ffmpeg_resolved 硬编码 true。
                ffmpeg_resolved: true,
                source: describe_source(Some(Location::UserConfigured), probe_location, probe_resolved),
                ffprobe: probe,
                ffprobe_resolved: probe_resolved,
                hint: HINT_USER_PATH.to_string(),
            };
        }
    }

    // ── 分支 2：只显式配置了 ffprobe（且真实存在）──────────────────────
    let configured_probe = custom_ffprobe.and_then(|raw| {
        let probe = crate::paths::to_absolute(raw).to_string_lossy().into_owned();
        if is_executable_file(&probe) { Some(probe) } else { None }
    });

    // ── 分支 3：自动探测 ────────────────────────────────────────────────
    let ffmpeg_hit = find_executable_from(FFMPEG_NAME);
    let probe_hit = match &configured_probe {
        Some(probe) => Some((probe.clone(), Location::UserConfigured)),
        None => find_executable_from(PROBE_NAME),
    };

    let ffmpeg_resolved = ffmpeg_hit.is_some();
    let probe_resolved = probe_hit.is_some();

    FFmpegPaths {
        ffmpeg: ffmpeg_hit.as_ref().map(|(p, _)| p.clone()).unwrap_or_else(|| FFMPEG_NAME.to_string()),
        ffmpeg_resolved,
        ffprobe: probe_hit.as_ref().map(|(p, _)| p.clone()).unwrap_or_else(|| PROBE_NAME.to_string()),
        ffprobe_resolved: probe_resolved,
        source: describe_source(
            ffmpeg_hit.as_ref().map(|(_, l)| *l),
            probe_hit.as_ref().map(|(_, l)| *l),
            probe_resolved,
        ),
        hint: if ffmpeg_resolved { HINT_AUTO_DETECTED.to_string() } else { HINT_NOT_FOUND.to_string() },
    }
}

fn non_blank(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|v| !v.is_empty())
}

fn describe_source(ffmpeg: Option<Location>, probe: Option<Location>, probe_resolved: bool) -> String {
    let base = ffmpeg.map(Location::label).unwrap_or("未找到").to_string();
    if !probe_resolved {
        return format!("{base}（ffprobe 未找到）");
    }
    match probe {
        Some(location) if Some(location) != ffmpeg => format!("{base}（ffprobe 来自{}）", location.label()),
        _ => base,
    }
}

fn join_dir(dir: &str, leaf: &str) -> String {
    Path::new(dir).join(leaf).to_string_lossy().into_owned()
}

fn env_dir(key: &str) -> Option<String> {
    std::env::var(key).ok().map(|v| v.trim().to_string()).filter(|v| !v.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_candidate_order_matches_spec() {
        let candidates = bundled_candidates("ffmpeg");
        let exe = executable_name("ffmpeg");
        assert_eq!(
            candidates,
            vec![
                exe.clone(),
                format!("ffmpeg/{exe}"),
                format!("tools/{exe}"),
                format!("tools/ffmpeg/{exe}"),
                format!("runtimes/ffmpeg/{exe}"),
            ]
        );
    }

    #[test]
    fn probe_candidates_put_well_known_before_bundled() {
        let candidates = probe_candidates();
        assert!(!candidates.is_empty());
        if cfg!(target_os = "macos") {
            assert_eq!(candidates.first().map(String::as_str), Some("/opt/homebrew/bin/ffprobe"));
        }
        if cfg!(target_os = "linux") {
            assert_eq!(candidates.first().map(String::as_str), Some("/usr/lib/jellyfin-ffmpeg/ffprobe"));
        }
    }

    #[test]
    fn is_on_path_requires_bare_name() {
        assert!(is_on_path("ffmpeg"));
        assert!(!is_on_path("/usr/bin/ffmpeg"));
        assert!(!is_on_path("./ffmpeg"));
    }

    #[test]
    fn resolve_falls_back_to_bare_names_and_hint() {
        // 传一个几乎不可能存在的路径，应当落回自动探测分支。
        let paths = resolve(Some("/nonexistent-dir-ffmpeg-web-rust/ffmpeg"), None);
        assert!(paths.ffmpeg.starts_with("ffmpeg") || paths.ffmpeg_resolved);
        if paths.ffmpeg_resolved {
            assert_eq!(paths.hint, HINT_AUTO_DETECTED);
        } else {
            assert_eq!(paths.hint, HINT_NOT_FOUND);
            assert_eq!(paths.ffmpeg, "ffmpeg");
            assert!(!paths.ffmpeg_resolved);
        }
    }
}
