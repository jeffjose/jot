//! The program in a pane: a shell, or a command run by the shell, on a
//! pseudo-terminal of its own. What it writes is read on a thread of its
//! own and handed to the window's thread, which never waits on it.

use std::ffi::CString;
use std::io;
use std::os::fd::RawFd;
use std::path::{Path, PathBuf};

pub struct Pty {
    /// Our end of the pseudo-terminal.
    pub fd: RawFd,
    /// The shell's process.
    pub pid: libc::pid_t,
}

/// What a reader thread hands over: a pane's output, or that it ended.
pub enum Output {
    Bytes(u64, Vec<u8>),
    Exit(u64),
}

fn shell() -> String {
    std::env::var("SHELL").ok().filter(|s| !s.is_empty()).unwrap_or_else(|| "/bin/sh".into())
}

fn winsize(cols: u16, rows: u16, w: u32, h: u32) -> libc::winsize {
    libc::winsize { ws_col: cols, ws_row: rows, ws_xpixel: (w.min(65535)) as u16, ws_ypixel: (h.min(65535)) as u16 }
}

impl Pty {
    /// Starts `command` (or, when empty, a login shell) in `cwd`. A
    /// command is run by the shell, and the shell stays on after it ends,
    /// in the same folder, as if it had been typed.
    pub fn spawn(command: &str, cwd: &Path, cols: u16, rows: u16, px: (u32, u32)) -> io::Result<Pty> {
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
        let env: Vec<(CString, CString)> = [("TERM", "xterm-256color"), ("COLORTERM", "truecolor"), ("TERM_PROGRAM", "jterm")]
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
                libc::execvp(program.as_ptr(), argv_ptrs.as_ptr());
                libc::_exit(127);
            }
        }
        unsafe {
            let flags = libc::fcntl(fd, libc::F_GETFD);
            libc::fcntl(fd, libc::F_SETFD, flags | libc::FD_CLOEXEC);
        }
        Ok(Pty { fd, pid })
    }

    /// Reads what the program writes on a thread of its own, handing it
    /// over with `send`, which also wakes the window.
    pub fn read_on_thread(&self, id: u64, send: impl Fn(Output) + Send + 'static) {
        let fd = unsafe { libc::dup(self.fd) };
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

    pub fn write(&self, mut bytes: &[u8]) {
        while !bytes.is_empty() {
            let n = unsafe { libc::write(self.fd, bytes.as_ptr() as *const libc::c_void, bytes.len()) };
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

    /// The folder the shell is in now.
    pub fn cwd(&self) -> Option<PathBuf> {
        std::fs::read_link(format!("/proc/{}/cwd", self.pid)).ok()
    }

    /// The name of what is running in the foreground: the shell itself
    /// when it is waiting for a command.
    pub fn foreground(&self) -> Option<String> {
        let pgrp = unsafe { libc::tcgetpgrp(self.fd) };
        let pid = if pgrp > 0 { pgrp } else { self.pid };
        let mut comm = std::fs::read_to_string(format!("/proc/{pid}/comm")).ok()?;
        comm.truncate(comm.trim_end().len());
        // A command run by `shell -c` has no job control: look for it
        // among the shell's children instead.
        if pid == self.pid {
            if let Ok(kids) = std::fs::read_to_string(format!("/proc/{0}/task/{0}/children", self.pid)) {
                if let Some(kid) = kids.split_whitespace().next() {
                    if let Ok(c) = std::fs::read_to_string(format!("/proc/{kid}/comm")) {
                        return Some(c.trim_end().to_string());
                    }
                }
            }
        }
        Some(comm)
    }

    /// Whether the shell is running something.
    pub fn busy(&self) -> bool {
        let pgrp = unsafe { libc::tcgetpgrp(self.fd) };
        (pgrp > 0 && pgrp != self.pid)
            || std::fs::read_to_string(format!("/proc/{0}/task/{0}/children", self.pid)).is_ok_and(|k| !k.trim().is_empty())
    }
}

impl Drop for Pty {
    fn drop(&mut self) {
        unsafe {
            libc::kill(self.pid, libc::SIGHUP);
            libc::close(self.fd);
            // Reaped without waiting; one not yet gone is reaped by the
            // next pane's, or when jterm ends.
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
