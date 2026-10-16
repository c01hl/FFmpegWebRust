//! 队列执行引擎的测试：真实 SQLite（内存库）+ 本机真实 ffmpeg，不用任何桩件。
//!
//! 覆盖需求点名的条目：端到端、入队幂等、Skip / Rename / 输出==输入、取消（运行中强杀 / 运行中优雅 /
//! 排队中）、失败路径与部分输出清理、批次聚合、并发上限（2 并行 / 1 串行 / 调小立刻生效）、
//! 日志截断、日志事件合并、运行期进度落库、停机、编码器解析与严格硬件模式。

use std::path::Path;
use std::path::PathBuf;
use std::process::Command as StdCommand;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tokio_util::sync::CancellationToken;

use crate::ffmpeg::hardware::HardwareDetector;
use crate::ffmpeg::locator;
use crate::ffmpeg::service::{FFmpegService, RunRequest};
use crate::models::enums::{FileExistsAction, HardwareAccelerationMode, TaskStatus};
use crate::models::{AppSettings, BatchTask, CommandTemplate, ConversionTask};
use crate::queue::{ConversionQueue, QueueEvent};
use crate::services::SettingsService;
use crate::store::Store;
use crate::templates::TemplateService;

use super::{LOG_EVENT_MAX_LINES, TaskRuntime};

/// 一次测试用的完整环境（含队列事件收集器）
struct Harness {
    store: Arc<Store>,
    settings: Arc<SettingsService>,
    ffmpeg: Arc<FFmpegService>,
    hardware: Arc<HardwareDetector>,
    queue: ConversionQueue,
    events: Arc<Mutex<Vec<QueueEvent>>>,
    collector: tokio::task::JoinHandle<()>,
    dir: PathBuf,
}

impl Drop for Harness {
    fn drop(&mut self) {
        self.collector.abort();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

impl Harness {
    async fn new(tag: &str) -> Self {
        let store = Arc::new(Store::open_in_memory().await.expect("打开内存库"));
        let settings = Arc::new(SettingsService::new(store.clone()));
        let app_settings = settings.get().await;
        let ffmpeg = Arc::new(FFmpegService::new_with_paths(locator::resolve(None, None)));
        let hardware = Arc::new(HardwareDetector::new(ffmpeg.clone()));
        hardware.apply_settings(&app_settings);
        let templates =
            Arc::new(TemplateService::new(store.clone(), ffmpeg.clone(), hardware.clone()));

        let queue = ConversionQueue::new(
            store.clone(),
            settings.clone(),
            templates,
            ffmpeg.clone(),
            hardware.clone(),
        );
        queue.start();

        let events = Arc::new(Mutex::new(Vec::new()));
        let collector = {
            let mut receiver = queue.subscribe();
            let events = events.clone();
            tokio::spawn(async move {
                while let Ok(event) = receiver.recv().await {
                    events.lock().unwrap().push(event);
                }
            })
        };

        let dir = crate::paths::temp_path(&format!("queue-{tag}"), "");
        std::fs::create_dir_all(&dir).expect("创建测试目录");

        Self { store, settings, ffmpeg, hardware, queue, events, collector, dir }
    }

    fn path(&self, name: &str) -> String {
        self.dir.join(name).to_string_lossy().to_string()
    }

    /// 用真实 ffmpeg 生成一个极小的测试素材（无音轨，转换最快）
    async fn source_clip(&self, name: &str, seconds: f64) -> String {
        let output = self.path(name);
        let args: Vec<String> = [
            "-f",
            "lavfi",
            "-i",
            &format!("testsrc2=duration={seconds}:size=96x54:rate=10"),
            "-c:v",
            "libx264",
            "-preset",
            "ultrafast",
            "-crf",
            "35",
            "-pix_fmt",
            "yuv420p",
            "-an",
            &output,
        ]
        .iter()
        .map(|value| value.to_string())
        .collect();

        let outcome = self
            .ffmpeg
            .run(RunRequest::new(args), None, CancellationToken::new())
            .await
            .expect("生成测试素材");
        assert!(outcome.success, "生成测试素材失败：{}", outcome.stderr_tail);
        assert!(Path::new(&output).is_file());
        output
    }

    /// 纯软件模板：命令里没有 `{encoder}`、`hardware_mode = None` → 完全不触发硬件检测
    async fn software_template(&self, extra: &str) -> CommandTemplate {
        self.insert_template(&software_args(extra), "mp4").await
    }

    async fn insert_template(&self, command_args: &str, extension: &str) -> CommandTemplate {
        let template = CommandTemplate {
            name: format!("测试模板 {}", crate::models::new_id()),
            command_args: command_args.to_string(),
            output_extension: extension.to_string(),
            target_codec: "h264".to_string(),
            ..CommandTemplate::default()
        };
        self.store.insert_template(&template).await.expect("写入模板");
        template
    }

    async fn insert_task(
        &self,
        template: &CommandTemplate,
        input: &str,
        output: &str,
    ) -> ConversionTask {
        let task = ConversionTask {
            input_path: input.to_string(),
            output_path: output.to_string(),
            template_id: template.id.clone(),
            template_name: template.name.clone(),
            ..ConversionTask::default()
        };
        self.store.insert_task(&task).await.expect("写入任务");
        task
    }

    async fn save_settings(&self, mutate: impl FnOnce(&mut AppSettings)) {
        let mut settings = self.settings.get().await;
        mutate(&mut settings);
        self.settings.save(settings).await.expect("保存设置");
    }

    async fn task(&self, task_id: &str) -> ConversionTask {
        self.store.get_task(task_id).await.expect("读任务").expect("任务存在")
    }

    /// 轮询直到任务进入终态
    async fn wait_terminal(&self, task_id: &str, timeout: Duration) -> ConversionTask {
        let deadline = Instant::now() + timeout;
        loop {
            let task = self.task(task_id).await;
            if task.status.is_terminal() {
                return task;
            }
            if Instant::now() >= deadline {
                panic!(
                    "任务 {task_id} 在 {timeout:?} 内没有进入终态（当前 {}）",
                    task.status.display_name()
                );
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    /// 轮询直到条件成立（返回是否在超时前成立）
    async fn wait_until(&self, timeout: Duration, mut condition: impl FnMut() -> bool) -> bool {
        let deadline = Instant::now() + timeout;
        loop {
            if condition() {
                return true;
            }
            if Instant::now() >= deadline {
                return false;
            }
            tokio::time::sleep(Duration::from_millis(15)).await;
        }
    }

    fn status_events(&self, task_id: &str) -> Vec<(TaskStatus, TaskStatus)> {
        self.events
            .lock()
            .unwrap()
            .iter()
            .filter_map(|event| match event {
                QueueEvent::Status { task_id: id, old_status, new_status, .. } if id == task_id => {
                    Some((*old_status, *new_status))
                }
                _ => None,
            })
            .collect()
    }

    fn running_event_count(&self, task_id: &str) -> usize {
        self.status_events(task_id)
            .into_iter()
            .filter(|(_, new_status)| *new_status == TaskStatus::Running)
            .count()
    }

    fn progress_events(&self, task_id: &str) -> Vec<f64> {
        self.events
            .lock()
            .unwrap()
            .iter()
            .filter_map(|event| match event {
                QueueEvent::Progress { task_id: id, progress, .. } if id == task_id => {
                    Some(*progress)
                }
                _ => None,
            })
            .collect()
    }

    fn log_events(&self, task_id: &str) -> Vec<Vec<String>> {
        self.events
            .lock()
            .unwrap()
            .iter()
            .filter_map(|event| match event {
                QueueEvent::Log { task_id: id, lines, .. } if id == task_id => Some(lines.clone()),
                _ => None,
            })
            .collect()
    }

    fn every_log_event(&self) -> Vec<(Vec<String>, bool)> {
        self.events
            .lock()
            .unwrap()
            .iter()
            .filter_map(|event| match event {
                QueueEvent::Log { lines, truncated, .. } => Some((lines.clone(), *truncated)),
                _ => None,
            })
            .collect()
    }
}

/// 纯软件命令行（`extra` 插在输入**之前**，便于塞 `-re` / `-loglevel debug` 这类输入期选项）
fn software_args(extra: &str) -> String {
    format!("{extra} -i {{input}} -c:v libx264 -preset ultrafast -crf 35 -pix_fmt yuv420p -an {{output}}")
}

// ════════════════════════════════════════════════════════════════════════════
// 事件序列化（形状必须与 docs/api-contract.md §2 一致）
// ════════════════════════════════════════════════════════════════════════════

#[test]
fn queue_events_serialize_to_the_documented_shape() {
    let progress = QueueEvent::Progress {
        task_id: "t1".to_string(),
        progress: 12.5,
        speed: Some(1.25),
        eta: Some(3.0),
        current_time: 1.0,
        total_duration: 8.0,
    };
    let json = serde_json::to_value(&progress).unwrap();
    assert_eq!(json["type"], "progress");
    assert_eq!(json["taskId"], "t1");
    assert_eq!(json["progress"], 12.5);
    assert_eq!(json["speed"], 1.25);
    assert_eq!(json["eta"], 3.0);
    assert_eq!(json["currentTime"], 1.0);
    assert_eq!(json["totalDuration"], 8.0);

    let task = ConversionTask::default();
    let status = QueueEvent::Status {
        task_id: task.id.clone(),
        old_status: TaskStatus::Pending,
        new_status: TaskStatus::Running,
        error_message: None,
        task: task.clone(),
    };
    let json = serde_json::to_value(&status).unwrap();
    assert_eq!(json["type"], "status");
    assert_eq!(json["oldStatus"], "Pending");
    assert_eq!(json["newStatus"], "Running");
    assert_eq!(json["task"]["id"], serde_json::json!(task.id));
    assert!(json["errorMessage"].is_null());

    let log = QueueEvent::Log {
        task_id: "t1".to_string(),
        lines: vec!["a".to_string(), "b".to_string()],
        dropped: 3,
        truncated: true,
    };
    let json = serde_json::to_value(&log).unwrap();
    assert_eq!(json["type"], "log");
    assert_eq!(json["lines"].as_array().unwrap().len(), 2);
    assert_eq!(json["dropped"], 3);
    assert_eq!(json["truncated"], true);

    let changed =
        QueueEvent::QueueChanged { queued: 2, active: 1 };
    let json = serde_json::to_value(&changed).unwrap();
    assert_eq!(json["type"], "queueChanged");
    assert_eq!(json["queued"], 2);
    assert_eq!(json["active"], 1);

    let batch = QueueEvent::BatchStatus {
        batch_id: "b1".to_string(),
        status: TaskStatus::Completed,
        total_files: 3,
        completed_files: 3,
        failed_files: 0,
    };
    let json = serde_json::to_value(&batch).unwrap();
    assert_eq!(json["type"], "batchStatus");
    assert_eq!(json["batchId"], "b1");
    assert_eq!(json["completedFiles"], 3);
    assert_eq!(json["failedFiles"], 0);

    // 事件必须可以跨线程广播（axum AppState 要求）
    fn assert_send_sync<T: Send + Sync + 'static>() {}
    assert_send_sync::<QueueEvent>();
    assert_send_sync::<ConversionQueue>();
}

/// 某模式（输出文件名里的唯一标记）当前是否还有进程活着；`None` = 拿不到 `pgrep`
fn process_alive(pattern: &str) -> Option<bool> {
    let output = StdCommand::new("pgrep").args(["-f", pattern]).output().ok()?;
    Some(!output.stdout.is_empty())
}

// ════════════════════════════════════════════════════════════════════════════
// 端到端
// ════════════════════════════════════════════════════════════════════════════

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn enqueue_runs_to_completion_end_to_end() {
    let harness = Harness::new("e2e").await;
    let input = harness.source_clip("in.mp4", 2.0).await;
    // `-re` 让输入按真实速率读取，2 秒素材必然产生多个进度块
    let template = harness.software_template("-re").await;
    let output = harness.path("out.mp4");
    let task = harness.insert_task(&template, &input, &output).await;

    harness.queue.enqueue(&task.id).await.unwrap();
    let finished = harness.wait_terminal(&task.id, Duration::from_secs(90)).await;

    // 数据库状态
    assert_eq!(
        finished.status,
        TaskStatus::Completed,
        "错误：{:?}\n日志：{}",
        finished.error_message,
        finished.log_output
    );
    assert_eq!(finished.progress, 100.0);
    assert!(finished.error_message.is_none());
    assert!(finished.started_at.is_some());
    assert!(finished.completed_at.is_some());
    assert!(finished.duration_seconds().unwrap_or(0.0) > 0.0);
    assert!(finished.total_duration > 0.0, "ffprobe 的总时长应写回任务");
    assert!(finished.input_file_size > 0);
    assert!(finished.output_file_size.unwrap_or(0) > 0);
    assert!(finished.actual_command.contains("libx264"), "实际命令：{}", finished.actual_command);
    // 终态必须清掉运行期估算
    assert!(finished.processing_speed.is_none());
    assert!(finished.estimated_time_remaining.is_none());

    // 输出文件存在且非空
    let metadata = std::fs::metadata(&finished.output_path).expect("输出文件存在");
    assert!(metadata.len() > 0);

    // 进度事件：范围合法、出现过中间值、最后一个必然是 100
    // （事件经广播 → 收集任务，读之前要等收集器追上）
    let progresses = harness.settled_progress(&task.id).await;
    assert!(!progresses.is_empty(), "应收到进度事件");
    assert!(progresses.iter().all(|percent| (0.0..=100.0).contains(percent)));
    assert!(
        progresses.iter().any(|percent| *percent > 0.0 && *percent < 100.0),
        "应有中间进度：{progresses:?}"
    );
    assert_eq!(progresses.last().copied(), Some(100.0));

    // 日志事件：合并后每帧不超过 50 行，内容含进度摘要行
    let logs = harness.settled_logs(&task.id).await;
    assert!(!logs.is_empty(), "应收到日志事件");
    assert!(logs.iter().all(|lines| lines.len() <= LOG_EVENT_MAX_LINES));
    let all_lines: Vec<String> = logs.iter().flatten().cloned().collect();
    assert!(
        all_lines.iter().any(|line| line.contains("time=")),
        "日志里应有进度摘要行：{all_lines:?}"
    );
    assert!(finished.log_output.contains("time="));

    // 状态事件：Pending → Running → Completed
    let statuses = harness.status_events(&task.id);
    assert_eq!(statuses.first().copied(), Some((TaskStatus::Pending, TaskStatus::Running)));
    assert!(statuses.iter().any(|(_, new_status)| *new_status == TaskStatus::Completed));

    // 队列计数归零
    assert!(harness.wait_until(Duration::from_secs(5), || harness.queue.active_count() == 0).await);
    assert_eq!(harness.queue.queued_count(), 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn duplicate_enqueue_runs_the_task_exactly_once() {
    let harness = Harness::new("dedup").await;
    let input = harness.source_clip("in.mp4", 1.0).await;
    let template = harness.software_template("").await;
    let task = harness.insert_task(&template, &input, &harness.path("out.mp4")).await;

    harness.queue.enqueue(&task.id).await.unwrap();
    harness.queue.enqueue(&task.id).await.unwrap();
    harness.queue.enqueue(&task.id).await.unwrap();

    let finished = harness.wait_terminal(&task.id, Duration::from_secs(60)).await;
    assert_eq!(finished.status, TaskStatus::Completed);
    assert_eq!(harness.settled_running_count(&task.id, 1).await, 1, "同一任务只能被执行一次");

    // 终态并被清理后再次入队 → 会重新执行（供「重试 / 再次开始」使用）
    let mut reset = harness.task(&task.id).await;
    reset.status = TaskStatus::Pending;
    reset.progress = 0.0;
    reset.completed_at = None;
    harness.store.update_task(&reset).await.unwrap();
    harness.queue.enqueue(&task.id).await.unwrap();
    let again = harness.wait_terminal(&task.id, Duration::from_secs(60)).await;
    assert_eq!(again.status, TaskStatus::Completed);
    assert_eq!(harness.running_event_count(&task.id), 2, "终态清理后允许再次执行");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn conversion_does_not_depend_on_any_subscriber() {
    // 参考实现最重要的不变式：转换不依赖 UI 连接。这里起一个**完全没有任何订阅者**的队列
    // （等价于「页面刷新 / WebSocket 断开 / 浏览器关掉」），任务必须照样跑完、日志照样落库。
    let harness = Harness::new("nolisteners").await;
    let templates = Arc::new(TemplateService::new(
        harness.store.clone(),
        harness.ffmpeg.clone(),
        harness.hardware.clone(),
    ));
    let queue = ConversionQueue::new(
        harness.store.clone(),
        harness.settings.clone(),
        templates,
        harness.ffmpeg.clone(),
        harness.hardware.clone(),
    );
    queue.start();

    let input = harness.source_clip("in.mp4", 1.0).await;
    let template = harness.software_template("").await;
    let task = harness.insert_task(&template, &input, &harness.path("out.mp4")).await;

    queue.enqueue(&task.id).await.unwrap();
    let finished = harness.wait_terminal(&task.id, Duration::from_secs(60)).await;

    assert_eq!(finished.status, TaskStatus::Completed, "{:?}", finished.error_message);
    assert!(Path::new(&finished.output_path).is_file());
    assert!(!finished.log_output.is_empty(), "即使没人订阅，日志也必须落库");
    assert_eq!(queue.active_count(), 0);
    assert_eq!(queue.queued_count(), 0);
}

// ════════════════════════════════════════════════════════════════════════════
// 输出路径策略（规格 §3.5 / §4.1）
// ════════════════════════════════════════════════════════════════════════════

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn skip_policy_marks_the_task_skipped_and_keeps_the_existing_file() {
    let harness = Harness::new("skip").await;
    harness.save_settings(|settings| settings.file_exists_action = FileExistsAction::Skip).await;

    let input = harness.source_clip("in.mp4", 1.0).await;
    let template = harness.software_template("").await;
    let output = harness.path("out.mp4");
    std::fs::write(&output, b"existing-content").unwrap();

    let task = harness.insert_task(&template, &input, &output).await;
    harness.queue.enqueue(&task.id).await.unwrap();
    let finished = harness.wait_terminal(&task.id, Duration::from_secs(30)).await;

    assert_eq!(finished.status, TaskStatus::Skipped);
    assert_eq!(finished.log_output, "输出文件已存在，按设置跳过本次转换。");
    assert!(finished.error_message.is_none());
    assert!(finished.completed_at.is_some());
    assert!(finished.actual_command.is_empty(), "跳过时不应启动 ffmpeg");
    assert_eq!(std::fs::read(&output).unwrap(), b"existing-content");
    assert_eq!(harness.running_event_count(&task.id), 0, "跳过时不应有 Running 事件");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn rename_policy_produces_an_indexed_name() {
    let harness = Harness::new("rename").await;
    harness.save_settings(|settings| settings.file_exists_action = FileExistsAction::Rename).await;

    let input = harness.source_clip("in.mp4", 1.0).await;
    let template = harness.software_template("").await;
    let output = harness.path("out.mp4");
    std::fs::write(&output, b"old").unwrap();

    let task = harness.insert_task(&template, &input, &output).await;
    harness.queue.enqueue(&task.id).await.unwrap();
    let finished = harness.wait_terminal(&task.id, Duration::from_secs(60)).await;

    assert_eq!(finished.status, TaskStatus::Completed);
    assert!(finished.output_path.ends_with("out (1).mp4"), "实际：{}", finished.output_path);
    assert!(Path::new(&finished.output_path).is_file());
    assert_eq!(std::fs::read(&output).unwrap(), b"old", "原文件不能被碰");
    assert!(finished.log_output.contains("已自动重命名为 out (1).mp4"), "{}", finished.log_output);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn output_equal_to_input_forces_a_rename_and_keeps_the_source_intact() {
    let harness = Harness::new("selfsafe").await;
    // 最危险的组合：策略是「覆盖」，且输出路径等于输入路径
    harness
        .save_settings(|settings| settings.file_exists_action = FileExistsAction::Overwrite)
        .await;

    let input = harness.source_clip("in.mp4", 1.0).await;
    let before = std::fs::read(&input).unwrap();
    let template = harness.software_template("").await;
    let task = harness.insert_task(&template, &input, &input).await;

    harness.queue.enqueue(&task.id).await.unwrap();
    let finished = harness.wait_terminal(&task.id, Duration::from_secs(60)).await;

    assert_eq!(finished.status, TaskStatus::Completed);
    assert_ne!(finished.output_path, input);
    assert!(finished.output_path.ends_with("in (1).mp4"), "实际：{}", finished.output_path);
    assert!(Path::new(&finished.output_path).is_file());
    assert!(
        finished
            .log_output
            .contains("输出路径与输入文件相同，已自动改名以避免覆盖源文件。"),
        "{}",
        finished.log_output
    );
    // 源文件必须逐字节不变
    assert_eq!(std::fs::read(&input).unwrap(), before);
}

// ════════════════════════════════════════════════════════════════════════════
// 取消（规格 §3.9 / §3.10）
// ════════════════════════════════════════════════════════════════════════════

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn forceful_cancel_kills_the_running_process() {
    let harness = Harness::new("cancel-force").await;
    let input = harness.source_clip("in.mp4", 8.0).await;
    let template = harness.software_template("-re").await;
    // 唯一标记（任务 id）会出现在 ffmpeg 的 argv 里，便于用 pgrep 确认进程确实没了
    let marker = format!("cancelforce{}", crate::models::new_id());
    let output = harness.path(&format!("{marker}.mp4"));
    let task = harness.insert_task(&template, &input, &output).await;

    harness.queue.enqueue(&task.id).await.unwrap();
    assert!(harness.wait_until(Duration::from_secs(30), || harness.queue.active_count() == 1).await);
    // 让 ffmpeg 真正跑一会儿，确保已经产生部分输出
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_ne!(process_alive(&marker), Some(false), "此时 ffmpeg 进程应当存在");

    assert!(harness.queue.cancel(&task.id, false).await.unwrap());
    let finished = harness.wait_terminal(&task.id, Duration::from_secs(20)).await;

    assert_eq!(finished.status, TaskStatus::Cancelled);
    assert_eq!(finished.error_message.as_deref(), Some("用户取消"));
    assert!(finished.log_output.contains("任务已被取消。"), "{}", finished.log_output);
    assert!(finished.processing_speed.is_none() && finished.estimated_time_remaining.is_none());
    assert_eq!(finished.output_file_size, None);

    // 子进程必须已经消失（服务登记表 + 系统进程表双重确认）
    assert_eq!(harness.ffmpeg.running_count(), 0);
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline && process_alive(&marker).unwrap_or(false) {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_ne!(process_alive(&marker), Some(true), "ffmpeg 子进程仍存活");
    // 部分输出（< 64 MiB）必须被清理
    assert!(!Path::new(&finished.output_path).exists(), "部分输出应被删除");
    assert!(harness.wait_until(Duration::from_secs(5), || harness.queue.active_count() == 0).await);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn graceful_cancel_writes_q_and_ends_the_task() {
    let harness = Harness::new("cancel-graceful").await;
    let input = harness.source_clip("in.mp4", 8.0).await;
    let template = harness.software_template("-re").await;
    let marker = format!("cancelgrace{}", crate::models::new_id());
    let output = harness.path(&format!("{marker}.mp4"));
    let task = harness.insert_task(&template, &input, &output).await;

    harness.queue.enqueue(&task.id).await.unwrap();
    assert!(harness.wait_until(Duration::from_secs(30), || harness.queue.active_count() == 1).await);
    tokio::time::sleep(Duration::from_millis(400)).await;

    assert!(harness.queue.cancel(&task.id, true).await.unwrap());
    let finished = harness.wait_terminal(&task.id, Duration::from_secs(20)).await;

    assert_eq!(finished.status, TaskStatus::Cancelled);
    assert!(
        finished.log_output.contains("已发送优雅退出指令（q）"),
        "日志：{}",
        finished.log_output
    );
    assert!(finished.log_output.contains("任务已被取消。"));
    assert_eq!(harness.ffmpeg.running_count(), 0);
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline && process_alive(&marker).unwrap_or(false) {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_ne!(process_alive(&marker), Some(true), "ffmpeg 子进程仍存活");
    assert!(!Path::new(&finished.output_path).exists(), "取消后部分输出应被清理");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cancelling_a_pending_task_never_starts_ffmpeg() {
    let harness = Harness::new("cancel-pending").await;
    harness.save_settings(|settings| settings.max_concurrent_tasks = 1).await;

    let input = harness.source_clip("in.mp4", 2.0).await;
    let template = harness.software_template("-re").await;
    let first = harness.insert_task(&template, &input, &harness.path("first.mp4")).await;
    let second = harness.insert_task(&template, &input, &harness.path("second.mp4")).await;

    harness.queue.enqueue(&first.id).await.unwrap();
    harness.queue.enqueue(&second.id).await.unwrap();
    assert!(harness.wait_until(Duration::from_secs(30), || harness.queue.active_count() == 1).await);
    assert_eq!(harness.queue.queued_count(), 1, "第二个任务应当在排队");

    assert!(harness.queue.cancel(&second.id, false).await.unwrap());
    assert_eq!(harness.queue.queued_count(), 0);

    let cancelled = harness.task(&second.id).await;
    assert_eq!(cancelled.status, TaskStatus::Cancelled);
    assert_eq!(cancelled.error_message.as_deref(), Some("用户取消"));
    assert!(cancelled.started_at.is_none(), "从未启动");
    assert!(cancelled.actual_command.is_empty());

    // 第一个任务不受影响
    let finished = harness.wait_terminal(&first.id, Duration::from_secs(90)).await;
    assert_eq!(finished.status, TaskStatus::Completed);
    // 取消一个已结束的任务 → false
    assert!(!harness.queue.cancel(&first.id, true).await.unwrap());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cancelling_a_never_enqueued_pending_task_marks_it_cancelled() {
    // §3.9 的 Pending 分支按**库里的状态**判断：批量「取消全部」会对从未入队的子任务调用 cancel，
    // 这种情况必须同样置 Cancelled 并触发批次汇总（否则静默失效）。
    let harness = Harness::new("cancel-db-pending").await;
    let template = harness.software_template("").await;
    let input = harness.source_clip("in.mp4", 1.0).await;

    let batch = BatchTask {
        name: "批量任务 - 取消".to_string(),
        template_id: template.id.clone(),
        ..BatchTask::default()
    };
    harness.store.insert_batch(&batch).await.unwrap();
    let task = harness.insert_task(&template, &input, &harness.path("out.mp4")).await;
    let mut stored = harness.task(&task.id).await;
    stored.batch_id = Some(batch.id.clone());
    harness.store.update_task(&stored).await.unwrap();

    // 从未入队
    assert!(harness.queue.cancel(&task.id, false).await.unwrap());

    let cancelled = harness.task(&task.id).await;
    assert_eq!(cancelled.status, TaskStatus::Cancelled);
    assert_eq!(cancelled.error_message.as_deref(), Some("用户取消"));
    assert!(cancelled.completed_at.is_some());
    assert!(cancelled.started_at.is_none());

    // 批次汇总必须被触发：唯一子任务被取消 → failed_files = 1、批次 Failed
    let updated = harness.wait_for_batch(&batch.id, Duration::from_secs(5)).await;
    assert_eq!(updated.total_files, 1);
    assert_eq!(updated.completed_files, 0);
    assert_eq!(updated.failed_files, 1);
    assert_eq!(updated.status, TaskStatus::Failed);
}

// ════════════════════════════════════════════════════════════════════════════
// 失败路径
// ════════════════════════════════════════════════════════════════════════════
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn missing_input_fails_with_the_documented_message_and_leaves_no_partial_output() {
    let harness = Harness::new("badinput").await;
    let template = harness.software_template("").await;
    let output = harness.path("out.mp4");
    let task = harness.insert_task(&template, "/definitely/not/here/missing.mp4", &output).await;

    harness.queue.enqueue(&task.id).await.unwrap();
    let finished = harness.wait_terminal(&task.id, Duration::from_secs(60)).await;

    assert_eq!(finished.status, TaskStatus::Failed);
    let message = finished.error_message.clone().unwrap_or_default();
    assert!(message.starts_with("FFmpeg 退出码"), "实际消息：{message}");
    assert!(finished.log_output.contains("❌ 转换失败。FFmpeg 输出："), "{}", finished.log_output);
    assert!(finished.completed_at.is_some());
    assert_eq!(finished.output_file_size, None);
    assert!(!Path::new(&output).exists(), "不应留下部分输出");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn missing_template_fails_with_the_documented_message() {
    let harness = Harness::new("notemplate").await;
    let input = harness.source_clip("in.mp4", 1.0).await;
    let template = harness.software_template("").await;
    let task = harness.insert_task(&template, &input, &harness.path("out.mp4")).await;
    let mut stored = harness.task(&task.id).await;
    stored.template_id = "ffffffffffffffffffffffff".to_string();
    harness.store.update_task(&stored).await.unwrap();

    harness.queue.enqueue(&task.id).await.unwrap();
    let finished = harness.wait_terminal(&task.id, Duration::from_secs(30)).await;

    assert_eq!(finished.status, TaskStatus::Failed);
    assert_eq!(finished.error_message.as_deref(), Some("模板不存在，可能已被删除。"));
    assert!(finished.actual_command.is_empty());
    assert!(finished.log_output.contains("模板不存在，可能已被删除。"));
}

// ════════════════════════════════════════════════════════════════════════════
// 批次（规格 §3.11）
// ════════════════════════════════════════════════════════════════════════════

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn batch_status_is_aggregated_after_every_child_finishes() {
    let harness = Harness::new("batch").await;
    let template = harness.software_template("").await;
    let input = harness.source_clip("in.mp4", 1.0).await;

    let batch = BatchTask {
        name: "批量任务 - 测试".to_string(),
        template_id: template.id.clone(),
        ..BatchTask::default()
    };
    harness.store.insert_batch(&batch).await.unwrap();

    let mut ids = Vec::new();
    for index in 0..3 {
        let task =
            harness.insert_task(&template, &input, &harness.path(&format!("b{index}.mp4"))).await;
        let mut stored = harness.task(&task.id).await;
        stored.batch_id = Some(batch.id.clone());
        harness.store.update_task(&stored).await.unwrap();
        ids.push(task.id.clone());
        harness.queue.enqueue(&task.id).await.unwrap();
    }
    for id in &ids {
        harness.wait_terminal(id, Duration::from_secs(90)).await;
    }

    let updated = harness.wait_for_batch(&batch.id, Duration::from_secs(10)).await;
    assert_eq!(updated.total_files, 3);
    assert_eq!(updated.completed_files, 3);
    assert_eq!(updated.failed_files, 0);
    assert_eq!(updated.status, TaskStatus::Completed);
    assert!(updated.completed_at.is_some());

    // 每个子任务终态后都应有一次 BatchStatus 事件
    let _ = harness
        .wait_until(Duration::from_secs(5), || {
            harness.events.lock().unwrap().iter().filter(|event| {
                matches!(event, QueueEvent::BatchStatus { .. })
            }).count() >= 3
        })
        .await;
    let batch_events: Vec<(TaskStatus, i32, i32)> = harness
        .events
        .lock()
        .unwrap()
        .iter()
        .filter_map(|event| match event {
            QueueEvent::BatchStatus { status, completed_files, failed_files, .. } => {
                Some((*status, *completed_files, *failed_files))
            }
            _ => None,
        })
        .collect();
    assert!(batch_events.len() >= 3, "每个子任务结束后都应有批次事件：{batch_events:?}");
    assert_eq!(batch_events.last().copied(), Some((TaskStatus::Completed, 3, 0)));

    // 含失败子任务的批次：failed_files 计入 Failed/Cancelled，批次判为 Failed
    let bad_batch = BatchTask {
        name: "批量任务 - 含失败".to_string(),
        template_id: template.id.clone(),
        ..BatchTask::default()
    };
    harness.store.insert_batch(&bad_batch).await.unwrap();
    let mut bad_ids = Vec::new();
    for index in 0..2 {
        let input_path = if index == 0 {
            "/definitely/not/here/missing.mp4".to_string()
        } else {
            input.clone()
        };
        let task = harness
            .insert_task(&template, &input_path, &harness.path(&format!("bad{index}.mp4")))
            .await;
        let mut stored = harness.task(&task.id).await;
        stored.batch_id = Some(bad_batch.id.clone());
        harness.store.update_task(&stored).await.unwrap();
        bad_ids.push(task.id.clone());
        harness.queue.enqueue(&task.id).await.unwrap();
    }
    for id in &bad_ids {
        harness.wait_terminal(id, Duration::from_secs(90)).await;
    }
    let updated = harness.wait_for_batch(&bad_batch.id, Duration::from_secs(10)).await;
    assert_eq!(updated.completed_files, 1);
    assert_eq!(updated.failed_files, 1);
    assert_eq!(updated.status, TaskStatus::Failed);
    assert!(updated.completed_at.is_some());
}

// ════════════════════════════════════════════════════════════════════════════
// 并发上限（有意差异 #6）
// ════════════════════════════════════════════════════════════════════════════

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_tasks_run_concurrently_when_the_limit_is_two() {
    let harness = Harness::new("limit2").await;
    harness.save_settings(|settings| settings.max_concurrent_tasks = 2).await;

    let input = harness.source_clip("in.mp4", 3.0).await;
    let template = harness.software_template("-re").await;
    let a = harness.insert_task(&template, &input, &harness.path("a.mp4")).await;
    let b = harness.insert_task(&template, &input, &harness.path("b.mp4")).await;

    harness.queue.enqueue(&a.id).await.unwrap();
    harness.queue.enqueue(&b.id).await.unwrap();

    // 观测并发：active 必须达到 2（两个任务各需 ~3 秒，重叠可观测）
    let overlapped =
        harness.wait_until(Duration::from_secs(30), || harness.queue.active_count() == 2).await;
    assert!(overlapped, "并发上限为 2 时应当同时跑两个任务");

    harness.wait_terminal(&a.id, Duration::from_secs(120)).await;
    harness.wait_terminal(&b.id, Duration::from_secs(120)).await;
    assert_eq!(harness.task(&a.id).await.status, TaskStatus::Completed);
    assert_eq!(harness.task(&b.id).await.status, TaskStatus::Completed);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_limit_of_one_serialises_the_tasks() {
    let harness = Harness::new("limit1").await;
    harness.save_settings(|settings| settings.max_concurrent_tasks = 1).await;

    let input = harness.source_clip("in.mp4", 2.0).await;
    let template = harness.software_template("-re").await;
    let a = harness.insert_task(&template, &input, &harness.path("a.mp4")).await;
    let b = harness.insert_task(&template, &input, &harness.path("b.mp4")).await;

    harness.queue.enqueue(&a.id).await.unwrap();
    harness.queue.enqueue(&b.id).await.unwrap();

    // 两个任务都跑完之前，active 绝不能超过 1
    let mut max_active = 0;
    let deadline = Instant::now() + Duration::from_secs(120);
    loop {
        let active = harness.queue.active_count();
        max_active = max_active.max(active);
        assert!(active <= 1, "并发上限为 1 时同时只能有一个任务在跑");
        let finished = harness.task(&a.id).await.status.is_terminal()
            && harness.task(&b.id).await.status.is_terminal();
        if finished || Instant::now() >= deadline {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(max_active, 1);
    assert_eq!(harness.wait_terminal(&a.id, Duration::from_secs(60)).await.status, TaskStatus::Completed);
    assert_eq!(harness.wait_terminal(&b.id, Duration::from_secs(60)).await.status, TaskStatus::Completed);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn lowering_the_limit_takes_effect_for_subsequent_tasks() {
    // PORT NOTE（有意差异 #6）：参考实现的 worker 池「只增不减」，调小并发后实际并发不会下降；
    // 本实现的调度器每次派发前重新读取上限，因此调小立刻对后续任务生效。
    let harness = Harness::new("limitdown").await;
    harness.save_settings(|settings| settings.max_concurrent_tasks = 2).await;

    let input = harness.source_clip("in.mp4", 2.0).await;
    let template = harness.software_template("-re").await;
    let a = harness.insert_task(&template, &input, &harness.path("a.mp4")).await;
    let b = harness.insert_task(&template, &input, &harness.path("b.mp4")).await;
    harness.queue.enqueue(&a.id).await.unwrap();
    harness.queue.enqueue(&b.id).await.unwrap();
    assert!(harness.wait_until(Duration::from_secs(30), || harness.queue.active_count() == 2).await);

    // 调到 1：后入队的第三个任务必须等前面的跑完
    harness.save_settings(|settings| settings.max_concurrent_tasks = 1).await;
    let c = harness.insert_task(&template, &input, &harness.path("c.mp4")).await;
    harness.queue.enqueue(&c.id).await.unwrap();

    let mut started_early = false;
    let early_deadline = Instant::now() + Duration::from_millis(600);
    while Instant::now() < early_deadline {
        if harness.task(&c.id).await.status == TaskStatus::Running {
            started_early = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(!started_early, "并发上限调到 1 后，后续任务不应在前两个跑完前开始");

    harness.wait_terminal(&a.id, Duration::from_secs(120)).await;
    harness.wait_terminal(&b.id, Duration::from_secs(120)).await;
    harness.wait_terminal(&c.id, Duration::from_secs(120)).await;
    assert!(harness.wait_until(Duration::from_secs(5), || harness.queue.active_count() == 0).await);
}

// ════════════════════════════════════════════════════════════════════════════
// 日志与进度（有意差异 #4 / #5）
// ════════════════════════════════════════════════════════════════════════════

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn progress_is_persisted_during_the_run() {
    let harness = Harness::new("progressdb").await;
    let input = harness.source_clip("in.mp4", 3.0).await;
    let template = harness.software_template("-re").await;
    let task = harness.insert_task(&template, &input, &harness.path("out.mp4")).await;

    harness.queue.enqueue(&task.id).await.unwrap();

    // 运行期间读库：必须能看到 0 < progress < 100（参考实现这里永远是 0）
    let mut observed = false;
    let mut samples: Vec<f64> = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(20);
    while Instant::now() < deadline {
        let row = harness.task(&task.id).await;
        if row.status == TaskStatus::Running {
            samples.push(row.progress);
            if row.progress > 0.0 && row.progress < 100.0 && row.current_time > 0.0 {
                observed = true;
                break;
            }
        }
        if row.status.is_terminal() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(30)).await;
    }
    assert!(observed, "运行期应当把进度落库，采样：{samples:?}");

    let finished = harness.wait_terminal(&task.id, Duration::from_secs(60)).await;
    assert_eq!(finished.status, TaskStatus::Completed);
    assert_eq!(finished.progress, 100.0);
    assert!(finished.current_time > 0.0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn log_events_are_merged_in_batches_of_at_most_fifty_lines() {
    let harness = Harness::new("merge").await;
    // 直接驱动运行期上下文：137 行日志必须合并成「每帧 ≤ 50 行」的若干事件（有意差异 #5）
    let template = harness.software_template("").await;
    let task = harness.insert_task(&template, "unused", "unused").await;
    let runtime = TaskRuntime::new(task);
    let inner = &harness.queue.inner;

    for index in 0..137 {
        runtime.append_log(inner, format!("第 {index} 行"), false).await;
    }
    runtime.flush_log_events(inner);

    // 事件经广播 → 收集任务，等收集器追上再断言
    assert!(
        harness.wait_until(Duration::from_secs(5), || harness.every_log_event().len() >= 3).await,
        "137 行应至少拆成 3 个事件，实际 {}",
        harness.every_log_event().len()
    );
    let log_events: Vec<Vec<String>> =
        harness.every_log_event().into_iter().map(|(lines, _)| lines).collect();
    assert!(log_events.len() >= 3, "137 行应至少拆成 3 个事件，实际 {}", log_events.len());
    assert!(log_events.iter().all(|lines| lines.len() <= LOG_EVENT_MAX_LINES));
    let total: usize = log_events.iter().map(Vec::len).sum();
    assert_eq!(total, 137, "不能丢行");
    assert_eq!(log_events[0].len(), LOG_EVENT_MAX_LINES);
    assert_eq!(log_events.last().unwrap().len(), 37);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn log_truncation_respects_max_log_lines() {
    let harness = Harness::new("truncate").await;
    harness.save_settings(|settings| settings.max_log_lines = 50).await;

    // `-loglevel debug` 的 stderr 远超 50 行
    let input = harness.source_clip("in.mp4", 1.0).await;
    let template = harness.software_template("-loglevel debug").await;
    let task = harness.insert_task(&template, &input, &harness.path("out.mp4")).await;

    harness.queue.enqueue(&task.id).await.unwrap();
    let finished = harness.wait_terminal(&task.id, Duration::from_secs(90)).await;
    assert_eq!(finished.status, TaskStatus::Completed);

    let lines = finished.log_output.lines().count();
    assert!(lines <= 50, "日志行数应受 max_log_lines 限制，实际 {lines}");
    assert!(
        finished.log_output.contains("已省略早期"),
        "应记录截断提示：{}",
        finished.log_output
    );

    // 截断必须通过 Log 事件上报 truncated = true
    let observed = harness
        .wait_until(Duration::from_secs(5), || {
            harness.every_log_event().iter().any(|(_, truncated)| *truncated)
        })
        .await;
    assert!(
        observed,
        "没有任何 Log 事件上报 truncated = true；事件（行数, truncated）= {:?}",
        harness
            .every_log_event()
            .iter()
            .map(|(lines, truncated)| (lines.len(), *truncated))
            .collect::<Vec<_>>()
    );

    // 真实 ffmpeg 的日志事件同样满足「每帧 ≤ 50 行」，且确实发生了合并
    let log_events = harness.settled_logs(&task.id).await;
    assert!(!log_events.is_empty());
    assert!(log_events.iter().all(|lines| lines.len() <= LOG_EVENT_MAX_LINES));
    let total: usize = log_events.iter().map(Vec::len).sum();
    assert!(total >= 40, "debug 级别应产生几十行日志，实际 {total}");
    assert!(log_events.len() < total, "应当发生合并：{total} 行 / {} 个事件", log_events.len());
}

// ════════════════════════════════════════════════════════════════════════════
// 编码器解析（规格 §3.6.1 / templates.md §5.2）
// ════════════════════════════════════════════════════════════════════════════

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn encoder_placeholder_is_resolved_through_the_four_stage_priority() {
    let harness = Harness::new("encoder").await;
    // 第 2 级：用户在设置里指定的首选编码器
    harness
        .save_settings(|settings| settings.preferred_hardware_encoder = "libx264".to_string())
        .await;

    let mut template = harness
        .insert_template(
            "-i {input} -c:v {encoder} {quality_args} {preset_args} -an {output}",
            "mp4",
        )
        .await;
    template.hardware_mode = HardwareAccelerationMode::Auto;
    harness.store.update_template(&template).await.unwrap();

    let input = harness.source_clip("in.mp4", 1.0).await;
    let task = harness.insert_task(&template, &input, &harness.path("out.mp4")).await;
    harness.queue.enqueue(&task.id).await.unwrap();
    let finished = harness.wait_terminal(&task.id, Duration::from_secs(120)).await;

    assert_eq!(finished.status, TaskStatus::Completed, "日志：{}", finished.log_output);
    assert!(finished.actual_command.contains("libx264"), "实际命令：{}", finished.actual_command);
    assert!(!finished.actual_command.contains("{encoder}"));
    assert!(Path::new(&finished.output_path).is_file());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn required_hardware_that_is_missing_fails_with_the_documented_message() {
    let harness = Harness::new("required").await;
    let report = harness.hardware.detect(false).await;
    if report
        .available_encoders()
        .iter()
        .any(|encoder| encoder.family.eq_ignore_ascii_case("nvenc"))
    {
        // 本机真的有 NVENC：用例前提不成立，跳过
        return;
    }

    let mut template =
        harness.insert_template("-i {input} -c:v {encoder} -an {output}", "mp4").await;
    template.hardware_mode = HardwareAccelerationMode::Required;
    template.required_encoder = Some("nvenc".to_string());
    harness.store.update_template(&template).await.unwrap();

    let input = harness.source_clip("in.mp4", 1.0).await;
    let task = harness.insert_task(&template, &input, &harness.path("out.mp4")).await;
    harness.queue.enqueue(&task.id).await.unwrap();
    let finished = harness.wait_terminal(&task.id, Duration::from_secs(120)).await;

    assert_eq!(finished.status, TaskStatus::Failed);
    let message = finished.error_message.clone().unwrap_or_default();
    assert!(
        message.starts_with("该模板要求使用 NVIDIA NVENC 硬件编码，但本机不可用。"),
        "实际：{message}"
    );
    assert!(message.ends_with("中确认本机支持的编码器。"));
    assert!(!Path::new(&finished.output_path).exists());
}

// ════════════════════════════════════════════════════════════════════════════
// 停机（规格 §3.15）
// ════════════════════════════════════════════════════════════════════════════

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn shutdown_returns_immediately_when_idle() {
    let harness = Harness::new("shutdown-idle").await;
    assert_eq!(harness.queue.active_count(), 0);

    let started = Instant::now();
    harness.queue.shutdown().await;
    assert!(started.elapsed() < Duration::from_secs(2), "空闲停机应当立即返回");

    // 停机后不再接受新任务
    assert!(harness.queue.enqueue("0123456789abcdef01234567").await.is_err());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn shutdown_waits_for_the_running_task_to_finish() {
    let harness = Harness::new("shutdown-wait").await;
    let input = harness.source_clip("in.mp4", 2.0).await;
    let template = harness.software_template("-re").await;
    let task = harness.insert_task(&template, &input, &harness.path("out.mp4")).await;

    harness.queue.enqueue(&task.id).await.unwrap();
    assert!(harness.wait_until(Duration::from_secs(30), || harness.queue.active_count() == 1).await);

    let started = Instant::now();
    harness.queue.shutdown().await;
    let elapsed = started.elapsed();
    assert!(elapsed < Duration::from_secs(10), "停机等待时间应受 10 秒上限约束：{elapsed:?}");

    // 在跑的任务被允许自然结束（不被强杀）
    let finished = harness.task(&task.id).await;
    assert_eq!(finished.status, TaskStatus::Completed, "{:?}", finished.error_message);
}

impl Harness {
    /// 等事件收集器追上，再返回进度事件（终态那一次进度一定会发）
    async fn settled_progress(&self, task_id: &str) -> Vec<f64> {
        let _ = self
            .wait_until(Duration::from_secs(5), || {
                self.progress_events(task_id).last() == Some(&100.0)
            })
            .await;
        self.progress_events(task_id)
    }

    /// 等事件收集器追上，再返回日志事件
    async fn settled_logs(&self, task_id: &str) -> Vec<Vec<String>> {
        let _ = self
            .wait_until(Duration::from_secs(5), || !self.log_events(task_id).is_empty())
            .await;
        self.log_events(task_id)
    }

    /// 等收集器追上后统计「转 Running」的事件数
    async fn settled_running_count(&self, task_id: &str, at_least: usize) -> usize {
        let _ = self
            .wait_until(Duration::from_secs(5), || self.running_event_count(task_id) >= at_least)
            .await;
        self.running_event_count(task_id)
    }

    /// 等批次汇总落库（`update_batch_status` 发生在任务终态之后）
    async fn wait_for_batch(&self, batch_id: &str, timeout: Duration) -> BatchTask {
        let deadline = Instant::now() + timeout;
        loop {
            let batch = self.store.get_batch(batch_id).await.expect("读批次").expect("批次存在");
            if batch.status.is_terminal() || Instant::now() >= deadline {
                return batch;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }
}
