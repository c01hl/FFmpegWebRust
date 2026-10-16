//! FFmpeg 能力探测 / 媒体探测 / 子进程执行与进度解析
//! （规格 `ffmpeg-core.md` §3，源 `Services/FFmpegService.cs`）。
//!
//! 三个关注点：
//!
//! 1. **能力缓存**：`FFmpegInfo` 缓存 30 分钟（规格 §3.2、§7）。`set_paths` 会清空它。
//! 2. **媒体探测**：`ffprobe -v error -print_format json -show_format -show_streams <file>`
//!    （参数顺序逐字，规格 §3.7），30 秒超时。
//! 3. **执行与进度**：`ffmpeg -hide_banner -y|-n -progress pipe:2 <args...>`，进度块以
//!    `progress=continue|end` 收尾时才结算（规格 §3.12），stderr 保留最近 200 行。

use std::collections::HashMap;
use std::process::ExitStatus;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio_util::sync::CancellationToken;

use crate::ffmpeg::locator::{self, FFmpegPaths};
use crate::ffmpeg::util::{self, BoundedLineBuffer};

/// `FFmpegInfo` 能力缓存 TTL（分钟，规格 §7）。
pub const INFO_CACHE_TTL_MINUTES: i64 = 30;
/// 抓取型命令超时（秒，规格 §7）。
pub const CAPTURE_TIMEOUT_SECS: u64 = 30;
/// 取消后等待优雅退出的秒数（规格 §7）。
pub const GRACEFUL_STOP_SECS: u64 = 5;
/// stderr 尾部缓冲行数（规格 §7）。
pub const STDERR_TAIL_MAX_LINES: usize = util::STDERR_TAIL_LINES;
/// 诊断信息截断长度（规格 §7）。
pub const DIAGNOSTIC_MAX_CHARS: usize = util::DIAGNOSTIC_MAX_CHARS;
/// 「文件已存在」判定特征串 1（逐字，规格 §3.10）。
pub const SKIPPED_MARKER_QUESTION: &str = "already exists. Overwrite?";
/// 「文件已存在」判定特征串 2（逐字，规格 §3.10）。
pub const SKIPPED_MARKER_EXITING: &str = "Not overwriting - exiting";
/// 「文件已存在」时追加到诊断信息前的中文提示（逐字，规格 §3.11）。
pub const SKIPPED_PREFIX: &str = "输出文件已存在，且当前策略为「不覆盖」。请启用覆盖或使用自动重命名。";

/// FFmpeg 子系统错误。
#[derive(Debug, thiserror::Error)]
pub enum FFmpegError {
    /// 找不到可用的 ffmpeg（提示文案来自定位器）
    #[error("未找到可用的 FFmpeg：{0}")]
    NotFound(String),
    /// 子进程启动失败
    #[error("FFmpeg 执行失败：{0}")]
    Spawn(String),
    /// ffprobe 探测失败（文件不存在 / 退出码非 0 / JSON 无法解析）
    #[error("FFprobe 解析失败：{0}")]
    Probe(String),
    /// IO 错误
    #[error("IO 错误：{0}")]
    Io(#[from] std::io::Error),
    /// 其它错误
    #[error("{0}")]
    Other(String),
}

// ────────────────────────────────────────────────────────────────────────────
// 能力模型
// ────────────────────────────────────────────────────────────────────────────

/// FFmpeg 版本与编译期能力清单（规格 §4.1）。
#[derive(Debug, Clone, serde::Serialize)]
pub struct FFmpegInfo {
    /// 是否可用：`version` 非空且 != `"Unknown"`
    pub available: bool,
    /// 解析出的 ffmpeg 路径（可能是裸名 `"ffmpeg"`）
    pub path: String,
    /// `ffmpeg version` 后首个 token（如 `9.0.2` / `n7.1.1` / `N-118527-g...`）
    pub version: String,
    /// 完整的 `ffmpeg version ...` 行（界面展示用）
    pub version_line: String,
    /// 从版本串里「第一个数字开始的最长数字串」
    pub major_version: i32,
    /// `-encoders` 编码器名
    pub encoders: Vec<String>,
    /// `-hwaccels` 硬件加速方法
    pub hwaccels: Vec<String>,
    /// `-formats` 容器（demux/mux 名称集合）
    pub formats: Vec<String>,
    /// `-filters` 滤镜名
    pub filters: Vec<String>,
    /// `configuration:` 行内容
    pub build_configuration: String,
    /// `-codecs` 编解码器名（C# `SupportedCodecs`；接口层按需使用）
    pub codecs: Vec<String>,
}

impl FFmpegInfo {
    /// 是否包含某个编码器（忽略大小写，对应 C# `SupportedEncoders.Contains`）。
    pub fn has_encoder(&self, name: &str) -> bool {
        self.encoders.iter().any(|e| e.eq_ignore_ascii_case(name.trim()))
    }

    /// 是否包含某个编码器（别名，语义同 [`Self::has_encoder`]）。
    pub fn has_encoder_ci(&self, name: &str) -> bool {
        self.has_encoder(name)
    }

    /// 是否包含某个滤镜（忽略大小写）。
    ///
    /// 注意：这里**不**套用 `HardwareReport::has_filter` 的「清单为空视为放行」规则
    /// （规格 §4.4 的那条规则只属于检测报告）。
    pub fn has_filter(&self, name: &str) -> bool {
        self.filters.iter().any(|f| f.eq_ignore_ascii_case(name.trim()))
    }

    /// 是否包含某个编解码器（忽略大小写）。
    pub fn has_codec(&self, name: &str) -> bool {
        self.codecs.iter().any(|c| c.eq_ignore_ascii_case(name.trim()))
    }

    /// 是否支持某个硬件加速方法（忽略大小写）。
    pub fn has_hwaccel(&self, name: &str) -> bool {
        self.hwaccels.iter().any(|h| h.eq_ignore_ascii_case(name.trim()))
    }
}

// ────────────────────────────────────────────────────────────────────────────
// 媒体模型
// ────────────────────────────────────────────────────────────────────────────

/// 单条流的信息（规格 §4.2）。
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct MediaStream {
    /// 流序号
    pub index: i32,
    /// `video` / `audio` / `subtitle` / ...
    pub codec_type: String,
    pub codec_name: String,
    pub codec_long_name: String,
    pub width: Option<i32>,
    pub height: Option<i32>,
    /// `r_frame_rate` 分式解析结果（如 `30000/1001` → 29.97）
    pub frame_rate: Option<f64>,
    pub bit_rate: Option<i64>,
    pub sample_rate: Option<i32>,
    pub channels: Option<i32>,
    pub pixel_format: Option<String>,
    pub duration: Option<f64>,
    /// 旋转角，已归一化到 0/90/180/270（规格 §3.8 `GetRotation`）
    pub rotation: i32,
}

impl MediaStream {
    /// `1920x1080`，无分辨率时为 `-`。
    pub fn resolution_display(&self) -> String {
        match (self.width, self.height) {
            (Some(width), Some(height)) if width > 0 && height > 0 => format!("{width}x{height}"),
            _ => "-".to_string(),
        }
    }

    /// 一行中文摘要（界面/日志用）。
    pub fn summary(&self) -> String {
        let codec =
            if self.codec_name.is_empty() { "未知编码".to_string() } else { self.codec_name.clone() };
        match self.codec_type.as_str() {
            "video" => {
                let mut parts = vec![format!("视频 {codec}"), self.resolution_display()];
                if let Some(fps) = self.frame_rate {
                    if fps > 0.0 {
                        parts.push(format!("{fps:.2}fps"));
                    }
                }
                if self.rotation != 0 {
                    parts.push(format!("旋转 {}°", self.rotation));
                }
                parts.join(" ")
            }
            "audio" => {
                let mut parts = vec![format!("音频 {codec}")];
                if let Some(rate) = self.sample_rate {
                    parts.push(format!("{rate}Hz"));
                }
                if let Some(channels) = self.channels {
                    parts.push(format!("{channels}声道"));
                }
                parts.join(" ")
            }
            "subtitle" => format!("字幕 {codec}"),
            "data" => format!("数据 {codec}"),
            "attachment" => format!("附件 {codec}"),
            other => format!("{other} {codec}"),
        }
    }

    /// 是否是视频流。
    pub fn is_video(&self) -> bool {
        self.codec_type == "video"
    }

    /// 是否是音频流。
    pub fn is_audio(&self) -> bool {
        self.codec_type == "audio"
    }
}

/// 媒体文件信息（规格 §4.3）。
#[derive(Debug, Clone, serde::Serialize)]
pub struct MediaInfo {
    pub path: String,
    pub file_name: String,
    /// 文件系统里的真实字节数（不用 ffprobe 的 `size`）
    pub size_bytes: u64,
    /// 秒；优先 `format.duration`，缺失时回退首条视频/音频流的时长
    pub duration_seconds: f64,
    /// 原值，如 `mov,mp4,m4a,3gp,3g2,mj2`
    pub format_name: String,
    pub format_long_name: String,
    /// 容器总码率（bit/s），缺失为 0
    pub bit_rate: i64,
    /// 首条视频流
    pub video: Option<MediaStream>,
    /// 首条音频流
    pub audio: Option<MediaStream>,
    /// 全部流（保序）
    pub streams: Vec<MediaStream>,
}

impl MediaInfo {
    /// 容器显示名（规格 §3.8 `FormatDisplayName`）。
    pub fn format_display(&self) -> String {
        let first = self.format_name.split(',').next().unwrap_or("").trim();
        match first.to_ascii_lowercase().as_str() {
            "mov" | "mp4" | "m4a" | "3gp" | "3g2" | "mj2" => "MP4 / MOV".to_string(),
            "matroska" | "webm" => {
                if self.format_name.to_ascii_lowercase().contains("webm") {
                    "WebM".to_string()
                } else {
                    "Matroska (MKV)".to_string()
                }
            }
            "avi" => "AVI".to_string(),
            "flv" => "FLV".to_string(),
            "asf" => "ASF / WMV".to_string(),
            "mpeg" => "MPEG".to_string(),
            "mpegts" => "MPEG-TS".to_string(),
            "ogg" => "OGG".to_string(),
            "wav" => "WAV".to_string(),
            "flac" => "FLAC".to_string(),
            "mp3" => "MP3".to_string(),
            "aac" => "AAC".to_string(),
            "image2" => "图片序列".to_string(),
            "gif" => "GIF".to_string(),
            _ => first.to_uppercase(),
        }
    }

    /// 时长显示（规格 §3.8 `DurationDisplay`）：<=0 → `-`；≥1h 用 `h:mm:ss`，否则 `m:ss`。
    pub fn duration_display(&self) -> String {
        if self.duration_seconds <= 0.0 {
            return "-".to_string();
        }
        let total = self.duration_seconds.floor() as u64;
        let hours = total / 3600;
        let minutes = (total % 3600) / 60;
        let seconds = total % 60;
        if self.duration_seconds >= 3600.0 {
            format!("{hours}:{minutes:02}:{seconds:02}")
        } else {
            format!("{minutes}:{seconds:02}")
        }
    }

    /// 是否有视频流。
    pub fn has_video(&self) -> bool {
        self.video.is_some()
    }

    /// 是否有音频流。
    pub fn has_audio(&self) -> bool {
        self.audio.is_some()
    }

    /// 字幕流数量。
    pub fn subtitle_stream_count(&self) -> usize {
        self.streams.iter().filter(|s| s.codec_type == "subtitle").count()
    }

    /// 首条视频流的旋转角。
    pub fn rotation(&self) -> i32 {
        self.video.as_ref().map(|v| v.rotation).unwrap_or(0)
    }
}

// ────────────────────────────────────────────────────────────────────────────
// 进度模型
// ────────────────────────────────────────────────────────────────────────────

/// 一次进度结算的结果（规格 §3.12 `ConversionProgress` 的 Rust 形态）。
#[derive(Debug, Clone, serde::Serialize)]
pub struct ProgressTick {
    pub frame: Option<u64>,
    pub fps: Option<f64>,
    /// 形如 `1234.5kbits/s`，原样保留
    pub bitrate: Option<String>,
    pub total_size: Option<u64>,
    /// 已处理到的秒数（`progress=end` 时兜底为总时长）
    pub out_time_seconds: f64,
    /// `speed=1.23x` 里的 1.23；`N/A` 或缺失为 `None`
    pub speed: Option<f64>,
    /// 已知总时长时为 `out_time / duration * 100`（钳制到 0..=100），未知为 `-1`
    pub percent: f64,
    /// `"continue"` / `"end"`（原样）
    pub progress: Option<String>,
}

impl ProgressTick {
    /// 预计剩余秒数（规格 §3.12：`speed > 0.01` 且总时长已知才计算）。
    pub fn eta_seconds(&self, duration: f64) -> Option<f64> {
        let speed = self.speed?;
        if speed > 0.01 && duration > 0.0 {
            let remaining = duration - self.out_time_seconds;
            if remaining > 0.0 {
                return Some(remaining / speed);
            }
        }
        None
    }
}

/// 执行过程中的事件（推给调用方，用于日志与进度条）。
#[derive(Debug, Clone)]
pub enum RunEvent {
    /// 进程已启动
    Started { pid: Option<u32>, command_line: String },
    /// 一行 stderr（不含进度块内部的行）
    Stderr(String),
    /// 一次进度结算
    Progress(ProgressTick),
    /// 进程结束
    Finished {
        exit_code: Option<i32>,
        stderr_tail: String,
        elapsed_ms: u64,
        cancelled: bool,
        timed_out: bool,
    },
}

/// 一次 ffmpeg 执行的请求（规格 §3.1 `FFmpegRunRequest`）。
///
/// `args` **不含**可执行文件本身，也**不含**服务自动追加的
/// `-hide_banner -y|-n -progress pipe:2`。
#[derive(Debug, Clone)]
pub struct RunRequest {
    /// ffmpeg 参数
    pub args: Vec<String>,
    /// 总时长（秒），用于计算百分比；0 表示未知
    pub duration_hint: f64,
    /// true → `-y`；false → `-n`
    pub overwrite: bool,
    /// 超时秒数；`None` = 不限制
    pub timeout_seconds: Option<u64>,
    /// 进程登记键；传入后可通过 [`FFmpegService::stop_task`] 停止
    pub task_key: Option<String>,
}

impl RunRequest {
    /// 构造请求（默认覆盖写入、无超时、不登记）。
    pub fn new(args: Vec<String>) -> Self {
        Self { args, duration_hint: 0.0, overwrite: true, timeout_seconds: None, task_key: None }
    }

    /// 设置总时长（秒），用于百分比与 ETA。
    pub fn with_duration_hint(mut self, duration_hint: f64) -> Self {
        self.duration_hint = duration_hint;
        self
    }

    /// 设置覆盖策略。
    pub fn with_overwrite(mut self, overwrite: bool) -> Self {
        self.overwrite = overwrite;
        self
    }

    /// 设置超时。
    pub fn with_timeout(mut self, timeout_seconds: Option<u64>) -> Self {
        self.timeout_seconds = timeout_seconds;
        self
    }

    /// 设置进程登记键。
    pub fn with_task_key(mut self, task_key: impl Into<String>) -> Self {
        self.task_key = Some(task_key.into());
        self
    }
}

/// 一次执行的最终结果（规格 §3.1 `FFmpegRunResult`）。
#[derive(Debug, Clone)]
pub struct RunOutcome {
    /// 真实退出码；进程被信号杀死时为 `None`（C# 记为 -1）
    pub exit_code: Option<i32>,
    pub success: bool,
    pub cancelled: bool,
    pub timed_out: bool,
    pub elapsed_ms: u64,
    /// 诊断信息：stderr 尾部 ≤200 行拼接后截断到 8000 字符（保留尾部）
    pub stderr_tail: String,
    /// 尾部最多 200 行原始日志
    pub log_lines: Vec<String>,
}

// ────────────────────────────────────────────────────────────────────────────
// 抓取型命令
// ────────────────────────────────────────────────────────────────────────────

/// 一条「跑完抓全部输出」的命令结果（规格 §3.2 `CaptureResult`）。
#[derive(Debug, Clone, Default)]
struct CaptureResult {
    exit_code: Option<i32>,
    stdout: String,
    stderr: String,
}

/// 执行一条命令并抓取全部输出（对应 C# `RunCaptureAsync`，30 秒超时）。
///
/// * stdout 为空而 stderr 非空时，把 stderr 当 stdout（兼容把清单打到 stderr 的构建）。
/// * 超时：杀进程树并返回 stderr = `"执行超时"`（与 C# 逐字一致）。
/// * stdin 保持管道但立即关闭：C# 不重定向 stdin，这里避免半开管道让 ffmpeg 等待输入。
async fn run_capture(program: &str, args: &[String], timeout: Duration) -> CaptureResult {
    let mut cmd = util::base_command(program);
    cmd.args(args);
    let mut child = match cmd.spawn() {
        Ok(child) => child,
        Err(err) => {
            return CaptureResult { exit_code: None, stdout: String::new(), stderr: err.to_string() };
        }
    };
    let pid = child.id();
    drop(child.stdin.take()); // 关闭 stdin（子进程读 stdin 会立刻 EOF）

    let stdout_pipe = child.stdout.take();
    let stderr_pipe = child.stderr.take();
    let stdout_task = tokio::spawn(async move { read_all(stdout_pipe).await });
    let stderr_task = tokio::spawn(async move { read_all(stderr_pipe).await });

    match tokio::time::timeout(timeout, child.wait()).await {
        Ok(Ok(status)) => {
            let stdout = join_reader(stdout_task).await;
            let stderr = join_reader(stderr_task).await;
            let stdout = if stdout.trim().is_empty() && !stderr.trim().is_empty() {
                stderr.clone()
            } else {
                stdout
            };
            CaptureResult { exit_code: status.code(), stdout, stderr }
        }
        Ok(Err(err)) => {
            let stdout = join_reader(stdout_task).await;
            CaptureResult { exit_code: None, stdout, stderr: err.to_string() }
        }
        Err(_elapsed) => {
            util::kill_process_tree(pid, &mut child).await;
            // 读取任务会被丢弃（detach）；管道随进程销毁而关闭。
            CaptureResult { exit_code: None, stdout: String::new(), stderr: "执行超时".to_string() }
        }
    }
}

async fn read_all<R: tokio::io::AsyncRead + Unpin>(pipe: Option<R>) -> String {
    use tokio::io::AsyncReadExt;
    let mut buffer = String::new();
    if let Some(mut pipe) = pipe {
        let _ = pipe.read_to_string(&mut buffer).await;
    }
    buffer
}

async fn join_reader(task: tokio::task::JoinHandle<String>) -> String {
    match tokio::time::timeout(util::READER_JOIN_WAIT, task).await {
        Ok(Ok(text)) => text,
        _ => String::new(),
    }
}

// ────────────────────────────────────────────────────────────────────────────
// 版本 / 清单解析
// ────────────────────────────────────────────────────────────────────────────

/// `-version` 输出的解析结果（规格 §3.3 `ParseVersionOutput`）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ParsedVersion {
    pub version: String,
    pub version_line: String,
    pub major_version: i32,
    pub configuration: String,
}

/// 解析 `<ffmpeg> -hide_banner -version` 的输出（逐行、Trim、大小写不敏感）。
pub fn parse_version_output(output: &str) -> ParsedVersion {
    let mut parsed = ParsedVersion::default();
    for raw_line in output.split('\n') {
        let line = raw_line.trim();
        if line.is_empty() {
            continue;
        }
        if starts_with_ignore_case(line, "ffmpeg version") {
            parsed.version_line = line.to_string();
            let rest = strip_prefix_ignore_case(line, "ffmpeg version").unwrap_or("");
            let token = rest.split_whitespace().next().unwrap_or("Unknown");
            parsed.version = token.to_string();
            parsed.major_version = leading_digit_run(token);
            continue;
        }
        if let Some(rest) = strip_prefix_ignore_case(line, "configuration:") {
            parsed.configuration = rest.to_string();
        }
    }
    parsed
}

/// 从「第一个数字」开始取最长数字串（`n7.1.1` → 7，`N-118527-g...` → 118527，无数字 → 0）。
fn leading_digit_run(token: &str) -> i32 {
    match token.find(|c: char| c.is_ascii_digit()) {
        Some(index) => {
            let digits: String = token[index..].chars().take_while(|c| c.is_ascii_digit()).collect();
            digits.parse::<i32>().unwrap_or(0) // 溢出/为空都按 C# TryParse 语义退化为 0
        }
        None => 0,
    }
}

fn starts_with_ignore_case(line: &str, prefix: &str) -> bool {
    line.get(..prefix.len()).map(|head| head.eq_ignore_ascii_case(prefix)).unwrap_or(false)
}

fn strip_prefix_ignore_case<'a>(line: &'a str, prefix: &str) -> Option<&'a str> {
    if starts_with_ignore_case(line, prefix) {
        line.get(prefix.len()..).map(str::trim)
    } else {
        None
    }
}

/// 解析 `-formats`(flags=3) / `-codecs`(flags=6) / `-encoders`(flags=6) 的表格
/// （规格 §3.4 `ParseListingTable`，保序去重）。
///
/// PORT NOTE：这里保留了参考实现的**启发式**（首字符大写 + 标志段以空格结束且长度 ≤ flags +
/// 标志首位 ∈ `V A S D E T`），不做列宽解析——规格 §8.11 明确要求保持一致。
pub fn parse_listing_table(output: &str, flags: usize) -> Vec<String> {
    let mut result: Vec<String> = Vec::new();
    for raw_line in output.split('\n') {
        let line = raw_line.trim_end_matches('\r');
        if line.len() < 2 {
            continue;
        }
        let trimmed = line.trim_start();
        if trimmed.is_empty() {
            continue;
        }
        let first = trimmed.chars().next().unwrap_or(' ');
        if !first.is_ascii_uppercase() {
            continue;
        }
        if trimmed.starts_with("Encoders:")
            || trimmed.starts_with("Decoders:")
            || trimmed.starts_with("Codecs:")
            || trimmed.starts_with("File formats:")
        {
            continue;
        }
        if trimmed.starts_with("---") {
            continue;
        }
        let flag_end = match trimmed.find(' ') {
            Some(index) if index > 0 => index,
            _ => continue,
        };
        if flag_end > flags {
            continue;
        }
        let flag_first = trimmed.chars().next().unwrap_or(' ');
        if !matches!(flag_first, 'V' | 'A' | 'S' | 'D' | 'E' | 'T') {
            continue;
        }
        let remainder = trimmed[flag_end + 1..].trim_start();
        if remainder.is_empty() {
            continue;
        }
        let name = match remainder.find(' ') {
            Some(index) => remainder[..index].trim(),
            None => remainder.trim(),
        };
        if name.is_empty() || name.starts_with('=') {
            continue;
        }
        if !name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | ',' | '.' | '@'))
        {
            continue;
        }
        if !result.iter().any(|existing| existing == name) {
            result.push(name.to_string());
        }
    }
    result
}

/// 解析 `-hwaccels`（规格 §3.5）。
pub fn parse_hwaccels(output: &str) -> Vec<String> {
    let mut result: Vec<String> = Vec::new();
    for raw_line in output.split('\n') {
        let line = raw_line.trim();
        if line.is_empty() {
            continue;
        }
        if starts_with_ignore_case(line, "Hardware acceleration") {
            continue;
        }
        if !line.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-')) {
            continue;
        }
        if !result.iter().any(|existing| existing == line) {
            result.push(line.to_string());
        }
    }
    result
}

/// 解析 `-filters`（规格 §3.6 `ParseFilters` + `IsFilterSignature`）。
pub fn parse_filters(output: &str) -> Vec<String> {
    let mut result: Vec<String> = Vec::new();
    for raw_line in output.split('\n') {
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with("Filters:") || line.starts_with("---") {
            continue;
        }
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() < 3 {
            continue;
        }
        let name = parts[1];
        if !name.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.')) {
            continue;
        }
        if parts[2..].iter().take(2).any(|token| is_filter_signature(token)) {
            if !result.iter().any(|existing| existing == name) {
                result.push(name.to_string());
            }
        }
    }
    result
}

/// `->` 不在首位/末两位且整体长度 ≤ 8（如 `AA->A`、`V->V`、`V->N`）。
fn is_filter_signature(token: &str) -> bool {
    match token.find("->") {
        Some(arrow) => arrow > 0 && arrow + 2 < token.len() && token.len() <= 8,
        None => false,
    }
}

// ────────────────────────────────────────────────────────────────────────────
// ffprobe JSON 解析
// ────────────────────────────────────────────────────────────────────────────

/// 解析 ffprobe 的 JSON 输出（规格 §3.8 `ParseMediaInfo`）。
///
/// 容错规则：每个标量都接受 JSON `String` 或 `Number`；缺字段/类型不符/解析失败一律退化为
/// `None`（C# 里是 `null` / `0`）。
pub fn parse_media_info(file_path: &str, json: &str) -> Option<MediaInfo> {
    let root: serde_json::Value = serde_json::from_str(json).ok()?;

    let mut streams: Vec<MediaStream> = Vec::new();
    if let Some(array) = root.get("streams").and_then(|value| value.as_array()) {
        for element in array {
            streams.push(MediaStream {
                index: get_i32(element, "index").unwrap_or(0),
                codec_type: get_string(element, "codec_type")
                    .unwrap_or_else(|| "unknown".to_string()),
                codec_name: get_string(element, "codec_name").unwrap_or_default(),
                codec_long_name: get_string(element, "codec_long_name").unwrap_or_default(),
                width: positive_i32(get_i32(element, "width")),
                height: positive_i32(get_i32(element, "height")),
                frame_rate: parse_fraction(get_string(element, "r_frame_rate").as_deref()),
                bit_rate: positive_i64(get_i64(element, "bit_rate")),
                sample_rate: positive_i32(get_i32(element, "sample_rate")),
                channels: positive_i32(get_i32(element, "channels")),
                pixel_format: get_string(element, "pix_fmt"),
                duration: positive_f64(get_f64(element, "duration")),
                rotation: get_rotation(element),
            });
        }
    }

    let video = streams.iter().find(|s| s.codec_type == "video").cloned();
    let audio = streams.iter().find(|s| s.codec_type == "audio").cloned();

    let format = root.get("format");
    let mut duration = format.and_then(|f| get_f64(f, "duration")).unwrap_or(0.0);
    let bit_rate = format.and_then(|f| get_i64(f, "bit_rate")).unwrap_or(0);
    let format_name =
        format.and_then(|f| get_string(f, "format_name")).unwrap_or_else(|| "unknown".to_string());
    let format_long_name = format.and_then(|f| get_string(f, "format_long_name")).unwrap_or_default();
    if duration <= 0.0 {
        duration = video
            .as_ref()
            .and_then(|v| v.duration)
            .or_else(|| audio.as_ref().and_then(|a| a.duration))
            .unwrap_or(0.0);
    }

    let size_bytes = std::fs::metadata(file_path).map(|meta| meta.len()).unwrap_or(0);
    let file_name = std::path::Path::new(file_path)
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();

    Some(MediaInfo {
        path: file_path.to_string(),
        file_name,
        size_bytes,
        duration_seconds: duration,
        format_name,
        format_long_name,
        bit_rate,
        video,
        audio,
        streams,
    })
}

fn get_string(element: &serde_json::Value, name: &str) -> Option<String> {
    match element.get(name)? {
        serde_json::Value::String(text) => Some(text.clone()),
        serde_json::Value::Number(number) => Some(number.to_string()),
        _ => None,
    }
}

fn get_i32(element: &serde_json::Value, name: &str) -> Option<i32> {
    match element.get(name)? {
        serde_json::Value::Number(number) => number.as_i64().and_then(|v| i32::try_from(v).ok()),
        serde_json::Value::String(text) => text.trim().parse::<i32>().ok(),
        _ => None,
    }
}

fn get_i64(element: &serde_json::Value, name: &str) -> Option<i64> {
    match element.get(name)? {
        serde_json::Value::Number(number) => number.as_i64().or_else(|| number.as_f64().map(|f| f as i64)),
        serde_json::Value::String(text) => text.trim().parse::<i64>().ok(),
        _ => None,
    }
}

fn get_f64(element: &serde_json::Value, name: &str) -> Option<f64> {
    match element.get(name)? {
        serde_json::Value::Number(number) => number.as_f64(),
        serde_json::Value::String(text) => text.trim().parse::<f64>().ok(),
        _ => None,
    }
}

/// 解析 `"25/1"` / `"30000/1001"` 形式的分式帧率（规格 §3.8 `ParseFraction`）。
///
/// 分母为 0 时返回 `None`（C# 返回 0；Rust 侧用 `Option` 表达「无有效帧率」）。
pub fn parse_fraction(text: Option<&str>) -> Option<f64> {
    let text = text?;
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return None;
    }
    let parts: Vec<&str> = trimmed.split('/').collect();
    if parts.len() == 2 {
        if let (Ok(numerator), Ok(denominator)) =
            (parts[0].trim().parse::<f64>(), parts[1].trim().parse::<f64>())
        {
            if denominator != 0.0 {
                return Some(numerator / denominator);
            }
            return None;
        }
    }
    trimmed.parse::<f64>().ok()
}

/// `side_data_list[].rotation`，遇到第一项含 `rotation` 即返回并归一化到 `0..360`（规格 §3.8）。
fn get_rotation(stream: &serde_json::Value) -> i32 {
    let Some(items) = stream.get("side_data_list").and_then(|value| value.as_array()) else {
        return 0;
    };
    for item in items {
        if let Some(rotation) = item.get("rotation") {
            let value = match rotation {
                serde_json::Value::Number(number) => number.as_i64().unwrap_or(0) as i32,
                serde_json::Value::String(text) => text.trim().parse::<i32>().unwrap_or(0),
                _ => 0,
            };
            return ((value % 360) + 360) % 360;
        }
    }
    0
}

fn positive_i32(value: Option<i32>) -> Option<i32> {
    value.filter(|v| *v > 0)
}

fn positive_i64(value: Option<i64>) -> Option<i64> {
    value.filter(|v| *v > 0)
}

fn positive_f64(value: Option<f64>) -> Option<f64> {
    value.filter(|v| *v > 0.0)
}

// ────────────────────────────────────────────────────────────────────────────
// 进度解析（-progress pipe:2）
// ────────────────────────────────────────────────────────────────────────────

/// `-progress` 已知 key 白名单（11 个，规格 §3.12；Ordinal 精确匹配）。
const PROGRESS_KEYS: &[&str] = &[
    "frame",
    "fps",
    "bitrate",
    "total_size",
    "out_time_us",
    "out_time_ms",
    "out_time",
    "dup_frames",
    "drop_frames",
    "speed",
    "progress",
];

/// 进度块累积器（规格 §3.12 `ProgressAccumulator`）。
#[derive(Debug, Default)]
struct ProgressAccumulator {
    pending: HashMap<String, String>,
}

impl ProgressAccumulator {
    /// 尝试消费一行；返回 `Some(tick)` 表示这一行收尾了一个进度块。
    fn try_consume(&mut self, line: &str, total_duration: f64) -> Option<ProgressTick> {
        if line.trim().is_empty() {
            return None;
        }
        let equals = line.find('=')?;
        if equals == 0 {
            return None;
        }
        let key = line[..equals].trim();
        let value = line[equals + 1..].trim();
        if !PROGRESS_KEYS.iter().any(|known| *known == key) {
            return None;
        }
        if key != "progress" {
            self.pending.insert(key.to_string(), value.to_string());
            return None;
        }

        let mut current_time = self.read_seconds();
        let speed = self.read_speed();
        let frame = self.pending.get("frame").and_then(|v| v.trim().parse::<u64>().ok());
        let fps = self.pending.get("fps").and_then(|v| parse_optional_number(v));
        let bitrate = self.pending.get("bitrate").map(|v| v.trim().to_string()).filter(|v| !v.is_empty());
        let total_size = self.pending.get("total_size").and_then(|v| v.trim().parse::<u64>().ok());
        // 已知 key 但未被消费的：dup_frames / drop_frames（规格 §3.12：吸收但不暴露）
        self.pending.clear();

        let is_end = value.eq_ignore_ascii_case("end");
        // 块收尾时用总时长兜底，避免最后一帧差一点点到不了 100%
        if is_end && total_duration > 0.0 {
            current_time = total_duration;
        }

        let percent = if total_duration > 0.0 {
            (current_time / total_duration * 100.0).clamp(0.0, 100.0)
        } else {
            -1.0
        };

        Some(ProgressTick {
            frame,
            fps,
            bitrate,
            total_size,
            out_time_seconds: current_time,
            speed,
            percent,
            progress: Some(value.to_string()),
        })
    }

    /// 已处理秒数：`out_time_us` → `out_time_ms`（**同样除以 1e6**）→ `out_time`（规格 §3.12）。
    ///
    /// PORT NOTE：`out_time_ms` 在 ffmpeg 里实际写的是**微秒**，参考实现刻意除以 1e6
    /// （规格 §8.1 明确要求保留）。若改成除以 1e3，进度会瞬间跑满。
    fn read_seconds(&self) -> f64 {
        if let Some(micros) = self.pending.get("out_time_us") {
            if let Ok(value) = micros.trim().parse::<i64>() {
                if value > 0 {
                    return value as f64 / 1_000_000.0;
                }
            }
        }
        if let Some(millis) = self.pending.get("out_time_ms") {
            if let Ok(value) = millis.trim().parse::<i64>() {
                if value > 0 {
                    return value as f64 / 1_000_000.0;
                }
            }
        }
        if let Some(text) = self.pending.get("out_time") {
            if let Some(seconds) = parse_timecode(text) {
                return seconds;
            }
        }
        0.0
    }

    fn read_speed(&self) -> Option<f64> {
        let raw = self.pending.get("speed")?;
        let cleaned = raw.trim().trim_end_matches(['x', 'X']);
        if cleaned.eq_ignore_ascii_case("N/A") {
            return None;
        }
        cleaned.parse::<f64>().ok()
    }
}

/// 解析 `HH:MM:SS.ffffff`（对应 C# 的 `TimeSpan.TryParse`；负数带 `-` 也接受）。
fn parse_timecode(text: &str) -> Option<f64> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return None;
    }
    let (negative, body) = match trimmed.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, trimmed),
    };
    let mut parts = body.split(':');
    let hours: f64 = parts.next()?.trim().parse().ok()?;
    let minutes: f64 = parts.next()?.trim().parse().ok()?;
    let seconds: f64 = parts.next()?.trim().parse().ok()?;
    if parts.next().is_some() {
        return None;
    }
    let total = hours * 3600.0 + minutes * 60.0 + seconds;
    Some(if negative { -total } else { total })
}

fn parse_optional_number(text: &str) -> Option<f64> {
    let trimmed = text.trim();
    if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("N/A") {
        return None;
    }
    trimmed.parse::<f64>().ok()
}

// ────────────────────────────────────────────────────────────────────────────
// 服务
// ────────────────────────────────────────────────────────────────────────────

/// 能力缓存条目。
struct CachedInfo {
    info: FFmpegInfo,
    cached_at: DateTime<Utc>,
}

/// 运行中任务的登记句柄。
#[derive(Clone)]
struct TaskHandle {
    /// 子进程 stdin（优雅停止 = 写入 `q`）
    stdin: Arc<tokio::sync::Mutex<Option<tokio::process::ChildStdin>>>,
    /// 触发后立刻杀进程树
    force_kill: CancellationToken,
    /// `run()` 结束时取消，避免优雅停止看门狗误杀已被复用的 pid
    finished: CancellationToken,
}

struct ServiceInner {
    paths: RwLock<FFmpegPaths>,
    info: Mutex<Option<CachedInfo>>,
    running: Mutex<HashMap<String, TaskHandle>>,
}

/// FFmpeg 执行器（对应 C# `FFmpegService`，Singleton）。
///
/// `Clone` 共享同一份 `Arc` 内部状态：能力缓存与进程登记表都是进程级的。
#[derive(Clone)]
pub struct FFmpegService {
    inner: Arc<ServiceInner>,
}

impl Default for FFmpegService {
    fn default() -> Self {
        Self::new()
    }
}

impl FFmpegService {
    /// 用内部的默认路径解析（不读设置，等价于 `resolve(None, None)`）。
    pub fn new() -> Self {
        Self::new_with_paths(locator::resolve(None, None))
    }

    /// 用指定的路径解析结果构造。
    pub fn new_with_paths(paths: FFmpegPaths) -> Self {
        Self {
            inner: Arc::new(ServiceInner {
                paths: RwLock::new(paths),
                info: Mutex::new(None),
                running: Mutex::new(HashMap::new()),
            }),
        }
    }

    /// 从设置里读取 `ffmpeg_path` / `ffprobe_path` 构造（空串 = 交给自动探测）。
    pub fn from_settings(settings: &crate::models::AppSettings) -> Self {
        Self::new_with_paths(locator::resolve(
            Some(settings.ffmpeg_path.as_str()),
            Some(settings.ffprobe_path.as_str()),
        ))
    }

    /// 设置变更后重新解析路径（并清空能力缓存）。
    ///
    /// 对应参考实现里「设置保存成功 → `Locator.Invalidate()` + 能力缓存失效」的同两个事件。
    pub fn apply_settings(&self, settings: &crate::models::AppSettings) {
        self.set_paths(locator::resolve(
            Some(settings.ffmpeg_path.as_str()),
            Some(settings.ffprobe_path.as_str()),
        ));
    }

    /// 当前解析出的路径。
    pub fn paths(&self) -> FFmpegPaths {
        match self.inner.paths.read() {
            Ok(guard) => guard.clone(),
            // 读锁中毒：返回值语义上等价于「未找到」，不会 panic
            Err(poisoned) => poisoned.into_inner().clone(),
        }
    }

    /// 替换路径解析结果，并清空能力缓存（对应「设置写入成功」的副作用）。
    pub fn set_paths(&self, paths: FFmpegPaths) {
        match self.inner.paths.write() {
            Ok(mut guard) => *guard = paths,
            Err(poisoned) => *poisoned.into_inner() = paths,
        }
        self.invalidate_capabilities();
    }

    /// 清空能力缓存。
    pub fn invalidate_capabilities(&self) {
        match self.inner.info.lock() {
            Ok(mut guard) => *guard = None,
            Err(poisoned) => *poisoned.into_inner() = None,
        }
    }

    /// 当前运行中的任务数。
    pub fn running_count(&self) -> usize {
        match self.inner.running.lock() {
            Ok(guard) => guard.len(),
            Err(poisoned) => poisoned.into_inner().len(),
        }
    }

    /// 版本与能力探测（规格 §3.2）。缓存 30 分钟；`force_refresh` 绕过缓存。
    ///
    /// 返回 `None` 表示 `-version` 失败或输出为空 —— 上层视为「FFmpeg 不可用」。
    pub async fn info(&self, force_refresh: bool) -> Option<FFmpegInfo> {
        let paths = self.paths();

        if !force_refresh {
            if let Ok(guard) = self.inner.info.lock() {
                if let Some(cached) = guard.as_ref() {
                    if (Utc::now() - cached.cached_at).num_minutes() < INFO_CACHE_TTL_MINUTES {
                        return Some(cached.info.clone());
                    }
                }
            }
        }

        let version_args = vec!["-hide_banner".to_string(), "-version".to_string()];
        let version = run_capture(&paths.ffmpeg, &version_args, util::CAPTURE_TIMEOUT).await;
        if version.exit_code != Some(0) || version.stdout.trim().is_empty() {
            return None;
        }
        let parsed = parse_version_output(&version.stdout);

        // 5 条清单命令并行发出（规格 §3.2）；不检查各自退出码，解析失败退化为空列表。
        let formats_args = vec!["-hide_banner".to_string(), "-formats".to_string()];
        let codecs_args = vec!["-hide_banner".to_string(), "-codecs".to_string()];
        let encoders_args = vec!["-hide_banner".to_string(), "-encoders".to_string()];
        let hwaccels_args = vec!["-hide_banner".to_string(), "-hwaccels".to_string()];
        let filters_args = vec!["-hide_banner".to_string(), "-filters".to_string()];
        let (formats, codecs, encoders, hwaccels, filters) = tokio::join!(
            run_capture(&paths.ffmpeg, &formats_args, util::CAPTURE_TIMEOUT),
            run_capture(&paths.ffmpeg, &codecs_args, util::CAPTURE_TIMEOUT),
            run_capture(&paths.ffmpeg, &encoders_args, util::CAPTURE_TIMEOUT),
            run_capture(&paths.ffmpeg, &hwaccels_args, util::CAPTURE_TIMEOUT),
            run_capture(&paths.ffmpeg, &filters_args, util::CAPTURE_TIMEOUT),
        );

        let info = FFmpegInfo {
            available: !parsed.version.is_empty() && parsed.version != "Unknown",
            path: paths.ffmpeg.clone(),
            version: parsed.version,
            version_line: parsed.version_line,
            major_version: parsed.major_version,
            encoders: parse_listing_table(&encoders.stdout, 6),
            hwaccels: parse_hwaccels(&hwaccels.stdout),
            formats: parse_listing_table(&formats.stdout, 3),
            filters: parse_filters(&filters.stdout),
            build_configuration: parsed.configuration,
            codecs: parse_listing_table(&codecs.stdout, 6),
        };

        if let Ok(mut guard) = self.inner.info.lock() {
            *guard = Some(CachedInfo { info: info.clone(), cached_at: Utc::now() });
        }
        Some(info)
    }

    /// ffprobe 媒体探测（规格 §3.7）。
    ///
    /// 文件不存在、退出码非 0、stdout 为空、JSON 解析失败都返回 `Err(FFmpegError::Probe)`。
    pub async fn probe_media(&self, path: &str) -> Result<MediaInfo, FFmpegError> {
        let trimmed = path.trim();
        if trimmed.is_empty() {
            return Err(FFmpegError::Probe("文件路径为空".to_string()));
        }
        if !tokio::fs::metadata(trimmed).await.map(|meta| meta.is_file()).unwrap_or(false) {
            return Err(FFmpegError::Probe(format!("文件不存在或不是普通文件：{trimmed}")));
        }

        let paths = self.paths();
        let args: Vec<String> = vec![
            "-v".to_string(),
            "error".to_string(),
            "-print_format".to_string(),
            "json".to_string(),
            "-show_format".to_string(),
            "-show_streams".to_string(),
            trimmed.to_string(),
        ];
        let result = run_capture(&paths.ffprobe, &args, util::CAPTURE_TIMEOUT).await;
        if result.exit_code != Some(0) || result.stdout.trim().is_empty() {
            let detail = if result.stderr.trim().is_empty() {
                "ffprobe 未返回任何信息".to_string()
            } else {
                result.stderr.trim().to_string()
            };
            return Err(FFmpegError::Probe(detail));
        }
        parse_media_info(trimmed, &result.stdout)
            .ok_or_else(|| FFmpegError::Probe("ffprobe 输出无法解析为 JSON".to_string()))
    }

    /// 运行一次 ffmpeg（规格 §3.9-§3.13）。
    ///
    /// * `events` 用于推送启动/日志/进度/结束事件；
    /// * `cancel` 触发**优雅停止**：写 `q` 到 stdin，5 秒内未退出则杀进程树；
    /// * 超时（`request.timeout_seconds`）则**直接杀树**，不发 `q`。
    pub async fn run(
        &self,
        request: RunRequest,
        events: Option<tokio::sync::mpsc::UnboundedSender<RunEvent>>,
        cancel: CancellationToken,
    ) -> Result<RunOutcome, FFmpegError> {
        let paths = self.paths();

        // 固定前置 4 个 token（顺序严格，规格 §3.9）
        let mut args: Vec<String> = Vec::with_capacity(request.args.len() + 4);
        args.push("-hide_banner".to_string());
        args.push(if request.overwrite { "-y".to_string() } else { "-n".to_string() });
        args.push("-progress".to_string());
        args.push("pipe:2".to_string());
        args.extend(request.args.iter().cloned());

        let command_line = util::render_command_line(&paths.ffmpeg, &args);

        let mut cmd = util::base_command(&paths.ffmpeg);
        cmd.args(&args);
        let started = Instant::now();
        let mut child = match cmd.spawn() {
            Ok(child) => child,
            Err(err) => {
                let detail = format!("无法启动 FFmpeg（{}）：{}", paths.ffmpeg, err);
                if let Some(sender) = &events {
                    let _ = sender.send(RunEvent::Stderr(detail.clone()));
                }
                return Err(match err.kind() {
                    std::io::ErrorKind::NotFound => {
                        FFmpegError::NotFound(format!("{}（{}）", paths.ffmpeg, paths.hint))
                    }
                    _ => FFmpegError::Spawn(detail),
                });
            }
        };
        let pid = child.id();

        if let Some(sender) = &events {
            let _ = sender
                .send(RunEvent::Started { pid, command_line: command_line.clone() });
        }

        // stdin 常开：优雅停止要往里写 `q`（规格 §3.10）
        let stdin: Arc<tokio::sync::Mutex<Option<tokio::process::ChildStdin>>> =
            Arc::new(tokio::sync::Mutex::new(child.stdin.take()));
        let handle = TaskHandle {
            stdin: stdin.clone(),
            force_kill: CancellationToken::new(),
            finished: CancellationToken::new(),
        };

        let task_key = request.task_key.as_ref().map(|k| k.trim().to_string()).filter(|k| !k.is_empty());
        if let Some(key) = &task_key {
            if let Ok(mut map) = self.inner.running.lock() {
                // 同一个 key 重复登记会覆盖（与 C# 的 TaskId 语义一致）
                map.insert(key.clone(), handle.clone());
            }
        }

        let stderr_tail = Arc::new(Mutex::new(BoundedLineBuffer::new(STDERR_TAIL_MAX_LINES)));
        let skipped_existing = Arc::new(AtomicBool::new(false));
        let duration_hint = request.duration_hint;

        let stderr_task = {
            let tail = stderr_tail.clone();
            let skipped = skipped_existing.clone();
            let sender = events.clone();
            let pipe = child.stderr.take();
            tokio::spawn(async move {
                let Some(pipe) = pipe else {
                    return;
                };
                let mut lines = BufReader::new(pipe).lines();
                let mut accumulator = ProgressAccumulator::default();
                // 进程被杀时读取会报错，属预期情况
                while let Ok(Some(line)) = lines.next_line().await {
                    if let Ok(mut buffer) = tail.lock() {
                        buffer.push(line.clone());
                    }
                    if util::contains_ignore_case(&line, SKIPPED_MARKER_QUESTION)
                        || util::contains_ignore_case(&line, SKIPPED_MARKER_EXITING)
                    {
                        skipped.store(true, Ordering::SeqCst);
                    }
                    match (accumulator.try_consume(&line, duration_hint), &sender) {
                        (Some(tick), Some(sender)) => {
                            let _ = sender.send(RunEvent::Progress(tick));
                        }
                        (Some(_), None) => {}
                        (None, Some(sender)) => {
                            let _ = sender.send(RunEvent::Stderr(line));
                        }
                        (None, None) => {}
                    }
                }
            })
        };

        let stdout_task = {
            let skipped = skipped_existing.clone();
            let pipe = child.stdout.take();
            tokio::spawn(async move {
                let Some(pipe) = pipe else {
                    return;
                };
                let mut lines = BufReader::new(pipe).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    // stdout 只用于检测「已存在」提问，内容不进日志（规格 §3.10）
                    if util::contains_ignore_case(&line, SKIPPED_MARKER_QUESTION) {
                        skipped.store(true, Ordering::SeqCst);
                    }
                }
            })
        };

        let timeout_at = request
            .timeout_seconds
            .filter(|seconds| *seconds > 0)
            .map(|seconds| tokio::time::Instant::now() + Duration::from_secs(seconds));
        let graceful_wait = util::GRACEFUL_STOP_WAIT;
        let mut graceful_deadline: Option<tokio::time::Instant> = None;
        let mut give_up_deadline: Option<tokio::time::Instant> = None;
        let mut cancelled = false;
        let mut timed_out = false;
        let mut force_killed = false;
        let mut exit_status: Option<ExitStatus> = None;
        let mut wait_error: Option<std::io::Error> = None;

        loop {
            // 先 select 出「唤醒原因」，再在 select 之外动 child —— 避免借用冲突。
            let wake = tokio::select! {
                status = child.wait() => Wake::Exit(status),
                _ = cancel.cancelled(), if !cancelled => Wake::Cancelled,
                _ = handle.force_kill.cancelled(), if !force_killed => Wake::ForceKill,
                _ = sleep_until_opt(graceful_deadline), if graceful_deadline.is_some() => Wake::GracefulTimeout,
                _ = sleep_until_opt(timeout_at), if timeout_at.is_some() && !timed_out => Wake::Timeout,
                _ = sleep_until_opt(give_up_deadline), if give_up_deadline.is_some() => Wake::GiveUp,
            };
            match wake {
                Wake::Exit(Ok(status)) => {
                    exit_status = Some(status);
                    break;
                }
                Wake::Exit(Err(err)) => {
                    wait_error = Some(err);
                    break;
                }
                Wake::Cancelled => {
                    // 取消 = 先写 `q`，5 秒不退再杀树（规格 §3.13）
                    cancelled = true;
                    util::write_quit(&stdin).await;
                    graceful_deadline = Some(tokio::time::Instant::now() + graceful_wait);
                }
                Wake::ForceKill => {
                    force_killed = true;
                    util::kill_process_tree(pid, &mut child).await;
                    give_up_deadline = Some(tokio::time::Instant::now() + give_up_wait());
                }
                Wake::GracefulTimeout => {
                    graceful_deadline = None;
                    util::kill_process_tree(pid, &mut child).await;
                    give_up_deadline = Some(tokio::time::Instant::now() + give_up_wait());
                }
                Wake::Timeout => {
                    // 超时：直接杀树，不发 `q`（规格 §3.13）
                    timed_out = true;
                    util::kill_process_tree(pid, &mut child).await;
                    give_up_deadline = Some(tokio::time::Instant::now() + give_up_wait());
                }
                Wake::GiveUp => {
                    // PORT NOTE：理论上 SIGKILL 之后进程必然消亡；这里只是兜底，
                    // 避免读取线程/等待永久挂住整个任务（C# 在该场景会一直 await）。
                    break;
                }
            }
        }

        // 等读取线程收口（最多 3 秒，规格 §3.11）
        let _ = tokio::time::timeout(util::READER_JOIN_WAIT, async {
            let _ = tokio::join!(stderr_task, stdout_task);
        })
        .await;

        let elapsed_ms = started.elapsed().as_millis() as u64;
        let exit_code = exit_status.and_then(|status| status.code());
        let skipped = skipped_existing.load(Ordering::SeqCst);

        let (diagnostic_raw, log_lines) = match stderr_tail.lock() {
            Ok(guard) => (guard.join(), guard.lines()),
            Err(poisoned) => {
                let guard = poisoned.into_inner();
                (guard.join(), guard.lines())
            }
        };

        let mut diagnostic = diagnostic_raw;
        if skipped && exit_code != Some(0) {
            diagnostic = format!("{SKIPPED_PREFIX}\n{diagnostic}");
        }
        let stderr_tail_text = util::truncate(&diagnostic, DIAGNOSTIC_MAX_CHARS);

        let mut success = !cancelled && !timed_out && exit_code == Some(0);
        if cancelled && exit_code == Some(0) {
            // PORT NOTE：C# 显式把「取消但退出码 0」判为失败（`FFmpegService.cs:725-730`），照抄。
            success = false;
        }

        // 登记表清理（`finally` 的等价物）
        handle.finished.cancel();
        if let Some(key) = &task_key {
            if let Ok(mut map) = self.inner.running.lock() {
                map.remove(key);
            }
        }

        if let Some(sender) = &events {
            let _ = sender.send(RunEvent::Finished {
                exit_code,
                stderr_tail: stderr_tail_text.clone(),
                elapsed_ms,
                cancelled,
                timed_out,
            });
        }

        if let Some(err) = wait_error {
            return Err(FFmpegError::Io(err));
        }

        Ok(RunOutcome {
            exit_code,
            success,
            cancelled,
            timed_out,
            elapsed_ms,
            stderr_tail: stderr_tail_text,
            log_lines,
        })
    }

    /// 停止某个登记的任务（对应 C# `KillAsync`）。
    ///
    /// `graceful = true`：写 `q`，并起一个 5 秒看门狗，超时则杀进程树；
    /// `graceful = false`：立即杀进程树。返回 `false` 表示没有该 key 的登记。
    pub async fn stop_task(&self, task_key: &str, graceful: bool) -> bool {
        let handle = match self.inner.running.lock() {
            Ok(map) => map.get(task_key.trim()).cloned(),
            Err(poisoned) => poisoned.into_inner().get(task_key.trim()).cloned(),
        };
        let Some(handle) = handle else {
            return false;
        };

        if graceful {
            util::write_quit(&handle.stdin).await;
            let finished = handle.finished.clone();
            let force_kill = handle.force_kill.clone();
            tokio::spawn(async move {
                tokio::select! {
                    _ = tokio::time::sleep(Duration::from_secs(GRACEFUL_STOP_SECS)) => {
                        if !finished.is_cancelled() {
                            force_kill.cancel();
                        }
                    }
                    _ = finished.cancelled() => {}
                }
            });
        } else {
            handle.force_kill.cancel();
        }
        true
    }

    /// 向运行的进程写一段输入（对应 C# `SendInputAsync`，主要给交互式确认用）。
    pub async fn send_input(&self, task_key: &str, input: &str) -> bool {
        let handle = match self.inner.running.lock() {
            Ok(map) => map.get(task_key.trim()).cloned(),
            Err(poisoned) => poisoned.into_inner().get(task_key.trim()).cloned(),
        };
        let Some(handle) = handle else {
            return false;
        };
        let mut guard = handle.stdin.lock().await;
        let Some(pipe) = guard.as_mut() else {
            return false;
        };
        match pipe.write_all(input.as_bytes()).await {
            Ok(()) => pipe.flush().await.is_ok(),
            Err(_) => false,
        }
    }
}

/// `select!` 唤醒原因（把「动 child」的逻辑移出 select，规避借用冲突）。
enum Wake {
    Exit(std::io::Result<ExitStatus>),
    Cancelled,
    ForceKill,
    GracefulTimeout,
    Timeout,
    GiveUp,
}

async fn sleep_until_opt(deadline: Option<tokio::time::Instant>) {
    match deadline {
        Some(deadline) => tokio::time::sleep_until(deadline).await,
        None => std::future::pending::<()>().await,
    }
}

fn give_up_wait() -> Duration {
    Duration::from_secs(10)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_version_output_handles_release_and_git_builds() {
        let parsed = parse_version_output(
            "ffmpeg version 9.0.2 Copyright (c) 2000-2026 the FFmpeg developers\n\
             built with Apple clang version 21.0.0\n\
             configuration: --enable-gpl --enable-libx264\n\
             libavutil      60.  8.100 / 60.  8.100\n",
        );
        assert_eq!(parsed.version, "9.0.2");
        assert_eq!(parsed.major_version, 9);
        assert_eq!(parsed.configuration, "--enable-gpl --enable-libx264");
        assert!(parsed.version_line.starts_with("ffmpeg version 9.0.2"));

        let git = parse_version_output("ffmpeg version N-118527-g1234567890\n");
        assert_eq!(git.version, "N-118527-g1234567890");
        assert_eq!(git.major_version, 118527);

        let n_version = parse_version_output("FFmpeg version n7.1.1 Copyright\n");
        assert_eq!(n_version.version, "n7.1.1");
        assert_eq!(n_version.major_version, 7);

        assert_eq!(parse_version_output("no version here").version, "");
    }

    #[test]
    fn parse_listing_table_follows_spec_heuristics() {
        let formats = "File formats:\n D. = Demuxing supported\n .E = Muxing supported\n ---\n  DE matroska         Matroska\n  DE  mp4             MP4\n  DE  mp4             duplicate\n";
        assert_eq!(parse_listing_table(formats, 3), vec!["matroska", "mp4"]);

        let encoders = "Encoders:\n V....D libx264              libx264 H.264\n V....D h264_nvenc           NVIDIA NVENC\n A..... aac                  AAC\n";
        assert_eq!(parse_listing_table(encoders, 6), vec!["libx264", "h264_nvenc", "aac"]);

        let codecs = "Codecs:\n DEV.L. h264                 H.264 / AVC\n D.A.L. mp3                  MP3\n";
        assert_eq!(parse_listing_table(codecs, 6), vec!["h264", "mp3"]);
    }

    #[test]
    fn parse_hwaccels_filters_banner_and_noise() {
        let output = "Hardware acceleration methods:\nvideotoolbox\ncuda\nvaapi\n  \nSome Bad Line\n";
        assert_eq!(parse_hwaccels(output), vec!["videotoolbox", "cuda", "vaapi"]);
    }

    #[test]
    fn parse_filters_uses_signature_column() {
        let output = "Filters:\n  T.. = Timeline support\n  TSC a3dither3x3      V->V       Signed 3x3\n  T.. scale            V->V       Scale\n  ... split            V->N       Split\n";
        assert_eq!(parse_filters(output), vec!["a3dither3x3", "scale", "split"]);
    }

    #[test]
    fn progress_accumulator_divides_out_time_ms_by_1e6() {
        let mut accumulator = ProgressAccumulator::default();
        assert!(accumulator.try_consume("frame=25", 10.0).is_none());
        assert!(accumulator.try_consume("fps=25.00", 10.0).is_none());
        assert!(accumulator.try_consume("out_time_us=5000000", 10.0).is_none());
        assert!(accumulator.try_consume("out_time_ms=5000000", 10.0).is_none());
        assert!(accumulator.try_consume("out_time=00:00:05.000000", 10.0).is_none());
        assert!(accumulator.try_consume("speed=1.5x", 10.0).is_none());
        let tick = accumulator.try_consume("progress=continue", 10.0).expect("进度块收尾");
        assert_eq!(tick.out_time_seconds, 5.0);
        assert_eq!(tick.percent, 50.0);
        assert_eq!(tick.speed, Some(1.5));
        assert_eq!(tick.frame, Some(25));
        assert_eq!(tick.progress.as_deref(), Some("continue"));
        assert_eq!(tick.eta_seconds(10.0), Some(10.0 / 3.0));

        // 未知 key 不进入进度块
        assert!(accumulator.try_consume("stream_0_0_q=29.0", 10.0).is_none());
        // 没有 '=' 的普通日志行
        assert!(accumulator.try_consume("frame=1", 10.0).is_none());
    }

    #[test]
    fn progress_end_forces_hundred_percent() {
        let mut accumulator = ProgressAccumulator::default();
        accumulator.try_consume("out_time_ms=9000000", 10.0);
        let tick = accumulator.try_consume("progress=end", 10.0).expect("收尾");
        assert_eq!(tick.out_time_seconds, 10.0);
        assert_eq!(tick.percent, 100.0);
    }

    #[test]
    fn progress_without_duration_reports_minus_one() {
        let mut accumulator = ProgressAccumulator::default();
        accumulator.try_consume("out_time_us=1000000", 0.0);
        let tick = accumulator.try_consume("progress=continue", 0.0).expect("收尾");
        assert_eq!(tick.percent, -1.0);
        assert_eq!(tick.eta_seconds(10.0), None);
    }

    #[test]
    fn parse_media_info_reads_format_and_streams() {
        let json = r#"{
            "streams": [
                {"index": 0, "codec_name": "h264", "codec_long_name": "H.264 / AVC",
                 "codec_type": "video", "width": 320, "height": 240,
                 "r_frame_rate": "30000/1001", "pix_fmt": "yuv420p",
                 "side_data_list": [{"rotation": -90}]},
                {"index": 1, "codec_name": "aac", "codec_type": "audio",
                 "sample_rate": "48000", "channels": 2, "bit_rate": "128000"}
            ],
            "format": {"format_name": "mov,mp4,m4a,3gp,3g2,mj2", "format_long_name": "QuickTime / MOV",
                       "duration": "2.500000", "bit_rate": "500000"}
        }"#;
        let info = parse_media_info("/tmp/does-not-exist.mp4", json).expect("解析成功");
        assert_eq!(info.duration_seconds, 2.5);
        assert_eq!(info.bit_rate, 500_000);
        assert_eq!(info.format_display(), "MP4 / MOV");
        assert_eq!(info.duration_display(), "0:02");
        assert!(info.has_video() && info.has_audio());
        assert_eq!(info.subtitle_stream_count(), 0);
        assert_eq!(info.rotation(), 270);
        let video = info.video.expect("视频流");
        assert_eq!(video.resolution_display(), "320x240");
        assert!((video.frame_rate.unwrap_or(0.0) - 29.970_029).abs() < 0.001);
        let audio = info.audio.expect("音频流");
        assert_eq!(audio.sample_rate, Some(48000));
        assert_eq!(audio.channels, Some(2));
        assert_eq!(audio.resolution_display(), "-");
        assert!(audio.summary().contains("音频 aac"));
    }

    #[test]
    fn parse_fraction_and_timecode_edges() {
        assert_eq!(parse_fraction(Some("25/1")), Some(25.0));
        assert_eq!(parse_fraction(Some("24")), Some(24.0));
        assert_eq!(parse_fraction(Some("0/0")), None);
        assert_eq!(parse_fraction(Some("")), None);
        assert_eq!(parse_fraction(None), None);
        assert_eq!(parse_timecode("00:01:23.4567890"), Some(83.456789));
        assert_eq!(parse_timecode("-00:00:02.5"), Some(-2.5));
        assert_eq!(parse_timecode("01:02"), None);
    }

    #[test]
    fn truncate_keeps_tail_with_prefix() {
        let long = "a".repeat(9000);
        let truncated = util::truncate(&long, DIAGNOSTIC_MAX_CHARS);
        assert!(truncated.starts_with(util::TRUNCATED_PREFIX));
        assert_eq!(truncated.chars().count(), util::TRUNCATED_PREFIX.chars().count() + 8000);
        assert_eq!(util::truncate("short", 8000), "short");
    }

    #[test]
    fn run_request_builders() {
        let request = RunRequest::new(vec!["-i".into(), "in.mp4".into()])
            .with_duration_hint(2.5)
            .with_overwrite(false)
            .with_timeout(Some(30))
            .with_task_key("task-1");
        assert_eq!(request.duration_hint, 2.5);
        assert!(!request.overwrite);
        assert_eq!(request.timeout_seconds, Some(30));
        assert_eq!(request.task_key.as_deref(), Some("task-1"));
    }
}
