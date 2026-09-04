use super::{del, ins, lines, mod_, splitter};

#[test]
fn equal_strings() {
    for text in ["", "x", "x_y_z_", "_", " x_y "] {
        lines(|t| {
            t.text(text, text);
            t.default(&[]);
            t.test_all();
        });
    }
}

#[test]
fn trivial_cases() {
    lines(|t| {
        t.text("x_", "y_");
        t.default(&[mod_(0, 0, 1, 1)]);
        t.test_all();
    });
    lines(|t| {
        t.text("x", "");
        t.default(&[mod_(0, 0, 1, 1)]);
        t.test_all();
    });
    lines(|t| {
        t.text("", "x");
        t.default(&[mod_(0, 0, 1, 1)]);
        t.test_all();
    });
    lines(|t| {
        t.text("x", "y");
        t.default(&[mod_(0, 0, 1, 1)]);
        t.test_all();
    });
    lines(|t| {
        t.text("x_z", "y_z");
        t.default(&[mod_(0, 0, 1, 1)]);
        t.test_all();
    });
    lines(|t| {
        t.text("z_x", "z_y");
        t.default(&[mod_(1, 1, 1, 1)]);
        t.test_all();
    });
    lines(|t| {
        t.text("x", "x_");
        t.default(&[ins(1, 1, 1)]);
        t.test_all();
    });
    lines(|t| {
        t.text("x_", "x");
        t.default(&[del(1, 1, 1)]);
        t.test_all();
    });
}

#[test]
fn simple_cases() {
    lines(|t| {
        t.text("x_z", "y_z");
        t.default(&[mod_(0, 0, 1, 1)]);
        t.test_all();
    });
    lines(|t| {
        t.text("x_", "x_z");
        t.default(&[mod_(1, 1, 1, 1)]);
        t.test_all();
    });
    lines(|t| {
        t.text("x_y", "n_m");
        t.default(&[mod_(0, 0, 2, 2)]);
        t.test_all();
    });
    lines(|t| {
        t.text("x_y_z", "n_y_m");
        t.default(&[mod_(0, 0, 1, 1), mod_(2, 2, 1, 1)]);
        t.test_all();
    });
    lines(|t| {
        t.text("x_y_z", "n_k_y");
        t.default(&[mod_(0, 0, 1, 2), del(2, 3, 1)]);
        t.test_all();
    });
    lines(|t| {
        t.text("x_y_z", "y");
        t.default(&[del(0, 0, 1), del(2, 1, 1)]);
        t.test_all();
    });
    lines(|t| {
        t.text("a_b_x", "x_m_n");
        t.default(&[del(0, 0, 2), ins(3, 1, 2)]);
        t.test_all();
    });
}

#[test]
fn empty_last_line() {
    lines(|t| {
        t.text("x_", "");
        t.default(&[del(0, 0, 1)]);
        t.test_all();
    });
    lines(|t| {
        t.text("", "x_");
        t.default(&[ins(0, 0, 1)]);
        t.test_all();
    });
    lines(|t| {
        t.text("x_", "x");
        t.default(&[del(1, 1, 1)]);
        t.test_all();
    });
    lines(|t| {
        t.text("x_", "x_z ");
        t.default(&[mod_(1, 1, 1, 1)]);
        t.test_all();
    });
}

#[test]
fn whitespace_only_changes() {
    lines(|t| {
        t.text("x ", " x");
        t.default(&[mod_(0, 0, 1, 1)]);
        t.trim(&[]);
        t.test_all();
    });
    lines(|t| {
        t.text("x \t", "\t x");
        t.default(&[mod_(0, 0, 1, 1)]);
        t.trim(&[]);
        t.test_all();
    });
    lines(|t| {
        t.text("x_", "x ");
        t.default(&[mod_(0, 0, 2, 1)]);
        t.trim(&[del(1, 1, 1)]);
        t.test_all();
    });
    lines(|t| {
        t.text(" x_y ", "x _ y");
        t.default(&[mod_(0, 0, 2, 2)]);
        t.trim(&[]);
        t.test_all();
    });
    lines(|t| {
        t.text("x y ", "x  y");
        t.default(&[mod_(0, 0, 1, 1)]);
        t.ignore(&[]);
        t.test_all();
    });
    lines(|t| {
        t.text("x y_x y_x y", "  x y  _x y  _x   y");
        t.default(&[mod_(0, 0, 3, 3)]);
        t.trim(&[mod_(2, 2, 1, 1)]);
        t.ignore(&[]);
        t.test_all();
    });
}

#[test]
fn algorithm_specific() {
    lines(|t| {
        t.text("x_y_z_AAAAA", "AAAAA_x_y_z");
        t.default(&[del(0, 0, 3), ins(4, 1, 3)]);
        t.test_all();
    });
    lines(|t| {
        t.text("x_y_z", " y_ m_ n");
        t.default(&[mod_(0, 0, 3, 3)]);
        t.trim(&[del(0, 0, 1), mod_(2, 1, 1, 2)]);
        t.test_all();
    });
    lines(|t| {
        t.text("}_ }", " }");
        t.default(&[del(0, 0, 1)]);
        t.test_default();
    });
    lines(|t| {
        t.text("{_}", "{_ {_ }_}_x");
        t.default(&[ins(1, 1, 2), ins(2, 4, 1)]);
        t.test_default();
    });
}

#[test]
fn non_deterministic_cases() {
    lines(|t| {
        t.text("", "__");
        t.default(&[ins(1, 1, 2)]);
        t.test_all();
    });
    lines(|t| {
        t.text("__", "");
        t.default(&[del(1, 1, 2)]);
        t.test_all();
    });
}

#[test]
fn regression_shifted_similar_lines_are_a_single_change() {
    lines(|t| {
        t.text(" X_  X", "  X_   X");
        t.matching_default("--_---", "---_----");
        t.test_default();
    });
}

#[test]
fn prefer_chunks_bounded_by_empty_line() {
    lines(|t| {
        t.text(
            "A_B_o_o_Y_Z_ _A_B_z_z_Y_Z",
            "A_B_o_o_Y_Z_ _A_B_u_u_Y_Z_ _A_B_z_z_Y_Z",
        );
        t.matching_default(
            " _ _ _ _ _ _ _ _ _ _ _ _ ",
            " _ _ _ _ _ _ _-_-_-_-_-_-_-_ _ _ _ _ _ ",
        );
        t.test_all();
    });
    lines(|t| {
        t.text(
            "A_B_o_o_Y_Z_ _A_B_ _ _Y_Z",
            "A_B_o_o_Y_Z_ _A_B_u_u_Y_Z_ _A_B_ _ _Y_Z",
        );
        t.matching_default(
            " _ _ _ _ _ _ _ _ _ _ _ _ ",
            " _ _ _ _ _ _ _ _ _-_-_-_-_-_-_-_ _ _ _ ",
        );
        t.test_all();
    });
    lines(|t| {
        t.text("A_B_o_o_ _A_B_z_z", "A_B_o_o_ _A_B_u_u_ _A_B_z_z");
        t.matching_default(" _ _ _ _ _ _ _ _ ", " _ _ _ _ _-_-_-_-_-_ _ _ _ ");
        t.test_all();
    });
    lines(|t| {
        t.text("o_o_Y_Z_ _z_z_Y_Z", "o_o_Y_Z_ _u_u_Y_Z_ _z_z_Y_Z");
        t.matching_default(" _ _ _ _ _ _ _ _ ", " _ _ _ _ _-_-_-_-_-_ _ _ _ ");
        t.test_all();
    });
}

#[test]
fn prefer_chunks_bounded_by_short_line() {
    lines(|t| {
        t.text(
            "A====_B====_o====_o====_Y====_Z====_!_A====_B====_z====_z====_Y====_Z====",
            "A====_B====_o====_o====_Y====_Z====_!_A====_B====_u====_u====_Y====_Z====_!_A====_B====_z====_z====_Y====_Z====",
        );
        t.matching_default(
            "     _     _     _     _     _     _ _     _     _     _     _     _     ",
            "     _     _     _     _     _     _ _-----_-----_-----_-----_-----_-----_-_     _     _     _     _     _     ",
        );
        t.test_all();
    });
    lines(|t| {
        t.text(
            "A====_B====_o====_o====_Y====_Z====_ _A====_B====_!_!_Y====_Z====",
            "A====_B====_o====_o====_Y====_Z====_ _A====_B====_u====_u====_Y====_Z====_ _A====_B====_!_!_Y====_Z====",
        );
        t.matching_default(
            "     _     _     _     _     _     _ _     _     _ _ _     _     ",
            "     _     _     _     _     _     _ _-----_-----_-----_-----_-----_-----_-_     _     _ _ _     _     ",
        );
        t.test_all();
    });
    lines(|t| {
        t.text(
            "A====_B====_o====_o====_Y====_Z====_!_A====_B====_ _ _Y====_Z====",
            "A====_B====_o====_o====_Y====_Z====_!_A====_B====_u====_u====_Y====_Z====_!_A====_B====_ _ _Y====_Z====",
        );
        t.matching_default(
            "     _     _     _     _     _     _ _     _     _ _ _     _     ",
            "     _     _     _     _     _     _ _     _     _-----_-----_-----_-----_-_-----_-----_ _ _     _     ",
        );
        t.test_all();
    });
}

#[test]
fn prefer_chunks_bounded_by_empty_line_using_inserted_content() {
    lines(|t| {
        t.text("e_x_A_B_z", "x_A_B_ _q_A_B_z");
        t.matching_default("-_ _ _ _ ", " _ _ _-_-_-_-_ ");
        t.test_all();
    });
}

#[test]
fn prefer_smaller_amount_of_chunks() {
    lines(|t| {
        t.text("X_A_X_Y_", "X_Y_");
        t.matching_default("-_-_ _ _", " _ _");
        t.test_all();
    });
    lines(|t| {
        t.text(" __x___y", "__y");
        t.matching_default("-**-*__ ", "__ ");
        t.default(&[del(0, 0, 3)]);
        t.test_all();
    });
    lines(|t| {
        t.text(
            "U======_X======_Y======_z======_X======_Y======_X======_U======_X======_z======",
            "U======_Y======_X======_U======_X======",
        );
        t.matching_default(
            "       _-------_-------_-------_-------_       _       _       _       _-------",
            "       _       _       _       _       ",
        );
        t.test_all();
    });
}

#[test]
fn regression_can_trim_chunks_after_compare_two_steps() {
    lines(|t| {
        t.text("q__7_ 6_ 7", "_7");
        t.matching_default("-*_ _--*--", "_ ");
        t.test_default();
    });
}

#[test]
fn regression_can_trim_chunks_after_optimize_line_chunks() {
    lines(|t| {
        t.text("A=====_ B=====_ }_}_B=====_", "A=====_ }_}_B=====_");
        t.matching_default("      _-------_  _ _      _", "      _  _ _      _");
        t.test_all();
    });
}

#[test]
fn bad_cases_caused_by_compare_two_step_logic() {
    lines(|t| {
        t.text("x_!", "!_x_y");
        t.matching_default("-_ ", " _-_-");
        t.test_all();
    });
    lines(|t| {
        t.text("!_x_y", "x_!");
        t.matching_default("-_ _-", " _-");
        t.test_all();
    });
    lines(|t| {
        t.text("x_! ", "!_x_y");
        t.matching_default("-_--", "-_-_-");
        t.matching_trim("-_  ", " _-_-");
        t.test_all();
    });
    lines(|t| {
        t.text("!_x_y", "x_! ");
        t.matching_default("-_ _-", " _--");
        t.test_all();
    });
    splitter(|t| {
        t.text("M===_X===_Y===", " Y===_X===_N");
        t.default(&[mod_(0, 0, 1, 1), mod_(1, 1, 1, 1), mod_(2, 2, 1, 1)]);
        t.test_default();
    });
}

#[test]
fn bad_cases_caused_by_compare_smart_logic() {
    lines(|t| {
        t.text("A=====_ B=====_ }_}_B=====", "A=====_ }_}_B=====");
        t.matching_default("      _-------_  _ _      ", "      _  _ _      ");
        t.matching_trim("      _       _--_-_------", "      _--_-_      ");
        t.test_all();
    });
    lines(|t| {
        t.text("A=====_ B=====_X_ }_}_Z_B=====_", "A=====_ }_}_B=====_");
        t.matching_default("      _-------_-_--_-_-_      _", "      _--_-_      _");
        t.test_all();
    });
}

#[test]
fn trim_changed_blocks_after_second_step_correction() {
    lines(|t| {
        t.text("====}_==== }_Y_====}", "====}_Y_====}");
        t.matching_default("     _------_ _     ", "     _ _     ");
        t.matching_ignore("     _      _-_-----", "     _-_     ");
        t.test_all();
    });
}

#[test]
fn second_step_correction_processes_all_confusing_lines() {
    lines(|t| {
        t.text("====}_==== }_Y_==== }_====}", "==== }_Y_==== }");
        t.matching_default("-----_      _ _      _-----", "      _ _      ");
        t.test_default();
    });
}

#[test]
fn ignore_whitespace_policy_does_not_apply_two_step_correction() {
    lines(|t| {
        t.text("1_ _  1", "  1");
        t.matching_default("-_-_   ", "   ");
        t.matching_trim(" _-_---", "   ");
        t.test_all();
    });
    lines(|t| {
        t.text("  1_ _1", "  1");
        t.matching_default("   _-_-", "   ");
        t.test_all();
    });
    lines(|t| {
        t.text("X_ Y_X", "Y ");
        t.matching_default("-_--_-", "--");
        t.matching_trim("-_  _-", "  ");
        t.test_all();
    });
}

#[test]
fn regression_second_step_correction_runs_without_ambiguous_matchings() {
    lines(|t| {
        t.text("}_ }", " }_}");
        t.matching_default("-_--", "--_-");
        t.matching_trim(" _  ", "  _ ");
        t.test_all();
    });
    lines(|t| {
        t.text(" }_}_ }", "}_}_}");
        t.matching_default("--_ _--", "-_ _-");
        t.matching_trim("  _ _  ", " _ _ ");
        t.test_all();
    });
    lines(|t| {
        t.text("X_X __Y", "X__Z");
        t.matching_default(" _--__-", " __-");
        t.matching_trim("-_  __-", " __-");
        t.test_all();
    });
}

#[test]
fn regression_second_step_with_too_many_possible_matchings() {
    lines(|t| {
        t.text(
            " X_X_X_X_X_X_X_X_X_X_X_X_X_X_X_X_X_X_X_X_X_X_X_ X",
            "X_X_X_X_X_X_X_X_X_X_X_X_X_X_X_X_X_X_X ",
        );
        t.matching_default(
            "--_ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _-_-_-_-_-_--",
            "-_ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _--",
        );
        t.matching_trim(
            "  _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _-_-_-_-_--",
            " _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _  ",
        );
        t.test_all();
    });
}

#[test]
fn regression_second_step_correction_searches_matchings_in_its_prefix() {
    lines(|t| {
        t.text("Z_X_Z_X __Y", "X__Y");
        t.matching_default("-_ _-_--__ ", " __ ");
        t.matching_trim("-_-_-_  __ ", " __ ");
        t.test_all();
    });
    lines(|t| {
        t.text("Z_X_K_Z_X __Y", "K _X__Y");
        t.matching_default("-_-_-_-_--__ ", "--_-__ ");
        t.matching_trim("-_-_ _-_  __ ", "  _ __ ");
        t.test_all();
    });
}

#[test]
fn regression_multiple_similar_braces_nearby() {
    lines(|t| {
        t.text(
            "function fn() {_  return {_    a: 1_  }_}_",
            "function fn() {_  return {_    a: 1_  };_}_",
        );
        t.matching_default(
            "               _          _        _---_ _",
            "               _          _        _----_ _",
        );
        t.test_all();
    });
    lines(|t| {
        t.text(
            "function fn() {_  return {_    a: 1_  }_}_",
            "function fn() {_  return {_    a: 1_  }_};_",
        );
        t.matching_default(
            "               _          _        _   _-_",
            "               _          _        _   _--_",
        );
        t.test_all();
    });
}
