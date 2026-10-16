# Democritus UI redesign — "Bohemian dusk"

This folder is the design handoff for restyling the Democritus desktop app.
Implement the look described here on top of the EXISTING app. Do not change
app logic, data, safety rules or features; this is a visual and layout pass,
plus a theme switch and a security-check panel.

## Files in this folder

- `mockups/BohoChat.dc.html`, `mockups/BohoMemory.dc.html`, `mockups/BohoOperator.dc.html`
  Reference mockups of the Chat, Memory and Settings & safety screens. They are
  written in a design-tool format (`<x-dc>` markup, `{{t.x}}` color holes, a
  small `class Component` script). Read them for layout, sizes, shapes, copy and
  ornaments. Do not ship them; rebuild the screens in the app's own code.
  Colors in them are `{{t.name}}` tokens: the values are in the `PAL` object in
  each file's script and in the table below.
- `assets/dusk-background.jpg` — app background photo (portrait, 1500×2000).
- `assets/democritus-icon-1024.png` / `democritus-icon.svg` — app icon. Generate
  all icon sizes with `npx tauri icon design/assets/democritus-icon-1024.png`
  and remove any old placeholder icon.

## Look in one sentence

Cream paper panels with notched, chamfered edges floating over a dusk city
photo; a thin high-contrast serif wordmark; tiny letter-spaced caps labels and
small ornaments (starbursts, dot pairs, a checker strip) like an editorial poster.

## Theme tokens

Implement as CSS custom properties on `:root`, switched by `data-theme` on `<html>`.

| Token | Light | Dark | Used for |
|---|---|---|---|
| paper | #EDE6D6 | #17234B | main panels |
| paper2 | #F6F1E6 | #1D2C5A | message cards, document page |
| sel | #DDD4BF | #283C72 | selected list row |
| line | #BEB49D | #3A4D82 | dividers, outlines |
| line2 | #D5CBB4 | #2C3E6C | faint dividers |
| lineStrong | #8F8672 | #6476A6 | dashed borders |
| muted | #4F5870 | #AEBBD8 | secondary text, tiny labels |
| ink | #142347 | #F3EBDD | primary text; inverted chips use ink bg + paper text |
| accent | #E0823A | #F2994A | primary buttons, ornaments, badges, "blocked" log rows |
| accentInk | #142347 | #10183A | text on accent |
| onPhoto | #F6EFE2 | #F6EFE2 | text drawn directly on the photo |
| slip | rgba(15,26,58,0.74) | rgba(8,12,30,0.8) | the user's message bubbles |
| codeBg | #142347 | #0C1331 | code blocks |
| codeText | #EDE6D6 | #E8DFCC | code text |
| codeMuted | #9AACD0 | #8796BE | code comments, code header |
| codeKw | #F4A261 | #F4A261 | code keywords |
| codeLine | #2A3B68 | #24345F | code header divider |

Background (both themes): `dusk-background.jpg`, `background-size: cover;
background-position: center 72%`, with a gradient overlay
`linear-gradient(180deg, rgba(10,14,30,.15) 0%, rgba(10,14,30,0) 40%, rgba(10,10,14,.35) 100%)`.
The app window has 22px padding so the photo frames every panel.

## Theme switch (required)

- Settings → Appearance: a 3-way control **Light / Dark / Follow system**.
  Default: Follow system (`prefers-color-scheme`), updating live when Windows changes.
- Persist the choice in the app's existing settings storage.
- Apply before first paint (no flash of the wrong theme).
- The mockup `BohoOperator.dc.html` shows the control's placement and style.

## Typography — fonts must be bundled (the app is offline)

Never load fonts from Google or any CDN at runtime. Install them as npm packages
(normal project dependencies, all SIL Open Font License) and import locally:

- Display / wordmark: **Italiana** (`@fontsource/italiana`) — DEMOCRITUS wordmark
  46px, letter-spacing 3px; page titles; current-state text 24px; composer text 22px.
- Body: **Manrope** 400/500/600/700 (`@fontsource/manrope`) — 14px base, line-height 1.5.
- Mono: **IBM Plex Mono** 400/500 (`@fontsource/ibm-plex-mono`) — model names, code,
  file names, numbers like "N° 014".
- Tiny labels: Manrope 9px, weight 700, letter-spacing 1.4px, UPPERCASE, color `muted`.

## Shapes

All panels are flat (no shadows, no rounded corners) and cut with `clip-path`:

- Top band (header): chamfered top corners and a notch rising from the bottom
  middle, so the photo shows through:
  `polygon(18px 0, calc(100% - 18px) 0, 100% 18px, 100% 100%, 70% 100%, calc(70% - 28px) calc(100% - 28px), calc(30% + 28px) calc(100% - 28px), 30% 100%, 0 100%, 0 18px)`
  with `padding-bottom: 40px`.
  Important: dropdowns inside the header must not be clipped. Put the clipped
  shape on an absolutely positioned background layer behind the header content
  (see the header in `BohoChat.dc.html`).
- Composer band (bottom of the chat): a raised tab in the middle of its top edge:
  `polygon(0 26px, 26% 26px, calc(26% + 26px) 0, calc(74% - 26px) 0, 74% 26px, 100% 26px, 100% calc(100% - 16px), calc(100% - 16px) 100%, 16px 100%, 0 calc(100% - 16px))`
- Side panels: one or two 14px chamfered corners.
- Primary button: hexagon-ish `polygon(10px 0, calc(100% - 10px) 0, 100% 50%, calc(100% - 10px) 100%, 10px 100%, 0 50%)`.
- Send button: diamond `polygon(50% 0, 100% 50%, 50% 100%, 0 50%)`, 48×48.

## Screens

### Chat (BohoChat)
- Top band, 3 columns: left = icon + tiny labels ("A LOCAL ASSISTANT / ATOMS & THE VOID",
  conversation number, checker strip); middle = DEMOCRITUS wordmark with sparkle
  ornaments and the conversation title in tiny caps; right = **Brain** picker
  (the existing model selector, restyled) + "OFFLINE · LOCALHOST" + tok/s.
- Left panel: New conversation (accent button), conversations grouped by project
  with 3-digit numbers, then links to Memory / Performance / Settings & safety.
- Middle: messages directly over the photo. User messages = `slip` bubbles with
  `onPhoto` text, right-aligned. Democritus messages = `paper2` cards with a tiny
  header (starburst, DEMOCRITUS · model name · N° number), the collapsible
  thinking section (dashed outline button), code blocks in code colors with Copy,
  and the feedback row (thumbs up/down, edit, tok/s, "memory update to review" chip).
- Brain switch divider: thin `onPhoto` line with sparkles and caps text
  "BRAIN SWITCHED TO … · HANDOFF PASSED".
- Composer band at the bottom: attach button, Italiana textarea with an underline,
  diamond send button, tiny caps hints (ENTER · SEND, SHIFT+ENTER · NEW LINE, ESC · STOP).
- Right panel "Continuity": current state (Italiana 24px), project steps
  checklist, models in memory with GPU/RAM bar and Unload, recalled files count.
  Keep every feature the app already has; only restyle and place them.

### Memory (BohoMemory)
- Same top band with "MEMORY" wordmark, Chat back link, Auto-apply and Pause controls.
- Left: file index; center: the selected file shown as a printed page; right:
  proposed update with before/after lines, Apply (accent) / Edit first / Reject,
  recent changes.

### Settings & safety (BohoOperator)
- Top band = Lockdown: "LOCKDOWN" in Italiana; when locked the band inverts
  (ink background, paper text).
- Appearance section (theme switch) at the top of the main panel.
- Permissions list: numbered rows, Italiana titles, round toggle switches
  (accent when on).
- Download allowlist chips; activity log panel; "Reset everything learned".

## Security check panel (add to Settings & safety)

A "Security check" section listing each item as a green/red/grey status row
with a one-line explanation and, where possible, a fix button:

1. Ollama listens only on 127.0.0.1 (check the bound address; red if 0.0.0.0).
2. `OLLAMA_HOST` environment variable is unset or localhost.
3. Windows Firewall outbound block rules exist for Democritus and Ollama
   (button: "Create rules", which runs `New-NetFirewallRule` with admin rights,
   only after the operator confirms in a dialog that shows the exact commands).
4. Ollama version, with a note to keep it updated (no automatic update checks).
5. Lockdown state and network permission state.
6. Model files on disk are only .gguf / .safetensors.
These are code paths that must be tested on Windows; list them in HANDOFF.md.

## Accessibility and quality
- Text contrast at least 4.5:1 (3:1 for 24px+); the token pairs above meet this.
- Real `<button>` / `<input>` / `<label>`; aria-labels on icon-only buttons;
  visible focus outlines in `accent`.
- Window minimum size about 1024×700; panels wrap instead of overflowing.
- Respect `prefers-reduced-motion`.
