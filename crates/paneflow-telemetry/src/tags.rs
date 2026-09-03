pub fn is_canonical_tag_format(s: &str) -> bool {
    if s.is_empty() {
        return false;
    }
    s.chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '.')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_string_is_not_canonical() {
        assert!(!is_canonical_tag_format(""));
    }

    #[test]
    fn lowercase_alpha_only_is_canonical() {
        assert!(is_canonical_tag_format("network"));
        assert!(is_canonical_tag_format("disk"));
        assert!(is_canonical_tag_format("signature"));
    }

    #[test]
    fn lowercase_with_dot_or_hyphen_is_canonical() {
        assert!(is_canonical_tag_format("tar.gz"));
        assert!(is_canonical_tag_format("rpm-ostree"));
        assert!(is_canonical_tag_format("a.b-c"));
    }

    #[test]
    fn uppercase_is_rejected() {
        assert!(!is_canonical_tag_format("DEB"));
        assert!(!is_canonical_tag_format("Deb"));
        assert!(!is_canonical_tag_format("rpm-Ostree"));
    }

    #[test]
    fn whitespace_is_rejected() {
        assert!(!is_canonical_tag_format(" deb"));
        assert!(!is_canonical_tag_format("deb "));
        assert!(!is_canonical_tag_format("with space"));
        assert!(!is_canonical_tag_format("with\ttab"));
    }

    #[test]
    fn special_characters_are_rejected() {
        assert!(!is_canonical_tag_format("deb!"));
        assert!(!is_canonical_tag_format("deb,rpm"));
        assert!(!is_canonical_tag_format("rpm/ostree"));
        assert!(!is_canonical_tag_format("deb#"));
        assert!(!is_canonical_tag_format("emoji😀"));
    }

    #[test]
    fn digits_are_canonical() {
        assert!(is_canonical_tag_format("v1"));
        assert!(is_canonical_tag_format("0"));
        assert!(is_canonical_tag_format("tar.gz2"));
    }
}
