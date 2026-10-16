//! 全局设置（单条记录，对应参考实现 `Models/AppSettings.cs`）。

use serde::{Deserialize, Serialize};

use super::enums::{FileExistsAction, OutputNamingRule};

/// 并发任务上限的合法区间（模型层允许 1..16；参考实现的设置页表单只允许 1..8，
/// 这里统一采用模型层的 1..16，见 `docs/reference/backend-data.md` §2.2 的说明）。
pub const MAX_CONCURRENT_TASKS_RANGE: (i32, i32) = (1, 16);
/// 单任务日志保留行数区间
pub const MAX_LOG_LINES_RANGE: (i32, i32) = (50, 20_000);
/// 历史保留天数区间（0 = 不清理）
pub const HISTORY_RETENTION_DAYS_RANGE: (i32, i32) = (0, 3_650);
/// 监听端口区间（0 = 自动挑选空闲端口）
pub const LISTEN_PORT_RANGE: (i32, i32) = (0, 65_535);

/// 默认端口（与参考实现的 5265 错开，便于两个应用同时运行）
pub const DEFAULT_LISTEN_PORT: i32 = 5266;

/// 应用全局设置
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppSettings {
    /// 主键（24 位十六进制）
    pub id: String,
    /// ffmpeg 可执行文件路径；空 = 自动探测
    pub ffmpeg_path: String,
    /// ffprobe 可执行文件路径；空 = 自动探测
    pub ffprobe_path: String,
    /// 默认输出目录（首次运行取系统「视频」目录）
    pub default_output_directory: String,
    /// 输出命名规则
    pub output_naming: OutputNamingRule,
    /// 输出后缀（`output_naming = Suffix` 时使用）
    pub output_suffix: String,
    /// 输出文件已存在时的处理方式
    pub file_exists_action: FileExistsAction,
    /// 优先使用硬件编码
    pub prefer_hardware_acceleration: bool,
    /// 优先使用硬件解码（输入侧 `-hwaccel`）
    pub prefer_hardware_decoding: bool,
    /// 首选硬件编码器名；空 = 自动
    pub preferred_hardware_encoder: String,
    /// 硬件编码初始化失败时自动回退软件编码
    pub auto_fallback_to_software: bool,
    /// 同时执行的任务上限（1..16）
    pub max_concurrent_tasks: i32,
    /// 单任务保留的日志行数（50..20000）
    pub max_log_lines: i32,
    /// 任务历史保留天数，0 = 不清理（0..3650）
    pub task_history_retention_days: i32,
    /// 界面主题：`Light` / `Dark` / `System`
    pub theme: String,
    /// 追加到所有 ffmpeg 命令的全局参数（如 `-threads 4`）
    pub global_ffmpeg_arguments: String,
    /// 转换完成后发送浏览器通知
    pub notify_on_completion: bool,
    /// 启动后自动打开浏览器
    pub auto_open_browser: bool,
    /// 监听端口；0 = 自动挑一个空闲端口
    pub listen_port: i32,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            id: crate::models::new_id(),
            ffmpeg_path: String::new(),
            ffprobe_path: String::new(),
            // 由 SettingsService 在首次运行时填成系统「视频」目录
            default_output_directory: String::new(),
            output_naming: OutputNamingRule::Suffix,
            output_suffix: "_converted".to_string(),
            file_exists_action: FileExistsAction::Ask,
            prefer_hardware_acceleration: true,
            prefer_hardware_decoding: false,
            preferred_hardware_encoder: String::new(),
            auto_fallback_to_software: true,
            max_concurrent_tasks: 1,
            max_log_lines: 500,
            task_history_retention_days: 30,
            theme: "System".to_string(),
            global_ffmpeg_arguments: String::new(),
            notify_on_completion: true,
            auto_open_browser: true,
            listen_port: DEFAULT_LISTEN_PORT,
        }
    }
}

impl AppSettings {
    /// 规范化（幂等，原地修改）。
    ///
    /// 所有读取路径都会调用它：设置读取/保存、队列每次取设置、FFmpeg 定位。
    /// 这样数据库里即使存了越界值或用户手改过的值，运行时也一定是合法的。
    pub fn normalize(&mut self) {
        self.ffmpeg_path = normalize_path_field(&self.ffmpeg_path);
        self.ffprobe_path = normalize_path_field(&self.ffprobe_path);
        self.default_output_directory = self.default_output_directory.trim().to_string();
        self.preferred_hardware_encoder = self.preferred_hardware_encoder.trim().to_string();
        self.global_ffmpeg_arguments = self.global_ffmpeg_arguments.trim().to_string();

        self.output_suffix = {
            let trimmed = self.output_suffix.trim();
            if trimmed.is_empty() {
                "_converted".to_string()
            } else {
                trimmed.to_string()
            }
        };

        self.max_concurrent_tasks = clamp_or(self.max_concurrent_tasks, MAX_CONCURRENT_TASKS_RANGE, 1);
        self.max_log_lines = clamp_or(self.max_log_lines, MAX_LOG_LINES_RANGE, 500);
        self.task_history_retention_days =
            clamp_or(self.task_history_retention_days, HISTORY_RETENTION_DAYS_RANGE, 30);
        self.listen_port = clamp_or(self.listen_port, LISTEN_PORT_RANGE, DEFAULT_LISTEN_PORT);

        if !matches!(self.theme.as_str(), "Light" | "Dark" | "System") {
            self.theme = "System".to_string();
        }
    }
}

/// 路径字段：去首尾空白，并去掉用户从终端粘贴时带上的首尾引号
fn normalize_path_field(value: &str) -> String {
    let trimmed = value.trim();
    let unquoted = trimmed
        .strip_prefix('"')
        .and_then(|v| v.strip_suffix('"'))
        .unwrap_or(trimmed);
    unquoted.trim().to_string()
}

/// 越界就回退到默认值（原实现 `Normalize()` 的行为：不是夹到边界，而是整段回退）
fn clamp_or(value: i32, (min, max): (i32, i32), fallback: i32) -> i32 {
    if value < min || value > max {
        fallback
    } else {
        value
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_defaults_are_stable() {
        let mut s = AppSettings::default();
        let before = s.clone();
        s.normalize();
        // 默认值本身已经是规范化的，normalize 不应改变任何字段（除 id 外）
        assert_eq!(s.output_suffix, before.output_suffix);
        assert_eq!(s.max_concurrent_tasks, before.max_concurrent_tasks);
        assert_eq!(s.theme, before.theme);
    }

    #[test]
    fn normalize_fixes_bad_values() {
        let mut s = AppSettings {
            output_suffix: "   ".to_string(),
            max_concurrent_tasks: 99,
            max_log_lines: 1,
            theme: "neon".to_string(),
            ffmpeg_path: "  \"/usr/local/bin/ffmpeg\"  ".to_string(),
            task_history_retention_days: -1,
            ..AppSettings::default()
        };
        s.normalize();
        assert_eq!(s.output_suffix, "_converted");
        assert_eq!(s.max_concurrent_tasks, 1);
        assert_eq!(s.max_log_lines, 500);
        assert_eq!(s.theme, "System");
        assert_eq!(s.ffmpeg_path, "/usr/local/bin/ffmpeg");
        assert_eq!(s.task_history_retention_days, 30);
    }

    #[test]
    fn normalize_is_idempotent() {
        let mut s = AppSettings {
            max_concurrent_tasks: 8,
            theme: "Dark".to_string(),
            ..AppSettings::default()
        };
        s.normalize();
        let once = s.clone();
        s.normalize();
        assert_eq!(s, once);
    }
}
