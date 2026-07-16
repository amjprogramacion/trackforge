use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result, anyhow, bail};
use serde::Deserialize;

use crate::tools::ToolPaths;

#[derive(Debug, Deserialize)]
pub struct Probe {
    pub streams: Vec<Stream>,
    pub format: Option<Format>,
}

#[derive(Debug, Deserialize)]
pub struct Format {
    pub duration: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct Stream {
    pub index: usize,
    pub codec_type: String,
    pub codec_name: Option<String>,
    pub channels: Option<u32>,
    pub sample_rate: Option<String>,
    pub bit_rate: Option<String>,
    pub tags: Option<Tags>,
    pub disposition: Option<Disposition>,
}

#[derive(Debug, Deserialize)]
pub struct Tags {
    pub language: Option<String>,
    pub title: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct Disposition {
    pub default: Option<u8>,
}

#[derive(Debug, Clone)]
pub struct AudioTrack {
    pub stream_index: usize,
    pub audio_ordinal: usize,
    pub codec: String,
    pub channels: Option<u32>,
    pub sample_rate: Option<String>,
    pub bit_rate: Option<String>,
    pub language: Option<String>,
    pub title: Option<String>,
    pub is_default: bool,
}

impl AudioTrack {
    pub fn label(&self) -> String {
        let mut parts = vec![
            format!("#{} audio:{}", self.audio_ordinal, self.stream_index),
            self.codec.clone(),
        ];

        if let Some(channels) = self.channels {
            parts.push(format!("{channels}ch"));
        }

        if let Some(sample_rate) = &self.sample_rate {
            parts.push(format!("{sample_rate}Hz"));
        }

        if let Some(bit_rate) = &self.bit_rate {
            parts.push(format!(
                "{} kbps",
                bit_rate.parse::<u64>().unwrap_or(0) / 1000
            ));
        }

        if let Some(language) = &self.language {
            parts.push(language.clone());
        }

        if let Some(title) = &self.title {
            parts.push(format!("\"{title}\""));
        }

        if self.is_default {
            parts.push("default".to_string());
        }

        parts.join(" | ")
    }
}

pub fn probe_file(input: &Path, tools: &ToolPaths) -> Result<Probe> {
    let output = Command::new(&tools.ffprobe)
        .args([
            "-v",
            "error",
            "-print_format",
            "json",
            "-show_streams",
            "-show_format",
            input
                .to_str()
                .ok_or_else(|| anyhow!("The file path is not valid UTF-8"))?,
        ])
        .output()
        .with_context(|| format!("Could not run {}", tools.ffprobe.display()))?;

    if !output.status.success() {
        bail!(
            "ffprobe could not read the file:\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    serde_json::from_slice(&output.stdout).context("ffprobe returned invalid JSON")
}

pub fn duration_seconds(probe: &Probe) -> Option<f64> {
    probe
        .format
        .as_ref()
        .and_then(|format| format.duration.as_ref())
        .and_then(|duration| duration.parse::<f64>().ok())
        .filter(|duration| duration.is_finite() && *duration > 0.0)
}

pub fn audio_tracks(probe: &Probe) -> Vec<AudioTrack> {
    probe
        .streams
        .iter()
        .filter(|stream| stream.codec_type == "audio")
        .enumerate()
        .map(|(audio_ordinal, stream)| AudioTrack {
            stream_index: stream.index,
            audio_ordinal,
            codec: stream
                .codec_name
                .clone()
                .unwrap_or_else(|| "unknown".to_string()),
            channels: stream.channels,
            sample_rate: stream.sample_rate.clone(),
            bit_rate: stream.bit_rate.clone(),
            language: stream.tags.as_ref().and_then(|tags| tags.language.clone()),
            title: stream.tags.as_ref().and_then(|tags| tags.title.clone()),
            is_default: stream
                .disposition
                .as_ref()
                .and_then(|disposition| disposition.default)
                .unwrap_or(0)
                == 1,
        })
        .collect()
}
