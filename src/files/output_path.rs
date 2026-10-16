//! 输出路径解析与文件名占位符。
//!
//! 对应参考实现 `Services/OutputPathResolver.cs`（冲突策略）与
//! `Services/FileService.cs:85-169`（文件名模板 + 清洗）。
//!
//! 这两件事在语义上是分开的：
//!
//! 1. **用户命名阶段**：`format_file_name` 把「输出文件名模板」展开成文件名；
//! 2. **任务执行前**：`resolve_output_path` 再做一次冲突解析 —— 排队等待期间目标文件
//!    可能已被别人创建，所以必须重新判定。
//!
//! 安全不变式（移植时必须保留）：
//! - 输出路径与输入文件相同时**强制改名**，与「文件已存在时的处理方式」无关
//!   （否则会一边读一边写同一个文件，源文件直接损坏）。
//! - 文件名清洗在**占位符展开之后**执行，因此 `/`、`\` 这类路径分隔符会被替换掉，
//!   用户模板无法逃出输出目录。

use std::path::Path;

use chrono::{Datelike, Local, Timelike};
use rand::RngExt;
use regex::Regex;

use crate::models::enums::FileExistsAction;
use crate::paths;

/// 自动改名的最大尝试次数（参考实现 `MaxRenameAttempts = 9999`）
const MAX_RENAME_ATTEMPTS: u32 = 9999;

/// 冲突解析结果
#[derive(Debug, Clone, PartialEq)]
pub struct OutputPathResolution {
    /// 最终使用的输出路径
    pub output_path: String,
    /// 决定 ffmpeg 加 `-y`(true) / `-n`(false)
    pub overwrite: bool,
    /// 是否应当跳过本次转换
    pub skip: bool,
    /// 相对请求路径是否发生了自动改名
    pub renamed: bool,
    /// 给用户看的说明（会写进任务日志）
    pub note: Option<String>,
}

impl OutputPathResolution {
    fn passthrough(path: String, overwrite: bool) -> Self {
        Self { output_path: path, overwrite, skip: false, renamed: false, note: None }
    }

    fn skipped(path: String) -> Self {
        Self {
            output_path: path,
            overwrite: false,
            skip: true,
            renamed: false,
            note: Some("输出文件已存在，按设置跳过本次转换。".to_string()),
        }
    }
}

/// 冲突解析决策表（顺序严格，命中即返回）。
///
/// `input_path` 用于「输出 == 输入」安全检查，**必须传**；传空字符串则跳过该检查。
pub fn resolve_output_path(requested: &str, action: FileExistsAction, input_path: &str) -> OutputPathResolution {
    // 1) 请求路径为空：原样返回，交给上层报错
    if requested.trim().is_empty() {
        return OutputPathResolution::passthrough(requested.to_string(), true);
    }

    let absolute = paths::to_absolute(requested);
    let absolute_str = absolute.to_string_lossy().to_string();

    // 2) 输出 == 输入：无条件强制改名（优先于一切策略）
    if !input_path.trim().is_empty() && paths::paths_equal(&absolute_str, input_path) {
        let mut resolution = rename(&absolute);
        resolution.note = Some("输出路径与输入文件相同，已自动改名以避免覆盖源文件。".to_string());
        return resolution;
    }

    // 3) 文件不存在
    if !absolute.is_file() {
        return OutputPathResolution::passthrough(absolute_str, true);
    }

    // 4) 存在 + 跳过
    if action == FileExistsAction::Skip {
        return OutputPathResolution::skipped(absolute_str);
    }

    // 5) 存在 + 覆盖
    if action == FileExistsAction::Overwrite {
        let mut resolution = OutputPathResolution::passthrough(absolute_str, true);
        resolution.note = Some("输出文件已存在，将按设置覆盖。".to_string());
        return resolution;
    }

    // 6) 存在 + 改名
    if action == FileExistsAction::Rename {
        return rename(&absolute);
    }

    // 7) 存在 + 询问（默认分支）：执行层不再等用户确认，直接改成不冲突的名字。
    //    真正"询问"发生在界面预览阶段（见 docs/reference/backend-data.md §4.9）。
    let mut resolution = rename(&absolute);
    resolution.note = Some("输出文件已存在，已自动重命名以避免覆盖原文件。".to_string());
    resolution
}

/// 自动改名：`a.mp4` → `a (1).mp4` → `a (2).mp4` …，全部占用时用本地时间戳兜底
fn rename(absolute: &Path) -> OutputPathResolution {
    let Some(dir) = absolute.parent() else {
        return OutputPathResolution::passthrough(absolute.to_string_lossy().to_string(), true);
    };
    if dir.as_os_str().is_empty() {
        return OutputPathResolution::passthrough(absolute.to_string_lossy().to_string(), true);
    }

    let stem = absolute.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
    let extension = absolute.extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();

    for index in 1..=MAX_RENAME_ATTEMPTS {
        let candidate = dir.join(format!("{stem} ({index}){extension}"));
        if !candidate.exists() {
            return renamed_resolution(candidate, &stem);
        }
    }

    // 兜底：本地时间戳（参考实现就是这么做的）
    let stamp = Local::now().format("%Y%m%d%H%M%S");
    let candidate = dir.join(format!("{stem}_{stamp}{extension}"));
    renamed_resolution(candidate, &stem)
}

fn renamed_resolution(candidate: std::path::PathBuf, _stem: &str) -> OutputPathResolution {
    let file_name = candidate
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    OutputPathResolution {
        output_path: candidate.to_string_lossy().to_string(),
        overwrite: true,
        skip: false,
        renamed: true,
        note: Some(format!("目标文件已存在，已自动重命名为 {file_name}。")),
    }
}

/// 输出目录是否可写：不存在则先建，然后真的写一个探测文件再删掉。
///
/// 与原实现的差异：探测文件一定会被删除（原实现用 `DeleteOnClose`，
/// 进程崩溃时会残留 `.ffmpegwebui-write-test-*`）。
pub fn is_output_path_writable(output_path: &str) -> bool {
    let absolute = paths::to_absolute(output_path);
    let Some(dir) = absolute.parent() else { return false };
    if dir.as_os_str().is_empty() {
        return false;
    }

    if !dir.is_dir() && !paths::ensure_directory(dir) {
        return false;
    }

    let probe = dir.join(format!(".ffmpegwebui-write-test-{}", random_hex(16)));
    match std::fs::OpenOptions::new().write(true).create_new(true).open(&probe) {
        Ok(mut file) => {
            use std::io::Write;
            let wrote = file.write_all(b"x").is_ok();
            drop(file);
            let _ = std::fs::remove_file(&probe);
            wrote
        }
        Err(_) => false,
    }
}

// ────────────────────────────────────────────────────────────────────────────
// 文件名模板（占位符）
// ────────────────────────────────────────────────────────────────────────────

/// 展开「输出文件名模板」，返回**不含扩展名**的文件名（调用方负责拼 `.{ext}`）。
///
/// 展开顺序固定：内置占位符 → `{now:FMT}` → `{random:N}` → `{random}` → `{counter:N}` → 清洗。
/// 未知占位符原样保留。
pub fn format_file_name(template: &str, input_path: &str, extension: &str) -> String {
    let input = Path::new(input_path);
    let file_stem = input.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();

    // 模板为空 → 直接用输入文件名
    if template.trim().is_empty() {
        return sanitize_file_name(&file_stem);
    }

    let now = Local::now();
    let input_ext = input.extension().map(|e| e.to_string_lossy().to_string()).unwrap_or_default();
    let output_ext = normalize_extension(extension);
    let dir_name = input
        .parent()
        .and_then(|p| p.file_name())
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();

    // 1) 固定占位符（大小写不敏感）
    let mut result = template.to_string();
    for (name, value) in [
        ("filename", file_stem.clone()),
        ("ext", input_ext),
        ("outputext", output_ext),
        ("dir", dir_name),
        ("datetime", now.format("%Y%m%d_%H%M%S").to_string()),
        ("date", now.format("%Y%m%d").to_string()),
        ("time", now.format("%H%M%S").to_string()),
        ("year", now.format("%Y").to_string()),
        ("month", now.format("%m").to_string()),
        ("day", now.format("%d").to_string()),
        ("hour", now.format("%H").to_string()),
        ("minute", now.format("%M").to_string()),
        ("second", now.format("%S").to_string()),
    ] {
        result = replace_placeholder_ignore_case(&result, name, &value);
    }

    // 2) {now:FMT} —— 格式非法时原样保留
    if let Ok(re) = Regex::new(r"(?i)\{now:([^}]+)\}") {
        result = re
            .replace_all(&result, |caps: &regex::Captures| {
                match format_dotnet_datetime(&caps[1], &now) {
                    Some(rendered) => rendered,
                    // 原实现抛异常被 catch → 保留原文
                    None => caps[0].to_string(),
                }
            })
            .to_string();
    }

    // 3) {random:N}（N 夹到 1..32）
    if let Ok(re) = Regex::new(r"(?i)\{random:(\d+)\}") {
        result = re
            .replace_all(&result, |caps: &regex::Captures| {
                let digits = caps[1].parse::<usize>().unwrap_or(8).clamp(1, 32);
                random_hex(16)[..digits].to_string()
            })
            .to_string();
    }

    // 4) {random}（与 {random:N} 是两次独立生成）
    result = replace_placeholder_ignore_case(&result, "random", &random_hex(4));

    // 5) {counter:N}（N 夹到 1..10）
    if let Ok(re) = Regex::new(r"(?i)\{counter:(\d+)\}") {
        result = re
            .replace_all(&result, |caps: &regex::Captures| {
                let digits = caps[1].parse::<u32>().map(|d| d.clamp(1, 10)).unwrap_or(1);
                let ticks = ticks_since_year_one(&now);
                let modulus = 10_i64.pow(digits);
                format!("{:0width$}", ticks % modulus, width = digits as usize)
            })
            .to_string();
    }

    // 6) 清洗（必须在展开之后，否则用户模板能用 `/` 逃出输出目录）
    sanitize_file_name(&result)
}

/// 大小写不敏感地替换 `{name}`（原实现所有占位符名匹配都不区分大小写）
fn replace_placeholder_ignore_case(text: &str, name: &str, value: &str) -> String {
    let pattern = format!(r"(?i)\{{{}\}}", regex::escape(name));
    match Regex::new(&pattern) {
        Ok(re) => re.replace_all(text, regex::NoExpand(value)).to_string(),
        Err(_) => text.to_string(),
    }
}

/// 清空：替换非法字符 → 去首尾空白与尾部点 → Windows 保留设备名 → 空则 `output`
pub fn sanitize_file_name(name: &str) -> String {
    let mut cleaned: String = name.chars().map(|c| if is_invalid_file_name_char(c) { '_' } else { c }).collect();

    cleaned = cleaned.trim().to_string();
    cleaned = cleaned.trim_end_matches(['.', ' ']).to_string();

    if cfg!(target_os = "windows") && is_reserved_device_name(&cleaned) {
        cleaned = format!("_{cleaned}");
    }

    if cleaned.is_empty() {
        "output".to_string()
    } else {
        cleaned
    }
}

/// 当前平台的非法文件名字符。
///
/// 刻意**不**统一成 Windows 的集合：macOS/Linux 上 `:` 是合法字符，
/// 按 Windows 规则清洗会把用户本来看起来正常的名字改掉。
fn is_invalid_file_name_char(c: char) -> bool {
    if c == '\0' || c == '/' {
        return true;
    }
    if cfg!(target_os = "windows") {
        return matches!(c, '<' | '>' | ':' | '"' | '\\' | '|' | '?' | '*') || (c as u32) < 0x20;
    }
    false
}

/// Windows 保留设备名
fn is_reserved_device_name(name: &str) -> bool {
    let upper = name.to_ascii_uppercase();
    let base = upper.split('.').next().unwrap_or(&upper);
    const RESERVED: [&str; 22] = [
        "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8", "COM9",
        "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
    ];
    RESERVED.contains(&base)
}

/// `.NET DateTime.Now.Ticks` 的等价计算：自 0001-01-01 起的 100ns 数
///
/// 注意不能用 `(nanos + 62135596800 * 1_000_000_000) / 100`：
/// `62135596800 * 1e9 = 6.2e19` 超出 `i64`（上限约 9.2e18），`cargo build` 会直接报
/// `arithmetic_overflow`（`cargo check` 不报，容易漏）。这里先各自除以 100 再相加。
fn ticks_since_year_one(now: &chrono::DateTime<Local>) -> i64 {
    /// 0001-01-01 → 1970-01-01 的秒数
    const SECONDS_YEAR_ONE_TO_EPOCH: i64 = 62_135_596_800;
    /// 上述秒数对应的 100ns 单位数（62135596800 * 10^7 = 6.2e17，安全落在 i64 内）
    const TICKS_YEAR_ONE_TO_EPOCH: i64 = SECONDS_YEAR_ONE_TO_EPOCH * 10_000_000;

    let nanos = now.timestamp_nanos_opt().unwrap_or(0);
    nanos / 100 + TICKS_YEAR_ONE_TO_EPOCH
}

/// 归一化输出扩展名：去掉前导点，空则 `mp4`
fn normalize_extension(extension: &str) -> String {
    let trimmed = extension.trim().trim_start_matches('.').to_lowercase();
    if trimmed.is_empty() {
        "mp4".to_string()
    } else {
        trimmed
    }
}

/// 生成输出路径：`{目录}/{输入名}{后缀}.{扩展名}`
///
/// 与原实现的差异：输出目录为空时**直接返回空字符串**让上层报错。
/// 原实现用 `Path.Combine` 会把它落到进程 CWD，是个隐患。
pub fn generate_output_path(input: &str, output_directory: &str, extension: &str, suffix: Option<&str>) -> String {
    if output_directory.trim().is_empty() {
        return String::new();
    }
    let stem = Path::new(input)
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    let extension = normalize_extension(extension);
    let suffix = suffix.unwrap_or("");
    Path::new(output_directory)
        .join(format!("{stem}{suffix}.{extension}"))
        .to_string_lossy()
        .to_string()
}

/// 生成不冲突的输出路径（`(1)`、`(2)` … 递增，兜底时间戳）
pub fn generate_unique_output_path(
    input: &str,
    output_directory: &str,
    extension: &str,
    suffix: Option<&str>,
) -> String {
    let candidate = generate_output_path(input, output_directory, extension, suffix);
    if candidate.is_empty() || !Path::new(&candidate).exists() {
        return candidate;
    }
    resolve_output_path(&candidate, FileExistsAction::Rename, "").output_path
}

/// 生成小写十六进制随机串（`bytes` 个字节 → `2 * bytes` 个字符）
fn random_hex(bytes: usize) -> String {
    let mut rng = rand::rng();
    let data: Vec<u8> = (0..bytes).map(|_| rng.random::<u8>()).collect();
    data.iter().map(|b| format!("{b:02x}")).collect()
}

// ────────────────────────────────────────────────────────────────────────────
// .NET 自定义日期格式 → 渲染
// ────────────────────────────────────────────────────────────────────────────

/// 渲染 .NET 自定义日期格式串（`{now:FMT}` 用）。
///
/// 只支持 `docs/reference/backend-data.md` §4.6 列出的子集；遇到无法识别的格式符返回
/// `None` —— 调用方保留原文，与原实现「抛异常被 catch 后返回原样」的行为一致。
pub fn format_dotnet_datetime(format: &str, now: &chrono::DateTime<Local>) -> Option<String> {
    let chars: Vec<char> = format.chars().collect();
    let mut out = String::with_capacity(format.len());
    let mut index = 0;

    while index < chars.len() {
        let c = chars[index];

        // 转义字面量：\x
        if c == '\\' {
            index += 1;
            if index < chars.len() {
                out.push(chars[index]);
                index += 1;
            }
            continue;
        }

        // 引号字面量：'text'
        if c == '\'' || c == '"' {
            let quote = c;
            index += 1;
            while index < chars.len() && chars[index] != quote {
                out.push(chars[index]);
                index += 1;
            }
            index += 1; // 跳过结束引号
            continue;
        }

        // 非字母：字面量（含 : / - _ 空格）
        if !c.is_ascii_alphabetic() {
            out.push(c);
            index += 1;
            continue;
        }

        // 字母：取同字符连续段
        let start = index;
        while index < chars.len() && chars[index] == c {
            index += 1;
        }
        let length = index - start;

        let rendered = match c {
            'y' => match length {
                1 => (now.year() % 100).to_string(),
                2 => format!("{:02}", now.year() % 100),
                3 => format!("{:03}", now.year()),
                _ => format!("{:04}", now.year()),
            },
            'M' => match length {
                1 => now.month().to_string(),
                2 => format!("{:02}", now.month()),
                3 => month_abbrev(now.month()),
                4 => month_full(now.month()),
                _ => return None,
            },
            'd' => match length {
                1 => now.day().to_string(),
                2 => format!("{:02}", now.day()),
                3 => weekday_abbrev(now.weekday()),
                4 => weekday_full(now.weekday()),
                _ => return None,
            },
            'H' => match length {
                1 => now.hour().to_string(),
                _ => format!("{:02}", now.hour()),
            },
            'h' => {
                let hour = now.hour() % 12;
                let hour = if hour == 0 { 12 } else { hour };
                match length {
                    1 => hour.to_string(),
                    _ => format!("{hour:02}"),
                }
            }
            'm' => match length {
                1 => now.minute().to_string(),
                _ => format!("{:02}", now.minute()),
            },
            's' => match length {
                1 => now.second().to_string(),
                _ => format!("{:02}", now.second()),
            },
            'f' => {
                // 亚秒：run 长度 = 小数位数（最多 7 位，chrono 支持到纳秒）
                let nanos = now.timestamp_subsec_nanos();
                let digits = length.min(7);
                let full = format!("{nanos:09}");
                full[..digits].to_string()
            }
            't' => {
                let is_pm = now.hour() >= 12;
                match length {
                    1 => if is_pm { "P" } else { "A" }.to_string(),
                    _ => if is_pm { "PM" } else { "AM" }.to_string(),
                }
            }
            'z' => {
                let offset = now.offset().local_minus_utc();
                let sign = if offset < 0 { '-' } else { '+' };
                let seconds = offset.abs();
                let (hours, minutes) = (seconds / 3600, (seconds % 3600) / 60);
                match length {
                    1 => format!("{sign}{hours}"),
                    2 => format!("{sign}{hours:02}"),
                    _ => format!("{sign}{hours:02}:{minutes:02}"),
                }
            }
            // 其它字母不是受支持的格式符：原实现会抛异常 → 保留原文
            _ => return None,
        };

        out.push_str(&rendered);
    }

    Some(out)
}

fn month_abbrev(month: u32) -> String {
    ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"]
        .get((month as usize).saturating_sub(1))
        .unwrap_or(&"Jan")
        .to_string()
}

fn month_full(month: u32) -> String {
    [
        "January",
        "February",
        "March",
        "April",
        "May",
        "June",
        "July",
        "August",
        "September",
        "October",
        "November",
        "December",
    ]
    .get((month as usize).saturating_sub(1))
    .unwrap_or(&"January")
    .to_string()
}

fn weekday_abbrev(weekday: chrono::Weekday) -> String {
    weekday.to_string()[..3].to_string()
}

fn weekday_full(weekday: chrono::Weekday) -> String {
    weekday.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn temp_dir(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("ffmpeg-web-rust-{tag}-{}", random_hex(6)));
        fs::create_dir_all(&dir).expect("创建临时目录");
        dir
    }

    #[test]
    fn empty_requested_path_passes_through() {
        let resolution = resolve_output_path("", FileExistsAction::Rename, "/in/a.mp4");
        assert_eq!(resolution.output_path, "");
        assert!(resolution.overwrite);
        assert!(!resolution.renamed);
    }

    #[test]
    fn missing_file_is_not_conflict() {
        let dir = temp_dir("noconflict");
        let target = dir.join("out.mp4");
        let resolution = resolve_output_path(&target.to_string_lossy(), FileExistsAction::Rename, "/in/a.mp4");
        assert!(!resolution.renamed);
        assert!(resolution.overwrite);
        assert!(resolution.note.is_none());
    }

    #[test]
    fn skip_policy_marks_skipped() {
        let dir = temp_dir("skip");
        let target = dir.join("out.mp4");
        fs::write(&target, b"x").unwrap();
        let resolution = resolve_output_path(&target.to_string_lossy(), FileExistsAction::Skip, "/in/a.mp4");
        assert!(resolution.skip);
        assert!(!resolution.overwrite);
        assert_eq!(resolution.note.as_deref(), Some("输出文件已存在，按设置跳过本次转换。"));
    }

    #[test]
    fn overwrite_policy_keeps_path() {
        let dir = temp_dir("overwrite");
        let target = dir.join("out.mp4");
        fs::write(&target, b"x").unwrap();
        let resolution = resolve_output_path(&target.to_string_lossy(), FileExistsAction::Overwrite, "/in/a.mp4");
        assert!(resolution.overwrite);
        assert!(!resolution.renamed);
        assert_eq!(resolution.note.as_deref(), Some("输出文件已存在，将按设置覆盖。"));
    }

    #[test]
    fn ask_policy_renames_with_dedicated_note() {
        let dir = temp_dir("ask");
        let target = dir.join("out.mp4");
        fs::write(&target, b"x").unwrap();
        let resolution = resolve_output_path(&target.to_string_lossy(), FileExistsAction::Ask, "/in/a.mp4");
        assert!(resolution.renamed);
        assert_eq!(resolution.note.as_deref(), Some("输出文件已存在，已自动重命名以避免覆盖原文件。"));
        assert!(resolution.output_path.ends_with("out (1).mp4"), "{}", resolution.output_path);
    }

    #[test]
    fn rename_produces_parenthesised_sequence() {
        let dir = temp_dir("rename");
        let target = dir.join("clip.mov");
        fs::write(&target, b"x").unwrap();
        fs::write(dir.join("clip (1).mov"), b"x").unwrap();
        let resolution = resolve_output_path(&target.to_string_lossy(), FileExistsAction::Rename, "/in/a.mp4");
        assert!(resolution.output_path.ends_with("clip (2).mov"), "{}", resolution.output_path);
        assert_eq!(resolution.note.as_deref(), Some("目标文件已存在，已自动重命名为 clip (2).mov。"));
    }

    #[test]
    fn output_equal_to_input_forces_rename_even_with_overwrite() {
        let dir = temp_dir("selfoverwrite");
        let target = dir.join("source.mp4");
        fs::write(&target, b"x").unwrap();
        let resolution =
            resolve_output_path(&target.to_string_lossy(), FileExistsAction::Overwrite, &target.to_string_lossy());
        assert!(resolution.renamed, "输出==输入必须强制改名");
        assert!(resolution.output_path.ends_with("source (1).mp4"), "{}", resolution.output_path);
        assert_eq!(resolution.note.as_deref(), Some("输出路径与输入文件相同，已自动改名以避免覆盖源文件。"));
    }

    #[test]
    fn writable_probe_creates_and_removes_file() {
        let dir = temp_dir("writable");
        let target = dir.join("out.mp4");
        assert!(is_output_path_writable(&target.to_string_lossy()));
        let leftovers: Vec<_> = fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().contains("write-test"))
            .collect();
        assert!(leftovers.is_empty(), "探测文件必须被删掉");
    }

    #[test]
    fn format_file_name_supports_builtin_placeholders() {
        let name = format_file_name("{filename}_{outputext}", "/videos/trip.mp4", "mkv");
        assert_eq!(name, "trip_mkv");
    }

    #[test]
    fn format_file_name_is_case_insensitive() {
        assert_eq!(format_file_name("{FileName}", "/v/a.mp4", "mp4"), "a");
        assert_eq!(format_file_name("{RANDOM:4}", "/v/a.mp4", "mp4").len(), 4);
    }

    #[test]
    fn format_file_name_empty_template_uses_input_name() {
        assert_eq!(format_file_name("   ", "/v/holiday.mkv", "mp4"), "holiday");
    }

    #[test]
    fn format_file_name_keeps_unknown_placeholders() {
        assert_eq!(format_file_name("{unknown}-{filename}", "/v/a.mp4", "mp4"), "{unknown}-a");
    }

    #[test]
    fn format_file_name_uses_dotnet_datetime_format() {
        let name = format_file_name("{now:yyyy-MM-dd}", "/v/a.mp4", "mp4");
        let re = Regex::new(r"^\d{4}-\d{2}-\d{2}$").unwrap();
        assert!(re.is_match(&name), "{name}");
    }

    #[test]
    fn format_file_name_rejects_bad_format_and_keeps_literal() {
        // 'q' 不是受支持的格式符：原实现抛异常后保留原文
        let name = format_file_name("{now:qqq}", "/v/a.mp4", "mp4");
        assert_eq!(name, "{now:qqq}");
    }

    #[test]
    fn counter_placeholder_is_zero_padded() {
        let name = format_file_name("{counter:5}", "/v/a.mp4", "mp4");
        assert_eq!(name.len(), 5, "{name}");
        assert!(name.chars().all(|c| c.is_ascii_digit()), "{name}");
    }

    #[test]
    fn sanitize_replaces_separators_and_empty_becomes_output() {
        assert_eq!(sanitize_file_name("a/b"), "a_b");
        assert_eq!(sanitize_file_name("   "), "output");
        assert_eq!(sanitize_file_name("name..."), "name");
    }

    #[test]
    fn filename_template_cannot_escape_output_directory() {
        // 展开后清洗，`/` 被替换 → 无法写出目录
        let name = format_file_name("../../etc/passwd", "/v/a.mp4", "mp4");
        assert!(!name.contains('/'), "{name}");
    }

    #[test]
    fn generate_output_path_combines_suffix_and_extension() {
        let path = generate_output_path("/v/a.mp4", "/out", "mp4", Some("_converted"));
        assert!(path.ends_with("a_converted.mp4"), "{path}");
        assert_eq!(generate_output_path("/v/a.mp4", "", "mp4", None), "");
    }

    #[test]
    fn generate_unique_output_path_avoids_collision() {
        let dir = temp_dir("unique");
        let existing = dir.join("a_converted.mp4");
        fs::write(&existing, b"x").unwrap();
        let path = generate_unique_output_path("/v/a.mp4", &dir.to_string_lossy(), "mp4", Some("_converted"));
        assert!(path.ends_with("a_converted (1).mp4"), "{path}");
    }

    #[test]
    fn dotnet_format_handles_literals_and_offsets() {
        let now = Local::now();
        let rendered = format_dotnet_datetime("yyyyMMdd_HHmmss", &now).expect("应能渲染");
        assert_eq!(rendered.len(), "20260214_030507".len());
        assert!(format_dotnet_datetime("zzz", &now).unwrap().starts_with(['+', '-']));
        assert!(format_dotnet_datetime("hh tt", &now).is_some());
    }
}
