mod ffmpeg;
mod ffprobe;
mod tools;

use std::path::PathBuf;
use std::{fs, process};

use anyhow::{Context, Result, bail};
use clap::Parser;
use dialoguer::{Confirm, Input, MultiSelect, Select, theme::ColorfulTheme};
use ffmpeg::{ConvertOptions, DeleteAudioOptions, TargetFormat, TrackMode};
use ffprobe::AudioTrack;
use tools::ToolPaths;

#[derive(Debug, Parser)]
#[command(
    author,
    version,
    about = "Detect, convert and remultiplex audio tracks in video files."
)]
struct Cli {
    /// Input video file.
    input: Option<PathBuf>,

    /// Output file. If omitted, it is created next to the original with the _trackforge suffix.
    #[arg(short, long)]
    output: Option<PathBuf>,

    /// Global stream index of the audio track to convert, as shown by ffprobe.
    #[arg(long)]
    audio_index: Option<usize>,

    /// Target audio format: aac, ac3, mp3, opus, flac or wav.
    #[arg(short, long)]
    format: Option<TargetFormat>,

    /// What to do with the converted track: replace or add.
    #[arg(short, long)]
    mode: Option<TrackMode>,

    /// Mark the converted track as the primary/default audio track.
    #[arg(long)]
    make_default: bool,

    /// Delete one or more audio tracks by stream index. Example: --delete-audio 1,3
    #[arg(long, value_delimiter = ',')]
    delete_audio: Vec<usize>,

    /// Replace the original file when conversion completes successfully.
    #[arg(long)]
    replace_original: bool,

    /// Show the ffmpeg command that would be run without converting.
    #[arg(long)]
    dry_run: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Operation {
    Convert,
    DeleteAudio,
}

impl Operation {
    fn label(self) -> &'static str {
        match self {
            Operation::Convert => "Convert an audio track",
            Operation::DeleteAudio => "Delete one or more audio tracks",
        }
    }
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let theme = ColorfulTheme::default();
    let is_guided = cli.input.is_none();

    let input = match cli.input {
        Some(input) => validate_input_path(sanitize_path_buf(input))?,
        None => prompt_input_path(&theme)?,
    };
    let requested_output = cli.output.map(sanitize_path_buf);

    let tools = ToolPaths::discover()?;

    let probe = ffprobe::probe_file(&input, &tools)?;
    let tracks = ffprobe::audio_tracks(&probe);

    if tracks.is_empty() {
        bail!("No audio tracks were found in {}", input.display());
    }

    println!("Detected audio tracks:");
    for track in &tracks {
        println!("  {}", track.label());
    }

    let operation = if !cli.delete_audio.is_empty() {
        Operation::DeleteAudio
    } else if cli.audio_index.is_some() || cli.format.is_some() || cli.mode.is_some() {
        Operation::Convert
    } else {
        prompt_operation(&theme)?
    };

    if operation == Operation::DeleteAudio {
        let selected_stream_indices = if cli.delete_audio.is_empty() {
            prompt_audio_tracks_to_delete(&theme, &tracks)?
        } else {
            validate_audio_indices(&tracks, cli.delete_audio)?
        };

        let replace_original = if cli.replace_original {
            true
        } else if is_guided {
            Confirm::with_theme(&theme)
                .with_prompt("Replace the original file when processing completes successfully")
                .default(false)
                .interact()?
        } else {
            false
        };

        let output = requested_output.unwrap_or_else(|| default_delete_audio_output_path(&input));
        let delete_output = if replace_original {
            temporary_replacement_path(&input, "audio-delete")
        } else {
            output
        };

        let options = DeleteAudioOptions {
            tools: &tools,
            input: &input,
            output: &delete_output,
            probe: &probe,
            stream_indices: &selected_stream_indices,
            default_audio_ordinal: None,
            titles: &[],
        };

        let args = ffmpeg::build_delete_audio_args(&options);
        println!("\nffmpeg command:");
        println!("{} {}", tools.ffmpeg.display(), args.join(" "));

        if cli.dry_run {
            println!("\nDry run enabled: no files were written.");
            if replace_original {
                println!(
                    "When run without --dry-run, the result would replace: {}",
                    input.display()
                );
            }
            return Ok(());
        }

        ffmpeg::delete_audio_tracks(options)?;

        if replace_original {
            replace_original_file(&input, &delete_output)?;
            println!("\nDone: replaced {}", input.display());
        } else {
            println!("\nDone: {}", delete_output.display());
        }

        return Ok(());
    }

    let should_prompt_for_default =
        is_guided || cli.audio_index.is_none() || cli.format.is_none() || cli.mode.is_none();
    let should_prompt_for_replace_original = should_prompt_for_default;

    let track = match cli.audio_index {
        Some(stream_index) => tracks
            .iter()
            .find(|track| track.stream_index == stream_index)
            .with_context(|| format!("No audio track has stream index {stream_index}"))?,
        None => {
            let labels = tracks.iter().map(|track| track.label()).collect::<Vec<_>>();
            let selected = Select::with_theme(&theme)
                .with_prompt("Choose the audio track")
                .items(&labels)
                .default(0)
                .interact()?;
            &tracks[selected]
        }
    };

    let format = match cli.format {
        Some(format) => format,
        None => {
            let formats = TargetFormat::ALL
                .iter()
                .map(|format| format.label())
                .collect::<Vec<_>>();
            let selected = Select::with_theme(&theme)
                .with_prompt("Choose the target format")
                .items(&formats)
                .default(0)
                .interact()?;
            TargetFormat::ALL[selected]
        }
    };

    let mode = match cli.mode {
        Some(mode) => mode,
        None => {
            let modes = [TrackMode::Replace, TrackMode::Add];
            let labels = modes.iter().map(|mode| mode.label()).collect::<Vec<_>>();
            let selected = Select::with_theme(&theme)
                .with_prompt("What do you want to do with the converted track")
                .items(&labels)
                .default(0)
                .interact()?;
            modes[selected]
        }
    };

    let make_default = if cli.make_default {
        true
    } else if should_prompt_for_default {
        Confirm::with_theme(&theme)
            .with_prompt("Make the converted track the primary/default track")
            .default(false)
            .interact()?
    } else {
        false
    };

    let replace_original = if cli.replace_original {
        true
    } else if should_prompt_for_replace_original {
        Confirm::with_theme(&theme)
            .with_prompt("Replace the original file when processing completes successfully")
            .default(false)
            .interact()?
    } else {
        false
    };

    let output = requested_output.unwrap_or_else(|| default_output_path(&input, format));
    let conversion_output = if replace_original {
        temporary_replacement_path(&input, format.slug())
    } else {
        output
    };

    let options = ConvertOptions {
        tools: &tools,
        input: &input,
        output: &conversion_output,
        probe: &probe,
        track,
        format,
        mode,
        make_default,
        default_audio_ordinal: None,
        titles: &[],
    };

    let args = ffmpeg::build_args(&options);
    println!("\nffmpeg command:");
    println!("{} {}", tools.ffmpeg.display(), args.join(" "));

    if cli.dry_run {
        println!("\nDry run enabled: no files were written.");
        if replace_original {
            println!(
                "When run without --dry-run, the result would replace: {}",
                input.display()
            );
        }
        return Ok(());
    }

    ffmpeg::convert(options)?;

    if replace_original {
        replace_original_file(&input, &conversion_output)?;
        println!("\nDone: replaced {}", input.display());
    } else {
        println!("\nDone: {}", conversion_output.display());
    }

    Ok(())
}

fn prompt_operation(theme: &ColorfulTheme) -> Result<Operation> {
    let operations = [Operation::Convert, Operation::DeleteAudio];
    let labels = operations
        .iter()
        .map(|operation| operation.label())
        .collect::<Vec<_>>();
    let selected = Select::with_theme(theme)
        .with_prompt("What do you want to do")
        .items(&labels)
        .default(0)
        .interact()?;

    Ok(operations[selected])
}

fn prompt_audio_tracks_to_delete(
    theme: &ColorfulTheme,
    tracks: &[AudioTrack],
) -> Result<Vec<usize>> {
    loop {
        let labels = tracks.iter().map(|track| track.label()).collect::<Vec<_>>();
        let selected = MultiSelect::with_theme(theme)
            .with_prompt("Choose the audio tracks you want to delete")
            .items(&labels)
            .interact()?;

        if selected.is_empty() {
            println!("Select at least one audio track to delete.");
            continue;
        }

        return Ok(selected
            .into_iter()
            .map(|index| tracks[index].stream_index)
            .collect());
    }
}

fn validate_audio_indices(tracks: &[AudioTrack], indices: Vec<usize>) -> Result<Vec<usize>> {
    for stream_index in &indices {
        if !tracks
            .iter()
            .any(|track| track.stream_index == *stream_index)
        {
            bail!("No audio track has stream index {stream_index}");
        }
    }

    Ok(indices)
}

fn prompt_input_path(theme: &ColorfulTheme) -> Result<PathBuf> {
    loop {
        let value: String = Input::with_theme(theme)
            .with_prompt("Video file path")
            .interact_text()?;

        let sanitized = sanitize_path_input(&value);
        match validate_input_path(PathBuf::from(&sanitized)) {
            Ok(path) => return Ok(path),
            Err(error) => println!("{error}"),
        }
    }
}

fn validate_input_path(path: PathBuf) -> Result<PathBuf> {
    if path.exists() {
        Ok(path)
    } else {
        bail!("The input file does not exist: {}", path.display())
    }
}

fn sanitize_path_buf(path: PathBuf) -> PathBuf {
    PathBuf::from(sanitize_path_input(&path.to_string_lossy()))
}

fn sanitize_path_input(value: &str) -> String {
    value
        .trim()
        .trim_matches('"')
        .trim_matches('\'')
        .chars()
        .filter(|character| !is_invisible_path_marker(*character))
        .collect::<String>()
        .trim()
        .trim_matches('"')
        .trim_matches('\'')
        .to_string()
}

fn is_invisible_path_marker(character: char) -> bool {
    matches!(
        character,
        '\u{200e}'
            | '\u{200f}'
            | '\u{202a}'
            | '\u{202b}'
            | '\u{202c}'
            | '\u{202d}'
            | '\u{202e}'
            | '\u{2066}'
            | '\u{2067}'
            | '\u{2068}'
            | '\u{2069}'
            | '\u{feff}'
    )
}

fn default_output_path(input: &std::path::Path, format: TargetFormat) -> PathBuf {
    let parent = input.parent().unwrap_or_else(|| std::path::Path::new("."));
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

fn default_delete_audio_output_path(input: &std::path::Path) -> PathBuf {
    let parent = input.parent().unwrap_or_else(|| std::path::Path::new("."));
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

fn temporary_replacement_path(input: &std::path::Path, slug: &str) -> PathBuf {
    let parent = input.parent().unwrap_or_else(|| std::path::Path::new("."));
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

fn backup_path(input: &std::path::Path) -> PathBuf {
    let parent = input.parent().unwrap_or_else(|| std::path::Path::new("."));
    let file_name = input
        .file_name()
        .and_then(|file_name| file_name.to_str())
        .unwrap_or("video");

    parent.join(format!("{file_name}.trackforge-backup"))
}

fn replace_original_file(original: &std::path::Path, replacement: &std::path::Path) -> Result<()> {
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
            "Could not place the converted file at the original path. An attempt was made to restore the original. Error: {error}"
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

#[cfg(test)]
mod tests {
    use super::sanitize_path_input;

    #[test]
    fn sanitizes_pasted_windows_paths() {
        assert_eq!(
            sanitize_path_input("\u{202a}\"M:\\PELICULAS\\Bitelchus Bitelchus.mkv\"\u{202c}"),
            "M:\\PELICULAS\\Bitelchus Bitelchus.mkv"
        );
    }
}
