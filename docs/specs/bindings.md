# Spec: Bindings

- **Status:** Draft v0.3. [M5](../milestones/m5-surfaces-and-bindings.md) implements display bindings (§1–§4) for `label`, `textColor` and `background`. [ADR 0024](../adr/0024-surfaces-as-two-way-devices.md) replaces value mappings with helpers (§5). [ADR 0026](../adr/0026-sources-and-selectors.md) makes every binding a `Source` and the binding kinds stateless transforms (§2); [ADR 0029](../adr/0029-dataflow-sources-transforms-sinks.md) makes helpers reducers (§5). Its markers follow the [surfaces spec](surfaces.md): **(Now)** is [M5 Phase 5](../milestones/m5-surfaces-and-bindings.md), and implemented; **(M6)**, **(M8)** and **(M9)** are those milestones; **(Later)** has no milestone yet.
- **Related:** [ADR 0018](../adr/0018-flows-as-step-trees.md), [ADR 0024](../adr/0024-surfaces-as-two-way-devices.md), [ADR 0019](../adr/0019-dynamic-action-targets-and-interfaces.md), [ADR 0004](../adr/0004-javascript-expressions-and-scripts.md), [ADR 0026](../adr/0026-sources-and-selectors.md), [ADR 0029](../adr/0029-dataflow-sources-transforms-sinks.md), [ADR 0031](../adr/0031-templates-instances-and-components.md), [dataflow](dataflow.md), [composition](composition.md), [state and event log](state-and-event-log.md), [plugin protocol §8](plugin-protocol.md), [flows](flows.md)

Bindings are where Covalence's **dataflow** ([dataflow spec](dataflow.md)) meets a control. Flows do things in order; bindings keep something continuously in step with state. Every binding is a `Source` and evaluates as a JS expression ([ADR 0004](../adr/0004-javascript-expressions-and-scripts.md)), but users only write code when they choose to.

A `control_binding` covers both directions of one control, since a surface is a two-way device ([ADR 0024](../adr/0024-surfaces-as-two-way-devices.md)):
- **Display bindings** (§1–§4): sources → the control's outputs (image, LED, level, text). Each property is a sink (dataflow §7).
- **Helpers** (§5): reducers over the control's input → cells with derived state and events. A derived value can be shown, sent to a variable or drive an action parameter. Derived events trigger flows through event triggers.

A control's input is already state and events without any binding (surfaces §4). Flows react to it; bindings don't run steps. Surfaces, controls, input state, events, layers and control groups are in the [surfaces spec](surfaces.md).

Shapes use TypeScript notation for readability. The real source of truth will be the schema in `protocol/`.

---

## 1. Display bindings

```ts
interface ControlBinding {        // project entity `control_binding`
  id: string;
  layer: string;                  // (Now) the layer it's on (M5: `page`)
  control: string;                // one of the layer's LayerControl IDs
  props: { [prop in ControlProp]?: Source<Value> };   // (M6) was `Binding`, §2
  helpers?: Helper[];             // (M8) §5
  gestures?: GestureTiming;       // (M8) surfaces §4.2: doubleWindow, longAfter, holdEvery, as Sources
}

type ControlProp =
  | "label" | "textColor" | "background" | "icon" | "image"   // rendered key image
  | "led"                                                     // LED colour
  | "value"                                                   // motor fader, encoder ring
  | "text";                                                   // text-only displays
```

Each property has an expected type: `label` and `text` are strings, colours are `color` values, `icon` and `image` are asset IDs or URLs, and `value` is a number from 0 to 1. The editor checks the binding's result against it (§4).

Each property drives one output capability (surfaces §2), and **(Now)** a property can only be bound on a control whose layer declares that output (`noOutput`). Whichever layer controls a control sets its whole look; nothing falls through from layers underneath (surfaces §5.2).

| Properties | Output |
|---|---|
| `label`, `textColor`, `background`, `icon`, `image` | `image` |
| `led` | `led` |
| `value` | `level`: moves a motor fader, sets an encoder ring or a meter |
| `text` | `text` |

---

## 2. Binding kinds

**(M6)** A binding is a `Source` (dataflow §1): a literal, `{ref}`, `{expr}` or a stateless transform (`{use}`, dataflow §4.1). The five kinds below are what M5 stored. M6 migrates them, and the editor keeps offering the same five choices:

| M5 kind | Becomes |
|---|---|
| `{kind: "value", path, fallback?}` | `{ref: path, fallback?}` |
| `{kind: "value", path, format, fallback?}` | `{use: "format", input: {ref: path}, format, fallback?}` |
| `{kind: "template", parts, fallback?}` | `{use: "template", parts, fallback?}`, each `{path, format?}` part a `{ref}` or a `format` |
| `{kind: "rules", rules, else}` | `{use: "rules", rules, else}`, each `when` a `{use: "compare", input: {ref: path}, op, value}` |
| `{kind: "map", path, …}` | `{use: "map", input: {ref: path}, …}` |
| `{kind: "expr", source, fallback?}` | `{expr: source, fallback?}` |

- Because options are sources, transforms chain, and `when` can combine conditions with `all`, `any` and `not` (open question 2).
- A literal is a binding too: `background: "#222"` needs no kind.

The M5 shapes, kept here as the reference for the migration and for §2.1:

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
  | { root: "vars" | "sys" | "surface" | "layer" | "flow"; path: Path };   // (Now) `layer` replaces `page`

type Condition = { path: Ref; op: "==" | "!=" | ">" | ">=" | "<" | "<=" | "truthy" | "falsy"; value?: Value };
type Format = "auto" | "mmss" | "hhmmss" | "percent" | "fixed1" | "fixed2" | "upper";
```

| Kind | For | Example (text form) |
|---|---|---|
| `value` | Show one value | `pp.timers.countdown.remaining` → `04:59` (format `auto` uses the schema's `kind`: a `duration` shows as `mm:ss`) |
| `template` | Text with values in it | `Cam {params.camera.name}` |
| `rules` | Conditional styling; first match wins | when `atem.program == 3` → red; when `atem.preview == 3` → green; else grey |
| `map` | Scale a number, or pick from a list | fader 0–1 → −60…+10 dB; level 0–100 → `["green", "yellow", "red"]` |
| `expr` | Anything else | `target(vars.selectedCam).tally.program ? "red" : "#333"` |

### 2.1 What M5 implements

The shapes above, with these rules, which the native evaluator and the compiled expression both follow (`covalence_core::binding`). **(M6)** The same rules define the `format`, `template`, `rules`, `compare` and `map` transforms (dataflow §4.1); only the shapes change.

- **Properties:** `label`, `textColor` and `background`. The others are `unsupported`, as are refs through a `target`.
- **`Path`s are segment arrays** ([ADR 0017](../adr/0017-wire-paths-as-segment-arrays.md)): `["outputs", 1, "source"]`.
- **`value`:** `undefined` stays `undefined`, so the fallback applies; anything else is formatted.
- **Formats:** `auto` is `String(v)` (formatting by the schema's `kind` comes later). `mmss` and `hhmmss` are the standard library's. `percent` is `Math.round(Number(v) * 100)` followed by `%`, for values from 0 to 1. `fixed1` and `fixed2` are `Number(v).toFixed(n)`. `upper` is `String(v).toUpperCase()`.
- **`template`:** if any value in it is `undefined`, the whole template is, and the fallback applies.
- **`rules`:** `==` and `!=` are strict (`===`, `!==`). `>`, `>=`, `<` and `<=` follow JavaScript: strings compare as strings, anything else as numbers. A condition with no `value` compares with `undefined`.
- **`map`:** only a number maps; anything else is `undefined`. `t = (v − from[0]) / (from[1] − from[0])`, clamped to 0–1 unless `clamp: false`. `log` is `log10(1 + 9t)`; `exp` is `(10^t − 1) / 9`. A range gives `to[0] + (to[1] − to[0]) · t`. A list gives `to[floor(t · length)]`, kept inside the list.
- **Results:** a label takes text, or a number or boolean written as `String()` writes it. A colour takes `#rrggbb` or `#rgb`. Anything else is a type error: the fallback applies and the error is reported.

### 2.2 The editor's preview (M5)

`POST /control-bindings/preview` takes `{binding, size?}`, a control binding saved or not, and evaluates it once against live state exactly as the binding worker would (§3), with the same 10 ms limit for `expr`. It never writes or journals.

```ts
{
  look: { label: string; textColor: string; background: string; stale: boolean };
  props: { [prop]: { value?: string; error?: string; expr?: string; reads: string[] } };
  image: string;    // base64 PNG, size × size (default 144), drawn by the key renderer
}
```

- `value` is what the property shows: its result, or its fallback. `error` is why the result wasn't used. `expr` is the compiled expression (§2), with connections by alias. `reads` are text paths.
- The image is byte for byte what a surface showing the binding would be sent, so the editor shows the real key rather than an imitation.
- A binding that doesn't parse is `422 {errors}`, as on save. Validation (unknown layers, duplicate controls) is left to saving.
- **Validation** (errors block saving): the layer and control exist (`unknownLayer`; M5: `unknownPage`, and `unknownControl`), no other binding has the control (`duplicateControl`), connections exist (`unknownConnection`), `expr` compiles (`exprSyntax`), and every literal fits its property (`badValue`): `rules` values and `else`, `map` lists, fallbacks, a `template` only on a label, a `map` range only on a label, and two different `from` numbers. **(Now)** The control must be one of the layer's controls, and each bound property's output must be declared for it (`noOutput`, §1). **(Later)** A control owned by a control group can't also have a binding (`duplicateControl`, surfaces §6).
- `showStale: false` isn't supported yet.

- **Converting to `expr` is one-way.** The editor shows the compiled expression as the starting point. Going back is only offered if the expression hasn't been edited.
- **`Ref` stores IDs,** so renaming an alias never breaks a binding. The editor shows the alias.

---

## 3. Evaluation

- **Bindings are evaluated in the core.** `expr` bindings use QuickJS, the same engine as flows, on a thread of their own. `value`, `template`, `rules` and `map` are evaluated natively, with the same results as their compiled expressions ([ADR 0023](../adr/0023-key-rendering-and-binding-evaluation.md)). `expr` bindings have a 10 ms time limit.
- **Dependencies are tracked at run time** ([ADR 0021](../adr/0021-expressions-as-sandboxed-snippets.md)). Each evaluation records the paths it reads, and the binding re-evaluates when any of them change. A binding that reads `vars.selectedCam` moves to the new camera's paths as soon as the variable changes.
- **Once per scheduler turn,** after patches are coalesced, so a binding never shows a half-applied turn.
- **Only bindings on visible layers are evaluated:** a layer that controls at least one control on an attached surface. **(Later)** Armed layers, whose `when` reads input state, are evaluated too, so they can be drawn the moment they take over (surfaces §5.2). A layer that comes into view is evaluated in full before it's drawn.
- **Missing values read as `undefined`** ([ADR 0004](../adr/0004-javascript-expressions-and-scripts.md)). If the result is `undefined` or the evaluation throws, the property uses `fallback`, then the control's default. Errors are reported in the editor and the log, rate-limited, and never stop other bindings.
- **Stale values are shown as stale.** If any value a binding read has `quality: stale` ([state spec §1.3](state-and-event-log.md)), the control is dimmed and marked automatically. A binding can opt out with `showStale: false`.
- **Budget:** a state change to a re-rendered key image in under 16 ms ([architecture](../architecture.md#performance-budgets-targets-to-test-against)).
- **(M8) Bindings are consumers in the dataflow graph** (dataflow §5.3). They're evaluated after the turn's cells have settled, so a binding that reads a helper or a computed variable never shows a half-updated value.

---

## 4. Typing

- The editor knows the type of every `Ref` from the state schema, plugin manifests and interfaces, and checks each kind against the property's expected type.
  - `rules`: each `value` must match the property type, and each `Condition.value` must match the type at `path`.
  - `map`: `path` must be a number.
  - `expr`: checked with the generated TypeScript definitions (ADR 0004).
- **Type errors block publishing. Warnings don't**, the same rule as [flows §4](flows.md).

---

## 5. Helpers (M8)

A helper turns a control's input into derived state, events, or both (surfaces §4.3). **A helper is a reducer** (dataflow §4.2) whose `on` and `input` default to its control, and whose value is a cell under the layer or the surface ([ADR 0029](../adr/0029-dataflow-sources-transforms-sinks.md)). Helpers replace value mappings ([ADR 0024](../adr/0024-surfaces-as-two-way-devices.md)). A fader or encoder driving a mixer volume or a camera's zoom speed is a helper with a `to` sink.

```ts
type Helper = ReducerSource & {                 // {use: "toggle" | "position" | …, …options}, dataflow §4.2
  as?: string;                                  // the cell's name; default the `use`
  scope?: "layer" | "surface";                  // where the cell lives; default "layer"
  to?: Sink[];                                  // dataflow §7
  journal?: "full" | { sampled: Duration };
};
```

- **Options are `Source`s,** so `min`, `max`, `count` or `items` can follow state. `follow`, `holdoff` and `pickup` are in dataflow §4.3.
- **`to` is a list of sinks** (dataflow §7): `{set}` to a writable variable, or `{action: {target, action, param, params?, rateLimit?}}`. Scaling on the way out is a `map` on the cell's value, not an option of the route: `to: {action: {…, param: "level"}}` reads the cell, and a `map` helper can sit in between if the units differ.
- **`gestures`** timing moved to the control binding (§1). It configures the control's events rather than computing a value.

| Need | How |
|---|---|
| A key that runs a flow | No helper: an event trigger on `press` |
| A key that only counts while another is held | No helper: a `state` trigger over both keys' `pressed` |
| A latching key | `toggle`, shown with a `rules` source on `background` |
| A GPIO contact or an OSC button that latches | Not a helper: a variable whose `value` is `{use: "toggle", on: {from: {path: …}, event: "press"}}` (dataflow §6) |
| An encoder as a volume | `position` with `follow` on the volume and `to: {action: setVolume}`; `value` bound to the same volume sets its ring |
| An encoder as a zoom speed | `velocity` with `decay`, `to: {action: zoom}` |
| An encoder picking a camera | `select` over a list, `to: {set: vars.selectedCam}` |
| A motor fader | `absolute` with `follow`, `to: {action}`; `value` bound to the followed state moves it |
| A spring-return lever as a pan speed | `rate` with a `deadband`, `to: {action: pan}` |
| A 3D mouse's axes as pan and tilt | `script` returning `{values: {pan, tilt}}` |

- **No helper is needed for input state and edge events.** They exist for every control (surfaces §4.1, §4.2).
- **Validation:** a helper must read an input the control has (`noInput`), `as` names are unique on the control, a `set` sink must be a writable variable (`readOnly`), a `script` must compile (`exprSyntax`), and an action sink's action and parameters are checked as an `action` step's are (flows §4).
- **A value is state first.** It's written under `as` (surfaces §3) whether or not it has a sink, so bindings and `state` triggers can use it. `scope: "surface"` gives each surface the layer is on its own value.
- **`follow`** replaces value mappings' `feedback`; the output side is a display binding on `value`.
- **`script`** gets `{input, event?, previous}` and returns `{values?, events?}`, under the bindings' 10 ms limit. A failure keeps the previous values and is reported like a binding error.
- **Actions from a sink** go through the output gate and the journal like any other action, with `cause` → the cell's patch, rate-limited with the latest value winning (dataflow §7). High-rate cells are journaled sampled (dataflow §5.1).

---

## 6. Layer parameters and templates (M9)

- **A layer can declare parameters:** `params: { camera: { kind: "target", interface: "camera.ptz@1" } }`. Their values live at `layer.<slug>.params.<name>`, and the layer's own bindings, helpers and owned flows read them as `params.<name>` (dataflow §8). Nothing reads them implicitly from another layer or from a free-standing flow ([ADR 0031](../adr/0031-templates-instances-and-components.md), [ADR 0032](../adr/0032-one-template-primitive.md)).
- **One layer, a selector row.** The layer's `camera` parameter defaults to `{ref: vars.selectedCam}`, and buttons at the top set that variable. Every binding and action on the layer that uses `params.camera` follows the selection. This is the main "control any camera" layout ([ADR 0019](../adr/0019-dynamic-action-targets-and-interfaces.md)).
- **Or instances.** "Create from template" makes an **instance** ([composition §7](composition.md)), and one instance with `forEach: {targets: {interface: "camera.ptz@1"}}` makes a layer per camera, including cameras added during the show ([composition §4](composition.md)). Instances follow their template: there are no copies to drift. To spread a list over the controls of one layer, a control group (surfaces §6) does it instead.

---

## 7. Plugin presets become components (M9)

Manifest `presets` ([plugin protocol §4](plugin-protocol.md#4-manifest)) become **components** ([composition §8](composition.md)): a reusable control with its look, helpers, flows and parameters, such as "tally key" or "record toggle". A component written against an interface works for any target that implements it, so placing "tally of the selected camera" needs no typing.

---

## 8. Open questions

1. ~~Should bindings on hidden pages keep evaluating when the page is shown on another surface or a web panel?~~ Decided for M5: "visible" means shown on any attached surface, and a layer's bindings are evaluated once for all of them.
2. ~~Should the `rules` kind allow combining conditions with AND/OR, or should more complex logic go to `expr`?~~ Decided by [ADR 0026](../adr/0026-sources-and-selectors.md): a rule's `when` is a `Source<boolean>`, so `all`, `any` and `not` combine conditions.
3. Is a visual editor for helpers (curves, ranges, acceleration) needed for v1, or are the form fields enough?
4. ~~Should value mappings support two-way, such as an encoder that changes a value and also shows it, as one entity?~~ Decided by [ADR 0024](../adr/0024-surfaces-as-two-way-devices.md): one `control_binding` holds the control's helpers and its display bindings, and `follow` keeps the input side in step with state.
