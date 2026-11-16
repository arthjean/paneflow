use crate::refusal::{base64, declaration, declares, launcher_label};
use std::path::Path;

#[test]
fn base64_matches_the_rfc_4648_vectors() {
    for (plain, encoded) in [
        ("", ""),
        ("f", "Zg=="),
        ("fo", "Zm8="),
        ("foo", "Zm9v"),
        ("foob", "Zm9vYg=="),
        ("fooba", "Zm9vYmE="),
        ("foobar", "Zm9vYmFy"),
        ("Apply?", "QXBwbHk/"),
    ] {
        assert_eq!(base64(plain.as_bytes()), encoded, "{plain:?}");
    }
    assert_eq!(base64(&[0xfb, 0xff]), "+/8=");
}

#[test]
fn the_declaration_is_an_error_record_of_its_own_named_after_the_tool() {
    assert_eq!(
        declaration("copilot", "Apply?"),
        "\u{1b}]7501;state=error:id=paneflow-shim:app=copilot:msg=QXBwbHk/\u{1b}\\"
    );
}

#[test]
fn a_refusal_is_declared_only_to_a_pane_terminal_with_no_agent_above() {
    assert!(declares(true, true, 0));
    assert!(!declares(false, true, 0), "outside a pane");
    assert!(!declares(true, false, 0), "captured output");
    assert!(!declares(true, true, 1), "a running agent owns the pane");
}

#[test]
fn the_launcher_label_keeps_the_last_two_names() {
    assert_eq!(
        launcher_label(Path::new("/opt/vscode/bin/copilot-shim/copilot")),
        "copilot-shim/copilot"
    );
    assert_eq!(launcher_label(Path::new("copilot")), "copilot");
    assert_eq!(launcher_label(Path::new("/")), "");
}
