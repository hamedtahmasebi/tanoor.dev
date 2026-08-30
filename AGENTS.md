# Tanoor — Agent Instructions

## Project identity

Tanoor is a **native desktop application** built with Tauri + React. Every UI
decision must be made with that in mind. It is NOT a web app or a browser
extension.

---

## UI philosophy — desktop-first

### Feel like a real OS application

- Controls should behave like native platform controls: focus rings, keyboard
  navigation, consistent cursor shapes, no layout jank.
- Avoid hover-only affordances that don't translate to keyboard or
  accessibility. Every interactive element must be reachable via Tab and
  operable via Enter/Space.
- Avoid CSS transitions that last longer than ~150 ms for structural changes
  (open/close, show/hide). Micro-transitions on hover (border-color, background)
  are fine at ~100 ms.

### Generalize; never one-off

The single most important rule:

> **There should be a very limited number of component variants. Each variant
> is defined by exactly one CSS class. Editing that class edits every instance.**

Concretely:
- A "primary button" is always `.send-button`. A "secondary button" is always
  `.quiet-button`. Do not add inline styles or ad-hoc `style={{}}` props to
  vary button appearance.
- A "modal overlay + dialog shell" is always `.dialog-overlay` + `.dialog` +
  `.dialog-header` + `.dialog-body` + `.dialog-footer`. Every modal in the app
  reuses this shell verbatim. Feature-specific modifiers (e.g. `.model-dialog`)
  may add extra layout but must not override the shell's core sizing or chrome.
- A "radio-style option list" is always `.option-list` + `.option-list-item`.
  Do not build a custom radio list for a new feature — add rows to the shared
  class.
- A "segmented control" (compact tab strip) is always `.segmented-control` +
  `.segmented-control-item`.
- A "dropdown / listbox" should use `.project-dropdown` pattern (border-radius
  6 px, `box-shadow: 0 16px 35px #17181699`, absolute-positioned below its
  trigger). Use the same pattern for any new dropdown — don't invent a new
  shadow/border combination.

### Keyboard navigation is not optional

Every list, dropdown, or option group must implement arrow-key navigation:

- `ArrowDown` / `ArrowUp` — move focus within a vertical list.
- `ArrowRight` / `ArrowLeft` — move selection within a horizontal segmented
  control.
- `Enter` / `Space` — activate the focused item.
- `Escape` — close the nearest overlay/dialog (handled globally in `App.tsx`
  via `closeModelDialog`, `closeSettings`, etc.).
- Wrap-around (last → first, first → last) is expected.
- After programmatic focus (`el.focus()`), do not steal focus back elsewhere.

### CSS tokens and color palette

The palette lives in `:root` (and throughout `styles.css`). Do not hardcode
hex colors in component files. When you need a new color value, check whether
an existing class already uses a close enough color before introducing a new
one. Aim to reuse values within ~10 % of each other rather than adding a
new unique hex.

Background layers (dark → light):
```
#20221f  raw-diff / deepest pits
#222321  activity-bar
#242522  diff-file / turn-block backgrounds
#252624  editor-panel / app root
#272825  dialog / review-dialog
#282927  editor-header
#292a28  workspace-sidebar
#2b2d2a  task-composer / option-list-item
#2c2e2b  dialog-header / settings-header
#2d2f2c  step-dot / detail-review-card
#2e302d  context-chip / diff-line background
#30322f  project-dropdown / welcome-symbol
#333532  inline-palette
#343632  review-comment
#353733  task-row active / option-list-item selected
#383a36  workspace-nav-item active
```

Accent/status colors:
- Running / in-progress: `#baa77c` / `#c2b087`
- Approved / success: `#9eaf9c` / `#83a989`
- Failed / error: `#b58c87` / `#c7a29a`
- Reasoning / o-series badge: `#b4a375` on `#2a261a`

### Typography scale

| Use | Size |
|---|---|
| Section heading uppercase | 9 px, weight 700, letter-spacing .1em |
| Meta / badge / footnote | 9 px |
| Label / hint / status | 10 px |
| Body / list item | 11 px |
| Composer prompt | 13 px |
| Page heading (h1) | 25 px, weight 500 |

Monospace stack: `ui-monospace, SFMono-Regular, Menlo, Consolas, monospace`

---

## Data-flow rules

- All persistent state lives in `src/store.ts` (Zustand). Components receive
  state via the store and call store actions. Do not `invoke` Tauri commands
  directly from components.
- New backend commands must be:
  1. Added to `CommandContract` in `src/api.ts`.
  2. Wrapped in a typed helper on the `api` object.
  3. Called only from `store.ts` actions.
  4. Registered in `src-tauri/src/lib.rs`'s `invoke_handler`.

## Agent adapter pattern

When adding support for a new AI agent beyond Codex:

1. Add a Rust `AgentRunner` implementation in `src-tauri/src/runner.rs` that
   implements the `AgentRunner` trait.
2. Add a corresponding static model catalog returned by `get_agent_models` (or
   a new `list_agents` command if multiple agents are selectable).
3. On the TypeScript side, the `AgentModelCatalog` type already provides
   `agentId`, `models`, and `effortLevels`. The `ModelDialog` component is
   generic over that catalog — it needs no changes when a new agent is added.
4. Persist the chosen agent in `AppSettings` alongside `codexModel`/`codexEffort`.

## Naming conventions

| Layer | Convention |
|---|---|
| Rust structs / enums | `PascalCase` |
| Rust fields / fn args | `snake_case` |
| Serde serialization | `camelCase` via `#[serde(rename_all = "camelCase")]` |
| TypeScript types / interfaces | `PascalCase` |
| TypeScript variables / functions | `camelCase` |
| CSS classes | `kebab-case` |
| CSS modifier classes | `--modifier` suffix (BEM-like) |
