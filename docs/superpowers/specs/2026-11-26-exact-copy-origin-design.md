# Fly new cards in from the exact source

## Intent

Every new card (text, screenshot, image or files) should visibly fly out of the
spot on screen it was copied from, so it reads as "that thing became a clip".

## Why it misses today

1. The compositor shifts the whole picture up while the line glides down (up to
   330 DIP). The launch point ignores this.
2. The source gets the card's top edge, plus rope sag and the 1.25 start scale,
   not its centre.
3. A launch is never closer than 40 DIP below the line, so sources near the top
   are pushed down.
4. Copying something already on the line only slides the old card to the front.
   It never flies from the source.
5. `origin.rs` only knows text focus (first line of the selection, caret,
   focused control).

## Approaches

| | What | Trade-off |
|---|---|---|
| A | Fix the flight maths only | Small; screenshots, images and files stay guesses |
| **B (chosen)** | A, plus per-type detection using recent mouse gestures | Covers the common cases; watches mouse buttons system-wide |
| C | B, plus searching the screen for the copied image | More exact for some captures; reads the screen, costs CPU |

## Design

### Gestures (new `src/gesture.rs`)

Raw Input (`RIDEV_INPUTSINK`) delivers mouse button events to Dryer's window.
It remembers the last drag (press and release points), left click and right
click, each with its time. Positions stay in memory and are never saved. No
keyboard monitoring, and no low-level hook that could slow the mouse.

### Choosing the source (`src/origin.rs`)

Runs before the line is revealed. The first candidate that applies wins. The
result is the point where the card's centre starts.

| Copied | Candidates, in order |
|---|---|
| Image | Drag (last 5 s) whose size matches the image, within 4 px: its centre. Window under a recent click whose size matches: its centre. Right click: the image element there, else the point. Image as large as the monitor: screen centre. |
| Files | Right click: the point. Selected items of the focused list: centre of their union. Focused control. |
| Text and other | Centre of all visible selection rectangles. Left click (last 1.5 s, e.g. a "Copy" button). Right click. Drag (last 30 s): its centre. Caret. Focused control. |

Last resort for all: the pointer. A right click counts for 15 s and only if at
most one left click (the menu item) followed it. All UI Automation calls keep
the 150 ms timeout.

### Flight (`main.rs`, `render.rs`, `physics.rs`)

- The line's slide is drawn by Dryer instead of the compositor offset, so a
  flying card can ignore it. A card starts exactly at the source in screen
  coordinates and blends into the line's position as it lands.
- The card's centre, allowing for rope sag and start scale, is placed on the
  source. No minimum distance; a card may fly down from above the line.
- A re-copied clip flies from the source like a new one. Restoring by clicking a
  card is unchanged.

## Goals and limits

- Goal: the first drawn frame of a flight is centred on the detected source,
  whether the line was hidden or shown.
- Primary monitor only; sources elsewhere start at the nearest side edge.
- Keyboard-only captures with no mouse trail (Print Screen) start at the screen
  centre. Keyboard copies in apps without accessibility data start at the caret
  or focused control.
- Not doing: screen pixel reading, keyboard hooks, new settings.

## Testing

Unit tests for gesture tracking, candidate selection and flight start/end
positions (all pure functions). Then a manual run: text, region and window
snips, browser "Copy image", Explorer copy by keyboard and by menu, each with
the line hidden and shown.
