# jterm

A small, fast terminal for Linux, on X11 and Wayland. It splits its
window into panes, side by side or one above another, nested as you like,
and keeps sets of panes as workspaces, each in a project's folder. The
keys are GNU screen's, moved onto Alt. When jterm starts again it brings
the workspaces back as you left them: every pane in its place, its
command started afresh in the folder it was in.

The terminal itself is [libghostty-vt](https://mitchellh.com/writing/libghostty-1-0)
1.0, Ghostty's, built from source and linked in. Everything else, the
window, the text and the panes, is jterm's own: no toolkit and no web
view, the pixels drawn on the CPU as jog's and hotshot's are, and only
the rows that changed.

## Build

It needs Zig 0.17 to build libghostty-vt.

```sh
git clone --recurse-submodules git@github.com:jeffjose/jterm.git
cd jterm
cargo build --release
cp target/release/jterm ~/.local/bin/
```

The first build compiles libghostty-vt, which takes about two minutes;
later builds take seconds.

## Use

```sh
jterm            # the workspaces as you left them
jterm --fresh    # the workspaces in the config instead
```

| Key | |
|---|---|
| `Alt s` / `Alt v` | split: one above another / side by side |
| `Alt h` `j` `k` `l` | move to the pane left, down, up, right |
| `Ctrl t` | a new pane to the right of this one |
| `Ctrl w`, `Alt x` | close the pane |
| `Ctrl Shift t` | bring back the pane closed last, as a tab comes back |
| `Alt Shift m` | the pane alone, and back |
| `Alt` arrows, `Alt =` | resize, make all equal |
| `Alt 1`–`9`, `Alt ,` `Alt .` | go to a workspace, the one before or after |
| `Alt Shift n` | a new workspace, in this pane's folder |
| ``Alt ` `` | the roaming terminal |
| `Ctrl k` | find a workspace, pane or project folder |
| `Ctrl p` | run a command |
| `Ctrl ,` | settings: theme, text size, captions, status line |
| `Ctrl Shift c` / `v` | copy / paste |
| `Shift PageUp` / `PageDown` | scroll back |
| `Ctrl =` `-` `0` | text larger, smaller, as it was |

A selection is copied as you make it, and a middle click pastes it. A
double click selects a word. The line between two panes can be dragged.

Each pane has a caption along its bottom, as screen's regions do: red for
the one you are in. It shows the title the program set, or else the
folder and what is running in it. The status line lists the workspaces;
one whose panes printed while you were elsewhere is amber, with an `@`.

The roaming terminal is one terminal for all workspaces, shown over
whichever you are in and hidden again with the same key. It keeps its own
folder, for wandering about without disturbing a workspace's panes.

## Workspaces

`~/.config/jterm/config.toml` is written on the first run with every
setting in it commented out. Workspaces there are the ones jterm starts
with when it has none saved:

```toml
[[workspace]]
name = "jterm"
cwd = "~/scripts/jterm"
panes = ["claude", "claude", ""]     # "" is your shell; side by side

[[workspace]]
name = "notes"
cwd = "~/notes"
panes = ["", ""]
split = "h"                          # one above another

[[workspace]]
name = "hotshot"
cwd = "~/scripts/hotshot"
# v for side by side, h for stacked, nested as deep as you like
layout = { v = [ { h = ["claude", "claude"] }, "", "" ], sizes = [2, 1, 1] }
```

A command is run by your shell, and the shell stays on after it ends, in
the same folder. As you work, jterm saves the workspaces to
`~/.local/state/jterm/session.toml`.

## Speed

`seq 1 3000000`, 21 MB of output, scrolls through in 0.87 s in a
1400×900 window (i7-12700K, X11).
