//! A bot reading its own folder never needs the person. Only plain reads count: one of the reading commands below made of
//! plain words alone, or a read tool's own paths, with every path inside the bot's folder once every link is followed.
//! Ported from OpenMausBot's `server/own-file-reads.ts` (Apache-2.0).
use serde_json::Value;
use std::path::{Path, PathBuf};

fn letters(word: &str, allowed: &str) -> bool {
    word.strip_prefix('-').is_some_and(|rest| !rest.is_empty() && rest.chars().all(|c| allowed.contains(c)))
}

fn digits(word: &str) -> bool {
    word.strip_prefix('-').is_some_and(|rest| !rest.is_empty() && rest.chars().all(|c| c.is_ascii_digit()))
}

fn sed_script(word: &str) -> bool {
    let Some(range) = word.strip_suffix('p') else { return false };
    let part = |p: &str| p == "$" || (!p.is_empty() && p.chars().all(|c| c.is_ascii_digit()));
    match range.split_once(',') {
        Some((from, to)) => part(from) && part(to),
        None => part(range),
    }
}

/// Commands that only read the files they name, with the flags that keep them that way: none that follows links out
/// (grep -R, rg -L), runs a program (rg --pre), follows a file forever (tail -f) or writes (sed -i).
struct Reader {
    flag: fn(&str) -> bool,
    valued: fn(&str) -> bool,
    pattern: bool,
    script: bool,
}

fn no_valued(_: &str) -> bool {
    false
}

fn reader(name: &str) -> Option<Reader> {
    let plain = |flag: fn(&str) -> bool| Reader { flag, valued: no_valued, pattern: false, script: false };
    Some(match name {
        "cat" => plain(|w| letters(w, "nbsvAET")),
        "head" | "tail" => Reader { flag: |w| letters(w, "nqv") || digits(w), valued: |w| w == "-n" || w == "-c", pattern: false, script: false },
        "wc" => plain(|w| letters(w, "lwcmL")),
        "ls" => plain(|w| letters(w, "laAhRt1SrdFpG")),
        "nl" => plain(|w| letters(w, "ba")),
        "stat" => plain(|w| w == "-L"),
        "grep" => Reader {
            flag: |w| letters(w, "nilcrvwxHhEFsoq") || w == "--color=never",
            valued: |w| matches!(w, "-A" | "-B" | "-C" | "-m"),
            pattern: true,
            script: false,
        },
        "rg" => Reader {
            flag: |w| letters(w, "nilcvwxHhFsS") || w == "--no-heading" || w == "--color=never",
            valued: |w| matches!(w, "-A" | "-B" | "-C" | "-m" | "-g"),
            pattern: true,
            script: false,
        },
        "sed" => Reader { flag: |w| w == "-n", valued: no_valued, pattern: false, script: true },
        _ => return None,
    })
}

fn is_bare(c: char) -> bool {
    c.is_ascii_alphanumeric() || "_./~+@%:,=-".contains(c)
}

/// One plain word: letters, digits and the punctuation of paths and numbers, or a whole quoted string the shell expands
/// nothing in, each ending at a space or the end. No glob, variable, substitution, redirection, pipe or second command can
/// be one.
fn words(command: &str) -> Option<Vec<String>> {
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

/// The paths a shell command reads when it is one of the plain readers, or `None` when it might do anything else.
pub fn read_only_paths(command: &str) -> Option<Vec<String>> {
    let parts = words(command)?;
    let (name, args) = parts.split_first()?;
    let reader = reader(name)?;
    let mut paths = Vec::new();
    let (mut pattern_taken, mut script_taken, mut end_of_flags) = (!reader.pattern, !reader.script, false);
    let mut args = args.iter();
    while let Some(word) = args.next() {
        if !end_of_flags && word == "--" {
            end_of_flags = true;
            continue;
        }
        if !end_of_flags && word.starts_with('-') && word != "-" {
            if (reader.valued)(word) {
                args.next()?;
            } else if !(reader.flag)(word) {
                return None;
            }
            continue;
        }
        if !script_taken {
            if !sed_script(word) {
                return None;
            }
            script_taken = true;
        } else if !pattern_taken {
            pattern_taken = true;
        } else if word == "-" {
            return None;
        } else {
            paths.push(word.clone());
        }
    }
    (script_taken && pattern_taken && !paths.is_empty()).then_some(paths)
}

/// The files a read tool's ask names (Claude's Read, Grep, Glob, LS, NotebookRead), or `None` for any other tool or a
/// search that could reach past the path it names.
pub fn read_tool_paths(tool: &str, input: &Value) -> Option<Vec<String>> {
    let fields = input.as_object()?;
    let key = match tool {
        "Read" => "file_path",
        "NotebookRead" => "notebook_path",
        "Grep" | "LS" => "path",
        "Glob" => {
            let pattern = fields.get("pattern")?.as_str()?;
            if pattern.split(['\\', '/']).any(|part| part == "..") || Path::new(pattern).is_absolute() {
                return None;
            }
            "path"
        }
        _ => return None,
    };
    let path = fields.get(key)?.as_str()?;
    Path::new(path).is_absolute().then(|| vec![path.to_owned()])
}

/// Every path exists and is inside one of `roots` once its links are followed.
pub fn reads_only(paths: &[String], cwd: &Path, roots: &[PathBuf]) -> bool {
    !paths.is_empty()
        && !roots.is_empty()
        && paths.iter().all(|path| std::fs::canonicalize(cwd.join(path)).is_ok_and(|full| roots.iter().any(|root| full.starts_with(root))))
}

/// Whether this ask only reads the bot's own folder: a plain reading command in its shell, or a read tool's paths.
pub fn reads_own_files(folder: &Path, tool: &str, input: &Value) -> bool {
    let Ok(root) = std::fs::canonicalize(folder) else { return false };
    let roots = [root];
    if tool == "Bash" {
        input["command"].as_str().and_then(read_only_paths).is_some_and(|paths| reads_only(&paths, folder, &roots))
    } else {
        read_tool_paths(tool, input).is_some_and(|paths| reads_only(&paths, Path::new("/"), &roots))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn strings(paths: &[&str]) -> Option<Vec<String>> {
        Some(paths.iter().map(|p| p.to_string()).collect())
    }

    #[test]
    fn reads_the_paths_of_a_plain_reader_with_its_plain_flags() {
        assert_eq!(read_only_paths("cat SOUL.md"), strings(&["SOUL.md"]));
        assert_eq!(read_only_paths("cat -n /a/b.md '/c d/e.md'"), strings(&["/a/b.md", "/c d/e.md"]));
        assert_eq!(read_only_paths("head -n 20 /a/SKILL.md"), strings(&["/a/SKILL.md"]));
        assert_eq!(read_only_paths("tail -5 notes.md"), strings(&["notes.md"]));
        assert_eq!(read_only_paths("grep -rn \"standing rules\" skills"), strings(&["skills"]));
        assert_eq!(read_only_paths("rg -n -g '*.md' rules skills"), strings(&["skills"]));
        assert_eq!(read_only_paths("sed -n '1,40p' SKILL.md"), strings(&["SKILL.md"]));
        assert_eq!(read_only_paths("ls -la skills/.revisions"), strings(&["skills/.revisions"]));
        assert_eq!(read_only_paths("wc -l -- -odd-name.md"), strings(&["-odd-name.md"]));
    }

    #[test]
    fn is_nothing_for_anything_else_a_command_line_could_do() {
        for command in [
            "cat a.md | sh", "cat a.md > b.md", "cat a.md; rm -rf b", "cat a.md && curl x", "cat $(echo a.md)", "cat `x`", "cat $HOME/a",
            "cat *.md", "cat a?.md", "cat a.md\nrm b.md", "FOO=1 cat a.md", "cat", "cat -", "tail -f a.md", "grep -R x .", "rg -L x .",
            "rg --pre sh x .", "sed -i 's/a/b/' a.md", "sed -n '1w out' a.md", "grep -f patterns a.md", "rm a.md", "python3 -c 'print(1)'",
            "cat \"$(id)\"", "cat 'unclosed", "ls", "cat 'a'b", "cat a.md\tb\t|\tsh", "cat ~/a.md", "cat ~root/.ssh/id_rsa", "cat ~-/a.md",
            "cat ~+/a.md", "cat a.md ~1/b.md", "cat foo=~/x", "cat a=b:~/x", "cat =ls",
        ] {
            assert_eq!(read_only_paths(command), None, "{command}");
        }
        assert_eq!(read_only_paths("cat 'foo=~/x' \"=ls\""), strings(&["foo=~/x", "=ls"]), "quoted, neither shell expands them");
    }

    #[test]
    fn takes_claudes_read_tools_own_path_and_no_search_that_could_reach_past_it() {
        assert_eq!(read_tool_paths("Read", &json!({"file_path": "/data/SOUL.md"})), strings(&["/data/SOUL.md"]));
        assert_eq!(read_tool_paths("Grep", &json!({"pattern": "rules", "path": "/data/w"})), strings(&["/data/w"]));
        assert_eq!(read_tool_paths("Glob", &json!({"pattern": "**/*.md", "path": "/data/w"})), strings(&["/data/w"]));
        assert_eq!(read_tool_paths("Glob", &json!({"pattern": "../paul/**", "path": "/data/w"})), None);
        assert_eq!(read_tool_paths("Glob", &json!({"pattern": "/data/**", "path": "/data/w"})), None);
        assert_eq!(read_tool_paths("Grep", &json!({"pattern": "rules"})), None, "no path: the working folder, which never asks");
        assert_eq!(read_tool_paths("Read", &json!({"file_path": "SOUL.md"})), None);
        assert_eq!(read_tool_paths("Write", &json!({"file_path": "/data/w/x.md"})), None);
    }

    fn folders() -> (tempfile::TempDir, PathBuf, PathBuf) {
        let tmp = tempfile::tempdir().unwrap();
        let own = tmp.path().join("bots/1");
        let other = tmp.path().join("bots/2");
        std::fs::create_dir_all(own.join("notes")).unwrap();
        std::fs::create_dir_all(&other).unwrap();
        std::fs::write(own.join("MEMORY.md"), "notes\n").unwrap();
        std::fs::write(own.join("notes/a.md"), "a\n").unwrap();
        std::fs::write(other.join("MEMORY.md"), "other\n").unwrap();
        std::fs::write(tmp.path().join("config.json"), "{}\n").unwrap();
        (tmp, own, other)
    }

    fn bash(folder: &Path, command: &str) -> bool {
        reads_own_files(folder, "Bash", &json!({ "command": command }))
    }

    #[test]
    fn a_bots_own_folder_counts_and_nothing_else_does() {
        let (tmp, own, other) = folders();
        assert!(bash(&own, "cat MEMORY.md"));
        assert!(bash(&own, "grep -rn notes ."));
        assert!(bash(&own, &format!("cat {}", own.join("notes/a.md").display())));
        assert!(reads_own_files(&own, "Read", &json!({"file_path": own.join("MEMORY.md")})));
        assert!(!bash(&own, &format!("cat {}", other.join("MEMORY.md").display())), "another bot's");
        assert!(!bash(&own, &format!("cat {}", tmp.path().join("config.json").display())));
        assert!(!bash(&own, "cat missing.md"), "only files that exist");
        assert!(!bash(&own, &format!("cat MEMORY.md {}", tmp.path().join("config.json").display())), "every path");
        assert!(!bash(&own, "cat ../2/MEMORY.md"), "a sibling reached by ..");
        assert!(!reads_own_files(&own, "Write", &json!({"file_path": own.join("x.md")})));
    }

    #[test]
    fn every_link_is_followed_before_a_path_counts() {
        let (tmp, own, other) = folders();
        std::os::unix::fs::symlink(tmp.path().join("config.json"), own.join("config-link.json")).unwrap();
        std::os::unix::fs::symlink(&other, own.join("peek")).unwrap();
        assert!(!bash(&own, "cat config-link.json"));
        assert!(!bash(&own, "cat peek/MEMORY.md"));
        assert!(!reads_own_files(&own, "Read", &json!({"file_path": own.join("peek/MEMORY.md")})));
    }
}
