//! Process-owned OAuth handoff material (native 2.1.293 `eM`/`k`/`VNn`).

use harness_runtime::headless::host::OAuthDescriptorCredential;
use std::io::{self, Read};
use std::path::Path;
use std::sync::Mutex;

const LIMIT: usize = 65_536;
const FD_ENV: &str = "CLAUDE_CODE_OAUTH_TOKEN_FILE_DESCRIPTOR";
const SNAPSHOT_ENV: &str = "CLAUDE_BG_AUTH_SNAPSHOT_PATH";
// CCR_OAUTH_TOKEN_FILE is the native source label, not an environment override.
const TOKEN_FILE: &str = "/home/claude/.claude/remote/.oauth_token";

#[derive(Default)]
pub(crate) struct OAuthDescriptorCache(Mutex<State>);

#[derive(Default)]
struct State {
    // None = not read yet; Some(None) = a cached miss, like native undefined/null.
    token: Option<Option<String>>,
    scopes: Option<Vec<String>>,
    from_background_snapshot: bool,
}

impl State {
    fn read_once(&mut self, read: impl FnOnce() -> Option<String>) {
        if self.token.is_none() {
            self.token = Some(read());
        }
    }
}

impl OAuthDescriptorCache {
    pub(crate) fn credential(&self) -> Option<OAuthDescriptorCredential> {
        let mut state = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let host_managed = env_truthy("CLAUDE_CODE_PROVIDER_MANAGED_BY_HOST");
        consume_background_snapshot(&mut state, host_managed);
        state.read_once(|| {
            let remote = env_truthy("CLAUDE_CODE_REMOTE");
            let raw_fd = std::env::var(FD_ENV).ok().filter(|value| !value.is_empty());
            let (token, consumed, persist) =
                read_descriptor_material(raw_fd.as_deref(), Path::new(TOKEN_FILE), !remote);
            if consumed {
                std::env::remove_var(FD_ENV);
            }
            if persist {
                if let Some(token) = &token {
                    persist_remote_token(token, remote);
                }
            }
            token
        });
        let access_token = match state.token.as_ref().and_then(Option::as_ref) {
            Some(token) => token.clone(),
            // Native on() suppresses stored OAuth for a host-managed launch
            // even when the descriptor cache holds null. Preserve that source
            // fact independently of whether this handoff contains a token.
            None if host_managed => String::new(),
            None => return None,
        };
        Some(OAuthDescriptorCredential {
            access_token,
            scopes: state.scopes.clone(),
            from_background_snapshot: state.from_background_snapshot,
            host_managed,
        })
    }
}

// Native Ld/remote gates use JS string truthiness, so even "0" is present.
fn env_truthy(name: &str) -> bool {
    std::env::var(name).is_ok_and(|value| !value.is_empty())
}

fn consume_background_snapshot(state: &mut State, host_managed: bool) {
    let Some(path) = std::env::var(SNAPSHOT_ENV)
        .ok()
        .filter(|path| !path.is_empty())
    else {
        return;
    };
    let path = Path::new(&path);
    let outcome = read_bounded_file(path, true, !host_managed);
    // VNn retries busy/permission failures once and retains a busy announcement.
    let outcome = if outcome.as_ref().is_err_and(retryable_snapshot_error) {
        read_bounded_file(path, true, !host_managed)
    } else {
        outcome
    };
    if outcome.as_ref().is_err_and(retryable_snapshot_error) {
        return;
    }
    std::env::remove_var(SNAPSHOT_ENV);
    let Ok(bytes) = outcome else {
        return;
    };
    let _ = std::fs::remove_file(path);
    let Ok(value) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
        return;
    };
    // Gateway snapshots belong to the separate gateway credential owner.
    if value["gatewayToken"]
        .as_str()
        .is_some_and(|token| !token.is_empty())
    {
        return;
    }
    let Some(token) = value["accessToken"]
        .as_str()
        .filter(|token| !token.is_empty())
    else {
        return;
    };
    state.token = Some(Some(token.to_owned()));
    state.from_background_snapshot = true;
    if let Some(scopes) = value["scopes"]
        .as_array()
        .filter(|scopes| !scopes.is_empty())
    {
        state.scopes = scopes
            .iter()
            .map(|scope| scope.as_str().map(str::to_owned))
            .collect();
    }
    for (field, env) in [
        ("subscriptionType", "CLAUDE_CODE_SUBSCRIPTION_TYPE"),
        ("rateLimitTier", "CLAUDE_CODE_RATE_LIMIT_TIER"),
    ] {
        if let Some(value) = value[field].as_str().filter(|value| !value.is_empty()) {
            std::env::set_var(env, value);
        }
    }
}

fn retryable_snapshot_error(error: &io::Error) -> bool {
    #[cfg(unix)]
    {
        error.raw_os_error().is_some_and(|code| {
            code == rustix::io::Errno::BUSY.raw_os_error()
                || code == rustix::io::Errno::PERM.raw_os_error()
        })
    }
    #[cfg(not(unix))]
    {
        error.kind() == io::ErrorKind::PermissionDenied
    }
}

fn read_descriptor_material(
    raw_fd: Option<&str>,
    fallback: &Path,
    local: bool,
) -> (Option<String>, bool, bool) {
    let fallback_token = || {
        read_bounded_file(fallback, true, false)
            .ok()
            .and_then(trim_token)
    };
    let Some(raw_fd) = raw_fd else {
        return (fallback_token(), false, false);
    };
    // Native parseInt permits a decimal prefix, e.g. " 3extra"; NaN does not fall back.
    let Some(fd) = parse_descriptor(raw_fd) else {
        let value = lingxi_core::host::effort::trim_js_whitespace(raw_fd);
        let digits = value
            .strip_prefix('+')
            .or_else(|| value.strip_prefix('-'))
            .unwrap_or(value);
        if digits
            .bytes()
            .next()
            .is_some_and(|byte| byte.is_ascii_digit())
        {
            // A numeric prefix outside RawFd's range is not NaN. Native's
            // failed descriptor open still reaches the regular-file fallback.
            return (fallback_token(), false, false);
        }
        return (None, false, false);
    };
    #[cfg(unix)]
    {
        use std::os::unix::fs::FileTypeExt;
        let path = std::path::PathBuf::from(format!("/dev/fd/{fd}"));
        let kind = std::fs::metadata(&path)
            .ok()
            .map(|metadata| metadata.file_type());
        if kind.is_some_and(|kind| kind.is_socket()) {
            return if local {
                (
                    read_inherited_descriptor(fd).ok().and_then(trim_token),
                    true,
                    true,
                )
            } else {
                (fallback_token(), false, false)
            };
        }
        match read_bounded_file(&path, false, false) {
            Ok(bytes) => (trim_token(bytes), false, true),
            Err(error) => {
                if let Some(token) = fallback_token() {
                    return (Some(token), false, false);
                }
                let permission = error.raw_os_error().is_some_and(|code| {
                    code == rustix::io::Errno::ACCESS.raw_os_error()
                        || code == rustix::io::Errno::PERM.raw_os_error()
                });
                if local && permission {
                    (
                        read_inherited_descriptor(fd).ok().and_then(trim_token),
                        true,
                        true,
                    )
                } else {
                    (None, false, false)
                }
            }
        }
    }
    #[cfg(not(unix))]
    {
        let _ = (fd, local);
        (fallback_token(), false, false)
    }
}

fn parse_descriptor(value: &str) -> Option<i32> {
    let value = lingxi_core::host::effort::trim_js_whitespace(value);
    let count = value
        .bytes()
        .enumerate()
        .take_while(|(index, byte)| {
            byte.is_ascii_digit() || (*index == 0 && matches!(*byte, b'+' | b'-'))
        })
        .count();
    value.get(..count)?.parse().ok()
}

fn trim_token(bytes: Vec<u8>) -> Option<String> {
    let text = String::from_utf8_lossy(&bytes);
    let trimmed = lingxi_core::host::effort::trim_js_whitespace(&text);
    (!trimmed.is_empty()).then(|| trimmed.to_owned())
}

fn read_bounded_file(path: &Path, regular_only: bool, refuse_symlink: bool) -> io::Result<Vec<u8>> {
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        let mut flags = rustix::fs::OFlags::empty();
        if regular_only || refuse_symlink {
            flags |= rustix::fs::OFlags::NONBLOCK;
        }
        if refuse_symlink {
            flags |= rustix::fs::OFlags::NOFOLLOW;
        }
        options.custom_flags(flags.bits() as i32);
    }
    #[cfg(not(unix))]
    if refuse_symlink && std::fs::symlink_metadata(path)?.file_type().is_symlink() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "handoff file is a symlink",
        ));
    }
    let mut file = options.open(path)?;
    if regular_only && !file.metadata()?.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "handoff is not a regular file",
        ));
    }
    let mut output = Vec::new();
    let mut buffer = [0_u8; 8192];
    loop {
        let count = match file.read(&mut buffer) {
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            result => result?,
        };
        if count == 0 {
            return Ok(output);
        }
        if output.len() + count > LIMIT {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "handoff exceeds byte limit",
            ));
        }
        output.extend_from_slice(&buffer[..count]);
    }
}

#[cfg(unix)]
fn read_inherited_descriptor(fd: i32) -> io::Result<Vec<u8>> {
    use std::os::unix::fs::FileTypeExt;
    let kind = std::fs::metadata(format!("/dev/fd/{fd}"))?.file_type();
    if !kind.is_socket() && !kind.is_fifo() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "inherited descriptor is not a pipe or socket",
        ));
    }
    let result = (|| {
        let mut output = Vec::new();
        let mut buffer = [0_u8; 8192];
        loop {
            let count = match nix::unistd::read(fd, &mut buffer) {
                Err(nix::errno::Errno::EINTR) => continue,
                result => result.map_err(io::Error::from)?,
            };
            if count == 0 {
                return Ok(output);
            }
            if output.len() + count > LIMIT {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "handoff exceeds byte limit",
                ));
            }
            let newline = buffer[..count].iter().position(|byte| *byte == b'\n');
            output.extend_from_slice(&buffer[..newline.map_or(count, |index| index + 1)]);
            if newline.is_some() {
                return Ok(output);
            }
        }
    })();
    let _ = nix::unistd::close(fd);
    result
}

fn persist_remote_token(token: &str, remote: bool) {
    if !remote
        || std::env::var("CLAUDE_CODE_REMOTE_SESSION_ORIGIN")
            .ok()
            .as_deref()
            == Some("review")
    {
        return;
    }
    let parent = Path::new(TOKEN_FILE).parent().expect("token file parent");
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    if builder.create(parent).is_err() {
        return;
    }
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    if let Ok(mut file) = options.open(TOKEN_FILE) {
        use std::io::Write;
        let _ = file.write_all(token.as_bytes());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn descriptor_parse_and_file_trim_follow_native_js_semantics() {
        assert_eq!(parse_descriptor("\u{feff} +12extra"), Some(12));
        assert_eq!(parse_descriptor("invalid"), None);
        assert_eq!(
            trim_token("\u{feff} token \r\n".as_bytes().to_vec()).as_deref(),
            Some("token")
        );
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("oauth");
        std::fs::write(&path, " token\n").unwrap();
        assert_eq!(
            read_descriptor_material(None, &path, true),
            (Some("token".into()), false, false)
        );
        assert_eq!(
            read_descriptor_material(Some("invalid"), &path, true),
            (None, false, false)
        );
        assert_eq!(
            read_descriptor_material(Some("99999999999999999999"), &path, true),
            (Some("token".into()), false, false)
        );
        std::fs::write(&path, vec![b'x'; LIMIT + 1]).unwrap();
        assert_eq!(
            read_descriptor_material(None, &path, true),
            (None, false, false)
        );
    }

    #[cfg(unix)]
    #[test]
    fn socket_handoff_stops_at_newline_and_closes_consumed_descriptor() {
        use std::io::Write;
        use std::os::fd::IntoRawFd;
        let (reader, mut writer) = std::os::unix::net::UnixStream::pair().unwrap();
        writer.write_all(b" token\nignored").unwrap();
        let fd = reader.into_raw_fd();
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            read_descriptor_material(Some(&fd.to_string()), &dir.path().join("absent"), true),
            (Some("token".into()), true, true)
        );
        assert!(writer.write_all(b"after close").is_err());
    }

    #[test]
    fn successful_and_missing_handoffs_are_cached_without_rereading() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("oauth");
        let mut absent = State::default();
        absent.read_once(|| read_descriptor_material(None, &path, true).0);
        std::fs::write(&path, "first").unwrap();
        absent.read_once(|| panic!("a cached miss must not reread"));
        assert_eq!(absent.token, Some(None));
        let mut present = State::default();
        present.read_once(|| read_descriptor_material(None, &path, true).0);
        std::fs::write(&path, "second").unwrap();
        present.read_once(|| panic!("a cached token must not reread"));
        assert_eq!(present.token, Some(Some("first".into())));
    }
}
