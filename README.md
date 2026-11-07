# <img src="assets/jterm.svg" width="28" align="top"> jterm

A terminal emulator with built-in splits and workspaces, for Linux (X11
and Wayland). Built on [libghostty-vt](https://mitchellh.com/writing/libghostty-1-0).

```sh
git clone --recurse-submodules git@github.com:jeffjose/jterm.git
cd jterm && cargo build --release   # needs Zig 0.17
target/release/jterm setup          # add it, with its icon, to your programs
```

Keys are listed in Settings (`Ctrl ,`); `jterm --help` lists the rest.
