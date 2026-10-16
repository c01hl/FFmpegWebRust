//! FFmpeg 子系统自检程序（对**真实** ffmpeg 跑一遍完整链路）。
//!
//! ```bash
//! cargo run --bin ffmpeg_selftest
//! ```
//!
//! 覆盖：路径定位 → 版本与能力探测 → 用本服务的 `run()` 生成测试素材 → ffprobe 媒体探测 →
//! 带进度事件的真实转换 → 优雅取消 / `stop_task` → 硬件编码器检测报告。
//!
//! 所有临时文件都放在系统临时目录并在结束时删除。

use std::sync::Arc;
use std::time::Duration;

use ffmpeg_web_rust::ffmpeg::encoder_catalog::EncoderCatalog;
use ffmpeg_web_rust::ffmpeg::hardware::{HardwareDetector, HardwarePreferences};
use ffmpeg_web_rust::ffmpeg::locator;
use ffmpeg_web_rust::ffmpeg::service::{
    FFmpegService, RunEvent, RunOutcome, RunRequest,
};
use tokio::sync::mpsc::UnboundedReceiver;
use tokio_util::sync::CancellationToken;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    section("1. 路径定位 locator::resolve(None, None)");
    let paths = locator::resolve(None, None);
    println!("  ffmpeg          : {} (resolved={})", paths.ffmpeg, paths.ffmpeg_resolved);
    println!("  ffprobe         : {} (resolved={})", paths.ffprobe, paths.ffprobe_resolved);
    println!("  source          : {}", paths.source);
    println!("  hint            : {}", paths.hint);
    println!("  内置候选顺序    : {:?}", locator::bundled_candidates("ffmpeg"));
    println!("  ffprobe 首选    : {:?}", locator::probe_candidates().first());

    let service = Arc::new(FFmpegService::new_with_paths(paths.clone()));

    section("2. 版本与能力探测 service.info(force_refresh = true)");
    let Some(info) = service.info(true).await else {
        anyhow::bail!("info() 返回 None，FFmpeg 不可用");
    };
    println!("  available       : {}", info.available);
    println!("  version         : {} (major = {})", info.version, info.major_version);
    println!("  version_line    : {}", info.version_line);
    println!("  path            : {}", info.path);
    println!(
        "  encoders={} codecs={} formats={} filters={} hwaccels={}",
        info.encoders.len(),
        info.codecs.len(),
        info.formats.len(),
        info.filters.len(),
        info.hwaccels.len()
    );
    println!("  hwaccels        : {:?}", info.hwaccels);
    println!("  has libx264     : {}", info.has_encoder("libx264"));
    println!("  has h264_nvenc  : {}", info.has_encoder("h264_nvenc"));
    println!("  has hevc_videotoolbox: {}", info.has_encoder("hevc_videotoolbox"));
    println!("  has scale filter: {}", info.has_filter("scale"));
    println!("  build config 前 80 字 : {}", &info.build_configuration.chars().take(80).collect::<String>());

    let temp = std::env::temp_dir();
    let source = temp.join(format!("ffmpeg-selftest-src-{}.mp4", std::process::id()));
    let output = temp.join(format!("ffmpeg-selftest-out-{}.mp4", std::process::id()));
    let long_output = temp.join(format!("ffmpeg-selftest-long-{}.mp4", std::process::id()));
    let source_str = source.to_string_lossy().into_owned();
    let output_str = output.to_string_lossy().into_owned();
    let long_str = long_output.to_string_lossy().into_owned();

    section("3. 用服务自身的 run() 生成测试素材（testsrc2 + sine，2 秒）");
    let generate = RunRequest::new(
        [
            "-f",
            "lavfi",
            "-i",
            "testsrc2=duration=2:size=640x360:rate=25",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=440:duration=2",
            "-c:v",
            "libx264",
            "-preset",
            "ultrafast",
            "-crf",
            "30",
            "-pix_fmt",
            "yuv420p",
            "-c:a",
            "aac",
            "-shortest",
            source_str.as_str(),
        ]
        .iter()
        .map(|value| value.to_string())
        .collect::<Vec<_>>(),
    )
    .with_task_key("selftest-generate");
    let generated = service.run(generate, None, CancellationToken::new()).await?;
    print_outcome("  生成素材", &generated);
    println!("  文件大小        : {} 字节", tokio::fs::metadata(&source).await?.len());

    section("4. ffprobe 媒体探测 service.probe_media()");
    let media = service.probe_media(&source_str).await?;
    println!("  file_name       : {}", media.file_name);
    println!("  size_bytes      : {}", media.size_bytes);
    println!(
        "  duration        : {} s（显示 {}）",
        media.duration_seconds,
        media.duration_display()
    );
    println!(
        "  format          : {} / {}（显示 {}）",
        media.format_name,
        media.format_long_name,
        media.format_display()
    );
    println!("  bit_rate        : {}", media.bit_rate);
    println!(
        "  video           : {} / frame_rate={:?} pix_fmt={:?} rotation={}",
        media.video.as_ref().map(|v| v.summary()).unwrap_or_else(|| "无".into()),
        media.video.as_ref().and_then(|v| v.frame_rate),
        media.video.as_ref().and_then(|v| v.pixel_format.clone()),
        media.rotation()
    );
    println!(
        "  audio           : {}",
        media.audio.as_ref().map(|a| a.summary()).unwrap_or_else(|| "无".into())
    );
    println!("  streams         : {}", media.streams.len());
    println!("  subtitles       : {}", media.subtitle_stream_count());
    println!("  has_video/audio : {} / {}", media.has_video(), media.has_audio());

    section("5. 真实转换 + 进度事件（-progress pipe:2 解析）");
    let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
    let printer = tokio::spawn(print_events(receiver));
    let convert = RunRequest::new(
        [
            // `-re` 让输入按真实速率读取，这样 2 秒的素材能产生多个进度块，便于观察百分比
            "-re",
            "-i",
            source_str.as_str(),
            "-c:v",
            "libx264",
            "-preset",
            "ultrafast",
            "-crf",
            "32",
            "-c:a",
            "aac",
            output_str.as_str(),
        ]
        .iter()
        .map(|value| value.to_string())
        .collect::<Vec<_>>(),
    )
    .with_duration_hint(media.duration_seconds)
    .with_task_key("selftest-convert");
    let converted = service.run(convert, Some(sender), CancellationToken::new()).await?;
    let _ = printer.await;
    print_outcome("  转换", &converted);
    println!("  输出大小        : {} 字节", tokio::fs::metadata(&output).await?.len());
    let converted_media = service.probe_media(&output_str).await?;
    println!(
        "  转换后          : {} / {} / {}",
        converted_media.format_display(),
        converted_media.video.as_ref().map(|v| v.summary()).unwrap_or_default(),
        converted_media.audio.as_ref().map(|a| a.summary()).unwrap_or_default()
    );

    section("6. 优雅停止：CancellationToken（写 q → 5 秒内退出）");
    let cancel = CancellationToken::new();
    let cancel_handle = cancel.clone();
    let cancel_request = long_request(&long_str, "selftest-cancel");
    let long_run = {
        let service = service.clone();
        tokio::spawn(async move { service.run(cancel_request, None, cancel_handle).await })
    };
    tokio::time::sleep(Duration::from_millis(1500)).await;
    println!("  running_count   : {}", service.running_count());
    cancel.cancel();
    match long_run.await? {
        Ok(outcome) => {
            print_outcome("  取消", &outcome);
            println!("  cancelled       : {}", outcome.cancelled);
        }
        Err(err) => println!("  取消返回错误    : {err}"),
    }
    println!("  结束后的 running_count: {}", service.running_count());

    section("7. stop_task(task_key, graceful = true)");
    let stop_request = long_request(&long_str, "selftest-stop");
    let stop_run = {
        let service = service.clone();
        tokio::spawn(async move { service.run(stop_request, None, CancellationToken::new()).await })
    };
    tokio::time::sleep(Duration::from_millis(1500)).await;
    println!("  running_count   : {}", service.running_count());
    println!("  stop_task 返回  : {}", service.stop_task("selftest-stop", true).await);
    println!("  未知 key 返回   : {}", service.stop_task("no-such-task", true).await);
    match stop_run.await? {
        Ok(outcome) => print_outcome("  被 stop_task 停止", &outcome),
        Err(err) => println!("  返回错误        : {err}"),
    }
    println!("  结束后的 running_count: {}", service.running_count());

    section("8. 硬件编码器检测（detect(force_refresh = true)）");
    let detector = HardwareDetector::with_preferences(
        service.clone(),
        HardwarePreferences::default(),
    );
    let report = detector.detect(true).await;
    println!("  platform        : {}", report.platform);
    println!("  ffmpeg_version  : {}", report.ffmpeg_version);
    println!("  ffmpeg_path     : {}", report.ffmpeg_path);
    println!("  hwaccels        : {:?}", report.hwaccels);
    println!("  filters          : {} 项（has_filter(scale) = {}）", report.supported_filters.len(), report.has_filter("scale"));
    println!("  gpu_summary     : {}", report.gpu_summary);
    println!("  summary         : {}", report.summary);
    println!(
        "  可用硬件编码器  : {} / has_hardware_acceleration = {}",
        report.available_hardware_encoders().len(),
        report.has_hardware_acceleration()
    );
    println!("  条目数          : {}", report.encoders.len());
    println!("  codec          推荐编码器");
    for (codec, encoder) in &report.recommended {
        println!("    {codec:<12} {encoder}");
    }
    println!("  硬件条目明细：");
    for entry in report.encoders.iter().filter(|e| e.is_hardware) {
        println!(
            "    {:<20} available={:<5} quality={:<5} missing_from_build={:<5} reason={}",
            entry.encoder_name,
            entry.available,
            entry.verified_by_trial,
            entry.missing_from_build,
            if entry.reason.is_empty() { "-" } else { entry.reason.as_str() }
        );
    }
    println!("  pick_required(&report, \"nvenc\") = {:?}", detector.pick_required(&report, "nvenc"));
    println!(
        "  pick_required(&report, \"videotoolbox\") = {:?}",
        detector.pick_required(&report, "videotoolbox")
    );
    println!(
        "  recommended_encoder(\"h264\", None)    = {:?}",
        detector.recommended_encoder("h264", None).await
    );
    println!(
        "  recommended_encoder(\"hevc\", None)    = {:?}",
        detector.recommended_encoder("hevc", None).await
    );
    println!(
        "  recommended_encoder(\"h264\", Some(\"nvenc\")) = {:?}",
        detector.recommended_encoder("h264", Some("nvenc")).await
    );
    println!(
        "  is_encoder_available(\"libx264\")      = {}",
        detector.is_encoder_available("libx264").await
    );
    println!(
        "  last_report() / 画像查询            : {} / {:?}",
        detector.last_report().is_some(),
        HardwareDetector::resolve_profile("hevc_videotoolbox").map(|p| p.display_name.clone())
    );
    println!(
        "  第二次 detect(false) 命中内存缓存   : {} 条",
        detector.detect(false).await.encoders.len()
    );

    section("9. 画像表抽样（31 条）");
    println!("  画像条数        : {}", EncoderCatalog::all().len());
    println!("  platform        : {}（macOS 上 h264 候选）", EncoderCatalog::platform());
    for profile in EncoderCatalog::for_codec("h264") {
        println!(
            "    {:<22} sort={:<3} quality=\"{}\" preset=\"{}\"",
            profile.name,
            profile.sort_preference,
            profile.render_quality_args(None),
            profile.preset_args
        );
    }
    println!(
        "  normalize_codec(\"hvc1\") = {}, expand_family(\"vaapi\") = {:?}",
        EncoderCatalog::normalize_codec("hvc1"),
        EncoderCatalog::expand_family("vaapi")
    );

    // 清理临时文件
    for path in [&source, &output, &long_output] {
        let _ = tokio::fs::remove_file(path).await;
    }
    println!("\n自检完成（临时文件已清理）。");
    Ok(())
}

fn long_request(output: &str, task_key: &str) -> RunRequest {
    RunRequest::new(
        [
            "-f",
            "lavfi",
            "-i",
            "testsrc2=duration=600:size=1280x720:rate=30",
            "-c:v",
            "libx264",
            "-preset",
            "ultrafast",
            "-crf",
            "30",
            output,
        ]
        .iter()
        .map(|value| value.to_string())
        .collect::<Vec<_>>(),
    )
    .with_duration_hint(600.0)
    .with_task_key(task_key)
}

fn print_outcome(label: &str, outcome: &RunOutcome) {
    println!(
        "{label}: exit={:?} success={} cancelled={} timed_out={} 耗时={}ms 日志行={}",
        outcome.exit_code,
        outcome.success,
        outcome.cancelled,
        outcome.timed_out,
        outcome.elapsed_ms,
        outcome.log_lines.len()
    );
    if !outcome.stderr_tail.trim().is_empty() {
        // 只展示尾部 3 行，避免刷屏（完整内容在 outcome.stderr_tail / log_lines 里）
        let lines: Vec<&str> = outcome.stderr_tail.lines().filter(|l| !l.trim().is_empty()).collect();
        let tail: Vec<&str> = lines.iter().rev().take(3).copied().collect();
        for line in tail.iter().rev() {
            println!("  stderr 尾部     : {}", line.trim());
        }
    }
}

/// 打印事件流：启动命令、进度百分比（按 20% 采样）、结束事件。
async fn print_events(mut receiver: UnboundedReceiver<RunEvent>) {
    let mut last_bucket: i64 = -1;
    let mut logged = 0usize;
    while let Some(event) = receiver.recv().await {
        match event {
            RunEvent::Started { pid, command_line } => {
                println!("  pid             : {pid:?}");
                println!("  command_line    : {command_line}");
            }
            RunEvent::Stderr(line) => {
                if logged < 5 {
                    println!("  日志            : {}", line.trim());
                    logged += 1;
                }
            }
            RunEvent::Progress(tick) => {
                let bucket = (tick.percent / 20.0).floor() as i64;
                if bucket != last_bucket || tick.progress.as_deref() == Some("end") {
                    last_bucket = bucket;
                    println!(
                        "  进度            : {:.2}% time={:.3}s speed={:?} eta={:?} frame={:?} progress={:?}",
                        tick.percent,
                        tick.out_time_seconds,
                        tick.speed,
                        tick.eta_seconds(2.0),
                        tick.frame,
                        tick.progress
                    );
                }
            }
            RunEvent::Finished { exit_code, elapsed_ms, cancelled, timed_out, .. } => {
                println!(
                    "  Finished 事件   : exit={exit_code:?} elapsed={elapsed_ms}ms cancelled={cancelled} timed_out={timed_out}"
                );
            }
        }
    }
}

fn section(title: &str) {
    println!("\n=== {title} ===");
}
