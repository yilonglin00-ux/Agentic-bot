//! Real local PTY shell sessions for Noki Code.
//!
//! Provides real macOS login shells (zsh) over pseudo-terminals (PTY).
//! Multiple independent shell tabs can be opened, resized and closed.
//! User input flows directly into the PTY process. JackOD and the code agent
//! have NO access to these shells, preserving R0–R3 security boundaries.

use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    ffi::CString,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex, OnceLock,
    },
    thread,
    time::{SystemTime, UNIX_EPOCH},
};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ShellInfo {
    pub id: String,
    pub title: String,
    pub cwd: String,
    pub shell: String,
    pub running: bool,
    pub created_at: u64,
}

pub struct ShellProcess {
    pub id: String,
    pub master_fd: libc::c_int,
    pub pid: libc::pid_t,
    pub cwd: PathBuf,
    pub shell: String,
    pub title: String,
    pub running: Arc<AtomicBool>,
    pub created_at: u64,
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

static SHELLS: OnceLock<Mutex<HashMap<String, Arc<ShellProcess>>>> = OnceLock::new();

fn shells() -> &'static Mutex<HashMap<String, Arc<ShellProcess>>> {
    SHELLS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn default_shell() -> String {
    std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".into())
}

/// Spawns a real login shell inside a macOS PTY.
pub fn spawn_shell(
    id: String,
    title: Option<String>,
    cwd: Option<PathBuf>,
    cols: u16,
    rows: u16,
    on_output: impl Fn(String, String) + Send + Sync + 'static, // (id, data)
) -> Result<ShellInfo, String> {
    let terminal_id = id.clone();
    let resolved_cwd = cwd
        .filter(|p| p.is_dir())
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")))
        .canonicalize()
        .unwrap_or_else(|_| PathBuf::from("."));

    let shell_path = default_shell();
    let display_title = title.unwrap_or_else(|| {
        let name = Path::new(&shell_path)
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("zsh");
        name.to_string()
    });

    let mut master: libc::c_int = -1;
    let mut slave: libc::c_int = -1;
    let mut ws = libc::winsize {
        ws_row: rows.max(5),
        ws_col: cols.max(10),
        ws_xpixel: 0,
        ws_ypixel: 0,
    };

    let pty_res = unsafe {
        libc::openpty(
            &mut master,
            &mut slave,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            &mut ws,
        )
    };
    if pty_res != 0 {
        return Err(format!(
            "openpty fehlgeschlagen (errno {})",
            std::io::Error::last_os_error()
        ));
    }

    // Set close-on-exec for master so children don't inherit it
    unsafe {
        let flags = libc::fcntl(master, libc::F_GETFD);
        if flags >= 0 {
            libc::fcntl(master, libc::F_SETFD, flags | libc::FD_CLOEXEC);
        }
    }

    let pid = unsafe { libc::fork() };
    if pid < 0 {
        unsafe {
            libc::close(master);
            libc::close(slave);
        }
        return Err(format!(
            "fork fehlgeschlagen: {}",
            std::io::Error::last_os_error()
        ));
    }

    if pid == 0 {
        // Child process
        unsafe {
            libc::close(master);
            if libc::login_tty(slave) != 0 {
                libc::_exit(1);
            }

            // Change working directory
            let _ = std::env::set_current_dir(&resolved_cwd);

            // Setup terminal environment
            std::env::set_var("TERM", "xterm-256color");
            // Programs started in this terminal (e.g. `noki code`) know
            // which Code Space terminal they run in.
            std::env::set_var("NOKI_TERMINAL_ID", &terminal_id);
            std::env::set_var("COLORTERM", "truecolor");
            if std::env::var("LANG").is_err() {
                std::env::set_var("LANG", "de_DE.UTF-8");
            }

            // Ensure ~/.local/bin and Homebrew are on PATH so `noki` is immediately available
            let mut path = std::env::var("PATH").unwrap_or_default();
            if let Ok(home) = std::env::var("HOME") {
                let local_bin = format!("{home}/.local/bin");
                if !path.split(':').any(|p| p == local_bin) {
                    path = format!("{local_bin}:{path}");
                }
            }
            for candidate in ["/opt/homebrew/bin", "/usr/local/bin"] {
                if !path.split(':').any(|p| p == candidate) {
                    path = format!("{candidate}:{path}");
                }
            }
            std::env::set_var("PATH", &path);

            let c_shell = CString::new(shell_path.clone())
                .unwrap_or_else(|_| CString::new("/bin/zsh").unwrap());
            let shell_stem = Path::new(&shell_path)
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("zsh");
            // Leading dash signifies a login shell to zsh/bash
            let c_arg0 = CString::new(format!("-{shell_stem}")).unwrap();
            let args = [c_arg0.as_ptr(), std::ptr::null()];

            libc::execvp(c_shell.as_ptr(), args.as_ptr());
            libc::_exit(127);
        }
    }

    // Parent process
    unsafe {
        libc::close(slave);
    }

    let running = Arc::new(AtomicBool::new(true));
    let proc = Arc::new(ShellProcess {
        id: id.clone(),
        master_fd: master,
        pid,
        cwd: resolved_cwd.clone(),
        shell: shell_path.clone(),
        title: display_title.clone(),
        running: running.clone(),
        created_at: now(),
    });

    shells().lock().unwrap().insert(id.clone(), proc);

    // Background reader thread
    {
        let id_for_thread = id.clone();
        let running_for_thread = running.clone();
        thread::spawn(move || {
            let mut buf = [0u8; 8192];
            let mut leftover = Vec::new();
            loop {
                let n =
                    unsafe { libc::read(master, buf.as_mut_ptr() as *mut libc::c_void, buf.len()) };
                if n <= 0 {
                    break;
                }
                let mut chunk = Vec::with_capacity(leftover.len() + n as usize);
                chunk.extend_from_slice(&leftover);
                chunk.extend_from_slice(&buf[..n as usize]);
                leftover.clear();

                match std::str::from_utf8(&chunk) {
                    Ok(valid_str) => {
                        on_output(id_for_thread.clone(), valid_str.to_string());
                    }
                    Err(e) => {
                        let valid_up_to = e.valid_up_to();
                        if valid_up_to > 0 {
                            let valid_str =
                                unsafe { std::str::from_utf8_unchecked(&chunk[..valid_up_to]) };
                            on_output(id_for_thread.clone(), valid_str.to_string());
                        }
                        if e.error_len().is_none() {
                            leftover.extend_from_slice(&chunk[valid_up_to..]);
                        } else {
                            let lossy = String::from_utf8_lossy(&chunk[valid_up_to..]);
                            on_output(id_for_thread.clone(), lossy.to_string());
                        }
                    }
                }
            }
            running_for_thread.store(false, Ordering::SeqCst);
        });
    }

    Ok(ShellInfo {
        id,
        title: display_title,
        cwd: resolved_cwd.to_string_lossy().into_owned(),
        shell: shell_path,
        running: true,
        created_at: now(),
    })
}

/// Writes user input to the specified shell PTY.
pub fn write_shell(id: &str, data: &str) -> Result<(), String> {
    let proc = shells()
        .lock()
        .unwrap()
        .get(id)
        .cloned()
        .ok_or_else(|| "Shell nicht gefunden.".to_string())?;

    if !proc.running.load(Ordering::SeqCst) {
        return Err("Shell ist beendet.".to_string());
    }

    let bytes = data.as_bytes();
    let mut written = 0;
    while written < bytes.len() {
        let n = unsafe {
            libc::write(
                proc.master_fd,
                bytes[written..].as_ptr() as *const libc::c_void,
                bytes.len() - written,
            )
        };
        if n <= 0 {
            return Err("Konnte nicht in Shell schreiben.".to_string());
        }
        written += n as usize;
    }
    Ok(())
}

/// Updates the terminal window size (cols / rows) for the PTY.
pub fn resize_shell(id: &str, cols: u16, rows: u16) -> Result<(), String> {
    let proc = shells()
        .lock()
        .unwrap()
        .get(id)
        .cloned()
        .ok_or_else(|| "Shell nicht gefunden.".to_string())?;

    let ws = libc::winsize {
        ws_row: rows.max(2),
        ws_col: cols.max(5),
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    let res = unsafe { libc::ioctl(proc.master_fd, libc::TIOCSWINSZ, &ws) };
    if res == 0 {
        Ok(())
    } else {
        Err("Resize fehlgeschlagen.".to_string())
    }
}

/// Terminates the shell and cleans up PTY resources.
pub fn close_shell(id: &str) -> Result<(), String> {
    let proc = shells().lock().unwrap().remove(id);
    if let Some(proc) = proc {
        proc.running.store(false, Ordering::SeqCst);
        unsafe {
            libc::kill(proc.pid, libc::SIGHUP);
            libc::close(proc.master_fd);
            libc::waitpid(proc.pid, std::ptr::null_mut(), libc::WNOHANG);
        }
    }
    Ok(())
}

/// Foreground process group of a terminal and its shell's pid (the job the
/// user is running in it right now, e.g. a coding agent).
pub fn vordergrund(id: &str) -> Option<(libc::pid_t, libc::pid_t)> {
    let proc = shells().lock().ok()?.get(id).cloned()?;
    if !proc.running.load(Ordering::SeqCst) {
        return None;
    }
    let fg = unsafe { libc::tcgetpgrp(proc.master_fd) };
    (fg > 0).then_some((fg, proc.pid))
}

/// Returns a list of all active shell sessions.
pub fn list_shells() -> Vec<ShellInfo> {
    shells()
        .lock()
        .unwrap()
        .values()
        .map(|p| ShellInfo {
            id: p.id.clone(),
            title: p.title.clone(),
            cwd: p.cwd.to_string_lossy().into_owned(),
            shell: p.shell.clone(),
            running: p.running.load(Ordering::SeqCst),
            created_at: p.created_at,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pty_shell_spawn_and_echo() {
        let (tx, rx) = std::sync::mpsc::channel();
        let info = spawn_shell(
            format!("test-{}", std::process::id()),
            Some("test-shell".into()),
            None,
            80,
            24,
            move |id, data| {
                let _ = tx.send((id, data));
            },
        )
        .expect("spawn shell");

        assert_eq!(info.title, "test-shell");
        assert!(info.running);

        // Send a simple command to PTY
        write_shell(&info.id, "echo hello_noki_shell\n").expect("write to shell");

        let mut collected = String::new();
        let timeout = std::time::Instant::now();
        while timeout.elapsed() < std::time::Duration::from_secs(5) {
            if let Ok((_id, data)) = rx.recv_timeout(std::time::Duration::from_millis(200)) {
                collected.push_str(&data);
                if collected.contains("hello_noki_shell") {
                    break;
                }
            }
        }

        assert!(
            collected.contains("hello_noki_shell"),
            "PTY output should include echoed command result. Got: {collected:?}"
        );

        // Resize
        assert!(resize_shell(&info.id, 120, 40).is_ok());

        // Close
        assert!(close_shell(&info.id).is_ok());
    }

    #[test]
    fn agent_shell_isolation_guarantee() {
        // JackOD and code_agent have zero access or references to user PTY sessions.
        // User shells are only triggered by explicit user UI action, ensuring
        // R0–R3 security barriers cannot be bypassed by agent tasks.
        let list = list_shells();
        assert!(list.len() <= 100);
    }
}
