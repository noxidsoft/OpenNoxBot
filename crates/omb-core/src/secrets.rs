use keyring::{Entry, Error};

const SERVICE: &str = "OpenNoxBot";

#[derive(Clone, Copy)]
pub enum Secret {
    Jev,
    EmailPassword,
}

impl Secret {
    fn account(self) -> &'static str {
        match self {
            Secret::Jev => "jev-api-key",
            Secret::EmailPassword => "email-password",
        }
    }

    /// Passwords are stored exactly as typed; pasted API keys often carry stray whitespace.
    fn trimmed(self) -> bool {
        matches!(self, Secret::Jev)
    }
}

pub fn save(secret: Secret, value: &str) -> Result<(), String> {
    save_to(secret.account(), if secret.trimmed() { value.trim() } else { value })
}

pub fn read(secret: Secret) -> Result<Option<String>, String> {
    match entry(secret.account())?.get_password() {
        Ok(value) => Ok(Some(value)),
        Err(Error::NoEntry) => Ok(None),
        Err(e) => Err(e.to_string()),
    }
}

pub fn is_saved(secret: Secret) -> Result<bool, String> {
    is_saved_in(secret.account())
}

pub fn clear(secret: Secret) -> Result<(), String> {
    clear_in(secret.account())
}

fn entry(account: &str) -> Result<Entry, String> {
    Entry::new(SERVICE, account).map_err(|e| e.to_string())
}

fn save_to(account: &str, value: &str) -> Result<(), String> {
    if value.trim().is_empty() || value.len() > 4096 || value.chars().any(char::is_control) {
        return Err("Enter a valid key".into());
    }
    entry(account)?.set_password(value).map_err(|e| e.to_string())
}

fn is_saved_in(account: &str) -> Result<bool, String> {
    match entry(account)?.get_password() {
        Ok(_) => Ok(true),
        Err(Error::NoEntry) => Ok(false),
        Err(e) => Err(e.to_string()),
    }
}

fn clear_in(account: &str) -> Result<(), String> {
    match entry(account)?.delete_credential() {
        Ok(()) | Err(Error::NoEntry) => Ok(()),
        Err(e) => Err(e.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blank_or_control_keys_are_refused_before_the_keyring_is_touched() {
        for bad in ["", "   ", "a\nb", "a\u{7}b"] {
            assert_eq!(save(Secret::Jev, bad), Err("Enter a valid key".into()));
        }
        assert!(save(Secret::Jev, &"k".repeat(4097)).is_err());
    }

    #[test]
    #[ignore = "writes a throwaway entry to the OS keyring"]
    fn round_trips_through_the_os_keyring() {
        let account = format!("test-roundtrip-{}", std::process::id());
        assert_eq!(is_saved_in(&account), Ok(false));
        save_to(&account, "secret-value").unwrap();
        assert_eq!(is_saved_in(&account), Ok(true));
        assert_eq!(entry(&account).unwrap().get_password().unwrap(), "secret-value");
        clear_in(&account).unwrap();
        assert_eq!(is_saved_in(&account), Ok(false));
        assert_eq!(clear_in(&account), Ok(()));
    }
}
