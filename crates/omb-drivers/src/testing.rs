use crate::host::Host;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::time::Duration;

pub fn fake(bin: &Path, name: &str, body: &str) {
    let path = bin.join(name);
    std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

pub fn host(root: &Path) -> Host {
    let bin = root.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    Host {
        home: root.join("home"),
        path: bin.into(),
        codex_home: None,
        openai_api_key: false,
        jev_api_key: false,
        timeout: Duration::from_secs(10),
    }
}
