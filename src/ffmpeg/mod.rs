//! FFmpeg 子系统（Rust 版实现依据：`docs/reference/ffmpeg-core.md`）。
//!
//! 模块划分与参考实现的对应关系：
//!
//! | 模块 | 参考实现 | 职责 | 规格章节 |
//! | --- | --- | --- | --- |
//! | [`locator`] | `FFmpegLocator.cs` | ffmpeg / ffprobe 可执行文件定位 | §2 |
//! | [`service`] | `FFmpegService.cs` | 版本/能力探测、ffprobe 媒体探测、子进程执行与进度解析 | §3 |
//! | [`encoder_catalog`] | `EncoderCatalog.cs` | 31 条编码器参数画像 | §6 |
//! | [`hardware`] | `HardwareDetectionService.cs` | 硬件编码器检测（平台过滤 → 构建清单过滤 → 试跑 → 三层缓存） | §5 |
//!
//! 全局约定：`FFmpegService` / `HardwareDetector` 内部都是 `Arc`，可以直接 `clone` 进 axum 的
//! AppState（`Send + Sync + 'static`）。

pub mod encoder_catalog;
pub mod hardware;
pub mod locator;
pub mod service;

pub use encoder_catalog::{EncoderCatalog, EncoderProfile, EncoderType};
pub use hardware::{DetectedEncoder, HardwareDetector, HardwarePreferences, HardwareReport};
pub use locator::FFmpegPaths;
pub use service::{
    FFmpegError, FFmpegInfo, FFmpegService, MediaInfo, MediaStream, ProgressTick, RunEvent,
    RunOutcome, RunRequest,
};

/// 子系统内部共享工具（不对外暴露）。
///
/// 这里的常量与算法逐字对应规格 §7「魔法常量汇总」与 §3.11 / §3.12 的文本处理规则。
pub(crate) mod util {
    use std::collections::VecDeque;
    use std::process::Stdio;

    /// stderr 尾部环形缓冲行数（规格 §7：200 行）。
    pub const STDERR_TAIL_LINES: usize = 200;
    /// 诊断信息截断长度（规格 §7：保留末尾 8000 字符）。
    pub const DIAGNOSTIC_MAX_CHARS: usize = 8000;
    /// 截断时追加的前缀（规格 §3.11 `Truncate`）。
    pub const TRUNCATED_PREFIX: &str = "…（已截断前部输出）\n";
    /// 取消后写 `q` 等待进程退出的时长，超时则杀进程树（规格 §3.13）。
    pub const GRACEFUL_STOP_WAIT: std::time::Duration = std::time::Duration::from_secs(5);
    /// 进程结束后等待 stdout/stderr 读取线程收口的时长（规格 §3.11：3 秒）。
    pub const READER_JOIN_WAIT: std::time::Duration = std::time::Duration::from_secs(3);
    /// 抓取型命令（`-version` / `-formats` / ffprobe 探测）的超时（规格 §7：30 秒）。
    pub const CAPTURE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

    /// 保留末尾 `max` 个字符；超长时加上「已截断」前缀（对应 C# `Truncate`）。
    ///
    /// 注意：C# 的 `string.Length` 是 UTF-16 码元数，这里按 Unicode 标量值计数，
    /// 对纯 ASCII 输出完全等价（规格 §3.11）。
    pub fn truncate(text: &str, max: usize) -> String {
        if text.is_empty() {
            return String::new();
        }
        let count = text.chars().count();
        if count <= max {
            return text.to_string();
        }
        let tail: String = text.chars().skip(count - max).collect();
        format!("{TRUNCATED_PREFIX}{tail}")
    }

    /// 大小写不敏感的 `Contains`（C# 里到处用的 `StringComparison.OrdinalIgnoreCase`）。
    pub fn contains_ignore_case(haystack: &str, needle: &str) -> bool {
        if needle.is_empty() {
            return true;
        }
        haystack.to_ascii_lowercase().contains(&needle.to_ascii_lowercase())
    }

    /// 有界行缓冲：只保留最近 `capacity` 行（对应 C# `BoundedLineBuffer`）。
    #[derive(Debug, Clone)]
    pub struct BoundedLineBuffer {
        capacity: usize,
        lines: VecDeque<String>,
    }

    impl BoundedLineBuffer {
        pub fn new(capacity: usize) -> Self {
            Self { capacity: capacity.max(1), lines: VecDeque::with_capacity(capacity.max(1)) }
        }

        pub fn push(&mut self, line: String) {
            if self.lines.len() >= self.capacity {
                let _ = self.lines.pop_front();
            }
            self.lines.push_back(line);
        }

        /// 用换行连接（对应 C# `ToString()` 里的 `Environment.NewLine` 拼接）。
        pub fn join(&self) -> String {
            self.lines.iter().cloned().collect::<Vec<_>>().join("\n")
        }

        pub fn lines(&self) -> Vec<String> {
            self.lines.iter().cloned().collect()
        }
    }

    /// 类 POSIX 分词（对应 `CommandLineBuilder.Tokenize` 的最小可用实现）：
    /// 空白分隔；单引号内原样；双引号内允许 `\` 转义；反斜杠转义下一个字符。
    pub fn tokenize(input: &str) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        let mut current = String::new();
        let mut has_token = false;
        let mut chars = input.chars().peekable();

        while let Some(ch) = chars.next() {
            match ch {
                ' ' | '\t' | '\n' | '\r' => {
                    if has_token {
                        out.push(std::mem::take(&mut current));
                        has_token = false;
                    }
                }
                '\'' => {
                    has_token = true;
                    for inner in chars.by_ref() {
                        if inner == '\'' {
                            break;
                        }
                        current.push(inner);
                    }
                }
                '"' => {
                    has_token = true;
                    while let Some(inner) = chars.next() {
                        if inner == '"' {
                            break;
                        }
                        if inner == '\\' {
                            if let Some(escaped) = chars.next() {
                                current.push(escaped);
                            }
                        } else {
                            current.push(inner);
                        }
                    }
                }
                '\\' => {
                    if let Some(escaped) = chars.next() {
                        current.push(escaped);
                        has_token = true;
                    }
                }
                _ => {
                    current.push(ch);
                    has_token = true;
                }
            }
        }
        if has_token {
            out.push(current);
        }
        out
    }

    /// 渲染可读命令行（对应 C# `CommandLineBuilder.RenderCommandLine` / `QuoteForDisplay`）。
    ///
    /// 仅用于展示与日志；真正传给子进程的是参数数组（不经 shell，无注入风险）。
    pub fn render_command_line(program: &str, args: &[String]) -> String {
        let mut out = String::from(program);
        for arg in args {
            out.push(' ');
            out.push_str(&quote_for_display(arg));
        }
        out
    }

    /// 含 `空格 制表符 " ' ; & | > <` 的参数加双引号；内部 `"` 转义为 `\"`；空串渲染为 `""`。
    pub fn quote_for_display(arg: &str) -> String {
        if arg.is_empty() {
            return "\"\"".to_string();
        }
        let needs_quote = arg
            .chars()
            .any(|c| matches!(c, ' ' | '\t' | '"' | '\'' | ';' | '&' | '|' | '>' | '<'));
        if !needs_quote {
            return arg.to_string();
        }
        format!("\"{}\"", arg.replace('"', "\\\""))
    }

    /// 构造子进程命令：三条管道 + 独立进程组。
    ///
    /// Unix 用 `setpgid(0, 0)`（`process_group(0)`），Windows 用 `CREATE_NEW_PROCESS_GROUP`，
    /// 这样杀进程时可以把整棵树（含 ffmpeg 自己拉起的子进程）一起收掉。
    pub fn base_command(program: &str) -> tokio::process::Command {
        let mut cmd = tokio::process::Command::new(program);
        cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());

        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            // 子进程成为新进程组的组长：pgid == pid，便于 `kill -KILL -pid`。
            cmd.as_std_mut().process_group(0);
        }
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            // Win32 `CREATE_NEW_PROCESS_GROUP`
            const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
            cmd.as_std_mut().creation_flags(CREATE_NEW_PROCESS_GROUP);
        }
        cmd
    }

    /// 杀掉整棵进程树（对应 C# `Process.Kill(entireProcessTree: true)`）。
    ///
    /// Unix：因为 spawn 时已经 `process_group(0)`，向 `-pgid` 发 SIGKILL 即可收掉整组；
    /// `/bin/kill` 不可用时退化为只杀直接子进程。Windows：只杀直接子进程
    /// （不引入新依赖，无法调用 Job Object / `taskkill /T`）。
    pub async fn kill_process_tree(pid: Option<u32>, child: &mut tokio::process::Child) {
    // Windows 分支只杀直接子进程（不引入 Job Object），用不到 pid；
    // 显式消费掉，否则 Windows 构建会报 `unused variable: pid`（Linux 的同类问题见 file_service）。
    #[cfg(not(unix))]
    let _ = pid;
        #[cfg(unix)]
        if let Some(pid) = pid {
            let status = tokio::process::Command::new("/bin/kill")
                .arg("-KILL")
                .arg(format!("-{pid}"))
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .await;
            if matches!(status, Ok(ref s) if s.success()) {
                return;
            }
        }
        let _ = child.start_kill();
    }

    /// 写入 `q` 触发 ffmpeg 优雅退出（对应 C# `TryGracefulStop`：单个小写 `q`，无换行）。
    pub async fn write_quit(stdin: &tokio::sync::Mutex<Option<tokio::process::ChildStdin>>) {
        use tokio::io::AsyncWriteExt;
        let mut guard = stdin.lock().await;
        if let Some(pipe) = guard.as_mut() {
            // 管道可能已被 ffmpeg 关闭，写失败是预期情况。
            let _ = pipe.write_all(b"q").await;
            let _ = pipe.flush().await;
        }
    }

    /// 逻辑核心数（用于试跑并发度 `clamp(cpus / 2, 2, 4)`）。
    pub fn cpu_count() -> usize {
        std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1)
    }
}
