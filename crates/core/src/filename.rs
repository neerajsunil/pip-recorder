//! Recording file names from a user pattern such as `{date} {time} {source}`.

/// Matches the timestamped names Pip has always used: `2026-10-07-14-32-08`.
pub const DEFAULT_FILE_NAME: &str = "{year}-{month}-{day}-{hour}-{minute}-{second}";

/// Local wall-clock time, supplied by the platform.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LocalTime {
    pub year: u16,
    pub month: u16,
    pub day: u16,
    pub hour: u16,
    pub minute: u16,
    pub second: u16,
}

/// Expands a pattern into a safe file stem (no extension).
///
/// Tokens: `{year} {month} {day} {hour} {minute} {second}`, `{date}`
/// (`YYYY-MM-DD`), `{time}` (`HH-mm-ss`) and `{source}`. Characters Windows
/// forbids in file names become `-`; an empty result falls back to the default.
pub fn file_name(pattern: &str, time: LocalTime, source: &str) -> String {
    let two = |value: u16| format!("{value:02}");
    let expanded = pattern
        .replace(
            "{date}",
            &format!("{:04}-{}-{}", time.year, two(time.month), two(time.day)),
        )
        .replace(
            "{time}",
            &format!(
                "{}-{}-{}",
                two(time.hour),
                two(time.minute),
                two(time.second)
            ),
        )
        .replace("{year}", &format!("{:04}", time.year))
        .replace("{month}", &two(time.month))
        .replace("{day}", &two(time.day))
        .replace("{hour}", &two(time.hour))
        .replace("{minute}", &two(time.minute))
        .replace("{second}", &two(time.second))
        .replace("{source}", source);
    let safe: String = expanded
        .chars()
        .map(|c| {
            if c.is_control() || r#"<>:"/\|?*"#.contains(c) {
                '-'
            } else {
                c
            }
        })
        .collect();
    let safe = safe.trim().trim_end_matches('.').trim();
    if safe.is_empty() {
        return file_name(DEFAULT_FILE_NAME, time, source);
    }
    safe.chars().take(120).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    const TIME: LocalTime = LocalTime {
        year: 2026,
        month: 10,
        day: 7,
        hour: 9,
        minute: 5,
        second: 3,
    };

    #[test]
    fn default_pattern_matches_previous_names() {
        assert_eq!(
            file_name(DEFAULT_FILE_NAME, TIME, "Display 1"),
            "2026-10-07-09-05-03"
        );
    }

    #[test]
    fn tokens_and_unsafe_characters() {
        assert_eq!(
            file_name("{source} {date} {time}", TIME, "Notes: draft"),
            "Notes- draft 2026-10-07 09-05-03"
        );
        assert_eq!(file_name("a/b\\c?", TIME, ""), "a-b-c-");
        assert_eq!(file_name("   ...", TIME, ""), "2026-10-07-09-05-03");
        assert_eq!(file_name("{source}", TIME, ""), "2026-10-07-09-05-03");
    }
}
