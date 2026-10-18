# Spec: Bindings

- **Status:** Draft v0.1
- **Related:** [ADR 0018](../adr/0018-flows-as-step-trees.md), [ADR 0019](../adr/0019-dynamic-action-targets-and-interfaces.md), [ADR 0004](../adr/0004-javascript-expressions-and-scripts.md), [state and event log](state-and-event-log.md), [plugin protocol §8](plugin-protocol.md), [flows](flows.md)

Bindings are Covalence's **dataflow**. Flows do things in order; bindings keep something continuously in step with state. Every binding is evaluated as a JS expression ([ADR 0004](../adr/0004-javascript-expressions-and-scripts.md)), but users only write code when they choose to.

There are two directions:
- **Display bindings** (§1–§4): state → how a control looks.
- **Value mappings** (§5): a continuous control (fader, encoder) → an action parameter, with optional feedback.

Discrete inputs such as button presses are not bindings. They're `gesture` triggers on flows.

Shapes use TypeScript notation for readability. The real source of truth will be the schema in `protocol/`.

---

## 1. Display bindings

```ts
interface ControlBinding {        // project entity `control_binding`
  id: string;
  page: string;
  control: string;                // ControlDef.id, or a grid position on a page template
  props: { [prop in ControlProp]?: Binding };
}

type ControlProp =
  | "label" | "textColor" | "background" | "icon" | "image"   // rendered key image
  | "led"                                                     // LED colour
  | "value"                                                   // motor fader, encoder ring
  | "text";                                                   // text-only displays
```

Each property has an expected type: `label` and `text` are strings, colours are `color` values, `icon` and `image` are asset IDs or URLs, and `value` is a number from 0 to 1. The editor checks the binding's result against it (§4).

---

## 2. Binding kinds

A binding is stored in one of five structured forms. All but `expr` compile to an expression.

```ts
type Binding =
  | { kind: "value";    path: Ref; format?: Format; fallback?: Value }
  | { kind: "template"; parts: (string | { path: Ref; format?: Format })[]; fallback?: string }
  | { kind: "rules";    rules: { when: Condition; value: Value }[]; else: Value }
  | { kind: "map";      path: Ref; from: [number, number]; to: [number, number] | Value[];
                        curve?: "linear" | "log" | "exp"; clamp?: boolean }
  | { kind: "expr";     source: Expr; fallback?: Value };

type Ref =
  | { conn: string; path: Path }                     // fixed connection, ID form (state spec §1.1)
  | { target: Ref; path: Path }                      // through a target held in state (ADR 0019)
  | { root: "var" | "sys" | "surface" | "page" | "flow"; path: Path };

type Condition = { path: Ref; op: "==" | "!=" | ">" | ">=" | "<" | "<=" | "truthy" | "falsy"; value?: Value };
type Format = "auto" | "mmss" | "hhmmss" | "percent" | "fixed1" | "fixed2" | "upper";
```

| Kind | For | Example (text form) |
|---|---|---|
| `value` | Show one value | `pp.timers.countdown.remaining` → `04:59` (format `auto` uses the schema's `kind`: a `duration` shows as `mm:ss`) |
| `template` | Text with values in it | `Cam {page.params.camera.name}` |
| `rules` | Conditional styling; first match wins | when `atem.program == 3` → red; when `atem.preview == 3` → green; else grey |
| `map` | Scale a number, or pick from a list | fader 0–1 → −60…+10 dB; level 0–100 → `["green", "yellow", "red"]` |
| `expr` | Anything else | `target(var.selectedCam).tally.program ? "red" : "#333"` |

- **Converting to `expr` is one-way.** The editor shows the compiled expression as the starting point. Going back is only offered if the expression hasn't been edited.
- **`Ref` stores IDs,** so renaming an alias never breaks a binding. The editor shows the alias.

---

## 3. Evaluation

- **Bindings are evaluated in the core** with QuickJS, the same engine as flows.
- **Dependencies are tracked at run time** ([ADR 0019](../adr/0019-dynamic-action-targets-and-interfaces.md)). Each evaluation records the paths it reads, and the binding re-evaluates when any of them change. A binding that reads `var.selectedCam` moves to the new camera's paths as soon as the variable changes.
- **Once per scheduler turn,** after patches are coalesced, so a binding never shows a half-applied turn.
- **Only bindings on visible pages are evaluated.** A page that comes into view is evaluated in full before it's drawn.
- **Missing values read as `undefined`** ([ADR 0004](../adr/0004-javascript-expressions-and-scripts.md)). If the result is `undefined` or the evaluation throws, the property uses `fallback`, then the control's default. Errors are reported in the editor and the log, rate-limited, and never stop other bindings.
- **Stale values are shown as stale.** If any value a binding read has `quality: stale` ([state spec §1.3](state-and-event-log.md)), the control is dimmed and marked automatically. A binding can opt out with `showStale: false`.
- **Budget:** a state change to a re-rendered key image in under 16 ms ([architecture](../architecture.md#performance-budgets-targets-to-test-against)).

---

## 4. Typing

- The editor knows the type of every `Ref` from the state schema, plugin manifests and interfaces, and checks each kind against the property's expected type.
  - `rules`: each `value` must match the property type, and each `Condition.value` must match the type at `path`.
  - `map`: `path` must be a number.
  - `expr`: checked with the generated TypeScript definitions (ADR 0004).
- **Type errors block publishing. Warnings don't**, the same rule as [flows §4](flows.md).

---

## 5. Value mappings (continuous controls)

A fader or encoder that should drive a value continuously, like a mixer channel volume or a camera zoom speed, uses a value mapping instead of a flow.

```ts
interface ValueMapping {
  id: string;
  page: string;
  control: string;                        // a fader, encoder or touch strip
  target: TargetSpec;                     // fixed or dynamic, per flows §2.1
  action: string;                         // "setVolume"
  param: string;                          // the parameter that receives the value
  params?: { [k: string]: Value | { expr: Expr } };   // the other parameters
  map: { from: [number, number]; to: [number, number]; curve?: "linear" | "log" | "exp" };
  mode: "absolute" | "relative";          // relative: encoders add their delta
  rateLimit?: Duration;                   // default 20ms; the latest value always wins
  feedback?: Ref;                         // state to follow: moves motor faders, sets encoder rings
  pickup?: boolean;                       // non-motor faders: ignore input until it crosses the current value
}
```

- Actions from a mapping go through the output gate and the journal like any other action. High-rate updates are journaled with sampling, as for high-rate state ([state spec open question 4](state-and-event-log.md#6-open-questions)).
- Only the latest value is kept while the rate limit is waiting, so a slow device never builds a backlog.
- `feedback` is a display binding on the control's `value` property, set up automatically.

---

## 6. Page parameters and templates

- **A page can declare parameters:** `params: { camera: { kind: "target", interface: "camera.ptz@1" } }`. Their values live at `page.<page>.params.<name>`, and bindings and flows on that page read them as `page.params.<name>`.
- **One page, a selector row.** Buttons at the top set `page.params.camera`. Every binding and action on the page that uses it follows the selection. This is the main "control any camera" layout ([ADR 0019](../adr/0019-dynamic-action-targets-and-interfaces.md)).
- **Or stamp out copies.** "Create from template" makes one page per matching target, each with its parameter set to that target.

---

## 7. Plugin presets

Manifest `presets` ([plugin protocol §4](plugin-protocol.md#4-manifest)) can include ready-made bindings, such as "tally colour" or "preset name label". A preset written against an interface works for any target that implements it, so picking "tally of the selected camera" from a menu needs no typing.

---

## 8. Open questions

1. Should bindings on hidden pages keep evaluating when the page is shown on another surface or a web panel? (Probably: "visible" means visible anywhere.)
2. Should the `rules` kind allow combining conditions with AND/OR, or should more complex logic go to `expr`?
3. Is a visual editor for value mappings (curves, ranges) needed for v1, or are the form fields enough?
4. Should value mappings support two-way, such as an encoder that changes a value and also shows it, as one entity rather than a mapping plus a feedback binding? The `feedback` field covers the common case.
