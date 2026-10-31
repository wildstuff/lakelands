//! What the writer keeps of a description at the head (lakelands#1575).
//!
//! Under the lease every append to the lake is this handle's (008), so
//! what changes a description at the head is known in-process: an event
//! that ends a range ([`super::ends_a_range`]: a declaration, a migration,
//! a setting), a rebuild or a scrub, a step of the lake's own choice of an
//! order index, and the lease itself. The writer keeps what it told a
//! principal of the lake or of a type at the head, beside the head's shape
//! and under its lock, and tells it again without a read while none of
//! those came since: a record written in between moves the head but not
//! the description, which names the head it answers at (009). A handle
//! without the lease keeps nothing, since another process appends there
//! and only the engine's head says what it did.
//!
//! A writer that lost its lease without noticing still serves what it
//! kept, at its own head, until it notices: with no heartbeat on the
//! lease, it finds out at its next append, fold or regain, where the memo
//! is dropped. That answer is exact for the position it names, the one
//! [`super::Lake::head`] answers this handle, though another process may
//! have declared since, which a `find` from the same handle sees. This
//! choice is interim until lakelands#1610 settles it.
//!
//! What moves without an event is told at each answer: the pages sorted
//! whole, as this handle tallies them, and a host setting's value in
//! force on the answer's clock. A description where a view's state told
//! whether the view is kept is not kept at all. The key is the whole
//! principal: what it may create is told by its writer, not its grants
//! alone.

use std::any::Any;
use std::sync::Arc;

use lakelands_api::describe::{LakeDescription, SettingValue, SortedWhole, TypeDescription};
use lakelands_api::settings::in_force;
use lakelands_api::{FieldName, Position, Principal, Reply, Timestamp, TypeName, Value};

use super::Lake;

/// The most descriptions kept: a full memo is emptied, not ranked.
const KEPT: usize = 256;

/// A description kept: who was told it, of which type or of the lake, the
/// position it was read at, and it with what it tells at its answer.
type Kept = (Principal, Option<TypeName>, u64, Arc<dyn Any + Send + Sync>);

/// The descriptions kept, and how often they were dropped: a read that
/// began before a drop is not kept.
#[derive(Default)]
pub(super) struct Memo {
    kept: Vec<Kept>,
    drops: u64,
}

impl Memo {
    /// Drop every description kept.
    pub(super) fn drop_all(&mut self) {
        self.kept.clear();
        self.drops += 1;
    }
}

/// A description of the lake or of a type, with its parts told at each
/// answer.
pub(crate) trait Described: Clone + Send + Sync + 'static {
    /// Its types, and its settings.
    fn parts(&mut self) -> (&mut [TypeDescription], &mut [SettingValue]);
}

impl Described for LakeDescription {
    fn parts(&mut self) -> (&mut [TypeDescription], &mut [SettingValue]) {
        (&mut self.types, &mut self.settings)
    }
}

impl Described for TypeDescription {
    fn parts(&mut self) -> (&mut [TypeDescription], &mut [SettingValue]) {
        (std::slice::from_mut(self), &mut [])
    }
}

/// What a description tells at its answer rather than with what it
/// describes.
#[derive(Clone, Debug, Default)]
pub(crate) struct Answered {
    /// Whether a view's state at the read told whether the view is kept:
    /// such a description is not kept.
    pub(crate) read_state: bool,
    /// By type, in the description's order, the fields its reader is told
    /// the pages sorted whole by.
    pub(crate) whole: Vec<Vec<FieldName>>,
    /// By place among the description's settings, a host setting's value
    /// and its periods.
    pub(crate) host: Vec<(usize, Option<Value>, Option<Value>)>,
}

impl Answered {
    /// Tell `d` the pages sorted whole as `lake` tallies them now, and its
    /// host settings' values in force at `now`.
    pub(crate) fn tell<D: Described>(&self, lake: &Lake, d: &mut D, now: Timestamp) {
        let (types, settings) = d.parts();
        let tally = lakelands_core::lock(&lake.sorted_whole);
        for (t, fields) in types.iter_mut().zip(&self.whole) {
            let told = tally
                .iter()
                .filter(|((ty, f), _)| *ty == t.name && fields.contains(f));
            t.lifecycle.sorted_whole = told
                .map(|((_, field), (pages, micros))| SortedWhole {
                    field: field.clone(),
                    pages: *pages,
                    millis: micros / 1000,
                })
                .collect();
        }
        for (i, value, periods) in &self.host {
            if let Some(s) = settings.get_mut(*i) {
                s.value = value.clone().map(|v| in_force(v, periods.as_ref(), now));
            }
        }
    }
}

impl Lake {
    /// The description of `ty`, or of the lake where none, told to `who`
    /// at the head and kept since; else how often the kept ones were
    /// dropped, for [`Self::keep_described`].
    ///
    /// It is told only at a head inside the range it holds for. An append
    /// publishes the head last, so from its fold's commit on a read of the
    /// engine can keep a description of a position the head has not reached:
    /// told at that head, the description would be of a later position
    /// than the one it names (lakelands#1632). Such a read goes to the
    /// engine, whose view gives the position and the shape at once.
    pub(crate) fn described<D: Described>(
        &self,
        who: &Principal,
        ty: Option<&TypeName>,
    ) -> Result<Reply<D>, u64> {
        let head = self.head();
        let held = lakelands_core::lock(&self.shapes);
        // INTERIM: lakelands#1610 — a lease lost unnoticed still serves.
        let ended = held.ended.filter(|_| self.is_writer());
        let fresh =
            |at: u64| ended.is_some_and(|(from, to)| from <= at && (from..=to).contains(&head.0));
        let kept = (held.described.kept.iter())
            .find(|(p, t, at, _)| p == who && t.as_ref() == ty && fresh(*at))
            .map(|(.., told)| told.clone());
        let drops = held.described.drops;
        drop(held);
        let told = kept
            .as_ref()
            .and_then(|t| t.downcast_ref::<(D, Answered)>());
        let Some((data, answered)) = told else {
            return Err(drops);
        };
        let mut data = data.clone();
        answered.tell(self, &mut data, super::wall_clock());
        Ok(Reply {
            position: head,
            data,
        })
    }

    /// Keep `data`, told to `who` of `ty` at the head at `at`, unless a
    /// drop came since the read began (`drops`), the head's shape is not
    /// known to hold for `at`, or a view's state told it.
    pub(crate) fn keep_described<D: Described>(
        &self,
        (who, ty): (&Principal, Option<&TypeName>),
        (drops, at): (u64, Position),
        data: &D,
        answered: Answered,
    ) {
        let mut held = lakelands_core::lock(&self.shapes);
        let ended = held
            .ended
            .is_some_and(|(from, to)| (from..=to).contains(&at.0));
        if answered.read_state || !self.is_writer() || !ended || held.described.drops != drops {
            return;
        }
        let memo = &mut held.described;
        if memo.kept.len() >= KEPT {
            memo.kept.clear();
        }
        memo.kept
            .retain(|(p, t, ..)| !(p == who && t.as_ref() == ty));
        let told = Arc::new((data.clone(), answered));
        memo.kept.push((who.clone(), ty.cloned(), at.0, told));
    }

    /// Drop every description kept: the lake's own choice of an order
    /// index, told in one, moved.
    pub(crate) fn undescribe(&self) {
        lakelands_core::lock(&self.shapes).described.drop_all();
    }
}
