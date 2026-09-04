use super::{del, ins, mod_, splitter, splitter_with};

#[test]
fn splitter_blocks() {
    splitter(|t| {
        t.text("x", "z");
        t.default(&[mod_(0, 0, 1, 1)]);
        t.test_all();
    });
    splitter(|t| {
        t.text("x_y", "a_b");
        t.default(&[mod_(0, 0, 2, 2)]);
        t.test_all();
    });
    splitter(|t| {
        t.text("x_y", "a_b y");
        t.default(&[mod_(0, 0, 1, 1), mod_(1, 1, 1, 1)]);
        t.test_all();
    });
    splitter(|t| {
        t.text("x y", "x a_b y");
        t.default(&[mod_(0, 0, 1, 2)]);
        t.test_all();
    });
    splitter(|t| {
        t.text("x_y", "x a_y b");
        t.default(&[mod_(0, 0, 1, 1), mod_(1, 1, 1, 1)]);
        t.test_all();
    });
    splitter(|t| {
        t.text("x_", "x a_...");
        t.default(&[mod_(0, 0, 2, 2)]);
        t.test_all();
    });
    splitter(|t| {
        t.text("x_y_", "a_b_");
        t.default(&[mod_(0, 0, 2, 2)]);
        t.test_all();
    });
    splitter(|t| {
        t.text("x_y", " x _ y ");
        t.default(&[mod_(0, 0, 2, 2)]);
        t.test_default();
    });
    splitter(|t| {
        t.text("x_y", " x _ y.");
        t.default(&[mod_(0, 0, 1, 1), mod_(1, 1, 1, 1)]);
        t.test_default();
    });
    splitter(|t| {
        t.text("a_x_b_", " x_");
        t.default(&[del(0, 0, 1), mod_(1, 0, 1, 1), del(2, 1, 1)]);
        t.test_default();
    });
    splitter(|t| {
        t.text("a_x_b_", "!x_");
        t.default(&[del(0, 0, 1), mod_(1, 0, 1, 1), del(2, 1, 1)]);
        t.test_all();
    });
    splitter_with(false, false, |t| {
        t.text("< i", "<i<_i");
        t.default(&[mod_(0, 0, 1, 2)]);
        t.test_default();
    });
    splitter_with(false, false, |t| {
        t.text(".l j", "U_._l j+");
        t.default(&[ins(0, 0, 2), mod_(0, 2, 1, 1)]);
        t.test_default();
    });
    splitter(|t| {
        t.text("      run();", "      //commentary_      while(run(true));");
        t.default(&[mod_(0, 0, 1, 2)]);
        t.test_default();
    });
}

#[test]
fn squash() {
    splitter_with(true, false, |t| {
        t.text("x", "z");
        t.default(&[mod_(0, 0, 1, 1)]);
        t.test_all();
    });
    splitter_with(true, false, |t| {
        t.text("x_y", "a_b");
        t.default(&[mod_(0, 0, 2, 2)]);
        t.test_all();
    });
    splitter_with(true, false, |t| {
        t.text("x_y", "a_b y");
        t.default(&[mod_(0, 0, 2, 2)]);
        t.test_all();
    });
    splitter_with(true, false, |t| {
        t.text("a_x_b_", " x_");
        t.default(&[mod_(0, 0, 3, 1)]);
        t.test_default();
    });
    splitter_with(true, false, |t| {
        t.text("a_x_b_", "!x_");
        t.default(&[mod_(0, 0, 3, 1)]);
        t.test_all();
    });
}

#[test]
fn trim() {
    splitter_with(false, true, |t| {
        t.text("_", "     _    ");
        t.default(&[mod_(0, 0, 2, 2)]);
        t.trim(&[]);
        t.test_all();
    });
    splitter_with(false, true, |t| {
        t.text("", "     _    ");
        t.default(&[mod_(0, 0, 1, 2)]);
        t.trim(&[ins(1, 1, 1)]);
        t.ignore(&[]);
        t.test_all();
    });
    splitter_with(false, true, |t| {
        t.text("     _    ", "");
        t.default(&[mod_(0, 0, 2, 1)]);
        t.trim(&[del(1, 1, 1)]);
        t.ignore(&[]);
        t.test_all();
    });
    splitter_with(false, true, |t| {
        t.text("x_y", "z_ ");
        t.default(&[mod_(0, 0, 2, 2)]);
        t.test_all();
    });
    splitter_with(false, true, |t| {
        t.text("z", "z_ ");
        t.default(&[ins(1, 1, 1)]);
        t.ignore(&[]);
        t.test_all();
    });
    splitter_with(false, true, |t| {
        t.text("z_ x", "z_ w");
        t.default(&[mod_(1, 1, 1, 1)]);
        t.test_all();
    });
    splitter_with(false, true, |t| {
        t.text("__z__", "z");
        t.default(&[del(0, 0, 2), del(3, 1, 2)]);
        t.ignore(&[]);
        t.test_all();
    });
}
