use std::collections::HashMap;

use crate::local_array::tree::*;

use crate::prefix_record::InternalPrefixRecord;
use std::fmt::Debug;

use crate::af::AddressFamily;
use routecore::record::{MergeUpdate, Meta};

pub(crate) type PrefixIterResult<'a, AF, Meta> = Result<
    std::collections::hash_map::Values<
        'a,
        PrefixId<AF>,
        InternalPrefixRecord<AF, Meta>,
    >,
    Box<dyn std::error::Error>,
>;

#[cfg(feature = "dynamodb")]
pub(crate) type PrefixIterMut<'a, AF, Meta> = Result<
    std::slice::IterMut<'a, InternalPrefixRecord<AF, Meta>>,
    Box<dyn std::error::Error>,
>;

pub(crate) type SizedNodeResult<'a, AF> =
    Result<SizedStrideRefMut<'a, AF>, Box<dyn std::error::Error>>;
pub(crate) type SizedNodeRefResult<'a, AF> =
    Result<SizedStrideRefMut<'a, AF>, Box<dyn std::error::Error>>;
pub(crate) type SizedNodeRefOption<'a, AF> = Option<SizedStrideRef<'a, AF>>;

pub(crate) trait StorageBackend {
    type AF: AddressFamily;
    type Meta: Meta + MergeUpdate;

    fn init(start_node: Option<SizedStrideNode<Self::AF>>) -> Self;
    fn acquire_new_node_id(
        &self,
        // sort: <<Self as StorageBackend>::NodeType as SortableNodeId>::Sort,
        //
        level: u8,
    ) -> StrideNodeId;
    // store_node should return an index with the associated type `Part` of the associated type
    // of this trait.
    // `id` is optional, since a vec uses the indexes as the ids of the nodes,
    // other storage data-structures may use unordered lists, where the id is in the
    // record, e.g., dynamodb
    fn store_node(
        &mut self,
        id: StrideNodeId,
        next_node: SizedStrideNode<Self::AF>,
    ) -> Option<StrideNodeId>;
    fn update_node(
        &mut self,
        current_node_id: StrideNodeId,
        updated_node: SizedStrideNode<Self::AF>,
    );
    fn retrieve_node(
        &'_ self,
        index: StrideNodeId,
    ) -> SizedNodeRefOption<'_, Self::AF>;
    fn retrieve_node_mut(
        &mut self,
        index: StrideNodeId,
    ) -> SizedNodeResult<Self::AF>;
    fn retrieve_node_with_guard(
        &self,
        index: StrideNodeId,
    ) -> CacheGuard<Self::AF>;
    fn get_nodes(&self) -> Vec<SizedStrideRef<Self::AF>>;
    fn get_root_node_id(&self, stride_size: u8) -> StrideNodeId;
    // fn get_root_node_mut(
    //     &mut self,
    //     stride_size: u8,
    // ) -> Option<SizedStrideNode<Self::AF, Self::NodeType>>;
    fn get_nodes_len(&self) -> usize;
    // The Node and Prefix ID consist of the same type, that
    // have a `sort` field, that descibes the index of the local array
    // (stored inside each node) and the `part` fiels, that describes
    // the index of the prefix in the global store.
    fn acquire_new_prefix_id(
        &self,
        prefix: &InternalPrefixRecord<Self::AF, Self::Meta>,
        // sort: &<<Self as StorageBackend>::NodeType as SortableNodeId>::Sort,
    ) -> PrefixId<Self::AF>;
    fn store_prefix(
        &mut self,
        id: PrefixId<Self::AF>,
        next_node: InternalPrefixRecord<Self::AF, Self::Meta>,
    ) -> Result<PrefixId<Self::AF>, Box<dyn std::error::Error>>;
    fn retrieve_prefix(
        &self,
        index: PrefixId<Self::AF>,
    ) -> Option<&InternalPrefixRecord<Self::AF, Self::Meta>>;
    fn retrieve_prefix_mut(
        &mut self,
        index: PrefixId<Self::AF>,
    ) -> Option<&mut InternalPrefixRecord<Self::AF, Self::Meta>>;
    fn retrieve_prefix_with_guard(
        &self,
        index: StrideNodeId,
    ) -> PrefixCacheGuard<Self::AF, Self::Meta>;
    fn get_prefixes(
        &self,
    ) -> &HashMap<
        PrefixId<Self::AF>,
        InternalPrefixRecord<Self::AF, Self::Meta>,
    >;
    fn get_prefixes_len(&self) -> usize;
    fn prefixes_iter(&self) -> PrefixIterResult<'_, Self::AF, Self::Meta>;
    #[cfg(feature = "dynamodb")]
    fn prefixes_iter_mut(
        &mut self,
    ) -> PrefixIterMut<'_, Self::AF, Self::Meta>;
}

#[derive(Debug)]
pub(crate) struct InMemStorage<
    AF: AddressFamily,
    Meta: routecore::record::Meta,
> {
    // each stride in its own vec avoids having to store SizedStrideNode, an enum, that will have
    // the size of the largest variant as its memory footprint (Stride8).
    pub nodes3: HashMap<StrideNodeId, TreeBitMapNode<AF, Stride3, 14, 8>>,
    pub nodes4: HashMap<StrideNodeId, TreeBitMapNode<AF, Stride4, 30, 16>>,
    pub nodes5: HashMap<StrideNodeId, TreeBitMapNode<AF, Stride5, 62, 32>>,
    // pub nodes6: Vec<TreeBitMapNode<AF, Stride6, 126, 64>>,
    // pub nodes7: Vec<TreeBitMapNode<AF, Stride7, 254, 128>>,
    // pub nodes8: Vec<TreeBitMapNode<AF, Stride8, 510, 256>>,
    pub prefixes: HashMap<PrefixId<AF>, InternalPrefixRecord<AF, Meta>>,
}

impl<AF: AddressFamily, Meta: routecore::record::Meta + MergeUpdate>
    StorageBackend for InMemStorage<AF, Meta>
{
    type AF = AF;
    type Meta = Meta;

    fn init(
        start_node: Option<SizedStrideNode<Self::AF>>,
    ) -> InMemStorage<AF, Meta> {
        let mut nodes3 = HashMap::new();
        let mut nodes4 = HashMap::new();
        let mut nodes5 = HashMap::new();
        // let mut nodes6 = vec![];
        // let mut nodes7 = vec![];
        // let mut nodes8 = vec![];
        if let Some(n) = start_node {
            match n {
                SizedStrideNode::Stride3(node) => {
                    nodes3.insert(
                        StrideNodeId::new(StrideType::Stride3, 0),
                        node,
                    );
                }
                SizedStrideNode::Stride4(node) => {
                    nodes4.insert(
                        StrideNodeId::new(StrideType::Stride4, 0),
                        node,
                    );
                }
                SizedStrideNode::Stride5(node) => {
                    nodes5.insert(
                        StrideNodeId::new(StrideType::Stride5, 0),
                        node,
                    );
                } // SizedStrideNode::Stride6(nodes) => {
                  //     nodes6 = vec![nodes];
                  // }
                  // SizedStrideNode::Stride7(nodes) => {
                  //     nodes7 = vec![nodes];
                  // }
                  // SizedStrideNode::Stride8(nodes) => {
                  //     nodes8 = vec![nodes];
                  // }
            }
        }

        InMemStorage {
            nodes3,
            nodes4,
            nodes5,
            // nodes6,
            // nodes7,
            // nodes8,
            prefixes: HashMap::new(),
        }
    }

    fn acquire_new_node_id(
        &self,
        // sort: <<Self as StorageBackend>::NodeType as SortableNodeId>::Sort,
        level: u8,
    ) -> StrideNodeId {
        // We're ignoring the part parameter here, because we want to store
        // the index into the global self.nodes vec in the local vec.
        match level {
            3 => StrideNodeId::new(
                StrideType::Stride3,
                self.nodes3.len() as u32,
            ),
            4 => StrideNodeId::new(
                StrideType::Stride4,
                self.nodes4.len() as u32,
            ),

            5 => StrideNodeId::new(
                StrideType::Stride5,
                self.nodes5.len() as u32,
            ),

            // 6 => InMemStrideNodeId::new(
            //     &sort,
            //     &StrideNodeId(StrideType::Stride6, self.nodes6.len() as u32),
            // ),
            // 7 => InMemStrideNodeId::new(
            //     &sort,
            //     &StrideNodeId(StrideType::Stride7, self.nodes7.len() as u32),
            // ),
            // 8 => InMemStrideNodeId::new(
            //     &sort,
            //     &StrideNodeId(StrideType::Stride8, self.nodes8.len() as u32),
            // ),
            _ => panic!("Invalid level"),
        }
    }

    fn store_node(
        &mut self,
        id: StrideNodeId,
        next_node: SizedStrideNode<Self::AF>,
    ) -> Option<StrideNodeId> {
        match next_node {
            SizedStrideNode::Stride3(node) => {
                // let id = self.nodes3.len() as u32;
                self.nodes3.insert(id, node);
                Some(id)
            }
            SizedStrideNode::Stride4(node) => {
                // let id = self.nodes4.len() as u32;
                self.nodes4.insert(id, node);
                Some(id)
            }
            SizedStrideNode::Stride5(node) => {
                // let id = self.nodes5.len() as u32;
                self.nodes5.insert(id, node);
                Some(id)
            }
        }
    }

    fn update_node(
        &mut self,
        current_node_id: StrideNodeId,
        updated_node: SizedStrideNode<Self::AF>,
    ) {
        match updated_node {
            SizedStrideNode::Stride3(node) => {
                let _default_val = self.nodes3.insert(current_node_id, node);
                // std::mem::replace(
                //     self.nodes3.get_mut(&current_node_id).unwrap(),
                //     node,
                // );
            }
            SizedStrideNode::Stride4(node) => {
                let _default_val = self.nodes4.insert(current_node_id, node);
                // std::mem::replace(
                //     self.nodes4.get_mut(&current_node_id).unwrap(),
                //     node,
                // );
            }
            SizedStrideNode::Stride5(node) => {
                let _default_val = self.nodes5.insert(current_node_id, node);
                // std::mem::replace(
                //     self.nodes5.get_mut(&current_node_id).unwrap(),
                //     node,
                // );
            }
        }
    }

    fn retrieve_node(
        &self,
        id: StrideNodeId,
    ) -> SizedNodeRefOption<'_, Self::AF> {
        match id.get_stride_type() {
            StrideType::Stride3 => {
                self.nodes3.get(&id).map(|n| SizedStrideRef::Stride3(n))
            }
            StrideType::Stride4 => {
                self.nodes4.get(&id).map(|n| SizedStrideRef::Stride4(n))
            }
            StrideType::Stride5 => {
                self.nodes5.get(&id).map(|n| SizedStrideRef::Stride5(n))
            }
        }
    }

    fn retrieve_node_mut(
        &'_ mut self,
        id: StrideNodeId,
    ) -> SizedNodeResult<'_, Self::AF> {
        match id.get_stride_type() {
            StrideType::Stride3 => Ok(self
                .nodes3
                .get_mut(&id)
                .map(|n| SizedStrideRefMut::Stride3(n))
                .unwrap_or_else(|| panic!("Node not found"))),
            StrideType::Stride4 => Ok(self
                .nodes4
                .get_mut(&id)
                .map(|n| SizedStrideRefMut::Stride4(n))
                .unwrap_or_else(|| panic!("Node not found"))),
            StrideType::Stride5 => Ok(self
                .nodes5
                .get_mut(&id)
                .map(|n| SizedStrideRefMut::Stride5(n))
                .unwrap_or_else(|| panic!("Node not found"))),
        }
    }

    // Don't use this function, this is just a placeholder and a really
    // inefficient implementation.
    fn retrieve_node_with_guard(
        &self,
        _id: StrideNodeId,
    ) -> CacheGuard<Self::AF> {
        panic!("Not Implemented for InMeMStorage");
    }

    fn get_nodes(&self) -> Vec<SizedStrideRef<Self::AF>> {
        self.nodes3
            .iter()
            .map(|n| SizedStrideRef::Stride3(n.1))
            .chain(self.nodes4.iter().map(|n| SizedStrideRef::Stride4(n.1)))
            .chain(self.nodes5.iter().map(|n| SizedStrideRef::Stride5(n.1)))
            .collect()
    }

    fn get_root_node_id(&self, first_stride_size: u8) -> StrideNodeId {
        let first_stride_type = match first_stride_size {
            3 => StrideType::Stride3,
            4 => StrideType::Stride4,
            5 => StrideType::Stride5,
            _ => panic!("Invalid stride size"),
        };
        StrideNodeId::new(first_stride_type, 0)
    }

    // fn get_root_node_mut(
    //     &mut self,
    //     stride_size: u8,
    // ) -> Option<SizedStrideNode<Self::AF, Self::NodeType>> {
    //     match stride_size {
    //         3 => Some(SizedStrideNode::Stride3(self.nodes3[0])),
    //         4 => Some(SizedStrideNode::Stride4(self.nodes4[0])),
    //         5 => Some(SizedStrideNode::Stride5(self.nodes5[0])),
    //         // 6 => Some(SizedStrideNode::Stride6(self.nodes6[0])),
    //         // 7 => Some(SizedStrideNode::Stride7(self.nodes7[0])),
    //         // 8 => Some(SizedStrideNode::Stride8(self.nodes8[0])),
    //         _ => panic!("invalid stride size"),
    //     }
    // }

    fn get_nodes_len(&self) -> usize {
        self.nodes3.len() + self.nodes4.len() + self.nodes5.len()
        // + self.nodes6.len()
        // + self.nodes7.len()
        // + self.nodes8.len()
    }

    fn acquire_new_prefix_id(
        &self,
        prefix: &InternalPrefixRecord<Self::AF, Self::Meta>,
        // sort: &<<Self as StorageBackend>::NodeType as SortableNodeId>::Sort,
    ) -> PrefixId<AF> {
        // The return value the StrideType doesn't matter here,
        // because we store all prefixes in one huge vec (unlike the nodes,
        // which are stored in separate vec for each stride size).
        // We'll return the index to the end of the vec.
        PrefixId::<AF>::new(prefix.net, prefix.len)
    }

    fn store_prefix(
        &mut self,
        id: PrefixId<Self::AF>,
        next_node: InternalPrefixRecord<Self::AF, Self::Meta>,
    ) -> Result<PrefixId<Self::AF>, Box<dyn std::error::Error>> {
        self.prefixes.insert(id, next_node);
        Ok(id)
    }

    fn retrieve_prefix(
        &self,
        part_id: PrefixId<Self::AF>,
    ) -> Option<&InternalPrefixRecord<Self::AF, Self::Meta>> {
        self.prefixes.get(&part_id)
    }

    fn retrieve_prefix_mut(
        &mut self,
        part_id: PrefixId<Self::AF>,
    ) -> Option<&mut InternalPrefixRecord<Self::AF, Self::Meta>> {
        self.prefixes.get_mut(&part_id)
    }

    fn retrieve_prefix_with_guard(
        &self,
        _index: StrideNodeId,
    ) -> PrefixCacheGuard<Self::AF, Self::Meta> {
        panic!("nOt ImPlEmEnTed for InMemNode");
    }

    fn get_prefixes(
        &self,
    ) -> &HashMap<
        PrefixId<Self::AF>,
        InternalPrefixRecord<Self::AF, Self::Meta>,
    > {
        &self.prefixes
    }

    fn get_prefixes_len(&self) -> usize {
        self.prefixes.len()
    }

    fn prefixes_iter(&self) -> PrefixIterResult<Self::AF, Self::Meta> {
        Ok(self.prefixes.values())
    }

    #[cfg(feature = "dynamodb")]
    fn prefixes_iter_mut(
        &mut self,
    ) -> Result<
        std::slice::IterMut<'_, InternalPrefixRecord<AF, Meta>>,
        Box<dyn std::error::Error>,
    > {
        Ok(self.prefixes.iter_mut())
    }
}

pub(crate) struct CacheGuard<'a, AF: 'static + AddressFamily> {
    pub guard: std::cell::Ref<'a, SizedStrideNode<AF>>,
}

impl<'a, AF: 'static + AddressFamily> std::ops::Deref for CacheGuard<'a, AF> {
    type Target = SizedStrideNode<AF>;

    fn deref(&self) -> &Self::Target {
        &self.guard
    }
}

pub(crate) struct PrefixCacheGuard<
    'a,
    AF: 'static + AddressFamily,
    Meta: routecore::record::Meta,
> {
    pub guard: std::cell::Ref<'a, InternalPrefixRecord<AF, Meta>>,
}

impl<'a, AF: 'static + AddressFamily, Meta: routecore::record::Meta>
    std::ops::Deref for PrefixCacheGuard<'a, AF, Meta>
{
    type Target = InternalPrefixRecord<AF, Meta>;

    fn deref(&self) -> &Self::Target {
        &self.guard
    }
}
