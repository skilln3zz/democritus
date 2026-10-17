# Democritus UI — "Neumorphic off-white" (replaces the bohemian dusk look)

Restyle the existing Democritus app to this design. It REPLACES the bohemian
dusk theme (design/UI-SPEC.md): remove the dusk background photo, the Italiana
and Manrope fonts, the notched clip-path panels and the poster ornaments.
Keep every feature, all app logic, the safety rules and the theme switch.
This is a visual pass plus the ferrofluid thinking orb.

## Files

- `mockups/NeuChat.dc.html` — chat screen, including the ferrofluid orb (its canvas
  drawing code is in the script at the bottom: `draw()` and `tick()`).
- `mockups/NeuSettings.dc.html` — Settings & security (lockdown, permissions, security check).
- `mockups/NeuTrain.dc.html` — Build Democritus page. Only for when Step 6E exists;
  until then, ignore it.
- `assets/democritus-icon-1024.png` — the app icon (already applied if the icon task ran).

The mockups are in a design-tool format (`<x-dc>` markup, `{{holes}}`, a small
`class Component` script). Read them for layout, sizes, spacing, shadows, copy and
behavior. Do not ship them; rebuild the screens in the app's own code. The operator
edited these mockups by hand: where a mockup differs from this text, the mockup wins.

## The look in one sentence

Everything is made of one warm off-white material: cards and buttons look softly
pushed out of the surface (raised), inputs and "current/selected" things look pressed
in (inset), with a single terracotta accent for primary actions.

## Tokens (light theme — the main theme)

| Token | Value | Use |
|---|---|---|
| surface | #ECE8E1 | page and every card/button background (same color everywhere) |
| ink | #2D2A26 | main text |
| muted | #5F594F | secondary text, labels (contrast ≥ 5:1 on surface) |
| accent | #B5582A | primary buttons, send, toggles on, highlights |
| on-accent | #FFFFFF | text/icons on accent |
| accent-soft | #D8A07F | secondary bar fill |
| ok | #4E9B6A | pass lights, done checks |
| fail | #C0453A | fail lights, blocked log lines |
| info | #B8B1A5 | neutral lights |
| shade-dark | #CEC8BD | dark side of raised shadows |
| shade-inset | #D2CCC1 | dark side of inset shadows |
| shade-light | #FFFFFF | light side of all shadows |
| code text | #3A362F, keywords accent, comments #8A8377 | code blocks (inset) |

Shadow recipes (always one dark + one light shadow, light from the top-left):
- Raised large (cards, panels): `10px 10px 22px shade-dark, -10px -10px 22px shade-light`, radius 28–32px
- Raised medium (buttons, header bar): `6px 6px 12px …, -6px -6px 12px …`, radius = half the height (pill)
- Raised small (icon buttons): `4px 4px 8px …, -4px -4px 8px …`, circles
- Inset (inputs, selected list row, user messages, code, logs, status rows):
  `inset 4px 4px 9px shade-inset, inset -4px -4px 9px shade-light`
- Accent buttons: accent fill + `5px 5px 12px #C9BFAF, -5px -5px 12px #FFFFFF`
- Pressed state for any raised button: switch it to the inset recipe.

## Dark theme (keep the Light / Dark / Follow system switch)

Neumorphism needs a mid-tone surface, so dark mode is charcoal, not black:
surface #2B2D31, ink #ECE8E1, muted #A7A196, shade-dark #1F2124, shade-inset #222427,
shade-light #373A3F, accent #D4723F (on-accent #1A1A1A), ok #6BBF8A, fail #E0675B.
Check every text pair for ≥ 4.5:1 contrast and adjust if needed. Light stays the default.

## Typography (bundle locally — the app is offline)

Install as npm packages (normal dependencies, SIL Open Font License) and import locally;
never load from Google at runtime. Remove the old @fontsource Italiana and Manrope packages.
- Headings: **Fraunces** semibold 600 (`@fontsource-variable/fraunces` or `@fontsource/fraunces`)
  — page titles 38px, section titles 26px, current-state text 23px.
- Body: **Figtree** 500 base, 600/700 for emphasis (`@fontsource/figtree`) — 15px base,
  small labels 13px weight 700 in muted.
- Mono: **IBM Plex Mono** 500 (already installed) — model names, code, logs.

## Chat screen (NeuChat)

- Three areas: left sidebar card, main column, right "Continuity" card, all raised.
- Sidebar: starts directly with the New conversation pill button (accent dot +), then an
  inset search field, conversations grouped by project (selected one is inset with an accent
  dot), and three raised round icon buttons at the bottom (Memory, Build Democritus, Settings).
  No logo or app name at the top of the sidebar.
- Header bar (raised pill): conversation title + subtitle, the Brain picker (inset pill;
  dropdown is a raised card, selected model row inset), and an inset "Offline · local" status
  with a green dot.
- Messages: user messages are INSET bubbles aligned right; Democritus replies are RAISED cards
  aligned left. Reply header = only the model tag (small inset pill). No "Democritus" name in
  replies. Brain-switch divider = small inset pill centered with an accent icon.
- Feedback row: round raised thumbs buttons, tok/s text, raised "memory update to review" pill
  in accent text.
- Composer: raised pill container, round raised attach button, inset text field, round accent
  send button (54px).
- Right Continuity card: current state (Fraunces), round project-progress dial (conic gradient
  in a raised ring), "In memory" inset bar with GPU/RAM split, Unload button, inset "Recalled
  this turn" box.

## The ferrofluid thinking orb (required)

- Shown beside the reply that is being generated, the size of the send button: a 54px raised
  circle containing a 46px inset well containing a 44px canvas (draw at 2× for sharpness).
- While the model is inside `<think>…</think>`: hide the thinking text by default; show
  "Working it out… N thoughts so far" and a "Show thoughts" button that reveals it.
  The orb is a glossy black ferrofluid drop with spikes whose height follows the rate of
  incoming thinking tokens: each token raises the target intensity, the target decays every
  frame (×0.962, floor 0.22 while thinking), and the drawn intensity eases toward it (×0.09).
- When the answer starts: spikes melt into a calm, gently wobbling drop. After the reply,
  show "Thought for X seconds · N thoughts, hidden".
- No Thinking/Done status pill — the orb alone shows the state.
- Use the exact shape and shading code from `draw()` in NeuChat.dc.html (15 spikes,
  exponent 7, radial gradient #4b4b4b → #151515 → #020202, soft highlight, floor shadow).
- Stop the animation loop when nothing is generating; respect prefers-reduced-motion
  (smaller, slower spikes); keep CPU use tiny.
- Setting: "Thinking display: Orb / Show thoughts / Both" (default Orb).

## Settings & security (NeuSettings)

- Header with a round raised back button and the Fraunces title.
- Lockdown: a 108px round raised button with a lock icon; when locked it becomes inset with an
  accent lock and the text changes.
- Permissions: soft pill toggle switches (inset track; accent track when on; round raised knob).
- Security check: each row is an inset strip with a glowing status light (ok / fail / info),
  title, explanation, and fix buttons where they exist (Create rules = accent pill).
- Inset mini activity log at the bottom; blocked lines in fail color.
- Keep the Appearance (Light/Dark/Follow system) control — style it as a raised segmented pill
  with the active segment inset.

## Other pages (Memory, Performance, etc.)

Apply the same system: raised cards on the surface, inset inputs and selected rows, Fraunces
titles, accent only for primary actions.

## Quality

- Text contrast ≥ 4.5:1 (3:1 for 24px+). Neumorphic edges are subtle, so every interactive
  element needs a clear focus outline (2px accent, 3px offset) and a visible pressed state.
- Real buttons, labels and aria-labels on icon-only buttons.
- Minimum window size 1024×700; panels wrap instead of overflowing.
