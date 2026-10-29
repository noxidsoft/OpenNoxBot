use omb_core::policy::shell::{digits, letters, words};
use omb_wire::HostPolicy;

pub enum Gate {
    Run,
    Ask,
    Refuse(&'static str),
}

/// What a host's policy does with a command before it is sent.
pub fn gate(policy: HostPolicy, command: &str) -> Gate {
    if command.trim().is_empty() {
        return Gate::Refuse("The command is empty");
    }
    match policy {
        HostPolicy::Full => Gate::Run,
        _ if is_status_command(command) => Gate::Run,
        HostPolicy::Ask => Gate::Ask,
        HostPolicy::ReadOnly => Gate::Refuse("This host is read-only: only status commands run"),
    }
}

const SERVICE_ACTIONS: [&str; 7] = ["status", "is-active", "is-enabled", "is-failed", "list-units", "list-unit-files", "list-timers"];
const DOCKER_ACTIONS: [&str; 5] = ["ps", "images", "version", "logs", "stats"];

fn log_path(word: &str) -> bool {
    word.starts_with("/var/log/") && !word.split('/').any(|part| part == "..")
}

fn argument(word: &str) -> bool {
    !word.starts_with('-')
}

/// How a command's words may look: its flags, the flags that take a value, and what a bare word may be.
struct Shape<'a> {
    flag: &'a dyn Fn(&str) -> bool,
    valued: &'a dyn Fn(&str) -> bool,
    bare: &'a dyn Fn(usize, &str) -> bool,
}

fn never(_: &str) -> bool {
    false
}

fn follows(args: &[String], shape: &Shape) -> bool {
    let (mut index, mut args) = (0, args.iter());
    while let Some(word) = args.next() {
        if word.starts_with('-') && word.len() > 1 {
            if (shape.valued)(word) {
                if !args.next().is_some_and(|value| argument(value)) {
                    return false;
                }
            } else if !(shape.flag)(word) {
                return false;
            }
        } else if (shape.bare)(index, word) {
            index += 1;
        } else {
            return false;
        }
    }
    true
}

/// Commands that only report on the machine: no writes, no follow modes, no way to read arbitrary files. Each is plain words
/// with the flags below; anything the shell could expand, chain, redirect or pipe is never one.
pub fn is_status_command(command: &str) -> bool {
    let Some(parts) = words(command) else { return false };
    let Some((name, args)) = parts.split_first() else { return false };
    let none = |_: usize, _: &str| false;
    let any = |_: usize, word: &str| argument(word);
    let logs = |_: usize, word: &str| log_path(word);
    let plain = |flag: &dyn Fn(&str) -> bool, bare: &dyn Fn(usize, &str) -> bool| follows(args, &Shape { flag, valued: &never, bare });
    match name.as_str() {
        "uptime" | "nproc" | "whoami" => plain(&never, &none),
        "hostname" => plain(&|w| letters(w, "sfi"), &none),
        "id" => plain(&|w| letters(w, "ugnG"), &none),
        "uname" => plain(&|w| letters(w, "asrmvonip"), &none),
        "date" => plain(&|w| w == "-u", &|_, w| w.starts_with('+')),
        "df" => plain(&|w| letters(w, "hTiklPaH"), &any),
        "free" => plain(&|w| letters(w, "hmgkbtwl"), &none),
        "ls" => plain(&|w| letters(w, "laAhRt1SrdFpGi"), &any),
        "ps" => plain(&|w| letters(w, "efaxuwlLHAj"), &|_, w| w.chars().all(|c| "auxwef".contains(c))),
        "cat" => plain(&|w| letters(w, "n"), &logs),
        "tail" | "head" => follows(args, &Shape { flag: &|w| letters(w, "qv") || digits(w), valued: &|w| w == "-n" || w == "-c", bare: &logs }),
        "grep" => follows(
            args,
            &Shape {
                flag: &|w| letters(w, "nilcvwxHhEFsoq"),
                valued: &|w| matches!(w, "-A" | "-B" | "-C" | "-m"),
                bare: &|index, word| index == 0 || log_path(word),
            },
        ),
        "systemctl" => match args.split_first() {
            Some((action, rest)) if SERVICE_ACTIONS.contains(&action.as_str()) => follows(
                rest,
                &Shape {
                    flag: &|w| matches!(w, "--no-pager" | "-l" | "--full" | "--failed" | "--all" | "-a"),
                    valued: &|w| matches!(w, "-t" | "--type" | "--state"),
                    bare: &any,
                },
            ),
            _ => false,
        },
        "journalctl" => follows(
            args,
            &Shape {
                flag: &|w| matches!(w, "--no-pager" | "-x" | "-e" | "-r" | "-k" | "--utc" | "-q"),
                valued: &|w| matches!(w, "-u" | "--unit" | "-n" | "--lines" | "-p" | "--priority" | "--since" | "--until"),
                bare: &none,
            },
        ),
        "docker" => match args.split_first() {
            Some((action, rest)) if DOCKER_ACTIONS.contains(&action.as_str()) => match action.as_str() {
                "ps" | "images" => follows(rest, &Shape { flag: &|w| letters(w, "aqls") || w == "--no-trunc", valued: &never, bare: &none }),
                "version" => rest.is_empty(),
                "logs" => follows(
                    rest,
                    &Shape {
                        flag: &|w| matches!(w, "-t" | "--timestamps"),
                        valued: &|w| matches!(w, "--tail" | "-n" | "--since" | "--until"),
                        bare: &|index, word| index == 0 && argument(word),
                    },
                ),
                _ => rest.iter().any(|w| w == "--no-stream") && follows(rest, &Shape { flag: &|w| w == "--no-stream" || w == "--no-trunc", valued: &never, bare: &|_, w| argument(w) }),
            },
            _ => false,
        },
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_commands_run_without_asking() {
        for command in [
            "uptime", "hostname", "hostname -f", "whoami", "id -un", "uname -a", "nproc", "date +%s", "date -u", "df -h", "df -h /var", "free -m",
            "ls -la /etc/nginx", "ps aux", "ps -ef", "systemctl status nginx", "systemctl is-active nginx", "systemctl list-units --failed --no-pager",
            "journalctl -u nginx -n 50 --no-pager", "journalctl --since \"2 hours ago\" -p err", "docker ps", "docker ps -a", "docker logs --tail 100 web",
            "docker stats --no-stream", "docker logs web", "docker version", "tail -n 100 /var/log/syslog", "head -20 /var/log/nginx/error.log", "cat /var/log/auth.log",
            "grep -i error /var/log/app/app.log", "grep -n -C 3 timeout /var/log/syslog",
        ] {
            assert!(is_status_command(command), "{command}");
        }
    }

    #[test]
    fn anything_that_changes_state_follows_or_reaches_further_does_not() {
        for command in [
            "systemctl restart nginx", "systemctl stop nginx", "systemctl daemon-reload", "systemctl", "journalctl -f", "journalctl --vacuum-size=1M",
            "tail -f /var/log/syslog", "docker exec -it web sh", "docker rm web", "docker logs -f web", "docker stats", "docker inspect web",
            "docker run alpine", "date -s 2020-01-01", "date 010203042026", "hostname newname", "ps -o args", "df --output=source", "ls /etc/shadow | sh",
            "uptime; rm -rf /", "uptime && reboot", "uptime | sh", "uptime > /etc/x", "cat /etc/shadow", "cat /var/log/../../etc/shadow", "cat ~/.ssh/id_rsa",
            "cat /var/log/*", "cat $(echo /var/log/x)", "cat `x`", "tail -n 5 /home/u/.env", "grep -r secret /", "grep secret /etc/passwd", "rm -rf /",
            "sudo uptime", "sh -c uptime", "bash -lc id", "uptime\nreboot", "", "   ", "ls -R -- /", "df -x tmpfs", "free --si", "id -u root; id",
        ] {
            assert!(!is_status_command(command), "{command:?}");
        }
    }

    #[test]
    fn the_hosts_policy_decides_what_to_do_with_a_command() {
        assert!(matches!(gate(HostPolicy::ReadOnly, "uptime"), Gate::Run));
        assert!(matches!(gate(HostPolicy::ReadOnly, "reboot"), Gate::Refuse(_)));
        assert!(matches!(gate(HostPolicy::Ask, "uptime"), Gate::Run));
        assert!(matches!(gate(HostPolicy::Ask, "reboot"), Gate::Ask));
        assert!(matches!(gate(HostPolicy::Full, "reboot"), Gate::Run));
        for policy in [HostPolicy::ReadOnly, HostPolicy::Ask, HostPolicy::Full] {
            assert!(matches!(gate(policy, "  "), Gate::Refuse(_)));
        }
    }
}
