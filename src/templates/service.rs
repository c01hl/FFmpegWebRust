//! 模板服务：CRUD、校验、试运行、导入导出、系统模板同步。
//!
//! 规格依据：`docs/reference/templates.md`
//! §1（数据模型 / 规范化 / 唯一名 / 查询排序）、§6（服务契约）、§7（校验清单，逐字消息）、
//! §8（试运行，逐字消息 + 已知缺陷修正）、§9（导入导出）、§10（系统模板同步）。
//!
//! 分层职责（与参考实现一致）：本模块**不**解析命令行（`crate::command`）、
//! **不**执行进程细节（`crate::ffmpeg::service`）、**不**写 SQL（`crate::store`）。
//!
//! # 缓存策略（刻意不做的事）
//!
//! `availability()` / `test()` 都会走 [`HardwareDetector::detect`]，而它自身已经带三层缓存
//! （30 分钟内存报告 + 数据库报告 + 进程内单编码器试跑缓存，§6.4）。因此本服务**不新增**
//! 任何模板级缓存：多一层缓存只会多一个可能过期的地方，而 `list()` 里唯一需要避免的是
//! 「同一份报告被重复 clone 51 次」，那用局部 `HashMap`（推荐编码器去重）解决即可。

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

use anyhow::{anyhow, bail};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use crate::command::builder::BUILT_IN_PLACEHOLDERS;
use crate::command::{CommandContext, MediaSummary, build};
use crate::ffmpeg::encoder_catalog::{EncoderCatalog, EncoderProfile};
use crate::ffmpeg::hardware::{HardwareDetector, HardwareReport};
use crate::ffmpeg::service::{FFmpegService, MediaInfo, RunRequest};
use crate::files::{format_file_size, get_file_size};
use crate::models::dto::{TemplateIssue, TemplateTestResult};
use crate::models::enums::{
    HardwareAccelerationMode, TemplateIssueSeverity, TemplateParameterKind, TemplateType,
};
use crate::models::{CommandTemplate, new_id};
use crate::store::Store;

use super::import_export::{self, ExportItem};
use super::presets::{self, CATEGORY_ORDER, PRESET_VERSION};

/// 试运行临时目录前缀（§8.2 第 4 步：`ffmpegwebui-test-{guid:N}`）
const TEST_DIR_PREFIX: &str = "ffmpegwebui-test";
/// 试运行超时：3 分钟（§8.2 第 9 步）
const TEST_TIMEOUT_SECS: u64 = 180;
/// 测试素材生成超时：60 秒（§8.3）
const SAMPLE_TIMEOUT_SECS: u64 = 60;
/// 测试素材时长（秒）：`testsrc2=duration=3` / `sine=…:duration=3`（§8.3）
const SAMPLE_DURATION_SECS: f64 = 3.0;
/// 分类名缺失时的规范化值（§1.6）
const DEFAULT_CATEGORY: &str = "未分类";
/// 名字缺失时的规范化值（§1.7）
const DEFAULT_NAME: &str = "未命名模板";
/// `CopyTemplateAsync` 的默认新名字后缀（§6.1，注意有空格）
const COPY_SUFFIX: &str = " (副本)";

// ════════════════════════════════════════════════════════════════════════════
// 对外数据结构
// ════════════════════════════════════════════════════════════════════════════

/// 系统模板同步结果。
///
/// 参考实现是 `added/updated/unchanged/preset_version` 四个字段 + `Summary` 计算属性；
/// 本移植把它们都做成字段，方便 API 层直接序列化（`api-contract.md` §1.3）。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[serde(default)]
pub struct SystemSyncResult {
    /// 新增数量
    pub added: i32,
    /// 更新数量
    pub updated: i32,
    /// 未变化数量
    pub unchanged: i32,
    /// 删除的陈旧系统模板数量（参考实现从不删除，见偏差 #14）
    pub removed: i32,
    /// 当前预设版本
    pub preset_version: i32,
    /// 中文摘要（日志与界面共用）
    pub summary: String,
}

impl SystemSyncResult {
    /// 是否发生了任何改变（新增 / 更新 / 删除）
    ///
    /// PORT NOTE（有意偏差 #15）：参考实现的 `Changed` 只算 `Added > 0 || Updated > 0`。
    /// 本移植把「删除了陈旧系统模板」也算改变——否则一次只做清理的同步会报告
    /// 「系统模板已是最新」，与事实不符。文案格式本身仍是参考实现的逐字格式。
    pub fn changed(&self) -> bool {
        self.added > 0 || self.updated > 0 || self.removed > 0
    }

    /// 参考实现的逐字摘要文案（§10.2）
    pub fn summary_text(&self) -> String {
        if self.changed() {
            format!(
                "系统模板已更新：新增 {} 个，更新 {} 个（预设 v{}）",
                self.added, self.updated, self.preset_version
            )
        } else {
            format!("系统模板已是最新（预设 v{}）", self.preset_version)
        }
    }

    /// 填好 `summary` 后返回自身
    fn with_summary(mut self) -> Self {
        self.summary = self.summary_text();
        self
    }
}

/// 模板列表项 = 模板本体 + 服务端计算的可用性字段。
///
/// `#[serde(flatten)]` 让 `CommandTemplate` 的字段出现在 JSON 顶层，
/// 再叠加 `available` / `unavailableReason` / `missingFilters` / `inputFormatsDisplay`
/// （`api-contract.md` §1.3）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TemplateListItem {
    /// 模板本体（展平到顶层）
    #[serde(flatten)]
    pub template: CommandTemplate,
    /// 本机是否可用
    pub available: bool,
    /// 不可用原因（可用时为 `null`）
    pub unavailable_reason: Option<String>,
    /// 缺少的 FFmpeg 滤镜
    pub missing_filters: Vec<String>,
    /// 输入格式展示文本（`任意格式` 或 `mp4 / mkv`）
    pub input_formats_display: String,
}

/// 单条模板的本机可用性（不落库）
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TemplateAvailability {
    pub available: bool,
    /// 不可用原因（中文，可直接展示）
    pub reason: Option<String>,
    /// 缺少的滤镜名列表
    pub missing_filters: Vec<String>,
    /// 是否会用到硬件编码器
    pub uses_hardware: bool,
    /// 本机会为该模板挑选的编码器（不需要编码器画像时为 `None`）
    pub recommended_encoder: Option<String>,
}

/// 分类统计项
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CategoryCount {
    pub name: String,
    pub count: usize,
}

/// 导入结果
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportResult {
    /// 成功导入数量
    pub imported: usize,
    /// 跳过数量（空名/空命令/坏条目）
    ///
    /// PORT NOTE：参考实现对「空名称或空命令」的条目**静默跳过且不计数**（§9.6 第 3 点）。
    /// 本移植把它们计入 `skipped`，因为 API 明确要返回这个字段（`api-contract.md` §1.3）。
    pub skipped: usize,
    /// 逐项错误（坏条目 / 逐项解析失败）
    pub errors: Vec<String>,
}

// ════════════════════════════════════════════════════════════════════════════
// 服务
// ════════════════════════════════════════════════════════════════════════════

/// 模板服务（`Arc` 内部状态，可自由 `Clone` 进 axum 的 AppState）
#[derive(Clone)]
pub struct TemplateService {
    store: Arc<Store>,
    ffmpeg: Arc<FFmpegService>,
    hardware: Arc<HardwareDetector>,
}

/// 计算可用性时的推荐编码器缓存键：`(编码格式, 必需编码器, 严格模式)`
///
/// 同一份硬件报告下这个键的取值完全相同，`list()` 里 51 条模板通常只落到 3~5 个键上。
type EncoderPickCache = HashMap<(String, Option<String>, bool), Option<String>>;

/// 试运行时「要不要挑编码器画像」的三态结果
///
/// 这是 §8.6 缺陷 A 的修正点：参考实现用 `Option<EncoderProfile>` 的 `None` 同时表示
/// 「不需要画像」和「没有可用编码器」，于是所有硬编码软件编码器的模板试运行必然失败。
#[derive(Debug, Clone)]
enum TestEncoder {
    /// 模板自带编码器（软件、命令里没有 `{encoder}`）：**不需要**画像，照常执行
    NotNeeded,
    /// 已解析出的编码器画像
    Profile(EncoderProfile),
    /// 模板要求硬件，但本机确实没有可用编码器 → 试运行失败（可读结论）
    Missing,
}

impl TemplateService {
    pub fn new(
        store: Arc<Store>,
        ffmpeg: Arc<FFmpegService>,
        hardware: Arc<HardwareDetector>,
    ) -> Self {
        Self { store, ffmpeg, hardware }
    }

    /// 数据库句柄（供 API 层做统计/任务查询）
    pub fn store(&self) -> &Arc<Store> {
        &self.store
    }

    // ════════════════════════════════════════════════════════════════════════
    // 系统模板同步
    // ════════════════════════════════════════════════════════════════════════

    /// 启动时调用：按 `PRESET_VERSION` 增量同步内置模板；用户模板原样保留（§10.2）。
    ///
    /// 同步以**稳定 key**（`presets::stable_key`）为身份，而不是 `Name`：
    /// - 库里的行 id 是 `system_template_id(key)` 派生的确定性哈希 → 直接按 id 命中；
    /// - 参考实现（C#）写过的旧库里 id 是随机 `ObjectId` → 按 `Name` 回退命中（认领，不重建）；
    /// - 命中且 `preset_version >= PRESET_VERSION` → `unchanged`（管理员手改在版本不变时保留，§10.4）；
    /// - 命中但版本更旧 → 整条覆盖（保留 `Id`/`CreatedAt`，§10.2）；
    /// - 未命中 → 插入（新预设）。
    ///
    /// 幂等：连续两次调用第二次必然是 `added = 0 && updated = 0`（§6.2 的回归断言）。
    pub async fn initialize_system_templates(&self) -> anyhow::Result<SystemSyncResult> {
        let catalog = presets::all_with_keys();
        let existing = self.system_templates().await?;

        // 按稳定 key 归组（同名多份时取第一条，等价于参考实现的 group-by 取首条）
        let mut by_key: HashMap<String, CommandTemplate> = HashMap::new();
        // 认不出 key 的系统模板：不是当前目录里的预设（旧版本残留 / 预设改名）
        let mut unknown: Vec<CommandTemplate> = Vec::new();
        for row in existing {
            match presets::stable_key(&row) {
                Some(key) => {
                    by_key.entry(key).or_insert(row);
                }
                None => unknown.push(row),
            }
        }

        let (mut added, mut updated, mut unchanged) = (0, 0, 0);
        for (key, preset) in catalog {
            match by_key.remove(&key) {
                Some(current) if current.preset_version >= preset.preset_version => unchanged += 1,
                Some(current) => {
                    let mut next = preset;
                    // 保留 Id：任务/批次里记录的 templateId 不会悬空（§10.2）
                    next.id = current.id;
                    next.created_at = current.created_at;
                    next.updated_at = Utc::now();
                    self.store.update_template(&next).await?;
                    updated += 1;
                }
                None => {
                    self.insert_preset(preset).await?;
                    added += 1;
                }
            }
        }

        // ── 清理陈旧系统模板 ────────────────────────────────────────────────
        //
        // PORT NOTE（有意偏差 #14）：参考实现**从不删除**系统模板，于是预设被移除或改名后
        // 库里会永久残留一条「永远不会再被更新」的陈旧模板（§10.3）。本移植在同步末尾删除
        // 「Type == System 且认不出 key」的条目——也就是既有系统模板集合里、当前目录中
        // 不存在的那部分。用户模板（Type == User）一律不受影响。
        //
        // 代价（已确认可接受）：管理员通过 `import_json(as_system = true)` 塞进来的系统模板
        // 也会被下一次同步删除——系统模板的所有权归预设目录，需要自定义请用用户模板。
        let mut removed = 0;
        for row in unknown {
            self.store.delete_template(&row.id).await?;
            removed += 1;
        }
        // 同一 key 出现多行时，多余的那些也清掉（正常情况下 name 的唯一索引不会允许）
        for (_, row) in by_key {
            self.store.delete_template(&row.id).await?;
            removed += 1;
        }

        Ok(SystemSyncResult {
            added,
            updated,
            unchanged,
            removed,
            preset_version: PRESET_VERSION,
            summary: String::new(),
        }
        .with_summary())
    }

    /// 重置为出厂预设（用户模板保留，§10.5）。
    ///
    /// PORT NOTE（有意偏差 #16）：参考实现的重置会**更换所有系统模板的 Id**，
    /// 历史任务里的模板引用因此悬空（§10.5 缺陷 F）。本移植的 id 由 key 确定性派生
    /// （`system_template_id`），因此「删掉再插回」得到的 id 与原来完全一致，
    /// 引用不会失效，也不需要额外做「按 key 原地重建」。
    pub async fn reset_system_templates(&self) -> anyhow::Result<SystemSyncResult> {
        let removed = self.store.delete_system_templates().await? as i32;

        let mut added = 0;
        for (_key, preset) in presets::all_with_keys() {
            self.insert_preset(preset).await?;
            added += 1;
        }

        Ok(SystemSyncResult {
            added,
            updated: 0,
            unchanged: 0,
            removed,
            preset_version: PRESET_VERSION,
            summary: String::new(),
        }
        .with_summary())
    }

    /// 插入一条预设。
    ///
    /// PORT NOTE（有意偏差 #17）：`templates.name` 有全局唯一索引，而「用户先建了同名模板、
    /// 之后才启动同步」是可能的。参考实现会直接抛唯一索引冲突并让启动同步整体失败；
    /// 这里给预设名加 `(2)`/`(3)` 后缀（**只改系统模板的名字，绝不触碰用户模板**），
    /// 保证同步一定能完成。预设的 id 是确定性派生 的，下一次同步按 id 命中，**不会重复插入**。
    async fn insert_preset(&self, mut preset: CommandTemplate) -> anyhow::Result<()> {
        preset.name = self.ensure_unique_name(Some(&preset.name), None).await?;
        self.store.insert_template(&preset).await
    }

    /// 库中全部系统模板
    async fn system_templates(&self) -> anyhow::Result<Vec<CommandTemplate>> {
        Ok(self
            .store
            .list_templates(true)
            .await?
            .into_iter()
            .filter(CommandTemplate::is_system)
            .collect())
    }

    // ════════════════════════════════════════════════════════════════════════
    // 查询
    // ════════════════════════════════════════════════════════════════════════

    /// 列表（排序：推荐 → 分类序 → `sort_order` → 名称，§1.8），每项附可用性字段。
    pub async fn list(&self, include_system: bool) -> anyhow::Result<Vec<TemplateListItem>> {
        let mut templates = self.store.list_templates(include_system).await?;
        sort_templates(&mut templates);

        // 报告只取一次（`detect(false)` 自带 30 分钟缓存），推荐编码器按 (codec, required, strict) 去重
        let report = self.hardware.detect(false).await;
        let mut cache: EncoderPickCache = HashMap::new();

        let mut items = Vec::with_capacity(templates.len());
        for template in templates {
            let input_formats_display = template.input_formats_display();
            let availability = self.compute_availability(&template, &report, &mut cache).await;
            items.push(TemplateListItem {
                available: availability.available,
                unavailable_reason: availability.reason,
                missing_filters: availability.missing_filters,
                input_formats_display,
                template,
            });
        }
        Ok(items)
    }

    /// 按 id 取单条
    pub async fn get(&self, id: &str) -> anyhow::Result<Option<CommandTemplate>> {
        self.store.get_template(id).await
    }

    /// 按名称精确查找（与参考实现一致：区分大小写、区分全半角）
    pub async fn find_by_name(&self, name: &str) -> anyhow::Result<Option<CommandTemplate>> {
        self.store.get_template_by_name(name.trim()).await
    }

    /// 分类统计（排序：预设分类顺序 → 名称，§1.8）
    pub async fn categories(&self) -> anyhow::Result<Vec<CategoryCount>> {
        let templates = self.store.list_templates(true).await?;
        let mut counts: HashMap<String, usize> = HashMap::new();
        for template in templates {
            let category = template.category.trim().to_string();
            if category.is_empty() {
                continue;
            }
            *counts.entry(category).or_insert(0) += 1;
        }

        let mut out: Vec<CategoryCount> =
            counts.into_iter().map(|(name, count)| CategoryCount { name, count }).collect();
        out.sort_by(|a, b| {
            category_rank(&a.name).cmp(&category_rank(&b.name)).then_with(|| a.name.cmp(&b.name))
        });
        Ok(out)
    }

    // ════════════════════════════════════════════════════════════════════════
    // 写入（创建 / 更新 / 删除 / 复制）
    // ════════════════════════════════════════════════════════════════════════

    /// 创建用户模板。
    ///
    /// PORT NOTE（有意偏差 #18）：参考实现的 `CreateTemplateAsync` 对重名**自动改名**
    /// （`EnsureUniqueName` → `名字 (2)`）。本移植改成**报错**，因为 API 契约把重名映射为
    /// HTTP 409（`api-contract.md` §1.3），静默改名会让用户以为自己填的名字生效了。
    /// `copy` / `import` 仍按参考实现自动改名（那两处「重名」是预期内的）。
    pub async fn create(&self, mut template: CommandTemplate) -> anyhow::Result<CommandTemplate> {
        normalize_template(&mut template);
        if template.name.is_empty() {
            bail!("模板名称不能为空");
        }
        if self.store.get_template_by_name(&template.name).await?.is_some() {
            bail!("模板名称「{}」已存在，请换一个名称。", template.name);
        }

        template.id = new_id();
        template.r#type = TemplateType::User;
        let now = Utc::now();
        template.created_at = now;
        template.updated_at = now;

        self.store.insert_template(&template).await?;
        Ok(template)
    }

    /// 更新**用户**模板。
    ///
    /// PORT NOTE（有意偏差 #19）：参考实现的 `UpdateTemplateAsync` 对系统模板返回 `false`
    /// （以及重名时自动改名）。本移植对「系统模板」「重名」都返回可读错误，让 API 层映射成
    /// 403 / 409，调用方不会被静默忽略。
    pub async fn update(&self, mut template: CommandTemplate) -> anyhow::Result<CommandTemplate> {
        let existing = self
            .store
            .get_template(&template.id)
            .await?
            .ok_or_else(|| anyhow!("模板不存在或已被删除"))?;
        if existing.is_system() {
            bail!("系统模板不可修改，请先复制为自定义模板。");
        }

        normalize_template(&mut template);
        if template.name.is_empty() {
            bail!("模板名称不能为空");
        }
        if let Some(other) = self.store.get_template_by_name(&template.name).await? {
            if other.id != template.id {
                bail!("模板名称「{}」已存在，请换一个名称。", template.name);
            }
        }

        template.r#type = TemplateType::User;
        template.created_at = existing.created_at; // 保留创建时间（§6.1）
        template.updated_at = Utc::now();
        self.store.update_template(&template).await?;
        Ok(template)
    }

    /// 删除用户模板；系统模板拒绝删除（API 映射成 403）；不存在返回 `false`
    pub async fn delete(&self, id: &str) -> anyhow::Result<bool> {
        let Some(existing) = self.store.get_template(id).await? else {
            return Ok(false);
        };
        if existing.is_system() {
            bail!("系统模板不可删除，请改用复制后编辑副本。");
        }
        self.store.delete_template(id).await
    }

    /// 复制模板（源可以是系统模板）为新的用户模板（§6.1）。
    ///
    /// 与参考实现一致：新 id、`Type = User`、`IsRecommended = false`、`PresetVersion = 0`、
    /// `SortOrder = 源 + 1`、深拷贝参数（Rust 的 `Vec::clone` 天然是深拷贝）、自动唯一名。
    ///
    /// PORT NOTE（有意偏差 #20）：参考实现的复制**没有**复制 `RequiredFilters`（§6.1 源码
    /// `144-165` 未赋值），副本因此丢掉「依赖 subtitles 滤镜」这类信息，会在缺滤镜的 FFmpeg
    /// 上显示为可用。本移植把 `RequiredFilters` 一起复制。
    pub async fn copy(&self, id: &str, new_name: Option<String>) -> anyhow::Result<CommandTemplate> {
        let source = self
            .store
            .get_template(id)
            .await?
            .ok_or_else(|| anyhow!("源模板不存在"))?;

        let requested = match new_name {
            Some(name) if !name.trim().is_empty() => name,
            _ => format!("{}{}", source.name, COPY_SUFFIX),
        };

        let mut copy = source.clone();
        copy.id = new_id();
        copy.r#type = TemplateType::User;
        copy.name = self.ensure_unique_name(Some(&requested), None).await?;
        copy.is_recommended = false;
        copy.preset_version = 0;
        copy.sort_order = source.sort_order + 1;
        copy.parameters = source.parameters.clone();
        copy.required_filters = source.required_filters.clone();
        let now = Utc::now();
        copy.created_at = now;
        copy.updated_at = now;
        normalize_template(&mut copy);

        self.store.insert_template(&copy).await?;
        Ok(copy)
    }

    // ════════════════════════════════════════════════════════════════════════
    // 校验
    // ════════════════════════════════════════════════════════════════════════

    /// 模板校验：§7 的 18 条检查（顺序即返回顺序，消息逐字）。
    ///
    /// `available_filters` 是当前 FFmpeg 构建的 `-filters` 清单：
    /// - `None` 或空列表 → 「滤镜清单未知」→ **不报**缺滤镜（宁可漏报也不误报，§6.4/§7.1）；
    /// - 非空 → 追加一条缺少必需滤镜的错误。
    ///
    /// PORT NOTE（有意偏差 #21）：参考实现的 `Validate` **不检查滤镜**（那是 `GetMissingFilters`
    /// 的事，只在试运行与列表里用）。这里按交付要求把「缺少必需滤镜」也纳入校验结果
    /// （追加在 §7 的 18 条之后，`field = RequiredFilters`，严重级 Error，消息与试运行/列表同款），
    /// 这样 `POST /api/templates/validate` 一次就能告诉前端「这条模板在这台机器上跑不了」。
    ///
    /// 校验**不做**的事（§7 末）：不检查编码器是否存在、不检查参数取值范围 / 选项合法性、
    /// 不检查 `SortOrder` / `Tags`。
    pub fn validate(
        &self,
        template: &CommandTemplate,
        available_filters: Option<&[String]>,
    ) -> Vec<TemplateIssue> {
        let mut issues: Vec<TemplateIssue> = Vec::new();

        // 1) 名称
        if is_blank(&template.name) {
            issues.push(TemplateIssue::error("模板名称不能为空", Some("Name")));
        }

        // 2) 命令参数；成立时立即返回
        if is_blank(&template.command_args) {
            issues.push(TemplateIssue::error("命令参数不能为空", Some("CommandArgs")));
            return issues;
        }

        // 3) 输出扩展名
        if is_blank(&template.output_extension) {
            issues.push(TemplateIssue::error(
                "必须指定输出扩展名，否则无法生成输出文件名",
                Some("OutputExtension"),
            ));
        }

        let placeholders = crate::command::extract_placeholders(&template.command_args);
        let has_placeholder = |name: &str| placeholders.iter().any(|p| p.eq_ignore_ascii_case(name));

        // 4) 必须读输入
        if !has_placeholder("input") {
            issues.push(TemplateIssue::error(
                "命令中缺少 {input} 占位符，FFmpeg 将无法读取输入文件",
                Some("CommandArgs"),
            ));
        }

        // 5) 必须写输出
        if !has_placeholder("output") && !has_placeholder("output_pattern") {
            issues.push(TemplateIssue::error(
                "命令中缺少 {output}（或用于图片序列的 {output_pattern}）占位符，转换结果没有输出目标",
                Some("CommandArgs"),
            ));
        }

        // 6) 未声明的占位符（每个各一条）
        for placeholder in &placeholders {
            let declared = template
                .parameters
                .iter()
                .any(|p| !is_blank(&p.name) && p.name.eq_ignore_ascii_case(placeholder));
            if !is_builtin_placeholder(placeholder) && !declared {
                issues.push(TemplateIssue::error(
                    format!(
                        "占位符 {{{placeholder}}} 没有对应取值：请在上方『参数』中声明它，或改用内置占位符"
                    ),
                    Some("Parameters"),
                ));
            }
        }

        // 7) 双引号数量
        if template.command_args.matches('"').count() % 2 == 1 {
            issues.push(TemplateIssue::error(
                "双引号数量为奇数，命令可能被错误拆分。含空格的路径请写成 \"{input}\"",
                Some("CommandArgs"),
            ));
        }

        // 8) / 9) 硬件模板：`-crf` 误用、编码器名认不出
        let required = template
            .required_encoder
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty());
        let is_hardware_template = template.hardware_mode == HardwareAccelerationMode::Required
            || required.map(|value| !is_software_family(value)).unwrap_or(false);
        if is_hardware_template {
            if let Some(required) = required {
                let has_hardware_profile = EncoderCatalog::expand_family(required)
                    .iter()
                    .any(|member| {
                        EncoderCatalog::find(member).map(EncoderProfile::is_hardware).unwrap_or(false)
                    });
                if has_hardware_profile {
                    if contains_ci(&template.command_args, "-crf") {
                        issues.push(TemplateIssue::warning(
                            "硬件编码器不支持 -crf，该选项会被 FFmpeg 忽略。建议改用 {quality_args} 占位符，它会自动展开成当前编码器正确的质量参数。",
                            Some("CommandArgs"),
                        ));
                    }
                } else if EncoderCatalog::find(required).is_none() {
                    issues.push(TemplateIssue::warning(
                        format!(
                            "无法识别的编码器「{required}」。可用族名：nvenc / qsv / amf / videotoolbox / vaapi / vulkan，或具体编码器名如 h264_nvenc。"
                        ),
                        Some("RequiredEncoder"),
                    ));
                }
            }
        }

        // 10) Required 但没写编码器族
        if template.hardware_mode == HardwareAccelerationMode::Required
            && required.is_none()
        {
            issues.push(TemplateIssue::error(
                "模板要求必须使用硬件加速，但没有指定编码器族。这样在任何机器上都会失败。",
                Some("RequiredEncoder"),
            ));
        }

        // 11) 硬件模板没用 `{encoder}`
        if template.hardware_mode != HardwareAccelerationMode::None && !has_placeholder("encoder") {
            issues.push(TemplateIssue::warning(
                "硬件加速模板建议使用 {encoder} 占位符，这样在本机没有该厂商硬件时会自动回退到可用的编码器。",
                Some("CommandArgs"),
            ));
        }

        // 12) / 13) 复制流模式
        let copy_mode = contains_ci(&template.command_args, "-c copy")
            || contains_ci(&template.command_args, "-c:v copy")
            || contains_ci(&template.command_args, "-codec copy");
        if copy_mode {
            issues.push(TemplateIssue::info(
                "「复制流」模式下画面不会被重新编码。如果源文件的编码格式与目标容器不兼容，转换会失败——这正是「快速换容器」模板的已知限制。",
                None,
            ));
            if contains_ci(&template.command_args, "-vf")
                || contains_ci(&template.command_args, "-filter_complex")
            {
                issues.push(TemplateIssue::error(
                    "「-c copy」不能与滤镜同时使用：不重新编码就无法应用滤镜。",
                    Some("CommandArgs"),
                ));
            }
        }

        // 14) ~ 16) 逐参数检查
        for parameter in &template.parameters {
            if is_blank(&parameter.name) {
                issues.push(TemplateIssue::warning(
                    "存在一个没有名字的参数，它不会被使用。",
                    Some("Parameters"),
                ));
                continue;
            }
            if is_builtin_placeholder(&parameter.name)
                && !matches!(
                    parameter.name.to_ascii_lowercase().as_str(),
                    "quality" | "q" | "crf"
                )
            {
                issues.push(TemplateIssue::warning(
                    format!("参数名「{}」与内置占位符同名，内置值可能被覆盖。建议改名。", parameter.name),
                    Some("Parameters"),
                ));
            }
            if parameter.kind == TemplateParameterKind::Select && parameter.options.is_empty() {
                let label = if is_blank(&parameter.label) {
                    parameter.name.clone()
                } else {
                    parameter.label.clone()
                };
                issues.push(TemplateIssue::warning(
                    format!("参数「{label}」是下拉选项但没有配置任何选项。"),
                    Some("Parameters"),
                ));
            }
        }

        // 17) `{quality_args}` 但没有 `{encoder}`
        if !has_placeholder("encoder") && has_placeholder("quality_args") {
            issues.push(TemplateIssue::warning(
                "使用了 {quality_args} 但没有 {encoder}：质量参数将按默认编码器展开。",
                Some("CommandArgs"),
            ));
        }

        // 18) 依赖的滤镜清单（总是提示一次）
        if !template.required_filters.is_empty() {
            issues.push(TemplateIssue::info(
                format!(
                    "该模板依赖以下 FFmpeg 滤镜：{}。如果当前 FFmpeg 构建没有编译它们，模板会显示为不可用。",
                    template.required_filters.join("、")
                ),
                Some("RequiredFilters"),
            ));
        }

        // 19) 本机缺滤镜（见本方法文档里的 PORT NOTE）
        let missing = missing_filters(template, available_filters);
        if !missing.is_empty() {
            issues.push(TemplateIssue::error(missing_filters_message(&missing), Some("RequiredFilters")));
        }

        issues
    }

    /// 参数默认值字典（§2.3）：空名参数不进字典
    pub fn default_values(template: &CommandTemplate) -> HashMap<String, String> {
        let mut values = HashMap::new();
        for parameter in &template.parameters {
            if !is_blank(&parameter.name) {
                values.insert(parameter.name.clone(), parameter.default_value.clone());
            }
        }
        values
    }

    // ════════════════════════════════════════════════════════════════════════
    // 试运行
    // ════════════════════════════════════════════════════════════════════════

    /// 试运行：生成 3 秒测试素材 → 用模板真正跑一遍 → 删除临时目录（§8）。
    ///
    /// 失败路径的消息、`command`、`output`、耗时全部按 §8.2 的表格取值。
    pub async fn test(
        &self,
        template: &CommandTemplate,
        values: &HashMap<String, String>,
    ) -> anyhow::Result<TemplateTestResult> {
        // 1) 滤镜缺失（先于校验判断，与 §8.2 的顺序一致；报告里没有滤镜清单 → 不判定缺失，§7.1）
        let report = self.hardware.detect(false).await;
        let missing = missing_filters_against_report(template, &report);
        if !missing.is_empty() {
            return Ok(failed_test(missing_filters_message(&missing), &template.command_args));
        }

        // 2) 校验：任何 Error 都直接失败（不生成素材、不跑 FFmpeg）
        let issues = self.validate(template, None);
        let errors: Vec<&TemplateIssue> =
            issues.iter().filter(|i| i.severity == TemplateIssueSeverity::Error).collect();
        if !errors.is_empty() {
            let joined = errors.iter().map(|e| e.message.as_str()).collect::<Vec<_>>().join("；");
            return Ok(failed_test(
                format!("模板校验未通过：{joined}"),
                &template.command_args,
            ));
        }

        // 3) FFmpeg 可用性
        if self.ffmpeg.info(false).await.filter(|info| info.available).is_none() {
            return Ok(failed_test(
                "FFmpeg 不可用，无法试运行。请先在设置中指定 FFmpeg 路径。",
                &template.command_args,
            ));
        }

        // 4) 临时目录 + 5) 素材 → 执行 → 6) 清理（无论成败都删）
        let work_dir = crate::paths::temp_path(TEST_DIR_PREFIX, "");
        if !crate::paths::ensure_directory(&work_dir) {
            return Ok(failed_test(
                "无法生成测试素材，请检查 FFmpeg 是否完整。",
                &template.command_args,
            ));
        }
        let result = self.run_trial(template, values, &work_dir).await;
        let _ = std::fs::remove_dir_all(&work_dir);
        Ok(result)
    }

    /// 试运行的主体（素材已生成、临时目录已就绪）
    async fn run_trial(
        &self,
        template: &CommandTemplate,
        values: &HashMap<String, String>,
        work_dir: &Path,
    ) -> TemplateTestResult {
        let extension = if is_blank(&template.output_extension) {
            "mp4".to_string()
        } else {
            template.output_extension.trim().trim_start_matches('.').to_string()
        };
        let input_path = work_dir.join("sample.mp4");
        let output_path = work_dir.join(format!("result.{extension}"));
        let input_text = input_path.to_string_lossy().to_string();
        let output_text = output_path.to_string_lossy().to_string();

        // 5) 生成测试素材（逐字命令见 §8.3）
        if !self.generate_sample(&input_path).await {
            return failed_test("无法生成测试素材，请检查 FFmpeg 是否完整。", &template.command_args);
        }

        // 6) 参数：默认值 → 覆盖入参 → 路径类参数替换为测试素材
        let mut parameters = Self::default_values(template);
        for (key, value) in values {
            parameters.insert(key.clone(), value.clone());
        }
        substitute_required_paths(template, &mut parameters, &input_path);

        // 7) 媒体信息 + 编码器解析
        let media = self.ffmpeg.probe_media(&input_text).await.ok().map(|info| media_summary(&info));
        let encoder = match self.resolve_test_encoder(template).await {
            TestEncoder::NotNeeded => None,
            TestEncoder::Profile(profile) => Some(profile),
            TestEncoder::Missing => {
                return failed_test(
                    format!(
                        "❌ 该模板要求使用 {} 硬件编码，但本机没有可用的对应编码器。请改用「推荐」分类下会自动挑选编码器的模板。",
                        family_display_name(template.required_encoder.as_deref())
                    ),
                    &template.command_args,
                );
            }
        };

        // 8) 构建命令（试运行不注入 Threads / 硬件解码 / 全局参数，§8.2）
        let mut context = CommandContext::new(input_text, output_text);
        context.parameters = parameters;
        context.encoder = encoder.clone();
        context.media = media.clone();
        context.audio_encoder = "aac".to_string();
        let built = build(template, &context);
        let command_text = format!("ffmpeg -hide_banner -y {}", built.command_line);

        // 9) 执行：3 分钟超时、覆盖输出
        let duration = media.as_ref().map(|m| m.duration).unwrap_or(0.0);
        let started = Instant::now();
        let outcome = self
            .ffmpeg
            .run(
                RunRequest::new(built.arguments.clone())
                    .with_duration_hint(duration)
                    .with_overwrite(true)
                    .with_timeout(Some(TEST_TIMEOUT_SECS)),
                None,
                CancellationToken::new(),
            )
            .await;
        let duration_ms = started.elapsed().as_millis() as u64;

        let outcome = match outcome {
            Ok(outcome) => outcome,
            Err(error) => {
                return TemplateTestResult {
                    success: false,
                    message: format!("❌ 模板试运行失败：{error}"),
                    command: command_text,
                    output: String::new(),
                    duration_ms,
                };
            }
        };

        // 10) 判定：进程成功 **且** 输出文件非空
        let output_size = get_file_size(&output_path.to_string_lossy());
        if outcome.success && output_size > 0 {
            let suffix = match &encoder {
                Some(profile) if profile.is_hardware() => {
                    format!("（使用硬件编码器 {}）", profile.name)
                }
                Some(profile) => format!("（使用编码器 {}）", profile.name),
                None => String::new(),
            };
            return TemplateTestResult {
                success: true,
                message: format!("✅ 模板试运行成功{suffix}，输出 {}。", format_file_size(output_size)),
                command: command_text,
                output: outcome.stderr_tail,
                duration_ms,
            };
        }

        let reason = if outcome.cancelled {
            "试运行被取消".to_string()
        } else if outcome.timed_out {
            format!("试运行超时（超过 {} 分钟）", TEST_TIMEOUT_SECS / 60)
        } else {
            match outcome.exit_code {
                Some(code) => format!("FFmpeg 退出码 {code}"),
                None => "FFmpeg 进程被信号终止".to_string(),
            }
        };
        TemplateTestResult {
            success: false,
            message: format!("❌ 模板试运行失败：{reason}"),
            command: command_text,
            output: outcome.stderr_tail,
            duration_ms,
        }
    }

    /// 生成测试素材（§8.3 的逐字命令），成功判定 = 退出码 0 且文件存在
    async fn generate_sample(&self, input_path: &Path) -> bool {
        let mut arguments: Vec<String> = [
            "-f",
            "lavfi",
            "-i",
            "testsrc2=duration=3:size=640x360:rate=25",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=440:duration=3",
            "-c:v",
            "libx264",
            "-preset",
            "ultrafast",
            "-pix_fmt",
            "yuv420p",
            "-c:a",
            "aac",
            "-shortest",
        ]
        .iter()
        .map(|token| token.to_string())
        .collect();
        arguments.push(input_path.to_string_lossy().to_string());

        let outcome = self
            .ffmpeg
            .run(
                RunRequest::new(arguments)
                    .with_duration_hint(SAMPLE_DURATION_SECS)
                    .with_overwrite(true)
                    .with_timeout(Some(SAMPLE_TIMEOUT_SECS)),
                None,
                CancellationToken::new(),
            )
            .await;

        match outcome {
            Ok(outcome) => outcome.exit_code == Some(0) && input_path.exists(),
            Err(_) => false,
        }
    }

    /// 试运行挑编码器（§8.4 + 缺陷 A 的修正）。
    ///
    /// 目标编码格式只有两步：`TargetCodec` → `NormalizeCodec(OutputExtension)` → 兜底 `h264`。
    async fn resolve_test_encoder(&self, template: &CommandTemplate) -> TestEncoder {
        // ★ §8.6 缺陷 A 的修正点：不需要画像的模板返回 `NotNeeded`（继续执行），
        //   而不是像参考实现那样返回 `None` 并被上层解释成「本机缺少硬件编码器」。
        if !needs_encoder_profile(template) {
            return TestEncoder::NotNeeded;
        }

        let codec = test_target_codec(template);
        let strict = template.hardware_mode == HardwareAccelerationMode::Required;
        let required = template
            .required_encoder
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty());

        let Some(name) = self.hardware.recommended_encoder_with(&codec, required, strict).await
        else {
            return TestEncoder::Missing;
        };

        let profile = EncoderCatalog::resolve_profile(&name)
            .or_else(|| EncoderCatalog::software_encoder_for(&codec))
            .or_else(|| EncoderCatalog::find("libx264"));
        match profile {
            Some(profile) => TestEncoder::Profile(profile.clone()),
            // 理论上不可达（兜底链的最后一段一定存在）；不 unwrap，退化成「没有可用编码器」
            None => TestEncoder::Missing,
        }
    }

    // ════════════════════════════════════════════════════════════════════════
    // 导入 / 导出
    // ════════════════════════════════════════════════════════════════════════

    /// 导出为参考实现格式的 JSON（§9；**有损**，丢掉的字段见 `import_export` 模块文档）。
    ///
    /// 刻意**不**扩展 schema（不加 `Id`/`RequiredFilters`/`IsRecommended` 等键），
    /// 以保证生成的文件仍能被参考实现（C#）原样导入。
    pub fn export_json(&self, templates: &[CommandTemplate]) -> serde_json::Value {
        let document = import_export::export_document(
            templates,
            env!("CARGO_PKG_VERSION"),
            PRESET_VERSION,
        );
        serde_json::to_value(document).unwrap_or(serde_json::Value::Null)
    }

    /// 导入参考实现或本应用导出的 JSON（§9.6）。
    ///
    /// * 顶层键名大小写敏感（`Templates`）：小写 `templates` → 导入 0 个且不报错；
    /// * 每条都分配新 id、分类空白 → `导入`、名字冲突自动加 `(2)`/`(3)`；
    /// * `as_system = true` → `Type = System` + 当前预设版本；
    /// * 空名称 / 空命令的条目跳过（计入 `skipped`）。
    pub async fn import_json(
        &self,
        value: &serde_json::Value,
        as_system: bool,
    ) -> anyhow::Result<ImportResult> {
        let raw_items = import_export::document_items(value)
            .map_err(|reason| anyhow!("模板文件不是有效的 JSON：{reason}"))?;

        let mut result = ImportResult::default();
        for (index, raw) in raw_items.into_iter().enumerate() {
            let item: ExportItem = match import_export::item_from_value(&raw) {
                Ok(item) => item,
                Err(reason) => {
                    result.skipped += 1;
                    result.errors.push(format!("第 {} 项导入失败：{reason}", index + 1));
                    continue;
                }
            };

            // 参考实现的逐项跳过规则：只检查 Name / CommandArgs（其余字段全部容错）
            if is_blank(&item.name) || is_blank(&item.command_args) {
                result.skipped += 1;
                continue;
            }

            let mut template = item.into_template(as_system, PRESET_VERSION, Utc::now());
            normalize_template(&mut template);
            template.name = self.ensure_unique_name(Some(&template.name), None).await?;
            self.store.insert_template(&template).await?;
            result.imported += 1;
        }

        Ok(result)
    }

    // ════════════════════════════════════════════════════════════════════════
    // 可用性
    // ════════════════════════════════════════════════════════════════════════

    /// 本机可用性（列表项与预览用）。
    ///
    /// 规则（按顺序）：
    /// 1. 缺少必需滤镜 → 不可用（消息与试运行同款）；
    /// 2. `HardwareMode == Required` 但没写编码器族 → 不可用（这是模板自身的错误，任何机器都跑不了）；
    /// 3. `Required` 且本机 `pick_required` 挑不出该族的可用编码器 → 不可用；
    /// 4. 其余一律**可用**——包括硬编码软件编码器的纯软件模板，以及 `Auto` / `None` 模式
    ///    会自动回退到软件编码器的模板（§8.6 缺陷 A 的同类问题：这里不能把「不需要硬件画像」
    ///    误判成「硬件不可用」）。
    pub async fn availability(&self, template: &CommandTemplate) -> TemplateAvailability {
        let report = self.hardware.detect(false).await;
        let mut cache = EncoderPickCache::new();
        self.compute_availability(template, &report, &mut cache).await
    }

    /// 可用性计算（复用已取到的报告与推荐编码器缓存）
    async fn compute_availability(
        &self,
        template: &CommandTemplate,
        report: &HardwareReport,
        cache: &mut EncoderPickCache,
    ) -> TemplateAvailability {
        let missing_filters = missing_filters_against_report(template, report);

        // 是否需要编码器画像：不需要时 `recommended_encoder` 为空，且**绝不能**因此判不可用
        let needs_profile = needs_encoder_profile(template);
        let codec = test_target_codec(template);
        let required = template
            .required_encoder
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string);
        let strict = template.hardware_mode == HardwareAccelerationMode::Required;

        let recommended_encoder = if needs_profile {
            let key = (codec.clone(), required.clone(), strict);
            match cache.get(&key) {
                Some(picked) => picked.clone(),
                None => {
                    let picked =
                        self.hardware.recommended_encoder_with(&codec, required.as_deref(), strict).await;
                    cache.insert(key, picked.clone());
                    picked
                }
            }
        } else {
            None
        };

        let uses_hardware = template.hardware_mode != HardwareAccelerationMode::None
            || required
                .as_deref()
                .map(|value| {
                    EncoderCatalog::expand_family(value)
                        .iter()
                        .any(|member| {
                            EncoderCatalog::find(member)
                                .map(EncoderProfile::is_hardware)
                                .unwrap_or(false)
                        })
                })
                .unwrap_or(false)
            || recommended_encoder
                .as_deref()
                .and_then(EncoderCatalog::resolve_profile)
                .map(EncoderProfile::is_hardware)
                .unwrap_or(false);

        let mut availability = TemplateAvailability {
            available: true,
            reason: None,
            missing_filters: missing_filters.clone(),
            uses_hardware,
            recommended_encoder,
        };

        if !missing_filters.is_empty() {
            availability.available = false;
            availability.reason = Some(missing_filters_message(&missing_filters));
            return availability;
        }

        if template.hardware_mode == HardwareAccelerationMode::Required {
            let Some(required) = required.as_deref() else {
                availability.available = false;
                availability.reason = Some(
                    "模板要求必须使用硬件加速，但没有指定编码器族。这样在任何机器上都会失败。"
                        .to_string(),
                );
                return availability;
            };
            if availability.recommended_encoder.is_none() {
                availability.available = false;
                availability.reason = Some(format!(
                    "该模板要求使用 {} 硬件编码，但本机不可用。请改用「推荐」分类下会自动挑选编码器的模板，或在「设置 → 硬件加速能力」中确认本机支持的编码器。",
                    family_display_name(Some(required))
                ));
            }
        }

        availability
    }
}

// ════════════════════════════════════════════════════════════════════════════
// 纯函数（规范化 / 唯一名 / 排序 / 各类判定）
// ════════════════════════════════════════════════════════════════════════════

/// 是否空白（C# `string.IsNullOrWhiteSpace`）
fn is_blank(value: &str) -> bool {
    value.trim().is_empty()
}

/// 大小写不敏感包含（C# `Contains(..., OrdinalIgnoreCase)`）
fn contains_ci(haystack: &str, needle_lowercase: &str) -> bool {
    haystack.to_lowercase().contains(needle_lowercase)
}

/// 是否为内置占位符（大小写不敏感，§3.3 的 37 个）
fn is_builtin_placeholder(name: &str) -> bool {
    BUILT_IN_PLACEHOLDERS.iter().any(|builtin| builtin.eq_ignore_ascii_case(name.trim()))
}

/// 是否属于软件编码族（`required_encoder` 的「硬件模板」判定用，§7 第 8/9 条）
fn is_software_family(name: &str) -> bool {
    let trimmed = name.trim();
    if trimmed.eq_ignore_ascii_case("software") {
        return true;
    }
    EncoderCatalog::find(trimmed).map(|profile| !profile.is_hardware()).unwrap_or(false)
}

/// 保序去重
fn dedup_preserve_order(values: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for value in values {
        if !out.contains(&value) {
            out.push(value);
        }
    }
    out
}

/// `NormalizeTemplate`（§1.6）：**所有写路径**的入口都调用一次。
///
/// 注意两点刻意行为：
/// 1. 源码里那段「`Required` 但没写编码器族」的空 `if` 被保留为「不做静默修正」，
///    留给 [`TemplateService::validate`] 报错；
/// 2. 参数名内部的空格不会被去掉（`"a b"` 保持 `"a b"`），它永远匹配不到 `{...}` 正则，
///    成为「死参数」——校验只在参数名**全空白**时报 Warning。
pub fn normalize_template(template: &mut CommandTemplate) {
    template.name = template.name.trim().to_string();
    template.description = template.description.trim().to_string();
    template.command_args = template.command_args.trim().to_string();

    template.category = if is_blank(&template.category) {
        DEFAULT_CATEGORY.to_string()
    } else {
        template.category.trim().to_string()
    };

    // 扩展名：Trim → 去前导 '.' → 小写
    template.output_extension =
        template.output_extension.trim().trim_start_matches('.').to_lowercase();

    template.target_codec = template.target_codec.trim().to_string();

    // 空白 → None（可空字段）
    template.required_encoder = template
        .required_encoder
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);

    // 派生字段：硬件加速标志永远由策略推导，不信任调用方
    template.requires_hardware_acceleration =
        template.hardware_mode == HardwareAccelerationMode::Required;

    template.supported_input_formats = dedup_preserve_order(
        template
            .supported_input_formats
            .iter()
            .filter(|value| !is_blank(value))
            .map(|value| value.trim().trim_start_matches('.').to_lowercase()),
    );

    template.tags = dedup_preserve_order(
        template.tags.iter().filter(|value| !is_blank(value)).map(|value| value.trim().to_string()),
    );

    for parameter in &mut template.parameters {
        // C#：`Trim().Trim('{','}')` —— 剥离首尾花括号，`{{a}}` → `a`
        parameter.name = parameter.name.trim().trim_matches(['{', '}']).to_string();
        parameter.label = if is_blank(&parameter.label) {
            parameter.name.clone()
        } else {
            parameter.label.trim().to_string()
        };
        for option in &mut parameter.options {
            if is_blank(&option.label) {
                option.label = option.value.clone();
            }
        }
    }
}

/// 分类排序权重（§1.8）：预设分类按下标，空白分类排倒数第二，未知分类排最后
pub fn category_rank(category: &str) -> i32 {
    let trimmed = category.trim();
    if trimmed.is_empty() {
        return i32::MAX - 1;
    }
    match CATEGORY_ORDER.iter().position(|known| *known == trimmed) {
        Some(index) => index as i32,
        None => i32::MAX - 2,
    }
}

/// 模板列表排序：`IsRecommended` 降序 → `CategoryRank` → `SortOrder` → `Name`（§1.8）
fn sort_templates(templates: &mut [CommandTemplate]) {
    templates.sort_by(|a, b| {
        b.is_recommended
            .cmp(&a.is_recommended)
            .then_with(|| category_rank(&a.category).cmp(&category_rank(&b.category)))
            .then_with(|| a.sort_order.cmp(&b.sort_order))
            .then_with(|| a.name.cmp(&b.name))
    });
}

/// 缺少的必需滤镜（§7.1）。
///
/// * 模板没有 `required_filters` → 空；
/// * 滤镜清单未知（`None`）或为空 → 空（**宁可漏报也不误报**）。
fn missing_filters(template: &CommandTemplate, available: Option<&[String]>) -> Vec<String> {
    if template.required_filters.is_empty() {
        return Vec::new();
    }
    let Some(available) = available.filter(|list| !list.is_empty()) else {
        return Vec::new();
    };
    template
        .required_filters
        .iter()
        .filter(|required| {
            !available.iter().any(|name| name.eq_ignore_ascii_case(required.trim()))
        })
        .cloned()
        .collect()
}

/// 用检测报告算缺少的必需滤镜（报告里没有滤镜清单 → 空）
fn missing_filters_against_report(
    template: &CommandTemplate,
    report: &HardwareReport,
) -> Vec<String> {
    if report.supported_filters.is_empty() {
        return Vec::new();
    }
    missing_filters(template, Some(&report.supported_filters))
}

/// 缺滤镜的统一消息（校验 / 列表 / 试运行三处共用，§8.2 逐字）
fn missing_filters_message(missing: &[String]) -> String {
    format!(
        "当前 FFmpeg 构建缺少该模板需要的滤镜：{}。请换用其它模板，或改用带完整滤镜支持的 FFmpeg 构建（如 gyan.dev / Homebrew 的完整版）。",
        missing.join("、")
    )
}

/// 编码器族的中文显示名（§5.2：编码器名 → 族 → 显示名；认不出就原样返回）
fn family_display_name(required: Option<&str>) -> String {
    let Some(required) = required.map(str::trim).filter(|value| !value.is_empty()) else {
        return "指定的硬件编码器".to_string();
    };
    let family = EncoderCatalog::find(required)
        .map(|profile| profile.family.clone())
        .unwrap_or_else(|| required.to_string());
    EncoderCatalog::family_display_name(&family)
}

/// 模板是否需要「编码器画像」。
///
/// 与参考实现的 `ConversionQueue.ResolveEncoderAsync` 判定一致：命令里出现 `{encoder}`
/// **或** 声明了非 `None` 的硬件策略。
fn needs_encoder_profile(template: &CommandTemplate) -> bool {
    template.hardware_mode != HardwareAccelerationMode::None
        || contains_ci(&template.command_args, "{encoder}")
}

/// 试运行用的目标编码格式（§8.4 的**两步**推断，与真实转换的五步不同）
fn test_target_codec(template: &CommandTemplate) -> String {
    let source = if is_blank(&template.target_codec) {
        template.output_extension.clone()
    } else {
        template.target_codec.clone()
    };
    let codec = EncoderCatalog::normalize_codec(&source);
    match codec.as_str() {
        "h264" | "hevc" | "av1" | "vp9" | "vp8" => codec,
        _ => "h264".to_string(),
    }
}

/// 试运行时把「看起来像路径」的参数替换成测试素材（§2.6）。
///
/// 判定：参数名（小写）含 `path` / `file` / `watermark` / `subtitle` / `image`；
/// 已有值且该文件真实存在时保留用户填的值，否则替换成素材路径。
fn substitute_required_paths(
    template: &CommandTemplate,
    parameters: &mut HashMap<String, String>,
    sample: &Path,
) {
    for parameter in &template.parameters {
        if is_blank(&parameter.name) {
            continue;
        }
        let name = parameter.name.to_lowercase();
        let looks_like_path = name.contains("path")
            || name.contains("file")
            || name.contains("watermark")
            || name.contains("subtitle")
            || name.contains("image");
        if !looks_like_path {
            continue;
        }

        // 参考实现的参数字典是 `OrdinalIgnoreCase`：这里的查找同样忽略大小写，
        // 命中时改写**原有的键**，避免同义键并存导致取值不确定
        let existing_key = parameters
            .keys()
            .find(|key| key.eq_ignore_ascii_case(&parameter.name))
            .cloned();
        let current = existing_key
            .as_ref()
            .and_then(|key| parameters.get(key))
            .map(|value| value.trim().to_string())
            .unwrap_or_default();
        if !current.is_empty() && crate::paths::to_absolute(&current).exists() {
            continue;
        }
        let target_key = existing_key.unwrap_or_else(|| parameter.name.clone());
        parameters.insert(target_key, sample.to_string_lossy().to_string());
    }
}

/// `MediaInfo` → `CommandContext` 用的轻量摘要
fn media_summary(info: &MediaInfo) -> MediaSummary {
    MediaSummary::from_parts(
        info.duration_seconds,
        info.video.as_ref().and_then(|stream| stream.width).unwrap_or(0),
        info.video.as_ref().and_then(|stream| stream.height).unwrap_or(0),
        info.video.as_ref().and_then(|stream| stream.frame_rate).unwrap_or(0.0),
        info.video
            .as_ref()
            .map(|stream| stream.codec_name.clone())
            .filter(|name| !name.is_empty()),
        info.audio
            .as_ref()
            .map(|stream| stream.codec_name.clone())
            .filter(|name| !name.is_empty()),
        info.audio.as_ref().and_then(|stream| stream.sample_rate).unwrap_or(0),
        info.audio.as_ref().and_then(|stream| stream.channels).unwrap_or(0),
        info.video.is_some(),
        info.audio.is_some(),
    )
}

/// 构造一条「没跑起来」的失败结果（`command` 用原始模板串，`duration_ms = 0`，§8.2 表格）
fn failed_test(message: impl Into<String>, template_command_args: &str) -> TemplateTestResult {
    TemplateTestResult {
        success: false,
        message: message.into(),
        command: template_command_args.to_string(),
        output: String::new(),
        duration_ms: 0,
    }
}

impl TemplateService {
    /// `EnsureUniqueName`（§1.7）。
    ///
    /// 冲突时从 `(2)` 开始加后缀（半角括号 + 空格），判定为**精确字符串相等**
    /// （区分大小写、区分全半角），查询范围是**全库**（含系统模板）。
    async fn ensure_unique_name(
        &self,
        name: Option<&str>,
        exclude_id: Option<&str>,
    ) -> anyhow::Result<String> {
        let base = match name.map(str::trim).filter(|value| !value.is_empty()) {
            Some(value) => value.to_string(),
            None => DEFAULT_NAME.to_string(),
        };

        let mut candidate = base.clone();
        let mut counter = 1;
        loop {
            match self.store.get_template_by_name(&candidate).await? {
                Some(existing) if Some(existing.id.as_str()) != exclude_id => {
                    counter += 1;
                    candidate = format!("{base} ({counter})");
                }
                _ => return Ok(candidate),
            }
        }
    }
}

// ════════════════════════════════════════════════════════════════════════════
// 测试
// ════════════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::TemplateParameter;

    // ── 测试脚手架 ──────────────────────────────────────────────────────────

    /// 内存库 + 真实 FFmpeg + 真实硬件探测器（不加数据库缓存，报告缓存在进程内）
    async fn service() -> TemplateService {
        let store = Arc::new(Store::open_in_memory().await.expect("内存库必须可打开"));
        let ffmpeg = Arc::new(FFmpegService::new());
        let hardware = Arc::new(HardwareDetector::new(ffmpeg.clone()));
        TemplateService::new(store, ffmpeg, hardware)
    }

    /// 一条「结构完整」的用户模板（不会触发任何校验 Error）
    fn valid_template(name: &str) -> CommandTemplate {
        CommandTemplate {
            name: name.to_string(),
            command_args: "-i \"{input}\" -c:v libx264 \"{output}\"".to_string(),
            output_extension: "mp4".to_string(),
            ..Default::default()
        }
    }

    /// 按稳定 key 取预设（每次调用都会新建对象，id 是确定性派生的）
    fn preset_by_key(key: &str) -> CommandTemplate {
        presets::all_with_keys()
            .into_iter()
            .find(|(candidate, _)| candidate == key)
            .map(|(_, template)| template)
            .unwrap_or_else(|| panic!("预设 key 必须存在：{key}"))
    }

    fn messages(issues: &[TemplateIssue]) -> Vec<String> {
        issues.iter().map(|issue| issue.message.clone()).collect()
    }

    fn issue_with<'a>(issues: &'a [TemplateIssue], needle: &str) -> &'a TemplateIssue {
        issues
            .iter()
            .find(|issue| issue.message.contains(needle))
            .unwrap_or_else(|| panic!("没有找到包含「{needle}」的问题：{:?}", messages(issues)))
    }

    fn count_of(issues: &[TemplateIssue], severity: TemplateIssueSeverity) -> usize {
        issues.iter().filter(|issue| issue.severity == severity).count()
    }

    // ══ 系统模板同步 ═══════════════════════════════════════════════════════

    #[tokio::test]
    async fn initial_sync_inserts_all_presets() {
        let service = service().await;
        let result = service.initialize_system_templates().await.expect("同步成功");

        assert_eq!(presets::all().len(), 51, "内置目录必须是 51 条");
        assert_eq!(result.preset_version, PRESET_VERSION);
        assert_eq!(result.added, 51);
        assert_eq!(result.updated, 0);
        assert_eq!(result.unchanged, 0);
        assert_eq!(result.removed, 0);
        assert_eq!(
            result.summary,
            format!("系统模板已更新：新增 51 个，更新 0 个（预设 v{PRESET_VERSION}）")
        );
        assert_eq!(service.store.list_templates(true).await.expect("列表").len(), 51);
    }

    #[tokio::test]
    async fn second_sync_is_noop() {
        let service = service().await;
        service.initialize_system_templates().await.expect("首次同步");

        let second = service.initialize_system_templates().await.expect("再次同步");
        assert_eq!(second.added, 0);
        assert_eq!(second.updated, 0);
        assert_eq!(second.unchanged, 51);
        assert_eq!(second.removed, 0);
        assert!(!second.changed());
        assert_eq!(second.summary, format!("系统模板已是最新（预设 v{PRESET_VERSION}）"));
        assert_eq!(service.store.list_templates(true).await.expect("列表").len(), 51);
    }

    #[tokio::test]
    async fn outdated_preset_is_updated_and_keeps_id() {
        let service = service().await;
        service.initialize_system_templates().await.expect("首次同步");

        let expected = preset_by_key("rec.mp4_h264_auto");
        let mut row = service.find_by_name(&expected.name).await.expect("查询").expect("存在");
        let id = row.id.clone();
        let created_at = row.created_at;

        // 模拟「旧版本号 + 被改过的内容」（升级路径：§10.3）
        row.command_args = "-i \"{input}\" -c copy \"{output}\"".to_string();
        row.preset_version = PRESET_VERSION - 1;
        service.store.update_template(&row).await.expect("改库");

        let result = service.initialize_system_templates().await.expect("同步");
        assert_eq!(result.updated, 1);
        assert_eq!(result.added, 0);
        assert_eq!(result.unchanged, 50);
        assert_eq!(result.removed, 0);

        let after = service.get(&id).await.expect("查询").expect("仍在");
        assert_eq!(after.id, id, "升级必须保留 Id（任务引用不悬空）");
        assert_eq!(after.created_at, created_at, "升级必须保留 CreatedAt");
        assert_eq!(after.command_args, expected.command_args, "内容应被预设覆盖");
        assert_eq!(after.preset_version, PRESET_VERSION);
    }

    #[tokio::test]
    async fn removed_preset_is_deleted() {
        let service = service().await;
        service.initialize_system_templates().await.expect("首次同步");

        // 模拟「上一版的预设在本版目录里被移除」：key 派生 id 不在当前目录里
        let stale = CommandTemplate {
            id: presets::system_template_id("gone.removed_preset"),
            name: "已移除的预设".to_string(),
            command_args: "-i \"{input}\" -c copy \"{output}\"".to_string(),
            output_extension: "mp4".to_string(),
            r#type: TemplateType::System,
            preset_version: PRESET_VERSION,
            ..Default::default()
        };
        service.store.insert_template(&stale).await.expect("写库");
        assert_eq!(service.store.list_templates(true).await.expect("列表").len(), 52);

        let result = service.initialize_system_templates().await.expect("同步");
        assert_eq!(result.removed, 1, "陈旧系统模板必须被删除（有意偏差 #14）");
        assert_eq!(result.added, 0);
        assert_eq!(result.updated, 0);
        assert_eq!(result.unchanged, 51);
        assert_eq!(
            result.summary,
            format!("系统模板已更新：新增 0 个，更新 0 个（预设 v{PRESET_VERSION}）")
        );
        assert!(service.get(&stale.id).await.expect("查询").is_none());
        assert_eq!(service.store.list_templates(true).await.expect("列表").len(), 51);

        // 再同步一次：没有可删的了
        let third = service.initialize_system_templates().await.expect("同步");
        assert_eq!(third.removed, 0);
        assert_eq!(third.unchanged, 51);
    }

    #[tokio::test]
    async fn legacy_row_with_random_id_is_adopted_by_name() {
        let service = service().await;
        let expected = preset_by_key("rec.mp4_h264_auto");

        // 参考实现（C#）写下的旧行：随机 ObjectId + 同名 + 旧版本号
        let legacy_id = new_id();
        let legacy = CommandTemplate {
            id: legacy_id.clone(),
            name: expected.name.clone(),
            command_args: "-i \"{input}\" -c:v libx264 \"{output}\"".to_string(),
            output_extension: "mp4".to_string(),
            r#type: TemplateType::System,
            preset_version: PRESET_VERSION - 1,
            ..Default::default()
        };
        service.store.insert_template(&legacy).await.expect("写库");

        let result = service.initialize_system_templates().await.expect("同步");
        assert_eq!(result.updated, 1, "同名旧行应被认领并覆盖，而不是新增第二条");
        assert_eq!(result.added, 50);
        assert_eq!(result.removed, 0);
        assert_eq!(service.store.list_templates(true).await.expect("列表").len(), 51);

        let adopted = service.get(&legacy_id).await.expect("查询").expect("旧 id 必须保留");
        assert_eq!(adopted.command_args, expected.command_args);
        assert_eq!(adopted.preset_version, PRESET_VERSION);
    }

    #[tokio::test]
    async fn hand_edited_preset_survives_until_version_bump() {
        let service = service().await;
        service.initialize_system_templates().await.expect("同步");

        let mut row = service
            .find_by_name(&preset_by_key("rec.mp4_h264_auto").name)
            .await
            .expect("查询")
            .expect("存在");
        row.description = "管理员手改的说明".to_string();
        service.store.update_template(&row).await.expect("改库");

        let result = service.initialize_system_templates().await.expect("同步");
        assert_eq!(result.unchanged, 51, "版本号未变时管理员手改必须保留（§10.4）");
        let after = service.get(&row.id).await.expect("查询").expect("仍在");
        assert_eq!(after.description, "管理员手改的说明");
    }

    #[tokio::test]
    async fn user_templates_survive_sync_and_reset() {
        let service = service().await;
        let user = service.create(valid_template("我的模板")).await.expect("创建");

        service.initialize_system_templates().await.expect("同步");
        let system_ids: Vec<String> = service
            .store
            .list_templates(true)
            .await
            .expect("列表")
            .into_iter()
            .filter(CommandTemplate::is_system)
            .map(|template| template.id)
            .collect();

        let result = service.reset_system_templates().await.expect("重置");
        assert_eq!(result.added, 51);
        assert_eq!(result.removed, 51);
        assert!(result.changed());
        assert_eq!(
            result.summary,
            format!("系统模板已更新：新增 51 个，更新 0 个（预设 v{PRESET_VERSION}）")
        );

        let all = service.store.list_templates(true).await.expect("列表");
        assert_eq!(all.len(), 52, "51 条系统模板 + 1 条用户模板");
        let kept = service.get(&user.id).await.expect("查询").expect("用户模板必须保留");
        assert_eq!(kept.name, "我的模板");
        assert_eq!(kept.r#type, TemplateType::User);

        // 有意偏差 #16：确定性 id 让重置后系统模板 id 不变，历史引用不悬空
        let system_ids_after: Vec<String> = all
            .into_iter()
            .filter(CommandTemplate::is_system)
            .map(|template| template.id)
            .collect();
        assert_eq!(
            {
                let mut before = system_ids.clone();
                before.sort();
                before
            },
            {
                let mut after = system_ids_after;
                after.sort();
                after
            }
        );
    }

    // ══ CRUD ═══════════════════════════════════════════════════════════════

    #[tokio::test]
    async fn create_normalizes_every_write_path() {
        let service = service().await;
        let mut template = CommandTemplate {
            name: "  我的模板  ".to_string(),
            description: "  说明  ".to_string(),
            command_args: "  -i \"{input}\" -c:v {encoder} -c:a aac \"{output}\"  ".to_string(),
            category: "   ".to_string(),
            output_extension: ".MP4".to_string(),
            target_codec: " H264 ".to_string(),
            required_encoder: Some("  nvenc  ".to_string()),
            hardware_mode: HardwareAccelerationMode::Required,
            supported_input_formats: vec![" MP4".to_string(), ".mp4".to_string(), "".to_string()],
            tags: vec![" 压缩 ".to_string(), "压缩".to_string(), "".to_string()],
            parameters: vec![TemplateParameter {
                name: " {crf} ".to_string(),
                label: String::new(),
                ..Default::default()
            }],
            ..Default::default()
        };
        template.requires_hardware_acceleration = false; // 派生字段应被 Normalize 覆盖

        let created = service.create(template).await.expect("创建成功");
        assert_eq!(created.name, "我的模板");
        assert_eq!(created.description, "说明");
        assert_eq!(created.command_args, "-i \"{input}\" -c:v {encoder} -c:a aac \"{output}\"");
        assert_eq!(created.category, DEFAULT_CATEGORY, "空分类 → 未分类");
        assert_eq!(created.output_extension, "mp4");
        assert_eq!(created.target_codec, "H264", "target_codec 只 Trim");
        assert_eq!(created.required_encoder.as_deref(), Some("nvenc"));
        assert!(created.requires_hardware_acceleration, "由 HardwareMode 推导");
        assert_eq!(created.supported_input_formats, vec!["mp4".to_string()]);
        assert_eq!(created.tags, vec!["压缩".to_string()]);
        assert_eq!(created.parameters[0].name, "crf", "参数名剥离花括号");
        assert_eq!(created.parameters[0].label, "crf", "空 label → name");
        assert_eq!(created.r#type, TemplateType::User);
    }

    #[tokio::test]
    async fn create_rejects_duplicate_and_blank_names() {
        let service = service().await;
        let first = service.create(valid_template("重复名")).await.expect("首次");
        assert!(crate::models::is_valid_id(&first.id));

        let error = service.create(valid_template("重复名")).await.expect_err("重名必须失败");
        assert!(error.to_string().contains("已存在"), "错误文案：{error}");

        // 与系统模板重名同样被拒绝（name 全局唯一，§1.5）
        service.initialize_system_templates().await.expect("同步");
        let preset_name = preset_by_key("rec.fast_remux").name;
        let error = service.create(valid_template(&preset_name)).await.expect_err("重名必须失败");
        assert!(error.to_string().contains("已存在"), "错误文案：{error}");

        let error = service
            .create(CommandTemplate { name: "   ".to_string(), ..Default::default() })
            .await
            .expect_err("空名必须失败");
        assert_eq!(error.to_string(), "模板名称不能为空");
    }

    #[tokio::test]
    async fn delete_refuses_system_templates() {
        let service = service().await;
        service.initialize_system_templates().await.expect("同步");
        let preset = preset_by_key("rec.fast_remux");

        let error = service.delete(&preset.id).await.expect_err("系统模板不可删除");
        assert!(error.to_string().contains("系统模板不可删除"), "错误文案：{error}");
        assert!(service.get(&preset.id).await.expect("查询").is_some());

        assert!(!service.delete("000000000000000000000000").await.expect("不存在返回 false"));

        let user = service.create(valid_template("可删模板")).await.expect("创建");
        assert!(service.delete(&user.id).await.expect("删除"));
        assert!(service.get(&user.id).await.expect("查询").is_none());
        assert!(!service.delete(&user.id).await.expect("再删返回 false"));
    }

    #[tokio::test]
    async fn update_rejects_system_templates_and_duplicates() {
        let service = service().await;
        service.initialize_system_templates().await.expect("同步");

        // 系统模板不可更新
        let mut preset = service.get(&preset_by_key("rec.fast_remux").id).await.expect("查询").expect("存在");
        preset.description = "篡改".to_string();
        let error = service.update(preset).await.expect_err("系统模板不可改");
        assert!(error.to_string().contains("系统模板不可修改"), "错误文案：{error}");

        // 用户模板正常更新，保留 CreatedAt
        let created = service.create(valid_template("甲")).await.expect("创建甲");
        service.create(valid_template("乙")).await.expect("创建乙");
        let mut edited = created.clone();
        edited.description = "改过了".to_string();
        let updated = service.update(edited).await.expect("更新成功");
        assert_eq!(
            updated.created_at.timestamp_millis(),
            created.created_at.timestamp_millis(),
            "保留创建时间（库精度是毫秒）"
        );
        assert!(updated.updated_at >= created.updated_at);

        // 改成别人的名字 → 报错
        let mut clash = updated.clone();
        clash.name = "乙".to_string();
        let error = service.update(clash).await.expect_err("重名必须失败");
        assert!(error.to_string().contains("已存在"), "错误文案：{error}");

        // 不存在的 id
        let error = service
            .update(CommandTemplate { id: new_id(), ..Default::default() })
            .await
            .expect_err("不存在");
        assert_eq!(error.to_string(), "模板不存在或已被删除");
    }

    #[tokio::test]
    async fn copy_auto_uniquifies_and_copies_parameters() {
        let service = service().await;
        service.initialize_system_templates().await.expect("同步");
        let source = preset_by_key("special.burn_subtitles");

        let first = service.copy(&source.id, None).await.expect("复制预设");
        assert_eq!(first.name, format!("{}{}", source.name, COPY_SUFFIX));
        assert_eq!(first.r#type, TemplateType::User);
        assert_eq!(first.preset_version, 0);
        assert!(!first.is_recommended);
        assert_eq!(first.sort_order, source.sort_order + 1);
        assert_eq!(first.parameters, source.parameters, "参数必须深拷贝");
        assert_eq!(first.required_filters, source.required_filters, "有意偏差 #20");
        assert_ne!(first.id, source.id);
        assert!(crate::models::is_valid_id(&first.id));

        // 再复制一次：自动加 (2)
        let second = service.copy(&source.id, None).await.expect("再复制");
        assert_eq!(second.name, format!("{}{} (2)", source.name, COPY_SUFFIX));

        // 指定名字
        let named = service.copy(&source.id, Some("我的字幕模板".to_string())).await.expect("指定名");
        assert_eq!(named.name, "我的字幕模板");

        // 源不存在
        let error = service.copy("000000000000000000000000", None).await.expect_err("源不存在");
        assert_eq!(error.to_string(), "源模板不存在");
    }

    #[tokio::test]
    async fn queries_and_sorting_follow_the_spec() {
        let service = service().await;
        service.initialize_system_templates().await.expect("同步");

        // 分类统计：按预设顺序，计数之和 = 总条数（此时库里只有系统模板）
        let categories = service.categories().await.expect("分类");
        let names: Vec<&str> = categories.iter().map(|item| item.name.as_str()).collect();
        assert_eq!(names, CATEGORY_ORDER.to_vec());
        assert_eq!(categories[0].count, 4, "推荐 4 条");
        assert_eq!(categories.iter().map(|item| item.count).sum::<usize>(), 51);

        service.create(valid_template("我的模板")).await.expect("创建");

        let all = service.list(true).await.expect("列表");
        assert_eq!(all.len(), 52);
        assert_eq!(all[0].template.name, "MP4 · H.264（自动选择最佳编码器）", "推荐优先");
        assert!(all[0].template.is_recommended);
        assert_eq!(
            all[0].input_formats_display, "任意格式",
            "空输入格式列表的展示文本"
        );
        assert!(all[0].available, "Auto 模板必须判为可用（不需要硬件画像）");
        assert!(all[0].unavailable_reason.is_none());

        // 用户模板排在最后（category_rank：未知分类 > 已知分类）
        assert_eq!(all.last().expect("非空").template.name, "我的模板");

        let user_only = service.list(false).await.expect("仅用户模板");
        assert_eq!(user_only.len(), 1);
        assert_eq!(user_only[0].template.r#type, TemplateType::User);


        // 用户模板的「未分类」排在已知分类之后
        let categories = service.categories().await.expect("分类");
        assert_eq!(categories.last().expect("非空").name, DEFAULT_CATEGORY);
    }

    #[tokio::test]
    async fn default_values_skip_unnamed_parameters() {
        let template = CommandTemplate {
            parameters: vec![
                TemplateParameter {
                    name: "crf".to_string(),
                    default_value: "23".to_string(),
                    ..Default::default()
                },
                TemplateParameter { name: "  ".to_string(), ..Default::default() },
            ],
            ..Default::default()
        };
        let values = TemplateService::default_values(&template);
        assert_eq!(values.len(), 1);
        assert_eq!(values.get("crf").map(String::as_str), Some("23"));
    }

    // ══ 校验：§7 的每一条（逐字断言）═══════════════════════════════════════

    #[tokio::test]
    async fn validate_reports_empty_name() {
        let service = service().await;
        let mut template = valid_template("");
        template.name = "   ".to_string();
        let issues = service.validate(&template, None);
        let issue = issue_with(&issues, "模板名称不能为空");
        assert_eq!(issue.message, "模板名称不能为空");
        assert_eq!(issue.severity, TemplateIssueSeverity::Error);
        assert_eq!(issue.field.as_deref(), Some("Name"));
    }

    #[tokio::test]
    async fn validate_empty_command_args_short_circuits() {
        let service = service().await;
        let mut template = valid_template("");
        template.name = "x".to_string();
        template.command_args = "   ".to_string();
        let issues = service.validate(&template, None);
        assert_eq!(messages(&issues), vec!["命令参数不能为空"], "必须立即返回，不报后续问题");
    }

    #[tokio::test]
    async fn validate_requires_output_extension() {
        let service = service().await;
        let mut template = valid_template("x");
        template.output_extension = "  ".to_string();
        let issues = service.validate(&template, None);
        let issue = issue_with(&issues, "必须指定输出扩展名");
        assert_eq!(issue.message, "必须指定输出扩展名，否则无法生成输出文件名");
        assert_eq!(issue.field.as_deref(), Some("OutputExtension"));
    }

    #[tokio::test]
    async fn validate_requires_input_and_output_placeholders() {
        let service = service().await;
        let template = CommandTemplate {
            name: "x".to_string(),
            command_args: "-c:v libx264 out.mp4".to_string(),
            output_extension: "mp4".to_string(),
            ..Default::default()
        };
        let issues = service.validate(&template, None);
        assert_eq!(
            issue_with(&issues, "缺少 {input}").message,
            "命令中缺少 {input} 占位符，FFmpeg 将无法读取输入文件"
        );
        assert_eq!(
            issue_with(&issues, "没有输出目标").message,
            "命令中缺少 {output}（或用于图片序列的 {output_pattern}）占位符，转换结果没有输出目标"
        );

        // `{output_pattern}` 可以替代 `{output}`（图片序列模板）
        let sequence = CommandTemplate {
            name: "x".to_string(),
            command_args: "-i \"{input}\" frame_%04d.jpg \"{output_pattern}\"".to_string(),
            output_extension: "jpg".to_string(),
            ..Default::default()
        };
        let issues = service.validate(&sequence, None);
        assert_eq!(count_of(&issues, TemplateIssueSeverity::Error), 0, "{issues:?}");
    }

    #[tokio::test]
    async fn validate_reports_undeclared_placeholders() {
        let service = service().await;
        let template = CommandTemplate {
            name: "x".to_string(),
            command_args: "-i \"{input}\" -vf {watermark} \"{output}\"".to_string(),
            output_extension: "mp4".to_string(),
            ..Default::default()
        };
        let issues = service.validate(&template, None);
        assert_eq!(
            issue_with(&issues, "占位符 {watermark}").message,
            "占位符 {watermark} 没有对应取值：请在上方『参数』中声明它，或改用内置占位符"
        );
        assert_eq!(issue_with(&issues, "占位符").field.as_deref(), Some("Parameters"));

        // 声明之后就不再报
        let declared = CommandTemplate {
            parameters: vec![TemplateParameter { name: "watermark".to_string(), ..Default::default() }],
            ..template.clone()
        };
        let issues = service.validate(&declared, None);
        assert!(messages(&issues).iter().all(|message| !message.contains("没有对应取值")));
    }

    #[tokio::test]
    async fn validate_reports_odd_quote_count() {
        let service = service().await;
        let template = CommandTemplate {
            name: "x".to_string(),
            command_args: "-i {input} -metadata title=\"a b {output}".to_string(),
            output_extension: "mp4".to_string(),
            ..Default::default()
        };
        let issues = service.validate(&template, None);
        assert_eq!(
            issue_with(&issues, "双引号数量为奇数").message,
            "双引号数量为奇数，命令可能被错误拆分。含空格的路径请写成 \"{input}\""
        );
    }

    #[tokio::test]
    async fn validate_warns_about_crf_on_hardware_encoder() {
        let service = service().await;
        let template = CommandTemplate {
            name: "x".to_string(),
            command_args: "-i \"{input}\" -c:v {encoder} -crf 23 \"{output}\"".to_string(),
            output_extension: "mp4".to_string(),
            hardware_mode: HardwareAccelerationMode::Required,
            required_encoder: Some("nvenc".to_string()),
            ..Default::default()
        };
        let issues = service.validate(&template, None);
        let issue = issue_with(&issues, "硬件编码器不支持 -crf");
        assert_eq!(
            issue.message,
            "硬件编码器不支持 -crf，该选项会被 FFmpeg 忽略。建议改用 {quality_args} 占位符，它会自动展开成当前编码器正确的质量参数。"
        );
        assert_eq!(issue.severity, TemplateIssueSeverity::Warning);
        assert_eq!(issue.field.as_deref(), Some("CommandArgs"));
    }

    #[tokio::test]
    async fn validate_warns_about_unknown_required_encoder() {
        let service = service().await;
        let template = CommandTemplate {
            name: "x".to_string(),
            command_args: "-i \"{input}\" -c:v {encoder} \"{output}\"".to_string(),
            output_extension: "mp4".to_string(),
            required_encoder: Some("bogus".to_string()),
            ..Default::default()
        };
        let issues = service.validate(&template, None);
        assert_eq!(issues.len(), 1, "只应报「认不出编码器」：{:?}", messages(&issues));
        assert_eq!(
            issues[0].message,
            "无法识别的编码器「bogus」。可用族名：nvenc / qsv / amf / videotoolbox / vaapi / vulkan，或具体编码器名如 h264_nvenc。"
        );
        assert_eq!(issues[0].field.as_deref(), Some("RequiredEncoder"));
    }

    #[tokio::test]
    async fn validate_errors_when_required_hardware_has_no_family() {
        let service = service().await;
        let template = CommandTemplate {
            name: "x".to_string(),
            command_args: "-i \"{input}\" -c:v {encoder} \"{output}\"".to_string(),
            output_extension: "mp4".to_string(),
            hardware_mode: HardwareAccelerationMode::Required,
            ..Default::default()
        };
        let issues = service.validate(&template, None);
        let issue = issue_with(&issues, "没有指定编码器族");
        assert_eq!(
            issue.message,
            "模板要求必须使用硬件加速，但没有指定编码器族。这样在任何机器上都会失败。"
        );
        assert_eq!(issue.severity, TemplateIssueSeverity::Error);
        assert_eq!(issue.field.as_deref(), Some("RequiredEncoder"));
    }

    #[tokio::test]
    async fn validate_warns_when_hardware_template_avoids_encoder_placeholder() {
        let service = service().await;
        let template = CommandTemplate {
            name: "x".to_string(),
            command_args: "-i \"{input}\" -c:v h264_nvenc \"{output}\"".to_string(),
            output_extension: "mp4".to_string(),
            hardware_mode: HardwareAccelerationMode::Auto,
            ..Default::default()
        };
        let issues = service.validate(&template, None);
        assert_eq!(
            issue_with(&issues, "建议使用 {encoder}").message,
            "硬件加速模板建议使用 {encoder} 占位符，这样在本机没有该厂商硬件时会自动回退到可用的编码器。"
        );
    }

    #[tokio::test]
    async fn validate_copy_mode_info_and_filter_error() {
        let service = service().await;
        let copy_only = CommandTemplate {
            name: "x".to_string(),
            command_args: "-i \"{input}\" -c copy \"{output}\"".to_string(),
            output_extension: "mp4".to_string(),
            ..Default::default()
        };
        let issues = service.validate(&copy_only, None);
        let issue = issue_with(&issues, "「复制流」模式");
        assert_eq!(
            issue.message,
            "「复制流」模式下画面不会被重新编码。如果源文件的编码格式与目标容器不兼容，转换会失败——这正是「快速换容器」模板的已知限制。"
        );
        assert_eq!(issue.severity, TemplateIssueSeverity::Info);
        assert!(issue.field.is_none(), "该提示不关联字段");
        assert_eq!(count_of(&issues, TemplateIssueSeverity::Error), 0);

        let copy_with_filter = CommandTemplate {
            command_args: "-i \"{input}\" -c:v copy -vf scale=1280:-2 \"{output}\"".to_string(),
            ..copy_only.clone()
        };
        let issues = service.validate(&copy_with_filter, None);
        assert_eq!(
            issue_with(&issues, "不能与滤镜同时使用").message,
            "「-c copy」不能与滤镜同时使用：不重新编码就无法应用滤镜。"
        );
        assert_eq!(issue_with(&issues, "不能与滤镜同时使用").severity, TemplateIssueSeverity::Error);
    }

    #[tokio::test]
    async fn validate_warns_about_unnamed_and_shadowing_parameters() {
        let service = service().await;
        let template = CommandTemplate {
            parameters: vec![
                TemplateParameter { name: "  ".to_string(), ..Default::default() },
                TemplateParameter {
                    name: "fps".to_string(),
                    kind: TemplateParameterKind::Number,
                    ..Default::default()
                },
                // `crf`/`quality`/`q` 是明确允许覆盖内置值的例外
                TemplateParameter {
                    name: "crf".to_string(),
                    kind: TemplateParameterKind::Number,
                    ..Default::default()
                },
            ],
            ..valid_template("x")
        };
        let issues = service.validate(&template, None);

        assert_eq!(
            issue_with(&issues, "没有名字的参数").message,
            "存在一个没有名字的参数，它不会被使用。"
        );
        assert_eq!(
            issue_with(&issues, "与内置占位符同名").message,
            "参数名「fps」与内置占位符同名，内置值可能被覆盖。建议改名。"
        );
        assert_eq!(
            messages(&issues).iter().filter(|message| message.contains("与内置占位符同名")).count(),
            1,
            "crf 是被允许的例外"
        );
    }

    #[tokio::test]
    async fn validate_warns_about_select_without_options() {
        let service = service().await;
        let template = CommandTemplate {
            parameters: vec![TemplateParameter {
                name: "audio_bitrate".to_string(),
                label: String::new(),
                kind: TemplateParameterKind::Select,
                ..Default::default()
            }],
            ..valid_template("x")
        };
        let issues = service.validate(&template, None);
        assert_eq!(
            issue_with(&issues, "没有配置任何选项").message,
            "参数「audio_bitrate」是下拉选项但没有配置任何选项。"
        );
    }

    #[tokio::test]
    async fn validate_warns_about_quality_args_without_encoder() {
        let service = service().await;
        let template = CommandTemplate {
            name: "x".to_string(),
            command_args: "-i \"{input}\" {quality_args} \"{output}\"".to_string(),
            output_extension: "mp4".to_string(),
            ..Default::default()
        };
        let issues = service.validate(&template, None);
        assert_eq!(
            issue_with(&issues, "质量参数将按默认编码器展开").message,
            "使用了 {quality_args} 但没有 {encoder}：质量参数将按默认编码器展开。"
        );
    }

    #[tokio::test]
    async fn validate_info_lists_required_filters() {
        let service = service().await;
        let template = CommandTemplate {
            required_filters: vec!["subtitles".to_string(), "scale".to_string()],
            ..valid_template("x")
        };
        let issues = service.validate(&template, None);
        let issue = issue_with(&issues, "该模板依赖以下 FFmpeg 滤镜");
        assert_eq!(
            issue.message,
            "该模板依赖以下 FFmpeg 滤镜：subtitles、scale。如果当前 FFmpeg 构建没有编译它们，模板会显示为不可用。"
        );
        assert_eq!(issue.severity, TemplateIssueSeverity::Info);
        assert_eq!(issue.field.as_deref(), Some("RequiredFilters"));
    }

    #[tokio::test]
    async fn validate_reports_missing_filters_only_when_list_is_known() {
        let service = service().await;
        let template = CommandTemplate {
            required_filters: vec!["subtitles".to_string()],
            ..valid_template("x")
        };

        // 清单未知（None）→ 不报（宁可漏报也不误报）
        assert!(messages(&service.validate(&template, None))
            .iter()
            .all(|message| !message.contains("缺少该模板需要的滤镜")));
        // 空清单 → 同样不报
        assert!(messages(&service.validate(&template, Some(&[])))
            .iter()
            .all(|message| !message.contains("缺少该模板需要的滤镜")));

        // 清单里有别的滤镜 → 报（消息与试运行/列表同款）
        let available = vec!["scale".to_string()];
        let issues = service.validate(&template, Some(&available));
        let issue = issue_with(&issues, "缺少该模板需要的滤镜");
        assert_eq!(
            issue.message,
            "当前 FFmpeg 构建缺少该模板需要的滤镜：subtitles。请换用其它模板，或改用带完整滤镜支持的 FFmpeg 构建（如 gyan.dev / Homebrew 的完整版）。"
        );
        assert_eq!(issue.severity, TemplateIssueSeverity::Error);

        // 清单里有该滤镜（大小写不敏感）→ 不报
        let available = vec!["SubTitles".to_string()];
        assert!(messages(&service.validate(&template, Some(&available)))
            .iter()
            .all(|message| !message.contains("缺少该模板需要的滤镜")));
    }

    // ══ 导入 / 导出 ════════════════════════════════════════════════════════

    #[tokio::test]
    async fn export_import_roundtrip_changes_ids_and_uniquifies_names() {
        let service = service().await;
        service.initialize_system_templates().await.expect("同步");

        let source = vec![
            preset_by_key("rec.mp4_h264_auto"),
            preset_by_key("special.burn_subtitles"),
            preset_by_key("compress.max_size"),
        ];
        let exported = service.export_json(&source);

        assert_eq!(exported["Format"], "FFmpegWebUI.Templates");
        assert_eq!(exported["SchemaVersion"], 1);
        assert_eq!(exported["PresetVersion"], PRESET_VERSION);
        assert_eq!(exported["Templates"].as_array().expect("数组").len(), 3);
        let item = &exported["Templates"][0];
        assert_eq!(item["Name"], source[0].name, "键名逐字（PascalCase）");
        assert_eq!(item["HardwareMode"], 1, "枚举是数字");
        assert!(item.get("Id").is_none(), "有损：不含 Id");
        assert!(item.get("RequiredFilters").is_none(), "有损：不含 RequiredFilters");
        assert!(item.get("IsRecommended").is_none(), "有损：不含 IsRecommended");

        let result = service.import_json(&exported, false).await.expect("导入");
        assert_eq!(result.imported, 3, "errors = {:?}", result.errors);
        assert_eq!(result.skipped, 0);
        assert!(result.errors.is_empty());

        let imported = service
            .find_by_name(&format!("{} (2)", source[0].name))
            .await
            .expect("查询")
            .expect("自动加后缀的用户模板");
        assert_ne!(imported.id, source[0].id, "导入必须分配新 id");
        assert_eq!(imported.r#type, TemplateType::User);
        assert_eq!(imported.preset_version, 0);
        assert!(!imported.is_recommended);
        assert_eq!(imported.parameters, source[0].parameters, "参数必须存活");
        assert_eq!(imported.target_codec, source[0].target_codec);
        assert_eq!(imported.hardware_mode, source[0].hardware_mode);

        // 有损项：RequiredFilters 丢失（导出格式里没有这个键）
        let burned = service
            .find_by_name(&format!("{} (2)", source[1].name))
            .await
            .expect("查询")
            .expect("字幕模板");
        assert!(burned.required_filters.is_empty(), "导出格式不含 RequiredFilters");

        // 重复导入同一份文件 → 更多副本，不报错
        let again = service.import_json(&exported, false).await.expect("再导入");
        assert_eq!(again.imported, 3);
        assert!(service.find_by_name(&format!("{} (3)", source[0].name)).await.expect("查询").is_some());

        // 再导出一次：结构仍是参考格式
        let re_exported = service.export_json(&[imported]);
        assert_eq!(re_exported["Templates"].as_array().expect("数组").len(), 1);
        assert_eq!(
            re_exported["Templates"][0]["Parameters"].as_array().expect("参数数组").len(),
            source[0].parameters.len()
        );
    }

    #[tokio::test]
    async fn import_accepts_reference_files_and_tolerates_bad_items() {
        let service = service().await;

        // 参考实现导出的形状：PascalCase 键、数字枚举、null 键消失、随机 id（这里没有 id 键）
        let value = serde_json::json!({
            "Format": "FFmpegWebUI.Templates",
            "SchemaVersion": 1,
            "ExportedAt": "2026-10-16T06:53:59.1234567Z",
            "AppVersion": "2.0.0",
            "PresetVersion": 4,
            "Templates": [
                {
                    "Name": "参考实现导入的模板",
                    "Description": "来自 C# 版本",
                    "CommandArgs": "-i \"{input}\" -c copy \"{output}\"",
                    "Category": "",
                    "HardwareMode": 1,
                    "TargetCodec": "h264",
                    "Parameters": [
                        { "Name": "x", "Label": "", "Description": "", "Kind": 2,
                          "DefaultValue": "1", "Options": [
                            { "Value": "1", "Label": "", "Description": "" } ],
                          "TrueValue": "1", "FalseValue": "" }
                    ],
                    "SortOrder": 7
                }
            ]
        });

        let result = service.import_json(&value, false).await.expect("导入");
        assert_eq!(result.imported, 1, "errors = {:?}", result.errors);
        let template = service
            .find_by_name("参考实现导入的模板")
            .await
            .expect("查询")
            .expect("存在");
        assert_eq!(template.category, "导入", "空分类 → 导入");
        assert_eq!(template.hardware_mode, HardwareAccelerationMode::Auto);
        assert!(!template.requires_hardware_acceleration);
        assert_eq!(template.sort_order, 7);
        assert_eq!(template.parameters[0].label, "x", "空 label → name");
        assert_eq!(template.parameters[0].options[0].label, "1", "空 option.label → value");
        assert_eq!(template.parameters[0].kind, TemplateParameterKind::Text);

        // 空名称 / 空命令 → 跳过（计入 skipped）
        let value = serde_json::json!({ "Templates": [
            { "Name": "", "CommandArgs": "-i \"{input}\" \"{output}\"" },
            { "Name": "有名字", "CommandArgs": "" }
        ]});
        let result = service.import_json(&value, false).await.expect("导入");
        assert_eq!(result.imported, 0);
        assert_eq!(result.skipped, 2);

        // 坏条目 → 逐项报错，不影响其它条目
        let value = serde_json::json!({ "Templates": [
            123,
            { "Name": "好条目", "CommandArgs": "-i \"{input}\" -c copy \"{output}\"", "OutputExtension": "mp4" }
        ]});
        let result = service.import_json(&value, false).await.expect("导入");
        assert_eq!(result.imported, 1);
        assert_eq!(result.skipped, 1);
        assert_eq!(result.errors.len(), 1);
        assert!(result.errors[0].contains("第 1 项导入失败"), "错误：{:?}", result.errors);

        // 键名大小写敏感：小写 templates 不识别 → 0 条且不报错
        let value = serde_json::json!({ "templates": [
            { "Name": "x", "CommandArgs": "-i \"{input}\" \"{output}\"" }
        ]});
        let result = service.import_json(&value, false).await.expect("导入");
        assert_eq!(result.imported, 0);
        assert_eq!(result.skipped, 0);

        // 顶层结构错误 → Err（API 映射成 400）
        let error = service.import_json(&serde_json::json!([]), false).await.expect_err("顶层不是对象");
        assert!(error.to_string().starts_with("模板文件不是有效的 JSON："), "{error}");

        // asSystemTemplate = true → System + 当前预设版本
        let value = serde_json::json!({ "Templates": [
            { "Name": "管理员导入的系统模板", "CommandArgs": "-i \"{input}\" -c copy \"{output}\"" }
        ]});
        let result = service.import_json(&value, true).await.expect("导入");
        assert_eq!(result.imported, 1);
        let template = service
            .find_by_name("管理员导入的系统模板")
            .await
            .expect("查询")
            .expect("存在");
        assert_eq!(template.r#type, TemplateType::System);
        assert_eq!(template.preset_version, PRESET_VERSION);
    }

    // ══ 可用性 ═════════════════════════════════════════════════════════════

    #[tokio::test]
    async fn availability_marks_software_templates_available() {
        let service = service().await;
        let preset = preset_by_key("compress.crf23");
        assert_eq!(preset.hardware_mode, HardwareAccelerationMode::None);
        assert!(!preset.command_args.contains("{encoder}"));

        let availability = service.availability(&preset).await;
        assert!(availability.available, "纯软件模板必须判为可用：{:?}", availability.reason);
        assert!(availability.reason.is_none());
        assert!(availability.missing_filters.is_empty());
        assert!(!availability.uses_hardware);
        assert!(availability.recommended_encoder.is_none(), "不需要编码器画像");

        // 硬编码软件编码器的「快速换容器」同样可用
        let remux = service.availability(&preset_by_key("rec.fast_remux")).await;
        assert!(remux.available);
    }

    #[tokio::test]
    async fn availability_reports_missing_required_hardware() {
        let service = service().await;
        let availability = service.availability(&preset_by_key("hw.nvenc.h264")).await;
        assert!(availability.uses_hardware);
        if crate::paths::platform() == "macos" {
            assert!(!availability.available, "macOS 永远没有 NVENC");
            assert_eq!(
                availability.reason.as_deref(),
                Some("该模板要求使用 NVIDIA NVENC 硬件编码，但本机不可用。请改用「推荐」分类下会自动挑选编码器的模板，或在「设置 → 硬件加速能力」中确认本机支持的编码器。")
            );
        } else {
            // 非 macOS：有 NVENC 则可用，没有则必须给出同一句话
            assert_eq!(availability.available, availability.reason.is_none());
        }

        // Required 但没写编码器族 → 模板自身错误
        let broken = CommandTemplate {
            hardware_mode: HardwareAccelerationMode::Required,
            ..valid_template("x")
        };
        let availability = service.availability(&broken).await;
        assert!(!availability.available);
        assert_eq!(
            availability.reason.as_deref(),
            Some("模板要求必须使用硬件加速，但没有指定编码器族。这样在任何机器上都会失败。")
        );
    }

    #[tokio::test]
    async fn availability_reports_missing_filters() {
        let service = service().await;
        let template = CommandTemplate {
            required_filters: vec!["definitely_missing_filter_xyz".to_string()],
            ..valid_template("x")
        };
        let availability = service.availability(&template).await;
        assert!(!availability.available);
        assert_eq!(availability.missing_filters, vec!["definitely_missing_filter_xyz".to_string()]);
        assert!(availability
            .reason
            .expect("必须有原因")
            .contains("当前 FFmpeg 构建缺少该模板需要的滤镜"));
    }

    // ══ 试运行（真实 ffmpeg 9.0.2）═════════════════════════════════════════

    #[tokio::test]
    async fn trial_run_of_software_template_succeeds() {
        let service = service().await;
        // 「压缩 · 均衡（CRF 23)」：HardwareMode = None 且命令里没有 {encoder}
        // —— 正是参考实现 §8.6 缺陷 A 会误判成「缺少硬件编码器」的那类模板
        let preset = preset_by_key("compress.crf23");

        let result = service.test(&preset, &HashMap::new()).await.expect("试运行不返回 Err");

        assert!(result.success, "试运行必须成功：{}\n{}", result.message, result.output);
        assert!(result.message.starts_with("✅ 模板试运行成功"), "消息：{}", result.message);
        assert!(
            !result.message.contains("硬件编码器") && !result.message.contains("没有可用"),
            "不得复现缺陷 A：{}",
            result.message
        );
        assert!(result.command.starts_with("ffmpeg -hide_banner -y "), "命令：{}", result.command);
        assert!(result.command.contains("libx264"), "命令：{}", result.command);
        assert!(result.command.contains("sample.mp4"));
        assert!(result.duration_ms > 0, "耗时必须是实测毫秒");
    }

    #[tokio::test]
    async fn trial_run_with_parameter_override_succeeds() {
        let service = service().await;
        // 参数覆盖（`{audio_bitrate}` 会被替换成 96k）
        let preset = preset_by_key("rec.audio_mp3");
        let mut values = HashMap::new();
        values.insert("audio_bitrate".to_string(), "96k".to_string());

        let result = service.test(&preset, &values).await.expect("试运行不返回 Err");
        assert!(result.success, "试运行必须成功：{}\n{}", result.message, result.output);
        assert!(result.command.contains("96k"), "命令：{}", result.command);
        assert!(result.duration_ms > 0);
    }

    #[tokio::test]
    async fn trial_run_of_hardware_template_reports_documented_message() {
        let service = service().await;
        let preset = preset_by_key("hw.nvenc.h264");
        let result = service.test(&preset, &HashMap::new()).await.expect("试运行不返回 Err");

        if crate::paths::platform() == "macos" {
            assert!(!result.success);
            assert_eq!(
                result.message,
                "❌ 该模板要求使用 NVIDIA NVENC 硬件编码，但本机没有可用的对应编码器。请改用「推荐」分类下会自动挑选编码器的模板。"
            );
            assert_eq!(result.duration_ms, 0, "没跑 FFmpeg 就不该有耗时");
            assert_eq!(result.command, preset.command_args, "失败时展示原始模板串");
            assert!(result.output.is_empty());
        } else {
            // 非 macOS：要么成功，要么是同一句「没有可用编码器」，不得 panic
            if !result.success {
                assert!(result.message.starts_with("❌ 该模板要求使用"), "消息：{}", result.message);
            }
        }
    }

    #[tokio::test]
    async fn trial_run_fails_before_running_on_missing_filters_and_validation() {
        let service = service().await;

        // 缺滤镜：不跑 FFmpeg，直接给结论
        let missing_filter = CommandTemplate {
            required_filters: vec!["definitely_missing_filter_xyz".to_string()],
            ..valid_template("缺滤镜模板")
        };
        let result = service.test(&missing_filter, &HashMap::new()).await.expect("试运行");
        assert_eq!(
            result.message,
            "当前 FFmpeg 构建缺少该模板需要的滤镜：definitely_missing_filter_xyz。请换用其它模板，或改用带完整滤镜支持的 FFmpeg 构建（如 gyan.dev / Homebrew 的完整版）。"
        );
        assert_eq!(result.command, missing_filter.command_args);
        assert_eq!(result.duration_ms, 0);

        // 校验 Error：同样不跑
        let invalid = CommandTemplate { name: String::new(), ..valid_template("") };
        let result = service.test(&invalid, &HashMap::new()).await.expect("试运行");
        assert!(result.message.starts_with("模板校验未通过："), "消息：{}", result.message);
        assert!(result.message.contains("模板名称不能为空"));
        assert_eq!(result.duration_ms, 0);
    }

    #[tokio::test]
    async fn every_preset_passes_validation_without_errors() {
        let service = service().await;
        let report = service.hardware.detect(false).await;

        // §7 的 18 条规则：51 个预设都不应报 Error（滤镜清单传 None = 未知，不判定缺失）
        for preset in presets::all() {
            let issues = service.validate(&preset, None);
            let errors: Vec<&str> = issues
                .iter()
                .filter(|issue| issue.severity == TemplateIssueSeverity::Error)
                .map(|issue| issue.message.as_str())
                .collect();
            assert!(errors.is_empty(), "预设「{}」不应有 Error：{:?}", preset.name, errors);
        }

        // 传入真实滤镜清单后：报 Error 的预设必须**恰好**是那些本机真的缺滤镜的预设
        // （本机 Homebrew 的 ffmpeg 9.0.2 没有编译 `subtitles`，所以「烧录字幕」会命中，
        //   这正说明第 19 条检查有效；别的预设一条都不该被误报）
        for preset in presets::all() {
            let expected_missing = missing_filters(&preset, Some(&report.supported_filters));
            let issues = service.validate(&preset, Some(&report.supported_filters));
            let errors: Vec<String> = issues
                .iter()
                .filter(|issue| issue.severity == TemplateIssueSeverity::Error)
                .map(|issue| issue.message.clone())
                .collect();
            if expected_missing.is_empty() {
                assert!(errors.is_empty(), "预设「{}」被误报缺滤镜：{:?}", preset.name, errors);
            } else {
                assert_eq!(errors.len(), 1, "预设「{}」的错误：{:?}", preset.name, errors);
                assert!(
                    errors[0].contains("缺少该模板需要的滤镜")
                        && errors[0].contains(&expected_missing[0]),
                    "预设「{}」的错误消息：{}",
                    preset.name,
                    errors[0]
                );
            }
        }
    }

    // ══ 试运行的路径参数替换（§2.6）════════════════════════════════════════

    #[test]
    fn substitute_required_paths_replaces_only_path_like_parameters() {
        let sample = Path::new("/tmp/ffmpegwebui-test-sample.mp4");
        let template = CommandTemplate {
            parameters: vec![
                TemplateParameter { name: "watermark_image".to_string(), ..Default::default() },
                TemplateParameter { name: "image_path".to_string(), ..Default::default() },
                TemplateParameter { name: "crf".to_string(), ..Default::default() },
                TemplateParameter { name: "subtitle".to_string(), ..Default::default() },
            ],
            ..Default::default()
        };
        let mut values = HashMap::new();
        values.insert("subtitle".to_string(), sample.to_string_lossy().to_string());
        values.insert("crf".to_string(), "23".to_string());
        // 键比较忽略大小写（参考实现的参数字典是 OrdinalIgnoreCase）
        values.insert(
            "Watermark_Image".to_string(),
            "/definitely/not/exists.png".to_string(),
        );

        substitute_required_paths(&template, &mut values, sample);

        let watermark = values
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case("watermark_image"))
            .map(|(_, value)| value.clone());
        assert_eq!(
            watermark.as_deref(),
            Some(sample.to_string_lossy().as_ref()),
            "已被填成不存在路径的路径参数 → 测试素材"
        );
        assert_eq!(
            values.keys().filter(|key| key.eq_ignore_ascii_case("watermark_image")).count(),
            1,
            "命中原键时必须改写原键，而不是新增一个小写键"
        );
        assert_eq!(
            values.get("image_path").map(String::as_str),
            Some(sample.to_string_lossy().as_ref())
        );
        assert_eq!(values.get("crf").map(String::as_str), Some("23"), "非路径参数不动");
        assert_eq!(
            values.get("subtitle").map(String::as_str),
            Some(sample.to_string_lossy().as_ref()),
            "已存在且真实存在的文件 → 保留用户填的值"
        );
    }

    // ══ 规范化 / 排序 的纯函数行为 ═════════════════════════════════════════

    #[test]
    fn category_rank_follows_the_frozen_order() {
        assert_eq!(category_rank("推荐"), 0);
        assert_eq!(category_rank("特殊处理"), 6);
        assert_eq!(category_rank("  视频转码  "), 1, "比较前会 Trim");
        assert_eq!(category_rank(""), i32::MAX - 1);
        assert_eq!(category_rank("   "), i32::MAX - 1);
        assert_eq!(category_rank("我自己的分类"), i32::MAX - 2);
    }

    #[test]
    fn normalize_trims_braces_and_dedups_lists() {
        let mut template = CommandTemplate {
            name: "  a  ".to_string(),
            category: "".to_string(),
            output_extension: "..MP4".to_string(),
            supported_input_formats: vec![" MKV ".to_string(), ".mkv".to_string(), "".to_string()],
            tags: vec![" t ".to_string(), "t".to_string()],
            parameters: vec![TemplateParameter {
                name: "{{crf}}".to_string(),
                label: "  ".to_string(),
                kind: TemplateParameterKind::Select,
                options: vec![crate::models::TemplateParameterOption {
                    value: "23".to_string(),
                    label: "  ".to_string(),
                    description: String::new(),
                }],
                ..Default::default()
            }],
            ..Default::default()
        };
        normalize_template(&mut template);
        assert_eq!(template.name, "a");
        assert_eq!(template.category, DEFAULT_CATEGORY);
        assert_eq!(template.output_extension, "mp4");
        assert_eq!(template.supported_input_formats, vec!["mkv".to_string()]);
        assert_eq!(template.tags, vec!["t".to_string()]);
        assert_eq!(template.parameters[0].name, "crf", "{{crf}} → crf");
        assert_eq!(template.parameters[0].label, "crf");
        assert_eq!(template.parameters[0].options[0].label, "23", "空选项 label → value");
    }
}
