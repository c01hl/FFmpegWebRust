-- FFmpeg WebUI (Rust) SQLite schema —— 逐字对应 docs/reference/backend-data.md §1.6。
--
-- 约定：
--   * id 统一是 24 位小写十六进制 TEXT（与参考实现的 ObjectId 字符串形式一致，前端无需改格式）；
--   * 时间统一是 Unix 毫秒 UTC 的 INTEGER（与 LiteDB 的 BSON date 精度一致）；
--   * 枚举存 INTEGER（数值即契约）；
--   * 列表/字典列存 TEXT（JSON），空列表存 '[]'。
--
-- 全部语句都是幂等的（IF NOT EXISTS），由 Store::open / Store::open_in_memory 自动执行。

-- 全局设置：单行（id 任意 24hex，服务层保证表内只有一行）
CREATE TABLE IF NOT EXISTS settings (
  id                          TEXT PRIMARY KEY,
  ffmpeg_path                 TEXT    NOT NULL DEFAULT '',
  ffprobe_path                TEXT    NOT NULL DEFAULT '',
  default_output_directory    TEXT    NOT NULL DEFAULT '',
  output_naming               INTEGER NOT NULL DEFAULT 1,   -- OutputNamingRule
  output_suffix               TEXT    NOT NULL DEFAULT '_converted',
  file_exists_action          INTEGER NOT NULL DEFAULT 0,   -- FileExistsAction
  prefer_hardware_acceleration INTEGER NOT NULL DEFAULT 1,
  prefer_hardware_decoding    INTEGER NOT NULL DEFAULT 0,
  preferred_hardware_encoder  TEXT    NOT NULL DEFAULT '',
  auto_fallback_to_software   INTEGER NOT NULL DEFAULT 1,
  max_concurrent_tasks        INTEGER NOT NULL DEFAULT 1,
  max_log_lines               INTEGER NOT NULL DEFAULT 500,
  task_history_retention_days INTEGER NOT NULL DEFAULT 30,
  theme                       TEXT    NOT NULL DEFAULT 'System',
  global_ffmpeg_arguments     TEXT    NOT NULL DEFAULT '',
  notify_on_completion        INTEGER NOT NULL DEFAULT 1,
  auto_open_browser           INTEGER NOT NULL DEFAULT 1,
  listen_port                 INTEGER NOT NULL DEFAULT 5265
);

CREATE TABLE IF NOT EXISTS templates (
  id            TEXT PRIMARY KEY,
  name          TEXT NOT NULL,
  description   TEXT NOT NULL DEFAULT '',
  command_args  TEXT NOT NULL DEFAULT '',
  type          INTEGER NOT NULL DEFAULT 1,
  category      TEXT NOT NULL DEFAULT '',
  supported_input_formats TEXT NOT NULL DEFAULT '[]',  -- JSON 数组
  output_extension        TEXT NOT NULL DEFAULT '',
  target_codec            TEXT NOT NULL DEFAULT '',
  requires_hardware_acceleration INTEGER NOT NULL DEFAULT 0,
  hardware_mode           INTEGER NOT NULL DEFAULT 0,
  required_encoder        TEXT,
  parameters              TEXT NOT NULL DEFAULT '[]',  -- JSON 数组
  required_filters        TEXT NOT NULL DEFAULT '[]',
  tags                    TEXT NOT NULL DEFAULT '[]',
  is_recommended          INTEGER NOT NULL DEFAULT 0,
  preset_version          INTEGER NOT NULL DEFAULT 0,
  created_at              INTEGER NOT NULL,             -- Unix ms UTC
  updated_at              INTEGER NOT NULL,
  sort_order              INTEGER NOT NULL DEFAULT 0
);
CREATE UNIQUE INDEX IF NOT EXISTS ix_templates_name     ON templates(name);
CREATE INDEX        IF NOT EXISTS ix_templates_type     ON templates(type);
CREATE INDEX        IF NOT EXISTS ix_templates_category ON templates(category);

CREATE TABLE IF NOT EXISTS batch_tasks (
  id              TEXT PRIMARY KEY,
  name            TEXT NOT NULL DEFAULT '',
  template_id     TEXT NOT NULL,
  total_files     INTEGER NOT NULL DEFAULT 0,
  completed_files INTEGER NOT NULL DEFAULT 0,
  failed_files    INTEGER NOT NULL DEFAULT 0,
  status          INTEGER NOT NULL DEFAULT 0,
  created_at      INTEGER NOT NULL,
  completed_at    INTEGER
);
CREATE INDEX IF NOT EXISTS ix_batch_status     ON batch_tasks(status);
CREATE INDEX IF NOT EXISTS ix_batch_created_at ON batch_tasks(created_at);

CREATE TABLE IF NOT EXISTS tasks (
  id                    TEXT PRIMARY KEY,
  input_path            TEXT    NOT NULL,
  output_path           TEXT    NOT NULL,
  template_id           TEXT    NOT NULL,
  template_name         TEXT    NOT NULL DEFAULT '',
  actual_command        TEXT    NOT NULL DEFAULT '',
  status                INTEGER NOT NULL DEFAULT 0,
  progress              REAL    NOT NULL DEFAULT 0,
  total_duration        REAL    NOT NULL DEFAULT 0,
  current_time          REAL    NOT NULL DEFAULT 0,
  estimated_time_remaining REAL,
  processing_speed      REAL,
  log_output            TEXT    NOT NULL DEFAULT '',
  error_message         TEXT,
  created_at            INTEGER NOT NULL,
  started_at            INTEGER,
  completed_at          INTEGER,
  input_file_size       INTEGER NOT NULL DEFAULT 0,
  output_file_size      INTEGER,
  batch_id              TEXT,
  template_parameters   TEXT,             -- JSON 对象；NULL = 无
  retry_of_task_id      TEXT,
  output_path_resolved  INTEGER NOT NULL DEFAULT 0,
  overwrite_existing    INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS ix_tasks_status     ON tasks(status);
CREATE INDEX IF NOT EXISTS ix_tasks_created_at ON tasks(created_at);
CREATE INDEX IF NOT EXISTS ix_tasks_batch_id   ON tasks(batch_id);

CREATE TABLE IF NOT EXISTS encoders (
  id            TEXT PRIMARY KEY,
  name          TEXT NOT NULL,
  display_name  TEXT NOT NULL DEFAULT '',
  type          INTEGER NOT NULL DEFAULT 0,
  family        TEXT NOT NULL DEFAULT '',
  codec         TEXT NOT NULL DEFAULT '',
  is_available  INTEGER NOT NULL DEFAULT 0,
  supports_quality_options INTEGER NOT NULL DEFAULT 1,
  supported_codecs TEXT NOT NULL DEFAULT '[]',
  last_checked_at  INTEGER NOT NULL,
  unavailable_reason TEXT,
  missing_from_build INTEGER NOT NULL DEFAULT 0,
  ffmpeg_version TEXT NOT NULL DEFAULT ''
);
CREATE UNIQUE INDEX IF NOT EXISTS ix_encoders_name ON encoders(name);
CREATE INDEX        IF NOT EXISTS ix_encoders_type ON encoders(type);
