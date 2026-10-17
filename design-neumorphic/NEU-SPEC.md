# Democritus UI — neumorphic off-white (current design, supersedes all earlier specs)

This folder is the single source of truth for how Democritus should look.
It replaces the earlier bohemian dusk spec and the first neumorphic spec.
Keep every feature, all app logic and the safety rules; this is the visual and
interaction design, plus the ferrofluid orb, the Training tab and the Coding agent page.

## Files

- `mockups/NeuChat.dc.html` — chat, with the thinking orb beside the reply, the coding-agent
  button in the composer, and no right-hand panel.
- `mockups/NeuTrain.dc.html` — the Training tab: big ferrofluid orb + status window. DARK theme.
- `mockups/NeuSettings.dc.html` — Settings & security: lockdown, permissions (outlined switches),
  security check, activity log.
- `mockups/NeuCode.dc.html` — the Coding agent page: folder, safety strip, and a conversation that
  works like chat (orb + reply card with tool steps, explanation, diff and Approve / Reject / Edit first).
- `assets/democritus-icon-1024.png` — the app icon.

The mockups use a design-tool format (`<x-dc>` markup, `{{holes}}`, a `class Component`
script). Read them for layout, sizes, spacing, shadows, copy and behavior, and for the
ferrofluid drawing code. Do not ship them; rebuild in the app's own code. The operator
edited these by hand: where a mockup differs from this text, THE MOCKUP WINS.

## The material

One warm off-white surface. Cards and buttons look pushed out of it (raised); inputs,
selected rows, the user's own messages, code and logs look pressed into it (inset).
A single terracotta accent marks primary actions. No borders except where noted.

### Light theme (default)

| Token | Value | Use |
|---|---|---|
| surface | #ECE8E1 | page and every card/button background |
| ink | #2D2A26 | main text |
| muted | #5F594F | secondary text and labels |
| accent | #B5582A | primary buttons, send, toggles on, highlights |
| on-accent | #FFFFFF | text on accent |
| accent-dark | #8F4421 | borders/shadow on accent elements |
| outline | #C6BCA8 | switch and knob outlines |
| ok | #4E9B6A | pass lights, done checks, added diff lines |
| fail | #C0453A | fail lights, blocked log lines, removed diff lines |
| info | #B8B1A5 | neutral lights |
| shade-dark | #CEC8BD | dark side of raised shadows |
| shade-inset | #D2CCC1 | dark side of inset shadows |
| shade-light | #FFFFFF | light side of all shadows |

Shadow recipes (always one dark + one light, light from the top-left):
- Raised large (cards, panels): `10px 10px 22px shade-dark, -10px -10px 22px #FFF`, radius 26–32px
- Raised medium (buttons, header bar): `6px 6px 12px …, -6px -6px 12px …`, radius = half the height
- Raised small (icon buttons): `4px 4px 8px …, -4px -4px 8px …`, circles
- Inset: `inset 4px 4px 9px shade-inset, inset -4px -4px 9px #FFF`
- Accent buttons: accent fill + `5px 5px 12px #C9BFAF, -5px -5px 12px #FFF`
- A raised button that is "pressed"/active switches to the inset recipe.

### Dark theme (Light / Dark / Follow system switch stays)

Neumorphism needs a mid-tone, so dark mode is charcoal, not black. The Training tab's
palette is the reference: surface #202327, page #17191C, sunken panel #1A1D20,
ink #E9E6E2, muted #9A948C, accent #E8793A, ok #5FB37C, shade-dark #0F1113,
shade-inset #101214, shade-light #262A2F. Check every pair for ≥ 4.5:1 contrast.

## Typography (bundle locally; the app is offline)

Install as npm packages and import locally; never load fonts from the internet at runtime.
- Headings: **Fraunces** 600 — page titles 32–38px, section titles 26px.
- Body: **Figtree** 500, with 600/700 for emphasis — 15px base, small labels 13px/700 in muted.
- Mono: **IBM Plex Mono** 500 — model names, file paths, code, logs, numbers.

## Switches (fixed detail)

Toggle switches must stay visible when off:
- Track 64×34px, radius 17px, **1.5px border**: `outline` when off, `accent-dark` when on.
- Inset shadow on the track; accent fill when on.
- Knob 26px circle, surface colour, **1px `outline` border**, small drop shadow.
- Focus ring stays distinct: 2px accent, 3px offset.

## Chat (NeuChat)

- Left sidebar card: starts directly with the New conversation pill (accent dot +), then an
  inset search field, then conversations (the selected one inset with an accent dot), then
  round icon buttons at the bottom. No logo or app name in the sidebar.
- Header pill: conversation title and subtitle, Brain picker (inset pill; dropdown is a raised
  card with the current model inset), inset "Offline · local" status with a green dot, gear.
- Messages: user = INSET bubbles aligned right; Democritus = RAISED cards aligned left.
  Reply header shows only the model tag. No "Democritus" name on replies, no status pill.
- No right-hand Current state / Continuity panel. The chat fills the width. The RAM/VRAM and
  loaded-model readout lives in the gear menu.
- Composer: raised pill; attach button, inset text field, **coding-agent `</>` button (46px round,
  accent-filled when the agent is on)**, then the 54px round accent send button.

## The ferrofluid orb (the signature element)

One behavior, three places: small beside the reply being generated in chat, small beside the
coding agent's reply, and large on the Training tab. The drawing code is in all three
mockups (`draw()` and `tick()`); copy it rather than reinventing it. Build it ONCE as a shared
component (`<FerroOrb size intensity onEvent>`) and reuse it in all three places.

- A glossy black drop with **11 cones**. Each cone has a wide flowing base that necks into a
  sharp tip (`0.28·u^1.1 + 0.72·u^5` across its angular width).
- Each cone has its **own amplitude, width and slight angular drift**, driven by three sine waves
  at unrelated speeds, then sharpened twice with smoothstep so it snaps out and back rather than
  gliding. The result is deliberately irregular.
- The body shrinks as the cones extend, wobbles on three frequencies, and sags slightly (y × 1.04).
- **Bursts**: every 3.5–8.5 seconds all cones jump toward full length together, grow ~30% longer,
  and the halo flares; it decays over about a second.
- Shading: radial gradient #8e8e8e → #3c3c3c → #141414 → #030303, a sharp white specular
  highlight top-left, a soft stretched highlight across the top, and a warm accent rim light at
  the lower right that brightens with activity. On dark backgrounds add the orange halo behind it.
- **What drives it**: in chat, the rate of thinking tokens; in the coding agent, tool steps and
  tokens while it works (calm while it waits for approval); on the Training tab, training activity.
  Each event raises a target intensity, which decays every frame and is eased toward.
- Stop the animation loop when nothing is running. Respect `prefers-reduced-motion`
  (shorter, slower). Keep CPU use negligible.

### In chat

- 54px raised circle containing a 46px inset well containing the canvas (draw at 2×).
- While thinking: hide the thinking text, show "Working it out… N thoughts so far" and a
  "Show thoughts" button. When the answer starts the cones melt into a calm drop; afterwards
  show "Thought for X seconds · N thoughts, hidden".
- Setting: "Thinking display: Orb / Show thoughts / Both" (default Orb).

### In the coding agent

- Same 54px raised ring → 46px inset well → canvas (88px drawn, 44px shown) as in chat, beside the
  agent's reply card.
- **Working** (reading, searching, thinking): cones up, bursts every 3–7 seconds.
- **Waiting for your approval**: calm, slow breathing, no bursts. The agent has stopped; it is
  waiting for you, so the orb must not look busy.
- Approved / rejected / stopped / agent off: settles to a still drop.
- Labels: "The agent is working" / "Waiting for your approval" / "The agent is idle" / "The agent is off".

## Training tab (NeuTrain) — its own tab, dark theme

- Tab bar at the top: Chat · Memory · **Training** · Settings (active tab raised with an accent dot).
  "PC kept awake · safe to leave overnight" status at the right.
- Centre: the big orb (380px) in a sunken panel, with "Learning" / "Paused" underneath and a line
  saying what it is doing.
- Status window below: stage name and model version, progress bar, percent / time left / start and
  finish / checkpoint age, Pause (sinks in when paused) and Cancel, the six stages
  (Preflight · Lessons · Training · Packaging · Testing · Done), four stat tiles
  (Step, Loss, GPU temp, VRAM) and a live log.
- Pausing calms the fluid, stops bursts, greys the progress bar and frees the GPU for chat.

## Coding agent (NeuCode) — its own page, works like the chat

- Header: back button, "Coding agent" title, the chosen folder as an inset mono chip, Change folder,
  and an Agent on/off button (accent when on). No subtitle.
- **Safety strip** stated in plain words: "Sandbox: this folder only · Can't run commands ·
  No internet · Every write needs your approval · Step N of 20" (live) and a red **Stop agent**
  button. Stop is greyed out (inset) when nothing is running or waiting.
- **No file list panel.** The conversation fills the width, exactly like the chat window.
- Messages use the chat rules: your request is an INSET bubble on the right; the agent's answer is
  one RAISED reply card on the left with the orb beside it and the model tag (`qwen3-coder`).
- Inside the reply card, in order:
  1. While working: "Working it out… N thoughts so far". When done:
     "Worked for X seconds · N steps · N thoughts, hidden". Plus a Show / Hide thoughts toggle.
  2. Tool steps as small INSET rows, appearing one by one as they happen. The running step shows an
     accent dot and "Reading src/invoice.ts…"; finished steps show a green check and their result
     (time or match count).
  3. The explanation, then the proposed change: an inset block with a status dot, title
     ("Proposed change · waiting for you"), file and line, +N −N, and the diff (removed lines in fail
     tint with a left border, added lines in ok tint).
  4. Approve / Reject / Edit first with "Nothing is written to your folder until you approve."
     After a decision the buttons are REPLACED by the result, so a decision can't be flipped by a
     second click: "Applied ✓ · Written to src/invoice.ts · logged" with an **Undo** button, or
     "Rejected · Nothing was written. Tell the agent what to try instead."
  5. Thumbs up / down and the tok/s · tokens line, as in chat.
- Stop agent, or switching the agent off mid-run, ends the run with a red line in the card:
  "Stopped by you. Nothing was written to your folder."
- Composer: identical to chat (attach, inset text field with "Enter to send · Shift+Enter for a new
  line", 54px accent send) but **without the coding-agent `</>` button**, because you are already in
  the coding agent.

## Settings & security (NeuSettings)

- Lockdown: a 108px round raised button with a lock; pressed in and accent-coloured when locked,
  with the text changing to say what is blocked.
- Permissions list with the outlined switches above; all off by default.
- Security check: inset rows with glowing status lights (ok / fail / info), a short explanation,
  and fix buttons where they exist ("Create rules" in accent). Inset mini activity log below.
- Appearance (Light / Dark / Follow system) as a raised segmented pill with the active segment inset.

## Quality bar

- Text contrast ≥ 4.5:1 (3:1 at 24px+). Neumorphic edges are faint, so every interactive element
  needs a visible focus outline (2px accent, 3px offset) and a clear pressed state.
- Real `<button>`, `<a>`, `<input>` with labels; aria-labels on icon-only buttons; the orb is
  `role="img"` with a label that says whether it is thinking or idle.
- Minimum window 1024×700; panels wrap rather than overflow.
