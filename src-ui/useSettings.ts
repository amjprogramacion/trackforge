import { ref, watch } from "vue";

const MIN_CONCURRENT_JOBS = 1;
const MAX_CONCURRENT_JOBS = 4;
const STORAGE_KEY = "trackforge_max_concurrent_jobs";

function clampConcurrentJobs(value: number) {
  if (!Number.isFinite(value)) return MIN_CONCURRENT_JOBS;
  return Math.min(MAX_CONCURRENT_JOBS, Math.max(MIN_CONCURRENT_JOBS, Math.round(value)));
}

const storedConcurrentJobs = Number.parseInt(localStorage.getItem(STORAGE_KEY) ?? "1", 10);
const maxConcurrentJobs = ref(clampConcurrentJobs(storedConcurrentJobs));

watch(maxConcurrentJobs, (value) => {
  const clamped = clampConcurrentJobs(value);
  if (clamped !== value) {
    maxConcurrentJobs.value = clamped;
    return;
  }
  localStorage.setItem(STORAGE_KEY, String(clamped));
});

export function useSettings() {
  return {
    maxConcurrentJobs,
    minConcurrentJobs: MIN_CONCURRENT_JOBS,
    maxConcurrentJobsLimit: MAX_CONCURRENT_JOBS,
  };
}
