pub(crate) fn working_directory_from_ghostty(raw: &str) -> Option<String> {
    let rest = raw.strip_prefix("file://")?;
    let path = if rest.starts_with('/') {
        rest.to_owned()
    } else {
        let (_, path) = rest.split_once('/')?;
        format!("/{path}")
    };
    let decoded = percent_decode_uri_path(&path)?;

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
        assert_eq!(
            working_directory_from_ghostty("file://DESKTOP-123/c/dev/path%20with%20space"),
            Some(r"C:\dev\path with space".to_owned())
        );
    }
}
