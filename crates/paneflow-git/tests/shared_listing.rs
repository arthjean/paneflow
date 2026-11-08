#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::BTreeSet;
use std::path::Path;
use std::sync::atomic::AtomicBool;

use gix::bstr::{BStr, ByteSlice};
use gix::dir::entry::Kind;
use gix::dir::walk::{ListRequest, Listing};

const DIRECTORIES: usize = 48;
const FILES_PER_DIRECTORY: usize = 3;

struct Tree {
    directories: Vec<String>,
    files: Vec<String>,
}

fn tree(root: &Path) -> Tree {
    let mut directories = vec![String::new()];
    std::fs::write(root.join("top.txt"), b"top\n").expect("write the root file");
    std::fs::write(root.join("untracked.txt"), b"untracked\n").expect("write the root file");
    let mut files = vec!["top.txt".to_owned()];
    for directory in 0..DIRECTORIES {
        let name = format!("dir-{directory:02}/nested");
        std::fs::create_dir_all(root.join(&name)).expect("create a directory");
        for file in 0..FILES_PER_DIRECTORY {
            std::fs::write(root.join(&name).join(format!("file-{file}.txt")), b"x\n")
                .expect("write a file");
            files.push(format!("{name}/file-{file}.txt"));
        }
        directories.push(format!("dir-{directory:02}"));
        directories.push(name);
    }
    Tree { directories, files }
}

fn requests<'a>(tree: &'a Tree, extra: &'a [&'a str]) -> Vec<ListRequest<'a>> {
    tree.directories
        .iter()
        .map(String::as_str)
        .chain(extra.iter().copied())
        .map(|directory| {
            let prefix = if directory.is_empty() {
                String::new()
            } else {
                format!("{directory}/")
            };
            let mut stat_names: Vec<&BStr> = tree
                .files
                .iter()
                .filter_map(|file| file.strip_prefix(prefix.as_str()))
                .filter(|name| !name.contains('/'))
                .map(|name| name.as_bytes().as_bstr())
                .collect();
            stat_names.sort_unstable();
            ListRequest {
                directory: directory.as_bytes().as_bstr(),
                stat_names,
            }
        })
        .collect()
}

fn names(listing: &Listing, directory: &str) -> BTreeSet<String> {
    listing
        .directory(directory.as_bytes().as_bstr())
        .unwrap_or_else(|| panic!("{directory:?} was listed"))
        .entries()
        .iter()
        .map(|entry| entry.file_name.to_string_lossy().into_owned())
        .collect()
}

fn assert_complete(listing: &Listing) {
    assert_eq!(listing.len(), 1 + 2 * DIRECTORIES);
    let root = names(listing, "");
    assert!(
        root.contains("top.txt") && root.contains("dir-00"),
        "{root:?}"
    );
    for directory in 0..DIRECTORIES {
        assert_eq!(
            names(listing, &format!("dir-{directory:02}")),
            BTreeSet::from(["nested".to_owned()])
        );
        assert_eq!(
            names(listing, &format!("dir-{directory:02}/nested")),
            (0..FILES_PER_DIRECTORY)
                .map(|file| format!("file-{file}.txt"))
                .collect::<BTreeSet<_>>()
        );
    }
    let entry = listing
        .directory(b"dir-07/nested".as_bstr())
        .and_then(|directory| directory.get(b"file-1.txt".as_bstr()))
        .expect("a listed file");
    assert_eq!(entry.kind(), Some(Kind::File));
    let metadata = entry
        .metadata()
        .expect("the requested entry has its metadata");
    assert!(metadata.is_file());
    assert_eq!(metadata.len(), 2);
    let nested = listing
        .directory(b"dir-07".as_bstr())
        .and_then(|directory| directory.get(b"nested".as_bstr()))
        .expect("a listed directory");
    assert_eq!(nested.kind(), Some(Kind::Directory));
}

#[test]
fn a_listing_read_on_the_shared_pool_holds_every_entry_of_every_directory() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let tree = tree(tmp.path());

    let listing = Listing::read(tmp.path(), &requests(&tree, &[]), false, None);

    assert_complete(&listing);
    assert!((1..=8).contains(&Listing::pool_threads()));
}

#[test]
fn an_entry_nobody_asked_about_still_has_its_kind() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let tree = tree(tmp.path());

    let listing = Listing::read(tmp.path(), &requests(&tree, &[]), false, None);

    let untracked = listing
        .directory(b"".as_bstr())
        .and_then(|directory| directory.get(b"untracked.txt".as_bstr()))
        .expect("the untracked file is listed");
    assert_eq!(untracked.kind(), Some(Kind::File));
    assert_eq!(
        untracked.metadata().is_some(),
        cfg!(windows),
        "only the Windows enumeration carries the metadata of a name nobody asked about"
    );
}

#[test]
fn concurrent_listings_each_get_their_own_complete_result() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let tree = tree(tmp.path());
    let requests = requests(&tree, &[]);

    let listings: Vec<Listing> = std::thread::scope(|scope| {
        let readers: Vec<_> = (0..6)
            .map(|_| scope.spawn(|| Listing::read(tmp.path(), &requests, false, None)))
            .collect();
        readers
            .into_iter()
            .map(|reader| reader.join().expect("a listing thread"))
            .collect()
    });

    for listing in &listings {
        assert_complete(listing);
    }
}

#[test]
fn a_directory_that_cannot_be_read_is_left_out_for_its_caller_to_read() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let tree = tree(tmp.path());

    let listing = Listing::read(tmp.path(), &requests(&tree, &["missing"]), false, None);

    assert!(listing.directory(b"missing".as_bstr()).is_none());
    assert_eq!(listing.len(), 1 + 2 * DIRECTORIES);
}

#[cfg(unix)]
#[test]
fn a_directory_replaced_by_a_symlink_is_left_out_for_its_caller_to_check() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let tree = tree(tmp.path());
    std::fs::rename(tmp.path().join("dir-03"), tmp.path().join("moved")).expect("move a directory");
    std::os::unix::fs::symlink("moved", tmp.path().join("dir-03")).expect("link the old name");

    let listing = Listing::read(tmp.path(), &requests(&tree, &[]), false, None);

    assert!(listing.directory(b"dir-03".as_bstr()).is_none());
    let link = listing
        .directory(b"".as_bstr())
        .and_then(|directory| directory.get(b"dir-03".as_bstr()))
        .expect("the link is listed in its parent");
    assert_eq!(link.kind(), Some(Kind::Symlink));
}

#[test]
fn an_interrupted_listing_reads_no_directory() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let tree = tree(tmp.path());
    let interrupt = AtomicBool::new(true);

    let listing = Listing::read(tmp.path(), &requests(&tree, &[]), false, Some(&interrupt));

    assert!(listing.is_empty());
}
