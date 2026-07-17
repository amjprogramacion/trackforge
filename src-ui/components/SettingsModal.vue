<script setup lang="ts">
import { computed } from "vue";
import { useSettings } from "../useSettings";

defineProps<{
  modelValue: boolean;
  processing: boolean;
}>();

const emit = defineEmits<{
  "update:modelValue": [value: boolean];
}>();

const { maxConcurrentJobs, minConcurrentJobs, maxConcurrentJobsLimit } = useSettings();
const concurrency = computed({
  get: () => maxConcurrentJobs.value,
  set: (value: number) => {
    maxConcurrentJobs.value = value;
  },
});

function close() {
  emit("update:modelValue", false);
}
</script>

<template>
  <Teleport to="body">
    <Transition name="settings-modal-fade">
      <div v-if="modelValue" class="settings-modal-overlay" @click.self="close">
        <div class="settings-modal-box" role="dialog" aria-modal="true" aria-label="Settings">
          <div class="settings-modal-header">
            <span class="settings-modal-title">Settings</span>
            <button class="settings-modal-close" type="button" title="Close" aria-label="Close settings" @click="close">✕</button>
          </div>

          <div class="settings-modal-body">
            <nav class="settings-tab-rail" aria-label="Settings sections">
              <button class="settings-tab-item settings-tab-item-active" type="button">General</button>
            </nav>

            <div class="settings-tab-content">
              <section class="settings-section">
                <p class="settings-label">Processing</p>
                <div class="settings-row">
                  <span class="settings-row-label">Concurrent file operations</span>
                  <input
                    v-model.number="concurrency"
                    class="settings-input"
                    type="number"
                    :min="minConcurrentJobs"
                    :max="maxConcurrentJobsLimit"
                    :disabled="processing"
                    aria-label="Concurrent file operations"
                  />
                </div>
                <p class="settings-hint">
                  Process between {{ minConcurrentJobs }} and {{ maxConcurrentJobsLimit }} files at once. Higher values use more CPU, memory and disk bandwidth.
                </p>
                <p v-if="processing" class="settings-hint settings-hint-warning">
                  This setting cannot be changed while the queue is running.
                </p>
              </section>
            </div>
          </div>
        </div>
      </div>
    </Transition>
  </Teleport>
</template>

<style scoped>
.settings-modal-overlay {
  position: fixed;
  inset: 0;
  z-index: 300;
  display: flex;
  align-items: center;
  justify-content: center;
  background: rgba(0, 0, 0, 0.6);
}

.settings-modal-box {
  width: 640px;
  max-width: calc(100vw - 32px);
  overflow: hidden;
  display: flex;
  flex-direction: column;
  border: 1px solid var(--border-color);
  border-radius: var(--border-radius-lg);
  background: var(--bg-secondary);
  box-shadow: 0 20px 60px rgba(0, 0, 0, 0.5);
}

.settings-modal-header {
  display: flex;
  flex-shrink: 0;
  align-items: center;
  justify-content: space-between;
  padding: var(--space-4) var(--space-4) var(--space-3);
  border-bottom: 1px solid var(--border-color);
}

.settings-modal-title {
  color: var(--text-primary);
  font-size: var(--font-size-md);
  font-weight: 600;
}

.settings-modal-close {
  padding: 4px;
  border-radius: var(--border-radius-sm);
  background: none;
  color: var(--text-muted);
  font-size: 12px;
  transition: color var(--transition), background var(--transition);
}

.settings-modal-close:hover {
  background: var(--bg-card);
  color: var(--text-primary);
}

.settings-modal-body {
  min-height: 320px;
  display: flex;
}

.settings-tab-rail {
  width: 148px;
  flex-shrink: 0;
  display: flex;
  flex-direction: column;
  gap: 2px;
  padding: var(--space-3) var(--space-2);
  border-right: 1px solid var(--border-color);
}

.settings-tab-item {
  overflow: hidden;
  padding: 7px var(--space-3);
  border: none;
  border-radius: var(--border-radius-sm);
  background: transparent;
  color: var(--text-secondary);
  font-size: var(--font-size-sm);
  text-align: left;
  text-overflow: ellipsis;
  white-space: nowrap;
  transition: background var(--transition), color var(--transition);
}

.settings-tab-item:hover,
.settings-tab-item-active {
  background: var(--bg-card);
  color: var(--text-primary);
}

.settings-tab-item-active {
  font-weight: 500;
}

.settings-tab-content {
  min-width: 0;
  flex: 1;
  overflow-y: auto;
}

.settings-section {
  display: flex;
  flex-direction: column;
  gap: var(--space-3);
  padding: var(--space-4);
}

.settings-label {
  margin-bottom: 2px;
  color: var(--text-secondary);
  font-size: var(--font-size-xs);
  letter-spacing: 0.6px;
  text-transform: uppercase;
}

.settings-row {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: var(--space-3);
}

.settings-row-label {
  display: flex;
  align-items: center;
  gap: var(--space-1);
  color: var(--text-secondary);
  font-size: var(--font-size-sm);
}

.settings-hint {
  margin-top: -8px;
  color: var(--text-muted);
  font-size: 10px;
  line-height: 1.4;
}

.settings-hint-warning {
  margin-top: 0;
  color: var(--color-accent);
}

.settings-input {
  width: 64px;
  padding: 4px var(--space-2);
  border: 1px solid var(--border-color);
  border-radius: var(--border-radius-sm);
  outline: none;
  background: var(--bg-card);
  color: var(--text-primary);
  font-size: var(--font-size-sm);
  text-align: left;
}

.settings-input:focus {
  border-color: var(--color-accent);
}

.settings-input:disabled {
  opacity: 0.4;
  cursor: not-allowed;
}

.settings-modal-fade-enter-active,
.settings-modal-fade-leave-active {
  transition: opacity 150ms ease;
}

.settings-modal-fade-enter-active .settings-modal-box,
.settings-modal-fade-leave-active .settings-modal-box {
  transition: transform 150ms ease, opacity 150ms ease;
}

.settings-modal-fade-enter-from,
.settings-modal-fade-leave-to {
  opacity: 0;
}

.settings-modal-fade-enter-from .settings-modal-box,
.settings-modal-fade-leave-to .settings-modal-box {
  transform: scale(0.95);
  opacity: 0;
}

@media (max-width: 560px) {
  .settings-modal-body {
    min-height: 280px;
    flex-direction: column;
  }

  .settings-tab-rail {
    width: 100%;
    border-right: 0;
    border-bottom: 1px solid var(--border-color);
  }
}
</style>
