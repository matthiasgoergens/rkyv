//! Validation implementations and helper types.

pub mod validators;
pub mod owned;

use crate::{Archive, ArchivePointee, Fallible, RelPtr};
use bytecheck::CheckBytes;
use core::{
    alloc::Layout,
    any::TypeId,
    fmt,
    ops::Range,
};
use ptr_meta::{Pointee, PtrExt};
#[cfg(feature = "std")]
use std::error::Error;

// Replace this trait with core::mem::{align_of_val_raw, size_of_val_raw} when they get stabilized.

/// Gets the layout of a type from its pointer.
pub trait LayoutRaw {
    /// Gets the layout of the type.
    fn layout_raw(value: *const Self) -> Layout;
}

impl<T> LayoutRaw for T {
    #[inline]
    fn layout_raw(_: *const Self) -> Layout {
        Layout::new::<T>()
    }
}

impl<T> LayoutRaw for [T] {
    #[inline]
    fn layout_raw(value: *const Self) -> Layout {
        let (_, metadata) = PtrExt::to_raw_parts(value);
        Layout::array::<T>(metadata).unwrap()
    }
}

impl LayoutRaw for str {
    #[inline]
    fn layout_raw(value: *const Self) -> Layout {
        let (_, metadata) = PtrExt::to_raw_parts(value);
        Layout::array::<u8>(metadata).unwrap()
    }
}

/// A prefix range for [`ArchiveContext`].
///
/// Ranges must be popped in the reverse order they are pushed.
pub struct ArchivePrefixRange {
    range: Range<*const u8>,
    depth: usize,
}

/// A suffix range for [`ArchiveContext`].
///
/// Ranges must be popped in the reverse order they are pushed.
pub struct ArchiveSuffixRange {
    start: *const u8,
    depth: usize,
}

/// A context that can validate nonlocal archive memory.
pub trait ArchiveContext: Fallible {
    /// Checks that the given relative pointer can be dereferenced.
    ///
    /// The returned pointer is guaranteed to be located within the archive. This means that the
    /// returned pointer is safe to check, but may be vulnerable to memory overlap attacks unless
    /// the subtree range is properly restricted. Use `check_subtree_ptr` to perform the subtree
    /// range check as well.
    ///
    /// # Safety
    ///
    /// - `base` must be inside the archive this context was created for.
    /// - `metadata` must be the metadata for the pointer defined by `base` and `offset`.
    unsafe fn check_ptr<T: LayoutRaw + Pointee + ?Sized>(
        &mut self,
        base: *const u8,
        offset: isize,
        metadata: T::Metadata,
    ) -> Result<*const T, Self::Error>;

    /// Checks that the given `RelPtr` can be dereferenced.
    ///
    /// The returned pointer is guaranteed to be located within the archive. This means that the
    /// returned pointer is safe to check, but may be vulnerable to memory overlap attacks unless
    /// the subtree range is properly restricted. Use `check_subtree_ptr` to perform the subtree
    /// range check as well.
    ///
    /// # Safety
    ///
    /// - `rel_ptr` must be inside the archive this context was created for.
    #[inline]
    unsafe fn check_rel_ptr<T: ArchivePointee + LayoutRaw + ?Sized>(
        &mut self,
        rel_ptr: &RelPtr<T>,
    ) -> Result<*const T, Self::Error> {
        let metadata = T::pointer_metadata(rel_ptr.metadata());
        self.check_ptr(rel_ptr.base(), rel_ptr.offset(), metadata)
    }

    /// Checks that the given pointer is located completely within the subtree range.
    unsafe fn check_subtree_ptr_bounds<T: LayoutRaw + ?Sized>(
        &mut self,
        ptr: *const T,
    ) -> Result<(), Self::Error>;

    /// Checks that the given relative pointer to a subtree can be dereferenced.
    ///
    /// # Safety
    ///
    /// - `base` must be inside the archive this context was created for.
    /// - `metadata` must be the metadata for the pointer defined by `base` and `offset`.
    #[inline]
    unsafe fn check_subtree_ptr<T: LayoutRaw + Pointee + ?Sized>(
        &mut self,
        base: *const u8,
        offset: isize,
        metadata: T::Metadata,
    ) -> Result<*const T, Self::Error> {
        let ptr = self.check_ptr(base, offset, metadata)?;
        self.check_subtree_ptr_bounds(ptr)?;
        Ok(ptr)
    }

    /// Checks that the given `RelPtr` to a subtree can be dereferenced.
    ///
    /// # Safety
    ///
    /// - `rel_ptr` must be inside the archive this context was created for.
    #[inline]
    unsafe fn check_subtree_rel_ptr<T: ArchivePointee + LayoutRaw + ?Sized>(
        &mut self,
        rel_ptr: &RelPtr<T>,
    ) -> Result<*const T, Self::Error> {
        let ptr = self.check_rel_ptr(rel_ptr)?;
        self.check_subtree_ptr_bounds(ptr)?;
        Ok(ptr)
    }

    /// Pushes a new subtree range onto the context and starts validating it.
    ///
    /// After calling `push_subtree_claim_to`, the context will have a subtree range starting at
    /// the original start and ending at `root`. After popping the returned range, the context will
    /// have a subtree range starting at `end` and ending at the original end.
    ///
    /// # Safety
    ///
    /// `root` and `end` must be located inside the archive.
    unsafe fn push_prefix_subtree_range(
        &mut self,
        root: *const u8,
        end: *const u8,
    ) -> Result<ArchivePrefixRange, Self::Error>;

    /// Pushes a new subtree range onto the context and starts validating it.
    ///
    /// The claimed range spans from the end of `start` to the end of the current subobject range.
    ///
    /// # Safety
    ///
    /// `` must be located inside the archive.
    #[inline]
    unsafe fn push_prefix_subtree<T: LayoutRaw + ?Sized>(
        &mut self,
        root: *const T,
    ) -> Result<ArchivePrefixRange, Self::Error> {
        let layout = T::layout_raw(root);
        self.push_prefix_subtree_range(root as *const u8, (root as *const u8).add(layout.size()))
    }

    /// Pops the given range, restoring the original state with the pushed range removed.
    ///
    /// If the range was not popped in reverse order, an error is returned.
    fn pop_prefix_range(&mut self, range: ArchivePrefixRange) -> Result<(), Self::Error>;

    /// Pushes a new subtree range onto the context and starts validating it.
    ///
    /// After calling `push_prefix_subtree_range`, the context will have a subtree range starting at
    /// `start` and ending at `root`. After popping the returned range, the context will have a
    /// subtree range starting at the original start and ending at `start`.
    ///
    /// # Safety
    ///
    /// `start` and `root` must be located inside the archive.
    unsafe fn push_suffix_subtree_range(
        &mut self,
        start: *const u8,
        root: *const u8,
    ) -> Result<ArchiveSuffixRange, Self::Error>;

    /// Finishes the given range, restoring the original state with the pushed range removed.
    ///
    /// If the range was not popped in reverse order, an error is returned.
    fn pop_suffix_range(&mut self, range: ArchiveSuffixRange) -> Result<(), Self::Error>;

    /// Verifies that all outstanding claims have been returned.
    fn finish(&mut self) -> Result<(), Self::Error>;
}

/// A context that can validate shared archive memory.
///
/// Shared pointers require this kind of context to validate.
pub trait SharedArchiveContext: ArchiveContext {
    /// Claims `count` shared bytes located `offset` bytes away from `base`.
    ///
    /// Returns whether the bytes need to be checked.
    ///
    /// # Safety
    ///
    /// `base` must be inside the archive this context was created for.
    unsafe fn check_shared_ptr<T: LayoutRaw + ?Sized>(
        &mut self,
        ptr: *const T,
        type_id: TypeId,
    ) -> Result<Option<*const T>, Self::Error>;
}

/// Errors that can occur when checking an archive.
#[derive(Debug)]
pub enum CheckArchiveError<T, C> {
    /// An error that occurred while validating an object
    CheckBytesError(T),
    /// A context error occurred
    ContextError(C),
}

impl<T: fmt::Display, C: fmt::Display> fmt::Display for CheckArchiveError<T, C> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CheckArchiveError::CheckBytesError(e) => write!(f, "check bytes error: {}", e),
            CheckArchiveError::ContextError(e) => write!(f, "context error: {}", e),
        }
    }
}

#[cfg(feature = "std")]
impl<T: Error + 'static, C: Error + 'static> Error for CheckArchiveError<T, C> {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            CheckArchiveError::CheckBytesError(e) => Some(e as &dyn Error),
            CheckArchiveError::ContextError(e) => Some(e as &dyn Error),
        }
    }
}

/// The error type that can be produced by checking the given type with the given validator.
pub type CheckTypeError<T, C> =
    CheckArchiveError<<T as CheckBytes<C>>::Error, <C as Fallible>::Error>;

/// Checks the given archive with an additional context.
///
/// See [`check_archived_value`](crate::validation::validators::check_archived_value) for more details.
#[inline]
pub fn check_archived_value_with_context<
    'a,
    T: Archive,
    C: ArchiveContext + ?Sized,
>(
    buf: &'a [u8],
    pos: usize,
    context: &mut C,
) -> Result<&'a T::Archived, CheckTypeError<T::Archived, C>>
where
    T::Archived: CheckBytes<C> + Pointee<Metadata = ()>,
{
    unsafe {
        let ptr = context
            .check_ptr(buf.as_ptr(), pos as isize, ())
            .map_err(CheckArchiveError::ContextError)?;

        let range = context
            .push_prefix_subtree(ptr)
            .map_err(CheckArchiveError::ContextError)?;
        let result = CheckBytes::check_bytes(ptr, context)
            .map_err(CheckArchiveError::CheckBytesError)?;
        context.pop_prefix_range(range)
            .map_err(CheckArchiveError::ContextError)?;

        context.finish()
            .map_err(CheckArchiveError::ContextError)?;
        Ok(result)
    }
}

/// Checks the given archive with an additional context.
///
/// See [`check_archived_value`](crate::validation::validators::check_archived_value) for more details.
#[inline]
pub fn check_archived_root_with_context<
    'a,
    T: Archive,
    C: ArchiveContext + ?Sized,
>(
    buf: &'a [u8],
    context: &mut C,
) -> Result<&'a T::Archived, CheckTypeError<T::Archived, C>>
where
    T::Archived: CheckBytes<C> + Pointee<Metadata = ()>,
{
    check_archived_value_with_context::<T, C>(
        buf,
        buf.len() - core::mem::size_of::<T::Archived>(),
        context,
    )
}
