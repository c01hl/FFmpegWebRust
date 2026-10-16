//! 跨平台原生文件/文件夹选择器（对应参考实现 `Services/FileDialogService.cs`，规格见
//! `docs/reference/backend-data.md` §7）。
//!
//! 为什么必须由**服务端**弹对话框：浏览器拿不到真实绝对路径（拖放因此被放弃）。
//! 这假设「服务端与用户在同一台机器」，与默认只监听回环地址是一致的。
//!
//! 后端：macOS 用 `osascript`，Linux 优先 `zenity`、回退 `kdialog`，Windows 用 PowerShell +
//! WinForms。全部用 `tokio::process::Command` 以 **argv** 形式启动；`osascript` 的脚本经 `-e`
//! 传递，Windows 的脚本经 `-EncodedCommand` 传递 —— **任何平台都不经过 shell**（无 `sh -c`）。

use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::io::AsyncReadExt;
use tokio::process::Command;

/// 选择器的后端信息（界面据此决定「浏览」按钮是否可用）
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DialogBackend {
    pub is_supported: bool,
    /// 后端描述，例如 `macOS 原生选择器（osascript）`
    pub backend: String,
}

/// 选择类型
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DialogKind {
    File,
    Folder,
}

/// 请求
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DialogRequest {
    pub kind: DialogKind,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub initial_directory: Option<String>,
    /// 文件类型过滤，例如 `媒体文件`（各平台后端自行解释）
    #[serde(default)]
    pub filter: Option<String>,
}

impl DialogRequest {
    pub fn file() -> Self {
        Self { kind: DialogKind::File, title: "选择文件".to_string(), initial_directory: None, filter: None }
    }

    pub fn folder() -> Self {
        Self { kind: DialogKind::Folder, title: "选择文件夹".to_string(), initial_directory: None, filter: None }
    }
}

/// 结果：`path` 为 `None` 时 `reason` 说明原因（`cancelled` / `unsupported` / `timeout` / 具体错误）
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DialogResult {
    pub path: Option<String>,
    pub reason: Option<String>,
}

impl DialogResult {
    pub fn cancelled() -> Self {
        Self { path: None, reason: Some("cancelled".to_string()) }
    }

    pub fn unsupported(backend: &str) -> Self {
        Self { path: None, reason: Some(format!("unsupported:{backend}")) }
    }
}

// ────────────────────────────────────────────────────────────────────────────
// 常量（与参考实现逐字一致的文案/过滤器）
// ────────────────────────────────────────────────────────────────────────────

/// 对话框最长等待时间，避免某些环境把界面卡死
/// （对应 `FileDialogService.DialogTimeout` = 5 分钟；超时即杀子进程）
const DIALOG_TIMEOUT: Duration = Duration::from_secs(5 * 60);

/// 媒体扩展名（逐字，`FileDialogService.cs:197-198`；zenity/kdialog 用空格分隔）
const MEDIA_PATTERNS: &str = "*.mp4 *.mkv *.avi *.mov *.wmv *.flv *.webm *.m4v *.ts *.mpg \
*.mp3 *.wav *.flac *.aac *.ogg *.m4a *.opus *.gif *.png *.jpg";

/// 同上的 PowerShell 形式（`;` 分隔，逐字来自 WinForms 脚本）
const MEDIA_PATTERNS_POWERSHELL: &str = "*.mp4;*.mkv;*.avi;*.mov;*.wmv;*.flv;*.webm;*.m4v;*.ts;*.mpg;\
*.mp3;*.wav;*.flac;*.aac;*.ogg;*.m4a;*.opus;*.gif;*.png;*.jpg";

/// 过滤器显示名的默认值（原实现里 zenity/Windows 脚本中硬编码的文案）
const DEFAULT_FILTER_LABEL: &str = "媒体文件";

/// Linux 无可用后端时的描述（逐字，§7.1）
const LINUX_UNAVAILABLE: &str = "不可用（未安装 zenity 或 kdialog）";

/// zenity / kdialog 用退出码 1 表示「用户取消」
const HELPER_CANCEL_EXIT_CODE: i32 = 1;

/// Windows 上按顺序尝试的 PowerShell（逐字，`FileDialogService.cs:306`）
const WINDOWS_SHELLS: [&str; 3] = ["powershell.exe", "pwsh.exe", "pwsh"];

/// PowerShell 脚本前导语句：统一输出编码，避免中文乱码（逐字，`FileDialogService.cs:303`）
const POWERSHELL_PREAMBLE: &str = "[Console]::OutputEncoding = [System.Text.Encoding]::UTF8\n";

/// AppleScript 文件引用被序列化成字符串时可能出现的前缀（逐字，忽略大小写匹配）
const APPLESCRIPT_REFERENCE_PREFIXES: [&str; 5] = ["alias ", "file ", "folder ", "disk ", "document file "];

/// 启动卷名（逐字硬编码，§7.4；改名过的系统盘会被错拼成 `/Volumes/<卷名>/…`，见 PORT NOTE）
const BOOT_VOLUME_NAME: &str = "Macintosh HD";

// ────────────────────────────────────────────────────────────────────────────
// 后端探测
// ────────────────────────────────────────────────────────────────────────────

/// 平台分支（`cfg!` 判定的可测试版本）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Platform {
    Windows,
    MacOs,
    Linux,
}

fn current_platform() -> Platform {
    if cfg!(target_os = "windows") {
        Platform::Windows
    } else if cfg!(target_os = "macos") {
        Platform::MacOs
    } else {
        // 其它 Unix（FreeBSD 等）与原实现一样归入 Linux 分支
        Platform::Linux
    }
}

/// 当前平台可用的对话框后端
pub fn backend() -> DialogBackend {
    resolve_backend(current_platform(), &helper_exists)
}

/// 纯函数版本的后端解析：`exists` 探测辅助程序是否存在（便于单元测试注入）
fn resolve_backend(platform: Platform, exists: &dyn Fn(&str) -> bool) -> DialogBackend {
    match platform {
        Platform::Windows => {
            // PORT NOTE: 原实现判定 Windows 恒为可用（依赖系统自带 PowerShell）；这里改为探测
            // `powershell.exe` / `pwsh.exe` / `pwsh`，探测不到则如实报「不可用」，
            // 让界面给出「手动粘贴路径」的降级提示（§7.1 文案不变）。
            let found = WINDOWS_SHELLS.iter().any(|shell| exists(shell));
            DialogBackend {
                is_supported: found,
                backend: if found {
                    "Windows 文件对话框（PowerShell）".to_string()
                } else {
                    "不可用（未找到 PowerShell）".to_string()
                },
            }
        }
        Platform::MacOs => {
            // PORT NOTE: 原实现判定 macOS 恒为可用（依赖系统自带 osascript）；这里同样改为探测，
            // 系统被裁剪掉 osascript 时如实报「不可用」而不是弹一个必然失败的对话框。
            let found = exists("osascript");
            DialogBackend {
                is_supported: found,
                backend: if found {
                    "macOS 原生选择器（osascript）".to_string()
                } else {
                    "不可用（未安装 osascript）".to_string()
                },
            }
        }
        Platform::Linux => {
            if exists("zenity") {
                DialogBackend { is_supported: true, backend: "zenity".to_string() }
            } else if exists("kdialog") {
                DialogBackend { is_supported: true, backend: "kdialog".to_string() }
            } else {
                DialogBackend { is_supported: false, backend: LINUX_UNAVAILABLE.to_string() }
            }
        }
    }
}

/// 辅助程序是否存在：先查 `PATH`，再查 `/usr/bin`（系统自带工具可能不在精简过的 PATH 里）
fn helper_exists(name: &str) -> bool {
    if command_exists(name) {
        return true;
    }
    if cfg!(unix) {
        return Path::new("/usr/bin").join(name).is_file();
    }
    false
}

/// 遍历 `PATH` 判断命令是否存在（对应 `CommandExists`，`FileDialogService.cs:248-263`）。
///
/// 与原实现一致：**不检查可执行位**（文件存在即算可用）；
/// PORT NOTE: Windows 上额外补一次 `.exe` 后缀（原实现只在调用处自带后缀，这里更稳）。
fn command_exists(command: &str) -> bool {
    let Some(path_var) = std::env::var_os("PATH") else { return false };
    for directory in std::env::split_paths(&path_var) {
        if directory.as_os_str().is_empty() {
            continue;
        }
        if directory.join(command).is_file() {
            return true;
        }
        if cfg!(windows)
            && !command.to_ascii_lowercase().ends_with(".exe")
            && directory.join(format!("{command}.exe")).is_file()
        {
            return true;
        }
    }
    false
}

// ────────────────────────────────────────────────────────────────────────────
// 弹出选择器
// ────────────────────────────────────────────────────────────────────────────

/// 弹出原生选择器；用户取消 / 后端不可用 / 超时分别用 `reason` 区分
pub async fn pick(request: DialogRequest) -> DialogResult {
    let backend = backend();
    pick_with_backend(request, &backend).await
}

/// 后端已解析后的分发（与 `pick` 拆开是为了能在无 GUI 的 CI 里测试「不可用」分支）
async fn pick_with_backend(request: DialogRequest, backend: &DialogBackend) -> DialogResult {
    if !backend.is_supported {
        // 后端不可用：不做任何子进程尝试，直接把描述回给界面（§7.1 的降级文案）
        return DialogResult::unsupported(&backend.backend);
    }

    let start = resolve_start_directory(request.initial_directory.as_deref());
    match current_platform() {
        Platform::MacOs => pick_macos(&request, &start).await,
        Platform::Linux => pick_linux(&request, &start).await,
        Platform::Windows => pick_windows(&request, &start).await,
    }
}

/// macOS：`/usr/bin/osascript -e "<完整脚本>"`（结果已由 `POSIX path of` 转成 POSIX 路径）
async fn pick_macos(request: &DialogRequest, start: &str) -> DialogResult {
    let script = build_macos_script(request.kind, &request.title, start);
    // 脚本内部已经把「用户取消」（错误号 -128）转成空串 + 退出码 0，
    // 所以退出码非 0 是真错误，不能当成取消。
    outcome_to_result(run_helper(&macos_argv(&script), None).await)
}

/// Linux：先 zenity，再 kdialog
async fn pick_linux(request: &DialogRequest, start: &str) -> DialogResult {
    let label = filter_label(request.filter.as_deref());
    let mut failure: Option<String> = None;

    if helper_exists("zenity") {
        let argv = match request.kind {
            DialogKind::File => zenity_file_argv(&request.title, start, &label),
            DialogKind::Folder => zenity_folder_argv(&request.title, start),
        };
        match run_helper(&argv, Some(HELPER_CANCEL_EXIT_CODE)).await {
            HelperOutcome::Failed(message) => failure = Some(message),
            other => return outcome_to_result(other),
        }
    }

    if helper_exists("kdialog") {
        let argv = match request.kind {
            DialogKind::File => kdialog_file_argv(&request.title, start, &label),
            DialogKind::Folder => kdialog_folder_argv(&request.title, start),
        };
        match run_helper(&argv, Some(HELPER_CANCEL_EXIT_CODE)).await {
            HelperOutcome::Failed(message) => failure = Some(message),
            other => return outcome_to_result(other),
        }
    }

    // PORT NOTE: 原实现在「zenity 里点了取消」后还会再弹一次 kdialog（§7.3 明确列为坑）。
    // 这里用退出码区分取消与失败：取消直接返回 cancelled，只有启动失败/异常退出才回退。
    match failure {
        Some(message) => failed_result(message),
        None => DialogResult::unsupported(LINUX_UNAVAILABLE),
    }
}

/// Windows：按 `powershell.exe` → `pwsh.exe` → `pwsh` 顺序尝试
async fn pick_windows(request: &DialogRequest, start: &str) -> DialogResult {
    let label = filter_label(request.filter.as_deref());
    let script = match request.kind {
        DialogKind::File => windows_file_script(&request.title, start, &label),
        DialogKind::Folder => windows_folder_script(&request.title, start),
    };

    let mut failure: Option<String> = None;
    for shell in WINDOWS_SHELLS {
        match run_helper(&windows_argv(shell, &script), None).await {
            // PORT NOTE: 原实现超时后（返回 null）还会继续试下一个 shell；这里超时即终止，
            // 避免一次请求最长等 15 分钟。
            HelperOutcome::Timeout => return outcome_to_result(HelperOutcome::Timeout),
            HelperOutcome::Failed(message) => failure = Some(message),
            other => return outcome_to_result(other),
        }
    }

    failed_result(failure.unwrap_or_else(|| "未找到可用的 PowerShell".to_string()))
}

// ────────────────────────────────────────────────────────────────────────────
// macOS：AppleScript 脚本与 alias → POSIX 转换
// ────────────────────────────────────────────────────────────────────────────

/// 生成 macOS 选择器脚本（逐字，§7.2 / `BuildMacChooseScript`）。
///
/// 两个关键点（源码注释里解释过，移植必须保留）：
/// 1. 结果**必须**用 `POSIX path of (...)` 包住。否则拿到的是 AppleScript alias 字符串
///    （形如 `alias Macintosh HD:Users:me:Movies:a.mp4`），传回程序后会被判为「文件不存在」。
/// 2. 起始目录不可用不能让整个选择器失败：**AppleScript 的一个 `try` 只能有一个 `on error`
///    分支**，所以「用户取消」（错误号 `-128`）与「起始目录无效」必须在同一个错误块里用
///    `errNum` 区分：`-128` → 返回空串；其它错误 → 退化为不带起始目录再试一次。
fn build_macos_script(kind: DialogKind, title: &str, start: &str) -> String {
    let verb = match kind {
        DialogKind::File => "choose file",
        DialogKind::Folder => "choose folder",
    };
    let escaped_title = escape_applescript(title);
    let escaped_start = escape_applescript(start);
    let with_start =
        format!("POSIX path of ({verb} with prompt \"{escaped_title}\" default location POSIX file \"{escaped_start}\")");
    let without_start = format!("POSIX path of ({verb} with prompt \"{escaped_title}\")");

    let mut lines: Vec<String> = vec![
        "set theResult to \"\"".to_string(),
        "try".to_string(),
        // 把选择器窗口带到前台，否则从后台启动的进程弹出来的选择器会藏在别的窗口后面
        "    tell application \"System Events\" to set frontmost of first process whose frontmost is true to true"
            .to_string(),
        "end try".to_string(),
    ];

    if start.trim().is_empty() {
        // 单个 try：用户取消（-128）或不支持的环境都返回空字符串
        lines.push("try".to_string());
        lines.push(format!("    set theResult to {without_start}"));
        lines.push("on error".to_string());
        lines.push("    return \"\"".to_string());
        lines.push("end try".to_string());
    } else {
        lines.push("try".to_string());
        lines.push(format!("    set theResult to {with_start}"));
        lines.push("on error errMsg number errNum".to_string());
        lines.push("    if errNum is -128 then return \"\"".to_string());
        lines.push("    try".to_string());
        lines.push(format!("        set theResult to {without_start}"));
        lines.push("    on error".to_string());
        lines.push("        return \"\"".to_string());
        lines.push("    end try".to_string());
        lines.push("end try".to_string());
    }

    lines.push("return theResult".to_string());
    lines.join("\n")
}

/// AppleScript 字符串转义：`\` → `\\`，`"` → `\"`（逐字，不转义换行/其他字符）
fn escape_applescript(text: &str) -> String {
    text.replace('\\', "\\\\").replace('"', "\\\"")
}

/// macOS 完整 argv（含程序名，便于测试逐项断言）
fn macos_argv(script: &str) -> Vec<String> {
    vec!["/usr/bin/osascript".to_string(), "-e".to_string(), script.to_string()]
}

/// AppleScript HFS alias 路径 → POSIX 路径（§7.4，`ConvertAppleScriptAliasToPosix`）。
///
/// 形如 `Macintosh HD:Users:me:Movies:25 4K.mp4` → `/Users/me/Movies/25 4K.mp4`。
/// 第一段是卷名：启动卷映射到 `/`，其它卷映射到 `/Volumes/<卷名>/…`。
///
/// PORT NOTE: 启动卷名仍然硬编码为 `Macintosh HD`（与参考实现一致，保证行为可预期）。
/// 更稳妥的做法是拿卷名去和 `/Volumes` 下的挂载名对比；这里刻意不做文件系统访问，
/// 好让转换保持纯函数、可在 CI 里测试。
fn convert_applescript_alias_to_posix(hfs_path: &str) -> Option<String> {
    let value = hfs_path.trim();
    if value.is_empty() {
        return None;
    }

    // HFS 路径用 ':' 分隔，因此路径段内部不会再出现 ':'，不需要反转义
    let parts: Vec<&str> = value.split(':').collect();
    if parts.len() <= 1 {
        return None;
    }

    let volume = parts[0];
    let rest = parts[1..].iter().copied().filter(|part| !part.is_empty());

    let is_boot_volume = volume.eq_ignore_ascii_case(BOOT_VOLUME_NAME);
    let mut segments: Vec<&str> = Vec::new();
    if !is_boot_volume {
        segments.push("Volumes");
        segments.push(volume);
    }
    segments.extend(rest);

    if segments.is_empty() {
        return Some("/".to_string());
    }
    Some(format!("/{}", segments.join("/")))
}

// ────────────────────────────────────────────────────────────────────────────
// Linux：argv 构造
// ────────────────────────────────────────────────────────────────────────────

/// `zenity --file-selection`（逐字，§7.3）
fn zenity_file_argv(title: &str, start: &str, label: &str) -> Vec<String> {
    vec![
        "zenity".to_string(),
        "--file-selection".to_string(),
        format!("--title={title}"),
        format!("--filename={}", ensure_trailing_separator(start)),
        format!("--file-filter={label} | {MEDIA_PATTERNS}"),
        "--file-filter=所有文件 | *".to_string(),
    ]
}

/// `zenity --file-selection --directory`（逐字，§7.3）
fn zenity_folder_argv(title: &str, start: &str) -> Vec<String> {
    vec![
        "zenity".to_string(),
        "--file-selection".to_string(),
        "--directory".to_string(),
        format!("--title={title}"),
        format!("--filename={}", ensure_trailing_separator(start)),
    ]
}

/// `kdialog --getopenfilename`（§7.3：过滤器形如 `描述 | 模式`）
///
/// PORT NOTE: 参考实现（`FileDialogService.cs:216`）只传了模式（没有 `描述 |` 前缀），
/// 规格 §7.3 写的是 `"媒体文件 | *.mp4 … *.jpg"`，这里按规格传完整过滤器。
fn kdialog_file_argv(title: &str, start: &str, label: &str) -> Vec<String> {
    vec![
        "kdialog".to_string(),
        "--getopenfilename".to_string(),
        ensure_trailing_separator(start),
        format!("{label} | {MEDIA_PATTERNS}"),
        "--title".to_string(),
        title.to_string(),
    ]
}

/// `kdialog --getexistingdirectory`（逐字，§7.3）
fn kdialog_folder_argv(title: &str, start: &str) -> Vec<String> {
    vec![
        "kdialog".to_string(),
        "--getexistingdirectory".to_string(),
        ensure_trailing_separator(start),
        "--title".to_string(),
        title.to_string(),
    ]
}

// ────────────────────────────────────────────────────────────────────────────
// Windows：PowerShell 脚本与 argv 构造
// ────────────────────────────────────────────────────────────────────────────

/// WinForms `OpenFileDialog` 脚本（逐字，§7.6）
fn windows_file_script(title: &str, start: &str, filter_label: &str) -> String {
    let literal_start = powershell_literal(start);
    let label = escape_powershell_single_quotes(filter_label);
    let lines = [
        "Add-Type -AssemblyName System.Windows.Forms".to_string(),
        "$dialog = New-Object System.Windows.Forms.OpenFileDialog".to_string(),
        format!("$dialog.Title = {}", powershell_literal(title)),
        format!("$dialog.Filter = '{label}|{MEDIA_PATTERNS_POWERSHELL}|所有文件|*.*'"),
        "$dialog.CheckFileExists = $true".to_string(),
        "$dialog.CheckPathExists = $true".to_string(),
        format!("if (Test-Path -LiteralPath {literal_start}) {{ $dialog.InitialDirectory = {literal_start} }}"),
        "if ($dialog.ShowDialog() -eq [System.Windows.Forms.DialogResult]::OK) { Write-Output $dialog.FileName }"
            .to_string(),
    ];
    format!("{}\n", lines.join("\n"))
}

/// WinForms `FolderBrowserDialog` 脚本（逐字，§7.6）
fn windows_folder_script(title: &str, start: &str) -> String {
    let literal_start = powershell_literal(start);
    let lines = [
        "Add-Type -AssemblyName System.Windows.Forms".to_string(),
        "$dialog = New-Object System.Windows.Forms.FolderBrowserDialog".to_string(),
        format!("$dialog.Description = {}", powershell_literal(title)),
        "$dialog.ShowNewFolderButton = $true".to_string(),
        format!("if (Test-Path -LiteralPath {literal_start}) {{ $dialog.SelectedPath = {literal_start} }}"),
        "if ($dialog.ShowDialog() -eq [System.Windows.Forms.DialogResult]::OK) { Write-Output $dialog.SelectedPath }"
            .to_string(),
    ];
    format!("{}\n", lines.join("\n"))
}

/// PowerShell 单引号字符串字面量（内部单引号翻倍），可安全承载中文与特殊字符
fn powershell_literal(text: &str) -> String {
    format!("'{}'", escape_powershell_single_quotes(text))
}

fn escape_powershell_single_quotes(text: &str) -> String {
    text.replace('\'', "''")
}

/// Windows 完整 argv（含程序名）：`{shell} -NoProfile -NonInteractive -STA -EncodedCommand {base64}`
fn windows_argv(shell: &str, script: &str) -> Vec<String> {
    vec![
        shell.to_string(),
        "-NoProfile".to_string(),
        "-NonInteractive".to_string(),
        "-STA".to_string(),
        "-EncodedCommand".to_string(),
        encode_powershell_command(script),
    ]
}

/// `前导语句 + 脚本` 的 UTF-16LE + Base64（用 `-EncodedCommand` 避免引号转义与中文乱码）
fn encode_powershell_command(script: &str) -> String {
    let text = format!("{POWERSHELL_PREAMBLE}{script}");
    let mut bytes = Vec::with_capacity(text.len() * 2);
    for unit in text.encode_utf16() {
        bytes.extend_from_slice(&unit.to_le_bytes());
    }
    base64_encode(&bytes)
}

/// 标准 Base64 编码（不引入新依赖；PowerShell 用 `-EncodedCommand` 需要它）
fn base64_encode(data: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = chunk.get(1).copied().unwrap_or(0) as u32;
        let b2 = chunk.get(2).copied().unwrap_or(0) as u32;
        let triple = (b0 << 16) | (b1 << 8) | b2;

        out.push(ALPHABET[(triple >> 18) as usize & 0x3F] as char);
        out.push(ALPHABET[(triple >> 12) as usize & 0x3F] as char);
        out.push(if chunk.len() > 1 { ALPHABET[(triple >> 6) as usize & 0x3F] as char } else { '=' });
        out.push(if chunk.len() > 2 { ALPHABET[triple as usize & 0x3F] as char } else { '=' });
    }
    out
}

// ────────────────────────────────────────────────────────────────────────────
// 公共：路径规整与子进程执行
// ────────────────────────────────────────────────────────────────────────────

/// 起始目录（对应 `ResolveStartDirectory`，§7.5）：传入非空且目录存在 → 绝对路径；
/// 否则 → 默认输出目录（系统「视频」目录 → 主目录 → 临时目录）。
fn resolve_start_directory(initial_directory: Option<&str>) -> String {
    if let Some(directory) = initial_directory {
        if !directory.trim().is_empty() {
            let absolute = crate::paths::to_absolute(directory);
            if absolute.is_dir() {
                return absolute.to_string_lossy().to_string();
            }
        }
    }
    crate::paths::default_output_directory().to_string_lossy().to_string()
}

/// 保证以当前平台的目录分隔符结尾（对应 `EnsureTrailingSeparator`，§7.5：
/// 空串原样返回；只判断平台分隔符，不判断 `/`）
fn ensure_trailing_separator(path: &str) -> String {
    if path.is_empty() {
        return String::new();
    }
    if path.ends_with(std::path::MAIN_SEPARATOR) {
        path.to_string()
    } else {
        format!("{path}{}", std::path::MAIN_SEPARATOR)
    }
}

/// 过滤器显示名：去空白、剔除会破坏过滤器结构的 `|`；空则用默认值 `媒体文件`
fn filter_label(filter: Option<&str>) -> String {
    let label = filter
        .unwrap_or_default()
        .replace('|', "")
        .replace('\r', " ")
        .replace('\n', " ")
        .trim()
        .to_string();
    if label.is_empty() { DEFAULT_FILTER_LABEL.to_string() } else { label }
}

/// 子进程执行结果
enum HelperOutcome {
    /// 正常退出（退出码 0）
    Output(String),
    /// 用户取消（辅助程序以「取消」退出码退出）
    Cancelled,
    /// 超时（子进程已被杀掉）
    Timeout,
    /// 启动失败或异常退出（携带中文原因）
    Failed(String),
}

/// 以 argv 形式启动辅助程序并等待退出。
///
/// - 关闭 stdin、重定向 stdout/stderr（与参考实现一致）；
/// - `DIALOG_TIMEOUT` 超时 → 杀掉子进程（`start_kill`；`kill_on_drop` 兜底）→ `Timeout`；
/// - 退出码 `cancel_exit_code` → `Cancelled`（zenity/kdialog 的 1）；
/// - 其余非 0 退出码 → `Failed`（带上 stderr 首行，便于界面提示）。
async fn run_helper(argv: &[String], cancel_exit_code: Option<i32>) -> HelperOutcome {
    let Some((program, args)) = argv.split_first() else {
        return HelperOutcome::Failed("对话框命令为空".to_string());
    };

    let mut command = Command::new(program);
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);

    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => return HelperOutcome::Failed(format!("无法启动「{program}」：{error}")),
    };

    // 两个管道必须并发读取，否则输出超过管道缓冲区时子进程会卡死
    let stdout_task = child.stdout.take().map(|pipe| tokio::spawn(read_to_end(pipe)));
    let stderr_task = child.stderr.take().map(|pipe| tokio::spawn(read_to_end(pipe)));

    let status = match tokio::time::timeout(DIALOG_TIMEOUT, child.wait()).await {
        Ok(Ok(status)) => status,
        Ok(Err(error)) => return HelperOutcome::Failed(format!("等待「{program}」退出失败：{error}")),
        Err(_) => {
            // 超时：对应参考实现的 Kill(entireProcessTree)。
            // PORT NOTE: tokio 只能杀直接子进程（这些辅助程序本身不再拉子进程）；
            // 另外 `kill_on_drop(true)` 作为 future 被丢弃时的兜底。
            let _ = child.start_kill();
            let _ = child.wait().await;
            return HelperOutcome::Timeout;
        }
    };

    let stdout = join_stream(stdout_task).await;
    let stderr = join_stream(stderr_task).await;

    if status.success() {
        return HelperOutcome::Output(stdout);
    }
    if let Some(code) = status.code() {
        if cancel_exit_code == Some(code) {
            return HelperOutcome::Cancelled;
        }
    }

    let code = status.code().map(|c| c.to_string()).unwrap_or_else(|| "未知".to_string());
    let detail = first_line(&stderr);
    HelperOutcome::Failed(if detail.is_empty() {
        format!("{program} 异常退出（退出码 {code}）")
    } else {
        format!("{program} 异常退出（退出码 {code}）：{detail}")
    })
}

async fn read_to_end<R: tokio::io::AsyncRead + Unpin>(mut pipe: R) -> String {
    let mut buffer = Vec::new();
    let _ = pipe.read_to_end(&mut buffer).await;
    String::from_utf8_lossy(&buffer).to_string()
}

async fn join_stream(task: Option<tokio::task::JoinHandle<String>>) -> String {
    match task {
        Some(task) => task.await.unwrap_or_default(),
        None => String::new(),
    }
}

/// 输出里的第一条非空行（用于失败时的中文提示，最长 200 字符）
fn first_line(text: &str) -> String {
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        return line.chars().take(200).collect();
    }
    String::new()
}

/// 子进程结果 → 接口结果
fn outcome_to_result(outcome: HelperOutcome) -> DialogResult {
    match outcome {
        HelperOutcome::Output(stdout) => match normalize_result(&stdout) {
            Some(path) => path_result(path),
            // stdout 为空 / 含 "User canceled"：用户取消（§7 的 `NormalizeResult`）
            None => DialogResult::cancelled(),
        },
        HelperOutcome::Cancelled => DialogResult::cancelled(),
        HelperOutcome::Timeout => failed_result("timeout".to_string()),
        HelperOutcome::Failed(message) => failed_result(message),
    }
}

fn path_result(path: String) -> DialogResult {
    DialogResult { path: Some(path), reason: None }
}

fn failed_result(reason: String) -> DialogResult {
    DialogResult { path: None, reason: Some(reason) }
}

/// 规整辅助程序的输出（对应 `NormalizeResult`，`FileDialogService.cs:381-403`）。
///
/// 返回 `None` 表示「用户取消」（空输出或含 `User canceled`）。
fn normalize_result(raw: &str) -> Option<String> {
    let mut value = raw.trim().to_string();
    if value.is_empty() {
        return None;
    }
    if value.to_ascii_lowercase().contains("user canceled") {
        return None;
    }

    // 兜底：万一某个 macOS / AppleScript 组合仍然把结果序列化成文件引用字符串
    // （实测会以 alias / file / folder / disk / document file 开头，后面跟 HFS 路径），
    // 在这里就转成 POSIX 路径，绝不让 "alias Macintosh HD:Users:…" 流到界面上去。
    let lower = value.to_ascii_lowercase();
    for prefix in APPLESCRIPT_REFERENCE_PREFIXES {
        if !lower.starts_with(prefix) {
            continue;
        }
        value = convert_applescript_alias_to_posix(&value[prefix.len()..])?;
        break;
    }

    // 目录会带末尾斜杠，去掉更符合直觉（根目录除外）
    let trimmed = trim_trailing_separator(&value);
    if trimmed.is_empty() { None } else { Some(trimmed) }
}

/// 去掉末尾目录分隔符（长度 > 1 时；根目录 `/` 保持）
fn trim_trailing_separator(path: &str) -> String {
    if path.chars().count() <= 1 {
        return path.to_string();
    }
    // 与原实现一致：只处理当前平台的目录分隔符（Unix 只有 `/`，Windows 还有 `\`）
    let trimmed = path.trim_end_matches(SEPARATORS);
    if trimmed.is_empty() { path.to_string() } else { trimmed.to_string() }
}

#[cfg(unix)]
const SEPARATORS: &[char] = &['/'];
#[cfg(windows)]
const SEPARATORS: &[char] = &['/', '\\'];

#[cfg(test)]
mod tests {
    use super::*;

    // ────────────────────────────────────────────────────────────────
    // AppleScript alias（HFS）→ POSIX
    // ────────────────────────────────────────────────────────────────

    #[test]
    fn alias_conversion_matches_spec_examples() {
        assert_eq!(
            convert_applescript_alias_to_posix("Macintosh HD:Users:me:Movies:25 4K.mp4").as_deref(),
            Some("/Users/me/Movies/25 4K.mp4")
        );
        // 卷名比较忽略大小写
        assert_eq!(
            convert_applescript_alias_to_posix("macintosh hd:Users:me:a.mp4").as_deref(),
            Some("/Users/me/a.mp4")
        );
        // 外接卷
        assert_eq!(
            convert_applescript_alias_to_posix("MySSD:Movies:a.mp4").as_deref(),
            Some("/Volumes/MySSD/Movies/a.mp4")
        );
        // 空段被过滤
        assert_eq!(
            convert_applescript_alias_to_posix("Macintosh HD::Users::me:a.mp4").as_deref(),
            Some("/Users/me/a.mp4")
        );
        // 前缀里的空白被 trim
        assert_eq!(convert_applescript_alias_to_posix("  Macintosh HD:Users:me  ").as_deref(), Some("/Users/me"));
    }

    #[test]
    fn alias_conversion_rejects_non_paths() {
        assert_eq!(convert_applescript_alias_to_posix(""), None);
        assert_eq!(convert_applescript_alias_to_posix("   "), None);
        // 只有卷名（没有分隔符）不是路径
        assert_eq!(convert_applescript_alias_to_posix("Macintosh HD"), None);
        // 卷名 + 空路径：启动卷 → 根目录
        assert_eq!(convert_applescript_alias_to_posix("Macintosh HD:").as_deref(), Some("/"));
        assert_eq!(convert_applescript_alias_to_posix("MySSD:").as_deref(), Some("/Volumes/MySSD"));
    }

    #[test]
    fn normalize_converts_alias_prefix_and_detects_cancel() {
        // 正常路径
        assert_eq!(normalize_result("/Users/me/Movies/a.mp4\n").as_deref(), Some("/Users/me/Movies/a.mp4"));
        // 兜底：alias 前缀（忽略大小写）+ HFS 路径
        assert_eq!(
            normalize_result("alias Macintosh HD:Users:me:Movies:a.mp4\n").as_deref(),
            Some("/Users/me/Movies/a.mp4")
        );
        assert_eq!(
            normalize_result("ALIAS Macintosh HD:Users:me:Movies:a.mp4").as_deref(),
            Some("/Users/me/Movies/a.mp4")
        );
        assert_eq!(normalize_result("file MySSD:Movies:a.mp4").as_deref(), Some("/Volumes/MySSD/Movies/a.mp4"));
        // 目录末尾斜杠被去掉，但根目录保持
        assert_eq!(normalize_result("/Users/me/Movies/").as_deref(), Some("/Users/me/Movies"));
        assert_eq!(normalize_result("/").as_deref(), Some("/"));
        // 取消：空输出 或 含 "User canceled"（忽略大小写）
        assert_eq!(normalize_result(""), None);
        assert_eq!(normalize_result("   \r\n\t "), None);
        assert_eq!(normalize_result("User canceled"), None);
        assert_eq!(normalize_result("user CANCELED.\n"), None);
    }

    // ────────────────────────────────────────────────────────────────
    // macOS：逐字脚本 + argv
    // ────────────────────────────────────────────────────────────────

    const MAC_FILE_WITH_START: &str = r#"set theResult to ""
try
    tell application "System Events" to set frontmost of first process whose frontmost is true to true
end try
try
    set theResult to POSIX path of (choose file with prompt "选择文件" default location POSIX file "/Users/me/Movies")
on error errMsg number errNum
    if errNum is -128 then return ""
    try
        set theResult to POSIX path of (choose file with prompt "选择文件")
    on error
        return ""
    end try
end try
return theResult"#;

    const MAC_FOLDER_NO_START: &str = r#"set theResult to ""
try
    tell application "System Events" to set frontmost of first process whose frontmost is true to true
end try
try
    set theResult to POSIX path of (choose folder with prompt "选择文件夹")
on error
    return ""
end try
return theResult"#;

    #[test]
    fn macos_script_is_verbatim_reference() {
        assert_eq!(
            build_macos_script(DialogKind::File, "选择文件", "/Users/me/Movies"),
            MAC_FILE_WITH_START
        );
        assert_eq!(build_macos_script(DialogKind::Folder, "选择文件夹", "   "), MAC_FOLDER_NO_START);
    }

    #[test]
    fn macos_script_escapes_backslash_and_quote() {
        let script = build_macos_script(DialogKind::File, "a\"b\\c", "/tmp/x\"y");
        assert!(script.contains(r#"with prompt "a\"b\\c" default location"#), "{script}");
        assert!(script.contains(r#"POSIX file "/tmp/x\"y""#), "{script}");
        // 兜底分支里的第二个 prompt 也必须是转义后的值
        assert_eq!(script.matches(r#"prompt "a\"b\\c""#).count(), 2);
    }

    #[test]
    fn macos_argv_is_osascript_with_dash_e() {
        let script = build_macos_script(DialogKind::File, "选择文件", "/Users/me/Movies");
        assert_eq!(
            macos_argv(&script),
            vec![
                "/usr/bin/osascript".to_string(),
                "-e".to_string(),
                MAC_FILE_WITH_START.to_string(),
            ]
        );
    }

    /// 生成的 AppleScript 必须能通过 osacompile 语法检查（macOS 上真正执行；缺 osacompile 时跳过）
    #[cfg(target_os = "macos")]
    #[test]
    fn generated_applescript_compiles_with_osacompile() {
        if !helper_exists("osacompile") {
            eprintln!("跳过：本机没有 osacompile");
            return;
        }

        let cases = [
            build_macos_script(DialogKind::File, "选择文件", "/Users/me/Movies"),
            build_macos_script(DialogKind::File, "选择文件", ""),
            build_macos_script(DialogKind::Folder, "选择包含媒体文件的文件夹", "/Users/me/Movies"),
            build_macos_script(DialogKind::Folder, "选择文件夹", "  "),
            // 转义后的引号/反斜杠也必须合法
            build_macos_script(DialogKind::File, "a\"b\\c", "/tmp/x\"y"),
        ];

        for script in cases {
            let output = std::process::Command::new("osacompile")
                .arg("-o")
                .arg("/dev/null")
                .arg("-e")
                .arg(&script)
                .output()
                .expect("无法执行 osacompile");
            assert!(
                output.status.success(),
                "osacompile 拒绝了生成的脚本（退出码 {:?}）：\n{}\n---\n{}",
                output.status.code(),
                String::from_utf8_lossy(&output.stderr),
                script
            );
        }
    }

    // ────────────────────────────────────────────────────────────────
    // Linux：argv 逐项断言
    // ────────────────────────────────────────────────────────────────

    #[test]
    fn zenity_argv_matches_spec() {
        assert_eq!(
            zenity_file_argv("选择文件", "/home/me/Movies", DEFAULT_FILTER_LABEL),
            vec![
                "zenity".to_string(),
                "--file-selection".to_string(),
                "--title=选择文件".to_string(),
                "--filename=/home/me/Movies/".to_string(),
                format!("--file-filter=媒体文件 | {MEDIA_PATTERNS}"),
                "--file-filter=所有文件 | *".to_string(),
            ]
        );
        assert_eq!(
            zenity_folder_argv("选择文件夹", "/home/me/Movies"),
            vec![
                "zenity".to_string(),
                "--file-selection".to_string(),
                "--directory".to_string(),
                "--title=选择文件夹".to_string(),
                "--filename=/home/me/Movies/".to_string(),
            ]
        );
        // 起始目录为空时 `--filename=` 保持空（不给 zenity 伪造一个绝对路径）
        assert_eq!(zenity_folder_argv("t", "")[4], "--filename=");
    }

    #[test]
    fn kdialog_argv_matches_spec() {
        assert_eq!(
            kdialog_file_argv("选择文件", "/home/me/Movies", DEFAULT_FILTER_LABEL),
            vec![
                "kdialog".to_string(),
                "--getopenfilename".to_string(),
                "/home/me/Movies/".to_string(),
                format!("媒体文件 | {MEDIA_PATTERNS}"),
                "--title".to_string(),
                "选择文件".to_string(),
            ]
        );
        assert_eq!(
            kdialog_folder_argv("选择文件夹", ""),
            vec![
                "kdialog".to_string(),
                "--getexistingdirectory".to_string(),
                "".to_string(),
                "--title".to_string(),
                "选择文件夹".to_string(),
            ]
        );
    }

    #[test]
    fn media_patterns_are_verbatim_and_cover_both_forms() {
        assert_eq!(
            MEDIA_PATTERNS,
            "*.mp4 *.mkv *.avi *.mov *.wmv *.flv *.webm *.m4v *.ts *.mpg *.mp3 *.wav *.flac *.aac *.ogg \
*.m4a *.opus *.gif *.png *.jpg"
        );
        // 两种形式的扩展名顺序必须完全一致
        let spaced: Vec<&str> = MEDIA_PATTERNS.split(' ').collect();
        let semicolon: Vec<&str> = MEDIA_PATTERNS_POWERSHELL.split(';').collect();
        assert_eq!(spaced, semicolon);
    }

    // ────────────────────────────────────────────────────────────────
    // Windows：脚本 + UTF-16LE/Base64 编码 argv
    // ────────────────────────────────────────────────────────────────

    /// 测试专用的 Base64 解码（与被测编码器相互独立）
    fn base64_decode(text: &str) -> Vec<u8> {
        const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut bits = 0u32;
        let mut bit_count = 0u32;
        let mut out = Vec::new();
        for byte in text.bytes() {
            if byte == b'=' {
                break;
            }
            let value = ALPHABET.iter().position(|c| *c == byte).expect("非法 Base64 字符") as u32;
            bits = (bits << 6) | value;
            bit_count += 6;
            if bit_count >= 8 {
                bit_count -= 8;
                out.push((bits >> bit_count) as u8);
            }
        }
        out
    }

    fn decode_utf16le(bytes: &[u8]) -> String {
        assert_eq!(bytes.len() % 2, 0, "UTF-16LE 字节数必须是偶数");
        let units: Vec<u16> = bytes.chunks_exact(2).map(|pair| u16::from_le_bytes([pair[0], pair[1]])).collect();
        String::from_utf16(&units).expect("不是合法的 UTF-16")
    }

    #[test]
    fn windows_script_is_verbatim_reference() {
        let expected = format!(
            "Add-Type -AssemblyName System.Windows.Forms\n\
             $dialog = New-Object System.Windows.Forms.OpenFileDialog\n\
             $dialog.Title = '选择文件'\n\
             $dialog.Filter = '媒体文件|{MEDIA_PATTERNS_POWERSHELL}|所有文件|*.*'\n\
             $dialog.CheckFileExists = $true\n\
             $dialog.CheckPathExists = $true\n\
             if (Test-Path -LiteralPath '/Users/me/Movies') {{ $dialog.InitialDirectory = '/Users/me/Movies' }}\n\
             if ($dialog.ShowDialog() -eq [System.Windows.Forms.DialogResult]::OK) {{ Write-Output $dialog.FileName }}\n"
        );
        assert_eq!(windows_file_script("选择文件", "/Users/me/Movies", DEFAULT_FILTER_LABEL), expected);

        let expected_folder = concat!(
            "Add-Type -AssemblyName System.Windows.Forms\n",
            "$dialog = New-Object System.Windows.Forms.FolderBrowserDialog\n",
            "$dialog.Description = '选择文件夹'\n",
            "$dialog.ShowNewFolderButton = $true\n",
            "if (Test-Path -LiteralPath '/Users/me/Movies') { $dialog.SelectedPath = '/Users/me/Movies' }\n",
            "if ($dialog.ShowDialog() -eq [System.Windows.Forms.DialogResult]::OK) { Write-Output $dialog.SelectedPath }\n",
        );
        assert_eq!(windows_folder_script("选择文件夹", "/Users/me/Movies"), expected_folder);
    }

    #[test]
    fn powershell_literal_doubles_single_quotes() {
        assert_eq!(powershell_literal("it's"), "'it''s'");
        assert_eq!(powershell_literal("中文'路径"), "'中文''路径'");
    }

    #[test]
    fn windows_argv_is_encoded_command_with_utf16le_script() {
        let script = windows_file_script("选择文件", "/Users/me/Movies", DEFAULT_FILTER_LABEL);
        let argv = windows_argv("powershell.exe", &script);

        assert_eq!(argv.len(), 6);
        assert_eq!(
            &argv[..5],
            &[
                "powershell.exe".to_string(),
                "-NoProfile".to_string(),
                "-NonInteractive".to_string(),
                "-STA".to_string(),
                "-EncodedCommand".to_string(),
            ]
        );

        // 解码回来必须是「前导语句 + 脚本」的 UTF-16LE 原文
        let decoded = decode_utf16le(&base64_decode(&argv[5]));
        assert_eq!(decoded, format!("{POWERSHELL_PREAMBLE}{script}"));
        assert!(decoded.contains("中文") || decoded.contains("选择文件"), "中文必须原样保留：\n{decoded}");
    }

    #[test]
    fn base64_encoding_matches_known_vectors() {
        // RFC 4648 的经典向量（含三种补位情况），并顺带验证 UTF-16LE 时的行为
        assert_eq!(base64_encode(b""), "");
        assert_eq!(base64_encode(b"f"), "Zg==");
        assert_eq!(base64_encode(b"fo"), "Zm8=");
        assert_eq!(base64_encode(b"foo"), "Zm9v");
        assert_eq!(base64_encode(b"foob"), "Zm9vYg==");
        assert_eq!(base64_encode(b"fooba"), "Zm9vYmE=");
        assert_eq!(base64_encode(b"foobar"), "Zm9vYmFy");
        assert_eq!(base64_encode(&[0xFF, 0xFE]), "//4=");
    }

    // ────────────────────────────────────────────────────────────────
    // 后端描述与「不可用」分支
    // ────────────────────────────────────────────────────────────────

    #[test]
    fn backend_descriptions_match_reference() {
        let mac = resolve_backend(Platform::MacOs, &|name| name == "osascript");
        assert!(mac.is_supported);
        assert_eq!(mac.backend, "macOS 原生选择器（osascript）");

        let windows = resolve_backend(Platform::Windows, &|name| name == "pwsh");
        assert!(windows.is_supported);
        assert_eq!(windows.backend, "Windows 文件对话框（PowerShell）");

        let zenity = resolve_backend(Platform::Linux, &|name| name == "zenity");
        assert!(zenity.is_supported);
        assert_eq!(zenity.backend, "zenity");

        let kdialog = resolve_backend(Platform::Linux, &|name| name == "kdialog");
        assert!(kdialog.is_supported);
        assert_eq!(kdialog.backend, "kdialog");
    }

    #[test]
    fn unavailable_backends_are_reported_as_unsupported() {
        let linux = resolve_backend(Platform::Linux, &|_| false);
        assert!(!linux.is_supported);
        assert_eq!(linux.backend, LINUX_UNAVAILABLE);
        assert_eq!(linux.backend, "不可用（未安装 zenity 或 kdialog）");

        // PORT NOTE: 参考实现把 Windows/macOS 恒判为可用；这里探测失败即报不可用
        assert!(!resolve_backend(Platform::MacOs, &|_| false).is_supported);
        assert!(!resolve_backend(Platform::Windows, &|_| false).is_supported);
    }

    /// 不可用后端必须**不启动任何子进程**就返回 `unsupported`
    #[tokio::test]
    async fn unsupported_backend_short_circuits() {
        for backend in [
            resolve_backend(Platform::Linux, &|_| false),
            resolve_backend(Platform::MacOs, &|_| false),
            resolve_backend(Platform::Windows, &|_| false),
        ] {
            for request in [DialogRequest::file(), DialogRequest::folder()] {
                let result = pick_with_backend(request, &backend).await;
                assert_eq!(result.path, None);
                assert_eq!(result.reason.as_deref(), Some(format!("unsupported:{}", backend.backend).as_str()));
            }
        }
    }

    #[test]
    fn backend_on_this_machine_is_usable() {
        let backend = backend();
        assert!(!backend.backend.is_empty());
        #[cfg(target_os = "macos")]
        {
            assert!(backend.is_supported, "macOS 自带 osascript，必须判定可用");
            assert_eq!(backend.backend, "macOS 原生选择器（osascript）");
        }
    }

    // ────────────────────────────────────────────────────────────────
    // 起始目录 / 过滤器 / 分隔符
    // ────────────────────────────────────────────────────────────────

    #[test]
    fn start_directory_falls_back_to_default_output_directory() {
        // 不存在的目录 → 默认输出目录（非空且绝对）
        let fallback = resolve_start_directory(Some("/definitely/not/here/at/all"));
        assert_eq!(fallback, crate::paths::default_output_directory().to_string_lossy());
        assert!(Path::new(&fallback).is_absolute());
        assert_eq!(resolve_start_directory(None), fallback);
        assert_eq!(resolve_start_directory(Some("   ")), fallback);

        // 存在的目录 → 词法规整成绝对路径
        let dir = std::env::temp_dir().join(format!("ffr-dialog-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(resolve_start_directory(Some(&dir.to_string_lossy())), dir.to_string_lossy());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn trailing_separator_helpers_match_reference() {
        assert_eq!(ensure_trailing_separator(""), "");
        assert_eq!(ensure_trailing_separator("/a/"), "/a/");
        let with_separator = format!("/a{}", std::path::MAIN_SEPARATOR);
        assert_eq!(ensure_trailing_separator("/a"), with_separator);

        assert_eq!(trim_trailing_separator("/a/"), "/a");
        assert_eq!(trim_trailing_separator("/"), "/");
        assert_eq!(trim_trailing_separator("rel/"), "rel");
    }

    #[test]
    fn filter_label_defaults_and_is_sanitized() {
        assert_eq!(filter_label(None), "媒体文件");
        assert_eq!(filter_label(Some("  ")), "媒体文件");
        assert_eq!(filter_label(Some("视频文件")), "视频文件");
        // `|` 会破坏过滤器结构，必须剔除
        assert_eq!(filter_label(Some("允许的文件|*.mp4")), "允许的文件*.mp4");
    }
}
