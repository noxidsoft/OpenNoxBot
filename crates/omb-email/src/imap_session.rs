use crate::query::criteria;
use crate::{Folder, MAX_SOURCE_BYTES, Mailbox, Search, Summary, scrub};
use imap::{Client, ClientBuilder, ConnectionMode, Session};
use imap_proto::NameAttribute;
use mail_parser::MessageParser;

#[derive(Clone)]
pub struct Config {
    pub host: String,
    pub port: u16,
    pub user: String,
    pub password: String,
}

type Connection = Session<imap::Connection>;

/// One logged-in session. The password is only ever sent inside TLS.
pub struct ImapMailbox {
    session: Connection,
    password: String,
}

impl ImapMailbox {
    pub fn connect(config: &Config) -> Result<Self, String> {
        let mode = if config.port == 993 { ConnectionMode::Tls } else { ConnectionMode::StartTls };
        let client: Client<imap::Connection> = ClientBuilder::new(config.host.as_str(), config.port)
            .mode(mode)
            .connect()
            .map_err(|e| scrub(&e.to_string(), &config.password))?;
        let session = client.login(&config.user, &config.password).map_err(|(e, _)| scrub(&e.to_string(), &config.password))?;
        Ok(Self { session, password: config.password.clone() })
    }

    fn fail(&self, error: imap::Error) -> String {
        scrub(&error.to_string(), &self.password)
    }
}

impl Drop for ImapMailbox {
    fn drop(&mut self) {
        let _ = self.session.logout();
    }
}

fn special_use(attributes: &[NameAttribute<'_>]) -> Option<String> {
    attributes.iter().find_map(|attribute| {
        let name = match attribute {
            NameAttribute::All => "\\All",
            NameAttribute::Archive => "\\Archive",
            NameAttribute::Drafts => "\\Drafts",
            NameAttribute::Flagged => "\\Flagged",
            NameAttribute::Junk => "\\Junk",
            NameAttribute::Sent => "\\Sent",
            NameAttribute::Trash => "\\Trash",
            _ => return None,
        };
        Some(name.to_owned())
    })
}

fn sender(header: &[u8]) -> (String, String, String) {
    let Some(message) = MessageParser::default().parse(header) else { return Default::default() };
    let from = message
        .from()
        .and_then(|list| list.first())
        .map(|mailbox| match mailbox.name() {
            Some(name) if !name.is_empty() => format!("{name} <{}>", mailbox.address().unwrap_or_default()),
            _ => format!("<{}>", mailbox.address().unwrap_or_default()),
        })
        .unwrap_or_default();
    (from, message.subject().unwrap_or_default().to_owned(), message.date().map(|d| d.to_rfc3339()).unwrap_or_default())
}

impl Mailbox for ImapMailbox {
    fn list(&mut self) -> Result<Vec<Folder>, String> {
        let names = self.session.list(None, Some("\"*\"")).map_err(|e| self.fail(e))?;
        Ok(names.iter().map(|name| Folder { path: name.name().to_owned(), special_use: special_use(name.attributes()) }).collect())
    }

    fn search(&mut self, folder: &str, search: &Search, limit: usize) -> Result<Vec<Summary>, String> {
        let query = criteria(search)?;
        self.session.examine(folder).map_err(|e| self.fail(e))?;
        let mut uids: Vec<u32> = self.session.uid_search(&query).map_err(|e| self.fail(e))?.into_iter().collect();
        uids.sort_unstable();
        let wanted: Vec<String> = uids.iter().rev().take(limit).map(u32::to_string).collect();
        if wanted.is_empty() {
            return Ok(Vec::new());
        }
        let fetched = self.session.uid_fetch(wanted.join(","), "(UID FLAGS RFC822.HEADER)").map_err(|e| self.fail(e))?;
        let mut rows: Vec<Summary> = fetched
            .iter()
            .filter_map(|fetch| {
                let (from, subject, date) = sender(fetch.header()?);
                Some(Summary { uid: fetch.uid?, from, subject, date, flags: fetch.flags().iter().map(|flag| format!("{flag:?}")).collect() })
            })
            .collect();
        rows.sort_by(|a, b| b.uid.cmp(&a.uid));
        Ok(rows)
    }

    fn source(&mut self, folder: &str, uid: u32) -> Result<Option<Vec<u8>>, String> {
        self.session.examine(folder).map_err(|e| self.fail(e))?;
        let sizes = self.session.uid_fetch(uid.to_string(), "(UID RFC822.SIZE)").map_err(|e| self.fail(e))?;
        let Some(size) = sizes.iter().find_map(|fetch| fetch.size) else { return Ok(None) };
        if u64::from(size) > MAX_SOURCE_BYTES {
            return Err(format!("This message is {} MB, too large to read here", size / (1024 * 1024)));
        }
        let fetched = self.session.uid_fetch(uid.to_string(), "BODY.PEEK[]").map_err(|e| self.fail(e))?;
        Ok(fetched.iter().find_map(|fetch| fetch.body().map(<[u8]>::to_vec)))
    }

    fn append_draft(&mut self, folder: &str, raw: &[u8]) -> Result<(), String> {
        if folder.contains(['"', '\\']) || folder.chars().any(char::is_control) {
            return Err("The Drafts folder has a name this tool cannot use".into());
        }
        self.session.append(folder, raw).flag(imap::types::Flag::Draft).finish().map(|_| ()).map_err(|e| self.fail(e))
    }
}
