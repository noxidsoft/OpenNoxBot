use crate::Search;

/// An IMAP quoted string. Only plain printable ASCII goes through, so nothing in a term can end the command or start a
/// literal.
fn quote(term: &str) -> Result<String, String> {
    if term.is_empty() || term.len() > 200 || !term.chars().all(|c| (' '..='~').contains(&c)) {
        return Err("Search terms must be 1 to 200 plain ASCII characters".into());
    }
    Ok(format!("\"{}\"", term.replace('\\', "\\\\").replace('"', "\\\"")))
}

/// `YYYY-MM-DD` as IMAP's `d-Mon-yyyy`.
fn imap_date(date: &str) -> Result<String, String> {
    const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
    let bad = || "since must be a date like 2026-01-31".to_owned();
    let parts: Vec<&str> = date.split('-').collect();
    let [year, month, day] = parts[..] else { return Err(bad()) };
    if year.len() != 4 || month.len() != 2 || day.len() != 2 {
        return Err(bad());
    }
    let (year, month, day) = (year.parse::<u32>().map_err(|_| bad())?, month.parse::<usize>().map_err(|_| bad())?, day.parse::<u32>().map_err(|_| bad())?);
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return Err(bad());
    }
    Ok(format!("{day}-{}-{year}", MONTHS[month - 1]))
}

pub fn criteria(search: &Search) -> Result<String, String> {
    let mut parts = Vec::new();
    for (key, value) in [("FROM", &search.from), ("SUBJECT", &search.subject), ("BODY", &search.text)] {
        if let Some(term) = value {
            parts.push(format!("{key} {}", quote(term)?));
        }
    }
    if let Some(date) = &search.since {
        parts.push(format!("SINCE {}", imap_date(date)?));
    }
    if search.unseen {
        parts.push("UNSEEN".into());
    }
    Ok(if parts.is_empty() { "ALL".into() } else { parts.join(" ") })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terms_are_quoted_and_escaped() {
        let search = Search { from: Some(r#"a"b\c"#.into()), unseen: true, since: Some("2026-03-05".into()), ..Search::default() };
        assert_eq!(criteria(&search).unwrap(), r#"FROM "a\"b\\c" SINCE 5-Mar-2026 UNSEEN"#);
        assert_eq!(criteria(&Search::default()).unwrap(), "ALL");
    }

    #[test]
    fn a_term_cannot_smuggle_a_second_command() {
        for bad in ["x\r\nA9 STORE 1:* +FLAGS (\\Deleted)", "x\ny", "{4}\r\nabcd", "é", "", "\u{0}"] {
            let search = Search { from: Some(bad.into()), ..Search::default() };
            assert!(criteria(&search).is_err(), "{bad:?}");
        }
        let injected = Search { subject: Some("\" ALL \"".into()), ..Search::default() };
        assert_eq!(criteria(&injected).unwrap(), r#"SUBJECT "\" ALL \"""#);
    }

    #[test]
    fn dates_must_be_real_looking() {
        for bad in ["2026-13-01", "2026-1-01", "yesterday", "2026-01-01\r\n", "2026-01-32"] {
            assert!(imap_date(bad).is_err(), "{bad}");
        }
    }
}
