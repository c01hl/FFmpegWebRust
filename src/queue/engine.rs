//! 转换队列本体：常驻 worker、18 步单任务生命周期、取消、停机、事件广播。
//!
//! 规格：`docs/reference/backend-data.md` §3（§3.1–§3.16）。有意差异见 `docs/architecture.md` §3
//! 的第 3/4/5/6 条，代码里全部带 `PORT NOTE` 标注。
//!
//! # 与参考实现的架构差异（并发模型）
//!
//! 参考实现是「`EnsureWorkers` 按需创建、**只增不减**」的 worker 池：把 `MaxConcurrentTasks`
//! 调小之后，多余的 worker 仍然会去抢同一个通道，实际并发不会下降，直到重启。
//!
//! 这里改成**单调度器 + 每任务一个 tokio 任务**：调度器每次派发前重新读取
//! `settings.max_concurrent_tasks`（`1..16`），并用 `running.len()` 作为并发闸门。
//! 因此**调小并发数立刻对后续任务生效**（已在跑的任务不会被中断，这是有意的：
//! 中断用户正在跑的转换比多跑一会儿更糟）。

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use chrono::Utc;
use tokio::sync::{Notify, broadcast, mpsc};
use tokio_util::sync::CancellationToken;

use crate::ffmpeg::encoder_catalog::EncoderProfile;
use crate::ffmpeg::hardware::HardwareDetector;
use crate::ffmpeg::service::{
    FFmpegService, MediaInfo, RunEvent, RunOutcome, RunRequest, SKIPPED_PREFIX,
};
use crate::files::output_path::{
    OutputPathResolution, is_output_path_writable, resolve_output_path,
};
use crate::models::enums::{FileExistsAction, TaskStatus};
use crate::models::{AppSettings, ConversionTask};
use crate::paths;
use crate::services::SettingsService;
use crate::store::Store;
use crate::templates::TemplateService;

use super::plan::{
    build_attempt, first_line, progress_raw_output, resolve_encoder, should_fallback_to_software,
    truncate_log,
};

// ────────────────────────────────────────────────────────────────────────────
// 常量（规格 §3 的节流/上限，逐条标注出处）
// ────────────────────────────────────────────────────────────────────────────

/// 事件广播通道容量。满了会丢事件（`broadcast` 语义）；日志已有 100–200ms 合并，
/// 正常不会被填满，丢的也只会是进度事件。
const QUEUE_EVENT_CAPACITY: usize = 2048;

/// 日志落库节流（规格 §3.8：700 ms）
const LOG_PERSIST_THROTTLE: Duration = Duration::from_millis(700);

/// 运行期进度落库间隔（本项目有意差异 #4：参考实现只在结束时写）
const PROGRESS_PERSIST_INTERVAL: Duration = Duration::from_secs(1);

/// 进度事件的最小间隔（规格 §3.7 要求服务端自造「UI 侧上限」；ffmpeg 本身约 2 次/秒）
const PROGRESS_EVENT_MIN_INTERVAL: Duration = Duration::from_millis(100);

/// 日志事件合并窗口（规格 §3.14 移植建议：100–200 ms）
const LOG_EVENT_WINDOW: Duration = Duration::from_millis(120);

/// 单个 `Log` 事件最多携带的行数（规格 §3.14 移植建议：每帧最多 N 行）
const LOG_EVENT_MAX_LINES: usize = 50;

/// 待推送日志行的内存上限：前端长时间不消费时丢最旧的行并计数（`dropped`）
const LOG_EVENT_BUFFER_MAX: usize = 2000;

/// 部分输出清理阈值（规格 §3.10：只删「明显是残片」的输出）
const PARTIAL_OUTPUT_DELETE_LIMIT: u64 = 64 * 1024 * 1024;

/// `shutdown()` 等待在跑任务自然结束的上限（冻结 API 的文档注释）
const SHUTDOWN_WAIT: Duration = Duration::from_secs(10);

/// `shutdown()` 超时后发优雅取消、再等多久交出控制权
const SHUTDOWN_CANCEL_WAIT: Duration = Duration::from_secs(6);

/// 调度器的兜底轮询间隔（`Notify` 只唤醒「已注册」的等待者，用它对抗丢失唤醒）
const DISPATCH_IDLE_POLL: Duration = Duration::from_millis(250);

// ────────────────────────────────────────────────────────────────────────────
// 事件（UI 通过 WebSocket 订阅的东西，规格 §3.14 / api-contract.md §2）
// ────────────────────────────────────────────────────────────────────────────

/// 队列事件。字段名与 `docs/api-contract.md` §2 的 WS 负载一一对应（外层信封由 API 层包）。
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase", rename_all_fields = "camelCase", tag = "type")]
pub enum QueueEvent {
    /// 每个 ffmpeg 进度块（服务端已做最小间隔节流）
    Progress {
        task_id: String,
        progress: f64,
        speed: Option<f64>,
        eta: Option<f64>,
        current_time: f64,
        total_duration: f64,
    },
    /// 状态变更（入队后转 Running、跳过、终态、失败、取消）
    Status {
        task_id: String,
        old_status: TaskStatus,
        new_status: TaskStatus,
        error_message: Option<String>,
        task: ConversionTask,
    },
    /// 日志（服务端已按 100–200ms 窗口合并，每帧最多 50 行）
    Log { task_id: String, lines: Vec<String>, dropped: usize, truncated: bool },
    /// 队列长度变化（参考实现没有这个事件，本项目有意新增，见需求）
    QueueChanged { queued: usize, active: usize },
    /// 批次汇总（每个子任务进入终态后推送一次）
    BatchStatus {
        batch_id: String,
        status: TaskStatus,
        total_files: i32,
        completed_files: i32,
        failed_files: i32,
    },
}

// ────────────────────────────────────────────────────────────────────────────
// 公开 API
// ────────────────────────────────────────────────────────────────────────────

/// 后台转换队列（`Clone` 廉价：内部是 `Arc`，可直接塞进 axum 的 `AppState`）。
#[derive(Clone)]
pub struct ConversionQueue {
    inner: Arc<QueueInner>,
}

impl ConversionQueue {
    /// 创建队列（**不启动** worker；请再调用一次 [`ConversionQueue::start`]）
    pub fn new(
        store: Arc<Store>,
        settings: Arc<SettingsService>,
        templates: Arc<TemplateService>,
        ffmpeg: Arc<FFmpegService>,
        hardware: Arc<HardwareDetector>,
    ) -> Self {
        let (events, _receiver) = broadcast::channel(QUEUE_EVENT_CAPACITY);
        Self {
            inner: Arc::new(QueueInner {
                store,
                settings,
                templates,
                ffmpeg,
                hardware,
                events,
                state: Mutex::new(QueueState::default()),
                wake: Notify::new(),
                started: AtomicBool::new(false),
                shutting_down: AtomicBool::new(false),
            }),
        }
    }

    /// 启动 worker（**幂等**；由 `main` 调用一次）。
    ///
    /// 必须在 tokio 运行时上下文里调用；不在运行时里调用不会 panic，只会记一条 error 日志。
    pub fn start(&self) {
        if self.inner.started.swap(true, Ordering::SeqCst) {
            return;
        }
        let Ok(handle) = tokio::runtime::Handle::try_current() else {
            self.inner.started.store(false, Ordering::SeqCst);
            tracing::error!("ConversionQueue::start 必须在 tokio 运行时内调用，worker 未启动");
            return;
        };

        let dispatcher = self.inner.clone();
        handle.spawn(async move { dispatch_loop(dispatcher).await });

        // 设置变更（并发数/日志行数/覆盖策略…）后唤醒调度器，让新上限立刻对后续任务生效
        let watcher = self.inner.clone();
        handle.spawn(async move {
            let mut receiver = watcher.settings.subscribe();
            // 先读一次：既落首次运行的默认设置，也让 `cached()` 立刻可用
            let _ = watcher.settings.get().await;
            while receiver.recv().await.is_ok() {
                watcher.wake.notify_one();
            }
        });
    }

    /// 订阅队列事件（每个订阅者拿到自己的 `Receiver`）
    pub fn subscribe(&self) -> broadcast::Receiver<QueueEvent> {
        self.inner.events.subscribe()
    }

    /// 排队中的任务数（已入队、尚未开始）
    pub fn queued_count(&self) -> usize {
        self.inner.state().pending.len()
    }

    /// 正在执行的任务数
    pub fn active_count(&self) -> usize {
        self.inner.state().running.len()
    }

    /// 入队。
    ///
    /// **同一 `task_id` 重复入队是幂等的**（本项目有意差异 #5）：只要该任务还在排队或正在执行，
    /// 重复调用直接返回 `Ok(())`，不会让它被执行两次。
    /// 任务已经进入终态并被清理后，再次入队会重新执行（供「重试 / 再次开始」使用）。
    pub async fn enqueue(&self, task_id: &str) -> anyhow::Result<()> {
        let task_id = task_id.trim();
        if task_id.is_empty() {
            anyhow::bail!("任务 id 为空，无法入队");
        }
        if self.inner.shutting_down.load(Ordering::SeqCst) {
            // 参考实现里通道关闭 → TryWrite 失败 → 记 warning 返回 false
            anyhow::bail!("队列正在关闭，拒绝新的任务入队");
        }

        {
            let mut state = self.inner.state();
            if state.in_flight.contains(task_id) {
                // 幂等：排队中 / 正在执行 → 什么都不做（修复参考实现「重复执行」的缺陷）
                return Ok(());
            }
            state.in_flight.insert(task_id.to_string());
            state.pending.push_back(task_id.to_string());
        }

        self.inner.emit_queue_changed();
        self.inner.wake.notify_one();
        Ok(())
    }

    /// 取消任务。
    ///
    /// * `Pending`：直接标记 `Cancelled`（不碰进程），并从待执行队列里摘掉；
    /// * `Running`：`graceful = true` → 向 ffmpeg 的 stdin 写 `q`，最多等 5 秒再杀进程树；
    ///   `graceful = false` → 立刻强杀进程树；
    /// * 其它状态：返回 `false`（不做事）。
    pub async fn cancel(&self, task_id: &str, graceful: bool) -> anyhow::Result<bool> {
        let task_id = task_id.trim().to_string();
        if task_id.is_empty() {
            return Ok(false);
        }

        // 1) 正在执行？标记取消请求 + 按需写 q / 强杀
        let running = { self.inner.state().running.get(&task_id).cloned() };
        if let Some(entry) = running {
            {
                let mut state = self.inner.state();
                state.cancel_requested.insert(task_id.clone());
            }
            // 该任务的日志缓冲（若已开始执行）—— 优雅取消要留一行说明
            let runtime = { self.inner.state().runtimes.get(&task_id).cloned() };
            if graceful {
                if let Some(runtime) = runtime.as_ref() {
                    runtime
                        .append_log(
                            &self.inner,
                            "已发送优雅退出指令（q），FFmpeg 会写完文件尾再退出。".to_string(),
                            false,
                        )
                        .await;
                }
                // `run()` 内部：写 q → 最多等 5 秒 → 杀进程树（规格 §3.9）
                entry.cancel.cancel();
            } else {
                // 强制取消 = 立刻杀进程树（**不**先写 q：规格 §3.9 明确要求不要复制参考实现
                // 「先 q 再 kill」的偶然顺序）
                let stopped = self.inner.ffmpeg.stop_task(&task_id, false).await;
                if !stopped {
                    // 竞态兜底：`run()` 刚 spawn 进程、还没把它登记进服务时，`stop_task` 找不到人。
                    // 此时用令牌让 `run()` 一进入等待循环就收尾（写 q → 最多 5 秒 → 杀进程树）。
                    entry.cancel.cancel();
                }
            }
            return Ok(true);
        }

        // 2) 还在排队？直接标记 Cancelled 并摘出队列
        let was_pending = {
            let mut state = self.inner.state();
            match state.pending.iter().position(|id| id == &task_id) {
                Some(index) => {
                    state.pending.remove(index);
                    state.in_flight.remove(&task_id);
                    true
                }
                None => false,
            }
        };
        if was_pending {
            let Some(task) = self.inner.store.get_task(&task_id).await? else {
                self.inner.emit_queue_changed();
                return Ok(true);
            };
            mark_cancelled(&self.inner, &task).await?;
            self.inner.emit_queue_changed();
            return Ok(true);
        }

        // 3) 库里还有状态可处理（例如「库里 Pending、但从未入队」或重启后残留）：
        //    §3.9 的分支是按**库里的状态**判断的，因此这里必须补齐 Pending 分支，
        //    否则批量「取消全部」会对从未入队的子任务静默失效。
        let Some(task) = self.inner.store.get_task(&task_id).await? else {
            return Ok(false);
        };
        match task.status {
            TaskStatus::Pending => {
                mark_cancelled(&self.inner, &task).await?;
                self.inner.emit_queue_changed();
                Ok(true)
            }
            TaskStatus::Running => {
                // 内存里没有登记却在跑：只可能来自别的进程，尽力停进程但不伪造状态
                let stopped = self.inner.ffmpeg.stop_task(&task_id, graceful).await;
                if !stopped {
                    tracing::warn!("任务 {task_id} 状态为 Running，但没有找到对应的 FFmpeg 进程");
                }
                Ok(stopped)
            }
            _ => Ok(false),
        }
    }

    /// 优雅停机：停止取新任务 → 等在跑的任务自然结束（最多 10 秒）→ 仍未结束的发出优雅取消
    /// （写 `q`，5 秒兜底强杀）再等最多 6 秒。队列空闲时立即返回。
    pub async fn shutdown(&self) {
        self.inner.shutting_down.store(true, Ordering::SeqCst);
        self.inner.wake.notify_one();

        let deadline = Instant::now() + SHUTDOWN_WAIT;
        while self.inner.active_count() > 0 && Instant::now() < deadline {
            self.inner.wait_for_change(Duration::from_millis(100)).await;
        }

        if self.inner.active_count() > 0 {
            let tokens: Vec<CancellationToken> = self
                .inner
                .state()
                .running
                .values()
                .map(|entry| entry.cancel.clone())
                .collect();
            if !tokens.is_empty() {
                tracing::info!("停机：{} 个任务仍在执行，发出优雅取消", tokens.len());
            }
            for token in tokens {
                token.cancel();
            }
            let cancel_deadline = Instant::now() + SHUTDOWN_CANCEL_WAIT;
            while self.inner.active_count() > 0 && Instant::now() < cancel_deadline {
                self.inner.wait_for_change(Duration::from_millis(100)).await;
            }
        }
        self.inner.emit_queue_changed();
    }
}

// ────────────────────────────────────────────────────────────────────────────
// 内部状态
// ────────────────────────────────────────────────────────────────────────────

struct QueueInner {
    store: Arc<Store>,
    settings: Arc<SettingsService>,
    templates: Arc<TemplateService>,
    ffmpeg: Arc<FFmpegService>,
    hardware: Arc<HardwareDetector>,
    events: broadcast::Sender<QueueEvent>,
    state: Mutex<QueueState>,
    wake: Notify,
    started: AtomicBool,
    shutting_down: AtomicBool,
}

/// 一个正在执行的任务
#[derive(Clone)]
struct RunningEntry {
    /// 优雅停止令牌：`run()` 收到后会写 `q`，5 秒不退则杀进程树
    cancel: CancellationToken,
}

#[derive(Default)]
struct QueueState {
    /// 待执行（等价参考实现的 `Channel` 里的 id，`Reader.Count` = `QueuedCount`）
    pending: VecDeque<String>,
    /// 排队中 + 正在执行的任务 id（**入队去重**用，有意差异 #5）
    in_flight: HashSet<String>,
    /// 正在执行的任务（`Count` = `ActiveCount`）
    running: HashMap<String, RunningEntry>,
    /// 已请求取消的任务 id（无论优雅还是强制；终态一律判为 `Cancelled`）
    cancel_requested: HashSet<String>,
    /// 运行期日志/进度上下文（取消路径要往里写日志；任务结束时清理）
    runtimes: HashMap<String, Arc<TaskRuntime>>,
}

impl QueueInner {
    fn state(&self) -> MutexGuard<'_, QueueState> {
        self.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// 正在执行的任务数
    fn active_count(&self) -> usize {
        self.state().running.len()
    }

    fn emit(&self, event: QueueEvent) {
        // 没有订阅者 / 订阅者太慢都会 Err，属正常情况
        let _ = self.events.send(event);
    }

    fn emit_queue_changed(&self) {
        let (queued, active) = {
            let state = self.state();
            (state.pending.len(), state.running.len())
        };
        self.emit(QueueEvent::QueueChanged { queued, active });
    }

    fn is_cancel_requested(&self, task_id: &str) -> bool {
        self.state().cancel_requested.contains(task_id)
    }

    /// 等待一次状态变化，或最多等 `timeout`（对抗 `Notify` 丢失唤醒）
    async fn wait_for_change(&self, timeout: Duration) {
        tokio::select! {
            _ = self.wake.notified() => {}
            _ = tokio::time::sleep(timeout) => {}
        }
    }

    /// 当前并发上限（规格 §3.3：`clamp(settings.MaxConcurrentTasks, 1, 16)`）
    async fn concurrency_limit(&self) -> usize {
        self.settings.get().await.max_concurrent_tasks.clamp(1, 16) as usize
    }
}

// ────────────────────────────────────────────────────────────────────────────
// 调度器
// ────────────────────────────────────────────────────────────────────────────

/// 派发循环：每次派发前重新读取并发上限（这是「调小并发立刻生效」的关键）。
async fn dispatch_loop(inner: Arc<QueueInner>) {
    loop {
        if inner.shutting_down.load(Ordering::SeqCst) {
            tracing::debug!("转换队列调度器退出（正在停机）");
            return;
        }

        let limit = inner.concurrency_limit().await;
        let next = {
            let mut state = inner.state();
            if state.running.len() >= limit {
                None
            } else {
                state.pending.pop_front()
            }
        };

        let Some(task_id) = next else {
            // 无活可干：等入队 / 等任务结束 / 等设置变更，最多睡 DISPATCH_IDLE_POLL 兜底
            inner.wait_for_change(DISPATCH_IDLE_POLL).await;
            continue;
        };

        let token = CancellationToken::new();
        {
            let mut state = inner.state();
            if !state.in_flight.contains(&task_id) || state.cancel_requested.contains(&task_id) {
                // 与 cancel() 竞争：已经被摘掉/取消，直接丢弃这个 id
                continue;
            }
            state.running.insert(task_id.clone(), RunningEntry { cancel: token.clone() });
        }
        inner.emit_queue_changed();

        let task_inner = inner.clone();
        tokio::spawn(async move { run_task(task_inner, task_id, token).await });
    }
}

/// 取一个任务直到终态，**并保证内存状态一定被清理**（`finally` 的等价物）
async fn run_task(inner: Arc<QueueInner>, task_id: String, cancel: CancellationToken) {
    if let Err(error) = execute_task(&inner, &task_id, &cancel).await {
        tracing::error!("任务 {task_id} 执行异常：{error:#}");
        if let Err(mark_error) = try_mark_failed(&inner, &task_id, &format!("{error:#}")).await {
            tracing::error!("标记任务 {task_id} 失败时又出错了：{mark_error:#}");
        }
    }

    // 步骤 18（`finally`）：移除内存状态
    {
        let mut state = inner.state();
        state.running.remove(&task_id);
        state.in_flight.remove(&task_id);
        state.cancel_requested.remove(&task_id);
        state.runtimes.remove(&task_id);
    }
    inner.emit_queue_changed();
    inner.wake.notify_one();
}

/// 单任务的 18 步生命周期（规格 §3.4）。
async fn execute_task(
    inner: &Arc<QueueInner>,
    task_id: &str,
    cancel: &CancellationToken,
) -> anyhow::Result<()> {
    // 1) 从库读任务
    let Some(mut task) = inner.store.get_task(task_id).await? else {
        return Ok(());
    };
    let initial_status = task.status;

    // 2) 状态不是 Pending/Running → 不执行（已被取消/跳过的任务不启动 ffmpeg）
    if !matches!(initial_status, TaskStatus::Pending | TaskStatus::Running) {
        return Ok(());
    }
    if inner.is_cancel_requested(task_id) {
        return Ok(());
    }

    let settings = inner.settings.get().await;

    // 3) 模板
    let template = match inner.templates.get(&task.template_id).await? {
        Some(template) => template,
        None => {
            return fail_before_start(inner, &mut task, "模板不存在，可能已被删除。").await;
        }
    };

    // 4) ffprobe 探测（失败不致命）
    let media: Option<MediaInfo> = match inner.ffmpeg.probe_media(&task.input_path).await {
        Ok(media) => Some(media),
        Err(error) => {
            tracing::debug!("任务 {task_id} 媒体探测失败（继续执行）：{error}");
            None
        }
    };

    // 5) 输出冲突解析 +「输出 == 输入」安全（规格 §3.5）
    let resolution = resolve_conflicts(&task, &settings);
    if resolution.skip {
        task.status = TaskStatus::Skipped;
        task.completed_at = Some(Utc::now());
        task.progress = 0.0;
        task.processing_speed = None;
        task.estimated_time_remaining = None;
        task.output_path = resolution.output_path.clone();
        task.log_output = resolution.note.clone().unwrap_or_default();
        inner.store.update_task(&task).await?;
        inner.emit(QueueEvent::Status {
            task_id: task.id.clone(),
            old_status: initial_status,
            new_status: TaskStatus::Skipped,
            error_message: None,
            task: task.clone(),
        });
        update_batch_status(inner, task.batch_id.as_deref()).await;
        return Ok(());
    }

    // 6) 写回输出路径 / 总时长 / 输入大小
    task.output_path = resolution.output_path.clone();
    if let Some(media) = media.as_ref() {
        if media.duration_seconds > 0.0 {
            task.total_duration = media.duration_seconds;
        }
    }
    task.input_file_size = input_file_size(media.as_ref(), &task.input_path);

    // 7) 确保输出目录存在
    let output_dir = Path::new(&task.output_path)
        .parent()
        .map(|dir| dir.to_path_buf())
        .filter(|dir| !dir.as_os_str().is_empty());
    if let Some(dir) = output_dir.as_ref() {
        if !dir.exists() {
            if let Err(error) = std::fs::create_dir_all(dir) {
                return fail_before_start(
                    inner,
                    &mut task,
                    &format!("无法创建输出目录：{error}"),
                )
                .await;
            }
        }
    }

    // 8) 可写性探测（写一个探测文件再删掉，规格 §4.4）
    if !is_output_path_writable(&task.output_path) {
        let dir = output_dir
            .as_ref()
            .map(|dir| dir.to_string_lossy().to_string())
            .unwrap_or_default();
        return fail_before_start(inner, &mut task, &format!("输出目录不可写：{dir}")).await;
    }

    // 9) 编码器解析（严格模式缺硬件 → 失败，文案逐字来自规格）
    //    先把「当前设置」同步给硬件检测器的偏好：参考实现的 `GetRecommendedEncoderAsync` 每次都直接读
    //    SettingsService，所以「刚改完设置就点转换」立刻生效；本项目偏好存在 HardwareDetector 里
    //    （由设置保存路径刷新），队列再同步一次，避免用旧偏好挑到别的编码器。
    //    只写偏好、**不**清报告缓存（`apply_settings` 会顺带 invalidate，那会让每个任务都重新试跑）。
    inner.hardware.set_preferences(crate::ffmpeg::hardware::HardwarePreferences {
        prefer_hardware_acceleration: settings.prefer_hardware_acceleration,
        preferred_hardware_encoder: {
            let preferred = settings.preferred_hardware_encoder.trim();
            if preferred.is_empty() { None } else { Some(preferred.to_string()) }
        },
    });
    let mut encoder: Option<EncoderProfile> = match resolve_encoder(&inner.hardware, &template).await
    {
        Ok(encoder) => encoder,
        Err(message) => return fail_before_start(inner, &mut task, &message).await,
    };

    // 10) 构建命令行（含 `-hwaccel` 注入）
    let mut disable_hardware_decode = false;
    let (first_built, _, first_hardware_decode_enabled) = build_attempt(
        &task,
        &template,
        encoder.clone(),
        media.as_ref(),
        &settings,
        disable_hardware_decode,
    );

    // 11) 落库：实际命令 + Running + 起跑时间
    task.actual_command = crate::command::render_command_line(&first_built.arguments);
    task.status = TaskStatus::Running;
    task.started_at = Some(Utc::now());
    task.error_message = None;
    task.progress = 0.0;
    task.current_time = 0.0;
    task.log_output = String::new();
    inner.store.update_task(&task).await?;

    // 14) 运行期上下文（日志缓冲 / 进度 / 产出标记）
    let runtime = TaskRuntime::new(task.clone());
    {
        let mut state = inner.state();
        state.runtimes.insert(task_id.to_string(), runtime.clone());
    }

    // 12) 命令构建警告与冲突说明写进日志（第一条提示）
    if !first_built.warnings.is_empty() {
        runtime
            .append_log(inner, format!("提示：{}", first_built.warnings.join("；")), false)
            .await;
    }
    if let Some(note) = resolution.note.clone() {
        runtime.append_log(inner, note, false).await;
    }

    // 13) 状态事件（PORT NOTE：参考实现把 old 硬编码成 `Pending`；这里用库里读到的真实旧值，
    //     语义上只影响「Running → Running」这种重启场景的事件内容，对本项目的路径等价）
    inner.emit(QueueEvent::Status {
        task_id: task.id.clone(),
        old_status: initial_status,
        new_status: TaskStatus::Running,
        error_message: None,
        task: runtime.snapshot(),
    });
    inner.emit_queue_changed();

    // 15) 第 1 次执行（重置产出标记 → 执行 → 取值判定，实现有意差异 #3）
    runtime.reset_produced_output();
    let mut outcome = run_once(inner, &runtime, first_built.arguments.clone(), resolution.overwrite, cancel)
        .await;

    // 16) 回退链第 1 步：关掉硬件解码，保留硬件编码
    let cancelled_by_user = || inner.is_cancel_requested(task_id) || cancel.is_cancelled();
    if !outcome.success && !outcome.cancelled && !cancelled_by_user() {
        let produced_any_output = runtime.take_produced_output();
        if settings.auto_fallback_to_software
            && first_hardware_decode_enabled
            && should_fallback_to_software(&outcome, produced_any_output)
        {
            runtime
                .append_log(inner, "⚠️ 硬件解码失败，正在改用软件解码重试……".to_string(), false)
                .await;
            runtime
                .append_log(inner, format!("失败原因：{}", first_line(&outcome.stderr_tail, 300)), false)
                .await;
            disable_hardware_decode = true;

            let (retry_built, _, _) = build_attempt(
                &task,
                &template,
                encoder.clone(),
                media.as_ref(),
                &settings,
                disable_hardware_decode,
            );
            runtime.set_actual_command(
                crate::command::render_command_line(&retry_built.arguments),
            );
            runtime.persist_all(inner).await;

            runtime.reset_produced_output();
            outcome =
                run_once(inner, &runtime, retry_built.arguments, resolution.overwrite, cancel).await;
        }
    }

    // 16) 回退链第 2 步：仍在用硬件编码器 → 换成同 codec 的软件编码器
    if !outcome.success && !outcome.cancelled && !cancelled_by_user() {
        let produced_any_output = runtime.take_produced_output();
        let hardware_profile = encoder.clone().filter(|profile| profile.is_hardware());
        if settings.auto_fallback_to_software && should_fallback_to_software(&outcome, produced_any_output)
        {
            if let Some(profile) = hardware_profile {
                let software = crate::ffmpeg::encoder_catalog::EncoderCatalog::software_encoder_for(
                    &profile.codec,
                )
                .filter(|candidate| !candidate.name.eq_ignore_ascii_case(&profile.name));

                if let Some(software) = software {
                    runtime
                        .append_log(
                            inner,
                            format!(
                                "⚠️ 硬件编码器 {} 执行失败，正在改用软件编码 {} 重试……",
                                profile.name, software.name
                            ),
                            false,
                        )
                        .await;
                    runtime
                        .append_log(
                            inner,
                            format!("失败原因：{}", first_line(&outcome.stderr_tail, 300)),
                            false,
                        )
                        .await;

                    let software_profile = software.clone();
                    encoder = Some(software_profile);
                    let (retry_built, _, _) = build_attempt(
                        &task,
                        &template,
                        encoder.clone(),
                        media.as_ref(),
                        &settings,
                        disable_hardware_decode,
                    );
                    runtime
                        .set_actual_command(crate::command::render_command_line(&retry_built.arguments));
                    runtime.persist_all(inner).await;

                    runtime.reset_produced_output();
                    outcome = run_once(
                        inner,
                        &runtime,
                        retry_built.arguments,
                        resolution.overwrite,
                        cancel,
                    )
                    .await;
                }
            }
        }
    }

    // 17) 终态写入
    let cancelled_by_user = inner.is_cancel_requested(task_id) || cancel.is_cancelled();
    finalize(inner, &runtime, &outcome, cancelled_by_user).await;

    // 18) 由 run_task 统一清理内存状态
    Ok(())
}

// ────────────────────────────────────────────────────────────────────────────
// 输出冲突解析（规格 §3.5 + §4.1）
// ────────────────────────────────────────────────────────────────────────────

/// 输出路径冲突解析。
///
/// * `output_path_resolved = true` → **跳过整个 Resolve**，用任务自己的路径与覆盖标记；
///   但「输出 == 输入」安全检查**永远**保留（会强制改名，避免毁掉源文件）。
/// * 否则按当前的 `settings.file_exists_action` 重新解析
///   （排队等待期间目标可能已被别人创建，所以必须重新判定）。
fn resolve_conflicts(task: &ConversionTask, settings: &AppSettings) -> OutputPathResolution {
    if task.output_path_resolved {
        if !task.output_path.trim().is_empty() && paths::paths_equal(&task.output_path, &task.input_path)
        {
            // 与 FileExistsAction 无关：输出的第 2 条规则短路一切策略
            return resolve_output_path(&task.output_path, FileExistsAction::Rename, &task.input_path);
        }
        return OutputPathResolution {
            output_path: task.output_path.clone(),
            overwrite: task.overwrite_existing,
            skip: false,
            renamed: false,
            note: None,
        };
    }

    resolve_output_path(&task.output_path, settings.file_exists_action, &task.input_path)
}

// ────────────────────────────────────────────────────────────────────────────
// 单次执行（规格 §3.7 的 `RunOnceAsync`）
// ────────────────────────────────────────────────────────────────────────────

/// 执行一次 ffmpeg，并把事件流（日志 / 进度）写进运行期上下文。
async fn run_once(
    inner: &Arc<QueueInner>,
    runtime: &Arc<TaskRuntime>,
    args: Vec<String>,
    overwrite: bool,
    cancel: &CancellationToken,
) -> RunOutcome {
    let total_duration = runtime.total_duration();
    let request = RunRequest::new(args)
        .with_duration_hint(total_duration)
        // 防挂起的核心：永远显式 `-y` / `-n`，绝不让 ffmpeg 读 stdin 提问（规格 §3.5）
        .with_overwrite(overwrite)
        .with_task_key(runtime.task_id.clone());

    let (sender, receiver) = mpsc::unbounded_channel::<RunEvent>();
    let done = CancellationToken::new();
    let consumer = {
        let inner = inner.clone();
        let runtime = runtime.clone();
        let done = done.clone();
        tokio::spawn(async move { consume_events(inner, runtime, receiver, done).await })
    };

    let result = inner.ffmpeg.run(request, Some(sender), cancel.clone()).await;
    // 进程结束后先让事件消费任务收口（它有界：把已排队的事件处理完就退出），
    // 再回去做终态写入 —— 「事件消费」与「终态落库」因此严格串行，
    // 不会出现「消费任务手里那份还在 Running 的快照把终态覆盖回去」。
    done.cancel();
    let _ = consumer.await;

    let outcome = match result {
        Ok(outcome) => outcome,
        Err(error) => {
            // 进程启动失败（可执行文件不存在等）：合成一个失败结果（规格 §3.9 退出码语义）
            let detail = error.to_string();
            if !detail.trim().is_empty() {
                runtime.append_log(inner, detail.clone(), false).await;
            }
            RunOutcome {
                exit_code: None,
                success: false,
                cancelled: false,
                timed_out: false,
                elapsed_ms: 0,
                stderr_tail: detail,
                log_lines: Vec::new(),
            }
        }
    };

    // 部分输出清理（规格 §3.10：仅「没有产出完整结果」且明显是残片时删除）
    cleanup_partial_output(&outcome, &runtime.output_path()).await;

    outcome
}

/// 消费 `RunEvent`：进度、日志、事件合并都在这一个任务里串行完成（避免 DB 写竞争）。
async fn consume_events(
    inner: Arc<QueueInner>,
    runtime: Arc<TaskRuntime>,
    mut receiver: mpsc::UnboundedReceiver<RunEvent>,
    done: CancellationToken,
) {
    let mut ticker = tokio::time::interval(LOG_EVENT_WINDOW);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    loop {
        tokio::select! {
            event = receiver.recv() => match event {
                Some(event) => handle_run_event(&inner, &runtime, event).await,
                None => break,
            },
            _ = done.cancelled() => {
                // 进程已结束：把已排队的事件排空再退出
                while let Ok(event) = receiver.try_recv() {
                    handle_run_event(&inner, &runtime, event).await;
                }
                break;
            }
            _ = ticker.tick() => {
                // 周期性：推送合并后的日志 + 按 700ms 节流落库
                runtime.flush_log_events(&inner);
                runtime.flush_logs_if_due(&inner).await;
            }
        }
    }

    // 退出前把剩余日志事件推给 UI（落库由 finalize 的强制 flush 负责）
    runtime.flush_log_events(&inner);
}

async fn handle_run_event(inner: &QueueInner, runtime: &TaskRuntime, event: RunEvent) {
    match event {
        RunEvent::Started { .. } => {}
        RunEvent::Stderr(line) => {
            // 普通 stderr 行不节流（规格 §3.8）
            runtime.append_log(inner, line, false).await;
        }
        RunEvent::Progress(tick) => runtime.on_progress(inner, tick).await,
        RunEvent::Finished { .. } => {}
    }
}

/// 部分输出清理（规格 §3.10）。
async fn cleanup_partial_output(outcome: &RunOutcome, output_path: &str) {
    if outcome.success || output_path.trim().is_empty() {
        return;
    }
    // 「不覆盖」策略产生的退出不算残片，绝不能删用户已有的文件
    if outcome.stderr_tail.starts_with(SKIPPED_PREFIX) {
        return;
    }
    let path = Path::new(output_path);
    let Ok(metadata) = tokio::fs::metadata(path).await else {
        return;
    };
    if !metadata.is_file() || metadata.len() >= PARTIAL_OUTPUT_DELETE_LIMIT {
        return;
    }
    // 删除失败静默忽略（参考实现同样忽略）
    let _ = tokio::fs::remove_file(path).await;
}

// ────────────────────────────────────────────────────────────────────────────
// 运行期上下文：内存任务副本 + 日志缓冲 + 节流状态
// ────────────────────────────────────────────────────────────────────────────

/// 待落库 / 待推送的日志缓冲
#[derive(Default)]
struct LogBuffers {
    /// 尚未写进数据库的行
    pending: Vec<String>,
    /// 上次落库时间（700 ms 节流判据）
    last_persisted: Option<Instant>,
    /// 尚未推送给 UI 的行（100–200ms 合并）
    event_buffer: VecDeque<String>,
    /// 因前端来不及消费而丢弃的行数（下一次事件里上报）
    dropped: usize,
    /// 是否发生过「按行数上限截断」
    truncated: bool,
}

/// 单个任务的运行期上下文。
///
/// PORT NOTE：参考实现把「内存里的任务对象 + `_pendingLogs` + `_lastPersisted` + 产出标记」散在
/// `ConcurrentDictionary` 里，落库时还依赖「读回文档、只覆盖部分字段」。这里每个任务一份
/// `TaskRuntime`，`live` 是**唯一权威副本**，落库即写整行 —— 语义等价，且不可能出现
/// 「旧对象把刚 flush 的日志覆盖掉」那类问题（规格 §3.4.1 专门警告过）。
struct TaskRuntime {
    task_id: String,
    live: Mutex<ConversionTask>,
    logs: Mutex<LogBuffers>,
    /// 本轮尝试是否产出过数据（有意差异 #3：每轮执行前重置、执行后判定）
    produced_any_output: AtomicBool,
    last_progress_event: Mutex<Option<Instant>>,
    last_progress_persist: Mutex<Option<Instant>>,
}

impl TaskRuntime {
    fn new(task: ConversionTask) -> Arc<Self> {
        Arc::new(Self {
            task_id: task.id.clone(),
            live: Mutex::new(task),
            logs: Mutex::new(LogBuffers::default()),
            produced_any_output: AtomicBool::new(false),
            last_progress_event: Mutex::new(None),
            last_progress_persist: Mutex::new(None),
        })
    }

    fn live(&self) -> MutexGuard<'_, ConversionTask> {
        self.live.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn logs(&self) -> MutexGuard<'_, LogBuffers> {
        self.logs.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn snapshot(&self) -> ConversionTask {
        self.live().clone()
    }

    fn total_duration(&self) -> f64 {
        self.live().total_duration
    }

    fn output_path(&self) -> String {
        self.live().output_path.clone()
    }

    fn set_actual_command(&self, command: String) {
        self.live().actual_command = command;
    }

    fn reset_produced_output(&self) {
        self.produced_any_output.store(false, Ordering::SeqCst);
    }

    fn take_produced_output(&self) -> bool {
        self.produced_any_output.swap(false, Ordering::SeqCst)
    }

    /// 只入缓冲（不同步落库），供终态日志使用
    fn push_log(&self, line: String) -> bool {
        let mut logs = self.logs();
        logs.pending.push(line.clone());
        logs.event_buffer.push_back(line);
        while logs.event_buffer.len() > LOG_EVENT_BUFFER_MAX {
            logs.event_buffer.pop_front();
            logs.dropped += 1;
        }
        logs.event_buffer.len() >= LOG_EVENT_MAX_LINES
    }

    /// 追加一行日志：入缓冲 → 按需立即合并推送 → 按 700 ms 节流落库（规格 §3.8）
    async fn append_log(&self, inner: &QueueInner, line: String, throttle: bool) {
        let flush_events_now = self.push_log(line);
        if flush_events_now {
            self.flush_log_events(inner);
        }

        let should_persist = {
            let logs = self.logs();
            !throttle
                || logs
                    .last_persisted
                    .map(|instant| instant.elapsed() >= LOG_PERSIST_THROTTLE)
                    .unwrap_or(true)
        };
        if should_persist {
            self.persist_logs(inner).await;
        }
    }

    /// 把缓冲里待推送的行合并成一个（最多 50 行的）事件
    fn flush_log_events(&self, inner: &QueueInner) {
        let (lines, dropped, truncated) = {
            let mut logs = self.logs();
            if logs.event_buffer.is_empty() && logs.dropped == 0 {
                return;
            }
            let take = logs.event_buffer.len().min(LOG_EVENT_MAX_LINES);
            let lines: Vec<String> = logs.event_buffer.drain(..take).collect();
            let dropped = std::mem::take(&mut logs.dropped);
            (lines, dropped, logs.truncated)
        };
        if lines.is_empty() && dropped == 0 {
            return;
        }
        inner.emit(QueueEvent::Log { task_id: self.task_id.clone(), lines, dropped, truncated });
    }

    /// 700 ms 节流到期时落库（周期性 ticker 调用；进度行的兜底）
    async fn flush_logs_if_due(&self, inner: &QueueInner) {
        let due = {
            let logs = self.logs();
            if logs.pending.is_empty() {
                false
            } else {
                logs.last_persisted
                    .map(|instant| instant.elapsed() >= LOG_PERSIST_THROTTLE)
                    .unwrap_or(true)
            }
        };
        if due {
            self.persist_logs(inner).await;
        }
    }

    /// 把待落库的行并进 `live.log_output`（按行数上限截断）并整行写库
    async fn persist_logs(&self, inner: &QueueInner) {
        // 先取设置（await 必须在持锁之前）
        let max_log_lines = inner.settings.cached().await.max_log_lines;

        let snapshot = {
            let mut logs = self.logs();
            if logs.pending.is_empty() {
                return;
            }
            let pending: Vec<String> = logs.pending.drain(..).collect();
            let mut live = self.live();

            let combined = if live.log_output.is_empty() {
                pending.join("\n")
            } else {
                format!("{}\n{}", live.log_output, pending.join("\n"))
            };
            let truncated = truncate_log(&combined, max_log_lines);
            live.log_output = truncated.text;
            logs.truncated |= truncated.truncated;
            logs.last_persisted = Some(Instant::now());
            live.clone()
        };

        if let Err(error) = inner.store.update_task(&snapshot).await {
            tracing::warn!("任务 {} 日志落库失败：{error:#}", self.task_id);
        }
    }

    /// 把内存副本整体落库（回退重试改写 `actual_command`、终态字段等场景）
    async fn persist_all(&self, inner: &QueueInner) {
        let snapshot = self.snapshot();
        if let Err(error) = inner.store.update_task(&snapshot).await {
            tracing::warn!("任务 {} 状态落库失败：{error:#}", self.task_id);
        }
    }

    /// 一次进度结算（规格 §3.7）
    async fn on_progress(&self, inner: &QueueInner, tick: crate::ffmpeg::service::ProgressTick) {
        let total_duration = { self.live().total_duration };
        let percent = if tick.percent >= 0.0 { tick.percent } else { 0.0 };
        let speed = tick.speed;
        let eta = tick.eta_seconds(total_duration);
        let current_time = tick.out_time_seconds;

        {
            let mut live = self.live();
            live.progress = percent;
            live.current_time = current_time;
            live.processing_speed = speed;
            live.estimated_time_remaining = eta;
        }
        self.produced_any_output.store(true, Ordering::SeqCst);

        // 1) 进度事件（服务端自造的上限：最快 10 次/秒，收尾那一次一定发）
        let is_end = tick
            .progress
            .as_deref()
            .map(|value| value.eq_ignore_ascii_case("end"))
            .unwrap_or(false)
            || percent >= 100.0;
        let emit_now = {
            let mut last = self
                .last_progress_event
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let due = last
                .map(|instant| instant.elapsed() >= PROGRESS_EVENT_MIN_INTERVAL)
                .unwrap_or(true);
            if due || is_end {
                *last = Some(Instant::now());
                true
            } else {
                false
            }
        };
        if emit_now {
            inner.emit(QueueEvent::Progress {
                task_id: self.task_id.clone(),
                progress: percent,
                speed,
                eta,
                current_time,
                total_duration,
            });
        }

        // 2) 进度字段落库（本项目有意差异 #4：最多每秒一次）
        let persist_now = {
            let mut last = self
                .last_progress_persist
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let due = last
                .map(|instant| instant.elapsed() >= PROGRESS_PERSIST_INTERVAL)
                .unwrap_or(true);
            if due {
                *last = Some(Instant::now());
                true
            } else {
                false
            }
        };
        if persist_now {
            self.persist_all(inner).await;
        }

        // 3) 进度摘要行进日志（节流落库）
        self.append_log(inner, progress_raw_output(&tick), true).await;
    }
}

// ────────────────────────────────────────────────────────────────────────────
// 终态
// ────────────────────────────────────────────────────────────────────────────

/// 终态写入（规格 §3.4.1）。
async fn finalize(
    inner: &Arc<QueueInner>,
    runtime: &Arc<TaskRuntime>,
    outcome: &RunOutcome,
    cancelled_by_user: bool,
) {
    let success = outcome.success && !cancelled_by_user && !outcome.cancelled;
    let old_status = { runtime.live().status };

    {
        let mut live = runtime.live();
        if cancelled_by_user || outcome.cancelled {
            live.status = TaskStatus::Cancelled;
            live.error_message = Some("用户取消".to_string());
        } else if success {
            live.status = TaskStatus::Completed;
            live.progress = 100.0;
            live.error_message = None;
        } else {
            live.status = TaskStatus::Failed;
            live.error_message = Some(if outcome.timed_out {
                "执行超时".to_string()
            } else {
                format!("FFmpeg 退出码 {}", outcome.exit_code.unwrap_or(-1))
            });
        }
        // 终态一律清掉运行期估算值（需求：终端状态必须清空 speed / ETA）
        live.completed_at = Some(Utc::now());
        live.processing_speed = None;
        live.estimated_time_remaining = None;
        live.output_file_size = None;
    }

    if cancelled_by_user || outcome.cancelled {
        runtime.push_log("任务已被取消。".to_string());
    } else if !success {
        runtime.push_log(format!("❌ 转换失败。FFmpeg 输出：\n{}", outcome.stderr_tail));
    }

    // 强制刷日志：**先落库**（这一步才会做行数截断），再推最后一个合并事件，
    // 这样 `Log::truncated` 能如实反映「落库的日志被截断过」
    runtime.persist_logs(inner).await;
    runtime.flush_log_events(inner);

    // 仅成功时记录输出大小
    if success {
        let output_path = runtime.output_path();
        if let Ok(metadata) = tokio::fs::metadata(&output_path).await {
            runtime.live().output_file_size = Some(metadata.len() as i64);
        }
    }
    runtime.persist_all(inner).await;

    let task = runtime.snapshot();
    inner.emit(QueueEvent::Status {
        task_id: task.id.clone(),
        old_status,
        new_status: task.status,
        error_message: task.error_message.clone(),
        task: task.clone(),
    });
    update_batch_status(inner, task.batch_id.as_deref()).await;
}

/// 「排队中（或库里仍是 Pending）」的任务被取消：不碰进程，直接落库成 `Cancelled`。
///
/// 规格 §3.9 的 `Pending` 分支：置 `Cancelled`、`CompletedAt = now`、
/// `ErrorMessage = 用户取消`，落库 + 事件 + 批次汇总。
async fn mark_cancelled(inner: &Arc<QueueInner>, task: &ConversionTask) -> anyhow::Result<()> {
    let mut task = task.clone();
    let old_status = task.status;
    task.status = TaskStatus::Cancelled;
    task.completed_at = Some(Utc::now());
    task.error_message = Some("用户取消".to_string());
    task.processing_speed = None;
    task.estimated_time_remaining = None;
    inner.store.update_task(&task).await?;
    inner.emit(QueueEvent::Status {
        task_id: task.id.clone(),
        old_status,
        new_status: TaskStatus::Cancelled,
        error_message: task.error_message.clone(),
        task: task.clone(),
    });
    update_batch_status(inner, task.batch_id.as_deref()).await;
    Ok(())
}

/// 起跑前的失败（模板缺失 / 目录不可创建 / 不可写 / 编码器不可用）。
async fn fail_before_start(
    inner: &Arc<QueueInner>,
    task: &mut ConversionTask,
    message: &str,
) -> anyhow::Result<()> {
    let old_status = task.status;
    task.status = TaskStatus::Failed;
    task.error_message = Some(message.to_string());
    task.completed_at = Some(Utc::now());
    task.processing_speed = None;
    task.estimated_time_remaining = None;
    task.output_file_size = None;
    // 日志不清空（规格 §3.4.2），只把原因追加进去
    if task.log_output.is_empty() {
        task.log_output = message.to_string();
    } else {
        task.log_output = format!("{}\n{}", task.log_output, message);
    }
    inner.store.update_task(task).await?;
    inner.emit(QueueEvent::Status {
        task_id: task.id.clone(),
        old_status,
        new_status: TaskStatus::Failed,
        error_message: task.error_message.clone(),
        task: task.clone(),
    });
    update_batch_status(inner, task.batch_id.as_deref()).await;
    Ok(())
}

/// 异常出口（规格 §3.4.2 `TryMarkFailed`）：日志保留已产生的部分。
async fn try_mark_failed(
    inner: &Arc<QueueInner>,
    task_id: &str,
    message: &str,
) -> anyhow::Result<()> {
    let Some(mut task) = inner.store.get_task(task_id).await? else {
        return Ok(());
    };
    // PORT NOTE：参考实现无条件覆盖成 Failed。这里加一道保护：已经进入终态的任务不再被
    // 「收尾阶段的一次 DB 抖动」翻改成 Failed（否则用户会看到「已完成的任务又变成失败」）。
    if task.status.is_terminal() {
        return Ok(());
    }
    let old_status = task.status;
    task.status = TaskStatus::Failed;
    task.error_message = Some(message.to_string());
    task.completed_at = Some(Utc::now());
    task.estimated_time_remaining = None;
    task.processing_speed = None;
    inner.store.update_task(&task).await?;
    inner.emit(QueueEvent::Status {
        task_id: task.id.clone(),
        old_status,
        new_status: TaskStatus::Failed,
        error_message: task.error_message.clone(),
        task: task.clone(),
    });
    update_batch_status(inner, task.batch_id.as_deref()).await;
    Ok(())
}

// ────────────────────────────────────────────────────────────────────────────
// 批次状态维护（规格 §3.11）
// ────────────────────────────────────────────────────────────────────────────

/// 重算批次计数与状态；每个子任务进入终态后调用。
///
/// * `completed_files` 只数 `Completed`；
/// * `failed_files` 数 `Failed + Cancelled`；**`Skipped` 两边都不计**（保留参考实现语义）；
/// * 全部子任务终态后，批次定为 `Completed`（无失败）或 `Failed`，且 `completed_at` 只写一次。
async fn update_batch_status(inner: &Arc<QueueInner>, batch_id: Option<&str>) {
    let Some(batch_id) = batch_id.map(str::trim).filter(|value| !value.is_empty()) else {
        return;
    };
    let batch = match inner.store.get_batch(batch_id).await {
        Ok(Some(batch)) => batch,
        Ok(None) => return,
        Err(error) => {
            tracing::warn!("读取批次 {batch_id} 失败：{error:#}");
            return;
        }
    };
    let tasks = match inner.store.list_tasks_by_batch(batch_id).await {
        Ok(tasks) => tasks,
        Err(error) => {
            tracing::warn!("读取批次 {batch_id} 的子任务失败：{error:#}");
            return;
        }
    };
    if tasks.is_empty() {
        return;
    }

    let mut batch = batch;
    batch.total_files = tasks.len() as i32;
    batch.completed_files = tasks.iter().filter(|task| task.status == TaskStatus::Completed).count() as i32;
    batch.failed_files = tasks
        .iter()
        .filter(|task| matches!(task.status, TaskStatus::Failed | TaskStatus::Cancelled))
        .count() as i32;

    let finished = tasks.iter().all(|task| task.status.is_terminal());
    if finished {
        batch.status = if batch.failed_files == 0 { TaskStatus::Completed } else { TaskStatus::Failed };
        if batch.completed_at.is_none() {
            batch.completed_at = Some(Utc::now());
        }
    } else if tasks.iter().any(|task| task.status == TaskStatus::Running) {
        batch.status = TaskStatus::Running;
    }

    if let Err(error) = inner.store.update_batch(&batch).await {
        tracing::warn!("更新批次 {batch_id} 失败：{error:#}");
        return;
    }
    inner.emit(QueueEvent::BatchStatus {
        batch_id: batch.id.clone(),
        status: batch.status,
        total_files: batch.total_files,
        completed_files: batch.completed_files,
        failed_files: batch.failed_files,
    });
}

/// 输入文件大小：优先文件系统（最可靠），探测失败时退回 ffprobe 的 `size_bytes`。
fn input_file_size(media: Option<&MediaInfo>, input_path: &str) -> i64 {
    if let Ok(metadata) = std::fs::metadata(input_path) {
        if metadata.is_file() {
            return metadata.len() as i64;
        }
    }
    media.map(|media| media.size_bytes as i64).unwrap_or(0)
}

#[cfg(test)]
mod tests;
