#!/usr/bin/env bash
set -euo pipefail

project_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
bin_dir="$project_root/vendor/ffmpeg/bin"
temp_dir="$project_root/.tmp/ffmpeg-macos-download"

case "$(uname -m)" in
  arm64)
    ffmpeg_arch="arm64"
    ;;
  x86_64)
    ffmpeg_arch="amd64"
    ;;
  *)
    echo "Arquitectura macOS no soportada: $(uname -m)" >&2
    exit 1
    ;;
esac

base_url="https://ffmpeg.martin-riedl.de/redirect/latest/macos/$ffmpeg_arch/snapshot"

rm -rf "$temp_dir"
mkdir -p "$bin_dir" "$temp_dir"

download_tool() {
  local name="$1"
  local archive="$temp_dir/$name.zip"
  local extract_dir="$temp_dir/$name"

  echo "Descargando $name para macOS $ffmpeg_arch..."
  curl --fail --location --show-error --output "$archive" "$base_url/$name.zip"

  mkdir -p "$extract_dir"
  unzip -q "$archive" -d "$extract_dir"

  local executable
  executable="$(find "$extract_dir" -type f -name "$name" -perm -111 | head -n 1)"

  if [[ -z "$executable" ]]; then
    echo "No se encontro $name dentro de $archive" >&2
    exit 1
  fi

  cp "$executable" "$bin_dir/$name"
  chmod +x "$bin_dir/$name"
}

download_tool ffmpeg
download_tool ffprobe

rm -rf "$temp_dir"
echo "Listo. Binarios incluidos para la build:"
echo "  $bin_dir/ffmpeg"
echo "  $bin_dir/ffprobe"
