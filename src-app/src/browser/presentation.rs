use std::collections::BTreeMap;
use std::os::fd::OwnedFd;

use paneflow_browser_protocol::{
    BrowserError, BufferLayout, Document, FrameAck, FrameFormat, FrameLedger, FrameMessage,
    MAX_PENDING_FRAMES, POOL_BUFFERS,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PoolLayout {
    pub generation: u64,
    pub width: u32,
    pub height: u32,
    pub format: FrameFormat,
    pub modifier: u64,
    pub buffers: Vec<BufferLayout>,
}

pub trait TextureImporter {
    type Texture;

    fn requires_initialization(&self) -> bool {
        false
    }

    fn import(
        &mut self,
        pool: &PoolLayout,
        fds: Vec<OwnedFd>,
    ) -> Result<Vec<Self::Texture>, String>;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FrameIdentity {
    pub pool_generation: u64,
    pub buffer: u8,
    pub sequence: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FrameTiming {
    pub callback_ns: u64,
    pub ready_ns: u64,
    pub capture_timestamp_us: u64,
    pub capture_counter: Option<u64>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Intake {
    PoolRejected {
        document: Document,
        pool_generation: u64,
    },
    PoolImported {
        document: Document,
        pool_generation: u64,
    },
    Presented(FrameIdentity),
    Ignored(&'static str),
    Fatal(String),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ConsumerStats {
    pub pools_created: u64,
    pub pools_retired: u64,
    pub frames_received: u64,
    pub frames_presented: u64,
    pub frames_ignored: u64,
    pub releases_sent: u64,
    pub failures: u64,
}

struct ImportedPool<T> {
    document: Document,
    ready: bool,
    layout: PoolLayout,
    textures: Vec<T>,
}

pub struct FrameConsumer<T> {
    document: Option<Document>,
    pools: BTreeMap<u64, ImportedPool<T>>,
    ledger: FrameLedger,
    current: Option<(FrameIdentity, FrameTiming)>,
    replaced: Vec<(Document, FrameIdentity)>,
    stats: ConsumerStats,
}

impl<T> Default for FrameConsumer<T> {
    fn default() -> Self {
        Self {
            document: None,
            pools: BTreeMap::new(),
            ledger: FrameLedger::default(),
            current: None,
            replaced: Vec::new(),
            stats: ConsumerStats::default(),
        }
    }
}

impl<T> FrameConsumer<T> {
    pub fn stats(&self) -> ConsumerStats {
        self.stats
    }

    pub fn set_document(&mut self, document: Document) {
        let changed = self
            .document
            .as_ref()
            .is_none_or(|current| current != &document);
        if changed {
            self.ledger.invalidate();
            self.clear_current();
        }
        self.document = Some(document);
    }

    pub fn current(&self) -> Option<(&T, FrameIdentity, FrameTiming)> {
        let (identity, timing) = self.current?;
        let pool = self.pools.get(&identity.pool_generation)?;
        let texture = pool.textures.get(usize::from(identity.buffer))?;
        Some((texture, identity, timing))
    }

    pub fn current_size(&self) -> Option<(u32, u32)> {
        let (identity, _) = self.current?;
        let pool = self.pools.get(&identity.pool_generation)?;
        Some((pool.layout.width, pool.layout.height))
    }

    pub fn active_generation(&self) -> Option<u64> {
        self.ledger.active().map(|(_, pool)| pool)
    }

    pub fn pool_count(&self) -> usize {
        self.pools.len()
    }

    pub fn outstanding(&self) -> usize {
        self.ledger.outstanding()
    }

    fn accepts(&self, document: &Document) -> bool {
        self.document
            .as_ref()
            .is_some_and(|current| current == document)
    }

    fn clear_current(&mut self) {
        if let Some((identity, _)) = self.current.take()
            && let Some(pool) = self.pools.get(&identity.pool_generation)
        {
            self.replaced.push((pool.document.clone(), identity));
        }
    }

    pub fn reserve_pool(
        &mut self,
        document: Document,
        layout: PoolLayout,
        requires_initialization: bool,
    ) -> Result<(), Intake> {
        let pool_generation = layout.generation;
        if !self.accepts(&document) {
            self.stats.frames_ignored += 1;
            return Err(if requires_initialization {
                Intake::PoolRejected {
                    document,
                    pool_generation,
                }
            } else {
                Intake::Ignored("pool for another document generation")
            });
        }
        if self
            .active_generation()
            .is_some_and(|active| pool_generation <= active)
        {
            self.stats.frames_ignored += 1;
            return Err(Intake::Ignored("stale pool generation"));
        }
        if self.pools.len() >= 2 {
            return Err(Intake::Fatal(format!(
                "host announced pool {pool_generation} while two pools are alive"
            )));
        }
        if let Err(error) = self.ledger.resize(document.generation, pool_generation) {
            return Err(match error {
                BrowserError::StaleGeneration => {
                    self.stats.frames_ignored += 1;
                    Intake::Ignored("stale pool generation")
                }
                other => Intake::Fatal(format!("pool {pool_generation} refused: {other:?}")),
            });
        }
        self.pools.insert(
            pool_generation,
            ImportedPool {
                document,
                ready: false,
                layout,
                textures: Vec::new(),
            },
        );
        Ok(())
    }

    pub fn complete_pool(
        &mut self,
        document: &Document,
        pool_generation: u64,
        result: Result<Vec<T>, String>,
        ready: bool,
    ) -> Intake {
        let Some(pool) = self.pools.get(&pool_generation) else {
            return Intake::Ignored("pool import completed after retirement");
        };
        if &pool.document != document || !pool.textures.is_empty() {
            return Intake::Fatal("pool import completion does not identify a pending pool".into());
        }
        if !self.accepts(document) {
            self.pools.remove(&pool_generation);
            self.ledger
                .forget_pool(document.generation, pool_generation);
            return Intake::PoolRejected {
                document: document.clone(),
                pool_generation,
            };
        }
        let textures = match result {
            Ok(textures) if textures.len() == usize::from(POOL_BUFFERS) => textures,
            Ok(textures) => {
                return Intake::Fatal(format!(
                    "importer produced {} textures for {POOL_BUFFERS} buffers",
                    textures.len()
                ));
            }
            Err(error) => {
                self.stats.failures += 1;
                self.ledger.invalidate();
                return Intake::Fatal(error);
            }
        };
        if let Some(pool) = self.pools.get_mut(&pool_generation) {
            pool.textures = textures;
            pool.ready = ready;
        }
        self.stats.pools_created += 1;
        if ready {
            Intake::Ignored("pool imported")
        } else {
            Intake::PoolImported {
                document: document.clone(),
                pool_generation,
            }
        }
    }

    pub fn intake(
        &mut self,
        message: FrameMessage,
        fds: Vec<OwnedFd>,
        importer: &mut impl TextureImporter<Texture = T>,
    ) -> Intake {
        match message {
            FrameMessage::PoolCreated {
                document,
                pool_generation,
                width,
                height,
                format,
                modifier,
                buffers,
            } => {
                let layout = PoolLayout {
                    generation: pool_generation,
                    width,
                    height,
                    format,
                    modifier,
                    buffers,
                };
                if let Err(outcome) = self.reserve_pool(
                    document.clone(),
                    layout.clone(),
                    importer.requires_initialization(),
                ) {
                    return outcome;
                }
                let result = importer.import(&layout, fds);
                self.complete_pool(
                    &document,
                    pool_generation,
                    result,
                    !importer.requires_initialization(),
                )
            }

            FrameMessage::Frame {
                document,
                pool_generation,
                buffer,
                sequence,
                callback_ns,
                ready_ns,
                capture_timestamp_us,
                capture_counter,
                ..
            } => {
                self.stats.frames_received += 1;
                let identity = FrameIdentity {
                    pool_generation,
                    buffer,
                    sequence,
                };
                if !self.accepts(&document) || !self.pools.contains_key(&pool_generation) {
                    self.stats.frames_ignored += 1;
                    self.replaced.push((document.clone(), identity));
                    return Intake::Ignored("frame outside the imported pools");
                }
                if self
                    .pools
                    .get(&pool_generation)
                    .is_some_and(|pool| !pool.ready)
                {
                    return Intake::Fatal("host sent a frame before PoolReady".into());
                }
                match self
                    .ledger
                    .receive(document.generation, pool_generation, buffer, sequence)
                {
                    Ok(()) => (),
                    Err(BrowserError::StaleGeneration) => {
                        self.stats.frames_ignored += 1;
                        self.replaced.push((document.clone(), identity));
                        return Intake::Ignored("frame from a retired generation");
                    }
                    Err(other) => {
                        return Intake::Fatal(format!("frame {sequence} refused: {other:?}"));
                    }
                }
                let budget = (MAX_PENDING_FRAMES + 1) * self.pools.len().max(1);
                if self.ledger.outstanding() > budget {
                    return Intake::Fatal(format!(
                        "host exceeded {MAX_PENDING_FRAMES} pending frames across {} live pools",
                        self.pools.len()
                    ));
                }
                self.clear_current();
                self.current = Some((
                    identity,
                    FrameTiming {
                        callback_ns,
                        ready_ns,
                        capture_timestamp_us,
                        capture_counter,
                    },
                ));
                self.stats.frames_presented += 1;
                Intake::Presented(identity)
            }
            FrameMessage::PoolRetired {
                document,
                pool_generation,
            } => {
                if self
                    .pools
                    .get(&pool_generation)
                    .is_none_or(|pool| pool.document != document)
                {
                    return Intake::Ignored("retirement outside the imported pools");
                }
                if self.pools.remove(&pool_generation).is_some() {
                    self.stats.pools_retired += 1;
                }
                if self
                    .current
                    .is_some_and(|(identity, _)| identity.pool_generation == pool_generation)
                {
                    self.current = None;
                }
                self.ledger
                    .forget_pool(document.generation, pool_generation);
                Intake::Ignored("pool retired")
            }
            FrameMessage::Failed { reason, detail, .. } => {
                self.stats.failures += 1;
                Intake::Fatal(format!("{reason:?}: {detail}"))
            }
        }
    }

    pub fn host_lost(&mut self) {
        self.pools.clear();
        self.current = None;
        self.replaced.clear();
        self.ledger = FrameLedger::default();
    }

    pub fn take_releases(&mut self) -> Vec<FrameAck> {
        let mut acks = Vec::with_capacity(self.replaced.len());
        for (document, identity) in self.replaced.drain(..) {
            let _ = self.ledger.release(
                document.generation,
                identity.pool_generation,
                identity.buffer,
                identity.sequence,
            );
            self.stats.releases_sent += 1;
            acks.push(FrameAck::Release {
                document,
                pool_generation: identity.pool_generation,
                buffer: identity.buffer,
                sequence: identity.sequence,
            });
        }
        acks
    }

    pub fn has_pending_releases(&self) -> bool {
        !self.replaced.is_empty()
    }

    pub fn pool_textures(&self, document: &Document, generation: u64) -> Option<&[T]> {
        self.pools
            .get(&generation)
            .filter(|pool| &pool.document == document)
            .map(|pool| pool.textures.as_slice())
    }

    pub fn initialized(&mut self, document: &Document, generation: u64) -> Result<(), String> {
        let pool = self
            .pools
            .get_mut(&generation)
            .filter(|pool| &pool.document == document)
            .ok_or("completed initialization does not identify an imported pool")?;
        if pool.textures.len() != usize::from(POOL_BUFFERS) {
            return Err("pool initialization completed before import".into());
        }
        if pool.ready {
            return Err("duplicate pool initialization completion".into());
        }
        pool.ready = true;
        Ok(())
    }

    pub fn pool_layout(&self, generation: u64) -> Option<&PoolLayout> {
        self.pools.get(&generation).map(|pool| &pool.layout)
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use paneflow_browser_protocol::{
        BrowserId, FrameFailure, Owner, PlaneLayout, SessionId, WorkspaceId,
    };

    use super::*;

    #[derive(Default)]
    struct RecordingImporter {
        imports: Vec<u64>,
        fail: bool,
        forbid: bool,
        requires_ready: bool,
    }

    impl TextureImporter for RecordingImporter {
        type Texture = String;

        fn requires_initialization(&self) -> bool {
            self.requires_ready
        }

        fn import(&mut self, pool: &PoolLayout, fds: Vec<OwnedFd>) -> Result<Vec<String>, String> {
            assert!(!self.forbid, "import must not run during frame intake");
            assert!(fds.is_empty());
            if self.fail {
                return Err("import refused".to_string());
            }
            self.imports.push(pool.generation);
            Ok((0..POOL_BUFFERS)
                .map(|slot| format!("pool{}-slot{slot}", pool.generation))
                .collect())
        }
    }

    fn document(generation: u64) -> Document {
        Document {
            owner: Owner {
                workspace: WorkspaceId::try_from("ws".to_string()).unwrap(),
                session: SessionId::try_from("s1".to_string()).unwrap(),
            },
            browser: BrowserId::try_from("b1".to_string()).unwrap(),
            generation,
        }
    }

    fn pool(document: &Document, generation: u64) -> FrameMessage {
        FrameMessage::PoolCreated {
            document: document.clone(),
            pool_generation: generation,
            width: 640,
            height: 480,
            format: FrameFormat::Bgra8,
            modifier: 0,
            buffers: (0..POOL_BUFFERS)
                .map(|slot| BufferLayout {
                    slot,
                    planes: vec![PlaneLayout {
                        stride: 2560,
                        offset: 0,
                        size: 2560 * 480,
                    }],
                })
                .collect(),
        }
    }

    fn frame(document: &Document, generation: u64, buffer: u8, sequence: u64) -> FrameMessage {
        FrameMessage::Frame {
            document: document.clone(),
            pool_generation: generation,
            buffer,
            sequence,
            callback_ns: sequence * 1000,
            ready_ns: sequence * 1000 + 500,
            capture_timestamp_us: sequence,
            capture_counter: Some(sequence),
            dirty: None,
        }
    }

    fn pending_layout(generation: u64) -> PoolLayout {
        PoolLayout {
            generation,
            width: 640,
            height: 480,
            format: FrameFormat::Bgra8,
            modifier: 0,
            buffers: Vec::new(),
        }
    }

    #[test]
    fn pending_import_reservations_enforce_the_pool_budget_and_ready_boundary() {
        let mut consumer = FrameConsumer::<String>::default();
        let document = document(1);
        consumer.set_document(document.clone());
        for generation in 1..=2 {
            consumer
                .reserve_pool(document.clone(), pending_layout(generation), true)
                .unwrap();
        }
        assert_eq!(consumer.pool_count(), 2);
        assert!(matches!(
            consumer.reserve_pool(document.clone(), pending_layout(3), true),
            Err(Intake::Fatal(_))
        ));
        assert!(consumer.initialized(&document, 2).is_err());
        let textures = vec![String::new(); usize::from(POOL_BUFFERS)];
        assert!(matches!(
            consumer.complete_pool(&document, 2, Ok(textures), false),
            Intake::PoolImported { .. }
        ));
        consumer.initialized(&document, 2).unwrap();
        assert_eq!(consumer.pool_count(), 2);
    }

    #[test]
    fn navigation_rejects_a_pending_import_even_when_the_driver_failed() {
        let mut consumer = FrameConsumer::<String>::default();
        let old = document(1);
        consumer.set_document(old.clone());
        consumer
            .reserve_pool(old.clone(), pending_layout(1), true)
            .unwrap();
        consumer.set_document(document(2));
        assert_eq!(
            consumer.complete_pool(&old, 1, Err("driver failure".into()), false),
            Intake::PoolRejected {
                document: old,
                pool_generation: 1
            }
        );
        assert_eq!(consumer.pool_count(), 0);
        assert_eq!(consumer.stats().failures, 0);
    }

    #[test]
    fn host_loss_discards_import_completion_without_resurrecting_a_pool() {
        let mut consumer = FrameConsumer::<String>::default();
        let document = document(1);
        consumer.set_document(document.clone());
        consumer
            .reserve_pool(document.clone(), pending_layout(1), true)
            .unwrap();
        consumer.host_lost();
        assert!(matches!(
            consumer.complete_pool(
                &document,
                1,
                Ok(vec![String::new(); usize::from(POOL_BUFFERS)]),
                false
            ),
            Intake::Ignored(_)
        ));
        assert_eq!(consumer.pool_count(), 0);
        assert!(consumer.current().is_none());
    }

    #[test]
    fn imported_pool_refuses_frames_until_its_exact_initialization_completes() {
        let mut importer = RecordingImporter {
            requires_ready: true,
            ..Default::default()
        };
        let mut consumer = FrameConsumer::<String>::default();
        let document = document(1);
        consumer.set_document(document.clone());
        assert_eq!(
            consumer.intake(pool(&document, 1), vec![], &mut importer),
            Intake::PoolImported {
                document: document.clone(),
                pool_generation: 1
            }
        );
        assert!(matches!(
            consumer.intake(frame(&document, 1, 0, 1), vec![], &mut importer),
            Intake::Fatal(_)
        ));
        assert!(consumer.current().is_none());
        let mut other = document.clone();
        other.browser = BrowserId::try_from("b2".to_string()).unwrap();
        assert!(consumer.initialized(&other, 1).is_err());
        assert!(consumer.initialized(&document, 2).is_err());
        consumer.initialized(&document, 1).unwrap();
        assert!(consumer.initialized(&document, 1).is_err());
        assert!(matches!(
            consumer.intake(frame(&document, 1, 0, 1), vec![], &mut importer),
            Intake::Presented(_)
        ));
    }

    #[test]
    fn an_obsolete_native_pool_is_rejected_without_importing_or_submitting_work() {
        let mut importer = RecordingImporter {
            requires_ready: true,
            forbid: true,
            ..Default::default()
        };
        let mut consumer = FrameConsumer::<String>::default();
        let old = document(1);
        consumer.set_document(document(2));
        assert_eq!(
            consumer.intake(pool(&old, 1), vec![], &mut importer),
            Intake::PoolRejected {
                document: old,
                pool_generation: 1
            }
        );
        assert!(importer.imports.is_empty());
        assert_eq!(consumer.pool_count(), 0);
        assert!(!consumer.has_pending_releases());
    }

    #[test]
    fn initializing_pools_count_toward_the_two_pool_limit() {
        let mut importer = RecordingImporter {
            requires_ready: true,
            ..Default::default()
        };
        let mut consumer = FrameConsumer::<String>::default();
        let document = document(1);
        consumer.set_document(document.clone());
        for generation in 1..=2 {
            assert!(matches!(
                consumer.intake(pool(&document, generation), vec![], &mut importer),
                Intake::PoolImported { .. }
            ));
        }
        assert!(matches!(
            consumer.intake(pool(&document, 3), vec![], &mut importer),
            Intake::Fatal(_)
        ));
        assert_eq!(consumer.pool_count(), 2);
        assert_eq!(importer.imports, vec![1, 2]);
        consumer.host_lost();
        assert!(consumer.initialized(&document, 1).is_err());
    }

    #[test]
    fn retirement_requires_the_complete_pool_document() {
        let mut importer = RecordingImporter::default();
        let mut consumer = FrameConsumer::<String>::default();
        let original = document(1);
        consumer.set_document(original.clone());
        consumer.intake(pool(&original, 1), vec![], &mut importer);
        consumer.intake(frame(&original, 1, 0, 1), vec![], &mut importer);
        let mut other = original.clone();
        other.browser = BrowserId::try_from("b2".to_string()).unwrap();
        assert_eq!(
            consumer.intake(
                FrameMessage::PoolRetired {
                    document: other,
                    pool_generation: 1,
                },
                vec![],
                &mut importer
            ),
            Intake::Ignored("retirement outside the imported pools")
        );
        assert_eq!(consumer.pool_count(), 1);
        assert!(consumer.current().is_some());
        assert_eq!(consumer.outstanding(), 1);
    }

    #[test]
    fn replaced_frame_ack_keeps_its_original_owner_and_browser() {
        let mut importer = RecordingImporter::default();
        let mut consumer = FrameConsumer::<String>::default();
        let original = document(1);
        consumer.set_document(original.clone());
        consumer.intake(pool(&original, 1), vec![], &mut importer);
        consumer.intake(frame(&original, 1, 0, 1), vec![], &mut importer);
        let mut replacement = original.clone();
        replacement.owner.session = SessionId::try_from("s2".to_string()).unwrap();
        replacement.browser = BrowserId::try_from("b2".to_string()).unwrap();
        consumer.set_document(replacement);
        assert!(consumer.current().is_none());
        let acks = consumer.take_releases();
        assert_eq!(acks.len(), 1);
        assert!(matches!(&acks[0], FrameAck::Release { document, .. } if document == &original));
    }

    #[test]
    fn a_newer_generation_retires_the_older_one_and_refuses_its_frames() {
        let mut importer = RecordingImporter::default();
        let mut consumer = FrameConsumer::<String>::default();
        let document = document(1);
        consumer.set_document(document.clone());
        assert_eq!(
            consumer.intake(pool(&document, 1), vec![], &mut importer),
            Intake::Ignored("pool imported")
        );
        assert!(matches!(
            consumer.intake(frame(&document, 1, 0, 1), vec![], &mut importer),
            Intake::Presented(_)
        ));
        assert_eq!(
            consumer.intake(pool(&document, 2), vec![], &mut importer),
            Intake::Ignored("pool imported")
        );
        assert_eq!(consumer.active_generation(), Some(2));
        assert_eq!(
            consumer.intake(frame(&document, 1, 1, 2), vec![], &mut importer),
            Intake::Ignored("frame from a retired generation")
        );
        assert!(matches!(
            consumer.intake(frame(&document, 2, 0, 3), vec![], &mut importer),
            Intake::Presented(_)
        ));
        let (texture, identity, _) = consumer.current().unwrap();
        assert_eq!(texture, "pool2-slot0");
        assert_eq!(identity.pool_generation, 2);
        assert_eq!(
            consumer.intake(pool(&document, 1), vec![], &mut importer),
            Intake::Ignored("stale pool generation")
        );
        let acks = consumer.take_releases();
        assert_eq!(
            acks.len(),
            2,
            "the replaced frame and the refused late frame are both released"
        );
        assert!(
            acks.iter()
                .all(|ack| matches!(ack, FrameAck::Release { .. }))
        );
        assert_eq!(consumer.stats().frames_ignored, 2);
    }

    #[test]
    fn a_retired_pool_or_a_lost_host_clears_the_presented_surface() {
        let mut importer = RecordingImporter::default();
        let mut consumer = FrameConsumer::<String>::default();
        let document = document(1);
        consumer.set_document(document.clone());
        consumer.intake(pool(&document, 1), vec![], &mut importer);
        consumer.intake(frame(&document, 1, 0, 1), vec![], &mut importer);
        assert!(consumer.current().is_some());
        consumer.intake(
            FrameMessage::PoolRetired {
                document: document.clone(),
                pool_generation: 1,
            },
            vec![],
            &mut importer,
        );
        assert!(consumer.current().is_none());
        assert_eq!(consumer.pool_count(), 0);
        consumer.intake(pool(&document, 2), vec![], &mut importer);
        consumer.intake(frame(&document, 2, 0, 2), vec![], &mut importer);
        assert!(consumer.current().is_some());
        consumer.host_lost();
        assert!(consumer.current().is_none());
        assert_eq!(consumer.pool_count(), 0);
        assert_eq!(consumer.outstanding(), 0);
        assert!(
            consumer.take_releases().is_empty(),
            "nothing is acknowledged to a dead host"
        );
        let failed = consumer.intake(
            FrameMessage::Failed {
                document: document.clone(),
                reason: FrameFailure::WrongDevice,
                detail: "x".into(),
            },
            vec![],
            &mut importer,
        );
        assert!(matches!(failed, Intake::Fatal(_)));
    }

    #[test]
    fn frame_intake_never_imports_or_waits_and_releases_are_deferred_to_the_caller() {
        let mut importer = RecordingImporter::default();
        let mut consumer = FrameConsumer::<String>::default();
        let document = document(1);
        consumer.set_document(document.clone());
        consumer.intake(pool(&document, 1), vec![], &mut importer);
        importer.forbid = true;
        let started = Instant::now();
        for sequence in 1..=600 {
            let buffer = ((sequence - 1) % 3) as u8;
            let outcome =
                consumer.intake(frame(&document, 1, buffer, sequence), vec![], &mut importer);
            assert!(matches!(outcome, Intake::Presented(_)), "{outcome:?}");
            assert!(consumer.has_pending_releases() || sequence == 1);
            let acks = consumer.take_releases();
            assert_eq!(acks.len(), usize::from(sequence > 1));
        }
        assert!(
            started.elapsed() < Duration::from_millis(200),
            "intake must be bookkeeping only"
        );
        assert_eq!(consumer.stats().frames_presented, 600);
        assert_eq!(consumer.stats().releases_sent, 599);
        assert_eq!(consumer.outstanding(), 1);
    }

    #[test]
    fn a_host_holding_more_than_the_pending_budget_is_fatal() {
        let mut importer = RecordingImporter::default();
        let mut consumer = FrameConsumer::<String>::default();
        let document = document(1);
        consumer.set_document(document.clone());
        consumer.intake(pool(&document, 1), vec![], &mut importer);
        consumer.intake(frame(&document, 1, 0, 1), vec![], &mut importer);
        consumer.intake(frame(&document, 1, 1, 2), vec![], &mut importer);
        consumer.intake(frame(&document, 1, 2, 3), vec![], &mut importer);
        let refused = consumer.intake(frame(&document, 1, 0, 4), vec![], &mut importer);
        assert!(matches!(refused, Intake::Fatal(_)), "{refused:?}");
    }

    #[test]
    fn a_pool_replacement_doubles_the_pending_budget_while_the_old_pool_drains() {
        let mut importer = RecordingImporter::default();
        let mut consumer = FrameConsumer::<String>::default();
        let document = document(1);
        consumer.set_document(document.clone());
        consumer.intake(pool(&document, 1), vec![], &mut importer);
        consumer.intake(frame(&document, 1, 0, 1), vec![], &mut importer);
        consumer.intake(frame(&document, 1, 1, 2), vec![], &mut importer);
        consumer.intake(pool(&document, 2), vec![], &mut importer);
        for (buffer, sequence) in [(0, 3), (1, 4), (2, 5)] {
            let outcome =
                consumer.intake(frame(&document, 2, buffer, sequence), vec![], &mut importer);
            assert!(matches!(outcome, Intake::Presented(_)), "{outcome:?}");
        }
        assert_eq!(consumer.outstanding(), 5);
        assert_eq!(consumer.take_releases().len(), 4);
        assert_eq!(consumer.outstanding(), 1);
    }

    #[test]
    fn a_retire_deadline_reported_by_the_host_is_a_browser_error() {
        let mut importer = RecordingImporter::default();
        let mut consumer = FrameConsumer::<String>::default();
        let document = document(1);
        consumer.set_document(document.clone());
        consumer.intake(pool(&document, 1), vec![], &mut importer);
        let outcome = consumer.intake(
            FrameMessage::Failed {
                document: document.clone(),
                reason: FrameFailure::RetireTimeout,
                detail: "pool 1 still holds buffers [0]".into(),
            },
            vec![],
            &mut importer,
        );
        assert!(
            matches!(&outcome, Intake::Fatal(reason) if reason.contains("RetireTimeout")),
            "{outcome:?}"
        );
        assert_eq!(consumer.stats().failures, 1);
    }

    #[test]
    fn an_import_failure_is_fatal_and_a_document_change_drops_the_current_frame() {
        let mut importer = RecordingImporter {
            fail: true,
            ..Default::default()
        };
        let mut consumer = FrameConsumer::<String>::default();
        let first = document(1);
        consumer.set_document(first.clone());
        assert!(matches!(
            consumer.intake(pool(&first, 1), vec![], &mut importer),
            Intake::Fatal(_)
        ));
        importer.fail = false;
        consumer.intake(pool(&first, 2), vec![], &mut importer);
        consumer.intake(frame(&first, 2, 0, 1), vec![], &mut importer);
        assert!(consumer.current().is_some());
        consumer.set_document(document(2));
        assert!(consumer.current().is_none());
        assert_eq!(consumer.take_releases().len(), 1);
    }
}
