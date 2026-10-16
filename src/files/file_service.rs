//! 文件服务：媒体文件扫描、大小/磁盘空间、可访问性检查。
//!
//! 对应参考实现 `Services/FileService.cs`。

use std::path::Path;

use serde::Serialize;

/// 默认媒体扩展名（带点，与参考实现逐字一致）
pub const DEFAULT_MEDIA_EXTENSIONS: &[&str] = &[
    ".mp4", ".avi", ".mkv", ".mov", ".wmv", ".flv", ".webm", ".m4v", ".ts", ".mpg", ".mpeg", ".3gp", ".ogv",
    ".mp3", ".wav", ".flac", ".aac", ".ogg", ".wma", ".m4a", ".opus", ".aiff", ".gif", ".png", ".jpg", ".jpeg",
    ".bmp", ".tiff", ".webp",
];

/// 扫描到的媒体文件
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MediaFileEntry {
    pub path: String,
    pub name: String,
    pub size: i64,
}

/// 支持的媒体扩展名（带点）
pub fn supported_media_extensions() -> Vec<String> {
    DEFAULT_MEDIA_EXTENSIONS.iter().map(|e| e.to_string()).collect()
}

/// 输入文件是否可访问：存在且非空
pub fn is_file_accessible(path: &str) -> bool {
    if path.trim().is_empty() {
        return false;
    }
    let absolute = crate::paths::to_absolute(path);
    match std::fs::metadata(&absolute) {
        Ok(meta) => meta.is_file() && meta.len() > 0,
        Err(_) => false,
    }
}

/// 文件大小（字节）；不存在或读不到返回 0
pub fn get_file_size(path: &str) -> i64 {
    std::fs::metadata(crate::paths::to_absolute(path)).map(|m| m.len() as i64).unwrap_or(0)
}

/// 目录是否可写（不存在时会尝试创建）
pub fn is_directory_writable(path: &str) -> bool {
    if path.trim().is_empty() {
        return false;
    }
    super::output_path::is_output_path_writable(&Path::new(path).join("probe.tmp").to_string_lossy())
}

/// 目标所在文件系统的可用空间（字节）。取不到时返回 0。
pub fn available_disk_space(path: &str) -> u64 {
    let mut target = crate::paths::to_absolute(path);
    // 有些平台要求路径已存在：向上找到第一个存在的祖先目录
    while !target.is_dir() {
        match target.parent() {
            Some(parent) if parent != target => target = parent.to_path_buf(),
            _ => break,
        }
    }
    fs4::available_space(&target).unwrap_or(0)
}

/// 扫描目录下的媒体文件；按路径忽略大小写排序。目录不存在或无权限时返回空集合。
pub fn scan_media_files(directory: &str, extensions: &[String], recursive: bool) -> Vec<MediaFileEntry> {
    let directory = crate::paths::to_absolute(directory);
    if !directory.is_dir() {
        return Vec::new();
    }

    let wanted: Vec<String> = if extensions.is_empty() {
        DEFAULT_MEDIA_EXTENSIONS.iter().map(|e| e.to_ascii_lowercase()).collect()
    } else {
        extensions
            .iter()
            .map(|e| {
                let lower = e.trim().to_ascii_lowercase();
                if lower.starts_with('.') { lower } else { format!(".{lower}") }
            })
            .collect()
    };

    let mut results = Vec::new();
    collect(&directory, &wanted, recursive, &mut results, 0);

    results.sort_by(|a, b| a.path.to_lowercase().cmp(&b.path.to_lowercase()));
    results
}

/// 递归深度上限：防止误选 `/` 之类目录时把整块盘扫一遍
const MAX_SCAN_DEPTH: usize = 12;

fn collect(directory: &Path, wanted: &[String], recursive: bool, out: &mut Vec<MediaFileEntry>, depth: usize) {
    if out.len() >= 20_000 {
        return;
    }

    let Ok(entries) = std::fs::read_dir(directory) else { return };

    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(file_type) = entry.file_type() else { continue };

        if file_type.is_dir() {
            if recursive && depth < MAX_SCAN_DEPTH {
                collect(&path, wanted, recursive, out, depth + 1);
            }
            continue;
        }
        if !file_type.is_file() {
            continue;
        }

        let extension = path
            .extension()
            .map(|e| format!(".{}", e.to_string_lossy().to_ascii_lowercase()))
            .unwrap_or_default();
        if !wanted.contains(&extension) {
            continue;
        }

        let size = entry.metadata().map(|m| m.len() as i64).unwrap_or(0);
        let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        out.push(MediaFileEntry { path: path.to_string_lossy().to_string(), name, size });
    }
}

/// 人类可读的文件大小：`0 B` / `1.5 KB` / `2.34 MB`（与参考实现的 `FormatFileSize` 一致）
pub fn format_file_size(bytes: i64) -> String {
    if bytes <= 0 {
        return "0 B".to_string();
    }
    const UNITS: [&str; 6] = ["B", "KB", "MB", "GB", "TB", "PB"];
    let mut length = bytes as f64;
    let mut order = 0;
    while length >= 1024.0 && order < UNITS.len() - 1 {
        order += 1;
        length /= 1024.0;
    }
    format!("{} {}", trim_trailing_zeros(length), UNITS[order])
}

/// 最多两位小数、去掉无意义的尾随 0（对应 .NET 的 `0.##`）
fn trim_trailing_zeros(value: f64) -> String {
    let text = format!("{value:.2}");
    let text = text.trim_end_matches('0').trim_end_matches('.');
    if text.is_empty() { "0".to_string() } else { text.to_string() }
}

/// 在系统文件管理器中显示目标
pub fn reveal_in_file_manager(path: &str) -> Result<(), String> {
    let target = crate::paths::to_absolute(path);
    if !target.exists() {
        return Err(format!("路径不存在：{}", target.to_string_lossy()));
    }
    let target_arg = target.to_string_lossy().to_string();

    #[cfg(target_os = "macos")]
    let mut command = {
        let mut c = std::process::Command::new("open");
        c.arg("-R").arg(&target_arg);
        c
    };
    #[cfg(target_os = "windows")]
    let mut command = {
        let mut c = std::process::Command::new("explorer");
        c.arg(format!("/select,{target_arg}"));
        c
    };
    #[cfg(all(unix, not(target_os = "macos")))]
    let mut command = {
        let directory = if target.is_dir() {
            target.clone()
        } else {
            target.parent().map(std::path::PathBuf::from).unwrap_or(target.clone())
        };
        let mut c = std::process::Command::new("xdg-open");
        c.arg(directory);
        c
    };

    command
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("无法打开文件管理器：{e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn size_formatting_matches_reference() {
        assert_eq!(format_file_size(0), "0 B");
        assert_eq!(format_file_size(-5), "0 B");
        assert_eq!(format_file_size(512), "512 B");
        assert_eq!(format_file_size(1024), "1 KB");
        assert_eq!(format_file_size(1536), "1.5 KB");
        assert_eq!(format_file_size(1024 * 1024 * 3 / 2), "1.5 MB");
        assert_eq!(format_file_size(42 * 1024 * 1024 * 1024), "42 GB");
    }

    #[test]
    fn extensions_include_dot_and_are_complete() {
        let extensions = supported_media_extensions();
        assert_eq!(extensions.len(), DEFAULT_MEDIA_EXTENSIONS.len());
        assert!(extensions.iter().all(|e| e.starts_with('.')));
        assert!(extensions.contains(&".opus".to_string()));
    }

    #[test]
    fn scan_finds_media_files_and_respects_extension_filter() {
        let dir = std::env::temp_dir().join(format!("ffr-scan-{}", std::process::id()));
        let nested = dir.join("nested");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(dir.join("a.mp4"), b"x").unwrap();
        std::fs::write(dir.join("b.txt"), b"x").unwrap();
        std::fs::write(nested.join("c.mkv"), b"x").unwrap();

        let flat = scan_media_files(&dir.to_string_lossy(), &[], false);
        assert_eq!(flat.len(), 1, "{flat:?}");
        assert!(flat[0].name == "a.mp4");

        let deep = scan_media_files(&dir.to_string_lossy(), &[], true);
        assert_eq!(deep.len(), 2, "{deep:?}");

        let only_mkv = scan_media_files(&dir.to_string_lossy(), &["mkv".to_string()], true);
        assert_eq!(only_mkv.len(), 1);
        assert!(only_mkv[0].name == "c.mkv");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn scan_missing_directory_is_empty() {
        assert!(scan_media_files("/definitely/not/here", &[], false).is_empty());
    }

    #[test]
    fn accessible_requires_non_empty_file() {
        let dir = std::env::temp_dir().join(format!("ffr-access-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("empty.bin");
        std::fs::write(&file, b"").unwrap();
        assert!(!is_file_accessible(&file.to_string_lossy()), "空文件不算可访问");

        std::fs::write(&file, b"data").unwrap();
        assert!(is_file_accessible(&file.to_string_lossy()));
        assert_eq!(get_file_size(&file.to_string_lossy()), 4);
        assert_eq!(get_file_size("/nope/nope"), 0);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn disk_space_is_reported_for_existing_path() {
        let space = available_disk_space(&std::env::temp_dir().to_string_lossy());
        assert!(space > 0, "临时目录所在卷应能取到可用空间");
    }

    #[test]
    fn writable_probe_works_for_directory() {
        let dir = std::env::temp_dir().join(format!("ffr-writable-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert!(is_directory_writable(&dir.to_string_lossy()));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
