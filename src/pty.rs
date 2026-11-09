//! The program in a pane: a shell, or a command run by the shell, on a
//! pseudo-terminal of its own. What it writes is read on a thread of its
//! own and handed to the window's thread, which never waits on it.

use std::ffi::CString;
use std::io;
use std::os::fd::RawFd;
use std::path::{Path, PathBuf};

/// A program on a pseudo-terminal of this process's: the session server's.
pub struct Local {
    /// Our end of the pseudo-terminal.
    pub fd: RawFd,
    /// The shell's process.
    pub pid: libc::pid_t,
}

/// What a reader thread hands over: a pane's output, or that it ended.
pub enum Output {
    Bytes(u64, Vec<u8>),
    Exit(u64),
    /// Whether it reads keys one at a time, now (see [`Local::raw`]).
    Mode(u64, bool),
    /// A queued command typed at its prompt by the session.
    Ran(u64, String),
    /// Lines a /tee has written; that it ended, and why if it failed.
    TeeLines(u64, u64),
    TeeEnd(u64, u64, String),
    /// What its program is doing, changed.
    Proc(u64, Proc),
    /// A machine's session reached again, with what it holds (by its
    /// own names).
    Back(usize, Vec<crate::mux::Held>),
    /// A machine could not be reached, and why.
    Failed(usize, String),
    /// A machine's workspaces this window does not show.
    Listing(usize, Vec<crate::mux::Listed>, Vec<(u64, String, bool, String)>, Vec<(u64, String)>),
    /// Panes of a machine taken into this window.
    Taken(usize, Vec<crate::mux::Held>),
    /// A machine's shared workspaces, and the terminals in them now this
    /// window's.
    Workspaces(usize, Vec<String>, Vec<crate::mux::Held>),
    /// A machine's terminals' numbers, as its session keeps them.
    Numbers(usize, Vec<(u64, u32)>),
    /// The size its program writes for now, set by whichever window
    /// was typed into last.
    Size(u64, u16, u16),
    /// A machine's server is being swapped for a newer one.
    Restarting(usize),
    /// The connection to a machine's session ended.
    Gone(usize),
}

fn shell() -> String {
    std::env::var("SHELL").ok().filter(|s| !s.is_empty()).unwrap_or_else(|| "/bin/sh".into())
}

fn winsize(cols: u16, rows: u16, w: u32, h: u32) -> libc::winsize {
    libc::winsize { ws_col: cols, ws_row: rows, ws_xpixel: (w.min(65535)) as u16, ws_ypixel: (h.min(65535)) as u16 }
}

impl Local {
    /// Starts `command` (or, when empty, a login shell) in `cwd`. A
    /// command is run by the shell, and the shell stays on after it ends,
    /// in the same folder, as if it had been typed.
    /// `desktop`: the display the window asking for it is on, set (or, when
    /// that window has none of a name, unset) over what the session has.
    pub fn spawn(command: &str, cwd: &Path, cols: u16, rows: u16, px: (u32, u32), desktop: &[(CString, Option<CString>)]) -> io::Result<Local> {
        let shell = shell();
        let name = Path::new(&shell).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "sh".into());
        // Everything the child needs is made before the fork: after it,
        // only calls safe in a forked child are made.
        let program = CString::new(shell.clone()).unwrap();
        let argv: Vec<CString> = if command.trim().is_empty() {
            vec![CString::new(format!("-{name}")).unwrap()]
        } else {
            let line = format!("{command}; exec {shell}");
            vec![CString::new(name).unwrap(), CString::new("-c").unwrap(), CString::new(line).unwrap_or_else(|_| CString::new("true").unwrap())]
        };
        let mut argv_ptrs: Vec<*const libc::c_char> = argv.iter().map(|a| a.as_ptr()).collect();
        argv_ptrs.push(std::ptr::null());
        let dir = if cwd.is_dir() { cwd.to_path_buf() } else { home() };
        let dir = CString::new(dir.as_os_str().as_encoded_bytes()).unwrap_or_else(|_| CString::new("/").unwrap());
        let env: Vec<(CString, CString)> = [("TERM", "xterm-256color"), ("COLORTERM", "truecolor"), ("TERM_PROGRAM", "jot")]
            .iter()
            .map(|(k, v)| (CString::new(*k).unwrap(), CString::new(*v).unwrap()))
            .collect();
        let pwd = (CString::new("PWD").unwrap(), dir.clone());

        let mut fd: RawFd = -1;
        let mut ws = winsize(cols, rows, px.0, px.1);
        let pid = unsafe { libc::forkpty(&mut fd, std::ptr::null_mut(), std::ptr::null_mut(), &mut ws) };
        if pid < 0 {
            return Err(io::Error::last_os_error());
        }
        if pid == 0 {
            unsafe {
                // Rust ignores SIGPIPE; the shell and what it runs should not.
                for sig in [libc::SIGPIPE, libc::SIGCHLD, libc::SIGINT, libc::SIGQUIT, libc::SIGTERM, libc::SIGHUP] {
                    libc::signal(sig, libc::SIG_DFL);
                }
                let empty: libc::sigset_t = std::mem::zeroed();
                libc::sigprocmask(libc::SIG_SETMASK, &empty, std::ptr::null_mut());
                libc::chdir(dir.as_ptr());
                for (k, v) in env.iter().chain(std::iter::once(&pwd)) {
                    libc::setenv(k.as_ptr(), v.as_ptr(), 1);
                }
                // Not logged in over SSH, whatever started the session (a
                // window opened from an SSH login, or `jot bridge`): the
                // session outlives that connection, and prompts that say
                // "ssh" by these would say so on this machine's own panes.
                for k in [c"SSH_CONNECTION", c"SSH_CLIENT", c"SSH_TTY"] {
                    libc::unsetenv(k.as_ptr());
                }
                for (k, v) in desktop {
                    match v {
                        Some(v) => libc::setenv(k.as_ptr(), v.as_ptr(), 1),
                        None => libc::unsetenv(k.as_ptr()),
                    };
                }
                libc::execvp(program.as_ptr(), argv_ptrs.as_ptr());
                libc::_exit(127);
            }
        }
        unsafe {
            let flags = libc::fcntl(fd, libc::F_GETFD);
            libc::fcntl(fd, libc::F_SETFD, flags | libc::FD_CLOEXEC);
        }
        Ok(Local { fd, pid })
    }

    /// Reads what the program writes on a thread of its own, handing it
    /// over with `send`, which also wakes the window.
    pub fn read_on_thread(&self, id: u64, send: impl Fn(Output) + Send + 'static) {
        // Closed on exec, as the server swapped for a newer one leaves none.
        let fd = unsafe { libc::fcntl(self.fd, libc::F_DUPFD_CLOEXEC, 0) };
        std::thread::Builder::new()
            .name(format!("pty-{id}"))
            .spawn(move || {
                let mut buf = vec![0u8; 64 * 1024];
                loop {
                    let n = unsafe { libc::read(fd, buf.as_mut_ptr() as *mut libc::c_void, buf.len()) };
                    if n > 0 {
                        send(Output::Bytes(id, buf[..n as usize].to_vec()));
                    } else if n < 0 && io::Error::last_os_error().kind() == io::ErrorKind::Interrupted {
                        continue;
                    } else {
                        break;
                    }
                }
                unsafe { libc::close(fd) };
                send(Output::Exit(id));
            })
            .expect("pty reader thread");
    }

    pub fn write_fd(fd: RawFd, mut bytes: &[u8]) {
        while !bytes.is_empty() {
            let n = unsafe { libc::write(fd, bytes.as_ptr() as *const libc::c_void, bytes.len()) };
            if n < 0 {
                if io::Error::last_os_error().kind() == io::ErrorKind::Interrupted {
                    continue;
                }
                return;
            }
            bytes = &bytes[n as usize..];
        }
    }

    pub fn resize(&self, cols: u16, rows: u16, px: (u32, u32)) {
        let ws = winsize(cols, rows, px.0, px.1);
        unsafe { libc::ioctl(self.fd, libc::TIOCSWINSZ, &ws) };
    }

    /// Whether the program reads keys one at a time (ICANON off): a
    /// shell at its prompt, vim; not one reading whole lines.
    pub fn raw(&self) -> bool {
        let mut t: libc::termios = unsafe { std::mem::zeroed() };
        (unsafe { libc::tcgetattr(self.fd, &mut t) }) == 0 && t.c_lflag & libc::ICANON == 0
    }
}

impl Drop for Local {
    fn drop(&mut self) {
        unsafe {
            libc::kill(self.pid, libc::SIGHUP);
            libc::close(self.fd);
            // Reaped without waiting; one not yet gone is reaped by the
            // next pane's, or when jot ends.
            libc::waitpid(self.pid, std::ptr::null_mut(), libc::WNOHANG);
        }
    }
}

/// Reaps children that have ended, so none are left as zombies.
pub fn reap() {
    unsafe { while libc::waitpid(-1, std::ptr::null_mut(), libc::WNOHANG) > 0 {} }
}

pub fn home() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/"))
}

/// What a pane's program is doing, as read where it runs: its folder, what
/// has its terminal, and whether the shell is running something.
#[derive(Clone, Default, PartialEq, Debug)]
pub struct Proc {
    pub cwd: String,
    /// The name of what is in the foreground: the shell, waiting.
    pub fg: String,
    /// The foreground process group.
    pub pgrp: i32,
    pub busy: bool,
}

impl Local {
    /// What its program is doing now, from /proc.
    pub fn proc(&self) -> Proc {
        let pgrp = unsafe { libc::tcgetpgrp(self.fd) };
        let cwd = std::fs::read_link(format!("/proc/{}/cwd", self.pid)).map(|p| p.display().to_string()).unwrap_or_default();
        let kids = std::fs::read_to_string(format!("/proc/{0}/task/{0}/children", self.pid)).unwrap_or_default();
        let busy = (pgrp > 0 && pgrp != self.pid) || !kids.trim().is_empty();
        let comm = |pid: &str| std::fs::read_to_string(format!("/proc/{pid}/comm")).map(|c| c.trim_end().to_string()).ok();
        let who = if pgrp > 0 { pgrp } else { self.pid };
        // A command run by `shell -c` has no job control: look for it
        // among the shell's children instead.
        let fg = if who == self.pid { kids.split_whitespace().next().and_then(comm).or_else(|| comm(&who.to_string())) } else { comm(&who.to_string()) };
        Proc { cwd, fg: fg.unwrap_or_default(), pgrp, busy }
    }
}

/// A pane's program as the window has it: in the session's server, on
/// this machine or another, reached through the session. What it is
/// doing is as the server last said.
pub struct Pty {
    /// Its name in its session (not the window's: see [`crate::mux::app_id`]).
    pub pane: u64,
    /// Which of the window's machines it is on.
    pub machine: usize,
    /// The shell's process, on the machine it runs on.
    pub pid: libc::pid_t,
    pub mux: std::sync::Arc<crate::mux::Client>,
    /// Whether it reads keys one at a time, as the session last said.
    pub raw: bool,
    pub proc: Proc,
    /// Ended when it goes from the window (End, Quit and end all); else
    /// only no longer shown here, its program running on.
    pub end: bool,
    /// Every pane of this window showing it, the first answering what the
    /// program asks of its terminal: the terminal let go
    /// of only when the last goes.
    pub views: std::rc::Rc<std::cell::RefCell<Vec<u64>>>,
    /// Which of them this is (its id in the window).
    pub view: u64,
    /// Whether this window answers what the program asks of its terminal:
    /// the session has it at this window's size. Another window showing it
    /// at another size answers while it has it at that one, and only it: an
    /// answer reaches the session as a key typed does, and makes the
    /// answering window's size the pane's, so two answering at two sizes
    /// resized it back and forth, the program drawing itself again at each
    /// and asking again.
    pub speaks: std::rc::Rc<std::cell::Cell<bool>>,
}

impl Pty {
    pub fn write(&self, bytes: &[u8]) {
        if !bytes.is_empty() {
            self.mux.input(self.pane, bytes);
        }
    }

    pub fn resize(&self, cols: u16, rows: u16, px: (u32, u32)) {
        self.mux.resize(self.pane, cols, rows, px);
    }

    /// The process group in the foreground of its terminal: the shell's
    /// own when it waits for a command, else the job's.
    pub fn foreground_group(&self) -> libc::pid_t {
        self.proc.pgrp
    }

    /// The folder the shell is in now.
    pub fn cwd(&self) -> Option<PathBuf> {
        (!self.proc.cwd.is_empty()).then(|| PathBuf::from(&self.proc.cwd))
    }

    /// The name of what is running in the foreground: the shell itself
    /// when it is waiting for a command.
    pub fn foreground(&self) -> Option<String> {
        (!self.proc.fg.is_empty()).then(|| self.proc.fg.clone())
    }

    /// Whether the shell is running something.
    pub fn busy(&self) -> bool {
        self.proc.busy
    }

    /// A signal to what it is running (its foreground job), or with
    /// `job` false to the shell and all it started.
    pub fn signal(&self, sig: i32, job: bool) {
        self.mux.signal(self.pane, sig, job);
    }

    /// Whether it runs on another machine.
    pub fn remote(&self) -> bool {
        self.mux.host.is_some()
    }
}

impl Drop for Pty {
    /// No longer shown here, its program running on; or, when asked to
    /// end, its shell hung up on. Nothing, when the window is only going.
    fn drop(&mut self) {
        if self.end {
            self.mux.close(self.pane);
        }
        let mut views = self.views.borrow_mut();
        views.retain(|v| *v != self.view);
        if !self.end && views.is_empty() {
            self.mux.release(self.pane);
        }
    }
}
