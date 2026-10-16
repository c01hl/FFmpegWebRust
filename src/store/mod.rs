//! SQLite 持久化层（替代参考实现的 LiteDB）。
//!
//! **接口骨架已冻结**：本文件的方法签名会被模板服务、队列、API 层直接调用，
//! 实现方不得改名/改签名。schema 见 `docs/reference/backend-data.md` §1.6。
//!
//! 约定：
//! - 时间统一存 **Unix 毫秒 UTC（INTEGER）**，API 层转成 RFC3339。
//! - 枚举存整数（与原实现一致），JSON 列（数组/字典）存 TEXT。
//! - `id` 是 24 位小写十六进制字符串。
//!
//! 并发模型（见 `backend-data.md` §1.3 的 Rust 侧建议）：
//! - 连接池 + **短事务**，不使用进程内 `Mutex` 串行化写入；
//! - 连接建立时启用 `PRAGMA foreign_keys`、`journal_mode=WAL`、`busy_timeout`；
//! - 需要一次改多行的操作（批次级联删除、编码器批量 upsert）走单条事务。

use std::path::Path;
use std::time::Duration;

use anyhow::Context;
use chrono::{DateTime, Utc};
use sqlx::sqlite::{
    SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteRow, SqliteSynchronous,
};
use sqlx::{QueryBuilder, Row, SqlitePool};

use crate::models::enums::TaskStatus;
use crate::models::{AppSettings, BatchTask, CommandTemplate, ConversionTask, HardwareEncoder};

/// 建表语句（内嵌的 `schema.sql`，`open` / `open_in_memory` 自动执行，幂等）
const SCHEMA_SQL: &str = include_str!("schema.sql");

/// 连接池上限：本地单用户工具，写并发只来自队列的几个 worker，5 条足够。
const MAX_CONNECTIONS: u32 = 5;

/// 写锁等待上限：并发写入时排队而不是立刻报 `SQLITE_BUSY`
const BUSY_TIMEOUT: Duration = Duration::from_secs(5);

/// 64 位整数毫秒 → `DateTime<Utc>`。
///
/// 越界/非法值不会 panic：回退到 Unix 纪元（宁可显示错误时间，也不要让整行读不出来）。
fn ms_to_time(ms: i64) -> DateTime<Utc> {
    DateTime::<Utc>::from_timestamp_millis(ms).unwrap_or(DateTime::<Utc>::UNIX_EPOCH)
}

/// `DateTime<Utc>` → 64 位整数毫秒（UTC），与 LiteDB 的 BSON date 精度一致
fn time_to_ms(value: DateTime<Utc>) -> i64 {
    value.timestamp_millis()
}

/// 可空时间列 → `Option<DateTime<Utc>>`
fn opt_ms_to_time(ms: Option<i64>) -> Option<DateTime<Utc>> {
    ms.map(ms_to_time)
}

/// 布尔列（INTEGER 0/1）→ `bool`；不做严格校验，非 0 即真
fn get_bool(row: &SqliteRow, column: &str) -> anyhow::Result<bool> {
    Ok(row.try_get::<i64, _>(column)? != 0)
}

/// 布尔 → INTEGER
fn bool_to_int(value: bool) -> i64 {
    if value { 1 } else { 0 }
}

/// 任意可序列化值 → JSON 文本（列表/字典列）
fn to_json<T: serde::Serialize>(value: &T) -> anyhow::Result<String> {
    serde_json::to_string(value).context("序列化 JSON 列失败")
}

/// JSON 文本 → `Vec<String>`。
///
/// 容错：NULL / 空串 / 内容损坏都当作空列表（旧库或手改过的数据不应让整行读失败）。
fn from_json_string_list(text: Option<String>) -> Vec<String> {
    match text {
        Some(text) if !text.trim().is_empty() => serde_json::from_str(&text).unwrap_or_default(),
        _ => Vec::new(),
    }
}

/// JSON 文本 → 模板参数列表（容错同上）
fn from_json_parameters(text: Option<String>) -> Vec<crate::models::TemplateParameter> {
    match text {
        Some(text) if !text.trim().is_empty() => serde_json::from_str(&text).unwrap_or_default(),
        _ => Vec::new(),
    }
}

/// JSON 文本 → 字典（容错：损坏当作「无」）
fn from_json_map(text: Option<String>) -> Option<std::collections::HashMap<String, String>> {
    match text {
        Some(text) if !text.trim().is_empty() => serde_json::from_str(&text).ok(),
        _ => None,
    }
}

/// `tasks` 表的列清单（顺序与 `row_to_task` 一致）
///
/// ⚠️ `current_time` **必须加双引号**：`CURRENT_TIME` 是 SQLite 关键字，
/// 不加引号时 `SELECT current_time` 会被解析成「当前时间」常量（TEXT），
/// 而不是列引用 —— 症状是读回来的值变成 `'07:16:23'` 这种时刻字符串。
macro_rules! task_columns {
    () => {
        "id, input_path, output_path, template_id, template_name, actual_command, status, progress, \
         total_duration, \"current_time\", estimated_time_remaining, processing_speed, log_output, \
         error_message, created_at, started_at, completed_at, input_file_size, output_file_size, \
         batch_id, template_parameters, retry_of_task_id, output_path_resolved, overwrite_existing"
    };
}

/// `tasks` 表的列清单（**不含 `log_output`**，列表接口用，避免把整段日志搬进内存）
macro_rules! task_columns_lite {
    () => {
        "id, input_path, output_path, template_id, template_name, actual_command, status, progress, \
         total_duration, \"current_time\", estimated_time_remaining, processing_speed, \
         error_message, created_at, started_at, completed_at, input_file_size, output_file_size, \
         batch_id, template_parameters, retry_of_task_id, output_path_resolved, overwrite_existing"
    };
}

/// `tasks` 的 24 个绑定占位符
macro_rules! task_placeholders {
    () => {
        "?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?"
    };
}

/// 数据库句柄（`Clone` 廉价，内部是连接池）
#[derive(Clone)]
pub struct Store {
    pool: SqlitePool,
}

impl Store {
    /// 打开（必要时创建）数据库并执行 migration
    pub async fn open(path: &Path) -> anyhow::Result<Self> {
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                // 与原实现一致：目录创建失败静默，真正不可用时在连接时报错
                crate::paths::ensure_directory(parent);
            }
        }

        let options = SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true)
            // 外键约束（未来加引用时才有意义，但现在就打开，避免以后忘记）
            .foreign_keys(true)
            // WAL：读写不互相阻塞，队列的多个 worker 并发写也不会互相卡死
            .journal_mode(SqliteJournalMode::Wal)
            // 本地工具优先响应速度；WAL + NORMAL 崩溃不会损坏数据库
            .synchronous(SqliteSynchronous::Normal)
            .busy_timeout(BUSY_TIMEOUT);

        let pool = SqlitePoolOptions::new()
            .max_connections(MAX_CONNECTIONS)
            .acquire_timeout(BUSY_TIMEOUT)
            .connect_with(options)
            .await
            .with_context(|| format!("打开 SQLite 数据库失败：{}", path.display()))?;

        Self::from_pool(pool).await
    }

    /// 内存数据库（测试用）
    ///
    /// 为什么 `max_connections(1)`：SQLite 的 `:memory:` 是**每个连接一个独立的库**，
    /// 池里放多条连接就会出现「这条连接建的表那条连接看不见」。
    /// 这里不改成 shared-cache URI（那会引入表级锁并依赖连接不全部关闭），
    /// 而是只留一条常驻连接：`min_connections(1)` + 关闭空闲回收/最长寿命，
    /// 保证池里那唯一一条连接活着，库内容不会因为空闲回收而消失。
    pub async fn open_in_memory() -> anyhow::Result<Self> {
        let options = SqliteConnectOptions::new()
            .in_memory(true)
            .foreign_keys(true)
            // 内存库固定是 memory journal，不设置 journal_mode=WAL（SQLite 会拒绝并保持原样）
            .busy_timeout(BUSY_TIMEOUT);

        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .min_connections(1)
            .idle_timeout(None)
            .max_lifetime(None)
            .acquire_timeout(BUSY_TIMEOUT)
            .connect_with(options)
            .await
            .context("打开内存 SQLite 数据库失败")?;

        Self::from_pool(pool).await
    }

    /// 建表并向调用方交付句柄（`open` 与 `open_in_memory` 共用）
    async fn from_pool(pool: SqlitePool) -> anyhow::Result<Self> {
        // raw_sql 可以一次跑多条 DDL（schema.sql 全部是 IF NOT EXISTS，幂等）
        sqlx::raw_sql(SCHEMA_SQL)
            .execute(&pool)
            .await
            .context("执行 schema.sql 失败")?;
        Ok(Self { pool })
    }

    // ─────────────────────────── 诊断 ───────────────────────────

    /// 当前连接的日志模式（`PRAGMA journal_mode`；文件库应为 `wal`）
    ///
    /// 池里每条连接都用同一份连接选项建立，因此读哪条都一样。
    pub async fn journal_mode(&self) -> anyhow::Result<String> {
        let mode: String = sqlx::query_scalar("PRAGMA journal_mode")
            .fetch_one(&self.pool)
            .await?;
        Ok(mode.to_lowercase())
    }

    /// 当前连接是否启用外键约束（`PRAGMA foreign_keys`）
    pub async fn foreign_keys_enabled(&self) -> anyhow::Result<bool> {
        let value: i64 = sqlx::query_scalar("PRAGMA foreign_keys")
            .fetch_one(&self.pool)
            .await?;
        Ok(value != 0)
    }

    /// 当前连接的写锁等待上限，毫秒（`PRAGMA busy_timeout`）
    pub async fn busy_timeout_ms(&self) -> anyhow::Result<i64> {
        let value: i64 = sqlx::query_scalar("PRAGMA busy_timeout")
            .fetch_one(&self.pool)
            .await?;
        Ok(value)
    }

    // ─────────────────────────── 设置 ───────────────────────────

    /// 读取全局设置；不存在时返回 `None`（调用方负责落默认值）
    pub async fn load_settings(&self) -> anyhow::Result<Option<AppSettings>> {
        const SQL: &str = "SELECT id, ffmpeg_path, ffprobe_path, default_output_directory, \
             output_naming, output_suffix, file_exists_action, prefer_hardware_acceleration, \
             prefer_hardware_decoding, preferred_hardware_encoder, auto_fallback_to_software, \
             max_concurrent_tasks, max_log_lines, task_history_retention_days, theme, \
             global_ffmpeg_arguments, notify_on_completion, auto_open_browser, listen_port \
             FROM settings LIMIT 1";

        let row = sqlx::query(SQL).fetch_optional(&self.pool).await?;
        row.map(|row| row_to_settings(&row)).transpose()
    }

    /// 写入全局设置（单条，覆盖）
    ///
    /// PORT NOTE：参考实现是「读出旧记录 → 沿用其 Id → Upsert」，依赖单条文档语义。
    /// 这里在写入前顺手清掉**其它** id 的行，保证 `settings` 表永远只有一条，
    /// 这样即使调用方换了 id 也不会出现「读到旧那条」的诡异状态。
    pub async fn save_settings(&self, settings: &AppSettings) -> anyhow::Result<()> {
        const DELETE_OTHERS: &str = "DELETE FROM settings WHERE id <> ?";
        const UPSERT: &str = "INSERT INTO settings (id, ffmpeg_path, ffprobe_path, \
             default_output_directory, output_naming, output_suffix, file_exists_action, \
             prefer_hardware_acceleration, prefer_hardware_decoding, preferred_hardware_encoder, \
             auto_fallback_to_software, max_concurrent_tasks, max_log_lines, \
             task_history_retention_days, theme, global_ffmpeg_arguments, notify_on_completion, \
             auto_open_browser, listen_port) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, \
             ?, ?, ?, ?) ON CONFLICT(id) DO UPDATE SET \
             ffmpeg_path = excluded.ffmpeg_path, ffprobe_path = excluded.ffprobe_path, \
             default_output_directory = excluded.default_output_directory, \
             output_naming = excluded.output_naming, output_suffix = excluded.output_suffix, \
             file_exists_action = excluded.file_exists_action, \
             prefer_hardware_acceleration = excluded.prefer_hardware_acceleration, \
             prefer_hardware_decoding = excluded.prefer_hardware_decoding, \
             preferred_hardware_encoder = excluded.preferred_hardware_encoder, \
             auto_fallback_to_software = excluded.auto_fallback_to_software, \
             max_concurrent_tasks = excluded.max_concurrent_tasks, \
             max_log_lines = excluded.max_log_lines, \
             task_history_retention_days = excluded.task_history_retention_days, \
             theme = excluded.theme, global_ffmpeg_arguments = excluded.global_ffmpeg_arguments, \
             notify_on_completion = excluded.notify_on_completion, \
             auto_open_browser = excluded.auto_open_browser, listen_port = excluded.listen_port";

        let mut tx = self.pool.begin().await?;
        sqlx::query(DELETE_OTHERS)
            .bind(&settings.id)
            .execute(&mut *tx)
            .await?;
        sqlx::query(UPSERT)
            .bind(&settings.id)
            .bind(&settings.ffmpeg_path)
            .bind(&settings.ffprobe_path)
            .bind(&settings.default_output_directory)
            .bind(settings.output_naming.to_int())
            .bind(&settings.output_suffix)
            .bind(settings.file_exists_action.to_int())
            .bind(bool_to_int(settings.prefer_hardware_acceleration))
            .bind(bool_to_int(settings.prefer_hardware_decoding))
            .bind(&settings.preferred_hardware_encoder)
            .bind(bool_to_int(settings.auto_fallback_to_software))
            .bind(settings.max_concurrent_tasks)
            .bind(settings.max_log_lines)
            .bind(settings.task_history_retention_days)
            .bind(&settings.theme)
            .bind(&settings.global_ffmpeg_arguments)
            .bind(bool_to_int(settings.notify_on_completion))
            .bind(bool_to_int(settings.auto_open_browser))
            .bind(settings.listen_port)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }

    // ─────────────────────────── 模板 ───────────────────────────

    pub async fn insert_template(&self, template: &CommandTemplate) -> anyhow::Result<()> {
        const SQL: &str = "INSERT INTO templates (id, name, description, command_args, type, \
             category, supported_input_formats, output_extension, target_codec, \
             requires_hardware_acceleration, hardware_mode, required_encoder, parameters, \
             required_filters, tags, is_recommended, preset_version, created_at, updated_at, \
             sort_order) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)";

        sqlx::query(SQL)
            .bind(&template.id)
            .bind(&template.name)
            .bind(&template.description)
            .bind(&template.command_args)
            .bind(template.r#type.to_int())
            .bind(&template.category)
            .bind(to_json(&template.supported_input_formats)?)
            .bind(&template.output_extension)
            .bind(&template.target_codec)
            .bind(bool_to_int(template.requires_hardware_acceleration))
            .bind(template.hardware_mode.to_int())
            .bind(template.required_encoder.as_deref())
            .bind(to_json(&template.parameters)?)
            .bind(to_json(&template.required_filters)?)
            .bind(to_json(&template.tags)?)
            .bind(bool_to_int(template.is_recommended))
            .bind(template.preset_version)
            .bind(time_to_ms(template.created_at))
            .bind(time_to_ms(template.updated_at))
            .bind(template.sort_order)
            .execute(&self.pool)
            .await
            .context("写入模板失败")?;
        Ok(())
    }

    pub async fn update_template(&self, template: &CommandTemplate) -> anyhow::Result<()> {
        const SQL: &str = "UPDATE templates SET name = ?, description = ?, command_args = ?, \
             type = ?, category = ?, supported_input_formats = ?, output_extension = ?, \
             target_codec = ?, requires_hardware_acceleration = ?, hardware_mode = ?, \
             required_encoder = ?, parameters = ?, required_filters = ?, tags = ?, \
             is_recommended = ?, preset_version = ?, created_at = ?, updated_at = ?, \
             sort_order = ? WHERE id = ?";

        sqlx::query(SQL)
            .bind(&template.name)
            .bind(&template.description)
            .bind(&template.command_args)
            .bind(template.r#type.to_int())
            .bind(&template.category)
            .bind(to_json(&template.supported_input_formats)?)
            .bind(&template.output_extension)
            .bind(&template.target_codec)
            .bind(bool_to_int(template.requires_hardware_acceleration))
            .bind(template.hardware_mode.to_int())
            .bind(template.required_encoder.as_deref())
            .bind(to_json(&template.parameters)?)
            .bind(to_json(&template.required_filters)?)
            .bind(to_json(&template.tags)?)
            .bind(bool_to_int(template.is_recommended))
            .bind(template.preset_version)
            .bind(time_to_ms(template.created_at))
            .bind(time_to_ms(template.updated_at))
            .bind(template.sort_order)
            .bind(&template.id)
            .execute(&self.pool)
            .await
            .context("更新模板失败")?;
        Ok(())
    }

    pub async fn get_template(&self, id: &str) -> anyhow::Result<Option<CommandTemplate>> {
        const SQL: &str = "SELECT id, name, description, command_args, type, category, \
             supported_input_formats, output_extension, target_codec, requires_hardware_acceleration, \
             hardware_mode, required_encoder, parameters, required_filters, tags, is_recommended, \
             preset_version, created_at, updated_at, sort_order FROM templates WHERE id = ?";

        let row = sqlx::query(SQL).bind(id).fetch_optional(&self.pool).await?;
        row.map(|row| row_to_template(&row)).transpose()
    }

    pub async fn get_template_by_name(&self, name: &str) -> anyhow::Result<Option<CommandTemplate>> {
        const SQL: &str = "SELECT id, name, description, command_args, type, category, \
             supported_input_formats, output_extension, target_codec, requires_hardware_acceleration, \
             hardware_mode, required_encoder, parameters, required_filters, tags, is_recommended, \
             preset_version, created_at, updated_at, sort_order FROM templates WHERE name = ?";

        let row = sqlx::query(SQL).bind(name).fetch_optional(&self.pool).await?;
        row.map(|row| row_to_template(&row)).transpose()
    }

    /// 列出模板；`include_system = false` 时只返回用户模板
    pub async fn list_templates(&self, include_system: bool) -> anyhow::Result<Vec<CommandTemplate>> {
        const ALL: &str = "SELECT id, name, description, command_args, type, category, \
             supported_input_formats, output_extension, target_codec, requires_hardware_acceleration, \
             hardware_mode, required_encoder, parameters, required_filters, tags, is_recommended, \
             preset_version, created_at, updated_at, sort_order FROM templates \
             ORDER BY sort_order ASC, name ASC";
        // 用户模板 = type 1（TemplateType::User）
        const USER_ONLY: &str = "SELECT id, name, description, command_args, type, category, \
             supported_input_formats, output_extension, target_codec, requires_hardware_acceleration, \
             hardware_mode, required_encoder, parameters, required_filters, tags, is_recommended, \
             preset_version, created_at, updated_at, sort_order FROM templates \
             WHERE type = 1 ORDER BY sort_order ASC, name ASC";

        let sql = if include_system { ALL } else { USER_ONLY };
        let rows = sqlx::query(sql).fetch_all(&self.pool).await?;
        rows.iter().map(row_to_template).collect()
    }

    /// 返回是否真的删掉了一行
    pub async fn delete_template(&self, id: &str) -> anyhow::Result<bool> {
        let result = sqlx::query("DELETE FROM templates WHERE id = ?")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected() > 0)
    }

    /// 删除全部系统模板（系统预设同步时使用）
    pub async fn delete_system_templates(&self) -> anyhow::Result<usize> {
        // TemplateType::System = 0
        let result = sqlx::query("DELETE FROM templates WHERE type = 0")
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected() as usize)
    }

    // ─────────────────────────── 任务 ───────────────────────────

    pub async fn insert_task(&self, task: &ConversionTask) -> anyhow::Result<()> {
        let sql = concat!(
            "INSERT INTO tasks (",
            task_columns!(),
            ") VALUES (",
            task_placeholders!(),
            ")"
        );

        sqlx::query(sql)
            .bind(&task.id)
            .bind(&task.input_path)
            .bind(&task.output_path)
            .bind(&task.template_id)
            .bind(&task.template_name)
            .bind(&task.actual_command)
            .bind(task.status.to_int())
            .bind(task.progress)
            .bind(task.total_duration)
            .bind(task.current_time)
            .bind(task.estimated_time_remaining)
            .bind(task.processing_speed)
            .bind(&task.log_output)
            .bind(task.error_message.as_deref())
            .bind(time_to_ms(task.created_at))
            .bind(task.started_at.map(time_to_ms))
            .bind(task.completed_at.map(time_to_ms))
            .bind(task.input_file_size)
            .bind(task.output_file_size)
            .bind(task.batch_id.as_deref())
            .bind(match &task.template_parameters {
                Some(map) => Some(to_json(map)?),
                None => None,
            })
            .bind(task.retry_of_task_id.as_deref())
            .bind(bool_to_int(task.output_path_resolved))
            .bind(bool_to_int(task.overwrite_existing))
            .execute(&self.pool)
            .await
            .context("写入任务失败")?;
        Ok(())
    }

    /// 按 id 覆盖整行；返回是否命中
    pub async fn update_task(&self, task: &ConversionTask) -> anyhow::Result<bool> {
        let sql = concat!(
            "UPDATE tasks SET input_path = ?, output_path = ?, template_id = ?, template_name = ?, \
             actual_command = ?, status = ?, progress = ?, total_duration = ?, \"current_time\" = ?, \
             estimated_time_remaining = ?, processing_speed = ?, log_output = ?, error_message = ?, \
             created_at = ?, started_at = ?, completed_at = ?, input_file_size = ?, \
             output_file_size = ?, batch_id = ?, template_parameters = ?, retry_of_task_id = ?, \
             output_path_resolved = ?, overwrite_existing = ? WHERE id = ?"
        );

        let result = sqlx::query(sql)
            .bind(&task.input_path)
            .bind(&task.output_path)
            .bind(&task.template_id)
            .bind(&task.template_name)
            .bind(&task.actual_command)
            .bind(task.status.to_int())
            .bind(task.progress)
            .bind(task.total_duration)
            .bind(task.current_time)
            .bind(task.estimated_time_remaining)
            .bind(task.processing_speed)
            .bind(&task.log_output)
            .bind(task.error_message.as_deref())
            .bind(time_to_ms(task.created_at))
            .bind(task.started_at.map(time_to_ms))
            .bind(task.completed_at.map(time_to_ms))
            .bind(task.input_file_size)
            .bind(task.output_file_size)
            .bind(task.batch_id.as_deref())
            .bind(match &task.template_parameters {
                Some(map) => Some(to_json(map)?),
                None => None,
            })
            .bind(task.retry_of_task_id.as_deref())
            .bind(bool_to_int(task.output_path_resolved))
            .bind(bool_to_int(task.overwrite_existing))
            .bind(&task.id)
            .execute(&self.pool)
            .await
            .context("更新任务失败")?;
        Ok(result.rows_affected() > 0)
    }

    pub async fn get_task(&self, id: &str) -> anyhow::Result<Option<ConversionTask>> {
        let sql = concat!("SELECT ", task_columns!(), " FROM tasks WHERE id = ?");
        let row = sqlx::query(sql).bind(id).fetch_optional(&self.pool).await?;
        row.map(|row| row_to_task(&row)).transpose()
    }

    /// 历史倒序（`created_at DESC`）
    ///
    /// ⚠️ 本方法返回**完整**任务（含 `log_output`）。列表接口请用 `list_tasks_lite`，
    /// 避免一次把几十条任务的整段日志读进内存再序列化给前端。
    pub async fn list_tasks(&self, limit: i64, status: Option<TaskStatus>) -> anyhow::Result<Vec<ConversionTask>> {
        let rows = match status {
            Some(status) => {
                let sql = concat!(
                    "SELECT ",
                    task_columns!(),
                    " FROM tasks WHERE status = ? ORDER BY created_at DESC LIMIT ?"
                );
                sqlx::query(sql)
                    .bind(status.to_int())
                    .bind(limit)
                    .fetch_all(&self.pool)
                    .await?
            }
            None => {
                let sql = concat!(
                    "SELECT ",
                    task_columns!(),
                    " FROM tasks ORDER BY created_at DESC LIMIT ?"
                );
                sqlx::query(sql).bind(limit).fetch_all(&self.pool).await?
            }
        };
        rows.iter().map(row_to_task).collect()
    }

    /// 列表专用：与 `list_tasks` 同样的筛选/排序，但**不选 `log_output`**（该字段返回空串）。
    ///
    /// PORT NOTE：参考实现的列表页也读整份文档（LiteDB 没有列投影），日志多了会明显变慢。
    /// 这里额外提供一个投影版本，**不改变**任何既有签名。
    pub async fn list_tasks_lite(&self, limit: i64, status: Option<TaskStatus>) -> anyhow::Result<Vec<ConversionTask>> {
        let rows = match status {
            Some(status) => {
                let sql = concat!(
                    "SELECT ",
                    task_columns_lite!(),
                    " FROM tasks WHERE status = ? ORDER BY created_at DESC LIMIT ?"
                );
                sqlx::query(sql)
                    .bind(status.to_int())
                    .bind(limit)
                    .fetch_all(&self.pool)
                    .await?
            }
            None => {
                let sql = concat!(
                    "SELECT ",
                    task_columns_lite!(),
                    " FROM tasks ORDER BY created_at DESC LIMIT ?"
                );
                sqlx::query(sql).bind(limit).fetch_all(&self.pool).await?
            }
        };
        rows.iter().map(row_to_task_lite).collect()
    }

    pub async fn list_tasks_by_status(&self, status: TaskStatus) -> anyhow::Result<Vec<ConversionTask>> {
        let sql = concat!(
            "SELECT ",
            task_columns!(),
            " FROM tasks WHERE status = ? ORDER BY created_at ASC"
        );
        let rows = sqlx::query(sql)
            .bind(status.to_int())
            .fetch_all(&self.pool)
            .await?;
        rows.iter().map(row_to_task).collect()
    }

    pub async fn list_tasks_by_batch(&self, batch_id: &str) -> anyhow::Result<Vec<ConversionTask>> {
        let sql = concat!(
            "SELECT ",
            task_columns!(),
            " FROM tasks WHERE batch_id = ? ORDER BY created_at ASC"
        );
        let rows = sqlx::query(sql).bind(batch_id).fetch_all(&self.pool).await?;
        rows.iter().map(row_to_task).collect()
    }

    pub async fn delete_tasks(&self, ids: &[String]) -> anyhow::Result<usize> {
        if ids.is_empty() {
            return Ok(0);
        }
        // 动态 IN 列表：用 QueryBuilder 生成占位符，值仍然全部走绑定参数
        let mut builder = QueryBuilder::new("DELETE FROM tasks WHERE id IN (");
        {
            let mut separated = builder.separated(", ");
            for id in ids {
                separated.push_bind(id.as_str());
            }
            separated.push_unseparated(")");
        }
        let result = builder.build().execute(&self.pool).await?;
        Ok(result.rows_affected() as usize)
    }

    pub async fn delete_tasks_by_status(&self, status: TaskStatus) -> anyhow::Result<usize> {
        let result = sqlx::query("DELETE FROM tasks WHERE status = ?")
            .bind(status.to_int())
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected() as usize)
    }

    /// 删除 `created_at` 早于 cutoff 的任务
    pub async fn delete_tasks_before(&self, cutoff: DateTime<Utc>) -> anyhow::Result<usize> {
        let result = sqlx::query("DELETE FROM tasks WHERE created_at < ?")
            .bind(time_to_ms(cutoff))
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected() as usize)
    }

    pub async fn clear_tasks(&self) -> anyhow::Result<usize> {
        let result = sqlx::query("DELETE FROM tasks").execute(&self.pool).await?;
        Ok(result.rows_affected() as usize)
    }

    pub async fn count_tasks(&self) -> anyhow::Result<i64> {
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM tasks")
            .fetch_one(&self.pool)
            .await?;
        Ok(count)
    }

    // ─────────────────────────── 批次 ───────────────────────────

    pub async fn insert_batch(&self, batch: &BatchTask) -> anyhow::Result<()> {
        const SQL: &str = "INSERT INTO batch_tasks (id, name, template_id, total_files, \
             completed_files, failed_files, status, created_at, completed_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)";

        sqlx::query(SQL)
            .bind(&batch.id)
            .bind(&batch.name)
            .bind(&batch.template_id)
            .bind(batch.total_files)
            .bind(batch.completed_files)
            .bind(batch.failed_files)
            .bind(batch.status.to_int())
            .bind(time_to_ms(batch.created_at))
            .bind(batch.completed_at.map(time_to_ms))
            .execute(&self.pool)
            .await
            .context("写入批次失败")?;
        Ok(())
    }

    pub async fn update_batch(&self, batch: &BatchTask) -> anyhow::Result<bool> {
        const SQL: &str = "UPDATE batch_tasks SET name = ?, template_id = ?, total_files = ?, \
             completed_files = ?, failed_files = ?, status = ?, created_at = ?, completed_at = ? \
             WHERE id = ?";

        let result = sqlx::query(SQL)
            .bind(&batch.name)
            .bind(&batch.template_id)
            .bind(batch.total_files)
            .bind(batch.completed_files)
            .bind(batch.failed_files)
            .bind(batch.status.to_int())
            .bind(time_to_ms(batch.created_at))
            .bind(batch.completed_at.map(time_to_ms))
            .bind(&batch.id)
            .execute(&self.pool)
            .await
            .context("更新批次失败")?;
        Ok(result.rows_affected() > 0)
    }

    pub async fn get_batch(&self, id: &str) -> anyhow::Result<Option<BatchTask>> {
        const SQL: &str = "SELECT id, name, template_id, total_files, completed_files, failed_files, \
             status, created_at, completed_at FROM batch_tasks WHERE id = ?";
        let row = sqlx::query(SQL).bind(id).fetch_optional(&self.pool).await?;
        row.map(|row| row_to_batch(&row)).transpose()
    }

    /// 批次倒序
    pub async fn list_batches(&self, limit: i64) -> anyhow::Result<Vec<BatchTask>> {
        const SQL: &str = "SELECT id, name, template_id, total_files, completed_files, failed_files, \
             status, created_at, completed_at FROM batch_tasks \
             ORDER BY created_at DESC LIMIT ?";
        let rows = sqlx::query(SQL).bind(limit).fetch_all(&self.pool).await?;
        rows.iter().map(row_to_batch).collect()
    }

    /// 删除某批次及其全部子任务
    pub async fn delete_batch(&self, id: &str) -> anyhow::Result<usize> {
        // 一条事务：子任务 + 批次要么都删掉，要么都不动
        let mut tx = self.pool.begin().await?;
        sqlx::query("DELETE FROM tasks WHERE batch_id = ?")
            .bind(id)
            .execute(&mut *tx)
            .await?;
        let result = sqlx::query("DELETE FROM batch_tasks WHERE id = ?")
            .bind(id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(result.rows_affected() as usize)
    }

    // ─────────────────────── 硬件编码器缓存 ───────────────────────

    /// 按 `name` upsert
    pub async fn upsert_encoders(&self, encoders: &[HardwareEncoder]) -> anyhow::Result<()> {
        // supported_codecs 是参考实现的兼容字段，当前模型没有对应字段：写常量 '[]' 保持列非空。
        const SQL: &str = "INSERT INTO encoders (id, name, display_name, type, family, codec, \
             is_available, supports_quality_options, supported_codecs, last_checked_at, \
             unavailable_reason, missing_from_build, ffmpeg_version) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, '[]', ?, ?, ?, ?) \
             ON CONFLICT(name) DO UPDATE SET \
             display_name = excluded.display_name, type = excluded.type, family = excluded.family, \
             codec = excluded.codec, is_available = excluded.is_available, \
             supports_quality_options = excluded.supports_quality_options, \
             last_checked_at = excluded.last_checked_at, \
             unavailable_reason = excluded.unavailable_reason, \
             missing_from_build = excluded.missing_from_build, \
             ffmpeg_version = excluded.ffmpeg_version";

        let mut tx = self.pool.begin().await?;
        for encoder in encoders {
            sqlx::query(SQL)
                .bind(&encoder.id)
                .bind(&encoder.name)
                .bind(&encoder.display_name)
                .bind(encoder.encoder_type.to_int())
                .bind(&encoder.family)
                .bind(&encoder.codec)
                .bind(bool_to_int(encoder.is_available))
                .bind(bool_to_int(encoder.supports_quality_options))
                .bind(time_to_ms(encoder.last_checked_at))
                .bind(encoder.unavailable_reason.as_deref())
                .bind(bool_to_int(encoder.missing_from_build))
                .bind(&encoder.ffmpeg_version)
                .execute(&mut *tx)
                .await
                .with_context(|| format!("upsert 编码器失败：{}", encoder.name))?;
        }
        tx.commit().await?;
        Ok(())
    }

    pub async fn load_encoders(&self) -> anyhow::Result<Vec<HardwareEncoder>> {
        const SQL: &str = "SELECT id, name, display_name, type, family, codec, is_available, \
             supports_quality_options, last_checked_at, unavailable_reason, missing_from_build, \
             ffmpeg_version FROM encoders ORDER BY type ASC, name ASC";
        let rows = sqlx::query(SQL).fetch_all(&self.pool).await?;
        rows.iter().map(row_to_encoder).collect()
    }

    pub async fn clear_encoders(&self) -> anyhow::Result<usize> {
        let result = sqlx::query("DELETE FROM encoders").execute(&self.pool).await?;
        Ok(result.rows_affected() as usize)
    }
}

// ─────────────────────────── 行映射 ───────────────────────────
//
// 全部手写（`sqlx::Row` / `try_get`），不用 `query!` 宏：宏需要构建期连上 DATABASE_URL 的活库，
// 而本应用的库路径是运行时才确定的（见 `paths::database_path`）。

/// 行 → 任务（含 `log_output`）
fn row_to_task(row: &SqliteRow) -> anyhow::Result<ConversionTask> {
    Ok(ConversionTask {
        id: row.try_get("id")?,
        input_path: row.try_get("input_path")?,
        output_path: row.try_get("output_path")?,
        template_id: row.try_get("template_id")?,
        template_name: row.try_get("template_name")?,
        actual_command: row.try_get("actual_command")?,
        status: TaskStatus::from_int(row.try_get("status")?),
        progress: row.try_get("progress")?,
        total_duration: row.try_get("total_duration")?,
        current_time: row.try_get("current_time")?,
        estimated_time_remaining: row.try_get("estimated_time_remaining")?,
        processing_speed: row.try_get("processing_speed")?,
        log_output: row.try_get("log_output")?,
        error_message: row.try_get("error_message")?,
        created_at: ms_to_time(row.try_get("created_at")?),
        started_at: opt_ms_to_time(row.try_get("started_at")?),
        completed_at: opt_ms_to_time(row.try_get("completed_at")?),
        input_file_size: row.try_get("input_file_size")?,
        output_file_size: row.try_get("output_file_size")?,
        batch_id: row.try_get("batch_id")?,
        template_parameters: from_json_map(row.try_get("template_parameters")?),
        retry_of_task_id: row.try_get("retry_of_task_id")?,
        output_path_resolved: row.try_get::<i64, _>("output_path_resolved")? != 0,
        overwrite_existing: row.try_get::<i64, _>("overwrite_existing")? != 0,
    })
}

/// 行 → 任务（投影版：`log_output` 不在 SELECT 里，直接置空串）
fn row_to_task_lite(row: &SqliteRow) -> anyhow::Result<ConversionTask> {
    Ok(ConversionTask {
        id: row.try_get("id")?,
        input_path: row.try_get("input_path")?,
        output_path: row.try_get("output_path")?,
        template_id: row.try_get("template_id")?,
        template_name: row.try_get("template_name")?,
        actual_command: row.try_get("actual_command")?,
        status: TaskStatus::from_int(row.try_get("status")?),
        progress: row.try_get("progress")?,
        total_duration: row.try_get("total_duration")?,
        current_time: row.try_get("current_time")?,
        estimated_time_remaining: row.try_get("estimated_time_remaining")?,
        processing_speed: row.try_get("processing_speed")?,
        log_output: String::new(),
        error_message: row.try_get("error_message")?,
        created_at: ms_to_time(row.try_get("created_at")?),
        started_at: opt_ms_to_time(row.try_get("started_at")?),
        completed_at: opt_ms_to_time(row.try_get("completed_at")?),
        input_file_size: row.try_get("input_file_size")?,
        output_file_size: row.try_get("output_file_size")?,
        batch_id: row.try_get("batch_id")?,
        template_parameters: from_json_map(row.try_get("template_parameters")?),
        retry_of_task_id: row.try_get("retry_of_task_id")?,
        output_path_resolved: row.try_get::<i64, _>("output_path_resolved")? != 0,
        overwrite_existing: row.try_get::<i64, _>("overwrite_existing")? != 0,
    })
}

/// 行 → 批次
fn row_to_batch(row: &SqliteRow) -> anyhow::Result<BatchTask> {
    Ok(BatchTask {
        id: row.try_get("id")?,
        name: row.try_get("name")?,
        template_id: row.try_get("template_id")?,
        total_files: row.try_get("total_files")?,
        completed_files: row.try_get("completed_files")?,
        failed_files: row.try_get("failed_files")?,
        status: TaskStatus::from_int(row.try_get("status")?),
        created_at: ms_to_time(row.try_get("created_at")?),
        completed_at: opt_ms_to_time(row.try_get("completed_at")?),
    })
}

/// 行 → 模板
fn row_to_template(row: &SqliteRow) -> anyhow::Result<CommandTemplate> {
    use crate::models::enums::{HardwareAccelerationMode, TemplateType};

    Ok(CommandTemplate {
        id: row.try_get("id")?,
        name: row.try_get("name")?,
        description: row.try_get("description")?,
        command_args: row.try_get("command_args")?,
        r#type: TemplateType::from_int(row.try_get("type")?),
        category: row.try_get("category")?,
        supported_input_formats: from_json_string_list(row.try_get("supported_input_formats")?),
        output_extension: row.try_get("output_extension")?,
        target_codec: row.try_get("target_codec")?,
        requires_hardware_acceleration: get_bool(row, "requires_hardware_acceleration")?,
        hardware_mode: HardwareAccelerationMode::from_int(row.try_get("hardware_mode")?),
        required_encoder: row.try_get("required_encoder")?,
        parameters: from_json_parameters(row.try_get("parameters")?),
        required_filters: from_json_string_list(row.try_get("required_filters")?),
        tags: from_json_string_list(row.try_get("tags")?),
        is_recommended: get_bool(row, "is_recommended")?,
        preset_version: row.try_get("preset_version")?,
        created_at: ms_to_time(row.try_get("created_at")?),
        updated_at: ms_to_time(row.try_get("updated_at")?),
        sort_order: row.try_get("sort_order")?,
    })
}

/// 行 → 编码器缓存
fn row_to_encoder(row: &SqliteRow) -> anyhow::Result<HardwareEncoder> {
    use crate::ffmpeg::encoder_catalog::EncoderType;

    Ok(HardwareEncoder {
        id: row.try_get("id")?,
        name: row.try_get("name")?,
        display_name: row.try_get("display_name")?,
        encoder_type: EncoderType::from_int(row.try_get("type")?),
        family: row.try_get("family")?,
        codec: row.try_get("codec")?,
        is_available: get_bool(row, "is_available")?,
        supports_quality_options: get_bool(row, "supports_quality_options")?,
        last_checked_at: ms_to_time(row.try_get("last_checked_at")?),
        unavailable_reason: row.try_get("unavailable_reason")?,
        missing_from_build: get_bool(row, "missing_from_build")?,
        ffmpeg_version: row.try_get("ffmpeg_version")?,
    })
}

/// 行 → 全局设置
fn row_to_settings(row: &SqliteRow) -> anyhow::Result<AppSettings> {
    use crate::models::enums::{FileExistsAction, OutputNamingRule};

    Ok(AppSettings {
        id: row.try_get("id")?,
        ffmpeg_path: row.try_get("ffmpeg_path")?,
        ffprobe_path: row.try_get("ffprobe_path")?,
        default_output_directory: row.try_get("default_output_directory")?,
        output_naming: OutputNamingRule::from_int(row.try_get("output_naming")?),
        output_suffix: row.try_get("output_suffix")?,
        file_exists_action: FileExistsAction::from_int(row.try_get("file_exists_action")?),
        prefer_hardware_acceleration: get_bool(row, "prefer_hardware_acceleration")?,
        prefer_hardware_decoding: get_bool(row, "prefer_hardware_decoding")?,
        preferred_hardware_encoder: row.try_get("preferred_hardware_encoder")?,
        auto_fallback_to_software: get_bool(row, "auto_fallback_to_software")?,
        max_concurrent_tasks: row.try_get("max_concurrent_tasks")?,
        max_log_lines: row.try_get("max_log_lines")?,
        task_history_retention_days: row.try_get("task_history_retention_days")?,
        theme: row.try_get("theme")?,
        global_ffmpeg_arguments: row.try_get("global_ffmpeg_arguments")?,
        notify_on_completion: get_bool(row, "notify_on_completion")?,
        auto_open_browser: get_bool(row, "auto_open_browser")?,
        listen_port: row.try_get("listen_port")?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `ms_to_time` / `time_to_ms` 必须无损（毫秒精度是契约）
    #[test]
    fn time_roundtrip_keeps_milliseconds() {
        let now = Utc::now();
        assert_eq!(time_to_ms(ms_to_time(time_to_ms(now))), time_to_ms(now));
        assert_eq!(ms_to_time(0), DateTime::<Utc>::UNIX_EPOCH);
    }

    /// 损坏的 JSON 列不应让整行读失败
    #[test]
    fn json_helpers_are_lenient() {
        assert!(from_json_string_list(None).is_empty());
        assert!(from_json_string_list(Some("not json".into())).is_empty());
        assert!(from_json_string_list(Some("[]".into())).is_empty());
        assert_eq!(
            from_json_string_list(Some("[\"mp4\"]".into())),
            vec!["mp4".to_string()]
        );
        assert!(from_json_map(Some("".into())).is_none());
    }
}
