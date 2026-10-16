//! 硬件编码器检测（规格 `ffmpeg-core.md` §5，源 `Services/HardwareDetectionService.cs`）。
//!
//! 三级缓存（规格 §5.6，全部命中即不用真跑）：
//!
//! | 层级 | 内容 | TTL | 失效 |
//! | --- | --- | --- | --- |
//! | A. 内存报告 `last_report` | 完整检测报告 | 30 分钟 | `invalidate()` / `force_refresh` |
//! | B. 试跑缓存 `probe_cache` | 单个编码器的试跑结论 | **进程级永久** | 无（`force_refresh` **不清**它） |
//! | C. 数据库 `encoders` 表 | 上次检测结果 | **24 小时**（且 ffmpeg 版本不一致即失效） | 版本变化 / 全部不可用 |
//!
//! 检测流程：平台过滤 → 构建清单过滤 → 试跑（第 1 档带质量参数与预设，第 2 档最简参数）→
//! 排序（硬件优先）→ 汇总文案（规格 §5.2）。

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::{Arc, Mutex};

use chrono::{DateTime, Utc};

use crate::ffmpeg::encoder_catalog::{EncoderCatalog, EncoderProfile, EncoderType};
use crate::ffmpeg::service::{FFmpegError, FFmpegService};
use crate::ffmpeg::util;
use crate::models::hardware::HardwareEncoder;

/// 单次试跑超时（秒，规格 §7）。
pub const TRIAL_TIMEOUT_SECS: u64 = 15;
/// 试跑结束后等待 stdout/stderr 收口的秒数（规格 §7）。
pub const TRIAL_READER_WAIT_SECS: u64 = 2;
/// 硬件检测报告的内存缓存 TTL（分钟，规格 §7）。
pub const REPORT_CACHE_TTL_MINUTES: i64 = 30;
/// 数据库缓存的 TTL（小时，规格 §7）。
pub const DATABASE_CACHE_TTL_HOURS: i64 = 24;
/// 不可用原因的截断长度（规格 §7：300 字符 + 省略号）。
pub const REASON_MAX_CHARS: usize = 300;
/// 试跑测试片（规格 §7，逐字）。
pub const TRIAL_INPUT: &str = "testsrc2=duration=0.3:size=320x240:rate=25";

/// 致命特征串表（13 条，逐字，规格 §5.4；匹配时忽略大小写且用 `contains`）。
pub const FATAL_MARKERS: &[&str] = &[
    "Cannot load",
    "Device creation failed",
    "No capable devices found",
    "Error initializing",
    "not available",
    "Cannot open device",
    "Failed to create",
    "Unrecognized option",
    "Option not found",
    "Invalid argument",
    "No device available",
    "OpenCL error",
    "Error creating",
];

/// 一个编码器的检测结果（运行期模型，不落库）。
#[derive(Debug, Clone, serde::Serialize)]
pub struct DetectedEncoder {
    /// 编码器 id（= ffmpeg 编码器名）
    pub encoder_id: String,
    /// ffmpeg 编码器名（如 `h264_nvenc`）
    pub encoder_name: String,
    /// 界面显示名（如 `NVIDIA NVENC H.264`）
    pub display_name: String,
    /// 厂商类型
    pub encoder_type: EncoderType,
    /// 族标识：`nvenc` / `qsv` / `amf` / `videotoolbox` / `vaapi` / `vulkan` / `mf` / `software`
    pub family: String,
    /// 目标编码格式（`h264` / `hevc` / `av1` / ...）
    pub target_codec: String,
    /// 是否可用（试跑通过）
    pub available: bool,
    /// 不可用原因（中文，≤300 字符；可用时为空串）
    pub reason: String,
    /// 是否通过了**带质量参数与预设**的完整试跑。
    ///
    /// 对应原实现的 `SupportsQualityOptions`：`false` 表示「只能用最简参数」，
    /// 消费方据此丢弃画像的 `quality_args` / `preset_args`（规格 §5.2 末尾）。
    pub verified_by_trial: bool,
    /// 试跑用的完整命令行（仅用于展示/排障）
    pub trial_command: Option<String>,
    /// 是否因为「不在当前 FFmpeg 构建清单中」而不可用
    pub missing_from_build: bool,
    /// 是否硬件编码器（`encoder_type != Software`）
    pub is_hardware: bool,
    /// 该行检测时间（UTC），数据库缓存 TTL 判据
    pub last_checked_at: DateTime<Utc>,
    /// 检测时的 FFmpeg 版本（数据库缓存有效性判据）
    pub ffmpeg_version: String,
}

impl Default for DetectedEncoder {
    fn default() -> Self {
        Self {
            encoder_id: String::new(),
            encoder_name: String::new(),
            display_name: String::new(),
            encoder_type: EncoderType::Software,
            family: "software".to_string(),
            target_codec: String::new(),
            available: false,
            reason: String::new(),
            verified_by_trial: true,
            trial_command: None,
            missing_from_build: false,
            is_hardware: false,
            last_checked_at: Utc::now(),
            ffmpeg_version: String::new(),
        }
    }
}

/// 一次完整检测的报告（规格 §4.4 `HardwareDetectionReport`）。
#[derive(Debug, Clone, serde::Serialize)]
pub struct HardwareReport {
    /// 检测时间（内存 TTL 判据；复用时是数据库里的最新时间）
    pub probed_at: DateTime<Utc>,
    /// 平台显示名（`Windows` / `macOS` / `Linux`）
    pub platform: String,
    /// 可读的硬件摘要（尽力而为，仅依据 ffmpeg 能力清单与试跑结果）
    pub gpu_summary: String,
    /// 全部编码器（含不可用），已排序
    pub encoders: Vec<DetectedEncoder>,
    /// 编码格式 → 推荐编码器 id（自动挑选，不考虑模板指定的必需编码器）
    pub recommended: BTreeMap<String, String>,
    /// 中文一句话摘要，供日志/界面
    pub summary: String,
    /// 检测时的 FFmpeg 版本（`未检测到` 表示没找到 ffmpeg）
    pub ffmpeg_version: String,
    /// 检测时的 FFmpeg 路径
    pub ffmpeg_path: String,
    /// `-hwaccels` 清单
    pub hwaccels: Vec<String>,
    /// `-filters` 清单
    pub supported_filters: Vec<String>,
}

impl Default for HardwareReport {
    fn default() -> Self {
        Self {
            probed_at: Utc::now(),
            platform: crate::paths::platform_display_name().to_string(),
            gpu_summary: String::new(),
            encoders: Vec::new(),
            recommended: BTreeMap::new(),
            summary: String::new(),
            ffmpeg_version: String::new(),
            ffmpeg_path: String::new(),
            hwaccels: Vec::new(),
            supported_filters: Vec::new(),
        }
    }
}

impl HardwareReport {
    /// 滤镜是否可用：清单为空视为「未知，放行」（规格 §4.4 `HasFilter`）。
    pub fn has_filter(&self, name: &str) -> bool {
        self.supported_filters.is_empty()
            || self.supported_filters.iter().any(|f| f.eq_ignore_ascii_case(name.trim()))
    }

    /// 全部可用的硬件编码器。
    pub fn available_hardware_encoders(&self) -> Vec<&DetectedEncoder> {
        self.encoders.iter().filter(|e| e.available && e.is_hardware).collect()
    }

    /// 是否存在可用的硬件编码器。
    pub fn has_hardware_acceleration(&self) -> bool {
        self.encoders.iter().any(|e| e.available && e.is_hardware)
    }

    /// 全部可用编码器（含软件）。
    pub fn available_encoders(&self) -> Vec<&DetectedEncoder> {
        self.encoders.iter().filter(|e| e.available).collect()
    }

    /// 按编码器名查找（忽略大小写）。
    pub fn find_encoder(&self, encoder_name: &str) -> Option<&DetectedEncoder> {
        self.encoders.iter().find(|e| e.encoder_name.eq_ignore_ascii_case(encoder_name.trim()))
    }

    /// 检测缓存是否还在 30 分钟 TTL 内。
    pub fn is_fresh(&self) -> bool {
        (Utc::now() - self.probed_at).num_minutes() < REPORT_CACHE_TTL_MINUTES
    }
}

/// 自动挑选编码器时的用户偏好（对应 `AppSettings` 里与本模块相关的两项）。
#[derive(Debug, Clone, serde::Serialize)]
pub struct HardwarePreferences {
    /// 是否优先硬件编码（`AppSettings.PreferHardwareAcceleration`，默认 true）
    pub prefer_hardware_acceleration: bool,
    /// 用户指定的首选编码器（**具体编码器名**，不是族名；`AppSettings.PreferredHardwareEncoder`）
    pub preferred_hardware_encoder: Option<String>,
}

impl Default for HardwarePreferences {
    fn default() -> Self {
        Self { prefer_hardware_acceleration: true, preferred_hardware_encoder: None }
    }
}

/// 检测结果的持久化后端（对应参考实现的 LiteDB `encoders` 集合，规格 §5.6 C）。
///
/// `crate::store::Store` 已经实现了本 trait，直接 `with_cache_store(service, Arc::new(store))` 即可；
/// 没有数据库时用 [`HardwareDetector::new`]，第 C 层缓存自动停用。
pub trait EncoderCacheStore: Send + Sync + 'static {
    /// 读取全部缓存行（空 = 无缓存）。
    fn load_cached_encoders(
        &self,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<Vec<HardwareEncoder>, FFmpegError>> + Send + '_>,
    >;

    /// 覆盖式写入（对应 C# 的 `DeleteAll + InsertBulk`）。
    fn save_cached_encoders<'a>(
        &'a self,
        rows: &'a [HardwareEncoder],
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<(), FFmpegError>> + Send + 'a>,
    >;
}

impl EncoderCacheStore for crate::store::Store {
    fn load_cached_encoders(
        &self,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<Vec<HardwareEncoder>, FFmpegError>> + Send + '_>,
    > {
        Box::pin(async move { self.load_encoders().await.map_err(|err| FFmpegError::Other(err.to_string())) })
    }

    fn save_cached_encoders<'a>(
        &'a self,
        rows: &'a [HardwareEncoder],
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<(), FFmpegError>> + Send + 'a>,
    > {
        Box::pin(async move {
            self.clear_encoders().await.map_err(|err| FFmpegError::Other(err.to_string()))?;
            self.upsert_encoders(rows).await.map_err(|err| FFmpegError::Other(err.to_string()))
        })
    }
}

/// 试跑缓存的取值（**三态**，见 [`HardwareDetector::probe_encoder`] 的 PORT NOTE）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProbeCached {
    /// 第 1 档（带质量参数与预设）通过
    Full,
    /// 仅第 2 档（最简参数）通过
    Minimal,
    /// 两档都失败
    Failed,
}

/// 一次试跑的结果。
#[derive(Debug, Clone)]
struct ProbeOutcome {
    available: bool,
    /// 是否通过了带质量参数的完整试跑（= `SupportsQualityOptions`）
    verified_by_trial: bool,
    reason: String,
    trial_command: Option<String>,
}

/// 试跑进程的结果。
#[derive(Debug, Clone)]
struct TrialResult {
    success: bool,
    diagnostic: String,
}

struct DetectorInner {
    service: Arc<FFmpegService>,
    last_report: Mutex<Option<HardwareReport>>,
    probe_cache: Mutex<HashMap<String, ProbeCached>>,
    preferences: Mutex<HardwarePreferences>,
    store: Option<Arc<dyn EncoderCacheStore>>,
    /// 整段检测串行化（C# 用 `SemaphoreSlim(1,1)` 做 double-checked locking）
    gate: tokio::sync::Mutex<()>,
}

/// 硬件检测服务（对应 C# `HardwareDetectionService`，Singleton）。
#[derive(Clone)]
pub struct HardwareDetector {
    inner: Arc<DetectorInner>,
}

impl HardwareDetector {
    /// 不接数据库的探测器（内存两层缓存 + 进程级试跑缓存）。
    pub fn new(service: Arc<FFmpegService>) -> Self {
        Self::build(service, None)
    }

    /// 接上持久化后端（启用 24 小时数据库缓存，规格 §5.6 C）。
    pub fn with_cache_store(service: Arc<FFmpegService>, store: Arc<dyn EncoderCacheStore>) -> Self {
        Self::build(service, Some(store))
    }

    /// 带用户偏好构造（自动挑选编码器时用）。
    pub fn with_preferences(
        service: Arc<FFmpegService>,
        preferences: HardwarePreferences,
    ) -> Self {
        let detector = Self::build(service, None);
        detector.set_preferences(preferences);
        detector
    }

    fn build(service: Arc<FFmpegService>, store: Option<Arc<dyn EncoderCacheStore>>) -> Self {
        Self {
            inner: Arc::new(DetectorInner {
                service,
                last_report: Mutex::new(None),
                probe_cache: Mutex::new(HashMap::new()),
                preferences: Mutex::new(HardwarePreferences::default()),
                store,
                gate: tokio::sync::Mutex::new(()),
            }),
        }
    }

    /// 画像查询（对应 `EncoderCatalog.Find`）。
    pub fn resolve_profile(encoder_name: &str) -> Option<&'static EncoderProfile> {
        EncoderCatalog::find(encoder_name)
    }

    /// 设置自动挑选时的偏好（`PreferHardwareAcceleration` / `PreferredHardwareEncoder`）。
    pub fn set_preferences(&self, preferences: HardwarePreferences) {
        if let Ok(mut guard) = self.inner.preferences.lock() {
            *guard = preferences;
        }
    }

    /// 当前偏好。
    pub fn preferences(&self) -> HardwarePreferences {
        match self.inner.preferences.lock() {
            Ok(guard) => guard.clone(),
            Err(poisoned) => poisoned.into_inner().clone(),
        }
    }

    /// 从设置里同步偏好（`PreferHardwareAcceleration` / `PreferredHardwareEncoder`）。
    ///
    /// 设置变更后还会清空报告缓存，避免继续用旧偏好推荐编码器。
    pub fn apply_settings(&self, settings: &crate::models::AppSettings) {
        let preferred = settings.preferred_hardware_encoder.trim();
        self.set_preferences(HardwarePreferences {
            prefer_hardware_acceleration: settings.prefer_hardware_acceleration,
            preferred_hardware_encoder: if preferred.is_empty() {
                None
            } else {
                Some(preferred.to_string())
            },
        });
        self.invalidate();
    }

    /// 上一次检测报告（`None` = 从未检测）。
    pub fn last_report(&self) -> Option<HardwareReport> {
        match self.inner.last_report.lock() {
            Ok(guard) => guard.clone(),
            Err(poisoned) => poisoned.into_inner().clone(),
        }
    }

    /// 清空**报告**缓存（30 分钟那层）。
    ///
    /// PORT NOTE：与参考实现一致，**不清**试跑缓存（规格 §5.6 B：`forceRefresh` 不会清
    /// `_probeCache`），因此同一进程内「重新检测」只会刷新构建清单与软件编码器状态。
    pub fn invalidate(&self) {
        if let Ok(mut guard) = self.inner.last_report.lock() {
            *guard = None;
        }
    }

    /// 某个编码器当前是否可用（内部走 `detect(false)`）。
    pub async fn is_encoder_available(&self, encoder_name: &str) -> bool {
        let report = self.detect(false).await;
        report
            .encoders
            .iter()
            .any(|e| e.available && e.encoder_name.eq_ignore_ascii_case(encoder_name.trim()))
    }

    /// 模板/API 声明的必需编码器能否落地（族名或具体名）。
    ///
    /// 与参考实现的 `strict = true` 分支等价：该族在本机没有可用成员时返回 `None`，
    /// 不会退而求其次（规格 §5.8）。
    pub fn pick_required(&self, report: &HardwareReport, required: &str) -> Option<String> {
        let required = required.trim();
        if required.is_empty() {
            return None;
        }
        for member in EncoderCatalog::expand_family(required) {
            if let Some(hit) = report
                .encoders
                .iter()
                .find(|e| e.available && e.encoder_name.eq_ignore_ascii_case(&member))
            {
                return Some(hit.encoder_name.clone());
            }
        }
        None
    }

    /// 推荐编码器（规格 §5.5 的完整四级优先级）。
    ///
    /// * `required` 非空 → **严格模式**（第 1 步找不到可用成员就返回 `None`），
    ///   这与模板 `HardwareMode = Required` 的语义一致；
    /// * `required` 为空 → 用户首选（第 2 步）→ 自动挑选（第 3 步）→ 兜底（第 4 步）。
    ///
    /// 需要参考实现的宽松语义（`strict = false`）时用
    /// [`Self::recommended_encoder_with`]。
    pub async fn recommended_encoder(&self, codec: &str, required: Option<&str>) -> Option<String> {
        let strict = required.map(|r| !r.trim().is_empty()).unwrap_or(false);
        self.recommended_encoder_with(codec, required, strict).await
    }

    /// 带显式 `strict` 的推荐编码器（规格 §5.5）。
    pub async fn recommended_encoder_with(
        &self,
        codec: &str,
        required: Option<&str>,
        strict: bool,
    ) -> Option<String> {
        let report = self.detect(false).await;
        let codec = EncoderCatalog::normalize_codec(codec);
        let available: Vec<&DetectedEncoder> = report.encoders.iter().filter(|e| e.available).collect();

        // 1) 模板/调用方指定的必需编码器（族名或具体名）
        if let Some(required) = required.map(str::trim).filter(|value| !value.is_empty()) {
            for member in EncoderCatalog::expand_family(required) {
                if let Some(hit) = available.iter().find(|e| {
                    e.encoder_name.eq_ignore_ascii_case(&member)
                        && (codec.is_empty() || e.target_codec.eq_ignore_ascii_case(&codec))
                }) {
                    return Some(hit.encoder_name.clone());
                }
            }
            if strict {
                return None; // Required 模式：不偷偷换别的厂商
            }
        }

        let preferences = self.preferences();

        // 2) 用户在设置里指定的首选编码器（仅具体编码器名，不展开族）
        if let Some(preferred) = preferences
            .preferred_hardware_encoder
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            if let Some(hit) = available.iter().find(|e| {
                e.encoder_name.eq_ignore_ascii_case(preferred)
                    && (codec.is_empty() || e.target_codec.eq_ignore_ascii_case(&codec))
            }) {
                return Some(hit.encoder_name.clone());
            }
        }

        // 3) 自动选择 + 4) 兜底
        recommend_for_codec(&report.encoders, &codec, &preferences)
    }

    /// 执行（或复用缓存）一次完整检测。
    pub async fn detect(&self, force_refresh: bool) -> HardwareReport {
        if !force_refresh {
            if let Some(report) = self.cached_report() {
                return report;
            }
        }

        let _gate = self.inner.gate.lock().await;

        // double-checked locking（规格 §5.2 第 2a 步）
        if !force_refresh {
            if let Some(report) = self.cached_report() {
                return report;
            }
        }

        let report = self.detect_inner(force_refresh).await;
        if let Ok(mut guard) = self.inner.last_report.lock() {
            *guard = Some(report.clone());
        }
        report
    }

    fn cached_report(&self) -> Option<HardwareReport> {
        let guard = match self.inner.last_report.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        guard.as_ref().filter(|report| report.is_fresh()).cloned()
    }

    async fn detect_inner(&self, force_refresh: bool) -> HardwareReport {
        let paths = self.inner.service.paths();
        let platform = crate::paths::platform_display_name().to_string();
        let info = self.inner.service.info(force_refresh).await;

        // ── 无 FFmpeg：报告里只有软件编码器条目，且不写数据库 ────────────
        let Some(info) = info.filter(|info| info.available) else {
            let reason =
                format!("未找到可用的 FFmpeg，请先在设置中指定 FFmpeg 路径。{}", paths.hint);
            return HardwareReport {
                probed_at: Utc::now(),
                platform,
                gpu_summary: "未检测到 FFmpeg，无法探测硬件加速能力。".to_string(),
                encoders: build_unavailable_catalog(&reason, "未检测到"),
                recommended: BTreeMap::new(),
                summary: "未检测到 FFmpeg，无法进行硬件加速检测。".to_string(),
                ffmpeg_version: "未检测到".to_string(),
                ffmpeg_path: paths.ffmpeg.clone(),
                hwaccels: Vec::new(),
                supported_filters: Vec::new(),
            };
        };

        let version = info.version.clone();

        // ── C 层缓存：数据库（24 小时 / 版本一致 / 至少一个可用）─────────
        if !force_refresh {
            if let Some(report) = self.try_load_from_database(&version, &info.path).await {
                return report;
            }
        }

        let build_encoders: HashSet<String> =
            info.encoders.iter().map(|name| name.to_ascii_lowercase()).collect();
        let candidates: Vec<&'static EncoderProfile> =
            EncoderCatalog::all().iter().filter(|profile| profile.is_platform_supported()).collect();

        let results: Arc<Mutex<Vec<DetectedEncoder>>> = Arc::new(Mutex::new(Vec::new()));
        let semaphore = Arc::new(tokio::sync::Semaphore::new(probe_concurrency()));
        let mut probes = tokio::task::JoinSet::new();

        for profile in candidates {
            if !profile.is_hardware() {
                // 软件编码器：只查构建清单，绝不试跑（规格 §5.2 第 2l 步）
                let available = build_encoders.contains(&profile.name.to_ascii_lowercase());
                let reason = if available {
                    String::new()
                } else {
                    "该编码器不在当前 FFmpeg 构建中".to_string()
                };
                push_result(&results, create_entry(profile, available, reason, true, !available, &version, None));
                continue;
            }
            if !build_encoders.contains(&profile.name.to_ascii_lowercase()) {
                // 硬件编码器不在构建里：不试跑
                push_result(
                    &results,
                    create_entry(
                        profile,
                        false,
                        "该编码器不在当前 FFmpeg 构建中（FFmpeg 未启用对应硬件加速）".to_string(),
                        true,
                        true,
                        &version,
                        None,
                    ),
                );
                continue;
            }

            let detector = self.clone();
            let semaphore = semaphore.clone();
            let results = results.clone();
            let version = version.clone();
            let ffmpeg = info.path.clone();
            probes.spawn(async move {
                // 并发度受信号量限制：clamp(cpus / 2, 2, 4)
                let _permit = match semaphore.acquire_owned().await {
                    Ok(permit) => permit,
                    Err(_) => return,
                };
                let outcome = detector.probe_encoder(profile, &ffmpeg).await;
                let entry = create_entry(
                    profile,
                    outcome.available,
                    if outcome.available { String::new() } else { outcome.reason },
                    outcome.verified_by_trial,
                    false,
                    &version,
                    outcome.trial_command,
                );
                push_result(&results, entry);
            });
        }

        // 试跑全部收口（没有 CancellationToken，因此取消时不存在「部分结果」场景）
        while probes.join_next().await.is_some() {}

        let mut ordered = match results.lock() {
            Ok(guard) => guard.clone(),
            Err(poisoned) => poisoned.into_inner().clone(),
        };
        // 硬件(0)在前、软件(1)在后，再按 族 → 编码格式 → 名称 序数排序（规格 §5.2 第 2n 步）
        ordered.sort_by(|a, b| {
            let a_kind = if a.is_hardware { 0 } else { 1 };
            let b_kind = if b.is_hardware { 0 } else { 1 };
            a_kind
                .cmp(&b_kind)
                .then_with(|| a.family.cmp(&b.family))
                .then_with(|| a.target_codec.cmp(&b.target_codec))
                .then_with(|| a.encoder_name.cmp(&b.encoder_name))
        });

        let available_hardware: Vec<&DetectedEncoder> =
            ordered.iter().filter(|e| e.available && e.is_hardware).collect();

        let summary = if available_hardware.is_empty() {
            "未检测到可用的硬件编码器，将使用软件编码。".to_string()
        } else {
            let mut seen: HashSet<&str> = HashSet::new();
            let names: Vec<&str> = available_hardware
                .iter()
                .map(|e| e.display_name.as_str())
                .filter(|name| seen.insert(name))
                .take(4)
                .collect();
            format!("检测到 {} 个可用硬件编码器（{}）", available_hardware.len(), names.join("、"))
        };

        let preferences = self.preferences();
        let report = HardwareReport {
            probed_at: Utc::now(),
            platform,
            gpu_summary: build_gpu_summary(&info, &available_hardware),
            recommended: build_recommendations(&ordered, &preferences),
            summary,
            ffmpeg_version: version,
            ffmpeg_path: info.path.clone(),
            hwaccels: info.hwaccels.clone(),
            supported_filters: info.filters.clone(),
            encoders: ordered,
        };

        self.save_to_database(&report).await;
        report
    }

    /// 数据库缓存复用（规格 §5.6 C `TryLoadFromDatabase`）。
    async fn try_load_from_database(
        &self,
        version: &str,
        ffmpeg_path: &str,
    ) -> Option<HardwareReport> {
        let store = self.inner.store.as_ref()?;
        let rows = store.load_cached_encoders().await.ok()?;
        if rows.is_empty() {
            return None;
        }
        // 版本变化即失效（Ordinal 精确比较）
        if rows.iter().any(|row| row.ffmpeg_version != version) {
            return None;
        }
        // 上次完全失败就不复用
        if rows.iter().all(|row| !row.is_available) {
            return None;
        }
        let newest = rows.iter().map(|row| row.last_checked_at).max()?;
        if (Utc::now() - newest).num_hours() > DATABASE_CACHE_TTL_HOURS {
            return None;
        }

        let encoders: Vec<DetectedEncoder> = rows.iter().map(detected_from_row).collect();
        let preferences = self.preferences();
        Some(HardwareReport {
            // 复用时使用数据库里的最新检测时间
            probed_at: newest,
            platform: crate::paths::platform_display_name().to_string(),
            gpu_summary: format!("使用上次检测结果（{}）", newest.format("%Y-%m-%d %H:%M:%S UTC")),
            recommended: build_recommendations(&encoders, &preferences),
            summary: "使用上次检测结果".to_string(),
            ffmpeg_version: version.to_string(),
            ffmpeg_path: ffmpeg_path.to_string(),
            // PORT NOTE：数据库行里没有这两项，参考实现也不保存（规格 §5.6 C / §8.7）。
            // 后果：复用数据库缓存后 `has_filter()` 恒为 true（清单为空 = 放行）。
            hwaccels: Vec::new(),
            supported_filters: Vec::new(),
            encoders,
        })
    }

    async fn save_to_database(&self, report: &HardwareReport) {
        let Some(store) = self.inner.store.as_ref() else {
            return;
        };
        let rows: Vec<HardwareEncoder> = report.encoders.iter().map(row_from_detected).collect();
        // 写缓存失败不影响检测结果（规格 §5.6 C）
        if let Err(err) = store.save_cached_encoders(&rows).await {
            tracing::warn!(error = %err, "写入编码器检测缓存失败");
        }
    }

    /// 试跑（或复用进程级缓存）一个硬件编码器（规格 §5.3）。
    ///
    /// 第 1 档带质量参数与预设；失败后降级到第 2 档（仅指定编码器）；两档都失败才判不可用。
    ///
    /// PORT NOTE：参考实现的 `_probeCache` 只存一个 `bool`，第 2 档成功也写 `true`，
    /// 于是同一进程内后续 `detect(force_refresh: true)` 会把该编码器报成
    /// `SupportsQualityOptions = true`，**丢掉「不接受质量参数」这一降级信息**（规格 §5.3 的
    /// 「缓存放大」）。这里改用三态缓存（`Full` / `Minimal` / `Failed`）修正该缺陷：
    /// 首轮与后续轮次的 `verified_by_trial` 结论保持一致。
    async fn probe_encoder(&self, profile: &'static EncoderProfile, ffmpeg: &str) -> ProbeOutcome {
        if let Some(cached) = self.probe_cache_get(&profile.name) {
            return match cached {
                ProbeCached::Full => ProbeOutcome {
                    available: true,
                    verified_by_trial: true,
                    reason: String::new(),
                    trial_command: None,
                },
                ProbeCached::Minimal => ProbeOutcome {
                    available: true,
                    verified_by_trial: false,
                    reason: String::new(),
                    trial_command: None,
                },
                ProbeCached::Failed => ProbeOutcome {
                    available: false,
                    verified_by_trial: false,
                    reason: "试跑失败（缓存结果）".to_string(),
                    trial_command: None,
                },
            };
        }

        // 第 1 档：带质量参数与预设（模板真正会用的参数）
        let full_output = probe_output_path(&profile.name, "full");
        let full_args = build_probe_arguments(profile, true, &full_output);
        let full_command = util::render_command_line(ffmpeg, &full_args);
        let full = execute_probe(ffmpeg, &full_args).await;
        if full.success {
            self.probe_cache_set(&profile.name, ProbeCached::Full);
            return ProbeOutcome {
                available: true,
                verified_by_trial: true,
                reason: String::new(),
                trial_command: Some(full_command),
            };
        }

        // 第 2 档：仅指定编码器（兼容参数名变化的 FFmpeg 构建）
        let minimal_output = probe_output_path(&profile.name, "min");
        let minimal_args = build_probe_arguments(profile, false, &minimal_output);
        let minimal_command = util::render_command_line(ffmpeg, &minimal_args);
        let minimal = execute_probe(ffmpeg, &minimal_args).await;
        if minimal.success {
            self.probe_cache_set(&profile.name, ProbeCached::Minimal);
            return ProbeOutcome {
                available: true,
                verified_by_trial: false,
                reason: String::new(),
                trial_command: Some(minimal_command),
            };
        }

        self.probe_cache_set(&profile.name, ProbeCached::Failed);
        // 只汇总第 1 档的诊断
        ProbeOutcome {
            available: false,
            verified_by_trial: false,
            reason: summarize_failure(&full.diagnostic),
            trial_command: Some(full_command),
        }
    }

    fn probe_cache_get(&self, encoder_name: &str) -> Option<ProbeCached> {
        match self.inner.probe_cache.lock() {
            Ok(guard) => guard.get(encoder_name).copied(),
            Err(poisoned) => poisoned.into_inner().get(encoder_name).copied(),
        }
    }

    fn probe_cache_set(&self, encoder_name: &str, value: ProbeCached) {
        if let Ok(mut guard) = self.inner.probe_cache.lock() {
            guard.insert(encoder_name.to_string(), value);
        }
    }
}

// ────────────────────────────────────────────────────────────────────────────
// 组装与推荐
// ────────────────────────────────────────────────────────────────────────────

fn push_result(results: &Arc<Mutex<Vec<DetectedEncoder>>>, entry: DetectedEncoder) {
    if let Ok(mut guard) = results.lock() {
        guard.push(entry);
    }
}

/// 试跑并发度：`clamp(cpus / 2, 2, 4)`（规格 §5.2、§7）。
fn probe_concurrency() -> usize {
    (util::cpu_count() / 2).clamp(2, 4)
}

/// 构造一条检测结果（规格 §5.2 `CreateEntry` 的字段映射）。
#[allow(clippy::too_many_arguments)]
fn create_entry(
    profile: &'static EncoderProfile,
    available: bool,
    reason: String,
    verified_by_trial: bool,
    missing_from_build: bool,
    version: &str,
    trial_command: Option<String>,
) -> DetectedEncoder {
    DetectedEncoder {
        encoder_id: profile.name.clone(),
        encoder_name: profile.name.clone(),
        display_name: profile.display_name.clone(),
        encoder_type: profile.encoder_type,
        family: profile.family.clone(),
        target_codec: profile.codec.clone(),
        available,
        // 可用时原因置空（对应 C# `UnavailableReason = available ? null : reason`）
        reason: if available { String::new() } else { truncate_reason(&reason) },
        verified_by_trial,
        trial_command,
        missing_from_build,
        is_hardware: profile.is_hardware(),
        last_checked_at: Utc::now(),
        ffmpeg_version: version.to_string(),
    }
}

fn truncate_reason(reason: &str) -> String {
    let trimmed = reason.trim();
    if trimmed.chars().count() <= REASON_MAX_CHARS {
        return trimmed.to_string();
    }
    let head: String = trimmed.chars().take(REASON_MAX_CHARS).collect();
    format!("{head}…")
}

/// 未找到/不可用 FFmpeg 时的清单：只含当前平台的**软件**编码器（规格 §5.2 `BuildUnavailableCatalog`）。
fn build_unavailable_catalog(reason: &str, version: &str) -> Vec<DetectedEncoder> {
    EncoderCatalog::all()
        .iter()
        .filter(|profile| profile.is_platform_supported() && !profile.is_hardware())
        .map(|profile| create_entry(profile, false, reason.to_string(), true, true, version, None))
        .collect()
}

/// 尽力而为的硬件摘要：只依据 ffmpeg 能力清单 + 试跑结论，不额外跑系统命令。
fn build_gpu_summary(info: &crate::ffmpeg::service::FFmpegInfo, available_hardware: &[&DetectedEncoder]) -> String {
    let platform = crate::paths::platform_display_name();
    let arch = std::env::consts::ARCH;
    let cores = util::cpu_count();
    let hwaccels = if info.hwaccels.is_empty() {
        "无".to_string()
    } else {
        info.hwaccels.join("、")
    };

    if available_hardware.is_empty() {
        format!(
            "{platform}（{arch}，{cores} 逻辑核心）：FFmpeg 报告的硬件加速方法为 {hwaccels}，但没有硬件编码器通过试跑，实际将使用软件编码。"
        )
    } else {
        let mut seen: HashSet<&str> = HashSet::new();
        let names: Vec<&str> = available_hardware
            .iter()
            .map(|e| e.display_name.as_str())
            .filter(|name| seen.insert(name))
            .take(6)
            .collect();
        format!(
            "{platform}（{arch}，{cores} 逻辑核心）：已确认可用的硬件编码器 {}；FFmpeg 报告的硬件加速方法为 {hwaccels}。",
            names.join("、")
        )
    }
}

/// 为每个「有可用编码器」的编码格式生成推荐（规格 §5.5 第 3/4 步）。
fn build_recommendations(
    encoders: &[DetectedEncoder],
    preferences: &HardwarePreferences,
) -> BTreeMap<String, String> {
    let mut codecs: Vec<String> =
        encoders.iter().filter(|e| e.available).map(|e| e.target_codec.clone()).collect();
    codecs.sort();
    codecs.dedup();

    let mut recommended = BTreeMap::new();
    for codec in codecs {
        if let Some(name) = recommend_for_codec(encoders, &codec, preferences) {
            recommended.insert(codec, name);
        }
    }
    recommended
}

/// 自动挑选（不包含「必需编码器」与「用户首选」两步）。
fn recommend_for_codec(
    encoders: &[DetectedEncoder],
    codec: &str,
    preferences: &HardwarePreferences,
) -> Option<String> {
    let is_available = |name: &str| {
        encoders.iter().any(|e| e.available && e.encoder_name.eq_ignore_ascii_case(name))
    };
    let candidates = EncoderCatalog::for_codec(codec);

    if preferences.prefer_hardware_acceleration {
        for candidate in candidates.iter().filter(|profile| profile.is_hardware()) {
            if is_available(&candidate.name) {
                return Some(candidate.name.clone());
            }
        }
    }
    for candidate in candidates.iter().filter(|profile| !profile.is_hardware()) {
        if is_available(&candidate.name) {
            return Some(candidate.name.clone());
        }
    }
    // 兜底：**不检查** libx264 是否真的在构建清单里（规格 §5.5 第 4 步）
    Some(fallback_encoder(codec, &is_available))
}

/// 兜底编码器（规格 §5.5 第 4 步）。
fn fallback_encoder(codec: &str, is_available: &dyn Fn(&str) -> bool) -> String {
    if codec == "hevc" {
        if is_available("libx265") { "libx265".to_string() } else { "libx264".to_string() }
    } else {
        "libx264".to_string()
    }
}

fn row_from_detected(entry: &DetectedEncoder) -> HardwareEncoder {
    HardwareEncoder {
        id: crate::models::new_id(),
        name: entry.encoder_name.clone(),
        display_name: entry.display_name.clone(),
        encoder_type: entry.encoder_type,
        family: entry.family.clone(),
        codec: entry.target_codec.clone(),
        is_available: entry.available,
        supports_quality_options: entry.verified_by_trial,
        last_checked_at: entry.last_checked_at,
        unavailable_reason: if entry.available || entry.reason.trim().is_empty() {
            None
        } else {
            Some(entry.reason.clone())
        },
        missing_from_build: entry.missing_from_build,
        ffmpeg_version: entry.ffmpeg_version.clone(),
    }
}

fn detected_from_row(row: &HardwareEncoder) -> DetectedEncoder {
    DetectedEncoder {
        encoder_id: row.name.clone(),
        encoder_name: row.name.clone(),
        display_name: row.display_name.clone(),
        encoder_type: row.encoder_type,
        family: row.family.clone(),
        target_codec: row.codec.clone(),
        available: row.is_available,
        reason: row.unavailable_reason.clone().unwrap_or_default(),
        verified_by_trial: row.supports_quality_options,
        // 数据库里没有试跑命令行（参考实现也没有这一列）
        trial_command: None,
        missing_from_build: row.missing_from_build,
        is_hardware: row.encoder_type != EncoderType::Software,
        last_checked_at: row.last_checked_at,
        ffmpeg_version: row.ffmpeg_version.clone(),
    }
}

// ────────────────────────────────────────────────────────────────────────────
// 试跑命令行
// ────────────────────────────────────────────────────────────────────────────

/// 试跑参数（规格 §5.3 `BuildProbeArguments`，逐 token 顺序）。
///
/// `[
///   -hide_banner, -nostdin, -loglevel, error,
///   (device_args…),
///   -f, lavfi, -i, testsrc2=duration=0.3:size=320x240:rate=25,
///   (-vf, upload_filter),
///   -c:v, <encoder>,
///   (quality_args…, preset_args…),
///   -frames:v, 3, -f, rawvideo, -y, <output>
/// ]`
pub fn build_probe_arguments(
    profile: &EncoderProfile,
    include_quality: bool,
    output_path: &str,
) -> Vec<String> {
    let mut args: Vec<String> = vec![
        "-hide_banner".to_string(),
        "-nostdin".to_string(),
        "-loglevel".to_string(),
        "error".to_string(),
    ];
    args.extend(profile.device_args_tokens());
    args.extend(["-f", "lavfi", "-i", TRIAL_INPUT].map(String::from));
    if !profile.upload_filter.trim().is_empty() {
        args.push("-vf".to_string());
        args.push(profile.upload_filter.clone());
    }
    args.push("-c:v".to_string());
    args.push(profile.name.clone());
    if include_quality {
        let quality = profile.render_quality_args(None);
        if !quality.trim().is_empty() {
            args.extend(util::tokenize(&quality));
        }
        args.extend(profile.preset_args_tokens());
    }
    args.extend(["-frames:v", "3", "-f", "rawvideo", "-y"].map(String::from));
    args.push(output_path.to_string());
    args
}

/// 试跑临时输出路径（规格 §5.3）：`$TMPDIR/ffmpegwebui-probe-<编码器名>-<stage>-<guid32>.raw`。
pub fn probe_output_path(encoder_name: &str, stage: &str) -> String {
    let safe: String = encoder_name
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-'))
        .collect();
    std::env::temp_dir()
        .join(format!("ffmpegwebui-probe-{safe}-{stage}-{}.raw", uuid::Uuid::new_v4().simple()))
        .to_string_lossy()
        .into_owned()
}

/// 执行一次试跑（规格 §5.3 `ExecuteProbeAsync`）。
///
/// 判定「可用」的两个条件同时成立：退出码 == 0 **且** 临时输出字节数 > 0。
async fn execute_probe(program: &str, args: &[String]) -> TrialResult {
    let output_path = args.last().cloned().unwrap_or_default();

    let mut cmd = util::base_command(program);
    cmd.args(args);
    let mut child = match cmd.spawn() {
        Ok(child) => child,
        Err(err) => {
            return TrialResult { success: false, diagnostic: format!("无法启动 FFmpeg：{err}") };
        }
    };
    let pid = child.id();
    // 关 stdin：即使某选项触发交互提问也不会挂起
    drop(child.stdin.take());

    let stdout_pipe = child.stdout.take();
    let stderr_pipe = child.stderr.take();
    let stdout_task = tokio::spawn(async move { read_pipe(stdout_pipe).await });
    let stderr_task = tokio::spawn(async move { read_pipe(stderr_pipe).await });

    let mut timed_out = false;
    let mut exit_code: Option<i32> = None;
    match tokio::time::timeout(std::time::Duration::from_secs(TRIAL_TIMEOUT_SECS), child.wait()).await
    {
        Ok(Ok(status)) => exit_code = status.code(),
        Ok(Err(_)) => {}
        Err(_elapsed) => {
            timed_out = true;
            util::kill_process_tree(pid, &mut child).await;
        }
    }

    let stdout = join_probe_reader(stdout_task).await;
    let stderr = join_probe_reader(stderr_task).await;

    // 先量大小，再**无条件**删除临时文件
    let output_size = match tokio::fs::metadata(&output_path).await {
        Ok(meta) => meta.len(),
        Err(_) => 0,
    };
    let _ = tokio::fs::remove_file(&output_path).await;

    if timed_out {
        return TrialResult {
            success: false,
            diagnostic: "试跑超时（15 秒内未完成），通常是驱动未安装或设备被占用。".to_string(),
        };
    }

    // 优先 stderr
    let diagnostic = if stderr.trim().is_empty() { stdout } else { stderr };

    if exit_code != Some(0) {
        return TrialResult { success: false, diagnostic };
    }
    if output_size == 0 {
        return TrialResult {
            success: false,
            diagnostic: format!("试跑没有产生任何编码输出，编码器可能无法真正工作。\n{diagnostic}"),
        };
    }
    TrialResult { success: true, diagnostic }
}

async fn read_pipe<R: tokio::io::AsyncRead + Unpin>(pipe: Option<R>) -> String {
    use tokio::io::AsyncReadExt;
    let mut buffer = String::new();
    if let Some(mut pipe) = pipe {
        let _ = pipe.read_to_string(&mut buffer).await;
    }
    buffer
}

async fn join_probe_reader(task: tokio::task::JoinHandle<String>) -> String {
    match tokio::time::timeout(std::time::Duration::from_secs(TRIAL_READER_WAIT_SECS), task).await {
        Ok(Ok(text)) => text,
        _ => String::new(),
    }
}

/// 失败原因归纳（规格 §5.4 `SummarizeFailure`）：
/// 丢版本/configuration/lib 横幅行 → 优先取含致命特征串的前 2 行，否则最后 2 行 →
/// 单空格拼接 → 截断到 300 字符（超出加 `…`）。
pub fn summarize_failure(diagnostic: &str) -> String {
    if diagnostic.trim().is_empty() {
        return "试跑失败：FFmpeg 未给出更多信息".to_string();
    }

    let lines: Vec<&str> = diagnostic
        .split('\n')
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .filter(|line| !line.to_ascii_lowercase().starts_with("ffmpeg version"))
        .filter(|line| !line.to_ascii_lowercase().starts_with("configuration:"))
        .filter(|line| !line.to_ascii_lowercase().starts_with("lib"))
        .collect();

    let mut picked: Vec<&str> = lines
        .iter()
        .copied()
        .filter(|line| FATAL_MARKERS.iter().any(|marker| util::contains_ignore_case(line, marker)))
        .take(2)
        .collect();
    if picked.is_empty() {
        picked = lines.iter().rev().take(2).copied().collect();
        picked.reverse();
    }

    let text = picked.join(" ");
    if text.chars().count() > REASON_MAX_CHARS {
        let head: String = text.chars().take(REASON_MAX_CHARS).collect();
        return format!("{head}…");
    }
    if text.is_empty() {
        return "试跑失败".to_string();
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profile(name: &str) -> &'static EncoderProfile {
        EncoderCatalog::find(name).expect("画像存在")
    }

    #[test]
    fn probe_arguments_match_spec_for_nvenc_and_vaapi() {
        let args = build_probe_arguments(profile("h264_nvenc"), true, "/tmp/out.raw");
        assert_eq!(
            args,
            vec![
                "-hide_banner",
                "-nostdin",
                "-loglevel",
                "error",
                "-f",
                "lavfi",
                "-i",
                "testsrc2=duration=0.3:size=320x240:rate=25",
                "-c:v",
                "h264_nvenc",
                "-rc",
                "vbr",
                "-cq",
                "24",
                "-b:v",
                "0",
                "-preset",
                "p5",
                "-tune",
                "hq",
                "-frames:v",
                "3",
                "-f",
                "rawvideo",
                "-y",
                "/tmp/out.raw",
            ]
        );

        let vaapi = build_probe_arguments(profile("h264_vaapi"), false, "/tmp/out.raw");
        assert_eq!(
            vaapi,
            vec![
                "-hide_banner",
                "-nostdin",
                "-loglevel",
                "error",
                "-vaapi_device",
                "/dev/dri/renderD128",
                "-f",
                "lavfi",
                "-i",
                "testsrc2=duration=0.3:size=320x240:rate=25",
                "-vf",
                "format=nv12,hwupload",
                "-c:v",
                "h264_vaapi",
                "-frames:v",
                "3",
                "-f",
                "rawvideo",
                "-y",
                "/tmp/out.raw",
            ]
        );

        // 无质量参数的画像：第 1 档也只剩预设（ProRes）
        let prores = build_probe_arguments(profile("prores_videotoolbox"), true, "/tmp/out.raw");
        assert!(prores.windows(2).any(|w| w == ["-profile:v", "3"]));
        assert!(!prores.iter().any(|a| a == "-crf"));
    }

    #[test]
    fn probe_output_path_sanitizes_encoder_name() {
        let path = probe_output_path("h264_nvenc", "full");
        assert!(path.contains("ffmpegwebui-probe-h264_nvenc-full-"));
        assert!(path.ends_with(".raw"));
        let weird = probe_output_path("we/ird name", "min");
        assert!(weird.contains("ffmpegwebui-probe-weirdname-min-"));
    }

    #[test]
    fn summarize_failure_picks_fatal_marker_lines() {
        let diagnostic = "ffmpeg version 9.0.2 Copyright\n\
                          configuration: --enable-nvenc\n\
                          libavutil 60. 8.100\n\
                          [h264_nvenc @ 0x1] Cannot load libcuda.so.1\n\
                          Error initializing output stream 0:0\n";
        let reason = summarize_failure(diagnostic);
        assert!(reason.contains("Cannot load libcuda.so.1"));
        assert!(reason.contains("Error initializing output stream"));
        assert!(!reason.contains("configuration:"));

        assert_eq!(summarize_failure("   "), "试跑失败：FFmpeg 未给出更多信息");
        let long = summarize_failure(&"x".repeat(400));
        assert_eq!(long.chars().count(), REASON_MAX_CHARS + 1);
    }

    #[test]
    fn probe_concurrency_is_clamped() {
        let concurrency = probe_concurrency();
        assert!((2..=4).contains(&concurrency));
    }

    #[test]
    fn recommendations_follow_priority() {
        let software = EncoderCatalog::software_encoder_for("h264").expect("libx264").name.clone();
        // 平台过滤后该格式优先级最高的硬件候选（macOS 上是 videotoolbox，Win/Linux 上是 nvenc）
        let hardware = EncoderCatalog::for_codec("h264")
            .into_iter()
            .find(|profile| profile.is_hardware())
            .map(|profile| profile.name.clone());

        let mut encoders = vec![DetectedEncoder {
            encoder_name: software.clone(),
            target_codec: "h264".into(),
            available: true,
            is_hardware: false,
            ..Default::default()
        }];
        if let Some(name) = &hardware {
            encoders.push(DetectedEncoder {
                encoder_name: name.clone(),
                target_codec: "h264".into(),
                available: true,
                is_hardware: true,
                ..Default::default()
            });
        }

        // 硬件优先
        let preferences = HardwarePreferences::default();
        let expected = hardware.clone().unwrap_or_else(|| software.clone());
        assert_eq!(
            recommend_for_codec(&encoders, "h264", &preferences).as_deref(),
            Some(expected.as_str())
        );

        // 关掉硬件优先 → 选软件
        let soft = HardwarePreferences {
            prefer_hardware_acceleration: false,
            preferred_hardware_encoder: None,
        };
        assert_eq!(recommend_for_codec(&encoders, "h264", &soft).as_deref(), Some(software.as_str()));

        // 硬件不可用 → 软件
        if let Some(index) = hardware.as_ref().and_then(|name| encoders.iter().position(|e| &e.encoder_name == name)) {
            encoders[index].available = false;
            assert_eq!(
                recommend_for_codec(&encoders, "h264", &preferences).as_deref(),
                Some(software.as_str())
            );
        }

        // macOS 上不应该推荐 NVENC（画像表里根本没有这条平台画像）
        if cfg!(target_os = "macos") {
            assert_eq!(hardware.as_deref(), Some("h264_videotoolbox"));
            assert!(EncoderCatalog::find("h264_nvenc").is_some());
            assert!(EncoderCatalog::for_codec("h264").iter().all(|p| p.name != "h264_nvenc"));
        }

        // 兜底分支（hevc 没有可用候选时退回 libx264，且不检查构建清单）
        assert_eq!(fallback_encoder("hevc", &|_| false), "libx264");
        assert_eq!(fallback_encoder("hevc", &|name| name == "libx265"), "libx265");
        assert_eq!(fallback_encoder("h264", &|_| false), "libx264");
    }

    #[test]
    fn report_helpers_use_empty_filter_list_as_allow_all() {
        let report = HardwareReport::default();
        assert!(report.has_filter("scale"));
        let report = HardwareReport {
            supported_filters: vec!["scale".into()],
            ..Default::default()
        };
        assert!(report.has_filter("SCALE"));
        assert!(!report.has_filter("overlay"));
    }

    #[test]
    fn row_round_trip_keeps_quality_flag() {
        let entry = DetectedEncoder {
            encoder_name: "hevc_videotoolbox".into(),
            display_name: "Apple VideoToolbox H.265/HEVC".into(),
            encoder_type: EncoderType::VideoToolbox,
            family: "videotoolbox".into(),
            target_codec: "hevc".into(),
            available: true,
            reason: String::new(),
            verified_by_trial: false,
            missing_from_build: false,
            is_hardware: true,
            ffmpeg_version: "9.0.2".into(),
            ..Default::default()
        };
        let row = row_from_detected(&entry);
        assert_eq!(row.name, "hevc_videotoolbox");
        assert!(!row.supports_quality_options);
        assert!(row.unavailable_reason.is_none());
        let back = detected_from_row(&row);
        assert_eq!(back.encoder_name, entry.encoder_name);
        assert!(!back.verified_by_trial);
        assert!(back.is_hardware);
    }
}
