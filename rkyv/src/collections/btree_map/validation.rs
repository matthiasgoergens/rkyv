//! Validation implementation for BTreeMap.

use super::{
    split_meta, ArchivedBTreeMap, ClassifiedNode, InnerNode, InnerNodeEntry, LeafNode,
    LeafNodeEntry, Node, RawNode, MIN_ENTRIES_PER_INNER_NODE, MIN_ENTRIES_PER_LEAF_NODE,
};
use crate::{Archived, Fallible, rel_ptr::RelPtr, validation::{ArchiveContext, LayoutRaw}};
use bytecheck::{CheckBytes, Error, ErrorBox, StructCheckError};
use ptr_meta::PtrExt;
use core::{alloc::Layout, convert::Infallible, fmt, ptr};

impl<K, V, C> CheckBytes<C> for InnerNodeEntry<K, V>
where
    K: CheckBytes<C>,
    V: CheckBytes<C>,
    C: ArchiveContext + ?Sized,
    C::Error: Error,
{
    type Error = StructCheckError;

    unsafe fn check_bytes<'a>(
        value: *const Self,
        context: &mut C,
    ) -> Result<&'a Self, Self::Error> {
        let child_rel_ptr =
            RelPtr::manual_check_bytes(ptr::addr_of!((*value).ptr), context)
                .map_err(ErrorBox::new)
            .and_then(|child_rel_ptr|
                context.bounds_check_ptr(child_rel_ptr)
                    .map_err(ErrorBox::new)
            )
            .and_then(|child_ptr|
                CheckBytes::check_bytes(child_ptr, context)
                    .map_err(ErrorBox::new)
            )
            .map_err(|e| StructCheckError {
                field_name: "ptr",
                inner: e,
            })?;

        K::check_bytes(ptr::addr_of!((*value).key), context)
            .map_err(|e| StructCheckError {
                field_name: "key",
                inner: ErrorBox::new(e),
            })?;

        Ok(&*value)
    }
}

/// An error that can occur while checking a leaf node entry.
#[derive(Debug)]
pub enum LeafNodeEntryError<K, V> {
    /// An error occurred while checking the entry's key.
    KeyCheckError(K),
    /// An error occurred while checking the entry's value.
    ValueCheckError(V),
}

impl<K: fmt::Display, V: fmt::Display> fmt::Display for LeafNodeEntryError<K, V> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LeafNodeEntryError::KeyCheckError(e) => write!(f, "key check error: {}", e),
            LeafNodeEntryError::ValueCheckError(e) => write!(f, "value check error: {}", e),
        }
    }
}

#[cfg(feature = "std")]
const _: () = {
    use std::error::Error;

    impl<K: Error + 'static, V: Error + 'static> Error for LeafNodeEntryError<K, V> {
        fn source(&self) -> Option<&(dyn Error + 'static)> {
            match self {
                Self::KeyCheckError(e) => Some(e as &dyn Error),
                Self::ValueCheckError(e) => Some(e as &dyn Error),
            }
        }
    }
};

impl<K, V, C> CheckBytes<C> for LeafNodeEntry<K, V>
where
    K: CheckBytes<C>,
    V: CheckBytes<C>,
    C: Fallible + ?Sized,
    C::Error: Error,
{
    type Error = LeafNodeEntryError<K::Error, V::Error>;

    unsafe fn check_bytes<'a>(
        value: *const Self,
        context: &mut C,
    ) -> Result<&'a Self, Self::Error> {
        K::check_bytes(ptr::addr_of!((*value).key), context)
            .map_err(LeafNodeEntryError::KeyCheckError)?;
        V::check_bytes(ptr::addr_of!((*value).value), context)
            .map_err(LeafNodeEntryError::ValueCheckError)?;
        Ok(&*value)
    }
}

/// Errors that can occur while checking an archived B-tree.
#[derive(Debug)]
pub enum ArchivedBTreeMapError<K, V, C> {
    /// An error occurred while checking the bytes of a key
    KeyCheckError(K),
    /// An error occurred while checking the bytes of a value
    ValueCheckError(V),
    /// The number of entries in the inner node is less than the minimum number of entries required
    TooFewInnerNodeEntries(usize),
    /// The number of entries in the leaf node is less than the minimum number of entries
    TooFewLeafNodeEntries(usize),
    /// The child of an inner node had a first key that did not match the inner node's key
    MismatchedInnerChildKey,
    /// The leaf level of the B-tree contained an inner node
    InnerNodeInLeafLevel,
    /// The leaves of the B-tree were not all located at the same depth
    InvalidLeafNodeDepth {
        /// The depth of the first leaf node in the tree
        expected: usize,
        /// The depth of the invalid leaf node
        actual: usize,
    },
    /// A leaf node did not contain entries in sorted order
    UnsortedLeafNodeEntries,
    /// A leaf node is not linked after a node despite being the next leaf node
    UnlinkedLeafNode,
    /// A leaf node with lesser keys is linked after a leaf node with greater keys
    UnsortedLeafNode,
    /// The forward pointer of the last leaf did not have an offset of 0
    LastLeafForwardPointerNotNull,
    /// The number of entries the B-tree claims to have does not match the actual number of entries
    LengthMismatch {
        /// The number of entries the B-tree claims to have
        expected: usize,
        /// The actual number of entries in the B-tree
        actual: usize,
    },
    /// An context error occurred
    ContextError(C),
}

impl<K, V, C> From<Infallible> for ArchivedBTreeMapError<K, V, C> {
    fn from(_: Infallible) -> Self {
        unsafe { core::hint::unreachable_unchecked() }
    }
}

impl<K, V, C> fmt::Display for ArchivedBTreeMapError<K, V, C>
where
    K: fmt::Display,
    V: fmt::Display,
    C: fmt::Display,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::KeyCheckError(e) => write!(f, "key check error: {}", e),
            Self::ValueCheckError(e) => write!(f, "value check error: {}", e),
            Self::TooFewInnerNodeEntries(n) => write!(
                f,
                "too few inner node entries (expected at least {}): {}",
                MIN_ENTRIES_PER_INNER_NODE, n
            ),
            Self::TooFewLeafNodeEntries(n) => write!(
                f,
                "too few leaf node entries (expected at least {}): {}",
                MIN_ENTRIES_PER_LEAF_NODE, n,
            ),
            Self::MismatchedInnerChildKey => write!(f, "mismatched inner child key"),
            Self::InnerNodeInLeafLevel => write!(f, "inner node in leaf level"),
            Self::InvalidLeafNodeDepth { expected, actual } => write!(
                f,
                "expected leaf node depth {} but found leaf node depth {}",
                expected, actual,
            ),
            Self::UnsortedLeafNodeEntries => write!(f, "leaf node contains keys in unsorted order"),
            Self::UnlinkedLeafNode => write!(f, "leaf nodes are not linked in the sorted order"),
            Self::UnsortedLeafNode => write!(f, "leaf nodes are not linked in sorted order"),
            Self::LastLeafForwardPointerNotNull => {
                write!(f, "the forward pointer of the last leaf was not null")
            }
            Self::LengthMismatch { expected, actual } => write!(
                f,
                "expected {} entries but there were actually {} entries",
                expected, actual,
            ),
            Self::ContextError(e) => write!(f, "context error: {}", e),
        }
    }
}

#[cfg(feature = "std")]
const _: () = {
    use std::error::Error;

    impl<K, V, C> Error for ArchivedBTreeMapError<K, V, C>
    where
        K: Error + 'static,
        V: Error + 'static,
        C: Error + 'static,
    {
        fn source(&self) -> Option<&(dyn Error + 'static)> {
            match self {
                Self::KeyCheckError(e) => Some(e as &dyn Error),
                Self::ValueCheckError(e) => Some(e as &dyn Error),
                Self::TooFewInnerNodeEntries(_) => None,
                Self::TooFewLeafNodeEntries(_) => None,
                Self::MismatchedInnerChildKey => None,
                Self::InnerNodeInLeafLevel => None,
                Self::InvalidLeafNodeDepth { .. } => None,
                Self::UnsortedLeafNodeEntries => None,
                Self::UnlinkedLeafNode => None,
                Self::UnsortedLeafNode => None,
                Self::LastLeafForwardPointerNotNull => None,
                Self::LengthMismatch { .. } => None,
                Self::ContextError(e) => Some(e as &dyn Error),
            }
        }
    }
};

impl<K, V, C> CheckBytes<C> for RawNode<K, V>
where
    K: CheckBytes<C>,
    V: CheckBytes<C>,
    C: ArchiveContext + ?Sized,
    C::Error: Error,
{
    type Error = StructCheckError;

    unsafe fn check_bytes<'a>(
        value: *const Self,
        context: &mut C,
    ) -> Result<&'a Self, Self::Error> {
        CheckBytes::check_bytes(ptr::addr_of!((*value).meta), context)
            .map_err(|e| StructCheckError {
                field_name: "meta",
                inner: ErrorBox::new(e),
            })?;
        RelPtr::manual_check_bytes(ptr::addr_of!((*value).ptr), context)
            .map_err(|e| StructCheckError {
                field_name: "ptr",
                inner: ErrorBox::new(e),
            })?;
        Ok(&*value)
    }
}

impl<K, V, T> LayoutRaw for Node<K, V, [T]> {
    fn layout_raw(value: *const Self) -> Layout {
        let (_, len) = value.to_raw_parts();
        let result = Layout::new::<RawNode<K, V>>()
            .extend(Layout::array::<T>(len).unwrap())
            .unwrap()
            .0;
        #[cfg(not(feature = "strict"))]
        {
            result
        }
        #[cfg(feature = "strict")]
        {
            result.pad_to_align()
        }
    }
}

impl<K, V, C> CheckBytes<C> for InnerNode<K, V>
where
    K: CheckBytes<C>,
    V: CheckBytes<C>,
    C: ArchiveContext + ?Sized,
    C::Error: Error,
{
    type Error = ArchivedBTreeMapError<K::Error, V::Error, C::Error>;

    unsafe fn check_bytes<'a>(value: *const Self, context: &mut C) -> Result<&'a Self, Self::Error> {
        // meta and ptr have already been checked by the check_bytes for RawNode
        let meta = from_archived!(*ptr::addr_of!((*value).meta));
        let (is_inner, len) = split_meta(meta);
        debug_assert!(is_inner);

        if len < MIN_ENTRIES_PER_INNER_NODE {
            return Err(ArchivedBTreeMapError::TooFewInnerNodeEntries(len));
        }

        CheckBytes::check_bytes(ptr::addr_of!((*value).tail), context)?;

        Ok(&*value)
    }
}

impl<K, V, C> CheckBytes<C> for LeafNode<K, V>
where
    K: CheckBytes<C>,
    V: CheckBytes<C>,
    C: ArchiveContext + ?Sized,
    C::Error: Error,
{
    type Error = ArchivedBTreeMapError<K::Error, V::Error, C::Error>;

    unsafe fn check_bytes<'a>(value: *const Self, context: &mut C) -> Result<&'a Self, Self::Error> {
        // We already checked meta and verified that this is a leaf node
        let meta = from_archived!(*ptr::addr_of!((*value).meta));
        let (is_inner, len) = split_meta(meta);
        debug_assert!(!is_inner);

        RelPtr::manual_check_bytes(ptr::addr_of!((*value).ptr), context)?;

        if len < MIN_ENTRIES_PER_LEAF_NODE {
            return Err(ArchivedBTreeMapError::TooFewLeafNodeEntries(len));
        }

        CheckBytes::check_bytes(ptr::addr_of!((*value).tail), context)?;

        Ok(&*value)
    }
}

impl<K, V, C> CheckBytes<C> for ArchivedBTreeMap<K, V>
where
    K: CheckBytes<C> + Ord,
    V: CheckBytes<C>,
    C: ArchiveContext + ?Sized,
    C::Error: Error,
{
    type Error = ArchivedBTreeMapError<K::Error, V::Error, C::Error>;

    unsafe fn check_bytes<'a>(
        value: *const Self,
        context: &mut C,
    ) -> Result<&'a Self, Self::Error> {
        #[cfg(all(feature = "alloc", not(feature = "std")))]
        use alloc::collections::VecDeque;
        #[cfg(feature = "std")]
        use std::collections::VecDeque;

        let len = from_archived!(*Archived::<usize>::check_bytes(
            ptr::addr_of!((*value).len),
            context,
        )?) as usize;
        let root_rel_ptr = RelPtr::manual_check_bytes(ptr::addr_of!((*value).root), context)?;

        // Strategy:
        // 1. Walk all the nodes, claim their memory, and check their contents
        // 2. Check that inner nodes meet their invariant
        // - The keys are the first elements of the node in the next layer down
        // 3. Check that leaf nodes meet their invariant
        // - They are all linked together
        //   To do this, make a vector and pass it down the tree to collect the nodes in order from
        //   first to last. Then, go to the first node and walk forward while verifying that you're
        //   at the correct node at each step.
        // - The elements are all in sorted order
        // - There are no items that compare equal to each other

        // Walk all the inner nodes, claim their memory, and check their contents
        let mut nodes = VecDeque::new();
        let root_ptr = context
            .claim_owned_rel_ptr(root_rel_ptr)
            .map_err(ArchivedBTreeMapError::ContextError)?;
        nodes.push_back((Node::check_and_classify(root_ptr, context)?, 0));

        while let Some(&(ClassifiedNode::Inner(node), depth)) = nodes.front() {
            nodes.pop_front();

            let prev_child_ptr = context
                .claim_owned_rel_ptr(&node.ptr)
                .map_err(ArchivedBTreeMapError::ContextError)?;
            let prev_child_node = Node::check_and_classify(prev_child_ptr, context)?;
            // The invariant that this node contains keys less than the first key of this node will
            // be checked when we iterate through the leaf nodes in order and check ordering
            nodes.push_back((prev_child_node, depth + 1));

            for entry in node.tail.iter() {
                let child_ptr = context
                    .claim_owned_rel_ptr(&entry.ptr)
                    .map_err(ArchivedBTreeMapError::ContextError)?;
                let child_node = Node::check_and_classify(child_ptr, context)?;
                let child_key = match child_node {
                    ClassifiedNode::Inner(child_inner) => &child_inner.tail[0].key,
                    ClassifiedNode::Leaf(child_leaf) => &child_leaf.tail[0].key,
                };
                if child_key != &entry.key {
                    return Err(ArchivedBTreeMapError::MismatchedInnerChildKey);
                }
                nodes.push_back((child_node, depth + 1));
            }
        }

        // The remaining nodes must all be leaf nodes
        let mut entry_count = 0;
        for (i, (node, depth)) in nodes.iter().enumerate() {
            match node {
                ClassifiedNode::Inner(_) => {
                    return Err(ArchivedBTreeMapError::InnerNodeInLeafLevel)
                }
                ClassifiedNode::Leaf(leaf) => {
                    // Leaf nodes must all be the same depth
                    let expected_depth = nodes.front().unwrap().1;
                    if *depth != expected_depth {
                        return Err(ArchivedBTreeMapError::InvalidLeafNodeDepth {
                            expected: expected_depth,
                            actual: *depth,
                        });
                    }

                    // They must contain entries in sorted order
                    for (prev, next) in leaf.tail.iter().zip(leaf.tail.iter().skip(1)) {
                        if next.key < prev.key {
                            return Err(ArchivedBTreeMapError::UnsortedLeafNodeEntries);
                        }
                    }

                    // And they must link together in sorted order
                    if i < nodes.len() - 1 {
                        let next_ptr = context
                            .check_rel_ptr(leaf.ptr.base(), leaf.ptr.offset())
                            .map_err(ArchivedBTreeMapError::ContextError)?;
                        let next_node = match nodes[i + 1].0 {
                            ClassifiedNode::Inner(_) => {
                                return Err(ArchivedBTreeMapError::InnerNodeInLeafLevel)
                            }
                            ClassifiedNode::Leaf(leaf) => leaf,
                        };
                        if next_ptr != (next_node as *const LeafNode<K, V>).cast() {
                            return Err(ArchivedBTreeMapError::UnlinkedLeafNode);
                        }
                        if next_node.tail[0].key < leaf.tail[leaf.tail.len() - 1].key {
                            return Err(ArchivedBTreeMapError::UnsortedLeafNode);
                        }
                    } else {
                        // The last node must have a null pointer forward
                        if !leaf.ptr.is_null() {
                            return Err(ArchivedBTreeMapError::LastLeafForwardPointerNotNull);
                        }
                    }

                    // Keep track of the number of entries found
                    entry_count += leaf.tail.len();
                }
            }
        }

        // Make sure that the number of entries matches the length
        if entry_count != len {
            return Err(ArchivedBTreeMapError::LengthMismatch {
                expected: len,
                actual: entry_count,
            });
        }

        Ok(&*value)
    }
}
