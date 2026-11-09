#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod support;

use std::collections::BTreeSet;
use std::path::Path;

use paneflow_git::renames::{RenameSide, MAX_SCORE};
use paneflow_git::{with_snapshot, Options};

use support::{commit_all, git, init, write};

type Pair = (String, String, u32);

fn staged_sides(root: &Path) -> (Vec<RenameSide>, Vec<RenameSide>) {
    let raw = git(
        root,
        &[
            "diff",
            "--cached",
            "--no-renames",
            "--raw",
            "--no-abbrev",
            "-z",
            "HEAD",
        ],
    );
    let mut fields = raw.split('\0').filter(|field| !field.is_empty());
    let mut sources = Vec::new();
    let mut destinations = Vec::new();
    while let (Some(header), Some(path)) = (fields.next(), fields.next()) {
        let parts: Vec<&str> = header.trim_start_matches(':').split(' ').collect();
        let side = |mode: &str, id: &str| RenameSide {
            path: path.as_bytes().to_vec(),
            mode: u32::from_str_radix(mode, 8).expect("octal mode"),
            id: Some(id.to_string()),
        };
        match parts[4] {
            "D" => sources.push(side(parts[0], parts[2])),
            "A" => destinations.push(side(parts[1], parts[3])),
            _ => {}
        }
    }
    (sources, destinations)
}

fn cli_pairs(root: &Path, config: &[&str]) -> BTreeSet<Pair> {
    let mut args: Vec<&str> = config.iter().flat_map(|entry| ["-c", entry]).collect();
    args.extend(["diff", "--cached", "-M", "--name-status", "-z", "HEAD"]);
    let out = git(root, &args);
    let mut fields = out.split('\0').filter(|field| !field.is_empty());
    let mut pairs = BTreeSet::new();
    while let Some(status) = fields.next() {
        if let Some(score) = status.strip_prefix('R') {
            let source = fields.next().expect("rename source").to_string();
            let destination = fields.next().expect("rename destination").to_string();
            pairs.insert((source, destination, score.parse().expect("score")));
        } else {
            fields.next();
        }
    }
    pairs
}

fn gix_pairs(root: &Path) -> BTreeSet<Pair> {
    let (sources, destinations) = staged_sides(root);
    gix_pairs_of(root, &sources, &destinations)
}

fn gix_pairs_of(
    root: &Path,
    sources: &[RenameSide],
    destinations: &[RenameSide],
) -> BTreeSet<Pair> {
    with_snapshot(root, &Options::default(), |_, objects| {
        let renames = objects.track_renames(sources, destinations, objects.rename_limit())?;
        Ok(renames
            .into_iter()
            .map(|rename| {
                (
                    String::from_utf8(sources[rename.source].path.clone()).expect("utf-8"),
                    String::from_utf8(destinations[rename.destination].path.clone())
                        .expect("utf-8"),
                    rename.score * 100 / MAX_SCORE,
                )
            })
            .collect())
    })
    .expect("track renames")
}

fn numbered(case: &str, count: usize) -> String {
    (0..count)
        .map(|line| format!("{case} line {line:03} carries some ordinary text\n"))
        .collect()
}

fn edited(case: &str, count: usize, kept: usize) -> String {
    let mut text: String = numbered(case, count)
        .lines()
        .take(kept)
        .map(|line| format!("{line}\n"))
        .collect();
    text.push_str(&numbered(&format!("{case} replaced"), count - kept));
    text
}

#[test]
fn rename_scores_match_git_across_the_similarity_threshold() {
    support::isolate_process();
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path().join("repo");
    init(&root);
    write(&root, ".gitattributes", b"*.nodiff -diff\n");
    let mut moves = Vec::new();
    for kept in 14..=26 {
        let case = format!("sweep{kept}");
        write(
            &root,
            &format!("old/{case}.txt"),
            numbered(&case, 40).as_bytes(),
        );
        moves.push((
            format!("old/{case}.txt"),
            format!("new/{case}-moved.txt"),
            edited(&case, 40, kept),
        ));
    }
    for kept in 26..=34 {
        let case = format!("basename{kept}");
        write(
            &root,
            &format!("from/{case}.txt"),
            numbered(&case, 40).as_bytes(),
        );
        moves.push((
            format!("from/{case}.txt"),
            format!("to/{case}.txt"),
            edited(&case, 40, kept),
        ));
    }
    let crlf_source = numbered("crlf", 30);
    write(&root, "old/crlf.txt", crlf_source.as_bytes());
    moves.push((
        "old/crlf.txt".to_string(),
        "new/crlf-moved.txt".to_string(),
        crlf_source.replace('\n', "\r\n"),
    ));
    let binary_source = format!("\0{}", numbered("binary", 30));
    write(&root, "old/binary.dat", binary_source.as_bytes());
    moves.push((
        "old/binary.dat".to_string(),
        "new/binary-moved.dat".to_string(),
        binary_source.replace('\n', "\r\n"),
    ));
    let attribute_source = numbered("attribute", 30);
    write(&root, "old/forced.nodiff", attribute_source.as_bytes());
    moves.push((
        "old/forced.nodiff".to_string(),
        "new/forced-moved.nodiff".to_string(),
        attribute_source.replace('\n', "\r\n"),
    ));
    let duplicate = numbered("duplicate", 10);
    for name in ["a/copy.txt", "b/other.txt"] {
        write(&root, name, duplicate.as_bytes());
    }
    moves.push((
        "a/copy.txt".to_string(),
        "c/copy.txt".to_string(),
        duplicate.clone(),
    ));
    moves.push((
        "b/other.txt".to_string(),
        "d/elsewhere.txt".to_string(),
        duplicate,
    ));
    write(&root, "old/empty.txt", b"");
    moves.push((
        "old/empty.txt".to_string(),
        "new/empty-moved.txt".to_string(),
        String::new(),
    ));
    write(
        &root,
        "gone/only-deleted.txt",
        numbered("deleted", 12).as_bytes(),
    );
    commit_all(&root, "base");
    for (from, to, content) in &moves {
        std::fs::remove_file(root.join(from)).expect("remove source");
        write(&root, to, content.as_bytes());
    }
    std::fs::remove_file(root.join("gone/only-deleted.txt")).expect("delete");
    write(
        &root,
        "fresh/unrelated.txt",
        numbered("unrelated", 12).as_bytes(),
    );
    git(&root, &["add", "-A"]);

    let cli = cli_pairs(&root, &[]);
    let gix = gix_pairs(&root);
    assert!(
        cli.iter().any(|(_, _, score)| *score < 100) && cli.len() > 10,
        "the sweep produces inexact renames: {cli:?}"
    );
    assert!(
        cli.len() < moves.len(),
        "some edits fall below the threshold: {cli:?}"
    );
    assert_eq!(
        gix,
        cli,
        "only in gix: {:?}\nonly in the CLI: {:?}",
        gix.difference(&cli).collect::<Vec<_>>(),
        cli.difference(&gix).collect::<Vec<_>>()
    );
}

#[test]
fn renames_past_the_configured_limit_keep_only_exact_matches_like_git() {
    support::isolate_process();
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path().join("repo");
    init(&root);
    for index in 0..3 {
        write(
            &root,
            &format!("old/edited{index}.txt"),
            numbered(&format!("edited{index}"), 20).as_bytes(),
        );
    }
    write(&root, "old/exact.txt", numbered("exact", 20).as_bytes());
    commit_all(&root, "base");
    for index in 0..3 {
        std::fs::remove_file(root.join(format!("old/edited{index}.txt"))).expect("remove");
        write(
            &root,
            &format!("new/moved{index}.txt"),
            edited(&format!("edited{index}"), 20, 18).as_bytes(),
        );
    }
    std::fs::rename(root.join("old/exact.txt"), root.join("new/exact-moved.txt")).expect("move");
    git(&root, &["add", "-A"]);

    let unlimited = cli_pairs(&root, &[]);
    assert_eq!(unlimited.len(), 4, "{unlimited:?}");
    assert_eq!(gix_pairs(&root), unlimited);

    git(&root, &["config", "diff.renameLimit", "2"]);
    let limited = cli_pairs(&root, &[]);
    assert_eq!(
        limited,
        BTreeSet::from([(
            "old/exact.txt".to_string(),
            "new/exact-moved.txt".to_string(),
            100
        )]),
        "git skips inexact renames past the limit"
    );
    assert_eq!(gix_pairs(&root), limited);

    git(&root, &["config", "diff.renameLimit", "3"]);
    assert_eq!(cli_pairs(&root, &[]), unlimited);
    assert_eq!(gix_pairs(&root), unlimited);
}

#[test]
fn an_exact_rename_read_from_the_worktree_pairs_past_the_limit_in_a_sha256_repository() {
    support::isolate_process();
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path().join("repo");
    support::with_object_format(support::SHA256, || init(&root));
    write(&root, "old/exact.txt", numbered("exact", 20).as_bytes());
    for index in 0..2 {
        let case = format!("edited{index}");
        write(
            &root,
            &format!("old/{case}.txt"),
            numbered(&case, 20).as_bytes(),
        );
    }
    commit_all(&root, "base");
    std::fs::rename(root.join("old/exact.txt"), root.join("new-exact.txt")).expect("move");
    for index in 0..2 {
        let case = format!("edited{index}");
        std::fs::remove_file(root.join(format!("old/{case}.txt"))).expect("remove");
        write(
            &root,
            &format!("new-{case}.txt"),
            edited(&case, 20, 18).as_bytes(),
        );
    }
    git(&root, &["add", "-A"]);
    git(&root, &["config", "diff.renameLimit", "1"]);
    let (sources, mut destinations) = staged_sides(&root);
    assert_eq!(sources[0].id.as_ref().map(String::len), Some(64));
    for destination in &mut destinations {
        destination.id = None;
    }

    assert_eq!(
        gix_pairs_of(&root, &sources, &destinations),
        BTreeSet::from([(
            "old/exact.txt".to_string(),
            "new-exact.txt".to_string(),
            100
        )]),
        "a worktree side hashes with the repository's object format"
    );
}
