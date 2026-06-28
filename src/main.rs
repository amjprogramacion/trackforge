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
    about = "Detecta, convierte y remultiplexa pistas de audio en archivos de video."
)]
struct Cli {
    /// Archivo de video de entrada.
    input: Option<PathBuf>,

    /// Archivo de salida. Si se omite, se crea junto al original con sufijo _trackforge.
    #[arg(short, long)]
    output: Option<PathBuf>,

    /// Indice global de stream de la pista de audio a convertir, tal como lo muestra ffprobe.
    #[arg(long)]
    audio_index: Option<usize>,

    /// Formato de audio destino: aac, ac3, mp3, opus, flac o wav.
    #[arg(short, long)]
    format: Option<TargetFormat>,

    /// Que hacer con la pista convertida: replace o add.
    #[arg(short, long)]
    mode: Option<TrackMode>,

    /// Marca la pista convertida como pista de audio principal/default.
    #[arg(long)]
    make_default: bool,

    /// Elimina una o varias pistas de audio por stream index. Ejemplo: --delete-audio 1,3
    #[arg(long, value_delimiter = ',')]
    delete_audio: Vec<usize>,

    /// Reemplaza el archivo original cuando la conversion termina correctamente.
    #[arg(long)]
    replace_original: bool,

    /// Muestra el comando ffmpeg que se ejecutaria, sin convertir.
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
            Operation::Convert => "Convertir una pista de audio",
            Operation::DeleteAudio => "Eliminar una o varias pistas de audio",
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
        bail!("No se encontraron pistas de audio en {}", input.display());
    }

    println!("Pistas de audio detectadas:");
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
                .with_prompt("Quieres reemplazar el archivo original cuando termine correctamente")
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
        println!("\nComando ffmpeg:");
        println!("{} {}", tools.ffmpeg.display(), args.join(" "));

        if cli.dry_run {
            println!("\nDry run activado: no se ha escrito ningun archivo.");
            if replace_original {
                println!(
                    "Al ejecutar sin --dry-run, el resultado reemplazaria a: {}",
                    input.display()
                );
            }
            return Ok(());
        }

        ffmpeg::delete_audio_tracks(options)?;

        if replace_original {
            replace_original_file(&input, &delete_output)?;
            println!("\nListo: se ha reemplazado {}", input.display());
        } else {
            println!("\nListo: {}", delete_output.display());
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
            .with_context(|| {
                format!("No hay ninguna pista de audio con stream index {stream_index}")
            })?,
        None => {
            let labels = tracks.iter().map(|track| track.label()).collect::<Vec<_>>();
            let selected = Select::with_theme(&theme)
                .with_prompt("Elige la pista de audio")
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
                .with_prompt("Elige el formato destino")
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
                .with_prompt("Que quieres hacer con la pista convertida")
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
            .with_prompt("Quieres que la pista convertida sea la principal/default")
            .default(false)
            .interact()?
    } else {
        false
    };

    let replace_original = if cli.replace_original {
        true
    } else if should_prompt_for_replace_original {
        Confirm::with_theme(&theme)
            .with_prompt("Quieres reemplazar el archivo original cuando termine correctamente")
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
    println!("\nComando ffmpeg:");
    println!("{} {}", tools.ffmpeg.display(), args.join(" "));

    if cli.dry_run {
        println!("\nDry run activado: no se ha escrito ningun archivo.");
        if replace_original {
            println!(
                "Al ejecutar sin --dry-run, el resultado reemplazaria a: {}",
                input.display()
            );
        }
        return Ok(());
    }

    ffmpeg::convert(options)?;

    if replace_original {
        replace_original_file(&input, &conversion_output)?;
        println!("\nListo: se ha reemplazado {}", input.display());
    } else {
        println!("\nListo: {}", conversion_output.display());
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
        .with_prompt("Que quieres hacer")
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
            .with_prompt("Elige las pistas de audio que quieres eliminar")
            .items(&labels)
            .interact()?;

        if selected.is_empty() {
            println!("Selecciona al menos una pista de audio para eliminar.");
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
            bail!("No hay ninguna pista de audio con stream index {stream_index}");
        }
    }

    Ok(indices)
}

fn prompt_input_path(theme: &ColorfulTheme) -> Result<PathBuf> {
    loop {
        let value: String = Input::with_theme(theme)
            .with_prompt("Ruta del archivo de video")
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
        bail!("No existe el archivo de entrada: {}", path.display())
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
            "No se pudo poner el archivo convertido en la ruta original. Se intento restaurar el original. Error: {error}"
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
