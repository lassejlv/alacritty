//! OSC 7 file URIs reported by the shell. Remote hosts never become local paths.

use std::path::PathBuf;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkingDirectory {
    pub host: String,
    pub path: String,
}

impl WorkingDirectory {
    pub fn parse(body: &[u8]) -> Option<Self> {
        if body.len() > 8192 {
            return None;
        }
        let uri = std::str::from_utf8(body).ok()?.strip_prefix("file://")?;
        let (host, path) = uri.split_once('/')?;
        if !host.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_'))
            || path.contains(['?', '#'])
        {
            return None;
        }
        let mut decoded = vec![b'/'];
        let mut bytes = path.bytes();
        while let Some(byte) = bytes.next() {
            decoded.push(if byte == b'%' {
                let high = char::from(bytes.next()?).to_digit(16)?;
                let low = char::from(bytes.next()?).to_digit(16)?;
                (high * 16 + low) as u8
            } else {
                byte
            });
        }
        let path = String::from_utf8(decoded).ok()?;
        if path.chars().any(char::is_control) {
            return None;
        }
        Some(Self { host: host.to_owned(), path })
    }

    pub fn local_path(&self, hostname: &str) -> Option<PathBuf> {
        let host = self.host.trim_end_matches('.');
        if !host.is_empty()
            && !host.eq_ignore_ascii_case("localhost")
            && !host.eq_ignore_ascii_case(hostname.trim_end_matches('.'))
        {
            return None;
        }
        #[cfg(windows)]
        let path = self
            .path
            .strip_prefix('/')
            .filter(|p| p.as_bytes().get(1) == Some(&b':'))
            .unwrap_or(&self.path);
        #[cfg(not(windows))]
        let path = &self.path;
        Some(PathBuf::from(path))
    }
}
