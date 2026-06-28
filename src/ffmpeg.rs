use std::io::{BufRead, BufReader, Read, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::thread;
use std::time::Instant;

use anyhow::{Context, Result, bail};

use crate::ffprobe::{AudioTrack, Probe};
use crate::tools::ToolPaths;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TargetFormat {
    Aac,
    Ac3,
    Mp3,
    Opus,
    Flac,
    Wav,
}

impl TargetFormat {
    pub const ALL: [TargetFormat; 6] = [
        TargetFormat::Aac,
        TargetFormat::Ac3,
        TargetFormat::Mp3,
        TargetFormat::Opus,
        TargetFormat::Flac,
        TargetFormat::Wav,
    ];

    pub fn codec(self) -> &'static str {
        match self {
            TargetFormat::Aac => "aac",
            TargetFormat::Ac3 => "ac3",
            TargetFormat::Mp3 => "libmp3lame",
            TargetFormat::Opus => "libopus",
            TargetFormat::Flac => "flac",
            TargetFormat::Wav => "pcm_s16le",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            TargetFormat::Aac => "AAC",
            TargetFormat::Ac3 => "AC3",
            TargetFormat::Mp3 => "MP3",
            TargetFormat::Opus => "Opus",
            TargetFormat::Flac => "FLAC",
            TargetFormat::Wav => "WAV / PCM 16-bit",
        }
    }

    pub fn slug(self) -> &'static str {
        match self {
            TargetFormat::Aac => "aac",
            TargetFormat::Ac3 => "ac3",
            TargetFormat::Mp3 => "mp3",
            TargetFormat::Opus => "opus",
            TargetFormat::Flac => "flac",
            TargetFormat::Wav => "wav",
        }
    }
}

impl std::str::FromStr for TargetFormat {
    type Err = anyhow::Error;

    fn from_str(value: &str) -> Result<Self> {
        match value.to_lowercase().as_str() {
            "aac" => Ok(TargetFormat::Aac),
            "ac3" => Ok(TargetFormat::Ac3),
            "mp3" => Ok(TargetFormat::Mp3),
            "opus" => Ok(TargetFormat::Opus),
            "flac" => Ok(TargetFormat::Flac),
            "wav" | "pcm" => Ok(TargetFormat::Wav),
            _ => bail!("Formato no soportado: {value}. Usa aac, ac3, mp3, opus, flac o wav."),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrackMode {
    Replace,
    Add,
}

impl TrackMode {
    pub fn label(self) -> &'static str {
        match self {
            TrackMode::Replace => "Sustituir la pista original",
            TrackMode::Add => "Anadir la pista convertida y conservar la original",
        }
    }
}

impl std::str::FromStr for TrackMode {
    type Err = anyhow::Error;

    fn from_str(value: &str) -> Result<Self> {
        match value.to_lowercase().as_str() {
            "replace" | "sustituir" | "reemplazar" => Ok(TrackMode::Replace),
            "add" | "anadir" | "agregar" => Ok(TrackMode::Add),
            _ => bail!("Modo no soportado: {value}. Usa replace o add."),
        }
    }
}

pub struct ConvertOptions<'a> {
    pub tools: &'a ToolPaths,
    pub input: &'a Path,
    pub output: &'a Path,
    pub probe: &'a Probe,
    pub track: &'a AudioTrack,
    pub format: TargetFormat,
    pub mode: TrackMode,
    pub make_default: bool,
    pub default_audio_ordinal: Option<usize>,
    pub titles: &'a [AudioTitleUpdate],
}

pub struct DeleteAudioOptions<'a> {
    pub tools: &'a ToolPaths,
    pub input: &'a Path,
    pub output: &'a Path,
    pub probe: &'a Probe,
    pub stream_indices: &'a [usize],
    pub default_audio_ordinal: Option<usize>,
    pub titles: &'a [AudioTitleUpdate],
}

pub struct AudioTitleUpdate {
    pub audio_ordinal: usize,
    pub title: String,
}

#[derive(Debug, Clone)]
pub struct ProgressUpdate {
    pub progress: Option<f64>,
    pub eta_seconds: Option<u64>,
    pub processed_seconds: f64,
}

pub fn convert(options: ConvertOptions<'_>) -> Result<()> {
    convert_with_progress(options, |update| print_progress_update(&update))
}

pub fn convert_with_progress(
    options: ConvertOptions<'_>,
    on_progress: impl FnMut(ProgressUpdate) -> Result<()>,
) -> Result<()> {
    let args = build_args(&options);
    run_ffmpeg(
        options.tools,
        &args,
        options.input,
        options.probe,
        on_progress,
    )
}

pub fn delete_audio_tracks(options: DeleteAudioOptions<'_>) -> Result<()> {
    delete_audio_tracks_with_progress(options, |update| print_progress_update(&update))
}

pub fn delete_audio_tracks_with_progress(
    options: DeleteAudioOptions<'_>,
    on_progress: impl FnMut(ProgressUpdate) -> Result<()>,
) -> Result<()> {
    let args = build_delete_audio_args(&options);
    run_ffmpeg(
        options.tools,
        &args,
        options.input,
        options.probe,
        on_progress,
    )
}

fn run_ffmpeg(
    tools: &ToolPaths,
    args: &[String],
    input: &Path,
    probe: &Probe,
    on_progress: impl FnMut(ProgressUpdate) -> Result<()>,
) -> Result<()> {
    let mut child = Command::new(&tools.ffmpeg)
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("No se pudo ejecutar {}", tools.ffmpeg.display()))?;

    let stdout = child
        .stdout
        .take()
        .context("No se pudo leer el progreso de ffmpeg")?;
    let mut stderr = child
        .stderr
        .take()
        .context("No se pudo leer la salida de error de ffmpeg")?;

    let stderr_handle = thread::spawn(move || {
        let mut buffer = String::new();
        let _ = stderr.read_to_string(&mut buffer);
        buffer
    });

    let input_size = std::fs::metadata(input)
        .ok()
        .map(|metadata| metadata.len())
        .filter(|size| *size > 0);
    collect_progress(
        stdout,
        crate::ffprobe::duration_seconds(probe),
        input_size,
        on_progress,
    )?;

    let status = child
        .wait()
        .with_context(|| "No se pudo esperar a que terminase ffmpeg")?;
    let stderr = stderr_handle.join().unwrap_or_default();

    if status.success() {
        println!();
        Ok(())
    } else {
        if stderr.trim().is_empty() {
            bail!("ffmpeg termino con error.")
        } else {
            bail!("ffmpeg termino con error:\n{stderr}")
        }
    }
}

fn base_args(input: &Path) -> Vec<String> {
    vec![
        "-hide_banner".to_string(),
        "-y".to_string(),
        "-nostats".to_string(),
        "-v".to_string(),
        "error".to_string(),
        "-progress".to_string(),
        "pipe:1".to_string(),
        "-i".to_string(),
        input.display().to_string(),
    ]
}

pub fn build_args(options: &ConvertOptions<'_>) -> Vec<String> {
    let mut args = base_args(options.input);

    let mut converted_output_stream_index = 0usize;
    let mut converted_audio_ordinal = 0usize;
    let mut audio_ordinal = 0usize;

    match options.mode {
        TrackMode::Replace => {
            for (output_stream_index, stream) in options.probe.streams.iter().enumerate() {
                args.extend(["-map".to_string(), format!("0:{}", stream.index)]);

                if stream.index == options.track.stream_index {
                    converted_output_stream_index = output_stream_index;
                    converted_audio_ordinal = audio_ordinal;
                }

                if stream.codec_type == "audio" {
                    audio_ordinal += 1;
                }
            }
        }
        TrackMode::Add => {
            args.extend(["-map".to_string(), "0".to_string()]);
            converted_output_stream_index = options.probe.streams.len();
            converted_audio_ordinal = options
                .probe
                .streams
                .iter()
                .filter(|stream| stream.codec_type == "audio")
                .count();
            args.extend([
                "-map".to_string(),
                format!("0:{}", options.track.stream_index),
            ]);
        }
    }

    args.extend(["-c".to_string(), "copy".to_string()]);
    args.extend([
        format!("-c:{converted_output_stream_index}"),
        options.format.codec().to_string(),
    ]);

    if let Some(bitrate) = target_bitrate(options.format, options.track) {
        args.extend([format!("-b:{converted_output_stream_index}"), bitrate]);
    }

    if options.make_default {
        append_default_disposition(&mut args, converted_audio_ordinal);
    } else if let Some(default_audio_ordinal) = options.default_audio_ordinal {
        append_default_disposition(&mut args, default_audio_ordinal);
    }

    for update in options.titles {
        args.extend([
            format!("-metadata:s:a:{}", update.audio_ordinal),
            format!("title={}", update.title),
        ]);

        if options.mode == TrackMode::Add && update.audio_ordinal == options.track.audio_ordinal {
            args.extend([
                format!("-metadata:s:a:{converted_audio_ordinal}"),
                format!("title={}", update.title),
            ]);
        }
    }

    args.push(options.output.display().to_string());
    args
}

fn target_bitrate(format: TargetFormat, track: &AudioTrack) -> Option<String> {
    if matches!(format, TargetFormat::Flac | TargetFormat::Wav) {
        return None;
    }

    let bit_rate = track.bit_rate.as_deref()?.parse::<u64>().ok()?;
    if bit_rate == 0 {
        None
    } else if format == TargetFormat::Ac3 {
        Some(bit_rate.min(640_000).to_string())
    } else {
        Some(bit_rate.to_string())
    }
}

pub fn build_delete_audio_args(options: &DeleteAudioOptions<'_>) -> Vec<String> {
    let mut args = base_args(options.input);
    let mut input_audio_ordinal = 0usize;
    let mut output_audio_ordinal = 0usize;
    let mut default_output_audio_ordinal = None;

    for stream in &options.probe.streams {
        if stream.codec_type == "audio" {
            if options.stream_indices.contains(&stream.index) {
                input_audio_ordinal += 1;
                continue;
            }

            args.extend(["-map".to_string(), format!("0:{}", stream.index)]);

            if let Some(update) = options
                .titles
                .iter()
                .find(|update| update.audio_ordinal == input_audio_ordinal)
            {
                args.extend([
                    format!("-metadata:s:a:{output_audio_ordinal}"),
                    format!("title={}", update.title),
                ]);
            }

            if options.default_audio_ordinal == Some(input_audio_ordinal) {
                default_output_audio_ordinal = Some(output_audio_ordinal);
            }

            input_audio_ordinal += 1;
            output_audio_ordinal += 1;
            continue;
        }

        args.extend(["-map".to_string(), format!("0:{}", stream.index)]);
    }

    args.extend(["-c".to_string(), "copy".to_string()]);
    if let Some(default_output_audio_ordinal) = default_output_audio_ordinal {
        append_default_disposition(&mut args, default_output_audio_ordinal);
    }
    args.push(options.output.display().to_string());
    args
}

fn append_default_disposition(args: &mut Vec<String>, audio_ordinal: usize) {
    args.extend(["-disposition:a".to_string(), "0".to_string()]);
    args.extend([
        format!("-disposition:a:{audio_ordinal}"),
        "default".to_string(),
    ]);
}

fn collect_progress<R: Read>(
    reader: R,
    duration_seconds: Option<f64>,
    input_size: Option<u64>,
    mut on_progress: impl FnMut(ProgressUpdate) -> Result<()>,
) -> Result<()> {
    let started_at = Instant::now();
    let mut best_progress = 0.0;

    for line in BufReader::new(reader).lines() {
        let line = line?;

        if let Some(media_time) = parse_progress_time(&line) {
            let update = progress_update(media_time, duration_seconds, started_at);
            if should_emit_progress(&update, &mut best_progress) {
                on_progress(update)?;
            }
        } else if let Some(total_size) = parse_total_size(&line) {
            let Some(input_size) = input_size else {
                continue;
            };
            let update =
                progress_update_from_size(total_size, input_size, duration_seconds, started_at);
            if should_emit_progress(&update, &mut best_progress) {
                on_progress(update)?;
            }
        } else if line == "progress=end" {
            if let Some(duration) = duration_seconds {
                on_progress(progress_update(duration, duration_seconds, started_at))?;
            } else {
                on_progress(ProgressUpdate {
                    progress: Some(1.0),
                    eta_seconds: Some(0),
                    processed_seconds: 0.0,
                })?;
            }
        }
    }

    Ok(())
}

fn should_emit_progress(update: &ProgressUpdate, best_progress: &mut f64) -> bool {
    let Some(progress) = update.progress else {
        return true;
    };

    if progress >= *best_progress {
        *best_progress = progress;
        return true;
    }

    false
}

fn parse_progress_time(line: &str) -> Option<f64> {
    let (key, value) = line.split_once('=')?;
    match key {
        "out_time_us" | "out_time_ms" => value.parse::<f64>().ok().map(|value| value / 1_000_000.0),
        "out_time" => parse_ffmpeg_time(value),
        _ => None,
    }
}

fn parse_total_size(line: &str) -> Option<u64> {
    let (key, value) = line.split_once('=')?;
    if key == "total_size" {
        value.parse::<u64>().ok()
    } else {
        None
    }
}

fn parse_ffmpeg_time(value: &str) -> Option<f64> {
    let mut parts = value.split(':');
    let hours = parts.next()?.parse::<f64>().ok()?;
    let minutes = parts.next()?.parse::<f64>().ok()?;
    let seconds = parts.next()?.parse::<f64>().ok()?;
    Some((hours * 3600.0) + (minutes * 60.0) + seconds)
}

fn progress_update_from_size(
    total_size: u64,
    input_size: u64,
    duration_seconds: Option<f64>,
    started_at: Instant,
) -> ProgressUpdate {
    let progress = (total_size as f64 / input_size as f64).clamp(0.0, 1.0);
    let processed_seconds = duration_seconds.map_or(0.0, |duration| duration * progress);
    eta_progress_update(progress, processed_seconds, started_at)
}

fn progress_update(
    media_time: f64,
    duration_seconds: Option<f64>,
    started_at: Instant,
) -> ProgressUpdate {
    let Some(duration_seconds) = duration_seconds else {
        return ProgressUpdate {
            progress: None,
            eta_seconds: None,
            processed_seconds: media_time,
        };
    };

    let progress = (media_time / duration_seconds).clamp(0.0, 1.0);
    eta_progress_update(progress, media_time, started_at)
}

fn eta_progress_update(
    progress: f64,
    processed_seconds: f64,
    started_at: Instant,
) -> ProgressUpdate {
    if progress <= 0.0 {
        return ProgressUpdate {
            progress: Some(0.0),
            eta_seconds: None,
            processed_seconds,
        };
    }

    let elapsed = started_at.elapsed().as_secs_f64();
    let total_estimated = elapsed / progress;
    let remaining = (total_estimated - elapsed).max(0.0);

    ProgressUpdate {
        progress: Some(progress),
        eta_seconds: Some(remaining.round() as u64),
        processed_seconds,
    }
}

fn print_progress_update(update: &ProgressUpdate) -> Result<()> {
    match (update.progress, update.eta_seconds) {
        (Some(progress), Some(eta_seconds)) => {
            print!(
                "\rProgreso: {:>5.1}% | ETA: {}",
                progress * 100.0,
                format_duration(eta_seconds as f64)
            );
        }
        (Some(_), None) => {
            print!("\rProgreso:   0.0% | ETA: calculando...");
        }
        (None, _) => {
            print!("\rProcesado: {}", format_duration(update.processed_seconds));
        }
    }

    std::io::stdout().flush()?;
    Ok(())
}

fn format_duration(seconds: f64) -> String {
    let total_seconds = seconds.round().max(0.0) as u64;
    let hours = total_seconds / 3600;
    let minutes = (total_seconds % 3600) / 60;
    let seconds = total_seconds % 60;

    if hours > 0 {
        format!("{hours}h {minutes:02}m {seconds:02}s")
    } else {
        format!("{minutes}m {seconds:02}s")
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;
    use crate::ffprobe::{AudioTrack, Probe, Stream};

    #[test]
    fn conversion_uses_source_bitrate_when_available() {
        let tools = ToolPaths {
            ffmpeg: "ffmpeg".into(),
            ffprobe: "ffprobe".into(),
        };
        let probe = Probe {
            streams: vec![Stream {
                index: 1,
                codec_type: "audio".to_string(),
                codec_name: Some("eac3".to_string()),
                channels: Some(6),
                sample_rate: Some("48000".to_string()),
                bit_rate: Some("384000".to_string()),
                tags: None,
                disposition: None,
            }],
            format: None,
        };
        let track = AudioTrack {
            stream_index: 1,
            audio_ordinal: 0,
            codec: "eac3".to_string(),
            channels: Some(6),
            sample_rate: Some("48000".to_string()),
            bit_rate: Some("384000".to_string()),
            language: None,
            title: None,
            is_default: false,
        };
        let options = ConvertOptions {
            tools: &tools,
            input: Path::new("input.mkv"),
            output: Path::new("output.mkv"),
            probe: &probe,
            track: &track,
            format: TargetFormat::Ac3,
            mode: TrackMode::Replace,
            make_default: false,
            default_audio_ordinal: None,
            titles: &[],
        };

        let args = build_args(&options);
        assert!(args.windows(2).any(|pair| pair == ["-b:0", "384000"]));
    }

    #[test]
    fn delete_remaps_default_audio_ordinal() {
        let tools = ToolPaths {
            ffmpeg: "ffmpeg".into(),
            ffprobe: "ffprobe".into(),
        };
        let probe = Probe {
            streams: vec![
                Stream {
                    index: 0,
                    codec_type: "video".to_string(),
                    codec_name: Some("h264".to_string()),
                    channels: None,
                    sample_rate: None,
                    bit_rate: None,
                    tags: None,
                    disposition: None,
                },
                Stream {
                    index: 1,
                    codec_type: "audio".to_string(),
                    codec_name: Some("eac3".to_string()),
                    channels: Some(6),
                    sample_rate: Some("48000".to_string()),
                    bit_rate: Some("640000".to_string()),
                    tags: None,
                    disposition: None,
                },
                Stream {
                    index: 2,
                    codec_type: "audio".to_string(),
                    codec_name: Some("ac3".to_string()),
                    channels: Some(6),
                    sample_rate: Some("48000".to_string()),
                    bit_rate: Some("640000".to_string()),
                    tags: None,
                    disposition: None,
                },
            ],
            format: None,
        };
        let options = DeleteAudioOptions {
            tools: &tools,
            input: Path::new("input.mkv"),
            output: Path::new("output.mkv"),
            probe: &probe,
            stream_indices: &[1],
            default_audio_ordinal: Some(1),
            titles: &[],
        };

        let args = build_delete_audio_args(&options);
        assert!(
            args.windows(2)
                .any(|pair| pair == ["-disposition:a:0", "default"])
        );
    }
}
