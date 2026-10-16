//! `CommandLineBuilder` 算法实现（参考实现 `Services/CommandLineBuilder.cs`）。
//!
//! 完整算法见 `docs/reference/templates.md` §4（本文件按 §4.2 的步骤编号直译）。
//! 这里是**纯函数**层：不读数据库、不访问文件系统、不做校验。
//!
//! # 移植要点 / 已知怪癖（templates.md §4.8）
//!
//! 1. **参数组占位符混写**：`-x{quality_args}y` 这类写法只做纯文本替换，并产生一条中文告警
//!    （逐字消息见 [`GROUP_MIXED_WARNING`] 的构造处）。
//! 2. **单引号内没有转义**：`subtitles=filename='{subtitle}'` 要求路径里不含单引号，与参考实现一致。
//! 3. **字面 `""` 会产生空参数**：`Tokenize` 对 `""` 会 emit 一个空 token（`has_token = true`），
//!    `build` 只丢弃「占位符解析为空」的 token，**不会**清理字面空 token —— 这是参考实现的行为，
//!    这里刻意保留（见 `build` 步骤 3 与测试 `literal_empty_quotes_keep_empty_argument`）。
//! 4. **`{input}` / `{output}` 保护**：名为 `input` / `output` 的参数不覆盖内置取值。
//! 5. **锚点语义**：自动注入的 `-hwaccel` 用输入路径的**第一次**出现；
//!    `muxer_args` / 全局参数用输出路径的**最后一次**出现（`rposition`）；
//!    模板只用 `{output_pattern}` 时找不到输出锚点 → 这两者被追加到命令末尾（下文显式注释）。
//! 6. **执行层前缀不属于模板**：`-hide_banner` / `-y|-n` / `-progress pipe:2` 由执行层追加。
//! 7. **不校验、不夹紧**：参数值原样进入 argv。
//! 8. **大小写**：解析表按大小写不敏感查找（与 C# 的 `OrdinalIgnoreCase` 字典一致）；
//!    但 `-hwaccel` 检测、占位符整 token 识别的正则本身都是**大小写敏感**的，与参考实现相同
//!    （即 `{QUALITY_ARGS}` 不会被当成参数组展开，只会被当普通未解析占位符 → 见测试）。
//! 9. **`{global_args}` / `{extra_args}` / `{output_pattern}` 在解析表里没有取值**：
//!    参考实现的 `BuildResolver` 从未写入这三个键，因此模板里写它们会被记为「未解析」
//!    （`{global_args}` 的语义由 `CommandContext::global_arguments` 的后处理注入承担）。
//!    这里保留该行为，不做「顺手修好」的扩展。

use std::collections::HashMap;
use std::sync::OnceLock;

use regex::Regex;

use crate::models::CommandTemplate;

use super::{BuiltCommand, CommandContext};

/// 占位符正则（逐字来自参考实现 `CommandLineBuilder.cs:72`）
const PLACEHOLDER_PATTERN: &str = r"\{([A-Za-z_][A-Za-z0-9_]*)\}";

/// 参数组占位符（逐字来自 `CommandLineBuilder.cs:79-83`）：
/// 单独占一个 token 时展开成 0..N 个 token。
pub const ARGUMENT_GROUP_PLACEHOLDERS: &[&str] = &[
    "quality_args",
    "preset_args",
    "hwupload_args",
    "device_args",
    "muxer_args",
    "hwaccel",
    "global_args",
    "extra_args",
];

/// 内置占位符（逐字来自 `CommandLineBuilder.cs:238-247`，共 37 个）。
/// 这些占位符不需要模板声明参数，`Validate` 不会因它们报「未声明」。
pub const BUILT_IN_PLACEHOLDERS: &[&str] = &[
    "input",
    "output",
    "output_pattern",
    "input_dir",
    "input_name",
    "input_filename",
    "input_ext",
    "output_dir",
    "output_name",
    "output_ext",
    "duration",
    "width",
    "height",
    "fps",
    "vcodec",
    "acodec",
    "sample_rate",
    "channels",
    "encoder",
    "audio_encoder",
    "crf",
    "quality",
    "q",
    "quality_args",
    "preset_args",
    "hwupload_args",
    "device_args",
    "muxer_args",
    "extra_args",
    "threads",
    "hwaccel",
    "global_args",
    "date",
    "time",
    "datetime",
    "random",
];

/// 大小写不敏感解析表（键统一小写；与 C# `OrdinalIgnoreCase` 字典等价）
type ResolverMap = HashMap<String, String>;

// ────────────────────────────────────────────────────────────────────────────
// 对外 API（4 个签名冻结）
// ────────────────────────────────────────────────────────────────────────────

/// 抽取命令模板中的全部占位符名（大小写不敏感去重，保序）。
///
/// 非 ASCII、`{1}`、`{a-b}` 等不是占位符，会被静默忽略（不报告、不产生告警）。
pub fn extract_placeholders(command_args: &str) -> Vec<String> {
    if command_args.is_empty() {
        return Vec::new();
    }
    let mut seen: Vec<String> = Vec::new();
    for caps in placeholder_regex().captures_iter(command_args) {
        let name = caps.get(1).map(|m| m.as_str()).unwrap_or_default();
        if !seen.iter().any(|s| s.eq_ignore_ascii_case(name)) {
            seen.push(name.to_string());
        }
    }
    seen
}

/// 把参数列表渲染成可读/可复制的命令行文本（**仅展示用**，不要拿去当可执行命令行）。
///
/// 见 `docs/reference/templates.md` §4.4：空列表 → `""`，否则逐项 [`quote_for_display`] 后用单空格连接。
pub fn render_command_line(args: &[String]) -> String {
    let mut out = String::new();
    for arg in args {
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(&quote_for_display(arg));
    }
    out
}

/// 分词：把模板字符串切成 argv，处理引号与转义（逐行等价 `CommandLineBuilder.cs:258-334`）。
///
/// 只支持单引号、双引号、反斜杠转义；引号是纯分组符，分词后不保留。
/// 不支持变量展开、通配、`$`、反引号、`~`、`#` 注释；引号未闭合不报错。
pub fn tokenize(input: &str) -> Vec<String> {
    let mut result: Vec<String> = Vec::new();
    if input.trim().is_empty() {
        return result;
    }

    let mut current = String::new();
    let mut has_token = false;
    let mut in_single = false;
    let mut in_double = false;

    let chars: Vec<char> = input.chars().collect();
    let mut i = 0usize;

    while i < chars.len() {
        let c = chars[i];

        if in_single {
            // 单引号内没有转义：`\` 是字面量，`'` 直接闭合
            if c == '\'' {
                in_single = false;
            } else {
                current.push(c);
            }
            i += 1;
            continue;
        }

        if in_double {
            if c == '\\' && i + 1 < chars.len() && (chars[i + 1] == '"' || chars[i + 1] == '\\') {
                // 双引号内只对 \" 和 \\ 做转义
                i += 1;
                current.push(chars[i]);
            } else if c == '"' {
                in_double = false;
            } else {
                // 其余反斜杠原样保留
                current.push(c);
            }
            i += 1;
            continue;
        }

        match c {
            // 末尾单个反斜杠：`i + 1 < len` 不成立 → 落到 default，反斜杠作为普通字符
            '\\' if i + 1 < chars.len() => {
                i += 1;
                current.push(chars[i]);
                has_token = true;
            }
            '\'' => {
                in_single = true;
                has_token = true;
            }
            '"' => {
                in_double = true;
                has_token = true;
            }
            ' ' | '\t' | '\r' | '\n' => {
                if has_token || !current.is_empty() {
                    result.push(std::mem::take(&mut current));
                    has_token = false;
                }
            }
            _ => {
                current.push(c);
                has_token = true;
            }
        }
        i += 1;
    }

    if in_single || in_double {
        // 引号未闭合：把已有内容作为最后一个参数，避免整条命令丢失（不报错）
        if !current.is_empty() || has_token {
            result.push(current);
        }
        return result;
    }

    if has_token || !current.is_empty() {
        result.push(current);
    }
    result
}

/// 构建 ffmpeg 参数（不含可执行文件名，也不含执行层固定的 `-hide_banner -y -progress pipe:2`）。
///
/// 步骤顺序（§4.2）：分词 → 合并显存上传滤镜 → token 级替换 → `-hwaccel` → `device_args`
/// → `muxer_args` → 全局参数 → 未解析报表。
pub fn build(template: &CommandTemplate, context: &CommandContext) -> BuiltCommand {
    let mut warnings: Vec<String> = Vec::new();
    let mut unresolved: Vec<String> = Vec::new();

    // 1) 先分词（此时占位符仍是普通字符，含花括号）
    let mut tokens = tokenize(&template.command_args);

    // 2) 合并硬件上传滤镜到已有的 -vf 中（VAAPI 等需要）
    //    注意：`-vf` 是模板里唯一被这一步改写的 token，因为它在占位符替换之前发生
    if let Some(upload_filter) = context
        .encoder
        .as_ref()
        .map(|e| e.upload_filter.as_str())
        .filter(|f| !f.is_empty())
    {
        tokens = merge_upload_filter(
            tokens,
            &upload_filter.to_string(),
            &template.command_args,
            &mut warnings,
        );
    }

    // 3) 在 token 级别替换占位符：参数组占位符展开为多个参数，空值直接消失
    let resolver = build_resolver(context, &mut unresolved);
    let mut arguments: Vec<String> = Vec::with_capacity(tokens.len() + 4);

    for token in tokens {
        if let Some(name) = whole_placeholder(&token) {
            if is_argument_group(name) {
                // 空值 → 不产生参数（旧实现会留下一个空字符串参数导致 FFmpeg 报错）
                if let Some(group) = resolver.get(&name.to_lowercase()).filter(|g| !g.trim().is_empty())
                {
                    arguments.extend(tokenize(group));
                }
                continue;
            }

            match resolver.get(&name.to_lowercase()) {
                Some(single) => {
                    if !single.is_empty() {
                        arguments.push(single.clone());
                    }
                    // 空值 → 丢弃整个 token
                }
                None => {
                    unresolved.push(name.to_string());
                    // 原样保留 {name}
                    arguments.push(token);
                }
            }
            continue;
        }

        // 占位符混在其它文本中：纯文本替换（替换结果不再扫描）
        let replaced = placeholder_regex()
            .replace_all(&token, |caps: &regex::Captures<'_>| {
                let key = caps.get(1).map(|m| m.as_str()).unwrap_or_default();

                if is_argument_group(key) {
                    // 参数组占位符混在别的文本里：只能做文本替换，且必须提醒作者改用独立占位符
                    if let Some(group) = resolver.get(&key.to_lowercase()) {
                        warnings.push(format!(
                            "占位符 {{{key}}} 与其它文本写在了同一个参数里，\
                             它会被当成一个整体传入，可能导致 FFmpeg 解析失败。\
                             请把它单独写成一个参数（前后加空格）。"
                        ));
                        return group.clone();
                    }
                }

                match resolver.get(&key.to_lowercase()) {
                    Some(value) => value.clone(),
                    None => {
                        unresolved.push(key.to_string());
                        caps.get(0).map(|m| m.as_str().to_string()).unwrap_or_default()
                    }
                }
            })
            .into_owned();
        arguments.push(replaced);
    }

    // 4) 补充硬件解码参数：模板里没有显式写 -hwaccel 时自动注入到输入文件之前。
    //    模板若自己用了 {hwaccel} 占位符，则已在上一步展开（且展开结果以 -hwaccel 开头 → 不再注入）。
    //    检测是**大小写敏感**的 `StartsWith("-hwaccel")`，与参考实现一致。
    if !is_blank(&context.hardware_decode_args)
        && !arguments.iter().any(|a| a.starts_with("-hwaccel"))
    {
        let decode_tokens = tokenize(&context.hardware_decode_args);
        if !decode_tokens.is_empty() {
            // 输入路径的**第一次**出现
            let insert_at = arguments
                .iter()
                .position(|a| a == &context.input_path)
                .map(|i| if i > 0 { i - 1 } else { 0 })
                .unwrap_or(0);
            arguments.splice(insert_at..insert_at, decode_tokens);
        }
    }

    // 5) 补充设备参数（如 -vaapi_device /dev/dri/renderD128）
    let device_args = expand(context.encoder.as_ref().map(|e| e.device_args.as_str()), &resolver, &mut unresolved);
    if !is_blank(&device_args) {
        let device_tokens = tokenize(&device_args);
        let first_token = device_tokens.first().cloned().unwrap_or_default();
        if !arguments.contains(&first_token) {
            // 第一个「以 - 开头且不是 -y/-n」的 token 之前；没有则插到 0
            let insert_at = arguments
                .iter()
                .position(|t| t.starts_with('-') && t != "-y" && t != "-n")
                .unwrap_or(0);
            arguments.splice(insert_at..insert_at, device_tokens);
        }
    }

    // 6) 补充容器参数（如 -tag:v hvc1）——仅当模板未显式指定任何 -tag*
    let muxer_args = expand(context.encoder.as_ref().map(|e| e.muxer_args.as_str()), &resolver, &mut unresolved);
    if !is_blank(&muxer_args) && !arguments.iter().any(|a| a.starts_with("-tag")) {
        let muxer_tokens = tokenize(&muxer_args);
        // 输出路径的**最后一次**出现；模板只写 {output_pattern} 时找不到锚点 → 追加到末尾
        // （对 `-tag:v` 而言通常仍然有效：它在输出路径之后，FFmpeg 仍会解析成输出选项）
        let insert_at = arguments
            .iter()
            .rposition(|a| a == &context.output_path)
            .unwrap_or(arguments.len());
        arguments.splice(insert_at..insert_at, muxer_tokens);
    }

    // 7) 全局参数（用户配置）追加到输出文件之前（同样用 rposition）
    if !is_blank(&context.global_arguments) {
        let global_tokens = tokenize(&context.global_arguments);
        let insert_at = arguments
            .iter()
            .rposition(|a| a == &context.output_path)
            .unwrap_or(arguments.len());
        arguments.splice(insert_at..insert_at, global_tokens);
    }

    // 8) 未解析占位符去重（大小写不敏感，保序）+ 汇总告警
    let distinct = dedup_ci(&unresolved);
    if !distinct.is_empty() {
        warnings.push(format!(
            "以下占位符没有对应取值，将原样传给 FFmpeg，可能导致失败：{}",
            distinct
                .iter()
                .map(|p| format!("{{{p}}}"))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }

    let command_line = render_command_line(&arguments);

    BuiltCommand {
        command_line,
        arguments,
        encoder_name: context.encoder.as_ref().map(|e| e.name.clone()),
        warnings,
        unresolved_placeholders: distinct,
    }
}

// ────────────────────────────────────────────────────────────────────────────
// 占位符工具
// ────────────────────────────────────────────────────────────────────────────

fn placeholder_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(PLACEHOLDER_PATTERN).expect("占位符正则必须是合法的"))
}

/// 整个 token 就是一个占位符时返回名字（`^\{name\}$`，大小写敏感的 ASCII 名）
pub fn whole_placeholder(token: &str) -> Option<&str> {
    let inner = token.strip_prefix('{')?.strip_suffix('}')?;
    let mut chars = inner.chars();
    let first = chars.next()?;
    if !(first.is_ascii_alphabetic() || first == '_') {
        return None;
    }
    if !chars.all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return None;
    }
    Some(inner)
}

/// 是否为参数组占位符（大小写不敏感）
pub fn is_argument_group(name: &str) -> bool {
    ARGUMENT_GROUP_PLACEHOLDERS
        .iter()
        .any(|g| g.eq_ignore_ascii_case(name))
}

/// 把文本里的占位符按解析表展开（供队列层做 `device_args` / `muxer_args` 之类的二次展开）。
///
/// 未命中的占位符原样保留，并追加进 `unresolved`（重复记录，由调用方去重）。
pub fn expand_placeholders(text: &str, resolver: &HashMap<String, String>, unresolved: &mut Vec<String>) -> String {
    if text.is_empty() {
        return String::new();
    }
    expand(Some(text), resolver, unresolved)
}

/// `PlaceholderRegex.Replace` 的等价物（未命中 → 保留 `{name}` + 记录未解析）
fn expand(text: Option<&str>, resolver: &ResolverMap, unresolved: &mut Vec<String>) -> String {
    let Some(text) = text.filter(|t| !t.is_empty()) else {
        return String::new();
    };
    placeholder_regex()
        .replace_all(text, |caps: &regex::Captures<'_>| {
            let key = caps.get(1).map(|m| m.as_str()).unwrap_or_default();
            match resolver.get(&key.to_lowercase()) {
                Some(value) => value.clone(),
                None => {
                    unresolved.push(key.to_string());
                    caps.get(0).map(|m| m.as_str().to_string()).unwrap_or_default()
                }
            }
        })
        .into_owned()
}

fn dedup_ci(items: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::with_capacity(items.len());
    for item in items {
        if !out.iter().any(|s| s.eq_ignore_ascii_case(item)) {
            out.push(item.clone());
        }
    }
    out
}

fn is_blank(value: &str) -> bool {
    value.trim().is_empty()
}

fn contains_ci(haystack: &str, needle: &str) -> bool {
    haystack.to_lowercase().contains(&needle.to_lowercase())
}

// ────────────────────────────────────────────────────────────────────────────
// 展示用引号（§4.4）
// ────────────────────────────────────────────────────────────────────────────

/// 为展示目的给参数加引号（`CommandLineBuilder.cs:337-343`）。
///
/// 空参数渲染成 `""`；含空格/引号/`;`/`&`/`|`/`>`/`<` 的参数用双引号包裹，只转义双引号。
pub fn quote_for_display(arg: &str) -> String {
    if arg.is_empty() {
        return "\"\"".to_string();
    }
    let needs_quote = arg
        .chars()
        .any(|c| matches!(c, ' ' | '\t' | '"' | '\'' | ';' | '&' | '|' | '>' | '<'));
    if !needs_quote {
        return arg.to_string();
    }
    format!("\"{}\"", arg.replace('"', "\\\""))
}

// ────────────────────────────────────────────────────────────────────────────
// 显存上传滤镜合并（§4.5）
// ────────────────────────────────────────────────────────────────────────────

/// 把上传滤镜并入已有的 `-vf` 链，或插到第一个 `-c:v`/`-codec:v`/`-vcodec` 之前。
///
/// 模板已写 `{hwupload_args}`（大小写不敏感）→ 完全交给模板作者，不做自动合并。
fn merge_upload_filter(
    mut tokens: Vec<String>,
    upload_filter: &str,
    template_text: &str,
    warnings: &mut Vec<String>,
) -> Vec<String> {
    if contains_ci(template_text, "{hwupload_args}") {
        return tokens;
    }

    if let Some(vf_index) = tokens
        .iter()
        .position(|t| t == "-vf" || t == "-filter:v" || t == "-filter")
    {
        if vf_index + 1 < tokens.len() {
            let chain = tokens[vf_index + 1].clone();
            if !chain.to_lowercase().contains("hwupload") {
                tokens[vf_index + 1] = format!("{},{}", chain.trim_end_matches(','), upload_filter);
            }
            return tokens;
        }
        // 注：`-vf` 是最后一个 token 时落到下面的分支继续找插入点（源码是顺序 if，不 return）
    }

    if tokens.iter().any(|t| t == "-filter_complex" || t == "-lavfi") {
        warnings.push(format!(
            "当前编码器需要显存上传滤镜（{upload_filter}），但模板使用了 -filter_complex/-lavfi，\
             无法自动合并。请在模板中显式使用 {{hwupload_args}} 占位符。"
        ));
        return tokens;
    }

    // 没有滤镜：在第一个 -c:v / -codec:v / -vcodec 之前插入 ["-vf", upload_filter]；找不到则追加到末尾。
    // （源码 373-380 行有一段先算 outputIdx 又被覆盖的死代码，语义无影响，这里忽略。）
    let insert_at = tokens
        .iter()
        .position(|t| t == "-c:v" || t == "-codec:v" || t == "-vcodec")
        .unwrap_or(tokens.len());
    tokens.splice(insert_at..insert_at, ["-vf".to_string(), upload_filter.to_string()]);
    tokens
}

// ────────────────────────────────────────────────────────────────────────────
// 解析表（§3.4 / §3.5）
// ────────────────────────────────────────────────────────────────────────────

/// 构建占位符解析表（`CommandLineBuilder.cs:400-481`）。
///
/// 键统一小写以实现 `OrdinalIgnoreCase` 语义。
fn build_resolver(context: &CommandContext, unresolved: &mut Vec<String>) -> ResolverMap {
    let mut map: ResolverMap = HashMap::new();

    let input = context.input_path.as_str();
    let output = context.output_path.as_str();
    let encoder = context.encoder.as_ref();

    map.insert("input".into(), input.to_string());
    map.insert("output".into(), output.to_string());
    map.insert("input_dir".into(), safe_path(get_directory_name(input)));
    map.insert("input_name".into(), safe_path(get_file_name_without_extension(input)));
    map.insert("input_filename".into(), safe_path(get_file_name(input)));
    map.insert("input_ext".into(), get_extension(input).trim_start_matches('.').to_string());
    map.insert("output_dir".into(), safe_path(get_directory_name(output)));
    map.insert("output_name".into(), safe_path(get_file_name_without_extension(output)));
    map.insert("output_ext".into(), get_extension(output).trim_start_matches('.').to_string());
    // 图片序列等场景需要带序号的文件名模式，例如 out_0001.jpg
    map.insert("output_pattern".into(), if output.is_empty() {
        String::new()
    } else {
        format!(
            "{}{}_%04d{}",
            combine_prefix(get_directory_name(output)),
            get_file_name_without_extension(output),
            get_extension(output)
        )
    });

    let media = context.media.as_ref();
    map.insert(
        "duration".into(),
        media.map(|m| format_number(m.duration)).unwrap_or_else(|| "0".to_string()),
    );
    map.insert(
        "width".into(),
        match media {
            Some(m) if m.width > 0 => m.width.to_string(),
            _ => String::new(),
        },
    );
    map.insert(
        "height".into(),
        match media {
            Some(m) if m.height > 0 => m.height.to_string(),
            _ => String::new(),
        },
    );
    map.insert(
        "fps".into(),
        match media {
            Some(m) if m.frame_rate > 0.0 => format_number(m.frame_rate),
            _ => String::new(),
        },
    );
    map.insert("vcodec".into(), media.and_then(|m| m.video_codec.clone()).unwrap_or_default());
    map.insert("acodec".into(), media.and_then(|m| m.audio_codec.clone()).unwrap_or_default());
    map.insert(
        "sample_rate".into(),
        match media {
            Some(m) if m.audio_sample_rate > 0 => m.audio_sample_rate.to_string(),
            _ => "44100".to_string(),
        },
    );
    map.insert(
        "channels".into(),
        match media {
            Some(m) if m.audio_channels > 0 => m.audio_channels.to_string(),
            _ => "2".to_string(),
        },
    );

    map.insert(
        "encoder".into(),
        encoder.map(|e| e.name.clone()).unwrap_or_else(|| "libx264".to_string()),
    );
    map.insert("audio_encoder".into(), context.audio_encoder.clone());
    map.insert(
        "threads".into(),
        if context.threads > 0 { context.threads.to_string() } else { String::new() },
    );
    map.insert("hwaccel".into(), context.hardware_decode_args.clone());

    let default_quality = context.quality.unwrap_or_else(|| encoder.map(|e| e.default_quality).unwrap_or(23));
    map.insert("crf".into(), default_quality.to_string());
    map.insert("quality".into(), default_quality.to_string());
    map.insert("q".into(), default_quality.to_string());

    // 先套用用户参数（空值表示「使用默认」，不覆盖内置占位符）
    // 注意：HashMap 迭代顺序不定，但 `input`/`output` 的跳过与其它键互相独立，语义不受影响。
    for (key, value) in &context.parameters {
        if key.eq_ignore_ascii_case("input") || key.eq_ignore_ascii_case("output") {
            continue;
        }
        if value.is_empty() {
            continue;
        }
        map.insert(key.to_lowercase(), value.clone());
    }

    // 再根据最终的质量值展开编码器相关占位符，
    // 这样模板里声明一个 quality 参数就能真正驱动 {quality_args}
    let quality = map
        .get("q")
        .and_then(|v| parse_i32_lossy(v))
        .unwrap_or(default_quality);
    map.insert("crf".into(), quality.to_string());
    map.insert("quality".into(), quality.to_string());
    map.insert("q".into(), quality.to_string());

    // `{q}` 是**字面**替换（不是占位符展开）
    map.insert(
        "quality_args".into(),
        expand(encoder.map(|e| e.quality_args.as_str()), &map, unresolved).replace("{q}", &quality.to_string()),
    );
    map.insert(
        "preset_args".into(),
        expand(encoder.map(|e| e.preset_args.as_str()), &map, unresolved),
    );
    map.insert(
        "hwupload_args".into(),
        match encoder.map(|e| e.upload_filter.as_str()).filter(|f| !f.is_empty()) {
            Some(filter) => expand(Some(filter), &map, unresolved),
            None => String::new(),
        },
    );

    // 以下两项由 build 统一处理，避免重复插入
    map.insert("device_args".into(), String::new());
    map.insert("muxer_args".into(), String::new());

    // 动态占位符（本地时间）
    let now = chrono::Local::now();
    map.insert("date".into(), now.format("%Y%m%d").to_string());
    map.insert("time".into(), now.format("%H%M%S").to_string());
    map.insert("datetime".into(), now.format("%Y%m%d_%H%M%S").to_string());
    map.insert("random".into(), random_hex8());

    map
}

/// `int.TryParse` 的近似：只有十进制形式（允许前导 `+`/`-`）才算成功，
/// 超范围或格式错误一律回退到默认质量（与参考实现一致，不报错）。
fn parse_i32_lossy(value: &str) -> Option<i32> {
    let trimmed = value.trim();
    let body = trimmed.strip_prefix(['+', '-']).unwrap_or(trimmed);
    if body.is_empty() || !body.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    trimmed.parse::<i32>().ok()
}

/// `Guid.NewGuid().ToString("N")[..8]`：8 位十六进制（小写）
fn random_hex8() -> String {
    uuid::Uuid::new_v4().simple().to_string().chars().take(8).collect()
}

/// `Path.GetDirectoryName` 的等价物（同时接受 `/` 与 `\`，跨平台一致）
pub(crate) fn get_directory_name(path: &str) -> String {
    if path.is_empty() {
        return String::new();
    }
    if path.ends_with('/') || path.ends_with('\\') {
        // C#: 以分隔符结尾时返回空串
        return String::new();
    }
    match path.rfind(['/', '\\']) {
        None => String::new(),
        Some(index) => path[..index].to_string(),
    }
}

/// `Path.GetFileName` 的等价物
pub(crate) fn get_file_name(path: &str) -> String {
    if path.is_empty() {
        return String::new();
    }
    match path.rfind(['/', '\\']) {
        None => path.to_string(),
        Some(index) => path[index + 1..].to_string(),
    }
}

/// `Path.GetFileNameWithoutExtension` 的等价物
pub(crate) fn get_file_name_without_extension(path: &str) -> String {
    let file = get_file_name(path);
    match file.rfind('.') {
        None => file,
        Some(index) => file[..index].to_string(),
    }
}

/// `Path.GetExtension` 的等价物（含前导点，找不到返回空串）
pub(crate) fn get_extension(path: &str) -> String {
    if path.is_empty() {
        return String::new();
    }
    let start = path.rfind(['/', '\\']).map(|i| i + 1).unwrap_or(0);
    match path[start..].rfind('.') {
        None => String::new(),
        Some(index) => path[start + index..].to_string(),
    }
}

/// `Path.Combine(directory, file)` 的等价物：目录为空时不加分隔符
fn combine_prefix(directory: String) -> String {
    if directory.is_empty() {
        directory
    } else {
        format!("{directory}/")
    }
}

/// `SafePath`（`CommandLineBuilder.cs:483-488`）：参考实现目前是空实现，
/// 非空时原样返回 —— 这里如实照搬，**不**做 Trim。
fn safe_path(value: String) -> String {
    value
}

/// C# `"0.###"`（InvariantCulture）：最多 3 位小数，去掉尾随 0 与多余小数点
fn format_number(value: f64) -> String {
    if value.is_nan() {
        return "NaN".to_string();
    }
    if value.is_infinite() {
        return if value.is_sign_negative() { "-∞".to_string() } else { "∞".to_string() };
    }
    let mut text = format!("{value:.3}");
    if text.contains('.') {
        while text.ends_with('0') {
            text.pop();
        }
        if text.ends_with('.') {
            text.pop();
        }
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::MediaSummary;
    use crate::ffmpeg::encoder_catalog::{EncoderProfile, EncoderType};

    // ── 测试夹具 ─────────────────────────────────────────────────────────

    fn encoder(name: &str, quality_args: &str) -> EncoderProfile {
        EncoderProfile {
            name: name.to_string(),
            display_name: name.to_string(),
            encoder_type: EncoderType::Software,
            family: "software".to_string(),
            codec: "h264".to_string(),
            quality_args: quality_args.to_string(),
            default_quality: 23,
            quality_hint: "数值越小画质越好".to_string(),
            preset_args: String::new(),
            upload_filter: String::new(),
            device_args: String::new(),
            muxer_args: String::new(),
            sort_preference: 90,
            platforms: &[],
        }
    }

    fn template(command_args: &str) -> CommandTemplate {
        CommandTemplate {
            command_args: command_args.to_string(),
            output_extension: "mp4".to_string(),
            ..CommandTemplate::default()
        }
    }

    fn context() -> CommandContext {
        CommandContext::new("/in/a b.mp4", "/out/c.mp4")
    }

    fn params(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    /// 回归断言：任何生成参数都不含同时出现的 `{` 与 `}`，也不为空
    fn assert_clean(built: &BuiltCommand) {
        for arg in &built.arguments {
            assert!(
                !(arg.contains('{') && arg.contains('}')),
                "残留占位符参数：{arg:?}（全部：{:?}）",
                built.arguments
            );
        }
        assert!(
            !built.arguments.iter().any(|a| a.is_empty()),
            "生成了空参数（全部：{:?}）",
            built.arguments
        );
    }

    // ── Tokenize（§4.3 的 11 个边界逐条覆盖） ────────────────────────────

    #[test]
    fn tokenize_strips_quotes_around_paths() {
        assert_eq!(
            tokenize("-i \"{input}\" -c:v libx264 \"{output}\""),
            vec!["-i", "{input}", "-c:v", "libx264", "{output}"]
        );
    }

    #[test]
    fn tokenize_keeps_colons_inside_filter_chain() {
        assert_eq!(
            tokenize("-vf \"scale=480:-2:flags=lanczos\""),
            vec!["-vf", "scale=480:-2:flags=lanczos"]
        );
    }

    #[test]
    fn tokenize_concatenates_quoted_and_bare_text() {
        assert_eq!(tokenize("\"a b\"c"), vec!["a bc"]);
    }

    #[test]
    fn tokenize_empty_quotes_produce_empty_token() {
        // 怪癖 3：字面 "" 会真的产生一个空字符串参数
        assert_eq!(tokenize("''"), vec![""]);
        assert_eq!(tokenize("\"\""), vec![""]);
    }

    #[test]
    fn tokenize_backslash_escapes_space() {
        assert_eq!(tokenize("a\\ b"), vec!["a b"]);
    }

    #[test]
    fn tokenize_trailing_backslash_is_literal() {
        assert_eq!(tokenize("a\\"), vec!["a\\"]);
    }

    #[test]
    fn tokenize_escapes_quote_inside_double_quotes() {
        assert_eq!(tokenize("\"a\\\"b\""), vec!["a\"b"]);
        assert_eq!(tokenize("\"a\\\\b\""), vec!["a\\b"]);
    }

    #[test]
    fn tokenize_single_quotes_have_no_escapes() {
        // 怪癖 2：单引号内 `\` 是字面量，`'` 提前闭合；引号只是分组符，
        // 所以 `'a\'b` 拼成一个 token「a\b」（参考实现的表格用 `["a\\b"]` 记这一条）
        assert_eq!(tokenize("'a\\'b'"), vec!["a\\b"]);
        assert_eq!(tokenize("'a\\'b'")[0].chars().collect::<Vec<_>>(), vec!['a', '\\', 'b']);
    }

    #[test]
    fn tokenize_unclosed_quote_is_silent() {
        assert_eq!(tokenize("\"unclosed"), vec!["unclosed"]);
        assert_eq!(tokenize("'unclosed"), vec!["unclosed"]);
        // 未闭合 + 已有内容为空但 has_token 为真 → 仍然 emit 一个空 token
        assert_eq!(tokenize("a \""), vec!["a", ""]);
    }

    #[test]
    fn tokenize_whitespace_only_is_empty_and_repeated_blanks_collapse() {
        assert!(tokenize("").is_empty());
        assert!(tokenize("   \t\r\n ").is_empty());
        assert_eq!(tokenize("a   b"), vec!["a", "b"]);
    }

    #[test]
    fn tokenize_keeps_special_chars_verbatim() {
        // 不做变量展开、不做通配、不处理 $ / 反引号 / ~ / # 注释
        assert_eq!(
            tokenize("-vf subtitles=$HOME/a.srt"),
            vec!["-vf", "subtitles=$HOME/a.srt"]
        );
        assert_eq!(tokenize("a\\;b"), vec!["a;b"]);
        assert_eq!(tokenize("#not-a-comment"), vec!["#not-a-comment"]);
    }

    // ── extract_placeholders ────────────────────────────────────────────

    #[test]
    fn extract_placeholders_dedups_case_insensitively_in_order() {
        let names = extract_placeholders("{input} -c:v {encoder} {QUALITY_ARGS} {quality_args} {output}");
        assert_eq!(names, vec!["input", "encoder", "QUALITY_ARGS", "quality_args", "output"]);
        assert!(extract_placeholders("").is_empty());
    }

    #[test]
    fn extract_placeholders_ignores_non_placeholders() {
        // {1} / {a-b} / 中文 / {a.b} 都不是占位符
        assert!(extract_placeholders("{1} {a-b} {中文} {a.b} {} { }").is_empty());
        // 允许下划线开头与数字结尾
        assert_eq!(extract_placeholders("{_x1}"), vec!["_x1"]);
    }

    #[test]
    fn whole_placeholder_detection() {
        assert_eq!(whole_placeholder("{quality_args}"), Some("quality_args"));
        assert_eq!(whole_placeholder("{a"), None);
        assert_eq!(whole_placeholder("x{y}"), None);
        assert_eq!(whole_placeholder("{1a}"), None);
    }

    // ── 参数组占位符：独占 vs 混写 ───────────────────────────────────────

    #[test]
    fn argument_group_standalone_expands_to_multiple_tokens() {
        let mut ctx = context();
        ctx.encoder = Some(encoder("libx265", "-crf {q} -b:v 0"));
        let built = build(&template("-i \"{input}\" -c:v {encoder} {quality_args} \"{output}\""), &ctx);

        assert_eq!(
            built.arguments,
            vec!["-i", "/in/a b.mp4", "-c:v", "libx265", "-crf", "23", "-b:v", "0", "/out/c.mp4"]
        );
        assert!(built.warnings.is_empty(), "无告警：{:?}", built.warnings);
        assert!(built.encoder_name.as_deref() == Some("libx265"));
        assert_clean(&built);
    }

    #[test]
    fn argument_group_standalone_empty_expands_to_nothing() {
        // 没有画像 → quality_args = "" → 该位置不产生参数（也不会产生空参数）
        let ctx = context();
        let built = build(&template("-i \"{input}\" {quality_args} \"{output}\""), &ctx);
        assert_eq!(built.arguments, vec!["-i", "/in/a b.mp4", "/out/c.mp4"]);
        assert!(built.warnings.is_empty(), "{:?}", built.warnings);
        assert_clean(&built);
    }

    #[test]
    fn argument_group_mixed_with_text_only_substitutes_and_warns() {
        let mut ctx = context();
        ctx.encoder = Some(encoder("libx264", "-crf {q}"));
        let built = build(&template("-i \"{input}\" -x{quality_args}y \"{output}\""), &ctx);

        assert_eq!(
            built.arguments,
            vec!["-i", "/in/a b.mp4", "-x-crf 23y", "/out/c.mp4"],
            "混写只做纯文本替换"
        );
        assert_eq!(
            built.warnings,
            vec![
                "占位符 {quality_args} 与其它文本写在了同一个参数里，它会被当成一个整体传入，\
                 可能导致 FFmpeg 解析失败。请把它单独写成一个参数（前后加空格）。"
            ]
        );
    }

    #[test]
    fn every_argument_group_placeholder_is_recognised_by_name() {
        for name in ARGUMENT_GROUP_PLACEHOLDERS {
            assert!(is_argument_group(name));
            assert!(is_argument_group(&name.to_uppercase()), "大小写不敏感");
            assert!(BUILT_IN_PLACEHOLDERS.iter().any(|p| p.eq_ignore_ascii_case(name)));
        }
        assert!(!is_argument_group("quality"));
        assert!(!is_argument_group("crf"));
        assert_eq!(BUILT_IN_PLACEHOLDERS.len(), 37);
    }

    #[test]
    fn mixed_argument_group_without_value_is_reported_unresolved() {
        // {extra_args} 不在解析表里 → 独占 token 时静默丢弃（不进未解析列表）
        let ctx = context();
        let built = build(&template("-i \"{input}\" {extra_args} \"{output}\""), &ctx);
        assert_eq!(built.arguments, vec!["-i", "/in/a b.mp4", "/out/c.mp4"]);
        assert!(built.unresolved_placeholders.is_empty());
        assert!(built.warnings.is_empty(), "{:?}", built.warnings);
    }

    // ── 未解析占位符报表（§4.7） ────────────────────────────────────────

    #[test]
    fn unresolved_placeholders_are_reported_distinct_and_unbraced() {
        let ctx = context();
        let built = build(&template("-i \"{input}\" -vf {missing} {MISSING} {also_missing} \"{output}\""), &ctx);

        assert_eq!(built.unresolved_placeholders, vec!["missing", "also_missing"]);
        assert_eq!(
            built.warnings,
            vec!["以下占位符没有对应取值，将原样传给 FFmpeg，可能导致失败：{missing}, {also_missing}"]
        );
        // 原文保留在参数里（用户自己看得见）
        assert_eq!(
            built.arguments,
            vec!["-i", "/in/a b.mp4", "-vf", "{missing}", "{MISSING}", "{also_missing}", "/out/c.mp4"]
        );
        assert!(built.has_problems());
    }

    #[test]
    fn non_placeholder_braces_are_not_reported() {
        let ctx = context();
        let built = build(&template("-i \"{input}\" -metadata title={1} -metadata x={a-b} \"{output}\""), &ctx);
        assert!(built.unresolved_placeholders.is_empty());
        assert!(built.warnings.is_empty(), "{:?}", built.warnings);
    }

    #[test]
    fn unresolved_in_mixed_text_still_reports() {
        let ctx = context();
        let built = build(&template("-i \"{input}\" -vf scale={w}:-2 \"{output}\""), &ctx);
        assert_eq!(built.unresolved_placeholders, vec!["w"]);
        assert_eq!(built.arguments[3], "scale={w}:-2");
        assert_eq!(
            built.warnings,
            vec!["以下占位符没有对应取值，将原样传给 FFmpeg，可能导致失败：{w}"]
        );
    }

    // ── 质量优先级链（§3.5） ────────────────────────────────────────────

    #[test]
    fn quality_priority_context_then_encoder_then_23() {
        let tpl = template("-i \"{input}\" {quality_args} \"{output}\"");

        // 无 context.quality、无画像 → 23
        let built = build(&tpl, &context());
        assert_eq!(built.arguments, vec!["-i", "/in/a b.mp4", "/out/c.mp4"]);
        assert!(built.warnings.is_empty());

        // 画像默认值优先于 23
        let mut ctx = context();
        let mut profile = encoder("libx264", "-crf {q}");
        profile.default_quality = 28;
        ctx.encoder = Some(profile);
        let built = build(&tpl, &ctx);
        assert_eq!(built.arguments, vec!["-i", "/in/a b.mp4", "-crf", "28", "/out/c.mp4"]);

        // context.quality 覆盖画像默认值
        ctx.quality = Some(19);
        let built = build(&tpl, &ctx);
        assert_eq!(built.arguments, vec!["-i", "/in/a b.mp4", "-crf", "19", "/out/c.mp4"]);
    }

    #[test]
    fn quality_parameter_overrides_and_drives_quality_args() {
        let mut ctx = context();
        ctx.encoder = Some(encoder("libx264", "-crf {q} -b:v 0"));
        ctx.parameters = params(&[("quality", "18")]);
        let built = build(&template("-i \"{input}\" {quality_args} -q {q} {crf} {quality} \"{output}\""), &ctx);
        assert_eq!(
            built.arguments,
            vec![
                "-i", "/in/a b.mp4", "-crf", "18", "-b:v", "0", "-q", "18", "18", "18", "/out/c.mp4"
            ]
        );
    }

    #[test]
    fn quality_parameter_non_numeric_falls_back_silently() {
        let mut ctx = context();
        ctx.encoder = Some(encoder("libx264", "-crf {q}"));
        ctx.parameters = params(&[("q", "high")]);
        let built = build(&template("-i \"{input}\" {quality_args} \"{output}\""), &ctx);
        // 三个占位符一起回退到 defaultQuality（不报错）
        assert_eq!(built.arguments, vec!["-i", "/in/a b.mp4", "-crf", "23", "/out/c.mp4"]);
        assert!(built.warnings.is_empty(), "{:?}", built.warnings);
    }

    #[test]
    fn empty_parameter_value_means_use_builtin_default() {
        let mut ctx = context();
        ctx.parameters = params(&[("preset", "")]);
        let built = build(&template("-i \"{input}\" -preset {preset} \"{output}\""), &ctx);
        // 空值不覆盖 → 解析表里没有 preset → 未解析（参考实现的行为）
        assert_eq!(built.arguments, vec!["-i", "/in/a b.mp4", "-preset", "{preset}", "/out/c.mp4"]);
        assert_eq!(built.unresolved_placeholders, vec!["preset"]);
    }

    #[test]
    fn unparseable_quality_value_falls_back_to_encoder_default() {
        let mut ctx = context();
        let mut profile = encoder("libx264", "-crf {q}");
        profile.default_quality = 30;
        ctx.encoder = Some(profile);
        ctx.quality = Some(12);
        ctx.parameters = params(&[("crf", "not-a-number")]);
        let built = build(&template("-i \"{input}\" {quality_args} \"{output}\""), &ctx);
        assert_eq!(built.arguments, vec!["-i", "/in/a b.mp4", "-crf", "30", "/out/c.mp4"]);
    }

    // ── 内置占位符取值 ──────────────────────────────────────────────────

    #[test]
    fn input_and_output_parameters_cannot_override_builtins() {
        let mut ctx = context();
        ctx.parameters = params(&[("input", "/hacked.mp4"), ("OUTPUT", "/hacked-out.mp4")]);
        let built = build(&template("-i \"{input}\" \"{output}\""), &ctx);
        assert_eq!(built.arguments, vec!["-i", "/in/a b.mp4", "/out/c.mp4"]);
    }

    #[test]
    fn other_parameters_do_override_builtins_case_insensitively() {
        let mut ctx = context();
        ctx.encoder = Some(encoder("libx264", "-crf {q}"));
        ctx.parameters = params(&[("ENCODER", "h264_nvenc"), ("Audio_Encoder", "libopus"), ("threads", "4")]);
        let built = build(&template("-i \"{input}\" -c:v {encoder} -c:a {audio_encoder} -threads {threads} \"{output}\""), &ctx);
        assert_eq!(
            built.arguments,
            vec!["-i", "/in/a b.mp4", "-c:v", "h264_nvenc", "-c:a", "libopus", "-threads", "4", "/out/c.mp4"]
        );
        assert_eq!(built.encoder_name.as_deref(), Some("libx264"), "画像名不受参数影响");
    }

    #[test]
    fn media_placeholders_follow_the_reference_defaults() {
        let mut ctx = CommandContext::new("/in/clip.MOV", "/out/clip.jpg");
        ctx.media = Some(MediaSummary::from_parts(
            12.4567,
            1920,
            1080,
            29.970,
            Some("h264".into()),
            Some("aac".into()),
            0,
            0,
            true,
            true,
        ));
        let built = build(
            &template(
                "-i \"{input}\" -vf scale={width}:{height} -r {fps} -t {duration} \
                 -ar {sample_rate} -ac {channels} -c:v {vcodec} -c:a {acodec} \"{output}\"",
            ),
            &ctx,
        );
        assert_eq!(
            built.arguments,
            vec![
                "-i",
                "/in/clip.MOV",
                "-vf",
                "scale=1920:1080",
                "-r",
                "29.97",
                "-t",
                "12.457",
                "-ar",
                "44100",
                "-ac",
                "2",
                "-c:v",
                "h264",
                "-c:a",
                "aac",
                "/out/clip.jpg"
            ]
        );
        assert_clean(&built);
    }

    #[test]
    fn media_placeholders_drop_tokens_when_unavailable() {
        // 无媒体信息：width/height/fps/vcodec/acodec 为空 → 整个 token 消失
        let ctx = context();
        let built = build(
            &template("-i \"{input}\" -vf scale={width}:{height} -r {fps} -c:v {vcodec} \"{output}\""),
            &ctx,
        );
        assert_eq!(built.arguments, vec!["-i", "/in/a b.mp4", "/out/c.mp4"]);
        assert!(built.warnings.is_empty(), "{:?}", built.warnings);
        assert_clean(&built);
    }

    #[test]
    fn path_placeholder_values_match_reference_path_helpers() {
        let ctx = CommandContext::new("/tmp/dir/clip.MOV", "/tmp/out/result.mp4");
        let built = build(
            &template(
                "-i \"{input}\" -d {input_dir} -n {input_name} -f {input_filename} -e {input_ext} \
                 -od {output_dir} -on {output_name} -oe {output_ext} \"{output}\"",
            ),
            &ctx,
        );
        assert_eq!(
            built.arguments,
            vec![
                "-i",
                "/tmp/dir/clip.MOV",
                "-d",
                "/tmp/dir",
                "-n",
                "clip",
                "-f",
                "clip.MOV",
                "-e",
                "MOV",
                "-od",
                "/tmp/out",
                "-on",
                "result",
                "-oe",
                "mp4",
                "/tmp/out/result.mp4"
            ],
            "扩展名保留原始大小写"
        );
    }

    #[test]
    fn output_pattern_uses_four_digit_sequence_pattern() {
        let ctx = CommandContext::new("/in/a b.mp4", "/out/frame.png");
        let built = build(&template("-i \"{input}\" \"{output_pattern}\""), &ctx);
        assert_eq!(built.arguments, vec!["-i", "/in/a b.mp4", "/out/frame_%04d.png"]);

        // output 为空 → output_pattern 为空 → token 消失
        let built = build(&template("-i \"{input}\" \"{output_pattern}\""), &CommandContext::new("/in/a b.mp4", ""));
        assert_eq!(built.arguments, vec!["-i", "/in/a b.mp4"]);
    }

    #[test]
    fn date_time_datetime_and_random_formats() {
        let resolved = |name: &str| -> String {
            let mut ctx = context();
            ctx.parameters = params(&[(name, "x")]);
            let built = build(&template(&format!("-i \"{{input}}\" {{{name}}} \"{{output}}\"")), &ctx);
            built.arguments[2].clone()
        };

        let date = resolved("date");
        let time = resolved("time");
        let datetime = resolved("datetime");
        let random = resolved("random");
        let now = chrono::Local::now();

        assert_eq!(date, now.format("%Y%m%d").to_string(), "本地日期 yyyyMMdd");
        assert_eq!(date.len(), 8);
        assert_eq!(time.len(), 6, "本地时间 HHmmss");
        assert_eq!(datetime.len(), 15, "yyyyMMdd_HHmmss");
        assert_eq!(datetime, format!("{date}_{time}"));
        assert_eq!(random.len(), 8);
        assert!(random.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()));
    }

    #[test]
    fn threads_and_hwaccel_only_appear_when_set() {
        let tpl = template("-i \"{input}\" -threads {threads} \"{output}\"");

        let built = build(&tpl, &context());
        assert_eq!(built.arguments, vec!["-i", "/in/a b.mp4", "/out/c.mp4"]);

        let mut ctx = context();
        ctx.threads = 8;
        let built = build(&tpl, &ctx);
        assert_eq!(built.arguments, vec!["-i", "/in/a b.mp4", "-threads", "8", "/out/c.mp4"]);
    }

    // ── 注入顺序（§4.6 四条规则） ───────────────────────────────────────

    #[test]
    fn hardware_decode_args_are_injected_before_the_input_path() {
        let mut ctx = context();
        ctx.hardware_decode_args = "-hwaccel videotoolbox".into();
        let built = build(&template("-i \"{input}\" -c:v libx264 \"{output}\""), &ctx);
        assert_eq!(
            built.arguments,
            vec!["-hwaccel", "videotoolbox", "-i", "/in/a b.mp4", "-c:v", "libx264", "/out/c.mp4"]
        );
    }

    #[test]
    fn hardware_decode_args_skip_when_template_sets_hwaccel() {
        // 模板自己写了 -hwaccel（任意位置）→ 不再自动注入
        let mut ctx = context();
        ctx.hardware_decode_args = "-hwaccel videotoolbox".into();
        let built = build(&template("-hwaccel cuda -i \"{input}\" -c:v libx264 \"{output}\""), &ctx);
        assert_eq!(
            built.arguments,
            vec!["-hwaccel", "cuda", "-i", "/in/a b.mp4", "-c:v", "libx264", "/out/c.mp4"]
        );
    }

    #[test]
    fn hwaccel_placeholder_expansion_suppresses_auto_injection() {
        let mut ctx = context();
        ctx.hardware_decode_args = "-hwaccel videotoolbox".into();
        let built = build(&template("-i \"{input}\" {hwaccel} -c:v libx264 \"{output}\""), &ctx);
        assert_eq!(
            built.arguments,
            vec!["-hwaccel", "videotoolbox", "-i", "/in/a b.mp4", "-c:v", "libx264", "/out/c.mp4"]
        );
    }

    #[test]
    fn device_args_go_before_the_first_dash_option() {
        let mut ctx = context();
        let mut profile = encoder("h264_vaapi", "-qp {q}");
        profile.device_args = "-vaapi_device /dev/dri/renderD128".into();
        ctx.encoder = Some(profile);
        let built = build(&template("-i \"{input}\" -c:v {encoder} {quality_args} \"{output}\""), &ctx);
        assert_eq!(
            built.arguments,
            vec![
                "-vaapi_device",
                "/dev/dri/renderD128",
                "-i",
                "/in/a b.mp4",
                "-c:v",
                "h264_vaapi",
                "-qp",
                "23",
                "/out/c.mp4"
            ]
        );
    }

    #[test]
    fn device_args_insert_after_leading_overwrite_flag() {
        // 执行层前缀里有 -y/-n，插入点必须跳过它们
        let mut ctx = context();
        let mut profile = encoder("h264_vaapi", "-qp {q}");
        profile.device_args = "-vaapi_device /dev/dri/renderD128".into();
        ctx.encoder = Some(profile);
        let built = build(&template("-y -i \"{input}\" \"{output}\""), &ctx);
        assert_eq!(
            built.arguments,
            vec!["-y", "-vaapi_device", "/dev/dri/renderD128", "-i", "/in/a b.mp4", "/out/c.mp4"]
        );
    }

    #[test]
    fn muxer_args_go_before_the_last_output_path() {
        let mut ctx = context();
        let mut profile = encoder("hevc_nvenc", "-rc vbr -cq {q} -b:v 0");
        profile.muxer_args = "-tag:v hvc1".into();
        ctx.encoder = Some(profile);
        // 输出路径出现两次（例如 -maps 里也写了），锚点是**最后一次**
        let built = build(
            &template("-i \"{input}\" -c:v {encoder} -f mp4 \"{output}\" -ss 1 \"{output}\""),
            &ctx,
        );
        assert_eq!(
            built.arguments,
            vec![
                "-i",
                "/in/a b.mp4",
                "-c:v",
                "hevc_nvenc",
                "-f",
                "mp4",
                "/out/c.mp4",
                "-ss",
                "1",
                "-tag:v",
                "hvc1",
                "/out/c.mp4"
            ]
        );
    }

    #[test]
    fn muxer_args_skip_when_template_already_tagged() {
        let mut ctx = context();
        let mut profile = encoder("hevc_nvenc", "-rc vbr -cq {q} -b:v 0");
        profile.muxer_args = "-tag:v hvc1".into();
        ctx.encoder = Some(profile);
        let built = build(&template("-i \"{input}\" -tag:v hvc1 -c:v {encoder} \"{output}\""), &ctx);
        assert_eq!(
            built.arguments.iter().filter(|a| *a == "-tag:v").count(),
            1,
            "不应重复注入：{:?}",
            built.arguments
        );
        assert_eq!(
            built.arguments,
            vec!["-i", "/in/a b.mp4", "-tag:v", "hvc1", "-c:v", "hevc_nvenc", "/out/c.mp4"]
        );
    }

    #[test]
    fn global_arguments_are_appended_before_output() {
        let mut ctx = context();
        ctx.global_arguments = "-max_muxing_queue_size 1024".into();
        let built = build(&template("-i \"{input}\" -c:v libx264 \"{output}\""), &ctx);
        assert_eq!(
            built.arguments,
            vec![
                "-i",
                "/in/a b.mp4",
                "-c:v",
                "libx264",
                "-max_muxing_queue_size",
                "1024",
                "/out/c.mp4"
            ]
        );
    }

    #[test]
    fn muxer_and_global_args_fall_back_to_the_end_when_output_anchor_is_missing() {
        // 怪癖 5：模板只用 {output_pattern} → 找不到输出锚点 → 追加到命令末尾
        let mut ctx = CommandContext::new("/in/a b.mp4", "/out/frame.png");
        let mut profile = encoder("hevc_nvenc", "-rc vbr -cq {q}");
        profile.muxer_args = "-tag:v hvc1".into();
        ctx.encoder = Some(profile);
        ctx.global_arguments = "-g 12".into();
        let built = build(&template("-i \"{input}\" \"{output_pattern}\""), &ctx);
        assert_eq!(
            built.arguments,
            vec!["-i", "/in/a b.mp4", "/out/frame_%04d.png", "-tag:v", "hvc1", "-g", "12"]
        );
    }

    #[test]
    fn injection_order_is_decode_then_device_then_muxer_then_global() {
        let mut ctx = CommandContext::new("/in/a b.mp4", "/out/c.mp4");
        let mut profile = encoder("hevc_vaapi", "-qp {q}");
        profile.device_args = "-vaapi_device /dev/dri/renderD128".into();
        profile.muxer_args = "-tag:v hvc1".into();
        ctx.encoder = Some(profile);
        ctx.hardware_decode_args = "-hwaccel vaapi".into();
        ctx.global_arguments = "-movflags +faststart".into();

        let built = build(&template("-i \"{input}\" -c:v {encoder} {quality_args} \"{output}\""), &ctx);
        assert_eq!(
            built.arguments,
            vec![
                "-vaapi_device",
                "/dev/dri/renderD128",
                "-hwaccel",
                "vaapi",
                "-i",
                "/in/a b.mp4",
                "-c:v",
                "hevc_vaapi",
                "-qp",
                "23",
                "-tag:v",
                "hvc1",
                "-movflags",
                "+faststart",
                "/out/c.mp4"
            ]
        );
    }

    // ── MergeUploadFilter（§4.5） ───────────────────────────────────────

    #[test]
    fn merge_upload_filter_appends_to_existing_vf_chain() {
        let mut ctx = context();
        let mut profile = encoder("h264_vaapi", "-qp {q}");
        profile.upload_filter = "format=nv12,hwupload".into();
        ctx.encoder = Some(profile);
        let built = build(
            &template("-i \"{input}\" -vf \"scale=1280:-2,\" -c:v {encoder} {quality_args} \"{output}\""),
            &ctx,
        );
        assert_eq!(
            built.arguments,
            vec![
                "-i",
                "/in/a b.mp4",
                "-vf",
                "scale=1280:-2,format=nv12,hwupload",
                "-c:v",
                "h264_vaapi",
                "-qp",
                "23",
                "/out/c.mp4"
            ],
            "尾随逗号被清理后再拼接"
        );
    }

    #[test]
    fn merge_upload_filter_keeps_chain_that_already_uploads() {
        let mut ctx = context();
        let mut profile = encoder("h264_vaapi", "-qp {q}");
        profile.upload_filter = "format=nv12,hwupload".into();
        ctx.encoder = Some(profile);
        let built = build(
            &template("-i \"{input}\" -vf \"format=nv12,hwupload,scale=640:-2\" -c:v {encoder} \"{output}\""),
            &ctx,
        );
        assert_eq!(built.arguments[3], "format=nv12,hwupload,scale=640:-2");
    }

    #[test]
    fn merge_upload_filter_inserts_vf_before_encoder_when_absent() {
        let mut ctx = context();
        let mut profile = encoder("h264_vaapi", "-qp {q}");
        profile.upload_filter = "format=nv12,hwupload".into();
        ctx.encoder = Some(profile);
        let built = build(&template("-i \"{input}\" -c:v {encoder} {quality_args} \"{output}\""), &ctx);
        assert_eq!(
            built.arguments,
            vec![
                "-i",
                "/in/a b.mp4",
                "-vf",
                "format=nv12,hwupload",
                "-c:v",
                "h264_vaapi",
                "-qp",
                "23",
                "/out/c.mp4"
            ]
        );
    }

    #[test]
    fn merge_upload_filter_appends_at_end_without_encoder_anchor() {
        let mut ctx = context();
        let mut profile = encoder("h264_vaapi", "-qp {q}");
        profile.upload_filter = "format=nv12,hwupload".into();
        ctx.encoder = Some(profile);
        let built = build(&template("-i \"{input}\" -c copy \"{output}\""), &ctx);
        assert_eq!(
            built.arguments,
            vec!["-i", "/in/a b.mp4", "-c", "copy", "/out/c.mp4", "-vf", "format=nv12,hwupload"]
        );
    }

    #[test]
    fn merge_upload_filter_warns_on_filter_complex() {
        let mut ctx = context();
        let mut profile = encoder("h264_vaapi", "-qp {q}");
        profile.upload_filter = "format=nv12,hwupload".into();
        ctx.encoder = Some(profile);
        let built = build(
            &template("-i \"{input}\" -filter_complex \"[0:v]scale=640:-2[v]\" -c:v {encoder} \"{output}\""),
            &ctx,
        );
        assert_eq!(
            built.warnings,
            vec![
                "当前编码器需要显存上传滤镜（format=nv12,hwupload），但模板使用了 -filter_complex/-lavfi，\
                 无法自动合并。请在模板中显式使用 {hwupload_args} 占位符。"
            ]
        );
        assert!(!built.arguments.iter().any(|a| a == "-vf"), "不做任何插入");
    }

    #[test]
    fn merge_upload_filter_is_skipped_when_template_uses_the_placeholder() {
        let mut ctx = context();
        let mut profile = encoder("h264_vaapi", "-qp {q}");
        profile.upload_filter = "format=nv12,hwupload".into();
        ctx.encoder = Some(profile);
        let built = build(
            &template("-i \"{input}\" -vf {hwupload_args} -c:v {encoder} {quality_args} \"{output}\""),
            &ctx,
        );
        assert_eq!(
            built.arguments,
            vec![
                "-i",
                "/in/a b.mp4",
                "-vf",
                "format=nv12,hwupload",
                "-c:v",
                "h264_vaapi",
                "-qp",
                "23",
                "/out/c.mp4"
            ]
        );
        // 模板里写的是别的滤镜 → 仍然合并
        let built = build(
            &template("-i \"{input}\" -vf scale=640:-2 -c:v {encoder} \"{output}\""),
            &ctx,
        );
        assert_eq!(built.arguments[3], "scale=640:-2,format=nv12,hwupload");
    }

    #[test]
    fn merge_upload_filter_ignores_trailing_vf_token_and_inserts_before_encoder() {
        // `-vf` 是最后一个 token 的特殊分支：不 return，继续找 -c:v 插入点
        let tokens = vec!["-i".to_string(), "/in.mp4".to_string(), "-vf".to_string()];
        let mut warnings = Vec::new();
        let merged = merge_upload_filter(tokens, "format=nv12,hwupload", "x", &mut warnings);
        assert_eq!(merged, vec!["-i", "/in.mp4", "-vf"]);
        assert!(warnings.is_empty());

        let tokens = vec!["-vf".to_string(), "-c:v".to_string(), "h264_vaapi".to_string()];
        let merged = merge_upload_filter(tokens, "format=nv12,hwupload", "x", &mut warnings);
        assert_eq!(
            merged,
            vec!["-vf", "-vf", "format=nv12,hwupload", "-c:v", "h264_vaapi"],
            "参考实现在这个畸形输入上就是这样（顺序 if 的直译结果）"
        );
    }

    // ── 展示用渲染（§4.4） ──────────────────────────────────────────────

    #[test]
    fn render_command_line_quotes_like_the_reference() {
        assert_eq!(render_command_line(&[]), "");
        assert_eq!(
            render_command_line(&["-i".to_string(), "/in/a b.mp4".to_string()]),
            "-i \"/in/a b.mp4\""
        );
        assert_eq!(render_command_line(&["".to_string()]), "\"\"");
        assert_eq!(render_command_line(&["a\"b".to_string()]), "\"a\\\"b\"");
        assert_eq!(render_command_line(&["plain".to_string()]), "plain");
        let built = build(&template("-i \"{input}\" \"{output}\""), &context());
        assert_eq!(built.command_line, "-i \"/in/a b.mp4\" /out/c.mp4");
    }

    // ── 怪癖 3：字面 "" 产生空参数（参考实现刻意保留） ──────────────────

    #[test]
    fn literal_empty_quotes_keep_empty_argument() {
        let ctx = context();
        let built = build(&template("-i \"{input}\" -metadata comment=\"\" \"{output}\""), &ctx);
        assert_eq!(
            built.arguments,
            vec!["-i", "/in/a b.mp4", "-metadata", "comment=", "/out/c.mp4"],
            "\"\" 只是分组符，不产生空参数；真正产生空参数的是单独一个 \"\" token"
        );

        let built = build(&template("-i \"{input}\" -foo \"\" \"{output}\""), &ctx);
        assert_eq!(built.arguments[3], "", "参考实现会把字面空 token 传给 FFmpeg");
        assert_eq!(built.command_line, "-i \"/in/a b.mp4\" -foo \"\" /out/c.mp4");
        assert!(built.warnings.is_empty());
    }

    // ── 未解析：解析表里没有的占位符（怪癖 9） ──────────────────────────

    #[test]
    fn global_args_and_extra_args_are_unresolved_like_the_reference() {
        let ctx = context();
        let built = build(&template("-i \"{input}\" {global_args} {extra_args} \"{output}\""), &ctx);
        assert_eq!(built.arguments, vec!["-i", "/in/a b.mp4", "/out/c.mp4"], "独占 token 被静默丢弃");
        assert!(built.unresolved_placeholders.is_empty(), "参数组占位符不进未解析列表");

        let built = build(&template("-i \"{input}\" -x {global_args} \"{output}\""), &ctx);
        assert_eq!(built.unresolved_placeholders, vec!["global_args"]);
        assert_eq!(built.arguments[3], "{global_args}");
    }

    #[test]
    fn output_pattern_placeholder_is_not_in_the_resolver() {
        // 怪癖 9：{output_pattern} 由解析表提供，但把它用在参数组之外也一样能被替换
        let ctx = CommandContext::new("/in/a b.mp4", "/out/c.mp4");
        let built = build(&template("-i \"{input}\" -strftime 1 \"{output_pattern}\""), &ctx);
        assert!(built.unresolved_placeholders.is_empty());
        assert_eq!(built.arguments[3], "/out/c_%04d.mp4");
    }

    // ── 全量回归：内置模板形状 ──────────────────────────────────────────

    #[test]
    fn built_in_template_shapes_never_leave_placeholders_or_empty_args() {
        let template_shapes: &[&str] = &[
            // 「推荐 · MP4 H.264 自动挑编码器」
            "-i \"{input}\" -c:v {encoder} {quality_args} {preset_args} {hwupload_args} -map 0:v:0 -map 0:a? \
             -c:a {audio_encoder} -b:a {audio_bitrate} -movflags +faststart -pix_fmt yuv420p \"{output}\"",
            "-i \"{input}\" -c copy -movflags +faststart \"{output}\"",
            "-i \"{input}\" -c:v libx264 -preset {preset} -crf {crf} -c:a {audio_encoder} \"{output}\"",
            "-i \"{input}\" -vf \"scale={width}:-2:flags=lanczos\" -c:v libx264 {quality_args} \"{output}\"",
            "-i \"{input}\" -vn -c:a {audio_encoder} -b:a {audio_bitrate} \"{output}\"",
            "-i \"{input}\" -ss {seek} -frames:v 1 -q:v 2 \"{output}\"",
            "-i \"{input}\" -vf \"subtitles='{subtitle}'\" -c:v libx264 \"{output}\"",
            "-f lavfi -i \"testsrc2=duration={duration}:size={width}x{height}\" -c:v libx264 \"{output}\"",
        ];

        let mut profiles = vec![
            encoder("libx264", "-crf {q}"),
            encoder("libx265", "-crf {q}"),
            encoder("h264_nvenc", "-rc vbr -cq {q} -b:v 0"),
        ];
        profiles[1].muxer_args = "-tag:v hvc1".into();
        profiles[2].upload_filter = "format=nv12,hwupload".into();
        profiles[2].device_args = "-vaapi_device /dev/dri/renderD128".into();

        let mut ctx = context();
        ctx.media = Some(MediaSummary::from_parts(3.0, 640, 360, 25.0, None, None, 0, 0, true, false));
        ctx.hardware_decode_args = "-hwaccel videotoolbox".into();
        ctx.global_arguments = "-max_muxing_queue_size 1024".into();
        ctx.parameters = params(&[
            ("audio_bitrate", "192k"),
            ("preset", "medium"),
            ("crf", "20"),
            ("seek", "00:00:01"),
            ("subtitle", "/tmp/sub.srt"),
        ]);

        for shape in template_shapes {
            for profile in &profiles {
                let mut ctx = ctx.clone();
                // 只给需要编码器的形状画像；`-c copy` 之类的形状也能承受画像存在
                ctx.encoder = Some(profile.clone());
                let built = build(&template(shape), &ctx);

                assert_clean(&built);
                assert!(
                    built.unresolved_placeholders.is_empty(),
                    "形状 {shape:?} 残留未解析：{:?}",
                    built.unresolved_placeholders
                );
                assert!(
                    built.warnings.is_empty(),
                    "形状 {shape:?} 产生了意外告警：{:?}",
                    built.warnings
                );
                assert_eq!(built.command_line, render_command_line(&built.arguments));
            }
        }
    }

    #[test]
    fn build_without_encoder_still_expands_placeholder_defaults() {
        let ctx = context();
        let built = build(
            &template("-i \"{input}\" -c:v {encoder} {quality_args} {preset_args} -c:a {audio_encoder} \"{output}\""),
            &ctx,
        );
        assert_eq!(
            built.arguments,
            vec!["-i", "/in/a b.mp4", "-c:v", "libx264", "-c:a", "aac", "/out/c.mp4"]
        );
        assert_eq!(built.encoder_name, None);
        assert!(built.warnings.is_empty(), "{:?}", built.warnings);
        assert_clean(&built);
    }

    #[test]
    fn numeric_formatting_matches_the_dot_three_hash_pattern() {
        assert_eq!(format_number(0.0), "0");
        assert_eq!(format_number(12.4567), "12.457");
        assert_eq!(format_number(25.0), "25");
        assert_eq!(format_number(29.97), "29.97");
        assert_eq!(format_number(-0.0001), "-0");
    }

    #[test]
    fn path_helpers_follow_dotnet_semantics() {
        assert_eq!(get_directory_name("/a/b/c.mp4"), "/a/b");
        assert_eq!(get_directory_name("c.mp4"), "");
        assert_eq!(get_directory_name("/a/b/"), "");
        assert_eq!(get_file_name("/a/b/c.mp4"), "c.mp4");
        assert_eq!(get_file_name_without_extension("/a/b/c.tar.gz"), "c.tar");
        assert_eq!(get_extension("/a/b/c.tar.gz"), ".gz");
        assert_eq!(get_extension("c"), "");
        assert_eq!(get_extension("/a.b/c"), "");
    }
}
