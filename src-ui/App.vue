<script setup lang="ts">
import { computed, onMounted, ref } from "vue";
import { convertFileSrc, invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import SettingsModal from "./components/SettingsModal.vue";
import { useSettings } from "./useSettings";
import {
  Check,
  Clock3,
  Eraser,
  FileVideo,
  Loader2,
  Pencil,
  ScrollText,
  Settings,
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
  thumbnailPath?: string;
};

type Mode = "replace" | "add";
type Status = "idle" | "running" | "done" | "error";

type JobProgress = {
  jobId: string;
  progress?: number;
  etaSeconds?: number;
  processedSeconds: number;
  stage?: string;
  log?: string;
};

type LogFile = {
  sessionId: string;
  startedAt: number;
  isCurrent: boolean;
  path: string;
  entries: LogEntry[];
};

type LogEntry = {
  timestamp: number;
  level: "info" | "success" | "error";
  message: string;
};

type LogSession = {
  id: string;
  startedAt: number;
  isCurrent: boolean;
  sizeBytes: number;
};

type TimingStep = {
  label: string;
  startedAt: number;
  finishedAt?: number;
  durationMs?: number;
};

type FileTimingSummary = {
  fileName: string;
  inputPath: string;
  outputPath?: string;
  status: "done" | "error";
  error?: string;
  affectedFiles: Array<{
    path: string;
    change: "created" | "modified";
  }>;
  settings: Array<{
    label: string;
    value: string;
  }>;
  steps: Array<{
    label: string;
    durationMs: number;
  }>;
  totalMs: number;
};

type RunSummary = {
  id: string;
  startedAt: string;
  finishedAt: string;
  status: "done" | "partial" | "error";
  files: FileTimingSummary[];
  totalMs: number;
};

type FileState = VideoFile & {
  id: string;
  thumbnailUrl?: string;
  convertEnabled: boolean;
  deleteEnabled: boolean;
  metadataEnabled: boolean;
  selectedAudioIndex?: number;
  selectedDeleteIndices: number[];
  targetFormat: string;
  mode: Mode;
  makeDefault: boolean;
  replaceOriginal: boolean;
  useLocalTemp: boolean;
  defaultAudioOrdinal?: number;
  originalDefaultAudioOrdinal?: number;
  titleDrafts: Record<number, string>;
  originalTitles: Record<number, string>;
  status: Status;
  progress?: number;
  etaSeconds?: number;
  message?: string;
  output?: string;
  runStartedAt?: number;
  runSteps: TimingStep[];
};

const files = ref<FileState[]>([]);
const isSelectingFiles = ref(false);
const isAnalyzingFiles = ref(false);
const isProcessing = ref(false);
const globalError = ref<string | undefined>();
const activeLog = ref<LogFile | undefined>();
const isLogOpen = ref(false);
const logSessions = ref<LogSession[]>([]);
const selectedLogSession = ref<LogSession | undefined>();
const selectedPreviousLog = ref<LogFile | undefined>();
const isPreviousLogsOpen = ref(false);
const isClearingLogs = ref(false);
const hasStoredLogs = ref(false);
const runSummary = ref<RunSummary | undefined>();
const isSummaryOpen = ref(false);
const history = ref<RunSummary[]>([]);
const selectedHistory = ref<RunSummary | undefined>();
const isHistoryOpen = ref(false);
const isClearingHistory = ref(false);
const isSettingsOpen = ref(false);
const { maxConcurrentJobs } = useSettings();

const runningFiles = computed(() => files.value.filter((file) => file.status === "running").length);
const queuedFiles = computed(() => files.value.filter(canProcessFile).length);
const canProcess = computed(() => files.value.some(canProcessFile) && !isProcessing.value);

onMounted(async () => {
  await listen<JobProgress>("trackforge://progress", (event) => {
    const file = files.value.find((item) => item.id === event.payload.jobId);
    if (!file) return;

    if (event.payload.progress !== undefined) {
      file.progress = event.payload.progress;
    }
    if (event.payload.etaSeconds !== undefined) {
      file.etaSeconds = event.payload.etaSeconds;
    }
    if (event.payload.stage) {
      startTimingStep(file, event.payload.stage);
    } else if (event.payload.log) {
      const inferredStep = timingStepFromLog(event.payload.log);
      if (inferredStep) startTimingStep(file, inferredStep);
    } else if (event.payload.progress !== undefined && event.payload.progress > 0) {
      startTimingStep(file, "Processing");
    }
    file.status = "running";
  });

  await loadHistory();
});

async function loadHistory() {
  try {
    history.value = await invoke<RunSummary[]>("list_history");
  } catch (error) {
    globalError.value = `Could not load operation history: ${String(error)}`;
  }
}

async function openHistory() {
  await loadHistory();
  selectedHistory.value = history.value[0];
  isHistoryOpen.value = true;
}

async function clearOperationHistory() {
  if (!window.confirm("Do you want to delete the entire operation history?")) return;

  isClearingHistory.value = true;
  try {
    await invoke("clear_history");
    history.value = [];
    selectedHistory.value = undefined;
  } catch (error) {
    globalError.value = `Could not clear operation history: ${String(error)}`;
  } finally {
    isClearingHistory.value = false;
  }
}

async function pickFiles() {
  globalError.value = undefined;
  isSelectingFiles.value = true;

  try {
    const paths = await invoke<string[]>("pick_video_files");
    if (paths.length === 0) return;

    isSelectingFiles.value = false;
    isAnalyzingFiles.value = true;
    const analyzed = await invoke<VideoFile[]>("analyze_files", { paths });
    const nextFiles = analyzed.map(toFileState);
    const existingPaths = new Set(files.value.map((file) => file.path));
    files.value.push(...nextFiles.filter((file) => !existingPaths.has(file.path)));
  } catch (error) {
    globalError.value = String(error);
  } finally {
    isSelectingFiles.value = false;
    isAnalyzingFiles.value = false;
  }
}

async function openLog() {
  try {
    activeLog.value = await invoke<LogFile>("read_log");
    isLogOpen.value = true;
  } catch (error) {
    globalError.value = String(error);
  }
}

async function openPreviousLogs() {
  try {
    const sessions = await invoke<LogSession[]>("list_log_sessions");
    hasStoredLogs.value = sessions.some((session) => session.sizeBytes > 0);
    logSessions.value = sessions.filter((session) => !session.isCurrent);
    selectedLogSession.value = logSessions.value[0];
    selectedPreviousLog.value = selectedLogSession.value
      ? await invoke<LogFile>("read_log_session", { sessionId: selectedLogSession.value.id })
      : undefined;
    isPreviousLogsOpen.value = true;
  } catch (error) {
    globalError.value = `Could not load previous logs: ${String(error)}`;
  }
}

async function selectLogSession(session: LogSession) {
  selectedLogSession.value = session;
  try {
    selectedPreviousLog.value = await invoke<LogFile>("read_log_session", { sessionId: session.id });
  } catch (error) {
    globalError.value = `Could not open the log session: ${String(error)}`;
  }
}

async function clearAllLogs() {
  if (!window.confirm("Do you want to delete all logs, including the current session log?")) return;

  isClearingLogs.value = true;
  try {
    await invoke("clear_logs");
    logSessions.value = [];
    selectedLogSession.value = undefined;
    selectedPreviousLog.value = undefined;
    hasStoredLogs.value = false;
    if (activeLog.value) activeLog.value.entries = [];
  } catch (error) {
    globalError.value = `Could not delete the logs: ${String(error)}`;
  } finally {
    isClearingLogs.value = false;
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
    thumbnailUrl: file.thumbnailPath ? convertFileSrc(file.thumbnailPath) : undefined,
    convertEnabled: false,
    deleteEnabled: false,
    metadataEnabled: false,
    selectedAudioIndex: audioTracks[0]?.streamIndex,
    selectedDeleteIndices: [],
    targetFormat: "ac3",
    mode: "add",
    makeDefault: false,
    replaceOriginal: true,
    useLocalTemp: false,
    defaultAudioOrdinal,
    originalDefaultAudioOrdinal: defaultAudioOrdinal,
    titleDrafts,
    originalTitles: { ...titleDrafts },
    status: "idle",
    runSteps: [],
  };
}

function mergeAnalyzedFile(file: FileState, analyzed: VideoFile) {
  const refreshed = toFileState(analyzed);
  file.path = refreshed.path;
  file.fileName = refreshed.fileName;
  file.durationSeconds = refreshed.durationSeconds;
  file.audioTracks = refreshed.audioTracks;
  file.thumbnailPath = refreshed.thumbnailPath;
  file.thumbnailUrl = refreshed.thumbnailUrl;
  file.selectedAudioIndex = refreshed.selectedAudioIndex;
  file.selectedDeleteIndices = [];
  file.defaultAudioOrdinal = refreshed.defaultAudioOrdinal;
  file.originalDefaultAudioOrdinal = refreshed.originalDefaultAudioOrdinal;
  file.titleDrafts = refreshed.titleDrafts;
  file.originalTitles = refreshed.originalTitles;
}

function removeFile(id: string) {
  files.value = files.value.filter((file) => file.id !== id);
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
  if (!file.metadataEnabled) return [];

  return file.audioTracks
    .filter((track) => (file.titleDrafts[track.audioOrdinal] ?? "") !== (file.originalTitles[track.audioOrdinal] ?? ""))
    .map((track) => ({
      audioOrdinal: track.audioOrdinal,
      title: file.titleDrafts[track.audioOrdinal] ?? "",
    }));
}

function defaultAudioOrdinalForRequest(file: FileState) {
  if (!file.metadataEnabled) return undefined;
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

function canProcessFile(file: FileState) {
  const hasConversion = file.convertEnabled && file.selectedAudioIndex !== undefined;
  const hasDeletion = file.deleteEnabled && file.selectedDeleteIndices.length > 0;
  const hasMetadata = file.metadataEnabled && hasMetadataChanges(file);

  return hasConversion || hasDeletion || hasMetadata;
}

function clearCompletedOperationAssignments(file: FileState) {
  file.convertEnabled = false;
  file.deleteEnabled = false;
  file.metadataEnabled = false;
}

function preparingMessage(file: FileState) {
  return file.useLocalTemp ? "Preparing local temporary files" : "Preparing";
}

function settingsSnapshot(file: FileState): FileTimingSummary["settings"] {
  const settings: FileTimingSummary["settings"] = [];
  const operations: string[] = [];

  if (file.convertEnabled) operations.push("Convert audio");
  if (file.deleteEnabled && file.selectedDeleteIndices.length) operations.push("Delete tracks");
  if (file.metadataEnabled && hasMetadataChanges(file)) operations.push("Edit metadata");
  settings.push({ label: "Operations", value: operations.join(", ") });

  if (file.convertEnabled) {
    const track = file.audioTracks.find((item) => item.streamIndex === file.selectedAudioIndex);
    settings.push({
      label: "Conversion",
      value: `${track?.label ?? `Stream ${file.selectedAudioIndex}`} to ${file.targetFormat.toUpperCase()} · ${file.mode === "add" ? "add track" : "replace track"} · ${file.makeDefault ? "default" : "not default"}`,
    });
  }

  if (file.deleteEnabled && file.selectedDeleteIndices.length) {
    const deleted = file.selectedDeleteIndices.map((streamIndex) =>
      file.audioTracks.find((track) => track.streamIndex === streamIndex)?.label ?? `Stream ${streamIndex}`,
    );
    settings.push({ label: "Deleted tracks", value: deleted.join(", ") });
  }

  const titles = changedTitles(file);
  if (titles.length) {
    settings.push({
      label: "Titles",
      value: titles.map((title) => `Track ${title.audioOrdinal + 1}: ${title.title || "no title"}`).join(" · "),
    });
  }

  const defaultAudio = defaultAudioOrdinalForRequest(file);
  if (defaultAudio !== undefined) {
    settings.push({ label: "Default track", value: `Track ${defaultAudio + 1}` });
  }

  settings.push({
    label: "Output",
    value: file.replaceOriginal ? "Replace the original file" : "Create a new file",
  });
  settings.push({ label: "Local temporary files", value: file.useLocalTemp ? "Yes" : "No" });
  settings.push({ label: "Concurrent operations", value: String(maxConcurrentJobs.value) });
  return settings;
}

async function refreshFileFromDisk(file: FileState) {
  const analyzed = await invoke<VideoFile[]>("analyze_files", { paths: [file.path] });
  const [refreshed] = analyzed;
  if (refreshed) {
    const keep = {
      convertEnabled: file.convertEnabled,
      deleteEnabled: file.deleteEnabled,
      metadataEnabled: file.metadataEnabled,
      targetFormat: file.targetFormat,
      mode: file.mode,
      makeDefault: file.makeDefault,
      replaceOriginal: file.replaceOriginal,
      useLocalTemp: file.useLocalTemp,
    };
    mergeAnalyzedFile(file, refreshed);
    Object.assign(file, keep);
  }
}

async function processQueue() {
  if (!canProcess.value) return;

  isProcessing.value = true;
  globalError.value = undefined;
  runSummary.value = undefined;
  isSummaryOpen.value = false;
  const queueStartedAt = performance.now();
  const startedAt = new Date().toISOString();
  const queue = files.value.filter(canProcessFile);
  const summaries = new Array<FileTimingSummary>(queue.length);
  let nextFileIndex = 0;

  async function runNextFile() {
    while (nextFileIndex < queue.length) {
      const fileIndex = nextFileIndex;
      nextFileIndex += 1;
      summaries[fileIndex] = await processQueuedFile(queue[fileIndex]);
    }
  }

  const workerCount = Math.min(maxConcurrentJobs.value, queue.length);
  try {
    await Promise.all(Array.from({ length: workerCount }, () => runNextFile()));
  } finally {
    isProcessing.value = false;
  }

  if (summaries.length > 0) {
    const failed = summaries.filter((summary) => summary.status === "error").length;
    const entry: RunSummary = {
      id: crypto.randomUUID(),
      startedAt,
      finishedAt: new Date().toISOString(),
      status: failed === 0 ? "done" : failed === summaries.length ? "error" : "partial",
      files: summaries,
      totalMs: performance.now() - queueStartedAt,
    };
    runSummary.value = entry;
    isSummaryOpen.value = true;
    try {
      await invoke("save_history_entry", { entry });
      history.value = [entry, ...history.value];
    } catch (error) {
      globalError.value = `Processing finished, but the operation could not be saved to history: ${String(error)}`;
    }
  }
}

async function processQueuedFile(file: FileState): Promise<FileTimingSummary> {
  file.status = "running";
  file.progress = 0;
  file.etaSeconds = undefined;
  file.runStartedAt = performance.now();
  file.runSteps = [];
  startTimingStep(file, preparingMessage(file));
  file.output = undefined;
  const inputPath = file.path;
  const settings = settingsSnapshot(file);
  let completedOutput: string | undefined;
  let replacedOriginal = false;

  try {
    const result = await invoke<{ output: string; replacedOriginal: boolean }>("process_file", {
      request: {
        jobId: file.id,
        input: file.path,
        replaceOriginal: file.replaceOriginal,
        useLocalTemp: file.useLocalTemp,
        convert: file.convertEnabled
          ? {
              audioIndex: file.selectedAudioIndex,
              format: file.targetFormat,
              mode: file.mode,
              makeDefault: file.makeDefault,
            }
          : undefined,
        deleteAudioIndices: file.deleteEnabled ? file.selectedDeleteIndices : [],
        defaultAudioOrdinal: defaultAudioOrdinalForRequest(file),
        titles: changedTitles(file),
      },
    });

    completedOutput = result.output;
    replacedOriginal = result.replacedOriginal;

    file.status = "done";
    file.progress = 1;
    file.output = result.output;
    if (result.replacedOriginal) {
      startTimingStep(file, "Rescanning");
      await refreshFileFromDisk(file);
      file.status = "done";
      file.progress = 1;
      file.output = result.output;
    }
    finishTiming(file);
    file.message = result.replacedOriginal ? "Original updated" : "File created";
    const summary = buildFileSummary(file, "done", inputPath, settings, result.output, result.replacedOriginal);
    clearCompletedOperationAssignments(file);
    return summary;
  } catch (error) {
    finishTiming(file);
    file.status = "error";
    file.message = "Processing failed. Check the log.";
    globalError.value = String(error);
    return buildFileSummary(file, "error", inputPath, settings, completedOutput, replacedOriginal, String(error));
  }
}

function timingStepFromLog(log: string) {
  if (log.includes("Copying input to local temporary storage") || log.includes("Copiando entrada al temporal local")) {
    return "Copying to local temporary storage";
  }
  if (log.includes("Starting FFmpeg") || log.includes("Iniciando FFmpeg")) return "Processing";
  if (log.includes("FFmpeg completed") || log.includes("FFmpeg terminado")) return "Finalising";
  if (log.includes("Copying") || log.includes("Copiando")) return "Copying to destination";
  if (log.includes("Replacing original") || log.includes("Reemplazando original")) return "Replacing original";
  return undefined;
}

function startTimingStep(file: FileState, label: string) {
  if (file.runSteps.at(-1)?.label === label && !file.runSteps.at(-1)?.finishedAt) {
    file.message = label;
    return;
  }

  finishOpenTimingStep(file);
  file.runSteps.push({ label, startedAt: performance.now() });
  file.message = label;
}

function finishOpenTimingStep(file: FileState) {
  const currentStep = file.runSteps.at(-1);
  if (!currentStep || currentStep.finishedAt !== undefined) return;

  currentStep.finishedAt = performance.now();
  currentStep.durationMs = Math.max(0, currentStep.finishedAt - currentStep.startedAt);
}

function finishTiming(file: FileState) {
  finishOpenTimingStep(file);
}

function buildFileSummary(
  file: FileState,
  status: FileTimingSummary["status"],
  inputPath: string,
  settings: FileTimingSummary["settings"],
  outputPath?: string,
  replacedOriginal = false,
  error?: string,
): FileTimingSummary {
  const finishedAt = performance.now();
  const totalMs = Math.max(0, finishedAt - (file.runStartedAt ?? finishedAt));
  const steps = file.runSteps.map((step) => ({
    label: step.label,
    durationMs: step.durationMs ?? Math.max(0, finishedAt - step.startedAt),
  }));

  return {
    fileName: file.fileName,
    inputPath,
    outputPath,
    status,
    error,
    affectedFiles: outputPath
      ? [{ path: outputPath, change: replacedOriginal ? "modified" : "created" }]
      : [],
    settings,
    steps,
    totalMs,
  };
}

function formatOperationDate(value: string) {
  return new Intl.DateTimeFormat("en-GB", {
    dateStyle: "medium",
    timeStyle: "medium",
  }).format(new Date(value));
}

function operationStatus(status: RunSummary["status"]) {
  if (status === "done") return "Completed";
  if (status === "partial") return "Completed with errors";
  return "Error";
}

function formatLogTime(timestamp: number) {
  if (!timestamp) return "--:--:--";
  return new Intl.DateTimeFormat("en-GB", {
    hour: "2-digit",
    minute: "2-digit",
    second: "2-digit",
  }).format(new Date(timestamp));
}

function readableLogMessage(message: string) {
  if (message.startsWith("Starting FFmpeg:") || message.startsWith("Iniciando FFmpeg:")) {
    return "Audio and video processing started.";
  }
  if (message.startsWith("FFmpeg progress") || message.startsWith("FFmpeg progreso")) {
    return message.replace(/FFmpeg (?:progress|progreso)/, "Processing progress:");
  }
  if (message === "FFmpeg completed successfully" || message === "FFmpeg terminado correctamente") {
    return "Audio and video processing completed successfully.";
  }
  if (message.startsWith("Paths prepared.") || message.startsWith("Rutas preparadas.")) {
    return "Working and destination paths prepared.";
  }
  return translateLegacyText(message).replaceAll(" | ", " · ");
}

function technicalLogDetails(message: string) {
  if (
    message.startsWith("Starting FFmpeg:") ||
    message.startsWith("Paths prepared.") ||
    message.startsWith("Iniciando FFmpeg:") ||
    message.startsWith("Rutas preparadas.")
  ) return translateLegacyText(message);
  return undefined;
}

function translateLegacyText(value: string) {
  const exact: Record<string, string> = {
    "Operaciones": "Operations",
    "Convertir audio": "Convert audio",
    "Eliminar pistas": "Delete tracks",
    "Editar metadatos": "Edit metadata",
    "Conversión": "Conversion",
    "Pistas eliminadas": "Deleted tracks",
    "Títulos": "Titles",
    "Pista principal": "Default track",
    "Salida": "Output",
    "Temporales locales": "Local temporary files",
    "Reemplazar archivo original": "Replace the original file",
    "Crear archivo nuevo": "Create a new file",
    "Sí": "Yes",
    "Preparando temporales locales": "Preparing local temporary files",
    "Preparando": "Preparing",
    "Copiando a temporal local": "Copying to local temporary storage",
    "Procesando": "Processing",
    "Finalizando": "Finalising",
    "Copiando al destino": "Copying to destination",
    "Reemplazando original": "Replacing original",
    "Reescaneando": "Rescanning",
    "Proceso completado": "Process completed",
    "Sesión iniciada": "Session started",
    "Sesion iniciada": "Session started",
  };
  if (exact[value]) return exact[value];

  return value
    .replace("Operacion iniciada.", "Operation started.")
    .replace("Operación iniciada.", "Operation started.")
    .replace("Archivo:", "File:")
    .replace("Pistas a eliminar:", "Tracks to delete:")
    .replace("Titulos a cambiar:", "Titles to change:")
    .replace("Títulos a cambiar:", "Titles to change:")
    .replace("Reemplazar original:", "Replace original:")
    .replace("Temporal local:", "Local temporary storage:")
    .replaceAll(" Si", " Yes")
    .replaceAll(" Sí", " Yes")
    .replace("Pista ", "Track ")
    .replace("sin título", "no title")
    .replace("añadir pista", "add track")
    .replace("sustituir pista", "replace track")
    .replace("no predeterminada", "not default")
    .replace("predeterminada", "default");
}

function formatBytes(bytes: number) {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${Math.round(bytes / 1024)} KB`;
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
}

function formatDuration(seconds?: number) {
  if (!seconds || !Number.isFinite(seconds)) return "Duration unavailable";
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
  return track.language?.trim() || "no language";
}

function progressLabel(file: FileState) {
  if (file.status === "done") return "Completed";
  if (file.status === "error") return "Error";
  if (file.status === "running" && file.progress === undefined) return "Processing";
  if (file.progress === undefined) return "Pending";

  const percent = Math.round(file.progress * 100);
  const eta = file.etaSeconds !== undefined ? ` - ETA ${formatDuration(file.etaSeconds)}` : "";
  return `${percent}%${eta}`;
}

function progressWidth(file: FileState) {
  if (file.status === "running") return `${Math.max(3, Math.round((file.progress ?? 0) * 100))}%`;
  return `${Math.round((file.progress ?? 0) * 100)}%`;
}

function formatElapsed(milliseconds: number) {
  const seconds = Math.max(0, Math.round(milliseconds / 1000));
  const hours = Math.floor(seconds / 3600);
  const minutes = Math.floor((seconds % 3600) / 60);
  const rest = seconds % 60;

  if (hours > 0) return `${hours}h ${String(minutes).padStart(2, "0")}m ${String(rest).padStart(2, "0")}s`;
  if (minutes > 0) return `${minutes}m ${String(rest).padStart(2, "0")}s`;
  return `${rest}s`;
}
</script>

<template>
  <main class="app-shell">
    <SettingsModal v-model="isSettingsOpen" :processing="isProcessing" />

    <div v-if="isAnalyzingFiles" class="modal-backdrop" role="status" aria-live="polite">
      <div class="loading-modal">
        <Loader2 :size="28" class="spin" />
        <h3>Analysing videos</h3>
        <p>Detecting tracks and preparing thumbnails.</p>
      </div>
    </div>

    <div v-if="isLogOpen && activeLog" class="modal-backdrop">
      <div class="log-modal readable-log-modal">
        <div class="log-modal-header">
          <div>
            <p class="eyebrow">Current session</p>
            <h3>TrackForge activity</h3>
            <p>Started on {{ formatOperationDate(new Date(activeLog.startedAt).toISOString()) }}</p>
          </div>
          <div class="history-header-actions">
            <button class="ghost-button" type="button" @click="openPreviousLogs">
              <Clock3 :size="16" />
              Previous logs
            </button>
            <button class="ghost-button" type="button" @click="isLogOpen = false">Close</button>
          </div>
        </div>

        <div v-if="!activeLog.entries.length" class="log-empty">
          No activity has been recorded in this session yet.
        </div>
        <div v-else class="log-entries">
          <article v-for="(entry, entryIndex) in activeLog.entries" :key="`${entry.timestamp}-${entryIndex}`" class="log-entry" :class="entry.level">
            <time>{{ formatLogTime(entry.timestamp) }}</time>
            <div>
              <p>{{ readableLogMessage(entry.message) }}</p>
              <details v-if="technicalLogDetails(entry.message)">
                <summary>Technical details</summary>
                <code>{{ technicalLogDetails(entry.message) }}</code>
              </details>
            </div>
          </article>
        </div>
        <p class="log-file-path">{{ activeLog.path }}</p>
      </div>
    </div>

    <div v-if="isPreviousLogsOpen" class="modal-backdrop">
      <div class="previous-logs-modal" role="dialog" aria-modal="true" aria-label="Previous logs">
        <div class="log-modal-header">
          <div>
            <p class="eyebrow">Activity archive</p>
            <h3>Previous logs</h3>
          </div>
          <div class="history-header-actions">
            <button
              class="ghost-button danger-button"
              type="button"
              :disabled="isClearingLogs || !hasStoredLogs"
              @click="clearAllLogs"
            >
              <Trash2 :size="16" />
              {{ isClearingLogs ? "Deleting" : "Delete all" }}
            </button>
            <button class="ghost-button" type="button" @click="isPreviousLogsOpen = false">Close</button>
          </div>
        </div>

        <div v-if="!logSessions.length" class="log-empty log-empty-large">
          There are no logs from previous sessions.
        </div>
        <div v-else class="previous-logs-layout">
          <nav class="log-session-list" aria-label="Previous log sessions">
            <button
              v-for="session in logSessions"
              :key="session.id"
              class="history-list-item"
              :class="{ active: selectedLogSession?.id === session.id }"
              type="button"
              @click="selectLogSession(session)"
            >
              <span>{{ session.id === "legacy" ? "Previous log" : formatOperationDate(new Date(session.startedAt).toISOString()) }}</span>
              <strong>{{ session.id === "legacy" ? "Legacy session" : "Finished session" }}</strong>
              <small>{{ formatBytes(session.sizeBytes) }}</small>
            </button>
          </nav>

          <div v-if="selectedPreviousLog" class="previous-log-detail">
            <div class="previous-log-heading">
              <div>
                <p class="eyebrow">Session</p>
                <h4>{{ selectedPreviousLog.sessionId === "legacy" ? "Log created before session logging" : formatOperationDate(new Date(selectedPreviousLog.startedAt).toISOString()) }}</h4>
              </div>
              <span>{{ selectedPreviousLog.entries.length }} event(s)</span>
            </div>

            <div v-if="!selectedPreviousLog.entries.length" class="log-empty">This session contains no entries.</div>
            <div v-else class="log-entries">
              <article v-for="(entry, entryIndex) in selectedPreviousLog.entries" :key="`${entry.timestamp}-${entryIndex}`" class="log-entry" :class="entry.level">
                <time>{{ formatLogTime(entry.timestamp) }}</time>
                <div>
                  <p>{{ readableLogMessage(entry.message) }}</p>
                  <details v-if="technicalLogDetails(entry.message)">
                    <summary>Technical details</summary>
                    <code>{{ technicalLogDetails(entry.message) }}</code>
                  </details>
                </div>
              </article>
            </div>
            <p class="log-file-path">{{ selectedPreviousLog.path }}</p>
          </div>
        </div>
      </div>
    </div>

    <div v-if="isSummaryOpen && runSummary" class="modal-backdrop">
      <div class="summary-modal">
        <div class="summary-modal-header">
          <div>
            <p class="eyebrow">Summary</p>
            <h3>Processing finished</h3>
          </div>
          <button class="ghost-button" type="button" @click="isSummaryOpen = false">Close</button>
        </div>

        <div class="summary-total">
          <span>Total queue time</span>
          <strong>{{ formatElapsed(runSummary.totalMs) }}</strong>
        </div>

        <div class="summary-list">
          <section v-for="(item, itemIndex) in runSummary.files" :key="`${item.fileName}-${itemIndex}`" class="summary-file">
            <div class="summary-file-header">
              <div>
                <h4>{{ item.fileName }}</h4>
                <p>{{ item.status === "done" ? "Completed" : "Error" }}</p>
              </div>
              <strong>{{ formatElapsed(item.totalMs) }}</strong>
            </div>

            <div class="summary-steps">
              <div v-for="(step, stepIndex) in item.steps" :key="`${item.fileName}-${step.label}-${stepIndex}`" class="summary-step">
                <span>{{ step.label }}</span>
                <b>{{ formatElapsed(step.durationMs) }}</b>
              </div>
            </div>

            <p v-if="item.error" class="summary-error">Detailed error available in the log.</p>
          </section>
        </div>
      </div>
    </div>

    <div v-if="isHistoryOpen" class="modal-backdrop">
      <div class="history-modal" role="dialog" aria-modal="true" aria-label="Operation history">
        <div class="history-modal-header">
          <div>
            <p class="eyebrow">Activity</p>
            <h3>Operation history</h3>
          </div>
          <div class="history-header-actions">
            <button
              class="ghost-button danger-button"
              type="button"
              :disabled="!history.length || isClearingHistory"
              @click="clearOperationHistory"
            >
              <Trash2 :size="16" />
              {{ isClearingHistory ? "Clearing" : "Clear history" }}
            </button>
            <button class="ghost-button" type="button" @click="isHistoryOpen = false">Close</button>
          </div>
        </div>

        <div v-if="!history.length" class="history-empty">
          <Clock3 :size="36" />
          <h4>There are no operations yet</h4>
          <p>Future runs will appear here with their files, settings and timings.</p>
        </div>

        <div v-else class="history-layout">
          <nav class="history-list" aria-label="Saved operations">
            <button
              v-for="entry in history"
              :key="entry.id"
              class="history-list-item"
              :class="{ active: selectedHistory?.id === entry.id }"
              type="button"
              @click="selectedHistory = entry"
            >
              <span>{{ formatOperationDate(entry.startedAt) }}</span>
              <strong>{{ operationStatus(entry.status) }}</strong>
              <small>{{ entry.files.length }} file(s) · {{ formatElapsed(entry.totalMs) }}</small>
            </button>
          </nav>

          <div v-if="selectedHistory" class="history-detail">
            <div class="history-detail-summary">
              <div>
                <p class="eyebrow">Run at</p>
                <h4>{{ formatOperationDate(selectedHistory.startedAt) }}</h4>
                <p>{{ operationStatus(selectedHistory.status) }} · {{ selectedHistory.files.length }} file(s)</p>
              </div>
              <strong>{{ formatElapsed(selectedHistory.totalMs) }}</strong>
            </div>

            <div class="history-files">
              <section v-for="(item, itemIndex) in selectedHistory.files" :key="`${selectedHistory.id}-${itemIndex}`" class="history-file">
                <div class="summary-file-header">
                  <div>
                    <h4>{{ item.fileName }}</h4>
                    <p>{{ item.status === "done" ? "Completed" : "Error" }}</p>
                    <code class="history-source">{{ item.inputPath }}</code>
                  </div>
                  <strong>{{ formatElapsed(item.totalMs) }}</strong>
                </div>

                <div class="history-block">
                  <p class="eyebrow">Changed files</p>
                  <div v-if="item.affectedFiles.length" class="history-path-list">
                    <div v-for="affected in item.affectedFiles" :key="`${affected.change}-${affected.path}`">
                      <span>{{ affected.change === "modified" ? "Modified" : "Created" }}</span>
                      <code>{{ affected.path }}</code>
                    </div>
                  </div>
                  <p v-else class="history-muted">No files were changed.</p>
                </div>

                <div class="history-block">
                  <p class="eyebrow">Settings</p>
                  <dl class="history-settings">
                    <template v-for="setting in item.settings" :key="setting.label">
                      <dt>{{ translateLegacyText(setting.label) }}</dt>
                      <dd>{{ translateLegacyText(setting.value) }}</dd>
                    </template>
                  </dl>
                </div>

                <div class="history-block">
                  <p class="eyebrow">Time by stage</p>
                  <div class="summary-steps">
                    <div v-for="(step, stepIndex) in item.steps" :key="`${step.label}-${stepIndex}`" class="summary-step">
                      <span>{{ translateLegacyText(step.label) }}</span>
                      <b>{{ formatElapsed(step.durationMs) }}</b>
                    </div>
                  </div>
                </div>

                <p v-if="item.error" class="history-error">{{ translateLegacyText(item.error) }}</p>
              </section>
            </div>
          </div>
        </div>
      </div>
    </div>

    <aside class="control-sidebar">
      <div class="brand">
        <button class="settings-button" type="button" title="Settings" aria-label="Settings" @click="isSettingsOpen = true">
          <Settings :size="15" />
        </button>
        <h1>TrackForge</h1>
        <p class="brand-version">v0.1.0</p>
      </div>

      <div class="sidebar-divider" />
      <button class="run-button sidebar-run-button" type="button" :disabled="!canProcess" @click="processQueue">
        {{ isProcessing ? "Processing" : "Run changes" }}
      </button>
      <div class="sidebar-divider" />

      <div class="sidebar-files-section">
        <p class="sidebar-heading">Files</p>
        <button class="add-files-button" type="button" :disabled="isSelectingFiles || isAnalyzingFiles || isProcessing" @click="pickFiles">
          {{ isAnalyzingFiles ? "Reading files" : isSelectingFiles ? "File picker open" : "+ Add files" }}
        </button>
      </div>
      <div class="sidebar-divider" />

      <div class="sidebar-section sidebar-section-end">
        <div v-if="isProcessing" class="processing-status">
          <Loader2 :size="14" class="spin" />
          <span>{{ runningFiles || 1 }} processing</span>
        </div>

        <button class="sidebar-action-button" type="button" @click="openHistory">
          <Clock3 :size="14" />
          History
          <span v-if="history.length" class="button-count">{{ history.length }}</span>
        </button>

        <button class="sidebar-action-button" type="button" @click="openLog">
          <ScrollText :size="14" />
          Logs
        </button>

        <div class="queue-summary" v-if="files.length">
          <p class="eyebrow">Work queue</p>
          <strong>{{ queuedFiles }}</strong>
        </div>
      </div>
    </aside>

    <section class="workspace">
      <header v-if="files.length" class="topbar">
        <div>
          <p class="eyebrow">Selected files</p>
          <h2>{{ files.length }} file(s)</h2>
        </div>
        <button class="ghost-button" type="button" :disabled="isProcessing || !files.length" @click="files = []">
          <Trash2 :size="17" />
          Clear
        </button>
      </header>

      <div v-if="globalError" class="error-box">{{ globalError }}</div>

      <div v-if="!files.length" class="empty-state">
        <p class="empty-title">No files selected</p>
        <p class="empty-sub">Add files from the sidebar to prepare operations.</p>
      </div>

      <section v-else class="file-stack">
        <article
          v-for="file in files"
          :key="file.id"
          class="file-card"
          :class="{
            running: file.status === 'running',
            done: file.status === 'done',
            error: file.status === 'error',
          }"
        >
          <div class="file-layout">
            <div class="thumbnail">
              <img v-if="file.thumbnailUrl" :src="file.thumbnailUrl" :alt="file.fileName" />
              <FileVideo v-else :size="38" />
            </div>

            <div class="file-body">
              <div class="detail-header">
                <div>
                  <p class="eyebrow">Video</p>
                  <h3>{{ file.fileName }}</h3>
                  <p class="path">{{ file.path }}</p>
                </div>
                <div class="file-card-actions">
                  <Check v-if="file.status === 'done'" :size="18" class="ok" />
                  <Loader2 v-else-if="file.status === 'running'" :size="18" class="spin" />
                  <button class="icon-button" type="button" @click="removeFile(file.id)" title="Remove from queue">
                    <Trash2 :size="18" />
                  </button>
                </div>
              </div>

              <div class="meta-row">
                <span>{{ formatDuration(file.durationSeconds) }}</span>
                <span>{{ file.audioTracks.length }} audio track(s)</span>
                <span>{{ progressLabel(file) }}</span>
              </div>

              <div class="progress-track">
                <div class="progress-fill" :style="{ width: progressWidth(file) }" />
              </div>

              <div class="operation-strip">
                <label class="operation-toggle">
                  <input type="checkbox" v-model="file.convertEnabled" />
                  <Wand2 :size="15" />
                  Convert
                </label>
                <label class="operation-toggle">
                  <input type="checkbox" v-model="file.deleteEnabled" />
                  <Eraser :size="15" />
                  Delete
                </label>
                <label class="operation-toggle">
                  <input type="checkbox" v-model="file.metadataEnabled" />
                  <Pencil :size="15" />
                  Metadata
                </label>
              </div>

              <div class="file-options" v-if="file.convertEnabled">
                <label>
                  Format
                  <select v-model="file.targetFormat">
                    <option value="aac">AAC</option>
                    <option value="ac3">AC3</option>
                    <option value="mp3">MP3</option>
                    <option value="opus">Opus</option>
                    <option value="flac">FLAC</option>
                    <option value="wav">WAV / PCM</option>
                  </select>
                </label>

                <label>
                  Result
                  <select v-model="file.mode">
                    <option value="add">Add converted track</option>
                    <option value="replace">Replace original track</option>
                  </select>
                </label>

              </div>

              <div class="file-preferences">
                <label class="file-preference">
                  <span class="file-preference-copy">
                    <b>Replace original</b>
                    <small>Overwrite the source after successful processing.</small>
                  </span>
                  <span class="preference-switch">
                    <input v-model="file.replaceOriginal" type="checkbox" />
                    <span class="preference-switch-track"><span class="preference-switch-thumb" /></span>
                  </span>
                </label>

                <label class="file-preference">
                  <span class="file-preference-copy">
                    <b>Local temporary files</b>
                    <small>Process locally before copying the result back.</small>
                  </span>
                  <span class="preference-switch">
                    <input v-model="file.useLocalTemp" type="checkbox" />
                    <span class="preference-switch-track"><span class="preference-switch-thumb" /></span>
                  </span>
                </label>

                <label class="file-preference" :class="{ disabled: !file.convertEnabled }">
                  <span class="file-preference-copy">
                    <b>Make converted track default</b>
                    <small>Use the new track as the default audio track.</small>
                  </span>
                  <span class="preference-switch">
                    <input v-model="file.makeDefault" type="checkbox" :disabled="!file.convertEnabled" />
                    <span class="preference-switch-track"><span class="preference-switch-thumb" /></span>
                  </span>
                </label>
              </div>

              <div class="track-table">
                <div class="track-head">
                  <span>Default</span>
                  <span>Codec</span>
                  <span>Language and title</span>
                  <span>Details</span>
                  <span>Operations</span>
                </div>
                <div v-for="track in file.audioTracks" :key="track.streamIndex" class="track-row">
                  <span class="default-cell">
                    <label class="default-radio">
                      <input
                        type="radio"
                        :name="`default-${file.id}`"
                        :value="track.audioOrdinal"
                        :disabled="file.deleteEnabled && file.selectedDeleteIndices.includes(track.streamIndex)"
                        v-model="file.defaultAudioOrdinal"
                      />
                      <span>{{ file.defaultAudioOrdinal === track.audioOrdinal ? "Default" : "Set default" }}</span>
                    </label>
                  </span>
                  <span class="codec-cell">
                    <b>{{ track.codec.toUpperCase() }}</b>
                    <small>audio:{{ track.streamIndex }}</small>
                  </span>
                  <span class="language-cell">
                    <b>{{ trackLanguage(track) }}</b>
                    <input
                      v-model="file.titleDrafts[track.audioOrdinal]"
                      :disabled="!file.metadataEnabled"
                      :class="{ dirty: trackHasMetadataChange(file, track) }"
                      class="title-input"
                      type="text"
                      placeholder="Track title"
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
                    <label class="radio-row" :class="{ disabled: !file.convertEnabled }">
                      <input
                        type="radio"
                        :name="`convert-${file.id}`"
                        :value="track.streamIndex"
                        :disabled="!file.convertEnabled"
                        v-model="file.selectedAudioIndex"
                      />
                      Convert
                    </label>
                    <label class="checkbox-row" :class="{ disabled: !file.deleteEnabled }">
                      <input
                        type="checkbox"
                        :disabled="!file.deleteEnabled"
                        :checked="file.selectedDeleteIndices.includes(track.streamIndex)"
                        @change="toggleDeleteTrack(file, track.streamIndex)"
                      />
                      Delete
                    </label>
                  </span>
                </div>
              </div>

              <div v-if="file.message" class="file-message" :class="file.status">
                {{ file.message }}
              </div>
            </div>
          </div>
        </article>
      </section>
    </section>
  </main>
</template>
