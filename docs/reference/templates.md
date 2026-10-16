# FFmpegWebUI 模板子系统移植规格（Rust 版实现依据）

> 本文档是 FFmpegWebUI 参考实现（C#/.NET 10 Blazor Server）中「命令模板」子系统的完整、逐字规格，
> 目标读者是 **FFmpegWebRust 的 Rust 实现者**。凡标注「逐字」的字符串，都必须原样复制到 Rust 源码中
> （包括中文全角/半角括号混用、末尾标点、空格数量）。凡标注「需决策」的内容，是参考实现的缺陷或
> 未被实现的部分，Rust 版必须自行拍板。
>
> **行号基准**：全部行号基于 `/Users/c01h/code/github/FFmpegWebUI` 仓库当前工作区（git HEAD `6eb4f3b`），
> 相对路径为 `FFmpegWebUI/`（仓库根目录下没有嵌套目录；例如 `Models/CommandTemplate.cs`）。
>
> **源码文件清单（本文档的全部依据）**
>
> | 文件 | 行数 | 作用 |
> |---|---|---|
> | `Models/CommandTemplate.cs` | 137 | `CommandTemplate` / `TemplateParameter` / `TemplateParameterOption` |
> | `Models/Enums.cs` | 107 | `TemplateType` / `TemplateParameterKind` / `HardwareAccelerationMode` / `TemplateIssueSeverity` 等 |
> | `Services/CommandLineBuilder.cs` | 489 | 分词、占位符替换、命令组装 |
> | `Services/TemplateService.cs` | 848 | CRUD、校验、试运行、导入导出、系统模板同步 |
> | `Services/ITemplateService.cs` | 83 | 服务接口与 `SystemTemplateSyncResult` |
> | `Services/SystemTemplatePresets.cs` | 658 | 51 个内置模板 + 分类顺序 + 预设版本 |
> | `Models/DTOs.cs` | 137 | `TemplateIssue` / `TemplateTestResult` |
> | `Models/HardwareEncoder.cs` | 87 | `HardwareEncoder` / `HardwareDetectionReport`（`HasFilter`） |
> | `Services/EncoderCatalog.cs` | 378 | `EncoderProfile` 编码器画像（`{quality_args}` 等的取值来源） |
> | `Services/ConversionQueue.cs` | 884 | 真实转换时如何装配 `CommandContext` 与挑选编码器 |
> | `Services/HardwareDetectionService.cs` | 559 | `NormalizeCodec`、编码器推荐、滤镜清单 |
> | `Data/LiteDbContext.cs` | — | LiteDB 集合名与索引（`templates`，`Name` 唯一索引） |
> | 附带证据 | — | `Components/Shared/TemplateParameterEditor.razor`（参数控件映射）、`tools/verify/TemplateChecks.cs`、`tools/verify/PipelineChecks.cs`（回归断言） |

---

## 0. 术语与职责边界

| 术语 | 含义 |
|---|---|
| 模板（template） | 一条 FFmpeg 命令的「参数模板」字符串 + 元数据 + 参数声明。**不包含 `ffmpeg` 可执行文件名，也不包含 `-hide_banner`/`-y`/`-progress`**（这些由执行层追加） |
| 占位符（placeholder） | `{name}` 形式的替换点，正则 `\{([A-Za-z_][A-Za-z0-9_]*)\}` |
| 参数（parameter） | 模板自己声明的占位符，界面据此生成输入控件 |
| 内置占位符 | 由程序（而非模板作者）提供取值的占位符集合，见 §4 |
| 参数组占位符 | 展开后可能产生 **0 个或多个 token** 的占位符（`{quality_args}` 等），见 §3.2、§4.2 |
| 编码器画像（EncoderProfile） | 每个 FFmpeg 视频/音频编码器的一组参数（质量参数、预设参数、显存上传滤镜、设备参数、容器参数） |
| 系统模板 | `Type == System` 的模板，随应用升级同步，用户不能删除/修改 |
| 用户模板 | `Type == User` 的模板，可自由增删改 |

**分层职责（Rust 版必须保持相同切分）**

1. `CommandLineBuilder`（`Services/CommandLineBuilder.cs:70-489`）：纯函数，把 `(模板, CommandContext)` 变成
   `BuiltCommand{ CommandLine, Arguments, EncoderName, Warnings, UnresolvedPlaceholders }`。
   **不做任何校验、不读数据库、不访问文件系统**（唯一的例外是 `Path.GetDirectoryName` 之类的纯字符串路径运算）。
2. `TemplateService`：CRUD + 校验 + 试运行 + 导入导出 + 同步。
3. `EncoderCatalog`：静态编码器画像表（无 IO）。
4. `HardwareDetectionService`：探测本机可用编码器与滤镜（有 IO、有缓存），向上面两层提供事实。
5. 执行层（`FFmpegService.RunAsync`）：`["-hide_banner", "-y" | "-n", "-progress", "pipe:2", ...Arguments]`
   写入 `ProcessStartInfo.ArgumentList`，**不经过 shell**。

---

## 1. 数据模型

### 1.1 `CommandTemplate` 逐字段规格

源码：`Models/CommandTemplate.cs:6-81`。C# 类型 → Rust 类型建议；「默认值」指 C# 属性初始值（新建对象时的值，**也是 LiteDB 反序列化缺失字段时的行为基准：LiteDB 会先构造对象再覆盖存在的字段，因此缺失字段保留下面的默认值**）。

| # | C# 字段 | C# 类型 | Rust 建议类型 | 默认值 | 语义 / 约束 | 源码行 |
|---|---|---|---|---|---|---|
| 1 | `Id` | `LiteDB.ObjectId` | `String`（12 字节十六进制，24 字符） | `ObjectId.NewObjectId()`（随机，非增量） | 主键。没有稳定字符串 key；系统模板同步靠 `Name` 匹配（见 §10） | 8 |
| 2 | `Name` | `string` | `String` | `""` | 模板名（中文）。**必须非空**；DB 层有唯一索引 | 11 |
| 3 | `Description` | `string` | `String` | `""` | 说明文字（中文，含换行时保留） | 14 |
| 4 | `CommandArgs` | `string` | `String` | `""` | FFmpeg 参数模板，含占位符。**不含 `ffmpeg` 前缀，不含 `-hide_banner`/`-y`** | 17 |
| 5 | `Type` | `TemplateType` | `enum TemplateType u8` | `User`(1) | System / User。系统模板不可删改（通过服务层拦截） | 20 |
| 6 | `Category` | `string` | `String` | `""` | 分类名（中文）。空时规范化为 `"未分类"`；导入时为空则 `"导入"` | 23 |
| 7 | `SupportedInputFormats` | `List<string>` | `Vec<String>` | `[]`（空 = 任意格式） | 输入扩展名白名单，小写、无点、去重、去空 | 26 |
| 8 | `OutputExtension` | `string` | `String` | `""` | 默认输出扩展名，**不含点**，小写。校验要求非空 | 29 |
| 9 | `TargetCodec` | `string` | `String` | `""` | `h264/hevc/av1/vp9/vp8/prores`，用于 `{encoder}` 自动挑选；空则按 `RequiredEncoder` → `OutputExtension` → 命令中出现的软件编码器 → 扩展名兜底 推断（§5.2） | 35 |
| 10 | `RequiresHardwareAcceleration` | `bool` | `bool` | `false` | **派生字段**：`NormalizeTemplate` 会强制置为 `HardwareMode == Required`；构建命令时**完全不使用**它 | 38 |
| 11 | `HardwareMode` | `HardwareAccelerationMode` | `enum` | `None`(0) | 硬件加速策略（§1.4） | 41 |
| 12 | `RequiredEncoder` | `string?` | `Option<String>` | `null` | 族名（`nvenc`/`qsv`/`amf`/`videotoolbox`/`vaapi`/`vulkan`）或具体编码器名（`h264_nvenc`）。空白 → 规范化为 `null` | 47 |
| 13 | `Parameters` | `List<TemplateParameter>` | `Vec<TemplateParameter>` | `[]` | 参数声明，界面据此生成控件 | 50 |
| 14 | `RequiredFilters` | `List<string>` | `Vec<String>` | `[]` | 模板依赖的 FFmpeg 滤镜名（如 `subtitles`）；用于提前判定「本机构建缺该滤镜」 | 57 |
| 15 | `Tags` | `List<string>` | `Vec<String>` | `[]` | 搜索标签；规范化时去空、Trim、按当前区域性去重 | 60 |
| 16 | `IsRecommended` | `bool` | `bool` | `false` | 「推荐」优先展示。**排序第一关键字** | 63 |
| 17 | `PresetVersion` | `int` | `i32` | `0` | 系统模板预设版本号，同步算法唯一依据（§10） | 66 |
| 18 | `CreatedAt` | `DateTime` | `chrono::DateTime<Utc>` | `DateTime.UtcNow` | 创建时间（UTC） | 69 |
| 19 | `UpdatedAt` | `DateTime` | 同上 | `DateTime.UtcNow` | 最后修改时间（UTC） | 72 |
| 20 | `SortOrder` | `int` | `i32` | `0` | 越小越靠前；内置模板按声明顺序 0..50；用户新建建议 `max+10` | 75 |
| 21 | `InputFormatsDisplay` | `string`（只读计算属性） | `fn input_formats_display()` | — | 空列表 → `"任意格式"`，否则 `string.Join(" / ", SupportedInputFormats)` | 78-80 |

**注意（重要）**：字段 21 是 C# 计算属性，**没有 setter，不参与 LiteDB 存储**（LiteDB 反序列化不会写它），
也**不在导出 JSON 中**。Rust 版不要把它建模为数据字段。

**不存在 `Hidden`/`Disabled` 之类的隐藏标记**：模板目录只有 `Type`（System/User）、`IsRecommended`、
`SortOrder`、`PresetVersion` 四个展示/管理相关字段。任何「隐藏」需求在参考实现里是靠「不加入预设列表」实现的
（例如 Vulkan Video 有编码器画像但没有对应模板，见 §5.1）。

### 1.2 `TemplateParameter` 逐字段规格

源码：`Models/CommandTemplate.cs:84-115`。

| # | C# 字段 | 类型 | 默认值 | 语义 | 行号 |
|---|---|---|---|---|---|
| 1 | `Name` | `string` | `""` | 占位符名（**不含花括号**）。规范化：`Trim().Trim('{','}')` | 87 |
| 2 | `Label` | `string` | `""` | 界面显示名。规范化：空 → 取 `Name` | 90 |
| 3 | `Description` | `string` | `""` | 帮助说明（Select 类型显示为所选选项的 `Description`，见 §2.1） | 93 |
| 4 | `Kind` | `TemplateParameterKind` | `Select`(0) | 控件类型，仅 4 种（§2.1） | 96 |
| 5 | `DefaultValue` | `string` | `""` | 默认值。**所有类型统一用字符串**（含 Number/Boolean） | 99 |
| 6 | `Options` | `List<TemplateParameterOption>` | `[]` | 仅 `Kind == Select` 使用 | 102 |
| 7 | `Min` | `double?` | `null` | 数字下界（仅用于界面 `min` 属性与提示，**不参与命令构建、不参与校验**） | 105 |
| 8 | `Max` | `double?` | `null` | 数字上界（同上） | 108 |
| 9 | `TrueValue` | `string` | `"1"` | Boolean 勾选时的取值 | 111 |
| 10 | `FalseValue` | `string` | `""` | Boolean 未勾选时的取值。**空串 = 该 token 被丢弃** | 114 |

**没有 `Step` 字段**（需求中提到的 step 在参考实现里不存在）：UI 固定 `step="any"`（`TemplateParameterEditor.razor:44`）。
Rust 版若需要 step，必须新增字段并保证 JSON 向后兼容（见 §9）。

### 1.3 `TemplateParameterOption` 逐字段规格

源码：`Models/CommandTemplate.cs:118-137`。

| # | C# 字段 | 类型 | 默认值 | 语义 | 行号 |
|---|---|---|---|---|---|
| 1 | `Value` | `string` | `""` | **提交给 FFmpeg 的值**（就是替换进命令行的文本） | 121 |
| 2 | `Label` | `string` | `""` | 界面显示文本。规范化：空白 → 取 `Value` | 124 |
| 3 | `Description` | `string` | `""` | 选中该选项时显示的补充说明 | 127 |

构造函数：`TemplateParameterOption(string value, string label, string description = "")`（`131-136`）。
C# 目标类型 `new("128k", "128 kbps")` 即 `Description = ""`。

**允许 `Value == ""`**（例如「自动（推荐）」项），其效果等价于「该参数取空值」→ 占位符所在 token 被丢弃（§4.2）。

### 1.4 枚举全表（数值即持久化/JSON 表示）

源码：`Models/Enums.cs`。

| 枚举 | 成员（数值） | 行号 |
|---|---|---|
| `TemplateType` | `System = 0`, `User = 1` | 4-10 |
| `HardwareAccelerationMode` | `None = 0`（纯软件）, `Auto = 1`（有硬件优先，否则回退软件）, `Required = 2`（必须是指定族，否则模板不可用） | 53-61 |
| `TemplateParameterKind` | `Select = 0`, `Number = 1`, `Text = 2`, `Boolean = 3` | 86-96 |
| `TemplateIssueSeverity` | `Info = 0`, `Warning = 1`, `Error = 2` | 99-107 |
| `EncoderType`（模板间接依赖） | `Software = 0`, `Nvenc = 1`, `Qsv = 2`, `Amf = 3`, `VideoToolbox = 4`, `Vaapi = 5`, `Vulkan = 6`, `MediaFoundation = 7`, `Other = 99` | 30-50 |
| `TaskStatus` | `Pending = 0`, `Running = 1`, `Completed = 2`, `Failed = 3`, `Cancelled = 4`, `Skipped = 5` | 13-27 |
| `OutputNamingRule` | `Original = 0`, `Suffix = 1` | 64-70 |
| `FileExistsAction` | `Ask = 0`, `Overwrite = 1`, `Skip = 2`, `Rename = 3` | 73-83 |

**枚举持久化表示**：LiteDB 默认把 enum 存为整数；`System.Text.Json` 在导出/导入时也**没有**注册
`JsonStringEnumConverter`，因此 JSON 中 `type`/`kind`/`hardwareMode` 全是**整数**（§9）。
Rust 版如果要用 serde 整数枚举，必须显式 `#[serde(rename_all)]` 不要加，保持数字。

### 1.5 存储表示

- 数据库：LiteDB 单文件，默认 `AppPaths.DataDirectory/data.db`（可用配置 `FFmpegWebUI:DatabasePath` 覆盖），
  见 `Data/LiteDbContext.cs`。
- 集合名：**`templates`**；同库还有 `tasks` / `batch_tasks` / `encoders` / `settings`。
- 索引：`Templates.EnsureIndex(x => x.Name, unique: true)`（**Name 全局唯一**，System 与 User 也不能同名）、
  `x => x.Type`、`x => x.Category`。
- BSON 字段名 = C# 属性名（PascalCase），`Id` 映射为 BSON `_id`。
- 时间以 UTC 存入（LiteDB 存为 UTC `DateTime`）。
- **Rust 版需决策**：LiteDB 文件格式无法在 Rust 中直接读写。建议换成 SQLite/JSON 文件并把「Name 唯一」
  约束提到应用层（参考实现同时有 DB 唯一索引 + 应用层 `EnsureUniqueName` 双重保证，见 §1.7）。

### 1.6 `NormalizeTemplate` 规范化算法（逐行等价伪代码）

源码：`Services/TemplateService.cs:772-813`。**任何写入路径（创建/更新/复制导入）都会调用它**，
所以在 Rust 中它应当在所有写操作的入口处执行一次。

```
fn normalize_template(t: &mut CommandTemplate) {
    t.name        = (t.name).trim();
    t.description = (t.description).trim();
    t.command_args= (t.command_args).trim();            // 去掉命令模板首尾空白

    t.category    = if blank(t.category) { "未分类" } else { t.category.trim() };

    // 扩展名：Trim → 去前导 '.' → 小写
    t.output_extension = t.output_extension.trim().trim_start_matches('.').to_lowercase();

    t.target_codec = t.target_codec.trim();

    // 空白 → None（可空字段）
    t.required_encoder = if blank(t.required_encoder) { None } else { Some(trim) };

    // 派生：硬件加速标志永远由策略推导，不信任调用方传入的值
    t.requires_hardware_acceleration = (t.hardware_mode == Required);

    // 注：源码这里有一段空 if（785-789），刻意不做「Required 但没写编码器族」的静默修正，
    //     而是留给 Validate() 报错。Rust 版保持同样选择。

    // 输入格式：去空 → Trim → 去前导 '.' → 小写 → 去重（保序）
    t.supported_input_formats = dedup_preserve_order(
        t.supported_input_formats.filter(|f| !blank(f)).map(|f| f.trim().trim_start_matches('.').to_lowercase()));

    // 标签：去空 → Trim → 去重（按当前区域性比较，Rust 用精确比较即可）
    t.tags = dedup_preserve_order(t.tags.filter(|s| !blank(s)).map(|s| s.trim()));

    for p in &mut t.parameters {
        p.name  = p.name.trim().trim_matches('{').trim_matches('}');   // 允许用户写 "{crf}" 也允许 "crf"
        p.label = if blank(p.label) { p.name.clone() } else { p.label.trim() };
        p.default_value = p.default_value.unwrap_or_default();          // C#: ??= string.Empty
        for o in &mut p.options {
            o.label = if blank(o.label) { o.value.clone() } else { o.label.clone() };
        }
    }
}
```

注意：`Trim('{','}')` 只剥离**首尾**的花括号，`{{a}}` → `a`；参数名内部的空格不会被去掉（`"a b"` 保持
`"a b"`，但这样的名字永远无法被 `{...}` 正则匹配到 → 将是「死参数」，校验仅在 `Name` 全空白时报 Warning）。

### 1.7 `EnsureUniqueName` 算法

源码：`Services/TemplateService.cs:815-832`。被 `CreateTemplateAsync` / `UpdateTemplateAsync` /
`CopyTemplateAsync` / `CreateSystemTemplateAsync` / `UpdateSystemTemplateAsync` / `ImportFromJsonAsync` 调用。

```
fn ensure_unique_name(name: Option<&str>, exclude_id: Option<ObjectId>, db) -> String {
    let base = if blank(name) { "未命名模板" } else { name.trim() };
    let mut candidate = base.clone();
    let mut counter = 1;
    loop {
        // 全库查询（含 System 模板），排除自身
        let conflict = db.find_by_name(&candidate).into_iter()
                          .find(|t| exclude_id.is_none() || t.id != exclude_id.unwrap());
        if conflict.is_none() { return candidate; }
        counter += 1;
        candidate = format!("{base} ({counter})");   // 注意半角括号 + 空格
    }
}
```

行为：第二次冲突得到 `原名 (2)`，第三次 `原名 (3)`，依此类推（**从 2 开始，不是 1**）。
`CopyTemplateAsync` 的默认新名字是 `"{source.Name} (副本)"`（`TemplateService.cs:147`）。

判定为「冲突」的方式是**精确字符串相等**（`t.Name == candidate`，区分大小写、区分全半角）。

### 1.8 查询与排序规则

| 方法 | 源码行 | 行为 |
|---|---|---|
| `GetAllTemplatesAsync(includeSystem = true)` | 39-55 | `includeSystem == false` 时只取 `Type == User`。排序：`IsRecommended` **降序** → `CategoryRank(Category)` 升序 → `SortOrder` 升序 → `Name` 按当前区域性升序 |
| `GetTemplatesByCategoryAsync(category)` | 57-68 | 精确匹配 `Category`；排序：`SortOrder` → `Name` |
| `GetTemplateByIdAsync(ObjectId)` | 70-72 | 找不到返回 `null` |
| `GetCategoriesAsync()` | 74-88 | 取库中所有非空 `Category` 去重，按 `CategoryRank` → 名称排序 |
| `CategoryRank(category)` | 91-96 | 空白 → `i32::MAX - 1`；在 `CategoryOrder` 中 → 下标；未知分类 → `i32::MAX - 2`。**即：已知分类按预设顺序，空白分类排在未知分类之前，两者都在最后** |

`CategoryOrder`（`SystemTemplatePresets.cs:16-25`，逐字顺序）：

```
["推荐", "视频转码", "硬件加速", "视频压缩", "分辨率与画面", "音频处理", "特殊处理"]
```

> 这 7 个分类名是**写死的**；用户自建分类（如 `"导入"`、`"未分类"`）永远排在它们之后。

---

## 2. 参数模型

### 2.1 四种 `Kind` 与控件映射

证据：`Components/Shared/TemplateParameterEditor.razor:24-62`（真实渲染逻辑）。

| Kind | 数值 | 控件 | 写入字典的值 | 备注 |
|---|---|---|---|---|
| `Select` | 0 | `<select>`，选项文本 = `option.Label`，`title` = `option.Description`，`value` = `option.Value` | 选中项的 `Value`（可为 `""`） | 若选中项 `Description` 非空，显示在控件下方（`TemplateParameterEditor.razor:35-38`, `145-148`；按 `Value` 精确匹配取第一条） |
| `Number` | 1 | `<input type="number" min="{Min}" max="{Max}" step="any">` | 输入框原始文本（**不解析、不夹紧、不校验**） | 标签右侧显示 `(min ~ max)`，未设的用 `—` / `∞`（`16-21`） |
| `Text` | 2 | `<input type="text">`，`placeholder` = `DefaultValue` | 输入框原始文本 | `Description` 显示在控件下方 |
| `Boolean` | 3 | `<input type="checkbox">` | 勾选 → `TrueValue`；取消 → `FalseValue` | 右侧文字 `已启用` / `已关闭` |

**Boolean 的「真」判定**（`TemplateParameterEditor.razor:138-143`，仅影响复选框的初始勾选状态）：

```
fn is_boolean_true(param, value) -> bool {
    if value.is_empty() { return false; }
    value.eq_ignore_ascii_case(&param.true_value)
        || matches!(value, "1" | "true" | "True" | "yes" | "on")
}
```

**取值字典**：`Dictionary<string,string>`，**键比较器是 `OrdinalIgnoreCase`**（大小写不敏感）。
未在字典中的参数回退到 `DefaultValue`（`134-135`）。字典实例在每次修改时**复制后整体替换**
（`153-163`），Rust 中相当于每次返回新的 `HashMap`。

### 2.2 参数值如何进入命令行

唯一通路是**占位符替换**：`context.Parameters[param.Name]` → 解析表 `map[name]` → 替换 `{name}`。
参数值**不会被引号包裹、不会被转义、不会被 shell 解释**（命令直接走 `argv`）。

由此推导出的三条硬规则（Rust 版必须保持）：

1. 参数值里含空格时，命令模板必须自己写引号：`-vf "scale={width}:-2"` → 分词后是**一个** token
   `scale=480:-2`（引号只用于分组、会被丢弃，见 §4.3）。
2. 参数值若包含 `{`/`}` 不会二次展开（替换是单遍正则替换，替换结果不再扫描）。
3. 参数值若为空字符串（或 `FalseValue = ""`），则
   - 若该占位符独占一个 token → 该 token **整体消失**（不产生空参数）；
   - 若混在其它文本中 → 变成「空字符串拼接」，可能产生 `-vf ""` 这类**空 token**（见 §4.8 怪癖 3）。

### 2.3 默认值字典

源码：`Services/TemplateService.cs:270-281`。

```
fn create_default_parameter_values(t: &CommandTemplate) -> HashMap<String,String> {  // 键序 OrdinalIgnoreCase
    let mut m = HashMap::new();
    for p in &t.parameters {
        if !blank(p.name) { m.insert(p.name.clone(), p.default_value.clone().unwrap_or_default()); }
    }
    m
}
```

- 空名参数**不进字典**（界面上也不渲染，`TemplateParameterEditor.razor:129-130`）。
- 空字符串默认值会进字典，但 `CommandLineBuilder.BuildResolver` **会跳过空值**（§4.4），
  效果等同于「用内置默认值」。
- 执行层在创建任务时还会用 `task.TemplateParameters` 覆盖（`ConversionQueue.cs:488-498`），
  所以 Rust 版应当把「参数值」作为任务的一部分持久化（重试时使用同一套值）。

### 2.4 `TrueValue` / `FalseValue` 的真实作用域（重要）

它们**只在界面层被使用**（`TemplateParameterEditor.razor:52`），把复选框状态翻译成字符串写进参数字典。
`CommandLineBuilder` 完全不知道 Boolean 语义：它只是把字典里的字符串替换进去。
因此：

- 模板作者必须自己保证 `TrueValue` 是合法参数（默认 `"1"`）；`FalseValue` 为空 → 该 token 消失。
- 参考实现的 **51 个内置模板中没有任何一个使用 `Boolean` 参数**（全部是 Select / Number / Text）。
  Rust 版若实现 Boolean，行为应与上述一致即可。

### 2.5 校验时的参数相关规则（摘录，完整表见 §7）

- 参数名与内置占位符同名 → Warning（但 `quality` / `q` / `crf` 三个例外，被明确允许覆盖）。
- `Kind == Select` 且 `Options` 为空 → Warning。
- 参数名为空 → Warning「存在一个没有名字的参数，它不会被使用。」并 `continue`（跳过后续检查）。

### 2.6 试运行时的路径参数替换

源码：`Services/TemplateService.cs:604-623`（试运行专用，正常转换不做此替换）。

```
fn substitute_required_paths(template, params: &mut HashMap<String,String>, sample_path, _workdir) {
    for p in &template.parameters {
        if blank(p.name) { continue; }

        let name_l = p.name.to_lowercase();
        let looks_like_path = name_l.contains("path") || name_l.contains("file")
                           || name_l.contains("watermark") || name_l.contains("subtitle")
                           || name_l.contains("image");
        if !looks_like_path { continue; }

        let unresolved = params.get(p.name).map_or(true, |v| v.trim().is_empty());
        // 已有值且该文件真实存在 → 保留用户填的值
        if !unresolved && Path::new(&to_absolute(params[p.name])).exists() { continue; }

        params.insert(p.name.clone(), sample_path.to_owned());  // 否则替换成测试素材
    }
}
```

`to_absolute` = `AppPaths.ToAbsolute`（`Services/AppPaths.cs:118-131`）：Trim → 去首尾 `"` → 展开 `~` →
`Path.GetFullPath`（失败则返回展开后的字符串）。

`tools/verify/TemplateChecks.cs:184-207` 是同一逻辑的强化版（对名字含 `subtitle` 的参数生成一份真实的
`srt`，其余替换为 mp4），可作为 Rust 测试的参照。

---

## 3. 占位符参考

### 3.1 语法与命名

- 正则（`CommandLineBuilder.cs:72`）：`\{([A-Za-z_][A-Za-z0-9_]*)\}` —— 名称必须**以字母或下划线开头**，
  后续只允许字母/数字/下划线。非 ASCII（中文名）、含 `-`、以数字开头、含点号的 `{...}` **不是占位符**，
  会被原样保留且**不会产生「未解析」告警**。
- 整 token 判定正则（`73`）：`^\{([A-Za-z_][A-Za-z0-9_]*)\}$`。
- `ExtractPlaceholders(text)`（`230-235`）：抽取全部占位符名并**按 `OrdinalIgnoreCase` 去重**（保序）。
  校验与界面都用它。
- 比较：所有占位符名比较都是**大小写不敏感**（`OrdinalIgnoreCase`）。`{INPUT}` 等价 `{input}`。

### 3.2 参数组占位符集合（逐字）

`CommandLineBuilder.cs:79-83`：

```
quality_args, preset_args, hwupload_args, device_args, muxer_args, hwaccel, global_args, extra_args
```

语义：**这些占位符单独占一个 token 时，会展开成 0..N 个 token**（先取字符串值，再用 `Tokenize` 切分）。
若它们与其它文本写在同一个 token 里 → 只做纯文本替换，并产生一条 Warning（§4.2）。

### 3.3 内置占位符集合（逐字）

`CommandLineBuilder.cs:238-247`。这些占位符**不需要**模板声明参数；`Validate` 不会因它们报「未声明」。

```
input, output, output_pattern,
input_dir, input_name, input_filename, input_ext,
output_dir, output_name, output_ext,
duration, width, height, fps, vcodec, acodec, sample_rate, channels,
encoder, audio_encoder, crf, quality, q,
quality_args, preset_args, hwupload_args, device_args, muxer_args, extra_args,
threads, hwaccel, global_args, date, time, datetime, random
```

（共 37 个；含 8 个参数组占位符。**注意列表末尾的逗号是 C# 集合初始化器的尾逗号，不是元素**。）

### 3.4 内置占位符取值表（`BuildResolver`，`CommandLineBuilder.cs:400-481`）

| 占位符 | 取值 | 空值行为 |
|---|---|---|
| `input` | `context.InputPath` | 空字符串 token → 丢弃（占位符独占 token 时） |
| `output` | `context.OutputPath` | 同上 |
| `output_pattern` | `Path.Combine(dir(output), f"{stem(output)}_%04d{ext(output)}")`；`output` 为空则空 | 用于图片序列（`result_0001.jpg`） |
| `input_dir` | `Path.GetDirectoryName(input)` | — |
| `input_name` | `Path.GetFileNameWithoutExtension(input)` | 不含扩展名 |
| `input_filename` | `Path.GetFileName(input)` | 含扩展名 |
| `input_ext` | `Path.GetExtension(input).TrimStart('.')` | **保留原始大小写** |
| `output_dir` / `output_name` / `output_ext` | 同上，针对 `output` | — |
| `duration` | `Media.Duration`，格式 `"0.###"`，InvariantCulture；无媒体信息 → `"0"` | 总是有值 |
| `width` / `height` | 媒体宽/高，`> 0` 时十进制；否则空 | 空 → token 消失 |
| `fps` | `Media.FrameRate`，格式 `"0.###"`；`<= 0` → 空 | — |
| `vcodec` / `acodec` | 媒体视频/音频编码名（如 `h264`、`aac`）；无 → 空 | — |
| `sample_rate` | `Media.AudioSampleRate > 0 ? 该值 : "44100"` | — |
| `channels` | `Media.AudioChannels > 0 ? 该值 : "2"` | — |
| `encoder` | `Encoder.Name`；**无画像时默认 `"libx264"`** | 不会空 |
| `audio_encoder` | `context.AudioEncoder`（默认 `"aac"`） | — |
| `threads` | `context.Threads > 0 ? 该值 : 空` | 空 → token 消失 |
| `hwaccel` | `context.HardwareDecodeArgs`（如 `-hwaccel videotoolbox`，含前导 `-hwaccel`） | 空 → token 消失 |
| `crf` / `quality` / `q` | 见 §3.5 | 总是有值 |
| `quality_args` | `Expand(Encoder.QualityArgs)` 后把 `{q}` 字面替换为质量数值 | 无画像/无质量参数 → `""` → 展开为空 |
| `preset_args` | `Expand(Encoder.PresetArgs)` | 同上 |
| `hwupload_args` | `Encoder.UploadFilter` 为空 → `""`；否则 `Expand(UploadFilter)`（如 `format=nv12,hwupload`） | — |
| `device_args` / `muxer_args` | **解析表中固定为空字符串**（由 `Build` 的后处理负责插入，避免重复注入） | token 消失 |
| `date` | `DateTime.Now.ToString("yyyyMMdd")`（**本地时间**） | — |
| `time` | `DateTime.Now.ToString("HHmmss")` | — |
| `datetime` | `DateTime.Now.ToString("yyyyMMdd_HHmmss")` | — |
| `random` | `Guid.NewGuid().ToString("N")` 的前 8 个字符（十六进制小写） | — |

> `input_dir` / `input_name` / `input_filename` / `output_dir` / `output_name` 的取值会经过
> `SafePath`（`CommandLineBuilder.cs:483-488`），该函数目前是**空实现**：非空时原样返回
> （注释说「去掉可能的尾部斜杠以便拼接」，但代码并没有做）。Rust 版直接原样返回即可，不要自行 Trim。

> **`{time}` 陷阱**：「生成封面缩略图」模板故意把参数命名为 `seek`，因为 `{time}` 是内置的动态时间占位符
> （源码注释见 `SystemTemplatePresets.cs:541`）。Rust 版移植时务必保留这一说明，否则会踩同样的坑。

### 3.5 质量值（`crf` / `quality` / `q`）的优先级

`BuildResolver` 的精确顺序（`CommandLineBuilder.cs:439-461`）：

```
defaultQuality = context.Quality ?? encoder.DefaultQuality ?? 23     // EncoderProfile.DefaultQuality 默认 23
map["crf"] = map["quality"] = map["q"] = defaultQuality

for (key, value) in context.Parameters {          // 用户参数覆盖内置值
    if key in {"input","output"}      { continue }     // 保护 input/output 不被参数覆盖
    if value.is_empty()               { continue }     // 空值 = 使用默认
    map[key] = value                                    // 因此名为 quality/crf/q 的参数在此生效
}

quality = i32::from_str(map["q"]).unwrap_or(defaultQuality)   // 解析失败回退默认
map["crf"] = map["quality"] = map["q"] = quality.to_string()

map["quality_args"] = expand(encoder.QualityArgs).replace("{q}", quality)   // 注意：replace 是字面替换
map["preset_args"]  = expand(encoder.PresetArgs)
map["hwupload_args"]= if encoder.UploadFilter.is_empty() { "" } else { expand(encoder.UploadFilter) }
map["device_args"] = "";  map["muxer_args"] = ""
```

推论（Rust 版必须复现）：

1. 用户参数 `quality` / `crf` / `q`（三者任一名）会**同时**改写这三个占位符。
2. 非数字值（如 `"high"`）会让三个占位符一起回退到 `defaultQuality`；**不会报错**。
3. `{preset}`（x264 的 `-preset {preset}`）**不是**内置占位符，只能由模板参数提供；
   内置模板里它由参数 `preset`（Select，默认 `medium`）驱动。
4. `context.Parameters` 里名为 `input`/`output` 的键被**显式忽略**，但会**留在**参数字典里（不影响其它逻辑）。

### 3.6 `CommandContext` 全字段

源码：`CommandLineBuilder.cs:8-39`。

| 字段 | 类型 | 默认值 | 说明 |
|---|---|---|---|
| `InputPath` | `string` | `""` | 输入文件绝对路径 |
| `OutputPath` | `string` | `""` | 输出文件绝对路径（也是后处理插入位置的定位锚） |
| `Parameters` | `Dictionary<string,string>`（OrdinalIgnoreCase） | 空 | 用户参数值 |
| `Encoder` | `EncoderProfile?` | `null` | 已解析的视频编码器画像；决定 `{encoder}`/`{quality_args}`/`{preset_args}`/`{hwupload_args}`/设备/容器参数 |
| `AudioEncoder` | `string` | `"aac"` | `{audio_encoder}` |
| `Quality` | `int?` | `null` | 覆盖画像默认质量 |
| `Media` | `MediaInfo?` | `null` | 媒体信息，供 `{duration}` 等 |
| `Threads` | `int` | `0` | `0` = 不指定（`{threads}` 为空）。参考实现的三个调用点**都传 0** |
| `HardwareDecodeArgs` | `string` | `""` | 如 `-hwaccel videotoolbox`；非空时自动注入到输入之前 |
| `GlobalArguments` | `string` | `""` | 用户设置里的全局附加参数，追加到输出之前 |

### 3.7 真实转换时的上下文装配（`ConversionQueue.cs:480-516`，供 Rust 对齐行为）

```
parameters = { p.name -> p.default_value }                      // 全量默认值
if task.template_parameters.is_some() { parameters.merge(task.template_parameters) }   // 任务上的值优先

context = CommandContext {
    input_path: task.input_path,
    output_path: task.output_path,
    parameters,
    encoder,
    media: media_info,
    audio_encoder: resolve_audio_encoder(template, encoder),
    threads: 0,
    hardware_decode_args: if settings.prefer_hardware_decoding && !disable_hardware_decode
                          { build_hardware_decode_args(media) } else { "" },
    global_arguments: settings.global_ffmpeg_arguments,
}
```

- `ResolveAudioEncoder`（`ConversionQueue.cs:608-622`）：命令模板中包含 `libopus` → 用它；否则
  `libmp3lame` → `flac` → `aac`；都不含则按输出扩展名映射（`webm`→`libopus`，`mp3`→`libmp3lame`，
  `flac`→`flac`，`ogg`→`libopus`，其它→`aac`）。
- `BuildHardwareDecodeArgs`（`624-638`）：无媒体信息或无视频流 → `""`；视频编码属于
  `h264/hevc/av1/vp9/mpeg2video/vc1` 时才返回 `-hwaccel <平台默认>`，平台默认：
  macOS → `videotoolbox`，Windows → `d3d11va`，其它 → `vaapi`。
- 重试路径：首次失败且勾选了「硬件解码」时用 `disableHardwareDecode: true` 重试；再失败则用软件编码器重试
  （`ConversionQueue.cs:243/298/315`）。Rust 版的失败降级策略可参照，但**不属于模板子系统**。

---

## 4. `CommandLineBuilder` 完整算法

### 4.1 对外 API

```rust
struct BuiltCommand {
    command_line: String,                 // 展示 / 复制用（已加引号渲染）
    arguments: Vec<String>,               // 真正传给进程的 argv（不经 shell）
    encoder_name: Option<String>,
    warnings: Vec<String>,                // 中文告警
    unresolved_placeholders: Vec<String>, // 去重后仍未解析的占位符名（不含花括号）
}

fn build(template: &CommandTemplate, context: &CommandContext) -> BuiltCommand
fn render_command_line(args: &[String]) -> String
fn extract_placeholders(command_args: &str) -> Vec<String>
fn tokenize(input: &str) -> Vec<String>
```

### 4.2 `Build` 总流程（伪代码，步骤编号对应源码注释）

```
fn build(template, context) -> BuiltCommand {
    let mut warnings = vec![];
    let mut unresolved = vec![];

    // 1) 先分词：此占位符仍是普通字符（含花括号）
    let mut tokens = tokenize(&template.command_args);

    // 2) 显存上传滤镜并入已有 -vf（仅当 encoder.upload_filter 非空）
    if let Some(uf) = &context.encoder.as_ref().and_then(|e| e.upload_filter.as_ref()).filter(|s| !s.is_empty()) {
        tokens = merge_upload_filter(tokens, uf, &template.command_args, &mut warnings);
    }

    // 3) token 级替换：参数组展开为多 token；空值 token 消失
    let resolver = build_resolver(context, &mut unresolved);
    let mut arguments: Vec<String> = vec![];
    for token in tokens {
        if let Some(name) = whole_placeholder(&token) {           // ^\{name\}$
            if ARGUMENT_GROUP_PLACEHOLDERS.contains_ci(name) {
                if let Some(g) = resolver.get(name).filter(|g| !g.trim().is_empty()) {
                    arguments.extend(tokenize(g));                // 展开为多个 token
                }
                continue;                                          // 空值 → 不产生参数
            }
            match resolver.get(name) {
                Some(v) if !v.is_empty() => arguments.push(v.clone()),
                Some(_)                  => {}                     // 空值 → 丢弃整个 token
                None => { unresolved.push(name); arguments.push(token); }  // 原样保留
            }
            continue;
        }
        // 占位符混在其它文本中：纯文本替换
        arguments.push(PLACEHOLDER_RE.replace_all(&token, |m| {
            let key = m.name;
            if ARGUMENT_GROUP_PLACEHOLDERS.contains_ci(key) {
                if let Some(g) = resolver.get(key) {
                    warnings.push(format!(
                        "占位符 {{{key}}} 与其它文本写在了同一个参数里，它会被当成一个整体传入，可能导致 FFmpeg 解析失败。请把它单独写成一个参数（前后加空格）。"));
                    return g.clone();
                }
            }
            match resolver.get(key) {
                Some(v) => v.clone(),
                None => { unresolved.push(key); m.to_string() }     // 保留 {key}
            }
        }));
    }

    // 4) 补充硬件解码参数：模板没写 -hwaccel 时自动注入到输入之前
    if !blank(context.hardware_decode_args)
        && !arguments.iter().any(|a| a.starts_with("-hwaccel")) {
        let decode_tokens = tokenize(&context.hardware_decode_args);
        if !decode_tokens.is_empty() {
            let input_index = arguments.iter().position(|a| a == &context.input_path);  // 首次出现
            let insert_at = if let Some(i) = input_index { if i > 0 { i - 1 } else { 0 } } else { 0 };
            splice(&mut arguments, insert_at, decode_tokens);
        }
    }

    // 5) 补充设备参数（如 -vaapi_device /dev/dri/renderD128）
    let device_args = expand(context.encoder.device_args, resolver, &mut unresolved);
    if !blank(device_args) && !arguments.contains(first_token(&device_args)) {
        let device_tokens = tokenize(&device_args);
        let insert_at = arguments.iter().position(|t| t.starts_with('-') && t != "-y" && t != "-n")
                              .unwrap_or(0);
        splice(&mut arguments, insert_at, device_tokens);
    }

    // 6) 补充容器参数（如 -tag:v hvc1）——仅当模板未显式指定任何 -tag*
    let muxer_args = expand(context.encoder.muxer_args, resolver, &mut unresolved);
    if !blank(muxer_args) && !arguments.iter().any(|a| a.starts_with("-tag")) {
        let muxer_tokens = tokenize(&muxer_args);
        let output_index = arguments.iter().rposition(|a| a == &context.output_path).unwrap_or(arguments.len());
        splice(&mut arguments, output_index, muxer_tokens);
    }

    // 7) 追加全局参数到输出之前
    if !blank(context.global_arguments) {
        let global_tokens = tokenize(&context.global_arguments);
        let output_index = arguments.iter().rposition(|a| a == &context.output_path).unwrap_or(arguments.len());
        splice(&mut arguments, output_index, global_tokens);
    }

    // 8) 未解析占位符去重 + 汇总告警
    let distinct: Vec<String> = dedup_ci(unresolved);
    if !distinct.is_empty() {
        warnings.push(format!("以下占位符没有对应取值，将原样传给 FFmpeg，可能导致失败：{}",
            distinct.iter().map(|p| format!("{{{p}}}")).join(", ")));   // 逗号+空格 分隔
    }

    BuiltCommand {
        command_line: render_command_line(&arguments),
        arguments,
        encoder_name: context.encoder.map(|e| e.name),
        warnings,
        unresolved_placeholders: distinct,
    }
}
```

**注意步骤顺序**：3 → 4 → 5 → 6 → 7。设备参数插入在解码参数之后、容器参数之前。
所有插入都是**就地拼接**（`InsertRange`），索引按当前列表实时计算。

### 4.3 `Tokenize` 逐步规格（逐行等价，`CommandLineBuilder.cs:258-334`）

类 POSIX 分词，但**只支持**单引号、双引号、反斜杠转义；引号是纯粹的分组符，**分词后不保留**。

```
fn tokenize(input: &str) -> Vec<String> {
    let mut result = vec![];
    if input.trim().is_empty() { return result; }        // 纯空白 → 空列表（注意：是 trim 后判空）

    let mut current = String::new();
    let mut has_token = false;
    let mut in_single = false;
    let mut in_double = false;
    let chars: Vec<char> = input.chars().collect();
    let mut i = 0;

    while i < chars.len() {
        let c = chars[i];

        if in_single {
            if c == '\'' { in_single = false; } else { current.push(c); }
            i += 1; continue;
        }

        if in_double {
            if c == '\\' && i + 1 < chars.len() && (chars[i+1] == '"' || chars[i+1] == '\\') {
                i += 1; current.push(chars[i]);          // 只对 \" 和 \\ 做转义
            } else if c == '"' {
                in_double = false;
            } else {
                current.push(c);                         // 其余反斜杠原样保留
            }
            i += 1; continue;
        }

        match c {
            '\\' if i + 1 < chars.len() => { i += 1; current.push(chars[i]); has_token = true; }
            '\'' => { in_single = true; has_token = true; }
            '"'  => { in_double = true; has_token = true; }
            ' ' | '\t' | '\r' | '\n' => {
                if has_token || !current.is_empty() {
                    result.push(std::mem::take(&mut current));
                    has_token = false;
                }
            }
            _ => { current.push(c); has_token = true; }
        }
        i += 1;
    }

    if in_single || in_double {
        // 引号未闭合：把已有内容作为最后一个参数，避免整条命令丢失（不报错！）
        if !current.is_empty() || has_token { result.push(current); }
        return result;
    }
    if has_token || !current.is_empty() { result.push(current); }
    result
}
```

必须复现的边界行为：

| 输入 | 输出 | 说明 |
|---|---|---|
| `-i "{input}" -c:v libx264 "{output}"` | `["-i","{input}","-c:v","libx264","{output}"]` | 引号被剥离 |
| `-vf "scale=480:-2:flags=lanczos"` | `["-vf","scale=480:-2:flags=lanczos"]` | 含 `:` 不分词 |
| `"a b"c` | `["a bc"]` | 引号内外拼接成同一 token |
| `''` 或 `""` | `[""]`（**空字符串 token**） | `has_token = true` 导致空 token 被 emit |
| `a\ b` | `["a b"]` | 反斜杠转义空格 |
| `a\`（末尾单个反斜杠） | `["a\\"]` | `when i+1 < len` 不成立 → 落到 `default`，反斜杠作为普通字符 |
| `"a\"b"` | `["a\"b"]`（内容为 `a"b`） | 双引号内 `\"` 转义 |
| `'a\'b'` | `["a\\b"]` | **单引号内没有转义**，`\` 是字面量；`'` 会提前闭合 |
| `"unclosed` | `["unclosed"]` | 不抛错，静默返回已有内容 |
| `""`+空白 | 不产生额外 token | 空白只在 `has_token \|\| !current.is_empty()` 时 emit |

> **与 shell 的差异**：不做变量展开、不做通配、不处理 `$`、反引号、`~`、`#` 注释、
> 不支持 `'{'...'}'` 之外的引号嵌套。Rust 版 **不要**用 `shlex`（`shlex` 对单引号内的转义、
> 空 token、未闭合引号的行为不同）；请按上面的状态机直译，或包一层自定义实现并用上面的表格做单测。

### 4.4 `QuoteForDisplay`（仅展示用，`CommandLineBuilder.cs:337-343`）

```
fn quote_for_display(arg: &str) -> String {
    if arg.is_empty() { return "\"\"".to_string(); }
    let needs_quote = arg.chars().any(|c| matches!(c, ' '|'\t'|'"'|'\''|';'|'&'|'|'|'>'|'<'));
    if !needs_quote { return arg.to_string(); }
    format!("\"{}\"", arg.replace('"', "\\\""))     // 只转义双引号
}
```

`RenderCommandLine(args)`：空列表 → `""`；否则 `args.map(quote_for_display).join(" ")`（单空格）。

**该函数只用于显示 / 复制粘贴**，执行用的是 `argv` 列表。参考实现的 `FFmpegService.RunAsync`
（`FFmpegService.cs:582`）把它用于**日志里的命令行**：`paths.FFmpeg + " " + RenderCommandLine(args)`；
`TemplateTestAsync` 展示的是 `"ffmpeg -hide_banner -y " + built.CommandLine`（`TemplateService.cs:561`）。

Rust 版注意：Windows 的 `cmd`、PowerShell、bash 对这段渲染文本的解析方式各不相同，
**不要**把它当作可执行命令行使用（参考实现也从不这样做）。

### 4.5 `MergeUploadFilter`（`CommandLineBuilder.cs:346-382`）

目的：VAAPI 等编码器必须把帧上传到显存（`format=nv12,hwupload`）。若模板自己写了 `{hwupload_args}`，
则完全交给模板作者，不做自动合并。

```
fn merge_upload_filter(tokens, upload_filter, template_text, warnings) -> tokens {
    if template_text.contains_ci("{hwupload_args}") { return tokens; }        // 模板自行控制

    if let Some(i) = tokens.position(|t| t == "-vf" || t == "-filter:v" || t == "-filter") {
        if let Some(chain) = tokens.get(i + 1) {
            if !chain.contains_ci("hwupload") {
                tokens[i + 1] = format!("{},{}", chain.trim_end_matches(','), upload_filter);
            }
            return tokens;
        }
        // 注：`-vf` 是最后一个 token 时直接落到下面的分支继续找插入点（源码是顺序 if，不 return）
    }

    if tokens.any(|t| t == "-filter_complex" || t == "-lavfi") {
        warnings.push(format!(
            "当前编码器需要显存上传滤镜（{upload_filter}），但模板使用了 -filter_complex/-lavfi，无法自动合并。请在模板中显式使用 {{hwupload_args}} 占位符。"));
        return tokens;
    }

    // 没有滤镜：在第一个 -c:v / -codec:v / -vcodec 之前插入 ["-vf", upload_filter]；找不到则追加到末尾
    let insert_at = tokens.position(|t| t == "-c:v" || t == "-codec:v" || t == "-vcodec")
                         .unwrap_or(tokens.len());
    tokens.splice(insert_at..insert_at, ["-vf", upload_filter]);
    tokens
}
```

细节：合并已有 `-vf` 时**会**先 `TrimEnd(',')`；若链里已含 `hwupload`（大小写不敏感）则不改动。
源码 `373-380` 行有一段先算 `outputIdx` 又紧接着被覆盖的**死代码**（`tokens.FindIndex(t => t.StartsWith("-c:") ...)`），
语义上无影响，Rust 版可忽略，只需实现「精确等于 `-c:v`/`-codec:v`/`-vcodec` 的插入点」。

### 4.6 位置规则（必须写进 Rust 版的文档与测试）

| 规则 | 触发者 | 位置 | 源码 |
|---|---|---|---|
| `-hwaccel xxx` 必须在 `-i` 之前 | `CommandContext.HardwareDecodeArgs`（用户设置「优先使用硬件解码」） | 输入路径 token 的**前一个**位置；`input` 在索引 0 时插入到 0；找不到输入路径则插到最前 | `156-167` |
| `-vaapi_device` 必须在所有输入参数之前（全局选项） | `EncoderProfile.DeviceArgs` | 第一个「以 `-` 开头且不是 `-y`/`-n`」的 token 之前；没有则插到 0 | `169-177` |
| `-vf` 显存上传滤镜必须在编码器之前 | `EncoderProfile.UploadFilter` | 第一个 `-c:v`/`-codec:v`/`-vcodec` 之前；不存在则追加末尾 | `346-382` |
| `-tag:v hvc1`（HEVC-in-MP4） | `EncoderProfile.MuxerArgs` | 输出路径 token 之前（`rposition`，即**最后一个**等于输出路径的 token）；找不到则追加末尾。已被模板显式写 `-tag*` 时跳过 | `179-187` |
| 全局附加参数 | `CommandContext.GlobalArguments` | 同上（输出路径之前） | `189-196` |
| `-ss <time>` 放在 `-i` 之前（快速定位） | 模板作者手写 | 「画面 · 按时间裁剪片段」与「导出封面」模板就是这么写的 | `SystemTemplatePresets.cs:494`, `535` |

**推论（Rust 版要保留的语义）**：
- 若模板**自己**写了 `-hwaccel`（任意位置），则不会再自动注入（判断条件是「存在以 `-hwaccel` 开头的参数」）。
- 若模板用了 `{hwaccel}` 占位符，该 token 在步骤 3 就展开成 `-hwaccel videotoolbox`（**一个 token**），
  因此步骤 4 的检测同样生效；占位符为空时 `{hwaccel}` 整个 token 消失。
- 输出路径在命令里出现多次时，容器参数与全局参数都插在**最后一次**出现之前；
  自动注入的 `-hwaccel` 用的是**第一次**出现输入路径的前一个位置。

### 4.7 未解析占位符的报表

1. 每次遇到「非参数组占位符、解析表里没有」→ 记下名字（**重复记录**），保留原文 `{name}`。
2. 最后 `Distinct(OrdinalIgnoreCase)` 去重，写入 `BuiltCommand.UnresolvedPlaceholders`（**不带花括号**）。
3. 若有未解析项，追加一条 Warning（逐字）：
   `以下占位符没有对应取值，将原样传给 FFmpeg，可能导致失败：{a}, {b}`
   （占位符用 `{`+名+`}` 包裹，`", "` 分隔）。
4. **独占 token 的参数组占位符永远不会进未解析列表**（缺失/空值都静默丢弃）。
5. 命令里含大括号的非占位符文本（如 `{1}`、`{中文}`、`{a-b}`）不会被报告。
   回归测试断言：`tools/verify/TemplateChecks.cs:74-79` 要求展开后 `Arguments` 里**不存在**
   同时含 `{` 和 `}` 的参数，且**不存在空字符串参数**（否则 `failed`）。

### 4.8 已知怪癖与边界（Rust 版逐条决策）

1. **参数组占位符混写**：`-x{quality_args}y` 形式只做文本替换 → 必须警告（见上文逐字消息）。
2. **单引号无转义**：`subtitles=filename='{subtitle}'` 要求路径中不含单引号
   （内置模板的字幕说明里明确写了「路径中不要包含单引号」）。
3. **`""` 会产生空参数**：模板里写 `-foo ""` 会真的传一个空字符串给 FFmpeg。
   `Build` 只在「占位符解析为空」时丢弃 token，不会清理字面 `""`。
   内置模板不这么写，回归测试也因此通过。
4. **`{input}` / `{output}` 保护**：名为 `input`/`output` 的参数不会覆盖内置值。
5. **`hardware_decode_args` 的 `IndexOf` 语义**：按字符串相等查找输入路径；
   若模板用 `{output_pattern}` 而不是 `{output}`，输出锚点找不到 → 容器/全局参数被追加到命令末尾
   （对 `-tag:v` 而言通常仍然有效，因为它在输出路径之后、FFmpeg 仍会解析成输出选项；
   但 Rust 版应当明确这一点，不要假设「一定在输出前」）。
6. **`-progress pipe:2` 与 `-nodin` 不属于模板**：执行层固定前缀
   `["-hide_banner", overwrite ? "-y" : "-n", "-progress", "pipe:2"]`（`FFmpegService.cs:560-566`）。
7. **不校验、不夹紧**：`Build` 对参数值不做任何合法性检查（Number 的 Min/Max 不生效）。
8. **大小写**：占位符名比较不区分大小写；但**参数名去重、命令行中的编码器名等保持原样**。

---

## 5. 编码器画像（`EncoderCatalog`，`{quality_args}` 的真正来源）

源码：`Services/EncoderCatalog.cs:9-378`。Rust 版应把这张表原样搬成一个静态数组
（`EncoderProfile` 的字段见 `9-58`：`Name`/`DisplayName`/`Type`/`Family`/`Codec`/`IsHardware`(派生)/
`QualityArgs`/`DefaultQuality`(默认 23)/`QualityHint`(默认「数值越小画质越好」)/`PresetArgs`/
`UploadFilter`/`DeviceArgs`/`MuxerArgs`/`SortPreference`/`SupportsCurrentPlatform`(默认 true)）。

### 5.1 全部画像（逐字，`QualityArgs` 中的 `{q}` 会被替换为质量数值）

| 编码器 | 族 | 编码格式 | 类型 | 质量参数（QualityArgs） | 默认质量 | 预设参数（PresetArgs） | 上传滤镜 | 设备参数 | 容器参数 | 优先级 | 平台限制 | 源码行 |
|---|---|---|---|---|---|---|---|---|---|---|---|---|
| `h264_nvenc` | nvenc | h264 | Nvenc | `-rc vbr -cq {q} -b:v 0` | 24 | `-preset p5 -tune hq` | — | — | — | 10 | Win/Linux | 66-73 |
| `hevc_nvenc` | nvenc | hevc | Nvenc | `-rc vbr -cq {q} -b:v 0` | 28 | `-preset p5 -tune hq` | — | — | `-tag:v hvc1` | 10 | Win/Linux | 74-82 |
| `av1_nvenc` | nvenc | av1 | Nvenc | `-rc vbr -cq {q} -b:v 0` | 30 | `-preset p5 -tune hq` | — | — | — | 10 | Win/Linux | 83-90 |
| `h264_qsv` | qsv | h264 | Qsv | `-global_quality {q}` | 24 | `-preset medium` | — | — | — | 20 | Win/Linux | 93-100 |
| `hevc_qsv` | qsv | hevc | Qsv | `-global_quality {q}` | 28 | `-preset medium` | — | — | `-tag:v hvc1` | 20 | Win/Linux | 101-109 |
| `av1_qsv` | qsv | av1 | Qsv | `-global_quality {q}` | 30 | `-preset medium` | — | — | — | 20 | Win/Linux | 110-117 |
| `vp9_qsv` | qsv | vp9 | Qsv | `-global_quality {q}` | 30 | `-preset medium` | — | — | — | 20 | Win/Linux | 118-125 |
| `h264_amf` | amf | h264 | Amf | `-rc cqp -qp_i {q} -qp_p {q}` | 24 | `-quality balanced` | — | — | — | 30 | Win/Linux | 128-135 |
| `hevc_amf` | amf | hevc | Amf | `-rc cqp -qp_i {q} -qp_p {q}` | 28 | `-quality balanced` | — | — | `-tag:v hvc1` | 30 | Win/Linux | 136-144 |
| `av1_amf` | amf | av1 | Amf | `-rc cqp -qp_i {q} -qp_p {q}` | 30 | `-quality balanced` | — | — | — | 30 | Win/Linux | 145-152 |
| `h264_videotoolbox` | videotoolbox | h264 | VideoToolbox | `-q:v {q}` | 60 | `null` | — | — | — | 5 | macOS | 155-162 |
| `hevc_videotoolbox` | videotoolbox | hevc | VideoToolbox | `-q:v {q}` | 60 | `null` | — | — | `-tag:v hvc1` | 5 | macOS | 163-171 |
| `prores_videotoolbox` | videotoolbox | prores | VideoToolbox | `null`（无质量参数） | 23（未设置） | `-profile:v 3` | — | — | — | 5 | macOS | 172-179 |
| `h264_vaapi` | vaapi | h264 | Vaapi | `-qp {q}` | 24 | `null` | `format=nv12,hwupload` | `-vaapi_device /dev/dri/renderD128` | — | 15 | Linux | 182-191 |
| `hevc_vaapi` | vaapi | hevc | Vaapi | `-qp {q}` | 28 | `null` | `format=nv12,hwupload` | `-vaapi_device /dev/dri/renderD128` | `-tag:v hvc1` | 15 | Linux | 192-202 |
| `av1_vaapi` | vaapi | av1 | Vaapi | `-qp {q}` | 30 | `null` | `format=nv12,hwupload` | `-vaapi_device /dev/dri/renderD128` | — | 15 | Linux | 203-212 |
| `h264_vulkan` | vulkan | h264 | Vulkan | `-qp {q}` | 24 | `null` | — | — | — | 40 | Win/Linux | 215-222 |
| `hevc_vulkan` | vulkan | hevc | Vulkan | `-qp {q}` | 28 | `null` | — | — | `-tag:v hvc1` | 40 | Win/Linux | 223-231 |
| `av1_vulkan` | vulkan | av1 | Vulkan | `-qp {q}` | 30 | `null` | — | — | — | 40 | Win/Linux | 232-239 |
| `h264_mf` | mf | h264 | MediaFoundation | `-rate_control quality -quality {q}` | 70 | `null` | — | — | — | 60 | Windows | 242-249 |
| `hevc_mf` | mf | hevc | MediaFoundation | `-rate_control quality -quality {q}` | 70 | `null` | — | — | `-tag:v hvc1` | 60 | Windows | 250-258 |
| `libx264` | software | h264 | Software | `-crf {q}` | 23 | `-preset medium` | — | — | — | 90 | 全平台 | 261-267 |
| `libx265` | software | hevc | Software | `-crf {q}` | 28 | `-preset medium` | — | — | `-tag:v hvc1` | 90 | 全平台 | 268-275 |
| `libsvtav1` | software | av1 | Software | `-crf {q} -b:v 0` | 32 | `-preset 8` | — | — | — | 90 | 全平台 | 276-282 |
| `libaom-av1` | software | av1 | Software | `-crf {q} -b:v 0` | 32 | `-cpu-used 4` | — | — | — | 95 | 全平台 | 283-289 |
| `libvpx-vp9` | software | vp9 | Software | `-crf {q} -b:v 0` | 32 | `-row-mt 1` | — | — | — | 90 | 全平台 | 290-296 |
| `libvpx` | software | vp8 | Software | `-crf {q} -b:v 1M` | 10 | `null` | — | — | — | 90 | 全平台 | 297-303 |
| `aac` | software | aac | Software | `-b:a {q}k` | 192 | `null` | — | — | — | 90 | 全平台 | 304-310 |
| `libmp3lame` | software | mp3 | Software | `-b:a {q}k` | 192 | `null` | — | — | — | 90 | 全平台 | 311-317 |
| `libopus` | software | opus | Software | `-b:a {q}k` | 128 | `null` | — | — | — | 90 | 全平台 | 318-324 |
| `flac` | software | flac | Software | `null` | 23（未设置） | `null` | — | — | — | 90 | 全平台 | 325-331 |

（共 31 条；`QualityHint` 及 `DisplayName` 的逐字文案见源码对应行，如需在界面复用以外的用途请照抄。）

**族的显示名**（`EncoderCatalog.cs:366-377`）：
`nvenc`→`NVIDIA NVENC`，`qsv`→`Intel Quick Sync`，`amf`→`AMD AMF`，`videotoolbox`→`Apple VideoToolbox`，
`vaapi`→`Linux VA-API`，`vulkan`→`Vulkan Video`，`mf`→`Windows Media Foundation`，`software`→`软件编码`，
其它 → 原样返回。

**族/画像辅助函数**：
- `Find(name)`：按名字精确（忽略大小写）查一条；找不到 `null`。
- `ByFamily(family)`：该族全部画像；`ByCodec(codec)`：该格式全部画像**按 SortPreference 升序**。
- `SoftwareFallback(codec)`：该格式的软件编码器中 `SortPreference` 最小者。
- `ExpandFamily(nameOrFamily)`：先当族名展开出全部成员名；若族不存在则返回 `[nameOrFamily]` 单元素。

### 5.2 编码器选择算法（真实转换，`HardwareDetectionService.cs:180-244`）

```
fn get_recommended_encoder(codec, required_encoder: Option<String>, strict: bool) -> Option<String> {
    let report = detect();                                   // 有缓存，见 §6.4
    let normalized_codec = normalize_codec(codec);

    // 1) 模板/调用方指定的必需编码器（族名或具体名）
    if let Some(req) = required_encoder.filter(|s| !blank(s)) {
        for member in expand_family(&req) {
            if let Some(hit) = report.available_encoders().find(|e| e.name.eq_ignore_ascii_case(member)
                && (normalized_codec.is_empty() || e.codec.eq_ignore_ascii_case(normalized_codec))) {
                return Some(hit.name);
            }
        }
        if strict { return None; }        // Required 模式：本机没有该族 → 明确失败（不偷偷换别人的硬件）
    }

    // 2) 用户在设置里指定的首选编码器
    if let Some(pref) = settings.preferred_hardware_encoder.filter(|s| !blank(s)) {
        if let Some(hit) = report.available_encoders().find(|e| e.name.eq_ignore_ascii_case(pref)
            && (normalized_codec.is_empty() || e.codec.eq_ignore_ascii_case(normalized_codec))) {
            return Some(hit.name);
        }
    }

    // 3) 自动：先硬件（按 SortPreference 顺序），再软件
    let candidates = profiles().filter(|p| p.supports_current_platform && p.codec == normalized_codec)
                              .sorted_by_key(|p| p.sort_preference);
    if settings.prefer_hardware_acceleration {
        for c in candidates.filter(|c| c.is_hardware) {
            if report.available_contains(c.name) { return Some(c.name); }
        }
    }
    for c in candidates.filter(|c| !c.is_hardware) {
        if report.available_contains(c.name) { return Some(c.name); }
    }

    // 4) 兜底
    match normalized_codec { "hevc" => if available_contains("libx265") { "libx265" } else { "libx264" },
                             _      => "libx264" }
}
```

`ConversionQueue.ResolveEncoderAsync`（`528-567`）决定「要不要挑编码器」与「挑失败怎么办」：

```
fn resolve_encoder(template, settings) -> Option<EncoderProfile> {
    let uses_placeholder = template.command_args.contains_ci("{encoder}");
    let needs = uses_placeholder || template.hardware_mode != None;
    if !needs { return None; }                       // 模板硬编码了软件编码器 → Encoder = None

    let codec   = determine_target_codec(template);
    let strict  = template.hardware_mode == Required;
    let Some(name) = get_recommended_encoder(codec, template.required_encoder, strict) else {
        // Required 模式下没有可用编码器 → 抛出（逐字消息）
        let family = if blank(required) { "指定的硬件编码器".into() }
                     else { family_display_name(find(required).map(|p| p.family).unwrap_or(required)) };
        return Err(format!(
            "该模板要求使用 {family} 硬件编码，但本机不可用。请改用「推荐」分类下会自动挑选编码器的模板，或在「设置 → 硬件加速能力」中确认本机支持的编码器。"));
    };
    let mut profile = find(name) ?? software_fallback(codec) ?? find("libx264");
    // 探测显示该编码器只能用最简参数 → 退化：去掉质量与预设参数
    if let Some(d) = last_report.encoders.find(|e| e.name == profile.name) {
        if !d.supports_quality_options { profile.quality_args = None; profile.preset_args = None; }
    }
    Some(profile)
}
```

`DetermineTargetCodec`（`ConversionQueue.cs:569-606`，逐序）：

```
1. template.target_codec 非空 → NormalizeCodec(target_codec)
2. template.required_encoder 非空 → find(required).codec，否则 ByFamily(required).first().codec
3. OutputExtension 归一（NormalizeCodec）∈ {h264,hevc,av1,vp9} → 用它
4. 命令中出现 "libx265","libx264","libsvtav1","libvpx-vp9","libvpx"（按此顺序）→ 对应画像的 codec
5. 扩展名兜底：webm→vp9，mkv→h264，mov→h264，其它→h264
```

`NormalizeCodec`（`HardwareDetectionService.cs:247-264`）逐字映射：

```
avc | h264 | x264 | avc1                 -> h264
h265 | hevc | x265 | hvc1 | hev1        -> hevc
av1 | av01                              -> av1
vp9 | vp09                              -> vp9
vp8                                     -> vp8
prores                                  -> prores
aac -> aac ; mp3 -> mp3 ; opus -> opus ; flac -> flac
其它                                    -> 原样（小写、Trim 后）
```

### 5.3 `{encoder}` 与 `{quality_args}` 的配合（移植要点）

- 模板只写 `-c:v {encoder} {quality_args} {preset_args} {hwupload_args}`，**不写具体编码器名**，
  由运行期选出的画像展开 → 同一模板在 NVENC / QSV / AMF / VideoToolbox / VAAPI / x264 上都能得到
  **该编码器正确的质量参数**（这是参考实现最关键的设计点）。
- `required_encoder` 只用于「挑选」与「严格性」，**不参与命令构建**。
- `hardware_mode == Required` 且本机没有该族 → 拒绝执行（抛错 / 试运行失败），**不会**静默替换成别的厂商。
- `hardware_mode == Auto` 且本机没有硬件 → 自动回退到软件编码器（同一个模板，参数由软件画像展开）。
- `hardware_mode == None` 但模板里含 `{encoder}` → 仍会挑选（`needs_encoder` 为真），
  例如 `AddScaleTemplate` 生成的缩放模板 `target_codec = h264`、`hardware_mode = None`，
  命令里**没有** `{encoder}`，因此 `Encoder = None`，`{quality_args}` 之类也不会出现。

---

## 6. 模板服务契约与生命周期

### 6.1 CRUD 方法与副作用（`Services/TemplateService.cs:102-204`）

| 方法 | 允许对象 | 关键副作用 |
|---|---|---|
| `CreateTemplateAsync` | 任意 → **强制 User** | 生成新 `Id`；`Type = User`；`CreatedAt/UpdatedAt = UtcNow`；`EnsureUniqueName`；`NormalizeTemplate`；`Upsert`；返回对象 |
| `UpdateTemplateAsync` | **仅 User**（`existing.Type == System` → 返回 `false`，数据库不变） | `Type = User`；保留原 `CreatedAt`；刷新 `UpdatedAt`；改名去重；规范化；`Update` |
| `DeleteTemplateAsync` | **仅 User**（System → `false`） | 硬删除 |
| `CopyTemplateAsync` | 任意源 → 生成 User 副本 | 新 `Id`；名字 = `newName` 或 `"{源名} (副本)"`；`IsRecommended = false`；`PresetVersion = 0`；`SortOrder = 源.SortOrder + 1`；**深拷贝 `Parameters`（含 `Options`）**；`Tags`/`SupportedInputFormats` 浅拷贝列表。**注意：`RequiredFilters` 不会被复制**（源码 `144-165` 未赋值） |
| `CreateSystemTemplateAsync` | 管理端 | `Type = System`；`PresetVersion = PresetVersion`（当前 = 4） |
| `UpdateSystemTemplateAsync` | 管理端 | `Type` 沿用库中原值；若为 System 则把 `PresetVersion` 刷成当前版本（**这正是「手改的系统模板在下次启动同步中不会被覆盖」的原因**，见 §10.4） |
| `DeleteSystemTemplateAsync` | 管理端 | 无条件删除（不检查 Type） |

`CopyTemplateAsync` 的源不存在时抛 `ArgumentException("源模板不存在", nameof(sourceId))`（`141-142`）。

### 6.2 系统模板与用户模板的可见性差异

- `GetAllTemplatesAsync(includeSystem: true)` 同时返回两类，界面上用 `Type` 区分（系统模板通常显示「系统」徽标、
  禁用编辑/删除）。
- 「模板列表」页在渲染前会再调一次 `InitializeSystemTemplatesAsync()`（`Components/Pages/Templates.razor:452`），
  `TemplateSelector` 组件也会（`Components/Shared/TemplateSelector.razor:290`），
  应用启动时也会（`Program.cs:154-155`，并把 `sync.Summary` 写进日志）。
- 「渲染时同步」因此必须**幂等且廉价**（两次连续调用必须产生 `Added = 0 && Updated = 0`，
  这是 `tools/verify/PipelineChecks.cs:162-163` 的断言）。

### 6.3 `TemplateIssue` 与 `TemplateTestResult` 数据结构

```rust
struct TemplateIssue { severity: TemplateIssueSeverity, message: String, field: Option<String> }   // Models/DTOs.cs:119-122

struct TemplateTestResult { success: bool, message: String, command: String, output: String, duration_secs: f64 }
// Models/DTOs.cs:132-137；Command 是展示用的命令行文本，"Output" 是 FFmpeg 诊断输出（stderr 尾部）
```

`field` 取值就是 C# 属性名（`"Name"`/`"CommandArgs"`/`"OutputExtension"`/`"Parameters"`/`"RequiredEncoder"`/
`"RequiredFilters"`），用于界面把问题定位到具体编辑器控件；有些问题不带 `field`（`null`）。

### 6.4 硬件检测缓存（试运行与校验的隐式依赖）

`HardwareDetectionService.DetectAsync(forceRefresh = false)`（`35-171`）的缓存层次：

1. **内存报告缓存**：`_lastReport` 且「距 `DetectedAt` 不超过 30 分钟」→ 直接返回（`38-41`、`46-49` 双重检查 + `SemaphoreSlim` 串行化）。
2. **数据库缓存**：集合 `encoders`；仅当 **FFmpeg 版本串和可执行文件路径都没变** 时复用（`71-79`）。
3. **单编码器试跑缓存**：`ConcurrentDictionary<string,bool> _probeCache`（`24`，进程内、不落库）。
   探测分两档：先「带质量参数 + 预设参数」跑一次，失败再「只指定编码器」跑一次；
   第二档通过 → 该编码器 `SupportsQualityOptions = false`（`281-299`），
   调用方据此把 `QualityArgs`/`PresetArgs` 置空（`ConversionQueue.cs:557-564`）。
4. 探测候选的筛选：只探测 `SupportsCurrentPlatform` 的画像；硬件编码器还要出现在
   `ffmpeg -encoders` 列表里才试跑；**软件编码器只检查是否被编译进来，不试跑**（`96-114`）。
5. 报告里带 `SupportedFilters`（来自 `ffmpeg` 版本输出解析），供 `GetMissingFilters` 使用。

`HardwareDetectionReport.HasFilter`（`Models/HardwareEncoder.cs:75-76`）：
`SupportedFilters` 为空 → **一律返回 true**（宁可漏报也不误报）；否则大小写不敏感包含判断。

Rust 版可以自由选择探测实现，但必须保留两个可观测语义：
(a) 「报告里没有滤镜清单 ⇒ 不判定缺失」；
(b) 「编码器只能用最简参数 ⇒ 自动去掉质量/预设参数，而不是让转换失败」。

---

## 7. 模板校验（`Validate`）完整清单

源码：`Services/TemplateService.cs:287-443`。**顺序即下表顺序**；返回 `IReadOnlyList<TemplateIssue>`，
不抛异常。Rust 版必须产生**逐字相同的中文消息**（界面直接展示它们），并保持相同顺序
（`TestTemplateAsync` 会把所有 Error 用 `；` 连接展示，顺序会影响文案）。

| # | 条件 | 严重级 | 消息（逐字） | `field` | 源码行 |
|---|---|---|---|---|---|
| 1 | `Name` 为空白 | Error | `模板名称不能为空` | `Name` | 291-294 |
| 2 | `CommandArgs` 为空白 | Error | `命令参数不能为空` | `CommandArgs` | 295-299 |
| — | 条件 2 成立时**立即返回**（后续检查全部跳过） | — | — | — | 298 |
| 3 | `OutputExtension` 为空白 | Error | `必须指定输出扩展名，否则无法生成输出文件名` | `OutputExtension` | 300-303 |
| 4 | 占位符中不含 `input`（大小写不敏感） | Error | `命令中缺少 {input} 占位符，FFmpeg 将无法读取输入文件` | `CommandArgs` | 311-315 |
| 5 | 占位符中既不含 `output` 也不含 `output_pattern` | Error | `命令中缺少 {output}（或用于图片序列的 {output_pattern}）占位符，转换结果没有输出目标` | `CommandArgs` | 316-323 |
| 6 | 存在占位符，既不是内置占位符、也没有被任何参数声明（对每个占位符各报一条，**不排除重复名**） | Error | `占位符 {<名>} 没有对应取值：请在上方『参数』中声明它，或改用内置占位符` | `Parameters` | 325-332 |
| 7 | `CommandArgs` 中双引号 `"` 的数量为奇数 | Error | `双引号数量为奇数，命令可能被错误拆分。含空格的路径请写成 "{input}"` | `CommandArgs` | 335-340 |
| 8 | 「硬件模板」（`HardwareMode == Required` 或 `RequiredEncoder` 非空且不是 software 族成员）**且** `RequiredEncoder` 非空**且** `ExpandFamily(RequiredEncoder)` 中存在 `IsHardware` 画像**且** `CommandArgs` 含 `-crf` | Warning | `硬件编码器不支持 -crf，该选项会被 FFmpeg 忽略。建议改用 {quality_args} 占位符，它会自动展开成当前编码器正确的质量参数。` | `CommandArgs` | 342-363 |
| 9 | 同上分支但 `EncoderCatalog.Find(RequiredEncoder) == null`（族和具体名都认不出） | Warning | `无法识别的编码器「{required}」。可用族名：nvenc / qsv / amf / videotoolbox / vaapi / vulkan，或具体编码器名如 h264_nvenc。` | `RequiredEncoder` | 364-369 |
| 10 | `HardwareMode == Required` 且 `RequiredEncoder` 为空白 | Error | `模板要求必须使用硬件加速，但没有指定编码器族。这样在任何机器上都会失败。` | `RequiredEncoder` | 372-377 |
| 11 | `HardwareMode != None` 且 `CommandArgs` 不含 `{encoder}`（大小写不敏感） | Warning | `硬件加速模板建议使用 {encoder} 占位符，这样在本机没有该厂商硬件时会自动回退到可用的编码器。` | `CommandArgs` | 380-386 |
| 12 | `CommandArgs` 含 `-c copy` / `-c:v copy` / `-codec copy`（大小写不敏感） | Info | `「复制流」模式下画面不会被重新编码。如果源文件的编码格式与目标容器不兼容，转换会失败——这正是「快速换容器」模板的已知限制。` | **无（null）** | 388-394 |
| 13 | 上一条成立**且** `CommandArgs` 含 `-vf` 或 `-filter_complex` | Error | `「-c copy」不能与滤镜同时使用：不重新编码就无法应用滤镜。` | `CommandArgs` | 395-400 |
| 14 | 参数 `Name` 为空白（每个各一条，随后 `continue` 跳过该参数的后续检查） | Warning | `存在一个没有名字的参数，它不会被使用。` | `Parameters` | 405-409 |
| 15 | 参数名是内置占位符**且**不是 `quality`/`q`/`crf` | Warning | `参数名「{name}」与内置占位符同名，内置值可能被覆盖。建议改名。` | `Parameters` | 411-417 |
| 16 | `Kind == Select` 且 `Options.Count == 0` | Warning | `参数「{label}」是下拉选项但没有配置任何选项。`（`label` 为空时取 `name`） | `Parameters` | 418-423 |
| 17 | 占位符中**没有** `encoder` 但 `CommandArgs` 含 `{quality_args}` | Warning | `使用了 {quality_args} 但没有 {encoder}：质量参数将按默认编码器展开。` | `CommandArgs` | 426-432 |
| 18 | `RequiredFilters.Count > 0`（始终触发一次，把滤镜列表列出来） | Info | `该模板依赖以下 FFmpeg 滤镜：{滤镜1、滤镜2}。如果当前 FFmpeg 构建没有编译它们，模板会显示为不可用。`（分隔符是**中文顿号 `、`**） | `RequiredFilters` | 434-440 |

逐字消息模板（把上表还原成可直译的格式串，`{}` 是 C# 内插占位）：

```
模板名称不能为空
命令参数不能为空
必须指定输出扩展名，否则无法生成输出文件名
命令中缺少 {input} 占位符，FFmpeg 将无法读取输入文件
命令中缺少 {output}（或用于图片序列的 {output_pattern}）占位符，转换结果没有输出目标
占位符 {{{placeholder}}} 没有对应取值：请在上方『参数』中声明它，或改用内置占位符
双引号数量为奇数，命令可能被错误拆分。含空格的路径请写成 "{input}"
硬件编码器不支持 -crf，该选项会被 FFmpeg 忽略。建议改用 {quality_args} 占位符，它会自动展开成当前编码器正确的质量参数。
无法识别的编码器「{required}」。可用族名：nvenc / qsv / amf / videotoolbox / vaapi / vulkan，或具体编码器名如 h264_nvenc。
模板要求必须使用硬件加速，但没有指定编码器族。这样在任何机器上都会失败。
硬件加速模板建议使用 {encoder} 占位符，这样在本机没有该厂商硬件时会自动回退到可用的编码器。
「复制流」模式下画面不会被重新编码。如果源文件的编码格式与目标容器不兼容，转换会失败——这正是「快速换容器」模板的已知限制。
「-c copy」不能与滤镜同时使用：不重新编码就无法应用滤镜。
存在一个没有名字的参数，它不会被使用。
参数名「{name}」与内置占位符同名，内置值可能被覆盖。建议改名。
参数「{label}」是下拉选项但没有配置任何选项。
使用了 {quality_args} 但没有 {encoder}：质量参数将按默认编码器展开。
该模板依赖以下 FFmpeg 滤镜：{filters_joined_by_、}。如果当前 FFmpeg 构建没有编译它们，模板会显示为不可用。
```

**校验「不做」的事（Rust 版不要擅自加）**：不检查编码器是否存在于本机、不检查滤镜是否真的可用
（那是 `GetMissingFilters`）、不检查参数类型/取值范围、不检查 `Options` 里的值是否合法、
不检查 `RequiredEncoder` 与 `HardwareMode` 的业务组合（除 #10）、不检查 `SortOrder`/`Tags`。

**额外的已知不一致**：`-c copy` 检测是**朴素子串匹配**（`Contains`），因此 `-c:v copy`/`-codec copy`
也能被 `-c copy` 之外的两条覆盖；而 `-crf` 检测同样朴素，会命中 `-crf` 出现在滤镜/元数据字符串里的情况。

### 7.1 `GetMissingFilters`（`Services/TemplateService.cs:445-454`）

```
fn get_missing_filters(template, report: Option<&HardwareDetectionReport>) -> Vec<String> {
    if template.required_filters.is_empty() { return vec![]; }
    let Some(r) = report else { return vec![] };                  // 报告缺失 → 不判断
    if r.supported_filters.is_empty() { return vec![]; }          // 无滤镜清单 → 不判断（避免误报）
    template.required_filters.iter().filter(|f| !r.has_filter(f)).cloned().collect()
}
```

界面语义：非空 → 该模板显示为「不可用 / 缺少滤镜」，试运行直接拒绝（§8.2 第 3 步）。
当前内置模板中只有「画面 · 烧录字幕（硬字幕）」声明了 `RequiredFilters = ["subtitles"]`。

---

## 8. 试运行（`TestTemplateAsync`）

源码：`Services/TemplateService.cs:460-582`（辅助：`GenerateSampleAsync` 584-601、
`SubstituteRequiredPaths` 604-623、`ResolveEncoderForTestAsync` 625-648）。

### 8.1 目的

用一段**自动生成的测试素材**把模板真正跑一遍，确认「在这台机器、这个 FFmpeg 构建上真的能出结果」，
从而在用户批量转换之前暴露：缺滤镜、编码器不可用、参数组展开错误、路径引号问题等。

### 8.2 完整算法（含全部用户可见消息）

```
async fn test_template(template, parameter_values: Option<Map<String,String>>) -> TemplateTestResult {
    // 1) 校验；有 Error 直接失败（不生成素材、不跑 FFmpeg）
    let issues = validate(template);
    let errors = issues.iter().filter(|i| i.severity == Error).collect_vec();

    // 2) 滤镜缺失检查
    let filter_report = hardware_detection.detect().await;                 // 走 §6.4 的缓存
    let missing = get_missing_filters(template, Some(&filter_report));
    if !missing.is_empty() {
        return fail(format!("当前 FFmpeg 构建缺少该模板需要的滤镜：{missing_join_、}。\
             请换用其它模板，或改用带完整滤镜支持的 FFmpeg 构建（如 gyan.dev / Homebrew 的完整版）。"),
            command = template.command_args, output = "", dur = 0);
    }

    if !errors.is_empty() {
        return fail(format!("模板校验未通过：{}", errors.map(|e| e.message).join("；")),
            command = template.command_args, output = "", dur = 0);
    }

    // 3) FFmpeg 可用性
    let settings = settings_service.get_settings().await.normalize();
    let info = ffmpeg_service.get_ffmpeg_info().await;
    if info.is_none() || !info.unwrap().is_available {
        return fail("FFmpeg 不可用，无法试运行。请先在设置中指定 FFmpeg 路径。", template.command_args, "", 0);
    }

    // 4) 建临时目录：系统临时目录 / ffmpegwebui-test-{guid:N}
    let work_dir = temp_dir().join(format!("ffmpegwebui-test-{}", uuid_simple()));
    create_dir_all(&work_dir)?;
    let extension = if blank(template.output_extension) { "mp4" } else { template.output_extension.trim_start_matches('.') };
    let input_path  = work_dir.join("sample.mp4");
    let output_path = work_dir.join(format!("result.{extension}"));

    // 5) 生成 3 秒带音轨测试素材（见 8.3）
    if !generate_sample(&input_path).await {
        return fail("无法生成测试素材，请检查 FFmpeg 是否完整。", template.command_args, "", 0);
    }

    // 6) 参数：默认值 → 覆盖入参 → 路径类参数替换为测试素材
    let mut parameters = create_default_parameter_values(template);
    if let Some(overrides) = parameter_values {
        for (k, v) in overrides { parameters.insert(k, v.unwrap_or_default()); }
    }
    substitute_required_paths(template, &mut parameters, &input_path, &work_dir);

    // 7) 媒体信息 + 编码器解析
    let media   = ffmpeg_service.get_media_info(&input_path).await;
    let encoder = resolve_encoder_for_test(template).await;

    // 7b) 无可用编码器 → 直接给出可读结论（不假装能跑）
    if encoder.is_none() {
        let family = family_display_name(find(&template.required_encoder.unwrap_or_default())
                        .map(|p| p.family).unwrap_or(template.required_encoder.clone().unwrap_or_default()));
        return fail(format!("❌ 该模板要求使用 {family} 硬件编码，但本机没有可用的对应编码器。\
             请改用「推荐」分类下会自动挑选编码器的模板。"), template.command_args, "", 0);
    }

    // 8) 构建命令（注意：不需要 Threads / HardwareDecodeArgs / GlobalArguments，全部留默认）
    let context = CommandContext {
        input_path, output_path, parameters, encoder, media,
        audio_encoder: "aac", global_arguments: String::new(), ..Default::default()
    };
    let built = ffmpeg_service.build_command(template, &context);

    // 9) 执行：超时 3 分钟，覆盖输出，失败时清理半成品
    let started = Instant::now();
    let run = ffmpeg_service.run(FFmpegRunRequest {
        task_id: new_object_id(),
        arguments: built.arguments.clone(),
        total_duration: media.as_ref().map(|m| m.duration).unwrap_or(0.0),
        output_path: output_path.clone(),
        overwrite: true,
        timeout: Duration::minutes(3),
        cleanup_partial_output: true,
    }).await;
    let elapsed = started.elapsed();

    let output_size = file_service.get_file_size(&output_path);
    let command_text = format!("ffmpeg -hide_banner -y {}", built.command_line);

    // 10) 判定：进程成功 **且** 输出文件大小 > 0
    if run.success && output_size > 0 {
        let suffix = match encoder {
            Some(e) if e.is_hardware => format!("（使用硬件编码器 {}）", e.name),
            Some(e)                  => format!("（使用编码器 {}）", e.name),
            None                     => String::new(),
        };
        return ok(format!("✅ 模板试运行成功{suffix}，输出 {}.", file_service.format_file_size(output_size)),
                  command_text, run.diagnostic, elapsed);
    }

    let reason = if run.cancelled { "试运行被取消" }
                 else if run.timed_out { "试运行超时（超过 3 分钟）" }
                 else { format!("FFmpeg 退出码 {}", run.exit_code) };
    return fail(format!("❌ 模板试运行失败：{reason}"), command_text, run.diagnostic, elapsed);

    // finally: 递归删除临时目录（吞掉异常）
}
```

所有失败结果的 `Command`/`Output`/`Duration` 取值：

| 失败点 | `success` | `message` | `command` | `output` | `duration` |
|---|---|---|---|---|---|
| 缺滤镜 | false | 见上 | `template.CommandArgs`（**原始模板串**，不是展开后的） | `""` | 0 |
| 校验 Error | false | `模板校验未通过：…`（多个 Error 用 `；` 连接） | 同上 | `""` | 0 |
| FFmpeg 不可用 | false | 见上 | 同上 | `""` | 0 |
| 素材生成失败 | false | 见上 | 同上 | `""` | 0 |
| 编码器不可用 | false | 见上 | 同上 | `""` | 0 |
| 执行失败 | false | `❌ 模板试运行失败：{reason}` | `ffmpeg -hide_banner -y ` + 展开后命令行 | FFmpeg 诊断输出 | 实测耗时 |
| 成功 | true | `✅ 模板试运行成功（使用…编码器 X），输出 1.2 MB。` | 同上 | 诊断输出 | 实测耗时 |

### 8.3 测试素材生成（逐字参数，`TemplateService.cs:584-601`）

```
ffmpeg
  -f lavfi -i "testsrc2=duration=3:size=640x360:rate=25"
  -f lavfi -i "sine=frequency=440:duration=3"
  -c:v libx264 -preset ultrafast -pix_fmt yuv420p
  -c:a aac -shortest
  <work_dir>/sample.mp4
```

执行参数：`TotalDuration = 3`，`Overwrite = true`，`Timeout = 60s`；成功判定 = 进程退出码 0 **且文件存在**。
（执行层仍会加 `-hide_banner -y -progress pipe:2` 前缀。）

### 8.4 试运行时如何挑编码器（`ResolveEncoderForTestAsync`）

```
fn resolve_encoder_for_test(template) -> Option<EncoderProfile> {
    if template.hardware_mode == None && !template.command_args.contains_ci("{encoder}") {
        return None;                                   // ← 注意：下面会把 None 当作「硬件不可用」失败
    }
    let codec = normalize_codec(if blank(template.target_codec) { &template.output_extension }
                                else { &template.target_codec });
    let codec = if codec in ["h264","hevc","av1","vp9","vp8"] { codec } else { "h264".into() };
    let strict = template.hardware_mode == Required;
    let name = hardware_detection.get_recommended_encoder(codec, template.required_encoder, strict).await;
    name.map(|n| find(n).unwrap_or_else(|| software_fallback(codec)))
}
```

注意与真实转换的差异（Rust 版可统一，但要知道差异）：

| 维度 | 试运行 | 真实转换 |
|---|---|---|
| 目标编码格式 | `TargetCodec` → `NormalizeCodec(OutputExtension)` → 兜底 `h264`（**只有两步**） | 五步推断（§5.2 `DetermineTargetCodec`） |
| 兜底链 | `Find(name) ?? SoftwareFallback(codec)` | `Find(name) ?? SoftwareFallback(codec) ?? Find("libx264")` |
| 质量参数退化 | **不考虑** `SupportsQualityOptions` | 会置空 `QualityArgs`/`PresetArgs` |
| 上下文 | `AudioEncoder = "aac"`，无 `Threads`/`HardwareDecodeArgs`/`GlobalArguments` | 全部按设置装配（§3.7） |

### 8.5 试运行会「缓存」什么

- 试运行本身**不缓存任何模板级结果**（每次都是「生成素材 → 跑一遍 → 删临时目录」；
  临时目录名带随机 GUID，`finally` 里递归删除）。
- 它会**间接**产生两类缓存：`HardwareDetectionService` 的 30 分钟内存报告
  + `encoders` 集合里的落库报告 + 进程内 `_probeCache`（§6.4）。
- 界面层把结果存在组件字段里（`TemplateEditor.razor:848 _testResult`、`Convert.razor:574 _testResult`），
  仅当前页面会话有效，切换页面即失效。

### 8.6 参考实现的已知缺陷（Rust 版必须修正）

**缺陷 A（严重）**：`ResolveEncoderForTestAsync` 在「`hardware_mode == None` 且命令里没有 `{encoder}`」时
返回 `None`，而 `TestTemplateAsync` 把 `None` 一律解释为「本机缺少该模板要求的硬件编码器」，
于是**所有硬编码软件编码器的模板（如「压缩 · 均衡」「快速换容器」「音频 · 提取为 MP3」）试运行必然报错**，
且消息里编码器族名为空：

```
❌ 该模板要求使用  硬件编码，但本机没有可用的对应编码器。请改用「推荐」分类下会自动挑选编码器的模板。
```

（族名来自 `FamilyDisplayName(Find("")?.Family ?? "")` → 默认分支返回空串。）
回归测试没有覆盖到它，因为 `PipelineChecks.cs:165` 只挑了「Auto + h264」（含 `{encoder}`）的模板做试运行。
Rust 版应把「返回 None」与「没有可用编码器」区分开：前者表示「模板自带编码器，无需画像」，
应当继续构建并执行命令（`Encoder = None` 时 `{encoder}` 默认展开为 `libx264`，其余参数组为空）。

**缺陷 B（轻微）**：`TestTemplateAsync` 里 `settings` 被取出并 `Normalize()` 后**未被使用**
（`483`），试运行不遵循「优先硬件解码」「全局附加参数」等设置。

**缺陷 C（轻微）**：试运行把 `-y` 写进了展示用命令行，但真实执行是 `-y`/`-n` + `-progress pipe:2`，
因此 `TemplateTestResult.Command` 与真实执行的 argv 不完全一致（仅展示，不影响正确性）。

---

## 9. 导入 / 导出 JSON

源码：`Services/TemplateService.cs:654-766`。序列化器：`System.Text.Json`，
选项为 `WriteIndented = true`、`DefaultIgnoreCondition = WhenWritingNull`、
`Encoder = JavaScriptEncoder.UnsafeRelaxedJsonEscaping`（**中文不转义**）。

### 9.1 顶层结构（`TemplateExportFile`，源码 742-750）

| JSON 键 | 类型 | 默认值 | 说明 |
|---|---|---|---|
| `Format` | string | `"FFmpegWebUI.Templates"` | 固定标识（**导入时不校验**） |
| `SchemaVersion` | int | `1` | 固定（**导入时不校验**） |
| `ExportedAt` | string(ISO-8601 UTC) | — | `DateTime.UtcNow` |
| `AppVersion` | string | — | 参考实现为 `"2.0.0"`（`Services/AppInfo.cs`） |
| `PresetVersion` | int | — | 导出时 `SystemTemplatePresets.Version`（当前 4） |
| `Templates` | array | `[]` | 模板数组 |

### 9.2 单项结构（`TemplateExportItem`，源码 752-766）

| JSON 键 | 类型 | 可空 | 导出时是否可能出现 `null`（被省略） | 说明 |
|---|---|---|---|---|
| `Name` | string | 否 | 永不 | 空名/空 `CommandArgs` 的项在**导入时被跳过** |
| `Description` | string | 是 | 是（原值为 null 时省略） | — |
| `CommandArgs` | string | 否 | 永不 | 模板命令串 |
| `Category` | string | 是 | 是 | 导入时空白 → `"导入"` |
| `SupportedInputFormats` | string[] | 是 | 是 | 导入时 `null` → `[]` |
| `OutputExtension` | string | 是 | 是 | 导入时 `null` → `""` |
| `TargetCodec` | string | 是 | 是 | 导入时 `null` → `""` |
| `HardwareMode` | **number** | 否 | 永不 | `0/1/2`（None/Auto/Required） |
| `RequiredEncoder` | string | 是 | 是 | 导入时保留 |
| `Parameters` | object[] | 是 | 是 | 元素为 `TemplateParameter` 原样序列化（§9.3） |
| `Tags` | string[] | 是 | 是 | 导入时 `null` → `[]` |
| `SortOrder` | number | 否 | 永不 | 导入时原样保留 |

### 9.3 `Parameters` 元素结构（即 `TemplateParameter` 的原样序列化）

| JSON 键 | 类型 | 说明 |
|---|---|---|
| `Name` | string | 占位符名（无花括号） |
| `Label` | string | 显示名 |
| `Description` | string | 说明 |
| `Kind` | number | `0=Select,1=Number,2=Text,3=Boolean` |
| `DefaultValue` | string | 默认值（**始终输出**，即使为 `""`——因为它是非空 string 属性） |
| `Options` | object[] | 元素为 `{ "Value": string, "Label": string, "Description": string }`（三个键始终输出） |
| `Min` | number? | 为 null 时**省略该键** |
| `Max` | number? | 同上 |
| `TrueValue` | string | 始终输出（默认 `"1"`） |
| `FalseValue` | string | 始终输出（默认 `""`） |

### 9.4 逐字示例（导出单个推荐模板的真实形状）

```json
{
  "Format": "FFmpegWebUI.Templates",
  "SchemaVersion": 1,
  "ExportedAt": "2026-10-16T06:53:59.1234567Z",
  "AppVersion": "2.0.0",
  "PresetVersion": 4,
  "Templates": [
    {
      "Name": "MP4 · H.264（自动选择最佳编码器）",
      "Description": "输出兼容性最好的 H.264 MP4。会自动使用本机可用的硬件编码器（NVENC / Quick Sync / AMF / VideoToolbox / VA-API），没有硬件时回退到 x264。",
      "CommandArgs": "-i \"{input}\" -c:v {encoder} {quality_args} {preset_args} {hwupload_args} -map 0:v:0 -map 0:a? -c:a {audio_encoder} -b:a {audio_bitrate} -movflags +faststart -pix_fmt yuv420p \"{output}\"",
      "Category": "推荐",
      "SupportedInputFormats": [],
      "OutputExtension": "mp4",
      "TargetCodec": "h264",
      "HardwareMode": 1,
      "Parameters": [
        {
          "Name": "quality",
          "Label": "质量",
          "Description": "数值越小画质越好、文件越大。留空则由当前编码器的最佳默认值决定。",
          "Kind": 0,
          "DefaultValue": "",
          "Options": [
            { "Value": "", "Label": "自动（推荐）", "Description": "" },
            { "Value": "18", "Label": "高画质", "Description": "" },
            { "Value": "23", "Label": "均衡", "Description": "" },
            { "Value": "28", "Label": "小体积", "Description": "" },
            { "Value": "32", "Label": "极小体积", "Description": "" }
          ],
          "TrueValue": "1",
          "FalseValue": ""
        }
      ],
      "Tags": ["mp4", "h264", "通用", "硬件加速"],
      "SortOrder": 0
    }
  ]
}
```

（`RequiredEncoder` 为 `null` 时**整个键消失**；`Min`/`Max` 同理。）

### 9.5 导出是**有损**的（Rust 版必须知道）

**导出不包含**：`Id`、`Type`、`RequiresHardwareAcceleration`、`RequiredFilters`、`IsRecommended`、
`PresetVersion`（每条的）、`CreatedAt`、`UpdatedAt`。
因此「导出 → 导入」**不是无损往返**：导入得到的模板 `Type = User`、`IsRecommended = false`、
`PresetVersion = 0`（除非 `asSystemTemplate = true` → `System` + 当前版本）、`RequiredFilters = []`、
`CreatedAt/UpdatedAt = 现在`、`RequiresHardwareAcceleration` 由 `HardwareMode == Required` 推导。

> 需求里说的「round-trip 兼容」在参考实现里的实际含义是：**导出的 JSON 必须能被自己原样导入不报错**
> （`PipelineChecks.cs:354-357` 断言「导入 3 个」「重复导入仍是 3 个且自动改名」）。
> Rust 版若想做到无损往返，需要扩展 schema（新增键）并**保留对旧格式的兼容读取**。

### 9.6 导入规则（`ImportFromJsonAsync`，源码 691-740）

```
fn import_from_json(json: &str, as_system_template: bool) -> Result<usize> {
    if blank(json) { return Ok(0); }
    let payload: TemplateExportFile = serde_json::from_str(json)      // C#: System.Text.Json，默认大小写敏感
        .map_err(|e| InvalidOperationException(format!("模板文件不是有效的 JSON：{e}")))?;
    let Some(items) = payload.templates.filter(|v| !v.is_empty()) else { return Ok(0) };

    let mut imported = 0;
    for item in items {
        if blank(item.name) || blank(item.command_args) { continue; }      // 静默跳过，不计数、不报错
        let mut t = CommandTemplate {
            name: item.name,
            description: item.description.unwrap_or_default(),
            command_args: item.command_args,
            category: if blank(item.category) { "导入" } else { item.category },
            supported_input_formats: item.supported_input_formats.unwrap_or_default(),
            output_extension: item.output_extension.unwrap_or_default(),
            target_codec: item.target_codec.unwrap_or_default(),
            hardware_mode: item.hardware_mode,
            requires_hardware_acceleration: item.hardware_mode == Required,   // 派生
            required_encoder: item.required_encoder,
            parameters: deep_clone(item.parameters.unwrap_or_default()),
            tags: item.tags.unwrap_or_default(),
            sort_order: item.sort_order,
            r#type: if as_system_template { System } else { User },
            preset_version: if as_system_template { CURRENT_PRESET_VERSION } else { 0 },
            created_at: now_utc(), updated_at: now_utc(),
        };
        normalize_template(&mut t);
        t.name = ensure_unique_name(Some(&t.name), None);    // 与库中所有模板（含系统）比对
        db.templates.upsert(t);
        imported += 1;
    }
    Ok(imported)
}
```

要点与坑：

1. **`JsonException` → `InvalidOperationException("模板文件不是有效的 JSON：" + 原始消息)`**（`702`）。
   其它异常（如类型不匹配）也走同一路径（都派生自 `JsonException`）。
2. **键名大小写敏感**：`System.Text.Json` 默认 `PropertyNameCaseInsensitive = false`，
   所以 `{"templates": [...]}`（小写）不会绑定到 `Templates`，结果是「导入 0 个」而**不报错**。
   Rust 版若用 serde 默认行为（字段名精确匹配）即可复现同样语义。
3. 逐项跳过规则只检查 `Name`/`CommandArgs`，其余字段全部容错。
4. 名字冲突 → 自动加后缀 `(2)`、`(3)`…（§1.7），**重复导入同一文件不会报错，只会产生更多副本**。
5. `Upsert` 的键是 `Id`；这里每次都新建对象（新 `Id`），所以是纯新增，不会覆盖已有模板。
6. 导入不校验模板合法性（可以导入一个 `Validate()` 报 Error 的模板）。
7. 版本键 `Format`/`SchemaVersion` 被反序列化但**从未被检查**；Rust 版建议保留这两个键并在读取时容忍缺失。

---

## 10. 系统模板同步

源码：`Services/SystemTemplatePresets.cs:13`（版本常量）、`Services/TemplateService.cs:210-264`（同步与重置）。

### 10.1 版本机制

- 版本常量：`SystemTemplatePresets.Version = 4`（`SystemTemplatePresets.cs:13`，注释明确要求
  「**每次修改模板内容都要提升 Version**」）。
- 每个模板被创建时都打上 `PresetVersion = Version`（`SystemTemplatePresets.cs:65`）。
- **没有「每个模板的引入版本」概念**：`CreateAll()` 生成的所有 51 个模板一律是
  `PresetVersion = 4`；仓库历史只有一次提交（`6eb4f3b`），无法从历史还原「哪个模板在哪个版本引入」。
  实现同步只需要「当前常量」与「库中每条的 `PresetVersion`」两个数字。
- `ITemplateService.PresetVersion` 暴露该常量（`TemplateService.cs:33`）。

### 10.2 增量同步算法（`InitializeSystemTemplatesAsync`，逐字等价）

```
fn initialize_system_templates() -> SystemTemplateSyncResult {
    let presets  = SystemTemplatePresets::create_all();            // 全新的对象，Id 也是新的随机值
    let existing = db.templates.find(|t| t.type == System);        // 只查系统模板

    // 按 Name 分组（C# 用 CurrentCulture 比较），同名多份时取第一条
    let mut by_name: Map<String, CommandTemplate> = group_by_name_take_first(existing);

    let (mut added, mut updated, mut unchanged) = (0, 0, 0);

    for mut preset in presets {
        match by_name.get(&preset.name) {
            Some(current) if current.preset_version >= preset.preset_version => {
                unchanged += 1;
            }
            Some(current) => {
                preset.id         = current.id;                    // 保留原 Id：已有任务/批次引用不失效
                preset.created_at = current.created_at;
                preset.updated_at = now_utc();
                db.templates.update(preset);                       // 整条覆盖
                updated += 1;
            }
            None => { db.templates.insert(preset); added += 1; }
        }
    }

    SystemTemplateSyncResult { added, updated, unchanged, preset_version: CURRENT }
}
```

`SystemTemplateSyncResult`（`Services/ITemplateService.cs:74-83`）：

```
struct SystemTemplateSyncResult { added: i32, updated: i32, unchanged: i32, preset_version: i32 }
impl { fn changed(&self) -> bool { self.added > 0 || self.updated > 0 }
       fn summary(&self) -> String {
           if self.changed() { format!("系统模板已更新：新增 {} 个，更新 {} 个（预设 v{}）", self.added, self.updated, self.preset_version) }
           else             { format!("系统模板已是最新（预设 v{}）", self.preset_version) }
       } }
```

### 10.3 升级行为（应用升级时到底发生什么）

| 场景 | 结果 |
|---|---|
| 全新安装 | 51 条全部 `Insert`；`Summary = "系统模板已更新：新增 51 个，更新 0 个（预设 v4）"`（测试只断言 `Added > 0`） |
| 再次启动（版本未变） | 全部 `unchanged`；`Added = 0, Updated = 0`；`Summary = "系统模板已是最新（预设 v4）"` |
| 升级后 `Version` 从 3 → 4 | 库中每条 `PresetVersion = 3 < 4` → 全部 **整体覆盖**（`Id`/`CreatedAt` 保留，其余字段全按新预设） |
| 升级后**新增**了模板 | 新名字找不到 → `Insert`（新增一条） |
| 升级后**删除**了某个预设 | **不会**从库里删除（库中该条仍停留旧版本号，且再也不会被 `Updated`：`current.PresetVersion(3) >= preset.PresetVersion` 不成立时会走 update… 注意：只有在 `presets` 里还存在同名项时才会被碰到；被删掉的模板根本不在 `presets` 里，因此永远留在库中且从此不再更新） → **陈旧模板残留**，Rust 版需要决策（建议：显式删除「`Type == System` 且当前版本不再存在且 `PresetVersion < CURRENT`」的条目，或标记为归档） |
| 预设改名 | 旧名字留在库中（陈旧），新名字被 `Insert` → **同一模板出现两份** |

### 10.4 用户编辑如何被保留

- **用户模板完全不受影响**：同步只查询/写入 `Type == System` 的记录。
- 「复制系统模板 → 编辑副本」是最推荐的自定义方式（副本是 `Type = User`）。
- 系统模板本身**不能**通过 `UpdateTemplateAsync` 修改（直接返回 `false`），只能由管理端
  `UpdateSystemTemplateAsync` 修改，而它会**把 `PresetVersion` 刷成当前版本**
  （`TemplateService.cs:184-197`）。因此：管理员手改一条系统模板后，只要应用没有再次提升 `Version`，
  这条手改内容在后续启动同步中会被判为「已是最新」而**保留**；一旦 `Version` 提升，就会被预设内容覆盖。
- 同步不会触碰 `Tasks`/`BatchTasks` 中对模板 `Id` 的引用：同一条模板升级时保留 `Id`（`233`）。

### 10.5 重置（`ResetSystemTemplatesAsync`，`249-264`）

```
fn reset_system_templates() {
    for id in db.templates.find(|t| t.type == System).map(|t| t.id) { db.templates.delete(id); }
    db.templates.insert_bulk(SystemTemplatePresets::create_all());     // 全新 Id
}
```

- **所有系统模板的 `Id` 都会变**，历史任务里记录的模板引用会悬空（参考实现未处理）。
- 不做 `EnsureUniqueName`，也不做去重（但 `PresetVersion` 直接是当前值，故后续同步为 `unchanged`）。
- 界面入口：`Components/Pages/TemplatesAdmin.razor:920`。

---

---

## 11. 内置模板完整目录（51 个，逐字）

### 11.0 如何读这一节

- **来源**：`Services/SystemTemplatePresets.cs`，全部由 `CreateAll()`（28-597 行）通过一个**局部函数 `Add`**
  依次登记；另有 5 次 `AddHardwareFamily`（每次生成 2 个模板）与 4 次 `AddScaleTemplate`（每次生成 1 个）
  → **37 + 10 + 4 = 51** 个模板。
- **key/id**：参考实现**没有**稳定字符串 ID（每条的 `Id` 都是随机 `ObjectId`，同步靠 `Name` 匹配）。
  下面每个模板给出的 `key:`（形如 `sys.mp4-h264-auto`）是**本文档为 Rust 版建议的稳定标识**，
  不属于参考实现；Rust 版若采用，请把它作为「预设识别键」并用它做同步，同时保留 `Name` 作为展示名，
  这样预设置换名/改文案都不会产生重复条目（比参考实现更稳，见 §13）。
- **version introduced**：全部为 **4**（`SystemTemplatePresets.Version = 4`，见 §10.1；仓库无更早历史）。
- **flags**：只有两种 —— `system`（`Type = System`）与 `recommended`（`IsRecommended = true`）。
  **不存在 `hidden` 标记**；未列入本节的画像（如 Vulkan Video）没有模板就是「不做入口」。
- **`command_args`**：给出的是 C# 多段字符串拼接**之后**的最终值（`"…" + "…"` 直接连接，**不插空格**），
  已逐字核对。C# 源码里的写法用双引号包裹带空格的 token。
- **`parameters`** 每项给出：`name / label / kind / default / min / max / true / false / description / options`。
  `min`/`max` 为 `null` 表示未设置（C# `double?` 为 null → JSON 中该键被省略）。
  `options` 每项为 `[value, label, description]` 三元组。
- **公共参数对象复用（重要）**：`qualityAuto`（72-87 行）与 `audioBitrate`（89-99 行）在源码中是
  **同一个对象实例**被塞进多个模板的 `Parameters` 列表。Rust 版必须**按值深拷贝**，
  否则改一个模板的参数会污染其它模板。

---

### [S01] 推荐 · 0 · `sys.mp4-h264-auto`

```yaml
name: "MP4 · H.264（自动选择最佳编码器）"
category: 推荐
sort_order: 0
preset_version: 4
type: System
flags: [system, recommended]
output_extension: mp4
input_formats: []            # 空 = 任意格式
target_codec: h264
hardware_mode: Auto(1)
required_encoder: null
requires_hardware_acceleration: false
required_filters: []
tags: [mp4, h264, 通用, 硬件加速]
source_lines: SystemTemplatePresets.cs:102-112
description: "输出兼容性最好的 H.264 MP4。会自动使用本机可用的硬件编码器（NVENC / Quick Sync / AMF / VideoToolbox / VA-API），没有硬件时回退到 x264。"
command_args: >
  -i "{input}" -c:v {encoder} {quality_args} {preset_args} {hwupload_args} -map 0:v:0 -map 0:a? -c:a {audio_encoder} -b:a {audio_bitrate} -movflags +faststart -pix_fmt yuv420p "{output}"
parameters:
  - qualityAuto
  - audioBitrate
```

`qualityAuto`（`SystemTemplatePresets.cs:72-87`，逐字）：

```yaml
name: quality ; label: 质量 ; kind: Select(0) ; default: ""
description: "数值越小画质越好、文件越大。留空则由当前编码器的最佳默认值决定。"
min: null ; max: null ; true: "1" ; false: ""
options:
  - ["",   "自动（推荐）", ""]
  - ["18", "高画质",      ""]
  - ["23", "均衡",        ""]
  - ["28", "小体积",      ""]
  - ["32", "极小体积",    ""]
```

`audioBitrate`（`SystemTemplatePresets.cs:89-99`，逐字）：

```yaml
name: audio_bitrate ; label: 音频码率 ; kind: Select(0) ; default: "128k"
description: ""        # 未设置 → 空字符串
min: null ; max: null ; true: "1" ; false: ""
options:
  - ["96k",  "96 kbps",  ""]
  - ["128k", "128 kbps", ""]
  - ["192k", "192 kbps", ""]
  - ["256k", "256 kbps", ""]
```

校验结果：无 Error / 无 Warning（`{encoder}` 存在，故不触发 #17）。

---

### [S02] 推荐 · 1 · `sys.mp4-hevc-auto`

```yaml
name: "MP4 · H.265 / HEVC（自动选择最佳编码器）"
category: 推荐
sort_order: 1
preset_version: 4
type: System
flags: [system, recommended]
output_extension: mp4
input_formats: []
target_codec: hevc
hardware_mode: Auto(1)
required_encoder: null
requires_hardware_acceleration: false
required_filters: []
tags: [mp4, hevc, 压缩, 硬件加速]
source_lines: SystemTemplatePresets.cs:114-124
description: "同等画质下体积比 H.264 小约 40%。会自动使用本机可用的硬件编码器，没有硬件时回退到 x265。"
command_args: >
  -i "{input}" -c:v {encoder} {quality_args} {preset_args} {hwupload_args} -map 0:v:0 -map 0:a? -c:a {audio_encoder} -b:a {audio_bitrate} -tag:v hvc1 -movflags +faststart "{output}"
parameters: [qualityAuto, audioBitrate]
```

---

### [S03] 推荐 · 2 · `sys.audio-mp3`

```yaml
name: "音频 · 提取为 MP3"
category: 推荐
sort_order: 2
preset_version: 4
type: System
flags: [system, recommended]
output_extension: mp3
input_formats: [mp4, mkv, avi, mov, webm, flv, ts, wav, flac, m4a, aac, ogg, wmv]
target_codec: ""
hardware_mode: None(0)
required_encoder: null
requires_hardware_acceleration: false
required_filters: []
tags: [mp3, 音频, 提取]
source_lines: SystemTemplatePresets.cs:126-133
description: "从视频或音频中提取音轨并转成通用性最好的 MP3。"
command_args: >
  -i "{input}" -vn -map 0:a:0 -c:a libmp3lame -b:a {audio_bitrate} "{output}"
parameters: [audioBitrate]
```

---

### [S04] 推荐 · 3 · `sys.fast-remux`

```yaml
name: "快速换容器（不改画质，秒完成）"
category: 推荐
sort_order: 3
preset_version: 4
type: System
flags: [system, recommended]
output_extension: mp4
input_formats: []
target_codec: ""
hardware_mode: None(0)
required_encoder: null
requires_hardware_acceleration: false
required_filters: []
tags: [封装, 无损, 快速]
source_lines: SystemTemplatePresets.cs:135-142
description: "只重新封装容器，不重新编码，速度极快、画质完全无损。仅当目标容器支持源编码格式时可用。"
command_args: >
  -i "{input}" -map 0 -c copy -movflags +faststart "{output}"
parameters: []
```

校验结果：触发 Info #12（`-c copy` 提示）。无 `{encoder}`、无参数 → **试运行时会被参考实现的缺陷 A 误判失败**（§8.6）。

---

### [S05] 视频转码 · 4 · `sys.transcode-mp4-x264`

```yaml
name: "转换为 MP4（H.264 软件编码 x264)"
category: 视频转码
sort_order: 4
preset_version: 4
type: System
flags: [system]
output_extension: mp4
input_formats: [avi, mkv, mov, wmv, flv, webm, ts, mpg, m4v, 3gp]
target_codec: ""
hardware_mode: None(0)
required_encoder: null
requires_hardware_acceleration: false
required_filters: []
tags: [mp4, h264, 软件编码]
source_lines: SystemTemplatePresets.cs:143-165
description: "使用 CPU 软件编码，画质与体积控制最精细，兼容所有平台。"
command_args: >
  -i "{input}" -c:v libx264 -crf {quality} -preset {preset} -c:a aac -b:a {audio_bitrate} -movflags +faststart -pix_fmt yuv420p "{output}"
parameters:
  - name: quality ; label: "CRF 质量" ; kind: Select(0) ; default: "23"
    description: "18 接近无损，23 为默认均衡值，28 体积明显更小。"
    min: null ; max: null ; true: "1" ; false: ""
    options:
      - ["18", "18 · 高画质", ""]
      - ["23", "23 · 均衡",   ""]
      - ["28", "28 · 小体积", ""]
  - name: preset ; label: "编码速度" ; kind: Select(0) ; default: "medium"
    description: "越慢压缩率越高（体积更小）。"
    min: null ; max: null ; true: "1" ; false: ""
    options:
      - ["veryfast", "很快",          ""]
      - ["fast",     "快",            ""]
      - ["medium",   "中等（推荐）",  ""]
      - ["slow",     "慢 · 更小",     ""]
      - ["veryslow", "很慢 · 最小",   ""]
  - audioBitrate
```

---

### [S06] 视频转码 · 5 · `sys.transcode-mkv-x265`

```yaml
name: "转换为 MKV（H.265 软件编码 x265)"
category: 视频转码
sort_order: 5
preset_version: 4
type: System
flags: [system]
output_extension: mkv
input_formats: []
target_codec: ""
hardware_mode: None(0)
required_encoder: null
requires_hardware_acceleration: false
required_filters: []
tags: [mkv, hevc, 归档]
source_lines: SystemTemplatePresets.cs:167-186
description: "MKV 容器 + x265 软件编码，适合本地归档保存。"
command_args: >
  -i "{input}" -c:v libx265 -crf {quality} -preset {preset} -c:a aac -b:a {audio_bitrate} "{output}"
parameters:
  - name: quality ; label: "CRF 质量" ; kind: Select(0) ; default: "28"
    description: "x265 的 CRF 通常比 x264 高 3~5。"
    min: null ; max: null ; true: "1" ; false: ""
    options:
      - ["22", "22 · 高画质", ""]
      - ["28", "28 · 均衡",   ""]
      - ["33", "33 · 小体积", ""]
  - name: preset ; label: "编码速度" ; kind: Select(0) ; default: "medium"
    description: ""          # 源码未设置 Description
    min: null ; max: null ; true: "1" ; false: ""
    options:
      - ["veryfast", "很快",         ""]
      - ["medium",   "中等（推荐）", ""]
      - ["slow",     "慢 · 更小",    ""]
  - audioBitrate
```

---

### [S07] 视频转码 · 6 · `sys.transcode-webm-vp9`

```yaml
name: "转换为 WebM（VP9 + Opus）"
category: 视频转码
sort_order: 6
preset_version: 4
type: System
flags: [system]
output_extension: webm
input_formats: []
target_codec: ""
hardware_mode: None(0)
required_encoder: null
requires_hardware_acceleration: false
required_filters: []
tags: [webm, vp9]
source_lines: SystemTemplatePresets.cs:188-201
description: "开源免专利格式，适合网页嵌入。"
command_args: >
  -i "{input}" -c:v libvpx-vp9 -crf {quality} -b:v 0 -row-mt 1 -c:a libopus -b:a {audio_bitrate} "{output}"
parameters:
  - name: quality ; label: "CRF 质量" ; kind: Select(0) ; default: "32"
    description: ""
    min: null ; max: null ; true: "1" ; false: ""
    options:
      - ["24", "24 · 高画质", ""]
      - ["32", "32 · 均衡",   ""]
      - ["40", "40 · 小体积", ""]
  - audioBitrate
```

---

### [S08] 视频转码 · 7 · `sys.transcode-av1-svt`

```yaml
name: "转换为 AV1（SVT-AV1，体积最小)"
category: 视频转码
sort_order: 7
preset_version: 4
type: System
flags: [system]
output_extension: mp4
input_formats: []
target_codec: av1
hardware_mode: None(0)
required_encoder: null
requires_hardware_acceleration: false
required_filters: []
tags: [av1, mp4, 压缩]
source_lines: SystemTemplatePresets.cs:203-224
description: "压缩率最高的开放格式，编码速度较慢但体积最小。需要 FFmpeg 编译了 libsvtav1。"
command_args: >
  -i "{input}" -c:v libsvtav1 -crf {quality} -preset {preset} -c:a aac -b:a {audio_bitrate} -movflags +faststart "{output}"
parameters:
  - name: quality ; label: "CRF 质量" ; kind: Select(0) ; default: "32"
    description: ""
    min: null ; max: null ; true: "1" ; false: ""
    options:
      - ["24", "24 · 高画质", ""]
      - ["32", "32 · 均衡",   ""]
      - ["40", "40 · 小体积", ""]
  - name: preset ; label: "编码速度" ; kind: Select(0) ; default: "8"
    description: "数字越大越快、压缩率越低。"
    min: null ; max: null ; true: "1" ; false: ""
    options:
      - ["4",  "4 · 最慢/最小", ""]
      - ["8",  "8 · 均衡（推荐）", ""]
      - ["12", "12 · 快", ""]
  - audioBitrate
```

---

### [S09] 视频转码 · 8 · `sys.transcode-mov-prores`

```yaml
name: "转换为 MOV（ProRes 422，剪辑友好）"
category: 视频转码
sort_order: 8
preset_version: 4
type: System
flags: [system]
output_extension: mov
input_formats: []
target_codec: prores
hardware_mode: None(0)
required_encoder: null
requires_hardware_acceleration: false
required_filters: []
tags: [mov, prores, 剪辑]
source_lines: SystemTemplatePresets.cs:226-232
description: "生成适合 Final Cut / Premiere 剪辑的中间码率素材，体积较大。"
command_args: >
  -i "{input}" -c:v prores_ks -profile:v 3 -vendor apl0 -pix_fmt yuv422p10le -c:a pcm_s16le "{output}"
parameters: []
```

---

### [S10] 视频转码 · 9 · `sys.gif`

```yaml
name: "转换为 GIF 动图"
category: 视频转码
sort_order: 9
preset_version: 4
type: System
flags: [system]
output_extension: gif
input_formats: []
target_codec: ""
hardware_mode: None(0)
required_encoder: null
requires_hardware_acceleration: false
required_filters: []
tags: [gif, 动图]
source_lines: SystemTemplatePresets.cs:234-254
description: "生成高质量调色板的 GIF，默认限制在 480 宽、12fps。"
command_args: >
  -i "{input}" -vf "fps={fps},scale={width}:-2:flags=lanczos,split[s0][s1];[s0]palettegen[p];[s1][p]paletteuse" -loop 0 "{output}"
parameters:
  - name: fps ; label: "帧率" ; kind: Select(0) ; default: "12"
    description: ""
    min: null ; max: null ; true: "1" ; false: ""
    options:
      - ["8",  "8 fps · 最小",  ""]
      - ["12", "12 fps · 推荐", ""]
      - ["15", "15 fps",        ""]
      - ["25", "25 fps · 最流畅", ""]
  - name: width ; label: "宽度" ; kind: Select(0) ; default: "480"
    description: "高度按比例自动计算且保证为偶数。"
    min: null ; max: null ; true: "1" ; false: ""
    options:
      - ["320", "320 px", ""]
      - ["480", "480 px", ""]
      - ["640", "640 px", ""]
      - ["800", "800 px", ""]
```

校验结果：`fps`、`width` 与内置占位符同名 → 触发 **2 条 Warning #15**（「参数名…与内置占位符同名，内置值可能被覆盖。建议改名。」）。
运行期因为参数覆盖发生在内置值之后（§4.4），**参数值生效**：`{fps}` 用参数值（默认 12）而不是源视频帧率，
`{width}` 用参数值（默认 480）而不是源视频宽度。Rust 版必须保持这一优先级，否则行为变化。
另外注意该模板的 `-vf` 里含 `split[s0][s1]` 等方括号（不是占位符），且 `;` 不需要转义（走 argv）。

---

### [S11] 硬件加速 · 10 · `sys.hw-h264-auto`

```yaml
name: "硬件加速 · H.264（自动挑选可用编码器）"
category: 硬件加速
sort_order: 10
preset_version: 4
type: System
flags: [system]
output_extension: mp4
input_formats: []
target_codec: h264
hardware_mode: Auto(1)
required_encoder: null
requires_hardware_acceleration: false
required_filters: []
tags: [硬件加速, h264]
source_lines: SystemTemplatePresets.cs:255-263
description: "自动使用本机最合适的 H.264 硬件编码器；模板会带上对应编码器的正确质量参数。"
command_args: >
  -i "{input}" -c:v {encoder} {quality_args} {preset_args} {hwupload_args} -c:a {audio_encoder} -b:a {audio_bitrate} -movflags +faststart "{output}"
parameters: [qualityAuto, audioBitrate]
```

---

### [S12] 硬件加速 · 11 · `sys.hw-hevc-auto`

```yaml
name: "硬件加速 · H.265（自动挑选可用编码器）"
category: 硬件加速
sort_order: 11
preset_version: 4
type: System
flags: [system]
output_extension: mp4
input_formats: []
target_codec: hevc
hardware_mode: Auto(1)
required_encoder: null
requires_hardware_acceleration: false
required_filters: []
tags: [硬件加速, hevc]
source_lines: SystemTemplatePresets.cs:265-273
description: "自动使用本机最合适的 H.265 硬件编码器，画质/体积比优于 H.264。"
command_args: >
  -i "{input}" -c:v {encoder} {quality_args} {preset_args} {hwupload_args} -c:a {audio_encoder} -b:a {audio_bitrate} -tag:v hvc1 -movflags +faststart "{output}"
parameters: [qualityAuto, audioBitrate]
```

---

### [S13]–[S22] 厂商专用硬件模板（10 个，由 `AddHardwareFamily` 生成）

生成器源码：`SystemTemplatePresets.cs:599-643`；调用点：275-279 行（顺序固定为 nvenc、qsv、amf、videotoolbox、vaapi）。
每个族生成 **H.264** 与 **H.265 / HEVC** 两个模板，二者除 `target_codec`、命令里的 `-tag:v hvc1`、
名字后缀与描述末句外完全相同。

**五个族的参数（按调用顺序）**

| 调用 | `displayFamily` | `family`（= `required_encoder`） | `vendorDescription` | 生成的 sort_order |
|---|---|---|---|---|
| 1 | `NVIDIA NVENC` | `nvenc` | `NVIDIA 显卡` | 12（H.264）、13（H.265） |
| 2 | `Intel Quick Sync` | `qsv` | `Intel 核显 / 独显` | 14、15 |
| 3 | `AMD AMF` | `amf` | `AMD 显卡` | 16、17 |
| 4 | `Apple VideoToolbox` | `videotoolbox` | `Apple Silicon / macOS` | 18、19 |
| 5 | `Linux VA-API` | `vaapi` | `Linux 下的 Intel / AMD 显卡` | 20、21 |

**名字与描述模板（逐字，`{}` 为 C# 内插占位）**

```
H.264 名字: "{displayFamily} · H.264"
H.265 名字: "{displayFamily} · H.265 / HEVC"

H.264 描述: "使用 {vendorDescription} 的 {displayFamily} 硬件编码 H.264，速度快、CPU 占用低。"
H.265 描述: "使用 {vendorDescription} 的 {displayFamily} 硬件编码 H.265，画质/体积比更好。"
```

（注意：`"使用 " + vendorDescription + " 的 "` 会产生**一个空格**：例如 `使用 NVIDIA 显卡 的 NVIDIA NVENC 硬件编码 H.264，速度快、CPU 占用低。`）

**两个命令模板（逐字）**

```text
H.264: -i "{input}" -c:v {encoder} {quality_args} {preset_args} {hwupload_args} -c:a {audio_encoder} -b:a {audio_bitrate} -movflags +faststart "{output}"
H.265: -i "{input}" -c:v {encoder} {quality_args} {preset_args} {hwupload_args} -c:a {audio_encoder} -b:a {audio_bitrate} -tag:v hvc1 -movflags +faststart "{output}"
```

**公共元数据**

```yaml
category: 硬件加速
preset_version: 4
type: System
flags: [system]               # 注意：这 10 个模板 recommended=false
output_extension: mp4
input_formats: []             # 源码传 null → []
hardware_mode: Required(2)
requires_hardware_acceleration: true
tags: ["硬件加速", "h264"|"hevc", <family>]     # 例如 ["硬件加速","h264","nvenc"]
required_filters: []          # 源码传 null → []
```

**参数（每个模板 2 个，`hardQuality` + `audio`；注意这两个对象在 `AddHardwareFamily` 内部**每次调用**新建，
5 次调用共 5 组不同实例，但内容完全相同）**

```yaml
- name: quality ; label: 质量 ; kind: Select(0) ; default: ""
  description: "留空使用该编码器的推荐默认值。"
  min: null ; max: null ; true: "1" ; false: ""
  options:                       # 与 S01 的 qualityAuto 不同：这里是 20/24/28/32
    - ["",   "自动（推荐）", ""]
    - ["20", "高画质",      ""]
    - ["24", "均衡",        ""]
    - ["28", "小体积",      ""]
    - ["32", "极小体积",    ""]
- name: audio_bitrate ; label: 音频码率 ; kind: Select(0) ; default: "128k"
  description: ""
  min: null ; max: null ; true: "1" ; false: ""
  options:
    - ["96k",  "96 kbps",  ""]
    - ["128k", "128 kbps", ""]
    - ["192k", "192 kbps", ""]
    - ["256k", "256 kbps", ""]
```

**逐个条目**（描述已代入插值结果，逐字）：

| 编号 | key（建议） | name | source_lines | description |
|---|---|---|---|---|
| S13 | `sys.hw-nvenc-h264` | `NVIDIA NVENC · H.264` | 275（生成器 630-635） | `使用 NVIDIA 显卡 的 NVIDIA NVENC 硬件编码 H.264，速度快、CPU 占用低。` |
| S14 | `sys.hw-nvenc-hevc` | `NVIDIA NVENC · H.265 / HEVC` | 275（生成器 637-642） | `使用 NVIDIA 显卡 的 NVIDIA NVENC 硬件编码 H.265，画质/体积比更好。` |
| S15 | `sys.hw-qsv-h264` | `Intel Quick Sync · H.264` | 276 | `使用 Intel 核显 / 独显 的 Intel Quick Sync 硬件编码 H.264，速度快、CPU 占用低。` |
| S16 | `sys.hw-qsv-hevc` | `Intel Quick Sync · H.265 / HEVC` | 276 | `使用 Intel 核显 / 独显 的 Intel Quick Sync 硬件编码 H.265，画质/体积比更好。` |
| S17 | `sys.hw-amf-h264` | `AMD AMF · H.264` | 277 | `使用 AMD 显卡 的 AMD AMF 硬件编码 H.264，速度快、CPU 占用低。` |
| S18 | `sys.hw-amf-hevc` | `AMD AMF · H.265 / HEVC` | 277 | `使用 AMD 显卡 的 AMD AMF 硬件编码 H.265，画质/体积比更好。` |
| S19 | `sys.hw-videotoolbox-h264` | `Apple VideoToolbox · H.264` | 278 | `使用 Apple Silicon / macOS 的 Apple VideoToolbox 硬件编码 H.264，速度快、CPU 占用低。` |
| S20 | `sys.hw-videotoolbox-hevc` | `Apple VideoToolbox · H.265 / HEVC` | 278 | `使用 Apple Silicon / macOS 的 Apple VideoToolbox 硬件编码 H.265，画质/体积比更好。` |
| S21 | `sys.hw-vaapi-h264` | `Linux VA-API · H.264` | 279 | `使用 Linux 下的 Intel / AMD 显卡 的 Linux VA-API 硬件编码 H.264，速度快、CPU 占用低。` |
| S22 | `sys.hw-vaapi-hevc` | `Linux VA-API · H.265 / HEVC` | 279 | `使用 Linux 下的 Intel / AMD 显卡 的 Linux VA-API 硬件编码 H.265，画质/体积比更好。` |

`target_codec` 依次为：S13 h264、S14 hevc、S15 h264、S16 hevc、S17 h264、S18 hevc、S19 h264、S20 hevc、S21 h264、S22 hevc。

运行期要点（Rust 版必须保持）：`hardware_mode = Required` + `required_encoder = <族>` →
本机没有该族可用编码器时**拒绝执行/试运行失败**（不降级到别的厂商）；VAAPI 族还会自动带上
`-vaapi_device /dev/dri/renderD128`（画像的 `DeviceArgs`）与把 `format=nv12,hwupload` 并入 `-vf`
（因为命令里已有 `{hwupload_args}`，合并逻辑不介入，展开由占位符完成）。

---

### [S23] 视频压缩 · 22 · `sys.compress-crf18`

```yaml
name: "压缩 · 高画质（CRF 18)"
category: 视频压缩
sort_order: 22
preset_version: 4
type: System
flags: [system]
output_extension: mp4
input_formats: []
target_codec: ""
hardware_mode: None(0)
required_encoder: null
requires_hardware_acceleration: false
required_filters: []
tags: [压缩]
source_lines: SystemTemplatePresets.cs:282-285
description: "肉眼几乎无损失，体积约为原文件的一半。"
command_args: >
  -i "{input}" -c:v libx264 -crf 18 -preset slow -c:a aac -b:a 192k -movflags +faststart "{output}"
parameters: []
```

---

### [S24] 视频压缩 · 23 · `sys.compress-crf23`

```yaml
name: "压缩 · 均衡（CRF 23)"
category: 视频压缩
sort_order: 23
preset_version: 4
type: System
flags: [system]
output_extension: mp4
input_formats: []
target_codec: ""
hardware_mode: None(0)
required_encoder: null
requires_hardware_acceleration: false
required_filters: []
tags: [压缩]
source_lines: SystemTemplatePresets.cs:287-290
description: "在画质和体积之间取得平衡，适合大多数场景。"
command_args: >
  -i "{input}" -c:v libx264 -crf 23 -preset medium -c:a aac -b:a 128k -movflags +faststart "{output}"
parameters: []
```

---

### [S25] 视频压缩 · 24 · `sys.compress-crf28`

```yaml
name: "压缩 · 小体积（CRF 28)"
category: 视频压缩
sort_order: 24
preset_version: 4
type: System
flags: [system]
output_extension: mp4
input_formats: []
target_codec: ""
hardware_mode: None(0)
required_encoder: null
requires_hardware_acceleration: false
required_filters: []
tags: [压缩]
source_lines: SystemTemplatePresets.cs:292-295
description: "明显缩小体积，适合手机观看或分享。"
command_args: >
  -i "{input}" -c:v libx264 -crf 28 -preset fast -c:a aac -b:a 96k -movflags +faststart "{output}"
parameters: []
```

---

### [S26] 视频压缩 · 25 · `sys.compress-max-size`

```yaml
name: "压缩 · 限制文件大小"
category: 视频压缩
sort_order: 25
preset_version: 4
type: System
flags: [system]
output_extension: mp4
input_formats: []
target_codec: ""
hardware_mode: None(0)
required_encoder: null
requires_hardware_acceleration: false
required_filters: []
tags: [压缩, 体积限制]
source_lines: SystemTemplatePresets.cs:297-323
description: "按目标体积限制输出文件大小，适合有上传大小限制的场景。"
command_args: >
  -i "{input}" -c:v libx264 -preset fast -crf 26 -maxrate {maxrate} -bufsize {bufsize} -fs {maxsize}M -c:a aac -b:a 96k -movflags +faststart "{output}"
parameters:
  - name: maxsize ; label: "目标大小 (MB)" ; kind: Number(1) ; default: "50"
    description: "达到该大小后 FFmpeg 会停止写入。"
    min: 1 ; max: 100000 ; true: "1" ; false: ""
    options: []
  - name: maxrate ; label: "最大码率" ; kind: Select(0) ; default: "1500k"
    description: ""
    min: null ; max: null ; true: "1" ; false: ""
    options:
      - ["800k",  "800 kbps",  ""]
      - ["1500k", "1500 kbps", ""]
      - ["3000k", "3000 kbps", ""]
      - ["6000k", "6000 kbps", ""]
  - name: bufsize ; label: "缓冲区" ; kind: Select(0) ; default: "3000k"
    description: ""
    min: null ; max: null ; true: "1" ; false: ""
    options:
      - ["1600k",  "1600 kbps",  ""]
      - ["3000k",  "3000 kbps",  ""]
      - ["6000k",  "6000 kbps",  ""]
      - ["12000k", "12000 kbps", ""]
```

这是**唯一**使用 `Number` 类型的内置模板；`Min=1`/`Max=100000` 只影响界面 `min`/`max` 属性与 `(1 ~ 100000)` 提示，
**不做夹紧、不做校验**。

---

### [S27]–[S30] 分辨率与画面 · 26–29 · 缩放模板（由 `AddScaleTemplate` 生成）

生成器源码：`SystemTemplatePresets.cs:645-657`；调用点 324-327 行。

**四个条目**

| 编号 | key（建议） | name | width | height | sort_order | tags | source_lines |
|---|---|---|---|---|---|---|---|
| S27 | `sys.scale-4k` | `调整为 4K（3840×2160）` | 3840 | 2160 | 26 | `["缩放","2160p"]` | 324 |
| S28 | `sys.scale-1080p` | `调整为 1080p（1920×1080）` | 1920 | 1080 | 27 | `["缩放","1080p"]` | 325 |
| S29 | `sys.scale-720p` | `调整为 720p（1280×720）` | 1280 | 720 | 28 | `["缩放","720p"]` | 326 |
| S30 | `sys.scale-480p` | `调整为 480p（854×480）` | 854 | 480 | 29 | `["缩放","480p"]` | 327 |

**公共元数据**

```yaml
category: 分辨率与画面
preset_version: 4
type: System
flags: [system]                 # recommended=false
output_extension: mp4
input_formats: []               # 源码传 null → []
target_codec: h264
hardware_mode: None(0)          # 注意：不是 Auto
required_encoder: null          # 源码传 null
requires_hardware_acceleration: false
required_filters: []            # 源码传 null → []
parameters: []                  # 源码传 null → []
```

**描述（逐字，`{width}`/`{height}` 已代入）**

```
S27: 等比缩放到不超过 3840×2160，必要时用黑边补齐，保证画面不变形。
S28: 等比缩放到不超过 1920×1080，必要时用黑边补齐，保证画面不变形。
S29: 等比缩放到不超过 1280×720，必要时用黑边补齐，保证画面不变形。
S30: 等比缩放到不超过 854×480，必要时用黑边补齐，保证画面不变形。
```

**命令模板（逐字，代入 width/height 后；C# 是三段字符串拼接，拼接处无空格但源码片段自带尾随空格）**

```text
S27: -i "{input}" -vf "scale=3840:2160:force_original_aspect_ratio=decrease,pad=3840:2160:(ow-iw)/2:(oh-ih)/2,setsar=1" -c:v libx264 -crf 23 -preset medium -c:a aac -b:a 128k -movflags +faststart "{output}"
S28: -i "{input}" -vf "scale=1920:1080:force_original_aspect_ratio=decrease,pad=1920:1080:(ow-iw)/2:(oh-ih)/2,setsar=1" -c:v libx264 -crf 23 -preset medium -c:a aac -b:a 128k -movflags +faststart "{output}"
S29: -i "{input}" -vf "scale=1280:720:force_original_aspect_ratio=decrease,pad=1280:720:(ow-iw)/2:(oh-ih)/2,setsar=1" -c:v libx264 -crf 23 -preset medium -c:a aac -b:a 128k -movflags +faststart "{output}"
S30: -i "{input}" -vf "scale=854:480:force_original_aspect_ratio=decrease,pad=854:480:(ow-iw)/2:(oh-ih)/2,setsar=1" -c:v libx264 -crf 23 -preset medium -c:a aac -b:a 128k -movflags +faststart "{output}"
```

---

### [S31] 分辨率与画面 · 30 · `sys.scale-width`

```yaml
name: "缩放 · 指定宽度（高度自适应）"
category: 分辨率与画面
sort_order: 30
preset_version: 4
type: System
flags: [system]
output_extension: mp4
input_formats: []
target_codec: ""
hardware_mode: None(0)
required_encoder: null
requires_hardware_acceleration: false
required_filters: []
tags: [缩放]
source_lines: SystemTemplatePresets.cs:329-342
description: "只指定宽度，高度按原始比例自动计算，并保证为偶数。"
command_args: >
  -i "{input}" -vf "scale={width}:-2:flags=lanczos" -c:v libx264 -crf 23 -c:a aac -b:a 128k -movflags +faststart "{output}"
parameters:
  - name: width ; label: "目标宽度 (px)" ; kind: Select(0) ; default: "1280"
    description: ""
    min: null ; max: null ; true: "1" ; false: ""
    options:
      - ["640",  "640",  ""]
      - ["854",  "854",  ""]
      - ["1280", "1280", ""]
      - ["1920", "1920", ""]
      - ["2560", "2560", ""]
      - ["3840", "3840", ""]
```

参数名 `width` 与内置占位符同名 → 1 条 Warning #15；运行期参数值覆盖源视频宽度（§3.5）。

---

### [S32] 分辨率与画面 · 31 · `sys.crop-vertical-9x16`

```yaml
name: "裁剪 · 竖屏 9:16（1080×1920）"
category: 分辨率与画面
sort_order: 31
preset_version: 4
type: System
flags: [system]
output_extension: mp4
input_formats: []
target_codec: ""
hardware_mode: None(0)
required_encoder: null
requires_hardware_acceleration: false
required_filters: []
tags: [竖屏, 裁剪]
source_lines: SystemTemplatePresets.cs:344-348
description: "居中裁剪为竖屏短视频比例，适合抖音 / 视频号 / Shorts。"
command_args: >
  -i "{input}" -vf "scale=1080:1920:force_original_aspect_ratio=increase,crop=1080:1920" -c:v libx264 -crf 23 -c:a aac -b:a 128k -movflags +faststart "{output}"
parameters: []
```

---

### [S33] 分辨率与画面 · 32 · `sys.rotate-90`

```yaml
name: "旋转 · 顺时针 90°"
category: 分辨率与画面
sort_order: 32
preset_version: 4
type: System
flags: [system]
output_extension: mp4
input_formats: []
target_codec: ""
hardware_mode: None(0)
required_encoder: null
requires_hardware_acceleration: false
required_filters: []
tags: [旋转]
source_lines: SystemTemplatePresets.cs:350-363
description: "把横屏视频旋转为竖屏（或反过来）。"
command_args: >
  -i "{input}" -vf "transpose=1" -c:v libx264 -crf 23 -c:a aac -b:a 128k -movflags +faststart "{output}"
parameters:
  - name: angle ; label: "方向" ; kind: Select(0) ; default: "1"
    description: "对应 FFmpeg transpose 参数。"
    min: null ; max: null ; true: "1" ; false: ""
    options:
      - ["1", "顺时针 90°", ""]
      - ["2", "逆时针 90°", ""]
```

> **参考实现缺陷（Rust 版需决策）**：命令里写死了 `transpose=1`，参数 `angle` **没有任何占位符引用它**，
> 因此选择「逆时针 90°」不会改变命令（参数是死参数）。正确做法是把命令写成
> `-vf "transpose={angle}"`（或把 `angle` 从参数里删掉）。

---

### [S34] 分辨率与画面 · 33 · `sys.speed-change`

```yaml
name: "变速 · 修改播放速度"
category: 分辨率与画面
sort_order: 33
preset_version: 4
type: System
flags: [system]
output_extension: mp4
input_formats: []
target_codec: ""
hardware_mode: None(0)
required_encoder: null
requires_hardware_acceleration: false
required_filters: []
tags: [变速]
source_lines: SystemTemplatePresets.cs:365-385
description: "通过调整时间戳实现无音调变化的变速（视频不变调，音频会被重采样）。"
command_args: >
  -i "{input}" -filter_complex "[0:v]setpts=PTS/{speed}[v];[0:a]atempo={atempo}[a]" -map "[v]" -map "[a]" -c:v libx264 -crf 23 -c:a aac -b:a 128k "{output}"
parameters:
  - name: speed ; label: "速度倍率" ; kind: Select(0) ; default: "2"
    description: "大于 1 变快，小于 1 变慢。"
    min: null ; max: null ; true: "1" ; false: ""
    options:
      - ["0.5", "0.5× 慢放", ""]
      - ["1.5", "1.5×",      ""]
      - ["2",   "2× 快放",   ""]
      - ["4",   "4×",        ""]
  - name: atempo ; label: "音频同步倍率" ; kind: Select(0) ; default: "2"
    description: "需要与速度倍率一致，音频允许的范围是 0.5~100。"
    min: null ; max: null ; true: "1" ; false: ""
    options:
      - ["0.5", "0.5×", ""]
      - ["1.5", "1.5×", ""]
      - ["2",   "2×",   ""]
      - ["4",   "4×",   ""]
```

注意：占位符 `{speed}`、`{atempo}` 位于**同一个 token** 内部（`-filter_complex` 的引号内），
`{ativempo}` 之类都不是参数组占位符，因此走「纯文本替换」分支 —— 这正是设计意图。
另注：`[0:a]atempo=...` 需要输入有音频轨，否则 FFmpeg 会失败（模板未做 `?` 容错）。

---

### [S35] 分辨率与画面 · 34 · `sys.deinterlace`

```yaml
name: "去隔行 · 消除交错的横纹"
category: 分辨率与画面
sort_order: 34
preset_version: 4
type: System
flags: [system]
output_extension: mp4
input_formats: []
target_codec: ""
hardware_mode: None(0)
required_encoder: null
requires_hardware_acceleration: false
required_filters: []
tags: [去隔行, 老视频]
source_lines: SystemTemplatePresets.cs:387-393
description: "对老式摄像机/电视录制的隔行视频做反交错处理。"
command_args: >
  -i "{input}" -vf "yadif=1:-1:0" -c:v libx264 -crf 20 -preset medium -c:a aac -b:a 128k -movflags +faststart "{output}"
parameters: []
```

---

### [S36] 音频处理 · 35 · `sys.audio-aac-m4a`

```yaml
name: "音频 · 提取为 AAC / M4A"
category: 音频处理
sort_order: 35
preset_version: 4
type: System
flags: [system]
output_extension: m4a
input_formats: [mp4, mkv, avi, mov, webm, flv, ts, wav, flac, m4a, aac, ogg, wmv]
target_codec: ""
hardware_mode: None(0)
required_encoder: null
requires_hardware_acceleration: false
required_filters: []
tags: [音频, aac]
source_lines: SystemTemplatePresets.cs:394-399
description: "提取音轨并转为 AAC，体积小、兼容性好。"
command_args: >
  -i "{input}" -vn -map 0:a:0 -c:a aac -b:a {audio_bitrate} "{output}"
parameters: [audioBitrate]
```

---

### [S37] 音频处理 · 36 · `sys.audio-flac`

```yaml
name: "音频 · 提取为 FLAC（无损）"
category: 音频处理
sort_order: 36
preset_version: 4
type: System
flags: [system]
output_extension: flac
input_formats: [mp4, mkv, avi, mov, webm, flv, ts, wav, m4a, aac, ogg, wmv]
target_codec: ""
hardware_mode: None(0)
required_encoder: null
requires_hardware_acceleration: false
required_filters: []
tags: [音频, 无损]
source_lines: SystemTemplatePresets.cs:401-406
description: "无损提取音轨，不损失任何音质。"
command_args: >
  -i "{input}" -vn -map 0:a:0 -c:a flac -compression_level 8 "{output}"
parameters: []
```

---

### [S38] 音频处理 · 37 · `sys.audio-wav`

```yaml
name: "音频 · 提取为 WAV（PCM 无损）"
category: 音频处理
sort_order: 37
preset_version: 4
type: System
flags: [system]
output_extension: wav
input_formats: [mp4, mkv, avi, mov, webm, flv, ts, flac, m4a, aac, ogg, wmv]
target_codec: ""
hardware_mode: None(0)
required_encoder: null
requires_hardware_acceleration: false
required_filters: []
tags: [音频, wav, 无损]
source_lines: SystemTemplatePresets.cs:408-413
description: "未压缩 PCM，适合后续在音频软件里继续处理。"
command_args: >
  -i "{input}" -vn -map 0:a:0 -c:a pcm_s16le -ar 48000 "{output}"
parameters: []
```

---

### [S39] 音频处理 · 38 · `sys.audio-opus`

```yaml
name: "音频 · 提取为 Opus"
category: 音频处理
sort_order: 38
preset_version: 4
type: System
flags: [system]
output_extension: opus
input_formats: [mp4, mkv, avi, mov, webm, flv, ts, wav, flac, m4a, aac, ogg]
target_codec: ""
hardware_mode: None(0)
required_encoder: null
requires_hardware_acceleration: false
required_filters: []
tags: [音频, opus]
source_lines: SystemTemplatePresets.cs:415-420
description: "同码率下音质优于 MP3/AAC，适合网络传输。"
command_args: >
  -i "{input}" -vn -map 0:a:0 -c:a libopus -b:a 128k -vbr on "{output}"
parameters: []
```

---

### [S40] 音频处理 · 39 · `sys.audio-loudnorm`

```yaml
name: "音频 · 音量标准化（EBU R128）"
category: 音频处理
sort_order: 39
preset_version: 4
type: System
flags: [system]
output_extension: m4a
input_formats: [mp4, mkv, avi, mov, webm, flv, ts, wav, flac, m4a, aac, ogg, mp3]
target_codec: ""
hardware_mode: None(0)
required_encoder: null
requires_hardware_acceleration: false
required_filters: []
tags: [音频, 响度]
source_lines: SystemTemplatePresets.cs:422-427
description: "把整体响度标准化到 -16 LUFS，避免忽大忽小。"
command_args: >
  -i "{input}" -vn -af "loudnorm=I=-16:TP=-1.5:LRA=11" -c:a aac -b:a 192k "{output}"
parameters: []
```

---

### [S41] 音频处理 · 40 · `sys.remove-audio`

```yaml
name: "视频 · 移除音轨（静音视频）"
category: 音频处理
sort_order: 40
preset_version: 4
type: System
flags: [system]
output_extension: mp4
input_formats: []
target_codec: ""
hardware_mode: None(0)
required_encoder: null
requires_hardware_acceleration: false
required_filters: []
tags: [静音, 去音轨]
source_lines: SystemTemplatePresets.cs:429-434
description: "彻底去掉音频流，只保留画面。"
command_args: >
  -i "{input}" -an -c:v copy "{output}"
parameters: []
```

校验结果：含 `-c:v copy` → 触发 Info #12。

---

### [S42] 特殊处理 · 41 · `sys.watermark-image`

```yaml
name: "画面 · 添加图片水印"
category: 特殊处理
sort_order: 41
preset_version: 4
type: System
flags: [system]
output_extension: mp4
input_formats: []
target_codec: ""
hardware_mode: None(0)
required_encoder: null
requires_hardware_acceleration: false
required_filters: []
tags: [水印, 叠加]
source_lines: SystemTemplatePresets.cs:435-467
description: "在右下角叠加一张 PNG/JPG 水印，可通过参数调整位置与透明度。"
command_args: >
  -i "{input}" -i "{watermark}" -filter_complex "[1:v]format=rgba,colorchannelmixer=aa={opacity}[wm];[0:v][wm]overlay={position}" -c:v libx264 -crf 23 -preset medium -c:a copy "{output}"
parameters:
  - name: watermark ; label: "水印图片路径" ; kind: Text(2) ; default: ""
    description: "填写 PNG/JPG 的完整路径。"
    min: null ; max: null ; true: "1" ; false: ""
    options: []
  - name: position ; label: "水印位置" ; kind: Select(0) ; default: "W-w-20:H-h-20"
    description: ""
    min: null ; max: null ; true: "1" ; false: ""
    options:
      - ["W-w-20:H-h-20",     "右下角", ""]
      - ["20:20",             "左上角", ""]
      - ["W-w-20:20",         "右上角", ""]
      - ["20:H-h-20",         "左下角", ""]
      - ["(W-w)/2:(H-h)/2",   "居中",   ""]
  - name: opacity ; label: "不透明度" ; kind: Select(0) ; default: "1"
    description: ""
    min: null ; max: null ; true: "1" ; false: ""
    options:
      - ["0.3", "30%",     ""]
      - ["0.5", "50%",     ""]
      - ["0.8", "80%",     ""]
      - ["1",   "不透明",  ""]
```

`watermark` 是必填项（无默认值）：留空时 `"{watermark}"` token 解析为空 → **被丢弃**，
`-i` 会缺少参数并导致 FFmpeg 失败（校验层拦不住，因为参数已声明）。
试运行会把该参数替换成测试素材（§2.6）；`TemplateChecks.cs` 同样如此。

---

### [S43] 特殊处理 · 42 · `sys.burn-subtitles`

```yaml
name: "画面 · 烧录字幕（硬字幕）"
category: 特殊处理
sort_order: 42
preset_version: 4
type: System
flags: [system]
output_extension: mp4
input_formats: []
target_codec: ""
hardware_mode: None(0)
required_encoder: null
requires_hardware_acceleration: false
required_filters: [subtitles]
tags: [字幕, 硬字幕]
source_lines: SystemTemplatePresets.cs:469-490
description: "把字幕永久烧进画面，任何播放器都能显示。支持 srt/ass。需要 FFmpeg 编译时启用 libass（Homebrew / winget / gyan.dev 的完整构建都有，部分精简构建没有）。"
command_args: >
  -i "{input}" -vf "subtitles=filename='{subtitle}':force_style='FontSize={fontsize}'" -c:v libx264 -crf 23 -preset medium -c:a copy "{output}"
parameters:
  - name: subtitle ; label: "字幕文件路径" ; kind: Text(2) ; default: ""
    description: "srt / ass 文件的完整路径。路径中不要包含单引号。"
    min: null ; max: null ; true: "1" ; false: ""
    options: []
  - name: fontsize ; label: "字号" ; kind: Select(0) ; default: "24"
    description: ""
    min: null ; max: null ; true: "1" ; false: ""
    options:
      - ["16", "小",   ""]
      - ["24", "中",   ""]
      - ["32", "大",   ""]
      - ["40", "特大", ""]
```

描述是两段字符串字面量拼接（`"…支持 srt/ass。" + "需要 FFmpeg 编译时…"`），**中间没有空格**。
这是**唯一**声明 `required_filters` 的模板：`GetMissingFilters` 返回非空时，界面标记不可用、试运行直接拒绝
（消息见 §8.2）。注意命令里用**单引号**包住路径 —— 单引号内不能含单引号，且 `CommandLineBuilder.Tokenize`
在单引号内不做任何转义（§4.3）。

---

### [S44] 特殊处理 · 43 · `sys.trim-segment`

```yaml
name: "画面 · 按时间裁剪片段"
category: 特殊处理
sort_order: 43
preset_version: 4
type: System
flags: [system]
output_extension: mp4
input_formats: []
target_codec: ""
hardware_mode: None(0)
required_encoder: null
requires_hardware_acceleration: false
required_filters: []
tags: [裁剪, 剪辑]
source_lines: SystemTemplatePresets.cs:492-510
description: "截取视频中的一段，起止时间使用 时:分:秒 格式。"
command_args: >
  -ss {start} -i "{input}" -t {duration} -c:v libx264 -crf 20 -preset medium -c:a aac -b:a 128k -movflags +faststart "{output}"
parameters:
  - name: start ; label: "开始时间" ; kind: Text(2) ; default: "00:00:00"
    description: "格式 时:分:秒，例如 00:01:30。"
    min: null ; max: null ; true: "1" ; false: ""
    options: []
  - name: duration ; label: "时长" ; kind: Text(2) ; default: "00:00:30"
    description: "格式 时:分:秒，例如 00:00:30。"
    min: null ; max: null ; true: "1" ; false: ""
    options: []
```

**位置规则示例**：`-ss` 写在 `-i` **之前**（输入侧快速定位），不要擅自挪到后面（把 `-ss` 放到 `-i` 之后是
输出侧裁剪，语义与速度都不同）。`{start}`/`{duration}` 各自独占一个 token。

---

### [S45] 特殊处理 · 44 · `sys.image-sequence-jpg`

```yaml
name: "画面 · 导出为图片序列（JPG）"
category: 特殊处理
sort_order: 44
preset_version: 4
type: System
flags: [system]
output_extension: jpg
input_formats: []
target_codec: ""
hardware_mode: None(0)
required_encoder: null
requires_hardware_acceleration: false
required_filters: []
tags: [抽帧, 图片]
source_lines: SystemTemplatePresets.cs:512-531
description: "按指定帧率抽帧导出为一组 JPG。文件名会自动带上 4 位序号，例如输出名填 result 会生成 result_0001.jpg、result_0002.jpg ……"
command_args: >
  -i "{input}" -vf fps={fps} -q:v {jpgquality} "{output_pattern}"
parameters:
  - name: fps ; label: "抽帧频率" ; kind: Select(0) ; default: "1"
    description: ""
    min: null ; max: null ; true: "1" ; false: ""
    options:
      - ["0.5", "每 2 秒 1 张", ""]
      - ["1",   "每秒 1 张",    ""]
      - ["2",   "每秒 2 张",    ""]
      - ["5",   "每秒 5 张",    ""]
  - name: jpgquality ; label: "图片质量" ; kind: Select(0) ; default: "3"
    description: "数值越小画质越高（JPEG qscale）。"
    min: null ; max: null ; true: "1" ; false: ""
    options:
      - ["2", "高",         ""]
      - ["3", "中（推荐）", ""]
      - ["8", "小",         ""]
```

**唯一使用 `{output_pattern}` 的模板**：`{output_pattern}` 会展开成 `out_%04d.jpg` 形式的路径。
两个后果（Rust 版必须处理）：
1. 输出路径不再出现在 argv 中，`Build` 插入 `-tag:v`/全局参数时**找不到锚点** → 追加到末尾（§4.8 怪癖 5）。
2. 产出的是**多个文件**：判定成功时不能只检查「一个输出文件是否存在且非空」，
   参考实现的回归检查是 glob `result*`（`tools/verify/TemplateChecks.cs:209-219`）。

另注意 `fps` 与内置占位符同名（Warning #15）。

---

### [S46] 特殊处理 · 45 · `sys.thumbnail`

```yaml
name: "画面 · 生成封面缩略图"
category: 特殊处理
sort_order: 45
preset_version: 4
type: System
flags: [system]
output_extension: jpg
input_formats: []
target_codec: ""
hardware_mode: None(0)
required_encoder: null
requires_hardware_acceleration: false
required_filters: []
tags: [封面, 缩略图]
source_lines: SystemTemplatePresets.cs:533-551
description: "从指定时间点截取一张图片作为视频封面。"
command_args: >
  -ss {seek} -i "{input}" -frames:v 1 -vf "scale={width}:-2" -q:v 2 -pix_fmt yuvj420p "{output}"
parameters:
  - name: seek ; label: "截取时间" ; kind: Text(2) ; default: "00:00:01"
    description: "格式 时:分:秒，例如 00:00:05。"
    min: null ; max: null ; true: "1" ; false: ""
    options: []
  - name: width ; label: "宽度 (px)" ; kind: Select(0) ; default: "1280"
    description: ""
    min: null ; max: null ; true: "1" ; false: ""
    options:
      - ["640",  "640",  ""]
      - ["1280", "1280", ""]
      - ["1920", "1920", ""]
```

> **源码注释（逐字，`SystemTemplatePresets.cs:541`）**：`// 不能叫 time：{time} 是内置占位符（当前时间 HHmmss）`
> —— 参数名故意用 `seek` 而不是 `time`。Rust 版移植时保留该约束与注释。

---

### [S47] 特殊处理 · 46 · `sys.faststart`

```yaml
name: "网络优化 · 仅重建索引（faststart）"
category: 特殊处理
sort_order: 46
preset_version: 4
type: System
flags: [system]
output_extension: mp4
input_formats: [mp4, mov, m4v]
target_codec: ""
hardware_mode: None(0)
required_encoder: null
requires_hardware_acceleration: false
required_filters: []
tags: [faststart, 网络, 无损]
source_lines: SystemTemplatePresets.cs:553-558
description: "不动画面和声音，只把索引移到文件开头，让视频可以先播后下。"
command_args: >
  -i "{input}" -c copy -movflags +faststart "{output}"
parameters: []
```

---

### [S48] 特殊处理 · 47 · `sys.fix-timestamps`

```yaml
name: "网络优化 · 无损调整时间戳（修正音画不同步）"
category: 特殊处理
sort_order: 47
preset_version: 4
type: System
flags: [system]
output_extension: mp4
input_formats: []
target_codec: ""
hardware_mode: None(0)
required_encoder: null
requires_hardware_acceleration: false
required_filters: []
tags: [时间戳, 音画同步, 无损]
source_lines: SystemTemplatePresets.cs:560-563
description: "重新生成时间戳，修复某些录制工具产生的音画不同步问题。"
command_args: >
  -i "{input}" -c copy -fflags +genpts -avoid_negative_ts make_zero "{output}"
parameters: []
```

---

### [S49] 特殊处理 · 48 · `sys.soft-subtitle-track`

```yaml
name: "字幕 · 封装为可选字幕轨（不重新编码）"
category: 特殊处理
sort_order: 48
preset_version: 4
type: System
flags: [system]
output_extension: mp4
input_formats: [mp4, mkv, mov, m4v, webm, ts]
target_codec: ""
hardware_mode: None(0)
required_encoder: null
requires_hardware_acceleration: false
required_filters: []
tags: [字幕, 软字幕, 无损]
source_lines: SystemTemplatePresets.cs:565-580
description: "把字幕作为可开关的字幕轨封装进 MP4（mov_text）。速度极快、画质无损，而且不依赖 libass，任何 FFmpeg 构建都能用。播放器里可以自由开关字幕。"
command_args: >
  -i "{input}" -i "{subtitle}" -map 0 -map 1:0 -c copy -c:s mov_text -movflags +faststart "{output}"
parameters:
  - name: subtitle ; label: "字幕文件路径" ; kind: Text(2) ; default: ""
    description: "srt / ass 文件的完整路径。"
    min: null ; max: null ; true: "1" ; false: ""
    options: []
```

与 S43 的区别：这里是**软字幕**（`-c:s mov_text`，无需重新编码、不依赖 libass），`required_filters` 为空。

---

### [S50] 特殊处理 · 49 · `sys.keep-all-tracks`

```yaml
name: "多音轨 · 保留全部音轨与字幕"
category: 特殊处理
sort_order: 49
preset_version: 4
type: System
flags: [system]
output_extension: mkv
input_formats: [mp4, mkv, mov, avi, ts, webm, flv, m4v]
target_codec: ""
hardware_mode: None(0)
required_encoder: null
requires_hardware_acceleration: false
required_filters: []
tags: [多音轨, 归档, 无损]
source_lines: SystemTemplatePresets.cs:582-587
description: "把所有音轨和字幕轨一起封装到 MKV，不做任何重新编码，适合归档原始素材。"
command_args: >
  -i "{input}" -map 0 -c copy "{output}"
parameters: []
```

---

### [S51] 音频处理 · 50 · `sys.audio-copy-track`

```yaml
name: "音频 · 直接复制音轨（不重新编码）"
category: 音频处理
sort_order: 50
preset_version: 4
type: System
flags: [system]
output_extension: m4a
input_formats: [mp4, mkv, mov, flac, wav, ogg, m4a, aac, ts]
target_codec: ""
hardware_mode: None(0)
required_encoder: null
requires_hardware_acceleration: false
required_filters: []
tags: [音频, 无损, 快速]
source_lines: SystemTemplatePresets.cs:589-594
description: "只把音轨取出来重新封装，不做任何编码，速度和音质都是最优。"
command_args: >
  -i "{input}" -vn -map 0:a:0 -c:a copy "{output}"
parameters: []
```

> **注意声明顺序 ≠ 展示顺序**：这最后一条是「音频处理」分类，但源码把它写在「特殊处理」之后，
> 因此 `SortOrder = 50`（全部条目中最大）。列表展示时按 `CategoryRank` 先排「音频处理」（第 5 位），
> 所以它会出现在同分类其它条目之后。

### 11.1 全目录速查表（key / 名称 / 分类 / sort / 行号）

| # | key（建议） | name（逐字） | category | sort_order | source_lines |
|---|---|---|---|---|---|
| S01 | `sys.mp4-h264-auto` | MP4 · H.264（自动选择最佳编码器） | 推荐 | 0 | 102-112 |
| S02 | `sys.mp4-hevc-auto` | MP4 · H.265 / HEVC（自动选择最佳编码器） | 推荐 | 1 | 114-124 |
| S03 | `sys.audio-mp3` | 音频 · 提取为 MP3 | 推荐 | 2 | 126-133 |
| S04 | `sys.fast-remux` | 快速换容器（不改画质，秒完成） | 推荐 | 3 | 135-142 |
| S05 | `sys.transcode-mp4-x264` | 转换为 MP4（H.264 软件编码 x264) | 视频转码 | 4 | 143-165 |
| S06 | `sys.transcode-mkv-x265` | 转换为 MKV（H.265 软件编码 x265) | 视频转码 | 5 | 167-186 |
| S07 | `sys.transcode-webm-vp9` | 转换为 WebM（VP9 + Opus） | 视频转码 | 6 | 188-201 |
| S08 | `sys.transcode-av1-svt` | 转换为 AV1（SVT-AV1，体积最小) | 视频转码 | 7 | 203-224 |
| S09 | `sys.transcode-mov-prores` | 转换为 MOV（ProRes 422，剪辑友好） | 视频转码 | 8 | 226-232 |
| S10 | `sys.gif` | 转换为 GIF 动图 | 视频转码 | 9 | 234-254 |
| S11 | `sys.hw-h264-auto` | 硬件加速 · H.264（自动挑选可用编码器） | 硬件加速 | 10 | 255-263 |
| S12 | `sys.hw-hevc-auto` | 硬件加速 · H.265（自动挑选可用编码器） | 硬件加速 | 11 | 265-273 |
| S13 | `sys.hw-nvenc-h264` | NVIDIA NVENC · H.264 | 硬件加速 | 12 | 275 |
| S14 | `sys.hw-nvenc-hevc` | NVIDIA NVENC · H.265 / HEVC | 硬件加速 | 13 | 275 |
| S15 | `sys.hw-qsv-h264` | Intel Quick Sync · H.264 | 硬件加速 | 14 | 276 |
| S16 | `sys.hw-qsv-hevc` | Intel Quick Sync · H.265 / HEVC | 硬件加速 | 15 | 276 |
| S17 | `sys.hw-amf-h264` | AMD AMF · H.264 | 硬件加速 | 16 | 277 |
| S18 | `sys.hw-amf-hevc` | AMD AMF · H.265 / HEVC | 硬件加速 | 17 | 277 |
| S19 | `sys.hw-videotoolbox-h264` | Apple VideoToolbox · H.264 | 硬件加速 | 18 | 278 |
| S20 | `sys.hw-videotoolbox-hevc` | Apple VideoToolbox · H.265 / HEVC | 硬件加速 | 19 | 278 |
| S21 | `sys.hw-vaapi-h264` | Linux VA-API · H.264 | 硬件加速 | 20 | 279 |
| S22 | `sys.hw-vaapi-hevc` | Linux VA-API · H.265 / HEVC | 硬件加速 | 21 | 279 |
| S23 | `sys.compress-crf18` | 压缩 · 高画质（CRF 18) | 视频压缩 | 22 | 282-285 |
| S24 | `sys.compress-crf23` | 压缩 · 均衡（CRF 23) | 视频压缩 | 23 | 287-290 |
| S25 | `sys.compress-crf28` | 压缩 · 小体积（CRF 28) | 视频压缩 | 24 | 292-295 |
| S26 | `sys.compress-max-size` | 压缩 · 限制文件大小 | 视频压缩 | 25 | 297-323 |
| S27 | `sys.scale-4k` | 调整为 4K（3840×2160） | 分辨率与画面 | 26 | 324 |
| S28 | `sys.scale-1080p` | 调整为 1080p（1920×1080） | 分辨率与画面 | 27 | 325 |
| S29 | `sys.scale-720p` | 调整为 720p（1280×720） | 分辨率与画面 | 28 | 326 |
| S30 | `sys.scale-480p` | 调整为 480p（854×480） | 分辨率与画面 | 29 | 327 |
| S31 | `sys.scale-width` | 缩放 · 指定宽度（高度自适应） | 分辨率与画面 | 30 | 329-342 |
| S32 | `sys.crop-vertical-9x16` | 裁剪 · 竖屏 9:16（1080×1920） | 分辨率与画面 | 31 | 344-348 |
| S33 | `sys.rotate-90` | 旋转 · 顺时针 90° | 分辨率与画面 | 32 | 350-363 |
| S34 | `sys.speed-change` | 变速 · 修改播放速度 | 分辨率与画面 | 33 | 365-385 |
| S35 | `sys.deinterlace` | 去隔行 · 消除交错的横纹 | 分辨率与画面 | 34 | 387-393 |
| S36 | `sys.audio-aac-m4a` | 音频 · 提取为 AAC / M4A | 音频处理 | 35 | 394-399 |
| S37 | `sys.audio-flac` | 音频 · 提取为 FLAC（无损） | 音频处理 | 36 | 401-406 |
| S38 | `sys.audio-wav` | 音频 · 提取为 WAV（PCM 无损） | 音频处理 | 37 | 408-413 |
| S39 | `sys.audio-opus` | 音频 · 提取为 Opus | 音频处理 | 38 | 415-420 |
| S40 | `sys.audio-loudnorm` | 音频 · 音量标准化（EBU R128） | 音频处理 | 39 | 422-427 |
| S41 | `sys.remove-audio` | 视频 · 移除音轨（静音视频） | 音频处理 | 40 | 429-434 |
| S42 | `sys.watermark-image` | 画面 · 添加图片水印 | 特殊处理 | 41 | 435-467 |
| S43 | `sys.burn-subtitles` | 画面 · 烧录字幕（硬字幕） | 特殊处理 | 42 | 469-490 |
| S44 | `sys.trim-segment` | 画面 · 按时间裁剪片段 | 特殊处理 | 43 | 492-510 |
| S45 | `sys.image-sequence-jpg` | 画面 · 导出为图片序列（JPG） | 特殊处理 | 44 | 512-531 |
| S46 | `sys.thumbnail` | 画面 · 生成封面缩略图 | 特殊处理 | 45 | 533-551 |
| S47 | `sys.faststart` | 网络优化 · 仅重建索引（faststart） | 特殊处理 | 46 | 553-558 |
| S48 | `sys.fix-timestamps` | 网络优化 · 无损调整时间戳（修正音画不同步） | 特殊处理 | 47 | 560-563 |
| S49 | `sys.soft-subtitle-track` | 字幕 · 封装为可选字幕轨（不重新编码） | 特殊处理 | 48 | 565-580 |
| S50 | `sys.keep-all-tracks` | 多音轨 · 保留全部音轨与字幕 | 特殊处理 | 49 | 582-587 |
| S51 | `sys.audio-copy-track` | 音频 · 直接复制音轨（不重新编码） | 音频处理 | 50 | 589-594 |

**分类计数**：推荐 4 + 视频转码 6 + 硬件加速 12 + 视频压缩 4 + 分辨率与画面 9 + 音频处理 7 + 特殊处理 9 = **51**。

---

## 12. 生成模板用的辅助函数（逐字原文）

### 12.1 `CreateAll()` 内部的局部函数 `Add`（`SystemTemplatePresets.cs:30-70`）

```csharp
        var order = 0;
        var templates = new List<CommandTemplate>();

        void Add(
            string category,
            string name,
            string description,
            string args,
            string extension,
            string[]? inputFormats = null,
            string? targetCodec = null,
            HardwareAccelerationMode hardwareMode = HardwareAccelerationMode.None,
            string? requiredEncoder = null,
            List<TemplateParameter>? parameters = null,
            string[]? tags = null,
            bool recommended = false,
            string[]? requiredFilters = null)
        {
            templates.Add(new CommandTemplate
            {
                Name = name,
                Description = description,
                CommandArgs = args,
                Type = TemplateType.System,
                Category = category,
                SupportedInputFormats = inputFormats?.ToList() ?? [],
                OutputExtension = extension,
                TargetCodec = targetCodec ?? string.Empty,
                HardwareMode = hardwareMode,
                RequiresHardwareAcceleration = hardwareMode == HardwareAccelerationMode.Required,
                RequiredEncoder = requiredEncoder,
                Parameters = parameters ?? [],
                RequiredFilters = requiredFilters?.ToList() ?? [],
                Tags = tags?.ToList() ?? [],
                IsRecommended = recommended,
                PresetVersion = Version,
                SortOrder = order++,
                CreatedAt = DateTime.UtcNow,
                UpdatedAt = DateTime.UtcNow
            });
        }
```

**参数顺序（位置参数）**：`category, name, description, args, extension, inputFormats, targetCodec,
hardwareMode, requiredEncoder, parameters, tags, recommended, requiredFilters`。
`SortOrder` 由 `order++` 自增（从 0 开始，**按登记顺序**，与分类无关）。
`Id` 未赋值 → 走 `CommandTemplate` 的字段初始值 `ObjectId.NewObjectId()`（**每次调用 `CreateAll()` 都产生新 Id**）。

### 12.2 `AddHardwareFamily`（`SystemTemplatePresets.cs:599-643`，逐字）

```csharp
    /// <summary>生成某一硬件族的 H.264 / H.265 模板（在 CreateAll 内部作为局部函数调用）</summary>
    private static void AddHardwareFamily(
        Action<string, string, string, string, string, string[]?, string?, HardwareAccelerationMode, string?, List<TemplateParameter>?, string[]?, bool, string[]?> add,
        string displayFamily, string family, string vendorDescription)
    {
        var hardQuality = new TemplateParameter
        {
            Name = "quality",
            Label = "质量",
            Description = "留空使用该编码器的推荐默认值。",
            Kind = TemplateParameterKind.Select,
            DefaultValue = string.Empty,
            Options =
            [
                new("", "自动（推荐）"),
                new("20", "高画质"),
                new("24", "均衡"),
                new("28", "小体积"),
                new("32", "极小体积"),
            ]
        };

        var audio = new TemplateParameter
        {
            Name = "audio_bitrate",
            Label = "音频码率",
            Kind = TemplateParameterKind.Select,
            DefaultValue = "128k",
            Options = [new("96k", "96 kbps"), new("128k", "128 kbps"), new("192k", "192 kbps"), new("256k", "256 kbps")]
        };

        add("硬件加速", $"{displayFamily} · H.264",
            $"使用 {vendorDescription} 的 {displayFamily} 硬件编码 H.264，速度快、CPU 占用低。",
            "-i \"{input}\" -c:v {encoder} {quality_args} {preset_args} {hwupload_args} " +
            "-c:a {audio_encoder} -b:a {audio_bitrate} -movflags +faststart \"{output}\"",
            "mp4", null, "h264", HardwareAccelerationMode.Required, family, [hardQuality, audio],
            ["硬件加速", "h264", family], false, null);

        add("硬件加速", $"{displayFamily} · H.265 / HEVC",
            $"使用 {vendorDescription} 的 {displayFamily} 硬件编码 H.265，画质/体积比更好。",
            "-i \"{input}\" -c:v {encoder} {quality_args} {preset_args} {hwupload_args} " +
            "-c:a {audio_encoder} -b:a {audio_bitrate} -tag:v hvc1 -movflags +faststart \"{output}\"",
            "mp4", null, "hevc", HardwareAccelerationMode.Required, family, [hardQuality, audio],
            ["硬件加速", "hevc", family], false, null);
    }
```

### 12.3 `AddScaleTemplate`（`SystemTemplatePresets.cs:645-657`，逐字）

```csharp
    /// <summary>生成一个分辨率缩放模板</summary>
    private static void AddScaleTemplate(
        Action<string, string, string, string, string, string[]?, string?, HardwareAccelerationMode, string?, List<TemplateParameter>?, string[]?, bool, string[]?> add,
        string name, int width, int height)
    {
        add("分辨率与画面", name,
            $"等比缩放到不超过 {width}×{height}，必要时用黑边补齐，保证画面不变形。",
            $"-i \"{{input}}\" -vf \"scale={width}:{height}:force_original_aspect_ratio=decrease," +
            $"pad={width}:{height}:(ow-iw)/2:(oh-ih)/2,setsar=1\" " +
            "-c:v libx264 -crf 23 -preset medium -c:a aac -b:a 128k -movflags +faststart \"{output}\"",
            "mp4", null, "h264", HardwareAccelerationMode.None, null, null,
            ["缩放", $"{height}p"], false, null);
    }
```

### 12.4 两个共享参数对象 + 分类顺序 + 版本常量（`SystemTemplatePresets.cs:10-99`，逐字）

> ⚠️ 下面这段是**节选**（`⟨省略⟩` 标记处为省略内容，不是合法 C#），完整逐字原文见 §12.1–12.3 与 §11：
> 省略的两处分别是 `Add` 局部函数（12.1）与 51 条 `Add` 调用（§11）。其余部分逐字复制。

```csharp
public static class SystemTemplatePresets
{
    /// <summary>预设版本号（修改任何预设内容时递增）</summary>
    public const int Version = 4;

    /// <summary>分类展示顺序</summary>
    public static readonly string[] CategoryOrder =
    [
        "推荐",
        "视频转码",
        "硬件加速",
        "视频压缩",
        "分辨率与画面",
        "音频处理",
        "特殊处理",
    ];

    /// <summary>获取全部预设模板</summary>
    public static List<CommandTemplate> CreateAll()
    {
        var order = 0;
        var templates = new List<CommandTemplate>();

        ⟨省略：局部函数 Add，见 12.1⟩

        var qualityAuto = new TemplateParameter
        {
            Name = "quality",
            Label = "质量",
            Description = "数值越小画质越好、文件越大。留空则由当前编码器的最佳默认值决定。",
            Kind = TemplateParameterKind.Select,
            DefaultValue = string.Empty,
            Options =
            [
                new("", "自动（推荐）"),
                new("18", "高画质"),
                new("23", "均衡"),
                new("28", "小体积"),
                new("32", "极小体积"),
            ]
        };

        var audioBitrate = new TemplateParameter
        {
            Name = "audio_bitrate",
            Label = "音频码率",
            Kind = TemplateParameterKind.Select,
            DefaultValue = "128k",
            Options =
            [
                new("96k", "96 kbps"), new("128k", "128 kbps"), new("192k", "192 kbps"), new("256k", "256 kbps")
            ]
        };

        ⟨省略：51 条 Add / AddHardwareFamily / AddScaleTemplate 调用，逐字见 §11⟩

        return templates;
    }
```

（`= string.Empty` / `new(...)` 的写法说明：`TemplateParameterOption` 的 `Description` 全部为 `""`；
`options` 里的 `new("96k", "96 kbps")` 是 `(value, label)` 两参数重载。）

---

## 13. Rust 移植决策清单

### 13.1 必须 1:1 复现（否则行为可见地变化）

1. **先分词、后替换**：占位符替换发生在 token 级，路径中的空格不会破坏命令结构（§4.2/§4.3）。
2. **参数组占位符展开为多 token**：`-cq 24 -b:v 0` 必须是 2 个参数，不能挤进 1 个（§4.5）；
   回归断言见 `PipelineChecks.cs:184-186`。
3. **独占 token 的空值消失**（不产生空参数），**混写则只做文本替换 + Warning**（§4.2）。
4. **执行必须走 argv**（`Command::args`），绝不能把 `CommandLine` 交给 shell（§4.4）。
5. **`{encoder}`/`{quality_args}`/`{preset_args}`/`{hwupload_args}` 的编码器画像驱动机制**（§5.3），
   以及 `Required` 模式下「缺硬件就拒绝执行而不是换厂商」。
6. **自动注入位置规则**：`-hwaccel` 在 `-i` 前、`-vaapi_device` 在最前、`-tag:v`/全局参数在输出前（§4.6）。
7. **质量值语义**：`context.Quality ?? encoder.DefaultQuality ?? 23`，参数 `quality/crf/q` 可覆盖且回退默认（§3.5）。
8. **`output_pattern`（`_%04d`）与图片序列的成功判定**（§11 S45）。
9. **校验的 18 条检查、严重级、逐字中文消息与顺序**（§7）；`Error` 会阻断试运行。
10. **导入/导出的 JSON 键名、大小写敏感性、有损字段集、逐项跳过规则、自动改名**（§9）。
11. **同步算法**：按 `PresetVersion` 比较、保留 `Id`/`CreatedAt`、只碰 `Type == System`、幂等（§10.2）。
12. **规范化**（`NormalizeTemplate`）在所有写路径上执行（§1.6）。

### 13.2 建议修正（参考实现的问题）

| # | 问题 | 建议 |
|---|---|---|
| A | 试运行对「无 `{encoder}` 的软件模板」必然报「缺少硬件编码器」（§8.6 缺陷 A） | 把「不需要编码器画像」与「没有可用编码器」分开表示（`Option<Option<Profile>>` 或显式 enum），前者照常执行 |
| B | `transpose=1` 写死、参数 `angle` 是死参数（§11 S33） | 改成 `transpose={angle}` |
| C | 同步从不删除被移除/改名的系统模板，产生陈旧残留（§10.3） | 用稳定 `key` 匹配（而非 `Name`），并在同步末尾删除「旧版本且不在当前预设清单内」的系统模板（或标记归档） |
| D | 系统模板身份依赖 `Name`，改名会产生重复条目 | 引入 `key`（§11.0 建议的 `sys.*`）作为同步主键，`Name` 仅作展示，可随时改文案 |
| E | 导出有损（丢掉 `RequiredFilters`/`IsRecommended`/`PresetVersion`），无法无损往返（§9.5） | 扩展 schema（新增可选键，`SchemaVersion = 2`），读取时兼容 1；或用 `key` 让导入内容按需升级 |
| F | 重置系统模板会更换所有 `Id`，历史任务的模板引用悬空（§10.5） | 重置时保留 `Id`（按 key 原地重建），或把任务上的模板快照独立存档 |
| G | 无 `Step` 字段，Number 参数总是 `step="any"`（§1.2） | 若界面需要整数/步进，新增 `step`（可选）并纳入 JSON |
| H | `Validate` 用朴素 `Contains("-crf")`/`Contains("-c copy")` 判定（§7） | 建议做 token 级精确判定（`args` 分词后比对），减少误报 |
| I | 参数名与内置占位符同名（`fps`/`width`）只是 Warning，实际会静默覆盖内置值（§11 S10/S31/S45/S46） | 保留行为（内置模板依赖它），但在 UI 明确提示「该参数会覆盖同名内置值」 |
| J | 试运行忽略设置（`settings` 取出未用，缺陷 B） | 让试运行也遵循「优先硬件解码」等设置，或在结果里说明「未应用全局参数」 |
| K | `EnsureUniqueName` 用「精确字符串相等」判定（§1.7） | 视产品需求决定是否做大小写/全半角归一；若加，需与 DB 唯一约束一致 |

### 13.3 需要产品决策的点

1. **持久化选型**：LiteDB 无法在 Rust 侧读写 → 建议 SQLite（`templates` 表 + `name` UNIQUE 索引），
   或 JSON 文件 + 启动加载。无论哪种，都要保证 §10.3 的幂等同步语义。
2. **`ObjectId` 兼容**：参考实现是 12 字节随机 id。Rust 侧用 UUIDv4/UUIDv7 皆可；
   若要与 C# 版本共用数据文件（不推荐），则必须实现 `ObjectId`。
3. **`Name` 唯一性的作用域**：参考实现是**全局唯一**（系统模板与用户模板都不能同名）。
   若希望「用户模板可以叫和系统模板一样的名字」，需要改约束并调整 `EnsureUniqueName`。
4. **分类顺序**：`CategoryOrder` 是硬编码的 7 个中文分类；自建分类排在最后且按当前区域性排序。
   Rust 侧建议保留这份常量（否则界面顺序会变）。
5. **UI 的 `Kind` 扩展**：若需要「文件选择器」（参考实现里水印/字幕只是 `Text` + 手工填路径），
   需要在数据模型里新增 kind —— **会破坏与旧 JSON 的互操作**（旧数据里没有这个枚举值不是问题，
   但旧客户端读到新值会失败），建议用可选字段而非扩展枚举。
6. **时间字段**：`{date}`/`{time}`/`{datetime}` 使用**本地时间**（`DateTime.Now`），
   `CreatedAt`/`UpdatedAt` 用 UTC —— 不要统一。
7. **试运行超时**：3 分钟（素材生成 60 秒）是硬编码值；素材固定为 3 秒 640×360 25fps testsrc2 + 440Hz sine。
   若要支持「自定义测试素材」，需新增配置。

### 13.4 建议的 Rust 模块切分

```
core/template.rs        // CommandTemplate / TemplateParameter / TemplateParameterOption / 枚举 / normalize / validate
core/placeholders.rs    // 分词 + 占位符解析表 + 组装（§4）
core/encoder_catalog.rs // EncoderProfile 静态表 + 族/格式查询（§5.1）
core/presets.rs         // 51 个内置模板（§11）+ CategoryOrder + PRESET_VERSION（§12）
core/builder.rs         // build() 与 BuiltCommand（§4.2）
services/template_service.rs // CRUD / 同步 / 导入导出 / 试运行（§6/§8/§9/§10）
```

**测试建议（直接照搬参考实现的回归断言）**：

- 分词表测（§4.3 的边界表，尤其 `""`→空 token、未闭合引号、单引号无转义）。
- 51 个模板：`Validate` 无 Error；默认参数下 `build()` 结果**无残留 `{...}`**、**无空参数**；
  `hardware_mode == Required` 的模板在本机无对应硬件时返回「拒绝执行」而不是别的编码器。
- 参数组展开：「`-cq 24 -b:v 0` 变成 2 个参数」。
- 同步幂等：连续两次 `initialize_system_templates()`，第二次 `added == 0 && updated == 0`。
- 导入导出：导出 3 条 → 导入 3 条 → 再导入 3 条（自动改名，不报错）。
- 中文消息逐字比对（§7 / §8 的全部文案），确保 UI 文案不发生漂移。



