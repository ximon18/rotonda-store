// ----------- THE STORE ----------------------------------------------------
//
// The CustomAllocStore provides in-memory storage for the BitTreeMapNodes
// and for prefixes and their meta-data. The storage for node is on the
// `buckets` field, and the prefixes are stored in, well, the `prefixes`
// field. They are both organised in the same way, as chained hash tables,
// one per (prefix|node)-length. The hashing function (that is detailed
// lower down in this file), basically takes the address part of the
// node|prefix and uses `(node|prefix)-address part % bucket size`
// as its index.
//
// Both the prefixes and the buckets field have one bucket per (prefix|node)
// -length that start out with a fixed-size array. The size of the arrays is
// set in the rotonda_macros/maps.rs file.
//
// For lower (prefix|node)-lengths the number of elements in the array is
// equal to the number of prefixes in that length, so there's exactly one
// element per (prefix|node). For greater lengths there will be collisions,
// in that case the stored (prefix|node) will have a reference to another
// bucket (also of a fixed size), that holds a (prefix|node) that collided
// with the one that was already stored. A (node|prefix) lookup will have to
// go over all (nore|prefix) buckets until it matches the requested (node|
// prefix) or it reaches the end of the chain.
//
// The chained (node|prefixes) are occupied at a first-come, first-serve
// basis, and are not re-ordered on new insertions of (node|prefixes). This
// may change in the future, since it prevents iterators from being ordered.
//
// One of the nice things of having one table per (node|prefix)-length is that
// a search can start directly at the prefix-length table it wishes, and go
// go up and down into other tables if it needs to (e.g., because more- or
// less-specifics were asked for). In contrast if you do a lookup by
// traversing the tree of nodes, we would always have to go through the root-
// node first and then go up the tree to the requested node. The lower nodes
// of the tree (close to the root) would be a formidable bottle-neck then.
//
// The meta-data for a prefix is (also) stored as a linked-list of
// references, where each meta-data object has a reference to its
// predecessor. New meta-data instances are stored atomically without further
// ado, but updates to a piece of meta-data are done by merging the previous
// meta-data with the new meta-data, through use of the `MergeUpdate` trait.
//
// The `retrieve_prefix_*` methods retrieve only the most recent insert
// for a prefix (for now).
//
// Prefix example
//
//         (level 0 arrays)         prefixes  bucket
//                                    /len     size
//         ┌──┐
// len /0  │ 0│                        1        1     ■
//         └──┘                                       │
//         ┌──┬──┐                                    │
// len /1  │00│01│                     2        2     │
//         └──┴──┘                                 perfect
//         ┌──┬──┬──┬──┐                             hash
// len /2  │  │  │  │  │               4        4     │
//         └──┴──┴──┴──┘                              │
//         ┌──┬──┬──┬──┬──┬──┬──┬──┐                  │
// len /3  │  │  │  │  │  │  │  │  │   8        8     ■
//         └──┴──┴──┴──┴──┴──┴──┴──┘
//         ┌──┬──┬──┬──┬──┬──┬──┬──┐                        ┌────────────┐
// len /4  │  │  │  │  │  │  │  │  │   8        16 ◀────────│ collision  │
//         └──┴──┴──┴┬─┴──┴──┴──┴──┘                        └────────────┘
//                   └───┐
//                       │              ┌─collision─────────┐
//                   ┌───▼───┐          │                   │
//                   │       │ ◀────────│ 0x0100 and 0x0101 │
//                   │ 0x010 │          └───────────────────┘
//                   │       │
//                   ├───────┴──────────────┬──┬──┐
//                   │ StoredPrefix 0x0101  │  │  │
//                   └──────────────────────┴─┬┴─┬┘
//                                            │  │
//                       ┌────────────────────┘  └──┐
//            ┌──────────▼──────────┬──┐          ┌─▼┬──┐
//         ┌─▶│ metadata (current)  │  │          │ 0│ 1│ (level 1 array)
//         │  └─────────────────────┴──┘          └──┴──┘
//    merge└─┐                        │             │
//    update │           ┌────────────┘             │
//           │┌──────────▼──────────┬──┐        ┌───▼───┐
//         ┌─▶│ metadata (previous) │  │        │       │
//         │  └─────────────────────┴──┘        │  0x0  │
//    merge└─┐                        │         │       │
//    update │           ┌────────────┘         ├───────┴──────────────┬──┐
//           │┌──────────▼──────────┬──┐        │ StoredPrefix 0x0110  │  │
//            │ metadata (oldest)   │  │        └──────────────────────┴──┘
//            └─────────────────────┴──┘                                 │
//                                                         ┌─────────────┘
//                                              ┌──────────▼──────────────┐
//                                              │ metadata (current)      │
//                                              └─────────────────────────┘
//
use std::{
    fmt::Debug,
    sync::atomic::{AtomicUsize, Ordering},
};

use crossbeam_epoch::{self as epoch, Atomic};

use crossbeam_utils::Backoff;
use log::{debug, log_enabled, trace, warn};

use epoch::{Guard, Owned, Shared};
use std::marker::PhantomData;

use crate::local_array::tree::*;
use crate::local_array::{
    bit_span::BitSpan, store::errors::PrefixStoreError,
};

use crate::prefix_record::InternalPrefixRecord;
use crate::{
    impl_search_level, retrieve_node_mut_with_guard_closure,
    store_node_closure,
};

use super::atomic_types::*;
use crate::AddressFamily;

// ----------- CustomAllocStorage -------------------------------------------
//
// CustomAllocStorage is a storage backend that uses a custom allocator, that
// consitss of arrays that point to other arrays on collision.
#[derive(Debug)]
pub struct CustomAllocStorage<
    AF: AddressFamily,
    Meta: routecore::record::Meta + routecore::record::MergeUpdate,
    NB: NodeBuckets<AF>,
    PB: PrefixBuckets<AF, Meta>,
> {
    pub(crate) buckets: NB,
    pub prefixes: PB,
    pub default_route_prefix_serial: AtomicUsize,
    _m: PhantomData<Meta>,
    _af: PhantomData<AF>,
}

impl<
        'a,
        AF: AddressFamily,
        Meta: routecore::record::Meta,
        NB: NodeBuckets<AF>,
        PB: PrefixBuckets<AF, Meta>,
    > CustomAllocStorage<AF, Meta, NB, PB>
{
    pub(crate) fn init(
        root_node: SizedStrideNode<AF>,
        guard: &'a Guard,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        warn!("initialize storage backend");

        let store = CustomAllocStorage {
            buckets: NodeBuckets::<AF>::init(),
            prefixes: PrefixBuckets::<AF, Meta>::init(),
            // len_to_stride_size,
            default_route_prefix_serial: AtomicUsize::new(0),
            _af: PhantomData,
            _m: PhantomData,
        };

        store.store_node(
            StrideNodeId::dangerously_new_with_id_as_is(AF::zero(), 0),
            root_node,
            guard,
        )?;

        Ok(store)
    }

    pub(crate) fn acquire_new_node_id(
        &self,
        (prefix_net, sub_prefix_len): (AF, u8),
    ) -> StrideNodeId<AF> {
        StrideNodeId::new_with_cleaned_id(prefix_net, sub_prefix_len)
    }

    // Create a new node in the store with payload `next_node`.
    //
    // Next node will be ignored if a node with the same `id` already exists.
    #[allow(clippy::type_complexity)]
    pub(crate) fn store_node(
        &self,
        id: StrideNodeId<AF>,
        next_node: SizedStrideNode<AF>,
        guard: &Guard,
    ) -> Result<StrideNodeId<AF>, Box<dyn std::error::Error>> {
        struct SearchLevel<'s, AF: AddressFamily, S: Stride> {
            f: &'s dyn Fn(
                &SearchLevel<AF, S>,
                &NodeSet<AF, S>,
                TreeBitMapNode<AF, S>,
                u8,
                bool,
            ) -> Result<
                StrideNodeId<AF>,
                Box<dyn std::error::Error>,
            >,
        }

        let back_off = crossbeam_utils::Backoff::new();

        let search_level_3 =
            store_node_closure![Stride3; id; guard; back_off;];
        let search_level_4 =
            store_node_closure![Stride4; id; guard; back_off;];
        let search_level_5 =
            store_node_closure![Stride5; id; guard; back_off;];

        if log_enabled!(log::Level::Debug) {
            warn!(
                "{} insert node {}: {:?}",
                std::thread::current().name().unwrap(),
                id,
                next_node
            );
        }
        match next_node {
            SizedStrideNode::Stride3(new_node) => (search_level_3.f)(
                &search_level_3,
                self.buckets.get_store3(id),
                new_node,
                0,
                false,
            ),
            SizedStrideNode::Stride4(new_node) => (search_level_4.f)(
                &search_level_4,
                self.buckets.get_store4(id),
                new_node,
                0,
                false,
            ),
            SizedStrideNode::Stride5(new_node) => (search_level_5.f)(
                &search_level_5,
                self.buckets.get_store5(id),
                new_node,
                0,
                false,
            ),
        }
    }

    #[allow(clippy::type_complexity)]
    pub(crate) fn retrieve_node_with_guard(
        &'a self,
        id: StrideNodeId<AF>,
        guard: &'a Guard,
    ) -> Option<SizedStrideRef<'a, AF>> {
        struct SearchLevel<'s, AF: AddressFamily, S: Stride> {
            f: &'s dyn for<'a> Fn(
                &SearchLevel<AF, S>,
                &NodeSet<AF, S>,
                u8,
                &'a Guard,
            )
                -> Option<SizedStrideRef<'a, AF>>,
        }

        let search_level_3 = impl_search_level![Stride3; id;];
        let search_level_4 = impl_search_level![Stride4; id;];
        let search_level_5 = impl_search_level![Stride5; id;];

        match self.get_stride_for_id(id) {
            3 => {
                trace!("retrieve node {} from l{}", id, id.get_id().1);
                (search_level_3.f)(
                    &search_level_3,
                    self.buckets.get_store3(id),
                    0,
                    guard,
                )
            }

            4 => {
                trace!("retrieve node {} from l{}", id, id.get_id().1);
                (search_level_4.f)(
                    &search_level_4,
                    self.buckets.get_store4(id),
                    0,
                    guard,
                )
            }
            _ => {
                trace!("retrieve node {} from l{}", id, id.get_id().1);
                (search_level_5.f)(
                    &search_level_5,
                    self.buckets.get_store5(id),
                    0,
                    guard,
                )
            }
        }
    }

    #[allow(clippy::type_complexity)]
    pub(crate) fn retrieve_node_mut_with_guard(
        &'a self,
        id: StrideNodeId<AF>,
        guard: &'a Guard,
    ) -> Option<SizedStrideRefMut<'a, AF>> {
        struct SearchLevel<'s, AF: AddressFamily, S: Stride> {
            f: &'s dyn for<'a> Fn(
                &SearchLevel<AF, S>,
                &NodeSet<AF, S>,
                // [u8; 10],
                u8,
                &'a Guard,
            )
                -> Option<SizedStrideRefMut<'a, AF>>,
        }

        let search_level_3 =
            retrieve_node_mut_with_guard_closure![Stride3; id;];
        let search_level_4 =
            retrieve_node_mut_with_guard_closure![Stride4; id;];
        let search_level_5 =
            retrieve_node_mut_with_guard_closure![Stride5; id;];

        match self.buckets.get_stride_for_id(id) {
            3 => {
                trace!("retrieve node {} from l{}", id, id.get_id().1);
                (search_level_3.f)(
                    &search_level_3,
                    self.buckets.get_store3(id),
                    0,
                    guard,
                )
            }

            4 => {
                trace!("retrieve node {} from l{}", id, id.get_id().1);
                (search_level_4.f)(
                    &search_level_4,
                    self.buckets.get_store4(id),
                    0,
                    guard,
                )
            }
            _ => {
                trace!("retrieve node {} from l{}", id, id.get_id().1);
                (search_level_5.f)(
                    &search_level_5,
                    self.buckets.get_store5(id),
                    0,
                    guard,
                )
            }
        }
    }

    pub(crate) fn get_root_node_id(&self) -> StrideNodeId<AF> {
        StrideNodeId::dangerously_new_with_id_as_is(AF::zero(), 0)
    }

    pub fn get_nodes_len(&self) -> usize {
        0
    }

    // Prefixes related methods

    pub(crate) fn load_default_route_prefix_serial(&self) -> usize {
        self.default_route_prefix_serial.load(Ordering::SeqCst)
    }

    #[allow(dead_code)]
    fn increment_default_route_prefix_serial(&self) -> usize {
        self.default_route_prefix_serial
            .fetch_add(1, Ordering::SeqCst)
    }

    // THE CRITICAL SECTION
    //
    // CREATING OR UPDATING A PREFIX IN THE STORE
    //
    // YES, THE MAGIC HAPPENS HERE!
    //
    // This uses the TAG feature of crossbeam_utils::epoch to ensure that we
    // are not overwriting a prefix meta-data that already has been created
    // or was updated by another thread.
    //
    // Our plan:
    //
    // 1. LOAD
    //    Load the current prefix and meta-data from the store if any.
    // 2. INSERT
    //    If there is no current meta-data, create it.
    // 3. UPDATE
    //    If there is a prefix, meta-data combo, then load it and merge
    //    the existing meta-dat with our meta-data using the `MergeUpdate`
    //    trait (a so-called 'Read-Copy-Update').
    // 4. SUCCESS
    //    See if we can successfully store the updated meta-data in the store.
    // 5. DONE
    //    If Step 4 succeeded we're done!
    // 6. FAILURE - REPEAT
    //    If Step 4 failed we're going to do the whole thing again.

    pub(crate) fn upsert_prefix(
        &self,
        record: InternalPrefixRecord<AF, Meta>,
        guard: &Guard,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let backoff = Backoff::new();

        let (atomic_stored_prefix, level) = self
            .non_recursive_retrieve_prefix_mut_with_guard(
                PrefixId::new(record.net, record.len),
                guard,
            )?;
        let inner_stored_prefix =
            atomic_stored_prefix.0.load(Ordering::SeqCst, guard);

        match inner_stored_prefix.is_null() {
            true => {
                debug!("create new super-aggregated prefix record");
                let new_stored_prefix =
                    StoredPrefix::new::<PB>(record, level);

                match atomic_stored_prefix.0.compare_exchange(
                    Shared::null(),
                    Owned::new(new_stored_prefix).with_tag(1),
                    Ordering::SeqCst,
                    Ordering::SeqCst,
                    guard,
                ) {
                    Ok(spfx) => {
                        debug!("inserted new prefix record {:?}", &spfx);
                        Ok(())
                    }
                    Err(stored_prefix) => {
                        debug!(
                            "prefix can't be inserted as new {:?}",
                            stored_prefix.current
                        );
                        Err(Box::new(PrefixStoreError::PrefixAlreadyExist))
                    }
                }
            }
            false => {
                trace!(
                    "existing super-aggregated prefix record for {}/{}",
                    record.net,
                    record.len
                );
                let super_agg_record =
                    &unsafe { inner_stored_prefix.deref() }
                        .super_agg_record
                        .0;
                let mut inner_agg_record =
                    super_agg_record.load(Ordering::Acquire, guard);

                loop {
                    let prefix_record =
                        unsafe { inner_agg_record.as_ref() }.unwrap();
                    let new_record = Owned::new(InternalPrefixRecord::<
                        AF,
                        Meta,
                    >::new_with_meta(
                        record.net,
                        record.len,
                        prefix_record
                            .meta
                            .clone_merge_update(&record.meta)
                            .unwrap(),
                    ))
                    .into_shared(guard);

                    // CAS the nested Atomic InternalPrefixRecord.
                    match super_agg_record.compare_exchange(
                        inner_agg_record,
                        new_record,
                        Ordering::AcqRel,
                        Ordering::Acquire,
                        guard,
                    ) {
                        Ok(_rec) => {
                            if !inner_agg_record.is_null() {
                                unsafe {
                                    guard.defer_unchecked(move || {
                                        std::sync::atomic::fence(
                                            Ordering::Acquire,
                                        );

                                        std::mem::drop(
                                            inner_agg_record.into_owned(),
                                        )
                                    });
                                }
                            };
                            return Ok(());
                        }
                        Err(next_agg) => {
                            // Do it again
                            // warn!("contention {:?}", next_agg.current);
                            inner_agg_record = next_agg.current;
                            backoff.spin();
                            continue;
                        }
                    }
                }
            }
        }
    }

    #[allow(clippy::type_complexity)]
    fn non_recursive_retrieve_prefix_mut_with_guard(
        &'a self,
        search_prefix_id: PrefixId<AF>,
        guard: &'a Guard,
    ) -> Result<(&'a AtomicStoredPrefix<AF, Meta>, u8), PrefixStoreError>
    {
        let mut prefix_set = self
            .prefixes
            .get_root_prefix_set(search_prefix_id.get_len());
        let mut level: u8 = 0;
        let mut stored_prefix = None;

        loop {
            // HASHING FUNCTION
            let index = Self::hash_prefix_id(search_prefix_id, level);

            trace!("retrieve prefix with guard");

            let prefixes = prefix_set.0.load(Ordering::SeqCst, guard);
            debug!("prefixes at level {}? {:?}", level, !prefixes.is_null());

            let prefix_ref = if !prefixes.is_null() {
                debug!("prefix found.");
                unsafe { &prefixes.deref()[index] }
            } else {
                debug!("no prefix set.");
                return Ok((stored_prefix.unwrap(), level));
            };

            stored_prefix = Some(unsafe { prefix_ref.assume_init_ref() });

            if let Some(StoredPrefix {
                prefix,
                next_bucket,
                ..
            }) = stored_prefix.unwrap().get_stored_prefix_mut(guard)
            {
                if search_prefix_id == *prefix {
                    debug!("found requested prefix {:?}", search_prefix_id);
                    return Ok((stored_prefix.unwrap(), level));
                } else {
                    level += 1;
                    prefix_set = next_bucket;
                    continue;
                }
            }

            // No record at the deepest level, still we're returning a reference to it,
            // so the caller can insert a new record here.
            return Ok((stored_prefix.unwrap(), level));
        }
    }

    #[allow(clippy::type_complexity)]
    pub(crate) fn non_recursive_retrieve_prefix_with_guard(
        &'a self,
        id: PrefixId<AF>,
        guard: &'a Guard,
    ) -> (
        Option<&StoredPrefix<AF, Meta>>,
        Option<(
            PrefixId<AF>,
            u8,
            &'a PrefixSet<AF, Meta>,
            [Option<(&'a PrefixSet<AF, Meta>, usize)>; 26],
            usize,
        )>,
    ) {
        let mut prefix_set = self.prefixes.get_root_prefix_set(id.get_len());
        let mut parents = [None; 26];
        let mut level: u8 = 0;
        let backoff = Backoff::new();

        loop {
            // The index of the prefix in this array (at this len and
            // level) is calculated by performing the hash function
            // over the prefix.

            // HASHING FUNCTION
            let index = Self::hash_prefix_id(id, level);

            let mut prefixes = prefix_set.0.load(Ordering::Acquire, guard);

            if !prefixes.is_null() {
                let prefix_ref = unsafe { &mut prefixes.deref_mut()[index] };
                if let Some(stored_prefix) =
                    unsafe { prefix_ref.assume_init_ref() }
                        .get_stored_prefix(guard)
                {
                    if let Some(pfx_rec) = stored_prefix.get_record(guard) {
                        if id == pfx_rec.get_prefix_id() {
                            trace!("found requested prefix {:?}", id);
                            parents[level as usize] =
                                Some((prefix_set, index));
                            return (
                                Some(stored_prefix),
                                Some((id, level, prefix_set, parents, index)),
                            );
                        };
                        // Advance to the next level.
                        prefix_set = &stored_prefix.next_bucket;
                        level += 1;
                        backoff.spin();
                        continue;
                    }
                }
            }

            trace!("no prefix found for {:?}", id);
            parents[level as usize] = Some((prefix_set, index));
            return (None, Some((id, level, prefix_set, parents, index)));
        }
    }

    #[allow(clippy::type_complexity)]
    pub(crate) fn retrieve_prefix_with_guard(
        &'a self,
        prefix_id: PrefixId<AF>,
        guard: &'a Guard,
    ) -> Option<(&StoredPrefix<AF, Meta>, &'a usize)> {
        struct SearchLevel<'s, AF: AddressFamily, M: routecore::record::Meta> {
            f: &'s dyn for<'a> Fn(
                &SearchLevel<AF, M>,
                &PrefixSet<AF, M>,
                u8,
                &'a Guard,
            ) -> Option<(
                &'a StoredPrefix<AF, M>,
                &'a usize,
            )>,
        }

        let search_level = SearchLevel {
            f: &|search_level: &SearchLevel<AF, Meta>,
                 prefix_set: &PrefixSet<AF, Meta>,
                 mut level: u8,
                 guard: &Guard| {
                // HASHING FUNCTION
                let index = Self::hash_prefix_id(prefix_id, level);

                let prefixes = prefix_set.0.load(Ordering::SeqCst, guard);
                // trace!("nodes {:?}", unsafe { unwrapped_nodes.deref_mut().len() });
                let prefix_ref = unsafe { &prefixes.deref()[index] };
                if let Some(stored_prefix) =
                    unsafe { prefix_ref.assume_init_ref() }
                        .get_stored_prefix(guard)
                {
                    if let Some(pfx_rec) =
                        stored_prefix.super_agg_record.get_record(guard)
                    {
                        if prefix_id
                            == PrefixId::new(pfx_rec.net, pfx_rec.len)
                        {
                            trace!("found requested prefix {:?}", prefix_id);
                            return Some((
                                stored_prefix,
                                &stored_prefix.serial,
                            ));
                        };
                        level += 1;
                        (search_level.f)(
                            search_level,
                            &stored_prefix.next_bucket,
                            level,
                            guard,
                        );
                    };
                }
                None
            },
        };

        (search_level.f)(
            &search_level,
            self.prefixes.get_root_prefix_set(prefix_id.get_len()),
            0,
            guard,
        )
    }

    #[allow(dead_code)]
    fn remove_prefix(&mut self, index: PrefixId<AF>) -> Option<Meta> {
        match index.is_empty() {
            false => self.prefixes.remove(index),
            true => None,
        }
    }

    pub fn get_prefixes_len(&self) -> usize {
        (0..=AF::BITS)
            .map(|pfx_len| -> usize {
                self.prefixes
                    .get_root_prefix_set(pfx_len)
                    .get_len_recursive()
            })
            .sum()
    }

    // Stride related methods

    pub(crate) fn get_stride_for_id(&self, id: StrideNodeId<AF>) -> u8 {
        self.buckets.get_stride_for_id(id)
    }

    pub fn get_stride_sizes(&self) -> &[u8] {
        self.buckets.get_stride_sizes()
    }

    pub(crate) fn get_strides_len() -> u8 {
        NB::get_strides_len()
    }

    pub(crate) fn get_first_stride_size() -> u8 {
        NB::get_first_stride_size()
    }

    // Calculates the id of the node that COULD host a prefix in its
    // ptrbitarr.
    pub(crate) fn get_node_id_for_prefix(
        &self,
        prefix: &PrefixId<AF>,
    ) -> (StrideNodeId<AF>, BitSpan) {
        let mut acc = 0;
        for i in self.get_stride_sizes() {
            acc += *i;
            if acc >= prefix.get_len() {
                let node_len = acc - i;
                return (
                    StrideNodeId::new_with_cleaned_id(
                        prefix.get_net(),
                        node_len,
                    ),
                    // NOT THE HASHING FUNCTION!
                    BitSpan::new(
                        ((prefix.get_net() << node_len)
                            >> (AF::BITS - (prefix.get_len() - node_len)))
                            .dangerously_truncate_to_u32(),
                        prefix.get_len() - node_len,
                    ),
                );
            }
        }
        panic!("prefix length for {:?} is too long", prefix);
    }

    // ------- THE HASHING FUNCTION -----------------------------------------

    // Ok, so hashing is really hard, but we're keeping it simple, and
    // because we're keeping we're having lots of collisions, but we don't
    // care!
    //
    // We're using a part of bitarray representation of the address part of
    // a prefixas the as the hash. Sounds complicated, but isn't.
    // Suppose we have an IPv4 prefix, say 130.24.55.0/24.
    // The address part is 130.24.55.0 or as a bitarray that would be:
    //
    // pos  0    4    8    12   16   20   24   28
    // bit  1000 0010 0001 1000 0011 0111 0000 0000
    //
    // First, we're discarding the bits after the length of the prefix, so
    // we'll have:
    //
    // pos  0    4    8    12   16   20
    // bit  1000 0010 0001 1000 0011 0111
    //
    // Now we're dividing this bitarray into one or more levels. A level can
    // be an arbitrary number of bits between 1 and the length of the prefix,
    // but the number of bits summed over all levels should be exactly the
    // prefix length. So in our case they should add up to 24. A possible
    // division could be: 4, 4, 4, 4, 4, 4. Another one would be: 12, 12. The
    // actual division being used is described in the function
    // `<NB>::get_bits_for_len` in the `rotonda-macros` crate. Each level has
    // its own hash, so for our example prefix this would be:
    //
    // pos   0    4    8    12   16   20
    // level 0              1
    // hash  1000 0010 0001 1000 0011 0111
    //
    // level 1 hash: 1000 0010 0001
    // level 2 hash: 1000 0011 0011
    //
    // The hash is now converted to a usize integer, by shifting it all the
    // way to the right in a u32 and then converting to a usize. Why a usize
    // you ask? Because the hash is used by teh CustomAllocStorage as the
    // index to the array for that specific prefix length and level.
    // So for our example this means that the hash on level 1 is now 0x821
    // (decimal 2081) and the hash on level 2 is 0x833 (decimal 2099).
    // Now, if we only consider the hash on level 1 and that we're going to
    // use that as the index to the array that stores all prefixes, you'll
    // notice very quickly that all prefixes starting with 130.[16..31] will
    // cause a collision: they'll all point to the same array element. These
    // collisions are resolved by creating a linked list from each array
    // element, where each element in the list has an array of its own that
    // uses the hash function with the level incremented.

    pub(crate) fn hash_node_id(id: StrideNodeId<AF>, level: u8) -> usize {
        // Aaaaand, this is all of our hashing function.
        // I'll explain later.
        let last_level = if level > 0 {
            <NB>::len_to_store_bits(id.get_id().1, level - 1)
        } else {
            0
        };
        let this_level = <NB>::len_to_store_bits(id.get_id().1, level);
        trace!("bits division {}", this_level);
        trace!(
            "calculated index ({} << {}) >> {}",
            id.get_id().0,
            last_level,
            ((<AF>::BITS - (this_level - last_level)) % <AF>::BITS) as usize
        );
        // HASHING FUNCTION
        ((id.get_id().0 << last_level)
            >> ((<AF>::BITS - (this_level - last_level)) % <AF>::BITS))
            .dangerously_truncate_to_u32() as usize
    }

    pub(crate) fn hash_prefix_id(id: PrefixId<AF>, level: u8) -> usize {
        // Aaaaand, this is all of our hashing function.
        // I'll explain later.
        let last_level = if level > 0 {
            <PB>::get_bits_for_len(id.get_len(), level - 1)
        } else {
            0
        };
        let this_level = <PB>::get_bits_for_len(id.get_len(), level);
        trace!("bits division {}", this_level);
        trace!(
            "calculated index ({} << {}) >> {}",
            id.get_net(),
            last_level,
            ((<AF>::BITS - (this_level - last_level)) % <AF>::BITS) as usize
        );
        // HASHING FUNCTION
        ((id.get_net() << last_level)
            >> ((<AF>::BITS - (this_level - last_level)) % <AF>::BITS))
            .dangerously_truncate_to_u32() as usize
    }
}
