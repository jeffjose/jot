# <img src="assets/jot.svg" width="28" align="top"> jot

A terminal emulator with built-in splits and workspaces, for Linux (X11
and Wayland). Built on [libghostty-vt](https://mitchellh.com/writing/libghostty-1-0).

<p align="center"><img src="docs/hero.png" width="900" alt="jot: the sidebar with each machine's terminals, three panes, and the bar with the workspace you are in"></p>

```sh
git clone --recurse-submodules git@github.com:jeffjose/jot.git
cd jot && cargo build --release   # needs Zig 0.17
target/release/jot setup          # add it, with its icon, to your programs
```

### Terminals that stay

Every terminal lives in a session on its machine, so closing a pane or the
window only stops looking at it. Add a machine and its terminals and
workspaces sit beside this one's; the bar shows one machine's workspaces at
a time.

<p align="center"><img src="docs/machines.png" width="520" alt="The bar's machine list: woodwind, cinderblock, skyfall"></p>

### Quake

`jot --quake` drops a terminal from the top of the screen, laid out once for
this desk. Its bar lists every terminal of a machine; pick one and the pane
shows it.

<p align="center"><img src="docs/quake.png" width="640" alt="The quake window's bar: jot's mark and a machine's terminals"></p>

Keys are listed in Settings (`Ctrl ,`); `jot --help` lists the rest.
