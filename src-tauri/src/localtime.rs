//! Local date/time without a date crate: the OS clock on Windows, UTC elsewhere.

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LocalTime {
    pub year: u16,
    pub month: u16,
    pub day: u16,
    pub hour: u16,
    pub minute: u16,
    pub second: u16,
}

impl LocalTime {
    /// `2026-10-16`
    pub fn date(&self) -> String {
        format!("{:04}-{:02}-{:02}", self.year, self.month, self.day)
    }
    /// `2026-10-16 20:15:03`
    pub fn datetime(&self) -> String {
        format!("{} {:02}:{:02}:{:02}", self.date(), self.hour, self.minute, self.second)
    }
    /// `20261016-201503`, safe in file names.
    pub fn stamp(&self) -> String {
        format!("{:04}{:02}{:02}-{:02}{:02}{:02}", self.year, self.month, self.day, self.hour, self.minute, self.second)
    }
}

#[cfg(windows)]
pub fn now() -> LocalTime {
    #[repr(C)]
    struct SystemTime {
        year: u16,
        month: u16,
        day_of_week: u16,
        day: u16,
        hour: u16,
        minute: u16,
        second: u16,
        milliseconds: u16,
    }
    #[link(name = "kernel32")]
    extern "system" {
        fn GetLocalTime(t: *mut SystemTime);
    }
    // SAFETY: GetLocalTime fills a SYSTEMTIME struct, which this mirrors.
    let t = unsafe {
        let mut t: SystemTime = std::mem::zeroed();
        GetLocalTime(&mut t);
        t
    };
    LocalTime { year: t.year, month: t.month, day: t.day, hour: t.hour, minute: t.minute, second: t.second }
}

#[cfg(not(windows))]
pub fn now() -> LocalTime {
    from_unix(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0))
}

/// UTC breakdown of a Unix timestamp (Howard Hinnant's civil-from-days).
pub fn from_unix(secs: i64) -> LocalTime {
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    LocalTime {
        year: year as u16,
        month: month as u16,
        day: day as u16,
        hour: (rem / 3600) as u16,
        minute: (rem % 3600 / 60) as u16,
        second: (rem % 60) as u16,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_unix_time() {
        let t = from_unix(1_792_165_511); // 2026-10-16 15:45:11 UTC
        assert_eq!(t.datetime(), "2026-10-16 15:45:11");
        assert_eq!(t.stamp(), "20261016-154511");
        assert_eq!(from_unix(0).date(), "1970-01-01");
        assert_eq!(from_unix(951_782_400).date(), "2000-02-29");
    }

    #[test]
    fn local_clock_is_sane() {
        let t = now();
        assert!(t.year >= 2024 && (1..=12).contains(&t.month) && (1..=31).contains(&t.day));
    }
}
