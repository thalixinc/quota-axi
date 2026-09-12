//! Process-table + subprocess helpers mirroring `src/lib/process.ts`.

use std::io::Read;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// Run a command and return its stdout; rejects on non-zero exit or timeout.
/// Stdout is drained on a side thread so a large `ps` table cannot deadlock the pipe.
pub fn exec_file_text(command: &str, args: &[String], timeout_ms: u64) -> Result<String, String> {
    let mut child = Command::new(command)
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("{command}: {e}"))?;

    let stdout = child.stdout.take();
    let reader = std::thread::spawn(move || {
        let mut out = String::new();
        if let Some(mut s) = stdout {
            let _ = s.read_to_string(&mut out);
        }
        out
    });

    let deadline = Instant::now() + Duration::from_millis(timeout_ms);
    let status = loop {
        match child.try_wait().map_err(|e| format!("{command}: {e}"))? {
            Some(status) => break status,
            None => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    let _ = reader.join();
                    return Err(format!("{command} timed out"));
                }
                std::thread::sleep(Duration::from_millis(10));
            }
        }
    };
    let out = reader
        .join()
        .map_err(|_| format!("{command}: stdout reader panicked"))?;
    if !status.success() {
        return Err(format!("{command} exited with {status}"));
    }
    Ok(out)
}

/// Linux procps rejects the BSD `-x` selector alongside `-u`, so only macOS/BSD gets it.
/// Both list the current user's processes including ones without a controlling terminal.
pub fn current_user_process_list_args(effective_uid: u32) -> Vec<String> {
    let mut args: Vec<String> = if cfg!(target_os = "linux") {
        vec!["-u".to_string(), effective_uid.to_string()]
    } else {
        vec![
            "-x".to_string(),
            "-u".to_string(),
            effective_uid.to_string(),
        ]
    };
    args.push("-o".to_string());
    args.push("pid=,command=".to_string());
    args
}

/// The current user's effective uid (matches `process.geteuid()`).
pub fn effective_uid() -> Option<u32> {
    // SAFETY: geteuid is a thin syscall wrapper with no preconditions.
    let uid = unsafe { libc::geteuid() };
    if uid == u32::MAX {
        None
    } else {
        Some(uid)
    }
}
