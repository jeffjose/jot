# Design

How jot looks and speaks, and why. What has worked, kept for the next
thing built.

## The one rule

**Quiet chrome, one hue that means something.** The terminal is the
content; everything around it steps back. Color is spent on one thing at a
time, and the reader always knows what it means.

## Color

- **Surfaces come from the theme**, mixed from its background and text
  (`Theme::chrome()`): `panel`, `side`, `edge`, `field`, `hover`, and three
  weights of text, `text`, `dim`, `faint`. No surface has a color of its
  own; a new theme recolors everything.
- **The accent is for what you pick**: the menu row under the pointer, the
  chosen theme card, a switch that is on, the field being typed in, the
  one filled button. Never for where you are.
- **Where you are is neutral**: a lighter step behind a row or a caption, a
  thin light ring with a gap around a dot or a picture. Focus never tints.
- **The workspace's color is the one hue that identifies.** It lives on
  its square, its dot, the title bar's mark; nowhere else. jot itself has
  one too, its red (`#E54D2E`), for the quake window's square.
- **No status colors.** No green for done, red for failed, blue for
  running, amber for news. They read as another workspace's color, and
  most of the time say "fine". State, when it has to be shown, is a word
  or a glyph (`✓`, `0:42`).
- **Text on color picks dark or light by contrast** (WCAG), never by a
  brightness guess: cyan, teal and amber get dark text, violet and indigo
  light.

## Shape and size

- **One line a thing.** Rows are 26 px, captions 26, the foot bar 24. Never
  a two-line list item; a second fact goes dim at the right of the same
  line.
- **Small icons, tucked away.** Solar's linear set at a 2.25 line, 12 to
  13 px in a 24 to 28 px button. At rest the glyph alone; under the pointer
  a quiet lift (white at about 6 to 12%); chosen, the accent. No circles,
  no outlines.
- **Hairlines, not boxes.** Groups part by a 1 px `edge` line or space;
  cards only where a thing is picked from several (theme cards, formats).
- **Panels float** (`Canvas::panel`): radius 8, a 1 px edge, a soft shadow,
  a faint light along the top. Menus, tooltips, the palette, toasts.
- **Draw it right.** Dots are true circles (`Canvas::disc`), images are
  averaged down, never sampled; text is compared against VS Code at 4× before
  a font or blend change ships.

## Type

- **Geist** for the interface, **Fira Code** in the terminal. One UI size
  (13 px) with a small one for counts and keys; hierarchy comes from
  `text` / `dim` / `faint`, not from sizes.
- Section headings in Settings: a title and one dim line under it.
  Groups inside lists: small capitals, faint.

## Parts

- **The foot bar**: the workspace's square with its number (one day an
  icon you choose), its name, the folder of the pane you are in (dim), a dot
  for each of the machine's workspaces (the one you are in ringed), and at
  the right the machine as a button and the clock. One machine's set at a
  time.
- **Menus**: words only, keys dim at the right, `›` for a submenu, `✓` for
  what is in use, hairlines between groups. Right click gives one wherever
  something can be acted on (a bar, a pin, a tag).
- **Buttons**: one filled (accent) button a screen; the rest a quiet lift.
- **Choices**: every option visible in one segmented switch, up to about
  five (a "recommended" mark on the option it means, not beside the label);
  a dropdown with `⌄` beyond that; a toggle for on and off.
- **Numbers**: a slider for a range where "about here" matters (quality,
  volume), its number beside it to type exactly; a stepper (`−  value  +`)
  for small counts and sizes.
- **Numbers and badges**: a pane's number in an outlined box; a count in a
  pill only when it is a label (`current`).
- **Dropups** open from the foot bar's buttons, upward, their edges in line
  with the button.
- **The quake window** speaks the same language: its bar is the foot bar
  with a terminal list where the workspace dots would be.

## Words

- **Say it once.** The workspace's name is in the title bar or the foot
  bar's square; a thing's state is shown, not labelled.
- **No state labels.** Never "closed", "not open", "not saved", "(not
  shown)". A terminal is always running; we are only not looking at it.
- **No cribs.** Keys go in tooltips ("Close pane · Alt X"), not in a line
  of hints along the foot of a dialog.
- **Name things as people know them**: a pane is its project and folder
  first, the command it runs second. No counters like "4 new lines".
- **Plain verbs on buttons**: Restore, Convert, Save. Tooltips start with
  what the thing does.

## Working on it

- Mock first, on a design canvas, where comments can be left in place;
  build what is picked; show it running (screenshots under Xvfb) before
  calling it done.
- Each finished piece is committed and pushed.
- When a new part is needed, look for the jot part that already does the
  job (the foot bar, a menu, the sidebar row, a Settings row) before
  drawing a new one.
