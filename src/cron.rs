//! Daily subscription update: one marked line in root's crontab, kept in sync with UCI
//! (`auto_update`, `update_time`). Every other line of the crontab is left exactly as it is.
use crate::{config::Paths, state::Res, state::State, sys};
use std::fs;

const MARK: &str = "# openwrt-clash-lite";

/// (hour, minute) of an `HH:MM` time.
pub fn parse_time(s: &str) -> Res<(u8, u8)> {
    let bad = || format!("update time {s:?} must look like 06:10");
    let (h, m) = s.trim().split_once(':').ok_or_else(bad)?;
    let (h, m): (u8, u8) = (h.parse().map_err(|_| bad())?, m.parse().map_err(|_| bad())?);
    if h > 23 || m > 59 || s.trim().len() != 5 {
        return Err(bad().into());
    }
    Ok((h, m))
}

fn job_line(hour: u8, minute: u8) -> String {
    format!("{minute} {hour} * * * /usr/bin/openwrt-clash-lite auto-update >/dev/null 2>&1 {MARK}")
}

/// The crontab without our old line, plus `line` if given.
fn with_line(crontab: &str, line: Option<&str>) -> String {
    let mut out: String = crontab
        .lines()
        .filter(|l| !l.ends_with(MARK))
        .map(|l| format!("{l}\n"))
        .collect();
    if let Some(l) = line {
        out.push_str(l);
        out.push('\n');
    }
    out
}

/// Writes the crontab when it has to change and restarts cron so it reads it; returns whether
/// anything changed.
pub fn sync(paths: &Paths, state: &State) -> Res<bool> {
    let line = if state.auto_update {
        let (h, m) = parse_time(&state.update_time)?;
        Some(job_line(h, m))
    } else {
        None
    };
    let old = fs::read_to_string(&paths.crontab).unwrap_or_default();
    let new = with_line(&old, line.as_deref());
    if new == old {
        return Ok(false);
    }
    if let Some(dir) = paths.crontab.parent() {
        fs::create_dir_all(dir)?;
    }
    fs::write(&paths.crontab, new)?;
    sys::run_cmd("/etc/init.d/cron", &["restart"]);
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The real crontab of the author's main router (comment lines included).
    const ROUTER: &str = "3 3 12 12 * /usr/bin/nginx-util 'check_ssl'\n\
        10 6 * * * /bin/bash -c \"/usr/bin/node /root/ip/clash.js > /root/ip/clash.log 2>&1\"\n\
        # keep this comment\n\
        16 3 * * * reboot\n";

    #[test]
    fn times_are_validated() {
        assert_eq!(parse_time("06:10").unwrap(), (6, 10));
        assert_eq!(parse_time("23:59").unwrap(), (23, 59));
        for bad in ["6:10", "24:00", "06:60", "0610", "aa:bb", "", "06:10:00"] {
            assert!(parse_time(bad).is_err(), "{bad} must be rejected");
        }
    }

    #[test]
    fn our_line_is_added_once_and_everything_else_is_kept() {
        let line = job_line(6, 10);
        let once = with_line(ROUTER, Some(&line));
        assert!(
            once.starts_with(ROUTER),
            "existing lines untouched and in order"
        );
        assert!(once.ends_with(&format!("{line}\n")));
        assert_eq!(with_line(&once, Some(&line)), once, "idempotent");
        assert!(line.starts_with("10 6 * * * /usr/bin/openwrt-clash-lite auto-update"));
    }

    #[test]
    fn changing_the_time_replaces_and_disabling_removes_our_line() {
        let at_610 = with_line(ROUTER, Some(&job_line(6, 10)));
        let at_530 = with_line(&at_610, Some(&job_line(5, 30)));
        assert_eq!(at_530.matches(MARK).count(), 1);
        assert!(at_530.contains("30 5 * * *"));
        assert_eq!(
            with_line(&at_530, None),
            ROUTER,
            "back to the original crontab"
        );
    }

    #[test]
    fn an_empty_or_unterminated_crontab_works() {
        assert_eq!(with_line("", None), "");
        let out = with_line("16 3 * * * reboot", Some("x # openwrt-clash-lite"));
        assert_eq!(out, "16 3 * * * reboot\nx # openwrt-clash-lite\n");
    }

    #[test]
    fn sync_writes_only_when_something_changes() {
        let base = std::env::temp_dir().join(format!("oclite-cron-{}", std::process::id()));
        let paths = Paths::under(&base);
        fs::create_dir_all(paths.crontab.parent().unwrap()).unwrap();
        fs::write(&paths.crontab, ROUTER).unwrap();
        let mut state = State::default();
        assert!(
            !sync(&paths, &state).unwrap(),
            "off and no line: nothing to do"
        );
        state.auto_update = true;
        assert!(sync(&paths, &state).unwrap());
        assert!(!sync(&paths, &state).unwrap(), "already in place");
        state.update_time = "25:00".into();
        assert!(sync(&paths, &state).is_err());
        state.auto_update = false;
        assert!(sync(&paths, &state).unwrap());
        assert_eq!(fs::read_to_string(&paths.crontab).unwrap(), ROUTER);
        let _ = fs::remove_dir_all(&base);
    }
}
