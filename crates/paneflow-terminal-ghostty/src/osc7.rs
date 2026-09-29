pub(crate) fn working_directory_from_ghostty(raw: &str) -> Option<String> {
    let rest = raw.strip_prefix("file://")?;
    let (host, path) = rest.split_at(rest.find('/')?);
    if !is_local_host(host, local_hostname().as_deref()) {
        return None;
    }
    let decoded = percent_decode_uri_path(path)?;

    #[cfg(windows)]
    if let Some(msys_path) = msys_path_to_windows_path(&decoded) {
        return Some(msys_path);
    }

    #[cfg(windows)]
    if decoded.len() >= 3
        && decoded.as_bytes()[0] == b'/'
        && decoded.as_bytes()[1].is_ascii_alphabetic()
        && decoded.as_bytes()[2] == b':'
    {
        return Some(decoded[1..].replace('/', "\\"));
    }
    Some(decoded)
}

fn is_local_host(host: &str, local: Option<&str>) -> bool {
    host.is_empty()
        || host.eq_ignore_ascii_case("localhost")
        || local.is_some_and(|local| host.eq_ignore_ascii_case(local))
}

fn local_hostname() -> Option<String> {
    static LOCAL: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();
    LOCAL.get_or_init(read_local_hostname).clone()
}

#[cfg(unix)]
fn read_local_hostname() -> Option<String> {
    let mut buffer = [0_u8; 256];
    let status = unsafe { libc::gethostname(buffer.as_mut_ptr().cast(), buffer.len()) };
    if status != 0 {
        return None;
    }
    let end = buffer
        .iter()
        .position(|&byte| byte == 0)
        .unwrap_or(buffer.len());
    String::from_utf8(buffer[..end].to_vec())
        .ok()
        .filter(|name| !name.is_empty())
}

#[cfg(windows)]
fn read_local_hostname() -> Option<String> {
    std::env::var("COMPUTERNAME")
        .ok()
        .filter(|name| !name.is_empty())
}

#[cfg(windows)]
fn msys_path_to_windows_path(path: &str) -> Option<String> {
    let bytes = path.as_bytes();
    if bytes.len() < 2
        || bytes[0] != b'/'
        || !bytes[1].is_ascii_alphabetic()
        || (bytes.len() > 2 && bytes[2] != b'/')
    {
        return None;
    }

    let drive = (bytes[1] as char).to_ascii_uppercase();
    if bytes.len() == 2 {
        Some(format!("{drive}:\\"))
    } else {
        Some(format!("{drive}:\\{}", path[3..].replace('/', "\\")))
    }
}

fn percent_decode_uri_path(path: &str) -> Option<String> {
    let bytes = path.as_bytes();
    let mut output = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            match (
                bytes.get(index + 1).copied().and_then(hex_value),
                bytes.get(index + 2).copied().and_then(hex_value),
            ) {
                (Some(high), Some(low)) => {
                    output.push((high << 4) | low);
                    index += 3;
                }
                _ => {
                    output.push(bytes[index]);
                    index += 1;
                }
            }
        } else {
            output.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(output).ok()
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_report_from_another_host_is_not_a_local_working_directory() {
        assert!(is_local_host("", Some("devbox")));
        assert!(is_local_host("localhost", Some("devbox")));
        assert!(is_local_host("LOCALHOST", None));
        assert!(is_local_host("devbox", Some("devbox")));
        assert!(is_local_host("DEVBOX", Some("devbox")));
        assert!(!is_local_host("buildserver", Some("devbox")));
        assert!(!is_local_host("devbox", None));

        assert_eq!(
            working_directory_from_ghostty("file://definitely-not-this-machine.invalid/tmp"),
            None
        );
        assert_eq!(
            working_directory_from_ghostty("file://localhost/tmp/a%2520b"),
            Some("/tmp/a%20b".to_owned())
        );
        let local = local_hostname().expect("the machine has a host name");
        assert!(working_directory_from_ghostty(&format!("file://{local}/tmp")).is_some());
    }

    #[cfg(not(windows))]
    #[test]
    fn osc7_preserves_drive_like_posix_path() {
        assert_eq!(
            working_directory_from_ghostty("file:///C:/dev/path%20with%20space/%C3%A9"),
            Some("/C:/dev/path with space/é".to_owned())
        );
    }

    #[cfg(windows)]
    #[test]
    fn osc7_windows_and_msys_paths_are_decoded() {
        assert_eq!(
            working_directory_from_ghostty("file:///C:/dev/path%20with%20space/%C3%A9"),
            Some(r"C:\dev\path with space\é".to_owned())
        );
        let msys_host = local_hostname()
            .expect("the machine has a host name")
            .to_ascii_uppercase();
        assert_eq!(
            working_directory_from_ghostty(&format!(
                "file://{msys_host}/c/dev/path%20with%20space"
            )),
            Some(r"C:\dev\path with space".to_owned())
        );
    }
}
