use std::env;
use std::path::PathBuf;
use std::process::{Command, Stdio};

use anyhow::{Context, Result, bail};

#[derive(Debug, Clone)]
pub struct ToolPaths {
    pub ffmpeg: PathBuf,
    pub ffprobe: PathBuf,
}

impl ToolPaths {
    pub fn discover() -> Result<Self> {
        let ffmpeg = find_tool("ffmpeg")?;
        let ffprobe = find_tool("ffprobe")?;
        Ok(Self { ffmpeg, ffprobe })
    }
}

fn find_tool(name: &str) -> Result<PathBuf> {
    for candidate in candidate_paths(name)? {
        if candidate.is_file() && command_works(&candidate) {
            return Ok(candidate);
        }
    }

    if command_works(name) {
        return Ok(PathBuf::from(name));
    }

    bail!(
        "No se encontro `{name}`. Ejecuta `powershell -ExecutionPolicy Bypass -File tools\\install-ffmpeg-windows.ps1` para descargar la copia portable."
    )
}

fn candidate_paths(name: &str) -> Result<Vec<PathBuf>> {
    let executable_name = if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_string()
    };

    let mut roots = Vec::new();

    if let Ok(custom_dir) = env::var("TRACKFORGE_FFMPEG_DIR") {
        roots.push(PathBuf::from(custom_dir));
    }

    add_root_with_ancestors(
        &mut roots,
        env::current_dir().context("No se pudo leer el directorio actual")?,
    );

    if let Ok(exe_path) = env::current_exe()
        && let Some(exe_dir) = exe_path.parent()
    {
        add_root_with_ancestors(&mut roots, exe_dir.to_path_buf());
    }

    if let Some(manifest_dir) = option_env!("CARGO_MANIFEST_DIR") {
        add_root_with_ancestors(&mut roots, PathBuf::from(manifest_dir));
    }

    let mut candidates = Vec::new();
    for root in roots {
        candidates.push(
            root.join("vendor")
                .join("ffmpeg")
                .join("bin")
                .join(&executable_name),
        );
        candidates.push(root.join("ffmpeg").join("bin").join(&executable_name));
        candidates.push(root.join("bin").join(&executable_name));
        candidates.push(root.join(&executable_name));
    }

    candidates.dedup();
    Ok(candidates)
}

fn add_root_with_ancestors(roots: &mut Vec<PathBuf>, root: PathBuf) {
    roots.push(root.clone());

    for ancestor in root.ancestors().skip(1).take(3) {
        roots.push(ancestor.to_path_buf());
    }
}

fn command_works<S: AsRef<std::ffi::OsStr>>(program: S) -> bool {
    Command::new(program)
        .arg("-version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}
