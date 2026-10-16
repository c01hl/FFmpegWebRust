//! `Store` + `SettingsService` 自检程序（`cargo run --bin store_selftest`）。
//!
//! 用**临时文件数据库**跑一遍真实的 SQLite 往返：每个实体读回来都要逐字段相等，
//! 并验证唯一索引、状态筛选、按时间清理、时间精度与中文日志。
//! 每项检查打印一行 `[ok] …`，失败打印 `[FAIL] …` 并以非 0 退出。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};

use ffmpeg_web_rust::ffmpeg::encoder_catalog::EncoderType;
use ffmpeg_web_rust::models::enums::{
    FileExistsAction, HardwareAccelerationMode, OutputNamingRule, TaskStatus, TemplateParameterKind,
    TemplateType,
};
use ffmpeg_web_rust::models::{
    AppSettings, BatchTask, CommandTemplate, ConversionTask, HardwareEncoder, TemplateParameter,
    TemplateParameterOption,
};
use ffmpeg_web_rust::services::SettingsService;
use ffmpeg_web_rust::store::Store;

/// 简单的检查计数器
struct Checks {
    passed: usize,
    failed: usize,
}

impl Checks {
    fn new() -> Self {
        Self { passed: 0, failed: 0 }
    }

    /// 记录一项通过
    fn ok(&mut self, name: &str) {
        self.passed += 1;
        println!("[ok] {name}");
    }

    /// 记录一项失败（附原因）
    fn fail(&mut self, name: &str, detail: impl std::fmt::Display) {
        self.failed += 1;
        println!("[FAIL] {name}：{detail}");
    }

    /// 断言式检查；`detail` 只在失败时求值
    fn expect(&mut self, name: &str, condition: bool, detail: impl FnOnce() -> String) {
        if condition {
            self.ok(name);
        } else {
            self.fail(name, detail());
        }
    }
}

/// 毫秒时间戳 → `DateTime<Utc>`（自检里用固定时间，避免纳秒精度被列精度截断）
fn at(ms: i64) -> DateTime<Utc> {
    DateTime::<Utc>::from_timestamp_millis(ms).unwrap_or(DateTime::<Utc>::UNIX_EPOCH)
}

/// 固定时间基准（2026-02-14T03:12:44.123Z 附近的整毫秒）
const BASE_MS: i64 = 1_770_000_000_123;

#[tokio::main]
async fn main() -> ExitCode {
    let path = std::env::temp_dir().join(format!(
        "ffmpeg-web-rust-store-selftest-{}.sqlite3",
        std::process::id()
    ));
    println!("[info] 自检数据库：{}", path.display());

    let mut checks = Checks::new();
    let completed = match run(&path, &mut checks).await {
        Ok(()) => true,
        Err(error) => {
            checks.fail("自检流程", format!("{error:#}"));
            false
        }
    };

    cleanup(&path);
    println!(
        "[summary] 通过 {} 项，失败 {} 项",
        checks.passed, checks.failed
    );

    if completed && checks.failed == 0 && checks.passed > 0 {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

/// 全部检查；任何「基础设施级」失败（打不开库等）直接返回 Err
async fn run(path: &Path, checks: &mut Checks) -> anyhow::Result<()> {
    let store = Store::open(path).await?;

    check_pragmas(checks, &store).await?;
    check_settings(checks, &store).await?;
    check_templates(checks, &store).await?;
    check_batches(checks, &store).await?;
    check_tasks(checks, &store).await?;
    check_encoders(checks, &store).await?;
    check_batch_cascade(checks, &store).await?;
    check_concurrent_writes(checks, &store).await?;
    check_in_memory(checks).await?;
    check_settings_service(checks, &store).await?;

    Ok(())
}

/// 连接级 PRAGMA：WAL / 外键 / busy_timeout 必须真的生效
async fn check_pragmas(checks: &mut Checks, store: &Store) -> anyhow::Result<()> {
    let journal_mode = store.journal_mode().await?;
    let foreign_keys = store.foreign_keys_enabled().await?;
    let busy_timeout = store.busy_timeout_ms().await?;
    checks.expect(
        "PRAGMA 生效（journal_mode=wal / foreign_keys=1 / busy_timeout>0）",
        journal_mode == "wal" && foreign_keys && busy_timeout > 0,
        || format!("journal_mode={journal_mode} foreign_keys={foreign_keys} busy_timeout={busy_timeout}"),
    );
    Ok(())
}

/// 设置：写入 / 读回 / 单行语义
async fn check_settings(checks: &mut Checks, store: &Store) -> anyhow::Result<()> {
    let mut settings = AppSettings {
        ffmpeg_path: "/opt/ffmpeg/bin/ffmpeg".to_string(),
        ffprobe_path: "/opt/ffmpeg/bin/ffprobe".to_string(),
        default_output_directory: "/tmp/输出 目录".to_string(),
        output_naming: OutputNamingRule::Original,
        output_suffix: "_x".to_string(),
        file_exists_action: FileExistsAction::Rename,
        prefer_hardware_acceleration: false,
        prefer_hardware_decoding: true,
        preferred_hardware_encoder: "h264_nvenc".to_string(),
        auto_fallback_to_software: false,
        max_concurrent_tasks: 4,
        max_log_lines: 1234,
        task_history_retention_days: 7,
        theme: "Dark".to_string(),
        global_ffmpeg_arguments: "-threads 4".to_string(),
        notify_on_completion: false,
        auto_open_browser: false,
        listen_port: 8080,
        ..AppSettings::default()
    };
    store.save_settings(&settings).await?;

    let loaded = store
        .load_settings()
        .await?
        .ok_or_else(|| anyhow::anyhow!("设置写入后读不到"))?;
    checks.expect("settings round-trip", loaded == settings, || {
        format!("读回不一致：{loaded:?}")
    });

    // PORT NOTE 验证：settings 是单条记录，换 id 再存不应留下两行
    settings.id = ffmpeg_web_rust::models::new_id();
    settings.theme = "Light".to_string();
    store.save_settings(&settings).await?;
    let reloaded = store.load_settings().await?.unwrap_or_default();
    checks.expect(
        "settings 单行覆盖（表内只有一条）",
        reloaded.id == settings.id && reloaded.theme == "Light",
        || format!("读回的是 {} / {}", reloaded.id, reloaded.theme),
    );

    Ok(())
}

/// 模板：含参数/选项/null 的往返、按名查询、唯一索引冲突、系统模板删除
async fn check_templates(checks: &mut Checks, store: &Store) -> anyhow::Result<()> {
    let template = CommandTemplate {
        id: ffmpeg_web_rust::models::new_id(),
        name: "自检 · H.265 高质量".to_string(),
        description: "自检用模板，含中文与\"引号\"".to_string(),
        command_args: "-i {input} -c:v libx265 -crf {crf} {preset} {output}".to_string(),
        r#type: TemplateType::User,
        category: "视频转码".to_string(),
        supported_input_formats: vec!["mp4".to_string(), "mov".to_string()],
        output_extension: "mp4".to_string(),
        target_codec: "hevc".to_string(),
        requires_hardware_acceleration: false,
        hardware_mode: HardwareAccelerationMode::Auto,
        required_encoder: None, // null 列
        parameters: vec![
            TemplateParameter {
                name: "crf".to_string(),
                label: "质量（CRF）".to_string(),
                description: "越小越清晰".to_string(),
                kind: TemplateParameterKind::Number,
                default_value: "23".to_string(),
                options: Vec::new(),
                min: Some(0.0),
                max: Some(51.0),
                true_value: "1".to_string(),
                false_value: String::new(),
            },
            TemplateParameter {
                name: "preset".to_string(),
                label: "预设".to_string(),
                description: String::new(),
                kind: TemplateParameterKind::Select,
                default_value: "medium".to_string(),
                options: vec![
                    TemplateParameterOption::new("fast", "快", "体积略大"),
                    TemplateParameterOption::new("medium", "中", ""),
                ],
                min: None,
                max: None,
                true_value: "1".to_string(),
                false_value: String::new(),
            },
        ],
        required_filters: vec!["drawtext".to_string()],
        tags: vec!["自检".to_string(), "h265".to_string()],
        is_recommended: true,
        preset_version: 3,
        created_at: at(BASE_MS),
        updated_at: at(BASE_MS + 1_500),
        sort_order: 10,
    };
    store.insert_template(&template).await?;

    let loaded = store
        .get_template(&template.id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("模板写入后读不到"))?;
    checks.expect("template round-trip（含参数/选项/null）", loaded == template, || {
        format!("读回不一致：{loaded:?}")
    });

    let by_name = store.get_template_by_name(&template.name).await?;
    checks.expect(
        "template 按名查询",
        by_name.as_ref().map(|t| t.id.as_str()) == Some(template.id.as_str()),
        || "按名查到的不是同一条".to_string(),
    );

    // 唯一索引（ix_templates_name）
    let duplicate = CommandTemplate {
        name: template.name.clone(),
        ..CommandTemplate::default()
    };
    let result = store.insert_template(&duplicate).await;
    checks.expect(
        "模板重名必须报错（唯一索引）",
        result.is_err(),
        || "重名竟然插入成功".to_string(),
    );

    // 修改 + 系统模板
    let mut updated = template.clone();
    updated.sort_order = 99;
    updated.parameters.clear();
    updated.required_encoder = Some("nvenc".to_string());
    store.update_template(&updated).await?;
    let reloaded = store.get_template(&template.id).await?.unwrap_or_default();
    checks.expect("template update（含清空列表）", reloaded == updated, || {
        "更新后读回不一致".to_string()
    });

    let system_template = CommandTemplate {
        r#type: TemplateType::System,
        name: format!("系统预设 {}", ffmpeg_web_rust::models::new_id()),
        ..CommandTemplate::default()
    };
    store.insert_template(&system_template).await?;
    let removed = store.delete_system_templates().await?;
    checks.expect(
        "delete_system_templates 只删系统模板",
        removed == 1 && store.get_template(&system_template.id).await?.is_none(),
        || format!("删除了 {removed} 条"),
    );

    let user_only = store.list_templates(false).await?;
    checks.expect(
        "list_templates(false) 只返回用户模板",
        user_only.iter().all(|t| t.r#type == TemplateType::User) && !user_only.is_empty(),
        || format!("返回 {} 条", user_only.len()),
    );

    Ok(())
}

/// 批次：两个批次（含 completed_at 为 null / 非 null）往返
async fn check_batches(checks: &mut Checks, store: &Store) -> anyhow::Result<()> {
    let pending = BatchTask {
        id: ffmpeg_web_rust::models::new_id(),
        name: "批量任务 - 2026-02-14 11:12".to_string(),
        template_id: ffmpeg_web_rust::models::new_id(),
        total_files: 5,
        completed_files: 0,
        failed_files: 0,
        status: TaskStatus::Pending,
        created_at: at(BASE_MS),
        completed_at: None,
    };
    let done = BatchTask {
        id: ffmpeg_web_rust::models::new_id(),
        name: "批量任务 - 2026-02-13 09:00".to_string(),
        template_id: pending.template_id.clone(),
        total_files: 3,
        completed_files: 2,
        failed_files: 1,
        status: TaskStatus::Completed,
        created_at: at(BASE_MS - 86_400_000),
        completed_at: Some(at(BASE_MS - 86_000_000)),
    };
    store.insert_batch(&pending).await?;
    store.insert_batch(&done).await?;

    let loaded_pending = store.get_batch(&pending.id).await?.unwrap_or_default();
    let loaded_done = store.get_batch(&done.id).await?.unwrap_or_default();
    checks.expect(
        "batch round-trip（含 completed_at null）",
        loaded_pending == pending && loaded_done == done,
        || format!("读回不一致：{loaded_pending:?} / {loaded_done:?}"),
    );

    let mut updated = done.clone();
    updated.completed_files = 3;
    updated.failed_files = 0;
    store.update_batch(&updated).await?;
    checks.expect(
        "batch update",
        store.get_batch(&done.id).await?.as_ref() == Some(&updated),
        || "更新后读回不一致".to_string(),
    );

    let batches = store.list_batches(10).await?;
    checks.expect(
        "list_batches 倒序",
        batches.len() == 2 && batches[0].id == pending.id,
        || format!("返回 {} 条", batches.len()),
    );

    Ok(())
}

/// 任务：全字段往返（含 null 列与字典）、状态筛选、按时间清理、时间精度、中文日志
async fn check_tasks(checks: &mut Checks, store: &Store) -> anyhow::Result<()> {
    // 用「当前毫秒」当基准：既要靠它做「按时间清理」的判定，又要保证毫秒精度可无损往返
    let now_ms = Utc::now().timestamp_millis();
    let now = at(now_ms);
    let hour = 3_600_000;

    let mut parameters = HashMap::new();
    parameters.insert("crf".to_string(), "20".to_string());
    parameters.insert("preset".to_string(), "medium".to_string());

    let log = "提示：输出路径与输入文件相同，已自动改名。\n第二行：中文日志\nframe=1234  speed=2.1x\n";

    let task = ConversionTask {
        id: ffmpeg_web_rust::models::new_id(),
        input_path: "/Users/me/Movies/带 空格 的输入.mp4".to_string(),
        output_path: "/Users/me/Movies/输出_converted.mp4".to_string(),
        template_id: ffmpeg_web_rust::models::new_id(),
        template_name: "自检 · H.265 高质量".to_string(),
        actual_command: "-hide_banner -y -progress pipe:2 -i in.mp4 -c:v libx265 out.mp4".to_string(),
        status: TaskStatus::Running,
        progress: 42.5,
        total_duration: 123.456,
        current_time: 52.5,
        estimated_time_remaining: Some(9.5),
        processing_speed: Some(2.25),
        log_output: log.to_string(),
        error_message: Some("错误：编码器初始化失败".to_string()),
        created_at: now,
        started_at: Some(at(now_ms + 1_000)),
        completed_at: Some(at(now_ms + 36_000)),
        input_file_size: 104_857_600,
        output_file_size: Some(52_428_800),
        batch_id: Some(ffmpeg_web_rust::models::new_id()),
        template_parameters: Some(parameters),
        retry_of_task_id: Some(ffmpeg_web_rust::models::new_id()),
        output_path_resolved: true,
        overwrite_existing: true,
    };
    store.insert_task(&task).await?;

    let loaded = store
        .get_task(&task.id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("任务写入后读不到"))?;
    checks.expect("task round-trip（全部字段 + null + 参数字典）", loaded == task, || {
        format!("读回不一致：{loaded:?}")
    });

    checks.expect(
        "时间按毫秒精确往返",
        loaded.created_at.timestamp_millis() == now_ms
            && loaded.started_at.map(|t| t.timestamp_millis()) == Some(now_ms + 1_000)
            && loaded.completed_at.map(|t| t.timestamp_millis()) == Some(now_ms + 36_000),
        || format!("created_at={:?}", loaded.created_at),
    );

    checks.expect(
        "log_output 中文 + 换行原样往返",
        loaded.log_output == log
            && loaded.log_output.lines().count() == 3
            && loaded.log_output.contains("中文日志"),
        || format!("读回日志：{:?}", loaded.log_output),
    );

    // 所有可空列都为 null 的任务（同时是「按时间清理」里那条 10 天前的旧任务）
    let nullable = ConversionTask {
        id: ffmpeg_web_rust::models::new_id(),
        input_path: "/tmp/in.mp4".to_string(),
        output_path: "/tmp/out.mp4".to_string(),
        template_id: ffmpeg_web_rust::models::new_id(),
        created_at: at(now_ms - 10 * 24 * hour),
        ..ConversionTask::default()
    };
    store.insert_task(&nullable).await?;
    let loaded_nullable = store.get_task(&nullable.id).await?.unwrap_or_default();
    checks.expect(
        "task 全 null 列往返",
        loaded_nullable == nullable
            && loaded_nullable.started_at.is_none()
            && loaded_nullable.error_message.is_none()
            && loaded_nullable.template_parameters.is_none()
            && loaded_nullable.batch_id.is_none(),
        || format!("读回不一致：{loaded_nullable:?}"),
    );

    // 状态筛选
    let mut completed = ConversionTask {
        id: ffmpeg_web_rust::models::new_id(),
        status: TaskStatus::Completed,
        created_at: at(now_ms + 2_000),
        ..ConversionTask::default()
    };
    completed.progress = 100.0;
    store.insert_task(&completed).await?;

    let running = store.list_tasks(100, Some(TaskStatus::Running)).await?;
    let running_only = running.iter().all(|t| t.status == TaskStatus::Running);
    checks.expect(
        "list_tasks(status) 只返回该状态",
        running_only
            && running.iter().any(|t| t.id == task.id)
            && !running.iter().any(|t| t.id == completed.id),
        || format!("返回 {} 条，状态齐全={running_only}", running.len()),
    );

    let ordered = store.list_tasks(100, None).await?;
    let descending = ordered.windows(2).all(|w| w[0].created_at >= w[1].created_at);
    checks.expect(
        "list_tasks 按 created_at 倒序",
        descending && ordered.len() == 3,
        || format!("返回 {} 条，倒序={descending}", ordered.len()),
    );

    // 轻量列表：不返回 log_output，其它字段一致
    let lite = store.list_tasks_lite(100, Some(TaskStatus::Running)).await?;
    let mut full_row = task.clone();
    full_row.log_output = String::new();
    checks.expect(
        "list_tasks_lite 不返回 log_output",
        lite.len() == 1 && lite[0].log_output.is_empty() && lite[0] == full_row,
        || format!("返回 {} 条", lite.len()),
    );

    // 按时间清理：cutoff = 5 天前，只应删掉 10 天前那条
    let cutoff = at(now_ms - 5 * 24 * hour);
    let removed = store.delete_tasks_before(cutoff).await?;
    checks.expect(
        "delete_tasks_before 只删旧行",
        removed == 1
            && store.get_task(&nullable.id).await?.is_none()
            && store.get_task(&task.id).await?.is_some(),
        || format!("删除了 {removed} 条"),
    );

    checks.expect(
        "count_tasks 与实际行数一致",
        store.count_tasks().await? == 2 && store.list_tasks(100, None).await?.len() == 2,
        || "计数不一致".to_string(),
    );

    // update_task：命中返回 true 且落库，未命中返回 false
    let mut touched = completed.clone();
    touched.progress = 100.0;
    touched.error_message = Some("自检：更新".to_string());
    let hit = store.update_task(&touched).await?;
    let mut ghost = completed.clone();
    ghost.id = "ffffffffffffffffffffffff".to_string();
    let miss = store.update_task(&ghost).await?;
    checks.expect(
        "update_task 命中 true / 未命中 false",
        hit && !miss && store.get_task(&completed.id).await?.as_ref() == Some(&touched),
        || format!("命中={hit} 未命中={miss}"),
    );

    // 按 id 批量删除
    let cleared = store.delete_tasks(&[task.id.clone(), "不存在的id".to_string()]).await?;
    checks.expect(
        "delete_tasks 按 id 批量删除",
        cleared == 1 && store.get_task(&task.id).await?.is_none(),
        || format!("删除了 {cleared} 条"),
    );

    Ok(())
}

/// 编码器：按 name upsert（同一 name 只留一条，字段被覆盖）
async fn check_encoders(checks: &mut Checks, store: &Store) -> anyhow::Result<()> {
    let first = HardwareEncoder {
        id: ffmpeg_web_rust::models::new_id(),
        name: "h264_nvenc".to_string(),
        display_name: "NVIDIA NVENC H.264".to_string(),
        encoder_type: EncoderType::Nvenc,
        family: "nvenc".to_string(),
        codec: "h264".to_string(),
        is_available: false,
        supports_quality_options: false,
        last_checked_at: at(BASE_MS),
        unavailable_reason: Some("未检测到 NVIDIA 设备".to_string()),
        missing_from_build: false,
        ffmpeg_version: "7.1".to_string(),
    };
    let software = HardwareEncoder {
        id: ffmpeg_web_rust::models::new_id(),
        name: "libx264".to_string(),
        display_name: "libx264 H.264".to_string(),
        encoder_type: EncoderType::Software,
        family: "software".to_string(),
        codec: "h264".to_string(),
        is_available: true,
        supports_quality_options: true,
        last_checked_at: at(BASE_MS),
        unavailable_reason: None,
        missing_from_build: false,
        ffmpeg_version: "7.1".to_string(),
    };
    store.upsert_encoders(&[first.clone(), software.clone()]).await?;
    checks.expect(
        "encoders 首次写入",
        store.load_encoders().await?.len() == 2,
        || "条数不是 2".to_string(),
    );

    // 同名再 upsert（不同 id / 不同值）：必须只有一条，且字段被覆盖、id 保持原样
    let updated = HardwareEncoder {
        id: ffmpeg_web_rust::models::new_id(),
        display_name: "NVIDIA NVENC H.264（可用）".to_string(),
        is_available: true,
        supports_quality_options: true,
        last_checked_at: at(BASE_MS + 60_000),
        unavailable_reason: None,
        ffmpeg_version: "7.1.1".to_string(),
        ..first.clone()
    };
    store.upsert_encoders(&[updated.clone()]).await?;

    let loaded = store.load_encoders().await?;
    let nvenc = loaded.iter().find(|e| e.name == "h264_nvenc");
    checks.expect(
        "upsert_encoders 按 name 唯一（第二次是更新不是插入）",
        loaded.len() == 2
            && nvenc.map(|e| e.is_available) == Some(true)
            && nvenc.map(|e| e.ffmpeg_version.as_str()) == Some("7.1.1")
            && nvenc.map(|e| e.id.as_str()) == Some(first.id.as_str()),
        || format!("读回 {loaded:?}"),
    );

    let cleared = store.clear_encoders().await?;
    checks.expect(
        "clear_encoders",
        cleared == 2 && store.load_encoders().await?.is_empty(),
        || format!("清理了 {cleared} 条"),
    );

    Ok(())
}

/// 删除批次必须级联删掉子任务（一条事务）
async fn check_batch_cascade(checks: &mut Checks, store: &Store) -> anyhow::Result<()> {
    let now = at(Utc::now().timestamp_millis());
    let batch = BatchTask {
        id: ffmpeg_web_rust::models::new_id(),
        name: "批量任务 - 级联删除自检".to_string(),
        template_id: ffmpeg_web_rust::models::new_id(),
        total_files: 2,
        completed_files: 0,
        failed_files: 0,
        status: TaskStatus::Pending,
        created_at: now,
        completed_at: None,
    };
    store.insert_batch(&batch).await?;

    for index in 0..2 {
        let task = ConversionTask {
            id: ffmpeg_web_rust::models::new_id(),
            input_path: format!("/tmp/cascade-{index}.mp4"),
            output_path: format!("/tmp/cascade-{index}-out.mp4"),
            template_id: batch.template_id.clone(),
            created_at: at(now.timestamp_millis() + index as i64),
            batch_id: Some(batch.id.clone()),
            ..ConversionTask::default()
        };
        store.insert_task(&task).await?;
    }

    let children = store.list_tasks_by_batch(&batch.id).await?;
    let removed = store.delete_batch(&batch.id).await?;
    checks.expect(
        "delete_batch 级联删除子任务",
        children.len() == 2
            && removed == 1
            && store.list_tasks_by_batch(&batch.id).await?.is_empty()
            && store.get_batch(&batch.id).await?.is_none(),
        || format!("子任务 {} 条，删除返回 {removed}", children.len()),
    );

    Ok(())
}

/// 并发写入：队列的多个 worker 同时落库，不允许出现 SQLITE_BUSY
async fn check_concurrent_writes(checks: &mut Checks, store: &Store) -> anyhow::Result<()> {
    let pending_before = store.list_tasks_by_status(TaskStatus::Pending).await?.len();
    let total_before = store.count_tasks().await?;

    let mut handles = Vec::new();
    for index in 0..24 {
        let store = store.clone();
        handles.push(tokio::spawn(async move {
            let task = ConversionTask {
                id: ffmpeg_web_rust::models::new_id(),
                input_path: format!("/tmp/concurrent-{index}.mp4"),
                output_path: format!("/tmp/concurrent-{index}-out.mp4"),
                template_id: ffmpeg_web_rust::models::new_id(),
                created_at: at(BASE_MS + index as i64),
                ..ConversionTask::default()
            };
            let id = task.id.clone();
            store.insert_task(&task).await.map(|_| id)
        }));
    }

    let mut inserted = 0usize;
    let mut failures: Vec<String> = Vec::new();
    for handle in handles {
        match handle.await {
            Ok(Ok(_)) => inserted += 1,
            Ok(Err(error)) => failures.push(format!("{error:#}")),
            Err(error) => failures.push(format!("任务 panic：{error}")),
        }
    }

    let pending_after = store.list_tasks_by_status(TaskStatus::Pending).await?.len();
    checks.expect(
        "并发写入 24 条任务（连接池 + WAL + busy_timeout）",
        failures.is_empty() && inserted == 24 && pending_after == pending_before + 24,
        || format!("成功 {inserted} 条，失败 {failures:?}，库中 Pending={pending_after}"),
    );

    let cleared = store.clear_tasks().await?;
    checks.expect(
        "clear_tasks 清空全部任务",
        cleared == (total_before as usize + 24) && store.count_tasks().await? == 0,
        || format!("清理 {cleared} 条（预期 {}）", total_before + 24),
    );

    Ok(())
}

/// 内存库（`open_in_memory`）：同一句柄读写必须看到同一份数据
async fn check_in_memory(checks: &mut Checks) -> anyhow::Result<()> {
    let store = Store::open_in_memory().await?;
    let task = ConversionTask {
        id: ffmpeg_web_rust::models::new_id(),
        input_path: "/tmp/mem-in.mp4".to_string(),
        output_path: "/tmp/mem-out.mp4".to_string(),
        created_at: at(BASE_MS),
        ..ConversionTask::default()
    };
    store.insert_task(&task).await?;
    let loaded = store.get_task(&task.id).await?;
    checks.expect("open_in_memory 读写一致", loaded.as_ref() == Some(&task), || {
        format!("读回 {loaded:?}")
    });
    Ok(())
}

/// SettingsService：默认值落库、get/save 往返、normalize、广播、ffmpeg 校验
async fn check_settings_service(checks: &mut Checks, store: &Store) -> anyhow::Result<()> {
    let store = Arc::new(store.clone());
    let service = SettingsService::new(store.clone());
    let mut receiver = service.subscribe();

    let initial = service.get().await;
    let persisted = store.load_settings().await?;
    checks.expect(
        "SettingsService 首次运行落默认值（含默认输出目录）",
        !initial.default_output_directory.is_empty()
            && persisted.as_ref().map(|s| s.id.as_str()) == Some(initial.id.as_str()),
        || format!("默认输出目录={:?}", initial.default_output_directory),
    );

    // 非法值必须被 normalize 修正，合法值必须原样保存
    let mut modified = initial.clone();
    modified.theme = "Dark".to_string();
    modified.max_concurrent_tasks = 99; // 越界 → 回退 1
    modified.output_suffix = "   ".to_string(); // 空白 → _converted
    modified.ffmpeg_path = "  \"/usr/local/bin/ffmpeg\"  ".to_string();
    let saved = service.save(modified).await?;
    checks.expect(
        "SettingsService.save 会 normalize 非法值",
        saved.max_concurrent_tasks == 1
            && saved.output_suffix == "_converted"
            && saved.theme == "Dark"
            && saved.ffmpeg_path == "/usr/local/bin/ffmpeg",
        || format!("保存结果：{saved:?}"),
    );

    let reread = service.get().await;
    checks.expect(
        "SettingsService get → save → get 往返",
        reread == saved,
        || format!("读回不一致：{reread:?}"),
    );

    let cached = service.cached().await;
    checks.expect("SettingsService.cached 与 get 一致", cached == saved, || {
        "缓存与读库不一致".to_string()
    });
    checks.expect(
        "SettingsService 落库内容与返回值一致",
        store.load_settings().await?.as_ref() == Some(&saved),
        || "数据库内容与返回值不一致".to_string(),
    );

    let broadcast = tokio::time::timeout(Duration::from_secs(2), receiver.recv()).await;
    checks.expect(
        "SettingsService.subscribe 收到广播",
        matches!(broadcast, Ok(Ok(ref received)) if *received == saved),
        || "没有收到或内容不一致".to_string(),
    );

    let reset = service.reset().await?;
    let reset_broadcast = tokio::time::timeout(Duration::from_secs(2), receiver.recv()).await;
    checks.expect(
        "SettingsService.reset 恢复默认并广播",
        reset.max_concurrent_tasks == 1
            && reset.theme == "System"
            && !reset.default_output_directory.is_empty()
            && matches!(reset_broadcast, Ok(Ok(ref received)) if *received == reset),
        || format!("重置结果：{reset:?}"),
    );

    // ffmpeg 路径校验：不存在的路径必须是明确的失败原因
    let (valid, reason) = service
        .validate_ffmpeg_path("/definitely/not/here/ffmpeg")
        .await;
    checks.expect(
        "validate_ffmpeg_path 不存在的路径 → (false, 文件不存在：…)",
        !valid && reason.starts_with("文件不存在："),
        || format!("返回 ({valid}, {reason})"),
    );

    // 本机若装了 ffmpeg（PATH 里），顺便验证真路径能通过
    match service.ffmpeg_version("").await {
        Some(version) => {
            let (valid, detail) = service.validate_ffmpeg_path("").await;
            checks.expect(
                "validate_ffmpeg_path 对真实 ffmpeg 返回 true",
                valid && detail.starts_with("ffmpeg version") && detail == version,
                || format!("返回 ({valid}, {detail})，版本读取为 {version}"),
            );
        }
        None => println!("[ok] 本机 PATH 中没有 ffmpeg，跳过真实路径校验（不影响结论）"),
    }

    Ok(())
}

/// 删除自检数据库（含 WAL/SHM 附属文件），失败忽略
fn cleanup(path: &Path) {
    for suffix in ["", "-wal", "-shm"] {
        let target = PathBuf::from(format!("{}{suffix}", path.display()));
        let _ = std::fs::remove_file(target);
    }
}
