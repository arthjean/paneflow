use std::cmp::Ordering;
use std::collections::HashMap;
use std::rc::Rc;

use gix::attrs::StateRef;
use gix::bstr::ByteSlice;
use gix::object::Kind;
use gix::ObjectId;

use crate::{Error, Objects};

pub const DEFAULT_RENAME_LIMIT: usize = 1000;

pub const MAX_SCORE: u32 = 60_000;

pub const DEFAULT_RENAME_SCORE: u32 = 30_000;

const BASENAME_SCORE: u32 = DEFAULT_RENAME_SCORE + (MAX_SCORE - DEFAULT_RENAME_SCORE) / 2;

const CANDIDATES_PER_DESTINATION: usize = 4;

const IDENTICAL_ALTERNATIVES: usize = 100;

const SPAN_HASH_BASE: u32 = 107_927;

const SPAN_BYTES: u64 = 64;

const BINARY_SNIFF_BYTES: usize = 8000;

const OBJECT_TYPE_MASK: u32 = 0o170000;

const REGULAR_FILE_TYPE: u32 = 0o100000;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RenameSide {
    pub path: Vec<u8>,
    pub mode: u32,
    pub id: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rename {
    pub source: usize,
    pub destination: usize,
    pub score: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Role {
    Source,
    Destination,
}

pub(crate) type Spans = Vec<(u32, u64)>;

pub(crate) trait Contents {
    fn identity(&mut self, role: Role, index: usize) -> Result<Option<ObjectId>, Error>;
    fn size(&mut self, role: Role, index: usize) -> Result<Option<u64>, Error>;
    fn spans(&mut self, role: Role, index: usize) -> Result<Option<Rc<Spans>>, Error>;
}

impl Objects<'_> {
    pub fn rename_limit(&self) -> usize {
        self.repo
            .config_snapshot()
            .integer("diff.renameLimit")
            .map_or(DEFAULT_RENAME_LIMIT, |limit| {
                usize::try_from(limit).unwrap_or_default()
            })
    }

    pub fn track_renames(
        &self,
        sources: &[RenameSide],
        destinations: &[RenameSide],
        limit: usize,
    ) -> Result<Vec<Rename>, Error> {
        let mut contents = RepositoryContents::new(self.repo, sources, destinations);
        pair(sources, destinations, limit, &mut contents)
    }
}

pub(crate) fn pair(
    sources: &[RenameSide],
    destinations: &[RenameSide],
    limit: usize,
    contents: &mut impl Contents,
) -> Result<Vec<Rename>, Error> {
    let mut matcher = Matcher {
        sources,
        destinations,
        source_used: vec![false; sources.len()],
        renames: vec![None; destinations.len()],
    };
    if sources.is_empty() || destinations.is_empty() {
        return Ok(Vec::new());
    }
    matcher.exact(contents)?;
    matcher.by_basename(contents)?;
    matcher.by_similarity(limit, contents)?;
    Ok(matcher.renames.into_iter().flatten().collect())
}

struct Matcher<'a> {
    sources: &'a [RenameSide],
    destinations: &'a [RenameSide],
    source_used: Vec<bool>,
    renames: Vec<Option<Rename>>,
}

#[derive(Clone, Copy, Debug)]
struct Candidate {
    source: usize,
    destination: Option<usize>,
    score: u32,
    same_basename: bool,
}

impl Candidate {
    const UNUSED: Self = Self {
        source: 0,
        destination: None,
        score: 0,
        same_basename: false,
    };
}

fn compare_candidates(a: &Candidate, b: &Candidate) -> Ordering {
    match (a.destination, b.destination) {
        (None, None) => Ordering::Equal,
        (None, Some(_)) => Ordering::Greater,
        (Some(_), None) => Ordering::Less,
        (Some(_), Some(_)) if a.score == b.score => b.same_basename.cmp(&a.same_basename),
        (Some(_), Some(_)) => b.score.cmp(&a.score),
    }
}

fn keep_if_better(kept: &mut [Candidate; CANDIDATES_PER_DESTINATION], candidate: Candidate) {
    let mut worst = 0;
    for index in 1..kept.len() {
        if compare_candidates(&kept[index], &kept[worst]) == Ordering::Greater {
            worst = index;
        }
    }
    if compare_candidates(&kept[worst], &candidate) == Ordering::Greater {
        kept[worst] = candidate;
    }
}

impl Matcher<'_> {
    fn record(&mut self, source: usize, destination: usize, score: u32) {
        self.source_used[source] = true;
        self.renames[destination] = Some(Rename {
            source,
            destination,
            score,
        });
    }

    fn remaining_sources(&self) -> Vec<usize> {
        (0..self.sources.len())
            .filter(|source| !self.source_used[*source])
            .collect()
    }

    fn remaining_destinations(&self) -> impl Iterator<Item = usize> + '_ {
        (0..self.destinations.len()).filter(|destination| self.renames[*destination].is_none())
    }

    fn exact(&mut self, contents: &mut impl Contents) -> Result<(), Error> {
        let mut by_id: HashMap<ObjectId, Vec<usize>> = HashMap::new();
        for source in 0..self.sources.len() {
            if let Some(id) = contents.identity(Role::Source, source)? {
                by_id.entry(id).or_default().push(source);
            }
        }
        for destination in 0..self.destinations.len() {
            let Some(id) = contents.identity(Role::Destination, destination)? else {
                continue;
            };
            let Some(candidates) = by_id.get(&id) else {
                continue;
            };
            let target = &self.destinations[destination];
            let mut best = None;
            let mut best_score = 0;
            let mut alternatives = IDENTICAL_ALTERNATIVES;
            for &source in candidates {
                let side = &self.sources[source];
                if (!is_regular(side.mode) || !is_regular(target.mode)) && side.mode != target.mode
                {
                    continue;
                }
                if self.source_used[source] {
                    continue;
                }
                let score = 1 + u8::from(same_basename(&side.path, &target.path));
                if score > best_score {
                    best = Some(source);
                    best_score = score;
                    if score == 2 {
                        break;
                    }
                }
                alternatives -= 1;
                if alternatives == 0 {
                    break;
                }
            }
            if let Some(source) = best {
                self.record(source, destination, MAX_SCORE);
            }
        }
        Ok(())
    }

    fn by_basename(&mut self, contents: &mut impl Contents) -> Result<(), Error> {
        let sources = self.remaining_sources();
        let mut source_by_name: HashMap<&[u8], Option<usize>> = HashMap::new();
        for &source in &sources {
            source_by_name
                .entry(basename(&self.sources[source].path))
                .and_modify(|unique| *unique = None)
                .or_insert(Some(source));
        }
        let mut destination_by_name: HashMap<&[u8], Option<usize>> = HashMap::new();
        for destination in self.remaining_destinations() {
            destination_by_name
                .entry(basename(&self.destinations[destination].path))
                .and_modify(|unique| *unique = None)
                .or_insert(Some(destination));
        }
        for source in sources {
            let name = basename(&self.sources[source].path);
            let Some(destination) = destination_by_name.get(name) else {
                continue;
            };
            let (Some(Some(_)), Some(destination)) = (source_by_name.get(name), *destination)
            else {
                continue;
            };
            if self.renames[destination].is_some() {
                continue;
            }
            let score = similarity(
                &self.sources[source],
                &self.destinations[destination],
                source,
                destination,
                BASENAME_SCORE,
                contents,
            )?;
            if score >= BASENAME_SCORE {
                self.record(source, destination, score);
            }
        }
        Ok(())
    }

    fn by_similarity(&mut self, limit: usize, contents: &mut impl Contents) -> Result<(), Error> {
        let sources = self.remaining_sources();
        let destinations: Vec<usize> = self.remaining_destinations().collect();
        if sources.is_empty() || destinations.is_empty() {
            return Ok(());
        }
        if exceeds_limit(destinations.len(), sources.len(), limit) {
            return Ok(());
        }
        let mut matrix = Vec::with_capacity(destinations.len() * CANDIDATES_PER_DESTINATION);
        for destination in destinations {
            let mut kept = [Candidate::UNUSED; CANDIDATES_PER_DESTINATION];
            for &source in &sources {
                let score = similarity(
                    &self.sources[source],
                    &self.destinations[destination],
                    source,
                    destination,
                    DEFAULT_RENAME_SCORE,
                    contents,
                )?;
                keep_if_better(
                    &mut kept,
                    Candidate {
                        source,
                        destination: Some(destination),
                        score,
                        same_basename: same_basename(
                            &self.sources[source].path,
                            &self.destinations[destination].path,
                        ),
                    },
                );
            }
            matrix.extend(kept);
        }
        matrix.sort_by(compare_candidates);
        for candidate in matrix {
            let Some(destination) = candidate.destination else {
                break;
            };
            if candidate.score < DEFAULT_RENAME_SCORE {
                break;
            }
            if self.renames[destination].is_some() || self.source_used[candidate.source] {
                continue;
            }
            self.record(candidate.source, destination, candidate.score);
        }
        Ok(())
    }
}

fn exceeds_limit(destinations: usize, sources: usize, limit: usize) -> bool {
    limit > 0 && (destinations as u128) * (sources as u128) > (limit as u128) * (limit as u128)
}

fn is_regular(mode: u32) -> bool {
    mode & OBJECT_TYPE_MASK == REGULAR_FILE_TYPE
}

fn basename(path: &[u8]) -> &[u8] {
    path.rsplit(|byte| *byte == b'/').next().unwrap_or(path)
}

fn same_basename(source: &[u8], destination: &[u8]) -> bool {
    basename(source) == basename(destination)
}

fn similarity(
    source_side: &RenameSide,
    destination_side: &RenameSide,
    source: usize,
    destination: usize,
    minimum_score: u32,
    contents: &mut impl Contents,
) -> Result<u32, Error> {
    if !is_regular(source_side.mode) || !is_regular(destination_side.mode) {
        return Ok(0);
    }
    let (Some(source_size), Some(destination_size)) = (
        contents.size(Role::Source, source)?,
        contents.size(Role::Destination, destination)?,
    ) else {
        return Ok(0);
    };
    let max_size = u128::from(source_size.max(destination_size));
    let delta_size = max_size - u128::from(source_size.min(destination_size));
    if max_size * u128::from(MAX_SCORE - minimum_score) < delta_size * u128::from(MAX_SCORE) {
        return Ok(0);
    }
    let Some(source_spans) = contents.spans(Role::Source, source)? else {
        return Ok(0);
    };
    let Some(destination_spans) = contents.spans(Role::Destination, destination)? else {
        return Ok(0);
    };
    if destination_size == 0 {
        return Ok(0);
    }
    let copied = u128::from(copied_bytes(&source_spans, &destination_spans));
    Ok(u32::try_from(copied * u128::from(MAX_SCORE) / max_size).unwrap_or(MAX_SCORE))
}

pub(crate) fn copied_bytes(source: &Spans, destination: &Spans) -> u64 {
    let mut copied = 0;
    let mut remaining = destination.iter().peekable();
    for &(hash, source_count) in source {
        while remaining.next_if(|(other, _)| *other < hash).is_some() {}
        if let Some((_, destination_count)) = remaining.next_if(|(other, _)| *other == hash) {
            copied += source_count.min(*destination_count);
        }
    }
    copied
}

pub(crate) fn span_counts(data: &[u8], is_text: bool) -> Spans {
    let mut counts: HashMap<u32, u64> = HashMap::new();
    let mut accumulated = (0u32, 0u32);
    let mut length = 0u64;
    let mut bytes = data.iter().copied().peekable();
    while let Some(byte) = bytes.next() {
        if is_text && byte == b'\r' && bytes.peek() == Some(&b'\n') {
            continue;
        }
        let (first, second) = accumulated;
        accumulated = (
            ((first << 7) ^ (second >> 25)).wrapping_add(u32::from(byte)),
            (second << 7) ^ (first >> 25),
        );
        length += 1;
        if length < SPAN_BYTES && byte != b'\n' {
            continue;
        }
        *counts.entry(span_hash(accumulated)).or_default() += length;
        length = 0;
        accumulated = (0, 0);
    }
    if length > 0 {
        *counts.entry(span_hash(accumulated)).or_default() += length;
    }
    let mut spans: Spans = counts.into_iter().collect();
    spans.sort_unstable();
    spans
}

fn span_hash((first, second): (u32, u32)) -> u32 {
    first.wrapping_add(second.wrapping_mul(0x61)) % SPAN_HASH_BASE
}

pub(crate) fn sniffs_binary(data: &[u8]) -> bool {
    data[..data.len().min(BINARY_SNIFF_BYTES)].contains(&0)
}

#[derive(Default)]
struct Prepared {
    identity: Option<Option<ObjectId>>,
    size: Option<Option<u64>>,
    spans: Option<Option<Rc<Spans>>>,
    pending: Option<Vec<u8>>,
}

struct RepositoryContents<'a, 'repo> {
    repo: &'repo gix::Repository,
    sides: [&'a [RenameSide]; 2],
    prepared: [Vec<Prepared>; 2],
    worktree: Option<Worktree<'repo>>,
}

struct Worktree<'repo> {
    pipeline: gix::filter::Pipeline<'repo>,
    index: gix::worktree::IndexPersistedOrInMemory,
    attributes: gix::AttributeStack<'repo>,
    diff_attribute: gix::attrs::search::Outcome,
}

fn slot(role: Role) -> usize {
    match role {
        Role::Source => 0,
        Role::Destination => 1,
    }
}

impl<'a, 'repo> RepositoryContents<'a, 'repo> {
    fn new(
        repo: &'repo gix::Repository,
        sources: &'a [RenameSide],
        destinations: &'a [RenameSide],
    ) -> Self {
        Self {
            repo,
            sides: [sources, destinations],
            prepared: [
                sources.iter().map(|_| Prepared::default()).collect(),
                destinations.iter().map(|_| Prepared::default()).collect(),
            ],
            worktree: None,
        }
    }

    fn side(&self, role: Role, index: usize) -> &'a RenameSide {
        &self.sides[slot(role)][index]
    }

    fn worktree(&mut self) -> Result<&mut Worktree<'repo>, Error> {
        if self.worktree.is_none() {
            let (pipeline, index) = self
                .repo
                .filter_pipeline(None)
                .map_err(|error| Error::git("prepare the worktree conversion", error))?;
            let attributes = self
                .repo
                .attributes_only(
                    &index,
                    gix::worktree::stack::state::attributes::Source::WorktreeThenIdMapping,
                )
                .map_err(|error| Error::git("prepare the diff attributes", error))?;
            let diff_attribute = attributes.selected_attribute_matches(["diff"]);
            self.worktree = Some(Worktree {
                pipeline,
                index,
                attributes,
                diff_attribute,
            });
        }
        self.worktree
            .as_mut()
            .ok_or_else(|| Error::git("prepare the worktree conversion", "no worktree"))
    }

    fn read_worktree(&mut self, side: &RenameSide) -> Result<Option<Vec<u8>>, Error> {
        let Some(workdir) = self.repo.workdir() else {
            return Ok(None);
        };
        let Ok(relative) =
            gix::path::from_bstr(side.path.as_bstr()).map(std::borrow::Cow::into_owned)
        else {
            return Ok(None);
        };
        let path = workdir.join(&relative);
        let Ok(metadata) = std::fs::symlink_metadata(&path) else {
            return Ok(None);
        };
        if metadata.file_type().is_symlink() {
            return Ok(std::fs::read_link(&path).ok().and_then(|target| {
                gix::path::os_string_into_bstring(target.into_os_string())
                    .ok()
                    .map(|target| {
                        gix::path::to_unix_separators_on_windows(target)
                            .into_owned()
                            .into()
                    })
            }));
        }
        if !metadata.is_file() {
            return Ok(None);
        }
        let Ok(file) = std::fs::File::open(&path) else {
            return Ok(None);
        };
        let worktree = self.worktree()?;
        let converted = worktree
            .pipeline
            .convert_to_git(file, &relative, &worktree.index)
            .map_err(|error| Error::git("convert a worktree file", error))?;
        let mut data = Vec::new();
        let read = match converted {
            gix::filter::plumbing::pipeline::convert::ToGitOutcome::Unchanged(mut file) => {
                std::io::Read::read_to_end(&mut file, &mut data).map(|_| ())
            }
            gix::filter::plumbing::pipeline::convert::ToGitOutcome::Process(mut stream) => {
                std::io::Read::read_to_end(&mut stream, &mut data).map(|_| ())
            }
            gix::filter::plumbing::pipeline::convert::ToGitOutcome::Buffer(buffer) => {
                data.extend_from_slice(buffer);
                Ok(())
            }
        };
        Ok(read.ok().map(|()| data))
    }

    fn stored(&self, id: ObjectId) -> Result<Option<Vec<u8>>, Error> {
        let Some(object) = self
            .repo
            .try_find_object(id)
            .map_err(|error| Error::git("read an object", error))?
        else {
            return Ok(None);
        };
        Ok((object.kind == Kind::Blob).then(|| object.detach().data))
    }

    fn data(&mut self, role: Role, index: usize) -> Result<Option<Vec<u8>>, Error> {
        if let Some(pending) = self.prepared[slot(role)][index].pending.take() {
            return Ok(Some(pending));
        }
        let side = self.side(role, index);
        if side.id.is_none() {
            return self.read_worktree(side);
        }
        match self.identity(role, index)? {
            Some(id) => self.stored(id),
            None => Ok(None),
        }
    }

    fn is_binary(&mut self, side: &RenameSide, data: &[u8]) -> Result<bool, Error> {
        let config = self.repo.config_snapshot();
        let worktree = self.worktree()?;
        let platform = worktree
            .attributes
            .at_entry(side.path.as_bstr(), None)
            .map_err(|error| Error::git("read the diff attributes", error))?;
        platform.matching_attributes(&mut worktree.diff_attribute);
        let driver_binary = worktree
            .diff_attribute
            .iter_selected()
            .find_map(|matched| match matched.assignment.state {
                StateRef::Unset => Some(Some(true)),
                StateRef::Value(driver) => {
                    Some(config.boolean(format!("diff.{}.binary", driver.as_bstr()).as_str()))
                }
                StateRef::Set | StateRef::Unspecified => None,
            })
            .flatten();
        Ok(driver_binary.unwrap_or_else(|| sniffs_binary(data)))
    }
}

impl Contents for RepositoryContents<'_, '_> {
    fn identity(&mut self, role: Role, index: usize) -> Result<Option<ObjectId>, Error> {
        if let Some(identity) = self.prepared[slot(role)][index].identity {
            return Ok(identity);
        }
        let side = self.side(role, index);
        let identity = match &side.id {
            Some(id) => Some(
                ObjectId::from_hex(id.as_bytes())
                    .map_err(|error| Error::git("parse an object id", error))?,
            ),
            None => match self.read_worktree(side)? {
                Some(data) => {
                    let id = gix::objs::compute_hash(gix::hash::Kind::Sha1, Kind::Blob, &data)
                        .map_err(|error| Error::git("hash a worktree file", error))?;
                    let prepared = &mut self.prepared[slot(role)][index];
                    prepared.size = Some(Some(data.len() as u64));
                    prepared.pending = Some(data);
                    Some(id)
                }
                None => {
                    self.prepared[slot(role)][index].size = Some(None);
                    None
                }
            },
        };
        self.prepared[slot(role)][index].identity = Some(identity);
        Ok(identity)
    }

    fn size(&mut self, role: Role, index: usize) -> Result<Option<u64>, Error> {
        if let Some(size) = self.prepared[slot(role)][index].size {
            return Ok(size);
        }
        let size = match (
            self.side(role, index).id.is_some(),
            self.identity(role, index)?,
        ) {
            (true, Some(id)) => self
                .repo
                .try_find_header(id)
                .map_err(|error| Error::git("read an object header", error))?
                .filter(|header| header.kind() == Kind::Blob)
                .map(|header| header.size()),
            _ => self.prepared[slot(role)][index].size.flatten(),
        };
        self.prepared[slot(role)][index].size = Some(size);
        Ok(size)
    }

    fn spans(&mut self, role: Role, index: usize) -> Result<Option<Rc<Spans>>, Error> {
        if self.prepared[slot(role)][index].spans.is_none() {
            let side = self.side(role, index);
            let spans = match self.data(role, index)? {
                Some(data) => {
                    let is_text = !self.is_binary(side, &data)?;
                    Some(Rc::new(span_counts(&data, is_text)))
                }
                None => None,
            };
            self.prepared[slot(role)][index].spans = Some(spans);
        }
        Ok(self.prepared[slot(role)][index].spans.clone().flatten())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Memory {
        sides: [Vec<Vec<u8>>; 2],
        spans: [Vec<Rc<Spans>>; 2],
    }

    impl Memory {
        fn new(sources: &[&[u8]], destinations: &[&[u8]]) -> Self {
            let sides = [
                sources.iter().map(|data| data.to_vec()).collect::<Vec<_>>(),
                destinations.iter().map(|data| data.to_vec()).collect(),
            ];
            let spans = [
                sides[0]
                    .iter()
                    .map(|data| Rc::new(span_counts(data, !sniffs_binary(data))))
                    .collect(),
                sides[1]
                    .iter()
                    .map(|data| Rc::new(span_counts(data, !sniffs_binary(data))))
                    .collect(),
            ];
            Self { sides, spans }
        }
    }

    impl Contents for Memory {
        fn identity(&mut self, role: Role, index: usize) -> Result<Option<ObjectId>, Error> {
            let data = &self.sides[slot(role)][index];
            Ok(Some(
                gix::objs::compute_hash(gix::hash::Kind::Sha1, Kind::Blob, data)
                    .map_err(|error| Error::git("hash", error))?,
            ))
        }

        fn size(&mut self, role: Role, index: usize) -> Result<Option<u64>, Error> {
            Ok(Some(self.sides[slot(role)][index].len() as u64))
        }

        fn spans(&mut self, role: Role, index: usize) -> Result<Option<Rc<Spans>>, Error> {
            Ok(Some(Rc::clone(&self.spans[slot(role)][index])))
        }
    }

    fn side(path: &str) -> RenameSide {
        RenameSide {
            path: path.as_bytes().to_vec(),
            mode: 0o100644,
            id: None,
        }
    }

    fn lines(count: usize, tag: &str) -> Vec<u8> {
        (0..count)
            .map(|line| format!("{tag} line {line} with enough text to fill a span\n"))
            .collect::<String>()
            .into_bytes()
    }

    #[test]
    fn a_span_ends_at_a_newline_or_after_sixty_four_bytes() {
        let spans = span_counts(&[b'x'; 130], true);
        assert_eq!(spans.iter().map(|(_, count)| count).sum::<u64>(), 130);
        assert_eq!(spans.len(), 2, "two full spans share a hash, then a tail");
        let crlf = span_counts(b"a\r\nb\r\n", true);
        let lf = span_counts(b"a\nb\n", true);
        assert_eq!(crlf, lf, "a text file skips the carriage return of CRLF");
        assert_ne!(span_counts(b"a\r\nb\r\n", false), lf);
    }

    #[test]
    fn the_copied_bytes_are_the_smaller_count_of_each_shared_span() {
        assert_eq!(
            copied_bytes(&vec![(1, 5), (3, 2)], &vec![(1, 3), (2, 9), (3, 4)]),
            5
        );
        assert_eq!(copied_bytes(&vec![(7, 5)], &Vec::new()), 0);
    }

    #[test]
    fn an_exact_match_prefers_an_unused_source_with_the_same_basename() {
        let content: &[u8] = b"same\n";
        let mut memory = Memory::new(&[content, content], &[content]);
        let renames = pair(
            &[side("a/other.txt"), side("b/kept.txt")],
            &[side("c/kept.txt")],
            DEFAULT_RENAME_LIMIT,
            &mut memory,
        )
        .expect("pair");
        assert_eq!(
            renames,
            vec![Rename {
                source: 1,
                destination: 0,
                score: MAX_SCORE
            }]
        );
    }

    #[test]
    fn an_edit_above_half_pairs_and_an_edit_below_half_does_not() {
        let base = lines(20, "kept");
        let mut close = base.clone();
        close.extend(lines(4, "new"));
        let far = lines(20, "other");
        let mut memory = Memory::new(&[&base, &base], &[&close, &far]);
        let renames = pair(
            &[side("old/one.txt"), side("old/two.txt")],
            &[side("new/first.txt"), side("new/second.txt")],
            DEFAULT_RENAME_LIMIT,
            &mut memory,
        )
        .expect("pair");
        assert_eq!(renames.len(), 1, "{renames:?}");
        assert_eq!(renames[0].destination, 0);
        assert!(renames[0].score >= DEFAULT_RENAME_SCORE && renames[0].score < MAX_SCORE);
    }

    #[test]
    fn more_candidates_than_the_limit_keep_only_exact_renames() {
        let first = lines(20, "first");
        let second = lines(20, "second");
        let mut first_edited = first.clone();
        first_edited.extend(lines(2, "new"));
        let mut second_edited = second.clone();
        second_edited.extend(lines(2, "new"));
        let exact = lines(5, "exact");
        let sources = [side("a.txt"), side("c.txt"), side("exact.txt")];
        let destinations = [
            side("b.txt"),
            side("d.txt"),
            side("moved/exact-renamed.txt"),
        ];
        let renamed = |limit| {
            let mut memory = Memory::new(
                &[&first, &second, &exact],
                &[&first_edited, &second_edited, &exact],
            );
            pair(&sources, &destinations, limit, &mut memory).expect("pair")
        };
        let limited = renamed(1);
        assert_eq!(
            limited.len(),
            1,
            "two left on each side exceed a limit of one"
        );
        assert_eq!(limited[0].score, MAX_SCORE);
        assert_eq!(renamed(2).len(), 3, "two by two fits a limit of two");
        assert_eq!(renamed(0).len(), 3, "zero is unlimited");
    }

    #[test]
    fn a_symlink_pairs_only_with_an_identical_symlink() {
        let target: &[u8] = b"target";
        let mut memory = Memory::new(&[target, target], &[target]);
        let link = RenameSide {
            mode: 0o120000,
            ..side("new-link")
        };
        let renames = pair(
            &[
                side("file"),
                RenameSide {
                    mode: 0o120000,
                    ..side("old-link")
                },
            ],
            &[link],
            DEFAULT_RENAME_LIMIT,
            &mut memory,
        )
        .expect("pair");
        assert_eq!(renames.len(), 1);
        assert_eq!(renames[0].source, 1);
    }

    #[test]
    fn the_basename_score_sits_halfway_between_the_default_and_an_exact_match() {
        assert_eq!(BASENAME_SCORE, 45_000);
        assert!(exceeds_limit(1001, 1000, 1000));
        assert!(!exceeds_limit(1000, 1000, 1000));
        assert!(!exceeds_limit(usize::MAX, usize::MAX, 0));
    }
}
