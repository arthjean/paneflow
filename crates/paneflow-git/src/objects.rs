use std::path::Path;

use gix::object::Kind;
use gix::ObjectId;

use crate::{Error, Stored};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Header {
    pub is_blob: bool,
    pub size: u64,
}

pub struct Objects<'repo> {
    repo: &'repo gix::Repository,
}

impl<'repo> Objects<'repo> {
    pub(crate) fn new(repo: &'repo gix::Repository) -> Self {
        Self { repo }
    }

    pub fn workdir(&self) -> Option<&Path> {
        self.repo.workdir()
    }

    pub fn header(&self, id: &str) -> Result<Option<Header>, Error> {
        let Some(header) = self
            .repo
            .try_find_header(parse(id)?)
            .map_err(|error| Error::git("read an object header", error))?
        else {
            return Ok(None);
        };
        Ok(Some(Header {
            is_blob: header.kind() == Kind::Blob,
            size: header.size(),
        }))
    }

    pub fn head_entry(&self, path: &[u8]) -> Result<Option<Stored>, Error> {
        let head = self
            .repo
            .head()
            .map_err(|error| Error::git("read HEAD", error))?;
        if head.is_unborn() {
            return Ok(None);
        }
        let tree = self
            .repo
            .head_tree()
            .map_err(|error| Error::git("read the HEAD tree", error))?;
        let entry = tree
            .lookup_entry(path.split(|byte| *byte == b'/'))
            .map_err(|error| Error::git("look up a HEAD tree entry", error))?;
        Ok(entry.map(|entry| Stored {
            mode: u32::from(entry.mode().value()),
            id: entry.object_id().to_string(),
        }))
    }

    pub fn blob(&self, id: &str) -> Result<Option<Vec<u8>>, Error> {
        let Some(object) = self
            .repo
            .try_find_object(parse(id)?)
            .map_err(|error| Error::git("read an object", error))?
        else {
            return Ok(None);
        };
        if object.kind != Kind::Blob {
            return Ok(None);
        }
        Ok(Some(object.detach().data))
    }
}

fn parse(id: &str) -> Result<ObjectId, Error> {
    ObjectId::from_hex(id.as_bytes()).map_err(|error| Error::git("parse an object id", error))
}
