# FFmpeg Core 行为规格（从 C#/.NET Blazor 参考实现提取）

> 目的：让 Rust 开发者**不读 C# 源码**就能 1:1 复刻参考实现中「FFmpeg 定位 / 能力探测 / 媒体探测 / 进程执行与进度 / 硬件编码器检测 / 编码器参数画像」的全部行为。
>
> 参考项目根目录：`/Users/c01h/code/github/FFmpegWebUI/`
> 本文所有 `路径:行号` 均相对该根目录（即 `Services/FFmpegService.cs:159` = `/Users/c01h/code/github/FFmpegWebUI/Services/FFmpegService.cs` 第 159 行）。
>
> 覆盖的源文件（全部已逐行读取）：
>
> | 文件 | 行数 | 本文对应章节 |
> | --- | --- | --- |
> | `Services/FFmpegLocator.cs` | 261 | §2 |
> | `Services/FFmpegService.cs` | 1042 | §3 |
> | `Services/IHardwareDetectionService.cs` | 33 | §5.1 |
> | `Services/HardwareDetectionService.cs` | 559 | §5 |
> | `Services/EncoderCatalog.cs` | 378 | §6 |
> | `Services/AppInfo.cs` | 17 | §1.4 |
>
> 为把行为写死，还读取了被上述文件直接依赖的定义文件（不属于上述 6 个文件，但字段语义必须一并移植）：
> `Services/AppPaths.cs`（143 行）、`Models/DTOs.cs`（137）、`Models/HardwareEncoder.cs`（87）、`Models/Enums.cs`（107）、`Models/AppSettings.cs`（84）、`Services/CommandLineBuilder.cs`（489，仅取 `Tokenize` / `RenderCommandLine` / 占位符展开）、`Program.cs`（DI 生命周期）、`FFmpegWebUI.csproj:37-47`（内置二进制打包）。

---

## §0 术语与总体结构

| 组件 | C# 类型 | DI 生命周期 | 职责 |
| --- | --- | --- | --- |
| 定位器 | `FFmpegLocator : IFFmpegLocator` | **Singleton**（`Program.cs:53`） | 解析 ffmpeg / ffprobe 可执行文件绝对路径，带进程内缓存 |
| 执行器 | `FFmpegService : IFFmpegService, IDisposable` | **Singleton**（`Program.cs:59`） | 版本/能力探测、ffprobe 媒体探测、ffmpeg 子进程执行与进度解析 |
| 能力模型 | `FFmpegInfo`（record，`Models/DTOs.cs:6-24`） | 随探测结果 | 版本号 + 编译期支持清单 |
| 硬件检测 | `HardwareDetectionService : IHardwareDetectionService` | **Singleton**（`Program.cs:60`） | 平台过滤 → 构建清单过滤 → 试跑 → 报告 + 双层缓存 |
| 编码器画像 | `EncoderCatalog` / `EncoderProfile`（static / record） | 静态 | 31 条编码器参数画像（质量参数 / 预设 / 上传滤镜 / 设备参数 / 容器参数） |
| 应用元信息 | `AppInfo`（static） | 静态 | 名称 / 版本 / 端口基址 |

全局线程安全约定：所有可变缓存都用 `lock`（C# 13 `Lock` 类型）或 `ConcurrentDictionary` 保护；`HardwareDetectionService` 的整段检测用 `SemaphoreSlim(1,1)` 串行化，并做「进入锁前 + 进入锁后」两次缓存检查（double-checked locking）。

---

## §1 平台与路径基础设施（前置依赖，必须一起移植）

### 1.1 平台判定（`Services/AppPaths.cs:9-34`）

```text
IsWindows = OperatingSystem.IsWindows()
IsMacOS   = OperatingSystem.IsMacOS()
IsLinux   = OperatingSystem.IsLinux()
Platform  = Windows | OSX | Linux（其余一律当 Linux 处理）
PlatformDisplayName = "Windows" | "macOS" | "Linux" | "未知系统"（最后一项在 .NET 上不可达）
ExeSuffix = IsWindows ? ".exe" : ""          // macOS/Linux 均为空串
IsContainer = IsLinux && (File.Exists("/.dockerenv") || env DOTNET_RUNNING_IN_CONTAINER == "true")
```

Rust 对应：`cfg!(target_os = "windows")` / `"macos"` / `"linux"`；`EXE_SUFFIX = if windows {".exe"} else {""}`。注意 macOS 不加后缀。

### 1.2 路径规整 `AppPaths.ToAbsolute`（`Services/AppPaths.cs:117-130`）

```text
ToAbsolute(path):
  if path 为空或全空白 -> 原样返回
  expanded = ExpandHome(path.trim().trim('"'))      // 去掉首尾引号 + 展开 ~
  try: return realpath/GetFullPath(expanded)         // 不要求文件存在
  catch: return expanded
```

`ExpandHome`（`:89-101`）：仅当字符串等于 `~`、或以 `~/`、`~\` 开头时，替换为用户主目录（`UserProfile`）后 `Path.GetFullPath` 拼接；否则原样返回。

### 1.3 应用数据目录（`Services/AppPaths.cs:37-71`）—— 与定位器无关，但影响设置持久化位置

```text
DataDirectory:
  1) env FFMPEGWEBUI_DATA_DIR 非空 -> EnsureDirectory(ExpandHome(值))     // 唯一的 FFmpegWebUI 专属环境变量
  2) Windows : %LOCALAPPDATA%
     macOS   : ~/Library/Application Support
     Linux   : $XDG_DATA_HOME 或 ~/.local/share
  3) 若 root 为空 -> AppContext.BaseDirectory（即 exe 所在目录）
  最终 = EnsureDirectory(root + "/FFmpegWebUI")
```

数据库：LiteDB 单文件单例（`Program.cs:52`）；设置集合 `Settings`、编码器缓存集合名为 `"encoders"`（`HardwareDetectionService.cs:21`）。

### 1.4 应用元信息 `Services/AppInfo.cs:1-17`（全文）

```csharp
public static class AppInfo
{
    public const string Name = "FFmpeg WebUI";          // L7
    public const string Version = "2.0.0";              // L10
    public static string FullVersion => $"{Name} v{Version} · {AppPaths.PlatformDisplayName} · .NET {Environment.Version}";  // L13
    public static string BaseUrl(int port) => $"http://127.0.0.1:{port}";  // L16
}
```

Rust 移植：`FullVersion` 的 `·` 是 U+00B7 中点，两侧各一个半角空格；`.NET {Environment.Version}` 部分应替换为 Rust/构建信息（**这是一处必须做的差异化决策**）。`BaseUrl` 固定 `http://127.0.0.1:{port}`（只绑 loopback）。默认端口常量 `AppSettings.ListenPort = 5265`，`0` 表示自动挑选空闲端口（`Models/AppSettings.cs:64-65,73`）。

---

## §2 FFmpegLocator —— 可执行文件定位规格

> 源：`Services/FFmpegLocator.cs` 全文（261 行）。

### 2.1 解析结果形态（`FFmpegLocator.cs:7-17`）

```csharp
public record FFmpegPaths(string FFmpeg, string FFprobe, bool IsAutoDetected)
{
    public bool FFmpegResolved  { get; init; }      // 是否成功解析到实际存在的 ffmpeg
    public bool FFprobeResolved { get; init; }      // 是否成功解析到实际存在的 ffprobe
    public string Hint { get; init; } = "";         // 面向用户的解析说明
}
```

Rust 建议：

```rust
pub struct FfmpegPaths {
    pub ffmpeg: String,          // 可能是绝对路径，也可能是裸名 "ffmpeg"（交给子进程 PATH 解析）
    pub ffprobe: String,
    pub is_auto_detected: bool,  // false = 命中用户显式配置
    pub ffmpeg_resolved: bool,
    pub ffprobe_resolved: bool,
    pub hint: String,
}
```

**关键语义**：`ffmpeg` / `ffprobe` 字段永远是「可执行的字符串」，绝不返回 `Option`。找不到时返回裸名 `"ffmpeg"` / `"ffprobe"`（`:102-103`），由 `Command::new()` 继承 PATH 解析，从而让「后来才装好 FFmpeg」的场景仍有救；`*Resolved == false` 才是「找不到」的信号。

### 2.2 缓存语义（`FFmpegLocator.cs:35-54`）

```text
Resolve()  : lock { if cached != null return cached; cached = ResolveCore(); return cached }
Invalidate(): lock { cached = null }
```

* 进程内永久缓存，**没有 TTL**。
* 唯一失效点：`SettingsService` 保存设置后调用 `Locator.Invalidate()`（`Services/SettingsService.cs:82,104`）——即「设置写入成功」与「FFmpeg 能力缓存失效」是同两个事件（后者见 §3.8）。
* 定位器**读取数据库里「已保存」的 settings**，而不是注入 `ISettingsService`（`:58-61` 注释说明这是为了避免 DI 循环依赖）。

### 2.3 `ResolveCore()` 决策树（`FFmpegLocator.cs:56-111`）—— 逐字复刻

```text
settings = try { (db.Settings.FindAll().FirstOrDefault() ?? new AppSettings()).Normalize() }
           catch { new AppSettings() }              // 读库失败：用默认值，且【不】Normalize

# ── 分支 1：用户显式配置了 ffmpeg 路径 ─────────────────────────────
if settings.FFmpegPath 非空白:
    configured = AppPaths.ToAbsolute(settings.FFmpegPath)
    if File.Exists(configured) || IsOnPath(configured):        # IsOnPath 见 §2.6
        probe =
          if settings.FFprobePath 为空白:
              FindSiblingProbe(configured)                     # ffmpeg 同目录下 ffprobe(+后缀)
              ?? ProbeCandidates().FirstOrDefault(File.Exists)  # 见 §2.4 的 probe 专用候选序列
              ?? "ffprobe"                                      # 裸名兜底
          else:
              AppPaths.ToAbsolute(settings.FFprobePath)          # 不做存在性检查！
        return FFmpegPaths(configured, probe, IsAutoDetected=false) {
            FFmpegResolved  = true,                              # 因为是硬编码 true（即使只是裸名）
            FFprobeResolved = File.Exists(probe) || IsOnPath(probe),
            Hint = "使用设置中指定的路径"
        }

# ── 分支 2：只显式配置了 ffprobe ──────────────────────────────────
configuredProbe = null
if settings.FFprobePath 非空白:
    probe = AppPaths.ToAbsolute(settings.FFprobePath)
    if File.Exists(probe): configuredProbe = probe               # 只接受真实存在的文件

# ── 分支 3：自动探测 ─────────────────────────────────────────────
ffmpeg  = FindExecutable("ffmpeg")                               # 见 §2.4
ffprobe = configuredProbe ?? FindExecutable("ffprobe")
return FFmpegPaths(ffmpeg ?? "ffmpeg", ffprobe ?? "ffprobe", IsAutoDetected=true) {
    FFmpegResolved  = ffmpeg  != null,
    FFprobeResolved = ffprobe != null,
    Hint = ffmpeg != null ? "自动探测到 FFmpeg" : "未找到 FFmpeg，请在设置中指定路径"
}
```

**HINT 文案（逐字，界面会直接显示）**

| 场景 | `Hint` |
| --- | --- |
| 命中用户配置的 ffmpeg 路径 | `使用设置中指定的路径` |
| 自动探测到 ffmpeg | `自动探测到 FFmpeg` |
| 自动探测失败 | `未找到 FFmpeg，请在设置中指定路径` |

### 2.4 候选路径搜索顺序（`FFmpegLocator.cs:113-145`）

`FindExecutable(name)` 严格按 **三段顺序**，任一段命中立即返回（不做「谁更新/谁更优先」的择新）：

```text
# 第 1 段：随应用内置（相对 AppContext.BaseDirectory，即可执行文件所在目录）
for rel in BundledCandidates(name):            # 顺序即优先级
    candidate = baseDir / rel                  # 注意：拼接后【不】再做规范化
    if File.Exists(candidate): return candidate

# 第 2 段：系统 PATH（见 §2.5）
onPath = FindOnPath(name); if onPath != null: return onPath

# 第 3 段：平台常见安装位置（见 §2.7）
for candidate in WellKnownCandidates(name):
    if File.Exists(candidate): return candidate

return null
```

`BundledCandidates(name)`（`:137-145`）—— `exe = name + ExeSuffix`：

| 序号 | 相对路径（Windows 下 `<name>.exe`） |
| --- | --- |
| 1 | `<name>`（即 exe 同目录） |
| 2 | `ffmpeg/<name>` |
| 3 | `tools/<name>` |
| 4 | `tools/ffmpeg/<name>` |
| 5 | `runtimes/ffmpeg/<name>` |

**内置 `tools/ffmpeg` 的打包机制（`FFmpegWebUI.csproj:37-47`）**：把 `ffmpeg`、`ffprobe` 放到项目根 `tools/ffmpeg/` 下，构建时以 `Link="ffmpeg/%(RecursiveDir)%(Filename)%(Extension)"` 复制到输出/发布目录的 **`ffmpeg/` 子目录**。因此：
* 发布产物实际命中 **候选 2（`ffmpeg/<name>`）**；
* 候选 3/4/5 是为「直接把 tools/ 原样拷到输出目录」「开发期 `dotnet run`」「其他 RID 布局」保留的兼容位置。
* 参考仓库当前 `tools/` 下**没有** `ffmpeg/` 目录（只有 `screenshots/`、`verify/`），即默认不内置二进制。

Rust 移植提醒：Rust 二进制所在目录用 `std::env::current_exe()` 的父目录（`.NET` 的 `AppContext.BaseDirectory` 对单文件发布/符号链接的解析行为略有差异——**需要 Rust 侧自行拍定 `current_exe` 还是 `argv[0]`**）。

### 2.5 PATH 查找（`FFmpegLocator.cs:216-234`）

```text
pathVar = env("PATH"); if null/empty -> null
for dir in pathVar.split(Path.PathSeparator, RemoveEmptyEntries):   # ';' on Windows, ':' elsewhere
    try:
        candidate = dir.trim('"') / (name + ExeSuffix)
        if File.Exists(candidate): return candidate
    catch: 忽略（PATH 里的非法项）
return null
```

* 只读 `PATH`；**不**读取任何 `FFMPEG_*` 环境变量（参考实现里 FFmpeg 定位完全不看环境变量，只有 `FFMPEGWEBUI_DATA_DIR` 用于数据目录，`XDG_DATA_HOME` 用于 Linux 数据根）。
* 每段 `dir` 仅去掉首尾双引号（兼容 Windows `"C:\Program Files\..."` 式 PATH 项），不做 `~` 展开。
* 返回的是**文件绝对/相对拼接结果**（PATH 项是绝对路径，故通常为绝对路径）。

### 2.6 `IsOnPath` 判定（`FFmpegLocator.cs:237-239`）

```text
IsOnPath(value) = 既不含 Path.DirectorySeparatorChar，也不含 Path.AltDirectorySeparatorChar
```

即：值里没有任何目录分隔符 → 视为「可执行名，交给系统 PATH 解析」。Rust 侧用 `!value.contains(MAIN_SEPARATOR) && !value.contains(ALT_SEPARATOR)`（Windows 需同时判 `\` 与 `/`）。

### 2.7 平台常见安装位置（`FFmpegLocator.cs:147-214`）—— 精确顺序

拼装方式：`exe = name + ExeSuffix`；`home = UserProfile`。**顺序即优先级，命中即返回。**

**Windows（`:153-175`）**，共 8 条固定 + 2 条 home 相关：

```text
1  C:\ffmpeg\bin\<exe>
2  C:\ffmpeg\<exe>
3  <ProgramFiles>\ffmpeg\bin\<exe>
4  <ProgramFilesX86>\ffmpeg\bin\<exe>
5  <LocalAppData>\Microsoft\WinGet\Links\<exe>
6  C:\ProgramData\chocolatey\bin\<exe>
7  <LocalAppData>\Microsoft\WindowsApps\<exe>
8  <AppData>\ffmpeg\bin\<exe>
── 仅当 home 非空 ──
9  <home>\scoop\shims\<exe>
10 <home>\scoop\apps\ffmpeg\current\bin\<exe>
```

注意 1、2、6 是**硬编码字符串**（不随系统盘符变化），程序集上写的就是 `@"C:\ffmpeg\bin\"`。`<ProgramFiles>` 等来自 `Environment.SpecialFolder`。

**macOS（`:176-191`）**，6 条固定 + 2 条 home 相关：

```text
1 /opt/homebrew/bin/<exe>          # Apple Silicon Homebrew
2 /usr/local/bin/<exe>             # Intel Homebrew
3 /opt/local/bin/<exe>             # MacPorts
4 /usr/bin/<exe>
5 /nix/var/nix/profiles/default/bin/<exe>
6 /run/current-system/sw/bin/<exe>
── 仅当 home 非空 ──
7 <home>/.local/bin/<exe>
8 <home>/bin/<exe>
```

**Linux（`:192-211`）**，8 条固定 + 3 条 home 相关（注释明确：**优先 jellyfin-ffmpeg，因为它对 QSV/VAAPI 支持更完整**）：

```text
1 /usr/lib/jellyfin-ffmpeg/<exe>
2 /usr/local/bin/<exe>
3 /usr/bin/<exe>
4 /bin/<exe>
5 /snap/bin/<exe>
6 /var/lib/flatpak/exports/bin/<exe>
7 /nix/var/nix/profiles/default/bin/<exe>
8 /run/current-system/sw/bin/<exe>
── 仅当 home 非空 ──
9  <home>/.local/bin/<exe>
10 <home>/bin/<exe>
11 <home>/.nix-profile/bin/<exe>
```

> 非 Windows / 非 macOS 的一切平台都会走 Linux 分支（`else`）。

### 2.8 ffprobe 专用辅助（`FFmpegLocator.cs:242-260`）

```text
FindSiblingProbe(ffmpegPath):
    dir = dirname(ffmpegPath); if 空 -> null
    candidate = dir / ("ffprobe" + ExeSuffix)
    return File.Exists(candidate) ? candidate : null      # 任何异常 -> null

ProbeCandidates() = WellKnownCandidates("ffprobe")                          # 先平台位置
                 ++ BundledCandidates("ffprobe").map(p => baseDir / p)      # 再内置位置
ProbeName()       = "ffprobe"
```

**注意 `ProbeCandidates()` 与 `FindExecutable()` 的顺序相反**：probe 专用序列是「平台常见位置 → 内置目录」，而 ffmpeg 的搜索是「内置目录 → PATH → 平台常见位置」。且 `ProbeCandidates()` 里**不含 PATH 查找**，也**不检查 `IsOnPath`**（它只被 `FirstOrDefault(File.Exists)` 消费）。Rust 移植必须保留这一不对称，否则某些机器上 ffmpeg/ffprobe 会来自不同目录。

分支 1 中若用户只填了 ffmpeg、没填 ffprobe，最终 probe 的尝试顺序为：
`sibling → (平台常见位置 ∪ 内置位置，按上面的拼接顺序) → 裸名 "ffprobe"`。

---

## §3 FFmpegService —— 执行规格

> 源：`Services/FFmpegService.cs` 全文（1042 行）。

### 3.1 对外接口与数据模型（`FFmpegService.cs:7-88`）

```csharp
public interface IFFmpegService
{
    Task<FFmpegInfo?> GetFFmpegInfoAsync(bool forceRefresh = false);          // L10
    Task<MediaInfo?>   GetMediaInfoAsync(string filePath);                   // L13
    Task<FFmpegRunResult> RunAsync(FFmpegRunRequest request, CancellationToken ct = default); // L16
    Task<bool> SendInputAsync(ObjectId taskId, string input);                 // L19
    Task KillAsync(ObjectId taskId, bool graceful);                          // L22
    int RunningCount { get; }                                                // L25
    FFmpegPaths ResolvedPaths { get; }                                       // L28 (=> _locator.Resolve())
    BuiltCommand BuildCommand(CommandTemplate template, CommandContext ctx);  // L31 (委托 CommandLineBuilder.Build)
}
```

`FFmpegRunRequest`（`:35-63`，全部 init-only）：

| 字段 | 类型 | 必填 | 默认 | 语义 |
| --- | --- | --- | --- | --- |
| `TaskId` | ObjectId | ✔ | — | 进程登记表键（用于 `SendInputAsync` / `KillAsync`） |
| `Arguments` | IReadOnlyList\<string\> | ✔ | — | ffmpeg 参数（**不含** exe 名，也**不含**服务自动追加的 `-hide_banner -y/-n -progress pipe:2`） |
| `TotalDuration` | double（秒） | ✘ | 0 | 计算百分比/ETA 的分母；0 表示未知（进度恒为 0） |
| `OutputPath` | string? | ✘ | null | 失败清理用 |
| `Overwrite` | bool | ✘ | `true` | true → 追加 `-y`；false → 追加 `-n` |
| `OnProgress` | Action\<ConversionProgress\>? | ✘ | null | 进度回调（后台线程） |
| `OnLogLine` | Action\<string\>? | ✘ | null | 逐行日志回调（后台线程） |
| `Timeout` | TimeSpan? | ✘ | null | 超时；null = 不限制 |
| `CleanupPartialOutput` | bool | ✘ | `true` | 失败时删除半成品输出 |

`FFmpegRunResult`（`:66-88`）：

| 字段 | 类型 | 语义 |
| --- | --- | --- |
| `ExitCode` | int | 真实退出码；进程启动失败时为 `-1`；读取失败兜底 `-1` |
| `Success` | bool | `!Cancelled && !TimedOut && ExitCode == 0`；若 `Cancelled && ExitCode==0` 再强制置 false（`:725-730`） |
| `Cancelled` | bool | 由 CancellationToken 触发 |
| `TimedOut` | bool | 由 `request.Timeout` 触发（且 CT 未取消） |
| `SkippedExistingFile` | bool | 命中「文件已存在」特征串 |
| `Diagnostic` | string | stderr 尾部（≤200 行）拼接，再截断到 8000 字符 |
| `Duration` | TimeSpan | 从进程 Start 前到 wait 结束（stopwatch，含启动失败路径） |

> 注：`ExitCode == 255` 在注释里被称为「用户优雅中止」，但**代码里没有任何 255 特判**：`Success` 只看 `exitCode == 0`。Rust 侧不要臆造 255 分支。

### 3.2 版本与能力探测（`FFmpegService.cs:151-187`）

**缓存前置检查（`:153-156`）**：`!forceRefresh && _cachedInfo != null && (UtcNow - _cachedAt).TotalMinutes < 30` → 直接返回缓存。

**版本探测（`:158-163`）** —— 单条命令，30s 上限（`RunCaptureAsync`）：

```bash
<ffmpeg> -hide_banner -version
```

要求 `ExitCode == 0` **且** stdout 非空白，否则返回 `null`（上层视为 FFmpeg 不可用）。注意：`RunCaptureAsync` 在 stdout 为空而 stderr 非空时会把 **stderr 当 stdout**（`:907-911`），所以「退出码 0 但输出去了 stderr」的构建也能解析成功。

**能力清单（`:167-173`）** —— 5 条命令**并行**发出，全部走同一个 `RunCaptureAsync`：

```bash
<ffmpeg> -hide_banner -formats       # -> FFmpegInfo.SupportedFormats , Flags 宽度 3
<ffmpeg> -hide_banner -codecs        # -> FFmpegInfo.SupportedCodecs  , Flags 宽度 6
<ffmpeg> -hide_banner -encoders      # -> FFmpegInfo.SupportedEncoders, Flags 宽度 6
<ffmpeg> -hide_banner -hwaccels      # -> FFmpegInfo.SupportedHwaccels
<ffmpeg> -hide_banner -filters       # -> FFmpegInfo.SupportedFilters
```

（**是的，`-filters` 也在其中**；没有 `-decoders`、没有 `-pix_fmts`、没有 `-layouts`、没有 `-bsfs`、没有 `-muxers`/`-demuxers`、没有 `-devices`。）

每个任务的 stdout 单独解析（`Task.WhenAll` 后取值）；**不检查各任务的 ExitCode**，解析失败的清单退化为空列表。

**`RunCaptureAsync`（`:866-916`）—— 通用「跑一条命令并抓全部输出」**：

```text
启动: RedirectStandardOutput/Error = true, UseShellExecute=false, CreateNoWindow=true,
      StandardOutput/ErrorEncoding = UTF8, 参数经 ArgumentList 逐个传入（不走 shell）
启动异常: return CaptureResult(-1, "", ex.Message)
超时: 30 秒（CancellationTokenSource(TimeSpan.FromSeconds(30))）
      超时 -> Kill(entireProcessTree: true) 并 return CaptureResult(-1, "", "执行超时")
读取: 先并发 ReadToEndAsync(stdout/stderr)，再 WaitForExitAsync
stdout 为空 && stderr 非空 -> stdout = stderr          # 兼容把清单打到 stderr 的构建
返回: CaptureResult(process.ExitCode, stdout, stderr)
```

注意 `RunCaptureAsync` **不重定向 stdin**，也不加 `-nostdin`。

### 3.3 `ParseVersionOutput`（`FFmpegService.cs:189-226`）

逐行（`\n` 切分，`Trim()` 后判断，**大小写不敏感**）：

```text
若 line 以 "ffmpeg version" 开头:                      # OrdinalIgnoreCase
    rest  = line[14..].Trim()
    token = rest.Split(' ', RemoveEmptyEntries).FirstOrDefault() ?? "Unknown"   # 逐字保留首 token
    version = token                                     # 例: "n7.1.1" / "9.0.2" / "N-118527-g..." / "6.1.1-3ubuntu5"
    digits = token.SkipWhile(非数字).TakeWhile(数字)     # 从【第一个数字】开始的最长数字串
    if int.TryParse(digits) -> major = parsed           # "n7.1.1"->7, "9.0.2"->9, "N-118527-g.."->118527, 无数字->0
elif line 以 "configuration:" 开头:                     # OrdinalIgnoreCase
    configuration = line["configuration:".Length..].Trim()
```

返回的 `FFmpegInfo` 前 5 个清单参数先填 `[]`，`MajorVersion = major`，`IsAutoDetected = paths.IsAutoDetected`。

### 3.4 能力清单表格解析 `ParseListingTable(output, Flags)`（`FFmpegService.cs:228-272`）

用于 `-formats`(Flags=3) / `-codecs`(Flags=6) / `-encoders`(Flags=6)，**共用一套规则**：

```text
for rawLine in output.Split('\n'):
    line = rawLine.TrimEnd('\r')               # 只去尾部 CR，保留前导空格（前导空格有意义）
    if line.Length < 2: continue
    trimmed = line.TrimStart()
    if trimmed 为空: continue
    if trimmed[0] 不是大写字母: continue                                   # 跳过统计信息/说明行
    if trimmed 以 "Encoders:" / "Decoders:" / "Codecs:" / "File formats:" 开头: continue
    if trimmed 以 "---" 开头: continue

    flagEnd = trimmed.IndexOf(' ')                                          # 标志位段以空格结束
    if flagEnd <= 0 || flagEnd > Flags: continue                            # 标志段长度必须 ≤ 传入的 Flags
    flags = trimmed[..flagEnd]
    if flags[0] ∉ { 'V','A','S','D','E','T' }: continue                      # 视频/音频/字幕/数据/封装/附件

    remainder = trimmed[(flagEnd+1)..].TrimStart()
    if remainder 为空: continue
    nameEnd = remainder.IndexOf(' ')
    name = (nameEnd < 0 ? remainder : remainder[..nameEnd]).Trim()
    if name 为空 || name[0] == '=': continue
    if name 含任何非 [A-Za-z0-9_-.,@] 字符: continue                          # 严格白名单

    result.Add(name)

return result.Distinct(Ordinal).ToList()      # 保序去重，StringComparer.Ordinal
```

要点：
* 判据是「**首字符大写**」+「标志段以空格结束且长度 ≤ Flags」+「标志第一位在 `V A S D E T` 中」；
* 表格行的实际形态如 ` DEV.L. h264 ...`（formats 3 位）与 ` V....D h264_qsv ...`（codecs/encoders 6 位）；
* `-codecs` 与 `-encoders` 混在同一列表里收集（`-codecs` 行的名字即 codec 名，如 `h264`）；
* 名称字符白名单含 `.`、`-`、`_`、`,`、`@`（覆盖 `libx264`、`libvpx-vp9`、`aac@…` 之类）；
* `Distinct` 用 **Ordinal**（区分大小写）。

### 3.5 `ParseHwaccels`（`FFmpegService.cs:274-288`）

```text
for rawLine in output.Split('\n'):
    line = rawLine.Trim()
    if line 为空: continue
    if line 以 "Hardware acceleration" 开头（OrdinalIgnoreCase）: continue    # 头部标题行
    if line 含任何非 [A-Za-z0-9_-] 字符: continue                            # 必须"干净"
    result.Add(line)
return result.Distinct(Ordinal).ToList()
```

典型输出形如 `Hardware acceleration methods:` 后跟 `vdpau` / `cuda` / `vaapi` / `qsv` / `d3d11va` / `dxva2` / `opencl` / `vulkan` / `videotoolbox` 等。

### 3.6 `ParseFilters` / `IsFilterSignature`（`FFmpegService.cs:290-329`）

```text
for rawLine in output.Split('\n'):
    line = rawLine.Trim()
    if line 为空: continue
    if line 以 "Filters:" 开头 || line 以 "---" 开头: continue

    parts = line.Split(null /* 任意空白 */, RemoveEmptyEntries)
    if parts.Length < 3: continue
    name = parts[1]                                        # 第 0 段是 3 字符标志位（如 "TS"、"TSC"）
    if name 含任何非 [A-Za-z0-9_-.]: continue
    if IsFilterSignature(parts[2]) || parts.Skip(2).Take(2).Any(IsFilterSignature):
        result.Add(name)                                   # 用第 3 列（或其后 2 列内）的"输入->输出"签名定位
return result.Distinct(Ordinal).ToList()

IsFilterSignature(token):
    arrow = token.IndexOf("->")
    return arrow > 0 && arrow + 2 < token.Length && token.Length <= 8
```

`IsFilterSignature` 即：含 `->`、且 `->` 不在首位或末两位、且整个 token 长度 ≤ 8（如 `AA->A`、`V->V`、`V->N`）。多段签名的滤镜（如 `split`、`overlay` 只取前两段）也能命中。

### 3.7 媒体探测 `GetMediaInfoAsync`（`FFmpegService.cs:335-353`）

```text
1) filePath 空白 或 !File.Exists(filePath)  -> return null      # 不做任何尝试
2) paths = _locator.Resolve()
3) 命令行（注意：是 -v error，不是 -v quiet；不传 -show_error / -show_chapters / -show_programs）:

   <ffprobe> -v error -print_format json -show_format -show_streams <filePath>

4) result = RunCaptureAsync(paths.FFprobe, args)     # 30 秒超时
5) ExitCode != 0 || stdout 空白  -> return null
6) return ParseMediaInfo(filePath, result.StdOut)
```

**参数顺序逐字**（`FFmpegService.cs:340-347`）：`-v`、`error`、`-print_format`、`json`、`-show_format`、`-show_streams`、`<filePath>`。
没有 `-hide_banner`、没有 `-show_entries`、没有 `-of`、没有 `-i`、没有 `-nostdin`、没有网络/超时选项。

### 3.8 JSON 解析 `ParseMediaInfo`（`FFmpegService.cs:356-433`）

用 `System.Text.Json` 结构化解析（**无正则**），任何 `JsonException` → 返回 `null`。

**逐流字段（`streams[]`）** → `MediaStreamInfo`：

| 输出字段 | 目标属性 | 读取规则 |
| --- | --- | --- |
| `codec_type` | `CodecType` | 字符串；缺失 → `"unknown"` |
| `index` | `Index` | int |
| `codec_name` | `CodecName` | string? |
| `codec_long_name` | `CodecLongName` | string? |
| `width` | `Width` | int（非视频流通常缺失 → 0） |
| `height` | `Height` | int |
| `r_frame_rate` | `FrameRate` | **分式解析**（见下） |
| `duration` | `Duration` | double（秒） |
| `channels` | `Channels` | int |
| `sample_rate` | `SampleRate` | int（**注意 JSON 里是字符串** `"48000"`，`GetInt` 支持字符串→int） |
| `bit_rate` | `BitRate` | long（字符串亦可） |
| `side_data_list[].rotation` | `Rotation` | 见下 |
| `pix_fmt` | `PixelFormat` | string? |

**取标量的容错规则（`:458-502`）**：每个 `GetString`/`GetInt`/`GetLong`/`GetDouble` 都接受 JSON `String` 或 `Number`；字符串解析失败、类型不匹配、字段缺失一律返回默认（`null` / `0`）。double 一律用 `InvariantCulture` + `NumberStyles.Float`。

**`ParseFraction`（`:505-519`）**：`"25/1"` → `25.0`；`"30000/1001"` → `29.970029…`；非 `a/b` 形式则直接按 `double` 解析；分母为 0、空串、解析失败 → `0`。

**`GetRotation`（`:521-540`）**：仅当流含 `side_data_list` 数组时遍历；遇到**第一个**含 `rotation` 的项即返回 `((value % 360) + 360) % 360`（负数归一化，如 `-90` → `270`）；数字或字符串均可；数组缺失 → `0`。

**派生字段（`:387-427`）**：

```text
video = streams 中第一个 codec_type == "video"
audio = streams 中第一个 codec_type == "audio"
subtitleCount = streams.Count(codec_type == "subtitle")

duration  : 优先 root.format.duration；若 <= 0 则 video?.Duration ?? audio?.Duration ?? 0
bitRate   : root.format.bit_rate（缺失/非法 → 0）
formatName: root.format.format_name（缺失 → "unknown"）
fileSize  : try { FileInfo(filePath).Length } catch { 0 }      # 真实磁盘大小，不用 ffprobe 的 size
```

返回的 `MediaInfo`：

| 字段 | 来源 |
| --- | --- |
| `FilePath` | 入参 |
| `Duration` | 上面的 duration（秒） |
| `Format` | `format_name` 原值（如 `"mov,mp4,m4a,3gp,3g2,mj2"`） |
| `VideoCodec` / `AudioCodec` | 首条视频/音频流的 `codec_name` |
| `Width` / `Height` / `FrameRate` | 首条视频流；无视频 → 0 |
| `FileSize` | 文件系统字节数 |
| `FormatDisplay` | `FormatDisplayName(formatName)`，见下表 |
| `BitRate` | 容器总码率（bit/s） |
| `AudioChannels` / `AudioSampleRate` | 首条音频流；无音频 → 0 |
| `Rotation` | 首条视频流旋转角（0/90/180/270） |
| `PixelFormat` | 首条视频流 `pix_fmt` |
| `HasVideo` / `HasAudio` | 首条对应类型流是否存在 |
| `SubtitleStreamCount` | 字幕流数量 |
| `Streams` | 全部流（保序） |
| `DurationDisplay`（计算属性，`Models/DTOs.cs:85-89`） | `Duration<=0` → `"-"`；否则 ≥1h 用 `h:mm:ss`，否则 `m:ss` |

**`FormatDisplayName`（`FFmpegService.cs:435-456`）** —— 取 `format_name.Split(',')[0].Trim()`，再 `ToLowerInvariant()` 查表：

| 首段 | 显示名 |
| --- | --- |
| `mov`,`mp4`,`m4a`,`3gp`,`3g2`,`mj2` | `MP4 / MOV` |
| `matroska`,`webm` | 原串（**未小写**）含 `webm`（OrdinalIgnoreCase）→ `WebM`，否则 `Matroska (MKV)` |
| `avi` | `AVI` |
| `flv` | `FLV` |
| `asf` | `ASF / WMV` |
| `mpeg` | `MPEG` |
| `mpegts` | `MPEG-TS` |
| `ogg` | `OGG` |
| `wav` | `WAV` |
| `flac` | `FLAC` |
| `mp3` | `MP3` |
| `aac` | `AAC` |
| `image2` | `图片序列` |
| `gif` | `GIF` |
| 其它 | `first.ToUpperInvariant()`（用**首段大写**，不是原串） |

### 3.9 `RunAsync` —— 参数前置（`FFmpegService.cs:555-566`）

每次执行都会在调用方参数**之前**插入固定 4 个 token（顺序严格）：

```text
args = ["-hide_banner",
        request.Overwrite ? "-y" : "-n",     # 明确覆盖策略，避免 ffmpeg 读 stdin 永久挂起
        "-progress", "pipe:2",               # 进度以 key=value 形式写到 stderr
        ...request.Arguments]
```

完整命令行等价于：

```bash
<ffmpeg> -hide_banner -y -progress pipe:2 <调用方参数...>
```

**没有** `-nostdin`（改用显式 `-y/-n` + 保留 stdin 用于发 `q`）。渲染出的可读命令行是 `paths.FFmpeg + " " + RenderCommandLine(args)`，其中 `RenderCommandLine` 用空格连接，并对含 `空格 制表符 " ' ; & | > <` 的参数加双引号（内部 `"` 转义为 `\"`；空串渲染为 `""`）——**仅用于展示/复制**，真正传进程的是 `ArgumentList`（不经 shell）。

**进程启动参数**（`:568-582`）：`RedirectStandardOutput/Error/Input = true`、`UseShellExecute = false`、`CreateNoWindow = true`、stdout/stderr 编码 UTF-8。

**进程登记**（`:584-585`）：`lock` 下 `_running[request.TaskId] = handle`；`RunningCount` = 该字典大小。**同 TaskId 重复登记会覆盖**。

**启动失败**（`:598-611`）：移除登记、`Dispose`、返回 `ExitCode = -1`、`Success = false`、`Diagnostic = $"无法启动 FFmpeg（{paths.FFmpeg}）：{ex.Message}"`、`Duration` 为已耗时。

### 3.10 输出读取与进度分发（`FFmpegService.cs:587-678`）

* **stderr 有界缓冲**：`BoundedLineBuffer(200)` —— 只保留**最近 200 行**，超出时 `Dequeue` 最旧行（`:918-940`）。
* **stdout 线程**（`:637-655`）：逐行读，仅用于检测 `"already exists. Overwrite?"`（OrdinalIgnoreCase）→ `skippedExisting = true`；**stdout 内容不进日志**。
* **stderr 线程**（`:620-635`）：逐行读 → `OnStderrLine(line)`；读异常被吞（进程被杀时属预期）。
* 启动后立刻 `StandardInput.FlushAsync()`（`:658`）——保持 stdin 管道打开以便后续发 `q`。
* `OnStderrLine`（`:660-678`）逻辑顺序：

```text
1) stderrTail.Add(line)                                     # 先入 200 行环形缓冲
2) if line 含 "already exists. Overwrite?" 或 "Not overwriting - exiting"（均 OrdinalIgnoreCase）
       -> skippedExisting = true
3) if progressState.TryConsume(line, TotalDuration, out progress)
       -> request.OnProgress?.Invoke(progress)               # 本行【不再】作为日志
   else
       -> request.OnLogLine?.Invoke(line)                    # 普通日志行
```

**「文件已存在」判定的两个特征串是逐字的**：

```text
"already exists. Overwrite?"     # 交互式提问（未加 -y/-n 时）
"Not overwriting - exiting"      # -n 时的输出
```

### 3.11 超时、结束与清理（`FFmpegService.cs:680-750`）

```text
if request.Timeout.HasValue:
    timeoutCts = CreateLinkedTokenSource(cancellationToken); timeoutCts.CancelAfter(Timeout)
    try   await process.WaitForExitAsync(timeoutCts.Token)
    catch OperationCanceledException:
        timedOut = !cancellationToken.IsCancellationRequested    # 区分"超时"与"外部取消"
        TryKillProcess(process)                                  # 整棵进程树
else:
    await process.WaitForExitAsync(CancellationToken.None)       # 不限制
catch OperationCanceledException: TryKillProcess(process)
finally:
    等 stderr/stdout 读取线程最多 3 秒（Task.WhenAny(WhenAll(...), Delay(3s))）
    stopwatch.Stop()
    lock { _running.Remove(request.TaskId) }

exitCode = try { process.ExitCode } catch { -1 }
diagnostic = stderrTail.ToString()                               # 200 行用 Environment.NewLine 连接

if skippedExisting && exitCode != 0:
    diagnostic = "输出文件已存在，且当前策略为「不覆盖」。请启用覆盖或使用自动重命名。" + NewLine + diagnostic

success = !cancelled && !timedOut && exitCode == 0
if cancelled && exitCode == 0: success = false

if !success && request.CleanupPartialOutput && !skippedExisting && OutputPath 非空:
    TryDeletePartialOutput(OutputPath)

process.Dispose()
return { ExitCode, Success, Cancelled, TimedOut, SkippedExistingFile, Diagnostic = Truncate(diagnostic, 8000), Duration }
```

**`TryDeletePartialOutput`（`:840-858`）**：仅当文件存在**且 `Length < 64 MiB`** 时才删除（`1024*1024*64`）；任何异常吞掉。设计意图：小文件 = 失败残片；大文件可能用户有价值，宁可留着。

**`Truncate(text, 8000)`（`:860-864`）**：

```text
若 text 空 或 length <= 8000 -> 原样返回
否则 -> "…（已截断前部输出）\n" + text[len-8000 ..]      # 保留【尾部】8000 字符
```

### 3.12 进度解析 `ProgressAccumulator`（`FFmpegService.cs:942-1041`）—— 最关键的一段

ffmpeg 的 `-progress pipe:2` 会周期性输出**一组** `key=value` 行，最后以 `progress=continue` 或 `progress=end` 收尾。解析器**只在看到 `progress=` 这一行时才结算一次进度**。

**已知 key 白名单（`:1038-1040`，Ordinal 精确匹配）**：

```text
frame, fps, bitrate, total_size, out_time_us, out_time_ms, out_time, dup_frames, drop_frames, speed, progress
```

**`TryConsume(line, totalDuration, out progress)`（`:955-1006`）**：

```text
1) line 空白 -> return false
2) equals = line.IndexOf('=');  equals <= 0 -> return false        # 无 '=' 或以 '=' 开头 -> 普通日志行
3) key   = line[..equals].Trim()
   value = line[equals+1..].Trim()
4) if key 不在白名单 -> return false                              # 该行走 OnLogLine
5) if key != "progress": _pending[key] = value; return false      # 攒状态（后值覆盖前值）
6) 结算：                                                          # key == "progress"
   currentTime = ReadSeconds(_pending)
   speed       = ReadSpeed(_pending)
   frame       = _pending["frame"]   ?? ""
   bitrate     = _pending["bitrate"] ?? ""
   _pending.Clear()
   isEnd = (value == "end")                                        # OrdinalIgnoreCase
   if isEnd && totalDuration > 0: currentTime = totalDuration       # 兜底到 100%
   percentage = totalDuration > 0 ? clamp(currentTime / totalDuration * 100.0, 0, 100) : 0
   eta = null
   if speed > 0.01 && totalDuration > 0:
       remaining = totalDuration - currentTime
       if remaining > 0: eta = remaining / speed                    # 单位：秒
   raw = "  " 连接下列非空项：["frame={frame}", "bitrate={bitrate}", "speed={speed:0.##}x", "time={hh:mm:ss.ff}"]
   progress = ConversionProgress(percentage, currentTime, totalDuration, speed, eta, raw)
   return true
```

**`ReadSeconds` 的优先级（`:1008-1026`）—— 必须逐字照做**：

```text
1) out_time_us 存在 且 解析为 long 且 > 0  -> 值 / 1_000_000.0     # 微秒 → 秒
2) out_time_ms 存在 且 解析为 long 且 > 0  -> 值 / 1_000_000.0     # ⚠ 也是除以 1e6，不是 1e3
3) out_time 存在 且 TimeSpan.TryParse 成功 -> span.TotalSeconds    # 形如 "00:01:23.4567890"
4) 否则 -> 0
```

> ⚠ `out_time_ms` 被当作**微秒**处理（除以 1e6）。这是刻意的：ffmpeg 历史上 `out_time_ms` 实际写的是微秒（与 `out_time_us` 同值），社区广泛如此处理。**Rust 移植必须保留这个除 1e6 的语义**，否则进度会瞬间跑满。
>
> 另外两条隐含语义：(a) 值为 0 或负数时会**落空到下一优先级**（因为判断条件是 `> 0`）；(b) `TimeSpan.TryParse` 用当前 Culture 解析，Rust 侧应固定用 `HH:MM:SS.ffffff` 解析（含负号情况需自行决定）。

**`ReadSpeed`（`:1028-1036`）**：取 `speed` 值，`Trim()` 后 `TrimEnd('x','X')`；若结果等于 `"N/A"`（OrdinalIgnoreCase）→ `null`；否则按 `InvariantCulture` + `NumberStyles.Float` 解析，失败 → `null`。（ffmpeg 会输出形如 `speed=1.23x` 或 `speed=N/A`。）

**`fps` 与 `total_size` / `dup_frames` / `drop_frames`**：被识别为已知 key（因此**不会**进入用户日志），但**没有被消费**，不会出现在 `ConversionProgress` 的 `RawOutput` 里。Rust 侧若要 1:1 复刻，也应吸收但不暴露这三个值。

**`ConversionProgress`（`Models/DTOs.cs:93-99`）**：`(double Percentage, double CurrentTime, double TotalDuration, double? Speed, double? Eta, string RawOutput)`。

**`raw` 拼接格式逐字**：分隔符是两个半角空格 `"  "`；`speed` 用 `0.##` 定点（两位小数、去尾零）；时间用 `TimeSpan.FromSeconds(currentTime)` 按 `hh\:mm\:ss\.ff` 格式化（百分之一秒，`hh` 在 24h 处回绕，负数会带 `-`）。

### 3.13 输入与终止（`FFmpegService.cs:752-838`）

**`SendInputAsync(taskId, input)`**（`:752-772`）：

```text
查登记表；不存在 -> false
try: if process.HasExited -> false
     await StandardInput.WriteAsync(input);   await StandardInput.FlushAsync();   return true
catch: false
```

**`KillAsync(taskId, graceful)`**（`:774-794`）：查登记表（缺失 → 直接返回）；`handle.Cancelled = true`；`graceful ? TryGracefulStop : TryKillProcess`。**立即返回，不等进程退出**。

**`TryGracefulStop`（`:796-826`）—— 优雅停止 = 向 stdin 写 `q`**：

```text
try: if process.HasExited -> return
     StandardInput.Write("q");  StandardInput.Flush()       # 单个小写 'q'，无换行、无 \n
catch: 忽略（管道可能已关闭）

随后【后台任务】：等进程退出，超时 5 秒；超时 -> TryKillProcess(process)
```

**`TryKillProcess`（`:828-838`）**：

```text
try: if !process.HasExited -> process.Kill(entireProcessTree: true)   # Windows: 杀 Job/Object 树；POSIX: 杀进程组
catch: 忽略
```

**CancellationToken 注册（`:613-618`）**：`cancelled = true; handle.Cancelled = true; TryGracefulStop(process)` —— 即「取消 = 先发 `q`，5 秒不退再杀」。

**因果链小结（Rust 侧照抄）**：

| 触发 | 动作 | 结果标记 |
| --- | --- | --- |
| `CancellationToken` 取消 | 写 `q` → 5s 未退出则杀树 | `Cancelled = true`，`Success = false` |
| `request.Timeout` 到期 | 直接杀树（**不发 `q`**） | `TimedOut = true`，`Success = false` |
| `KillAsync(id, graceful: true)` | 写 `q` → 5s 未退出则杀树 | 只置 `handle.Cancelled`（不影响本次 `FFmpegRunResult`） |
| `KillAsync(id, graceful: false)` | 直接杀树 | 同上 |

> `handle.Cancelled` 字段在 `RunAsync` 里**只写不读**（`cancelled` 局部变量才参与结果），它是给 `ProcessHandle` 保留的状态位。

### 3.14 缓存与生命周期（`FFmpegService.cs:90-145`；`Program.cs:52-71`）

| 缓存 | 键 | TTL | 失效方式 |
| --- | --- | --- | --- |
| `_cachedInfo`（`FFmpegInfo?`） | 无（单例单槽） | **30 分钟**（依据 `_infoCachedAt`，UTC 比较） | (a) `forceRefresh = true`；(b) 订阅的 `ISettingsService.SettingsChanged` 事件触发 `OnSettingsChanged` → `_cachedInfo = null; _infoCachedAt = DateTime.MinValue`（`:111-118`） |
| `_running`（进程登记表） | `TaskId` | 随任务生命周期 | `finally` 中 `Remove` |
| `FFmpegLocator._cached` | 无 | 无 TTL | `SettingsService` 保存时 `Invalidate()` |
| `HardwareDetectionService._probeCache` | `encoderName` | **无 TTL（进程级永久）** | 见 §5.6 |
| `HardwareDetectionService._lastReport` | 无 | **30 分钟** | `forceRefresh = true`（但不清 `_probeCache`） |

`FFmpegService` 实现 `IDisposable`，`Dispose()` 只做 `_settingsService.SettingsChanged -= OnSettingsChanged`（`:120`）。构造函数订阅事件（`:108`），因此 `IFFmpegService` / `IHardwareDetectionService` / `IFFmpegLocator` / `ISettingsService` **必须注册为 Singleton**（与 `Program.cs:52-71` 一致），否则能力缓存会跨作用域错乱。

### 3.15 日志行上限（除 `BoundedLineBuffer(200)` 之外）

| 位置 | 上限 | 常量来源 |
| --- | --- | --- |
| `RunAsync` stderr 尾部缓冲 | 最近 **200** 行 | `FFmpegService.cs:587` |
| `RunAsync` Diagnostic 字符串 | **8000** 字符（保留尾部） | `FFmpegService.cs:747`、`:860-864` |
| 任务日志（DB / UI，属于 `ConversionQueue`） | `AppSettings.MaxLogLines`，默认 **500**，合法区间 **[50, 20000]**，越界重置为 500 | `Models/AppSettings.cs:47,71`；`Services/ConversionQueue.cs:463,468,764`（`TruncateLog` 只保留最后 N 行） |
| 试跑 stderr 读取 | 全量读入后仅取前 2 行/截 300 字符做原因 | `HardwareDetectionService.cs:468-486` |

---

## §4 数据模型（能力模型 / 媒体模型 / 硬件模型）

### 4.1 `FFmpegInfo`（`Models/DTOs.cs:6-24`）—— 逐字段

```csharp
public record FFmpegInfo(
    string Version,                  // "ffmpeg version" 后首个空格分隔 token，逐字（如 "7.1.1" / "n7.1.1" / "N-118527-g..."）；解析不到 -> "Unknown"
    string Path,                     // 解析出的 ffmpeg 可执行文件字符串（可能为裸名 "ffmpeg"）
    string Configuration,            // "configuration:" 行的内容（configure 参数串）；无 -> ""
    List<string> SupportedFormats,   // `-formats` 文件名（含 muxer/demuxer，如 "mp4"、"matroska"、"image2"）
    List<string> SupportedCodecs,    // `-codecs` 编解码器名（如 "h264"、"aac"、"pcm_s16le"）
    List<string> SupportedEncoders,  // `-encoders` 编码器名（如 "libx264"、"h264_nvenc"、"aac"）
    List<string> SupportedHwaccels,  // `-hwaccels` 加速方法（如 "videotoolbox"、"cuda"、"vaapi"）
    List<string> SupportedFilters)   // `-filters` 滤镜名（如 "scale"、"format"、"hwupload"）
{
    public int  MajorVersion   { get; init; }   // 从 Version 中【第一个数字开始】的最长数字串；解析失败 0
    public bool IsAutoDetected { get; init; }   // 来自 FFmpegPaths.IsAutoDetected
    public bool IsAvailable    => !string.IsNullOrEmpty(Version) && Version != "Unknown";
}
```

**判定「可用」的唯一条件**：`Version` 非空且 != `"Unknown"`。`GetFFmpegInfoAsync` 返回 `null`（命令失败）与返回 `Version = "Unknown"`（命令成功但输出里没有 `ffmpeg version` 行 → 注意此时 `ParseVersionOutput` 只在 `Version` 为默认值时才可能输出 `"Unknown"`，实际路径里 `-version` 成功即有该行）是两种不同的「不可用」。

### 4.2 `MediaStreamInfo`（`Models/DTOs.cs:27-40`）

`(int Index, string CodecType, string? CodecName, string? CodecLongName, int Width, int Height, double FrameRate, double Duration, int Channels, int SampleRate, long BitRate, int Rotation, string? PixelFormat)`
—— 语义见 §3.8 表。

### 4.3 `MediaInfo`（`Models/DTOs.cs:43-90`）

位置参数 9 个：`FilePath, Duration, Format, VideoCodec, AudioCodec, Width, Height, FrameRate, FileSize`。
init 属性：`FormatDisplay="unknown"`、`BitRate=0`、`AudioChannels=0`、`AudioSampleRate=0`、`Rotation=0`、`PixelFormat=null`、`HasVideo=false`、`HasAudio=false`、`SubtitleStreamCount=0`、`Streams=[]`。
计算属性 `DurationDisplay` 见 §3.8。

### 4.4 硬件检测模型（`Models/HardwareEncoder.cs`）

**`HardwareEncoder`（`:6-48`）** —— 也是「编码器缓存」集合的行结构（LiteDB 集合名 `encoders`）：

| 字段 | 类型 | 默认 | 语义 |
| --- | --- | --- | --- |
| `Id` | ObjectId | 新 ObjectId | 主键 |
| `Name` | string | `""` | FFmpeg 编码器名（如 `h264_nvenc`） |
| `DisplayName` | string | `""` | 显示名 |
| `Type` | EncoderType | `Software` | 厂商类型 |
| `Family` | string | `""` | 族标识 |
| `Codec` | string | `""` | 目标编码格式 |
| `IsAvailable` | bool | `false` | 是否可用 |
| `SupportsQualityOptions` | bool | `true` | 是否通过了**带质量参数**的第一档试跑；false = 只能用最简参数 |
| `IsHardware`（计算） | bool | — | `Type != Software` |
| `SupportedCodecs` | List\<string\> | `[]` | **兼容旧字段**：恒为 `[profile.Codec]` 单元素 |
| `LastCheckedAt` | DateTime | `UtcNow` | 该行检测时间（UTC），DB 缓存 TTL 判据 |
| `UnavailableReason` | string? | null | 不可用原因（可用时为 null） |
| `MissingFromBuild` | bool | false | 是否因「不在 FFmpeg 构建清单中」而不可用 |
| `FFmpegVersion` | string | `""` | 检测时的 FFmpeg 版本（DB 缓存有效性判据） |

**`HardwareDetectionReport`（`:51-87`）**：

| 字段 | 类型 | 默认 | 语义 |
| --- | --- | --- | --- |
| `Encoders` | List\<HardwareEncoder\> | `[]` | 全部编码器（含不可用），已排序（见 §5.5） |
| `DetectedAt` | DateTime | `UtcNow` | 检测时间（内存 TTL 判据） |
| `FFmpegVersion` | string | `""` | FFmpeg 版本 |
| `FFmpegPath` | string | `""` | FFmpeg 路径 |
| `Platform` | string | `""` | `Windows`/`macOS`/`Linux` |
| `Hwaccels` | List\<string\> | `[]` | 来自 `FFmpegInfo.SupportedHwaccels` |
| `SupportedFilters` | List\<string\> | `[]` | 来自 `FFmpegInfo.SupportedFilters` |
| `Summary` | string | `""` | 整体提示（文案见 §5.5） |
| `AvailableHardwareEncoders`（计算） | List | — | `Encoders.Where(e => e.IsAvailable && e.IsHardware)` |
| `HasHardwareAcceleration`（计算） | bool | — | 上述列表非空 |
| `HasFilter(name)`（方法，`:75-76`） | bool | — | `SupportedFilters.Count == 0 \|\| SupportedFilters.Contains(name, OrdinalIgnoreCase)`（**清单为空视为"未知，放行"**） |

### 4.5 枚举（`Models/Enums.cs`）

```text
EncoderType            : Software=0, Nvenc=1, Qsv=2, Amf=3, VideoToolbox=4, Vaapi=5, Vulkan=6, MediaFoundation=7, Other=99
HardwareAccelerationMode: None=0, Auto=1, Required=2
TaskStatus             : Pending=0, Running=1, Completed=2, Failed=3, Cancelled=4, Skipped=5
TemplateType           : System=0, User=1
OutputNamingRule       : Original=0, Suffix=1
FileExistsAction       : Ask=0, Overwrite=1, Skip=2, Rename=3
TemplateParameterKind  : Select=0, Number=1, Text=2, Boolean=3
TemplateIssueSeverity  : Info=0, Warning=1, Error=2
```

Rust 侧序列化约定必须与持久化一致：DB 里 `EncoderType` 以整数存储（本规格默认按 `i32` 复刻）。

### 4.6 `AppSettings` 中与本模块相关的字段（`Models/AppSettings.cs`）

| 字段 | 默认 | 与本模块的关系 |
| --- | --- | --- |
| `FFmpegPath` | `""` | 定位器分支 1 |
| `FFprobePath` | `""` | 定位器分支 1/2 |
| `PreferHardwareAcceleration` | `true` | §5.4 自动选择时是否优先硬件 |
| `PreferredHardwareEncoder` | `""` | §5.4 第 2 优先级，具体编码器名（**不是**族名） |
| `PreferHardwareDecoding` | `false` | 影响 `-hwaccel` 注入（`CommandLineBuilder`，不在本模块） |
| `AutoFallbackToSoftware` | `true` | 只在 ffmpeg 尚未写出有效数据时回退 |
| `GlobalFFmpegArguments` | `""` | 追加到输出文件前的全局参数 |
| `MaxLogLines` | 500 | 日志行上限 |

`Normalize()`（`:68-83`）会：钳制 `MaxConcurrentTasks∈[1,16]`、`MaxLogLines∈[50,20000]`、`TaskHistoryRetentionDays∈[0,3650]`、`ListenPort∈[0,65535]`；`OutputSuffix` 空则 `"_converted"`；`Theme ∈ {Light,Dark,System}`；字符串字段全部 `Trim()`，`FFmpegPath`/`FFprobePath` 额外 `Trim('"')`。**定位器与硬件检测都会在读取设置后立刻 `Normalize()`。**

---

## §5 HardwareDetectionService —— 硬件检测规格

> 源：`Services/IHardwareDetectionService.cs`（33 行）、`Services/HardwareDetectionService.cs`（559 行）。

### 5.1 接口（`IHardwareDetectionService.cs:6-33`）

```csharp
public interface IHardwareDetectionService
{
    Task<HardwareDetectionReport> DetectAsync(bool forceRefresh = false, CancellationToken ct = default); // L9
    HardwareDetectionReport? LastReport { get; }                       // L12  仅返回内存中的上一次报告，null = 从未检测
    Task<bool> IsEncoderAvailableAsync(string encoderName);            // L15  内部调用 DetectAsync()（forceRefresh=false）
    Task<string?> GetRecommendedEncoderAsync(string codec, string? requiredEncoder = null, bool strict = false); // L26
    EncoderProfile? ResolveProfile(string encoderName);                // L29  => EncoderCatalog.Find(...)
    DateTime? LastDetectionTime { get; }                               // L32  => LastReport?.DetectedAt
}
```

`strict` 的文档语义（`:21-25` 注释）：`true` 表示「指定编码器**必须**可用」——该族在本机没有可用成员时返回 `null`，而不是退而求其次。模板声明 `Required` 时使用该模式，以避免「模板写着 NVENC、实际用 VideoToolbox 跑」这种名不副实的结果。

### 5.2 `DetectAsync` 主流程（`HardwareDetectionService.cs:35-171`）

```text
1) if !forceRefresh && _lastReport != null && (UtcNow - _lastReport.DetectedAt).TotalMinutes < 30 -> return _lastReport
2) await _detectionGate.WaitAsync(ct)                     # SemaphoreSlim(1,1)
   try:
     2a) 再次做同样的 30 分钟检查（double-check），命中则返回
     2b) info = await ffmpegService.GetFFmpegInfoAsync(forceRefresh)
     2c) settings = (await settingsService.GetSettingsAsync()).Normalize()
     2d) paths = locator.Resolve()

     2e) if info == null || !info.IsAvailable:            # ── 无 FFmpeg 分支
            _lastReport = HardwareDetectionReport {
                DetectedAt     = UtcNow,
                Encoders       = BuildUnavailableCatalog("未找到可用的 FFmpeg，请先在设置中指定 FFmpeg 路径。" + paths.Hint),
                FFmpegVersion  = "未检测到",
                FFmpegPath     = paths.FFmpeg,
                Platform       = AppPaths.PlatformDisplayName,
                Summary        = "未检测到 FFmpeg，无法进行硬件加速检测。"
                # Hwaccels / SupportedFilters 保持 [] 默认值
            }
            return _lastReport                            # 【不写数据库】
     2f) version = info.Version
     2g) if !forceRefresh:
            cached = TryLoadFromDatabase(version, info.Path)     # 见 §5.6
            if cached != null: _lastReport = cached; return cached

     2h) buildEncoders = info.SupportedEncoders 的 HashSet（Ordinal）
     2i) results = ConcurrentBag<HardwareEncoder>
     2j) candidates = EncoderCatalog.Profiles.Where(p => p.SupportsCurrentPlatform)     # 【平台过滤】
     2k) probes = List<Task>;  probeGate = SemaphoreSlim(clamp(ProcessorCount / 2, 2, 4))
     2l) foreach profile in candidates:
             if !profile.IsHardware:                          # 软件编码器：只查构建清单，绝不试跑
                 avail = buildEncoders.Contains(profile.Name)
                 results.Add(CreateEntry(profile, avail,
                     reason: avail ? null : "该编码器不在当前 FFmpeg 构建中",
                     missingFromBuild: !avail, supportsQuality: true, version))
                 continue
             if !buildEncoders.Contains(profile.Name):         # 硬件编码器不在构建里：不试跑
                 results.Add(CreateEntry(profile, false,
                     "该编码器不在当前 FFmpeg 构建中（FFmpeg 未启用对应硬件加速）",
                     missingFromBuild: true, supportsQuality: true, version))
                 continue
             probes.Add(Task.Run(async () => {
                 await probeGate.WaitAsync(ct)
                 try:
                     probe = await ProbeEncoderAsync(profile, ct)          # 见 §5.3
                     results.Add(CreateEntry(profile, probe.Available, probe.Reason,
                         missingFromBuild: false, supportsQuality: probe.SupportsQualityOptions, version))
                 finally: probeGate.Release()
             }, ct))
     2m) try await Task.WhenAll(probes) catch OperationCanceledException { /* 取消时保留部分结果 */ }
     2n) ordered = results.OrderBy(Type == Software ? 1 : 0)      # 硬件(0)在前，软件(1)在后
                          .ThenBy(Family).ThenBy(Codec).ThenBy(Name)
     2o) availableHardware = ordered.Count(e => e.IsAvailable && e.IsHardware)
     2p) report = HardwareDetectionReport {
             Encoders = ordered, DetectedAt = UtcNow, FFmpegVersion = version, FFmpegPath = info.Path,
             Platform = AppPaths.PlatformDisplayName,
             Hwaccels = info.SupportedHwaccels, SupportedFilters = info.SupportedFilters,
             Summary = availableHardware > 0
                 ? $"检测到 {availableHardware} 个可用硬件编码器（{string.Join("、", ordered.Where(可用&&硬件).Select(DisplayName).Distinct().Take(4))}）"
                 : "未检测到可用的硬件编码器，将使用软件编码。"
         }
     2q) SaveToDatabase(report); _lastReport = report; return report
   finally: _detectionGate.Release()
```

**并发度常量**：`Math.Clamp(Environment.ProcessorCount / 2, 2, 4)` → 取值 **2 / 3 / 4**。Rust 侧用 `std::thread::available_parallelism()` 复刻。

**排序注意**：`OrderBy`/`ThenBy` 对 string 用**当前 Culture 的默认比较器**（本机近 Ordinal），Rust 侧统一用序数（字节）比较即可，但要在实现里显式写明，以免与 C# 在非 ASCII 环境下出现差异（本目录全 ASCII，实际无差异）。

**`BuildUnavailableCatalog(reason)`（`:510-514`）**：仅枚举 `SupportsCurrentPlatform && !IsHardware` 的画像（即软件编码器），全部 `available=false`、`MissingFromBuild=true`、`supportsQuality=true`、`FFmpegVersion="未检测到"`、`UnavailableReason=reason`。**因此「找不到 FFmpeg」时报告里只有软件编码器条目，没有任何硬件条目。**

**`CreateEntry`（`:492-508`）字段映射**：`Name/DisplayName/Type/Family/Codec` 取自画像；`IsAvailable = available`；`SupportsQualityOptions = supportsQuality`；`SupportedCodecs = [profile.Codec]`；`LastCheckedAt = UtcNow`；`UnavailableReason = available ? null : reason`；`MissingFromBuild`；`FFmpegVersion = version`。

### 5.3 两档试跑（`HardwareDetectionService.cs:270-449`）—— 核心

**`ProbeEncoderAsync(profile)`（`:272-305`）**

```text
if _probeCache.TryGetValue(profile.Name, out cached):
    return cached ? ProbeOutcome(true,  true,  null)                       # ⚠ 见下方"缓存放大"注意
                  : ProbeOutcome(false, false, "试跑失败（缓存结果）")

# 第 1 档：带质量参数与预设（模板真正会用的参数）
fullOutput  = ProbeOutputPath(name, "full")
fullArgs    = BuildProbeArguments(profile, includeQuality: true,  fullOutput)
fullResult  = await ExecuteProbeAsync(fullArgs, ct)
if fullResult.Success: _probeCache[name] = true;  return ProbeOutcome(true,  true,  null)

# 第 2 档：仅指定编码器（兼容参数名变化的 FFmpeg 构建）
minimalOutput = ProbeOutputPath(name, "min")
minimalArgs   = BuildProbeArguments(profile, includeQuality: false, minimalOutput)
minimalResult = await ExecuteProbeAsync(minimalArgs, ct)
if minimalResult.Success:
    _probeCache[name] = true;                                          # ⚠ 与第 1 档同样记 true
    return ProbeOutcome(true, false, "可用，但当前 FFmpeg 版本不接受该编码器的质量参数，将使用编码器默认质量")

_probeCache[name] = false
return ProbeOutcome(false, false, SummarizeFailure(fullResult.Diagnostic))   # 只汇总第 1 档的诊断
```

> ⚠ **缓存放大语义（Rust 移植必须决策）**：`_probeCache` 是 `ConcurrentDictionary<string,bool>`，只存一个布尔。第 2 档成功也写入 `true`，于是**同一进程内后续任何 `DetectAsync(forceRefresh: true)` 都会把该编码器报成 `SupportsQualityOptions = true`**（丢掉「不接受质量参数」的降级信息）。参考实现存在这个行为；若 Rust 侧想修正，应改成存 `enum { FullOk, MinimalOnly, Failed }`，并在文档中标注为「有意偏离」。

**试跑临时输出路径（`:307-311`）**

```text
safe = 编码器名中仅保留 [A-Za-z0-9_-] 的字符（其它字符全部丢弃）
path = Path.GetTempPath() / $"ffmpegwebui-probe-{safe}-{stage}-{Guid.NewGuid():N}.raw"
       stage ∈ {"full", "min"}；Guid 为 32 位无连字符小写十六进制
```

`GetTempPath()` 在 Linux/macOS 取 `$TMPDIR`（无则 `/tmp`），Windows 取 `%TEMP%`。Rust 对应 `std::env::temp_dir()`。

**`BuildProbeArguments(profile, includeQuality, outputPath)`（`:313-349`）** —— 逐 token 顺序：

```text
args = ["-hide_banner", "-nostdin", "-loglevel", "error"]

if profile.DeviceArgs 非空白:                     # 仅 VAAPI 系列有值
    args += Tokenize(profile.DeviceArgs)          # 例：-vaapi_device /dev/dri/renderD128

args += ["-f", "lavfi", "-i", "testsrc2=duration=0.3:size=320x240:rate=25"]

if profile.UploadFilter 非空白:                   # 仅 VAAPI 系列
    args += ["-vf", profile.UploadFilter]         # 例：format=nv12,hwupload

args += ["-c:v", profile.Name]

if includeQuality:
    q = profile.DefaultQuality.ToString()         # 十进制、无文化差异
    if profile.QualityArgs 非空白:
        args += Tokenize(profile.QualityArgs.Replace("{q}", q))   # {q} -> 默认质量数值
    if profile.PresetArgs 非空白:
        args += Tokenize(profile.PresetArgs)

args += ["-frames:v", "3", "-f", "rawvideo", "-y", outputPath]    # outputPath 必须是【最后一个】参数
```

> 注意：`-frames:v 3` 且 **没有** `-pix_fmt`；输出容器固定 `rawvideo`，扩展名固定 `.raw`；`-y` 在输出路径之前；`-nostdin` 在这里有（与 `RunAsync` 不同）。

**完整试跑命令行形态（示例：VAAPI H.264，第 1 档）**：

```bash
<ffmpeg> -hide_banner -nostdin -loglevel error -vaapi_device /dev/dri/renderD128 \
         -f lavfi -i "testsrc2=duration=0.3:size=320x240:rate=25" \
         -vf format=nv12,hwupload -c:v h264_vaapi -qp 24 \
         -frames:v 3 -f rawvideo -y /tmp/ffmpegwebui-probe-h264_vaapi-full-<guid>.raw
```

**完整试跑命令行形态（示例：NVENC H.264，第 1 档）**：

```bash
<ffmpeg> -hide_banner -nostdin -loglevel error \
         -f lavfi -i "testsrc2=duration=0.3:size=320x240:rate=25" \
         -c:v h264_nvenc -rc vbr -cq 24 -b:v 0 -preset p5 -tune hq \
         -frames:v 3 -f rawvideo -y /tmp/ffmpegwebui-probe-h264_nvenc-full-<guid>.raw
```

**完整试跑命令行形态（示例：VideoToolbox H.265，第 1 档；注意无预设）**：

```bash
<ffmpeg> -hide_banner -nostdin -loglevel error \
         -f lavfi -i "testsrc2=duration=0.3:size=320x240:rate=25" \
         -c:v hevc_videotoolbox -q:v 60 \
         -frames:v 3 -f rawvideo -y /tmp/ffmpegwebui-probe-hevc_videotoolbox-full-<guid>.raw
```

**完整试跑命令行形态（示例：ProRes VideoToolbox，第 1 档；QualityArgs 为 null，只剩预设）**：

```bash
<ffmpeg> -hide_banner -nostdin -loglevel error \
         -f lavfi -i "testsrc2=duration=0.3:size=320x240:rate=25" \
         -c:v prores_videotoolbox -profile:v 3 \
         -frames:v 3 -f rawvideo -y /tmp/ffmpegwebui-probe-prores_videotoolbox-full-<guid>.raw
```

**`ExecuteProbeAsync(arguments, ct)`（`:353-449`）**

```text
启动: RedirectStandardOutput/Error/Input = true, UseShellExecute=false, CreateNoWindow=true, 编码 UTF-8
      outputPath = arguments[^1]                          # 最后一个参数即临时输出文件
outputSize = 0
启动异常 -> ProbeRunResult(false, $"无法启动 FFmpeg：{ex.Message}")

try { process.StandardInput.Close() } catch { }           # 关 stdin：即使某选项触发交互提问也不会挂起

stderrTask = ReadToEndAsync(stderr); stdoutTask = ReadToEndAsync(stdout)     # 并发全量读

timeoutCts = linked(ct) + CancelAfter(15 秒)               # ★ 单次试跑超时 = 15 秒
try   await process.WaitForExitAsync(timeoutCts.Token)
catch OperationCanceledException:
      timedOut = true; try { if !HasExited { process.Kill(entireProcessTree: true) } } catch { }

等待两个读取任务最多 2 秒（Task.WhenAny(WhenAll(...), Delay(2s))），尽力取结果（异常忽略）

若 outputPath 存在: outputSize = FileInfo(outputPath).Length（异常吞）；随后【无条件 File.Delete(outputPath)】（异常吞）

if timedOut:
    return ProbeRunResult(false, "试跑超时（15 秒内未完成），通常是驱动未安装或设备被占用。")

diagnostic = stderr 空白 ? stdout : stderr                # ★ 优先 stderr
exitCode   = try { process.ExitCode } catch { -1 }
if exitCode != 0:            return ProbeRunResult(false, diagnostic)
if outputSize <= 0:          return ProbeRunResult(false, "试跑没有产生任何编码输出，编码器可能无法真正工作。" + NewLine + diagnostic)
return ProbeRunResult(true, diagnostic)
```

**判定「可用」的两个条件同时成立**：退出码 == 0 **且** 临时输出文件字节数 > 0。

### 5.4 失败原因归纳 `SummarizeFailure`（`HardwareDetectionService.cs:451-486`）

**致命特征串表（逐字，匹配时 `OrdinalIgnoreCase`，`Contains`）**：

```text
"Cannot load"
"Device creation failed"
"No capable devices found"
"Error initializing"
"not available"
"Cannot open device"
"Failed to create"
"Unrecognized option"
"Option not found"
"Invalid argument"
"No device available"
"OpenCL error"
"Error creating"
```

**算法**：

```text
if diagnostic 空白 -> "试跑失败：FFmpeg 未给出更多信息"

lines = diagnostic.Split('\n', RemoveEmptyEntries | TrimEntries)     # 逐行 trim、去空行
        .Where(l => !l.StartsWith("ffmpeg version", OrdinalIgnoreCase))       # 丢版本号行
        .Where(l => !l.StartsWith("configuration:", OrdinalIgnoreCase))       # 丢 configuration 行
        .Where(l => !l.StartsWith("lib", OrdinalIgnoreCase))                  # 丢 "libavutil ..." 等横幅行
picked = lines.Where(含任一 FatalMarker).Take(2)
if picked 为空: picked = lines.TakeLast(2)
text = string.Join(" ", picked)                                # 单空格连接
return text.Length > 300 ? text[..300] + "…" : (text.Length == 0 ? "试跑失败" : text)
```

即：**最多 2 行、单空格拼接、截断到 300 字符（超出加 `…` U+2026）**；全空兜底 `"试跑失败"`。

### 5.5 推荐编码器 `GetRecommendedEncoderAsync`（`HardwareDetectionService.cs:180-244`）—— 精确优先级

```text
report    = await DetectAsync()                                   # forceRefresh = false
settings  = (await GetSettingsAsync()).Normalize()
codec     = NormalizeCodec(codec)                                  # 见 §5.5.1
available = report.Encoders.Where(e => e.IsAvailable)              # 全部（含软件）

# ── 1) 调用方/模板指定了必需编码器（可为族名或具体名） ────────────────
if requiredEncoder 非空白:
    for member in EncoderCatalog.ExpandFamily(requiredEncoder):     # 族名 -> 该族所有成员名；否则 [原值]
        hit = available.FirstOrDefault(e => e.Name == member (OrdinalIgnoreCase)
                                         && (codec 为空 || e.Codec == codec (OrdinalIgnoreCase)))
        if hit != null: return hit.Name
    if strict: return null                                         # 严格模式：该族无可用成员 -> null
    # 宽松模式继续往下走

# ── 2) 用户在设置中指定的首选编码器（仅具体编码器名，不展开族） ────────
if settings.PreferredHardwareEncoder 非空白:
    hit = available.FirstOrDefault(e => e.Name == PreferredHardwareEncoder (OrdinalIgnoreCase)
                                     && (codec 为空 || e.Codec == codec (OrdinalIgnoreCase)))
    if hit != null: return hit.Name

# ── 3) 自动选择 ───────────────────────────────────────────────────
candidates = EncoderCatalog.ByCodec(codec)                          # 按 SortPreference 升序
                     .Where(p => p.SupportsCurrentPlatform)
if settings.PreferHardwareAcceleration:                            # 默认 true
    for candidate in candidates.Where(IsHardware):                 # 按 SortPreference 顺序
        if available.Any(e => e.Name == candidate.Name (OrdinalIgnoreCase)): return candidate.Name
for candidate in candidates.Where(!IsHardware):
    if available.Any(e => e.Name == candidate.Name (OrdinalIgnoreCase)): return candidate.Name

# ── 4) 兜底（几乎不可能走到，但必须复刻）────────────────────────────
return codec == "hevc" ? (available 含 "libx265" ? "libx265" : "libx264") : "libx264"
```

要点：
* `codec` 为空时第 1、2 步的 codec 过滤条件失效（任意 codec 都能命中），第 3 步按空 codec 查目录只会得到空候选，直接落到兜底 `"libx264"`。
* 第 1 步的 **`ExpandFamily` 优先按族名匹配**；若传入的是具体编码器名（如 `hevc_nvenc`，其 `Family == "nvenc"` 但 `ByFamily("hevc_nvenc")` 为空），返回的成员列表就是 `["hevc_nvenc"]` 本身。
* 兜底分支**不检查** `libx264` 是否真的在构建清单里（`available` 里没有也照样返回名字）。
* `strict = true` 只影响第 1 步；第 2/3/4 步在 `strict` 下**不可达**（已在第 1 步 return null）。

**`NormalizeCodec`（`:247-264`）** —— `Trim().ToLowerInvariant()` 后查表：

| 输入（小写后） | 归一结果 |
| --- | --- |
| `avc`, `h264`, `x264`, `avc1` | `h264` |
| `h265`, `hevc`, `x265`, `hvc1`, `hev1` | `hevc` |
| `av1`, `av01` | `av1` |
| `vp9`, `vp09` | `vp9` |
| `vp8` | `vp8` |
| `prores` | `prores` |
| `aac` / `mp3` / `opus` / `flac` | 同名 |
| 其他 | 原样（已小写、已 trim） |
| 输入空白 | `""` |

**消费方的「严格模式」措辞（`Services/ConversionQueue.cs:542-551`，用于对齐 UI/错误文案）**：

```text
throw new InvalidOperationException(
    $"该模板要求使用 {family} 硬件编码，但本机不可用。" +
    "请改用「推荐」分类下会自动挑选编码器的模板，或在「设置 → 硬件加速能力」中确认本机支持的编码器。");
其中 family = required 为空 ? "指定的硬件编码器"
                          : EncoderCatalog.FamilyDisplayName(EncoderCatalog.Find(required)?.Family ?? required)
```

另外，消费方在拿到推荐编码器后会检查 `LastReport`：若该编码器 `SupportsQualityOptions == false`，则把画像降级为 `profile with { QualityArgs = null, PresetArgs = null }`（`ConversionQueue.cs:557-564`）——这解释了为什么 `SupportsQualityOptions` 必须持久化。

### 5.6 双层缓存与失效（`HardwareDetectionService.cs:21-31, 516-558`）

**A. 进程内报告缓存 `_lastReport`**

* TTL = **30 分钟**，判据 `(UtcNow - _lastReport.DetectedAt).TotalMinutes < 30`。
* 进入临界区前后各查一次。
* `forceRefresh = true` 时跳过并覆盖。

**B. 进程内试跑缓存 `_probeCache`（`ConcurrentDictionary<string,bool>`，Ordinal）**

* 键 = 编码器名（如 `h264_nvenc`）。
* **无 TTL**，进程生命周期内永久有效。
* **`forceRefresh` 不会清空它**（代码里没有任何 `_probeCache.Clear()`），所以「重新检测」在同一进程内实际只会刷新构建清单与软件编码器状态，硬件编码器仍是缓存的结论。
* 命中的 `true` 会连带把 `SupportsQualityOptions` 报成 `true`（见 §5.3 的缓存放大问题）；命中的 `false` 的 reason 固定为 `"试跑失败（缓存结果）"`。

**C. 数据库缓存（LiteDB 集合 `"encoders"`，行 = `HardwareEncoder`）**

`TryLoadFromDatabase(version, ffmpegPath)`（`:516-545`）—— 全部条件满足才复用：

```text
1) 读全部行；为空 -> null
2) 任意一行的 FFmpegVersion 与当前 version 不等（Ordinal） -> null        # 版本变化即失效
3) 所有行都 !IsAvailable -> null                                        # 上次完全失败就不复用
4) newest = max(LastCheckedAt)；若 (UtcNow - newest).TotalHours > 24 -> null   # 24 小时 TTL
5) 复用成功 -> HardwareDetectionReport {
       Encoders = 原始行（保持数据库顺序，不再排序）,
       DetectedAt = newest,            # 注意：不是"现在"
       FFmpegVersion = version, FFmpegPath = ffmpegPath,
       Platform = AppPaths.PlatformDisplayName,
       Summary = "使用上次检测结果",
       Hwaccels = [], SupportedFilters = []      # ⚠ 从数据库复用时这两项为空
   }
   # 任何异常 -> null
```

> ⚠ DB 缓存**不保存也不恢复** `Hwaccels` / `SupportedFilters`（`HardwareEncoder` 行里没有这两个字段）。因此复用 DB 缓存后 `report.HasFilter(name)` 会恒为 `true`（清单为空 = 放行）。Rust 侧若要 1:1，需要保留该行为；若要修正，应把这两项一并持久化并标注为偏离。

`SaveToDatabase(report)`（`:547-558`）：`db.Encoders.DeleteAll()` 后 `InsertBulk(report.Encoders)`；任何异常吞掉（缓存写失败不影响检测结果）。**覆盖式写入**：每次成功检测都整体替换。

### 5.7 `IsEncoderAvailableAsync`（`:173-178`）

```text
report = await DetectAsync()                                  # forceRefresh = false
return report.Encoders.Any(e => e.IsAvailable && e.Name == encoderName (OrdinalIgnoreCase))
```

### 5.8 「厂商必需编码器（vendor-required）」的完整处理链

| 环节 | 行为 |
| --- | --- |
| 模板声明 | `CommandTemplate.HardwareMode = Required` + `RequiredEncoder`（族名或具体名）；`TemplateService` 校验：`Required` 模式必须给 `RequiredEncoder`，且其族/名必须能在 `EncoderCatalog` 中找到，否则报校验错误（`Services/TemplateService.cs:343-380`） |
| 编码器解析 | `ConversionQueue.ResolveEncoderAsync`：`strict = (HardwareMode == Required)`，调用 `GetRecommendedEncoderAsync(codec, RequiredEncoder, strict)`（`ConversionQueue.cs:531-537`） |
| 族的展开 | `EncoderCatalog.ExpandFamily(required)` → 该族全体成员名（按目录顺序，即 nvenc → `h264_nvenc, hevc_nvenc, av1_nvenc`） |
| 族内挑人 | 只在 `available`（已检测可用）里挑，且 codec 必须匹配；挑到第一个即返回 |
| 严格失败 | `strict == true` 且族内无可用成员 → 返回 `null` → 上层抛「该模板要求使用 X 硬件编码，但本机不可用」 |
| 宽松失败 | `strict == false` → 继续走「设置首选 → 自动选择（硬件优先）→ 软件 → 兜底 libx264」 |
| 质量降级 | 若该编码器 `SupportsQualityOptions == false`，模板构建时丢弃 `QualityArgs` 与 `PresetArgs` |

---

## §6 EncoderCatalog —— 编码器画像完整表

> 源：`Services/EncoderCatalog.cs`。共 **31** 条画像（`EncoderCatalog.cs:63-332`）。

### 6.1 `EncoderProfile` 字段定义（`EncoderCatalog.cs:9-58`）

| 字段 | 类型 | 默认 | 语义 |
| --- | --- | --- | --- |
| `Name` | string | 必填 | FFmpeg 编码器名（如 `h264_nvenc`） |
| `DisplayName` | string | 必填 | 界面显示名 |
| `Type` | EncoderType | 必填 | 厂商/类型 |
| `Family` | string | 必填 | 族标识：`nvenc`/`qsv`/`amf`/`videotoolbox`/`vaapi`/`vulkan`/`mf`/`software` |
| `Codec` | string | 必填 | 目标编码格式：`h264`/`hevc`/`av1`/`vp9`/`prores`/`vp8`/`aac`/`mp3`/`opus`/`flac` |
| `IsHardware`（计算） | bool | — | `Type != Software` |
| `QualityArgs` | string? | null | 质量参数模板，`{q}` 会被替换为质量数值；**null 表示该编码器不接受质量参数** |
| `DefaultQuality` | int | **23** | 默认质量数值 |
| `QualityHint` | string | `"数值越小画质越好"` | 质量范围提示文案（**仅提示，不做任何数值缩放/钳制**） |
| `PresetArgs` | string? | null | 预设参数 |
| `UploadFilter` | string? | null | 送入编码器前必须附加的滤镜（如 `format=nv12,hwupload`）；null = 可直接吃系统内存帧 |
| `DeviceArgs` | string? | null | 按需附加的设备初始化参数（如 `-vaapi_device /dev/dri/renderD128`） |
| `MuxerArgs` | string? | null | 输出容器附加参数（如 HEVC in MP4 需 `-tag:v hvc1`） |
| `SortPreference` | int | **0** | 同编码格式内优先级，越小越优先 |
| `SupportsCurrentPlatform` | bool | **true** | 该编码器是否存在于当前平台 |

### 6.2 全表（31 行，列内容逐字来自源码）

| # | `Name` | 行号 | `DisplayName` | `Type` | `Family` | `Codec` | 平台可用 | `QualityArgs` | `DefaultQuality` | `QualityHint` | `PresetArgs` | `UploadFilter` | `DeviceArgs` | `MuxerArgs` | `SortPref` |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| 1 | `h264_nvenc` | 66-73 | NVIDIA NVENC H.264 | Nvenc | nvenc | h264 | Win+Linux | `-rc vbr -cq {q} -b:v 0` | 24 | CQ 值，18(高)~32(小体积) | `-preset p5 -tune hq` | — | — | — | 10 |
| 2 | `hevc_nvenc` | 74-82 | NVIDIA NVENC H.265/HEVC | Nvenc | nvenc | hevc | Win+Linux | `-rc vbr -cq {q} -b:v 0` | 28 | CQ 值，22(高)~35(小体积) | `-preset p5 -tune hq` | — | — | `-tag:v hvc1` | 10 |
| 3 | `av1_nvenc` | 83-90 | NVIDIA NVENC AV1 | Nvenc | nvenc | av1 | Win+Linux | `-rc vbr -cq {q} -b:v 0` | 30 | CQ 值，24(高)~38(小体积) | `-preset p5 -tune hq` | — | — | — | 10 |
| 4 | `h264_qsv` | 93-100 | Intel Quick Sync H.264 | Qsv | qsv | h264 | Win+Linux | `-global_quality {q}` | 24 | ICQ 值，18(高)~32(小体积) | `-preset medium` | — | — | — | 20 |
| 5 | `hevc_qsv` | 101-109 | Intel Quick Sync H.265/HEVC | Qsv | qsv | hevc | Win+Linux | `-global_quality {q}` | 28 | ICQ 值，22(高)~35(小体积) | `-preset medium` | — | — | `-tag:v hvc1` | 20 |
| 6 | `av1_qsv` | 110-117 | Intel Quick Sync AV1 | Qsv | qsv | av1 | Win+Linux | `-global_quality {q}` | 30 | ICQ 值，24(高)~38(小体积) | `-preset medium` | — | — | — | 20 |
| 7 | `vp9_qsv` | 118-125 | Intel Quick Sync VP9 | Qsv | qsv | vp9 | Win+Linux | `-global_quality {q}` | 30 | *（默认）*数值越小画质越好 | `-preset medium` | — | — | — | 20 |
| 8 | `h264_amf` | 128-135 | AMD AMF H.264 | Amf | amf | h264 | Win+Linux | `-rc cqp -qp_i {q} -qp_p {q}` | 24 | QP 值，18(高)~32(小体积) | `-quality balanced` | — | — | — | 30 |
| 9 | `hevc_amf` | 136-144 | AMD AMF H.265/HEVC | Amf | amf | hevc | Win+Linux | `-rc cqp -qp_i {q} -qp_p {q}` | 28 | QP 值，22(高)~35(小体积) | `-quality balanced` | — | — | `-tag:v hvc1` | 30 |
| 10 | `av1_amf` | 145-152 | AMD AMF AV1 | Amf | amf | av1 | Win+Linux | `-rc cqp -qp_i {q} -qp_p {q}` | 30 | *（默认）*数值越小画质越好 | `-quality balanced` | — | — | — | 30 |
| 11 | `h264_videotoolbox` | 155-162 | Apple VideoToolbox H.264 | VideoToolbox | videotoolbox | h264 | **macOS only** | `-q:v {q}` | **60** | 质量 0~100，越大画质越好 | *（显式 null）* | — | — | — | 5 |
| 12 | `hevc_videotoolbox` | 163-171 | Apple VideoToolbox H.265/HEVC | VideoToolbox | videotoolbox | hevc | **macOS only** | `-q:v {q}` | **60** | 质量 0~100，越大画质越好 | *（显式 null）* | — | — | `-tag:v hvc1` | 5 |
| 13 | `prores_videotoolbox` | 172-179 | Apple VideoToolbox ProRes | VideoToolbox | videotoolbox | prores | **macOS only** | **null**（无质量参数） | 23（未设置，取默认） | *（默认）*数值越小画质越好 | `-profile:v 3` | — | — | — | **0**（未设置） |
| 14 | `h264_vaapi` | 182-191 | VA-API H.264 (Intel/AMD) | Vaapi | vaapi | h264 | **Linux only** | `-qp {q}` | 24 | QP 值，18(高)~32(小体积) | *（显式 null）* | `format=nv12,hwupload` | `-vaapi_device /dev/dri/renderD128` | — | 15 |
| 15 | `hevc_vaapi` | 192-202 | VA-API H.265/HEVC (Intel/AMD) | Vaapi | vaapi | hevc | **Linux only** | `-qp {q}` | 28 | QP 值，22(高)~35(小体积) | *（显式 null）* | `format=nv12,hwupload` | `-vaapi_device /dev/dri/renderD128` | `-tag:v hvc1` | 15 |
| 16 | `av1_vaapi` | 203-212 | VA-API AV1 (Intel/AMD) | Vaapi | vaapi | av1 | **Linux only** | `-qp {q}` | 30 | *（默认）*数值越小画质越好 | *（显式 null）* | `format=nv12,hwupload` | `-vaapi_device /dev/dri/renderD128` | — | 15 |
| 17 | `h264_vulkan` | 215-222 | Vulkan Video H.264 | Vulkan | vulkan | h264 | Win+Linux | `-qp {q}` | 24 | *（默认）*数值越小画质越好 | *（显式 null）* | — | — | — | 40 |
| 18 | `hevc_vulkan` | 223-231 | Vulkan Video H.265/HEVC | Vulkan | vulkan | hevc | Win+Linux | `-qp {q}` | 28 | *（默认）*数值越小画质越好 | *（显式 null）* | — | — | `-tag:v hvc1` | 40 |
| 19 | `av1_vulkan` | 232-239 | Vulkan Video AV1 | Vulkan | vulkan | av1 | Win+Linux | `-qp {q}` | 30 | *（默认）*数值越小画质越好 | *（显式 null）* | — | — | — | 40 |
| 20 | `h264_mf` | 242-249 | Media Foundation H.264 | MediaFoundation | mf | h264 | **Windows only** | `-rate_control quality -quality {q}` | **70** | *（默认）*数值越小画质越好 | *（显式 null）* | — | — | — | 60 |
| 21 | `hevc_mf` | 250-258 | Media Foundation H.265/HEVC | MediaFoundation | mf | hevc | **Windows only** | `-rate_control quality -quality {q}` | **70** | *（默认）*数值越小画质越好 | *（显式 null）* | — | — | `-tag:v hvc1` | 60 |
| 22 | `libx264` | 261-267 | 软件编码 H.264 (x264) | Software | software | h264 | 全平台 | `-crf {q}` | 23 | CRF 值，18(高)~32(小体积) | `-preset medium` | — | — | — | 90 |
| 23 | `libx265` | 268-275 | 软件编码 H.265 (x265) | Software | software | hevc | 全平台 | `-crf {q}` | 28 | CRF 值，22(高)~35(小体积) | `-preset medium` | — | — | `-tag:v hvc1` | 90 |
| 24 | `libsvtav1` | 276-282 | 软件编码 AV1 (SVT-AV1) | Software | software | av1 | 全平台 | `-crf {q} -b:v 0` | 32 | CRF 值，24(高)~42(小体积) | `-preset 8` | — | — | — | 90 |
| 25 | `libaom-av1` | 283-289 | 软件编码 AV1 (libaom) | Software | software | av1 | 全平台 | `-crf {q} -b:v 0` | 32 | *（默认）*数值越小画质越好 | `-cpu-used 4` | — | — | — | **95** |
| 26 | `libvpx-vp9` | 290-296 | 软件编码 VP9 (libvpx) | Software | software | vp9 | 全平台 | `-crf {q} -b:v 0` | 32 | *（默认）*数值越小画质越好 | `-row-mt 1` | — | — | — | 90 |
| 27 | `libvpx` | 297-303 | 软件编码 VP8 (libvpx) | Software | software | vp8 | 全平台 | `-crf {q} -b:v 1M` | **10** | *（默认）*数值越小画质越好 | *（显式 null）* | — | — | — | 90 |
| 28 | `aac` | 304-310 | AAC 音频 (内置) | Software | software | aac | 全平台 | `-b:a {q}k` | **192** | *（默认）*数值越小画质越好 | *（显式 null）* | — | — | — | 90 |
| 29 | `libmp3lame` | 311-317 | MP3 音频 (LAME) | Software | software | mp3 | 全平台 | `-b:a {q}k` | **192** | *（默认）*数值越小画质越好 | *（显式 null）* | — | — | — | 90 |
| 30 | `libopus` | 318-324 | Opus 音频 | Software | software | opus | 全平台 | `-b:a {q}k` | **128** | *（默认）*数值越小画质越好 | *（显式 null）* | — | — | — | 90 |
| 31 | `flac` | 325-331 | FLAC 无损音频 | Software | software | flac | 全平台 | **null**（无质量参数） | 23（未设置，取默认） | *（默认）*数值越小画质越好 | *（显式 null）* | — | — | — | 90 |

说明（逐字要点）：
* 「*（默认）*」= 源码未设置该字段，取 record 默认值：`QualityHint = "数值越小画质更好"` 的**准确原文**是 `"数值越小画质越好"`（`EncoderCatalog.cs:36`）。
* 「*（显式 null）*」= 源码里真的写了 `PresetArgs = null`（`h264_videotoolbox`、`hevc_videotoolbox`、`h264_vaapi`、`hevc_vaapi`、`av1_vaapi`、`h264_vulkan`、`hevc_vulkan`、`av1_vulkan`、`h264_mf`、`hevc_mf`、`libvpx`、`aac`、`libmp3lame`、`libopus`、`flac`）。语义上与「未设置」等价（都是 null）。
* 软件编码器（#22-31）**不设置** `SupportsCurrentPlatform`，因此取默认 `true` = 全平台可用。
* `QualityArgs = null` 的画像有 2 个：`prores_videotoolbox`、`flac`（它们只靠预设或完全不传质量参数）。
* 平台限制汇总：`videotoolbox` 3 条 = macOS only；`vaapi` 3 条 = Linux only；`mf` 2 条 = Windows only；`nvenc` 3 + `qsv` 4 + `amf` 3 + `vulkan` 3 = 13 条为 Windows+Linux；软件 10 条全平台。**macOS 上只有 3 条硬件候选（VideoToolbox），且 FFmpeg 侧不存在 NVENC/QSV 画像——macOS 用户即使接了 eGPU/N 卡也不会有 NVENC 候选。**
* `DeviceArgs` **只出现在 VAAPI 系列**，值固定 `-vaapi_device /dev/dri/renderD128`。**全表没有任何 `-init_hw_device`、`-hwaccel`、`-hwaccel_output_format`、`-device`、`-qsv_device`**——Rust 移植若需要多 GPU 选择/设备路径可配置化，属于新增行为而非复刻。
* `MuxerArgs` 只有 `-tag:v hvc1` 一种值（所有 HEVC 画像），用于 MP4/MOV 兼容。

### 6.3 质量参数如何使用（**没有** 1-5 档映射表）

* 数值来源优先级（`CommandLineBuilder.BuildResolver`，`Services/CommandLineBuilder.cs:439-463`）：`context.Quality`（用户/模板传入）→ `encoder.DefaultQuality` → `23`。
* `{q}`、`{quality}`、`{crf}` 三个占位符**语义完全相同**，都是同一个质量整数（`:440-442, 458-461`）。
* `{quality_args}` = `encoder.QualityArgs` 中 `{q}` → 质量数值，然后在**分词层面**展开成多个参数（`:463, 111-118`）。例：`h264_nvenc` 展开为 `-rc` `vbr` `-cq` `24` `-b:v` `0`。
* **不存在任何「档位 → 数值」映射表**（参考实现里没有 `quality 1-5 → -cq 19..35` 这类逻辑）。质量是**直通**的整数，不做范围钳制、不做缩放、不做逆向（VideoToolbox 的「越大越好」只体现在 `QualityHint` 文案上）。
* `-b:a {q}k` 的展开结果形如 `-b:a 192k`（`{q}` 后紧跟字面 `k`，分词时是一个 token）。
* 若检测报告说某编码器 `SupportsQualityOptions == false`，消费方会把 `QualityArgs` 与 `PresetArgs` **同时置空**（`ConversionQueue.cs:561-564`）。

### 6.4 目录查询函数（`EncoderCatalog.cs:334-377`）

| 函数 | 语义 |
| --- | --- |
| `Profiles` | 全部 31 条（IReadOnlyList，保持源码顺序） |
| `Find(name)` | 按 `Name` 匹配（**OrdinalIgnoreCase**），无则 null |
| `ByFamily(family)` | `Family` 匹配（OrdinalIgnoreCase），**保持源码顺序** |
| `ByCodec(codec)` | `Codec` 匹配（OrdinalIgnoreCase）后 **`OrderBy(SortPreference)`** |
| `SoftwareFallback(codec)` | `Type == Software && Codec 匹配` 后 `OrderBy(SortPreference).FirstOrDefault()`（同 SortPreference 时按源码顺序取第一个） |
| `ExpandFamily(nameOrFamily)` | `ByFamily(入参)` 的 `Name` 列表；**为空则返回 `[入参原值]`** |
| `FamilyDisplayName(family)` | `family.ToLowerInvariant()` 查表（见下） |

`FamilyDisplayName` 映射（`:366-377`）：

| family | 显示名 |
| --- | --- |
| `nvenc` | `NVIDIA NVENC` |
| `qsv` | `Intel Quick Sync` |
| `amf` | `AMD AMF` |
| `videotoolbox` | `Apple VideoToolbox` |
| `vaapi` | `Linux VA-API` |
| `vulkan` | `Vulkan Video` |
| `mf` | `Windows Media Foundation` |
| `software` | `软件编码` |
| 其它 | 原样返回入参 |

### 6.5 画像字段如何落到真实命令行（供 Rust 端复刻模板引擎）

按 `CommandLineBuilder.Build`（`Services/CommandLineBuilder.cs:86-215`）的插入顺序：

1. 模板文本 `CommandArgs` → 先 `Tokenize`（类 POSIX：单引号/双引号/反斜杠转义）。
2. `EncoderProfile.UploadFilter` 非空：合并进已有 `-vf` 链尾部（`chain.TrimEnd(',') + "," + uploadFilter`）；若模板用 `-filter_complex`/`-lavfi` 则**不合并**、只产生告警；否则在 `-c:v`/`-codec:v`/`-vcodec` 之前插入 `-vf <uploadFilter>`（`:345-382`）。
3. token 级占位符替换；`ArgumentGroupPlaceholders = { quality_args, preset_args, hwupload_args, device_args, muxer_args, hwaccel, global_args, extra_args }` 展开为多 token，空值不产生参数。
4. `HardwareDecodeArgs` 非空且参数里没有 `-hwaccel*`：插入到输入文件路径**前一个位置**。
5. `DeviceArgs`：若参数列表里还没有该参数的首 token，则插入到**第一个以 `-` 开头且不是 `-y`/`-n` 的参数之前**。
6. `MuxerArgs`：若参数里没有 `-tag*`，插入到**输出路径之前**。
7. `GlobalArguments`（`AppSettings.GlobalFFmpegArguments`）：插入到输出路径之前。

（第 4-7 步的行为属于模板引擎范围，此处列出是为了让 Rust 端知道画像字段的落点；`RunAsync` 还会在其前面追加 `-hide_banner -y|-n -progress pipe:2`，见 §3.9。）

---

## §7 魔法常量汇总（可直接做成 Rust `const`）

| 常量 | 值 | 出处 |
| --- | --- | --- |
| FFmpegInfo 缓存 TTL | **30 分钟** | `FFmpegService.cs:153` |
| 硬件检测报告内存 TTL | **30 分钟** | `HardwareDetectionService.cs:38,46` |
| 硬件检测 DB 缓存 TTL | **24 小时** | `HardwareDetectionService.cs:529` |
| `RunCaptureAsync` 超时 | **30 秒**（超时后杀进程树） | `FFmpegService.cs:893` |
| 单次试跑超时 | **15 秒**（超时后杀进程树） | `HardwareDetectionService.cs:390` |
| 优雅停止（写 `q`）后强杀等待 | **5 秒** | `FFmpegService.cs:814` |
| 结束后等待读取线程收口 | **3 秒** | `FFmpegService.cs:708` |
| 试跑 stderr/stdout 收口等待 | **2 秒** | `HardwareDetectionService.cs:407` |
| stderr 尾部缓冲行数 | **200 行** | `FFmpegService.cs:587` |
| Diagnostic 截断长度 | **8000 字符**（保留尾部，前缀 `…（已截断前部输出）\n`） | `FFmpegService.cs:747,860-864` |
| 试跑失败原因截断 | **300 字符** + `…`；最多 **2** 行 | `HardwareDetectionService.cs:485` |
| 失败原因优选行数 | **2**（命中 FatalMarkers 的前 2 行；否则最后 2 行） | `HardwareDetectionService.cs:479-482` |
| 半成品清理阈值 | 文件 **< 64 MiB** 才删 | `FFmpegService.cs:848` |
| 试跑并发上限 | `clamp(CPU 核数 / 2, 2, 4)` | `HardwareDetectionService.cs:94` |
| 试跑测试片 | `testsrc2=duration=0.3:size=320x240:rate=25` + `-frames:v 3` + `-f rawvideo` | `HardwareDetectionService.cs:323,347` |
| 进度 speed 阈值（算 ETA） | `speed > 0.01` | `FFmpegService.cs:990` |
| 进度百分比钳制 | `[0, 100]` | `FFmpegService.cs:986` |
| `-progress` 输出目标 | `pipe:2`（stderr） | `FFmpegService.cs:564-565` |
| 任务日志行上限 | `MaxLogLines` 默认 **500**，区间 **[50, 20000]** | `Models/AppSettings.cs:47,71` |
| 默认质量（`DefaultQuality` 缺省） | **23** | `EncoderCatalog.cs:33` |
| 默认质量提示 | `数值越小画质越好` | `EncoderCatalog.cs:36` |
| 默认并发任务数 / 端口 | `MaxConcurrentTasks = 1`、`ListenPort = 5265`（0 = 自动） | `Models/AppSettings.cs:44,65` |
| 应用版本 | `2.0.0` | `AppInfo.cs:10` |
| 硬件检测集合名 | `encoders` | `HardwareDetectionService.cs:21` |

---

## §8 Rust 移植决策点 / 已知歧义（必须先拍定）

1. **`out_time_ms` 除以 1e6**（`FFmpegService.cs:1016-1020`）。这是 ffmpeg 的历史怪癖（`out_time_ms` 实为微秒）。**必须保留**，否则进度会瞬间 100%。
2. **`ffmpeg`/`ffprobe` 永不返回 `Option`**，找不到时是裸名 + `*Resolved=false` + Hint 文案（§2.1、§2.3）。若 Rust 侧改成 `Option`，必须同时复刻 Hint 与 UI 行为，否则「FFmpeg 未安装但 PATH 后来修好」的场景会退化。
3. **`ffprobe` 候选顺序与 ffmpeg 相反**（probe 专用序列不含 PATH；先平台位置再内置目录，§2.8）。
4. **`-progress pipe:2` 而不是解析 `-stats` 文本**；且已知 key 白名单固定 11 个（`fps`/`total_size`/`dup_frames`/`drop_frames` 会被吸收但不暴露，§3.12）。
5. **取消 = 写 `q` 到 stdin + 5 秒后杀树；超时 = 直接杀树**（不走 `q`）。Rust 需要 `Child::stdin` 常开 + 独立的「5 秒看门狗」任务。
6. **`_probeCache` 只存 bool，且 `forceRefresh` 不清它**，导致第 2 档成功被「放大」为 `SupportsQualityOptions=true`（§5.3）。要 1:1 就照抄；要修正就用三态枚举并记录为偏离。
7. **DB 缓存不保存 `Hwaccels` / `SupportedFilters`**，复用后 `HasFilter()` 恒为 true（§5.6）。
8. **`FFmpegPaths.FFmpegResolved` 在用户显式配置分支被硬编码为 `true`**，即使配置的是裸名（§2.3）。
9. **VideoToolbox 的质量方向与其他厂商相反**（0~100 越大越好 vs 越小越好），且参考实现不做任何换算，只有一个文案提示。Rust 侧若要做统一的「画质 1-5 档」UI，必须自己引入映射表（参考实现中**不存在**）。
10. **没有 `-init_hw_device` / `-hwaccel` 画像字段**；只有 VAAPI 的硬编码 `-vaapi_device /dev/dri/renderD128`（多 GPU 机器上是明显的局限，需决定是否可配置化）。
11. **`ParseListingTable` 的启发式**（首字符大写 + 标志位宽度 ≤ Flags + 首标志 ∈ `V A S D E T`）依赖 ffmpeg 输出格式稳定；Rust 侧若用更严格的列宽解析会与参考实现产生差异（例如未来 ffmpeg 加长标志段）。
12. **`AppInfo.FullVersion` 里的 `.NET {Environment.Version}`** 必须替换为 Rust 侧等价信息（crate 版本 / rustc 版本），属于必然的文案差异。
13. **「macOS 无 NVENC/QSV 画像」**：平台表照抄即可，但如果目标用户群里有 macOS + 外置 NVIDIA 的场景，需要新增画像（新增行为）。
14. **`-n` 时 ffmpeg 打印 `Not overwriting - exiting`、退出码非 0**，参考实现据此置 `SkippedExistingFile` 并在 Diagnostic 前加中文前缀（§3.11）；Rust 侧若改用 `-nostdin` 或先做存在性预检，需要同步调整 `Success` 判定语义。
15. **UI 文案为简体中文**（Hint、Summary、UnavailableReason、错误提示）。Rust 端若引入 i18n，应把这批字符串集中到资源文件，且注意 `"…"`、`"、"`
    这些全角标点在 Summary 中的拼接方式（`、` 连接、前 4 个去重显示名）。
