use super::{chars, chars_raw, chars_smart, del, ins, CH_FACE, CH_GUN, CH_MAN, CH_SMILE};

#[test]
fn equal_strings() {
    for (text, matching) in [
        ("", ""),
        ("x", " "),
        ("x_y_z_", "      "),
        ("_", " "),
        ("xxx", "   "),
        ("xyz", "   "),
        (".x!", "   "),
    ] {
        chars(|t| {
            t.text(text, text);
            t.matching_default(matching, matching);
            t.test_all();
        });
    }
}

#[test]
fn trivial_cases() {
    chars(|t| {
        t.text("x", "");
        t.matching_default("-", "");
        t.test_all();
    });
    chars(|t| {
        t.text("", "x");
        t.matching_default("", "-");
        t.test_all();
    });
    chars(|t| {
        t.text("x", "y");
        t.matching_default("-", "-");
        t.test_all();
    });
    chars(|t| {
        t.text("x_", "");
        t.matching_default("--", "");
        t.matching_trim("- ", "");
        t.test_all();
    });
    chars(|t| {
        t.text("", "x_");
        t.matching_default("", "--");
        t.matching_trim("", "- ");
        t.test_all();
    });
    chars(|t| {
        t.text("x_", "y_");
        t.matching_default("- ", "- ");
        t.test_all();
    });
    chars(|t| {
        t.text("_x", "_y");
        t.matching_default(" -", " -");
        t.test_all();
    });
}

#[test]
fn simple_cases() {
    chars(|t| {
        t.text("xyx", "xxx");
        t.matching_default(" - ", " - ");
        t.test_all();
    });
    chars(|t| {
        t.text("xyyx", "xmx");
        t.matching_default(" -- ", " - ");
        t.test_all();
    });
    chars(|t| {
        t.text("xyy", "yyx");
        t.matching_default("-  ", "  -");
        t.test_all();
    });
    chars(|t| {
        t.text("x!!", "!!x");
        t.matching_default("-  ", "  -");
        t.test_all();
    });
    chars(|t| {
        t.text("xyx", "xx");
        t.matching_default(" - ", "  ");
        t.test_all();
    });
    chars(|t| {
        t.text("xx", "xyx");
        t.matching_default("  ", " - ");
        t.test_all();
    });
    chars(|t| {
        t.text("!...", "...!");
        t.matching_default("-   ", "   -");
        t.test_all();
    });
}

#[test]
fn whitespace_changes_only() {
    chars(|t| {
        t.text(" x y z ", "xyz");
        t.matching_default("- - - -", "   ");
        t.matching_trim("  - -  ", "   ");
        t.matching_ignore("       ", "   ");
        t.test_all();
    });
    chars(|t| {
        t.text("xyz", " x y z ");
        t.matching_default("   ", "- - - -");
        t.matching_trim("   ", "  - -  ");
        t.matching_ignore("   ", "       ");
        t.test_all();
    });
    chars(|t| {
        t.text("x ", "x");
        t.matching_default(" -", " ");
        t.matching_trim("  ", " ");
        t.test_all();
    });
    chars(|t| {
        t.text("x", " x");
        t.matching_default(" ", "- ");
        t.matching_trim(" ", "  ");
        t.test_all();
    });
    chars(|t| {
        t.text(" x ", "x");
        t.matching_default("- -", " ");
        t.matching_trim("   ", " ");
        t.test_all();
    });
    chars(|t| {
        t.text("x", " x ");
        t.matching_default(" ", "- -");
        t.matching_trim(" ", "   ");
        t.test_all();
    });
}

#[test]
fn whitespace_changes() {
    chars(|t| {
        t.text(" x ", "z");
        t.matching_default("---", "-");
        t.matching_trim(" - ", "-");
        t.test_all();
    });
    chars(|t| {
        t.text("x", " z ");
        t.matching_default("-", "---");
        t.matching_trim("-", " - ");
        t.test_all();
    });
    chars(|t| {
        t.text(" x", "z\t");
        t.matching_default("--", "-.-");
        t.matching_trim(" -", "-. ");
        t.test_all();
    });
    chars(|t| {
        t.text("x ", "\tz");
        t.matching_default("--", ".--");
        t.matching_trim("- ", ". -");
        t.test_all();
    });
    chars(|t| {
        t.text("x y z", "xy_z");
        t.matching_default(" - - ", "  - ");
        t.matching_trim(" - - ", "    ");
        t.matching_ignore("     ", "    ");
        t.test_all();
    });
    chars(|t| {
        t.text("x y \n z", "xy\nz");
        t.matching_default(" - -. - ", "  .  ");
        t.matching_trim(" -  .   ", "  .  ");
        t.matching_ignore("    .   ", "  .  ");
        t.test_all();
    });
}

#[test]
fn ignore_inner_whitespaces() {
    chars(|t| {
        t.text("x z y", "xmn");
        t.matching_default(" ----", " --");
        t.matching_ignore("  ---", " --");
        t.test_all();
    });
    chars(|t| {
        t.text("x y z ", "x y m ");
        t.matching_default("    - ", "    - ");
        t.test_all();
    });
    chars(|t| {
        t.text("x y z", "x y m ");
        t.matching_default("    -", "    --");
        t.matching_trim("    -", "    - ");
        t.test_all();
    });
    chars(|t| {
        t.text(" x y z", " m y z");
        t.matching_default(" -    ", " -    ");
        t.test_all();
    });
    chars(|t| {
        t.text("x y z", " m y z");
        t.matching_default("-    ", "--    ");
        t.matching_trim("-    ", " -    ");
        t.test_all();
    });
    chars(|t| {
        t.text("x y z", "x m z");
        t.matching_default("  -  ", "  -  ");
        t.test_all();
    });
    chars(|t| {
        t.text("x y z", "x  z");
        t.matching_default("  -  ", "    ");
        t.test_all();
    });
    chars(|t| {
        t.text("x  z", "x m z");
        t.matching_default("    ", "  -  ");
        t.test_all();
    });
    chars(|t| {
        t.text("x  z", "x n m z");
        t.matching_default("    ", "  ---  ");
        t.test_all();
    });
}

#[test]
fn empty_range_positions() {
    chars(|t| {
        t.text("x y", "x zy");
        t.default(&[ins(2, 2, 1)]);
        t.test_all();
    });
    chars(|t| {
        t.text("x y", "xz y");
        t.default(&[ins(1, 1, 1)]);
        t.test_all();
    });
    chars(|t| {
        t.text("x y z", "x  z");
        t.default(&[del(2, 2, 1)]);
        t.test_all();
    });
    chars(|t| {
        t.text("x  z", "x m z");
        t.default(&[ins(2, 2, 1)]);
        t.test_all();
    });
    chars(|t| {
        t.text("xyx", "xx");
        t.default(&[del(1, 1, 1)]);
        t.test_all();
    });
    chars(|t| {
        t.text("xx", "xyx");
        t.default(&[ins(1, 1, 1)]);
        t.test_all();
    });
    chars(|t| {
        t.text("xy", "x");
        t.default(&[del(1, 1, 1)]);
        t.test_all();
    });
    chars(|t| {
        t.text("x", "xy");
        t.default(&[ins(1, 1, 1)]);
        t.test_all();
    });
}

#[test]
fn algorithm_specific() {
    chars(|t| {
        t.text("x   y   z", "xX      Zz");
        t.matching_default("    -    ", " -      - ");
        t.matching_ignore("    -    ", " -------- ");
        t.test_all();
    });
}

#[test]
fn non_deterministic_cases() {
    chars(|t| {
        t.text("x", "  ");
        t.ignore(&[del(0, 0, 1)]);
        t.test_ignore();
    });
    chars(|t| {
        t.text("  ", "x");
        t.ignore(&[ins(0, 0, 1)]);
        t.test_ignore();
    });
    chars(|t| {
        t.text("x .. z", "x y .. z");
        t.matching_default("      ", "  --    ");
        t.matching_ignore("      ", "  -     ");
        t.default(&[ins(2, 2, 2)]);
        t.ignore(&[ins(2, 2, 1)]);
        t.test_all();
    });
    chars_smart(|t| {
        t.text(" x _ y _ z ", "x z");
        t.matching_default("-  ------ -", "   ");
        t.matching_trim("     -     ", "   ");
        t.default(&[del(0, 0, 1), del(3, 2, 6), del(10, 3, 1)]);
        t.trim(&[del(5, 2, 1)]);
        t.test_all();
    });
    chars_raw(|t| {
        t.text(" x _ y _ z ", "x z");
        t.matching_default("- -- ---- -", "   ");
        t.default(&[del(0, 0, 1), del(2, 1, 2), del(5, 2, 4), del(10, 3, 1)]);
        t.test_default();
    });
    chars_smart(|t| {
        t.text("x z", " x _ y _ z ");
        t.matching_default("   ", "-  ------ -");
        t.matching_trim("   ", "     -     ");
        t.default(&[ins(0, 0, 1), ins(2, 3, 6), ins(3, 10, 1)]);
        t.trim(&[ins(2, 5, 1)]);
        t.test_all();
    });
    chars_raw(|t| {
        t.text("x z", " x _ y _ z ");
        t.matching_default("   ", "- -- ---- -");
        t.default(&[ins(0, 0, 1), ins(1, 2, 2), ins(2, 5, 4), ins(3, 10, 1)]);
        t.test_default();
    });
}

#[test]
fn two_steps() {
    chars_smart(|t| {
        t.text("  a", "a  ");
        t.matching_default("-- ", " --");
        t.test_default();
    });
    chars_raw(|t| {
        t.text("  a", "a  ");
        t.matching_default("  -", "-  ");
        t.test_default();
    });
    chars_smart(|t| {
        t.text("bba", "abb");
        t.matching_default("  -", "-  ");
        t.test_default();
    });
    chars_raw(|t| {
        t.text("bba", "abb");
        t.matching_default("  -", "-  ");
        t.test_default();
    });
    chars_smart(|t| {
        t.text(
            &format!("{CH_SMILE}{CH_FACE} {CH_MAN}"),
            &format!(" {CH_GUN}{CH_FACE}{CH_MAN} "),
        );
        t.matching_default("--  -  ", "---    -");
        t.test_default();
    });
    chars_raw(|t| {
        t.text(
            &format!("{CH_SMILE}{CH_FACE} {CH_MAN}"),
            &format!(" {CH_GUN}{CH_FACE}{CH_MAN} "),
        );
        t.matching_default("----   ", " ----  -");
        t.test_default();
    });
}

#[test]
fn high_surrogates() {
    chars(|t| {
        t.text(CH_SMILE, CH_SMILE);
        t.matching_default("  ", "  ");
        t.test_all();
    });
    chars(|t| {
        t.text(CH_SMILE, CH_MAN);
        t.matching_default("--", "--");
        t.test_all();
    });
    chars(|t| {
        t.text(CH_SMILE, CH_GUN);
        t.matching_default("--", "--");
        t.test_all();
    });
    chars(|t| {
        t.text(CH_FACE, CH_GUN);
        t.matching_default("--", "--");
        t.test_all();
    });
    chars(|t| {
        t.text(
            &format!("{CH_SMILE}{CH_GUN}{CH_MAN}{CH_FACE}"),
            &format!("{CH_GUN}{CH_MAN}{CH_FACE}{CH_SMILE}"),
        );
        t.matching_default("--      ", "      --");
        t.test_all();
    });
    chars(|t| {
        t.text(&format!("{CH_SMILE} "), CH_GUN);
        t.matching_default("---", "--");
        t.matching_trim("-- ", "--");
        t.test_all();
    });
    chars(|t| {
        t.text(
            &format!(" {CH_SMILE}{CH_FACE} {CH_MAN}"),
            &format!("{CH_GUN}{CH_FACE}{CH_MAN}"),
        );
        t.matching_default("---  -  ", "--    ");
        t.matching_trim(" --  -  ", "--    ");
        t.matching_ignore(" --     ", "--    ");
        t.test_all();
    });
}
