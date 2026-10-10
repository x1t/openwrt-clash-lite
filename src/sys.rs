//! The only module with OS-specific calls. Non-Linux builds get inert stubs so the pure logic
//! (config merge, rule parsing, back-off, ...) can be unit-tested on a developer machine.
use std::{path::Path, process::Command};

/// Runs a program and returns (succeeded, stdout + stderr). A missing program counts as failure.
pub fn run_cmd(program: &str, args: &[&str]) -> (bool, String) {
    match Command::new(program).args(args).output() {
        Ok(out) => {
            let mut text = String::from_utf8_lossy(&out.stdout).into_owned();
            text.push_str(&String::from_utf8_lossy(&out.stderr));
            (out.status.success(), text)
        }
        Err(e) => (false, e.to_string()),
    }
}

#[cfg(target_os = "linux")]
mod imp {
    use std::{
        fs, os::unix::fs::PermissionsExt, os::unix::process::CommandExt, path::Path,
        process::Command,
    };

    pub fn terminate(pid: u32) {
        // SAFETY: plain syscall with integer arguments.
        unsafe { libc::kill(pid as i32, libc::SIGTERM) };
    }

    /// Makes the kernel send SIGTERM to the child if we die, even by SIGKILL, so mihomo still tears
    /// down its own routes and firewall rules instead of leaving them behind.
    pub fn die_with_parent(cmd: &mut Command) {
        // SAFETY: the closure only calls prctl, which is async-signal-safe.
        unsafe {
            cmd.pre_exec(|| {
                libc::prctl(
                    libc::PR_SET_PDEATHSIG,
                    libc::SIGTERM as libc::c_ulong,
                    0,
                    0,
                    0,
                );
                Ok(())
            });
        }
    }

    /// (pid, comm) of every process, from /proc.
    pub fn list_procs() -> Vec<(u32, String)> {
        let Ok(entries) = fs::read_dir("/proc") else {
            return Vec::new();
        };
        entries
            .flatten()
            .filter_map(|e| {
                let pid = e.file_name().to_str()?.parse::<u32>().ok()?;
                let comm = fs::read_to_string(e.path().join("comm")).ok()?;
                Some((pid, comm.trim().to_string()))
            })
            .collect()
    }

    pub fn restrict_file(path: &Path) {
        let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o600));
    }

    /// Symlink: costs no flash space and follows the source if it is updated.
    pub fn link_file(src: &Path, dst: &Path) -> std::io::Result<()> {
        std::os::unix::fs::symlink(src, dst)
    }

    pub fn make_executable(path: &Path) -> std::io::Result<()> {
        fs::set_permissions(path, fs::Permissions::from_mode(0o755))
    }

    pub fn rss_kb(pid: u32) -> Option<u64> {
        super::parse_rss(&fs::read_to_string(format!("/proc/{pid}/status")).ok()?)
    }

    /// Bytes available to unprivileged writers on the filesystem holding `path`.
    pub fn free_bytes(path: &Path) -> Option<u64> {
        use std::os::unix::ffi::OsStrExt;
        let c = std::ffi::CString::new(path.as_os_str().as_bytes()).ok()?;
        // SAFETY: statvfs only writes into the zeroed struct; `c` is a valid C string.
        let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
        if unsafe { libc::statvfs(c.as_ptr(), &mut st) } != 0 {
            return None;
        }
        Some(st.f_bavail as u64 * st.f_frsize as u64)
    }
}

#[cfg(not(target_os = "linux"))]
mod imp {
    use std::{path::Path, process::Command};

    pub fn terminate(_pid: u32) {}
    pub fn die_with_parent(_cmd: &mut Command) {}
    pub fn list_procs() -> Vec<(u32, String)> {
        Vec::new()
    }
    pub fn restrict_file(_path: &Path) {}
    pub fn link_file(src: &Path, dst: &Path) -> std::io::Result<()> {
        std::fs::copy(src, dst).map(|_| ())
    }
    pub fn make_executable(_path: &Path) -> std::io::Result<()> {
        Ok(())
    }
    pub fn rss_kb(_pid: u32) -> Option<u64> {
        None
    }
    pub fn free_bytes(_path: &Path) -> Option<u64> {
        None
    }
}

pub use imp::{
    die_with_parent, free_bytes, link_file, list_procs, make_executable, restrict_file, rss_kb,
    terminate,
};

/// Resident memory in kB from the text of `/proc/<pid>/status` (`VmRSS:   12345 kB`).
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn parse_rss(status: &str) -> Option<u64> {
    let line = status.lines().find(|l| l.starts_with("VmRSS:"))?;
    line.split_whitespace().nth(1)?.parse().ok()
}

pub fn file_exists(path: &str) -> bool {
    Path::new(path).exists()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rss_is_read_from_the_vmrss_line() {
        let status = "Name:\tclash_meta\nVmPeak:\t  900000 kB\nVmRSS:\t   41236 kB\nThreads:\t9\n";
        assert_eq!(parse_rss(status), Some(41236));
        assert_eq!(
            parse_rss("Name:\tkthreadd\n"),
            None,
            "kernel threads have no VmRSS"
        );
    }
}
