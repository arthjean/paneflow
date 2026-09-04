use super::{del, lines_inner, words, CH_FACE, CH_GUN, CH_MAN, CH_SMILE};

#[test]
fn simple_cases() {
    lines_inner(|t| {
        t.text("x z", "y z");
        t.matching_default("-  ", "-  ");
        t.test_all();
    });
    lines_inner(|t| {
        t.text("x z", "y z");
        t.matching_default("-  ", "-  ");
        t.test_all();
    });
    lines_inner(|t| {
        t.text(" x z", "y z");
        t.matching_default("--  ", "-  ");
        t.matching_trim(" -  ", "-  ");
        t.test_all();
    });
    lines_inner(|t| {
        t.text("x z ", "y z");
        t.matching_default("-  -", "-  ");
        t.matching_trim("-   ", "-  ");
        t.test_all();
    });
    lines_inner(|t| {
        t.text("x z ", "y z");
        t.matching_default("-  -", "-  ");
        t.matching_trim("-   ", "-  ");
        t.test_all();
    });
    lines_inner(|t| {
        t.text("x z", " y z ");
        t.matching_default("-  ", "--  -");
        t.matching_trim("-  ", " -   ");
        t.test_all();
    });
    lines_inner(|t| {
        t.text("x y", "x z ");
        t.matching_default("  -", "  --");
        t.matching_trim("  -", "  - ");
        t.test_all();
    });
    lines_inner(|t| {
        t.text("x,y", "x");
        t.matching_default(" --", " ");
        t.test_all();
    });
    lines_inner(|t| {
        t.text("x,y", "y");
        t.matching_default("-- ", " ");
        t.test_all();
    });
    lines_inner(|t| {
        t.text(".x=", ".!=");
        t.matching_default(" - ", " - ");
        t.test_all();
    });
    lines_inner(|t| {
        t.text("X xyz1 Z", "X xyz2 Z");
        t.matching_default("  ----  ", "  ----  ");
        t.test_all();
    });
}

#[test]
fn punctuation() {
    lines_inner(|t| {
        t.text(" x.z.x ", "x..x");
        t.matching_default("-  -  -", "    ");
        t.matching_trim("   -   ", "    ");
        t.test_all();
    });
    lines_inner(|t| {
        t.text("x..x", " x.z.x ");
        t.matching_default("    ", "-  -  -");
        t.matching_trim("    ", "   -   ");
        t.test_all();
    });
    lines_inner(|t| {
        t.text("x ... z", "y ... z");
        t.matching_default("-      ", "-      ");
        t.test_all();
    });
    lines_inner(|t| {
        t.text("x ... z", "x ... y");
        t.matching_default("      -", "      -");
        t.test_all();
    });
    lines_inner(|t| {
        t.text("x ,... z", "x ... y");
        t.matching_default("  -    -", "      -");
        t.test_all();
    });
    lines_inner(|t| {
        t.text("x . , .. z", "x ... y");
        t.matching_default("   ---   -", "      -");
        t.matching_ignore("    -    -", "      -");
        t.test_all();
    });
    lines_inner(|t| {
        t.text("x==y==z", "x====z");
        t.matching_default("   -   ", "      ");
        t.test_all();
    });
    lines_inner(|t| {
        t.text("x====z", "x==t==z");
        t.matching_default("      ", "   -   ");
        t.test_all();
    });
    lines_inner(|t| {
        t.text("X Y ) {_ A B C", "X Y Z ) {_ y B C ) {");
        t.matching_default("         -    ", "   --      -    ----");
        t.matching_ignore("         -    ", "    -      -     ---");
        t.test_all();
    });
    words(|t| {
        t.text("@Deprecated @NotNull", "@NotNull");
        t.matching_default(" ------------       ", "        ");
        t.test_all();
    });
    words(|t| {
        t.text("@Deprecated_ @NotNull", "@NotNull");
        t.matching_default(" -------------       ", "        ");
        t.test_all();
    });
}

#[test]
fn old_diff_bug() {
    lines_inner(|t| {
        t.text("x'y'>", "x'>");
        t.matching_default("  -- ", "   ");
        t.test_all();
    });
    lines_inner(|t| {
        t.text("x'>", "x'y'>");
        t.matching_default("   ", "  -- ");
        t.test_all();
    });
    lines_inner(|t| {
        t.text("x'>", "x'y'>");
        t.matching_default("   ", "  -- ");
        t.test_all();
    });
    lines_inner(|t| {
        t.text("x'y'>", "x'>");
        t.matching_default("  -- ", "   ");
        t.test_all();
    });
}

#[test]
fn whitespace_only_changes() {
    lines_inner(|t| {
        t.text("x  =z", "x=  z");
        t.matching_default(" --  ", "  -- ");
        t.test_default();
        t.test_trim();
    });
    lines_inner(|t| {
        t.text("x  =", "x=  z");
        t.matching_default(" -- ", "  ---");
        t.matching_ignore("    ", "    -");
        t.test_all();
    });
}

#[test]
fn newlines() {
    lines_inner(|t| {
        t.text(" x _ y _ z ", "x z");
        t.matching_default("- ------  -", "   ");
        t.matching_trim("     -     ", "   ");
        t.matching_ignore("     -     ", "   ");
        t.test_all();
    });
    lines_inner(|t| {
        t.text("x z", " x _ y _ z ");
        t.matching_default("   ", "- ------  -");
        t.matching_trim("   ", "     -     ");
        t.matching_ignore("   ", "     -     ");
        t.test_all();
    });
    words(|t| {
        t.text("_i", "i_");
        t.matching_default("- ", " -");
        t.matching_trim("  ", "  ");
        t.test_all();
    });
    words(|t| {
        t.text("i_", "_i");
        t.matching_default("- ", " -");
        t.test_all();
    });
    words(|t| {
        t.text("x_y", "xy");
        t.matching_ignore("   ", "  ");
        t.test_ignore();
    });
    words(|t| {
        t.text("A x_y B", "a xy b");
        t.matching_ignore("-------", "------");
        t.test_ignore();
    });
    words(|t| {
        t.text("A xy B", "a xy b");
        t.matching_ignore("-    -", "-    -");
        t.test_ignore();
    });
    words(|t| {
        t.text("A_B_", "X_");
        t.matching_default("--- ", "- ");
        t.test_all();
    });
}

#[test]
fn fixed_bugs() {
    lines_inner(|t| {
        t.text(".! ", ".  y!");
        t.matching_default("  -", " --- ");
        t.matching_trim("   ", " --- ");
        t.matching_ignore("   ", "   - ");
        t.test_all();
    });
    lines_inner(|t| {
        t.text(" x n", " y_  x m");
        t.matching_default("   -", "----   -");
        t.matching_trim("   -", " -     -");
        t.matching_ignore("   -", " -     -");
        t.test_all();
    });
    lines_inner(|t| {
        t.text("x_", "x!  ");
        t.matching_default(" -", " ---");
        t.matching_trim("  ", " -  ");
        t.matching_ignore("  ", " -  ");
        t.test_all();
    });
}

#[test]
fn inner_whitespaces() {
    lines_inner(|t| {
        t.text("<< x >>", "<.<>.>");
        t.matching_default("  ---  ", " -  - ");
        t.matching_ignore("   -   ", " -  - ");
        t.test_all();
    });
    lines_inner(|t| {
        t.text("<< x >>", "y<<x>>y");
        t.matching_default("  - -  ", "-     -");
        t.matching_ignore("       ", "-     -");
        t.test_all();
    });
    lines_inner(|t| {
        t.text("x .. z", "x y .. z");
        t.matching_default("      ", " --     ");
        t.matching_ignore("      ", "  -     ");
        t.test_all();
    });
    lines_inner(|t| {
        t.text("  x..z", "x..y  ");
        t.matching_default("--   -", "   ---");
        t.matching_trim("     -", "   -  ");
        t.test_all();
    });
    lines_inner(|t| {
        t.text(" x y x _ x z x ", "x x_x x");
        t.matching_default("- --  - - --  -", "       ");
        t.matching_trim("  --      --   ", "       ");
        t.matching_ignore("   -       -   ", "       ");
        t.test_all();
    });
}

#[test]
fn algorithm_specific() {
    lines_inner(|t| {
        t.text("...x", "x...");
        t.matching_default("--- ", " ---");
        t.test_all();
    });
    lines_inner(|t| {
        t.text("x x y", "x y");
        t.matching_default("--   ", "   ");
        t.matching_ignore("-    ", "   ");
        t.test_all();
    });
    lines_inner(|t| {
        t.text("y x x", "y x");
        t.matching_default("   --", "   ");
        t.matching_ignore("    -", "   ");
        t.test_all();
    });
    lines_inner(|t| {
        t.text("A X A B", "A B");
        t.matching_default("----   ", "   ");
        t.matching_ignore("---    ", "   ");
        t.test_all();
    });
    lines_inner(|t| {
        t.text("A.X A.Z", "A.X A.Y A.Z");
        t.matching_default("       ", "   ----    ");
        t.matching_ignore("       ", "    ---    ");
        t.test_all();
    });
    lines_inner(|t| {
        t.text("X.A Z.A", "X.A Y.A Z.A");
        t.matching_default("       ", "   ----    ");
        t.matching_ignore("       ", "    ---    ");
        t.test_all();
    });
    lines_inner(|t| {
        t.text(".   ", "   .");
        t.matching_default(" ---", "--- ");
        t.test_default();
    });
    lines_inner(|t| {
        t.text("A B_C D", "A_B C_D");
        t.matching_default(" --  - ", "  -- - ");
        t.matching_trim(" --  - ", "  --   ");
        t.matching_ignore("  -    ", "  -    ");
        t.test_all();
    });
    lines_inner(|t| {
        t.text("B_C_D_", "X_Y_Z_");
        t.matching_default("- - - ", "- - - ");
        t.test_all();
    });
    words(|t| {
        t.text("!x_!_z", "!_!_y z");
        t.matching_default(" -    ", "    -- ");
        t.test_default();
    });
}

#[test]
fn trailing_punctuation() {
    lines_inner(|t| {
        t.text("X = { };", "X = { _ };");
        t.matching_default("        ", "     --   ");
        t.matching_trim("        ", "          ");
        t.test_all();
    });
    lines_inner(|t| {
        t.text("X = { };_", "X = { _ };_");
        t.matching_default("      -- ", "       ----");
        t.matching_trim("      -- ", "        -- ");
        t.test_all();
    });
}

#[test]
fn legacy_cases_from_by_word_test() {
    lines_inner(|t| {
        t.text("abc def, 123", "ab def, 12");
        t.matching_default("---      ---", "--      --");
        t.test_all();
    });
    lines_inner(|t| {
        t.text(" a[xy]+1", ",a[]+1");
        t.matching_default("-  --   ", "-     ");
        t.matching_trim("   --   ", "-     ");
        t.test_all();
    });
    lines_inner(|t| {
        t.text("0987_  a.g();_", "yyyy_");
        t.matching_default("------------- ", "---- ");
        t.test_all();
    });
    lines_inner(|t| {
        t.text("  abc_2222_", "    x = abc_zzzz_");
        t.matching_default("      ---- ", " ------     ---- ");
        t.matching_trim("      ---- ", "    ---     ---- ");
        t.test_all();
    });
    lines_inner(|t| {
        t.text(
            "   if (eventMerger!=null && !dataSelection.getValueIsAdjusting()) {",
            "   if (eventMerger!=null && (dataSelection==null || !dataSelection.getValueIsAdjusting())) {",
        );
        t.matching_default(
            "                                                                   ",
            "                           ------------------------                                      -  ",
        );
        t.matching_ignore(
            "                                                                   ",
            "                            -----------------------                                      -  ",
        );
        t.test_all();
    });
    lines_inner(|t| {
        t.plain_text(
            "messageInsertStatement = connection.prepareStatement(\"INSERT INTO AUDIT (AUDIT_TYPE_ID, STATUS, SERVER_ID, INSTANCE_ID, REQUEST_ID) VALUES (?, ?, ?, ?, ?)\");",
            "messageInsertStatement = connection.prepareStatement(\"INSERT INTO AUDIT (AUDIT_TYPE_ID, CREATION_TIMESTAMP, STATUS, SERVER_ID, INSTANCE_ID, REQUEST_ID) VALUES (?, ?, ?, ?, ?, ?)\");",
        );
        t.matching_default(
            "                                                     .                                                                                                     .   ",
            "                                                     .                                  --------------------                                                                  --- .   ",
        );
        t.matching_ignore(
            "                                                     .                                                                                                     .   ",
            "                                                     .                                   -------------------                                                                  --- .   ",
        );
        t.test_all();
    });
    lines_inner(|t| {
        t.text("f(a, b);", "f(a,_  b);");
        t.matching_default("        ", "    --    ");
        t.matching_trim("        ", "          ");
        t.test_all();
    });
    lines_inner(|t| {
        t.text(" o.f(a)", "o. f( b)");
        t.matching_default("-    - ", "  -  -- ");
        t.matching_trim("     - ", "  -  -- ");
        t.matching_ignore("     - ", "      - ");
        t.test_all();
    });
    lines_inner(|t| {
        t.text(" 123 ", "xyz");
        t.matching_trim(" --- ", "---");
        t.test_trim();
    });
}

#[test]
fn empty_range_positions() {
    lines_inner(|t| {
        t.text("x? y", "x y");
        t.matching_default(" -  ", "   ");
        t.default(&[del(1, 1, 1)]);
        t.test_all();
    });
    lines_inner(|t| {
        t.text("x ?y", "x y");
        t.matching_default("  - ", "   ");
        t.default(&[del(2, 2, 1)]);
        t.test_all();
    });
}

#[test]
fn continuous_script() {
    words(|t| {
        t.text("ABCD", "DABC");
        t.matching_default("----", "----");
        t.test_default();
    });
    words(|t| {
        t.text("汉语漢語", "語汉语漢");
        t.matching_default("   -", "-   ");
        t.test_default();
    });
    words(|t| {
        t.text("AB漢CD", "DA漢CD");
        t.matching_default("--   ", "--   ");
        t.test_default();
    });
    words(|t| {
        t.text("AB漢CD", "DA语CD");
        t.matching_default("---  ", "---  ");
        t.test_default();
    });
    words(|t| {
        t.plain_text("a_c", "x_c");
        t.matching_default("---", "---");
        t.test_default();
    });
    words(|t| {
        t.text("!_?", "!_+");
        t.matching_default("  -", "  +");
        t.test_default();
    });
}

#[test]
fn high_surrogates() {
    words(|t| {
        t.text(
            &format!("{CH_SMILE}{CH_GUN}{CH_MAN}{CH_FACE}"),
            &format!("{CH_GUN}{CH_MAN}{CH_FACE}{CH_SMILE}"),
        );
        t.matching_default("--      ", "      --");
        t.test_default();
    });
    words(|t| {
        t.text(CH_SMILE, CH_GUN);
        t.matching_default("--", "--");
        t.test_default();
    });
    words(|t| {
        t.text(CH_FACE, CH_GUN);
        t.matching_default("--", "--");
        t.test_default();
    });
    words(|t| {
        t.text(&format!("{CH_SMILE} "), &format!(" {CH_GUN}"));
        t.matching_default("---", "---");
        t.matching_trim("-- ", " --");
        t.test_all();
    });
    words(|t| {
        t.text(
            &format!("{CH_SMILE}{CH_FACE} {CH_MAN}"),
            &format!(" {CH_GUN}{CH_FACE}{CH_MAN} "),
        );
        t.matching_default("--  -  ", "---    -");
        t.matching_trim("--  -  ", " --     ");
        t.matching_ignore("--     ", " --     ");
        t.test_all();
    });
}
