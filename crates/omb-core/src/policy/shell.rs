//! Words of a command line that no shell can expand into anything else.

pub fn letters(word: &str, allowed: &str) -> bool {
    word.strip_prefix('-').is_some_and(|rest| !rest.is_empty() && rest.chars().all(|c| allowed.contains(c)))
}

pub fn digits(word: &str) -> bool {
    word.strip_prefix('-').is_some_and(|rest| !rest.is_empty() && rest.chars().all(|c| c.is_ascii_digit()))
}

fn is_bare(c: char) -> bool {
    c.is_ascii_alphanumeric() || "_./~+@%:,=-".contains(c)
}

/// One plain word: letters, digits and the punctuation of paths and numbers, or a whole quoted string the shell expands
/// nothing in, each ending at a space or the end. No glob, variable, substitution, redirection, pipe or second command can
/// be one.
pub fn words(command: &str) -> Option<Vec<String>> {
    if command.contains(['\0', '\r', '\n']) {
        return None;
    }
    let mut rest = command.trim();
    let mut out = Vec::new();
    while !rest.is_empty() {
        rest = rest.trim_start_matches([' ', '\t']);
        let (word, after) = if let Some(quoted) = rest.strip_prefix('\'') {
            let end = quoted.find('\'')?;
            (quoted[..end].to_owned(), &quoted[end + 1..])
        } else if let Some(quoted) = rest.strip_prefix('"') {
            let end = quoted.find('"')?;
            if quoted[..end].contains(['$', '`', '\\', '!']) {
                return None;
            }
            (quoted[..end].to_owned(), &quoted[end + 1..])
        } else {
            let end = rest.find(|c: char| !is_bare(c)).unwrap_or(rest.len());
            let bare = &rest[..end];
            // The shell expands a ~ anywhere (bash after an =, as in foo=~/x) and a leading = (zsh's =cmd): to a home, the
            // folder it was in before or a command's own path, none of which is checked here.
            if bare.is_empty() || bare.contains('~') || bare.starts_with('=') {
                return None;
            }
            (bare.to_owned(), &rest[end..])
        };
        if !after.is_empty() && !after.starts_with([' ', '\t']) {
            return None;
        }
        out.push(word);
        rest = after;
    }
    (!out.is_empty()).then_some(out)
}
