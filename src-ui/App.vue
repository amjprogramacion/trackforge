<script setup lang="ts">
import { computed, onMounted, ref } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { open } from "@tauri-apps/plugin-dialog";
import {
  Check,
  Eraser,
  FileVideo,
  FolderOpen,
  Loader2,
  Music2,
  Pencil,
  RefreshCcw,
  Trash2,
  Wand2,
} from "lucide-vue-next";

type AudioTrack = {
  streamIndex: number;
  audioOrdinal: number;
  codec: string;
  channels?: number;
  sampleRate?: string;
  bitRate?: string;
  language?: string;
  title?: string;
  isDefault: boolean;
  label: string;
};

type VideoFile = {
  path: string;
  fileName: string;
  durationSeconds?: number;
  audioTracks: AudioTrack[];
};

type Mode = "replace" | "add";
type Operation = "convert" | "delete" | "metadata";
type Status = "idle" | "running" | "done" | "error";

type JobProgress = {
  jobId: string;
  progress?: number;
  etaSeconds?: number;
  processedSeconds: number;
};

type FileState = VideoFile & {
  id: string;
  selectedAudioIndex?: number;
  selectedDeleteIndices: number[];
  defaultAudioOrdinal?: number;
  originalDefaultAudioOrdinal?: number;
  titleDrafts: Record<number, string>;
  originalTitles: Record<number, string>;
  status: Status;
  progress?: number;
  etaSeconds?: number;
  message?: string;
  output?: string;
};

const files = ref<FileState[]>([]);
const activeFileId = ref<string | undefined>();
const operation = ref<Operation>("convert");
const targetFormat = ref("ac3");
const mode = ref<Mode>("add");
const makeDefault = ref(true);
const replaceOriginal = ref(false);
const isPicking = ref(false);
const isProcessing = ref(false);
const globalError = ref<string | undefined>();

const activeFile = computed(() => files.value.find((file) => file.id === activeFileId.value));
const hasFiles = computed(() => files.value.length > 0);
const canProcess = computed(() => {
  if (!hasFiles.value || isProcessing.value) return false;

  return files.value.some((file) => {
    if (operation.value === "convert") return file.selectedAudioIndex !== undefined;
    if (operation.value === "delete") return file.selectedDeleteIndices.length > 0;
    return hasMetadataChanges(file);
  });
});

onMounted(async () => {
  await listen<JobProgress>("trackforge://progress", (event) => {
    const file = files.value.find((item) => item.id === event.payload.jobId);
    if (!file) return;

    file.progress = event.payload.progress;
    file.etaSeconds = event.payload.etaSeconds;
    file.status = "running";
  });
});

async function pickFiles() {
  globalError.value = undefined;
  isPicking.value = true;

  try {
    const selected = await open({
      multiple: true,
      directory: false,
      filters: [
        {
          name: "Video",
          extensions: ["mkv", "mp4", "mov", "avi", "webm", "m4v"],
        },
      ],
    });

    const paths = Array.isArray(selected) ? selected : selected ? [selected] : [];
    if (paths.length === 0) return;

    const analyzed = await invoke<VideoFile[]>("analyze_files", { paths });
    const nextFiles = analyzed.map(toFileState);
    const existingPaths = new Set(files.value.map((file) => file.path));
    files.value.push(...nextFiles.filter((file) => !existingPaths.has(file.path)));
    activeFileId.value ||= files.value[0]?.id;
  } catch (error) {
    globalError.value = String(error);
  } finally {
    isPicking.value = false;
  }
}

function toFileState(file: VideoFile): FileState {
  const audioTracks = file.audioTracks ?? [];
  const defaultAudioOrdinal = audioTracks.find((track) => track.isDefault)?.audioOrdinal;
  const titleDrafts = Object.fromEntries(
    audioTracks.map((track) => [track.audioOrdinal, track.title?.trim() ?? ""]),
  );

  return {
    ...file,
    audioTracks,
    id: crypto.randomUUID(),
    selectedAudioIndex: audioTracks[0]?.streamIndex,
    selectedDeleteIndices: [],
    defaultAudioOrdinal,
    originalDefaultAudioOrdinal: defaultAudioOrdinal,
    titleDrafts,
    originalTitles: { ...titleDrafts },
    status: "idle",
  };
}

function mergeAnalyzedFile(file: FileState, analyzed: VideoFile) {
  const refreshed = toFileState(analyzed);
  file.path = refreshed.path;
  file.fileName = refreshed.fileName;
  file.durationSeconds = refreshed.durationSeconds;
  file.audioTracks = refreshed.audioTracks;
  file.selectedAudioIndex = refreshed.selectedAudioIndex;
  file.selectedDeleteIndices = [];
  file.defaultAudioOrdinal = refreshed.defaultAudioOrdinal;
  file.originalDefaultAudioOrdinal = refreshed.originalDefaultAudioOrdinal;
  file.titleDrafts = refreshed.titleDrafts;
  file.originalTitles = refreshed.originalTitles;
}

function removeFile(id: string) {
  files.value = files.value.filter((file) => file.id !== id);
  if (activeFileId.value === id) {
    activeFileId.value = files.value[0]?.id;
  }
}

function toggleDeleteTrack(file: FileState, streamIndex: number) {
  if (file.selectedDeleteIndices.includes(streamIndex)) {
    file.selectedDeleteIndices = file.selectedDeleteIndices.filter((index) => index !== streamIndex);
  } else {
    file.selectedDeleteIndices = [...file.selectedDeleteIndices, streamIndex];
    const deletedTrack = file.audioTracks.find((track) => track.streamIndex === streamIndex);
    if (deletedTrack?.audioOrdinal === file.defaultAudioOrdinal) {
      file.defaultAudioOrdinal = file.audioTracks.find(
        (track) => !file.selectedDeleteIndices.includes(track.streamIndex),
      )?.audioOrdinal;
    }
  }
}

function changedTitles(file: FileState) {
  return file.audioTracks
    .filter((track) => (file.titleDrafts[track.audioOrdinal] ?? "") !== (file.originalTitles[track.audioOrdinal] ?? ""))
    .map((track) => ({
      audioOrdinal: track.audioOrdinal,
      title: file.titleDrafts[track.audioOrdinal] ?? "",
    }));
}

function defaultAudioOrdinalForRequest(file: FileState) {
  return file.defaultAudioOrdinal !== file.originalDefaultAudioOrdinal
    ? file.defaultAudioOrdinal
    : undefined;
}

function hasMetadataChanges(file: FileState) {
  return changedTitles(file).length > 0 || defaultAudioOrdinalForRequest(file) !== undefined;
}

function trackHasMetadataChange(file: FileState, track: AudioTrack) {
  return (
    (file.titleDrafts[track.audioOrdinal] ?? "") !== (file.originalTitles[track.audioOrdinal] ?? "") ||
    (file.defaultAudioOrdinal === track.audioOrdinal && file.defaultAudioOrdinal !== file.originalDefaultAudioOrdinal)
  );
}

async function refreshFileFromDisk(file: FileState) {
  const analyzed = await invoke<VideoFile[]>("analyze_files", { paths: [file.path] });
  const [refreshed] = analyzed;
  if (refreshed) {
    mergeAnalyzedFile(file, refreshed);
  }
}

async function processQueue() {
  if (!canProcess.value) return;

  isProcessing.value = true;
  globalError.value = undefined;

  for (const file of files.value) {
    if (operation.value === "convert" && file.selectedAudioIndex === undefined) continue;
    if (operation.value === "delete" && file.selectedDeleteIndices.length === 0) continue;
    if (operation.value === "metadata" && !hasMetadataChanges(file)) continue;

    file.status = "running";
    file.progress = 0;
    file.etaSeconds = undefined;
    file.message = undefined;
    file.output = undefined;

    try {
      const result = operation.value === "convert"
        ? await invoke<{ output: string; replacedOriginal: boolean }>("convert_audio", {
            request: {
              jobId: file.id,
              input: file.path,
              audioIndex: file.selectedAudioIndex,
              format: targetFormat.value,
              mode: mode.value,
              makeDefault: makeDefault.value,
              replaceOriginal: replaceOriginal.value,
              defaultAudioOrdinal: makeDefault.value ? undefined : defaultAudioOrdinalForRequest(file),
              titles: changedTitles(file),
            },
          })
        : await invoke<{ output: string; replacedOriginal: boolean }>("delete_audio", {
            request: {
              jobId: file.id,
              input: file.path,
              audioIndices: operation.value === "delete" ? file.selectedDeleteIndices : [],
              replaceOriginal: replaceOriginal.value,
              defaultAudioOrdinal: defaultAudioOrdinalForRequest(file),
              titles: changedTitles(file),
            },
          });

      file.status = "done";
      file.progress = 1;
      file.output = result.output;
      file.message = result.replacedOriginal ? "Original reemplazado" : "Archivo creado";
      if (result.replacedOriginal) {
        await refreshFileFromDisk(file);
        file.status = "done";
        file.progress = 1;
        file.output = result.output;
        file.message = "Original reemplazado y reescaneado";
      }
    } catch (error) {
      file.status = "error";
      file.message = String(error);
      globalError.value = String(error);
    }
  }

  isProcessing.value = false;
}

function formatDuration(seconds?: number) {
  if (!seconds || !Number.isFinite(seconds)) return "Duracion no disponible";
  const rounded = Math.round(seconds);
  const hours = Math.floor(rounded / 3600);
  const minutes = Math.floor((rounded % 3600) / 60);
  const rest = rounded % 60;

  if (hours > 0) return `${hours}h ${String(minutes).padStart(2, "0")}m`;
  return `${minutes}m ${String(rest).padStart(2, "0")}s`;
}

function formatBitrate(bitRate?: string) {
  if (!bitRate) return "";
  const kbps = Math.round(Number(bitRate) / 1000);
  return Number.isFinite(kbps) && kbps > 0 ? `${kbps} kbps` : "";
}

function trackLanguage(track: AudioTrack) {
  return track.language?.trim() || "sin idioma";
}

function trackTitle(track: AudioTrack) {
  return track.title?.trim() || "";
}

function progressLabel(file: FileState) {
  if (file.status === "done") return "Completado";
  if (file.status === "error") return "Error";
  if (file.progress === undefined) return "Pendiente";

  const percent = Math.round(file.progress * 100);
  const eta = file.etaSeconds !== undefined ? ` - ETA ${formatDuration(file.etaSeconds)}` : "";
  return `${percent}%${eta}`;
}
</script>

<template>
  <main class="app-shell">
    <aside class="sidebar">
      <div class="brand">
        <div class="brand-mark"><Music2 :size="20" /></div>
        <div>
          <h1>TrackForge</h1>
          <p>Audio tools for video files</p>
        </div>
      </div>

      <button class="primary-button" type="button" :disabled="isPicking || isProcessing" @click="pickFiles">
        <FolderOpen :size="18" />
        {{ isPicking ? "Leyendo archivos" : "Seleccionar videos" }}
      </button>

      <div class="file-list" v-if="files.length">
        <button
          v-for="file in files"
          :key="file.id"
          type="button"
          class="file-row"
          :class="{ active: file.id === activeFileId }"
          @click="activeFileId = file.id"
        >
          <FileVideo :size="17" />
          <span>{{ file.fileName }}</span>
          <Check v-if="file.status === 'done'" :size="16" class="ok" />
          <Loader2 v-else-if="file.status === 'running'" :size="16" class="spin" />
        </button>
      </div>
    </aside>

    <section class="workspace">
      <header class="topbar">
        <div>
          <p class="eyebrow">Cola de trabajo</p>
          <h2>{{ files.length ? `${files.length} archivo(s) cargado(s)` : "Selecciona uno o varios videos" }}</h2>
        </div>
        <button class="ghost-button" type="button" :disabled="isProcessing || !files.length" @click="files = []">
          <Trash2 :size="17" />
          Limpiar
        </button>
      </header>

      <div v-if="globalError" class="error-box">{{ globalError }}</div>

      <div v-if="!files.length" class="empty-state">
        <FileVideo :size="48" />
        <h3>No hay archivos en la cola</h3>
        <p>Selecciona varios videos y TrackForge detectara sus pistas de audio.</p>
      </div>

      <div v-else class="content-grid">
        <section class="details">
          <div v-if="activeFile" class="file-detail">
            <div class="detail-header">
              <div>
                <p class="eyebrow">Archivo activo</p>
                <h3>{{ activeFile.fileName }}</h3>
                <p class="path">{{ activeFile.path }}</p>
              </div>
              <button class="icon-button" type="button" @click="removeFile(activeFile.id)" title="Quitar de la cola">
                <Trash2 :size="18" />
              </button>
            </div>

            <div class="meta-row">
              <span>{{ formatDuration(activeFile.durationSeconds) }}</span>
              <span>{{ activeFile.audioTracks.length }} pista(s) de audio</span>
              <span>{{ progressLabel(activeFile) }}</span>
            </div>

            <div class="progress-track">
              <div class="progress-fill" :style="{ width: `${Math.round((activeFile.progress ?? 0) * 100)}%` }" />
            </div>

            <div class="track-table">
              <div class="track-head">
                <span>Principal</span>
                <span>Codec</span>
                <span>Idioma y texto</span>
                <span>Detalle</span>
                <span>Uso</span>
              </div>
              <div v-for="track in activeFile.audioTracks" :key="track.streamIndex" class="track-row">
                <span class="default-cell">
                  <label class="default-radio">
                    <input
                      type="radio"
                      :name="`default-${activeFile.id}`"
                      :value="track.audioOrdinal"
                      :disabled="operation === 'delete' && activeFile.selectedDeleteIndices.includes(track.streamIndex)"
                      v-model="activeFile.defaultAudioOrdinal"
                    />
                    <span>{{ activeFile.defaultAudioOrdinal === track.audioOrdinal ? "Default" : "Principal" }}</span>
                  </label>
                </span>
                <span class="codec-cell">
                  <b>{{ track.codec.toUpperCase() }}</b>
                  <small>audio:{{ track.streamIndex }}</small>
                </span>
                <span class="language-cell">
                  <b>{{ trackLanguage(track) }}</b>
                  <input
                    v-model="activeFile.titleDrafts[track.audioOrdinal]"
                    :class="{
                      dirty:
                        (activeFile.titleDrafts[track.audioOrdinal] ?? '') !==
                        (activeFile.originalTitles[track.audioOrdinal] ?? ''),
                    }"
                    class="title-input"
                    type="text"
                    placeholder="Texto de pista"
                  />
                </span>
                <span class="detail-cell">
                  <b>{{ track.channels ?? "?" }}ch</b>
                  <small>
                    <template v-if="track.sampleRate">{{ track.sampleRate }}Hz</template>
                    <template v-if="formatBitrate(track.bitRate)"> - {{ formatBitrate(track.bitRate) }}</template>
                  </small>
                </span>
                <span class="track-actions">
                  <label v-if="operation === 'convert'" class="radio-row">
                    <input
                      type="radio"
                      :name="`convert-${activeFile.id}`"
                      :value="track.streamIndex"
                      v-model="activeFile.selectedAudioIndex"
                    />
                    Convertir
                  </label>
                  <label v-else-if="operation === 'delete'" class="checkbox-row">
                    <input
                      type="checkbox"
                      :checked="activeFile.selectedDeleteIndices.includes(track.streamIndex)"
                      @change="toggleDeleteTrack(activeFile, track.streamIndex)"
                    />
                    Borrar
                  </label>
                  <span v-else class="change-state" :class="{ dirty: trackHasMetadataChange(activeFile, track) }">
                    {{ trackHasMetadataChange(activeFile, track) ? "Editado" : "Sin cambios" }}
                  </span>
                </span>
              </div>
            </div>

            <div v-if="activeFile.message" class="file-message" :class="activeFile.status">
              {{ activeFile.message }}
              <template v-if="activeFile.output"> - {{ activeFile.output }}</template>
            </div>
          </div>
        </section>

        <aside class="action-panel">
          <div class="segmented">
            <button type="button" :class="{ active: operation === 'convert' }" @click="operation = 'convert'">
              <Wand2 :size="16" />
              Convertir
            </button>
            <button type="button" :class="{ active: operation === 'delete' }" @click="operation = 'delete'">
              <Eraser :size="16" />
              Borrar
            </button>
            <button type="button" :class="{ active: operation === 'metadata' }" @click="operation = 'metadata'">
              <Pencil :size="16" />
              Datos
            </button>
          </div>

          <div v-if="operation === 'convert'" class="control-group">
            <label>
              Formato destino
              <select v-model="targetFormat">
                <option value="aac">AAC</option>
                <option value="ac3">AC3</option>
                <option value="mp3">MP3</option>
                <option value="opus">Opus</option>
                <option value="flac">FLAC</option>
                <option value="wav">WAV / PCM</option>
              </select>
            </label>

            <label>
              Resultado
              <select v-model="mode">
                <option value="add">Anadir pista convertida</option>
                <option value="replace">Sustituir pista original</option>
              </select>
            </label>

            <label class="toggle-row strong">
              <input type="checkbox" v-model="makeDefault" />
              Marcar convertida como principal
            </label>
          </div>

          <div v-else-if="operation === 'delete'" class="control-group">
            <p class="hint">
              Marca una o varias pistas en la tabla. TrackForge conservara video, subtitulos y el resto de audios sin recomprimir.
              Si cambias nombres o pista principal, esos datos se guardaran junto con el borrado.
            </p>
          </div>

          <div v-else class="control-group">
            <p class="hint">
              Cambia el texto de una pista o selecciona cual sera la principal en la tabla. No se convertira ni borrara ninguna pista.
            </p>
          </div>

          <label class="toggle-row strong">
            <input type="checkbox" v-model="replaceOriginal" />
            Reemplazar original al terminar
          </label>

          <button class="run-button" type="button" :disabled="!canProcess" @click="processQueue">
            <Loader2 v-if="isProcessing" :size="18" class="spin" />
            <RefreshCcw v-else :size="18" />
            {{ isProcessing ? "Procesando" : "Ejecutar cola" }}
          </button>
        </aside>
      </div>
    </section>
  </main>
</template>
