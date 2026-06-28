use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager};

#[allow(dead_code)]
#[path = "../../src/ffmpeg.rs"]
mod ffmpeg;
#[allow(dead_code)]
#[path = "../../src/ffprobe.rs"]
mod ffprobe;
#[allow(dead_code)]
#[path = "../../src/tools.rs"]
mod tools;

use ffmpeg::{AudioTitleUpdate, ConvertOptions, DeleteAudioOptions, TargetFormat, TrackMode};
use tools::ToolPaths;

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
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct JobResult {
    output: String,
    replaced_original: bool,
}

#[tauri::command]
fn analyze_files(paths: Vec<String>) -> Result<Vec<VideoFileDto>, String> {
    let tools = discover_tools(None).map_err(error_string)?;

    paths
        .into_iter()
        .map(|path| analyze_file(PathBuf::from(path), &tools).map_err(error_string))
        .collect()
}

#[tauri::command]
fn convert_audio(app: AppHandle, request: ConvertRequest) -> Result<JobResult, String> {
    convert_audio_inner(app, request).map_err(error_string)
}

#[tauri::command]
fn delete_audio(app: AppHandle, request: DeleteAudioRequest) -> Result<JobResult, String> {
    delete_audio_inner(app, request).map_err(error_string)
}

fn analyze_file(path: PathBuf, tools: &ToolPaths) -> Result<VideoFileDto> {
    if !path.exists() {
        bail!("No existe el archivo: {}", path.display());
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
    })
}

fn convert_audio_inner(app: AppHandle, request: ConvertRequest) -> Result<JobResult> {
    let input = PathBuf::from(&request.input);
    let tools = discover_tools(Some(&app))?;
    let probe = ffprobe::probe_file(&input, &tools)?;
    validate_audio_ordinals(&probe, &request.titles)?;
    validate_default_audio_ordinal(&probe, request.default_audio_ordinal)?;
    let tracks = ffprobe::audio_tracks(&probe);
    let track = tracks
        .iter()
        .find(|track| track.stream_index == request.audio_index)
        .with_context(|| format!("No hay ninguna pista de audio con stream index {}", request.audio_index))?;

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
    if request.audio_indices.is_empty()
        && request.titles.is_empty()
        && request.default_audio_ordinal.is_none()
    {
        bail!("Selecciona pistas para eliminar o cambia algun metadato de audio.");
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

fn discover_tools(app: Option<&AppHandle>) -> Result<ToolPaths> {
    if let Some(app) = app {
        if let Ok(resource_dir) = app.path().resource_dir() {
            let ffmpeg = resource_dir.join("vendor").join("ffmpeg").join("bin").join("ffmpeg.exe");
            let ffprobe = resource_dir.join("vendor").join("ffmpeg").join("bin").join("ffprobe.exe");

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
            bail!("No hay ninguna pista de audio con stream index {stream_index}");
        }
    }

    Ok(())
}

fn validate_audio_ordinals(probe: &ffprobe::Probe, titles: &[AudioTitleRequest]) -> Result<()> {
    let audio_count = ffprobe::audio_tracks(probe).len();

    for title in titles {
        if title.audio_ordinal >= audio_count {
            bail!("No hay ninguna pista de audio con ordinal {}", title.audio_ordinal);
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
        bail!("No hay ninguna pista de audio con ordinal {ordinal}");
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
        "{stem}.trackforge-{}.{}.{}",
        process::id(),
        slug,
        extension
    ))
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
        bail!("La ruta temporal de salida no puede ser la misma que la del archivo original.")
    }

    let backup = backup_path(original);
    if backup.exists() {
        fs::remove_file(&backup).with_context(|| {
            format!(
                "No se pudo eliminar el backup anterior antes de reemplazar: {}",
                backup.display()
            )
        })?;
    }

    fs::rename(original, &backup).with_context(|| {
        format!(
            "No se pudo preparar el reemplazo del original. Comprueba que el archivo no esta abierto: {}",
            original.display()
        )
    })?;

    if let Err(error) = fs::rename(replacement, original) {
        let _ = fs::rename(&backup, original);
        bail!(
            "No se pudo poner el archivo procesado en la ruta original. Se intento restaurar el original. Error: {error}"
        );
    }

    fs::remove_file(&backup).with_context(|| {
        format!(
            "El reemplazo termino, pero no se pudo eliminar el backup temporal: {}",
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
        .invoke_handler(tauri::generate_handler![
            analyze_files,
            convert_audio,
            delete_audio
        ])
        .run(tauri::generate_context!())
        .expect("error while running TrackForge");
}
