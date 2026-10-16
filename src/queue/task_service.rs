//! 任务与批次服务（对应参考实现 `Services/TaskService.cs`）。
//!
//! 职责边界：
//! - **本文件**：任务的增删改查、批次聚合、历史维护、孤儿恢复、重试；
//! - `queue.rs`（`ConversionQueue`）：真正执行 ffmpeg 的后台 worker；
//! - `preview.rs`：`POST /api/tasks/preview` 的全部推断逻辑。
//!
//! 行为规格：`docs/reference/backend-data.md` §2.3（`OutputPathResolved`/`OverwriteExisting`）、
//! §3.2（入队）、§3.11（批次状态维护）、§3.12（重试）、§3.13（孤儿恢复）、
//! §4（输出路径），接口见 `docs/api-contract.md` §1.5–§1.7。

use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, Result, anyhow};
use chrono::{Duration as ChronoDuration, Local, Utc};

use super::ConversionQueue;
use super::preview::{self, PreviewRequest, PreviewResponse};
use crate::ffmpeg::hardware::HardwareDetector;
use crate::ffmpeg::service::FFmpegService;
use crate::files::get_file_size;
use crate::models::enums::TaskStatus;
use crate::models::{BatchTask, ConversionTask};
use crate::paths;
use crate::services::SettingsService;
use crate::store::Store;
use crate::templates::TemplateService;

/// 启动时对上次异常退出留下的 `Running` 任务写入的失败原因（§3.13 逐字）
pub const ORPHANED_TASK_MESSAGE: &str = "应用退出导致任务中断";

/// 部分输出的清理阈值（§3.10）：小于 64 MiB 才认为「明显是残片」并删除
pub const PARTIAL_OUTPUT_MAX_BYTES: u64 = 64 * 1024 * 1024;

// ────────────────────────────────────────────────────────────────────────────
// 请求 / 响应
// ────────────────────────────────────────────────────────────────────────────

/// `POST /api/tasks` 请求体
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateTaskRequest {
    pub input_path: String,
    pub output_path: String,
    pub template_id: String,
    #[serde(default)]
    pub parameter_values: std::collections::HashMap<String, String>,
    /// 传入 `Some` 时 → `output_path_resolved = true`（单文件转换路径总会传）
    #[serde(default)]
    pub overwrite_existing: Option<bool>,
    #[serde(default)]
    pub start_immediately: bool,
}

/// `POST /api/tasks` → 201
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateTaskResponse {
    pub task: ConversionTask,
    pub enqueued: bool,
}

/// `POST /api/tasks/delete` 结果
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeleteTasksResult {
    pub deleted: usize,
    pub skipped_running: usize,
}

/// `POST /api/batches` 请求体
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateBatchRequest {
    pub input_paths: Vec<String>,
    pub output_directory: String,
    pub template_id: String,
    #[serde(default)]
    pub parameter_values: std::collections::HashMap<String, String>,
    #[serde(default)]
    pub start_immediately: bool,
}

/// `POST /api/batches` 响应
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateBatchResponse {
    pub batch: BatchTask,
    pub tasks: Vec<ConversionTask>,
}

// ────────────────────────────────────────────────────────────────────────────
// TaskService
// ────────────────────────────────────────────────────────────────────────────

/// 任务/批次的统一入口（`Clone` 廉价：内部全是 `Arc`）
#[derive(Clone)]
pub struct TaskService {
    store: Arc<Store>,
    queue: Arc<ConversionQueue>,
    settings: Arc<SettingsService>,
    templates: Arc<TemplateService>,
    ffmpeg: Arc<FFmpegService>,
    hardware: Arc<HardwareDetector>,
}

impl TaskService {
    /// 构造（依赖全部由 `AppState` 注入，生命周期与进程一致）
    pub fn new(
        store: Arc<Store>,
        queue: Arc<ConversionQueue>,
        settings: Arc<SettingsService>,
        templates: Arc<TemplateService>,
        ffmpeg: Arc<FFmpegService>,
        hardware: Arc<HardwareDetector>,
    ) -> Self {
        Self { store, queue, settings, templates, ffmpeg, hardware }
    }

    /// 供 API 层读取队列计数（`GET /api/system/queue`）
    pub fn queue(&self) -> &Arc<ConversionQueue> {
        &self.queue
    }

    // ─────────────────────────── 单个任务 ───────────────────────────

    /// 创建任务（§2.3 的 `OutputPathResolved` / `OverwriteExisting` 组合语义）。
    ///
    /// - 单文件流程：调用方总会传 `overwrite_existing` → `output_path_resolved = true`，
    ///   执行层不再按全局策略改写路径；
    /// - 批量流程：传 `None` → `output_path_resolved = false`、`overwrite_existing = true`，
    ///   执行层在开跑前重新按 `fileExistsAction` 解析。
    pub async fn create_task(&self, request: CreateTaskRequest) -> Result<CreateTaskResponse> {
        let input_path = paths::to_absolute(&request.input_path).to_string_lossy().to_string();
        let template = self.load_template(&request.template_id).await?;

        let mut task = ConversionTask {
            input_path: input_path.clone(),
            output_path: request.output_path.trim().to_string(),
            template_id: request.template_id.clone(),
            // 模板名快照：模板被删后任务列表仍能展示
            template_name: template.name.clone(),
            template_parameters: Some(request.parameter_values.clone()),
            // 文件大小取磁盘真实值（参考实现同样在创建时读一次）
            input_file_size: get_file_size(&input_path),
            ..ConversionTask::default()
        };

        match request.overwrite_existing {
            Some(overwrite) => {
                task.output_path_resolved = true;
                task.overwrite_existing = overwrite;
            }
            None => {
                task.output_path_resolved = false;
                task.overwrite_existing = true;
            }
        }

        self.store.insert_task(&task).await.context("写入任务失败")?;

        // 只在这里入队一次；`start_task` 是给「稍后再开始」的任务用的另一个入口
        let enqueued = if request.start_immediately {
            self.enqueue_task(&task.id).await
        } else {
            false
        };

        Ok(CreateTaskResponse { task, enqueued })
    }

    /// 开始/重新开始任务（§3.2）：重置进度与日志 → 落库 → 入队。
    ///
    /// 已处于终态（完成/失败/取消/跳过）→ `Err`（API 层映射成 409）。
    pub async fn start_task(&self, id: &str) -> Result<ConversionTask> {
        let mut task = self
            .store
            .get_task(id)
            .await?
            .ok_or_else(|| anyhow!("任务不存在或已被删除"))?;

        if task.status.is_terminal() {
            return Err(anyhow!(
                "任务已处于终态（{}），无法重新开始；请使用「重试」创建一个新任务。",
                task.status.display_name()
            ));
        }
        if task.status == TaskStatus::Running {
            // 绝不重复入队（architecture.md §3 第 5 条：同一任务只会被执行一次）
            return Err(anyhow!("任务正在执行中，无法重复开始。"));
        }

        // §3.2 第 1 步：把任务重置为「等待中」再入队
        task.status = TaskStatus::Pending;
        task.progress = 0.0;
        task.current_time = 0.0;
        task.estimated_time_remaining = None;
        task.processing_speed = None;
        task.error_message = None;
        task.completed_at = None;
        task.log_output.clear();
        self.store.update_task(&task).await.context("重置任务失败")?;

        self.enqueue_task(&task.id).await;
        Ok(task)
    }

    /// 取消任务（§3.9）：`Pending` 直接置「已取消」；`Running` 由队列处理
    /// （`graceful = true` 写 `q` 并等 5 秒，false 立即杀进程树）。
    ///
    /// 返回队列是否真的处理了这次取消（终态任务 → `false`）。
    pub async fn cancel_task(&self, id: &str, graceful: bool) -> Result<bool> {
        let Some(task) = self.store.get_task(id).await? else {
            return Ok(false);
        };
        self.cancel_one(&task, graceful).await
    }

    /// 重试（§3.12）：**新建**一条任务 + 入队，原任务保持不动。原任务不存在 → `None`。
    pub async fn retry_task(&self, id: &str) -> Result<Option<ConversionTask>> {
        Ok(self.retry_inner(id).await?.map(|(task, _enqueued)| task))
    }

    /// 按 id 读任务（含 `logOutput`）
    pub async fn get_task(&self, id: &str) -> Result<Option<ConversionTask>> {
        self.store.get_task(id).await
    }

    /// 任务列表（`(items, total)`）。
    ///
    /// PORT NOTE：`Store::list_tasks_lite` 没有 `OFFSET`/`COUNT` 变体，这里把该筛选条件下的
    /// 全部行取回（不含 `log_output`，列表页不需要它）后在内存里切片。本地单用户工具，
    /// 行数在可接受范围内；换成 SQL 分页需要改 `store`（本文件不允许改）。
    pub async fn list_tasks(
        &self,
        limit: i64,
        offset: i64,
        status: Option<TaskStatus>,
    ) -> Result<(Vec<ConversionTask>, i64)> {
        let all = self.store.list_tasks_lite(i64::MAX, status).await?;
        let total = all.len() as i64;
        let items = paginate(all, limit, offset);
        Ok((items, total))
    }

    /// 某批次的子任务（按 `created_at` 正序）
    pub async fn list_tasks_by_batch(&self, batch_id: &str) -> Result<Vec<ConversionTask>> {
        self.store.list_tasks_by_batch(batch_id).await
    }

    /// 运行中的任务（按 `created_at` 倒序，与 `GET /api/tasks/running` 一致）
    pub async fn running_tasks(&self) -> Result<Vec<ConversionTask>> {
        let mut tasks = self.store.list_tasks_by_status(TaskStatus::Running).await?;
        tasks.reverse();
        Ok(tasks)
    }

    /// 等待中的任务（按 `created_at` 正序）
    pub async fn pending_tasks(&self) -> Result<Vec<ConversionTask>> {
        self.store.list_tasks_by_status(TaskStatus::Pending).await
    }

    /// 任务日志（`GET /api/tasks/{id}/log?tailLines=`）；任务不存在 → `None`。
    pub async fn task_log(&self, id: &str, tail_lines: Option<usize>) -> Result<Option<String>> {
        let Some(task) = self.store.get_task(id).await? else {
            return Ok(None);
        };
        let log = match tail_lines {
            Some(lines) if lines > 0 => tail_text(&task.log_output, lines),
            _ => task.log_output,
        };
        Ok(Some(log))
    }

    /// 按 id 批量删除；**运行中的任务不删**，计入 `skipped_running`。
    pub async fn delete_tasks(&self, ids: &[String]) -> Result<DeleteTasksResult> {
        let mut deletable: Vec<String> = Vec::with_capacity(ids.len());
        let mut skipped_running = 0usize;

        for id in ids {
            match self.store.get_task(id).await? {
                // 不存在的 id 静默跳过（不影响其它 id 的删除）
                None => continue,
                Some(task) if task.status == TaskStatus::Running => skipped_running += 1,
                Some(_) => deletable.push(id.clone()),
            }
        }

        let deleted = self.store.delete_tasks(&deletable).await?;
        Ok(DeleteTasksResult { deleted, skipped_running })
    }

    /// 按状态批量删除（`DELETE /api/tasks?status=Failed`）
    pub async fn delete_tasks_by_status(&self, status: TaskStatus) -> Result<usize> {
        self.store.delete_tasks_by_status(status).await
    }

    // ─────────────────────────── 批次 ───────────────────────────

    /// 创建批次（§2.4 + §4.8 批量命名规则）。
    ///
    /// 子任务走与单文件完全相同的 `create_task` 路径，只有一处不同：
    /// `overwrite_existing = None`（执行层按全局策略解析路径）。
    pub async fn create_batch(&self, request: CreateBatchRequest) -> Result<CreateBatchResponse> {
        if request.output_directory.trim().is_empty() {
            // PORT NOTE：参考实现的 `Path.Combine("")` 会把输出落到**进程 CWD**（§4.8 标注的隐患），
            // 这里按预览端点同一句话直接报错，避免批量任务悄悄写到工作目录里。
            return Err(anyhow!(preview::OUTPUT_PATH_UNDETERMINED_MESSAGE));
        }

        let template = self.load_template(&request.template_id).await?;
        let settings = self.settings.get().await;

        // §4.8：扩展名取模板 outputExtension（空 → mp4）；后缀只在「命名规则 = 添加后缀」时拼
        let extension = normalize_extension(&template.output_extension);
        let suffix = if settings.output_naming == crate::models::enums::OutputNamingRule::Suffix {
            Some(settings.output_suffix.as_str())
        } else {
            None
        };

        // 批次名固定为 `批量任务 - {yyyy-MM-dd HH:mm}`（**本地时间**）
        let mut batch = BatchTask {
            name: format!("批量任务 - {}", Local::now().format("%Y-%m-%d %H:%M")),
            template_id: request.template_id.clone(),
            total_files: request.input_paths.len() as i32,
            status: TaskStatus::Pending,
            ..BatchTask::default()
        };
        self.store.insert_batch(&batch).await.context("写入批次失败")?;

        let mut tasks: Vec<ConversionTask> = Vec::with_capacity(request.input_paths.len());
        for input in &request.input_paths {
            let output_path =
                crate::files::generate_output_path(input, &request.output_directory, &extension, suffix);
            let created = self
                .create_task(CreateTaskRequest {
                    input_path: input.clone(),
                    output_path,
                    template_id: request.template_id.clone(),
                    parameter_values: request.parameter_values.clone(),
                    // 批量：不在界面做冲突解析，交给执行层（§2.3）
                    overwrite_existing: None,
                    start_immediately: false,
                })
                .await
                .with_context(|| format!("创建批次子任务失败：{input}"))?;

            // 子任务归属批次（先插库再回填，因为 `create_task` 的契约里没有 batchId）
            let mut task = created.task;
            task.batch_id = Some(batch.id.clone());
            self.store.update_task(&task).await.context("回填批次 id 失败")?;
            tasks.push(task);
        }

        batch.total_files = tasks.len() as i32;
        self.store.update_batch(&batch).await.context("更新批次失败")?;

        if request.start_immediately {
            for task in &tasks {
                self.enqueue_task(&task.id).await;
            }
        }

        Ok(CreateBatchResponse { batch, tasks })
    }

    /// 批次列表（`(items, total)`；内存分页，理由同 `list_tasks`）
    pub async fn list_batches(&self, limit: i64, offset: i64) -> Result<(Vec<BatchTask>, i64)> {
        let all = self.store.list_batches(i64::MAX).await?;
        let total = all.len() as i64;
        let items = paginate(all, limit, offset);
        Ok((items, total))
    }

    /// 读单个批次
    pub async fn get_batch(&self, id: &str) -> Result<Option<BatchTask>> {
        self.store.get_batch(id).await
    }

    /// 取消批次内所有**未进入终态**的子任务（运行中的用非优雅方式，§3.11）；返回取消数量。
    pub async fn cancel_batch(&self, id: &str) -> Result<usize> {
        let tasks = self.store.list_tasks_by_batch(id).await?;
        let mut cancelled = 0usize;
        for task in tasks {
            if task.status.is_terminal() {
                continue;
            }
            // 「取消全部」语义：运行中的任务立即停，不等它写完文件尾（§3.11）
            let graceful = false;
            if self.cancel_one(&task, graceful).await.unwrap_or(false) {
                cancelled += 1;
            }
        }
        Ok(cancelled)
    }

    /// 重试批次内所有 `Failed` 子任务 → `(retried, queued)`
    pub async fn retry_failed_in_batch(&self, id: &str) -> Result<(usize, usize)> {
        let tasks = self.store.list_tasks_by_batch(id).await?;
        let mut retried = 0usize;
        let mut queued = 0usize;

        for task in tasks {
            if task.status != TaskStatus::Failed {
                continue;
            }
            if let Some((_new_task, enqueued)) = self.retry_inner(&task.id).await? {
                retried += 1;
                if enqueued {
                    queued += 1;
                }
            }
        }
        Ok((retried, queued))
    }

    /// 删除批次（连同其子任务）；返回被删掉的子任务数量。
    ///
    /// PORT NOTE：`Store::delete_batch` 在一条事务里同时删除子任务与批次记录，
    /// 因此这里先把子任务数读出来当返回值（契约 `{"deletedTasks":10}`）。
    /// 参考实现的行为相同（界面只在批次结束后提供这个入口）。
    pub async fn delete_batch(&self, id: &str) -> Result<usize> {
        let tasks = self.store.list_tasks_by_batch(id).await?;
        self.store.delete_batch(id).await?;
        Ok(tasks.len())
    }

    // ─────────────────────────── 历史维护 ───────────────────────────

    /// 清理历史：删除 `created_at < now - retention_days` 的任务，以及**已经没有子任务**的批次。
    ///
    /// `retention_days <= 0` → 什么都不做（设置里的 `0 = 不清理`）。
    pub async fn cleanup_history(&self, retention_days: i32) -> Result<(usize, usize)> {
        if retention_days <= 0 {
            return Ok((0, 0));
        }

        let cutoff = Utc::now() - ChronoDuration::days(retention_days as i64);

        // PORT NOTE：`Store::delete_tasks_before` 不带状态条件，会把「创建时间很旧、但仍在运行」的任务
        // 也删掉（行被删了进程还在跑）。这里显式排除 `Running` —— 运行中的任务不属于「历史」。
        // 崩溃残留的 `Running` 任务会先被 `recover_orphaned_tasks` 置为 `Failed`，随后自然进入清理范围。
        let all = self.store.list_tasks_lite(i64::MAX, None).await?;
        let stale: Vec<String> = all
            .into_iter()
            .filter(|task| task.status != TaskStatus::Running && task.created_at < cutoff)
            .map(|task| task.id)
            .collect();
        let deleted_tasks = self.store.delete_tasks(&stale).await?;

        // 批次只在「一个子任务都不剩」时删除：这样刚创建、还没跑完的批次不会被误删
        let mut deleted_batches = 0usize;
        for batch in self.store.list_batches(i64::MAX).await? {
            if self.store.list_tasks_by_batch(&batch.id).await?.is_empty() {
                if self.store.delete_batch(&batch.id).await? > 0 {
                    deleted_batches += 1;
                }
            }
        }

        Ok((deleted_tasks, deleted_batches))
    }

    /// 清空历史：删除全部终态任务 + 全部批次记录；返回删除的任务数。
    ///
    /// PORT NOTE：参考实现只删批次记录、不动任务；`Store` 没有「只删批次记录」的方法
    /// （`delete_batch` 会连同子任务一起删），因此除终态任务外，属于这些批次的
    /// **等待中**子任务也会被一起删掉。界面上「清空历史」只在没有进行中的工作时提供，
    /// 实际影响可忽略。
    pub async fn clear_history(&self) -> Result<usize> {
        let mut deleted_tasks = 0usize;
        for status in [
            TaskStatus::Completed,
            TaskStatus::Failed,
            TaskStatus::Cancelled,
            TaskStatus::Skipped,
        ] {
            deleted_tasks += self.store.delete_tasks_by_status(status).await?;
        }

        for batch in self.store.list_batches(i64::MAX).await? {
            self.store.delete_batch(&batch.id).await?;
        }

        Ok(deleted_tasks)
    }

    // ─────────────────────────── 启动恢复 ───────────────────────────

    /// 复位上次异常退出留下的 `Running` 任务（§3.13）：置 `Failed` + 中文原因 + 清理部分输出。
    ///
    /// PORT NOTE：`Pending` 任务**不会**在启动时自动重新入队（与参考实现一致）。
    /// 崩溃后队列的内存内容丢失，这些任务会停在「等待中」，需要用户手动「开始/重试」。
    /// 之所以不自动入队：待执行的任务可能指向已经变化的环境（磁盘空间、模板、输入文件），
    /// 悄悄开跑一批用户没预期的转换比「停在等待中」更容易造成意外。
    pub async fn recover_orphaned_tasks(&self) -> Result<usize> {
        let orphans = self.store.list_tasks_by_status(TaskStatus::Running).await?;
        let mut recovered = 0usize;

        for mut task in orphans {
            task.status = TaskStatus::Failed;
            task.error_message = Some(ORPHANED_TASK_MESSAGE.to_string());
            task.completed_at = Some(Utc::now());
            task.estimated_time_remaining = None;
            task.processing_speed = None;

            if let Err(error) = self.store.update_task(&task).await {
                tracing::warn!("复位中断任务 {} 失败：{error:#}", task.id);
                continue;
            }

            // §3.10：删除明显是残片的小文件（大文件保留，避免删掉可能已经完整的输出）
            cleanup_partial_output(&task);
            // 状态是我自己写的，队列不会替这批任务重算批次状态（§3.11）
            self.refresh_batch_status(&task.batch_id).await?;
            recovered += 1;
        }

        if recovered > 0 {
            tracing::info!("已清理 {recovered} 个中断的任务");
        }
        Ok(recovered)
    }

    // ─────────────────────────── 预览 ───────────────────────────

    /// `POST /api/tasks/preview`：命令预览 + 输出路径冲突解析（前端不做任何本地推断）。
    pub async fn preview(&self, request: PreviewRequest) -> Result<PreviewResponse> {
        preview::build_preview(&self.settings, &self.templates, &self.ffmpeg, &self.hardware, &request)
            .await
    }

    // ─────────────────────────── 内部工具 ───────────────────────────

    /// 入队（唯一入口）：保证「任务不会被入队两次」。
    ///
    /// `ConversionQueue::enqueue` 对同一 `task_id` 幂等（排队中/执行中重复入队直接 `Ok(())`），
    /// 只有「队列正在关闭」这类情况才返回 `Err` —— 那时把 `enqueued` 报成 `false`，
    /// 界面会提示「任务已创建但未能立即调度，请到任务列表查看」。
    async fn enqueue_task(&self, id: &str) -> bool {
        match self.queue.enqueue(id).await {
            Ok(()) => true,
            Err(error) => {
                tracing::warn!("任务入队失败：{id}（{error:#}）");
                false
            }
        }
    }

    /// 取消单个任务：先交给队列；队列「不认识」它时（尚未入队、或已从内存队列清理）
    /// 按 §3.9 的 `Pending` 分支自己落库。
    async fn cancel_one(&self, task: &ConversionTask, graceful: bool) -> Result<bool> {
        if task.status.is_terminal() {
            return Ok(false);
        }
        if self.queue.cancel(&task.id, graceful).await? {
            return Ok(true);
        }

        // PORT NOTE：`ConversionQueue::cancel` 只处理**它内存里知道**的任务（排队中 / 正在执行）。
        // 批量创建时子任务默认不入队（`startImmediately = false`），这类任务在库里是 `Pending`，
        // 队列会返回 `false`；若照抄「false 就什么都不做」，「取消全部」在这些任务上会静默失效。
        // 这里补齐 §3.9 的 Pending 语义：直接置 `Cancelled` + `用户取消`，并同步批次状态。
        let Some(mut current) = self.store.get_task(&task.id).await? else {
            return Ok(false);
        };
        if current.status != TaskStatus::Pending {
            // 正在执行的任务必然登记在队列里；走到这里说明状态刚被 worker 改过，交给 worker 收尾
            return Ok(false);
        }

        current.status = TaskStatus::Cancelled;
        current.error_message = Some("用户取消".to_string());
        current.completed_at = Some(Utc::now());
        current.estimated_time_remaining = None;
        current.processing_speed = None;
        self.store.update_task(&current).await.context("取消任务失败")?;
        self.refresh_batch_status(&current.batch_id).await?;
        Ok(true)
    }

    /// 批次状态维护（§3.11）：每次都从子任务重算，因此与队列里的同一套逻辑并存也安全（幂等）。
    async fn refresh_batch_status(&self, batch_id: &Option<String>) -> Result<()> {
        let Some(batch_id) = batch_id.as_deref().map(str::trim).filter(|id| !id.is_empty()) else {
            return Ok(());
        };
        let Some(mut batch) = self.store.get_batch(batch_id).await? else {
            return Ok(());
        };
        let tasks = self.store.list_tasks_by_batch(batch_id).await?;
        if tasks.is_empty() {
            return Ok(());
        }

        batch.total_files = tasks.len() as i32;
        batch.completed_files =
            tasks.iter().filter(|task| task.status == TaskStatus::Completed).count() as i32;
        // `Skipped` 既不计入完成也不计入失败（§3.11 的既定行为，移植时保留）
        batch.failed_files = tasks
            .iter()
            .filter(|task| matches!(task.status, TaskStatus::Failed | TaskStatus::Cancelled))
            .count() as i32;

        if tasks.iter().all(|task| task.status.is_terminal()) {
            batch.status =
                if batch.failed_files == 0 { TaskStatus::Completed } else { TaskStatus::Failed };
            // `CompletedAt` 只写一次（参考实现的 `??=`）
            if batch.completed_at.is_none() {
                batch.completed_at = Some(Utc::now());
            }
        } else if tasks.iter().any(|task| task.status == TaskStatus::Running) {
            batch.status = TaskStatus::Running;
        }

        self.store.update_batch(&batch).await.context("更新批次状态失败")?;
        Ok(())
    }

    /// 重试的实现体：返回 `(新任务, 是否成功入队)`
    async fn retry_inner(&self, id: &str) -> Result<Option<(ConversionTask, bool)>> {
        let Some(original) = self.store.get_task(id).await? else {
            return Ok(None);
        };

        let mut task = ConversionTask {
            input_path: original.input_path.clone(),
            // `OutputPath` 沿用旧值，但 `output_path_resolved` 不继承：
            // 重试时按当前 `fileExistsAction` 重新解析（§3.12）
            output_path: original.output_path.clone(),
            template_id: original.template_id.clone(),
            template_name: original.template_name.clone(),
            template_parameters: original.template_parameters.clone(),
            total_duration: original.total_duration,
            input_file_size: original.input_file_size,
            batch_id: original.batch_id.clone(),
            retry_of_task_id: Some(original.id.clone()),
            ..ConversionTask::default()
        };
        task.output_path_resolved = false;
        task.overwrite_existing = false;

        self.store.insert_task(&task).await.context("写入重试任务失败")?;
        let enqueued = self.enqueue_task(&task.id).await;
        Ok(Some((task, enqueued)))
    }

    /// 读取模板；缺失 → 契约里的中文错误
    async fn load_template(&self, id: &str) -> Result<crate::models::CommandTemplate> {
        match self.templates.get(id).await {
            Ok(Some(template)) => Ok(template),
            Ok(None) => Err(anyhow!(preview::TEMPLATE_MISSING_MESSAGE)),
            Err(error) => Err(error.context(format!("读取模板失败：{id}"))),
        }
    }
}

// ────────────────────────────────────────────────────────────────────────────
// 纯函数工具
// ────────────────────────────────────────────────────────────────────────────

/// 内存分页：`limit <= 0` 视为不限制
fn paginate<T>(items: Vec<T>, limit: i64, offset: i64) -> Vec<T> {
    let skip = offset.max(0) as usize;
    let take = if limit <= 0 { usize::MAX } else { limit as usize };
    items.into_iter().skip(skip).take(take).collect()
}

/// 取日志的最后 `count` 行（行数不足时原样返回）
fn tail_text(text: &str, count: usize) -> String {
    if count == 0 || text.is_empty() {
        return String::new();
    }
    let lines: Vec<&str> = text.lines().collect();
    if lines.len() <= count {
        return text.to_string();
    }
    lines[lines.len() - count..].join("\n")
}

/// 归一化输出扩展名（去前导点、小写；空 → `mp4`）
fn normalize_extension(extension: &str) -> String {
    let trimmed = extension.trim().trim_start_matches('.').to_lowercase();
    if trimmed.is_empty() {
        "mp4".to_string()
    } else {
        trimmed
    }
}

/// §3.10 部分输出清理：只删「存在且小于 64 MiB」的输出文件，且绝不碰输入文件
fn cleanup_partial_output(task: &ConversionTask) {
    if task.output_path.trim().is_empty() {
        return;
    }
    // 输出 == 输入是硬禁忌（§4.1 第 2 条）：宁可留着残片，也不能删源文件
    if paths::paths_equal(&task.output_path, &task.input_path) {
        return;
    }

    let path = Path::new(&task.output_path);
    let Ok(metadata) = std::fs::metadata(path) else {
        return;
    };
    if !metadata.is_file() || metadata.len() >= PARTIAL_OUTPUT_MAX_BYTES {
        return;
    }
    // 删除失败静默忽略（参考实现同样不报错）
    let _ = std::fs::remove_file(path);
}

// ────────────────────────────────────────────────────────────────────────────
// 测试
// ────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ffmpeg::encoder_catalog::EncoderType;
    use crate::models::enums::{FileExistsAction, HardwareAccelerationMode, OutputNamingRule};
    use crate::models::CommandTemplate;
    use std::collections::HashMap;
    use std::path::PathBuf;
    use std::process::Command;
    use std::time::Duration;

    // ── 测试脚手架 ──────────────────────────────────────────────

    /// 一次测试用的全套服务（真实 ffmpeg / 真实 SQLite 内存库 / 真实队列）
    struct Harness {
        store: Arc<Store>,
        settings: Arc<SettingsService>,
        templates: Arc<TemplateService>,
        ffmpeg: Arc<FFmpegService>,
        hardware: Arc<HardwareDetector>,
        queue: Arc<ConversionQueue>,
        service: TaskService,
        /// 每次测试独立的临时目录（输入素材、输出目录都在里面）
        dir: PathBuf,
    }

    impl Harness {
        async fn new(tag: &str) -> Self {
            let store = Arc::new(Store::open_in_memory().await.expect("打开内存库"));
            let settings = Arc::new(SettingsService::new(store.clone()));
            let app_settings = settings.get().await;
            let ffmpeg = Arc::new(FFmpegService::from_settings(&app_settings));
            let hardware = Arc::new(HardwareDetector::new(ffmpeg.clone()));
            hardware.apply_settings(&app_settings);
            let templates = Arc::new(TemplateService::new(store.clone(), ffmpeg.clone(), hardware.clone()));
            let queue = Arc::new(ConversionQueue::new(
                store.clone(),
                settings.clone(),
                templates.clone(),
                ffmpeg.clone(),
                hardware.clone(),
            ));
            queue.start();
            let service = TaskService::new(
                store.clone(),
                queue.clone(),
                settings.clone(),
                templates.clone(),
                ffmpeg.clone(),
                hardware.clone(),
            );

            let dir = temp_dir(tag);
            Self { store, settings, templates, ffmpeg, hardware, queue, service, dir }
        }

        /// 插入一个纯软件模板（命令里没有 `{encoder}` → 不做任何编码器注入）
        async fn software_template(&self) -> CommandTemplate {
            self.insert_template(CommandTemplate {
                name: format!("测试软件模板 {}", crate::models::new_id()),
                command_args: "-i {input} -c:v libx264 -preset ultrafast -an {output}".to_string(),
                output_extension: "mp4".to_string(),
                target_codec: "h264".to_string(),
                hardware_mode: HardwareAccelerationMode::None,
                ..CommandTemplate::default()
            })
            .await
        }

        /// 插入一个使用 `{encoder}` + `{quality_args}` 的模板（硬件自动模式）
        async fn hardware_template(&self) -> CommandTemplate {
            self.insert_template(CommandTemplate {
                name: format!("测试硬件模板 {}", crate::models::new_id()),
                command_args:
                    "-i {input} -c:v {encoder} {quality_args} {preset_args} -an {output}".to_string(),
                output_extension: "mp4".to_string(),
                target_codec: "h264".to_string(),
                hardware_mode: HardwareAccelerationMode::Auto,
                ..CommandTemplate::default()
            })
            .await
        }

        /// 插入一个「必须用 NVENC」的严格模板（本机（macOS）必然不可用）
        async fn strict_nvenc_template(&self) -> CommandTemplate {
            self.insert_template(CommandTemplate {
                name: format!("测试严格 NVENC 模板 {}", crate::models::new_id()),
                command_args: "-i {input} -c:v {encoder} {quality_args} {output}".to_string(),
                output_extension: "mp4".to_string(),
                target_codec: "h264".to_string(),
                hardware_mode: HardwareAccelerationMode::Required,
                required_encoder: Some("nvenc".to_string()),
                ..CommandTemplate::default()
            })
            .await
        }

        async fn insert_template(&self, template: CommandTemplate) -> CommandTemplate {
            self.store.insert_template(&template).await.expect("插入模板");
            let stored = self.templates.get(&template.id).await.expect("读模板").expect("模板存在");
            assert_eq!(stored.name, template.name);
            template
        }

        /// 生成本机可用的测试素材（1 秒 128x72 的 H.264 MP4）
        fn make_input_video(&self, name: &str) -> String {
            let path = self.dir.join(name);
            let output = Command::new(self.ffmpeg.paths().ffmpeg)
                .args([
                    "-hide_banner",
                    "-nostdin",
                    "-y",
                    "-f",
                    "lavfi",
                    "-i",
                    "testsrc=duration=1:size=128x72:rate=10",
                    "-pix_fmt",
                    "yuv420p",
                    "-c:v",
                    "libx264",
                    "-preset",
                    "ultrafast",
                ])
                .arg(&path)
                .output()
                .expect("执行 ffmpeg 生成测试素材");
            assert!(
                output.status.success() && path.is_file(),
                "生成测试素材失败：{}",
                String::from_utf8_lossy(&output.stderr)
            );
            path.to_string_lossy().to_string()
        }

        async fn set_file_exists_action(&self, action: FileExistsAction) {
            let mut settings = self.settings.get().await;
            settings.file_exists_action = action;
            self.settings.save(settings).await.expect("保存设置");
        }

        async fn set_output_naming(&self, naming: OutputNamingRule) {
            let mut settings = self.settings.get().await;
            settings.output_naming = naming;
            self.settings.save(settings).await.expect("保存设置");
        }

        async fn store_task(&self, task: ConversionTask) -> ConversionTask {
            self.store.insert_task(&task).await.expect("插入任务");
            task
        }

        fn preview_request(&self, template_id: &str, input_path: &str) -> PreviewRequest {
            PreviewRequest {
                input_path: input_path.to_string(),
                output_directory: self.dir.to_string_lossy().to_string(),
                output_file_name_template: None,
                output_extension: None,
                template_id: template_id.to_string(),
                parameter_values: HashMap::new(),
                overwrite_existing: false,
                allow_rename: true,
            }
        }
    }

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ffmpeg-web-rust-queue-{tag}-{}", crate::models::new_id()));
        std::fs::create_dir_all(&dir).expect("创建临时目录");
        dir
    }

    /// 轮询到任务进入终态（真实 ffmpeg 跑完）
    async fn wait_terminal(store: &Store, id: &str) -> ConversionTask {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(90);
        loop {
            let task = store.get_task(id).await.expect("读任务").expect("任务存在");
            if task.status.is_terminal() {
                return task;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "任务未在超时时间内结束，当前状态 {:?}",
                task.status
            );
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    // ── create_task ────────────────────────────────────────────

    #[tokio::test]
    async fn create_task_single_file_flow_marks_path_resolved() {
        let h = Harness::new("create-single").await;
        let template = h.software_template().await;
        let input = h.make_input_video("in.mp4");
        let output = h.dir.join("out.mp4").to_string_lossy().to_string();

        let response = h
            .service
            .create_task(CreateTaskRequest {
                input_path: input.clone(),
                output_path: output.clone(),
                template_id: template.id.clone(),
                parameter_values: HashMap::new(),
                overwrite_existing: Some(false),
                start_immediately: false,
            })
            .await
            .expect("创建任务");

        assert!(response.task.output_path_resolved, "单文件流程必须标记已解析");
        assert!(!response.task.overwrite_existing);
        assert!(!response.enqueued);
        assert_eq!(response.task.status, TaskStatus::Pending);
        assert_eq!(response.task.template_name, template.name);
        assert!(response.task.input_file_size > 0, "必须记录输入文件大小");
        assert_eq!(response.task.input_path, paths::to_absolute(&input).to_string_lossy());
    }

    #[tokio::test]
    async fn create_task_batch_flow_leaves_path_unresolved_and_overwrites() {
        let h = Harness::new("create-batch-flow").await;
        let template = h.software_template().await;
        let input = h.make_input_video("in.mp4");

        let response = h
            .service
            .create_task(CreateTaskRequest {
                input_path: input,
                output_path: h.dir.join("out.mp4").to_string_lossy().to_string(),
                template_id: template.id.clone(),
                parameter_values: HashMap::new(),
                overwrite_existing: None,
                start_immediately: false,
            })
            .await
            .expect("创建任务");

        assert!(!response.task.output_path_resolved, "批量流程不标记已解析");
        assert!(response.task.overwrite_existing, "批量流程默认允许覆盖（执行层再按全局策略解析）");
    }

    #[tokio::test]
    async fn create_task_keeps_parameter_values_and_is_pending() {
        let h = Harness::new("create-params").await;
        let template = h.software_template().await;
        let input = h.make_input_video("in.mp4");
        let mut values = HashMap::new();
        values.insert("crf".to_string(), "20".to_string());

        let response = h
            .service
            .create_task(CreateTaskRequest {
                input_path: input,
                output_path: h.dir.join("out.mp4").to_string_lossy().to_string(),
                template_id: template.id.clone(),
                parameter_values: values.clone(),
                overwrite_existing: Some(true),
                start_immediately: false,
            })
            .await
            .expect("创建任务");

        assert_eq!(response.task.template_parameters.clone().unwrap_or_default(), values);
        // 没有立即开始 → 队列里不该有它
        tokio::time::sleep(Duration::from_millis(400)).await;
        let stored = h.store.get_task(&response.task.id).await.unwrap().unwrap();
        assert_eq!(stored.status, TaskStatus::Pending);
    }

    #[tokio::test]
    async fn create_task_with_unknown_template_fails() {
        let h = Harness::new("create-missing-template").await;
        let input = h.make_input_video("in.mp4");
        let error = h
            .service
            .create_task(CreateTaskRequest {
                input_path: input,
                output_path: h.dir.join("out.mp4").to_string_lossy().to_string(),
                template_id: "000000000000000000000000".to_string(),
                parameter_values: HashMap::new(),
                overwrite_existing: Some(true),
                start_immediately: false,
            })
            .await
            .expect_err("模板不存在必须报错");
        assert!(error.to_string().contains(preview::TEMPLATE_MISSING_MESSAGE));
    }

    #[tokio::test]
    async fn create_task_start_immediately_enqueues_and_completes() {
        let h = Harness::new("create-start").await;
        let template = h.software_template().await;
        let input = h.make_input_video("in.mp4");
        let output = h.dir.join("done.mp4").to_string_lossy().to_string();

        let response = h
            .service
            .create_task(CreateTaskRequest {
                input_path: input,
                output_path: output.clone(),
                template_id: template.id.clone(),
                parameter_values: HashMap::new(),
                overwrite_existing: Some(true),
                start_immediately: true,
            })
            .await
            .expect("创建任务");

        assert!(response.enqueued, "start_immediately 必须真的入队");
        let finished = wait_terminal(&h.store, &response.task.id).await;
        assert_eq!(finished.status, TaskStatus::Completed, "错误：{:?}", finished.error_message);
        assert_eq!(finished.progress, 100.0);
        assert!(Path::new(&output).is_file(), "输出文件必须存在");
        assert!(finished.output_file_size.unwrap_or(0) > 0);
        h.queue.shutdown().await;
    }

    #[tokio::test]
    async fn start_task_then_completes_end_to_end() {
        let h = Harness::new("start-e2e").await;
        let template = h.software_template().await;
        let input = h.make_input_video("in.mp4");
        let output = h.dir.join("e2e.mp4").to_string_lossy().to_string();

        let created = h
            .service
            .create_task(CreateTaskRequest {
                input_path: input,
                output_path: output.clone(),
                template_id: template.id.clone(),
                parameter_values: HashMap::new(),
                overwrite_existing: Some(true),
                start_immediately: false,
            })
            .await
            .expect("创建任务");

        let started = h.service.start_task(&created.task.id).await.expect("开始任务");
        assert_eq!(started.status, TaskStatus::Pending);
        assert!(started.log_output.is_empty(), "开始前必须清空日志");

        let finished = wait_terminal(&h.store, &created.task.id).await;
        assert_eq!(finished.status, TaskStatus::Completed, "错误：{:?}", finished.error_message);
        assert!(finished.started_at.is_some() && finished.completed_at.is_some());
        assert!(Path::new(&output).is_file());
        h.queue.shutdown().await;
    }

    #[tokio::test]
    async fn start_task_rejects_terminal_task() {
        let h = Harness::new("start-terminal").await;
        let template = h.software_template().await;
        let input = h.make_input_video("in.mp4");

        let mut task = ConversionTask {
            input_path: input,
            output_path: h.dir.join("x.mp4").to_string_lossy().to_string(),
            template_id: template.id.clone(),
            status: TaskStatus::Completed,
            ..ConversionTask::default()
        };
        h.store.insert_task(&task).await.unwrap();

        let error = h.service.start_task(&task.id).await.expect_err("终态任务不能开始");
        assert!(error.to_string().contains("终态"), "{error}");

        task.status = TaskStatus::Skipped;
        h.store.update_task(&task).await.unwrap();
        assert!(h.service.start_task(&task.id).await.is_err());
    }

    #[tokio::test]
    async fn start_task_rejects_running_task() {
        let h = Harness::new("start-running").await;
        let task = h
            .store_task(ConversionTask {
                status: TaskStatus::Running,
                input_path: h.dir.join("a.mp4").to_string_lossy().to_string(),
                output_path: h.dir.join("b.mp4").to_string_lossy().to_string(),
                ..ConversionTask::default()
            })
            .await;
        let error = h.service.start_task(&task.id).await.expect_err("运行中不能重复开始");
        assert!(error.to_string().contains("正在执行中"), "{error}");
    }

    #[tokio::test]
    async fn start_task_missing_task_is_error() {
        let h = Harness::new("start-missing").await;
        assert!(h.service.start_task("000000000000000000000000").await.is_err());
    }

    // ── retry ──────────────────────────────────────────────────

    #[tokio::test]
    async fn retry_creates_new_task_with_copied_parameters() {
        let h = Harness::new("retry").await;
        let template = h.software_template().await;
        let input = h.make_input_video("in.mp4");
        let output = h.dir.join("retry.mp4").to_string_lossy().to_string();
        let mut values = HashMap::new();
        values.insert("crf".to_string(), "20".to_string());

        let original = ConversionTask {
            input_path: input.clone(),
            output_path: output.clone(),
            template_id: template.id.clone(),
            template_name: template.name.clone(),
            template_parameters: Some(values.clone()),
            status: TaskStatus::Failed,
            progress: 42.0,
            log_output: "旧日志".to_string(),
            error_message: Some("旧错误".to_string()),
            started_at: Some(Utc::now()),
            completed_at: Some(Utc::now()),
            ..ConversionTask::default()
        };
        h.store.insert_task(&original).await.unwrap();

        let retried = h.service.retry_task(&original.id).await.expect("重试").expect("任务存在");
        assert_ne!(retried.id, original.id, "重试必须新建任务");
        assert_eq!(retried.retry_of_task_id.as_deref(), Some(original.id.as_str()));
        assert_eq!(retried.input_path, original.input_path);
        assert_eq!(retried.output_path, original.output_path);
        assert_eq!(retried.template_id, original.template_id);
        assert_eq!(retried.template_name, original.template_name);
        assert_eq!(retried.template_parameters.clone().unwrap_or_default(), values);
        assert_eq!(retried.status, TaskStatus::Pending);
        assert_eq!(retried.progress, 0.0);
        assert!(retried.log_output.is_empty());
        assert!(retried.error_message.is_none());
        assert!(retried.started_at.is_none() && retried.completed_at.is_none());
        assert!(!retried.output_path_resolved, "重试不继承路径解析结果");
        assert!(!retried.overwrite_existing);

        // 入队 → 新任务会被 worker 捡起来（真实输入 → 最终完成）
        let finished = wait_terminal(&h.store, &retried.id).await;
        assert_eq!(finished.status, TaskStatus::Completed, "错误：{:?}", finished.error_message);
        h.queue.shutdown().await;
    }

    #[tokio::test]
    async fn retry_missing_task_returns_none() {
        let h = Harness::new("retry-missing").await;
        assert!(h.service.retry_task("000000000000000000000000").await.unwrap().is_none());
    }

    // ── delete ─────────────────────────────────────────────────

    #[tokio::test]
    async fn delete_tasks_skips_running() {
        let h = Harness::new("delete-running").await;
        let running = h
            .store_task(ConversionTask { status: TaskStatus::Running, ..ConversionTask::default() })
            .await;
        let pending = h
            .store_task(ConversionTask { status: TaskStatus::Pending, ..ConversionTask::default() })
            .await;

        let result = h
            .service
            .delete_tasks(&[running.id.clone(), pending.id.clone(), "000000000000000000000000".to_string()])
            .await
            .expect("删除任务");

        assert_eq!(result.deleted, 1);
        assert_eq!(result.skipped_running, 1);
        assert!(h.store.get_task(&running.id).await.unwrap().is_some(), "运行中的任务必须保留");
        assert!(h.store.get_task(&pending.id).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn delete_tasks_by_status_only_touches_that_status() {
        let h = Harness::new("delete-status").await;
        h.store_task(ConversionTask { status: TaskStatus::Failed, ..ConversionTask::default() }).await;
        h.store_task(ConversionTask { status: TaskStatus::Failed, ..ConversionTask::default() }).await;
        h.store_task(ConversionTask { status: TaskStatus::Completed, ..ConversionTask::default() }).await;

        assert_eq!(h.service.delete_tasks_by_status(TaskStatus::Failed).await.unwrap(), 2);
        assert_eq!(h.store.list_tasks_by_status(TaskStatus::Failed).await.unwrap().len(), 0);
        assert_eq!(h.store.list_tasks_by_status(TaskStatus::Completed).await.unwrap().len(), 1);
    }

    // ── 查询 ───────────────────────────────────────────────────

    #[tokio::test]
    async fn list_tasks_paginate_and_count() {
        let h = Harness::new("list-tasks").await;
        for index in 0..5 {
            let mut task = ConversionTask {
                status: if index % 2 == 0 { TaskStatus::Completed } else { TaskStatus::Failed },
                ..ConversionTask::default()
            };
            // created_at 递增，保证倒序稳定
            task.created_at = Utc::now() + ChronoDuration::milliseconds(index);
            h.store.insert_task(&task).await.unwrap();
        }

        let (page, total) = h.service.list_tasks(2, 0, None).await.unwrap();
        assert_eq!(total, 5);
        assert_eq!(page.len(), 2);
        assert!(page[0].created_at > page[1].created_at, "必须按 createdAt 倒序");

        let (second, _) = h.service.list_tasks(2, 2, None).await.unwrap();
        assert_eq!(second.len(), 2);
        assert_ne!(page[0].id, second[0].id);

        let (only_failed, total_failed) = h.service.list_tasks(10, 0, Some(TaskStatus::Failed)).await.unwrap();
        assert_eq!(total_failed, 2);
        assert!(only_failed.iter().all(|task| task.status == TaskStatus::Failed));
    }

    #[tokio::test]
    async fn running_and_pending_lists_are_ordered() {
        let h = Harness::new("lists").await;
        let mut older = ConversionTask { status: TaskStatus::Running, ..ConversionTask::default() };
        older.created_at = Utc::now() - ChronoDuration::minutes(5);
        let mut newer = ConversionTask { status: TaskStatus::Running, ..ConversionTask::default() };
        newer.created_at = Utc::now();
        h.store.insert_task(&older).await.unwrap();
        h.store.insert_task(&newer).await.unwrap();
        h.store_task(ConversionTask { status: TaskStatus::Pending, ..ConversionTask::default() }).await;

        let running = h.service.running_tasks().await.unwrap();
        assert_eq!(running.len(), 2);
        assert_eq!(running[0].id, newer.id, "运行中列表按 createdAt 倒序");
        assert_eq!(h.service.pending_tasks().await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn task_log_supports_tail_lines() {
        let h = Harness::new("task-log").await;
        let task = h
            .store_task(ConversionTask {
                log_output: "l1\nl2\nl3\nl4".to_string(),
                ..ConversionTask::default()
            })
            .await;

        assert_eq!(h.service.task_log(&task.id, None).await.unwrap().unwrap(), "l1\nl2\nl3\nl4");
        assert_eq!(h.service.task_log(&task.id, Some(2)).await.unwrap().unwrap(), "l3\nl4");
        assert_eq!(h.service.task_log(&task.id, Some(99)).await.unwrap().unwrap(), "l1\nl2\nl3\nl4");
        assert!(h.service.task_log("000000000000000000000000", Some(2)).await.unwrap().is_none());
    }

    // ── 批次 ───────────────────────────────────────────────────

    #[tokio::test]
    async fn create_batch_makes_children_with_documented_naming() {
        let h = Harness::new("batch-create").await;
        let template = h.software_template().await;
        let first = h.make_input_video("one.mp4");
        let second = h.make_input_video("two.mp4");

        let response = h
            .service
            .create_batch(CreateBatchRequest {
                input_paths: vec![first.clone(), second.clone()],
                output_directory: h.dir.to_string_lossy().to_string(),
                template_id: template.id.clone(),
                parameter_values: HashMap::new(),
                start_immediately: false,
            })
            .await
            .expect("创建批次");

        assert_eq!(response.tasks.len(), 2);
        assert_eq!(response.batch.total_files, 2);
        assert_eq!(response.batch.status, TaskStatus::Pending);
        assert!(response.batch.name.starts_with("批量任务 - "), "{}", response.batch.name);
        let stamp = response.batch.name.trim_start_matches("批量任务 - ");
        assert_eq!(stamp.len(), "2026-10-16 15:20".len(), "批次名时间格式必须是 yyyy-MM-dd HH:mm");
        assert!(stamp.contains('-') && stamp.contains(':'));

        for task in &response.tasks {
            assert_eq!(task.batch_id.as_deref(), Some(response.batch.id.as_str()));
            assert!(!task.output_path_resolved, "批次子任务交给执行层解析");
            assert!(task.overwrite_existing);
            assert!(task.output_path.ends_with("_converted.mp4"), "{}", task.output_path);
        }
        assert!(response.tasks[0].output_path.contains("one_converted.mp4"));
        assert!(response.tasks[1].output_path.contains("two_converted.mp4"));

        let stored = h.service.list_tasks_by_batch(&response.batch.id).await.unwrap();
        assert_eq!(stored.len(), 2);
    }

    #[tokio::test]
    async fn create_batch_original_naming_has_no_suffix() {
        let h = Harness::new("batch-naming-original").await;
        h.set_output_naming(OutputNamingRule::Original).await;
        let template = h.software_template().await;
        let input = h.make_input_video("plain.mp4");

        let response = h
            .service
            .create_batch(CreateBatchRequest {
                input_paths: vec![input],
                output_directory: h.dir.to_string_lossy().to_string(),
                template_id: template.id.clone(),
                parameter_values: HashMap::new(),
                start_immediately: false,
            })
            .await
            .expect("创建批次");

        assert!(response.tasks[0].output_path.ends_with("plain.mp4"), "{}", response.tasks[0].output_path);
    }

    #[tokio::test]
    async fn create_batch_start_immediately_enqueues_all_children() {
        let h = Harness::new("batch-start").await;
        let template = h.software_template().await;
        let first = h.make_input_video("a.mp4");
        let second = h.make_input_video("b.mp4");

        let response = h
            .service
            .create_batch(CreateBatchRequest {
                input_paths: vec![first, second],
                output_directory: h.dir.to_string_lossy().to_string(),
                template_id: template.id.clone(),
                parameter_values: HashMap::new(),
                start_immediately: true,
            })
            .await
            .expect("创建批次");

        for task in &response.tasks {
            let finished = wait_terminal(&h.store, &task.id).await;
            assert_eq!(finished.status, TaskStatus::Completed, "错误：{:?}", finished.error_message);
        }
        h.queue.shutdown().await;
    }

    #[tokio::test]
    async fn cancel_batch_cancels_only_unfinished_children() {
        let h = Harness::new("batch-cancel").await;
        let template = h.software_template().await;
        let input = h.make_input_video("c.mp4");

        let response = h
            .service
            .create_batch(CreateBatchRequest {
                input_paths: vec![input, h.dir.join("missing.mp4").to_string_lossy().to_string()],
                output_directory: h.dir.to_string_lossy().to_string(),
                template_id: template.id.clone(),
                parameter_values: HashMap::new(),
                start_immediately: false,
            })
            .await
            .expect("创建批次");

        // 先把第一个子任务标成已完成，验证它不会被取消
        let mut done = response.tasks[0].clone();
        done.status = TaskStatus::Completed;
        h.store.update_task(&done).await.unwrap();

        let cancelled = h.service.cancel_batch(&response.batch.id).await.expect("取消批次");
        assert_eq!(cancelled, 1, "只有一个未结束的子任务");

        let after = h.store.get_task(&response.tasks[1].id).await.unwrap().unwrap();
        assert_eq!(after.status, TaskStatus::Cancelled);
        assert_eq!(after.error_message.as_deref(), Some("用户取消"));
        let untouched = h.store.get_task(&done.id).await.unwrap().unwrap();
        assert_eq!(untouched.status, TaskStatus::Completed);

        // 取消也要把批次状态维护对（§3.11）：1 完成 + 1 取消 → Failed，完成 1、失败 1
        let batch = h.service.get_batch(&response.batch.id).await.unwrap().expect("批次存在");
        assert_eq!(batch.total_files, 2);
        assert_eq!(batch.completed_files, 1);
        assert_eq!(batch.failed_files, 1);
        assert_eq!(batch.status, TaskStatus::Failed);
        assert!(batch.completed_at.is_some());
    }

    #[tokio::test]
    async fn cancel_pending_task_that_was_never_enqueued_still_cancels() {
        let h = Harness::new("cancel-pending").await;
        let template = h.software_template().await;
        let input = h.make_input_video("c.mp4");

        let created = h
            .service
            .create_task(CreateTaskRequest {
                input_path: input,
                output_path: h.dir.join("c.mp4").to_string_lossy().to_string(),
                template_id: template.id.clone(),
                parameter_values: HashMap::new(),
                overwrite_existing: Some(true),
                start_immediately: false,
            })
            .await
            .expect("创建任务");

        // 队列内存里没有它（从未入队）→ TaskService 自己补齐 §3.9 的 Pending 语义
        assert!(h.service.cancel_task(&created.task.id, false).await.expect("取消任务"));
        let after = h.store.get_task(&created.task.id).await.unwrap().unwrap();
        assert_eq!(after.status, TaskStatus::Cancelled);
        assert_eq!(after.error_message.as_deref(), Some("用户取消"));
        assert!(after.completed_at.is_some());

        // 已终态 → false（界面据此提示「该任务已经结束。」）
        assert!(!h.service.cancel_task(&created.task.id, false).await.expect("再次取消"));
        // 不存在的任务 → false
        assert!(!h.service.cancel_task("000000000000000000000000", false).await.expect("未知任务"));
    }

    #[tokio::test]
    async fn retry_failed_in_batch_retries_every_failure() {
        let h = Harness::new("batch-retry").await;
        let template = h.software_template().await;
        let input = h.make_input_video("r.mp4");

        let response = h
            .service
            .create_batch(CreateBatchRequest {
                input_paths: vec![input, h.dir.join("r2.mp4").to_string_lossy().to_string()],
                output_directory: h.dir.to_string_lossy().to_string(),
                template_id: template.id.clone(),
                parameter_values: HashMap::new(),
                start_immediately: false,
            })
            .await
            .expect("创建批次");

        // 第一个失败、第二个等待中
        let mut failed = response.tasks[0].clone();
        failed.status = TaskStatus::Failed;
        failed.error_message = Some("测试失败".to_string());
        h.store.update_task(&failed).await.unwrap();

        let (retried, queued) = h.service.retry_failed_in_batch(&response.batch.id).await.expect("重试失败项");
        assert_eq!(retried, 1);
        assert_eq!(queued, 1, "重试必须真的入队");

        let children = h.service.list_tasks_by_batch(&response.batch.id).await.unwrap();
        assert_eq!(children.len(), 3, "重试会新建任务");
        let new_task = children
            .iter()
            .find(|task| task.retry_of_task_id.as_deref() == Some(failed.id.as_str()))
            .expect("必须存在带 retryOfTaskId 的新任务");
        assert_eq!(new_task.batch_id.as_deref(), Some(response.batch.id.as_str()));
        h.queue.shutdown().await;
    }

    #[tokio::test]
    async fn delete_batch_removes_children_and_returns_count() {
        let h = Harness::new("batch-delete").await;
        let template = h.software_template().await;
        let input = h.make_input_video("d.mp4");

        let response = h
            .service
            .create_batch(CreateBatchRequest {
                input_paths: vec![input],
                output_directory: h.dir.to_string_lossy().to_string(),
                template_id: template.id.clone(),
                parameter_values: HashMap::new(),
                start_immediately: false,
            })
            .await
            .expect("创建批次");

        assert_eq!(h.service.delete_batch(&response.batch.id).await.unwrap(), 1);
        assert!(h.service.get_batch(&response.batch.id).await.unwrap().is_none());
        assert!(h.service.list_tasks_by_batch(&response.batch.id).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn create_batch_rejects_blank_output_directory() {
        let h = Harness::new("batch-no-dir").await;
        let template = h.software_template().await;
        let input = h.make_input_video("n.mp4");

        let error = h
            .service
            .create_batch(CreateBatchRequest {
                input_paths: vec![input],
                output_directory: "   ".to_string(),
                template_id: template.id.clone(),
                parameter_values: HashMap::new(),
                start_immediately: false,
            })
            .await
            .expect_err("空输出目录必须报错（否则会落到进程 CWD）");
        assert_eq!(error.to_string(), preview::OUTPUT_PATH_UNDETERMINED_MESSAGE);
        // 批次记录也不能落库（否则界面上会出现一个永远没有子任务的空批次）
        assert_eq!(h.service.list_batches(10, 0).await.unwrap().1, 0);
    }

    #[tokio::test]
    async fn list_batches_paginate_and_count() {
        let h = Harness::new("list-batches").await;
        for index in 0..3 {
            let mut batch = BatchTask::default();
            batch.created_at = Utc::now() + ChronoDuration::milliseconds(index);
            h.store.insert_batch(&batch).await.unwrap();
        }
        let (items, total) = h.service.list_batches(2, 1).await.unwrap();
        assert_eq!(total, 3);
        assert_eq!(items.len(), 2);
    }

    // ── 历史维护与孤儿恢复 ─────────────────────────────────────

    #[tokio::test]
    async fn cleanup_history_respects_retention_window() {
        let h = Harness::new("cleanup").await;
        let old = h
            .store_task(ConversionTask { status: TaskStatus::Completed, ..ConversionTask::default() })
            .await;
        let recent = h
            .store_task(ConversionTask { status: TaskStatus::Completed, ..ConversionTask::default() })
            .await;

        // 把其中一条挪到 40 天前
        let mut aged = old.clone();
        aged.created_at = Utc::now() - ChronoDuration::days(40);
        h.store.update_task(&aged).await.unwrap();

        // retentionDays = 0 → 什么都不做
        assert_eq!(h.service.cleanup_history(0).await.unwrap(), (0, 0));

        let (tasks, batches) = h.service.cleanup_history(30).await.unwrap();
        assert_eq!(tasks, 1);
        assert_eq!(batches, 0);
        assert!(h.store.get_task(&old.id).await.unwrap().is_none());
        assert!(h.store.get_task(&recent.id).await.unwrap().is_some());
    }

    #[tokio::test]
    async fn cleanup_history_never_deletes_running_tasks() {
        let h = Harness::new("cleanup-running").await;
        let running = h
            .store_task(ConversionTask { status: TaskStatus::Running, ..ConversionTask::default() })
            .await;

        let mut aged = running.clone();
        aged.created_at = Utc::now() - ChronoDuration::days(90);
        h.store.update_task(&aged).await.unwrap();

        let (tasks, _batches) = h.service.cleanup_history(30).await.unwrap();
        assert_eq!(tasks, 0, "运行中的任务不算历史");
        assert!(h.store.get_task(&running.id).await.unwrap().is_some());
    }

    #[tokio::test]
    async fn cleanup_history_removes_batches_without_tasks() {
        let h = Harness::new("cleanup-batches").await;
        let template = h.software_template().await;
        let input = h.make_input_video("h.mp4");

        let response = h
            .service
            .create_batch(CreateBatchRequest {
                input_paths: vec![input],
                output_directory: h.dir.to_string_lossy().to_string(),
                template_id: template.id.clone(),
                parameter_values: HashMap::new(),
                start_immediately: false,
            })
            .await
            .expect("创建批次");

        // 把子任务挪到 40 天前 → 清理任务后批次成为空壳
        let mut aged = response.tasks[0].clone();
        aged.created_at = Utc::now() - ChronoDuration::days(40);
        h.store.update_task(&aged).await.unwrap();

        let (tasks, batches) = h.service.cleanup_history(30).await.unwrap();
        assert_eq!(tasks, 1);
        assert_eq!(batches, 1);
        assert!(h.service.get_batch(&response.batch.id).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn clear_history_removes_terminal_tasks_and_batches() {
        let h = Harness::new("clear").await;
        h.store_task(ConversionTask { status: TaskStatus::Completed, ..ConversionTask::default() }).await;
        h.store_task(ConversionTask { status: TaskStatus::Failed, ..ConversionTask::default() }).await;
        h.store_task(ConversionTask { status: TaskStatus::Cancelled, ..ConversionTask::default() }).await;
        h.store_task(ConversionTask { status: TaskStatus::Skipped, ..ConversionTask::default() }).await;
        h.store_task(ConversionTask { status: TaskStatus::Pending, ..ConversionTask::default() }).await;
        h.store.insert_batch(&BatchTask::default()).await.unwrap();

        assert_eq!(h.service.clear_history().await.unwrap(), 4);
        assert_eq!(h.service.list_batches(10, 0).await.unwrap().1, 0);
        assert_eq!(h.service.pending_tasks().await.unwrap().len(), 1, "等待中的任务不受影响");
    }

    #[tokio::test]
    async fn recover_orphaned_tasks_marks_running_failed() {
        let h = Harness::new("recover").await;
        let output = h.dir.join("partial.mp4");
        std::fs::write(&output, vec![0u8; 1024]).unwrap();

        let running = h
            .store_task(ConversionTask {
                status: TaskStatus::Running,
                input_path: h.dir.join("in.mp4").to_string_lossy().to_string(),
                output_path: output.to_string_lossy().to_string(),
                progress: 55.0,
                processing_speed: Some(1.5),
                estimated_time_remaining: Some(10.0),
                started_at: Some(Utc::now()),
                ..ConversionTask::default()
            })
            .await;
        let pending = h
            .store_task(ConversionTask { status: TaskStatus::Pending, ..ConversionTask::default() })
            .await;

        assert_eq!(h.service.recover_orphaned_tasks().await.unwrap(), 1);

        let recovered = h.store.get_task(&running.id).await.unwrap().unwrap();
        assert_eq!(recovered.status, TaskStatus::Failed);
        assert_eq!(recovered.error_message.as_deref(), Some(ORPHANED_TASK_MESSAGE));
        assert!(recovered.completed_at.is_some());
        assert!(recovered.estimated_time_remaining.is_none());
        assert!(recovered.processing_speed.is_none());
        assert!(!output.exists(), "小于 64 MiB 的部分输出必须被清理");

        let untouched = h.store.get_task(&pending.id).await.unwrap().unwrap();
        assert_eq!(untouched.status, TaskStatus::Pending, "等待中的任务不会被自动开始或改写");
    }

    #[tokio::test]
    async fn recover_orphaned_tasks_keeps_large_output() {
        let h = Harness::new("recover-large").await;
        let output = h.dir.join("large.mp4");
        let file = std::fs::File::create(&output).unwrap();
        // 稀疏文件：瞬间见效，不必真的写 64 MiB 数据
        file.set_len(PARTIAL_OUTPUT_MAX_BYTES + 1).unwrap();
        drop(file);

        let running = h
            .store_task(ConversionTask {
                status: TaskStatus::Running,
                input_path: h.dir.join("in.mp4").to_string_lossy().to_string(),
                output_path: output.to_string_lossy().to_string(),
                ..ConversionTask::default()
            })
            .await;

        assert_eq!(h.service.recover_orphaned_tasks().await.unwrap(), 1);
        assert!(output.exists(), "大文件可能是完整输出，必须保留");
        assert_eq!(
            h.store.get_task(&running.id).await.unwrap().unwrap().status,
            TaskStatus::Failed
        );
    }

    #[tokio::test]
    async fn recover_never_deletes_input_file() {
        let h = Harness::new("recover-input").await;
        let input = h.dir.join("same.mp4");
        std::fs::write(&input, b"source").unwrap();

        h.store_task(ConversionTask {
            status: TaskStatus::Running,
            input_path: input.to_string_lossy().to_string(),
            output_path: input.to_string_lossy().to_string(),
            ..ConversionTask::default()
        })
        .await;

        assert_eq!(h.service.recover_orphaned_tasks().await.unwrap(), 1);
        assert!(input.exists(), "输出 == 输入时绝不能删源文件");
    }

    // ── preview ───────────────────────────────────────────────

    #[tokio::test]
    async fn preview_without_conflict_reports_requested_path() {
        let h = Harness::new("preview-clean").await;
        let template = h.software_template().await;
        let input = h.make_input_video("clip.mp4");

        let preview = h
            .service
            .preview(h.preview_request(&template.id, &input))
            .await
            .expect("预览");

        assert!(!preview.renamed && !preview.skip && preview.overwrite);
        assert!(preview.conflict_notice.is_none());
        assert!(preview.paths_equal_warning.is_none());
        assert!(preview.output_directory_writable);
        assert!(preview.template_runnable && preview.blocking_error.is_none());
        assert!(!preview.uses_hardware_acceleration, "软件模板不做硬件加速");
        assert!(preview.encoder.is_none(), "模板里没有 {{encoder}} → 不注入编码器");
        assert_eq!(preview.template_name, template.name);
        assert!(preview.requested_output_path.ends_with("clip_converted.mp4"), "{}", preview.requested_output_path);
        assert_eq!(preview.resolved_output_path, preview.requested_output_path);
        assert!(preview.media.is_some(), "真实文件必须探测出媒体信息");
        assert!(preview.issues.is_empty(), "干净模板不应有校验问题：{:?}", preview.issues);
    }

    #[tokio::test]
    async fn preview_missing_input_leaves_media_null() {
        let h = Harness::new("preview-no-media").await;
        let template = h.software_template().await;
        let missing = h.dir.join("nope.mp4").to_string_lossy().to_string();

        let preview = h
            .service
            .preview(h.preview_request(&template.id, &missing))
            .await
            .expect("媒体探测失败不是预览错误");
        assert!(preview.media.is_none());
    }

    #[tokio::test]
    async fn preview_existing_output_with_ask_renames_with_exact_notice() {
        let h = Harness::new("preview-ask").await;
        let template = h.software_template().await;
        let input = h.make_input_video("clip.mp4");
        let existing = h.dir.join("clip_converted.mp4");
        std::fs::write(&existing, b"already here").unwrap();

        let preview = h
            .service
            .preview(h.preview_request(&template.id, &input))
            .await
            .expect("预览");

        assert!(preview.renamed, "Ask 在预览阶段等价于重命名");
        assert!(!preview.skip);
        assert!(preview.overwrite);
        assert_eq!(
            preview.conflict_notice.as_deref(),
            Some(preview::CONFLICT_NOTICE_RENAME)
        );
        assert_eq!(
            preview.resolved_output_path,
            h.dir.join("clip_converted (1).mp4").to_string_lossy()
        );
        assert!(preview.command_preview.contains("clip_converted (1).mp4"));
    }

    #[tokio::test]
    async fn preview_existing_output_with_skip_flags_skip() {
        let h = Harness::new("preview-skip").await;
        h.set_file_exists_action(FileExistsAction::Skip).await;
        let template = h.software_template().await;
        let input = h.make_input_video("clip.mp4");
        std::fs::write(h.dir.join("clip_converted.mp4"), b"keep me").unwrap();

        let preview = h
            .service
            .preview(h.preview_request(&template.id, &input))
            .await
            .expect("预览");

        assert!(preview.skip, "策略=跳过时必须标记 skip");
        assert!(!preview.overwrite, "跳过时命令必须用 -n");
        assert!(!preview.renamed);
        assert_eq!(preview.conflict_notice.as_deref(), Some(preview::CONFLICT_NOTICE_SKIP));
        assert!(preview.command_preview.contains("-n"));
    }

    #[tokio::test]
    async fn preview_existing_output_with_overwrite_uses_configured_policy() {
        let h = Harness::new("preview-overwrite").await;
        h.set_file_exists_action(FileExistsAction::Overwrite).await;
        let template = h.software_template().await;
        let input = h.make_input_video("clip.mp4");
        std::fs::write(h.dir.join("clip_converted.mp4"), b"old").unwrap();

        let preview = h
            .service
            .preview(h.preview_request(&template.id, &input))
            .await
            .expect("预览");

        assert!(!preview.renamed && !preview.skip && preview.overwrite);
        assert_eq!(preview.conflict_notice.as_deref(), Some(preview::CONFLICT_NOTICE_OVERWRITE));
        assert!(preview.command_preview.contains("-y"));
    }

    #[tokio::test]
    async fn preview_forced_overwrite_hides_conflict_notice() {
        let h = Harness::new("preview-forced").await;
        let template = h.software_template().await;
        let input = h.make_input_video("clip.mp4");
        std::fs::write(h.dir.join("clip_converted.mp4"), b"old").unwrap();

        let mut request = h.preview_request(&template.id, &input);
        request.overwrite_existing = true;

        let preview = h.service.preview(request).await.expect("预览");
        assert!(preview.overwrite && !preview.renamed && !preview.skip);
        assert!(preview.conflict_notice.is_none(), "强制覆盖时不再提示冲突（§5.1 第 3 步）");
        assert!(preview.command_preview.contains("-y"));
    }

    #[tokio::test]
    async fn preview_output_equal_to_input_forces_rename_with_exact_warning() {
        let h = Harness::new("preview-paths-equal").await;
        let template = h.software_template().await;
        let input = h.make_input_video("self.mp4");

        let mut request = h.preview_request(&template.id, &input);
        request.output_file_name_template = Some("{filename}".to_string());
        request.output_extension = Some("mp4".to_string());

        let preview = h.service.preview(request).await.expect("预览");
        assert!(preview.renamed, "输出==输入必须强制改名（无视策略）");
        assert_eq!(preview.paths_equal_warning.as_deref(), Some(preview::PATHS_EQUAL_WARNING));
        assert!(
            preview.resolved_output_path.ends_with("self (1).mp4"),
            "{}",
            preview.resolved_output_path
        );
    }

    #[tokio::test]
    async fn preview_strict_hardware_template_blocks_execution() {
        let h = Harness::new("preview-strict").await;
        let template = h.strict_nvenc_template().await;
        let input = h.make_input_video("clip.mp4");

        // 本机若真的装了 NVENC，这条断言就没有意义（macOS 上不会发生）
        let report = h.hardware.detect(false).await;
        let has_nvenc = report
            .available_hardware_encoders()
            .iter()
            .any(|encoder| encoder.family.eq_ignore_ascii_case("nvenc"));
        if has_nvenc {
            eprintln!("本机存在可用 NVENC，跳过严格模式断言");
            return;
        }

        let preview = h
            .service
            .preview(h.preview_request(&template.id, &input))
            .await
            .expect("预览");

        let blocking = preview.blocking_error.expect("严格模式缺硬件必须阻塞");
        assert_eq!(
            blocking,
            preview::strict_hardware_error(Some("nvenc")),
            "阻塞错误必须逐字来自规格"
        );
        assert!(!preview.template_runnable);
        assert!(!preview.uses_hardware_acceleration);
    }

    #[tokio::test]
    async fn preview_unknown_template_is_an_error() {
        let h = Harness::new("preview-unknown").await;
        let input = h.make_input_video("clip.mp4");
        let error = h
            .service
            .preview(h.preview_request("000000000000000000000000", &input))
            .await
            .expect_err("模板不存在必须报错");
        assert_eq!(error.to_string(), preview::TEMPLATE_MISSING_MESSAGE);
    }

    #[tokio::test]
    async fn preview_command_is_fully_resolved_and_has_overwrite_flag() {
        let h = Harness::new("preview-command").await;
        let template = h.hardware_template().await;
        let input = h.make_input_video("clip.mp4");

        let preview = h
            .service
            .preview(h.preview_request(&template.id, &input))
            .await
            .expect("预览");

        assert!(
            preview.command_preview.starts_with("-hide_banner -y -progress pipe:2"),
            "{}",
            preview.command_preview
        );
        assert!(!preview.command_preview.contains('{'), "命令里不能残留占位符");
        assert!(!preview.command_preview.contains('}'), "命令里不能残留占位符");
        assert!(preview.command_preview.contains("-i "), "必须包含输入参数");
        assert!(preview.encoder.is_some(), "含 {{encoder}} 的模板必须解析出编码器");
        let encoder = preview.encoder.as_ref().unwrap();
        assert!(preview.uses_hardware_acceleration == encoder.is_hardware);
        assert!(!encoder.name.is_empty() && !encoder.codec.is_empty());
    }

    #[tokio::test]
    async fn preview_warns_about_unknown_placeholders() {
        let h = Harness::new("preview-warning").await;
        let template = h
            .insert_template(CommandTemplate {
                name: format!("占位符模板 {}", crate::models::new_id()),
                command_args: "-i {input} -vf {unknown_filter} {output}".to_string(),
                output_extension: "mp4".to_string(),
                hardware_mode: HardwareAccelerationMode::None,
                ..CommandTemplate::default()
            })
            .await;
        let input = h.make_input_video("clip.mp4");

        let preview = h
            .service
            .preview(h.preview_request(&template.id, &input))
            .await
            .expect("预览");

        assert_eq!(preview.warnings.len(), 1, "{:?}", preview.warnings);
        assert!(
            preview.warnings[0].starts_with("以下占位符没有对应取值"),
            "{}",
            preview.warnings[0]
        );
        assert!(preview.warnings[0].contains("{unknown_filter}"));
    }

    #[tokio::test]
    async fn preview_session_uses_request_extension_and_template_fallback() {
        let h = Harness::new("preview-extension").await;
        let template = h.software_template().await;
        let input = h.make_input_video("clip.mp4");

        // 请求里给了 mkv → 用 mkv
        let mut request = h.preview_request(&template.id, &input);
        request.output_extension = Some(".mkv".to_string());
        let preview = h.service.preview(request).await.expect("预览");
        assert!(preview.requested_output_path.ends_with("clip_converted.mkv"), "{}", preview.requested_output_path);

        // 请求里空 → 取模板的 mp4
        let preview = h
            .service
            .preview(h.preview_request(&template.id, &input))
            .await
            .expect("预览");
        assert!(preview.requested_output_path.ends_with("clip_converted.mp4"));
    }

    #[tokio::test]
    async fn preview_without_output_directory_is_an_error() {
        let h = Harness::new("preview-no-dir").await;
        let template = h.software_template().await;
        let input = h.make_input_video("clip.mp4");

        let mut request = h.preview_request(&template.id, &input);
        request.output_directory = "   ".to_string();
        let error = h.service.preview(request).await.expect_err("空输出目录必须报错");
        assert_eq!(error.to_string(), preview::OUTPUT_PATH_UNDETERMINED_MESSAGE);
    }

    #[tokio::test]
    async fn preview_reports_unwritable_output_directory() {
        let h = Harness::new("preview-unwritable").await;
        let template = h.software_template().await;
        let input = h.make_input_video("clip.mp4");

        // `/dev/null` 不可能被当成目录使用 → 可写性探测必然失败
        let mut request = h.preview_request(&template.id, &input);
        request.output_directory = "/dev/null/nested".to_string();

        let preview = h.service.preview(request).await.expect("预览");
        assert!(!preview.output_directory_writable);
        let blocking = preview.blocking_error.expect("目录不可写必须阻塞");
        assert!(blocking.starts_with("输出目录不可写："), "{blocking}");
        assert!(blocking.ends_with("。请换一个目录或检查文件权限。"), "{blocking}");
        assert!(!preview.template_runnable);
    }

    #[tokio::test]
    async fn preview_honours_parameter_values_in_command() {
        let h = Harness::new("preview-params").await;
        // §3.5：`crf` / `quality` / `q` 三个占位符由**同一个**质量值决定，取 `map["q"]`。
        // 因此驱动质量的参数名必须是 `quality`（内置模板也都是这么命名的）。
        let template = h
            .insert_template(CommandTemplate {
                name: format!("参数模板 {}", crate::models::new_id()),
                command_args: "-i {input} -c:v libx264 -crf {crf} -an {output}".to_string(),
                output_extension: "mp4".to_string(),
                hardware_mode: HardwareAccelerationMode::None,
                parameters: vec![crate::models::TemplateParameter {
                    name: "quality".to_string(),
                    label: "质量".to_string(),
                    default_value: "23".to_string(),
                    ..crate::models::TemplateParameter::default()
                }],
                ..CommandTemplate::default()
            })
            .await;
        let input = h.make_input_video("clip.mp4");

        let mut request = h.preview_request(&template.id, &input);
        request.parameter_values.insert("quality".to_string(), "18".to_string());
        let preview = h.service.preview(request).await.expect("预览");
        assert!(preview.command_preview.contains("-crf 18"), "{}", preview.command_preview);

        // 不传参数 → 用模板默认值
        let preview = h
            .service
            .preview(h.preview_request(&template.id, &input))
            .await
            .expect("预览");
        assert!(preview.command_preview.contains("-crf 23"), "{}", preview.command_preview);
    }

    #[tokio::test]
    async fn preview_no_rename_flag_keeps_requested_path() {
        let h = Harness::new("preview-no-rename").await;
        let template = h.software_template().await;
        let input = h.make_input_video("clip.mp4");
        std::fs::write(h.dir.join("clip_converted.mp4"), b"old").unwrap();

        let mut request = h.preview_request(&template.id, &input);
        request.allow_rename = false;

        let preview = h.service.preview(request).await.expect("预览");
        assert!(!preview.renamed, "allowRename=false 时不允许改名");
        assert_eq!(
            preview.resolved_output_path,
            h.dir.join("clip_converted.mp4").to_string_lossy()
        );
        assert!(preview.overwrite);
    }

    #[tokio::test]
    async fn preview_encoder_reports_profile_details() {
        let h = Harness::new("preview-encoder").await;
        let template = h.hardware_template().await;
        let input = h.make_input_video("clip.mp4");

        let preview = h
            .service
            .preview(h.preview_request(&template.id, &input))
            .await
            .expect("预览");
        let encoder = preview.encoder.expect("必须解析出编码器");

        assert!(!encoder.display_name.is_empty());
        assert!(!encoder.family.is_empty());
        assert!(!encoder.quality_hint.is_empty());
        assert!(matches!(
            encoder.encoder_type,
            EncoderType::Software
                | EncoderType::VideoToolbox
                | EncoderType::Nvenc
                | EncoderType::Qsv
                | EncoderType::Amf
                | EncoderType::Vaapi
                | EncoderType::Vulkan
                | EncoderType::MediaFoundation
                | EncoderType::Other
        ));
    }

    // ── 工具函数 ───────────────────────────────────────────────

    #[test]
    fn pagination_and_tail_helpers() {
        let items: Vec<i32> = (0..5).collect();
        assert_eq!(paginate(items.clone(), 2, 1), vec![1, 2]);
        assert_eq!(paginate(items.clone(), 0, 2), vec![2, 3, 4]);
        assert!(paginate(items, 10, 99).is_empty());

        assert_eq!(tail_text("a\nb\nc", 2), "b\nc");
        assert_eq!(tail_text("a\nb\nc", 9), "a\nb\nc");
        assert_eq!(tail_text("a\nb\nc", 0), "");
    }

    #[test]
    fn extension_normalisation() {
        assert_eq!(normalize_extension("MP4"), "mp4");
        assert_eq!(normalize_extension(".mkv"), "mkv");
        assert_eq!(normalize_extension("  "), "mp4");
    }
}
