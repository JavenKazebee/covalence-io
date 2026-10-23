<script setup lang="ts">
import { computed, onBeforeUnmount, reactive, watch } from "vue";
import { DeckClient, newDeckState } from "../surfaces/deck";
import { keysOf, models, type Surface } from "../surfaces/model";

/**
 * A virtual Stream Deck (ADR 0012): attached to the core while it's on
 * screen, showing the key images the core draws and sending key down and up
 * with the browser's timestamps. Leaving the screen detaches it.
 */
const props = defineProps<{ surface: Surface }>();

const state = reactive(newDeckState());
const model = computed(() => models[props.surface.model]);
let client: DeckClient | null = null;

function connect() {
  client?.stop();
  client = null;
  const m = model.value;
  if (m === undefined) return;
  client = new DeckClient(state, props.surface, { id: props.surface.model, model: m });
  client.start();
}

// A renamed or replaced surface attaches again under its new slug.
watch(
  [() => props.surface.id, () => props.surface.slug, () => props.surface.model],
  () => {
    state.images = {};
    connect();
  },
  { immediate: true },
);
onBeforeUnmount(() => client?.stop());

const controls = computed(() => (model.value === undefined ? [] : keysOf(model.value.layout)));

function down(control: string, ev: PointerEvent) {
  if (ev.button !== 0) return;
  // Keep getting this pointer's events, so the up arrives even off the key.
  (ev.currentTarget as HTMLElement).setPointerCapture?.(ev.pointerId);
  client?.down(control);
}

function key(control: string, ev: KeyboardEvent, isDown: boolean) {
  if (ev.key !== " " && ev.key !== "Enter") return;
  ev.preventDefault();
  if (ev.repeat) return;
  if (isDown) client?.down(control);
  else client?.up(control);
}

const statusText = computed(() => {
  if (state.status === "attached") return "Attached";
  if (state.status === "connecting") return "Attaching…";
  if (state.reason === "attached elsewhere") return "Attached in another tab or window";
  if (state.retrying) return state.reason ? `Disconnected (${state.reason}), retrying…` : "Disconnected, retrying…";
  return state.reason ? `Detached: ${state.reason}` : "Detached";
});
const tone = computed(() => (state.status === "attached" ? "good" : state.retrying || state.status === "connecting" ? "warn" : "bad"));
</script>

<template>
  <div class="deck-wrap">
    <p
      v-if="model === undefined"
      class="error"
    >
      The browser can't be a {{ surface.model }}.
    </p>
    <template v-else>
      <div
        class="deck"
        :style="{ gridTemplateColumns: `repeat(${model.layout.cols}, var(--key))` }"
        role="group"
        :aria-label="surface.name"
        data-testid="deck"
      >
        <button
          v-for="c in controls"
          :key="c"
          type="button"
          class="key"
          :class="{ down: state.down[c] }"
          :data-control="c"
          :aria-label="`Key ${c}`"
          :disabled="state.status !== 'attached'"
          @pointerdown="down(c, $event)"
          @pointerup="client?.up(c)"
          @pointercancel="client?.up(c)"
          @keydown="key(c, $event, true)"
          @keyup="key(c, $event, false)"
          @blur="client?.up(c)"
          @contextmenu.prevent
        >
          <img
            v-if="state.images[c]"
            :src="state.images[c]"
            alt=""
            draggable="false"
          >
        </button>
      </div>
      <div
        class="row deck-status"
        :data-tone="tone"
      >
        <span class="dot" />
        <span data-testid="deck-status">{{ statusText }}</span>
        <button
          v-if="state.status === 'closed' && !state.retrying"
          type="button"
          class="secondary"
          @click="connect"
        >
          Attach here
        </button>
      </div>
    </template>
  </div>
</template>

<style scoped>
.deck-wrap {
  --key: 76px;
}

.deck {
  display: inline-grid;
  gap: 12px;
  padding: 18px;
  border-radius: 18px;
  background: #0b0b0c;
  box-shadow: inset 0 0 0 1px #2a2b2f;
  user-select: none;
  touch-action: none;
}

.key {
  width: var(--key);
  height: var(--key);
  padding: 0;
  border: none;
  border-radius: 10px;
  overflow: hidden;
  background: #000;
  box-shadow: 0 0 0 1px #26272b;
  transition: transform 40ms ease-out;
}

.key:disabled {
  opacity: 1;
  cursor: default;
}

.key:focus-visible {
  outline: 2px solid var(--accent);
  outline-offset: 2px;
}

.key.down {
  transform: scale(0.94);
  box-shadow: 0 0 0 2px var(--accent);
}

.key img {
  display: block;
  width: 100%;
  height: 100%;
  pointer-events: none;
}

.deck-status {
  margin-top: 0.5rem;
  gap: 0.4rem;
  color: var(--muted);
}

.deck-status button {
  padding: 0.1rem 0.6rem;
}

@media (max-width: 480px) {
  .deck-wrap {
    --key: 52px;
  }

  .deck {
    gap: 8px;
    padding: 10px;
  }
}
</style>
