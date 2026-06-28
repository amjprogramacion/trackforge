# TrackForge

CLI en Rust para detectar pistas de audio dentro de archivos de video, convertir una pista seleccionada y decidir si se sustituye la pista original o se anade como pista nueva.

## FFmpeg portable

TrackForge busca primero una copia portable en `vendor/ffmpeg/bin`. Para descargarla en Windows:

```powershell
powershell -ExecutionPolicy Bypass -File tools\install-ffmpeg-windows.ps1
```

Esto descarga `ffmpeg.exe` y `ffprobe.exe` sin instalarlos en el sistema ni tocar el `PATH`.

Tambien puedes usar una carpeta propia con:

```powershell
$env:TRACKFORGE_FFMPEG_DIR = "C:\ruta\a\ffmpeg\bin"
```

Si no encuentra una copia portable, TrackForge intenta usar `ffmpeg` y `ffprobe` desde el `PATH`.

## Requisitos

- Rust/Cargo

## Uso

Modo interactivo:

```powershell
cargo run
```

La app te pedira, en orden:

- Ruta del archivo de video.
- Si quieres convertir una pista o eliminar pistas de audio.
- Para convertir: pista de audio, formato destino, si quieres sustituir o anadir, y si quieres marcar la convertida como principal/default.
- Para eliminar: una o varias pistas de audio.
- Si quieres reemplazar el archivo original cuando el proceso termine correctamente.

Modo no interactivo:

```powershell
cargo run -- "C:\videos\pelicula.mkv" --audio-index 1 --format ac3 --mode add --make-default -o "C:\videos\pelicula_ac3.mkv"
```

Reemplazar el archivo original solo si la conversion termina bien:

```powershell
cargo run -- "C:\videos\pelicula.mkv" --audio-index 1 --format ac3 --mode replace --replace-original
```

Eliminar una o varias pistas de audio por stream index:

```powershell
cargo run -- "C:\videos\pelicula.mkv" --delete-audio 1,3
```

Eliminar pistas y reemplazar el archivo original solo si termina bien:

```powershell
cargo run -- "C:\videos\pelicula.mkv" --delete-audio 1,3 --replace-original
```

Ver el comando `ffmpeg` sin ejecutar la conversion:

```powershell
cargo run -- "C:\videos\pelicula.mkv" --audio-index 1 --format opus --mode replace --dry-run
```

Formatos soportados por ahora:

- `aac`
- `ac3`
- `mp3`
- `opus`
- `flac`
- `wav`

Al convertir, TrackForge intenta reutilizar el bitrate de la pista original cuando `ffprobe` lo informa. En AC3 se respeta ese valor hasta el limite tecnico de `640k`.

Modos:

- `replace`: sustituye la pista seleccionada por la convertida.
- `add`: conserva todas las pistas originales y anade la convertida al final.

La opcion `--make-default` limpia el flag `default` del resto de pistas de audio y marca la pista convertida como principal.
