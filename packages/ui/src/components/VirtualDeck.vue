<script setup lang="ts">
import { computed, onBeforeUnmount, reactive, watch } from "vue";
import { DeckClient, faderPosition, newDeckState } from "../surfaces/deck";
import { deckControls, models, type ControlDef, type Surface } from "../surfaces/model";

/**
 * A virtual Stream Deck (ADR 0012): attached to the core while it's on
 * screen, showing the key images the core draws and sending key down and up
 * with the browser's timestamps. Leaving the screen detaches it. It can also
 * have encoders and faders (ADR 0035): an encoder turns by dragging up and
 * down, the wheel or the arrow keys and presses like a key; a fader reports
 * where it's dragged, and a motorised one glides to the level it's sent.
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
  client = new DeckClient(state, props.surface, { id: props.surface.model, model: m, layout: props.surface.virtual });
  client.start();
}

// A renamed or replaced surface attaches again under its new slug, and a
// new layout with its new controls.
watch(
  [() => props.surface.id, () => props.surface.slug, () => props.surface.model, () => JSON.stringify(props.surface.virtual ?? {})],
  () => {
    state.images = {};
    state.levels = {};
    state.positions = {};
    connect();
  },
  { immediate: true },
);
onBeforeUnmount(() => client?.stop());

type Kind = "key" | "encoder" | "fader";
const all = computed(() => (model.value === undefined ? [] : deckControls(model.value, props.surface.virtual)));
const kindOf = (c: ControlDef): Kind =>
  c.inputs?.some((i) => i.kind === "relative") ? "encoder" : c.inputs?.some((i) => i.kind === "absolute") ? "fader" : "key";
const controls = computed(() => all.value.map((c) => ({ id: c.id, kind: kindOf(c), at: c.at, motor: c.outputs?.some((o) => o.motor) ?? false })));
const cols = computed(() => all.value.reduce((n, c) => Math.max(n, c.at.x + (c.at.w ?? 1)), 0));
const place = (at: ControlDef["at"]) => ({
  gridColumn: `${at.x + 1} / span ${at.w ?? 1}`,
  gridRow: `${at.y + 1} / span ${at.h ?? 1}`,
});

// ---- Encoders ----

/** Pixels of drag per detent. */
const DETENT_PX = 12;
let drag: { control: string; y: number; moved: boolean } | null = null;

function encoderDown(control: string, ev: PointerEvent) {
  if (ev.button !== 0) return;
  (ev.currentTarget as HTMLElement).setPointerCapture?.(ev.pointerId);
  drag = { control, y: ev.clientY, moved: false };
  client?.down(control);
}

function encoderMove(control: string, ev: PointerEvent) {
  if (drag?.control !== control) return;
  const detents = Math.trunc((drag.y - ev.clientY) / DETENT_PX);
  if (detents === 0) return;
  drag.y -= detents * DETENT_PX;
  drag.moved = true;
  client?.turn(control, detents);
}

function encoderUp(control: string) {
  drag = null;
  client?.up(control);
}

function wheel(control: string, ev: WheelEvent) {
  ev.preventDefault();
  client?.turn(control, ev.deltaY < 0 ? 1 : -1);
}

function encoderKey(control: string, ev: KeyboardEvent, isDown: boolean) {
  const step = { ArrowUp: 1, ArrowRight: 1, ArrowDown: -1, ArrowLeft: -1 }[ev.key];
  if (step !== undefined) {
    ev.preventDefault();
    if (isDown) client?.turn(control, step);
    return;
  }
  key(control, ev, isDown);
}

/** An encoder's ring: its level as an arc of 270°. */
function ring(control: string) {
  const level = state.levels[control];
  if (level === undefined) return {};
  return { "--level": `${level * 270}deg` };
}

// ---- Faders ----

function faderAt(ev: PointerEvent): number {
  const track = (ev.currentTarget as HTMLElement).getBoundingClientRect();
  return 1 - (ev.clientY - track.top) / track.height;
}

function faderDown(control: string, ev: PointerEvent) {
  if (ev.button !== 0) return;
  (ev.currentTarget as HTMLElement).setPointerCapture?.(ev.pointerId);
  client?.touch(control, true);
  client?.move(control, faderAt(ev));
}

function faderMove(control: string, ev: PointerEvent) {
  if (state.touched[control]) client?.move(control, faderAt(ev));
}

function faderKey(control: string, motor: boolean, ev: KeyboardEvent) {
  const step = { ArrowUp: 0.05, ArrowRight: 0.05, ArrowDown: -0.05, ArrowLeft: -0.05, PageUp: 0.25, PageDown: -0.25 }[ev.key];
  if (step === undefined) return;
  ev.preventDefault();
  const from = faderPosition(state, control, motor);
  client?.touch(control, true);
  client?.move(control, Math.round((from + step) * 100) / 100);
  client?.touch(control, false);
}

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
        :style="{ gridTemplateColumns: `repeat(${cols}, var(--key))` }"
        role="group"
        :aria-label="surface.name"
        data-testid="deck"
      >
        <template
          v-for="c in controls"
          :key="c.id"
        >
          <button
            v-if="c.kind === 'key'"
            type="button"
            class="key"
            :class="{ down: state.down[c.id] }"
            :style="place(c.at)"
            :data-control="c.id"
            :aria-label="`Key ${c.id}`"
            :disabled="state.status !== 'attached'"
            @pointerdown="down(c.id, $event)"
            @pointerup="client?.up(c.id)"
            @pointercancel="client?.up(c.id)"
            @keydown="key(c.id, $event, true)"
            @keyup="key(c.id, $event, false)"
            @blur="client?.up(c.id)"
            @contextmenu.prevent
          >
            <img
              v-if="state.images[c.id]"
              :src="state.images[c.id]"
              alt=""
              draggable="false"
            >
          </button>
          <button
            v-else-if="c.kind === 'encoder'"
            type="button"
            class="encoder"
            :class="{ down: state.down[c.id], lit: state.levels[c.id] !== undefined }"
            :style="{ ...place(c.at), ...ring(c.id) }"
            :data-control="c.id"
            :aria-label="`Encoder ${c.id}: drag, scroll or use the arrow keys to turn; press like a key`"
            :disabled="state.status !== 'attached'"
            @pointerdown="encoderDown(c.id, $event)"
            @pointermove="encoderMove(c.id, $event)"
            @pointerup="encoderUp(c.id)"
            @pointercancel="encoderUp(c.id)"
            @wheel="wheel(c.id, $event)"
            @keydown="encoderKey(c.id, $event, true)"
            @keyup="encoderKey(c.id, $event, false)"
            @blur="client?.up(c.id)"
            @contextmenu.prevent
          >
            <span class="knob" />
          </button>
          <div
            v-else
            class="fader"
            :class="{ touched: state.touched[c.id], motor: c.motor }"
            :style="{ ...place(c.at), '--pos': faderPosition(state, c.id, c.motor) }"
            role="slider"
            :tabindex="state.status === 'attached' ? 0 : -1"
            :data-control="c.id"
            :aria-label="`Fader ${c.id}`"
            aria-orientation="vertical"
            aria-valuemin="0"
            aria-valuemax="100"
            :aria-valuenow="Math.round(faderPosition(state, c.id, c.motor) * 100)"
            :aria-disabled="state.status !== 'attached'"
            @pointerdown="faderDown(c.id, $event)"
            @pointermove="faderMove(c.id, $event)"
            @pointerup="client?.touch(c.id, false)"
            @pointercancel="client?.touch(c.id, false)"
            @keydown="faderKey(c.id, c.motor, $event)"
            @contextmenu.prevent
          >
            <span class="track" />
            <span class="cap" />
          </div>
        </template>
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
  /* A deck with faders can be wider than a phone. */
  max-width: 100%;
  overflow-x: auto;
}

.deck {
  display: inline-grid;
  grid-auto-rows: var(--key);
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

.encoder {
  justify-self: center;
  align-self: center;
  width: calc(var(--key) * 0.8);
  height: calc(var(--key) * 0.8);
  padding: 0;
  border: none;
  border-radius: 50%;
  /* The ring: the level as an arc from 7:30 clockwise. */
  background: conic-gradient(from 225deg, var(--accent) 0 var(--level, 0deg), #26272b var(--level, 0deg) 270deg, transparent 270deg);
  display: grid;
  place-items: center;
  cursor: ns-resize;
}

.encoder:disabled {
  opacity: 1;
  cursor: default;
}

.encoder .knob {
  width: 74%;
  height: 74%;
  border-radius: 50%;
  background: radial-gradient(circle at 35% 30%, #3a3b40, #141416);
  box-shadow: 0 0 0 1px #2a2b2f;
  transition: transform 40ms ease-out;
}

.encoder.down .knob {
  transform: scale(0.92);
  box-shadow: 0 0 0 2px var(--accent);
}

.encoder:focus-visible,
.fader:focus-visible {
  outline: 2px solid var(--accent);
  outline-offset: 2px;
}

.fader {
  --cap: 34px;
  position: relative;
  justify-self: center;
  width: calc(var(--key) * 0.6);
  height: 100%;
  cursor: ns-resize;
  touch-action: none;
}

.fader .track {
  position: absolute;
  left: 50%;
  top: calc(var(--cap) / 2);
  bottom: calc(var(--cap) / 2);
  width: 6px;
  transform: translateX(-50%);
  border-radius: 3px;
  background: #26272b;
}

.fader .cap {
  position: absolute;
  left: 0;
  right: 0;
  height: var(--cap);
  /* --pos 0 is the bottom of the track, 1 the top. */
  top: calc((1 - var(--pos, 0)) * (100% - var(--cap)));
  border-radius: 6px;
  background: linear-gradient(#4a4b50, #2a2b2f);
  box-shadow: 0 0 0 1px #3a3b40, inset 0 -2px 0 #1a1b1e;
}

/* A motorised fader glides to its level; under a hand it follows at once. */
.fader.motor .cap {
  transition: top 180ms ease-out;
}

.fader.touched .cap {
  transition: none;
  box-shadow: 0 0 0 2px var(--accent);
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
