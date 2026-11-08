#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::BTreeSet;
use std::path::Path;
use std::sync::atomic::AtomicBool;

use gix::bstr::{BString, ByteSlice};
use gix::dir::walk::Listing;

const DIRECTORIES: usize = 48;
const FILES_PER_DIRECTORY: usize = 3;

fn tree(root: &Path) -> Vec<BString> {
    let mut directories = vec![BString::default()];
    std::fs::write(root.join("top.txt"), b"top\n").expect("write the root file");
    for directory in 0..DIRECTORIES {
        let name = format!("dir-{directory:02}/nested");
        std::fs::create_dir_all(root.join(&name)).expect("create a directory");
        for file in 0..FILES_PER_DIRECTORY {
            std::fs::write(root.join(&name).join(format!("file-{file}.txt")), b"x\n")
                .expect("write a file");
        }
        directories.push(format!("dir-{directory:02}").into());
        directories.push(name.into());
    }
    directories
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
    let metadata = entry.metadata.as_ref().expect("the entry metadata");
    assert!(metadata.is_file());
    assert_eq!(metadata.len(), 2);
}

#[test]
fn a_listing_read_on_the_shared_pool_holds_every_entry_of_every_directory() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let directories = tree(tmp.path());

    let listing = Listing::read(tmp.path(), &directories, false, None);

    assert_complete(&listing);
    assert!((1..=8).contains(&Listing::pool_threads()));
}

#[test]
fn concurrent_listings_each_get_their_own_complete_result() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let directories = tree(tmp.path());

    let listings: Vec<Listing> = std::thread::scope(|scope| {
        let readers: Vec<_> = (0..6)
            .map(|_| scope.spawn(|| Listing::read(tmp.path(), &directories, false, None)))
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
    let mut directories = tree(tmp.path());
    directories.push("missing".into());

    let listing = Listing::read(tmp.path(), &directories, false, None);

    assert!(listing.directory(b"missing".as_bstr()).is_none());
    assert_eq!(listing.len(), 1 + 2 * DIRECTORIES);
}

#[test]
fn an_interrupted_listing_reads_no_directory() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let directories = tree(tmp.path());
    let interrupt = AtomicBool::new(true);

    let listing = Listing::read(tmp.path(), &directories, false, Some(&interrupt));

    assert!(listing.is_empty());
}
