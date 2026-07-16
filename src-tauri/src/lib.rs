use std::collections::{HashSet, hash_map::DefaultHasher};
use std::fs::{self, OpenOptions};
use std::hash::{Hash, Hasher};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{self, Command, Stdio};
use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tauri::{AppHandle, Emitter, Manager};
#[cfg(not(target_os = "macos"))]
use tauri_plugin_dialog::DialogExt;

#[allow(dead_code)]
#[path = "../../src/ffmpeg.rs"]
mod ffmpeg;
#[allow(dead_code)]
#[path = "../../src/ffprobe.rs"]
mod ffprobe;
#[allow(dead_code)]
#[path = "../../src/tools.rs"]
mod tools;

use ffmpeg::{
    AudioTitleUpdate, ConversionPlan, ConvertOptions, DeleteAudioOptions, ProcessOptions,
    TargetFormat, TrackMode,
};
use tools::ToolPaths;

const THUMBNAIL_CACHE_VERSION: u8 = 2;
static SESSION_STARTED_AT: OnceLock<u64> = OnceLock::new();

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct AudioTrackDto {
    stream_index: usize,
    audio_ordinal: usize,
    codec: String,
    channels: Option<u32>,
    sample_rate: Option<String>,
    bit_rate: Option<String>,
    language: Option<String>,
    title: Option<String>,
    is_default: bool,
    label: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct VideoFileDto {
    path: String,
    file_name: String,
    duration_seconds: Option<f64>,
    audio_tracks: Vec<AudioTrackDto>,
    thumbnail_path: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ConvertRequest {
    job_id: String,
    input: String,
    audio_index: usize,
    format: String,
    mode: String,
    make_default: bool,
    replace_original: bool,
    output: Option<String>,
    default_audio_ordinal: Option<usize>,
    #[serde(default)]
    titles: Vec<AudioTitleRequest>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DeleteAudioRequest {
    job_id: String,
    input: String,
    audio_indices: Vec<usize>,
    replace_original: bool,
    output: Option<String>,
    default_audio_ordinal: Option<usize>,
    #[serde(default)]
    titles: Vec<AudioTitleRequest>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProcessFileRequest {
    job_id: String,
    input: String,
    replace_original: bool,
    use_local_temp: bool,
    convert: Option<ConvertOperationRequest>,
    #[serde(default)]
    delete_audio_indices: Vec<usize>,
    default_audio_ordinal: Option<usize>,
    #[serde(default)]
    titles: Vec<AudioTitleRequest>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ConvertOperationRequest {
    audio_index: usize,
    format: String,
    mode: String,
    make_default: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AudioTitleRequest {
    audio_ordinal: usize,
    title: String,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct JobProgress {
    job_id: String,
    progress: Option<f64>,
    eta_seconds: Option<u64>,
    processed_seconds: f64,
    stage: Option<String>,
    log: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct LogFileDto {
    session_id: String,
    started_at: u64,
    is_current: bool,
    path: String,
    entries: Vec<LogEntryDto>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct LogEntryDto {
    timestamp: u64,
    level: String,
    message: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct LogSessionDto {
    id: String,
    started_at: u64,
    is_current: bool,
    size_bytes: u64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct JobResult {
    output: String,
    replaced_original: bool,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(transparent)]
struct HistoryEntry(Value);

#[tauri::command]
fn analyze_files(paths: Vec<String>) -> Result<Vec<VideoFileDto>, String> {
    let tools = discover_tools(None).map_err(error_string)?;

    paths
        .into_iter()
        .map(|path| analyze_file(PathBuf::from(path), &tools).map_err(error_string))
        .collect()
}

#[tauri::command]
async fn convert_audio(app: AppHandle, request: ConvertRequest) -> Result<JobResult, String> {
    tauri::async_runtime::spawn_blocking(move || convert_audio_inner(app, request))
        .await
        .map_err(|error| format!("Could not wait for the conversion job: {error}"))?
        .map_err(error_string)
}

#[tauri::command]
async fn delete_audio(app: AppHandle, request: DeleteAudioRequest) -> Result<JobResult, String> {
    tauri::async_runtime::spawn_blocking(move || delete_audio_inner(app, request))
        .await
        .map_err(|error| format!("Could not wait for the audio job: {error}"))?
        .map_err(error_string)
}

#[tauri::command]
async fn process_file(app: AppHandle, request: ProcessFileRequest) -> Result<JobResult, String> {
    tauri::async_runtime::spawn_blocking(move || process_file_inner(app, request))
        .await
        .map_err(|error| format!("Could not wait for the audio job: {error}"))?
        .map_err(error_string)
}

#[tauri::command]
async fn pick_video_files(app: AppHandle) -> Result<Vec<String>, String> {
    tauri::async_runtime::spawn_blocking(move || pick_video_files_inner(&app))
        .await
        .map_err(|error| format!("Could not wait for the file picker: {error}"))?
        .map_err(error_string)
}

#[cfg(target_os = "macos")]
fn pick_video_files_inner(_app: &AppHandle) -> Result<Vec<String>> {
    let script = r#"
set selectedFiles to choose file with prompt "Select one or more videos" with multiple selections allowed
set selectedPaths to {}
repeat with selectedFile in selectedFiles
    set end of selectedPaths to POSIX path of selectedFile
end repeat
set AppleScript's text item delimiters to linefeed
return selectedPaths as text
"#;
    let output = Command::new("/usr/bin/osascript")
        .args(["-e", script])
        .output()
        .context("Could not open the macOS file picker")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        if stderr.contains("(-128)") || stderr.contains("User canceled") {
            return Ok(Vec::new());
        }
        bail!("The macOS file picker failed: {}", stderr.trim());
    }

    let paths = String::from_utf8(output.stdout)
        .context("The macOS file picker returned paths that are not valid UTF-8")?
        .lines()
        .map(str::trim)
        .filter(|path| !path.is_empty())
        .map(str::to_string)
        .collect::<Vec<_>>();
    validate_selected_video_paths(paths)
}

#[cfg(not(target_os = "macos"))]
fn pick_video_files_inner(app: &AppHandle) -> Result<Vec<String>> {
    let selected = app
        .dialog()
        .file()
        .add_filter("Video", &["mkv", "mp4", "mov", "avi", "webm", "m4v"])
        .blocking_pick_files()
        .unwrap_or_default();
    let paths = selected
        .into_iter()
        .map(|path| path.into_path().map_err(anyhow::Error::from))
        .collect::<Result<Vec<_>>>()?
        .into_iter()
        .map(|path| path.display().to_string())
        .collect();
    Ok(paths)
}

fn validate_selected_video_paths(paths: Vec<String>) -> Result<Vec<String>> {
    const VIDEO_EXTENSIONS: &[&str] = &["mkv", "mp4", "mov", "avi", "webm", "m4v"];
    let invalid = paths.iter().find(|path| {
        Path::new(path)
            .extension()
            .and_then(|extension| extension.to_str())
            .is_none_or(|extension| {
                !VIDEO_EXTENSIONS
                    .iter()
                    .any(|allowed| extension.eq_ignore_ascii_case(allowed))
            })
    });
    if let Some(path) = invalid {
        bail!("The selected file is not a supported video: {path}");
    }
    Ok(paths)
}

#[tauri::command]
fn read_log(app: AppHandle) -> Result<LogFileDto, String> {
    read_log_session_inner(&app, &session_started_at().to_string()).map_err(error_string)
}

#[tauri::command]
fn list_log_sessions(app: AppHandle) -> Result<Vec<LogSessionDto>, String> {
    log_sessions(&app).map_err(error_string)
}

#[tauri::command]
fn read_log_session(app: AppHandle, session_id: String) -> Result<LogFileDto, String> {
    read_log_session_inner(&app, &session_id).map_err(error_string)
}

#[tauri::command]
fn clear_logs(app: AppHandle) -> Result<(), String> {
    clear_log_files(&app).map_err(error_string)
}

#[tauri::command]
fn list_history(app: AppHandle) -> Result<Vec<HistoryEntry>, String> {
    read_history(&app).map_err(error_string)
}

#[tauri::command]
fn save_history_entry(app: AppHandle, entry: HistoryEntry) -> Result<(), String> {
    let mut history = read_history(&app).map_err(error_string)?;
    history.insert(0, entry);
    write_history(&app, &history).map_err(error_string)
}

#[tauri::command]
fn clear_history(app: AppHandle) -> Result<(), String> {
    let path = history_file_path(&app).map_err(error_string)?;
    if path.exists() {
        fs::remove_file(&path)
            .with_context(|| format!("Could not clear the history: {}", path.display()))
            .map_err(error_string)?;
    }
    Ok(())
}

fn analyze_file(path: PathBuf, tools: &ToolPaths) -> Result<VideoFileDto> {
    if !path.exists() {
        bail!("The file does not exist: {}", path.display());
    }

    let probe = ffprobe::probe_file(&path, tools)?;
    let audio_tracks = ffprobe::audio_tracks(&probe)
        .into_iter()
        .map(|track| {
            let label = track.label();

            AudioTrackDto {
                stream_index: track.stream_index,
                audio_ordinal: track.audio_ordinal,
                codec: track.codec,
                channels: track.channels,
                sample_rate: track.sample_rate,
                bit_rate: track.bit_rate,
                language: track.language,
                title: track.title,
                is_default: track.is_default,
                label,
            }
        })
        .collect();

    Ok(VideoFileDto {
        file_name: path
            .file_name()
            .and_then(|file_name| file_name.to_str())
            .unwrap_or("video")
            .to_string(),
        path: path.display().to_string(),
        duration_seconds: ffprobe::duration_seconds(&probe),
        audio_tracks,
        thumbnail_path: thumbnail_file(&path, &probe, tools)
            .ok()
            .map(|path| path.display().to_string()),
    })
}

fn thumbnail_file(input: &Path, probe: &ffprobe::Probe, tools: &ToolPaths) -> Result<PathBuf> {
    let output = thumbnail_path(input);
    if output.is_file() {
        return Ok(output);
    }

    if let Some(parent) = output.parent() {
        fs::create_dir_all(parent).with_context(|| {
            format!(
                "Could not create the thumbnail folder: {}",
                parent.display()
            )
        })?;
    }

    let seek = thumbnail_seek_seconds(ffprobe::duration_seconds(probe));
    let status = Command::new(&tools.ffmpeg)
        .args([
            "-hide_banner",
            "-y",
            "-v",
            "error",
            "-ss",
            &format!("{seek:.3}"),
            "-i",
            input
                .to_str()
                .ok_or_else(|| anyhow::anyhow!("The file path is not valid UTF-8"))?,
            "-frames:v",
            "1",
            "-vf",
            "scale=360:-1",
            output
                .to_str()
                .ok_or_else(|| anyhow::anyhow!("The thumbnail path is not valid UTF-8"))?,
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .with_context(|| format!("Could not run {}", tools.ffmpeg.display()))?;

    if status.success() && output.is_file() {
        Ok(output)
    } else {
        bail!("Could not generate the thumbnail")
    }
}

fn thumbnail_path(input: &Path) -> PathBuf {
    let mut hasher = DefaultHasher::new();
    THUMBNAIL_CACHE_VERSION.hash(&mut hasher);
    input.display().to_string().hash(&mut hasher);
    if let Ok(metadata) = fs::metadata(input) {
        metadata.len().hash(&mut hasher);
        if let Ok(modified) = metadata.modified() {
            modified.hash(&mut hasher);
        }
    }

    std::env::temp_dir()
        .join("trackforge-thumbnails")
        .join(format!("{:016x}.jpg", hasher.finish()))
}

fn thumbnail_seek_seconds(duration_seconds: Option<f64>) -> f64 {
    let Some(duration) =
        duration_seconds.filter(|duration| duration.is_finite() && *duration > 0.0)
    else {
        return 10.0;
    };

    if duration < 60.0 {
        return (duration * 0.5).clamp(1.0, (duration - 0.5).max(1.0));
    }

    (duration * 0.25).clamp(10.0, 600.0).min(duration - 1.0)
}

fn convert_audio_inner(app: AppHandle, request: ConvertRequest) -> Result<JobResult> {
    let _sleep_inhibitor = prevent_sleep_during_processing(&app, &request.job_id);
    let input = PathBuf::from(&request.input);
    let tools = discover_tools(Some(&app))?;
    let probe = ffprobe::probe_file(&input, &tools)?;
    validate_audio_ordinals(&probe, &request.titles)?;
    validate_default_audio_ordinal(&probe, request.default_audio_ordinal)?;
    let tracks = ffprobe::audio_tracks(&probe);
    let track = tracks
        .iter()
        .find(|track| track.stream_index == request.audio_index)
        .with_context(|| format!("No audio track has stream index {}", request.audio_index))?;

    let format = request.format.parse::<TargetFormat>()?;
    let mode = request.mode.parse::<TrackMode>()?;
    let requested_output = request.output.map(PathBuf::from);
    let output = requested_output.unwrap_or_else(|| default_output_path(&input, format));
    let conversion_output = if request.replace_original {
        temporary_replacement_path(&input, format.slug())
    } else {
        output
    };
    let titles = request
        .titles
        .into_iter()
        .map(|title| AudioTitleUpdate {
            audio_ordinal: title.audio_ordinal,
            title: title.title,
        })
        .collect::<Vec<_>>();

    let options = ConvertOptions {
        tools: &tools,
        input: &input,
        output: &conversion_output,
        probe: &probe,
        track,
        format,
        mode,
        make_default: request.make_default,
        default_audio_ordinal: request.default_audio_ordinal,
        titles: &titles,
    };

    let job_id = request.job_id.clone();
    ffmpeg::convert_with_progress(options, |update| {
        app.emit(
            "trackforge://progress",
            JobProgress {
                job_id: job_id.clone(),
                progress: update.progress,
                eta_seconds: update.eta_seconds,
                processed_seconds: update.processed_seconds,
                stage: None,
                log: None,
            },
        )?;
        Ok(())
    })?;

    if request.replace_original {
        replace_original_file(&input, &conversion_output)?;
        Ok(JobResult {
            output: input.display().to_string(),
            replaced_original: true,
        })
    } else {
        Ok(JobResult {
            output: conversion_output.display().to_string(),
            replaced_original: false,
        })
    }
}

fn delete_audio_inner(app: AppHandle, request: DeleteAudioRequest) -> Result<JobResult> {
    let _sleep_inhibitor = prevent_sleep_during_processing(&app, &request.job_id);
    if request.audio_indices.is_empty()
        && request.titles.is_empty()
        && request.default_audio_ordinal.is_none()
    {
        bail!("Select tracks to delete or change some audio metadata.");
    }

    let input = PathBuf::from(&request.input);
    let tools = discover_tools(Some(&app))?;
    let probe = ffprobe::probe_file(&input, &tools)?;
    validate_audio_indices(&probe, &request.audio_indices)?;
    validate_audio_ordinals(&probe, &request.titles)?;
    validate_default_audio_ordinal(&probe, request.default_audio_ordinal)?;

    let requested_output = request.output.map(PathBuf::from);
    let output = requested_output.unwrap_or_else(|| {
        if request.audio_indices.is_empty() {
            default_audio_metadata_output_path(&input)
        } else {
            default_delete_audio_output_path(&input)
        }
    });
    let delete_output = if request.replace_original {
        temporary_replacement_path(&input, "audio-delete")
    } else {
        output
    };
    let titles = request
        .titles
        .into_iter()
        .map(|title| AudioTitleUpdate {
            audio_ordinal: title.audio_ordinal,
            title: title.title,
        })
        .collect::<Vec<_>>();

    let options = DeleteAudioOptions {
        tools: &tools,
        input: &input,
        output: &delete_output,
        probe: &probe,
        stream_indices: &request.audio_indices,
        default_audio_ordinal: request.default_audio_ordinal,
        titles: &titles,
    };

    let job_id = request.job_id.clone();
    ffmpeg::delete_audio_tracks_with_progress(options, |update| {
        app.emit(
            "trackforge://progress",
            JobProgress {
                job_id: job_id.clone(),
                progress: update.progress,
                eta_seconds: update.eta_seconds,
                processed_seconds: update.processed_seconds,
                stage: None,
                log: None,
            },
        )?;
        Ok(())
    })?;

    if request.replace_original {
        replace_original_file(&input, &delete_output)?;
        Ok(JobResult {
            output: input.display().to_string(),
            replaced_original: true,
        })
    } else {
        Ok(JobResult {
            output: delete_output.display().to_string(),
            replaced_original: false,
        })
    }
}

fn process_file_inner(app: AppHandle, request: ProcessFileRequest) -> Result<JobResult> {
    let _sleep_inhibitor = prevent_sleep_during_processing(&app, &request.job_id);
    log_line(
        &app,
        format!(
            "job={} Operation started. File: {} | Conversion: {} | Tracks to delete: {} | Titles to change: {} | Replace original: {} | Local temporary storage: {}",
            request.job_id,
            request.input,
            if request.convert.is_some() {
                "Yes"
            } else {
                "No"
            },
            request.delete_audio_indices.len(),
            request.titles.len(),
            if request.replace_original {
                "Yes"
            } else {
                "No"
            },
            if request.use_local_temp { "Yes" } else { "No" },
        ),
    );

    if request.convert.is_none()
        && request.delete_audio_indices.is_empty()
        && request.titles.is_empty()
        && request.default_audio_ordinal.is_none()
    {
        bail!("Select at least one operation to process the file.");
    }

    let input = PathBuf::from(&request.input);
    let tools = discover_tools(Some(&app))?;
    let probe = ffprobe::probe_file(&input, &tools)?;
    validate_audio_indices(&probe, &request.delete_audio_indices)?;
    validate_audio_ordinals(&probe, &request.titles)?;
    validate_default_audio_ordinal(&probe, request.default_audio_ordinal)?;

    let tracks = ffprobe::audio_tracks(&probe);
    let parsed_conversion = request
        .convert
        .as_ref()
        .map(|convert| {
            let track = tracks
                .iter()
                .find(|track| track.stream_index == convert.audio_index)
                .with_context(|| {
                    format!("No audio track has stream index {}", convert.audio_index)
                })?;

            Ok::<_, anyhow::Error>((
                track,
                convert.format.parse::<TargetFormat>()?,
                convert.mode.parse::<TrackMode>()?,
                convert.make_default,
            ))
        })
        .transpose()?;

    let final_output = if request.replace_original {
        input.clone()
    } else {
        default_process_output_path(
            &input,
            parsed_conversion
                .as_ref()
                .map(|(_, format, _, _)| format.slug()),
            !request.delete_audio_indices.is_empty(),
            !request.titles.is_empty() || request.default_audio_ordinal.is_some(),
        )
    };
    let work_output = if request.use_local_temp {
        local_temporary_output_path(&input, "process")?
    } else if request.replace_original {
        temporary_replacement_path(&input, "process")
    } else {
        final_output.clone()
    };
    let local_input = request
        .use_local_temp
        .then(|| local_temporary_output_path(&input, "input"))
        .transpose()?;

    log_progress(
        &app,
        &request.job_id,
        format!(
            "Paths prepared. Working file: {} | Final destination: {}",
            work_output.display(),
            final_output.display()
        ),
    );

    if request.use_local_temp {
        ensure_local_temp_space(&app, &request.job_id, &input, &work_output)?;
        cleanup_stale_local_copy_temps(&final_output)?;
        let local_input = local_input
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Could not prepare the local temporary input"))?;
        app.emit(
            "trackforge://progress",
            JobProgress {
                job_id: request.job_id.clone(),
                progress: Some(0.0),
                eta_seconds: None,
                processed_seconds: 0.0,
                stage: Some("Copying to local temporary storage".to_string()),
                log: Some("Copying the input file to local temporary storage".to_string()),
            },
        )?;
        copy_input_to_local_temp(&app, &request.job_id, &input, local_input)?;
    }

    if let Some(parent) = work_output.parent() {
        fs::create_dir_all(parent).with_context(|| {
            format!(
                "Could not create the temporary processing folder: {}",
                parent.display()
            )
        })?;
    }

    let titles = request
        .titles
        .into_iter()
        .map(|title| AudioTitleUpdate {
            audio_ordinal: title.audio_ordinal,
            title: title.title,
        })
        .collect::<Vec<_>>();
    let conversion = parsed_conversion
        .as_ref()
        .map(|(track, format, mode, make_default)| ConversionPlan {
            track,
            format: *format,
            mode: *mode,
            make_default: *make_default,
        });

    let options = ProcessOptions {
        tools: &tools,
        input: local_input.as_deref().unwrap_or(&input),
        output: &work_output,
        probe: &probe,
        conversion,
        delete_stream_indices: &request.delete_audio_indices,
        default_audio_ordinal: request.default_audio_ordinal,
        titles: &titles,
    };

    let job_id = request.job_id.clone();
    let ffmpeg_args = ffmpeg::build_process_args(&options);
    log_progress(
        &app,
        &job_id,
        format!(
            "Starting FFmpeg: {} {}",
            tools.ffmpeg.display(),
            ffmpeg_args.join(" ")
        ),
    );
    let mut last_logged_progress_bucket = 0u32;
    let process_result = ffmpeg::process_with_progress(options, |update| {
        if let Some(progress) = update.progress {
            let bucket = (progress * 10.0).floor().clamp(0.0, 10.0) as u32;
            if bucket > last_logged_progress_bucket {
                last_logged_progress_bucket = bucket;
                log_line(
                    &app,
                    format!("job={} FFmpeg progress {}%", job_id, bucket * 10),
                );
            }
        }

        app.emit(
            "trackforge://progress",
            JobProgress {
                job_id: job_id.clone(),
                progress: update.progress,
                eta_seconds: update.eta_seconds,
                processed_seconds: update.processed_seconds,
                stage: None,
                log: None,
            },
        )?;
        Ok(())
    });

    let finalize_result = process_result.and_then(|()| {
        log_progress(
            &app,
            &request.job_id,
            "FFmpeg completed successfully".to_string(),
        );
        if request.use_local_temp {
            app.emit(
                "trackforge://progress",
                JobProgress {
                    job_id: request.job_id.clone(),
                    progress: Some(1.0),
                    eta_seconds: None,
                    processed_seconds: 0.0,
                    stage: Some("Copying to destination".to_string()),
                    log: Some("Copying the local file to the final destination".to_string()),
                },
            )?;
            finalize_local_temp_output(
                &app,
                &request.job_id,
                &work_output,
                &final_output,
                request.replace_original,
            )
        } else if request.replace_original {
            log_progress(
                &app,
                &request.job_id,
                "Replacing original with remote temporary file".to_string(),
            );
            replace_original_file(&input, &work_output)
        } else {
            Ok(())
        }
    });

    if request.use_local_temp {
        if let Some(local_input) = local_input.as_ref() {
            log_progress(
                &app,
                &request.job_id,
                format!("Deleting local temporary input: {}", local_input.display()),
            );
            let _ = fs::remove_file(local_input);
        }
        log_progress(
            &app,
            &request.job_id,
            format!("Deleting local temporary file: {}", work_output.display()),
        );
        let _ = fs::remove_file(&work_output);
    }

    if let Err(error) = finalize_result {
        log_progress(&app, &request.job_id, format!("ERROR: {error:#}"));
        return Err(error);
    }

    log_progress(&app, &request.job_id, "Process completed".to_string());

    if request.replace_original {
        Ok(JobResult {
            output: final_output.display().to_string(),
            replaced_original: true,
        })
    } else {
        Ok(JobResult {
            output: final_output.display().to_string(),
            replaced_original: false,
        })
    }
}

fn discover_tools(app: Option<&AppHandle>) -> Result<ToolPaths> {
    if let Some(app) = app {
        if let Ok(resource_dir) = app.path().resource_dir() {
            let ffmpeg = resource_dir
                .join("vendor")
                .join("ffmpeg")
                .join("bin")
                .join(tools::executable_name("ffmpeg"));
            let ffprobe = resource_dir
                .join("vendor")
                .join("ffmpeg")
                .join("bin")
                .join(tools::executable_name("ffprobe"));

            if ffmpeg.is_file() && ffprobe.is_file() {
                return Ok(ToolPaths { ffmpeg, ffprobe });
            }
        }
    }

    ToolPaths::discover()
}

fn validate_audio_indices(probe: &ffprobe::Probe, indices: &[usize]) -> Result<()> {
    let available = ffprobe::audio_tracks(probe)
        .into_iter()
        .map(|track| track.stream_index)
        .collect::<HashSet<_>>();

    for stream_index in indices {
        if !available.contains(stream_index) {
            bail!("No audio track has stream index {stream_index}");
        }
    }

    Ok(())
}

fn validate_audio_ordinals(probe: &ffprobe::Probe, titles: &[AudioTitleRequest]) -> Result<()> {
    let audio_count = ffprobe::audio_tracks(probe).len();

    for title in titles {
        if title.audio_ordinal >= audio_count {
            bail!("No audio track has ordinal {}", title.audio_ordinal);
        }
    }

    Ok(())
}

fn validate_default_audio_ordinal(probe: &ffprobe::Probe, ordinal: Option<usize>) -> Result<()> {
    let Some(ordinal) = ordinal else {
        return Ok(());
    };

    let audio_count = ffprobe::audio_tracks(probe).len();
    if ordinal >= audio_count {
        bail!("No audio track has ordinal {ordinal}");
    }

    Ok(())
}

fn default_output_path(input: &Path, format: TargetFormat) -> PathBuf {
    let parent = input.parent().unwrap_or_else(|| Path::new("."));
    let stem = input
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("output");
    let extension = input
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or("mkv");

    parent.join(format!("{stem}_trackforge_{}.{}", format.slug(), extension))
}

fn default_delete_audio_output_path(input: &Path) -> PathBuf {
    let parent = input.parent().unwrap_or_else(|| Path::new("."));
    let stem = input
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("output");
    let extension = input
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or("mkv");

    parent.join(format!("{stem}_trackforge_audio_removed.{extension}"))
}

fn default_audio_metadata_output_path(input: &Path) -> PathBuf {
    let parent = input.parent().unwrap_or_else(|| Path::new("."));
    let stem = input
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("output");
    let extension = input
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or("mkv");

    parent.join(format!("{stem}_trackforge_audio_metadata.{extension}"))
}

fn default_process_output_path(
    input: &Path,
    conversion_slug: Option<&str>,
    deletes_audio: bool,
    edits_metadata: bool,
) -> PathBuf {
    let parent = input.parent().unwrap_or_else(|| Path::new("."));
    let stem = input
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("output");
    let extension = input
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or("mkv");
    let mut parts = Vec::new();

    if let Some(slug) = conversion_slug {
        parts.push(slug.to_string());
    }
    if deletes_audio {
        parts.push("audio-delete".to_string());
    }
    if edits_metadata {
        parts.push("metadata".to_string());
    }
    if parts.is_empty() {
        parts.push("process".to_string());
    }

    parent.join(format!(
        "{stem}_trackforge_{}.{}",
        parts.join("_"),
        extension
    ))
}

fn temporary_replacement_path(input: &Path, slug: &str) -> PathBuf {
    let parent = input.parent().unwrap_or_else(|| Path::new("."));
    let stem = input
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("output");
    let extension = input
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or("mkv");

    parent.join(format!(
        ".{stem}.trackforge-{}.{}.{}",
        process::id(),
        slug,
        extension
    ))
}

fn local_temporary_output_path(input: &Path, slug: &str) -> Result<PathBuf> {
    let mut hasher = DefaultHasher::new();
    input.display().to_string().hash(&mut hasher);
    process::id().hash(&mut hasher);
    let extension = input
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or("mkv");
    let file_name = format!("{:016x}-{slug}.{extension}", hasher.finish());
    let dir = std::env::temp_dir().join("trackforge-processing");

    fs::create_dir_all(&dir).with_context(|| {
        format!(
            "Could not create the temporary processing folder: {}",
            dir.display()
        )
    })?;

    Ok(dir.join(file_name))
}

fn ensure_local_temp_space(
    app: &AppHandle,
    job_id: &str,
    input: &Path,
    work_output: &Path,
) -> Result<()> {
    let input_size = fs::metadata(input)
        .with_context(|| format!("Could not read the size of {}", input.display()))?
        .len();
    let required = required_local_temp_bytes(input_size);
    let temp_dir = work_output
        .parent()
        .ok_or_else(|| anyhow::anyhow!("The temporary path has no parent folder"))?;
    let available = fs2::available_space(temp_dir)
        .with_context(|| format!("Could not check the free space in {}", temp_dir.display()))?;

    log_progress(
        app,
        job_id,
        format!(
            "Local temporary space. Required: {} | Available: {} | Folder: {}",
            format_bytes(required),
            format_bytes(available),
            temp_dir.display()
        ),
    );

    if available < required {
        bail!(
            "There is not enough local temporary space. Required: {}, available: {}.",
            format_bytes(required),
            format_bytes(available)
        );
    }

    Ok(())
}

fn required_local_temp_bytes(input_size: u64) -> u64 {
    let margin = 512 * 1024 * 1024;
    input_size
        .saturating_mul(2)
        .saturating_add(input_size / 10)
        .saturating_add(margin)
}

fn copy_input_to_local_temp(
    app: &AppHandle,
    job_id: &str,
    input: &Path,
    local_input: &Path,
) -> Result<()> {
    remove_file_if_exists(local_input)?;
    log_progress(
        app,
        job_id,
        format!(
            "Copying input to local temporary storage. Source: {} | Destination: {}",
            input.display(),
            local_input.display()
        ),
    );

    if let Err(error) = fs::copy(input, local_input) {
        let _ = fs::remove_file(local_input);
        return Err(error).with_context(|| {
            format!(
                "Could not copy the input file to local temporary storage: {}",
                local_input.display()
            )
        });
    }

    log_progress(
        app,
        job_id,
        format!("Local input ready: {}", local_input.display()),
    );
    Ok(())
}

fn finalize_local_temp_output(
    app: &AppHandle,
    job_id: &str,
    work_output: &Path,
    final_output: &Path,
    replace_original: bool,
) -> Result<()> {
    if replace_original {
        replace_original_file_from_local_temp(app, job_id, final_output, work_output)
    } else {
        copy_local_temp_to_destination(app, job_id, work_output, final_output)
    }
}

fn copy_local_temp_to_destination(
    app: &AppHandle,
    job_id: &str,
    work_output: &Path,
    final_output: &Path,
) -> Result<()> {
    let destination_temp = temporary_replacement_path(final_output, "local-copy");
    let result = (|| {
        if let Some(parent) = destination_temp.parent() {
            fs::create_dir_all(parent).with_context(|| {
                format!(
                    "Could not create the destination folder: {}",
                    parent.display()
                )
            })?;
        }

        remove_file_if_exists(&destination_temp)?;

        log_progress(
            app,
            job_id,
            format!(
                "Copying local temporary file to destination. Source: {} | Destination temporary file: {}",
                work_output.display(),
                destination_temp.display()
            ),
        );
        fs::copy(work_output, &destination_temp).with_context(|| {
            format!(
                "Could not copy the local temporary file to the destination: {}",
                final_output.display()
            )
        })?;
        log_progress(
            app,
            job_id,
            format!(
                "Copy completed. Destination temporary file: {}",
                destination_temp.display()
            ),
        );

        if final_output.exists() {
            log_progress(
                app,
                job_id,
                format!("Deleting existing output: {}", final_output.display()),
            );
            fs::remove_file(final_output).with_context(|| {
                format!(
                    "Could not replace the existing output file: {}",
                    final_output.display()
                )
            })?;
        }

        log_progress(
            app,
            job_id,
            format!(
                "Renaming destination temporary file to final output: {}",
                final_output.display()
            ),
        );
        fs::rename(&destination_temp, final_output).with_context(|| {
            format!(
                "Could not place the processed file at the destination: {}",
                final_output.display()
            )
        })?;

        Ok(())
    })();

    if result.is_err() {
        log_progress(
            app,
            job_id,
            format!(
                "Error while copying/finalising. Attempting to delete destination temporary file: {}",
                destination_temp.display()
            ),
        );
        let _ = fs::remove_file(&destination_temp);
    }

    result
}

fn replace_original_file_from_local_temp(
    app: &AppHandle,
    job_id: &str,
    original: &Path,
    local_replacement: &Path,
) -> Result<()> {
    let destination_temp = temporary_replacement_path(original, "local-copy");
    let result = (|| {
        remove_file_if_exists(&destination_temp)?;

        log_progress(
            app,
            job_id,
            format!(
                "Copying local temporary file next to the original. Source: {} | Destination temporary file: {}",
                local_replacement.display(),
                destination_temp.display()
            ),
        );
        fs::copy(local_replacement, &destination_temp).with_context(|| {
            format!(
                "Could not copy the local temporary file next to the original: {}",
                original.display()
            )
        })?;
        log_progress(
            app,
            job_id,
            "Copy next to the original completed; starting replacement".to_string(),
        );

        replace_original_file(original, &destination_temp)
    })();

    if result.is_err() {
        log_progress(
            app,
            job_id,
            format!(
                "Error during replacement. Attempting to delete destination temporary file: {}",
                destination_temp.display()
            ),
        );
        let _ = fs::remove_file(&destination_temp);
    }

    result
}

fn remove_file_if_exists(path: &Path) -> Result<()> {
    if path.exists() {
        fs::remove_file(path)
            .with_context(|| format!("Could not delete the temporary file: {}", path.display()))?;
    }

    Ok(())
}

fn cleanup_stale_local_copy_temps(final_output: &Path) -> Result<()> {
    let Some(parent) = final_output.parent() else {
        return Ok(());
    };
    let Some(file_name) = final_output
        .file_name()
        .and_then(|file_name| file_name.to_str())
    else {
        return Ok(());
    };
    let stem = final_output
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or(file_name);
    let hidden_prefix = format!(".{stem}.trackforge-");
    let legacy_prefix = format!("{stem}.trackforge-");

    for entry in fs::read_dir(parent).with_context(|| {
        format!(
            "Could not read the destination folder: {}",
            parent.display()
        )
    })? {
        let entry = entry?;
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };

        if (name.starts_with(&hidden_prefix) || name.starts_with(&legacy_prefix))
            && name.contains(".local-copy.")
        {
            let _ = fs::remove_file(path);
        }
    }

    Ok(())
}

fn format_bytes(bytes: u64) -> String {
    const GIB: f64 = 1024.0 * 1024.0 * 1024.0;
    const MIB: f64 = 1024.0 * 1024.0;

    if bytes >= 1024 * 1024 * 1024 {
        format!("{:.1} GB", bytes as f64 / GIB)
    } else {
        format!("{:.0} MB", bytes as f64 / MIB)
    }
}

fn log_progress(app: &AppHandle, job_id: &str, message: String) {
    log_line(app, format!("job={job_id} {message}"));
    let _ = app.emit(
        "trackforge://progress",
        JobProgress {
            job_id: job_id.to_string(),
            progress: None,
            eta_seconds: None,
            processed_seconds: 0.0,
            stage: None,
            log: Some(message),
        },
    );
}

fn prevent_sleep_during_processing(app: &AppHandle, job_id: &str) -> Option<keepawake::KeepAwake> {
    match keepawake::Builder::default()
        .idle(true)
        .reason("TrackForge is processing media files")
        .app_name("TrackForge")
        .app_reverse_domain("com.trackforge.app")
        .create()
    {
        Ok(inhibitor) => {
            log_progress(app, job_id, "Sleep prevention enabled".to_string());
            Some(inhibitor)
        }
        Err(error) => {
            log_progress(
                app,
                job_id,
                format!("WARNING: Could not prevent system sleep: {error}"),
            );
            None
        }
    }
}

fn log_line(app: &AppHandle, message: impl AsRef<str>) {
    let Ok(path) = current_log_file_path(app) else {
        return;
    };

    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }

    if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(file, "[{}] {}", timestamp_millis(), message.as_ref());
    }
}

fn session_started_at() -> u64 {
    *SESSION_STARTED_AT.get_or_init(timestamp_millis)
}

fn current_log_file_path(app: &AppHandle) -> Result<PathBuf> {
    Ok(log_directory(app)?.join(format!("trackforge-session-{}.log", session_started_at())))
}

fn log_directory(app: &AppHandle) -> Result<PathBuf> {
    Ok(app.path().app_log_dir()?)
}

fn log_path_for_session(app: &AppHandle, session_id: &str) -> Result<(PathBuf, u64, bool)> {
    if session_id == "legacy" {
        let path = log_directory(app)?.join("trackforge.log");
        let started_at = file_modified_millis(&path).unwrap_or(0);
        return Ok((path, started_at, false));
    }

    let started_at = session_id
        .parse::<u64>()
        .with_context(|| format!("Invalid log session identifier: {session_id}"))?;
    let current = session_started_at();
    Ok((
        log_directory(app)?.join(format!("trackforge-session-{started_at}.log")),
        started_at,
        started_at == current,
    ))
}

fn read_log_session_inner(app: &AppHandle, session_id: &str) -> Result<LogFileDto> {
    let (path, started_at, is_current) = log_path_for_session(app, session_id)?;
    if !is_current && !path.is_file() {
        bail!("The requested log session no longer exists.");
    }

    let contents = fs::read_to_string(&path).unwrap_or_default();
    Ok(LogFileDto {
        session_id: session_id.to_string(),
        started_at,
        is_current,
        path: path.display().to_string(),
        entries: parse_log_entries(&contents),
    })
}

fn parse_log_entries(contents: &str) -> Vec<LogEntryDto> {
    contents
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            let (timestamp, mut message) = parse_log_line(line);
            if let Some(rest) = message.strip_prefix("job=") {
                if let Some((_, job_message)) = rest.split_once(' ') {
                    message = job_message.to_string();
                }
            }
            let level = if message.contains("ERROR") || message.starts_with("Error") {
                "error"
            } else if message.contains("completed")
                || message.contains("successfully")
                || message.contains("completado")
                || message.contains("terminado correctamente")
                || message.contains("completada")
            {
                "success"
            } else {
                "info"
            };

            LogEntryDto {
                timestamp,
                level: level.to_string(),
                message,
            }
        })
        .collect()
}

fn parse_log_line(line: &str) -> (u64, String) {
    let Some(rest) = line.strip_prefix('[') else {
        return (0, line.to_string());
    };
    let Some((timestamp, message)) = rest.split_once("] ") else {
        return (0, line.to_string());
    };

    (timestamp.parse().unwrap_or(0), message.to_string())
}

fn log_sessions(app: &AppHandle) -> Result<Vec<LogSessionDto>> {
    let directory = log_directory(app)?;
    fs::create_dir_all(&directory)
        .with_context(|| format!("Could not create the log folder: {}", directory.display()))?;

    let current = session_started_at();
    let mut sessions = Vec::new();
    for entry in fs::read_dir(&directory)
        .with_context(|| format!("Could not read the log folder: {}", directory.display()))?
    {
        let entry = entry?;
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };

        if name == "trackforge.log" {
            sessions.push(LogSessionDto {
                id: "legacy".to_string(),
                started_at: file_modified_millis(&path).unwrap_or(0),
                is_current: false,
                size_bytes: entry.metadata().map(|metadata| metadata.len()).unwrap_or(0),
            });
            continue;
        }

        let Some(started_at) = name
            .strip_prefix("trackforge-session-")
            .and_then(|name| name.strip_suffix(".log"))
            .and_then(|value| value.parse::<u64>().ok())
        else {
            continue;
        };
        sessions.push(LogSessionDto {
            id: started_at.to_string(),
            started_at,
            is_current: started_at == current,
            size_bytes: entry.metadata().map(|metadata| metadata.len()).unwrap_or(0),
        });
    }

    if !sessions.iter().any(|session| session.is_current) {
        sessions.push(LogSessionDto {
            id: current.to_string(),
            started_at: current,
            is_current: true,
            size_bytes: 0,
        });
    }
    sessions.sort_by(|left, right| right.started_at.cmp(&left.started_at));
    Ok(sessions)
}

fn clear_log_files(app: &AppHandle) -> Result<()> {
    let directory = log_directory(app)?;
    if !directory.exists() {
        return Ok(());
    }

    for entry in fs::read_dir(&directory)
        .with_context(|| format!("Could not read the log folder: {}", directory.display()))?
    {
        let entry = entry?;
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if name == "trackforge.log"
            || (name.starts_with("trackforge-session-") && name.ends_with(".log"))
        {
            fs::remove_file(&path)
                .with_context(|| format!("Could not delete the log: {}", path.display()))?;
        }
    }
    Ok(())
}

fn file_modified_millis(path: &Path) -> Option<u64> {
    fs::metadata(path)
        .ok()?
        .modified()
        .ok()?
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|duration| duration.as_millis() as u64)
}

fn history_file_path(app: &AppHandle) -> Result<PathBuf> {
    Ok(app.path().app_data_dir()?.join("operation-history.json"))
}

fn read_history(app: &AppHandle) -> Result<Vec<HistoryEntry>> {
    let path = history_file_path(app)?;
    if !path.exists() {
        return Ok(Vec::new());
    }

    let contents = fs::read_to_string(&path)
        .with_context(|| format!("Could not read the history: {}", path.display()))?;
    if contents.trim().is_empty() {
        return Ok(Vec::new());
    }

    serde_json::from_str(&contents).with_context(|| {
        format!(
            "The history does not contain valid JSON: {}",
            path.display()
        )
    })
}

fn write_history(app: &AppHandle, history: &[HistoryEntry]) -> Result<()> {
    let path = history_file_path(app)?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| {
            format!("Could not create the history folder: {}", parent.display())
        })?;
    }

    let contents = serde_json::to_vec_pretty(history)?;
    fs::write(&path, contents)
        .with_context(|| format!("Could not save the history: {}", path.display()))
}

fn timestamp_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

fn backup_path(input: &Path) -> PathBuf {
    let parent = input.parent().unwrap_or_else(|| Path::new("."));
    let file_name = input
        .file_name()
        .and_then(|file_name| file_name.to_str())
        .unwrap_or("video");

    parent.join(format!("{file_name}.trackforge-backup"))
}

fn replace_original_file(original: &Path, replacement: &Path) -> Result<()> {
    if original == replacement {
        bail!("The temporary output path cannot be the same as the original file path.")
    }

    let backup = backup_path(original);
    if backup.exists() {
        fs::remove_file(&backup).with_context(|| {
            format!(
                "Could not delete the previous backup before replacement: {}",
                backup.display()
            )
        })?;
    }

    fs::rename(original, &backup).with_context(|| {
        format!(
            "Could not prepare the original file for replacement. Check that the file is not open: {}",
            original.display()
        )
    })?;

    if let Err(error) = fs::rename(replacement, original) {
        let _ = fs::rename(&backup, original);
        bail!(
            "Could not place the processed file at the original path. An attempt was made to restore the original. Error: {error}"
        );
    }

    fs::remove_file(&backup).with_context(|| {
        format!(
            "Replacement completed, but the temporary backup could not be deleted: {}",
            backup.display()
        )
    })?;

    Ok(())
}

fn error_string(error: impl std::fmt::Display) -> String {
    error.to_string()
}

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            log_line(app.handle(), "Session started");
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            analyze_files,
            pick_video_files,
            process_file,
            read_log,
            list_log_sessions,
            read_log_session,
            clear_logs,
            list_history,
            save_history_entry,
            clear_history,
            convert_audio,
            delete_audio
        ])
        .run(tauri::generate_context!())
        .expect("error while running TrackForge");
}

#[cfg(test)]
mod log_tests {
    use super::{parse_log_entries, validate_selected_video_paths};

    #[test]
    fn parses_timestamp_and_hides_internal_job_id() {
        let entries = parse_log_entries("[1720000000123] job=abc-123 FFmpeg progreso 20%\n");

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].timestamp, 1_720_000_000_123);
        assert_eq!(entries[0].message, "FFmpeg progreso 20%");
        assert_eq!(entries[0].level, "info");
    }

    #[test]
    fn marks_error_and_success_entries() {
        let entries = parse_log_entries(
            "[1720000000123] job=one ERROR: fallo de prueba\n[1720000000456] job=one Proceso completado\n",
        );

        assert_eq!(entries[0].level, "error");
        assert_eq!(entries[1].level, "success");
    }

    #[test]
    fn validates_selected_video_extensions_case_insensitively() {
        let valid = validate_selected_video_paths(vec![
            "/tmp/video.MKV".to_string(),
            "/tmp/otro.mp4".to_string(),
        ]);
        let invalid = validate_selected_video_paths(vec!["/tmp/notas.txt".to_string()]);

        assert!(valid.is_ok());
        assert!(invalid.is_err());
    }
}
