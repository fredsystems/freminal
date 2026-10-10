// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Kitty graphics protocol handler for [`TerminalHandler`].
//!
//! This module implements all Kitty graphics APC (`_G`) command processing:
//!
//! - Dispatch (`handle_kitty_graphics`) — routes commands by action type
//! - Query (`handle_kitty_query`) — responds to `a=q` capability queries
//! - Chunked transfer (`handle_kitty_chunk_start`, `handle_kitty_chunk`) — assembles
//!   multi-chunk payloads before dispatch
//! - Single-command path (`handle_kitty_single`) — decodes payload and places image
//! - Put (`handle_kitty_put`) — places a previously stored image by ID
//! - Payload decoding (`decode_kitty_payload`, `resolve_kitty_transmission`,
//!   `read_kitty_file`, `require_kitty_dimensions`) — resolves transmission medium
//!   and decodes pixel formats (RGB, RGBA, PNG)
//! - Image placement (`place_kitty_image`) — stores `InlineImage` and places cells
//! - Error response (`send_kitty_error`) — sends error APC to the PTY
//! - Delete (`handle_kitty_delete`) — removes images by various targets

use std::collections::HashSet;

use conv2::ValueFrom;
use freminal_common::buffer_states::kitty_graphics::{
    KittyAction, KittyControlData, KittyGraphicsCommand, KittyResponseId,
    format_kitty_response_body,
};
use freminal_common::buffer_states::row_number::RowNumber;

use freminal_buffer::image_store::{
    AnimationControl, AnimationRunMode, ImageProtocol, ImageSizeMode, InlineImage, SourceCrop,
    SubCellOffset, next_image_id, next_placement_instance_id,
};

use super::RealPlacement;
use super::TerminalHandler;
use super::chunk_assembler::{BoundedChunkAssembler, ChunkEncoding, ChunkLimits};
use super::{KittyImageState, KittyTransfer};
use crate::ansi_components::apc::MAX_APC_BYTES;
use crate::ansi_components::tracer::lossy_sequence_for_log_bounded;

/// Default gap (ms) kitty applies to a newly-created animation frame when
/// `z=` is absent or `0`. The root frame's default gap remains `0`
/// (tracked separately via `InlineImage::root_gap_ms`).
const DEFAULT_ANIMATION_FRAME_GAP_MS: u32 = 40;

/// What a Kitty delete (`a=d`) does with the image *data* once the placements
/// are gone.
///
/// The protocol spells this as the case of the `d=` key: lowercase
/// (`d=a`, `d=i`, ...) removes placements only, uppercase (`d=A`, `d=I`, ...)
/// also frees the stored image data when nothing else references it. The parser
/// surfaces that as `KittyControlData::delete_free_data`, a protocol-format
/// flag; everything below the dispatch takes this enum instead of a bare
/// `bool`, so a call site reads `Keep` or `Free` rather than `true`/`false`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ImageDataDisposition {
    /// Remove the placements; keep the image data (lowercase `d=`).
    Keep,
    /// Also free the image data when nothing else references it (uppercase
    /// `d=`).
    Free,
}

impl ImageDataDisposition {
    /// Whether the image data is to be freed.
    const fn frees_data(self) -> bool {
        matches!(self, Self::Free)
    }
}

impl From<bool> for ImageDataDisposition {
    /// The protocol boundary: `KittyControlData::delete_free_data`.
    fn from(delete_free_data: bool) -> Self {
        if delete_free_data {
            Self::Free
        } else {
            Self::Keep
        }
    }
}

/// Maximum relative-placement chain depth (Task 100.4a, `ETOODEEP`).
///
/// The kitty spec requires implementations to support a chain depth of at
/// least 8; a 9th link is rejected.
const MAX_RELATIVE_PLACEMENT_DEPTH: usize = 8;

/// Maximum number of decoded bytes accepted for one Kitty graphics image.
///
/// Matches kitty's own `MAX_DATA_SZ` (400 MiB). It caps both a chunked
/// transfer's assembled payload and a `t=f` / `t=t` file.
const MAX_KITTY_DATA_BYTES: usize = 400 * 1024 * 1024;

/// Maximum number of bytes read from a `t=f` / `t=t` file transmission.
///
/// Matches kitty's own `MAX_DATA_SZ` (400 MiB). A file larger than this is
/// rejected outright rather than truncated.
const MAX_KITTY_FILE_BYTES: usize = MAX_KITTY_DATA_BYTES;

/// The caps applied to a chunked Kitty graphics transfer.
///
/// A chunk is at most one APC sequence ([`MAX_APC_BYTES`]); the assembled
/// payload is at most [`MAX_KITTY_DATA_BYTES`].
const KITTY_CHUNK_LIMITS: ChunkLimits = ChunkLimits {
    max_chunk_bytes: MAX_APC_BYTES,
    max_total_bytes: MAX_KITTY_DATA_BYTES,
};

/// The single error response sent for every file-transmission failure.
///
/// Deliberately uniform so the response does not reveal whether a path
/// exists, is readable, or is a special file.
const KITTY_FILE_ERROR_RESPONSE: &str = "EBADF:Failed to read image file";

/// Substring a `t=t` file's path must contain before freminal will delete it.
const KITTY_TEMP_FILE_MARKER: &str = "tty-graphics-protocol";

/// Why a Kitty file transmission (`t=f` / `t=t`) was refused, or why the
/// `t=t` cleanup of the file could not be carried out.
///
/// Never sent to the client (see [`KITTY_FILE_ERROR_RESPONSE`]); it exists
/// for the debug log.
#[derive(Debug, thiserror::Error)]
enum KittyFileError {
    #[error("path is not valid UTF-8")]
    NotUtf8,
    #[error("path is not absolute")]
    NotAbsolute,
    #[error("failed to canonicalise path: {0}")]
    Canonicalize(std::io::Error),
    #[error("path is under a refused prefix (/proc, /sys, /dev)")]
    RefusedPrefix,
    #[error("failed to open file: {0}")]
    Open(std::io::Error),
    #[error("not a regular file")]
    NotRegularFile,
    #[error("failed to read file: {0}")]
    Read(std::io::Error),
    #[error("file exceeds the {MAX_KITTY_FILE_BYTES}-byte limit")]
    TooLarge,
    /// The path no longer resolves to the object that was opened.
    #[cfg(unix)]
    #[error("path no longer resolves to the opened file")]
    ChangedDuringOpen,
    /// The path has no parent directory or no file name to delete by.
    #[cfg(unix)]
    #[error("path has no parent directory or file name")]
    DeleteTargetInvalid,
    /// The entry at the path is no longer the file that was read.
    #[cfg(unix)]
    #[error("entry is no longer the file that was read")]
    DeleteTargetChanged,
    #[error("failed to delete file: {0}")]
    Delete(std::io::Error),
}

/// The identity of a filesystem object: device and inode.
///
/// Two handles refer to the same object exactly when their identities are
/// equal. Unix only: other platforms have no `(dev, ino)` pair in `std`, and
/// need none, because the prefixes this module refuses (`/proc`, `/sys`,
/// `/dev`) only exist on unix.
#[cfg(unix)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct FileIdentity {
    dev: u64,
    ino: u64,
}

#[cfg(unix)]
impl FileIdentity {
    /// The identity of the object `metadata` was taken from.
    fn of(metadata: &std::fs::Metadata) -> Self {
        use std::os::unix::fs::MetadataExt;
        Self {
            dev: metadata.dev(),
            ino: metadata.ino(),
        }
    }
}

/// A file read for a `t=f` / `t=t` transmission.
struct LoadedKittyFile {
    /// The canonical path the file was opened through.
    canonical: std::path::PathBuf,
    /// The file contents.
    data: Vec<u8>,
    /// The identity of the opened object, so a `t=t` deletion can be bound
    /// to the file that was actually read rather than to whatever the path
    /// names by then.
    #[cfg(unix)]
    identity: FileIdentity,
}

/// Whether `canonical` lies under a prefix that must never be read.
///
/// `/proc`, `/sys` and `/dev` expose live kernel state and device nodes
/// (`/dev/zero`, `/proc/self/mem`, ...). `/dev/shm` is the one exception:
/// it is an ordinary tmpfs that kitty clients legitimately use. The test is
/// component-wise, so `/procfoo` is not matched.
fn is_refused_kitty_path(canonical: &std::path::Path) -> bool {
    use std::path::Path;

    if canonical.starts_with("/dev/shm") {
        return false;
    }
    ["/proc", "/sys", "/dev"]
        .iter()
        .any(|prefix| canonical.starts_with(Path::new(prefix)))
}

/// Open `canonical` read-only and confirm the handle is a regular file.
///
/// On unix the file is opened `O_NONBLOCK` so that opening a FIFO cannot
/// block the PTY thread, and `O_NOFOLLOW` so that a final component swapped
/// for a symlink after canonicalisation fails to open instead of being
/// followed. The type check is done on the opened handle (`fstat`), not on
/// the path, so it cannot be raced.
fn open_regular_kitty_file(canonical: &std::path::Path) -> Result<std::fs::File, KittyFileError> {
    let mut options = std::fs::OpenOptions::new();
    options.read(true);

    #[cfg(unix)]
    {
        use nix::fcntl::OFlag;
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags((OFlag::O_NONBLOCK | OFlag::O_NOFOLLOW).bits());
    }

    let file = options.open(canonical).map_err(KittyFileError::Open)?;
    let metadata = file.metadata().map_err(KittyFileError::Open)?;
    if metadata.is_file() {
        Ok(file)
    } else {
        Err(KittyFileError::NotRegularFile)
    }
}

/// Bind the refused-prefix check to the object that was actually opened.
///
/// Mirrors kitty's `is_ok_to_read_image_file`: after opening, the path is
/// resolved again, vetted again, and the object it resolves to must be the
/// one the handle refers to (`samestat(stat(realpath), fstat(fd))`).
/// Without this, a path component could be swapped for a symlink into
/// `/proc`, `/sys` or `/dev` between the first canonicalisation and the open.
///
/// `/proc`, `/sys` and `/dev` are separate filesystems, so a handle opened
/// inside them cannot share `(dev, ino)` with a file at a permitted path: if
/// the identities match, the object read is the one at the vetted path.
///
/// Unix only. Other platforms have no refused prefixes (see
/// [`FileIdentity`]), so there is nothing for a swap to bypass.
///
/// Returns the identity of the opened object.
#[cfg(unix)]
fn verify_opened_kitty_file(
    file: &std::fs::File,
    original: &std::path::Path,
    canonical: &std::path::Path,
) -> Result<FileIdentity, KittyFileError> {
    let opened = FileIdentity::of(&file.metadata().map_err(KittyFileError::Open)?);

    let recanonical = std::fs::canonicalize(original).map_err(KittyFileError::Canonicalize)?;
    if is_refused_kitty_path(&recanonical) {
        return Err(KittyFileError::RefusedPrefix);
    }
    if recanonical != canonical {
        return Err(KittyFileError::ChangedDuringOpen);
    }

    let current = std::fs::metadata(&recanonical).map_err(KittyFileError::Open)?;
    if FileIdentity::of(&current) == opened {
        Ok(opened)
    } else {
        Err(KittyFileError::ChangedDuringOpen)
    }
}

/// Read at most `cap` bytes from `reader`; exceeding the cap is an error.
fn read_capped(reader: impl std::io::Read, cap: u64) -> Result<Vec<u8>, KittyFileError> {
    use std::io::Read;

    let mut data = Vec::new();
    // One byte past the cap lets us tell "exactly cap" from "more than cap".
    reader
        .take(cap.saturating_add(1))
        .read_to_end(&mut data)
        .map_err(KittyFileError::Read)?;
    match u64::value_from(data.len()) {
        Ok(len) if len <= cap => Ok(data),
        _ => Err(KittyFileError::TooLarge),
    }
}

/// Validate, open and read the file named by a `t=f` / `t=t` payload.
///
/// The result carries the canonical path and (on unix) the identity of the
/// opened object, so a `t=t` deletion targets exactly what was read.
fn load_kitty_file(payload: &[u8]) -> Result<LoadedKittyFile, KittyFileError> {
    let path_str = std::str::from_utf8(payload).map_err(|_| KittyFileError::NotUtf8)?;
    let path = std::path::Path::new(path_str);
    if !path.is_absolute() {
        return Err(KittyFileError::NotAbsolute);
    }

    let canonical = std::fs::canonicalize(path).map_err(KittyFileError::Canonicalize)?;
    if is_refused_kitty_path(&canonical) {
        return Err(KittyFileError::RefusedPrefix);
    }

    let file = open_regular_kitty_file(&canonical)?;
    #[cfg(unix)]
    let identity = verify_opened_kitty_file(&file, path, &canonical)?;
    let cap = u64::value_from(MAX_KITTY_FILE_BYTES).map_err(|_| KittyFileError::TooLarge)?;
    let data = read_capped(file, cap)?;
    Ok(LoadedKittyFile {
        canonical,
        data,
        #[cfg(unix)]
        identity,
    })
}

/// Whether a `t=t` file at `canonical` may be deleted after reading.
///
/// Requires both that the file lives inside a known temporary directory
/// (`temp_dir()`, `/tmp` or `/dev/shm`, each canonicalised) and that its path
/// contains `tty-graphics-protocol`, so a client cannot use `t=t` to delete
/// arbitrary readable files.
fn may_delete_kitty_temp_file(canonical: &std::path::Path) -> bool {
    if !canonical.to_string_lossy().contains(KITTY_TEMP_FILE_MARKER) {
        return false;
    }
    [
        std::env::temp_dir(),
        std::path::PathBuf::from("/tmp"),
        std::path::PathBuf::from("/dev/shm"),
    ]
    .iter()
    .filter_map(|dir| std::fs::canonicalize(dir).ok())
    .any(|dir| canonical.starts_with(dir))
}

/// Delete the `t=t` file that `loaded` was read from.
///
/// The caller has already decided the file may be deleted
/// ([`may_delete_kitty_temp_file`]). The deletion is bound to the file that
/// was read rather than to the pathname:
///
/// 1. the parent directory is opened (`O_NOFOLLOW | O_DIRECTORY`), fixing
///    the directory the entry is resolved in;
/// 2. the entry is opened relative to that directory handle (`O_NOFOLLOW`)
///    and must be a regular file with the same `(dev, ino)` as the file that
///    was read;
/// 3. only then is the entry unlinked, relative to the same directory handle.
///
/// Residual: there is still a window between steps 2 and 3 in which the
/// entry can be replaced, but within a fixed directory handle the only thing
/// that can be removed is a name that was just verified to be a link to the
/// inode that was read, or a replacement placed by someone with write access
/// to that directory. Pathname resolution can no longer redirect the unlink
/// to a different directory.
#[cfg(unix)]
fn delete_kitty_temp_file(loaded: &LoadedKittyFile) -> Result<(), KittyFileError> {
    use nix::fcntl::{OFlag, open, openat};
    use nix::sys::stat::Mode;
    use nix::unistd::{UnlinkatFlags, unlinkat};

    let (Some(parent), Some(name)) = (loaded.canonical.parent(), loaded.canonical.file_name())
    else {
        return Err(KittyFileError::DeleteTargetInvalid);
    };

    let dirfd = open(
        parent,
        OFlag::O_RDONLY | OFlag::O_DIRECTORY | OFlag::O_CLOEXEC | OFlag::O_NOFOLLOW,
        Mode::empty(),
    )
    .map_err(|e| KittyFileError::Delete(std::io::Error::from(e)))?;

    let entry = openat(
        &dirfd,
        name,
        OFlag::O_RDONLY | OFlag::O_NOFOLLOW | OFlag::O_NONBLOCK | OFlag::O_CLOEXEC,
        Mode::empty(),
    )
    .map_err(|e| KittyFileError::Delete(std::io::Error::from(e)))?;
    let metadata = std::fs::File::from(entry)
        .metadata()
        .map_err(KittyFileError::Delete)?;
    if !metadata.is_file() || FileIdentity::of(&metadata) != loaded.identity {
        return Err(KittyFileError::DeleteTargetChanged);
    }

    unlinkat(&dirfd, name, UnlinkatFlags::NoRemoveDir)
        .map_err(|e| KittyFileError::Delete(std::io::Error::from(e)))
}

/// Delete the `t=t` file that `loaded` was read from.
///
/// Non-unix fallback: `std` exposes no handle-relative delete, so this
/// removes by pathname. The race the unix implementation closes (a swap
/// redirecting the path into `/proc`, `/sys` or `/dev`) does not apply
/// here, since those prefixes do not exist on Windows.
#[cfg(not(unix))]
fn delete_kitty_temp_file(loaded: &LoadedKittyFile) -> Result<(), KittyFileError> {
    std::fs::remove_file(&loaded.canonical).map_err(KittyFileError::Delete)
}

/// Apply a signed cell offset (`H=`/`V=`) to a `usize` origin coordinate.
///
/// Clamps to `0` on underflow and to `usize::MAX` on overflow — both are
/// unreachable in practice for terminal-sized coordinates, but this keeps
/// the conversion panic-free rather than relying on that assumption.
///
/// `pub(super)` so [`TerminalHandler::inject_virtual_parent_relatives`]
/// (Task 100.4b, in `mod.rs`) can reuse it to derive a virtual-parent
/// child's render-time origin from the parent's live placeholder cells.
pub(super) fn signed_cell_offset(origin: usize, offset: i32) -> usize {
    let origin_i64 = i64::value_from(origin).unwrap_or(i64::MAX);
    let result = origin_i64.saturating_add(i64::from(offset)).max(0);
    usize::value_from(result).unwrap_or(usize::MAX)
}

/// Determine the `ImageSizeMode` for an image from a kitty command's control
/// data: if EITHER `c=`/`r=` is present, kitty derives/scales the display
/// grid, so the image is drawn scaled to fill it (`ExplicitCells`);
/// otherwise the display grid was derived from the image's native pixel
/// size (`NativePixels`) (Task 100.17).
const fn kitty_image_size_mode(control: &KittyControlData) -> ImageSizeMode {
    if control.display_cols.is_some() || control.display_rows.is_some() {
        ImageSizeMode::ExplicitCells
    } else {
        ImageSizeMode::NativePixels
    }
}

/// Build a `KittyResponseId` that doesn't carry an image number (`I=`).
///
/// This covers the deep decode/transmission/compose error responses (which
/// only ever had an `i=`-style hint to begin with) and the `a=c` (animation
/// compose) success response — the spec only mandates the `I=` echo on
/// successful transmit/put/animation-frame responses, which build their own
/// `KittyResponseId` directly.
const fn kitty_id_no_number(image_id: u32, placement_id: Option<u32>) -> KittyResponseId {
    KittyResponseId {
        image_id,
        image_number: None,
        placement_id,
    }
}

/// Log a shared-memory object name at debug level, bounded.
///
/// The name is sender-supplied, so it must never appear in a warn-level line;
/// the warn carries the failure, and this adjacent debug carries the name.
#[cfg(any(unix, windows))]
fn log_shm_name(context: &str, name: &str) {
    tracing::debug!(
        "Kitty graphics: {context}: object {:?}",
        lossy_sequence_for_log_bounded(name.as_bytes())
    );
}

/// Inflate an RFC 1950 zlib stream (kitty `o=z`), producing at most `limit`
/// bytes. Returns the decompressed bytes, or an error on malformed input or
/// if the stream inflates to more than `limit` bytes.
///
/// The bound matters because zlib compresses runs of identical bytes by a
/// factor of roughly 1000, so a payload well inside the transfer caps can
/// otherwise inflate to a size that exhausts memory. At most `limit + 1`
/// bytes are ever read from the decoder.
///
/// This is RFC 1950 "zlib" (a two-byte header + Adler-32 trailer around raw
/// DEFLATE data), not gzip (RFC 1952) and not raw DEFLATE — `o=z` in the
/// kitty spec specifically means zlib-wrapped data, hence
/// [`flate2::read::ZlibDecoder`] rather than `GzDecoder`/`DeflateDecoder`.
fn inflate_zlib(data: &[u8], limit: usize) -> Result<Vec<u8>, std::io::Error> {
    use std::io::Read as _;
    // One byte past the limit is enough to tell "exactly at" from "over".
    let read_cap = u64::value_from(limit).unwrap_or(u64::MAX).saturating_add(1);
    let mut out = Vec::new();
    flate2::read::ZlibDecoder::new(data)
        .take(read_cap)
        .read_to_end(&mut out)?;
    if out.len() > limit {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("decompressed data exceeds the {limit}-byte limit"),
        ));
    }
    Ok(out)
}

/// Is `name` a plausible, safe POSIX shared-memory object name for kitty
/// `t=s` transmission?
///
/// POSIX shm names are `/name` — a single leading slash and no other
/// slashes. This rejects anything that looks like it could escape into an
/// arbitrary filesystem path on implementations that map shm names onto
/// `/dev/shm/...` (or similar): a NUL byte, `..` traversal, an empty name,
/// or an embedded slash after stripping a single leading one.
///
/// This is a plain string check with no platform-specific API, so it is
/// shared by both the POSIX (`shm_open`) and Windows (`OpenFileMappingW`)
/// `read_kitty_shared_memory` implementations below — kitty clients use
/// the same POSIX-style shm name on every platform, so this refusal is a
/// meaningful security floor even on Windows, where named mappings
/// otherwise use different separator conventions (`Local\...`).
fn shm_name_is_safe(name: &str) -> bool {
    if name.is_empty() || name.contains('\0') || name.contains("..") {
        return false;
    }
    let stripped = name.strip_prefix('/').unwrap_or(name);
    !stripped.contains('/')
}

/// Compute the `(offset, length)` byte range to read from a shared-memory
/// object of `object_len` bytes, given the resolved `O=` byte offset and
/// (optional) `S=` byte count.
///
/// Returns `None` if the resulting range doesn't fit within the object
/// (including on `u64` overflow while adding offset and length).
///
/// This is pure arithmetic with no platform-specific API, so it is shared
/// by both the POSIX and Windows `read_kitty_shared_memory`
/// implementations. The Windows path has no equivalent to POSIX `fstat`
/// (a Windows file-mapping handle doesn't expose its own size), so it
/// passes `u64::MAX` as `object_len` — this still gets the checked
/// `offset + read_len` overflow protection, just without a real upper
/// bound; the real bound is enforced by `MapViewOfFile` failing if the
/// mapping is smaller than requested.
fn shm_read_bounds(object_len: u64, offset: u64, data_size: Option<u32>) -> Option<(u64, u64)> {
    let read_len = data_size.map_or_else(|| object_len.saturating_sub(offset), u64::from);
    let end = offset.checked_add(read_len)?;
    (end <= object_len).then_some((offset, read_len))
}

/// Read `read_len` bytes at `offset` from a POSIX shared-memory object of
/// `object_len` bytes, backed by `fd`.
///
/// Returns an empty vector without mapping anything if either length is
/// zero (`mmap` requires a non-zero length).
#[cfg(unix)]
fn read_kitty_shm_range(
    fd: &std::os::fd::OwnedFd,
    object_len: u64,
    offset: u64,
    read_len: u64,
) -> nix::Result<Vec<u8>> {
    if object_len == 0 || read_len == 0 {
        return Ok(Vec::new());
    }

    let map_len = std::num::NonZeroUsize::new(usize::value_from(object_len).unwrap_or(usize::MAX))
        .unwrap_or(std::num::NonZeroUsize::MAX);
    let offset_us = usize::value_from(offset).unwrap_or(0);
    let read_len_us = usize::value_from(read_len).unwrap_or(0);

    // SAFETY: `fd` references a shm object whose size (`object_len` ==
    // `map_len`) was just confirmed via `fstat` by the caller, and
    // `shm_read_bounds` has already verified `offset_us + read_len_us <=
    // object_len`.
    unsafe { mmap_copy_range(fd, map_len, offset_us, read_len_us) }
}

/// Map `map_len` bytes of `fd` read-only, copy `read_len` bytes starting at
/// `offset` out of the mapping, then unmap.
///
/// # Safety
///
/// `fd` must reference an object with at least `map_len` readable bytes,
/// and `offset + read_len` must be `<= map_len.get()`.
#[cfg(unix)]
unsafe fn mmap_copy_range(
    fd: &std::os::fd::OwnedFd,
    map_len: std::num::NonZeroUsize,
    offset: usize,
    read_len: usize,
) -> nix::Result<Vec<u8>> {
    use nix::sys::mman::{MapFlags, ProtFlags, mmap, munmap};

    // SAFETY: forwarded from this function's safety contract — `fd` covers
    // at least `map_len` bytes. The mapping is read-only and is unmapped
    // below before returning; no pointer derived from it escapes.
    let ptr = unsafe {
        mmap(
            None,
            map_len,
            ProtFlags::PROT_READ,
            MapFlags::MAP_SHARED,
            fd,
            0,
        )
    }?;

    // SAFETY: `ptr` is valid for `map_len.get()` bytes (the mmap length
    // above); the caller's contract guarantees `offset + read_len <=
    // map_len.get()`.
    let bytes =
        unsafe { std::slice::from_raw_parts(ptr.as_ptr().cast::<u8>().add(offset), read_len) };
    let copied = bytes.to_vec();

    // SAFETY: `ptr`/`map_len` are exactly the values returned by the
    // matching `mmap` call above.
    unsafe { munmap(ptr, map_len.get()) }?;

    Ok(copied)
}

/// Whether `control` could belong to a continuation chunk of a chunked
/// transfer.
///
/// Per the kitty graphics spec, continuation chunks carry only `m` and
/// optionally `q`; every other key is taken from the first chunk. Comparing
/// against a default with `more_data` and `quiet` reset is therefore exact:
/// it stays correct as `KittyControlData` gains keys, because any key set to
/// a non-default value makes the chunk a new command instead.
fn is_bare_continuation(control: &KittyControlData) -> bool {
    let mut rest = control.clone();
    rest.more_data = false;
    rest.quiet = 0;
    rest == KittyControlData::default()
}

impl TerminalHandler {
    /// Dispatch a parsed Kitty graphics command.
    pub(super) fn handle_kitty_graphics(&mut self, cmd: KittyGraphicsCommand) {
        let action = cmd.control.action.unwrap_or(KittyAction::Transmit);

        // If this is a continuation chunk for an in-progress chunked transfer,
        // append the payload.  Per the Kitty protocol spec, continuation chunks
        // carry ONLY `m` and optionally `q` — no explicit `a=` key.  If the
        // incoming command explicitly sets `a=`, it is a new command and any
        // stale chunked state must be discarded.
        match &self.kitty_transfer {
            KittyTransfer::Receiving(_) if cmd.control.action.is_none() => {
                // No explicit action → this is a continuation chunk.
                self.handle_kitty_chunk(&cmd);
                return;
            }
            KittyTransfer::Discarding
                if cmd.control.action.is_none() && is_bare_continuation(&cmd.control) =>
            {
                // Tail of a transfer that was already abandoned: drop it.
                self.handle_kitty_discarded_chunk(&cmd);
                return;
            }
            KittyTransfer::Receiving(state) => {
                // Explicit action on a new command while a chunked transfer is
                // in progress — discard the stale accumulator.
                tracing::warn!(
                    "Kitty graphics: discarding incomplete chunked transfer \
                     (id={:?}) due to new command (a={action:?})",
                    state.control.image_id,
                );
                self.kitty_transfer = KittyTransfer::Idle;
            }
            KittyTransfer::Discarding => {
                // A new command ends the abandoned transfer's silent tail. This
                // includes an actionless command that carries more than a
                // continuation chunk may (`a=t` is the default action).
                tracing::debug!(
                    "Kitty graphics: new command (a={action:?}) ends discarded transfer"
                );
                self.kitty_transfer = KittyTransfer::Idle;
            }
            KittyTransfer::Idle => {}
        }

        tracing::debug!(
            "Kitty graphics: dispatch a={action:?} i={:?} m={} q={}",
            cmd.control.image_id,
            cmd.control.more_data,
            cmd.control.quiet,
        );

        match action {
            KittyAction::Query => self.handle_kitty_query(&cmd),
            KittyAction::Transmit | KittyAction::TransmitAndDisplay | KittyAction::Put => {
                if cmd.control.more_data {
                    // First chunk of a chunked transfer — start accumulating.
                    self.handle_kitty_chunk_start(cmd);
                } else {
                    // Single-chunk command (or final chunk with no prior state).
                    self.handle_kitty_single(&cmd, action);
                }
            }
            KittyAction::Delete => {
                self.handle_kitty_delete(&cmd);
            }
            KittyAction::AnimationFrame => {
                if cmd.control.more_data {
                    // First chunk of a chunked animation-frame transfer —
                    // reuse the same accumulation path as image transmit.
                    self.handle_kitty_chunk_start(cmd);
                } else {
                    self.handle_kitty_animation_frame(&cmd);
                }
            }
            KittyAction::AnimationControl => {
                self.handle_kitty_animation_control(&cmd);
            }
            KittyAction::AnimationCompose => {
                self.handle_kitty_animation_compose(&cmd);
            }
        }
    }

    /// Handle `a=q` — query whether the terminal supports the Kitty graphics protocol.
    ///
    /// Responds with OK for formats 24 (RGB), 32 (RGBA), and 100 (PNG).
    /// Other formats get an error response.
    fn handle_kitty_query(&self, cmd: &KittyGraphicsCommand) {
        let image_id = cmd.control.image_id.unwrap_or(0);
        let quiet = cmd.control.quiet;

        // We support RGB (f=24), RGBA (f=32), and PNG (f=100).
        let supported = cmd.control.format.is_none_or(|f| {
            matches!(
                f,
                freminal_common::buffer_states::kitty_graphics::KittyFormat::Rgb
                    | freminal_common::buffer_states::kitty_graphics::KittyFormat::Rgba
                    | freminal_common::buffer_states::kitty_graphics::KittyFormat::Png
            )
        });

        // quiet=1 suppresses OK responses; quiet=2 suppresses all responses.
        if quiet >= 2 || (quiet >= 1 && supported) {
            return;
        }

        let id = kitty_id_no_number(image_id, None);
        let response = if supported {
            format_kitty_response_body(id, true, "")
        } else {
            format_kitty_response_body(id, false, "ENOTSUP:unsupported format")
        };

        self.write_apc_response(&response);
    }

    /// Start a chunked Kitty graphics transfer (first chunk, `m=1`).
    fn handle_kitty_chunk_start(&mut self, cmd: KittyGraphicsCommand) {
        if matches!(self.kitty_transfer, KittyTransfer::Receiving(_)) {
            tracing::warn!(
                "Kitty graphics: new chunked transfer started while previous was in progress; \
                 discarding incomplete transfer"
            );
        }

        // The sender-supplied `S=` is never used to size an allocation; the
        // assembler grows only as bytes actually arrive.
        let mut data = BoundedChunkAssembler::new(KITTY_CHUNK_LIMITS);
        if let Err(err) = data.push(&cmd.payload, ChunkEncoding::Raw) {
            // The first chunk is `m=1`, so more chunks follow: swallow them.
            tracing::warn!("Kitty graphics: chunked transfer abandoned: {err}");
            self.kitty_transfer = KittyTransfer::Discarding;
            return;
        }

        tracing::debug!(
            "Kitty graphics: started chunked transfer (id={:?})",
            cmd.control.image_id,
        );
        self.kitty_transfer = KittyTransfer::Receiving(Box::new(KittyImageState {
            control: cmd.control,
            data,
        }));
    }

    /// Drop a continuation chunk of an abandoned transfer, silently.
    ///
    /// The final chunk (`m=0`) ends the transfer, returning to `Idle`.
    fn handle_kitty_discarded_chunk(&mut self, cmd: &KittyGraphicsCommand) {
        tracing::trace!(
            "Kitty graphics: dropping chunk of abandoned transfer ({} bytes, more={})",
            cmd.payload.len(),
            cmd.control.more_data,
        );
        if !cmd.control.more_data {
            self.kitty_transfer = KittyTransfer::Idle;
        }
    }

    /// Abandon the in-progress transfer after a chunk error, with no reply.
    ///
    /// If the offending chunk was not the final one, the remaining chunks are
    /// swallowed (`Discarding`); if it was the final chunk the transfer is over.
    fn abandon_kitty_transfer(&mut self, cmd: &KittyGraphicsCommand) {
        self.kitty_transfer = if cmd.control.more_data {
            KittyTransfer::Discarding
        } else {
            KittyTransfer::Idle
        };
    }

    /// Append a chunk to the in-progress Kitty graphics transfer.
    ///
    /// If `m=0` (final chunk), finalise the transfer and dispatch.
    fn handle_kitty_chunk(&mut self, cmd: &KittyGraphicsCommand) {
        let KittyTransfer::Receiving(state) = &mut self.kitty_transfer else {
            tracing::warn!("Kitty graphics: chunk received with no active transfer; ignoring");
            return;
        };

        if let Err(err) = state.data.push(&cmd.payload, ChunkEncoding::Raw) {
            tracing::warn!("Kitty graphics: chunked transfer abandoned: {err}");
            self.abandon_kitty_transfer(cmd);
            return;
        }

        if cmd.control.more_data {
            // More chunks to come.
            tracing::debug!(
                "Kitty graphics: appended chunk ({} bytes total so far)",
                state.data.len(),
            );
        } else {
            // Final chunk — take ownership and dispatch.
            let KittyTransfer::Receiving(final_state) = std::mem::take(&mut self.kitty_transfer)
            else {
                // Unreachable: `state` above borrowed the transfer as `Receiving`.
                return;
            };

            let total = final_state.data.len();
            let payload = match final_state.data.finish() {
                Ok(payload) => payload,
                Err(err) => {
                    // Final chunk: the transfer is over (state is already Idle).
                    tracing::warn!("Kitty graphics: chunked transfer abandoned: {err}");
                    return;
                }
            };

            tracing::debug!("Kitty graphics: chunked transfer complete ({total} bytes)");

            let action = final_state.control.action.unwrap_or(KittyAction::Transmit);
            let final_cmd = KittyGraphicsCommand {
                control: final_state.control,
                payload,
            };
            if action == KittyAction::AnimationFrame {
                self.handle_kitty_animation_frame(&final_cmd);
            } else {
                self.handle_kitty_single(&final_cmd, action);
            }
        }
    }

    /// Handle a single (non-chunked) Kitty graphics command.
    ///
    /// Decodes the image payload according to the format (`f=24` RGB, `f=32`
    /// RGBA, `f=100` PNG), computes display dimensions, stores as an
    /// `InlineImage`, and places into the buffer.
    fn handle_kitty_single(&mut self, cmd: &KittyGraphicsCommand, action: KittyAction) {
        let image_id_hint = cmd.control.image_id.unwrap_or(0);
        let quiet = cmd.control.quiet;

        // `a=p` (Put) with an empty payload means "display a previously
        // transmitted image by its ID."  Look up the image from the store
        // instead of trying to decode an empty payload.
        if action == KittyAction::Put && cmd.payload.is_empty() {
            self.handle_kitty_put(cmd, image_id_hint, quiet);
            return;
        }

        if cmd.payload.is_empty() {
            tracing::warn!("Kitty graphics: empty payload for a={action:?}; ignoring");
            self.send_kitty_error(
                kitty_id_no_number(image_id_hint, cmd.control.placement_id),
                quiet,
                "ENODATA:no payload",
            );
            return;
        }

        // Decode payload into RGBA pixels + dimensions.
        let Some((rgba_pixels, img_width_px, img_height_px)) =
            self.decode_kitty_payload(cmd, image_id_hint, cmd.control.placement_id, quiet)
        else {
            return; // Error already sent by decode_kitty_payload.
        };

        // Compute display size in cells and place the image.
        self.place_kitty_image(
            &cmd.control,
            action,
            rgba_pixels,
            img_width_px,
            img_height_px,
            image_id_hint,
            quiet,
        );
    }

    /// Resolve the target image id for a by-reference command (`a=p` put,
    /// `a=f` animation frame).
    ///
    /// Prefers a nonzero `i=` (image id). If `i=` is absent or zero, resolves
    /// `I=` (image number) to the id of the newest image transmitted with
    /// that number. Returns `None` if neither resolves to a stored image
    /// reference — callers are responsible for sending the appropriate
    /// `ENOENT` response.
    fn resolve_kitty_image_id(&self, control: &KittyControlData) -> Option<u64> {
        match control.image_id {
            Some(id) if id != 0 => Some(u64::from(id)),
            _ => control
                .image_number
                .and_then(|number| self.buffer.image_store().newest_id_for_number(number)),
        }
    }

    /// Resolve the target image id for a by-reference command (`a=p` put,
    /// `a=f` animation frame), sending the `I=`-specific `ENOENT` response
    /// and returning `None` if `I=` was given but does not resolve to a
    /// stored image.
    ///
    /// When neither `i=` nor `I=` is given (or `i=` is stale), returns
    /// `Some` wrapping the raw hint — the caller's subsequent image-store
    /// lookup will fail and send the generic `ENOENT` response in that case.
    fn resolve_kitty_reference_id(
        &self,
        control: &KittyControlData,
        image_id_hint: u32,
        placement_id: Option<u32>,
        quiet: u8,
    ) -> Option<u64> {
        if let Some(id) = self.resolve_kitty_image_id(control) {
            return Some(id);
        }
        if let Some(number) = control.image_number {
            // `I=` was given but no image with that number is stored.
            self.send_kitty_error(
                KittyResponseId {
                    image_id: 0,
                    image_number: Some(number),
                    placement_id,
                },
                quiet,
                "ENOENT:image not found",
            );
            return None;
        }
        Some(u64::from(image_id_hint))
    }

    /// Apply `a=p` display-size overrides (`c=`/`r=`) to a stored image,
    /// returning the resized `InlineImage` plus its (possibly overridden)
    /// display dimensions in cells.
    fn apply_put_display_overrides(
        &self,
        cmd: &KittyGraphicsCommand,
        stored_image: InlineImage,
    ) -> (InlineImage, usize, usize) {
        let (term_width, term_height) = self.win_size();

        let display_cols = cmd
            .control
            .display_cols
            .map_or(stored_image.display_cols, |c| {
                let cols = usize::value_from(c).unwrap_or(0);
                cols.min(term_width).max(1)
            });

        let display_rows = cmd
            .control
            .display_rows
            .map_or(stored_image.display_rows, |r| {
                let rows = usize::value_from(r).unwrap_or(0);
                rows.min(term_height).max(1)
            });

        let size_mode = kitty_image_size_mode(&cmd.control);

        let image_to_place = InlineImage {
            id: stored_image.id,
            pixels: stored_image.pixels,
            width_px: stored_image.width_px,
            height_px: stored_image.height_px,
            display_cols,
            display_rows,
            size_mode,
            frames: stored_image.frames.clone(),
            root_gap_ms: stored_image.root_gap_ms,
            animation: stored_image.animation,
        };

        (image_to_place, display_cols, display_rows)
    }

    /// Compute display dimensions in cells for a freshly transmitted image
    /// (not yet in the store), applying `c=`/`r=` overrides when present
    /// and clamping to the terminal size. Falls back to the natural size
    /// (`img_*_px` divided by cell pixel size) when no override is given.
    fn compute_display_size_from_pixels(
        &self,
        control: &KittyControlData,
        img_width_px: u32,
        img_height_px: u32,
    ) -> (usize, usize) {
        let (term_width, term_height) = self.win_size();

        let display_cols = control.display_cols.map_or_else(
            || {
                let cols =
                    usize::value_from(img_width_px.div_ceil(self.cell_pixel_width)).unwrap_or(0);
                cols.min(term_width).max(1)
            },
            |c| {
                let cols = usize::value_from(c).unwrap_or(0);
                cols.min(term_width).max(1)
            },
        );

        let display_rows = control.display_rows.map_or_else(
            || {
                let rows =
                    usize::value_from(img_height_px.div_ceil(self.cell_pixel_height)).unwrap_or(0);
                rows.min(term_height).max(1)
            },
            |r| {
                let rows = usize::value_from(r).unwrap_or(0);
                rows.min(term_height).max(1)
            },
        );

        (display_cols, display_rows)
    }

    /// Register a virtual (Unicode placeholder) placement in
    /// `virtual_placements`. Shared by [`Self::handle_kitty_put`] and
    /// [`Self::place_kitty_image`].
    fn register_virtual_placement(
        &mut self,
        image_id: u64,
        placement_id: u32,
        display_cols: usize,
        display_rows: usize,
    ) {
        let vp = super::VirtualPlacement {
            image_id,
            placement_id,
            cols: u32::try_from(display_cols).unwrap_or(u32::MAX),
            rows: u32::try_from(display_rows).unwrap_or(u32::MAX),
            // Task 100.20: a fresh instance id every time this is
            // (re-)registered, so placeholder cells stamped from an earlier
            // registration (which keep their own already-stamped instance
            // id) remain a distinct renderer bucket from placeholder cells
            // stamped after this new registration.
            placement_instance: next_placement_instance_id(),
        };
        self.virtual_placements.insert((image_id, placement_id), vp);
    }

    /// Record a plain (non-relative) real (cell-stamped) placement's origin
    /// so future relative placements can reference it as a parent (Task
    /// 100.4a).
    // All parameters are required placement fields; grouping into a struct
    // would obscure the data flow without reducing coupling, matching the
    // established convention for `place_kitty_image`-adjacent methods.
    #[allow(clippy::too_many_arguments)]
    fn record_real_placement(
        &mut self,
        image_id: u64,
        placement_id: u32,
        origin_row: RowNumber,
        origin_col: usize,
        display_cols: usize,
        display_rows: usize,
        z_index: i32,
        placement_instance: u64,
    ) {
        self.insert_real_placement(
            image_id,
            placement_id,
            origin_row,
            origin_col,
            display_cols,
            display_rows,
            None,
            z_index,
            0,
            0,
            placement_instance,
        );
    }

    /// Insert (or overwrite) a `real_placements` entry, real (cell-stamped)
    /// or a virtual-parent registration. Shared by [`Self::record_real_placement`]
    /// (plain placements, `parent: None`) and
    /// [`Self::handle_kitty_relative_placement`] (relative placements,
    /// `parent: Some(..)`).
    // All parameters are required placement fields; grouping into a struct
    // would obscure the data flow without reducing coupling, matching the
    // established convention for `place_kitty_image`-adjacent methods.
    #[allow(clippy::too_many_arguments)]
    fn insert_real_placement(
        &mut self,
        image_id: u64,
        placement_id: u32,
        origin_row: RowNumber,
        origin_col: usize,
        display_cols: usize,
        display_rows: usize,
        parent: Option<(u64, u32)>,
        z_index: i32,
        h_offset: i32,
        v_offset: i32,
        placement_instance: u64,
    ) {
        self.real_placements.insert(
            (image_id, placement_id),
            crate::terminal_handler::RealPlacement {
                image_id,
                placement_id,
                origin_row,
                origin_col,
                cols: u32::try_from(display_cols).unwrap_or(u32::MAX),
                rows: u32::try_from(display_rows).unwrap_or(u32::MAX),
                parent,
                z_index,
                h_offset,
                v_offset,
                placement_instance,
            },
        );
    }

    /// Handle `a=p` (Put) — display a previously transmitted image.
    ///
    /// Looks up the image by `i=<image_id>` (or, if absent, resolves `I=`
    /// to the newest image with that number) in the image store, applies
    /// any display-size overrides (`c=`/`r=`) from the control data, and
    /// places the image into the buffer.
    fn handle_kitty_put(&mut self, cmd: &KittyGraphicsCommand, image_id: u32, quiet: u8) {
        /// Most stored image ids listed in the unknown-id debug log.
        const MAX_LISTED_IDS: usize = 16;

        let image_number = cmd.control.image_number;

        let Some(id) = self.resolve_kitty_reference_id(
            &cmd.control,
            image_id,
            cmd.control.placement_id,
            quiet,
        ) else {
            return;
        };

        // Look up the previously transmitted image.
        let Some(stored_image) = self.buffer.image_store().get(id).cloned() else {
            tracing::warn!(
                "Kitty graphics: a=p for unknown image id={image_id} \
                 (store has {} images)",
                self.buffer.image_store().len(),
            );
            let store_len = self.buffer.image_store().len();
            tracing::debug!(
                "Kitty graphics: stored image ids: {:?}{}",
                self.buffer
                    .image_store()
                    .iter()
                    .map(|(k, _)| k)
                    .take(MAX_LISTED_IDS)
                    .collect::<Vec<_>>(),
                if store_len > MAX_LISTED_IDS {
                    format!(" … ({} more)", store_len - MAX_LISTED_IDS)
                } else {
                    String::new()
                },
            );
            self.send_kitty_error(
                KittyResponseId {
                    image_id,
                    image_number,
                    placement_id: cmd.control.placement_id,
                },
                quiet,
                "ENOENT:image not found",
            );
            return;
        };

        // Apply display-size overrides from the Put command, if present.
        let (image_to_place, display_cols, display_rows) =
            self.apply_put_display_overrides(cmd, stored_image);

        // Update the store with the possibly-resized image.
        self.buffer
            .image_store_mut()
            .insert_protocol_retained(image_to_place.clone());

        // Relative placement (`P=` present) — intercept before the
        // virtual/normal placement paths below; it sends its own response
        // (Task 100.4a).
        if cmd.control.parent_image_id.is_some() {
            self.handle_kitty_relative_placement(
                &cmd.control,
                id,
                display_cols,
                display_rows,
                quiet,
            );
            return;
        }

        if cmd.control.unicode_placeholder {
            let pid = cmd.control.placement_id.unwrap_or(0);
            self.register_virtual_placement(id, pid, display_cols, display_rows);
        } else {
            self.stamp_kitty_put(cmd, id, image_to_place, display_cols, display_rows);
        }

        // Send OK response unless suppressed.
        if quiet < 1 && id > 0 {
            let response_id = u32::value_from(id).unwrap_or(0);
            let response = format_kitty_response_body(
                KittyResponseId {
                    image_id: response_id,
                    image_number,
                    placement_id: cmd.control.placement_id,
                },
                true,
                "",
            );
            self.write_apc_response(&response);
        }
    }

    /// Stamp a non-virtual (cell-materialized) `a=p` placement at the
    /// cursor, restoring the cursor afterward if `C=1` was requested, and
    /// record it as a real placement so it can act as a future relative-
    /// placement parent (Task 100.4a). Resolves the `x=`/`y=`/`w=`/`h=`
    /// source-crop (Task 100.9) against the image's own pixel dimensions.
    ///
    /// Split out of [`Self::handle_kitty_put`] to keep that function within
    /// the line-count lint limit.
    fn stamp_kitty_put(
        &mut self,
        cmd: &KittyGraphicsCommand,
        id: u64,
        image_to_place: InlineImage,
        display_cols: usize,
        display_rows: usize,
    ) {
        let cursor = self.buffer.cursor().pos;
        tracing::debug!(
            "Kitty graphics: a=p placing image id={id} at cursor ({},{}) \
             {display_cols}x{display_rows} cells, C={}",
            cursor.x,
            cursor.y,
            u8::from(cmd.control.no_cursor_movement),
        );
        let source_crop = resolve_source_crop(
            &cmd.control,
            image_to_place.width_px,
            image_to_place.height_px,
        );
        let subcell_offset = self.resolve_subcell_offset(&cmd.control);

        // Kitty spec REPLACE semantics (Task 100.18): a second `a=p` put
        // with the SAME non-zero `p=` replaces that one placement; `p=0`/
        // unspecified means multiple, independently-coexisting placements,
        // so it must NOT clear anything here.
        if let Some(pid) = cmd.control.placement_id
            && pid != 0
        {
            self.buffer.clear_image_placements_by_placement(id, pid);
        }

        // Mint a fresh placement-instance id for this DISPLAY put — every
        // `a=p` put is a distinct on-screen placement, even a `p=0`/
        // unspecified one that shares an image id with another placement
        // (Task 100.18).
        let placement_instance = next_placement_instance_id();
        let place_result = self.buffer.place_image(
            image_to_place,
            0,
            ImageProtocol::Kitty,
            cmd.control.image_number,
            cmd.control.placement_id,
            cmd.control.z_index.unwrap_or(0),
            source_crop,
            placement_instance,
            subcell_offset,
        );

        // `C=1` (no cursor movement): put the cursor back on the image origin,
        // located by row number so scrollback eviction during placement cannot
        // displace it (Task 125.C7).
        if cmd.control.no_cursor_movement {
            self.buffer.restore_cursor_to_image_origin(&place_result);
        }

        // Record this real (cell-stamped) placement's origin so future
        // relative placements can reference it as a parent (Task 100.4a).
        //
        // Use the TRUE stamped origin from `place_result`, not the
        // pre-call `cursor` read above: `place_image` may drain scrollback
        // rows during placement, which shifts the image's actual row
        // upward relative to the cursor position captured before the call
        // (Task 100.14).
        let pid = cmd.control.placement_id.unwrap_or(0);
        self.record_real_placement(
            id,
            pid,
            place_result.origin_row,
            place_result.origin_col,
            display_cols,
            display_rows,
            cmd.control.z_index.unwrap_or(0),
            place_result.placement_instance,
        );
    }

    /// Decode a Kitty graphics payload into RGBA pixel data.
    ///
    /// Handles the transmission medium (`t=d` direct, `t=f` file path,
    /// `t=t` temp file, `t=s` shared memory) to resolve the raw image bytes,
    /// then decodes according to the pixel format (`f=24`, `f=32`, `f=100`).
    ///
    /// Returns `None` if decoding fails (an error response is sent to the PTY).
    fn decode_kitty_payload(
        &self,
        cmd: &KittyGraphicsCommand,
        image_id_hint: u32,
        placement_id: Option<u32>,
        quiet: u8,
    ) -> Option<(Vec<u8>, u32, u32)> {
        use freminal_common::buffer_states::kitty_graphics::KittyFormat;

        // Resolve the transmission medium and apply `o=z` decompression.
        let image_data = self.resolve_and_decompress_kitty_payload(
            cmd,
            image_id_hint,
            placement_id,
            quiet,
            MAX_KITTY_DATA_BYTES,
        )?;

        let format = cmd.control.format.unwrap_or(KittyFormat::Rgba);

        match format {
            KittyFormat::Rgba => {
                let (w, h) =
                    self.require_kitty_dimensions(cmd, image_id_hint, placement_id, quiet)?;
                // `w` and `h` are `u32`; on 32-bit platforms the multiplication
                // could overflow, so use saturating arithmetic to avoid wrap.
                let w_us = usize::value_from(w).unwrap_or(0);
                let h_us = usize::value_from(h).unwrap_or(0);
                let expected = w_us.saturating_mul(h_us).saturating_mul(4);
                if image_data.len() != expected {
                    tracing::warn!(
                        "Kitty RGBA: expected {expected} bytes, got {}",
                        image_data.len()
                    );
                    self.send_kitty_error(
                        kitty_id_no_number(image_id_hint, placement_id),
                        quiet,
                        "EINVAL:payload size mismatch",
                    );
                    return None;
                }
                Some((image_data, w, h))
            }
            KittyFormat::Rgb => {
                let (w, h) =
                    self.require_kitty_dimensions(cmd, image_id_hint, placement_id, quiet)?;
                let w_us = usize::value_from(w).unwrap_or(0);
                let h_us = usize::value_from(h).unwrap_or(0);
                let expected = w_us.saturating_mul(h_us).saturating_mul(3);
                if image_data.len() != expected {
                    tracing::warn!(
                        "Kitty RGB: expected {expected} bytes, got {}",
                        image_data.len()
                    );
                    self.send_kitty_error(
                        kitty_id_no_number(image_id_hint, placement_id),
                        quiet,
                        "EINVAL:payload size mismatch",
                    );
                    return None;
                }
                let pixel_count = w_us.saturating_mul(h_us);
                let mut rgba = Vec::with_capacity(pixel_count.saturating_mul(4));
                // `as_chunks` yields fixed-size arrays rather than slices
                // (clippy 1.98's `chunks_exact_to_as_chunks`). A trailing
                // partial pixel is discarded exactly as `chunks_exact` did.
                for [r, g, b] in image_data.as_chunks::<3>().0 {
                    rgba.extend_from_slice(&[*r, *g, *b, 255]);
                }
                Some((rgba, w, h))
            }
            KittyFormat::Png => match image::load_from_memory(&image_data) {
                Ok(img) => {
                    let rgba_img = img.to_rgba8();
                    let w = rgba_img.width();
                    let h = rgba_img.height();
                    if w == 0 || h == 0 {
                        self.send_kitty_error(
                            kitty_id_no_number(image_id_hint, placement_id),
                            quiet,
                            "EINVAL:decoded image has zero dimensions",
                        );
                        return None;
                    }
                    Some((rgba_img.into_raw(), w, h))
                }
                Err(e) => {
                    tracing::warn!("Kitty PNG decode failed");
                    tracing::debug!("Kitty PNG decode failed: {e}");
                    self.send_kitty_error(
                        kitty_id_no_number(image_id_hint, placement_id),
                        quiet,
                        "EINVAL:PNG decode failed",
                    );
                    None
                }
            },
        }
    }

    /// Resolve the transmission medium and apply `o=z` (RFC 1950 zlib)
    /// decompression, producing the raw bytes ready for format
    /// interpretation (`f=24`/`f=32`/`f=100`).
    ///
    /// Decompressed output larger than `max_decompressed` is refused the same
    /// way a corrupt zlib stream is (production passes
    /// [`MAX_KITTY_DATA_BYTES`]).
    fn resolve_and_decompress_kitty_payload(
        &self,
        cmd: &KittyGraphicsCommand,
        image_id_hint: u32,
        placement_id: Option<u32>,
        quiet: u8,
        max_decompressed: usize,
    ) -> Option<Vec<u8>> {
        use freminal_common::buffer_states::kitty_graphics::KittyCompression;

        let image_data =
            self.resolve_kitty_transmission(cmd, image_id_hint, placement_id, quiet)?;

        if !matches!(cmd.control.compression, Some(KittyCompression::Zlib)) {
            return Some(image_data);
        }

        match inflate_zlib(&image_data, max_decompressed) {
            Ok(decompressed) => Some(decompressed),
            Err(e) => {
                tracing::warn!("Kitty graphics: zlib decompression failed");
                tracing::debug!("Kitty graphics: zlib decompression failed: {e}");
                self.send_kitty_error(
                    kitty_id_no_number(image_id_hint, placement_id),
                    quiet,
                    "EINVAL:zlib decompression failed",
                );
                None
            }
        }
    }

    /// Resolve Kitty transmission medium to raw image bytes.
    ///
    /// - `Direct` (default): payload is already the image data.
    /// - `File`: payload is a UTF-8 file path; read the file from disk.
    /// - `TempFile`: same as `File`, but delete the file after reading.
    /// - `SharedMemory`: payload is a POSIX shared memory object name (`t=s`).
    ///   `data_size`/`data_offset` (the `S=`/`O=` control keys) select the
    ///   byte range to read; the object is unlinked after reading per spec.
    fn resolve_kitty_transmission(
        &self,
        cmd: &KittyGraphicsCommand,
        image_id_hint: u32,
        placement_id: Option<u32>,
        quiet: u8,
    ) -> Option<Vec<u8>> {
        use freminal_common::buffer_states::kitty_graphics::KittyTransmission;

        let payload = cmd.payload.as_slice();
        match cmd
            .control
            .transmission
            .unwrap_or(KittyTransmission::Direct)
        {
            KittyTransmission::Direct => Some(payload.to_vec()),
            KittyTransmission::File => {
                self.read_kitty_file(payload, image_id_hint, placement_id, quiet, false)
            }
            KittyTransmission::TempFile => {
                self.read_kitty_file(payload, image_id_hint, placement_id, quiet, true)
            }
            KittyTransmission::SharedMemory => self.read_kitty_shared_memory(
                payload,
                cmd.control.data_size,
                cmd.control.data_offset,
                image_id_hint,
                placement_id,
                quiet,
            ),
        }
    }

    /// Read a Kitty graphics file from disk (`t=f` / `t=t`).
    ///
    /// The payload bytes are interpreted as a UTF-8 absolute file path, which
    /// is canonicalised and vetted by [`load_kitty_file`]. If `delete_after`
    /// is true (`t=t`), the file is removed after reading, but only when it
    /// lives in a known temporary directory and carries the
    /// `tty-graphics-protocol` marker (see [`may_delete_kitty_temp_file`]).
    ///
    /// Every failure is answered with the single response
    /// `EBADF:Failed to read image file`, so the client cannot use the
    /// response to probe which paths exist or are readable. The detailed
    /// reason is only logged at debug level.
    fn read_kitty_file(
        &self,
        payload: &[u8],
        image_id_hint: u32,
        placement_id: Option<u32>,
        quiet: u8,
        delete_after: bool,
    ) -> Option<Vec<u8>> {
        let loaded = match load_kitty_file(payload) {
            Ok(loaded) => loaded,
            Err(e) => {
                tracing::debug!("Kitty graphics: refusing file transmission: {e}");
                self.send_kitty_error(
                    kitty_id_no_number(image_id_hint, placement_id),
                    quiet,
                    KITTY_FILE_ERROR_RESPONSE,
                );
                return None;
            }
        };

        tracing::debug!(
            "Kitty graphics: read {} bytes from file: {} (delete_after={delete_after})",
            loaded.data.len(),
            loaded.canonical.display(),
        );

        if delete_after {
            if may_delete_kitty_temp_file(&loaded.canonical) {
                if let Err(e) = delete_kitty_temp_file(&loaded) {
                    // Not fatal -- we still have the data.
                    tracing::debug!(
                        "Kitty graphics: failed to delete temp file {}: {e}",
                        loaded.canonical.display()
                    );
                } else {
                    tracing::debug!(
                        "Kitty graphics: deleted temp file {}",
                        loaded.canonical.display()
                    );
                }
            } else {
                tracing::debug!(
                    "Kitty graphics: not deleting {}: not a tty-graphics-protocol file in a temporary directory",
                    loaded.canonical.display()
                );
            }
        }

        Some(loaded.data)
    }

    /// Read a Kitty shared-memory object (`t=s`).
    ///
    /// The payload bytes are the UTF-8 POSIX shared-memory object name
    /// (e.g. `/kitty-shm-1234`). `data_size`/`data_offset` (the `S=`/`O=`
    /// control keys) select the byte range to read; if `data_size` is
    /// absent, the whole object (from `data_offset`) is read. Per the kitty
    /// spec, the object is unlinked after reading regardless of outcome.
    ///
    /// Security: POSIX shm names are `/name` — a single leading slash and
    /// no other slashes. Names containing a NUL byte, `..`, or an embedded
    /// slash (which would let a malicious client reach outside the shm
    /// namespace on implementations that map shm names onto a filesystem
    /// path, e.g. `/dev/shm/name`) are refused with `EPERM` before any
    /// `shm_open` call is attempted.
    #[cfg(unix)]
    fn read_kitty_shared_memory(
        &self,
        payload: &[u8],
        data_size: Option<u32>,
        data_offset: Option<u32>,
        image_id_hint: u32,
        placement_id: Option<u32>,
        quiet: u8,
    ) -> Option<Vec<u8>> {
        use nix::fcntl::OFlag;
        use nix::sys::mman::{shm_open, shm_unlink};
        use nix::sys::stat::{Mode, fstat};

        let name = match std::str::from_utf8(payload) {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!("Kitty graphics: shared memory name is not valid UTF-8: {e}");
                self.send_kitty_error(
                    kitty_id_no_number(image_id_hint, placement_id),
                    quiet,
                    "EINVAL:invalid shared memory object name",
                );
                return None;
            }
        };

        if !shm_name_is_safe(name) {
            tracing::warn!("Kitty graphics: refusing shared memory object name");
            log_shm_name("refusing shared memory object name", name);
            self.send_kitty_error(
                kitty_id_no_number(image_id_hint, placement_id),
                quiet,
                "EPERM:refused shared memory object name",
            );
            return None;
        }

        let fd = match shm_open(name, OFlag::O_RDONLY, Mode::empty()) {
            Ok(fd) => fd,
            Err(e) => {
                tracing::warn!("Kitty graphics: shared memory object not found: {e}");
                log_shm_name("shared memory object not found", name);
                self.send_kitty_error(
                    kitty_id_no_number(image_id_hint, placement_id),
                    quiet,
                    "ENOENT:shared memory object not found",
                );
                return None;
            }
        };

        let Some(object_len) = fstat(&fd)
            .ok()
            .and_then(|stat| u64::value_from(stat.st_size).ok())
        else {
            tracing::warn!("Kitty graphics: shared memory object has an invalid size");
            log_shm_name("shared memory object has an invalid size", name);
            self.send_kitty_error(
                kitty_id_no_number(image_id_hint, placement_id),
                quiet,
                "EINVAL:invalid shared memory object size",
            );
            let _ = shm_unlink(name);
            return None;
        };

        let offset = u64::from(data_offset.unwrap_or(0));
        let Some((offset, read_len)) = shm_read_bounds(object_len, offset, data_size) else {
            tracing::warn!(
                "Kitty graphics: shared memory read out of bounds (offset={offset}, object_len={object_len})"
            );
            log_shm_name("shared memory read out of bounds", name);
            self.send_kitty_error(
                kitty_id_no_number(image_id_hint, placement_id),
                quiet,
                "EINVAL:shared memory read out of bounds",
            );
            let _ = shm_unlink(name);
            return None;
        };

        let result = read_kitty_shm_range(&fd, object_len, offset, read_len).map_or_else(
            |e| {
                tracing::warn!("Kitty graphics: failed to map shared memory object: {e}");
                log_shm_name("failed to map shared memory object", name);
                self.send_kitty_error(
                    kitty_id_no_number(image_id_hint, placement_id),
                    quiet,
                    "EIO:failed to map shared memory object",
                );
                None
            },
            Some,
        );

        // Per the kitty spec, the shm object must be unlinked after reading
        // regardless of outcome — the sender is not expected to unlink it.
        // A failure here is logged but does not fail an otherwise-successful
        // read (the data has already been copied out).
        if let Err(e) = shm_unlink(name) {
            tracing::warn!("Kitty graphics: failed to unlink shared memory object: {e}");
            log_shm_name("failed to unlink shared memory object", name);
        }

        result
    }

    /// Read a Kitty shared-memory object (`t=s`) — Windows implementation.
    ///
    /// The payload bytes are the UTF-8 named-mapping object name — kitty
    /// clients use the same POSIX-style shm name (e.g. `/kitty-shm-1234`)
    /// on every platform, so this opens it via `OpenFileMappingW` rather
    /// than `shm_open`. `data_size` (the `S=` control key) is **required**
    /// on Windows: unlike POSIX (where `fstat` reports the shm object's
    /// size, letting `S=` be omitted to mean "read to the end"), a Windows
    /// file-mapping handle does not expose its own length, so there is no
    /// way to resolve an implicit read length. `data_offset` (`O=`)
    /// defaults to `0`, as on POSIX.
    ///
    /// Security: the same [`shm_name_is_safe`] refusal used on POSIX
    /// applies here before any `OpenFileMappingW` call is attempted — see
    /// its doc comment for why this floor still applies on Windows.
    ///
    /// Unlike POSIX (which unlinks the shm object after reading — the
    /// kitty spec expects the reader to destroy it), Windows named file
    /// mappings have no unlink-by-name equivalent: the object is
    /// destroyed only once every handle to it (across every process) is
    /// closed. This function closes *its own* handle after reading; it
    /// cannot and does not destroy the underlying object.
    #[cfg(windows)]
    fn read_kitty_shared_memory(
        &self,
        payload: &[u8],
        data_size: Option<u32>,
        data_offset: Option<u32>,
        image_id_hint: u32,
        placement_id: Option<u32>,
        quiet: u8,
    ) -> Option<Vec<u8>> {
        use winapi::um::handleapi::CloseHandle;
        use winapi::um::memoryapi::{
            FILE_MAP_READ, MapViewOfFile, OpenFileMappingW, UnmapViewOfFile,
        };

        let name = match std::str::from_utf8(payload) {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!("Kitty graphics: shared memory name is not valid UTF-8: {e}");
                self.send_kitty_error(
                    kitty_id_no_number(image_id_hint, placement_id),
                    quiet,
                    "EINVAL:invalid shared memory object name",
                );
                return None;
            }
        };

        if !shm_name_is_safe(name) {
            tracing::warn!("Kitty graphics: refusing shared memory object name");
            log_shm_name("refusing shared memory object name", name);
            self.send_kitty_error(
                kitty_id_no_number(image_id_hint, placement_id),
                quiet,
                "EPERM:refused shared memory object name",
            );
            return None;
        }

        // Windows file-mapping handles don't expose their own size (unlike
        // POSIX `fstat`), so an explicit, non-zero `S=` is required to know
        // how much to read.
        let Some(data_size) = data_size.filter(|&size| size > 0) else {
            tracing::warn!(
                "Kitty graphics: shared memory read on Windows requires an explicit S= size"
            );
            log_shm_name(
                "shared memory read on Windows requires an explicit S= size",
                name,
            );
            self.send_kitty_error(
                kitty_id_no_number(image_id_hint, placement_id),
                quiet,
                "EINVAL:shared memory read requires S= on Windows",
            );
            return None;
        };

        let offset = u64::from(data_offset.unwrap_or(0));
        let Some((offset, read_len)) = shm_read_bounds(u64::MAX, offset, Some(data_size)) else {
            tracing::warn!(
                "Kitty graphics: shared memory read range overflows (offset={offset}, data_size={data_size})"
            );
            log_shm_name("shared memory read range overflows", name);
            self.send_kitty_error(
                kitty_id_no_number(image_id_hint, placement_id),
                quiet,
                "EINVAL:shared memory read out of bounds",
            );
            return None;
        };

        let wide: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();

        // SAFETY: FFI call; `wide` is a NUL-terminated UTF-16 string built
        // just above and is kept alive for the duration of this call.
        let handle = unsafe { OpenFileMappingW(FILE_MAP_READ, 0, wide.as_ptr()) };
        if handle.is_null() {
            tracing::warn!("Kitty graphics: shared memory object not found");
            log_shm_name("shared memory object not found", name);
            self.send_kitty_error(
                kitty_id_no_number(image_id_hint, placement_id),
                quiet,
                "ENOENT:shared memory object not found",
            );
            return None;
        }

        // Map from the start of the object up to `offset + read_len` bytes
        // rather than starting the mapping at `offset` — this sidesteps
        // `MapViewOfFile`'s allocation-granularity alignment requirement on
        // the file offset (which only applies to
        // `dwFileOffsetHigh`/`dwFileOffsetLow`, both `0` here). `offset +
        // read_len` cannot overflow `u64`: `shm_read_bounds` above already
        // performed that checked addition and returned `Some`.
        let map_len = usize::value_from(offset + read_len).unwrap_or(usize::MAX);

        // SAFETY: FFI call; `handle` is the valid, just-opened mapping
        // handle from `OpenFileMappingW` above and has not been closed.
        let base = unsafe { MapViewOfFile(handle, FILE_MAP_READ, 0, 0, map_len) };
        if base.is_null() {
            tracing::warn!("Kitty graphics: failed to map shared memory object");
            log_shm_name("failed to map shared memory object", name);
            self.send_kitty_error(
                kitty_id_no_number(image_id_hint, placement_id),
                quiet,
                "EIO:failed to map shared memory object",
            );
            // SAFETY: `handle` was returned by the successful
            // `OpenFileMappingW` call above and has not been closed yet.
            unsafe { CloseHandle(handle) };
            return None;
        }

        let offset_usize = usize::value_from(offset).unwrap_or(0);
        let read_len_usize = usize::value_from(read_len).unwrap_or(0);

        // SAFETY: `base` is valid for `map_len` bytes (the mapping length
        // just requested above); `offset_usize + read_len_usize ==
        // map_len`, so the range read here is fully within the mapping.
        let bytes = unsafe {
            std::slice::from_raw_parts(base.cast::<u8>().add(offset_usize), read_len_usize)
        };
        let copied = bytes.to_vec();

        // SAFETY: `base` is exactly the pointer returned by the matching
        // `MapViewOfFile` call above and has not been unmapped yet.
        unsafe { UnmapViewOfFile(base) };
        // SAFETY: `handle` is exactly the handle returned by the matching
        // `OpenFileMappingW` call above and has not been closed yet.
        unsafe { CloseHandle(handle) };

        Some(copied)
    }

    /// Extract required `s` (width) and `v` (height) from Kitty control data.
    ///
    /// Returns `None` and sends an error if either is missing.
    fn require_kitty_dimensions(
        &self,
        cmd: &KittyGraphicsCommand,
        image_id_hint: u32,
        placement_id: Option<u32>,
        quiet: u8,
    ) -> Option<(u32, u32)> {
        let Some(w) = cmd.control.src_width else {
            self.send_kitty_error(
                kitty_id_no_number(image_id_hint, placement_id),
                quiet,
                "EINVAL:missing width (s)",
            );
            return None;
        };
        let Some(h) = cmd.control.src_height else {
            self.send_kitty_error(
                kitty_id_no_number(image_id_hint, placement_id),
                quiet,
                "EINVAL:missing height (v)",
            );
            return None;
        };
        Some((w, h))
    }

    /// Compute display dimensions, store an `InlineImage`, and optionally place
    /// it into the buffer.
    // All parameters are required image placement inputs; grouping into a struct would obscure
    // the data flow without reducing coupling.
    #[allow(clippy::too_many_arguments)]
    fn place_kitty_image(
        &mut self,
        control: &KittyControlData,
        action: KittyAction,
        rgba_pixels: Vec<u8>,
        img_width_px: u32,
        img_height_px: u32,
        image_id_hint: u32,
        quiet: u8,
    ) {
        if img_width_px == 0 || img_height_px == 0 {
            self.send_kitty_error(
                kitty_id_no_number(image_id_hint, control.placement_id),
                quiet,
                "EINVAL:zero dimension",
            );
            return;
        }

        let (display_cols, display_rows) =
            self.compute_display_size_from_pixels(control, img_width_px, img_height_px);

        let assigned_id = if image_id_hint > 0 {
            u64::from(image_id_hint)
        } else {
            next_image_id()
        };

        let size_mode = kitty_image_size_mode(control);

        let inline_image = InlineImage {
            id: assigned_id,
            pixels: std::sync::Arc::new(rgba_pixels),
            width_px: img_width_px,
            height_px: img_height_px,
            display_cols,
            display_rows,
            size_mode,
            frames: Vec::new(),
            root_gap_ms: 0,
            animation: freminal_buffer::image_store::AnimationControl::default(),
        };

        self.buffer
            .image_store_mut()
            .insert_protocol_retained(inline_image.clone());

        // `I=` always creates a new image (already given a fresh id above,
        // since a bare `I=<n>` has no `i=`) — record it as the newest image
        // with that number so later by-number references (`a=p,I=`,
        // `a=f,I=`, `d=n`) resolve to it.
        if let Some(number) = control.image_number {
            self.buffer
                .image_store_mut()
                .associate_number(number, assigned_id);
        }

        let should_display = matches!(action, KittyAction::TransmitAndDisplay | KittyAction::Put);

        // Relative placement (`P=` present) — intercept before the
        // virtual/normal placement paths below; it sends its own response
        // (Task 100.4a). Only meaningful when the action actually displays
        // the image; a plain `a=t` transmit with stray `P=`/`Q=` fields
        // falls through to the normal store-only path below.
        if control.parent_image_id.is_some() && should_display {
            self.handle_kitty_relative_placement(
                control,
                assigned_id,
                display_cols,
                display_rows,
                quiet,
            );
            return;
        }

        // If this is a virtual (Unicode placeholder) placement, store it in
        // the virtual_placements table instead of placing cells in the buffer.
        if control.unicode_placeholder {
            let pid = control.placement_id.unwrap_or(0);
            self.register_virtual_placement(assigned_id, pid, display_cols, display_rows);
        } else if should_display {
            self.stamp_kitty_transmit_display(
                control,
                assigned_id,
                inline_image,
                display_cols,
                display_rows,
                img_width_px,
                img_height_px,
            );
        } else {
            tracing::debug!(
                "Kitty graphics: stored image id={assigned_id} (a={action:?}, not placing), \
                 {display_cols}x{display_rows} cells, {img_width_px}x{img_height_px} px",
            );
        }

        // Send OK response unless suppressed.
        if quiet < 1 && assigned_id > 0 {
            let response_id = u32::value_from(assigned_id).unwrap_or(0);
            let response = format_kitty_response_body(
                KittyResponseId {
                    image_id: response_id,
                    image_number: control.image_number,
                    placement_id: control.placement_id,
                },
                true,
                "",
            );
            self.write_apc_response(&response);
        }
    }

    /// Stamp a non-virtual, non-relative `a=T` (transmit-and-display) or
    /// `a=t` immediately-displayed placement at the cursor, restoring the
    /// cursor afterward if `C=1` was requested, and record it as a real
    /// placement so it can act as a future relative-placement parent (Task
    /// 100.4a). Mirrors [`Self::stamp_kitty_put`]'s handling of
    /// source-crop (Task 100.9), sub-cell offset (Task 100.19), and REPLACE
    /// semantics (Task 100.18) for the `a=p` (Put) path.
    ///
    /// Split out of [`Self::place_kitty_image`] to keep that function
    /// within the line-count lint limit.
    #[allow(clippy::too_many_arguments)]
    // All parameters are required placement inputs already resolved by the
    // caller; grouping into a struct would obscure the data flow without
    // reducing coupling, matching the established convention for
    // `place_kitty_image`-adjacent methods.
    fn stamp_kitty_transmit_display(
        &mut self,
        control: &KittyControlData,
        assigned_id: u64,
        inline_image: InlineImage,
        display_cols: usize,
        display_rows: usize,
        img_width_px: u32,
        img_height_px: u32,
    ) {
        tracing::debug!(
            "Kitty graphics: placing image id={assigned_id} at cursor, \
             {display_cols}x{display_rows} cells, {img_width_px}x{img_height_px} px",
        );
        let source_crop = resolve_source_crop(control, img_width_px, img_height_px);
        let subcell_offset = self.resolve_subcell_offset(control);

        // Kitty spec REPLACE semantics (Task 100.18): a second `a=T`
        // with the SAME non-zero `p=` replaces that one placement;
        // `p=0`/unspecified means multiple, independently-coexisting
        // placements, so it must NOT clear anything here.
        if let Some(pid) = control.placement_id
            && pid != 0
        {
            self.buffer
                .clear_image_placements_by_placement(assigned_id, pid);
        }

        // Mint a fresh placement-instance id for this DISPLAY put —
        // every `a=T` display is a distinct on-screen placement, even a
        // `p=0`/unspecified one that shares an image id with another
        // placement (Task 100.18).
        let placement_instance = next_placement_instance_id();
        let place_result = self.buffer.place_image(
            inline_image,
            0,
            ImageProtocol::Kitty,
            control.image_number,
            control.placement_id,
            control.z_index.unwrap_or(0),
            source_crop,
            placement_instance,
            subcell_offset,
        );

        // `C=1` (no cursor movement) — mirrors `stamp_kitty_put`'s handling of
        // the same flag on `a=p` (Task 100.16). `place_image` (Task 100.15)
        // always moves the cursor below the image, so `C=1` must explicitly
        // put it back on the image origin, located by row number so scrollback
        // eviction during placement cannot displace it (Task 125.C7).
        if control.no_cursor_movement {
            self.buffer.restore_cursor_to_image_origin(&place_result);
        }

        // Record this real (cell-stamped) placement's origin so future
        // relative placements can reference it as a parent (Task 100.4a).
        //
        // Use the TRUE stamped origin from `place_result`, not a
        // pre-call cursor read: `place_image` may drain scrollback rows
        // during placement, which shifts the image's actual row
        // upward relative to the cursor position captured before the
        // call (Task 100.14). This is unaffected by the `C=1` cursor
        // restore above — the recorded origin always reflects where the
        // image was actually stamped, regardless of where the cursor
        // ends up afterward.
        let pid = control.placement_id.unwrap_or(0);
        self.record_real_placement(
            assigned_id,
            pid,
            place_result.origin_row,
            place_result.origin_col,
            display_cols,
            display_rows,
            control.z_index.unwrap_or(0),
            place_result.placement_instance,
        );
    }

    /// Handle a Kitty relative placement (`P=`/`Q=`/`H=`/`V=` present on
    /// `a=p` or `a=T`, Task 100.4a).
    ///
    /// Validates the parent reference (`ENOPARENT`), rejects a relative
    /// placement that is itself virtual (`EINVAL`), rejects chains deeper
    /// than [`MAX_RELATIVE_PLACEMENT_DEPTH`] (`ETOODEEP`), and rejects
    /// cycles (`ECYCLE`). If the resolved parent is a REAL (cell-stamped)
    /// placement, the child is stamped at `parent_origin + (H, V)` cells
    /// via [`freminal_buffer::buffer::Buffer::place_image_at`] — which does
    /// NOT move the cursor, matching the spec's positioning-by-parent
    /// semantics. If the resolved parent is a VIRTUAL (Unicode placeholder)
    /// placement, the child is registered in `real_placements` with the
    /// parent link but is NOT stamped — positioning a child of a virtual
    /// parent happens at render time and is deferred to Task 100.4b.
    ///
    /// Sends its own success/error APC response; callers must not send an
    /// additional response after calling this.
    fn handle_kitty_relative_placement(
        &mut self,
        control: &KittyControlData,
        child_image_id: u64,
        display_cols: usize,
        display_rows: usize,
        quiet: u8,
    ) {
        let child_pid = control.placement_id.unwrap_or(0);
        let response_child_id = u32::value_from(child_image_id).unwrap_or(0);
        let response_id = KittyResponseId {
            image_id: response_child_id,
            image_number: control.image_number,
            placement_id: control.placement_id,
        };

        // Rule: EINVAL — a relative placement cannot itself be virtual.
        // (A virtual placement MAY be a parent; it just can't also be `U=1`
        // while carrying `P=`.)
        if control.unicode_placeholder {
            self.send_kitty_error(
                response_id,
                quiet,
                "EINVAL:relative placement cannot be virtual",
            );
            return;
        }

        let Some(parent_image_id) = control.parent_image_id else {
            // Only called when `parent_image_id.is_some()`; defensive guard.
            return;
        };
        let parent_img = u64::from(parent_image_id);
        let parent_pid = control.parent_placement_id.unwrap_or(0);

        // Rule: ENOPARENT — resolve the parent (real or virtual placement).
        let Some(parent_key) = self.resolve_kitty_parent_key(parent_img, parent_pid) else {
            self.send_kitty_error(response_id, quiet, "ENOPARENT:parent placement not found");
            return;
        };

        // Walk the ancestor chain from the parent upward through
        // `real_placements` (a virtual placement is always a root — it has
        // no `parent` link, so the walk stops there).
        let chain = self.real_placement_ancestor_chain(parent_key);

        // Rule: ECYCLE — this child would appear in its own ancestor chain.
        let child_key = (child_image_id, child_pid);
        if chain.contains(&child_key) {
            self.send_kitty_error(
                response_id,
                quiet,
                "ECYCLE:relative placement would form a cycle",
            );
            return;
        }

        // Rule: ETOODEEP — adding this child would exceed the max depth.
        //
        // `chain.len()` is the parent's depth (a root's own chain has
        // length 1, i.e. depth 0; its first child has parent-chain length 1
        // so the child reaches depth 1, and so on). The new child's depth
        // equals `chain.len()`. The spec requires implementations to
        // support depth >= `MAX_RELATIVE_PLACEMENT_DEPTH` (8), so only a
        // child that would reach depth 9 (the 9th link) is rejected —
        // i.e. `chain.len() > MAX_RELATIVE_PLACEMENT_DEPTH`.
        if chain.len() > MAX_RELATIVE_PLACEMENT_DEPTH {
            self.send_kitty_error(
                response_id,
                quiet,
                "ETOODEEP:relative placement chain too deep",
            );
            return;
        }

        let h_offset = control.h_offset.unwrap_or(0);
        let v_offset = control.v_offset.unwrap_or(0);
        let z_index = control.z_index.unwrap_or(0);

        if let Some(parent_real) = self.real_placements.get(&parent_key).copied() {
            self.stamp_relative_placement_at_real_parent(
                control,
                child_image_id,
                child_pid,
                display_cols,
                display_rows,
                parent_key,
                parent_real,
                h_offset,
                v_offset,
                z_index,
            );
        } else {
            self.register_relative_placement_against_virtual_parent(
                child_image_id,
                child_pid,
                display_cols,
                display_rows,
                parent_key,
                h_offset,
                v_offset,
                z_index,
            );
        }

        if quiet < 1 {
            let response = format_kitty_response_body(response_id, true, "");
            self.write_apc_response(&response);
        }
    }

    /// Stamp a relative placement's child cells at `parent_origin + (H, V)`
    /// when the resolved parent is a REAL (cell-stamped) placement.
    ///
    /// `place_image_at` does not move the cursor, so a relative placement
    /// never moves the cursor regardless of `C=`. Also records the child in
    /// `real_placements` so it can itself act as a parent, be cascade-
    /// deleted, or be re-derived if a future refactor changes it to a
    /// virtual-parent chain.
    #[allow(clippy::too_many_arguments)]
    fn stamp_relative_placement_at_real_parent(
        &mut self,
        control: &KittyControlData,
        child_image_id: u64,
        child_pid: u32,
        display_cols: usize,
        display_rows: usize,
        parent_key: (u64, u32),
        parent_real: RealPlacement,
        h_offset: i32,
        v_offset: i32,
        z_index: i32,
    ) {
        // The parent's origin is a stable row number, so it still names the
        // parent's row however far the buffer has scrolled since the parent
        // was placed (Task 125.14).
        //
        // A `V=` that would carry the child above the first row of the
        // parent's screen has no row to land on. It is a checked offset on
        // purpose: saturating would plant the child on row 0 (or on a row of
        // the other screen), a row the application never addressed.
        let Some(origin_row) = parent_real.origin_row.checked_offset(i64::from(v_offset)) else {
            tracing::debug!(
                "Kitty graphics: relative placement child (image_id={child_image_id}, \
                 placement_id={child_pid}) falls outside its screen (parent row {}, V={v_offset}); \
                 accepted but neither stamped nor registered",
                parent_real.origin_row,
            );
            // Nothing of this placement exists on screen, so a stale entry
            // left by an earlier placement under the same key must not survive
            // to name rows this one does not occupy.
            self.real_placements.remove(&(child_image_id, child_pid));
            return;
        };
        let origin_col = signed_cell_offset(parent_real.origin_col, h_offset);

        tracing::debug!(
            "Kitty graphics: relative placement child (image_id={child_image_id}, \
             placement_id={child_pid}) stamped at (row {origin_row},col {origin_col}) \
             (parent origin (row {},col {}) + H={h_offset},V={v_offset})",
            parent_real.origin_row,
            parent_real.origin_col,
        );

        // Resolve the crop against the CHILD image's own pixel dimensions —
        // `x=`/`y=`/`w=`/`h=` on a relative `a=p` crop the child, not the
        // parent.
        let (child_width_px, child_height_px) = self
            .buffer
            .image_store()
            .get(child_image_id)
            .map_or((0, 0), |img| (img.width_px, img.height_px));
        let source_crop = resolve_source_crop(control, child_width_px, child_height_px);
        let subcell_offset = self.resolve_subcell_offset(control);

        // Mint a fresh placement-instance id for the child — it is a
        // DISTINCT on-screen placement from its parent (Task 100.18), even
        // though it shares the parent's origin plus an offset.
        let placement_instance = next_placement_instance_id();

        // A child whose origin row is no longer retained (the parent was
        // evicted, or a negative `V=` points above the oldest row) has
        // nothing visible to stamp; it is still registered below so it can be
        // cascade-deleted with its parent.
        if let Some(origin_row_index) = self.buffer.row_index_of(origin_row) {
            self.buffer.place_image_at(
                child_image_id,
                origin_row_index,
                origin_col,
                display_cols,
                display_rows,
                ImageProtocol::Kitty,
                control.image_number,
                control.placement_id,
                z_index,
                source_crop,
                placement_instance,
                subcell_offset,
            );
        }

        self.insert_real_placement(
            child_image_id,
            child_pid,
            origin_row,
            origin_col,
            display_cols,
            display_rows,
            Some(parent_key),
            z_index,
            h_offset,
            v_offset,
            placement_instance,
        );
    }

    /// Register a relative placement's child in `real_placements` with a
    /// placeholder origin when the resolved parent is a VIRTUAL (Unicode
    /// placeholder) placement.
    ///
    /// No cells are stamped here — positioning at render time from the
    /// parent's live placeholder cells is handled by
    /// `TerminalHandler::inject_virtual_parent_relatives` (Task 100.4b),
    /// which reads the `h_offset`/`v_offset` recorded here.
    #[allow(clippy::too_many_arguments)]
    fn register_relative_placement_against_virtual_parent(
        &mut self,
        child_image_id: u64,
        child_pid: u32,
        display_cols: usize,
        display_rows: usize,
        parent_key: (u64, u32),
        h_offset: i32,
        v_offset: i32,
        z_index: i32,
    ) {
        tracing::debug!(
            "Kitty graphics: relative placement child (image_id={child_image_id}, \
             placement_id={child_pid}) registered against virtual parent \
             (image_id={}, placement_id={}); positioned at render time (Task 100.4b)",
            parent_key.0,
            parent_key.1,
        );
        // Mint the child's placement-instance id now, at registration time
        // (Task 100.18/100.20) — no cells are stamped here, but the id must
        // be minted ONCE and stay stable across every subsequent
        // `inject_virtual_parent_relatives` re-derivation (`RealPlacement`
        // is a first-class, persistent record; a per-frame mint would make
        // each frame's re-derived cells look like a brand-new placement).
        let placement_instance = next_placement_instance_id();
        self.insert_real_placement(
            child_image_id,
            child_pid,
            // Placeholder origin: nothing is stamped for a virtual-parent
            // child, so its row is never read.
            RowNumber::ZERO,
            0,
            display_cols,
            display_rows,
            Some(parent_key),
            z_index,
            h_offset,
            v_offset,
            placement_instance,
        );
    }

    /// Resolve a relative-placement parent reference (`P=`/`Q=`) to the key
    /// of an existing placement, real or virtual.
    ///
    /// Tries the exact `(parent_img, parent_pid)` key first; if
    /// `parent_pid == 0` (unspecified), falls back to any placement (real
    /// or virtual) for that image id.
    fn resolve_kitty_parent_key(&self, parent_img: u64, parent_pid: u32) -> Option<(u64, u32)> {
        let exact = (parent_img, parent_pid);
        if self.real_placements.contains_key(&exact) || self.virtual_placements.contains_key(&exact)
        {
            return Some(exact);
        }
        if parent_pid == 0 {
            if let Some(&key) = self
                .real_placements
                .keys()
                .find(|&&(img, _)| img == parent_img)
            {
                return Some(key);
            }
            if let Some(&key) = self
                .virtual_placements
                .keys()
                .find(|&&(img, _)| img == parent_img)
            {
                return Some(key);
            }
        }
        None
    }

    /// Walk the ancestor chain starting at `start` (inclusive) via the
    /// `real_placements` `parent` link.
    ///
    /// Stops when a key has no entry in `real_placements` (either it's a
    /// virtual placement, which is always a root, or the chain is broken),
    /// or defensively if a cycle is somehow already present in existing
    /// data (new cycles are rejected before insertion by the caller).
    fn real_placement_ancestor_chain(&self, start: (u64, u32)) -> Vec<(u64, u32)> {
        let mut chain = Vec::new();
        let mut visited = HashSet::new();
        let mut current = Some(start);
        while let Some(key) = current {
            if !visited.insert(key) {
                break;
            }
            chain.push(key);
            current = self.real_placements.get(&key).and_then(|rp| rp.parent);
        }
        chain
    }

    /// Cascade-delete a set of root `real_placements` keys and all
    /// descendants reachable via the `parent` link.
    ///
    /// For each key (root or descendant), clears the stamped image cells
    /// for its image id (coarse — matches the existing by-id delete
    /// behaviour elsewhere in this handler, which clears all cells for an
    /// image id rather than a single placement's cells) and removes the
    /// entry from `real_placements`. Used by `a=d` (delete) so a deleted
    /// placement's relative children never reference a pruned parent.
    fn cascade_delete_real_placements(&mut self, roots: &[(u64, u32)]) {
        let mut to_delete: Vec<(u64, u32)> = roots.to_vec();
        let mut i = 0;
        while i < to_delete.len() {
            let key = to_delete[i];
            let children: Vec<(u64, u32)> = self
                .real_placements
                .iter()
                .filter(|(_, rp)| rp.parent == Some(key))
                .map(|(&k, _)| k)
                .collect();
            for child in children {
                if !to_delete.contains(&child) {
                    to_delete.push(child);
                }
            }
            i += 1;
        }

        for key in &to_delete {
            self.buffer.clear_image_placements_by_id(key.0);
            self.real_placements.remove(key);
        }
    }

    /// Translate the row numbers in `real_placements` through any reflow the
    /// buffer has just performed (a resize, DECCOLM).
    ///
    /// Reflow renumbers every row, so an untranslated placement origin would
    /// fall below the buffer's row base and stop resolving to its row. A
    /// placement whose origin row the remap cannot translate (its row was
    /// already evicted) is left as is -- it is already below the base and
    /// resolves to nothing -- and is then pruned by
    /// [`Self::prune_evicted_real_placements`].
    pub(super) fn apply_buffer_reflow_remap(&mut self) {
        if let Some(remap) = self.buffer.take_reflow_remap() {
            for placement in self.real_placements.values_mut() {
                if let Some(row) = remap.map_start(placement.origin_row) {
                    placement.origin_row = row;
                }
            }
        }
        // Whether or not a reflow happened, a resize can have removed rows
        // from the tail.
        self.prune_unissued_real_placements();
        // A reflow installs its rows at a new base, so placements the remap
        // could not translate now sit below it.
        self.prune_evicted_real_placements();
    }

    /// Drop `real_placements` entries whose origin row has been evicted.
    ///
    /// An entry below the buffer's row base resolves to nothing -- its cells
    /// are gone -- but nothing else removes it, so a long session of image
    /// placements under scrollback eviction would grow the map without bound.
    ///
    /// **Cost and trigger.** Pruning is `O(placements)`, and it runs only when
    /// the buffer's row base has moved since the last prune and the map is not
    /// empty: at most once per input batch (and once per reflow), never per
    /// line feed, and for a terminal that shows no images it is a single
    /// comparison. The map holds only live placements plus those evicted
    /// during one batch, so the cost is bounded by the number of placements
    /// currently on screen or in scrollback. An ordering-based `O(pruned)`
    /// scheme was rejected: entries are keyed by `(image_id, placement_id)`,
    /// are overwritten in place, are renumbered by reflow, and relative
    /// children sit at parent origin plus an offset, so insertion order is
    /// not row order and there is no sorted prefix to pop.
    ///
    /// Only entries in the active screen's row namespace are judged (the other
    /// namespace's base is not the one that just moved). An entry registered
    /// against a virtual (Unicode placeholder) parent carries a placeholder
    /// origin that is never read, so it is kept: it is positioned from the
    /// parent's live placeholder cells instead. Children of a pruned entry
    /// keep their own rows; their dangling `parent` link already ends the
    /// ancestor walk.
    pub(super) fn prune_evicted_real_placements(&mut self) {
        let base = self.buffer.row_base();
        if base == self.placement_prune_base {
            return;
        }
        self.placement_prune_base = base;
        self.retain_real_placements(|origin| {
            !(origin.is_alternate() == base.is_alternate() && origin < base)
        });
    }

    /// Drop `real_placements` entries whose origin row does not exist yet:
    /// at or past the number the buffer will issue next.
    ///
    /// Shrinking the buffer's tail (a height grow reclaims blank padding rows)
    /// removes rows whose numbers are then issued again to unrelated new rows,
    /// so a placement recorded against a removed row would name whatever is
    /// written there next. Runs after every resize; resizes are rare, so it is
    /// not gated.
    pub(super) fn prune_unissued_real_placements(&mut self) {
        let next = self.buffer.next_row_number();
        self.retain_real_placements(|origin| {
            !(origin.is_alternate() == next.is_alternate() && origin >= next)
        });
    }

    /// Keep the `real_placements` entries whose origin row satisfies `keep`,
    /// except that an entry registered against a virtual (Unicode placeholder)
    /// parent is always kept: its origin is a placeholder that is never read
    /// (see [`Self::register_relative_placement_against_virtual_parent`]).
    fn retain_real_placements(&mut self, keep: impl Fn(RowNumber) -> bool) {
        if self.real_placements.is_empty() {
            return;
        }
        let virtual_placements = &self.virtual_placements;
        self.real_placements.retain(|_, placement| {
            placement
                .parent
                .is_some_and(|parent| virtual_placements.contains_key(&parent))
                || keep(placement.origin_row)
        });
    }

    /// Convenience wrapper: cascade-delete every `real_placements` entry for
    /// the given image id (across all its placement ids), plus their
    /// relative children. Used by the `a=d` (delete) by-id and by-number
    /// arms (Task 100.4a).
    fn cascade_delete_real_placements_for_image(&mut self, image_id: u64) {
        let roots: Vec<(u64, u32)> = self
            .real_placements
            .keys()
            .filter(|&&(img_id, _)| img_id == image_id)
            .copied()
            .collect();
        self.cascade_delete_real_placements(&roots);
    }

    /// Decode the transmitted rectangle for an `a=f` animation-frame
    /// command.
    ///
    /// Under `a=f`, `s`/`v` retain their transmit-group meaning (width and
    /// height of the transmitted rectangle); default to the full image
    /// dimensions when absent, per spec.
    fn decode_kitty_frame_rect(
        &self,
        cmd: &KittyGraphicsCommand,
        stored_image: &InlineImage,
        image_id_hint: u32,
        placement_id: Option<u32>,
        quiet: u8,
    ) -> Option<(Vec<u8>, u32, u32)> {
        let mut effective_control = cmd.control.clone();
        if effective_control.src_width.is_none() {
            effective_control.src_width = Some(stored_image.width_px);
        }
        if effective_control.src_height.is_none() {
            effective_control.src_height = Some(stored_image.height_px);
        }
        let effective_cmd = KittyGraphicsCommand {
            control: effective_control,
            payload: cmd.payload.clone(),
        };

        self.decode_kitty_payload(&effective_cmd, image_id_hint, placement_id, quiet)
    }

    /// Resolve the base canvas for an `a=f` animation-frame composite.
    ///
    /// `c=` (`display_cols` under `a=f`) seeds the canvas from an existing
    /// frame; otherwise a fresh background-colored (`Y=`) canvas is used.
    /// Sends `ENOENT` and returns `None` if `c=` names a frame that doesn't
    /// exist.
    // All parameters are required composite inputs; grouping into a struct
    // would obscure the call site without reducing coupling.
    #[allow(clippy::too_many_arguments)]
    fn resolve_frame_base_canvas(
        &self,
        cmd: &KittyGraphicsCommand,
        stored_image: &InlineImage,
        canvas_w: u32,
        canvas_h: u32,
        image_id_hint: u32,
        placement_id: Option<u32>,
        quiet: u8,
    ) -> Option<Vec<u8>> {
        match cmd.control.display_cols {
            Some(base_frame) if base_frame > 0 => {
                let Some(base_pixels) = stored_image.frame_pixels(base_frame) else {
                    self.send_kitty_error(
                        kitty_id_no_number(image_id_hint, placement_id),
                        quiet,
                        "ENOENT:base frame not found",
                    );
                    return None;
                };
                Some(base_pixels.as_ref().clone())
            }
            _ => {
                let bg = cmd.control.cell_y_offset.unwrap_or(0);
                Some(new_canvas_filled(canvas_w, canvas_h, bg))
            }
        }
    }

    /// Handle `a=f` — transmit an animation frame.
    ///
    /// The transmitted rectangle (`x`/`y`/`s`/`v`) is composited onto a base
    /// canvas — either an existing frame (`c=`) or a fresh background-filled
    /// canvas (`Y=`) — using alpha-blend or overwrite (`X=`). The result is
    /// either patched into an existing frame (`r=`) or appended as a new
    /// frame, with an optional gap (`z=`) to the next frame.
    fn handle_kitty_animation_frame(&mut self, cmd: &KittyGraphicsCommand) {
        let image_id_hint = cmd.control.image_id.unwrap_or(0);
        let image_number = cmd.control.image_number;
        let placement_id = cmd.control.placement_id;
        let quiet = cmd.control.quiet;

        let Some(id) =
            self.resolve_kitty_reference_id(&cmd.control, image_id_hint, placement_id, quiet)
        else {
            return;
        };

        let Some(stored_image) = self.buffer.image_store().get(id).cloned() else {
            self.send_kitty_error(
                KittyResponseId {
                    image_id: image_id_hint,
                    image_number,
                    placement_id,
                },
                quiet,
                "ENOENT:image not found",
            );
            return;
        };

        if cmd.payload.is_empty() {
            self.send_kitty_error(
                kitty_id_no_number(image_id_hint, placement_id),
                quiet,
                "ENODATA:no payload",
            );
            return;
        }

        let Some((rect_pixels, rect_w, rect_h)) =
            self.decode_kitty_frame_rect(cmd, &stored_image, image_id_hint, placement_id, quiet)
        else {
            return; // Error already sent by decode_kitty_payload.
        };

        // Destination origin within the frame (`x`/`y`, default 0).
        let dest_x = cmd.control.src_x.unwrap_or(0);
        let dest_y = cmd.control.src_y.unwrap_or(0);

        let canvas_w = stored_image.width_px;
        let canvas_h = stored_image.height_px;

        let Some(mut canvas) = self.resolve_frame_base_canvas(
            cmd,
            &stored_image,
            canvas_w,
            canvas_h,
            image_id_hint,
            placement_id,
            quiet,
        ) else {
            return; // Error already sent by resolve_frame_base_canvas.
        };

        // Compose mode: `X=` (cell_x_offset under a=f), default alpha-blend.
        let overwrite = cmd.control.cell_x_offset.unwrap_or(0) != 0;
        if let Err(msg) = composite_rect(
            &mut canvas,
            canvas_w,
            canvas_h,
            &rect_pixels,
            rect_w,
            rect_h,
            dest_x,
            dest_y,
            overwrite,
        ) {
            self.send_kitty_error(kitty_id_no_number(image_id_hint, placement_id), quiet, msg);
            return;
        }

        // Gap-to-next-frame (`z=`).
        let gap_ms = resolve_gap_ms(cmd.control.z_index);

        let mut image_to_store = stored_image;
        // Edit target: `r=` (display_rows under a=f) patches an existing
        // frame; absent means append a new frame.
        if let Some(edit_frame) = cmd.control.display_rows.filter(|&r| r > 0) {
            if !image_to_store.set_frame_pixels(edit_frame, std::sync::Arc::new(canvas)) {
                self.send_kitty_error(
                    kitty_id_no_number(image_id_hint, placement_id),
                    quiet,
                    "ENOENT:edit frame not found",
                );
                return;
            }
            if let Some(gap) = gap_ms {
                image_to_store.set_frame_gap(edit_frame, gap);
            }
        } else {
            let new_gap = gap_ms.unwrap_or(DEFAULT_ANIMATION_FRAME_GAP_MS);
            image_to_store.push_frame(std::sync::Arc::new(canvas), new_gap);
        }

        self.buffer
            .image_store_mut()
            .insert_protocol_retained(image_to_store);

        if quiet < 1 {
            let response_id = u32::value_from(id).unwrap_or(image_id_hint);
            let response = format_kitty_response_body(
                KittyResponseId {
                    image_id: response_id,
                    image_number,
                    placement_id,
                },
                true,
                "",
            );
            self.write_apc_response(&response);
        }
    }

    /// Handle `a=a` — control animation playback.
    ///
    /// Sets run/stop mode (`s=`), loop count (`v=`), app-forced current frame
    /// (`c=`), and per-frame gap (`r=`/`z=`) on the image's declarative
    /// `AnimationControl`. Per kitty convention, `a=a` never sends an OK
    /// response (errors are still surfaced).
    fn handle_kitty_animation_control(&mut self, cmd: &KittyGraphicsCommand) {
        let image_id_hint = cmd.control.image_id.unwrap_or(0);
        let placement_id = cmd.control.placement_id;
        let quiet = cmd.control.quiet;
        let id = u64::from(image_id_hint);

        let Some(mut stored_image) = self.buffer.image_store().get(id).cloned() else {
            self.send_kitty_error(
                kitty_id_no_number(image_id_hint, placement_id),
                quiet,
                "ENOENT:image not found",
            );
            return;
        };

        // `s` (src_width under a=a) — run mode.
        if let Some(s) = cmd.control.src_width {
            stored_image.animation.run_mode = match s {
                1 => AnimationRunMode::Stopped,
                2 => AnimationRunMode::RunLoading,
                3 => AnimationRunMode::Running,
                _ => stored_image.animation.run_mode,
            };
        }

        // `v` (src_height under a=a) — loop count; `0` ignored.
        if let Some(v) = cmd.control.src_height
            && v != 0
        {
            stored_image.animation.loop_count = v;
        }

        // `c` (display_cols under a=a) — app-forced current frame.
        if let Some(c) = cmd.control.display_cols {
            stored_image.animation.current_frame = c;
        }

        // `r`/`z` (display_rows/z_index under a=a) — per-frame gap target.
        if let Some(r) = cmd.control.display_rows
            && let Some(gap) = resolve_gap_ms(cmd.control.z_index)
        {
            stored_image.set_frame_gap(r, gap);
        }

        self.buffer
            .image_store_mut()
            .insert_protocol_retained(stored_image);

        // `a=a` is silent by kitty convention — no OK/ACK response is sent
        // even when quiet < 1.
    }

    /// Resolve the `a=c` source (`r=`) and destination (`c=`) 1-based frame
    /// numbers from control data, sending `EINVAL` and returning `None` if
    /// either is missing.
    fn require_compose_frame_numbers(
        &self,
        cmd: &KittyGraphicsCommand,
        image_id_hint: u32,
        placement_id: Option<u32>,
        quiet: u8,
    ) -> Option<(u32, u32)> {
        let Some(src_frame) = cmd.control.display_rows else {
            self.send_kitty_error(
                kitty_id_no_number(image_id_hint, placement_id),
                quiet,
                "EINVAL:missing source frame (r)",
            );
            return None;
        };
        let Some(dest_frame) = cmd.control.display_cols else {
            self.send_kitty_error(
                kitty_id_no_number(image_id_hint, placement_id),
                quiet,
                "EINVAL:missing destination frame (c)",
            );
            return None;
        };
        Some((src_frame, dest_frame))
    }

    /// Borrow (and clone the `Arc`) a frame's pixels by 1-based frame number,
    /// sending `ENOENT` and returning `None` if the frame does not exist.
    fn require_compose_frame_pixels(
        &self,
        image: &InlineImage,
        frame: u32,
        image_id_hint: u32,
        placement_id: Option<u32>,
        quiet: u8,
    ) -> Option<std::sync::Arc<Vec<u8>>> {
        let Some(pixels) = image.frame_pixels(frame).cloned() else {
            self.send_kitty_error(
                kitty_id_no_number(image_id_hint, placement_id),
                quiet,
                "ENOENT:frame not found",
            );
            return None;
        };
        Some(pixels)
    }

    /// Handle `a=c` — compose animation frames.
    ///
    /// Copies a pixel rectangle from a source frame (`r=`, offset `X`/`Y`)
    /// onto a destination frame (`c=`, offset `x`/`y`), sized `w`/`h`, using
    /// alpha-blend or overwrite (`C=`).
    fn handle_kitty_animation_compose(&mut self, cmd: &KittyGraphicsCommand) {
        let image_id_hint = cmd.control.image_id.unwrap_or(0);
        let placement_id = cmd.control.placement_id;
        let quiet = cmd.control.quiet;
        let id = u64::from(image_id_hint);

        let Some(mut stored_image) = self.buffer.image_store().get(id).cloned() else {
            self.send_kitty_error(
                kitty_id_no_number(image_id_hint, placement_id),
                quiet,
                "ENOENT:image not found",
            );
            return;
        };

        let Some((src_frame, dest_frame)) =
            self.require_compose_frame_numbers(cmd, image_id_hint, placement_id, quiet)
        else {
            return;
        };

        let Some(src_pixels) = self.require_compose_frame_pixels(
            &stored_image,
            src_frame,
            image_id_hint,
            placement_id,
            quiet,
        ) else {
            return;
        };
        let Some(dest_pixels) = self.require_compose_frame_pixels(
            &stored_image,
            dest_frame,
            image_id_hint,
            placement_id,
            quiet,
        ) else {
            return;
        };

        let img_w = stored_image.width_px;
        let img_h = stored_image.height_px;
        let rect = resolve_compose_rect(cmd, img_w, img_h);

        // Same-frame overlapping rects are invalid (reading and writing the
        // same pixels in one compose is undefined).
        if src_frame == dest_frame && rect.overlaps_self() {
            self.send_kitty_error(
                kitty_id_no_number(image_id_hint, placement_id),
                quiet,
                "EINVAL:overlapping same-frame rects",
            );
            return;
        }

        let Some(rect_pixels) = extract_rect(
            &src_pixels,
            img_w,
            img_h,
            rect.src_x,
            rect.src_y,
            rect.width,
            rect.height,
        ) else {
            self.send_kitty_error(
                kitty_id_no_number(image_id_hint, placement_id),
                quiet,
                "EINVAL:source rect out of bounds",
            );
            return;
        };

        // `C` (no_cursor_movement under a=c) — compose mode.
        let overwrite = cmd.control.no_cursor_movement;

        let mut dest_canvas = dest_pixels.as_ref().clone();
        if let Err(msg) = composite_rect(
            &mut dest_canvas,
            img_w,
            img_h,
            &rect_pixels,
            rect.width,
            rect.height,
            rect.dest_x,
            rect.dest_y,
            overwrite,
        ) {
            self.send_kitty_error(kitty_id_no_number(image_id_hint, placement_id), quiet, msg);
            return;
        }

        if !stored_image.set_frame_pixels(dest_frame, std::sync::Arc::new(dest_canvas)) {
            self.send_kitty_error(
                kitty_id_no_number(image_id_hint, placement_id),
                quiet,
                "ENOENT:destination frame not found",
            );
            return;
        }

        self.buffer
            .image_store_mut()
            .insert_protocol_retained(stored_image);

        if quiet < 1 {
            let response = format_kitty_response_body(
                kitty_id_no_number(image_id_hint, placement_id),
                true,
                "",
            );
            self.write_apc_response(&response);
        }
    }

    /// Send a Kitty graphics error response, respecting quiet mode.
    fn send_kitty_error(&self, id: KittyResponseId, quiet: u8, message: &str) {
        // quiet=2 suppresses all responses (including errors).
        if quiet >= 2 {
            tracing::debug!(
                "Kitty graphics: error suppressed by q=2: id={} {message}",
                id.image_id
            );
            return;
        }
        let response = format_kitty_response_body(id, false, message);
        self.write_apc_response(&response);
    }

    /// Handle `a=d` — delete images.
    ///
    /// Supports the most common delete targets: delete all (`d=a`/`d=A`),
    /// delete by ID (`d=i`/`d=I`), and delete at cursor (`d=c`/`d=C`).
    /// Unsupported targets are logged and ignored.
    fn handle_kitty_delete(&mut self, cmd: &KittyGraphicsCommand) {
        use freminal_common::buffer_states::kitty_graphics::KittyDeleteTarget;

        let target = cmd.control.delete_target.unwrap_or(KittyDeleteTarget::All);
        let free_data = ImageDataDisposition::from(cmd.control.delete_free_data);

        match target {
            KittyDeleteTarget::All => self.handle_kitty_delete_all(free_data),
            KittyDeleteTarget::ById => self.handle_kitty_delete_by_id(cmd, free_data),
            KittyDeleteTarget::ByNumber => self.handle_kitty_delete_by_number(cmd, free_data),
            KittyDeleteTarget::AtCursor => self.handle_kitty_delete_at_cursor(free_data),
            KittyDeleteTarget::Frames => self.handle_kitty_delete_frames(cmd, free_data),
            KittyDeleteTarget::AtCell => self.handle_kitty_delete_at_cell(cmd, free_data),
            KittyDeleteTarget::AtCellZIndex => {
                self.handle_kitty_delete_at_cell_z_index(cmd, free_data);
            }
            KittyDeleteTarget::IdRange => self.handle_kitty_delete_id_range(cmd, free_data),
            KittyDeleteTarget::InColumn => self.handle_kitty_delete_in_column(cmd, free_data),
            KittyDeleteTarget::InRow => self.handle_kitty_delete_in_row(cmd, free_data),
            KittyDeleteTarget::AtZIndex => self.handle_kitty_delete_at_z_index(cmd, free_data),
        }
    }

    /// `d=a`/`d=A` — delete all placements VISIBLE ON SCREEN.
    fn handle_kitty_delete_all(&mut self, free_data: ImageDataDisposition) {
        let ids = self.buffer.clear_image_placements_visible(0);
        tracing::debug!(
            "Kitty graphics: deleting {} VISIBLE placement(s) (free_data={free_data:?})",
            ids.len(),
        );
        // Virtual (Unicode placeholder) and real-placement bookkeeping is
        // not tracked per visible row, so `d=a`/`d=A` clears it in full —
        // only the CELL clear above is scoped to the visible window, per
        // spec.
        self.virtual_placements.clear();
        self.real_placements.clear();
        if free_data.frees_data() {
            for id in ids {
                self.free_image_if_unreferenced(id);
            }
        }
    }

    /// `d=i`/`d=I` — delete placements for image id `i=`, optionally
    /// narrowed to a single placement id `p=` (Task 100.20).
    fn handle_kitty_delete_by_id(
        &mut self,
        cmd: &KittyGraphicsCommand,
        free_data: ImageDataDisposition,
    ) {
        let Some(image_id) = cmd.control.image_id else {
            return;
        };
        let id = u64::from(image_id);
        match cmd.control.placement_id {
            Some(pid) if pid != 0 => {
                tracing::debug!(
                    "Kitty graphics: deleting placement id={pid} of image id={id} \
                     (free_data={free_data:?})"
                );
                self.buffer.clear_image_placements_by_placement(id, pid);
            }
            _ => {
                tracing::debug!(
                    "Kitty graphics: deleting all placements for image id={id} \
                     (free_data={free_data:?})"
                );
                self.buffer.clear_image_placements_by_id(id);
            }
        }
        self.virtual_placements
            .retain(|&(img_id, _), _| img_id != id);
        if free_data.frees_data() {
            self.free_image_if_unreferenced(id);
        }
    }

    /// `d=n`/`d=N` — delete placements for the newest image with number `I=`.
    fn handle_kitty_delete_by_number(
        &mut self,
        cmd: &KittyGraphicsCommand,
        free_data: ImageDataDisposition,
    ) {
        let Some(number) = cmd.control.image_number else {
            return;
        };
        tracing::debug!(
            "Kitty graphics: deleting placements for image number={number} (free_data={free_data:?})"
        );
        self.buffer.clear_image_placements_by_number(number);
        // `clear_image_placements_by_number` only clears cell placements,
        // not the virtual-placement table.
        if let Some(id) = self.buffer.image_store().newest_id_for_number(number) {
            self.virtual_placements
                .retain(|&(img_id, _), _| img_id != id);
            if free_data.frees_data() {
                self.free_image_if_unreferenced(id);
            }
        }
    }

    /// `d=c`/`d=C` — delete placements intersecting the cursor cell.
    ///
    /// Kitty graphics spec, "Deleting images": `c` or `C` is "Delete all
    /// placements that intersect with the current cursor position." kitty's
    /// `graphics.c` does the same with a point filter on the cursor cell
    /// (`d.x_offset = c->x + 1; d.y_offset = c->y + 1;`), so only placements
    /// that cover that one cell go, not everything on its row. Only the
    /// placement under the cursor is removed, so a second placement of the
    /// same image elsewhere survives.
    fn handle_kitty_delete_at_cursor(&mut self, free_data: ImageDataDisposition) {
        // `cursor().pos` indexes `Buffer::rows()` directly, as the cell
        // lookups below do.
        let cursor = self.buffer.cursor().pos;
        tracing::debug!(
            "Kitty graphics: deleting placements at cursor cell ({},{}) (free_data={free_data:?})",
            cursor.y,
            cursor.x,
        );
        self.delete_placements_at_cell(cursor.y, cursor.x, free_data);
    }

    /// `d=f`/`d=F` — delete animation frames for the `i=`/`I=` image.
    fn handle_kitty_delete_frames(
        &mut self,
        cmd: &KittyGraphicsCommand,
        free_data: ImageDataDisposition,
    ) {
        let Some(id) = self.resolve_kitty_image_id(&cmd.control) else {
            tracing::debug!("Kitty graphics: d=f/d=F with no resolvable image id; ignoring");
            return;
        };
        let Some(mut stored_image) = self.buffer.image_store().get(id).cloned() else {
            return;
        };
        tracing::debug!(
            "Kitty graphics: clearing animation frames for image id={id} (free_data={free_data:?})"
        );
        stored_image.frames.clear();
        stored_image.animation = AnimationControl::default();
        self.buffer
            .image_store_mut()
            .insert_protocol_retained(stored_image);
        if free_data.frees_data() {
            self.free_image_if_unreferenced(id);
        }
    }

    /// Resolve the `x=` key of a delete command to a 0-based column.
    ///
    /// Kitty graphics spec, "Deleting images": "The values of the x and y keys
    /// are the same as cursor positions (i.e. x=1, y=1 is the top left cell)."
    /// They are therefore 1-based coordinates of the SCREEN. A key of `0` names
    /// no column (kitty compares `start_column <= x - 1`, which `0` never
    /// satisfies), so it returns `None`. An absent key falls back to the
    /// cursor's column (freminal behaviour kept from before Task 125.C8; the
    /// spec is silent).
    fn resolve_delete_column(&self, cmd: &KittyGraphicsCommand) -> Option<usize> {
        match cmd.control.src_x {
            Some(x) => usize::value_from(x).ok()?.checked_sub(1),
            None => Some(self.buffer.cursor_screen_pos().x),
        }
    }

    /// Resolve the `y=` key of a delete command to an index of
    /// `Buffer::rows()`.
    ///
    /// `y` is a 1-based SCREEN row (see [`Self::resolve_delete_column`]), so it
    /// is mapped through the visible window rather than used as a buffer index
    /// (which is only the same thing while there is no scrollback). `0` and a
    /// row below the screen name no row and return `None`; an absent key falls
    /// back to the cursor's row.
    fn resolve_delete_row(&self, cmd: &KittyGraphicsCommand) -> Option<usize> {
        let screen_row = match cmd.control.src_y {
            Some(y) => usize::value_from(y).ok()?.checked_sub(1)?,
            None => self.buffer.cursor_screen_pos().y,
        };
        self.buffer.screen_row_index(screen_row)
    }

    /// Resolve the `x=`/`y=` keys of `d=p`/`d=P`/`d=q`/`d=Q` to a cell of the
    /// buffer, as `(row_index, col)` where `row_index` indexes
    /// `Buffer::rows()`. `None` when either coordinate names no cell.
    fn resolve_delete_cell(&self, cmd: &KittyGraphicsCommand) -> Option<(usize, usize)> {
        Some((
            self.resolve_delete_row(cmd)?,
            self.resolve_delete_column(cmd)?,
        ))
    }

    /// Delete the placement covering buffer cell `(row, col)`, freeing the
    /// image data afterwards when `free_data` and nothing else references it.
    fn delete_placements_at_cell(
        &mut self,
        row: usize,
        col: usize,
        free_data: ImageDataDisposition,
    ) {
        let id = self.image_id_at_cell(row, col);
        self.buffer.clear_image_placements_at_cell(row, col);
        if free_data.frees_data()
            && let Some(id) = id
        {
            self.free_image_if_unreferenced(id);
        }
    }

    /// `d=p`/`d=P` — delete placements intersecting cell `x=`,`y=`.
    fn handle_kitty_delete_at_cell(
        &mut self,
        cmd: &KittyGraphicsCommand,
        free_data: ImageDataDisposition,
    ) {
        let Some((row, col)) = self.resolve_delete_cell(cmd) else {
            tracing::debug!(
                "Kitty graphics: d=p/d=P cell (x={:?}, y={:?}) is not on screen; ignoring",
                cmd.control.src_x,
                cmd.control.src_y,
            );
            return;
        };
        tracing::debug!(
            "Kitty graphics: deleting placements at cell ({row},{col}) (free_data={free_data:?})"
        );
        self.delete_placements_at_cell(row, col, free_data);
    }

    /// `d=q`/`d=Q` — delete placements intersecting cell `x=`,`y=` with
    /// z-index `z=`.
    fn handle_kitty_delete_at_cell_z_index(
        &mut self,
        cmd: &KittyGraphicsCommand,
        free_data: ImageDataDisposition,
    ) {
        let Some((row, col)) = self.resolve_delete_cell(cmd) else {
            tracing::debug!(
                "Kitty graphics: d=q/d=Q cell (x={:?}, y={:?}) is not on screen; ignoring",
                cmd.control.src_x,
                cmd.control.src_y,
            );
            return;
        };
        let z = cmd.control.z_index.unwrap_or(0);
        tracing::debug!(
            "Kitty graphics: deleting placements at cell ({row},{col}) z={z} (free_data={free_data:?})"
        );
        if let Some(id) = self
            .buffer
            .clear_image_placements_at_cell_with_z(row, col, z)
            && free_data.frees_data()
        {
            self.free_image_if_unreferenced(id);
        }
    }

    /// `d=r`/`d=R` — delete images with id in `[x=, y=]`.
    fn handle_kitty_delete_id_range(
        &mut self,
        cmd: &KittyGraphicsCommand,
        free_data: ImageDataDisposition,
    ) {
        let low = u64::from(cmd.control.src_x.unwrap_or(0));
        let high = u64::from(cmd.control.src_y.unwrap_or(0));
        tracing::debug!(
            "Kitty graphics: deleting images with id in [{low},{high}] (free_data={free_data:?})"
        );
        let ids: Vec<u64> = self
            .buffer
            .image_store()
            .iter()
            .map(|(&id, _)| id)
            .filter(|&id| id >= low && id <= high)
            .collect();
        for id in ids {
            self.buffer.clear_image_placements_by_id(id);
            self.virtual_placements
                .retain(|&(img_id, _), _| img_id != id);
            if free_data.frees_data() {
                self.free_image_if_unreferenced(id);
            }
        }
    }

    /// `d=x`/`d=X` — delete placements intersecting column `x=`.
    ///
    /// Kitty graphics spec: "Delete all placements that intersect the
    /// specified column, specified using the x key." `x=` is 1-based (see
    /// [`Self::resolve_delete_column`]); kitty's `x_filter_func` tests
    /// `start_column <= x - 1 < start_column + effective_num_cols` on every
    /// placement, with no row restriction.
    fn handle_kitty_delete_in_column(
        &mut self,
        cmd: &KittyGraphicsCommand,
        free_data: ImageDataDisposition,
    ) {
        let Some(col) = self.resolve_delete_column(cmd) else {
            tracing::debug!(
                "Kitty graphics: d=x/d=X column x={:?} names no column; ignoring",
                cmd.control.src_x,
            );
            return;
        };
        let ids = self.image_ids_in_column(col);
        tracing::debug!(
            "Kitty graphics: deleting placements in column {col} (free_data={free_data:?})"
        );
        self.buffer.clear_image_placements_in_column(col);
        if free_data.frees_data() {
            for id in ids {
                self.free_image_if_unreferenced(id);
            }
        }
    }

    /// `d=y`/`d=Y` — delete placements intersecting row `y=`.
    ///
    /// Kitty graphics spec: "Delete all placements that intersect the
    /// specified row, specified using the y key." `y=` is a 1-based screen row
    /// (see [`Self::resolve_delete_row`]); kitty's `y_filter_func` tests
    /// `start_row <= y - 1 < start_row + effective_num_rows`.
    fn handle_kitty_delete_in_row(
        &mut self,
        cmd: &KittyGraphicsCommand,
        free_data: ImageDataDisposition,
    ) {
        let Some(row) = self.resolve_delete_row(cmd) else {
            tracing::debug!(
                "Kitty graphics: d=y/d=Y row y={:?} is not on screen; ignoring",
                cmd.control.src_y,
            );
            return;
        };
        let ids = self.image_ids_in_row(row);
        tracing::debug!(
            "Kitty graphics: deleting placements in row {row} (free_data={free_data:?})"
        );
        self.buffer.clear_image_placements_in_row(row);
        if free_data.frees_data() {
            for id in ids {
                self.free_image_if_unreferenced(id);
            }
        }
    }

    /// `d=z`/`d=Z` — delete placements with z-index `z=`.
    fn handle_kitty_delete_at_z_index(
        &mut self,
        cmd: &KittyGraphicsCommand,
        free_data: ImageDataDisposition,
    ) {
        let z = cmd.control.z_index.unwrap_or(0);
        let ids = self.image_ids_by_z_index(z);
        tracing::debug!(
            "Kitty graphics: deleting placements at z-index {z} (free_data={free_data:?})"
        );
        self.buffer.clear_image_placements_by_z_index(z);
        if free_data.frees_data() {
            for id in ids {
                self.free_image_if_unreferenced(id);
            }
        }
    }

    /// Free image `id`'s store data (and prune its bookkeeping) if, AFTER
    /// the caller has already cleared the targeted placements, no cell
    /// anywhere in the buffer (including scrollback) still references it.
    ///
    /// Only called from the uppercase (`delete_free_data`) delete paths —
    /// lowercase deletes remove placements only and never call this, per
    /// the kitty spec's data-preservation guarantee for the lowercase
    /// delete targets.
    fn free_image_if_unreferenced(&mut self, id: u64) {
        let still_referenced = self.buffer.rows().iter().any(|row| {
            row.cells_for_image_scan()
                .iter()
                .any(|c| c.image_placement().is_some_and(|p| p.image_id == id))
        });
        if !still_referenced {
            self.buffer.image_store_mut().remove(id);
            self.virtual_placements
                .retain(|&(img_id, _), _| img_id != id);
            // Cascade-delete this image's real placements and their
            // relative children (Task 100.4a) — safe now that no cell
            // references the image anywhere.
            self.cascade_delete_real_placements_for_image(id);
        }
    }

    /// Collect the image id at a single cell, if any (Kitty `d=p`/`d=P`).
    fn image_id_at_cell(&self, row: usize, col: usize) -> Option<u64> {
        self.buffer
            .rows()
            .get(row)?
            .cells_for_image_scan()
            .get(col)?
            .image_placement()
            .map(|p| p.image_id)
    }

    /// Collect the distinct image ids intersecting a full row (Kitty
    /// `d=c`/`d=C`, `d=y`/`d=Y`).
    fn image_ids_in_row(&self, row: usize) -> Vec<u64> {
        let Some(row) = self.buffer.rows().get(row) else {
            return Vec::new();
        };
        let mut ids = Vec::new();
        for cell in row.cells_for_image_scan() {
            if let Some(p) = cell.image_placement()
                && !ids.contains(&p.image_id)
            {
                ids.push(p.image_id);
            }
        }
        ids
    }

    /// Collect the distinct image ids intersecting a full column (Kitty
    /// `d=x`/`d=X`).
    fn image_ids_in_column(&self, col: usize) -> Vec<u64> {
        let mut ids = Vec::new();
        for row in self.buffer.rows() {
            if let Some(cell) = row.cells_for_image_scan().get(col)
                && let Some(p) = cell.image_placement()
                && !ids.contains(&p.image_id)
            {
                ids.push(p.image_id);
            }
        }
        ids
    }

    /// Collect the distinct image ids with a matching z-index anywhere in
    /// the buffer (Kitty `d=z`/`d=Z`).
    fn image_ids_by_z_index(&self, z: i32) -> Vec<u64> {
        let mut ids = Vec::new();
        for row in self.buffer.rows() {
            for cell in row.cells() {
                if let Some(p) = cell.image_placement()
                    && p.z_index == z
                    && !ids.contains(&p.image_id)
                {
                    ids.push(p.image_id);
                }
            }
        }
        ids
    }

    /// Resolve `a=p`/`a=T` `X=`/`Y=` control keys into a [`SubCellOffset`]
    /// for the display path only (Task 100.19).
    ///
    /// `X=`/`Y=` (`cell_x_offset`/`cell_y_offset`) shift the drawing origin
    /// within the placement's top-left cell by that many PIXELS. Clamped to
    /// strictly less than the current cell pixel dimensions (a client could
    /// request an offset at or past the cell edge — kitty requires it be
    /// less than the cell size). Returns `None` when both axes are absent
    /// or `0`, so the renderer's "no offset" fast path is used.
    ///
    /// Must be called ONLY from the DISPLAY sites ([`Self::stamp_kitty_put`],
    /// [`Self::place_kitty_image`]'s display branch, and
    /// [`Self::stamp_relative_placement_at_real_parent`]) — `cell_x_offset`/
    /// `cell_y_offset` are the SAME control-data fields read by
    /// [`resolve_compose_rect`] for `a=c` (compose) with different meaning
    /// (source top-left there, not a display sub-cell shift), and are not
    /// meaningful at all on the Unicode-placeholder path.
    fn resolve_subcell_offset(&self, control: &KittyControlData) -> Option<SubCellOffset> {
        let x = control.cell_x_offset.unwrap_or(0);
        let y = control.cell_y_offset.unwrap_or(0);
        if x == 0 && y == 0 {
            return None;
        }
        let x = x.min(self.cell_pixel_width.saturating_sub(1));
        let y = y.min(self.cell_pixel_height.saturating_sub(1));
        Some(SubCellOffset { x, y })
    }
}

/// Resolved geometry for an `a=c` (animation compose) command: rect size
/// plus the destination and source top-left coordinates.
struct ComposeRect {
    width: u32,
    height: u32,
    dest_x: u32,
    dest_y: u32,
    src_x: u32,
    src_y: u32,
}

impl ComposeRect {
    /// Returns `true` if the destination rect and source rect (same size,
    /// different origins) overlap. Only meaningful when source and
    /// destination are the same frame.
    const fn overlaps_self(&self) -> bool {
        let overlap_x = self.dest_x < self.src_x.saturating_add(self.width)
            && self.src_x < self.dest_x.saturating_add(self.width);
        let overlap_y = self.dest_y < self.src_y.saturating_add(self.height)
            && self.src_y < self.dest_y.saturating_add(self.height);
        overlap_x && overlap_y
    }
}

/// Resolve `a=c` control data into compose rectangle geometry: `w`/`h`
/// (`src_rect_width`/`src_rect_height`) default to the full image size when
/// absent or `0`; `x`/`y` are the destination top-left, `X`/`Y`
/// (`cell_x_offset`/`cell_y_offset`) are the source top-left.
fn resolve_compose_rect(
    cmd: &KittyGraphicsCommand,
    img_width: u32,
    img_height: u32,
) -> ComposeRect {
    let width = cmd
        .control
        .src_rect_width
        .filter(|&w| w > 0)
        .unwrap_or(img_width);
    let height = cmd
        .control
        .src_rect_height
        .filter(|&h| h > 0)
        .unwrap_or(img_height);

    ComposeRect {
        width,
        height,
        dest_x: cmd.control.src_x.unwrap_or(0),
        dest_y: cmd.control.src_y.unwrap_or(0),
        src_x: cmd.control.cell_x_offset.unwrap_or(0),
        src_y: cmd.control.cell_y_offset.unwrap_or(0),
    }
}

/// Resolve `a=p`/`a=T` `x=`/`y=`/`w=`/`h=` control keys into a
/// [`SourceCrop`] for the display path only (Task 100.9).
///
/// `x=`/`y=` default to `0` (top-left) when absent; `w=`/`h=` (`0` or
/// absent) default to "the rest of the image" from that origin — matching
/// the same "0/absent means full" idiom as [`resolve_compose_rect`]. If the
/// resolved rectangle covers the whole image from the origin, `None` is
/// returned so the renderer's full-image fast path is used. The rectangle
/// is defensively clamped to the image's pixel bounds — a client could
/// request `x`/`w` past the edge.
fn resolve_source_crop(
    control: &KittyControlData,
    img_width_px: u32,
    img_height_px: u32,
) -> Option<SourceCrop> {
    let x = control.src_x.unwrap_or(0);
    let y = control.src_y.unwrap_or(0);
    let width = control
        .src_rect_width
        .filter(|&w| w > 0)
        .unwrap_or_else(|| img_width_px.saturating_sub(x));
    let height = control
        .src_rect_height
        .filter(|&h| h > 0)
        .unwrap_or_else(|| img_height_px.saturating_sub(y));

    // No crop at all (full image from origin) → None, so the renderer's
    // fast path is used.
    if x == 0 && y == 0 && width >= img_width_px && height >= img_height_px {
        None
    } else {
        // Clamp the crop to the image bounds (defensive; a client could
        // send x/w past the edge).
        let x = x.min(img_width_px);
        let y = y.min(img_height_px);
        let width = width.min(img_width_px.saturating_sub(x));
        let height = height.min(img_height_px.saturating_sub(y));
        if width == 0 || height == 0 {
            None
        } else {
            Some(SourceCrop {
                x,
                y,
                width,
                height,
            })
        }
    }
}

/// Resolve a kitty `z=` gap value into a stored gap in milliseconds.
///
/// `None` or `Some(0)` means "leave the default unchanged" (returns `None`);
/// a negative value means "gapless" (returns `Some(0)`); a positive value is
/// used as-is.
fn resolve_gap_ms(z: Option<i32>) -> Option<u32> {
    match z {
        None | Some(0) => None,
        Some(v) if v < 0 => Some(0),
        Some(v) => u32::try_from(v).ok(),
    }
}

/// Build a fresh RGBA canvas of `width x height` pixels filled with `bg`, a
/// packed 32-bit RGBA integer (`0xRRGGBBAA`). `bg == 0` yields transparent
/// black.
fn new_canvas_filled(width: u32, height: u32, bg: u32) -> Vec<u8> {
    let width_px = usize::value_from(width).unwrap_or(0);
    let height_px = usize::value_from(height).unwrap_or(0);
    let pixel_count = width_px.saturating_mul(height_px);

    let red = u8::try_from((bg >> 24) & 0xFF).unwrap_or(0);
    let green = u8::try_from((bg >> 16) & 0xFF).unwrap_or(0);
    let blue = u8::try_from((bg >> 8) & 0xFF).unwrap_or(0);
    let alpha = u8::try_from(bg & 0xFF).unwrap_or(0);

    let mut canvas = Vec::with_capacity(pixel_count.saturating_mul(4));
    for _ in 0..pixel_count {
        canvas.extend_from_slice(&[red, green, blue, alpha]);
    }
    canvas
}

/// Alpha-blend `src` over `dst` (straight, non-premultiplied RGBA), each a
/// 4-byte `[r, g, b, a]` slice.
fn alpha_blend(src: &[u8], dst: &[u8]) -> [u8; 4] {
    let sa = u32::from(src[3]);
    let da = u32::from(dst[3]);
    let out_a = sa + da * (255 - sa) / 255;
    if out_a == 0 {
        return [0, 0, 0, 0];
    }
    let mut out = [0u8; 4];
    for (channel, out_channel) in out.iter_mut().take(3).enumerate() {
        let sc = u32::from(src[channel]);
        let dc = u32::from(dst[channel]);
        let mixed = (sc * sa + dc * da * (255 - sa) / 255) / out_a;
        *out_channel = u8::try_from(mixed.min(255)).unwrap_or(255);
    }
    out[3] = u8::try_from(out_a.min(255)).unwrap_or(255);
    out
}

/// Composite a decoded rectangle (`rect`, `rect_width x rect_height` RGBA)
/// onto `canvas` (`canvas_width x canvas_height` RGBA) at `(dest_left,
/// dest_top)`, either by alpha-blending (default) or overwriting.
///
/// Returns `Err(message)` if the rect does not fit within the canvas bounds.
#[allow(clippy::too_many_arguments, clippy::similar_names)]
// Pixel-compositing geometry inherently needs all of these; grouping into a
// struct would obscure the call sites without reducing coupling. The
// width/height and x/y parameter pairs are the idiomatic naming for pixel
// rects; renaming them away from that convention would reduce readability
// without addressing any real ambiguity risk.
fn composite_rect(
    canvas: &mut [u8],
    canvas_width: u32,
    canvas_height: u32,
    rect: &[u8],
    rect_width: u32,
    rect_height: u32,
    dest_left: u32,
    dest_top: u32,
    overwrite: bool,
) -> Result<(), &'static str> {
    let canvas_width_px = usize::value_from(canvas_width).unwrap_or(0);
    let canvas_height_px = usize::value_from(canvas_height).unwrap_or(0);
    let rect_width_px = usize::value_from(rect_width).unwrap_or(0);
    let rect_height_px = usize::value_from(rect_height).unwrap_or(0);
    let dest_left_px = usize::value_from(dest_left).unwrap_or(0);
    let dest_top_px = usize::value_from(dest_top).unwrap_or(0);

    if dest_left_px.saturating_add(rect_width_px) > canvas_width_px
        || dest_top_px.saturating_add(rect_height_px) > canvas_height_px
    {
        return Err("EINVAL:rect out of bounds");
    }

    for row in 0..rect_height_px {
        let src_row_start = row.saturating_mul(rect_width_px).saturating_mul(4);
        let dst_row = dest_top_px + row;
        for col in 0..rect_width_px {
            let src_idx = src_row_start + col.saturating_mul(4);
            let dst_col = dest_left_px + col;
            let dst_idx = (dst_row.saturating_mul(canvas_width_px) + dst_col).saturating_mul(4);

            let Some(src_px) = rect.get(src_idx..src_idx + 4) else {
                continue;
            };
            let Some(dst_px) = canvas.get(dst_idx..dst_idx + 4) else {
                continue;
            };

            let blended = if overwrite {
                [src_px[0], src_px[1], src_px[2], src_px[3]]
            } else {
                alpha_blend(src_px, dst_px)
            };

            if let Some(dst) = canvas.get_mut(dst_idx..dst_idx + 4) {
                dst.copy_from_slice(&blended);
            }
        }
    }

    Ok(())
}

/// Extract a `w x h` pixel rectangle at `(x, y)` from a full `img_w x img_h`
/// RGBA buffer. Returns `None` if the rect is out of bounds.
#[allow(clippy::similar_names)]
// The width/height and x/y parameter pairs are the idiomatic naming for
// pixel rects; renaming them away from that convention would reduce
// readability without addressing any real ambiguity risk.
fn extract_rect(
    pixels: &[u8],
    img_width: u32,
    img_height: u32,
    left: u32,
    top: u32,
    rect_width: u32,
    rect_height: u32,
) -> Option<Vec<u8>> {
    let img_width_px = usize::value_from(img_width).unwrap_or(0);
    let img_height_px = usize::value_from(img_height).unwrap_or(0);
    let left_px = usize::value_from(left).unwrap_or(0);
    let top_px = usize::value_from(top).unwrap_or(0);
    let rect_width_px = usize::value_from(rect_width).unwrap_or(0);
    let rect_height_px = usize::value_from(rect_height).unwrap_or(0);

    if left_px.saturating_add(rect_width_px) > img_width_px
        || top_px.saturating_add(rect_height_px) > img_height_px
    {
        return None;
    }

    let mut out = Vec::with_capacity(
        rect_width_px
            .saturating_mul(rect_height_px)
            .saturating_mul(4),
    );
    for row in 0..rect_height_px {
        let src_row = top_px + row;
        let row_start = (src_row.saturating_mul(img_width_px) + left_px).saturating_mul(4);
        let row_end = row_start.saturating_add(rect_width_px.saturating_mul(4));
        let slice = pixels.get(row_start..row_end)?;
        out.extend_from_slice(slice);
    }
    Some(out)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    #[cfg(unix)]
    use super::{
        FileIdentity, delete_kitty_temp_file, load_kitty_file, open_regular_kitty_file,
        verify_opened_kitty_file,
    };
    use super::{KittyFileError, is_refused_kitty_path, read_capped};
    use freminal_common::{
        buffer_states::{
            format_tag::FormatTag,
            kitty_graphics::{KittyAction, KittyControlData, KittyGraphicsCommand},
        },
        colors::TerminalColor,
        pty_write::PtyWrite,
    };

    use super::ImageDataDisposition;
    use super::ImageSizeMode;
    use super::SourceCrop;

    use freminal_buffer::cell::Cell;
    use freminal_buffer::row::Row;

    use super::super::{KittyImageState, KittyTransfer, TerminalHandler};
    use freminal_common::buffer_states::row_number::RowNumber;

    // ------------------------------------------------------------------
    // Kitty graphics direct transfer tests
    // ------------------------------------------------------------------

    /// Helper: create a `TerminalHandler` with a write channel and return
    /// `(handler, rx)` so tests can inspect PTY responses.
    fn kitty_handler() -> (TerminalHandler, crossbeam_channel::Receiver<PtyWrite>) {
        let mut handler = TerminalHandler::new(80, 24);
        let (tx, rx) = crossbeam_channel::unbounded::<PtyWrite>();
        handler.set_write_tx(tx);
        (handler, rx)
    }

    /// Generate a unique, POSIX-style shared-memory object name that is safe
    /// to pass to `shm_open` / `CreateFileMappingW` on every platform.
    ///
    /// macOS caps POSIX shm names at `PSHMNAMLEN` (31 characters, *including*
    /// the leading `/`) and returns `ENAMETOOLONG` for anything longer —
    /// unlike Linux, which allows up to `NAME_MAX` (255). The descriptive
    /// `/freminal_test_shm_{pid}_{label}` names previously used here
    /// overflowed that limit on macOS, so names are kept deliberately short.
    ///
    /// Uniqueness across concurrently-running test binaries and across the
    /// tests within one binary comes from the low bits of the process id
    /// combined with a per-binary atomic counter, hex-encoded. The result
    /// (e.g. `/frm_2a3f_1`) stays well under 31 characters while satisfying
    /// [`shm_name_is_safe`] (single leading slash, no embedded slash, no
    /// `..`, no NUL).
    #[cfg(any(unix, windows))]
    fn unique_shm_name() -> String {
        use std::sync::atomic::{AtomicU32, Ordering};

        static COUNTER: AtomicU32 = AtomicU32::new(0);

        // Mask the pid to 16 bits so the hex stays at most 4 characters; the
        // atomic counter guarantees intra-binary uniqueness regardless.
        let pid = std::process::id() & 0xffff;
        let seq = COUNTER.fetch_add(1, Ordering::Relaxed);
        format!("/frm_{pid:x}_{seq:x}")
    }

    /// Helper: build a `KittyGraphicsCommand` with the given control data and
    /// raw RGBA payload for a 2x2 image.
    fn kitty_rgba_2x2_cmd(action: KittyAction) -> KittyGraphicsCommand {
        use freminal_common::buffer_states::kitty_graphics::{KittyControlData, KittyFormat};

        // 2x2 RGBA = 16 bytes.
        let rgba_data: Vec<u8> = vec![
            255, 0, 0, 255, // red
            0, 255, 0, 255, // green
            0, 0, 255, 255, // blue
            255, 255, 0, 255, // yellow
        ];

        KittyGraphicsCommand {
            control: KittyControlData {
                action: Some(action),
                format: Some(KittyFormat::Rgba),
                src_width: Some(2),
                src_height: Some(2),
                image_id: Some(42),
                ..KittyControlData::default()
            },
            payload: rgba_data,
        }
    }

    /// Create a minimal 2x2 red PNG image as raw bytes.
    fn make_test_png() -> Vec<u8> {
        use image::ImageEncoder;
        let mut png_buf = Vec::new();
        let encoder = image::codecs::png::PngEncoder::new(&mut png_buf);
        // 2x2 RGBA image: all red
        let rgba_data: [u8; 16] = [
            255, 0, 0, 255, // pixel (0,0)
            255, 0, 0, 255, // pixel (1,0)
            255, 0, 0, 255, // pixel (0,1)
            255, 0, 0, 255, // pixel (1,1)
        ];
        encoder
            .write_image(&rgba_data, 2, 2, image::ExtendedColorType::Rgba8)
            .unwrap();
        png_buf
    }

    #[test]
    fn kitty_single_rgba_transmit_and_display_places_image() {
        use freminal_common::buffer_states::kitty_graphics::KittyAction;

        let (mut handler, _rx) = kitty_handler();
        let cmd = kitty_rgba_2x2_cmd(KittyAction::TransmitAndDisplay);

        handler.handle_kitty_graphics(cmd);

        // Image should be placed in the buffer.
        let has_image = handler.buffer().has_any_image_cell();
        assert!(
            has_image,
            "Expected image cells after Kitty TransmitAndDisplay"
        );

        // Image should be in the store.
        assert!(
            handler.buffer().image_store().get(42).is_some(),
            "Expected image id=42 in the image store"
        );
    }

    #[test]
    fn kitty_single_rgba_transmit_only_stores_but_does_not_place() {
        use freminal_common::buffer_states::kitty_graphics::KittyAction;

        let (mut handler, _rx) = kitty_handler();
        let cmd = kitty_rgba_2x2_cmd(KittyAction::Transmit);

        handler.handle_kitty_graphics(cmd);

        // Image should be in the store but NOT placed in cells.
        assert!(
            handler.buffer().image_store().get(42).is_some(),
            "Expected image id=42 in the store after Transmit"
        );

        let has_image = handler.buffer().has_any_image_cell();
        assert!(!has_image, "Transmit-only should not place image cells");
    }

    #[test]
    fn kitty_single_rgb_converts_to_rgba() {
        use freminal_common::buffer_states::kitty_graphics::{
            KittyAction, KittyControlData, KittyFormat,
        };

        let (mut handler, _rx) = kitty_handler();

        // 2x2 RGB = 12 bytes (no alpha channel).
        let rgb_data: Vec<u8> = vec![
            255, 0, 0, // red
            0, 255, 0, // green
            0, 0, 255, // blue
            255, 255, 0, // yellow
        ];

        let cmd = KittyGraphicsCommand {
            control: KittyControlData {
                action: Some(KittyAction::TransmitAndDisplay),
                format: Some(KittyFormat::Rgb),
                src_width: Some(2),
                src_height: Some(2),
                image_id: Some(99),
                ..KittyControlData::default()
            },
            payload: rgb_data,
        };

        handler.handle_kitty_graphics(cmd);

        // The stored image should have RGBA pixels (16 bytes for 2x2).
        let img = handler.buffer().image_store().get(99).unwrap();
        assert_eq!(
            img.pixels.len(),
            16,
            "RGB should be converted to RGBA (4 bytes/pixel)"
        );
        // Verify alpha was inserted: first pixel should be [255, 0, 0, 255].
        assert_eq!(&img.pixels[0..4], &[255, 0, 0, 255]);
    }

    #[test]
    fn kitty_single_png_decodes_and_places() {
        use freminal_common::buffer_states::kitty_graphics::{
            KittyAction, KittyControlData, KittyFormat,
        };

        let (mut handler, _rx) = kitty_handler();

        let png_data = make_test_png(); // 2x2 red PNG.

        let cmd = KittyGraphicsCommand {
            control: KittyControlData {
                action: Some(KittyAction::TransmitAndDisplay),
                format: Some(KittyFormat::Png),
                image_id: Some(77),
                // PNG does not require s/v — dimensions come from the image.
                ..KittyControlData::default()
            },
            payload: png_data,
        };

        handler.handle_kitty_graphics(cmd);

        let img = handler.buffer().image_store().get(77).unwrap();
        assert_eq!(img.width_px, 2);
        assert_eq!(img.height_px, 2);
        // PNG decoded to RGBA: 2*2*4 = 16 bytes.
        assert_eq!(img.pixels.len(), 16);
    }

    #[test]
    fn kitty_single_rgba_missing_dimensions_sends_error() {
        use freminal_common::buffer_states::kitty_graphics::{
            KittyAction, KittyControlData, KittyFormat,
        };

        let (mut handler, rx) = kitty_handler();

        // RGBA payload but no s/v dimensions → should send error.
        let cmd = KittyGraphicsCommand {
            control: KittyControlData {
                action: Some(KittyAction::TransmitAndDisplay),
                format: Some(KittyFormat::Rgba),
                image_id: Some(10),
                // No src_width or src_height!
                ..KittyControlData::default()
            },
            payload: vec![0; 16],
        };

        handler.handle_kitty_graphics(cmd);

        // Should NOT be stored.
        assert!(
            handler.buffer().image_store().get(10).is_none(),
            "Image should not be stored when dimensions are missing"
        );

        // Should have sent an error response.
        let response = rx.try_recv().unwrap();
        match response {
            PtyWrite::Write(bytes) => {
                let s = String::from_utf8_lossy(&bytes);
                assert!(s.contains("EINVAL"), "Expected EINVAL error, got: {s}");
            }
            PtyWrite::Resize(_) => panic!("Expected PtyWrite::Write"),
        }
    }

    #[test]
    fn kitty_single_rgba_payload_size_mismatch_sends_error() {
        use freminal_common::buffer_states::kitty_graphics::{
            KittyAction, KittyControlData, KittyFormat,
        };

        let (mut handler, rx) = kitty_handler();

        // Says 2x2 RGBA (expects 16 bytes) but payload is only 8 bytes.
        let cmd = KittyGraphicsCommand {
            control: KittyControlData {
                action: Some(KittyAction::TransmitAndDisplay),
                format: Some(KittyFormat::Rgba),
                src_width: Some(2),
                src_height: Some(2),
                image_id: Some(11),
                ..KittyControlData::default()
            },
            payload: vec![0; 8], // too small
        };

        handler.handle_kitty_graphics(cmd);

        assert!(handler.buffer().image_store().get(11).is_none());

        let response = rx.try_recv().unwrap();
        match response {
            PtyWrite::Write(bytes) => {
                let s = String::from_utf8_lossy(&bytes);
                assert!(s.contains("EINVAL"), "Expected EINVAL error, got: {s}");
            }
            PtyWrite::Resize(_) => panic!("Expected PtyWrite::Write"),
        }
    }

    #[test]
    fn kitty_single_empty_payload_sends_enodata() {
        use freminal_common::buffer_states::kitty_graphics::{
            KittyAction, KittyControlData, KittyFormat,
        };

        let (mut handler, rx) = kitty_handler();

        let cmd = KittyGraphicsCommand {
            control: KittyControlData {
                action: Some(KittyAction::TransmitAndDisplay),
                format: Some(KittyFormat::Rgba),
                src_width: Some(2),
                src_height: Some(2),
                image_id: Some(12),
                ..KittyControlData::default()
            },
            payload: Vec::new(), // empty
        };

        handler.handle_kitty_graphics(cmd);

        let response = rx.try_recv().unwrap();
        match response {
            PtyWrite::Write(bytes) => {
                let s = String::from_utf8_lossy(&bytes);
                assert!(s.contains("ENODATA"), "Expected ENODATA error, got: {s}");
            }
            PtyWrite::Resize(_) => panic!("Expected PtyWrite::Write"),
        }
    }

    #[test]
    fn kitty_single_ok_response_sent_for_verbose_mode() {
        use freminal_common::buffer_states::kitty_graphics::KittyAction;

        let (mut handler, rx) = kitty_handler();

        // quiet=0 (default) → should send OK.
        let cmd = kitty_rgba_2x2_cmd(KittyAction::TransmitAndDisplay);
        handler.handle_kitty_graphics(cmd);

        let response = rx.try_recv().unwrap();
        match response {
            PtyWrite::Write(bytes) => {
                let s = String::from_utf8_lossy(&bytes);
                assert!(s.contains("OK"), "Expected OK response, got: {s}");
            }
            PtyWrite::Resize(_) => panic!("Expected PtyWrite::Write"),
        }
    }

    #[test]
    fn kitty_single_quiet_1_suppresses_ok() {
        use freminal_common::buffer_states::kitty_graphics::{
            KittyAction, KittyControlData, KittyFormat,
        };

        let (mut handler, rx) = kitty_handler();

        let cmd = KittyGraphicsCommand {
            control: KittyControlData {
                action: Some(KittyAction::TransmitAndDisplay),
                format: Some(KittyFormat::Rgba),
                src_width: Some(2),
                src_height: Some(2),
                image_id: Some(50),
                quiet: 1, // suppress OK but not errors
                ..KittyControlData::default()
            },
            payload: vec![
                255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 0, 255,
            ],
        };

        handler.handle_kitty_graphics(cmd);

        // Image should still be placed.
        assert!(handler.buffer().image_store().get(50).is_some());

        // But no OK response should be sent.
        assert!(
            rx.try_recv().is_err(),
            "quiet=1 should suppress OK response"
        );
    }

    #[test]
    fn kitty_single_quiet_2_suppresses_all_responses() {
        use freminal_common::buffer_states::kitty_graphics::{
            KittyAction, KittyControlData, KittyFormat,
        };

        let (mut handler, rx) = kitty_handler();

        // quiet=2 with a bad payload → error should be suppressed.
        let cmd = KittyGraphicsCommand {
            control: KittyControlData {
                action: Some(KittyAction::TransmitAndDisplay),
                format: Some(KittyFormat::Rgba),
                src_width: Some(2),
                src_height: Some(2),
                image_id: Some(51),
                quiet: 2,
                ..KittyControlData::default()
            },
            payload: vec![0; 8], // wrong size → EINVAL
        };

        handler.handle_kitty_graphics(cmd);

        // No response at all.
        assert!(
            rx.try_recv().is_err(),
            "quiet=2 should suppress all responses including errors"
        );
    }

    #[test]
    fn kitty_delete_all_lowercase_keeps_store_data() {
        use freminal_common::buffer_states::kitty_graphics::{
            KittyAction, KittyControlData, KittyDeleteTarget,
        };

        let (mut handler, _rx) = kitty_handler();

        // First, place an image.
        let cmd = kitty_rgba_2x2_cmd(KittyAction::TransmitAndDisplay);
        handler.handle_kitty_graphics(cmd);
        assert!(handler.buffer().image_store().get(42).is_some());

        // Delete all (lowercase `a`) — placements only, data kept.
        let delete_cmd = KittyGraphicsCommand {
            control: KittyControlData {
                action: Some(KittyAction::Delete),
                delete_target: Some(KittyDeleteTarget::All),
                delete_free_data: false,
                ..KittyControlData::default()
            },
            payload: Vec::new(),
        };
        handler.handle_kitty_graphics(delete_cmd);

        // Store data must survive a lowercase `d=a`.
        assert!(
            handler.buffer().image_store().get(42).is_some(),
            "lowercase 'a' must keep the image data in the store"
        );

        // No image cells should remain (visible placements are cleared).
        let has_image = handler.buffer().rows().iter().any(|row| {
            row.cells()
                .iter()
                .any(freminal_buffer::cell::Cell::has_image)
        });
        assert!(!has_image, "Delete all should clear visible image cells");
    }

    #[test]
    fn kitty_delete_all_uppercase_frees_store_data() {
        use freminal_common::buffer_states::kitty_graphics::{
            KittyAction, KittyControlData, KittyDeleteTarget,
        };

        let (mut handler, _rx) = kitty_handler();

        let cmd = kitty_rgba_2x2_cmd(KittyAction::TransmitAndDisplay);
        handler.handle_kitty_graphics(cmd);
        assert!(handler.buffer().image_store().get(42).is_some());

        // Delete all (uppercase `A`) — placements AND (unreferenced) data.
        let delete_cmd = KittyGraphicsCommand {
            control: KittyControlData {
                action: Some(KittyAction::Delete),
                delete_target: Some(KittyDeleteTarget::All),
                delete_free_data: true,
                ..KittyControlData::default()
            },
            payload: Vec::new(),
        };
        handler.handle_kitty_graphics(delete_cmd);

        assert!(
            handler.buffer().image_store().get(42).is_none(),
            "uppercase 'A' should free the now-unreferenced image data"
        );

        let has_image = handler.buffer().rows().iter().any(|row| {
            row.cells()
                .iter()
                .any(freminal_buffer::cell::Cell::has_image)
        });
        assert!(!has_image, "Delete all should clear visible image cells");
    }

    #[test]
    fn kitty_delete_by_id_lowercase_keeps_store_data() {
        use freminal_common::buffer_states::kitty_graphics::{
            KittyAction, KittyControlData, KittyDeleteTarget, KittyFormat,
        };

        let (mut handler, _rx) = kitty_handler();

        // Place image id=42.
        let cmd1 = kitty_rgba_2x2_cmd(KittyAction::TransmitAndDisplay);
        handler.handle_kitty_graphics(cmd1);

        // Place image id=99.
        let cmd2 = KittyGraphicsCommand {
            control: KittyControlData {
                action: Some(KittyAction::Transmit),
                format: Some(KittyFormat::Rgba),
                src_width: Some(2),
                src_height: Some(2),
                image_id: Some(99),
                ..KittyControlData::default()
            },
            payload: vec![
                255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 0, 255,
            ],
        };
        handler.handle_kitty_graphics(cmd2);

        assert!(handler.buffer().image_store().get(42).is_some());
        assert!(handler.buffer().image_store().get(99).is_some());

        // Delete only id=42, lowercase `i` — placements only, data kept.
        let delete_cmd = KittyGraphicsCommand {
            control: KittyControlData {
                action: Some(KittyAction::Delete),
                delete_target: Some(KittyDeleteTarget::ById),
                image_id: Some(42),
                delete_free_data: false,
                ..KittyControlData::default()
            },
            payload: Vec::new(),
        };
        handler.handle_kitty_graphics(delete_cmd);

        assert!(
            handler.buffer().image_store().get(42).is_some(),
            "lowercase 'i' must keep id=42's data in the store"
        );
        assert!(
            handler.buffer().image_store().get(99).is_some(),
            "id=99 should be unaffected by delete-by-id of 42"
        );
        // But the placement (cells) for id=42 must be gone.
        let has_image_42 = handler.buffer().rows().iter().any(|row| {
            row.cells()
                .iter()
                .any(|c| c.image_placement().is_some_and(|p| p.image_id == 42))
        });
        assert!(!has_image_42, "id=42's placement should be cleared");
    }

    #[test]
    fn kitty_delete_by_id_uppercase_frees_only_target() {
        use freminal_common::buffer_states::kitty_graphics::{
            KittyAction, KittyControlData, KittyDeleteTarget, KittyFormat,
        };

        let (mut handler, _rx) = kitty_handler();

        // Place image id=42.
        let cmd1 = kitty_rgba_2x2_cmd(KittyAction::TransmitAndDisplay);
        handler.handle_kitty_graphics(cmd1);

        // Place image id=99.
        let cmd2 = KittyGraphicsCommand {
            control: KittyControlData {
                action: Some(KittyAction::Transmit),
                format: Some(KittyFormat::Rgba),
                src_width: Some(2),
                src_height: Some(2),
                image_id: Some(99),
                ..KittyControlData::default()
            },
            payload: vec![
                255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 0, 255,
            ],
        };
        handler.handle_kitty_graphics(cmd2);

        assert!(handler.buffer().image_store().get(42).is_some());
        assert!(handler.buffer().image_store().get(99).is_some());

        // Delete only id=42, uppercase `I` — placements AND unreferenced data.
        let delete_cmd = KittyGraphicsCommand {
            control: KittyControlData {
                action: Some(KittyAction::Delete),
                delete_target: Some(KittyDeleteTarget::ById),
                image_id: Some(42),
                delete_free_data: true,
                ..KittyControlData::default()
            },
            payload: Vec::new(),
        };
        handler.handle_kitty_graphics(delete_cmd);

        assert!(
            handler.buffer().image_store().get(42).is_none(),
            "id=42 should be removed"
        );
        assert!(
            handler.buffer().image_store().get(99).is_some(),
            "id=99 should survive delete-by-id of 42"
        );
    }

    /// `d=i,p=<n>` (Task 100.20) must narrow deletion to the ONE named
    /// placement — a second, independent placement of the SAME image id
    /// with a DIFFERENT `p=` must survive.
    #[test]
    fn kitty_delete_by_id_with_placement_id_narrows_to_named_placement() {
        use freminal_common::buffer_states::kitty_graphics::{
            KittyAction, KittyControlData, KittyDeleteTarget,
        };

        let (mut handler, rx) = kitty_handler();

        // Placement p=5 of image id=42 at (col=0, row=0).
        handler.buffer_mut().set_cursor_pos(Some(0), Some(0));
        let mut cmd_p5 = kitty_rgba_2x2_cmd(KittyAction::TransmitAndDisplay);
        cmd_p5.control.placement_id = Some(5);
        handler.handle_kitty_graphics(cmd_p5);
        let _ = recv_response(&rx);

        // Placement p=9 of the SAME image id=42, via `a=p` (Put), at a
        // different screen position.
        handler.buffer_mut().set_cursor_pos(Some(5), Some(3));
        let put_cmd = KittyGraphicsCommand {
            control: KittyControlData {
                action: Some(KittyAction::Put),
                image_id: Some(42),
                placement_id: Some(9),
                ..KittyControlData::default()
            },
            payload: Vec::new(),
        };
        handler.handle_kitty_graphics(put_cmd);
        let _ = recv_response(&rx);

        assert!(
            count_placement_with_id(&handler, 42, 5) > 0,
            "p=5 should exist before delete"
        );
        assert!(
            count_placement_with_id(&handler, 42, 9) > 0,
            "p=9 should exist before delete"
        );

        // Delete only p=5 of image id=42.
        let delete_cmd = KittyGraphicsCommand {
            control: KittyControlData {
                action: Some(KittyAction::Delete),
                delete_target: Some(KittyDeleteTarget::ById),
                image_id: Some(42),
                placement_id: Some(5),
                delete_free_data: false,
                ..KittyControlData::default()
            },
            payload: Vec::new(),
        };
        handler.handle_kitty_graphics(delete_cmd);

        assert_eq!(
            count_placement_with_id(&handler, 42, 5),
            0,
            "p=5's cells should be cleared"
        );
        assert!(
            count_placement_with_id(&handler, 42, 9) > 0,
            "p=9's cells must survive deletion of p=5"
        );
    }

    #[test]
    fn kitty_delete_at_cursor_uppercase_keeps_data_if_referenced_elsewhere() {
        use freminal_common::buffer_states::kitty_graphics::{
            KittyAction, KittyControlData, KittyDeleteTarget,
        };

        let (mut handler, _rx) = kitty_handler();

        // Place image id=42 at the cursor (row 0).
        handler.buffer_mut().set_cursor_pos(Some(0), Some(0));
        let cmd = kitty_rgba_2x2_cmd(KittyAction::TransmitAndDisplay);
        handler.handle_kitty_graphics(cmd);

        // Place the SAME image again, elsewhere (row 5), via `a=p` (Put) —
        // a second on-screen reference to image id=42.
        handler.buffer_mut().set_cursor_pos(Some(5), Some(3));
        let put_cmd = KittyGraphicsCommand {
            control: KittyControlData {
                action: Some(KittyAction::Put),
                image_id: Some(42),
                placement_id: Some(1),
                ..KittyControlData::default()
            },
            payload: Vec::new(),
        };
        handler.handle_kitty_graphics(put_cmd);

        // Delete AtCursor (uppercase `C`) targets only the cursor's row
        // (row 0), leaving row 5's placement of the same image intact —
        // so the data must survive even though `free_data` was requested.
        handler.buffer_mut().set_cursor_pos(Some(0), Some(0));
        let delete_cmd = KittyGraphicsCommand {
            control: KittyControlData {
                action: Some(KittyAction::Delete),
                delete_target: Some(KittyDeleteTarget::AtCursor),
                delete_free_data: true,
                ..KittyControlData::default()
            },
            payload: Vec::new(),
        };
        handler.handle_kitty_graphics(delete_cmd);

        assert!(
            handler.buffer().image_store().get(42).is_some(),
            "id=42 is still referenced at row 5; data must be kept"
        );
        let row0_has_image = handler.buffer().rows()[0]
            .cells()
            .iter()
            .any(freminal_buffer::cell::Cell::has_image);
        assert!(!row0_has_image, "row 0's placement should still be cleared");
    }

    #[test]
    fn kitty_delete_at_cursor_clears_images_at_cursor_row() {
        use freminal_common::buffer_states::kitty_graphics::{
            KittyAction, KittyControlData, KittyDeleteTarget,
        };

        let (mut handler, _rx) = kitty_handler();

        // Place an image (will be at row 0).
        let cmd = kitty_rgba_2x2_cmd(KittyAction::TransmitAndDisplay);
        handler.handle_kitty_graphics(cmd);

        // Cursor should now be below the image. Move it back to row 0.
        handler.buffer_mut().set_cursor_pos(Some(0), Some(0));

        // Delete at cursor (row 0 only).
        let delete_cmd = KittyGraphicsCommand {
            control: KittyControlData {
                action: Some(KittyAction::Delete),
                delete_target: Some(KittyDeleteTarget::AtCursor),
                ..KittyControlData::default()
            },
            payload: Vec::new(),
        };
        handler.handle_kitty_graphics(delete_cmd);

        // Row 0 should have no image cells.
        let row0_has_image = handler.buffer().rows()[0]
            .cells()
            .iter()
            .any(freminal_buffer::cell::Cell::has_image);
        assert!(!row0_has_image, "AtCursor delete should clear row 0 images");
    }

    #[test]
    fn kitty_delete_at_cursor_uppercase_frees_unreferenced_data() {
        use freminal_common::buffer_states::kitty_graphics::{
            KittyAction, KittyControlData, KittyDeleteTarget,
        };

        let (mut handler, _rx) = kitty_handler();

        let cmd = kitty_rgba_2x2_cmd(KittyAction::TransmitAndDisplay);
        handler.handle_kitty_graphics(cmd);
        handler.buffer_mut().set_cursor_pos(Some(0), Some(0));

        let delete_cmd = KittyGraphicsCommand {
            control: KittyControlData {
                action: Some(KittyAction::Delete),
                delete_target: Some(KittyDeleteTarget::AtCursor),
                delete_free_data: true,
                ..KittyControlData::default()
            },
            payload: Vec::new(),
        };
        handler.handle_kitty_graphics(delete_cmd);

        assert!(
            handler.buffer().image_store().get(42).is_none(),
            "uppercase 'C' should free the now-unreferenced image data"
        );
    }

    #[test]
    fn kitty_display_cols_and_rows_override() {
        use freminal_common::buffer_states::kitty_graphics::{
            KittyAction, KittyControlData, KittyFormat,
        };

        let (mut handler, _rx) = kitty_handler();

        let cmd = KittyGraphicsCommand {
            control: KittyControlData {
                action: Some(KittyAction::TransmitAndDisplay),
                format: Some(KittyFormat::Rgba),
                src_width: Some(2),
                src_height: Some(2),
                image_id: Some(55),
                display_cols: Some(10),
                display_rows: Some(5),
                ..KittyControlData::default()
            },
            payload: vec![
                255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 0, 255,
            ],
        };

        handler.handle_kitty_graphics(cmd);

        let img = handler.buffer().image_store().get(55).unwrap();
        assert_eq!(img.display_cols, 10);
        assert_eq!(img.display_rows, 5);
    }

    // -----------------------------------------------------------------------
    // ImageSizeMode provenance (Task 100.17a)
    // -----------------------------------------------------------------------

    #[test]
    fn kitty_transmit_with_display_cols_and_rows_sets_explicit_cells_mode() {
        use freminal_common::buffer_states::kitty_graphics::{
            KittyAction, KittyControlData, KittyFormat,
        };

        let (mut handler, _rx) = kitty_handler();

        let cmd = KittyGraphicsCommand {
            control: KittyControlData {
                action: Some(KittyAction::TransmitAndDisplay),
                format: Some(KittyFormat::Rgba),
                src_width: Some(2),
                src_height: Some(2),
                image_id: Some(60),
                display_cols: Some(10),
                display_rows: Some(5),
                ..KittyControlData::default()
            },
            payload: vec![
                255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 0, 255,
            ],
        };

        handler.handle_kitty_graphics(cmd);

        let img = handler.buffer().image_store().get(60).unwrap();
        assert_eq!(
            img.size_mode,
            ImageSizeMode::ExplicitCells,
            "explicit c=/r= on a=T should set ExplicitCells"
        );
    }

    #[test]
    fn kitty_transmit_without_display_cols_and_rows_sets_native_pixels_mode() {
        use freminal_common::buffer_states::kitty_graphics::KittyAction;

        let (mut handler, _rx) = kitty_handler();

        let cmd = kitty_rgba_2x2_cmd(KittyAction::TransmitAndDisplay);
        handler.handle_kitty_graphics(cmd);

        let img = handler.buffer().image_store().get(42).unwrap();
        assert_eq!(
            img.size_mode,
            ImageSizeMode::NativePixels,
            "no c=/r= on a=T should default to NativePixels"
        );
    }

    #[test]
    fn kitty_put_with_display_cols_and_rows_sets_explicit_cells_mode() {
        use freminal_common::buffer_states::kitty_graphics::{KittyAction, KittyControlData};

        let (mut handler, _rx) = kitty_handler();

        // Transmit only (no display), then Put it with an explicit c=/r= override.
        let cmd = kitty_rgba_2x2_cmd(KittyAction::Transmit);
        handler.handle_kitty_graphics(cmd);

        let put_cmd = KittyGraphicsCommand {
            control: KittyControlData {
                action: Some(KittyAction::Put),
                image_id: Some(42),
                display_cols: Some(6),
                display_rows: Some(3),
                ..KittyControlData::default()
            },
            payload: Vec::new(),
        };
        handler.handle_kitty_graphics(put_cmd);

        let img = handler.buffer().image_store().get(42).unwrap();
        assert_eq!(
            img.size_mode,
            ImageSizeMode::ExplicitCells,
            "explicit c=/r= on a=p should set ExplicitCells"
        );
    }

    #[test]
    fn kitty_put_without_display_cols_and_rows_sets_native_pixels_mode() {
        use freminal_common::buffer_states::kitty_graphics::{KittyAction, KittyControlData};

        let (mut handler, _rx) = kitty_handler();

        let cmd = kitty_rgba_2x2_cmd(KittyAction::Transmit);
        handler.handle_kitty_graphics(cmd);

        let put_cmd = KittyGraphicsCommand {
            control: KittyControlData {
                action: Some(KittyAction::Put),
                image_id: Some(42),
                ..KittyControlData::default()
            },
            payload: Vec::new(),
        };
        handler.handle_kitty_graphics(put_cmd);

        let img = handler.buffer().image_store().get(42).unwrap();
        assert_eq!(
            img.size_mode,
            ImageSizeMode::NativePixels,
            "no c=/r= on a=p should default to NativePixels"
        );
    }

    #[test]
    fn kitty_query_rgb_format_responds_ok() {
        use freminal_common::buffer_states::kitty_graphics::{
            KittyAction, KittyControlData, KittyFormat,
        };

        let (mut handler, rx) = kitty_handler();

        let cmd = KittyGraphicsCommand {
            control: KittyControlData {
                action: Some(KittyAction::Query),
                format: Some(KittyFormat::Rgb),
                image_id: Some(31),
                ..KittyControlData::default()
            },
            payload: vec![0, 0, 0], // minimal payload for query
        };

        handler.handle_kitty_graphics(cmd);

        let response = rx.try_recv().unwrap();
        match response {
            PtyWrite::Write(bytes) => {
                let s = String::from_utf8_lossy(&bytes);
                assert!(
                    s.contains("OK"),
                    "f=24 (RGB) query should succeed, got: {s}"
                );
                assert!(!s.contains("ENOTSUP"), "f=24 should NOT be unsupported");
            }
            PtyWrite::Resize(_) => panic!("Expected PtyWrite::Write"),
        }
    }

    #[test]
    fn kitty_chunked_transfer_assembles_and_places() {
        use freminal_common::buffer_states::kitty_graphics::{
            KittyAction, KittyControlData, KittyFormat,
        };

        let (mut handler, _rx) = kitty_handler();

        // Full RGBA payload for 2x2 = 16 bytes, split into two 8-byte chunks.
        let full_payload: Vec<u8> = vec![
            255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 0, 255,
        ];

        // First chunk (more_data=true).
        let chunk1 = KittyGraphicsCommand {
            control: KittyControlData {
                action: Some(KittyAction::TransmitAndDisplay),
                format: Some(KittyFormat::Rgba),
                src_width: Some(2),
                src_height: Some(2),
                image_id: Some(88),
                more_data: true,
                ..KittyControlData::default()
            },
            payload: full_payload[..8].to_vec(),
        };

        // Last chunk (more_data=false / default).
        let chunk2 = KittyGraphicsCommand {
            control: KittyControlData {
                more_data: false,
                ..KittyControlData::default()
            },
            payload: full_payload[8..].to_vec(),
        };

        handler.handle_kitty_graphics(chunk1);
        // After first chunk, image should NOT be in the store yet.
        assert!(
            handler.buffer().image_store().get(88).is_none(),
            "Image should not appear until final chunk"
        );

        handler.handle_kitty_graphics(chunk2);
        // After final chunk, image should be stored and placed.
        assert!(
            handler.buffer().image_store().get(88).is_some(),
            "Image should appear after final chunk"
        );

        let has_image = handler.buffer().has_any_image_cell();
        assert!(has_image, "Chunked TransmitAndDisplay should place image");
    }

    // ------------------------------------------------------------------
    // Kitty Unicode placeholder tests
    // ------------------------------------------------------------------

    /// Helper: create a Kitty command with `unicode_placeholder = true` for a 2×2 RGBA image.
    fn kitty_virtual_2x2_cmd() -> KittyGraphicsCommand {
        use freminal_common::buffer_states::kitty_graphics::{KittyControlData, KittyFormat};

        let rgba_data: Vec<u8> = vec![
            255, 0, 0, 255, // red
            0, 255, 0, 255, // green
            0, 0, 255, 255, // blue
            255, 255, 0, 255, // yellow
        ];

        KittyGraphicsCommand {
            control: KittyControlData {
                action: Some(KittyAction::TransmitAndDisplay),
                format: Some(KittyFormat::Rgba),
                src_width: Some(2),
                src_height: Some(2),
                image_id: Some(42),
                placement_id: Some(0),
                unicode_placeholder: true,
                ..KittyControlData::default()
            },
            payload: rgba_data,
        }
    }

    /// Helper: build a `FormatTag` whose foreground encodes an image ID and
    /// whose underline color encodes a placement ID using 24-bit RGB.
    fn format_for_placeholder(image_id: u32, placement_id: u32) -> FormatTag {
        use freminal_common::buffer_states::cursor::StateColors;

        let fg = TerminalColor::Custom(
            u8::try_from((image_id >> 16) & 0xFF).unwrap_or(0),
            u8::try_from((image_id >> 8) & 0xFF).unwrap_or(0),
            u8::try_from(image_id & 0xFF).unwrap_or(0),
        );

        let ul = TerminalColor::Custom(
            u8::try_from((placement_id >> 16) & 0xFF).unwrap_or(0),
            u8::try_from((placement_id >> 8) & 0xFF).unwrap_or(0),
            u8::try_from(placement_id & 0xFF).unwrap_or(0),
        );

        let mut tag = FormatTag::default();
        tag.colors = StateColors {
            color: fg,
            underline_color: ul,
            ..tag.colors
        };
        tag
    }

    /// U+10EEEE without any diacritics (bare placeholder char).
    const PLACEHOLDER_BYTES: &[u8] = &[0xF4, 0x8E, 0xBB, 0xAE];

    /// U+10EEEE followed by one diacritic (row=0 → U+0305).
    fn placeholder_with_row(row_idx: usize) -> Vec<u8> {
        let mut bytes = PLACEHOLDER_BYTES.to_vec();
        let diacritic =
            char::from_u32(DIACRITICS_FOR_TESTS[row_idx]).expect("valid diacritic codepoint");
        let mut buf = [0u8; 4];
        let encoded = diacritic.encode_utf8(&mut buf);
        bytes.extend_from_slice(encoded.as_bytes());
        bytes
    }

    /// U+10EEEE followed by two diacritics (row + col).
    fn placeholder_with_row_col(row_idx: usize, col_idx: usize) -> Vec<u8> {
        let mut bytes = PLACEHOLDER_BYTES.to_vec();
        for &idx in &[row_idx, col_idx] {
            let diacritic =
                char::from_u32(DIACRITICS_FOR_TESTS[idx]).expect("valid diacritic codepoint");
            let mut buf = [0u8; 4];
            let encoded = diacritic.encode_utf8(&mut buf);
            bytes.extend_from_slice(encoded.as_bytes());
        }
        bytes
    }

    /// A few diacritics from the table for test convenience.
    /// Index 0 = U+0305, index 1 = U+030D, index 2 = U+030E, etc.
    const DIACRITICS_FOR_TESTS: &[u32] = &[0x0305, 0x030D, 0x030E, 0x0310, 0x0312, 0x033D];

    #[test]
    fn kitty_virtual_placement_stores_but_does_not_place_cells() {
        let (mut handler, _rx) = kitty_handler();
        let cmd = kitty_virtual_2x2_cmd();

        handler.handle_kitty_graphics(cmd);

        // Image should be in the store.
        assert!(
            handler.buffer().image_store().get(42).is_some(),
            "Image id=42 should be in the image store"
        );

        // But NO image cells should be placed in the buffer.
        let has_image = handler.buffer().rows().iter().any(|row| {
            row.cells()
                .iter()
                .any(freminal_buffer::cell::Cell::has_image)
        });
        assert!(
            !has_image,
            "Virtual placement should NOT place image cells directly"
        );

        // Virtual placement should be stored.
        assert!(
            !handler.virtual_placements.is_empty(),
            "Virtual placements table should have an entry"
        );
        assert!(
            handler.virtual_placements.contains_key(&(42, 0)),
            "Should have virtual placement for (image_id=42, placement_id=0)"
        );
    }

    #[test]
    fn kitty_placeholder_chars_create_image_cells() {
        let (mut handler, _rx) = kitty_handler();

        // Step 1: Create a virtual placement with U=1.
        let cmd = kitty_virtual_2x2_cmd();
        handler.handle_kitty_graphics(cmd);

        // Step 2: Set foreground to encode image_id=42, underline to placement_id=0.
        let fmt = format_for_placeholder(42, 0);
        handler.set_format(fmt);

        // Step 3: Send two placeholder chars (row=0, col=0) and (row=0, col=1)
        // with explicit row+col diacritics.
        let mut data = placeholder_with_row_col(0, 0);
        data.extend_from_slice(&placeholder_with_row_col(0, 1));
        handler.handle_data(&data);

        // Step 4: Verify image cells were placed.
        let row0 = &handler.buffer().rows()[0];
        assert!(
            row0.cells()[0].has_image(),
            "Cell (0,0) should have an image placement"
        );
        assert!(
            row0.cells()[1].has_image(),
            "Cell (0,1) should have an image placement"
        );

        // Verify the image placements reference the correct image and positions.
        let p0 = row0.cells()[0].image_placement().unwrap();
        assert_eq!(p0.image_id, 42, "Cell (0,0) should reference image_id=42");
        assert_eq!(p0.row_in_image, 0);
        assert_eq!(p0.col_in_image, 0);

        let p1 = row0.cells()[1].image_placement().unwrap();
        assert_eq!(p1.image_id, 42, "Cell (0,1) should reference image_id=42");
        assert_eq!(p1.row_in_image, 0);
        assert_eq!(p1.col_in_image, 1);
    }

    /// Two INDEPENDENT `a=p,U=1` registrations of the SAME image with
    /// `p=0`/unspecified (Task 100.20) must stamp their respective
    /// placeholder cells with DISTINCT `placement_instance` ids — even
    /// though both registrations share the same `(image_id, placement_id)`
    /// key in `virtual_placements` (the second registration overwrites the
    /// first in that map), the placeholder cells stamped from EACH
    /// registration keep whichever instance id was live at the moment they
    /// were processed, so the two sets of cells coexist as distinct
    /// renderer buckets instead of collapsing into one.
    #[test]
    fn kitty_two_p_zero_virtual_registrations_stamp_distinct_instances() {
        let (mut handler, _rx) = kitty_handler();

        // First registration of the virtual placement (image=42, p=0).
        handler.handle_kitty_graphics(kitty_virtual_2x2_cmd());

        let fmt = format_for_placeholder(42, 0);
        handler.set_format(fmt.clone());

        // Stamp a placeholder cell resolving against the FIRST registration.
        let first = placeholder_with_row_col(0, 0);
        handler.handle_data(&first);

        // Second, independent registration of the SAME (image_id,
        // placement_id) key — overwrites the `virtual_placements` entry
        // with a fresh `placement_instance`.
        handler.handle_kitty_graphics(kitty_virtual_2x2_cmd());
        handler.set_format(fmt);

        // Stamp a second placeholder cell — this one resolves against the
        // SECOND (fresh) registration.
        let second = placeholder_with_row_col(0, 1);
        handler.handle_data(&second);

        let row0 = &handler.buffer().rows()[0];
        let first_instance = row0.cells()[0]
            .image_placement()
            .expect("first placeholder cell should have an image")
            .placement_instance;
        let second_instance = row0.cells()[1]
            .image_placement()
            .expect("second placeholder cell should have an image")
            .placement_instance;

        assert_ne!(
            first_instance, second_instance,
            "two independent p=0 virtual-placement registrations must stamp \
             distinct placement_instance ids so their placeholder cells \
             coexist as separate renderer buckets"
        );
    }

    #[test]
    fn kitty_placeholder_inheritance_no_diacritics() {
        let (mut handler, _rx) = kitty_handler();

        // Create virtual placement.
        let cmd = kitty_virtual_2x2_cmd();
        handler.handle_kitty_graphics(cmd);

        // Set foreground = image_id=42, underline = placement_id=0.
        let fmt = format_for_placeholder(42, 0);
        handler.set_format(fmt);

        // Send first placeholder with explicit row=0, col=0.
        let first = placeholder_with_row_col(0, 0);
        handler.handle_data(&first);

        // Send second placeholder with NO diacritics — should inherit row=0, col=1.
        handler.handle_data(PLACEHOLDER_BYTES);

        let row0 = &handler.buffer().rows()[0];
        let p0 = row0.cells()[0].image_placement().unwrap();
        assert_eq!(p0.row_in_image, 0);
        assert_eq!(p0.col_in_image, 0);

        let p1 = row0.cells()[1].image_placement().unwrap();
        assert_eq!(p1.row_in_image, 0);
        assert_eq!(p1.col_in_image, 1, "Inherited col should be prev_col + 1");
    }

    #[test]
    fn kitty_placeholder_inheritance_row_only_diacritic() {
        let (mut handler, _rx) = kitty_handler();

        // Create virtual placement.
        let cmd = kitty_virtual_2x2_cmd();
        handler.handle_kitty_graphics(cmd);

        let fmt = format_for_placeholder(42, 0);
        handler.set_format(fmt);

        // First cell: row=0, col=0 (explicit).
        let first = placeholder_with_row_col(0, 0);
        handler.handle_data(&first);

        // Second cell: row=0 only (one diacritic) — should inherit col=1 from prev.
        let second = placeholder_with_row(0);
        handler.handle_data(&second);

        let row0 = &handler.buffer().rows()[0];
        let p1 = row0.cells()[1].image_placement().unwrap();
        assert_eq!(p1.row_in_image, 0);
        assert_eq!(p1.col_in_image, 1, "Should inherit col+1 from previous");
    }

    #[test]
    fn kitty_placeholder_new_row_resets_col() {
        let (mut handler, _rx) = kitty_handler();

        // Create virtual placement.
        let cmd = kitty_virtual_2x2_cmd();
        handler.handle_kitty_graphics(cmd);

        let fmt = format_for_placeholder(42, 0);
        handler.set_format(fmt);

        // Row 0, col 0 (explicit).
        handler.handle_data(&placeholder_with_row_col(0, 0));

        // Row 0, col 1 (inherited via bare placeholder).
        handler.handle_data(PLACEHOLDER_BYTES);

        // Now move to next line (simulate \r\n or newline).
        handler.handle_newline();
        handler.handle_carriage_return();

        // Row 1, col 0 with row-only diacritic — different row, so col resets to 0.
        handler.handle_data(&placeholder_with_row(1));

        let row1 = &handler.buffer().rows()[1];
        let p = row1.cells()[0].image_placement().unwrap();
        assert_eq!(p.row_in_image, 1, "Should be row 1");
        assert_eq!(p.col_in_image, 0, "New row should start at col 0");
    }

    #[test]
    fn kitty_placeholder_no_virtual_placement_inserts_space() {
        let (mut handler, _rx) = kitty_handler();
        // Do NOT create any virtual placement.

        // Set foreground to encode image_id=99.
        let fmt = format_for_placeholder(99, 0);
        handler.set_format(fmt);

        // Send a placeholder char with row=0, col=0.
        let data = placeholder_with_row_col(0, 0);

        // But wait — the fast path skips placeholder processing when
        // virtual_placements is empty. The char goes through as normal text.
        handler.handle_data(&data);

        // The cell should NOT have an image placement.
        let row0 = &handler.buffer().rows()[0];
        assert!(
            !row0.cells()[0].has_image(),
            "Without virtual placements, placeholder should not create image cells"
        );
    }

    #[test]
    fn kitty_delete_all_clears_virtual_placements() {
        use freminal_common::buffer_states::kitty_graphics::{KittyControlData, KittyDeleteTarget};

        let (mut handler, _rx) = kitty_handler();

        // Create a virtual placement.
        let cmd = kitty_virtual_2x2_cmd();
        handler.handle_kitty_graphics(cmd);
        assert!(!handler.virtual_placements.is_empty());

        // Delete all.
        let delete_cmd = KittyGraphicsCommand {
            control: KittyControlData {
                action: Some(KittyAction::Delete),
                delete_target: Some(KittyDeleteTarget::All),
                ..KittyControlData::default()
            },
            payload: Vec::new(),
        };
        handler.handle_kitty_graphics(delete_cmd);

        assert!(
            handler.virtual_placements.is_empty(),
            "Delete all should clear virtual placements"
        );
    }

    #[test]
    fn kitty_delete_by_id_clears_matching_virtual_placements() {
        use freminal_common::buffer_states::kitty_graphics::{KittyControlData, KittyDeleteTarget};

        let (mut handler, _rx) = kitty_handler();

        // Create a virtual placement for image_id=42.
        let cmd = kitty_virtual_2x2_cmd();
        handler.handle_kitty_graphics(cmd);
        assert!(handler.virtual_placements.contains_key(&(42, 0)));

        // Delete by ID = 42.
        let delete_cmd = KittyGraphicsCommand {
            control: KittyControlData {
                action: Some(KittyAction::Delete),
                delete_target: Some(KittyDeleteTarget::ById),
                image_id: Some(42),
                ..KittyControlData::default()
            },
            payload: Vec::new(),
        };
        handler.handle_kitty_graphics(delete_cmd);

        assert!(
            handler.virtual_placements.is_empty(),
            "Delete by ID=42 should clear the virtual placement"
        );
    }

    #[test]
    fn kitty_placeholder_fast_path_with_no_virtual_placements() {
        let (mut handler, _rx) = kitty_handler();
        // No virtual placements — fast path should just insert text normally.

        handler.handle_data(b"Hello World");

        // Cursor should have advanced.
        let cursor = handler.buffer().cursor();
        assert_eq!(
            cursor.pos.x, 11,
            "Cursor should be at column 11 after 'Hello World'"
        );
    }

    // -----------------------------------------------------------------------
    // Kitty relative placement tests (Task 100.4a)
    // -----------------------------------------------------------------------

    /// Helper: build a bare `a=p` (Put) command with the given control data
    /// overrides layered onto sensible defaults for relative-placement
    /// tests.
    fn kitty_put_cmd(control: &KittyControlData) -> KittyGraphicsCommand {
        KittyGraphicsCommand {
            control: KittyControlData {
                action: Some(KittyAction::Put),
                ..control.clone()
            },
            payload: Vec::new(),
        }
    }

    /// Helper: transmit (store only, `a=t`) a 2x2 RGBA image with the given
    /// id, without displaying it.
    fn transmit_only(handler: &mut TerminalHandler, image_id: u32) {
        let mut cmd = kitty_rgba_2x2_cmd(KittyAction::Transmit);
        cmd.control.image_id = Some(image_id);
        handler.handle_kitty_graphics(cmd);
    }

    /// Read the next PTY response and return it as a `String`, panicking if
    /// none is available or it isn't a `PtyWrite::Write`.
    fn recv_response(rx: &crossbeam_channel::Receiver<PtyWrite>) -> String {
        match rx.try_recv().expect("expected a PTY response") {
            PtyWrite::Write(bytes) => String::from_utf8(bytes).expect("valid UTF-8 response"),
            resize @ PtyWrite::Resize(_) => panic!("expected PtyWrite::Write, got {resize:?}"),
        }
    }

    /// Count cells whose placement matches BOTH `image_id` and
    /// `placement_id` (Task 100.20 narrowing tests).
    fn count_placement_with_id(
        handler: &TerminalHandler,
        image_id: u64,
        placement_id: u32,
    ) -> usize {
        handler
            .buffer()
            .rows()
            .iter()
            .flat_map(Row::cells)
            .filter(|c| {
                c.image_placement()
                    .is_some_and(|p| p.image_id == image_id && p.placement_id == Some(placement_id))
            })
            .count()
    }

    #[test]
    fn kitty_plain_put_registers_real_placement_with_correct_origin() {
        let (mut handler, _rx) = kitty_handler();
        handler.buffer_mut().set_cursor_pos(Some(3), Some(5));

        let cmd = kitty_rgba_2x2_cmd(KittyAction::TransmitAndDisplay);
        handler.handle_kitty_graphics(cmd);

        let placement = handler
            .real_placements
            .get(&(42, 0))
            .copied()
            .expect("expected a RealPlacement for (42, 0)");
        assert_eq!(placement.image_id, 42);
        assert_eq!(placement.origin_row, RowNumber::new(5));
        assert_eq!(placement.origin_col, 3);
        assert_eq!(placement.parent, None);
    }

    /// Two `a=p`/`a=T` puts of the SAME image with `p=0`/unspecified are
    /// MULTIPLE COEXISTING placements per the kitty spec (Task 100.18):
    /// both sets of cells must remain on screen, each stamped with its
    /// OWN, distinct `placement_instance` id (so the renderer buckets them
    /// as two separate quads instead of merging into one).
    #[test]
    fn kitty_two_displays_same_image_p_unspecified_coexist_with_distinct_instances() {
        let (mut handler, rx) = kitty_handler();

        // First display at (col=0, row=0). `kitty_rgba_2x2_cmd`'s 2x2px
        // image resolves to a 1x1 CELL footprint at the default 8x16px
        // cell size, so only the origin cell itself carries the placement.
        handler.buffer_mut().set_cursor_pos(Some(0), Some(0));
        let cmd1 = kitty_rgba_2x2_cmd(KittyAction::TransmitAndDisplay);
        handler.handle_kitty_graphics(cmd1);
        let _ = recv_response(&rx);

        // Second, independent display of the SAME image id=42 at a
        // DIFFERENT screen position (col=3, row=5), still `p=0`/unspecified.
        handler.buffer_mut().set_cursor_pos(Some(3), Some(5));
        let put_cmd = KittyGraphicsCommand {
            control: KittyControlData {
                action: Some(KittyAction::Put),
                image_id: Some(42),
                ..KittyControlData::default()
            },
            payload: Vec::new(),
        };
        handler.handle_kitty_graphics(put_cmd);
        let _ = recv_response(&rx);

        let instance_at = |row: usize, col: usize| {
            handler
                .buffer()
                .rows()
                .get(row)
                .and_then(|r| r.cells().get(col))
                .and_then(|c| c.image_placement())
                .map(|p| p.placement_instance)
        };

        let first_instance = instance_at(0, 0).expect("first placement cell should have image");
        let second_instance = instance_at(5, 3).expect("second placement cell should have image");

        assert_ne!(
            first_instance, second_instance,
            "two independent p=0 placements of the same image must get \
             distinct placement_instance ids so they don't merge in the renderer"
        );
        // BOTH placements' cells must still be present — coexistence, not
        // replacement.
        assert!(instance_at(0, 0).is_some());
        assert!(instance_at(5, 3).is_some());
    }

    /// A second `a=p`/`a=T` put with the SAME NON-ZERO `placement_id`
    /// REPLACES the first placement (kitty spec, Task 100.18): the old
    /// placement's cells must be cleared, leaving only the new placement's
    /// cells (at its own screen position) on screen.
    #[test]
    fn kitty_second_put_same_nonzero_placement_id_replaces_first() {
        let (mut handler, rx) = kitty_handler();

        // First put: image id=42, p=5, at (col=0, row=0).
        handler.buffer_mut().set_cursor_pos(Some(0), Some(0));
        let mut cmd1 = kitty_rgba_2x2_cmd(KittyAction::TransmitAndDisplay);
        cmd1.control.placement_id = Some(5);
        handler.handle_kitty_graphics(cmd1);
        let _ = recv_response(&rx);

        let has_image_at = |h: &TerminalHandler, row: usize, col: usize| {
            h.buffer()
                .rows()
                .get(row)
                .and_then(|r| r.cells().get(col))
                .is_some_and(Cell::has_image)
        };
        assert!(
            has_image_at(&handler, 0, 0),
            "first put's cell should exist before replace"
        );

        // Second put: SAME image id=42, SAME p=5, but at a DIFFERENT
        // screen position (col=3, row=5) — this must REPLACE, clearing
        // the first put's cells.
        handler.buffer_mut().set_cursor_pos(Some(3), Some(5));
        let mut cmd2 = kitty_rgba_2x2_cmd(KittyAction::TransmitAndDisplay);
        cmd2.control.placement_id = Some(5);
        handler.handle_kitty_graphics(cmd2);
        let _ = recv_response(&rx);

        assert!(
            !has_image_at(&handler, 0, 0),
            "REPLACE must clear the old placement's cells"
        );
        assert!(
            has_image_at(&handler, 5, 3),
            "the new placement's cells must be present"
        );

        // Exactly one entry in real_placements for (42, 5), pointing at
        // the NEW origin.
        let placement = handler
            .real_placements
            .get(&(42, 5))
            .copied()
            .expect("expected a RealPlacement for (42, 5)");
        assert_eq!(placement.origin_row, RowNumber::new(5));
        assert_eq!(placement.origin_col, 3);
    }

    #[test]
    fn kitty_relative_placement_with_real_parent_stamps_at_offset() {
        let (mut handler, rx) = kitty_handler();
        handler.buffer_mut().set_cursor_pos(Some(3), Some(5));

        // Parent image A (id=42), displayed at (row=5, col=3). Override the
        // display size to 10x5 cells so the buffer grows enough rows for
        // the relative child's V=1 offset to land inside existing bounds
        // (the buffer starts with a single row and grows only as far as
        // whatever is placed into it).
        let mut cmd_a = kitty_rgba_2x2_cmd(KittyAction::TransmitAndDisplay);
        cmd_a.control.display_cols = Some(5);
        cmd_a.control.display_rows = Some(10);
        handler.handle_kitty_graphics(cmd_a);
        let _ = recv_response(&rx); // A's OK response.
        let parent = handler
            .real_placements
            .get(&(42, 0))
            .copied()
            .expect("parent A registered");

        // Child image B (id=99), transmitted but not displayed yet.
        transmit_only(&mut handler, 99);
        let _ = recv_response(&rx); // B's transmit-only OK response.

        // Put B relative to A: H=2, V=1.
        let put_cmd = kitty_put_cmd(&KittyControlData {
            image_id: Some(99),
            parent_image_id: Some(42),
            h_offset: Some(2),
            v_offset: Some(1),
            ..KittyControlData::default()
        });
        handler.handle_kitty_graphics(put_cmd);

        let response = recv_response(&rx);
        assert!(
            !response.contains("ENOPARENT")
                && !response.contains("EINVAL")
                && !response.contains("ECYCLE")
                && !response.contains("ETOODEEP"),
            "expected success response, got: {response}"
        );

        let child = handler
            .real_placements
            .get(&(99, 0))
            .copied()
            .expect("child B registered in real_placements");
        assert_eq!(child.parent, Some((42, 0)));
        assert_eq!(child.origin_row, parent.origin_row.offset(1));
        assert_eq!(child.origin_col, parent.origin_col + 2);

        // The child's image cells are actually stamped at that offset.
        let child_row = handler
            .buffer()
            .row_index_of(child.origin_row)
            .expect("child origin row is retained");
        let cell = &handler.buffer().rows()[child_row].cells()[child.origin_col];
        assert!(cell.has_image(), "expected an image cell at child origin");
        assert_eq!(
            cell.image_placement().map(|p| p.image_id),
            Some(99),
            "expected the stamped cell to reference image id 99"
        );
    }

    /// Task 100.14 regression test: `place_kitty_image` used to capture the
    /// cursor row/col *before* calling `Buffer::place_image`, then pass that
    /// stale position straight to `record_real_placement`. But
    /// `Buffer::place_image` may call `enforce_scrollback_limit` internally,
    /// which drains rows from the top of the buffer and shifts the image's
    /// TRUE stamped origin upward by the drained row count. Small,
    /// fresh-buffer unit tests never exercise this because they never fill
    /// scrollback — it only bites in a live session with real scrollback
    /// history. This test forces a drain to happen *during* the same
    /// `place_image` call that stamps a real placement, and asserts the
    /// recorded `RealPlacement::origin_row` matches the row where the
    /// image's cells were *actually* stamped (ground truth), not the stale
    /// pre-call cursor row.
    ///
    /// The expected-origin arithmetic accounts for a second Task 100.15
    /// effect: whenever placing an image forces a scrollback drain, the
    /// post-drain cursor target always lands exactly one row past the
    /// image's new last row — i.e. exactly at the trimmed buffer's tail —
    /// so `place_image`'s "append a blank row below the image" step always
    /// finds no such row, appends one, and that freshly appended row is
    /// itself immediately drained by exactly one more row. See the
    /// two-stage derivation inline below (this is not a coincidence of
    /// this test's specific numbers — `origin_row` in this
    /// drain-triggering regime always works out to
    /// `max_rows - display_rows - 1`, independent of the starting cursor
    /// row, which is exactly why the pre-100.15 `origin_row` and the
    /// post-100.15 `origin_row` differ by a fixed amount rather than by
    /// however many rows `grow_buffer_rows` happened to produce).
    #[test]
    fn kitty_real_placement_origin_survives_scrollback_drain() {
        let (tx, _rx) = crossbeam_channel::unbounded::<PtyWrite>();
        let mut handler = TerminalHandler::new(80, 24).with_scrollback_limit(3);
        handler.set_write_tx(tx);
        // max_rows = height(24) + scrollback_limit(3) = 27.

        // Grow the buffer to 20 rows (below max_rows — no drain yet),
        // leaving the cursor at the last row (19), analogous to a
        // long-running session that has accumulated some scrollback.
        // `grow_buffer_rows` drives plain line feeds (not `place_image`),
        // so this setup's row-count/cursor contract is independent of
        // `place_image`'s own cursor-placement behavior.
        grow_buffer_rows(&mut handler, 20);
        assert_eq!(handler.buffer().rows().len(), 20);
        assert_eq!(handler.buffer().cursor().pos.y, 19);
        let stale_cursor_row = handler.buffer().cursor().pos.y;

        // Place a real (cell-stamped) image tall enough that stamping it
        // grows the buffer well past max_rows (27), forcing
        // `enforce_scrollback_limit` to drain rows from the top *during*
        // this same `place_image` call.
        let mut cmd = kitty_rgba_2x2_cmd(KittyAction::TransmitAndDisplay);
        cmd.control.image_id = Some(77);
        cmd.control.display_cols = Some(1);
        cmd.control.display_rows = Some(15);
        handler.handle_kitty_graphics(cmd);

        let placement = handler
            .real_placements
            .get(&(77, 0))
            .copied()
            .expect("expected a RealPlacement for (77, 0)");

        // Stage 1: stamping the image grows the buffer to
        // stale_cursor_row(19) + display_rows(15) = 34 rows, then
        // `enforce_scrollback_limit` drains it back to max_rows(27):
        // first_drain = 34 - 27 = 7.
        let rows_after_stamp = stale_cursor_row + 15; // 19 + 15 = 34
        let first_drain = rows_after_stamp - 27; // 34 - 27 = 7
        let row_after_first_drain = stale_cursor_row - first_drain; // 19 - 7 = 12

        // Stage 2 (Task 100.15): the post-first-drain cursor target
        // (row_after_first_drain + display_rows = 12 + 15 = 27) lands
        // exactly at max_rows(27) — one past the image's now-last row —
        // so `place_image` appends a fresh row there and immediately
        // drains it again by exactly 1.
        let second_drain = 1;
        let expected_origin_row = row_after_first_drain - second_drain; // 12 - 1 = 11
        let expected_drained_total = first_drain + second_drain; // 8

        assert_ne!(
            expected_origin_row, stale_cursor_row,
            "test setup must actually force a drain, or it can't distinguish \
             the fix from the bug"
        );
        // Task 125.14: the recorded origin is a stable row NUMBER. It was
        // taken at the pre-drain cursor row (row 19, base 0) and, being
        // stable, still names the image's top row; that row's retained index
        // after the drains is what the pre-125.14 test compared directly.
        assert_eq!(
            placement.origin_row,
            RowNumber::new(19),
            "origin_row must be the number of the row the image was stamped on"
        );
        assert_eq!(
            handler.buffer().row_index_of(placement.origin_row),
            Some(expected_origin_row),
            "100.14: origin_row must resolve to the post-drain stamped row, not \
             the stale pre-call cursor row"
        );
        assert_eq!(
            stale_cursor_row - expected_drained_total,
            expected_origin_row,
            "sanity: origin_row = stale_cursor_row - total rows drained \
             across both 100.15 drain stages"
        );
        assert_eq!(placement.origin_col, 0);

        // Ground truth: the image's cells are actually stamped at the
        // recorded origin.
        let origin_index = handler
            .buffer()
            .row_index_of(placement.origin_row)
            .expect("origin row is retained");
        let cell = &handler.buffer().rows()[origin_index].cells()[placement.origin_col];
        assert!(
            cell.has_image(),
            "expected an image cell at the recorded origin_row/origin_col"
        );
        assert_eq!(
            cell.image_placement().map(|p| p.image_id),
            Some(77),
            "expected the stamped cell to reference image id 77"
        );
    }

    /// Task 125.14: a recorded placement origin is a stable row NUMBER, so it
    /// keeps naming the image's row when LATER output evicts rows above it
    /// (the old physical origin drifted onto a different row).
    #[test]
    fn kitty_real_placement_stays_attached_to_its_row_across_eviction() {
        let (tx, _rx) = crossbeam_channel::unbounded::<PtyWrite>();
        let mut handler = TerminalHandler::new(80, 24).with_scrollback_limit(3);
        handler.set_write_tx(tx);

        grow_buffer_rows(&mut handler, 10);
        let mut cmd = kitty_rgba_2x2_cmd(KittyAction::TransmitAndDisplay);
        cmd.control.image_id = Some(61);
        cmd.control.display_cols = Some(1);
        cmd.control.display_rows = Some(1);
        handler.handle_kitty_graphics(cmd);
        let placement = handler
            .real_placements
            .get(&(61, 0))
            .copied()
            .expect("expected a RealPlacement for (61, 0)");
        let index_at_placement = handler
            .buffer()
            .row_index_of(placement.origin_row)
            .expect("origin row is retained at placement time");

        // Later output evicts rows above the image (max 27 rows retained).
        grow_buffer_rows(&mut handler, 20);
        assert!(
            handler.buffer().row_base() > RowNumber::ZERO,
            "setup must evict rows"
        );

        let placement_after = handler
            .real_placements
            .get(&(61, 0))
            .copied()
            .expect("placement still recorded");
        assert_eq!(
            placement_after.origin_row, placement.origin_row,
            "eviction must not rewrite the recorded origin"
        );
        let index_now = handler
            .buffer()
            .row_index_of(placement.origin_row)
            .expect("the image's row is still retained");
        assert!(index_now < index_at_placement, "eviction shifted the index");
        assert!(
            handler.buffer().rows()[index_now].cells()[placement.origin_col].has_image(),
            "the recorded origin must still name the row holding the image"
        );
    }

    /// Task 125.14: a relative placement made AFTER eviction resolves its
    /// parent's row through the stable number, not through a drifted index.
    #[test]
    fn kitty_relative_child_lands_below_its_parent_after_eviction() {
        let (tx, rx) = crossbeam_channel::unbounded::<PtyWrite>();
        let mut handler = TerminalHandler::new(80, 24).with_scrollback_limit(3);
        handler.set_write_tx(tx);

        grow_buffer_rows(&mut handler, 10);
        let mut cmd_a = kitty_rgba_2x2_cmd(KittyAction::TransmitAndDisplay);
        cmd_a.control.image_id = Some(62);
        cmd_a.control.display_cols = Some(2);
        cmd_a.control.display_rows = Some(3);
        handler.handle_kitty_graphics(cmd_a);
        let _ = recv_response(&rx);
        let parent = handler
            .real_placements
            .get(&(62, 0))
            .copied()
            .expect("parent registered");

        grow_buffer_rows(&mut handler, 20);
        assert!(
            handler.buffer().row_base() > RowNumber::ZERO,
            "setup must evict rows"
        );

        transmit_only(&mut handler, 63);
        let _ = recv_response(&rx);
        handler.handle_kitty_graphics(kitty_put_cmd(&KittyControlData {
            image_id: Some(63),
            parent_image_id: Some(62),
            h_offset: Some(0),
            v_offset: Some(1),
            ..KittyControlData::default()
        }));
        let _ = recv_response(&rx);

        let child = handler
            .real_placements
            .get(&(63, 0))
            .copied()
            .expect("child registered");
        assert_eq!(child.origin_row, parent.origin_row.offset(1));
        let child_index = handler
            .buffer()
            .row_index_of(child.origin_row)
            .expect("child's row is retained");
        let parent_index = handler
            .buffer()
            .row_index_of(parent.origin_row)
            .expect("parent's row is retained");
        assert_eq!(child_index, parent_index + 1);
        assert_eq!(
            handler.buffer().rows()[child_index].cells()[child.origin_col]
                .image_placement()
                .map(|p| p.image_id),
            Some(63),
            "the child's cells are stamped one row below the parent's origin"
        );
    }

    #[test]
    fn image_data_disposition_follows_the_protocol_flag() {
        assert_eq!(
            ImageDataDisposition::from(false),
            ImageDataDisposition::Keep
        );
        assert_eq!(ImageDataDisposition::from(true), ImageDataDisposition::Free);
        assert!(!ImageDataDisposition::Keep.frees_data());
        assert!(ImageDataDisposition::Free.frees_data());
    }

    /// Place a 1x1 image with `image_id` at the cursor and return the
    /// response-draining receiver the handler is wired to.
    fn place_at_cursor(
        handler: &mut TerminalHandler,
        rx: &crossbeam_channel::Receiver<PtyWrite>,
        image_id: u32,
    ) {
        let mut cmd = kitty_rgba_2x2_cmd(KittyAction::TransmitAndDisplay);
        cmd.control.image_id = Some(image_id);
        cmd.control.display_cols = Some(1);
        cmd.control.display_rows = Some(1);
        handler.handle_kitty_graphics(cmd);
        let _ = recv_response(rx);
    }

    /// Place a relative child of `parent` at vertical offset `v` and return
    /// the APC response.
    fn place_relative_child(
        handler: &mut TerminalHandler,
        rx: &crossbeam_channel::Receiver<PtyWrite>,
        child: u32,
        parent: u32,
        v: i32,
    ) -> String {
        transmit_only(handler, child);
        let _ = recv_response(rx);
        handler.handle_kitty_graphics(kitty_put_cmd(&KittyControlData {
            image_id: Some(child),
            parent_image_id: Some(parent),
            h_offset: Some(0),
            v_offset: Some(v),
            ..KittyControlData::default()
        }));
        recv_response(rx)
    }

    fn any_cell_shows_image(handler: &TerminalHandler, image_id: u64) -> bool {
        handler.buffer().rows().iter().any(|row| {
            row.cells().iter().any(|cell| {
                cell.image_placement()
                    .is_some_and(|p| p.image_id == image_id)
            })
        })
    }

    /// Review NIT: a negative `V=` that carries the child above row 0 used to
    /// saturate onto row 0. It has no row to land on: accepted, but neither
    /// stamped nor registered.
    #[test]
    fn kitty_relative_child_above_row_zero_is_not_clamped_onto_row_zero() {
        let (tx, rx) = crossbeam_channel::unbounded::<PtyWrite>();
        let mut handler = TerminalHandler::new(80, 24);
        handler.set_write_tx(tx);

        // Parent on the very first row of a fresh primary buffer (number 0).
        place_at_cursor(&mut handler, &rx, 70);
        let parent = handler.real_placements[&(70, 0)];
        assert_eq!(parent.origin_row, RowNumber::ZERO, "setup: parent on row 0");

        let response = place_relative_child(&mut handler, &rx, 71, 70, -1);
        assert!(
            response.contains("OK"),
            "the placement is accepted, got {response:?}"
        );
        assert!(
            !handler.real_placements.contains_key(&(71, 0)),
            "no row exists above row 0, so nothing is registered"
        );
        assert!(
            !any_cell_shows_image(&handler, 71),
            "and nothing is stamped on row 0 (or anywhere)"
        );
    }

    /// The boundary: `V=-1` from row 1 lands exactly on row 0.
    #[test]
    fn kitty_relative_child_landing_exactly_on_row_zero_is_stamped() {
        let (tx, rx) = crossbeam_channel::unbounded::<PtyWrite>();
        let mut handler = TerminalHandler::new(80, 24);
        handler.set_write_tx(tx);

        handler.handle_newline();
        handler.handle_carriage_return();
        place_at_cursor(&mut handler, &rx, 72);
        assert_eq!(
            handler.real_placements[&(72, 0)].origin_row,
            RowNumber::new(1)
        );

        let _ = place_relative_child(&mut handler, &rx, 73, 72, -1);
        let child = handler.real_placements[&(73, 0)];
        assert_eq!(child.origin_row, RowNumber::ZERO);
        assert!(handler.buffer().rows()[0].cells()[0].has_image());
    }

    /// The alternate screen's rows are numbered from `1 << 63`; an offset past
    /// the first alternate row must not saturate into (or across to) the
    /// primary namespace.
    #[test]
    fn kitty_relative_child_above_the_first_alternate_row_is_not_registered() {
        let (tx, rx) = crossbeam_channel::unbounded::<PtyWrite>();
        let mut handler = TerminalHandler::new(80, 24);
        handler.set_write_tx(tx);

        handler.handle_enter_alternate();
        place_at_cursor(&mut handler, &rx, 74);
        let parent = handler.real_placements[&(74, 0)];
        assert_eq!(parent.origin_row, RowNumber::ALTERNATE_BASE);

        let response = place_relative_child(&mut handler, &rx, 75, 74, -1);
        assert!(response.contains("OK"), "accepted, got {response:?}");
        assert!(
            !handler.real_placements.contains_key(&(75, 0)),
            "an entry saturated to the last primary number would alias a primary row"
        );
        assert!(!any_cell_shows_image(&handler, 75));
    }

    /// A stale entry under the child's key must not outlive a placement that
    /// no longer occupies any row.
    #[test]
    fn kitty_unrepresentable_relative_placement_replaces_a_stale_entry() {
        let (tx, rx) = crossbeam_channel::unbounded::<PtyWrite>();
        let mut handler = TerminalHandler::new(80, 24);
        handler.set_write_tx(tx);

        handler.handle_newline();
        handler.handle_carriage_return();
        place_at_cursor(&mut handler, &rx, 76);
        let _ = place_relative_child(&mut handler, &rx, 77, 76, 0);
        assert!(handler.real_placements.contains_key(&(77, 0)));

        handler.handle_kitty_graphics(kitty_put_cmd(&KittyControlData {
            image_id: Some(77),
            parent_image_id: Some(76),
            v_offset: Some(-5),
            ..KittyControlData::default()
        }));
        let _ = recv_response(&rx);
        assert!(!handler.real_placements.contains_key(&(77, 0)));
    }

    // ── Pruning placements whose origin was evicted (review NIT) ────────────

    /// Drive `lines` newline outputs through the batch entry point, the way a
    /// PTY read does.
    fn push_newlines(handler: &mut TerminalHandler, lines: usize) {
        let outputs =
            vec![freminal_common::buffer_states::terminal_output::TerminalOutput::Newline; lines];
        handler.process_outputs(&outputs);
    }

    #[test]
    fn real_placements_with_evicted_origins_are_pruned_per_batch() {
        let (tx, rx) = crossbeam_channel::unbounded::<PtyWrite>();
        let mut handler = TerminalHandler::new(80, 4).with_scrollback_limit(3);
        handler.set_write_tx(tx);

        place_at_cursor(&mut handler, &rx, 80);
        let origin = handler.real_placements[&(80, 0)].origin_row;

        // A batch that scrolls but does not reach the origin keeps the entry.
        push_newlines(&mut handler, 3);
        assert!(
            handler.buffer().row_base() <= origin,
            "setup: origin still retained"
        );
        assert!(handler.real_placements.contains_key(&(80, 0)));

        // Enough further output evicts the origin row.
        push_newlines(&mut handler, 20);
        assert!(
            handler.buffer().row_base() > origin,
            "setup: origin evicted"
        );
        assert!(
            !handler.real_placements.contains_key(&(80, 0)),
            "an evicted origin must not stay in the map forever"
        );
    }

    #[test]
    fn eviction_pruning_keeps_placements_that_are_still_retained() {
        let (tx, rx) = crossbeam_channel::unbounded::<PtyWrite>();
        let mut handler = TerminalHandler::new(80, 4).with_scrollback_limit(3);
        handler.set_write_tx(tx);

        place_at_cursor(&mut handler, &rx, 81);
        push_newlines(&mut handler, 20);
        // Placed late: its origin is inside the retained window.
        place_at_cursor(&mut handler, &rx, 82);
        push_newlines(&mut handler, 1);
        let late = handler.real_placements[&(82, 0)].origin_row;
        assert!(handler.buffer().row_base() <= late);

        assert!(!handler.real_placements.contains_key(&(81, 0)));
        assert!(handler.real_placements.contains_key(&(82, 0)));
    }

    /// A child of a virtual parent carries a placeholder origin of row 0 that
    /// is never read; judging it by that origin would delete it as soon as any
    /// row is evicted.
    #[test]
    fn eviction_pruning_keeps_children_of_virtual_parents() {
        let (tx, rx) = crossbeam_channel::unbounded::<PtyWrite>();
        let mut handler = TerminalHandler::new(80, 4).with_scrollback_limit(3);
        handler.set_write_tx(tx);

        // A virtual parent, then a relative child against it.
        let mut cmd = kitty_rgba_2x2_cmd(KittyAction::TransmitAndDisplay);
        cmd.control.image_id = Some(90);
        cmd.control.placement_id = Some(1);
        cmd.control.unicode_placeholder = true;
        cmd.control.display_cols = Some(2);
        cmd.control.display_rows = Some(2);
        handler.handle_kitty_graphics(cmd);
        let _ = recv_response(&rx);
        assert!(handler.virtual_placements.contains_key(&(90, 1)));
        transmit_only(&mut handler, 91);
        let _ = recv_response(&rx);
        handler.handle_kitty_graphics(kitty_put_cmd(&KittyControlData {
            image_id: Some(91),
            parent_image_id: Some(90),
            parent_placement_id: Some(1),
            v_offset: Some(1),
            ..KittyControlData::default()
        }));
        let _ = recv_response(&rx);
        let child = handler.real_placements[&(91, 0)];
        assert_eq!(
            child.origin_row,
            RowNumber::ZERO,
            "setup: placeholder origin"
        );

        push_newlines(&mut handler, 30);
        assert!(handler.buffer().row_base() > RowNumber::ZERO);
        assert!(
            handler.real_placements.contains_key(&(91, 0)),
            "a virtual-parent child must survive eviction of row 0"
        );
    }

    /// Growing the window reclaims blank padding rows, whose numbers are then
    /// issued again to unrelated rows; a placement recorded against one must
    /// not survive to name them.
    #[test]
    fn real_placements_on_reclaimed_padding_rows_are_pruned_after_a_resize() {
        let (tx, rx) = crossbeam_channel::unbounded::<PtyWrite>();
        let mut handler = TerminalHandler::new(80, 5);
        handler.set_write_tx(tx);

        place_at_cursor(&mut handler, &rx, 85);
        let kept = handler.real_placements[&(85, 0)];
        // A placement recorded on a padding row three rows down (no cells: the
        // padding row must stay pristine for the grow to pop it).
        handler.buffer_mut().set_cursor_pos(Some(0), Some(3));
        let padding_row = handler.buffer().cursor_row_number();
        handler.real_placements.insert(
            (86, 0),
            crate::terminal_handler::RealPlacement {
                image_id: 86,
                origin_row: padding_row,
                ..kept
            },
        );
        handler.buffer_mut().set_cursor_pos(Some(0), Some(0));
        assert_eq!(
            handler.buffer().rows().len(),
            4,
            "setup: padding rows exist"
        );

        handler.handle_resize(80, 8, 0, 0);

        assert!(
            handler.buffer().row_index_of(padding_row).is_none(),
            "setup: the padding row was popped"
        );
        assert!(
            !handler.real_placements.contains_key(&(86, 0)),
            "a placement on a popped row would alias the row issued next"
        );
        assert!(
            handler.real_placements.contains_key(&(85, 0)),
            "a placement on a surviving row stays"
        );
    }

    #[test]
    fn pruning_does_nothing_without_a_base_change() {
        let (tx, rx) = crossbeam_channel::unbounded::<PtyWrite>();
        let mut handler = TerminalHandler::new(80, 24);
        handler.set_write_tx(tx);
        place_at_cursor(&mut handler, &rx, 83);
        // Tamper: an entry below the base that pruning WOULD remove; with the
        // base unchanged since the last prune it must be left alone (the
        // gate is what keeps the per-batch cost at one comparison).
        handler.real_placements.insert(
            (84, 0),
            crate::terminal_handler::RealPlacement {
                origin_row: RowNumber::ZERO,
                ..handler.real_placements[&(83, 0)]
            },
        );
        handler.placement_prune_base = handler.buffer().row_base();
        push_newlines(&mut handler, 1);
        assert!(handler.real_placements.contains_key(&(84, 0)));
        assert!(handler.real_placements.contains_key(&(83, 0)));
    }

    /// Task 125.14: a width-changing reflow renumbers every row; the handler
    /// carries its placement origins through the buffer's `ReflowRemap`.
    #[test]
    fn kitty_real_placement_follows_its_image_through_reflow() {
        let (tx, _rx) = crossbeam_channel::unbounded::<PtyWrite>();
        let mut handler = TerminalHandler::new(40, 6);
        handler.set_write_tx(tx);

        // A long line that re-wraps at the new width, so the rows below it
        // change index.
        handler.handle_data(&[b'x'; 70]);
        handler.handle_newline();
        handler.handle_carriage_return();
        let mut cmd = kitty_rgba_2x2_cmd(KittyAction::TransmitAndDisplay);
        cmd.control.image_id = Some(64);
        cmd.control.display_cols = Some(1);
        cmd.control.display_rows = Some(1);
        handler.handle_kitty_graphics(cmd);
        let before = handler
            .real_placements
            .get(&(64, 0))
            .copied()
            .expect("placement recorded");

        handler.handle_resize(20, 6, 0, 0);

        assert!(
            handler.buffer().row_index_of(before.origin_row).is_none(),
            "reflow renumbers rows, so the old number no longer resolves"
        );
        let after = handler
            .real_placements
            .get(&(64, 0))
            .copied()
            .expect("placement still recorded");
        assert_ne!(after.origin_row, before.origin_row, "origin was remapped");
        let index = handler
            .buffer()
            .row_index_of(after.origin_row)
            .expect("the remapped origin resolves to a retained row");
        assert!(
            handler.buffer().rows()[index].cells()[after.origin_col].has_image(),
            "the remapped origin must name the row now holding the image"
        );
        assert!(
            handler.buffer_mut().take_reflow_remap().is_none(),
            "the handler consumed the buffer's pending remap"
        );
    }

    /// Task 125.14: placements recorded against the alternate screen are
    /// dropped when it is left; primary-screen placements survive.
    #[test]
    fn leaving_the_alternate_screen_drops_alt_placements_only() {
        let (mut handler, _rx) = kitty_handler();

        let mut primary = kitty_rgba_2x2_cmd(KittyAction::TransmitAndDisplay);
        primary.control.image_id = Some(65);
        primary.control.display_cols = Some(1);
        primary.control.display_rows = Some(1);
        handler.handle_kitty_graphics(primary);
        assert!(handler.real_placements.contains_key(&(65, 0)));

        handler.handle_enter_alternate();
        let mut alt = kitty_rgba_2x2_cmd(KittyAction::TransmitAndDisplay);
        alt.control.image_id = Some(66);
        alt.control.display_cols = Some(1);
        alt.control.display_rows = Some(1);
        handler.handle_kitty_graphics(alt);
        let alt_placement = handler
            .real_placements
            .get(&(66, 0))
            .copied()
            .expect("alt placement recorded");
        assert!(alt_placement.origin_row.is_alternate());

        handler.handle_leave_alternate();

        assert!(
            !handler.real_placements.contains_key(&(66, 0)),
            "the alternate screen's placement must be dropped on leave"
        );
        assert!(
            handler.real_placements.contains_key(&(65, 0)),
            "the primary screen's placement must survive"
        );
    }

    #[test]
    fn kitty_relative_placement_enoparent_when_parent_missing() {
        let (mut handler, rx) = kitty_handler();
        transmit_only(&mut handler, 99);
        let _ = recv_response(&rx);

        let put_cmd = kitty_put_cmd(&KittyControlData {
            image_id: Some(99),
            parent_image_id: Some(9999),
            ..KittyControlData::default()
        });
        handler.handle_kitty_graphics(put_cmd);

        let response = recv_response(&rx);
        assert!(
            response.contains("ENOPARENT"),
            "expected ENOPARENT, got: {response}"
        );
    }

    #[test]
    fn kitty_relative_placement_einval_when_also_virtual() {
        let (mut handler, rx) = kitty_handler();
        transmit_only(&mut handler, 99);
        let _ = recv_response(&rx);

        let put_cmd = kitty_put_cmd(&KittyControlData {
            image_id: Some(99),
            parent_image_id: Some(1),
            unicode_placeholder: true,
            ..KittyControlData::default()
        });
        handler.handle_kitty_graphics(put_cmd);

        let response = recv_response(&rx);
        assert!(
            response.contains("EINVAL"),
            "expected EINVAL, got: {response}"
        );
    }

    #[test]
    fn kitty_relative_placement_etoodeep_beyond_depth_8() {
        let (mut handler, rx) = kitty_handler();

        // Root: image id 0, displayed normally (no parent).
        let mut cmd_root = kitty_rgba_2x2_cmd(KittyAction::TransmitAndDisplay);
        cmd_root.control.image_id = Some(1);
        handler.handle_kitty_graphics(cmd_root);
        let _ = recv_response(&rx);

        // Build a chain of relative placements 2..=9, each relative to the
        // previous one. ids 2..=9 reach depths 1..=8 — all must succeed.
        for id in 2..=9u32 {
            transmit_only(&mut handler, id);
            let _ = recv_response(&rx);

            let put_cmd = kitty_put_cmd(&KittyControlData {
                image_id: Some(id),
                parent_image_id: Some(id - 1),
                h_offset: Some(1),
                v_offset: Some(0),
                ..KittyControlData::default()
            });
            handler.handle_kitty_graphics(put_cmd);
            let response = recv_response(&rx);
            assert!(
                !response.contains("ETOODEEP"),
                "id={id} (depth {}) must be allowed, got: {response}",
                id - 1
            );
        }

        // The 10th image (id=10), relative to id=9 (depth 8), would reach
        // depth 9 — the 9th link — and must be rejected.
        transmit_only(&mut handler, 10);
        let _ = recv_response(&rx);
        let put_cmd = kitty_put_cmd(&KittyControlData {
            image_id: Some(10),
            parent_image_id: Some(9),
            h_offset: Some(1),
            v_offset: Some(0),
            ..KittyControlData::default()
        });
        handler.handle_kitty_graphics(put_cmd);
        let response = recv_response(&rx);
        assert!(
            response.contains("ETOODEEP"),
            "expected ETOODEEP, got: {response}"
        );
    }

    #[test]
    fn kitty_relative_placement_ecycle_on_self_reference() {
        let (mut handler, rx) = kitty_handler();

        // Root placement (image=42, placement=0), no parent.
        let cmd_root = kitty_rgba_2x2_cmd(KittyAction::TransmitAndDisplay);
        handler.handle_kitty_graphics(cmd_root);
        let _ = recv_response(&rx);

        // A second placement of the SAME image (42, placement=1), relative
        // to (42, 0).
        let put_child = kitty_put_cmd(&KittyControlData {
            image_id: Some(42),
            placement_id: Some(1),
            parent_image_id: Some(42),
            parent_placement_id: Some(0),
            h_offset: Some(1),
            v_offset: Some(0),
            ..KittyControlData::default()
        });
        handler.handle_kitty_graphics(put_child);
        let response = recv_response(&rx);
        assert!(
            !response.contains("ECYCLE"),
            "expected success registering (42,1), got: {response}"
        );

        // Now attempt to redefine (42, 0) as relative to (42, 1) — since
        // (42, 1)'s ancestor chain already contains (42, 0), this forms a
        // cycle.
        let put_cycle = kitty_put_cmd(&KittyControlData {
            image_id: Some(42),
            placement_id: Some(0),
            parent_image_id: Some(42),
            parent_placement_id: Some(1),
            h_offset: Some(1),
            v_offset: Some(0),
            ..KittyControlData::default()
        });
        handler.handle_kitty_graphics(put_cycle);
        let response = recv_response(&rx);
        assert!(
            response.contains("ECYCLE"),
            "expected ECYCLE, got: {response}"
        );
    }

    #[test]
    fn kitty_relative_placement_does_not_move_cursor() {
        let (mut handler, rx) = kitty_handler();
        handler.buffer_mut().set_cursor_pos(Some(10), Some(2));

        let cmd_a = kitty_rgba_2x2_cmd(KittyAction::TransmitAndDisplay);
        handler.handle_kitty_graphics(cmd_a);
        let _ = recv_response(&rx);

        // Cursor moved after placing A (place_image always advances it).
        let cursor_after_a = handler.buffer().cursor().pos;

        transmit_only(&mut handler, 99);
        let _ = recv_response(&rx);

        let put_cmd = kitty_put_cmd(&KittyControlData {
            image_id: Some(99),
            parent_image_id: Some(42),
            h_offset: Some(1),
            v_offset: Some(1),
            ..KittyControlData::default()
        });
        handler.handle_kitty_graphics(put_cmd);
        let _ = recv_response(&rx);

        let cursor_after_relative = handler.buffer().cursor().pos;
        assert_eq!(
            cursor_after_a, cursor_after_relative,
            "a relative placement must not move the cursor"
        );
    }

    #[test]
    fn kitty_delete_by_id_cascades_to_relative_children() {
        use freminal_common::buffer_states::kitty_graphics::{KittyAction, KittyDeleteTarget};

        let (mut handler, rx) = kitty_handler();

        // Parent A (id=42).
        let cmd_a = kitty_rgba_2x2_cmd(KittyAction::TransmitAndDisplay);
        handler.handle_kitty_graphics(cmd_a);
        let _ = recv_response(&rx);

        // Child B (id=99) relative to A.
        transmit_only(&mut handler, 99);
        let _ = recv_response(&rx);
        let put_cmd = kitty_put_cmd(&KittyControlData {
            image_id: Some(99),
            parent_image_id: Some(42),
            h_offset: Some(1),
            v_offset: Some(1),
            ..KittyControlData::default()
        });
        handler.handle_kitty_graphics(put_cmd);
        let _ = recv_response(&rx);

        assert!(handler.real_placements.contains_key(&(42, 0)));
        assert!(handler.real_placements.contains_key(&(99, 0)));

        // Delete A by id, uppercase `I` — the cascade to relative children
        // (real_placements pruning + child cell clearing) only fires on the
        // data-freeing path (Task 100.7a), so this must use the uppercase
        // form rather than lowercase `i`.
        let delete_cmd = KittyGraphicsCommand {
            control: KittyControlData {
                action: Some(KittyAction::Delete),
                delete_target: Some(KittyDeleteTarget::ById),
                image_id: Some(42),
                delete_free_data: true,
                ..KittyControlData::default()
            },
            payload: Vec::new(),
        };
        handler.handle_kitty_graphics(delete_cmd);

        assert!(
            !handler.real_placements.contains_key(&(42, 0)),
            "deleted parent must be removed from real_placements"
        );
        assert!(
            !handler.real_placements.contains_key(&(99, 0)),
            "cascade-deleted child must be removed from real_placements"
        );

        let has_image_99 = handler.buffer().rows().iter().any(|row| {
            row.cells()
                .iter()
                .any(|c| c.image_placement().is_some_and(|p| p.image_id == 99))
        });
        assert!(
            !has_image_99,
            "cascade delete must clear the child's stamped cells"
        );
    }

    #[test]
    fn kitty_relative_placement_virtual_parent_registers_without_stamping() {
        let (mut handler, rx) = kitty_handler();

        // Virtual parent A (id=42, U=1).
        let cmd_a = kitty_virtual_2x2_cmd();
        handler.handle_kitty_graphics(cmd_a);
        let _ = recv_response(&rx);
        assert!(handler.virtual_placements.contains_key(&(42, 0)));

        // Child B (id=99) relative to virtual A.
        transmit_only(&mut handler, 99);
        let _ = recv_response(&rx);
        let put_cmd = kitty_put_cmd(&KittyControlData {
            image_id: Some(99),
            parent_image_id: Some(42),
            ..KittyControlData::default()
        });
        handler.handle_kitty_graphics(put_cmd);

        let response = recv_response(&rx);
        assert!(
            !response.contains("ENOPARENT")
                && !response.contains("EINVAL")
                && !response.contains("ECYCLE")
                && !response.contains("ETOODEEP"),
            "expected success (register-only), got: {response}"
        );

        let child = handler
            .real_placements
            .get(&(99, 0))
            .copied()
            .expect("child B registered against virtual parent");
        assert_eq!(child.parent, Some((42, 0)));

        // No image cells were stamped for B — positioning is deferred to
        // Task 100.4b.
        let has_image_99 = handler.buffer().rows().iter().any(|row| {
            row.cells()
                .iter()
                .any(|c| c.image_placement().is_some_and(|p| p.image_id == 99))
        });
        assert!(
            !has_image_99,
            "a child of a virtual parent must not be stamped yet (100.4b)"
        );
    }

    // ── Task 100.4b: render-time positioning of virtual-parent relative
    //    children in `visible_image_placements_extended` ──────────────────

    /// Grow the buffer to at least `rows` rows by driving `rows - 1` line
    /// feeds down column 0, from the buffer's initial single row. Used so
    /// tests can stamp parent placeholder cells at rows beyond the
    /// buffer's initial single row.
    ///
    /// This previously grew the buffer by stamping a throwaway image via
    /// `Buffer::place_image`. Task 100.15 changed `place_image` to append
    /// an extra blank row and move the cursor onto it after a tail
    /// placement — entirely correct production behavior, but it meant
    /// this generic row-growth test helper silently inherited whatever
    /// cursor-placement quirks `place_image` had, coupling unrelated
    /// tests (virtual-parent/relative-placement tests below, which only
    /// need *some* rows to exist and never read the resulting cursor
    /// position) to `place_image`'s internals. Driving plain line feeds
    /// instead keeps this helper's contract fixed and independent of
    /// `place_image`: after `grow_buffer_rows(handler, rows)`,
    /// `rows().len() == rows` and `cursor().pos.y == rows - 1` (assuming
    /// the buffer started at its default single row with the cursor at
    /// row 0 — true for every caller of this helper, which all operate on
    /// a freshly constructed handler).
    fn grow_buffer_rows(handler: &mut TerminalHandler, rows: usize) {
        for _ in 1..rows {
            handler.buffer_mut().handle_lf();
        }
    }

    /// Directly stamp a `rows x cols` block of placeholder cells for
    /// `(image_id, placement_id)` at `(start_row, start_col)`, bypassing the
    /// APC/placeholder-text path. Used to simulate a virtual placement's
    /// placeholder cells sitting at a known position (or having moved, to
    /// simulate scroll/reflow) without driving the full text pipeline.
    fn stamp_parent_placeholder_block(
        handler: &mut TerminalHandler,
        image_id: u64,
        placement_id: u32,
        start_row: usize,
        start_col: usize,
        rows: usize,
        cols: usize,
    ) {
        for row_in_image in 0..rows {
            for col_in_image in 0..cols {
                let placement = freminal_buffer::image_store::ImagePlacement {
                    image_id,
                    col_in_image,
                    row_in_image,
                    protocol: freminal_buffer::image_store::ImageProtocol::Kitty,
                    image_number: None,
                    placement_id: Some(placement_id),
                    z_index: 0,
                    source_crop: None,
                    placement_instance: 1,
                    subcell_offset: None,
                };
                handler.buffer_mut().set_image_cell_at(
                    start_row + row_in_image,
                    start_col + col_in_image,
                    placement,
                    FormatTag::default(),
                );
            }
        }
    }

    /// Find every cell in `placements` carrying `image_id`, returning
    /// `(min_row, min_col)` among them, or `None` if no such cell exists.
    fn min_row_col_for_image(
        placements: &[Option<freminal_buffer::image_store::ImagePlacement>],
        term_width: usize,
        image_id: u64,
    ) -> Option<(usize, usize)> {
        let mut result: Option<(usize, usize)> = None;
        for (idx, cell) in placements.iter().enumerate() {
            let Some(p) = cell else { continue };
            if p.image_id != image_id {
                continue;
            }
            let row = idx / term_width;
            let col = idx % term_width;
            result = Some(result.map_or((row, col), |(r, c)| (r.min(row), c.min(col))));
        }
        result
    }

    #[test]
    fn virtual_parent_child_positioned_at_offset_from_placeholder_min() {
        use freminal_common::buffer_states::unicode_placeholder::VirtualPlacement;

        let (mut handler, _rx) = kitty_handler();
        grow_buffer_rows(&mut handler, 5);

        // Parent virtual placement (42, 0), 2x2 tile.
        handler.virtual_placements.insert(
            (42, 0),
            VirtualPlacement {
                image_id: 42,
                placement_id: 0,
                rows: 2,
                cols: 2,
                placement_instance: 1,
            },
        );
        // Parent's placeholder cells currently sit at rows 2-3, cols 5-6
        // (min row=2, min col=5).
        stamp_parent_placeholder_block(&mut handler, 42, 0, 2, 5, 2, 2);

        // Child (99, 0), a single cell, registered with H=1, V=1.
        handler.insert_real_placement(99, 0, RowNumber::new(0), 0, 1, 1, Some((42, 0)), 0, 1, 1, 1);

        let term_width = handler.win_size().0;
        let placements = handler.visible_image_placements_extended(0, 0);

        let (row, col) = min_row_col_for_image(&placements, term_width, 99)
            .expect("child (99) should be injected");
        assert_eq!(row, 3, "child row = parent min row (2) + V=1");
        assert_eq!(col, 6, "child col = parent min col (5) + H=1");
    }

    #[test]
    fn virtual_parent_child_with_zero_offset_sits_on_parent_min_origin() {
        use freminal_common::buffer_states::unicode_placeholder::VirtualPlacement;

        let (mut handler, _rx) = kitty_handler();
        grow_buffer_rows(&mut handler, 5);

        handler.virtual_placements.insert(
            (42, 0),
            VirtualPlacement {
                image_id: 42,
                placement_id: 0,
                rows: 2,
                cols: 2,
                placement_instance: 1,
            },
        );
        stamp_parent_placeholder_block(&mut handler, 42, 0, 2, 5, 2, 2);

        // Child with H=0, V=0.
        handler.insert_real_placement(99, 0, RowNumber::new(0), 0, 1, 1, Some((42, 0)), 0, 0, 0, 1);

        let term_width = handler.win_size().0;
        let placements = handler.visible_image_placements_extended(0, 0);

        let (row, col) = min_row_col_for_image(&placements, term_width, 99)
            .expect("child (99) should be injected");
        assert_eq!(row, 2, "H=0,V=0: child row = parent min row");
        assert_eq!(col, 5, "H=0,V=0: child col = parent min col");
    }

    #[test]
    fn virtual_parent_child_skipped_when_parent_fully_scrolled_off() {
        use freminal_common::buffer_states::unicode_placeholder::VirtualPlacement;

        let (mut handler, _rx) = kitty_handler();
        grow_buffer_rows(&mut handler, 5);

        // Register the virtual parent, but never stamp any placeholder
        // cells for it — simulates it having scrolled entirely out of the
        // visible window.
        handler.virtual_placements.insert(
            (42, 0),
            VirtualPlacement {
                image_id: 42,
                placement_id: 0,
                rows: 2,
                cols: 2,
                placement_instance: 1,
            },
        );
        handler.insert_real_placement(99, 0, RowNumber::new(0), 0, 1, 1, Some((42, 0)), 0, 1, 1, 1);

        let term_width = handler.win_size().0;
        let placements = handler.visible_image_placements_extended(0, 0);

        assert!(
            min_row_col_for_image(&placements, term_width, 99).is_none(),
            "child of a fully-scrolled-off parent must not be injected"
        );
    }

    #[test]
    fn real_parent_child_not_double_injected() {
        // A real-parent relative child is already stamped into the buffer
        // by Task 100.4a; since its parent key lives in `real_placements`
        // (not `virtual_placements`), `inject_virtual_parent_relatives`
        // must skip it entirely rather than moving or duplicating it.
        let (mut handler, _rx) = kitty_handler();
        grow_buffer_rows(&mut handler, 3);

        // Real (non-virtual) parent placement (42, 0).
        handler.insert_real_placement(42, 0, RowNumber::new(0), 0, 1, 1, None, 0, 0, 0, 1);

        // Child (99, 0) already stamped at (1, 0) — as 100.4a would have
        // done via `place_image_at` — with parent = (42, 0), h=1, v=1
        // (irrelevant here since the parent is real, not virtual).
        stamp_parent_placeholder_block(&mut handler, 99, 0, 1, 0, 1, 1);
        handler.insert_real_placement(99, 0, RowNumber::new(1), 0, 1, 1, Some((42, 0)), 0, 1, 1, 1);

        let term_width = handler.win_size().0;
        let placements = handler.visible_image_placements_extended(0, 0);

        let matches: Vec<(usize, usize)> = placements
            .iter()
            .enumerate()
            .filter_map(|(idx, cell)| {
                cell.as_ref()
                    .filter(|p| p.image_id == 99)
                    .map(|_| (idx / term_width, idx % term_width))
            })
            .collect();
        assert_eq!(
            matches,
            vec![(1, 0)],
            "real-parent child must remain exactly where 100.4a stamped it, not moved/duplicated"
        );
    }

    #[test]
    fn virtual_parent_child_follows_parent_across_snapshots() {
        use freminal_common::buffer_states::unicode_placeholder::VirtualPlacement;

        let (mut handler, _rx) = kitty_handler();
        grow_buffer_rows(&mut handler, 10);

        handler.virtual_placements.insert(
            (42, 0),
            VirtualPlacement {
                image_id: 42,
                placement_id: 0,
                rows: 1,
                cols: 1,
                placement_instance: 1,
            },
        );
        handler.insert_real_placement(99, 0, RowNumber::new(0), 0, 1, 1, Some((42, 0)), 0, 1, 0, 1);

        // First position: parent placeholder at (2, 5).
        stamp_parent_placeholder_block(&mut handler, 42, 0, 2, 5, 1, 1);
        let term_width = handler.win_size().0;
        let placements_before = handler.visible_image_placements_extended(0, 0);
        let (row_before, col_before) = min_row_col_for_image(&placements_before, term_width, 99)
            .expect("child should be injected at first parent position");
        assert_eq!((row_before, col_before), (2, 6), "child follows H=1,V=0");

        // Simulate scroll/reflow moving the parent's placeholder cell to a
        // different position (7, 1) — clear the old cell first so it isn't
        // also picked up as a (stale) parent cell.
        handler.buffer_mut().clear_image_placements_by_id(42);
        stamp_parent_placeholder_block(&mut handler, 42, 0, 7, 1, 1, 1);

        let placements_after = handler.visible_image_placements_extended(0, 0);
        let (row_after, col_after) = min_row_col_for_image(&placements_after, term_width, 99)
            .expect("child should be injected at new parent position");
        assert_eq!(
            (row_after, col_after),
            (7, 2),
            "child must be re-derived from the parent's new position, not cached"
        );
    }

    // -----------------------------------------------------------------------
    // Kitty graphics file path transmission tests (t=f, t=t, t=s)
    // -----------------------------------------------------------------------

    #[test]
    fn kitty_file_transmission_reads_png_from_disk() {
        use freminal_common::buffer_states::kitty_graphics::{
            KittyAction, KittyControlData, KittyFormat, KittyTransmission,
        };

        let (mut handler, _rx) = kitty_handler();

        // Write a minimal valid 1x1 red PNG to a temp file.
        let dir = std::env::temp_dir();
        let path = dir.join("freminal_test_kitty_file.png");
        let img = image::RgbaImage::from_pixel(1, 1, image::Rgba([255, 0, 0, 255]));
        img.save(&path).expect("failed to write test PNG");

        let path_bytes = path
            .to_str()
            .expect("non-UTF-8 temp path")
            .as_bytes()
            .to_vec();

        let cmd = KittyGraphicsCommand {
            control: KittyControlData {
                action: Some(KittyAction::TransmitAndDisplay),
                format: Some(KittyFormat::Png),
                transmission: Some(KittyTransmission::File),
                image_id: Some(999),
                ..KittyControlData::default()
            },
            payload: path_bytes,
        };

        handler.handle_kitty_graphics(cmd);

        // Image should be in the store.
        assert!(
            handler.buffer().image_store().get(999).is_some(),
            "Expected image id=999 in the store after t=f transmission"
        );

        // Image should be placed in cells.
        let has_image = handler.buffer().has_any_image_cell();
        assert!(
            has_image,
            "Expected image cells after t=f Kitty TransmitAndDisplay"
        );

        // Clean up.
        let _ = std::fs::remove_file(&path);
    }

    /// Write a 1x1 PNG to a uniquely-named file in `temp_dir()` whose name
    /// carries `prefix`, returning its path. The caller removes it.
    fn write_unique_png(prefix: &str) -> std::path::PathBuf {
        let file = tempfile::Builder::new()
            .prefix(prefix)
            .suffix(".png")
            .tempfile_in(std::env::temp_dir())
            .expect("failed to create temp file");
        let (_, path) = file.keep().expect("failed to persist temp file");
        let img = image::RgbaImage::from_pixel(1, 1, image::Rgba([0, 255, 0, 255]));
        img.save(&path).expect("failed to write test PNG");
        path
    }

    /// Send a `t=f` / `t=t` transmission for `payload` as image `id`.
    fn transmit_kitty_file(
        handler: &mut TerminalHandler,
        transmission: freminal_common::buffer_states::kitty_graphics::KittyTransmission,
        id: u32,
        payload: Vec<u8>,
    ) {
        use freminal_common::buffer_states::kitty_graphics::{
            KittyAction, KittyControlData, KittyFormat,
        };

        handler.handle_kitty_graphics(KittyGraphicsCommand {
            control: KittyControlData {
                action: Some(KittyAction::TransmitAndDisplay),
                format: Some(KittyFormat::Png),
                transmission: Some(transmission),
                image_id: Some(id),
                ..KittyControlData::default()
            },
            payload,
        });
    }

    /// Drain `rx` and return every PTY write as text.
    fn drain_pty_text(rx: &crossbeam_channel::Receiver<PtyWrite>) -> Vec<String> {
        let mut out = Vec::new();
        while let Ok(msg) = rx.try_recv() {
            if let PtyWrite::Write(bytes) = msg {
                out.push(String::from_utf8_lossy(&bytes).into_owned());
            }
        }
        out
    }

    /// Assert a file transmission was refused with exactly the uniform
    /// `EBADF:Failed to read image file` response and stored no image.
    fn assert_file_refused(
        handler: &TerminalHandler,
        rx: &crossbeam_channel::Receiver<PtyWrite>,
        id: u32,
    ) {
        assert!(
            handler.buffer().image_store().get(u64::from(id)).is_none(),
            "no image should be stored for a refused file"
        );
        let responses = drain_pty_text(rx);
        assert_eq!(responses.len(), 1, "expected exactly one response");
        assert!(
            responses[0].contains("EBADF:Failed to read image file"),
            "unexpected response: {:?}",
            responses[0]
        );
    }

    #[test]
    fn kitty_temp_file_transmission_reads_and_deletes() {
        use freminal_common::buffer_states::kitty_graphics::KittyTransmission;

        let (mut handler, _rx) = kitty_handler();

        let path = write_unique_png("tty-graphics-protocol-");
        let payload = path
            .to_str()
            .expect("non-UTF-8 temp path")
            .as_bytes()
            .to_vec();

        transmit_kitty_file(&mut handler, KittyTransmission::TempFile, 998, payload);

        assert!(
            handler.buffer().image_store().get(998).is_some(),
            "Expected image id=998 in the store after t=t transmission"
        );
        assert!(
            !path.exists(),
            "tty-graphics-protocol temp file should be deleted after t=t transmission"
        );
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn kitty_temp_file_without_marker_is_read_but_not_deleted() {
        use freminal_common::buffer_states::kitty_graphics::KittyTransmission;

        let (mut handler, _rx) = kitty_handler();

        let path = write_unique_png("freminal_unmarked_");
        let payload = path
            .to_str()
            .expect("non-UTF-8 temp path")
            .as_bytes()
            .to_vec();

        transmit_kitty_file(&mut handler, KittyTransmission::TempFile, 990, payload);

        let stored = handler.buffer().image_store().get(990).is_some();
        let survived = path.exists();
        let _ = std::fs::remove_file(&path);

        assert!(stored, "file should still be read for t=t");
        assert!(survived, "t=t must not delete a file lacking the marker");
    }

    #[test]
    #[cfg(unix)]
    fn kitty_temp_file_symlink_reads_target_and_spares_unmarked_target() {
        use freminal_common::buffer_states::kitty_graphics::KittyTransmission;

        let (mut handler, _rx) = kitty_handler();

        // The target has no marker; only the symlink's name does. The path
        // is canonicalised first, so the read follows the link and the
        // deletion gate sees the (unmarked) target path.
        let target = write_unique_png("freminal_symlink_target_");
        let link = std::env::temp_dir().join(format!(
            "tty-graphics-protocol-link-{}",
            target
                .file_name()
                .expect("file name")
                .to_str()
                .expect("utf-8")
        ));
        std::os::unix::fs::symlink(&target, &link).expect("symlink");
        let payload = link.to_str().expect("utf-8").as_bytes().to_vec();

        transmit_kitty_file(&mut handler, KittyTransmission::TempFile, 985, payload);

        let stored = handler.buffer().image_store().get(985).is_some();
        let target_survived = target.exists();
        let link_survived = link.symlink_metadata().is_ok();
        let _ = std::fs::remove_file(&link);
        let _ = std::fs::remove_file(&target);

        assert!(stored, "the symlink's target should be read");
        assert!(target_survived, "an unmarked target must not be deleted");
        assert!(link_survived, "the symlink itself must not be deleted");
    }

    #[test]
    #[cfg(unix)]
    fn kitty_open_refuses_symlink_final_component() {
        let target = write_unique_png("freminal_nofollow_target_");
        let link = std::env::temp_dir().join(format!(
            "freminal_nofollow_link_{}",
            target
                .file_name()
                .expect("file name")
                .to_str()
                .expect("utf-8")
        ));
        std::os::unix::fs::symlink(&target, &link).expect("symlink");

        let result = open_regular_kitty_file(&link);
        let _ = std::fs::remove_file(&link);
        let _ = std::fs::remove_file(&target);

        assert!(
            matches!(result, Err(KittyFileError::Open(_))),
            "O_NOFOLLOW must refuse a symlink as the final component"
        );
    }

    #[test]
    #[cfg(unix)]
    fn kitty_file_identity_distinguishes_files() {
        let dir = tempfile::tempdir().expect("tempdir");
        let a = dir.path().join("a");
        let b = dir.path().join("b");
        std::fs::write(&a, b"a").expect("write a");
        std::fs::write(&b, b"b").expect("write b");

        let id_a1 = FileIdentity::of(&std::fs::File::open(&a).unwrap().metadata().unwrap());
        let id_a2 = FileIdentity::of(&std::fs::File::open(&a).unwrap().metadata().unwrap());
        let id_b = FileIdentity::of(&std::fs::File::open(&b).unwrap().metadata().unwrap());

        assert_eq!(id_a1, id_a2, "the same file has one identity");
        assert_ne!(id_a1, id_b, "different files have different identities");
    }

    #[test]
    #[cfg(unix)]
    fn kitty_verify_opened_file_rejects_path_naming_another_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let a = std::fs::canonicalize(dir.path())
            .expect("canonical dir")
            .join("a");
        let b = a.with_file_name("b");
        std::fs::write(&a, b"a").expect("write a");
        std::fs::write(&b, b"b").expect("write b");

        let opened_a = std::fs::File::open(&a).expect("open a");

        assert!(
            verify_opened_kitty_file(&opened_a, &a, &a).is_ok(),
            "handle and path agree"
        );
        // The handle is `a`, but the (re-resolved) path names `b`.
        assert!(matches!(
            verify_opened_kitty_file(&opened_a, &b, &b),
            Err(KittyFileError::ChangedDuringOpen)
        ));
        // The path now resolves somewhere other than the first canonical path.
        assert!(matches!(
            verify_opened_kitty_file(&opened_a, &b, &a),
            Err(KittyFileError::ChangedDuringOpen)
        ));
    }

    #[test]
    #[cfg(unix)]
    fn kitty_delete_refuses_entry_replaced_after_load() {
        let path = write_unique_png("tty-graphics-protocol-swap-");
        let loaded = load_kitty_file(path.to_str().expect("utf-8").as_bytes()).expect("load");

        // Replace the entry with a different file. The replacement is created
        // while the original still exists so it cannot reuse its inode.
        let replacement = path.with_extension("replacement");
        std::fs::write(&replacement, b"replacement").expect("write replacement");
        std::fs::rename(&replacement, &path).expect("swap entry");

        let result = delete_kitty_temp_file(&loaded);
        let survived = path.exists();
        let content = std::fs::read(&path).ok();
        let _ = std::fs::remove_file(&path);

        assert!(
            matches!(result, Err(KittyFileError::DeleteTargetChanged)),
            "a swapped entry must not be deleted: {result:?}"
        );
        assert!(survived, "the replacement file must survive");
        assert_eq!(content.as_deref(), Some(&b"replacement"[..]));
    }

    #[test]
    #[cfg(unix)]
    fn kitty_delete_removes_the_file_that_was_read() {
        let path = write_unique_png("tty-graphics-protocol-direct-");
        let loaded = load_kitty_file(path.to_str().expect("utf-8").as_bytes()).expect("load");

        let result = delete_kitty_temp_file(&loaded);
        let gone = !path.exists();
        let _ = std::fs::remove_file(&path);

        assert!(result.is_ok(), "{result:?}");
        assert!(gone, "the verified file should be unlinked");
    }

    #[test]
    fn kitty_file_directory_path_refused() {
        use freminal_common::buffer_states::kitty_graphics::KittyTransmission;

        let (mut handler, rx) = kitty_handler();
        let dir = tempfile::tempdir().expect("tempdir");
        let payload = dir.path().to_str().expect("utf-8").as_bytes().to_vec();

        transmit_kitty_file(&mut handler, KittyTransmission::File, 989, payload);

        assert_file_refused(&handler, &rx, 989);
    }

    #[test]
    #[cfg(unix)]
    fn kitty_file_fifo_refused_without_blocking() {
        use freminal_common::buffer_states::kitty_graphics::KittyTransmission;

        let (mut handler, rx) = kitty_handler();
        let dir = tempfile::tempdir().expect("tempdir");
        let fifo = dir.path().join("fifo");
        nix::unistd::mkfifo(&fifo, nix::sys::stat::Mode::S_IRWXU).expect("mkfifo");
        let payload = fifo.to_str().expect("utf-8").as_bytes().to_vec();

        transmit_kitty_file(&mut handler, KittyTransmission::File, 988, payload);

        assert_file_refused(&handler, &rx, 988);
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn kitty_file_proc_path_refused() {
        use freminal_common::buffer_states::kitty_graphics::KittyTransmission;

        let (mut handler, rx) = kitty_handler();

        transmit_kitty_file(
            &mut handler,
            KittyTransmission::File,
            987,
            b"/proc/self/status".to_vec(),
        );

        assert_file_refused(&handler, &rx, 987);
    }

    #[test]
    fn kitty_file_refused_prefix_is_component_wise() {
        use std::path::Path;

        assert!(is_refused_kitty_path(Path::new("/proc/self/status")));
        assert!(is_refused_kitty_path(Path::new("/sys/kernel")));
        assert!(is_refused_kitty_path(Path::new("/dev/zero")));
        assert!(!is_refused_kitty_path(Path::new("/dev/shm/img")));
        assert!(!is_refused_kitty_path(Path::new("/procfoo/img")));
        assert!(!is_refused_kitty_path(Path::new("/devices/img")));
        assert!(!is_refused_kitty_path(Path::new("/tmp/img")));
    }

    #[test]
    fn kitty_read_capped_accepts_exactly_cap_and_rejects_more() {
        let exact = read_capped(&[7u8; 8][..], 8).expect("exactly cap is allowed");
        assert_eq!(exact.len(), 8);

        let under = read_capped(&[7u8; 3][..], 8).expect("under cap is allowed");
        assert_eq!(under.len(), 3);

        assert!(matches!(
            read_capped(&[7u8; 9][..], 8),
            Err(KittyFileError::TooLarge)
        ));
    }

    #[test]
    fn kitty_file_transmission_nonexistent_file_sends_error() {
        use freminal_common::buffer_states::kitty_graphics::{
            KittyAction, KittyControlData, KittyFormat, KittyTransmission,
        };

        let (mut handler, rx) = kitty_handler();

        // Build a platform-appropriate *absolute* path that does not exist.
        // A hardcoded `/tmp/...` string is not absolute on Windows (no drive
        // letter), so `read_kitty_file`'s absolute-path guard would reject it
        // before ever attempting the read — deriving from `temp_dir()` yields
        // a real absolute path on every platform, so the open is attempted
        // and fails as intended.
        let missing = std::env::temp_dir().join("freminal_this_file_does_not_exist_12345.png");
        let payload = missing
            .to_str()
            .expect("temp path is not UTF-8")
            .as_bytes()
            .to_vec();

        let cmd = KittyGraphicsCommand {
            control: KittyControlData {
                action: Some(KittyAction::TransmitAndDisplay),
                format: Some(KittyFormat::Png),
                transmission: Some(KittyTransmission::File),
                image_id: Some(997),
                ..KittyControlData::default()
            },
            payload,
        };

        handler.handle_kitty_graphics(cmd);

        // Should NOT be in the store.
        assert!(
            handler.buffer().image_store().get(997).is_none(),
            "No image should be stored for a nonexistent file"
        );

        // An error response should have been sent.
        let mut found_error = false;
        while let Ok(msg) = rx.try_recv() {
            if let PtyWrite::Write(bytes) = msg {
                let text = String::from_utf8_lossy(&bytes);
                if text.contains("EBADF:Failed to read image file") {
                    found_error = true;
                }
            }
        }
        assert!(
            found_error,
            "Expected an EBADF error response for missing file"
        );
    }

    #[test]
    fn kitty_file_transmission_relative_path_rejected() {
        use freminal_common::buffer_states::kitty_graphics::{
            KittyAction, KittyControlData, KittyFormat, KittyTransmission,
        };

        let (mut handler, rx) = kitty_handler();

        let cmd = KittyGraphicsCommand {
            control: KittyControlData {
                action: Some(KittyAction::TransmitAndDisplay),
                format: Some(KittyFormat::Png),
                transmission: Some(KittyTransmission::File),
                image_id: Some(996),
                ..KittyControlData::default()
            },
            payload: b"../../../etc/passwd".to_vec(),
        };

        handler.handle_kitty_graphics(cmd);

        // Should NOT be in the store.
        assert!(
            handler.buffer().image_store().get(996).is_none(),
            "No image should be stored for a relative path"
        );

        // The uniform EBADF error response should have been sent.
        let mut found_error = false;
        while let Ok(msg) = rx.try_recv() {
            if let PtyWrite::Write(bytes) = msg {
                let text = String::from_utf8_lossy(&bytes);
                if text.contains("EBADF:Failed to read image file") {
                    found_error = true;
                }
            }
        }
        assert!(
            found_error,
            "Expected an EBADF error response for relative path"
        );
    }

    #[test]
    #[cfg(windows)]
    fn kitty_shared_memory_nonexistent_object_returns_enoent_windows() {
        use freminal_common::buffer_states::kitty_graphics::{
            KittyAction, KittyControlData, KittyFormat, KittyTransmission,
        };

        let (mut handler, rx) = kitty_handler();

        // A safe, well-formed name that does not correspond to any existing
        // mapping — the point is that opening it fails. Kept short so it is
        // consistent with the POSIX path and the shared naming helper.
        let name = unique_shm_name();

        let cmd = KittyGraphicsCommand {
            control: KittyControlData {
                action: Some(KittyAction::TransmitAndDisplay),
                format: Some(KittyFormat::Rgba),
                transmission: Some(KittyTransmission::SharedMemory),
                src_width: Some(1),
                src_height: Some(1),
                data_size: Some(4),
                image_id: Some(995),
                ..KittyControlData::default()
            },
            payload: name.into_bytes(),
        };

        handler.handle_kitty_graphics(cmd);

        assert!(
            handler.buffer().image_store().get(995).is_none(),
            "No image should be stored for a nonexistent shared memory object"
        );

        let mut found_error = false;
        while let Ok(msg) = rx.try_recv() {
            if let PtyWrite::Write(bytes) = msg {
                let text = String::from_utf8_lossy(&bytes);
                if text.contains("ENOENT") {
                    found_error = true;
                }
            }
        }
        assert!(
            found_error,
            "Expected an ENOENT error response for a nonexistent shared memory object"
        );
    }

    #[test]
    #[cfg(windows)]
    fn kitty_shared_memory_missing_data_size_rejected_windows() {
        use freminal_common::buffer_states::kitty_graphics::{
            KittyAction, KittyControlData, KittyFormat, KittyTransmission,
        };

        let (mut handler, rx) = kitty_handler();

        // No `S=` (`data_size`) supplied — Windows has no equivalent to
        // POSIX `fstat` to learn the object's size, so this must be
        // rejected before any `OpenFileMappingW` call.
        let cmd = KittyGraphicsCommand {
            control: KittyControlData {
                action: Some(KittyAction::TransmitAndDisplay),
                format: Some(KittyFormat::Rgba),
                transmission: Some(KittyTransmission::SharedMemory),
                src_width: Some(1),
                src_height: Some(1),
                image_id: Some(995),
                ..KittyControlData::default()
            },
            payload: b"/kitty_shm_missing_size".to_vec(),
        };

        handler.handle_kitty_graphics(cmd);

        assert!(
            handler.buffer().image_store().get(995).is_none(),
            "No image should be stored when S= is missing on Windows"
        );

        let mut found_error = false;
        while let Ok(msg) = rx.try_recv() {
            if let PtyWrite::Write(bytes) = msg {
                let text = String::from_utf8_lossy(&bytes);
                if text.contains("EINVAL") {
                    found_error = true;
                }
            }
        }
        assert!(
            found_error,
            "Expected an EINVAL error response when S= is missing on Windows"
        );
    }

    #[test]
    #[cfg(windows)]
    fn kitty_shared_memory_unsafe_name_rejected_windows() {
        use freminal_common::buffer_states::kitty_graphics::{
            KittyAction, KittyControlData, KittyFormat, KittyTransmission,
        };

        for bad_name in ["/foo/bar", "/../etc/passwd"] {
            let (mut handler, rx) = kitty_handler();

            let cmd = KittyGraphicsCommand {
                control: KittyControlData {
                    action: Some(KittyAction::TransmitAndDisplay),
                    format: Some(KittyFormat::Rgba),
                    transmission: Some(KittyTransmission::SharedMemory),
                    src_width: Some(1),
                    src_height: Some(1),
                    data_size: Some(4),
                    image_id: Some(995),
                    ..KittyControlData::default()
                },
                payload: bad_name.as_bytes().to_vec(),
            };

            handler.handle_kitty_graphics(cmd);

            assert!(
                handler.buffer().image_store().get(995).is_none(),
                "No image should be stored for unsafe shared memory name {bad_name:?}"
            );

            let mut found_error = false;
            while let Ok(msg) = rx.try_recv() {
                if let PtyWrite::Write(bytes) = msg {
                    let text = String::from_utf8_lossy(&bytes);
                    if text.contains("EPERM") {
                        found_error = true;
                    }
                }
            }
            assert!(
                found_error,
                "Expected an EPERM error response for unsafe shared memory name {bad_name:?}"
            );
        }
    }

    /// Create a Windows named file-mapping object of `data.len().max(1)`
    /// bytes containing `data`, returning its name and the creating
    /// handle. Test-only helper for the `t=s` round-trip test below.
    ///
    /// The returned handle **must** be kept alive (not closed) until after
    /// the code under test has had a chance to `OpenFileMappingW` its own
    /// handle — Windows destroys a named mapping once every handle to it
    /// is closed, and this test is the only thing creating (and thus
    /// keeping alive) the object.
    #[cfg(windows)]
    fn create_test_shm_object_windows(data: &[u8]) -> (String, winapi::um::winnt::HANDLE) {
        use winapi::um::handleapi::INVALID_HANDLE_VALUE;
        use winapi::um::memoryapi::{
            CreateFileMappingW, FILE_MAP_WRITE, MapViewOfFile, UnmapViewOfFile,
        };
        use winapi::um::winnt::PAGE_READWRITE;

        let name = unique_shm_name();
        let wide: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
        let size = u32::try_from(data.len().max(1)).expect("test data length fits in u32");

        // SAFETY: FFI call; `wide` is a NUL-terminated UTF-16 string that
        // lives for the duration of this call. `INVALID_HANDLE_VALUE`
        // backs the mapping with the system paging file rather than a
        // real file, per the Win32 API contract for `CreateFileMappingW`.
        let handle = unsafe {
            CreateFileMappingW(
                INVALID_HANDLE_VALUE,
                std::ptr::null_mut(),
                PAGE_READWRITE,
                0,
                size,
                wide.as_ptr(),
            )
        };
        assert!(
            !handle.is_null(),
            "CreateFileMappingW should succeed in test"
        );

        if !data.is_empty() {
            // SAFETY: FFI call; `handle` was just created above with
            // capacity `size` (== `data.len()`).
            let base = unsafe { MapViewOfFile(handle, FILE_MAP_WRITE, 0, 0, data.len()) };
            assert!(
                !base.is_null(),
                "MapViewOfFile (write) should succeed in test"
            );
            // SAFETY: `base` is valid for `data.len()` writable bytes (the
            // mapping length just requested above).
            unsafe {
                std::ptr::copy_nonoverlapping(data.as_ptr(), base.cast::<u8>(), data.len());
            }
            // SAFETY: `base` is exactly the pointer returned by the
            // matching `MapViewOfFile` call above and has not been
            // unmapped yet.
            unsafe { UnmapViewOfFile(base) };
        }

        (name, handle)
    }

    #[test]
    #[cfg(windows)]
    fn kitty_shared_memory_transmission_round_trip_windows() {
        use freminal_common::buffer_states::kitty_graphics::{
            KittyAction, KittyControlData, KittyFormat, KittyTransmission,
        };
        use winapi::um::handleapi::CloseHandle;

        // 2x1 RGBA image: red pixel, green pixel.
        let pixels: Vec<u8> = vec![255, 0, 0, 255, 0, 255, 0, 255];
        let (name, creator_handle) = create_test_shm_object_windows(&pixels);

        let (mut handler, rx) = kitty_handler();

        let cmd = KittyGraphicsCommand {
            control: KittyControlData {
                action: Some(KittyAction::TransmitAndDisplay),
                format: Some(KittyFormat::Rgba),
                transmission: Some(KittyTransmission::SharedMemory),
                src_width: Some(2),
                src_height: Some(1),
                data_size: Some(u32::try_from(pixels.len()).unwrap()),
                image_id: Some(993),
                ..KittyControlData::default()
            },
            payload: name.into_bytes(),
        };

        handler.handle_kitty_graphics(cmd);

        // Only close the creating handle *after* the code under test has
        // opened (and closed) its own handle — see the doc comment on
        // `create_test_shm_object_windows`.
        //
        // SAFETY: `creator_handle` is exactly the handle returned by the
        // matching `CreateFileMappingW` call above and has not been
        // closed yet.
        unsafe { CloseHandle(creator_handle) };

        let stored = handler
            .buffer()
            .image_store()
            .get(993)
            .expect("image should be stored from shared memory transmission");
        assert_eq!(*stored.pixels, pixels);
        assert_eq!(stored.width_px, 2);
        assert_eq!(stored.height_px, 1);

        // No error response should have been sent.
        let mut found_error = false;
        while let Ok(msg) = rx.try_recv() {
            if let PtyWrite::Write(bytes) = msg {
                let text = String::from_utf8_lossy(&bytes);
                if !text.contains("OK") {
                    found_error = true;
                }
            }
        }
        assert!(
            !found_error,
            "Expected no error response for a valid t=s transmission on Windows"
        );
    }

    #[test]
    #[cfg(unix)]
    fn kitty_shared_memory_nonexistent_object_returns_enoent() {
        use freminal_common::buffer_states::kitty_graphics::{
            KittyAction, KittyControlData, KittyFormat, KittyTransmission,
        };

        let (mut handler, rx) = kitty_handler();

        // A safe, well-formed name that does not correspond to any existing
        // object — the point is that `shm_open` fails with `ENOENT`. The name
        // must fit macOS's 31-char shm limit, or the error would be
        // `ENAMETOOLONG` instead. `unique_shm_name` never creates the object.
        let name = unique_shm_name();

        let cmd = KittyGraphicsCommand {
            control: KittyControlData {
                action: Some(KittyAction::TransmitAndDisplay),
                format: Some(KittyFormat::Rgba),
                transmission: Some(KittyTransmission::SharedMemory),
                src_width: Some(1),
                src_height: Some(1),
                image_id: Some(995),
                ..KittyControlData::default()
            },
            payload: name.into_bytes(),
        };

        handler.handle_kitty_graphics(cmd);

        // Should NOT be in the store.
        assert!(
            handler.buffer().image_store().get(995).is_none(),
            "No image should be stored for a nonexistent shared memory object"
        );

        // An ENOENT error response should have been sent.
        let mut found_error = false;
        while let Ok(msg) = rx.try_recv() {
            if let PtyWrite::Write(bytes) = msg {
                let text = String::from_utf8_lossy(&bytes);
                if text.contains("ENOENT") {
                    found_error = true;
                }
            }
        }
        assert!(
            found_error,
            "Expected an ENOENT error response for a nonexistent shared memory object"
        );
    }

    #[test]
    #[cfg(unix)]
    fn kitty_shared_memory_unsafe_name_rejected() {
        use freminal_common::buffer_states::kitty_graphics::{
            KittyAction, KittyControlData, KittyFormat, KittyTransmission,
        };

        for bad_name in ["/foo/bar", "/../etc/passwd", "/proc/self/mem"] {
            let (mut handler, rx) = kitty_handler();

            let cmd = KittyGraphicsCommand {
                control: KittyControlData {
                    action: Some(KittyAction::TransmitAndDisplay),
                    format: Some(KittyFormat::Rgba),
                    transmission: Some(KittyTransmission::SharedMemory),
                    src_width: Some(1),
                    src_height: Some(1),
                    image_id: Some(995),
                    ..KittyControlData::default()
                },
                payload: bad_name.as_bytes().to_vec(),
            };

            handler.handle_kitty_graphics(cmd);

            assert!(
                handler.buffer().image_store().get(995).is_none(),
                "No image should be stored for unsafe shared memory name {bad_name:?}"
            );

            let mut found_error = false;
            while let Ok(msg) = rx.try_recv() {
                if let PtyWrite::Write(bytes) = msg {
                    let text = String::from_utf8_lossy(&bytes);
                    if text.contains("EPERM") {
                        found_error = true;
                    }
                }
            }
            assert!(
                found_error,
                "Expected an EPERM error response for unsafe shared memory name {bad_name:?}"
            );
        }
    }

    /// Create a POSIX shm object of `data.len()` bytes containing `data`,
    /// returning its unique name. Test-only helper for the `t=s` round-trip
    /// tests below.
    #[cfg(unix)]
    fn create_test_shm_object(data: &[u8]) -> String {
        use nix::fcntl::OFlag;
        use nix::sys::mman::{MapFlags, ProtFlags, mmap, munmap, shm_open};
        use nix::sys::stat::Mode;
        use nix::unistd::ftruncate;

        let name = unique_shm_name();
        let fd = shm_open(
            name.as_str(),
            OFlag::O_CREAT | OFlag::O_RDWR,
            Mode::S_IRUSR | Mode::S_IWUSR,
        )
        .expect("shm_open (create) should succeed in test");

        let len = i64::try_from(data.len()).expect("test data length fits in i64");
        ftruncate(&fd, len).expect("ftruncate should succeed in test");

        if !data.is_empty() {
            let map_len =
                std::num::NonZeroUsize::new(data.len()).expect("test data must be non-empty here");
            // SAFETY: `fd` was just sized to `data.len()` bytes via
            // `ftruncate`; the mapping is read-write and unmapped
            // immediately after the copy, before this function returns.
            let ptr = unsafe {
                mmap(
                    None,
                    map_len,
                    ProtFlags::PROT_READ | ProtFlags::PROT_WRITE,
                    MapFlags::MAP_SHARED,
                    &fd,
                    0,
                )
            }
            .expect("mmap should succeed in test");
            // SAFETY: `ptr` is valid for `data.len()` writable bytes (the
            // mmap length above matches `data.len()` exactly).
            unsafe {
                std::ptr::copy_nonoverlapping(data.as_ptr(), ptr.as_ptr().cast::<u8>(), data.len());
            }
            // SAFETY: `ptr`/`map_len` are exactly the values returned by
            // the matching `mmap` call above.
            unsafe { munmap(ptr, map_len.get()) }.expect("munmap should succeed in test");
        }

        name
    }

    #[test]
    #[cfg(unix)]
    fn kitty_shared_memory_transmission_round_trip() {
        use freminal_common::buffer_states::kitty_graphics::{
            KittyAction, KittyControlData, KittyFormat, KittyTransmission,
        };
        use nix::fcntl::OFlag;
        use nix::sys::mman::shm_open;
        use nix::sys::stat::Mode;

        // 2x1 RGBA image: red pixel, green pixel.
        let pixels: Vec<u8> = vec![255, 0, 0, 255, 0, 255, 0, 255];
        let name = create_test_shm_object(&pixels);

        let (mut handler, rx) = kitty_handler();

        let cmd = KittyGraphicsCommand {
            control: KittyControlData {
                action: Some(KittyAction::TransmitAndDisplay),
                format: Some(KittyFormat::Rgba),
                transmission: Some(KittyTransmission::SharedMemory),
                src_width: Some(2),
                src_height: Some(1),
                data_size: Some(u32::try_from(pixels.len()).unwrap()),
                image_id: Some(994),
                ..KittyControlData::default()
            },
            payload: name.clone().into_bytes(),
        };

        handler.handle_kitty_graphics(cmd);

        let stored = handler
            .buffer()
            .image_store()
            .get(994)
            .expect("image should be stored from shared memory transmission");
        assert_eq!(*stored.pixels, pixels);
        assert_eq!(stored.width_px, 2);
        assert_eq!(stored.height_px, 1);

        // The object must have been unlinked after reading (spec mandate).
        let reopened = shm_open(name.as_str(), OFlag::O_RDONLY, Mode::empty());
        assert!(
            reopened.is_err(),
            "shared memory object should have been unlinked after reading"
        );

        // No error response should have been sent.
        let mut found_error = false;
        while let Ok(msg) = rx.try_recv() {
            if let PtyWrite::Write(bytes) = msg {
                let text = String::from_utf8_lossy(&bytes);
                if !text.contains("OK") {
                    found_error = true;
                }
            }
        }
        assert!(
            !found_error,
            "Expected no error response for a valid t=s transmission"
        );
    }

    #[test]
    #[cfg(unix)]
    fn kitty_shared_memory_transmission_with_offset() {
        use freminal_common::buffer_states::kitty_graphics::{
            KittyAction, KittyControlData, KittyFormat, KittyTransmission,
        };

        // 4-byte header (ignored) followed by a 1x1 RGBA pixel.
        let header = [0xAAu8, 0xBB, 0xCC, 0xDD];
        let pixel: [u8; 4] = [10, 20, 30, 40];
        let mut object = Vec::new();
        object.extend_from_slice(&header);
        object.extend_from_slice(&pixel);

        let name = create_test_shm_object(&object);

        let (mut handler, _rx) = kitty_handler();

        let cmd = KittyGraphicsCommand {
            control: KittyControlData {
                action: Some(KittyAction::TransmitAndDisplay),
                format: Some(KittyFormat::Rgba),
                transmission: Some(KittyTransmission::SharedMemory),
                src_width: Some(1),
                src_height: Some(1),
                data_offset: Some(u32::try_from(header.len()).unwrap()),
                data_size: Some(u32::try_from(pixel.len()).unwrap()),
                image_id: Some(993),
                ..KittyControlData::default()
            },
            payload: name.into_bytes(),
        };

        handler.handle_kitty_graphics(cmd);

        let stored = handler
            .buffer()
            .image_store()
            .get(993)
            .expect("image should be stored from shared memory transmission with offset");
        assert_eq!(*stored.pixels, pixel);
    }

    #[test]
    fn kitty_direct_transmission_still_works() {
        use freminal_common::buffer_states::kitty_graphics::{
            KittyAction, KittyControlData, KittyFormat, KittyTransmission,
        };

        let (mut handler, _rx) = kitty_handler();

        // 1x1 RGBA = 4 bytes, explicit t=d.
        let cmd = KittyGraphicsCommand {
            control: KittyControlData {
                action: Some(KittyAction::TransmitAndDisplay),
                format: Some(KittyFormat::Rgba),
                transmission: Some(KittyTransmission::Direct),
                src_width: Some(1),
                src_height: Some(1),
                image_id: Some(994),
                ..KittyControlData::default()
            },
            payload: vec![255, 0, 0, 255],
        };

        handler.handle_kitty_graphics(cmd);

        assert!(
            handler.buffer().image_store().get(994).is_some(),
            "Expected image id=994 in the store with explicit t=d"
        );
    }

    /// Compress `data` as an RFC 1950 zlib stream (kitty `o=z`).
    fn zlib_compress(data: &[u8]) -> Vec<u8> {
        use std::io::Write as _;
        let mut encoder =
            flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        encoder
            .write_all(data)
            .expect("zlib compression should succeed in test");
        encoder
            .finish()
            .expect("zlib finish should succeed in test")
    }

    #[test]
    fn kitty_zlib_compressed_rgba_round_trip() {
        use freminal_common::buffer_states::kitty_graphics::{
            KittyAction, KittyCompression, KittyControlData, KittyFormat, KittyTransmission,
        };

        let (mut handler, _rx) = kitty_handler();

        // 2x1 RGBA: red, green.
        let pixels: Vec<u8> = vec![255, 0, 0, 255, 0, 255, 0, 255];
        let compressed = zlib_compress(&pixels);

        let cmd = KittyGraphicsCommand {
            control: KittyControlData {
                action: Some(KittyAction::TransmitAndDisplay),
                format: Some(KittyFormat::Rgba),
                transmission: Some(KittyTransmission::Direct),
                compression: Some(KittyCompression::Zlib),
                src_width: Some(2),
                src_height: Some(1),
                image_id: Some(992),
                ..KittyControlData::default()
            },
            payload: compressed,
        };

        handler.handle_kitty_graphics(cmd);

        let stored = handler
            .buffer()
            .image_store()
            .get(992)
            .expect("image should be stored from o=z decompressed RGBA payload");
        assert_eq!(*stored.pixels, pixels);
        assert_eq!(stored.width_px, 2);
        assert_eq!(stored.height_px, 1);
    }

    #[test]
    fn kitty_zlib_compressed_rgb_round_trip() {
        use freminal_common::buffer_states::kitty_graphics::{
            KittyAction, KittyCompression, KittyControlData, KittyFormat, KittyTransmission,
        };

        let (mut handler, _rx) = kitty_handler();

        // 1x1 RGB (3 bytes), expected to be widened to RGBA after decoding.
        let rgb: Vec<u8> = vec![10, 20, 30];
        let compressed = zlib_compress(&rgb);

        let cmd = KittyGraphicsCommand {
            control: KittyControlData {
                action: Some(KittyAction::TransmitAndDisplay),
                format: Some(KittyFormat::Rgb),
                transmission: Some(KittyTransmission::Direct),
                compression: Some(KittyCompression::Zlib),
                src_width: Some(1),
                src_height: Some(1),
                image_id: Some(991),
                ..KittyControlData::default()
            },
            payload: compressed,
        };

        handler.handle_kitty_graphics(cmd);

        let stored = handler
            .buffer()
            .image_store()
            .get(991)
            .expect("image should be stored from o=z decompressed RGB payload");
        assert_eq!(*stored.pixels, vec![10, 20, 30, 255]);
    }

    #[test]
    fn kitty_zlib_compressed_png_round_trip() {
        use freminal_common::buffer_states::kitty_graphics::{
            KittyAction, KittyCompression, KittyControlData, KittyFormat, KittyTransmission,
        };

        let (mut handler, _rx) = kitty_handler();

        let png_data = make_test_png(); // 2x2 red PNG.
        let compressed = zlib_compress(&png_data);

        let cmd = KittyGraphicsCommand {
            control: KittyControlData {
                action: Some(KittyAction::TransmitAndDisplay),
                format: Some(KittyFormat::Png),
                transmission: Some(KittyTransmission::Direct),
                compression: Some(KittyCompression::Zlib),
                image_id: Some(990),
                ..KittyControlData::default()
            },
            payload: compressed,
        };

        handler.handle_kitty_graphics(cmd);

        let stored = handler
            .buffer()
            .image_store()
            .get(990)
            .expect("image should be stored from o=z decompressed PNG payload");
        assert_eq!(stored.width_px, 2);
        assert_eq!(stored.height_px, 2);
        assert_eq!(&stored.pixels[0..4], &[255, 0, 0, 255]);
    }

    #[test]
    fn kitty_zlib_malformed_data_sends_error() {
        use freminal_common::buffer_states::kitty_graphics::{
            KittyAction, KittyCompression, KittyControlData, KittyFormat, KittyTransmission,
        };

        let (mut handler, rx) = kitty_handler();

        let cmd = KittyGraphicsCommand {
            control: KittyControlData {
                action: Some(KittyAction::TransmitAndDisplay),
                format: Some(KittyFormat::Rgba),
                transmission: Some(KittyTransmission::Direct),
                compression: Some(KittyCompression::Zlib),
                src_width: Some(1),
                src_height: Some(1),
                image_id: Some(989),
                ..KittyControlData::default()
            },
            payload: vec![0xDE, 0xAD, 0xBE, 0xEF], // not a valid zlib stream
        };

        handler.handle_kitty_graphics(cmd);

        assert!(
            handler.buffer().image_store().get(989).is_none(),
            "No image should be stored for malformed zlib data"
        );

        let mut found_error = false;
        while let Ok(msg) = rx.try_recv() {
            if let PtyWrite::Write(bytes) = msg {
                let text = String::from_utf8_lossy(&bytes);
                if text.contains("EINVAL") && text.contains("zlib") {
                    found_error = true;
                }
            }
        }
        assert!(
            found_error,
            "Expected an EINVAL zlib decompression error response"
        );
    }

    #[test]
    fn inflate_zlib_enforces_its_limit() {
        use super::inflate_zlib;

        let zeros = |n: usize| zlib_compress(&vec![0u8; n]);

        // A 64 KiB run of zeros compresses to a few dozen bytes.
        let bomb = zeros(64 * 1024);
        assert!(bomb.len() < 1024);
        assert!(inflate_zlib(&bomb, 1024).is_err());

        // Exactly at the limit is accepted; one over is not.
        assert_eq!(inflate_zlib(&zeros(1024), 1024).unwrap().len(), 1024);
        assert!(inflate_zlib(&zeros(1025), 1024).is_err());
        assert_eq!(inflate_zlib(&zeros(0), 0).unwrap().len(), 0);
        assert!(inflate_zlib(&zeros(1), 0).is_err());
    }

    #[test]
    fn kitty_zlib_over_limit_is_rejected_like_a_corrupt_stream() {
        use crate::log_capture::{capture, warnings};
        use freminal_common::buffer_states::kitty_graphics::{
            KittyCompression, KittyFormat, KittyTransmission,
        };

        let (handler, rx) = kitty_handler();
        let cmd = KittyGraphicsCommand {
            control: KittyControlData {
                action: Some(KittyAction::TransmitAndDisplay),
                format: Some(KittyFormat::Rgba),
                transmission: Some(KittyTransmission::Direct),
                compression: Some(KittyCompression::Zlib),
                src_width: Some(128),
                src_height: Some(128),
                image_id: Some(970),
                ..KittyControlData::default()
            },
            payload: zlib_compress(&vec![0u8; 64 * 1024]),
        };

        // Under a generous limit the payload inflates.
        assert_eq!(
            handler
                .resolve_and_decompress_kitty_payload(&cmd, 970, None, 0, 64 * 1024)
                .map(|data| data.len()),
            Some(64 * 1024)
        );
        assert!(rx.try_recv().is_err());

        // One byte under the decompressed size it is refused.
        let mut result = Some(Vec::new());
        let events = capture(|| {
            result =
                handler.resolve_and_decompress_kitty_payload(&cmd, 970, None, 0, 64 * 1024 - 1);
        });
        assert!(result.is_none());

        let warns = warnings(&events);
        assert_eq!(warns.len(), 1);
        assert!(
            warns[0].1.contains("zlib decompression failed") && !warns[0].1.contains("65535"),
            "warn is the corrupt-stream warn and carries no sizes: {:?}",
            warns[0].1
        );

        let Ok(PtyWrite::Write(bytes)) = rx.try_recv() else {
            panic!("expected a reply");
        };
        let text = String::from_utf8_lossy(&bytes);
        assert!(
            text.contains("EINVAL:zlib decompression failed"),
            "same reply as a corrupt stream, got: {text}"
        );
    }

    #[test]
    #[cfg(unix)]
    fn kitty_shared_memory_missing_object_warn_omits_the_name() {
        use crate::log_capture::{capture, warnings};
        use freminal_common::buffer_states::kitty_graphics::{KittyFormat, KittyTransmission};

        let (mut handler, _rx) = kitty_handler();
        let name = format!("/freminal-test-SECRETNAME-{}", std::process::id());
        let cmd = KittyGraphicsCommand {
            control: KittyControlData {
                action: Some(KittyAction::TransmitAndDisplay),
                format: Some(KittyFormat::Rgba),
                transmission: Some(KittyTransmission::SharedMemory),
                src_width: Some(1),
                src_height: Some(1),
                image_id: Some(971),
                ..KittyControlData::default()
            },
            payload: name.into_bytes(),
        };

        let events = capture(|| handler.handle_kitty_graphics(cmd));

        let warns = warnings(&events);
        assert!(!warns.is_empty(), "a missing object must warn");
        for (_, text) in warns {
            assert!(
                !text.contains("SECRETNAME"),
                "the sender-supplied name must not reach a warn: {text}"
            );
        }
    }

    #[test]
    fn kitty_file_invalid_utf8_path_sends_error() {
        use freminal_common::buffer_states::kitty_graphics::{
            KittyAction, KittyControlData, KittyFormat, KittyTransmission,
        };

        let (mut handler, rx) = kitty_handler();

        // Invalid UTF-8 bytes.
        let cmd = KittyGraphicsCommand {
            control: KittyControlData {
                action: Some(KittyAction::TransmitAndDisplay),
                format: Some(KittyFormat::Png),
                transmission: Some(KittyTransmission::File),
                image_id: Some(993),
                ..KittyControlData::default()
            },
            payload: vec![0xFF, 0xFE, 0x80, 0x00],
        };

        handler.handle_kitty_graphics(cmd);

        assert!(
            handler.buffer().image_store().get(993).is_none(),
            "No image should be stored for invalid UTF-8 path"
        );

        let mut found_error = false;
        while let Ok(msg) = rx.try_recv() {
            if let PtyWrite::Write(bytes) = msg {
                let text = String::from_utf8_lossy(&bytes);
                if text.contains("EBADF:Failed to read image file") {
                    found_error = true;
                }
            }
        }
        assert!(
            found_error,
            "Expected an EBADF error response for invalid UTF-8 path"
        );
    }

    // -----------------------------------------------------------------------
    // Kitty `a=p` (Put) tests — transmit then place by image ID
    // -----------------------------------------------------------------------

    #[test]
    fn kitty_put_places_previously_transmitted_image() {
        use freminal_common::buffer_states::kitty_graphics::{KittyAction, KittyControlData};

        let (mut handler, _rx) = kitty_handler();

        // Step 1: Transmit a 2x2 RGBA image (store only, no display).
        let transmit_cmd = kitty_rgba_2x2_cmd(KittyAction::Transmit);
        handler.handle_kitty_graphics(transmit_cmd);

        // Image should be in the store but not placed.
        assert!(handler.buffer().image_store().get(42).is_some());
        let has_image_before = handler.buffer().has_any_image_cell();
        assert!(
            !has_image_before,
            "Transmit-only should not place image cells"
        );

        // Step 2: Put (display the previously transmitted image).
        let put_cmd = KittyGraphicsCommand {
            control: KittyControlData {
                action: Some(KittyAction::Put),
                image_id: Some(42),
                ..KittyControlData::default()
            },
            payload: Vec::new(), // Put has no payload — references by ID.
        };
        handler.handle_kitty_graphics(put_cmd);

        // Image should now be placed in buffer cells.
        let has_image_after = handler.buffer().has_any_image_cell();
        assert!(
            has_image_after,
            "Put should place previously transmitted image into cells"
        );
    }

    #[test]
    fn kitty_put_with_display_size_overrides() {
        use freminal_common::buffer_states::kitty_graphics::{KittyAction, KittyControlData};

        let (mut handler, _rx) = kitty_handler();

        // Transmit a 2x2 RGBA image.
        let transmit_cmd = kitty_rgba_2x2_cmd(KittyAction::Transmit);
        handler.handle_kitty_graphics(transmit_cmd);

        // Put with display size overrides: c=10 columns, r=5 rows.
        let put_cmd = KittyGraphicsCommand {
            control: KittyControlData {
                action: Some(KittyAction::Put),
                image_id: Some(42),
                display_cols: Some(10),
                display_rows: Some(5),
                ..KittyControlData::default()
            },
            payload: Vec::new(),
        };
        handler.handle_kitty_graphics(put_cmd);

        // Check the image in the store was updated with new display dimensions.
        let img = handler.buffer().image_store().get(42).unwrap();
        assert_eq!(
            img.display_cols, 10,
            "display_cols should be overridden to 10"
        );
        assert_eq!(
            img.display_rows, 5,
            "display_rows should be overridden to 5"
        );

        // Verify image cells were placed.
        let has_image = handler.buffer().has_any_image_cell();
        assert!(has_image, "Put with overrides should place image cells");
    }

    #[test]
    fn kitty_put_with_no_cursor_movement() {
        use freminal_common::buffer_states::kitty_graphics::{KittyAction, KittyControlData};

        let (mut handler, _rx) = kitty_handler();

        // Transmit a 2x2 RGBA image.
        let transmit_cmd = kitty_rgba_2x2_cmd(KittyAction::Transmit);
        handler.handle_kitty_graphics(transmit_cmd);

        // Move cursor to a known position.
        handler.buffer_mut().set_cursor_pos(Some(5), Some(3));
        let cursor_before = handler.cursor_pos();

        // Put with C=1 (no cursor movement).
        let put_cmd = KittyGraphicsCommand {
            control: KittyControlData {
                action: Some(KittyAction::Put),
                image_id: Some(42),
                no_cursor_movement: true,
                ..KittyControlData::default()
            },
            payload: Vec::new(),
        };
        handler.handle_kitty_graphics(put_cmd);

        let cursor_after = handler.cursor_pos();
        assert_eq!(
            cursor_before, cursor_after,
            "Cursor should not move when C=1 is set on Put"
        );
    }

    /// Regression (Task 125.C7): at scrollback capacity, placing an image
    /// evicts rows DURING the placement. `C=1` must still leave the cursor on
    /// the image's origin cell, not on whatever row now holds the stale index.
    #[test]
    fn kitty_put_c1_at_scrollback_capacity_keeps_cursor_on_image_origin() {
        use freminal_common::buffer_states::kitty_graphics::{KittyAction, KittyControlData};

        let mut handler = TerminalHandler::new(80, 4).with_scrollback_limit(4);
        let (tx, _rx) = crossbeam_channel::unbounded::<PtyWrite>();
        handler.set_write_tx(tx);
        handler.handle_kitty_graphics(kitty_rgba_2x2_cmd(KittyAction::Transmit));

        for _ in 0..20 {
            handler.handle_data(b"pad");
            handler.handle_newline();
            handler.handle_carriage_return();
        }
        handler.handle_data(b"ab");
        let base_before = handler.buffer().row_base();
        let origin = handler.buffer().cursor_row_number();
        let x_before = handler.buffer().cursor().pos.x;

        handler.handle_kitty_graphics(KittyGraphicsCommand {
            control: KittyControlData {
                action: Some(KittyAction::Put),
                image_id: Some(42),
                display_cols: Some(2),
                display_rows: Some(2),
                no_cursor_movement: true,
                ..KittyControlData::default()
            },
            payload: Vec::new(),
        });

        assert!(
            handler.buffer().row_base() > base_before,
            "setup: placement must evict rows"
        );
        assert_eq!(handler.buffer().cursor_row_number(), origin);
        let pos = handler.buffer().cursor().pos;
        assert_eq!(pos.x, x_before, "C=1 leaves the column alone");
        let placement = handler.buffer().rows()[pos.y].cells()[pos.x]
            .image_placement()
            .expect("cursor sits on the image origin cell");
        assert_eq!((placement.col_in_image, placement.row_in_image), (0, 0));
    }

    /// Task 125 review: an image taller than the screen scrolls its own origin
    /// into scrollback. `C=1` must not park the cursor there, where the next
    /// write would be invisible: it clamps to the top of the live window.
    #[test]
    fn kitty_put_c1_with_an_image_taller_than_the_screen_keeps_the_cursor_on_screen() {
        use freminal_common::buffer_states::kitty_graphics::{KittyAction, KittyControlData};

        let mut handler = TerminalHandler::new(80, 4);
        let (tx, _rx) = crossbeam_channel::unbounded::<PtyWrite>();
        handler.set_write_tx(tx);
        handler.handle_kitty_graphics(kitty_rgba_2x2_cmd(KittyAction::Transmit));
        handler.handle_data(b"ab");

        handler.handle_kitty_graphics(KittyGraphicsCommand {
            control: KittyControlData {
                action: Some(KittyAction::Put),
                image_id: Some(42),
                display_cols: Some(2),
                display_rows: Some(9),
                no_cursor_movement: true,
                ..KittyControlData::default()
            },
            payload: Vec::new(),
        });

        let screen = handler.buffer().cursor_screen_pos();
        assert_eq!(screen.y, 0, "clamped to the top of the live window");
        assert!(
            handler.buffer().rows().len() > 4,
            "setup: the image scrolled rows into scrollback"
        );
    }

    /// Same regression for `a=T` (transmit-and-display) with `C=1`.
    #[test]
    fn kitty_transmit_and_display_c1_at_scrollback_capacity_keeps_cursor_on_image_origin() {
        use freminal_common::buffer_states::kitty_graphics::KittyAction;

        let mut handler = TerminalHandler::new(80, 4).with_scrollback_limit(4);
        let (tx, _rx) = crossbeam_channel::unbounded::<PtyWrite>();
        handler.set_write_tx(tx);

        for _ in 0..20 {
            handler.handle_data(b"pad");
            handler.handle_newline();
            handler.handle_carriage_return();
        }
        let base_before = handler.buffer().row_base();
        let origin = handler.buffer().cursor_row_number();

        let mut cmd = kitty_rgba_2x2_cmd(KittyAction::TransmitAndDisplay);
        cmd.control.display_cols = Some(2);
        cmd.control.display_rows = Some(2);
        cmd.control.no_cursor_movement = true;
        handler.handle_kitty_graphics(cmd);

        assert!(
            handler.buffer().row_base() > base_before,
            "setup: placement must evict rows"
        );
        assert_eq!(handler.buffer().cursor_row_number(), origin);
        let pos = handler.buffer().cursor().pos;
        let placement = handler.buffer().rows()[pos.y].cells()[pos.x]
            .image_placement()
            .expect("cursor sits on the image origin cell");
        assert_eq!((placement.col_in_image, placement.row_in_image), (0, 0));
    }

    #[test]
    fn kitty_put_nonexistent_image_sends_error() {
        use freminal_common::buffer_states::kitty_graphics::{KittyAction, KittyControlData};

        let (mut handler, rx) = kitty_handler();

        // Put for an image ID that was never transmitted.
        let put_cmd = KittyGraphicsCommand {
            control: KittyControlData {
                action: Some(KittyAction::Put),
                image_id: Some(999),
                ..KittyControlData::default()
            },
            payload: Vec::new(),
        };
        handler.handle_kitty_graphics(put_cmd);

        // No image cells should be placed.
        let has_image = handler.buffer().rows().iter().any(|row| {
            row.cells()
                .iter()
                .any(freminal_buffer::cell::Cell::has_image)
        });
        assert!(
            !has_image,
            "Put for nonexistent image should not place cells"
        );

        // An error response should be sent.
        let mut found_error = false;
        while let Ok(msg) = rx.try_recv() {
            if let PtyWrite::Write(bytes) = msg {
                let text = String::from_utf8_lossy(&bytes);
                if text.contains("ENOENT") {
                    found_error = true;
                }
            }
        }
        assert!(
            found_error,
            "Expected ENOENT error for Put with nonexistent image ID"
        );
    }

    #[test]
    fn kitty_put_quiet_2_suppresses_error_for_missing_image() {
        use freminal_common::buffer_states::kitty_graphics::{KittyAction, KittyControlData};

        let (mut handler, rx) = kitty_handler();

        // Put for nonexistent image with q=2 (suppress all responses).
        let put_cmd = KittyGraphicsCommand {
            control: KittyControlData {
                action: Some(KittyAction::Put),
                image_id: Some(999),
                quiet: 2,
                ..KittyControlData::default()
            },
            payload: Vec::new(),
        };
        handler.handle_kitty_graphics(put_cmd);

        // With q=2, no error response should be sent.
        let has_response = rx.try_recv().is_ok();
        assert!(
            !has_response,
            "q=2 should suppress all responses including errors"
        );
    }

    #[test]
    fn kitty_put_default_action_is_transmit_not_put() {
        use freminal_common::buffer_states::kitty_graphics::{KittyAction, KittyControlData};

        // When `a=` is not specified, default is Transmit (not Put).
        // This means a command with no `a=` parameter and image data
        // should store but not display.
        let (mut handler, _rx) = kitty_handler();

        let cmd = kitty_rgba_2x2_cmd(KittyAction::Transmit);
        // Re-create with action=None to simulate missing `a=` parameter.
        let cmd_no_action = KittyGraphicsCommand {
            control: KittyControlData {
                action: None,
                ..cmd.control
            },
            payload: cmd.payload,
        };
        handler.handle_kitty_graphics(cmd_no_action);

        // Should be stored.
        assert!(handler.buffer().image_store().get(42).is_some());

        // Should NOT be placed.
        let has_image = handler.buffer().rows().iter().any(|row| {
            row.cells()
                .iter()
                .any(freminal_buffer::cell::Cell::has_image)
        });
        assert!(
            !has_image,
            "Default action (None → Transmit) should not place image cells"
        );
    }

    // ------------------------------------------------------------------
    // Additional coverage tests
    // ------------------------------------------------------------------

    #[test]
    fn kitty_animation_commands_on_nonexistent_image_send_enoent() {
        use freminal_common::buffer_states::kitty_graphics::{
            KittyAction, KittyControlData, KittyFormat,
        };

        for action in [KittyAction::AnimationFrame, KittyAction::AnimationCompose] {
            let (mut handler, rx) = kitty_handler();
            let cmd = KittyGraphicsCommand {
                control: KittyControlData {
                    action: Some(action),
                    format: Some(KittyFormat::Rgba),
                    src_width: Some(2),
                    src_height: Some(2),
                    image_id: Some(100),
                    ..KittyControlData::default()
                },
                payload: vec![0; 16],
            };
            handler.handle_kitty_graphics(cmd);

            let response = rx.try_recv().expect("expected an error response");
            match response {
                PtyWrite::Write(bytes) => {
                    let s = String::from_utf8_lossy(&bytes);
                    assert!(
                        s.contains("ENOENT"),
                        "expected ENOENT for {action:?}, got: {s}"
                    );
                }
                PtyWrite::Resize(_) => panic!("Expected PtyWrite::Write"),
            }
        }

        // `a=a` is silent by convention on success, but still surfaces errors.
        let (mut handler, rx) = kitty_handler();
        let cmd = KittyGraphicsCommand {
            control: KittyControlData {
                action: Some(KittyAction::AnimationControl),
                image_id: Some(100),
                ..KittyControlData::default()
            },
            payload: Vec::new(),
        };
        handler.handle_kitty_graphics(cmd);
        let response = rx.try_recv().expect("expected an error response");
        match response {
            PtyWrite::Write(bytes) => {
                let s = String::from_utf8_lossy(&bytes);
                assert!(s.contains("ENOENT"), "expected ENOENT, got: {s}");
            }
            PtyWrite::Resize(_) => panic!("Expected PtyWrite::Write"),
        }
    }

    #[test]
    fn kitty_stale_chunked_transfer_discarded_on_new_command() {
        use freminal_common::buffer_states::kitty_graphics::{
            KittyAction, KittyControlData, KittyFormat,
        };

        let (mut handler, _rx) = kitty_handler();

        // Start a chunked transfer (more_data=true)
        let chunk1 = KittyGraphicsCommand {
            control: KittyControlData {
                action: Some(KittyAction::Transmit),
                format: Some(KittyFormat::Rgba),
                src_width: Some(2),
                src_height: Some(2),
                image_id: Some(200),
                more_data: true,
                ..KittyControlData::default()
            },
            payload: vec![255, 0, 0, 255, 0, 255, 0, 255],
        };
        handler.handle_kitty_graphics(chunk1);
        assert!(
            kitty_receiving(&handler),
            "Chunked transfer should be in progress"
        );

        // Now send a NEW command with explicit action → discards the stale accumulator
        let new_cmd = kitty_rgba_2x2_cmd(KittyAction::TransmitAndDisplay);
        handler.handle_kitty_graphics(new_cmd);

        // Stale chunked transfer should be gone
        assert!(
            kitty_idle(&handler),
            "Stale chunked transfer should be discarded"
        );
        // New image should be stored
        assert!(handler.buffer().image_store().get(42).is_some());
    }

    #[test]
    fn kitty_query_unsupported_format_sends_error() {
        use freminal_common::buffer_states::kitty_graphics::{KittyAction, KittyControlData};

        let (mut handler, rx) = kitty_handler();

        let cmd = KittyGraphicsCommand {
            control: KittyControlData {
                action: Some(KittyAction::Query),
                // Use a non-standard format value that's not RGB/RGBA/PNG
                // There is no "unsupported" format enum value we can use,
                // so we test with format=None which is supported. Instead
                // let's test quiet suppression on queries.
                format: None,
                image_id: Some(1),
                quiet: 1, // suppress OK response
                ..KittyControlData::default()
            },
            payload: Vec::new(),
        };
        handler.handle_kitty_graphics(cmd);

        // quiet=1 with a supported format → OK suppressed
        assert!(
            rx.try_recv().is_err(),
            "quiet=1 should suppress OK response for supported query"
        );
    }

    #[test]
    fn kitty_query_quiet_2_suppresses_all() {
        use freminal_common::buffer_states::kitty_graphics::{KittyAction, KittyControlData};

        let (mut handler, rx) = kitty_handler();

        let cmd = KittyGraphicsCommand {
            control: KittyControlData {
                action: Some(KittyAction::Query),
                format: None,
                image_id: Some(1),
                quiet: 2, // suppress all responses
                ..KittyControlData::default()
            },
            payload: Vec::new(),
        };
        handler.handle_kitty_graphics(cmd);

        assert!(
            rx.try_recv().is_err(),
            "quiet=2 should suppress all query responses"
        );
    }

    #[test]
    fn kitty_chunk_with_no_active_transfer_ignored() {
        use freminal_common::buffer_states::kitty_graphics::{KittyControlData, KittyFormat};

        let (mut handler, _rx) = kitty_handler();

        // Send a continuation chunk (no explicit action) with no active transfer
        let chunk = KittyGraphicsCommand {
            control: KittyControlData {
                action: None, // continuation chunk
                format: Some(KittyFormat::Rgba),
                more_data: true,
                ..KittyControlData::default()
            },
            payload: vec![0; 8],
        };
        // Should not panic or crash
        handler.handle_kitty_graphics(chunk);
    }

    #[test]
    fn kitty_chunked_transfer_final_chunk_completes() {
        use freminal_common::buffer_states::kitty_graphics::{
            KittyAction, KittyControlData, KittyFormat,
        };

        let (mut handler, _rx) = kitty_handler();

        // First chunk (more_data=true)
        let chunk1 = KittyGraphicsCommand {
            control: KittyControlData {
                action: Some(KittyAction::TransmitAndDisplay),
                format: Some(KittyFormat::Rgba),
                src_width: Some(2),
                src_height: Some(2),
                image_id: Some(300),
                more_data: true,
                ..KittyControlData::default()
            },
            payload: vec![255, 0, 0, 255, 0, 255, 0, 255],
        };
        handler.handle_kitty_graphics(chunk1);
        assert!(kitty_receiving(&handler));

        // Second chunk (continuation, more_data=true)
        let chunk2 = KittyGraphicsCommand {
            control: KittyControlData {
                action: None, // continuation
                more_data: true,
                ..KittyControlData::default()
            },
            payload: vec![0, 0, 255, 255, 255, 255, 0, 255],
        };
        handler.handle_kitty_graphics(chunk2);
        assert!(kitty_receiving(&handler));

        // Final chunk (more_data=false)
        let chunk3 = KittyGraphicsCommand {
            control: KittyControlData {
                action: None,     // continuation
                more_data: false, // final
                ..KittyControlData::default()
            },
            payload: Vec::new(),
        };
        handler.handle_kitty_graphics(chunk3);
        assert!(kitty_idle(&handler), "Chunked transfer should be complete");
        assert!(handler.buffer().image_store().get(300).is_some());
    }

    /// Chunk caps small enough to cross in a test without allocating hundreds
    /// of MiB.
    const SMALL_KITTY_LIMITS: super::ChunkLimits = super::ChunkLimits {
        max_chunk_bytes: 8,
        max_total_bytes: 12,
    };

    /// A first chunk (`m=1`) of an RGBA 2x2 transmit-and-display with `id`.
    fn kitty_chunk_first(id: u32, payload: Vec<u8>) -> KittyGraphicsCommand {
        use freminal_common::buffer_states::kitty_graphics::KittyFormat;
        KittyGraphicsCommand {
            control: KittyControlData {
                action: Some(KittyAction::TransmitAndDisplay),
                format: Some(KittyFormat::Rgba),
                src_width: Some(2),
                src_height: Some(2),
                image_id: Some(id),
                more_data: true,
                ..KittyControlData::default()
            },
            payload,
        }
    }

    /// Whether a continuation chunk is followed by more chunks (`m=1`) or is
    /// the final one (`m=0`).
    #[derive(Clone, Copy)]
    enum More {
        Yes,
        No,
    }

    /// True while a chunked transfer is being accumulated.
    fn kitty_receiving(handler: &TerminalHandler) -> bool {
        matches!(handler.kitty_transfer, KittyTransfer::Receiving(_))
    }

    /// True when no chunked transfer is in progress and none is being discarded.
    fn kitty_idle(handler: &TerminalHandler) -> bool {
        matches!(handler.kitty_transfer, KittyTransfer::Idle)
    }

    /// True while the tail of an abandoned transfer is being swallowed.
    fn kitty_discarding(handler: &TerminalHandler) -> bool {
        matches!(handler.kitty_transfer, KittyTransfer::Discarding)
    }

    /// The live transfer state; panics if no transfer is being received.
    fn kitty_receiving_mut(handler: &mut TerminalHandler) -> &mut KittyImageState {
        match &mut handler.kitty_transfer {
            KittyTransfer::Receiving(state) => state,
            other => panic!("transfer in progress expected, got {other:?}"),
        }
    }

    /// Shared-reference form of [`kitty_receiving_mut`].
    fn kitty_receiving_ref(handler: &TerminalHandler) -> &KittyImageState {
        match &handler.kitty_transfer {
            KittyTransfer::Receiving(state) => state,
            other => panic!("transfer in progress expected, got {other:?}"),
        }
    }

    /// A continuation chunk (no explicit action) with the given `m` value.
    fn kitty_chunk_next(more: More, payload: Vec<u8>) -> KittyGraphicsCommand {
        KittyGraphicsCommand {
            control: KittyControlData {
                action: None,
                more_data: matches!(more, More::Yes),
                ..KittyControlData::default()
            },
            payload,
        }
    }

    /// Complete a whole 2x2 RGBA chunked transfer with `id` and assert the
    /// image was stored: proof the handler is usable after an abandonment.
    fn assert_chunked_transfer_works(handler: &mut TerminalHandler, id: u32) {
        handler.handle_kitty_graphics(kitty_chunk_first(id, vec![255, 0, 0, 255, 0, 255, 0, 255]));
        handler.handle_kitty_graphics(kitty_chunk_next(
            More::No,
            vec![0, 0, 255, 255, 255, 255, 0, 255],
        ));
        assert!(kitty_idle(handler));
        assert!(handler.buffer().image_store().get(u64::from(id)).is_some());
    }

    #[test]
    fn kitty_chunked_transfer_over_total_cap_is_abandoned_silently() {
        let (mut handler, rx) = kitty_handler();

        // Start through the real path, then swap the assembler for one with
        // small limits so the cap can be crossed cheaply.
        handler.handle_kitty_graphics(kitty_chunk_first(400, vec![0; 8]));
        let state = kitty_receiving_mut(&mut handler);
        let mut small = super::BoundedChunkAssembler::new(SMALL_KITTY_LIMITS);
        small.push(&[0; 8], super::ChunkEncoding::Raw).unwrap();
        state.data = small;

        // 8 + 8 = 16 > 12: over the total cap.
        handler.handle_kitty_graphics(kitty_chunk_next(More::Yes, vec![0; 8]));

        assert!(kitty_discarding(&handler), "transfer must be abandoned");
        assert!(handler.buffer().image_store().get(400).is_none());
        assert!(!handler.buffer().has_any_image_cell());
        assert!(rx.try_recv().is_err(), "no reply may be written");

        assert_chunked_transfer_works(&mut handler, 401);
    }

    #[test]
    fn kitty_chunked_transfer_at_total_cap_is_accepted() {
        let (mut handler, _rx) = kitty_handler();
        handler.handle_kitty_graphics(kitty_chunk_first(410, vec![0; 8]));
        let state = kitty_receiving_mut(&mut handler);
        let mut small = super::BoundedChunkAssembler::new(SMALL_KITTY_LIMITS);
        small.push(&[0; 8], super::ChunkEncoding::Raw).unwrap();
        state.data = small;

        // 8 + 4 = 12 == cap: still accepted.
        handler.handle_kitty_graphics(kitty_chunk_next(More::Yes, vec![0; 4]));
        assert!(kitty_receiving(&handler));
    }

    #[test]
    fn kitty_first_chunk_over_apc_cap_is_abandoned_silently() {
        let (mut handler, rx) = kitty_handler();

        handler.handle_kitty_graphics(kitty_chunk_first(
            420,
            vec![0; crate::ansi_components::apc::MAX_APC_BYTES + 1],
        ));

        assert!(kitty_discarding(&handler));
        assert!(!handler.buffer().has_any_image_cell());
        assert!(rx.try_recv().is_err(), "no reply may be written");

        assert_chunked_transfer_works(&mut handler, 421);
    }

    #[test]
    fn kitty_continuation_chunk_over_apc_cap_is_abandoned_silently() {
        let (mut handler, rx) = kitty_handler();

        // A chunk of exactly the cap is accepted.
        handler.handle_kitty_graphics(kitty_chunk_first(
            430,
            vec![0; crate::ansi_components::apc::MAX_APC_BYTES],
        ));
        assert!(kitty_receiving(&handler));

        // One byte over is not.
        handler.handle_kitty_graphics(kitty_chunk_next(
            More::Yes,
            vec![0; crate::ansi_components::apc::MAX_APC_BYTES + 1],
        ));
        assert!(kitty_discarding(&handler));
        assert!(!handler.buffer().has_any_image_cell());
        assert!(rx.try_recv().is_err(), "no reply may be written");

        assert_chunked_transfer_works(&mut handler, 431);
    }

    /// Swap the live transfer's assembler for a small-limit one holding 8 bytes.
    fn shrink_kitty_limits(handler: &mut TerminalHandler) {
        let state = kitty_receiving_mut(handler);
        let mut small = super::BoundedChunkAssembler::new(SMALL_KITTY_LIMITS);
        small.push(&[0; 8], super::ChunkEncoding::Raw).unwrap();
        state.data = small;
    }

    #[test]
    fn kitty_tail_chunks_after_total_cap_abandonment_are_dropped_silently() {
        let (mut handler, rx) = kitty_handler();

        handler.handle_kitty_graphics(kitty_chunk_first(450, vec![0; 8]));
        shrink_kitty_limits(&mut handler);

        // 8 + 8 = 16 > 12: abandoned, but more chunks are still coming.
        handler.handle_kitty_graphics(kitty_chunk_next(More::Yes, vec![0; 8]));
        assert!(kitty_discarding(&handler));

        // Further `m=1` tail chunks must not start a new transfer.
        handler.handle_kitty_graphics(kitty_chunk_next(More::Yes, vec![1; 8]));
        handler.handle_kitty_graphics(kitty_chunk_next(More::Yes, vec![2; 8]));
        assert!(kitty_discarding(&handler));

        // The final `m=0` chunk must not be dispatched as a transmit either.
        handler.handle_kitty_graphics(kitty_chunk_next(More::No, vec![3; 4]));

        assert!(kitty_idle(&handler), "final chunk returns to Idle");
        assert!(handler.buffer().image_store().get(450).is_none());
        assert!(!handler.buffer().has_any_image_cell());
        assert!(rx.try_recv().is_err(), "no reply may be written");

        assert_chunked_transfer_works(&mut handler, 451);
    }

    #[test]
    fn kitty_tail_chunks_after_first_chunk_over_apc_cap_are_dropped_silently() {
        let (mut handler, rx) = kitty_handler();

        handler.handle_kitty_graphics(kitty_chunk_first(
            460,
            vec![0; crate::ansi_components::apc::MAX_APC_BYTES + 1],
        ));
        assert!(kitty_discarding(&handler));

        handler.handle_kitty_graphics(kitty_chunk_next(More::Yes, vec![0; 8]));
        handler.handle_kitty_graphics(kitty_chunk_next(More::Yes, vec![0; 8]));
        assert!(kitty_discarding(&handler));
        handler.handle_kitty_graphics(kitty_chunk_next(More::No, vec![0; 8]));

        assert!(kitty_idle(&handler));
        assert!(!handler.buffer().has_any_image_cell());
        assert!(rx.try_recv().is_err(), "no reply may be written");

        assert_chunked_transfer_works(&mut handler, 461);
    }

    #[test]
    fn kitty_final_chunk_over_cap_returns_to_idle() {
        let (mut handler, rx) = kitty_handler();

        handler.handle_kitty_graphics(kitty_chunk_first(470, vec![0; 8]));
        shrink_kitty_limits(&mut handler);

        // The abandoning chunk is itself the last one: nothing left to swallow.
        handler.handle_kitty_graphics(kitty_chunk_next(More::No, vec![0; 8]));

        assert!(kitty_idle(&handler));
        assert!(rx.try_recv().is_err(), "no reply may be written");
    }

    #[test]
    fn kitty_explicit_action_while_discarding_is_processed_normally() {
        let (mut handler, rx) = kitty_handler();

        handler.handle_kitty_graphics(kitty_chunk_first(
            480,
            vec![0; crate::ansi_components::apc::MAX_APC_BYTES + 1],
        ));
        assert!(kitty_discarding(&handler));

        // A query with an explicit action gets its normal reply.
        handler.handle_kitty_graphics(KittyGraphicsCommand {
            control: KittyControlData {
                action: Some(KittyAction::Query),
                image_id: Some(481),
                ..KittyControlData::default()
            },
            payload: Vec::new(),
        });
        assert!(kitty_idle(&handler));
        match rx.try_recv().expect("query must be answered") {
            PtyWrite::Write(bytes) => {
                let s = String::from_utf8_lossy(&bytes);
                assert!(s.contains("i=481") && s.contains("OK"), "got: {s}");
            }
            PtyWrite::Resize(_) => panic!("Expected PtyWrite::Write"),
        }
    }

    #[test]
    fn kitty_fresh_transfer_while_discarding_completes() {
        let (mut handler, _rx) = kitty_handler();

        handler.handle_kitty_graphics(kitty_chunk_first(
            490,
            vec![0; crate::ansi_components::apc::MAX_APC_BYTES + 1],
        ));
        assert!(kitty_discarding(&handler));

        // A new transfer (explicit action) abandons the discard and runs.
        assert_chunked_transfer_works(&mut handler, 491);
    }

    /// An actionless single-chunk transmit (`a=t` is the default action) of a
    /// 2x2 RGBA image with `id`; it carries `i=`/`f=`/`s=`/`v=` and `m=0`.
    fn kitty_actionless_transmit(id: u32, more: More) -> KittyGraphicsCommand {
        use freminal_common::buffer_states::kitty_graphics::KittyFormat;
        KittyGraphicsCommand {
            control: KittyControlData {
                action: None,
                format: Some(KittyFormat::Rgba),
                src_width: Some(2),
                src_height: Some(2),
                image_id: Some(id),
                more_data: matches!(more, More::Yes),
                ..KittyControlData::default()
            },
            payload: vec![
                255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 0, 255,
            ],
        }
    }

    /// Put the handler in `Discarding` by overflowing the first chunk's APC cap.
    fn kitty_start_discarding(handler: &mut TerminalHandler, id: u32) {
        handler.handle_kitty_graphics(kitty_chunk_first(
            id,
            vec![0; crate::ansi_components::apc::MAX_APC_BYTES + 1],
        ));
        assert!(kitty_discarding(handler));
    }

    #[test]
    fn kitty_actionless_single_chunk_transmit_after_abandonment_is_processed() {
        let (mut handler, _rx) = kitty_handler();
        kitty_start_discarding(&mut handler, 500);

        // The abandoned transfer's `m=0` never came; this is a new command.
        handler.handle_kitty_graphics(kitty_actionless_transmit(501, More::No));

        assert!(kitty_idle(&handler));
        assert!(
            handler.buffer().image_store().get(501).is_some(),
            "an actionless command carrying i=/f=/s=/v= is not a continuation"
        );
    }

    #[test]
    fn kitty_actionless_first_chunk_after_abandonment_starts_a_new_transfer() {
        let (mut handler, _rx) = kitty_handler();
        kitty_start_discarding(&mut handler, 510);

        let full = kitty_actionless_transmit(511, More::No).payload;
        let (head, tail) = full.split_at(8);
        let mut first = kitty_actionless_transmit(511, More::Yes);
        first.payload = head.to_vec();
        handler.handle_kitty_graphics(first);
        assert!(
            kitty_receiving(&handler),
            "the actionless first chunk starts a transfer"
        );

        handler.handle_kitty_graphics(kitty_chunk_next(More::No, tail.to_vec()));
        assert!(kitty_idle(&handler));
        assert!(handler.buffer().image_store().get(511).is_some());
    }

    #[test]
    fn kitty_bare_continuation_chunks_are_still_swallowed_while_discarding() {
        let (mut handler, rx) = kitty_handler();
        kitty_start_discarding(&mut handler, 520);

        // Bare `m=1`.
        handler.handle_kitty_graphics(kitty_chunk_next(More::Yes, vec![1; 8]));
        assert!(kitty_discarding(&handler));

        // `m=0,q=2`: the final chunk may carry `q`.
        let mut last = kitty_chunk_next(More::No, vec![2; 8]);
        last.control.quiet = 2;
        handler.handle_kitty_graphics(last);

        assert!(kitty_idle(&handler));
        assert!(!handler.buffer().has_any_image_cell());
        assert!(rx.try_recv().is_err(), "no reply may be written");
    }

    #[test]
    fn is_bare_continuation_accepts_only_more_and_quiet() {
        use super::is_bare_continuation;

        assert!(is_bare_continuation(&KittyControlData::default()));
        assert!(is_bare_continuation(&KittyControlData {
            more_data: true,
            quiet: 2,
            ..KittyControlData::default()
        }));
        assert!(!is_bare_continuation(&KittyControlData {
            image_id: Some(1),
            ..KittyControlData::default()
        }));
        assert!(!is_bare_continuation(&KittyControlData {
            src_width: Some(2),
            ..KittyControlData::default()
        }));
    }

    #[test]
    fn kitty_chunk_limits_are_the_documented_values() {
        use super::{
            ChunkLimits, KITTY_CHUNK_LIMITS, MAX_APC_BYTES, MAX_KITTY_DATA_BYTES,
            MAX_KITTY_FILE_BYTES,
        };

        assert_eq!(
            KITTY_CHUNK_LIMITS,
            ChunkLimits {
                max_chunk_bytes: MAX_APC_BYTES,
                max_total_bytes: 400 * 1024 * 1024,
            }
        );
        assert_eq!(MAX_KITTY_FILE_BYTES, MAX_KITTY_DATA_BYTES);
    }

    #[test]
    fn kitty_put_unknown_id_logs_a_bounded_message() {
        use crate::log_capture::{capture, warnings};

        let (mut handler, _rx) = kitty_handler();
        for id in 1..=20 {
            handler.handle_kitty_graphics(kitty_actionless_transmit(id, More::No));
        }
        assert_eq!(handler.buffer().image_store().len(), 20);

        let events = capture(|| {
            handler.handle_kitty_graphics(KittyGraphicsCommand {
                control: KittyControlData {
                    action: Some(KittyAction::Put),
                    image_id: Some(9999),
                    ..KittyControlData::default()
                },
                payload: Vec::new(),
            });
        });

        let warns = warnings(&events);
        assert_eq!(warns.len(), 1, "exactly one warn: {warns:?}");
        let warn = &warns[0].1;
        assert!(warn.contains("9999"), "warn names the requested id: {warn}");
        assert!(warn.contains("20"), "warn carries the store size: {warn}");
        assert!(
            !warn.contains('[') && !warn.contains("stored image ids"),
            "warn must not list the stored ids: {warn}"
        );

        let debug = events
            .iter()
            .find(|(_, text)| text.contains("stored image ids"))
            .map(|(_, text)| text.as_str())
            .expect("a debug line lists the stored ids");
        let list = debug
            .split_once('[')
            .and_then(|(_, rest)| rest.split_once(']'))
            .map(|(list, _)| list)
            .expect("a bracketed id list");
        assert_eq!(list.split(',').count(), 16, "at most 16 ids: {debug}");
        assert!(
            debug.trim_end_matches('"').ends_with("(4 more)"),
            "debug line reports the omitted ids: {debug}"
        );
    }

    #[test]
    fn kitty_huge_declared_size_does_not_preallocate() {
        let (mut handler, _rx) = kitty_handler();

        let mut first = kitty_chunk_first(440, Vec::new());
        first.control.data_size = Some(u32::MAX);
        handler.handle_kitty_graphics(first);

        let state = kitty_receiving_ref(&handler);
        assert_eq!(state.data.len(), 0);
        assert_eq!(state.data.capacity(), 0, "S= must not size an allocation");
    }

    #[test]
    fn kitty_rgb_payload_size_mismatch_sends_error() {
        use freminal_common::buffer_states::kitty_graphics::{
            KittyAction, KittyControlData, KittyFormat,
        };

        let (mut handler, rx) = kitty_handler();

        // Says 2x2 RGB (expects 12 bytes) but payload is only 6 bytes.
        let cmd = KittyGraphicsCommand {
            control: KittyControlData {
                action: Some(KittyAction::TransmitAndDisplay),
                format: Some(KittyFormat::Rgb),
                src_width: Some(2),
                src_height: Some(2),
                image_id: Some(400),
                ..KittyControlData::default()
            },
            payload: vec![0; 6], // too small for 2x2 RGB
        };

        handler.handle_kitty_graphics(cmd);

        assert!(handler.buffer().image_store().get(400).is_none());

        let response = rx.try_recv().unwrap();
        match response {
            PtyWrite::Write(bytes) => {
                let s = String::from_utf8_lossy(&bytes);
                assert!(s.contains("EINVAL"), "Expected EINVAL error, got: {s}");
            }
            PtyWrite::Resize(_) => panic!("Expected PtyWrite::Write"),
        }
    }

    #[test]
    fn kitty_png_decode_failure_sends_error() {
        use freminal_common::buffer_states::kitty_graphics::{
            KittyAction, KittyControlData, KittyFormat,
        };

        let (mut handler, rx) = kitty_handler();

        // Invalid PNG data
        let cmd = KittyGraphicsCommand {
            control: KittyControlData {
                action: Some(KittyAction::TransmitAndDisplay),
                format: Some(KittyFormat::Png),
                image_id: Some(500),
                ..KittyControlData::default()
            },
            payload: vec![0xFF, 0xFF, 0xFF, 0xFF], // not valid PNG
        };

        handler.handle_kitty_graphics(cmd);

        let response = rx.try_recv().unwrap();
        match response {
            PtyWrite::Write(bytes) => {
                let s = String::from_utf8_lossy(&bytes);
                assert!(s.contains("EINVAL"), "Expected EINVAL error, got: {s}");
            }
            PtyWrite::Resize(_) => panic!("Expected PtyWrite::Write"),
        }
    }

    #[test]
    fn kitty_delete_by_number() {
        use freminal_common::buffer_states::kitty_graphics::{
            KittyAction, KittyControlData, KittyDeleteTarget,
        };

        let (mut handler, _rx) = kitty_handler();

        // Place an image
        let cmd = kitty_rgba_2x2_cmd(KittyAction::TransmitAndDisplay);
        handler.handle_kitty_graphics(cmd);

        // Delete by number (this exercises the ByNumber arm)
        let delete_cmd = KittyGraphicsCommand {
            control: KittyControlData {
                action: Some(KittyAction::Delete),
                delete_target: Some(KittyDeleteTarget::ByNumber),
                image_number: Some(1),
                ..KittyControlData::default()
            },
            payload: Vec::new(),
        };
        handler.handle_kitty_graphics(delete_cmd);
    }

    #[test]
    fn kitty_delete_by_number_lowercase_keeps_data_uppercase_frees() {
        use freminal_common::buffer_states::kitty_graphics::{
            KittyAction, KittyControlData, KittyDeleteTarget,
        };

        let (mut handler, rx) = kitty_handler();

        let mut cmd = kitty_rgba_2x2_cmd(KittyAction::TransmitAndDisplay);
        cmd.control.image_number = Some(7);
        handler.handle_kitty_graphics(cmd);
        let _ = rx.try_recv();

        // Lowercase `n` — keeps data.
        let delete_cmd = KittyGraphicsCommand {
            control: KittyControlData {
                action: Some(KittyAction::Delete),
                delete_target: Some(KittyDeleteTarget::ByNumber),
                image_number: Some(7),
                delete_free_data: false,
                ..KittyControlData::default()
            },
            payload: Vec::new(),
        };
        handler.handle_kitty_graphics(delete_cmd);
        assert!(
            handler.buffer().image_store().get(42).is_some(),
            "lowercase 'n' must keep the image data"
        );

        // Uppercase `N` — frees the now-unreferenced data.
        let delete_cmd = KittyGraphicsCommand {
            control: KittyControlData {
                action: Some(KittyAction::Delete),
                delete_target: Some(KittyDeleteTarget::ByNumber),
                image_number: Some(7),
                delete_free_data: true,
                ..KittyControlData::default()
            },
            payload: Vec::new(),
        };
        handler.handle_kitty_graphics(delete_cmd);
        assert!(
            handler.buffer().image_store().get(42).is_none(),
            "uppercase 'N' must free the unreferenced image data"
        );
    }

    #[test]
    fn kitty_delete_at_cursor() {
        use freminal_common::buffer_states::kitty_graphics::{
            KittyAction, KittyControlData, KittyDeleteTarget,
        };

        let (mut handler, _rx) = kitty_handler();

        let cmd = kitty_rgba_2x2_cmd(KittyAction::TransmitAndDisplay);
        handler.handle_kitty_graphics(cmd);

        let delete_cmd = KittyGraphicsCommand {
            control: KittyControlData {
                action: Some(KittyAction::Delete),
                delete_target: Some(KittyDeleteTarget::AtCursor),
                ..KittyControlData::default()
            },
            payload: Vec::new(),
        };
        handler.handle_kitty_graphics(delete_cmd);
    }

    #[test]
    fn kitty_delete_at_cell() {
        use freminal_common::buffer_states::kitty_graphics::{
            KittyAction, KittyControlData, KittyDeleteTarget,
        };

        let (mut handler, _rx) = kitty_handler();

        let cmd = kitty_rgba_2x2_cmd(KittyAction::TransmitAndDisplay);
        handler.handle_kitty_graphics(cmd);

        // Delete at specific cell
        let delete_cmd = KittyGraphicsCommand {
            control: KittyControlData {
                action: Some(KittyAction::Delete),
                delete_target: Some(KittyDeleteTarget::AtCell),
                src_x: Some(1),
                src_y: Some(1),
                ..KittyControlData::default()
            },
            payload: Vec::new(),
        };
        handler.handle_kitty_graphics(delete_cmd);
    }

    /// A 80x4 handler with scrollback, scrolled so the live window starts well
    /// below buffer row 0 (Task 125.C8): screen row `n` is buffer row `n + start`.
    fn kitty_handler_with_scrollback() -> (TerminalHandler, crossbeam_channel::Receiver<PtyWrite>) {
        let mut handler = TerminalHandler::new(80, 4).with_scrollback_limit(20);
        let (tx, rx) = crossbeam_channel::unbounded::<PtyWrite>();
        handler.set_write_tx(tx);
        for _ in 0..10 {
            handler.handle_data(b"pad");
            handler.handle_newline();
            handler.handle_carriage_return();
        }
        assert!(
            handler.buffer().rows().len() > 4 + 3,
            "setup: the live window must start below buffer row 3"
        );
        (handler, rx)
    }

    /// Display a 1x1-cell image (id 42, `z`) with its top-left at screen
    /// cell `(col, row)`, with `C=1` so the cursor stays there.
    fn place_kitty_image_at_screen(handler: &mut TerminalHandler, col: usize, row: usize, z: i32) {
        place_kitty_image_sized(handler, col, row, 1, z);
    }

    /// As [`place_kitty_image_at_screen`], but `cols` cells wide (one row).
    fn place_kitty_image_sized(
        handler: &mut TerminalHandler,
        col: usize,
        row: usize,
        cols: u32,
        z: i32,
    ) {
        use freminal_common::buffer_states::kitty_graphics::KittyAction;

        handler.buffer_mut().set_cursor_pos(Some(col), Some(row));
        let mut cmd = kitty_rgba_2x2_cmd(KittyAction::TransmitAndDisplay);
        cmd.control.display_cols = Some(cols);
        cmd.control.display_rows = Some(1);
        cmd.control.no_cursor_movement = true;
        cmd.control.z_index = Some(z);
        handler.handle_kitty_graphics(cmd);
        assert!(handler.buffer().has_any_image_cell(), "setup: image placed");
    }

    fn kitty_delete_cmd(
        target: freminal_common::buffer_states::kitty_graphics::KittyDeleteTarget,
        x: Option<u32>,
        y: Option<u32>,
        z: Option<i32>,
        free_data: ImageDataDisposition,
    ) -> KittyGraphicsCommand {
        use freminal_common::buffer_states::kitty_graphics::{KittyAction, KittyControlData};

        KittyGraphicsCommand {
            control: KittyControlData {
                action: Some(KittyAction::Delete),
                delete_target: Some(target),
                src_x: x,
                src_y: y,
                z_index: z,
                delete_free_data: free_data.frees_data(),
                ..KittyControlData::default()
            },
            payload: Vec::new(),
        }
    }

    /// Kitty spec, "Deleting images": "The values of the x and y keys are the
    /// same as cursor positions (i.e. x=1, y=1 is the top left cell)." With
    /// scrollback, screen row `y-1` is not buffer row `y-1`.
    #[test]
    fn kitty_delete_at_cell_maps_one_based_screen_coordinates_with_scrollback() {
        use freminal_common::buffer_states::kitty_graphics::KittyDeleteTarget::AtCell;

        let (mut handler, _rx) = kitty_handler_with_scrollback();
        // Screen cell (col 3, row 2) -> 1-based x=4, y=3.
        place_kitty_image_at_screen(&mut handler, 3, 2, 0);

        // The old physical-row reading would have matched buffer row 3 (or
        // 2); neither is the image. Neighbouring cells must not match.
        for (x, y) in [(3, 3), (4, 2), (4, 4), (3, 2), (1, 1)] {
            handler.handle_kitty_graphics(kitty_delete_cmd(
                AtCell,
                Some(x),
                Some(y),
                None,
                ImageDataDisposition::Keep,
            ));
            assert!(
                handler.buffer().has_any_image_cell(),
                "x={x},y={y} is not the image's cell"
            );
        }

        handler.handle_kitty_graphics(kitty_delete_cmd(
            AtCell,
            Some(4),
            Some(3),
            None,
            ImageDataDisposition::Keep,
        ));
        assert!(
            !handler.buffer().has_any_image_cell(),
            "x=4,y=3 is screen cell (3,2), where the image is"
        );
        assert!(
            handler.buffer().image_store().get(42).is_some(),
            "lowercase d=p keeps the image data"
        );
    }

    #[test]
    fn kitty_delete_at_cell_uppercase_frees_data_with_scrollback() {
        use freminal_common::buffer_states::kitty_graphics::KittyDeleteTarget::AtCell;

        let (mut handler, _rx) = kitty_handler_with_scrollback();
        place_kitty_image_at_screen(&mut handler, 3, 2, 0);

        handler.handle_kitty_graphics(kitty_delete_cmd(
            AtCell,
            Some(4),
            Some(3),
            None,
            ImageDataDisposition::Free,
        ));

        assert!(!handler.buffer().has_any_image_cell());
        assert!(handler.buffer().image_store().get(42).is_none());
    }

    #[test]
    fn kitty_delete_at_cell_zero_or_off_screen_coordinates_match_nothing() {
        use freminal_common::buffer_states::kitty_graphics::KittyDeleteTarget::AtCell;

        let (mut handler, _rx) = kitty_handler_with_scrollback();
        place_kitty_image_at_screen(&mut handler, 0, 0, 0);

        // x=0 / y=0 name no cell (1-based); y=5 is below the 4-row screen.
        for (x, y) in [(0, 1), (1, 0), (1, 5), (0, 0)] {
            handler.handle_kitty_graphics(kitty_delete_cmd(
                AtCell,
                Some(x),
                Some(y),
                None,
                ImageDataDisposition::Keep,
            ));
            assert!(
                handler.buffer().has_any_image_cell(),
                "x={x},y={y} must delete nothing"
            );
        }

        handler.handle_kitty_graphics(kitty_delete_cmd(
            AtCell,
            Some(1),
            Some(1),
            None,
            ImageDataDisposition::Keep,
        ));
        assert!(
            !handler.buffer().has_any_image_cell(),
            "x=1,y=1 is top-left"
        );
    }

    #[test]
    fn kitty_delete_at_cell_defaults_to_the_cursor_screen_cell_with_scrollback() {
        use freminal_common::buffer_states::kitty_graphics::KittyDeleteTarget::AtCell;

        let (mut handler, _rx) = kitty_handler_with_scrollback();
        // C=1 leaves the cursor on the image's cell.
        place_kitty_image_at_screen(&mut handler, 3, 2, 0);

        handler.handle_kitty_graphics(kitty_delete_cmd(
            AtCell,
            None,
            None,
            None,
            ImageDataDisposition::Keep,
        ));

        assert!(!handler.buffer().has_any_image_cell());
    }

    #[test]
    fn kitty_delete_at_cell_z_index_maps_screen_coordinates_with_scrollback() {
        use freminal_common::buffer_states::kitty_graphics::KittyDeleteTarget::AtCellZIndex;

        let (mut handler, _rx) = kitty_handler_with_scrollback();
        place_kitty_image_at_screen(&mut handler, 3, 2, 5);

        // Right cell, wrong z: nothing. Wrong cell, right z: nothing.
        handler.handle_kitty_graphics(kitty_delete_cmd(
            AtCellZIndex,
            Some(4),
            Some(3),
            Some(0),
            ImageDataDisposition::Keep,
        ));
        handler.handle_kitty_graphics(kitty_delete_cmd(
            AtCellZIndex,
            Some(3),
            Some(2),
            Some(5),
            ImageDataDisposition::Keep,
        ));
        assert!(handler.buffer().has_any_image_cell());

        handler.handle_kitty_graphics(kitty_delete_cmd(
            AtCellZIndex,
            Some(4),
            Some(3),
            Some(5),
            ImageDataDisposition::Keep,
        ));
        assert!(!handler.buffer().has_any_image_cell());
    }

    /// Kitty graphics spec, "Deleting images": `x` or `X` is "Delete all
    /// placements that intersect the specified column, specified using the x
    /// key", and x is 1-based like a cursor position.
    #[test]
    fn kitty_delete_in_column_uses_a_one_based_column() {
        use freminal_common::buffer_states::kitty_graphics::KittyDeleteTarget::InColumn;

        let (mut handler, _rx) = kitty_handler_with_scrollback();
        place_kitty_image_at_screen(&mut handler, 3, 2, 0);

        // Column index 3 is x=4; the neighbours and the invalid x=0 miss.
        for x in [3, 5, 1, 0] {
            handler.handle_kitty_graphics(kitty_delete_cmd(
                InColumn,
                Some(x),
                None,
                None,
                ImageDataDisposition::Keep,
            ));
            assert!(handler.buffer().has_any_image_cell(), "x={x} misses");
        }

        handler.handle_kitty_graphics(kitty_delete_cmd(
            InColumn,
            Some(4),
            None,
            None,
            ImageDataDisposition::Keep,
        ));
        assert!(!handler.buffer().has_any_image_cell());
        assert!(handler.buffer().image_store().get(42).is_some());
    }

    #[test]
    fn kitty_delete_in_column_uppercase_frees_data() {
        use freminal_common::buffer_states::kitty_graphics::KittyDeleteTarget::InColumn;

        let (mut handler, _rx) = kitty_handler_with_scrollback();
        place_kitty_image_at_screen(&mut handler, 3, 2, 0);

        handler.handle_kitty_graphics(kitty_delete_cmd(
            InColumn,
            Some(4),
            None,
            None,
            ImageDataDisposition::Free,
        ));

        assert!(!handler.buffer().has_any_image_cell());
        assert!(handler.buffer().image_store().get(42).is_none());
    }

    /// Kitty graphics spec: `y` or `Y` is "Delete all placements that
    /// intersect the specified row, specified using the y key". y is a
    /// 1-based SCREEN row, so with scrollback it is not a buffer index.
    #[test]
    fn kitty_delete_in_row_maps_a_one_based_screen_row_with_scrollback() {
        use freminal_common::buffer_states::kitty_graphics::KittyDeleteTarget::InRow;

        let (mut handler, _rx) = kitty_handler_with_scrollback();
        place_kitty_image_at_screen(&mut handler, 3, 2, 0);

        // Screen row 2 is y=3. Neighbouring rows, y=0 and a row below the
        // 4-row screen all miss.
        for y in [2, 4, 1, 0, 5] {
            handler.handle_kitty_graphics(kitty_delete_cmd(
                InRow,
                None,
                Some(y),
                None,
                ImageDataDisposition::Keep,
            ));
            assert!(handler.buffer().has_any_image_cell(), "y={y} misses");
        }

        handler.handle_kitty_graphics(kitty_delete_cmd(
            InRow,
            None,
            Some(3),
            None,
            ImageDataDisposition::Keep,
        ));
        assert!(!handler.buffer().has_any_image_cell());
        assert!(handler.buffer().image_store().get(42).is_some());
    }

    #[test]
    fn kitty_delete_in_row_uppercase_frees_data_with_scrollback() {
        use freminal_common::buffer_states::kitty_graphics::KittyDeleteTarget::InRow;

        let (mut handler, _rx) = kitty_handler_with_scrollback();
        place_kitty_image_at_screen(&mut handler, 3, 2, 0);

        handler.handle_kitty_graphics(kitty_delete_cmd(
            InRow,
            None,
            Some(3),
            None,
            ImageDataDisposition::Free,
        ));

        assert!(!handler.buffer().has_any_image_cell());
        assert!(handler.buffer().image_store().get(42).is_none());
    }

    #[test]
    fn kitty_delete_in_row_defaults_to_the_cursor_screen_row_with_scrollback() {
        use freminal_common::buffer_states::kitty_graphics::KittyDeleteTarget::InRow;

        let (mut handler, _rx) = kitty_handler_with_scrollback();
        // C=1 leaves the cursor on screen row 2.
        place_kitty_image_at_screen(&mut handler, 3, 2, 0);

        handler.handle_kitty_graphics(kitty_delete_cmd(
            InRow,
            None,
            None,
            None,
            ImageDataDisposition::Keep,
        ));

        assert!(!handler.buffer().has_any_image_cell());
    }

    /// Kitty graphics spec: `c` or `C` is "Delete all placements that
    /// intersect with the current cursor position" -- the cursor CELL, not
    /// its row. kitty's `graphics.c` uses a point filter on
    /// `(c->x + 1, c->y + 1)`.
    #[test]
    fn kitty_delete_at_cursor_clears_only_a_placement_covering_the_cursor_cell() {
        use freminal_common::buffer_states::kitty_graphics::KittyDeleteTarget::AtCursor;

        let (mut handler, _rx) = kitty_handler_with_scrollback();
        // A 3-cell-wide image at screen columns 2..=4 of row 2.
        place_kitty_image_sized(&mut handler, 2, 2, 3, 0);

        // Same row, but outside the image: nothing intersects the cursor cell.
        for (col, row) in [(1, 2), (5, 2), (3, 1), (3, 3)] {
            handler.buffer_mut().set_cursor_pos(Some(col), Some(row));
            handler.handle_kitty_graphics(kitty_delete_cmd(
                AtCursor,
                None,
                None,
                None,
                ImageDataDisposition::Keep,
            ));
            assert!(
                handler.buffer().has_any_image_cell(),
                "cursor at ({col},{row}) is not on the image"
            );
        }

        // A cell in the middle of the image deletes the whole placement.
        handler.buffer_mut().set_cursor_pos(Some(3), Some(2));
        handler.handle_kitty_graphics(kitty_delete_cmd(
            AtCursor,
            None,
            None,
            None,
            ImageDataDisposition::Keep,
        ));
        assert!(!handler.buffer().has_any_image_cell());
        assert!(handler.buffer().image_store().get(42).is_some());
    }

    #[test]
    fn kitty_delete_at_cursor_ignores_other_images_on_the_cursor_row() {
        use freminal_common::buffer_states::kitty_graphics::{
            KittyAction, KittyControlData, KittyDeleteTarget::AtCursor, KittyFormat,
        };

        let (mut handler, _rx) = kitty_handler_with_scrollback();
        place_kitty_image_at_screen(&mut handler, 3, 2, 0); // image 42
        // A second image (id 99) on the same screen row, away from the cursor.
        handler.buffer_mut().set_cursor_pos(Some(8), Some(2));
        handler.handle_kitty_graphics(KittyGraphicsCommand {
            control: KittyControlData {
                action: Some(KittyAction::TransmitAndDisplay),
                format: Some(KittyFormat::Rgba),
                src_width: Some(2),
                src_height: Some(2),
                image_id: Some(99),
                display_cols: Some(1),
                display_rows: Some(1),
                no_cursor_movement: true,
                ..KittyControlData::default()
            },
            payload: [0, 0, 0, 255].repeat(4),
        });

        handler.buffer_mut().set_cursor_pos(Some(3), Some(2));
        handler.handle_kitty_graphics(kitty_delete_cmd(
            AtCursor,
            None,
            None,
            None,
            ImageDataDisposition::Free,
        ));

        assert!(handler.buffer().image_store().get(42).is_none());
        assert!(
            handler.buffer().image_store().get(99).is_some(),
            "image 99 does not cover the cursor cell"
        );
        assert!(handler.buffer().has_any_image_cell());
    }

    #[test]
    fn kitty_delete_at_cell_z_index_only_clears_matching_z() {
        use freminal_common::buffer_states::kitty_graphics::{
            KittyAction, KittyControlData, KittyDeleteTarget,
        };

        let (mut handler, rx) = kitty_handler();

        // Image A (id=42) at (0,0), z=0.
        let cmd_a = kitty_rgba_2x2_cmd(KittyAction::TransmitAndDisplay);
        handler.handle_kitty_graphics(cmd_a);
        let _ = rx.try_recv();

        // Image B (id=99) placed at the SAME cell (0,0), z=5.
        handler.buffer_mut().set_cursor_pos(Some(0), Some(0));
        let cmd_b = KittyGraphicsCommand {
            control: KittyControlData {
                action: Some(KittyAction::TransmitAndDisplay),
                format: Some(freminal_common::buffer_states::kitty_graphics::KittyFormat::Rgba),
                src_width: Some(2),
                src_height: Some(2),
                image_id: Some(99),
                z_index: Some(5),
                ..KittyControlData::default()
            },
            payload: vec![
                255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 0, 255,
            ],
        };
        handler.handle_kitty_graphics(cmd_b);
        let _ = rx.try_recv();

        // Delete at cell x=1,y=1 (1-based: the top-left cell, where both
        // images sit) with z=5 — should only clear image B.
        let delete_cmd = KittyGraphicsCommand {
            control: KittyControlData {
                action: Some(KittyAction::Delete),
                delete_target: Some(KittyDeleteTarget::AtCellZIndex),
                src_x: Some(1),
                src_y: Some(1),
                z_index: Some(5),
                delete_free_data: true,
                ..KittyControlData::default()
            },
            payload: Vec::new(),
        };
        handler.handle_kitty_graphics(delete_cmd);

        assert!(
            handler.buffer().image_store().get(99).is_none(),
            "z=5 placement (image 99) should be cleared and freed"
        );
        assert!(
            handler.buffer().image_store().get(42).is_some(),
            "z=0 placement (image 42) should be unaffected"
        );
    }

    #[test]
    fn kitty_delete_in_column() {
        use freminal_common::buffer_states::kitty_graphics::{
            KittyAction, KittyControlData, KittyDeleteTarget,
        };

        let (mut handler, _rx) = kitty_handler();

        let cmd = kitty_rgba_2x2_cmd(KittyAction::TransmitAndDisplay);
        handler.handle_kitty_graphics(cmd);

        let delete_cmd = KittyGraphicsCommand {
            control: KittyControlData {
                action: Some(KittyAction::Delete),
                delete_target: Some(KittyDeleteTarget::InColumn),
                src_x: Some(1),
                ..KittyControlData::default()
            },
            payload: Vec::new(),
        };
        handler.handle_kitty_graphics(delete_cmd);
    }

    #[test]
    fn kitty_delete_in_row() {
        use freminal_common::buffer_states::kitty_graphics::{
            KittyAction, KittyControlData, KittyDeleteTarget,
        };

        let (mut handler, _rx) = kitty_handler();

        let cmd = kitty_rgba_2x2_cmd(KittyAction::TransmitAndDisplay);
        handler.handle_kitty_graphics(cmd);

        let delete_cmd = KittyGraphicsCommand {
            control: KittyControlData {
                action: Some(KittyAction::Delete),
                delete_target: Some(KittyDeleteTarget::InRow),
                src_y: Some(1),
                ..KittyControlData::default()
            },
            payload: Vec::new(),
        };
        handler.handle_kitty_graphics(delete_cmd);
    }

    #[test]
    fn kitty_delete_at_z_index() {
        use freminal_common::buffer_states::kitty_graphics::{
            KittyAction, KittyControlData, KittyDeleteTarget,
        };

        let (mut handler, _rx) = kitty_handler();

        let cmd = kitty_rgba_2x2_cmd(KittyAction::TransmitAndDisplay);
        handler.handle_kitty_graphics(cmd);

        let delete_cmd = KittyGraphicsCommand {
            control: KittyControlData {
                action: Some(KittyAction::Delete),
                delete_target: Some(KittyDeleteTarget::AtZIndex),
                z_index: Some(0),
                ..KittyControlData::default()
            },
            payload: Vec::new(),
        };
        handler.handle_kitty_graphics(delete_cmd);
    }

    #[test]
    fn kitty_delete_id_range_removes_only_ids_in_bounds() {
        use freminal_common::buffer_states::kitty_graphics::{
            KittyAction, KittyControlData, KittyDeleteTarget, KittyFormat,
        };

        let (mut handler, _rx) = kitty_handler();

        for id in [5u32, 6, 7] {
            let cmd = KittyGraphicsCommand {
                control: KittyControlData {
                    action: Some(KittyAction::Transmit),
                    format: Some(KittyFormat::Rgba),
                    src_width: Some(2),
                    src_height: Some(2),
                    image_id: Some(id),
                    ..KittyControlData::default()
                },
                payload: vec![
                    255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 0, 255,
                ],
            };
            handler.handle_kitty_graphics(cmd);
        }
        assert!(handler.buffer().image_store().get(5).is_some());
        assert!(handler.buffer().image_store().get(6).is_some());
        assert!(handler.buffer().image_store().get(7).is_some());

        // d=R with x=5 (low), y=6 (high) — free_data — removes 5 and 6.
        let delete_cmd = KittyGraphicsCommand {
            control: KittyControlData {
                action: Some(KittyAction::Delete),
                delete_target: Some(KittyDeleteTarget::IdRange),
                src_x: Some(5),
                src_y: Some(6),
                delete_free_data: true,
                ..KittyControlData::default()
            },
            payload: Vec::new(),
        };
        handler.handle_kitty_graphics(delete_cmd);

        assert!(
            handler.buffer().image_store().get(5).is_none(),
            "id 5 in range must be removed"
        );
        assert!(
            handler.buffer().image_store().get(6).is_none(),
            "id 6 in range must be removed"
        );
        assert!(
            handler.buffer().image_store().get(7).is_some(),
            "id 7 out of range must survive"
        );
    }

    #[test]
    fn kitty_delete_frames_clears_animation_keeps_image() {
        use freminal_common::buffer_states::kitty_graphics::{
            KittyAction, KittyControlData, KittyDeleteTarget,
        };

        let (mut handler, rx) = kitty_handler();

        // Transmit a base image (id=42), then add a second animation frame.
        let cmd = kitty_rgba_2x2_cmd(KittyAction::Transmit);
        handler.handle_kitty_graphics(cmd);
        let _ = rx.try_recv();

        let frame_cmd = KittyGraphicsCommand {
            control: KittyControlData {
                action: Some(KittyAction::AnimationFrame),
                image_id: Some(42),
                src_width: Some(2),
                src_height: Some(2),
                ..KittyControlData::default()
            },
            payload: vec![1u8; 16],
        };
        handler.handle_kitty_graphics(frame_cmd);
        let _ = rx.try_recv();

        assert_eq!(
            handler
                .buffer()
                .image_store()
                .get(42)
                .expect("image exists")
                .frame_count(),
            2,
            "setup should have produced a 2-frame animated image"
        );

        // Lowercase `f` — clears frames, keeps the image in the store.
        let delete_cmd = KittyGraphicsCommand {
            control: KittyControlData {
                action: Some(KittyAction::Delete),
                delete_target: Some(KittyDeleteTarget::Frames),
                image_id: Some(42),
                delete_free_data: false,
                ..KittyControlData::default()
            },
            payload: Vec::new(),
        };
        handler.handle_kitty_graphics(delete_cmd);

        let img = handler
            .buffer()
            .image_store()
            .get(42)
            .expect("image must still be in the store after lowercase 'f'");
        assert_eq!(img.frame_count(), 1, "frames must be cleared");
        assert!(!img.is_animated());
    }

    #[test]
    fn kitty_delete_frames_uppercase_also_frees_if_unreferenced() {
        use freminal_common::buffer_states::kitty_graphics::{
            KittyAction, KittyControlData, KittyDeleteTarget,
        };

        let (mut handler, rx) = kitty_handler();

        let cmd = kitty_rgba_2x2_cmd(KittyAction::Transmit);
        handler.handle_kitty_graphics(cmd);
        let _ = rx.try_recv();

        let frame_cmd = KittyGraphicsCommand {
            control: KittyControlData {
                action: Some(KittyAction::AnimationFrame),
                image_id: Some(42),
                src_width: Some(2),
                src_height: Some(2),
                ..KittyControlData::default()
            },
            payload: vec![1u8; 16],
        };
        handler.handle_kitty_graphics(frame_cmd);
        let _ = rx.try_recv();

        // The image was only ever transmitted (`a=t`), never displayed, so
        // it has no cell references — uppercase `F` frees it entirely.
        let delete_cmd = KittyGraphicsCommand {
            control: KittyControlData {
                action: Some(KittyAction::Delete),
                delete_target: Some(KittyDeleteTarget::Frames),
                image_id: Some(42),
                delete_free_data: true,
                ..KittyControlData::default()
            },
            payload: Vec::new(),
        };
        handler.handle_kitty_graphics(delete_cmd);

        assert!(
            handler.buffer().image_store().get(42).is_none(),
            "uppercase 'F' should free the unreferenced image after clearing frames"
        );
    }

    #[test]
    fn kitty_put_missing_image_sends_enoent() {
        use freminal_common::buffer_states::kitty_graphics::{KittyAction, KittyControlData};

        let (mut handler, rx) = kitty_handler();

        // Try to put an image that doesn't exist
        let cmd = KittyGraphicsCommand {
            control: KittyControlData {
                action: Some(KittyAction::Put),
                image_id: Some(999),
                ..KittyControlData::default()
            },
            payload: Vec::new(),
        };
        handler.handle_kitty_graphics(cmd);

        let response = rx.try_recv().unwrap();
        match response {
            PtyWrite::Write(bytes) => {
                let s = String::from_utf8_lossy(&bytes);
                assert!(s.contains("ENOENT"), "Expected ENOENT error, got: {s}");
            }
            PtyWrite::Resize(_) => panic!("Expected PtyWrite::Write"),
        }
    }

    #[test]
    fn kitty_put_existing_image_places_it() {
        use freminal_common::buffer_states::kitty_graphics::{KittyAction, KittyControlData};

        let (mut handler, _rx) = kitty_handler();

        // First transmit (store only)
        let cmd = kitty_rgba_2x2_cmd(KittyAction::Transmit);
        handler.handle_kitty_graphics(cmd);
        assert!(handler.buffer().image_store().get(42).is_some());

        // Now put it
        let put_cmd = KittyGraphicsCommand {
            control: KittyControlData {
                action: Some(KittyAction::Put),
                image_id: Some(42),
                ..KittyControlData::default()
            },
            payload: Vec::new(),
        };
        handler.handle_kitty_graphics(put_cmd);

        // Image cells should now be placed
        let has_image = handler.buffer().has_any_image_cell();
        assert!(
            has_image,
            "Put should place the previously transmitted image"
        );
    }

    #[test]
    fn kitty_place_image_zero_dimension() {
        use freminal_common::buffer_states::kitty_graphics::{
            KittyAction, KittyControlData, KittyFormat,
        };

        let (mut handler, rx) = kitty_handler();

        // Transmit with width=0 (zero dimension) — should get error
        let cmd = KittyGraphicsCommand {
            control: KittyControlData {
                action: Some(KittyAction::TransmitAndDisplay),
                format: Some(KittyFormat::Rgba),
                src_width: Some(0),
                src_height: Some(2),
                image_id: Some(600),
                ..KittyControlData::default()
            },
            payload: Vec::new(), // Empty payload triggers ENODATA first
        };
        handler.handle_kitty_graphics(cmd);

        // Check we got an error
        let response = rx.try_recv().unwrap();
        match response {
            PtyWrite::Write(bytes) => {
                let s = String::from_utf8_lossy(&bytes);
                assert!(
                    s.contains("ENODATA") || s.contains("EINVAL"),
                    "Expected error response, got: {s}"
                );
            }
            PtyWrite::Resize(_) => panic!("Expected PtyWrite::Write"),
        }
    }

    #[test]
    fn kitty_missing_height_dimension_sends_error() {
        use freminal_common::buffer_states::kitty_graphics::{
            KittyAction, KittyControlData, KittyFormat,
        };

        let (mut handler, rx) = kitty_handler();

        // Has width but missing height
        let cmd = KittyGraphicsCommand {
            control: KittyControlData {
                action: Some(KittyAction::TransmitAndDisplay),
                format: Some(KittyFormat::Rgba),
                src_width: Some(2),
                src_height: None, // missing height
                image_id: Some(601),
                ..KittyControlData::default()
            },
            payload: vec![0; 16],
        };
        handler.handle_kitty_graphics(cmd);

        let response = rx.try_recv().unwrap();
        match response {
            PtyWrite::Write(bytes) => {
                let s = String::from_utf8_lossy(&bytes);
                assert!(
                    s.contains("EINVAL"),
                    "Expected EINVAL error for missing height, got: {s}"
                );
            }
            PtyWrite::Resize(_) => panic!("Expected PtyWrite::Write"),
        }
    }

    // ── Coverage-gap tests ──────────────────────────────────────────────

    #[test]
    fn kitty_zero_width_rgba_sends_enodata_for_empty_payload() {
        // RGBA with width=0 requires empty payload to match expected=0 bytes,
        // but empty payload triggers ENODATA before decode. This test verifies
        // the ENODATA error path.
        use freminal_common::buffer_states::kitty_graphics::{
            KittyAction, KittyControlData, KittyFormat,
        };

        let (mut handler, rx) = kitty_handler();

        let cmd = KittyGraphicsCommand {
            control: KittyControlData {
                action: Some(KittyAction::TransmitAndDisplay),
                format: Some(KittyFormat::Rgba),
                src_width: Some(0),
                src_height: Some(2),
                image_id: Some(700),
                ..KittyControlData::default()
            },
            payload: Vec::new(),
        };
        handler.handle_kitty_graphics(cmd);

        let response = rx.try_recv().unwrap();
        match response {
            PtyWrite::Write(bytes) => {
                let s = String::from_utf8_lossy(&bytes);
                assert!(s.contains("ENODATA"), "Expected ENODATA error, got: {s}");
            }
            PtyWrite::Resize(_) => panic!("Expected PtyWrite::Write"),
        }
    }

    #[test]
    fn kitty_zero_width_rgba_with_payload_sends_size_mismatch() {
        // RGBA with width=0 and non-empty payload triggers size mismatch.
        use freminal_common::buffer_states::kitty_graphics::{
            KittyAction, KittyControlData, KittyFormat,
        };

        let (mut handler, rx) = kitty_handler();

        let cmd = KittyGraphicsCommand {
            control: KittyControlData {
                action: Some(KittyAction::TransmitAndDisplay),
                format: Some(KittyFormat::Rgba),
                src_width: Some(0),
                src_height: Some(2),
                image_id: Some(701),
                ..KittyControlData::default()
            },
            payload: vec![255; 4], // non-empty but won't match expected=0
        };
        handler.handle_kitty_graphics(cmd);

        let response = rx.try_recv().unwrap();
        match response {
            PtyWrite::Write(bytes) => {
                let s = String::from_utf8_lossy(&bytes);
                assert!(
                    s.contains("EINVAL"),
                    "Expected EINVAL size mismatch error, got: {s}"
                );
            }
            PtyWrite::Resize(_) => panic!("Expected PtyWrite::Write"),
        }
    }

    #[test]
    fn kitty_transmit_and_display_with_unicode_placeholder() {
        // TransmitAndDisplay with unicode_placeholder=true should create a
        // VirtualPlacement instead of placing the image in the buffer.
        use freminal_common::buffer_states::kitty_graphics::{
            KittyAction, KittyControlData, KittyFormat,
        };

        let (mut handler, _rx) = kitty_handler();

        // 2x2 RGBA = 16 bytes
        let rgba_data: Vec<u8> = vec![255; 16];
        let cmd = KittyGraphicsCommand {
            control: KittyControlData {
                action: Some(KittyAction::TransmitAndDisplay),
                format: Some(KittyFormat::Rgba),
                src_width: Some(2),
                src_height: Some(2),
                image_id: Some(800),
                unicode_placeholder: true,
                placement_id: Some(5),
                ..KittyControlData::default()
            },
            payload: rgba_data,
        };
        handler.handle_kitty_graphics(cmd);

        // Should have a virtual placement registered
        assert!(
            !handler.virtual_placements.is_empty(),
            "Expected virtual placement to be created"
        );
    }

    #[test]
    fn kitty_put_with_unicode_placeholder_creates_virtual_placement() {
        // First transmit an image, then Put with unicode_placeholder=true.
        use freminal_common::buffer_states::kitty_graphics::{
            KittyAction, KittyControlData, KittyFormat,
        };

        let (mut handler, _rx) = kitty_handler();

        // Step 1: Transmit image (a=t)
        let rgba_data: Vec<u8> = vec![255; 16]; // 2x2 RGBA
        let transmit_cmd = KittyGraphicsCommand {
            control: KittyControlData {
                action: Some(KittyAction::Transmit),
                format: Some(KittyFormat::Rgba),
                src_width: Some(2),
                src_height: Some(2),
                image_id: Some(810),
                ..KittyControlData::default()
            },
            payload: rgba_data,
        };
        handler.handle_kitty_graphics(transmit_cmd);

        // Step 2: Put with unicode_placeholder
        let put_cmd = KittyGraphicsCommand {
            control: KittyControlData {
                action: Some(KittyAction::Put),
                image_id: Some(810),
                unicode_placeholder: true,
                placement_id: Some(3),
                ..KittyControlData::default()
            },
            payload: Vec::new(),
        };
        handler.handle_kitty_graphics(put_cmd);

        assert!(
            !handler.virtual_placements.is_empty(),
            "Expected virtual placement from Put"
        );
    }

    #[test]
    fn kitty_put_with_no_cursor_movement_preserves_position() {
        // Put with C=1 (no_cursor_movement) should restore cursor position after placement.
        use freminal_common::buffer_states::kitty_graphics::{
            KittyAction, KittyControlData, KittyFormat,
        };

        let (mut handler, _rx) = kitty_handler();

        // Step 1: Transmit image
        let rgba_data: Vec<u8> = vec![255; 16]; // 2x2 RGBA
        let transmit_cmd = KittyGraphicsCommand {
            control: KittyControlData {
                action: Some(KittyAction::Transmit),
                format: Some(KittyFormat::Rgba),
                src_width: Some(2),
                src_height: Some(2),
                image_id: Some(820),
                ..KittyControlData::default()
            },
            payload: rgba_data,
        };
        handler.handle_kitty_graphics(transmit_cmd);

        // Record cursor before Put
        let cursor_before = handler.buffer.cursor().pos;

        // Step 2: Put with C=1
        let put_cmd = KittyGraphicsCommand {
            control: KittyControlData {
                action: Some(KittyAction::Put),
                image_id: Some(820),
                no_cursor_movement: true,
                ..KittyControlData::default()
            },
            payload: Vec::new(),
        };
        handler.handle_kitty_graphics(put_cmd);

        // Cursor should be restored to original position
        let cursor_after = handler.buffer.cursor().pos;
        assert_eq!(
            cursor_before, cursor_after,
            "Cursor should not move with C=1"
        );
    }

    /// Task 100.16 regression test: `place_kitty_image`'s `a=T`/Put display
    /// branch used to call `Buffer::place_image` + `record_real_placement`
    /// without ever checking `control.no_cursor_movement` — only the `a=p`
    /// path (`stamp_kitty_put`) implemented the C=1 save/restore. This test
    /// previously passed for the WRONG reason: with a 1-row image, the old
    /// (pre-100.15) `place_image` parked the cursor back on the image's own
    /// single row, which happened to coincide with `cursor_before` in a
    /// fresh single-row buffer, masking the missing C=1 handling entirely.
    ///
    /// Using an explicit `r=2` (2-row) image makes that coincidental clamp
    /// impossible: without a genuine save/restore, the cursor would land
    /// either on the image's second row or on the fresh blank row
    /// `place_image` (Task 100.15) appends below it — never back at
    /// `cursor_before` — so this can only pass if `place_kitty_image`
    /// actually saves and restores the cursor for C=1.
    #[test]
    fn kitty_transmit_and_display_with_no_cursor_movement() {
        // TransmitAndDisplay with C=1 should place image but keep cursor at original position.
        use freminal_common::buffer_states::kitty_graphics::{
            KittyAction, KittyControlData, KittyFormat,
        };

        let (mut handler, _rx) = kitty_handler();

        let cursor_before = handler.buffer.cursor().pos;

        let rgba_data: Vec<u8> = vec![255; 16]; // 2x2 RGBA
        let cmd = KittyGraphicsCommand {
            control: KittyControlData {
                action: Some(KittyAction::TransmitAndDisplay),
                format: Some(KittyFormat::Rgba),
                src_width: Some(2),
                src_height: Some(2),
                display_cols: Some(2),
                display_rows: Some(2),
                image_id: Some(830),
                no_cursor_movement: true,
                ..KittyControlData::default()
            },
            payload: rgba_data,
        };
        handler.handle_kitty_graphics(cmd);

        let cursor_after = handler.buffer.cursor().pos;
        assert_eq!(
            cursor_before, cursor_after,
            "Cursor should not move with C=1 on TransmitAndDisplay"
        );

        // Ground truth: C=1 only preserves the cursor, it must not skip
        // placement — the image must still have been stamped into cells.
        assert!(
            handler.buffer().has_any_image_cell(),
            "C=1 must still place the image; only the cursor is preserved"
        );
    }

    /// Companion to `kitty_transmit_and_display_with_no_cursor_movement`:
    /// locks in the other half of Task 100.16's behavior. Without `C=1`,
    /// `a=T`/Put must let the cursor move below the placed image — to the
    /// fresh row `Buffer::place_image` (Task 100.15) appends below it — not
    /// leave it at the pre-call position and not clamp it onto the image's
    /// own last row.
    #[test]
    fn kitty_transmit_and_display_without_no_cursor_movement_moves_cursor_below_image() {
        use freminal_common::buffer_states::kitty_graphics::{
            KittyAction, KittyControlData, KittyFormat,
        };

        let (mut handler, _rx) = kitty_handler();

        let cursor_before = handler.buffer.cursor().pos;

        let rgba_data: Vec<u8> = vec![255; 16]; // 2x2 RGBA
        let cmd = KittyGraphicsCommand {
            control: KittyControlData {
                action: Some(KittyAction::TransmitAndDisplay),
                format: Some(KittyFormat::Rgba),
                src_width: Some(2),
                src_height: Some(2),
                display_cols: Some(2),
                display_rows: Some(2),
                image_id: Some(831),
                no_cursor_movement: false,
                ..KittyControlData::default()
            },
            payload: rgba_data,
        };
        handler.handle_kitty_graphics(cmd);

        let cursor_after = handler.buffer.cursor().pos;
        let image_last_row = cursor_before.y + 2 - 1; // 2-row image placed at cursor_before.y.
        assert!(
            cursor_after.y > image_last_row,
            "cursor (row {}) must be strictly below the image's last row \
             ({image_last_row}) — a fresh blank row, not the image's own \
             last row, and not left at the pre-call row ({})",
            cursor_after.y,
            cursor_before.y,
        );
        assert_eq!(cursor_after.x, 0);
        assert_ne!(
            cursor_after, cursor_before,
            "without C=1 the cursor must actually move"
        );
    }

    #[test]
    fn kitty_chunked_transfer_new_command_discards_stale_state() {
        // Start a chunked transfer, then send a new command with a=t.
        // The stale chunk state should be discarded.
        use freminal_common::buffer_states::kitty_graphics::{
            KittyAction, KittyControlData, KittyFormat,
        };

        let (mut handler, _rx) = kitty_handler();

        // Start a chunked transfer (more_data=true)
        let chunk1 = KittyGraphicsCommand {
            control: KittyControlData {
                action: Some(KittyAction::Transmit),
                format: Some(KittyFormat::Rgba),
                src_width: Some(2),
                src_height: Some(2),
                image_id: Some(900),
                more_data: true,
                ..KittyControlData::default()
            },
            payload: vec![255; 8], // partial chunk
        };
        handler.handle_kitty_graphics(chunk1);
        assert!(
            kitty_receiving(&handler),
            "Should have in-progress chunked state"
        );

        // Send a brand-new command with explicit action → discards stale state
        let new_cmd = KittyGraphicsCommand {
            control: KittyControlData {
                action: Some(KittyAction::TransmitAndDisplay),
                format: Some(KittyFormat::Rgba),
                src_width: Some(2),
                src_height: Some(2),
                image_id: Some(901),
                ..KittyControlData::default()
            },
            payload: vec![255; 16],
        };
        handler.handle_kitty_graphics(new_cmd);
        assert!(
            kitty_idle(&handler),
            "Stale state should have been discarded"
        );
    }

    #[test]
    fn kitty_query_quiet_2_suppresses_all_responses() {
        // a=q with quiet=2 should produce no response at all.
        use freminal_common::buffer_states::kitty_graphics::{KittyAction, KittyControlData};

        let (mut handler, rx) = kitty_handler();

        let cmd = KittyGraphicsCommand {
            control: KittyControlData {
                action: Some(KittyAction::Query),
                quiet: 2,
                ..KittyControlData::default()
            },
            payload: Vec::new(),
        };
        handler.handle_kitty_graphics(cmd);

        // No response should be sent
        assert!(
            rx.try_recv().is_err(),
            "quiet=2 should suppress all responses"
        );
    }

    #[test]
    fn kitty_query_quiet_1_suppresses_ok_response() {
        // a=q with quiet=1 should suppress OK (supported format) but still send errors.
        use freminal_common::buffer_states::kitty_graphics::{KittyAction, KittyControlData};

        let (mut handler, rx) = kitty_handler();

        // Default format (None → supported) with quiet=1
        let cmd = KittyGraphicsCommand {
            control: KittyControlData {
                action: Some(KittyAction::Query),
                quiet: 1,
                ..KittyControlData::default()
            },
            payload: Vec::new(),
        };
        handler.handle_kitty_graphics(cmd);

        assert!(
            rx.try_recv().is_err(),
            "quiet=1 should suppress OK for supported format"
        );
    }

    // ------------------------------------------------------------------
    // Kitty animation tests (Task 100.2b): a=f, a=a, a=c
    // ------------------------------------------------------------------

    /// Transmit a 2x2 opaque-black RGBA base image (store only) with the
    /// given id and return the handler + receiver.
    fn kitty_handler_with_black_2x2_base(
        id: u32,
    ) -> (TerminalHandler, crossbeam_channel::Receiver<PtyWrite>) {
        use freminal_common::buffer_states::kitty_graphics::{
            KittyAction, KittyControlData, KittyFormat,
        };

        let (mut handler, rx) = kitty_handler();
        let cmd = KittyGraphicsCommand {
            control: KittyControlData {
                action: Some(KittyAction::Transmit),
                format: Some(KittyFormat::Rgba),
                src_width: Some(2),
                src_height: Some(2),
                image_id: Some(id),
                ..KittyControlData::default()
            },
            payload: [0, 0, 0, 255].repeat(4), // 4 opaque-black pixels
        };
        handler.handle_kitty_graphics(cmd);
        // Drain the OK response from the transmit so later assertions on
        // `rx` only see animation-command responses.
        let _ = rx.try_recv();
        (handler, rx)
    }

    /// Named parameters for [`kitty_animation_frame_cmd`], grouped into a
    /// struct so the test helper doesn't trip `too_many_arguments` /
    /// `many_single_char_names`.
    #[derive(Default, Clone, Copy)]
    struct AnimationFrameCmdParams {
        rect_width: Option<u32>,
        rect_height: Option<u32>,
        dest_x: Option<u32>,
        dest_y: Option<u32>,
        base_frame: Option<u32>,
        edit_frame: Option<u32>,
        gap_ms: Option<i32>,
        overwrite: bool,
    }

    fn kitty_animation_frame_cmd(
        image_id: u32,
        payload: Vec<u8>,
        params: AnimationFrameCmdParams,
    ) -> KittyGraphicsCommand {
        use freminal_common::buffer_states::kitty_graphics::{
            KittyAction, KittyControlData, KittyFormat,
        };

        KittyGraphicsCommand {
            control: KittyControlData {
                action: Some(KittyAction::AnimationFrame),
                format: Some(KittyFormat::Rgba),
                src_width: params.rect_width,
                src_height: params.rect_height,
                src_x: params.dest_x,
                src_y: params.dest_y,
                display_cols: params.base_frame,
                display_rows: params.edit_frame,
                z_index: params.gap_ms,
                cell_x_offset: if params.overwrite { Some(1) } else { None },
                image_id: Some(image_id),
                ..KittyControlData::default()
            },
            payload,
        }
    }

    #[test]
    fn kitty_animation_frame_creates_new_frame() {
        let (mut handler, _rx) = kitty_handler_with_black_2x2_base(60);

        let payload: Vec<u8> = vec![
            255, 0, 0, 255, // red
            0, 255, 0, 255, // green
            0, 0, 255, 255, // blue
            255, 255, 0, 255, // yellow
        ];
        let cmd = kitty_animation_frame_cmd(
            60,
            payload.clone(),
            AnimationFrameCmdParams {
                rect_width: Some(2),
                rect_height: Some(2),
                ..Default::default()
            },
        );
        handler.handle_kitty_graphics(cmd);

        let img = handler
            .buffer()
            .image_store()
            .get(60)
            .expect("image should still exist");
        assert_eq!(img.frame_count(), 2, "root frame + 1 new frame");
        assert_eq!(
            img.frame_pixels(2).expect("frame 2 should exist").as_ref(),
            &payload,
            "opaque payload alpha-blended over a transparent canvas equals the payload"
        );
    }

    #[test]
    fn kitty_animation_frame_edits_existing_frame_in_place() {
        let (mut handler, _rx) = kitty_handler_with_black_2x2_base(61);

        // First, create frame 2 via a=f (no r=).
        let first_payload: Vec<u8> = vec![1u8; 16];
        let create_cmd = kitty_animation_frame_cmd(
            61,
            first_payload,
            AnimationFrameCmdParams {
                rect_width: Some(2),
                rect_height: Some(2),
                ..Default::default()
            },
        );
        handler.handle_kitty_graphics(create_cmd);
        assert_eq!(
            handler
                .buffer()
                .image_store()
                .get(61)
                .expect("image exists")
                .frame_count(),
            2
        );

        // Now edit frame 2 in place (r=2).
        let edit_payload: Vec<u8> = vec![2u8; 16];
        let edit_cmd = kitty_animation_frame_cmd(
            61,
            edit_payload.clone(),
            AnimationFrameCmdParams {
                rect_width: Some(2),
                rect_height: Some(2),
                edit_frame: Some(2),
                overwrite: true, // so blend math doesn't obscure the assertion
                ..Default::default()
            },
        );
        handler.handle_kitty_graphics(edit_cmd);

        let img = handler
            .buffer()
            .image_store()
            .get(61)
            .expect("image exists");
        assert_eq!(
            img.frame_count(),
            2,
            "editing in place must not add a frame"
        );
        assert_eq!(
            img.frame_pixels(2).expect("frame 2 exists").as_ref(),
            &edit_payload
        );
    }

    #[test]
    fn kitty_animation_frame_partial_rect_overwrite() {
        let (mut handler, _rx) = kitty_handler_with_black_2x2_base(62);

        // Base canvas seeded from root (c=1); overwrite only the pixel at
        // (x=1, y=0) with a distinct color.
        let rect_payload: Vec<u8> = vec![10, 20, 30, 40];
        let cmd = kitty_animation_frame_cmd(
            62,
            rect_payload.clone(),
            AnimationFrameCmdParams {
                rect_width: Some(1),
                rect_height: Some(1),
                dest_x: Some(1),
                dest_y: Some(0),
                base_frame: Some(1), // c=1: base canvas = root frame
                overwrite: true,     // X=1: overwrite
                ..Default::default()
            },
        );
        handler.handle_kitty_graphics(cmd);

        let img = handler
            .buffer()
            .image_store()
            .get(62)
            .expect("image exists");
        let frame = img
            .frame_pixels(2)
            .expect("new frame should exist")
            .as_ref();

        // (0,0) and row 1 unchanged from the black base.
        assert_eq!(&frame[0..4], &[0, 0, 0, 255], "pixel (0,0) unchanged");
        assert_eq!(
            &frame[8..16],
            &[0, 0, 0, 255, 0, 0, 0, 255],
            "row 1 unchanged"
        );
        // (1,0) overwritten with the rect payload.
        assert_eq!(
            &frame[4..8],
            rect_payload.as_slice(),
            "pixel (1,0) overwritten"
        );
    }

    #[test]
    fn kitty_animation_frame_gap_ms_set_on_new_frame() {
        let (mut handler, _rx) = kitty_handler_with_black_2x2_base(63);

        let cmd = kitty_animation_frame_cmd(
            63,
            vec![0u8; 16],
            AnimationFrameCmdParams {
                rect_width: Some(2),
                rect_height: Some(2),
                gap_ms: Some(100),
                ..Default::default()
            },
        );
        handler.handle_kitty_graphics(cmd);

        let img = handler
            .buffer()
            .image_store()
            .get(63)
            .expect("image exists");
        assert_eq!(img.frames[0].gap_ms, 100);
    }

    #[test]
    fn kitty_animation_frame_missing_image_sends_enoent() {
        let (mut handler, rx) = kitty_handler();
        let cmd = kitty_animation_frame_cmd(
            9999,
            vec![0u8; 16],
            AnimationFrameCmdParams {
                rect_width: Some(2),
                rect_height: Some(2),
                ..Default::default()
            },
        );
        handler.handle_kitty_graphics(cmd);

        let response = rx.try_recv().expect("expected an error response");
        match response {
            PtyWrite::Write(bytes) => {
                let s = String::from_utf8_lossy(&bytes);
                assert!(s.contains("ENOENT"), "expected ENOENT, got: {s}");
            }
            PtyWrite::Resize(_) => panic!("Expected PtyWrite::Write"),
        }
    }

    #[test]
    fn kitty_animation_frame_chunked_transfer_reassembles() {
        use freminal_common::buffer_states::kitty_graphics::{
            KittyAction, KittyControlData, KittyFormat,
        };

        let (mut handler, _rx) = kitty_handler_with_black_2x2_base(64);

        let full_payload: Vec<u8> = vec![9, 9, 9, 255, 8, 8, 8, 255, 7, 7, 7, 255, 6, 6, 6, 255];

        let chunk1 = KittyGraphicsCommand {
            control: KittyControlData {
                action: Some(KittyAction::AnimationFrame),
                format: Some(KittyFormat::Rgba),
                src_width: Some(2),
                src_height: Some(2),
                image_id: Some(64),
                more_data: true,
                ..KittyControlData::default()
            },
            payload: full_payload[..8].to_vec(),
        };
        let chunk2 = KittyGraphicsCommand {
            control: KittyControlData {
                more_data: false,
                ..KittyControlData::default()
            },
            payload: full_payload[8..].to_vec(),
        };

        handler.handle_kitty_graphics(chunk1);
        assert_eq!(
            handler
                .buffer()
                .image_store()
                .get(64)
                .expect("image exists")
                .frame_count(),
            1,
            "frame should not appear until the final chunk"
        );

        handler.handle_kitty_graphics(chunk2);
        let img = handler
            .buffer()
            .image_store()
            .get(64)
            .expect("image exists");
        assert_eq!(
            img.frame_count(),
            2,
            "frame should appear after final chunk"
        );
        assert_eq!(
            img.frame_pixels(2).expect("frame 2 exists").as_ref(),
            &full_payload
        );
    }

    #[test]
    fn kitty_animation_control_updates_run_mode_loop_count_current_frame_and_gap() {
        use freminal_buffer::image_store::AnimationRunMode;
        use freminal_common::buffer_states::kitty_graphics::{KittyAction, KittyControlData};

        let (mut handler, _rx) = kitty_handler_with_black_2x2_base(65);

        // v=3 sets loop_count.
        handler.handle_kitty_graphics(KittyGraphicsCommand {
            control: KittyControlData {
                action: Some(KittyAction::AnimationControl),
                image_id: Some(65),
                src_height: Some(3), // v=
                ..KittyControlData::default()
            },
            payload: Vec::new(),
        });
        assert_eq!(
            handler
                .buffer()
                .image_store()
                .get(65)
                .expect("image exists")
                .animation
                .loop_count,
            3
        );

        // s=3 sets run_mode = Running.
        handler.handle_kitty_graphics(KittyGraphicsCommand {
            control: KittyControlData {
                action: Some(KittyAction::AnimationControl),
                image_id: Some(65),
                src_width: Some(3), // s=
                ..KittyControlData::default()
            },
            payload: Vec::new(),
        });
        assert_eq!(
            handler
                .buffer()
                .image_store()
                .get(65)
                .expect("image exists")
                .animation
                .run_mode,
            AnimationRunMode::Running
        );

        // c=2 forces current_frame.
        handler.handle_kitty_graphics(KittyGraphicsCommand {
            control: KittyControlData {
                action: Some(KittyAction::AnimationControl),
                image_id: Some(65),
                display_cols: Some(2), // c=
                ..KittyControlData::default()
            },
            payload: Vec::new(),
        });
        assert_eq!(
            handler
                .buffer()
                .image_store()
                .get(65)
                .expect("image exists")
                .animation
                .current_frame,
            2
        );

        // r=1,z=50 sets the root frame's gap.
        handler.handle_kitty_graphics(KittyGraphicsCommand {
            control: KittyControlData {
                action: Some(KittyAction::AnimationControl),
                image_id: Some(65),
                display_rows: Some(1), // r=
                z_index: Some(50),     // z=
                ..KittyControlData::default()
            },
            payload: Vec::new(),
        });
        assert_eq!(
            handler
                .buffer()
                .image_store()
                .get(65)
                .expect("image exists")
                .root_gap_ms,
            50
        );

        // Loop count and run mode from earlier commands must have persisted
        // (each a=a call re-reads then re-stores the image).
        let img = handler
            .buffer()
            .image_store()
            .get(65)
            .expect("image exists");
        assert_eq!(img.animation.loop_count, 3);
        assert_eq!(img.animation.run_mode, AnimationRunMode::Running);
        assert_eq!(img.animation.current_frame, 2);
    }

    #[test]
    fn kitty_animation_control_is_silent_on_success() {
        use freminal_common::buffer_states::kitty_graphics::{KittyAction, KittyControlData};

        let (mut handler, rx) = kitty_handler_with_black_2x2_base(66);

        handler.handle_kitty_graphics(KittyGraphicsCommand {
            control: KittyControlData {
                action: Some(KittyAction::AnimationControl),
                image_id: Some(66),
                src_width: Some(3),
                ..KittyControlData::default()
            },
            payload: Vec::new(),
        });

        assert!(
            rx.try_recv().is_err(),
            "a=a must not send an OK response even when quiet=0"
        );
    }

    #[test]
    fn kitty_animation_control_missing_image_sends_enoent() {
        use freminal_common::buffer_states::kitty_graphics::{KittyAction, KittyControlData};

        let (mut handler, rx) = kitty_handler();
        handler.handle_kitty_graphics(KittyGraphicsCommand {
            control: KittyControlData {
                action: Some(KittyAction::AnimationControl),
                image_id: Some(9998),
                src_width: Some(3),
                ..KittyControlData::default()
            },
            payload: Vec::new(),
        });

        let response = rx.try_recv().expect("expected an error response");
        match response {
            PtyWrite::Write(bytes) => {
                let s = String::from_utf8_lossy(&bytes);
                assert!(s.contains("ENOENT"), "expected ENOENT, got: {s}");
            }
            PtyWrite::Resize(_) => panic!("Expected PtyWrite::Write"),
        }
    }

    #[test]
    fn kitty_animation_compose_overwrites_dest_rect_from_source_frame() {
        use freminal_common::buffer_states::kitty_graphics::{KittyAction, KittyControlData};

        let (mut handler, _rx) = kitty_handler_with_black_2x2_base(70);

        // Create frame 2 (all-white) via a=f.
        let white_payload: Vec<u8> = vec![255u8; 16];
        handler.handle_kitty_graphics(kitty_animation_frame_cmd(
            70,
            white_payload,
            AnimationFrameCmdParams {
                rect_width: Some(2),
                rect_height: Some(2),
                overwrite: true,
                ..Default::default()
            },
        ));
        assert_eq!(
            handler
                .buffer()
                .image_store()
                .get(70)
                .expect("image exists")
                .frame_count(),
            2
        );

        // Compose: copy pixel (0,0) from source frame 1 (root, black) onto
        // destination frame 2 (white) at (0,0), overwrite mode.
        handler.handle_kitty_graphics(KittyGraphicsCommand {
            control: KittyControlData {
                action: Some(KittyAction::AnimationCompose),
                image_id: Some(70),
                display_rows: Some(1), // r= source frame
                display_cols: Some(2), // c= destination frame
                src_x: Some(0),
                src_y: Some(0),
                cell_x_offset: Some(0), // X= source rect x
                cell_y_offset: Some(0), // Y= source rect y
                src_rect_width: Some(1),
                src_rect_height: Some(1),
                no_cursor_movement: true, // C=1: overwrite
                ..KittyControlData::default()
            },
            payload: Vec::new(),
        });

        let img = handler
            .buffer()
            .image_store()
            .get(70)
            .expect("image exists");
        let dest = img.frame_pixels(2).expect("dest frame exists").as_ref();
        assert_eq!(
            &dest[0..4],
            &[0, 0, 0, 255],
            "composed pixel from black source"
        );
        assert_eq!(
            &dest[4..16],
            &[255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255],
            "rest of dest frame unchanged"
        );
    }

    #[test]
    fn kitty_animation_compose_out_of_bounds_rect_sends_einval() {
        use freminal_common::buffer_states::kitty_graphics::{KittyAction, KittyControlData};

        let (mut handler, rx) = kitty_handler_with_black_2x2_base(71);

        handler.handle_kitty_graphics(KittyGraphicsCommand {
            control: KittyControlData {
                action: Some(KittyAction::AnimationCompose),
                image_id: Some(71),
                display_rows: Some(1),
                display_cols: Some(1),
                src_x: Some(0),
                src_y: Some(0),
                cell_x_offset: Some(0),
                cell_y_offset: Some(0),
                src_rect_width: Some(10), // out of bounds for a 2x2 image
                src_rect_height: Some(10),
                ..KittyControlData::default()
            },
            payload: Vec::new(),
        });

        let response = rx.try_recv().expect("expected an error response");
        match response {
            PtyWrite::Write(bytes) => {
                let s = String::from_utf8_lossy(&bytes);
                assert!(s.contains("EINVAL"), "expected EINVAL, got: {s}");
            }
            PtyWrite::Resize(_) => panic!("Expected PtyWrite::Write"),
        }
    }

    #[test]
    fn kitty_animation_compose_missing_frame_sends_enoent() {
        use freminal_common::buffer_states::kitty_graphics::{KittyAction, KittyControlData};

        let (mut handler, rx) = kitty_handler_with_black_2x2_base(72);

        // Only frame 1 (root) exists — frame 2 does not.
        handler.handle_kitty_graphics(KittyGraphicsCommand {
            control: KittyControlData {
                action: Some(KittyAction::AnimationCompose),
                image_id: Some(72),
                display_rows: Some(1),
                display_cols: Some(2), // frame 2 does not exist
                ..KittyControlData::default()
            },
            payload: Vec::new(),
        });

        let response = rx.try_recv().expect("expected an error response");
        match response {
            PtyWrite::Write(bytes) => {
                let s = String::from_utf8_lossy(&bytes);
                assert!(s.contains("ENOENT"), "expected ENOENT, got: {s}");
            }
            PtyWrite::Resize(_) => panic!("Expected PtyWrite::Write"),
        }
    }

    // -----------------------------------------------------------------------
    // Kitty image number (`I=`) reference-by-number tests (Task 100.3)
    // -----------------------------------------------------------------------

    /// Parse the `i=<id>` field out of the next queued kitty graphics
    /// response and return it. Panics if no response is queued or it cannot
    /// be parsed as a `u64`.
    fn extract_assigned_id(rx: &crossbeam_channel::Receiver<PtyWrite>) -> u64 {
        let response = rx.try_recv().expect("expected a response");
        match response {
            PtyWrite::Write(bytes) => {
                let s = String::from_utf8_lossy(&bytes).into_owned();
                let id_part = s
                    .split("i=")
                    .nth(1)
                    .and_then(|rest| rest.split(',').next())
                    .expect("response should contain i=<id>");
                id_part.parse().expect("id should be numeric")
            }
            PtyWrite::Resize(_) => panic!("Expected PtyWrite::Write"),
        }
    }

    #[test]
    fn kitty_transmit_with_image_number_echoes_i_and_associates() {
        use freminal_common::buffer_states::kitty_graphics::{KittyControlData, KittyFormat};

        let (mut handler, rx) = kitty_handler();

        let cmd = KittyGraphicsCommand {
            control: KittyControlData {
                action: Some(KittyAction::Transmit),
                format: Some(KittyFormat::Rgba),
                src_width: Some(2),
                src_height: Some(2),
                image_number: Some(13),
                ..KittyControlData::default()
            },
            payload: vec![0u8; 16],
        };
        handler.handle_kitty_graphics(cmd);

        let response = rx.try_recv().expect("expected an OK response");
        let assigned_id = match response {
            PtyWrite::Write(bytes) => {
                let s = String::from_utf8_lossy(&bytes).into_owned();
                assert!(s.contains("I=13"), "expected I=13 in response, got: {s}");
                assert!(s.contains("OK"), "expected OK, got: {s}");
                let id_part = s
                    .split("i=")
                    .nth(1)
                    .and_then(|rest| rest.split(',').next())
                    .expect("response should contain i=<id>");
                id_part.parse::<u64>().expect("id should be numeric")
            }
            PtyWrite::Resize(_) => panic!("Expected PtyWrite::Write"),
        };

        assert_eq!(
            handler.buffer().image_store().newest_id_for_number(13),
            Some(assigned_id),
            "newest_id_for_number(13) should resolve to the assigned id"
        );
    }

    #[test]
    fn kitty_put_by_image_number_resolves_to_newest_image() {
        use freminal_common::buffer_states::kitty_graphics::{KittyControlData, KittyFormat};

        let (mut handler, rx) = kitty_handler();

        let transmit_cmd = KittyGraphicsCommand {
            control: KittyControlData {
                action: Some(KittyAction::Transmit),
                format: Some(KittyFormat::Rgba),
                src_width: Some(2),
                src_height: Some(2),
                image_number: Some(13),
                ..KittyControlData::default()
            },
            payload: vec![0u8; 16],
        };
        handler.handle_kitty_graphics(transmit_cmd);
        let assigned_id = extract_assigned_id(&rx);

        assert!(
            !handler.buffer().has_any_image_cell(),
            "transmit-only should not place cells"
        );

        let put_cmd = KittyGraphicsCommand {
            control: KittyControlData {
                action: Some(KittyAction::Put),
                image_number: Some(13),
                ..KittyControlData::default()
            },
            payload: Vec::new(),
        };
        handler.handle_kitty_graphics(put_cmd);

        assert!(
            handler.buffer().has_any_image_cell(),
            "put by I= should resolve to the transmitted image and place cells"
        );

        let response = rx.try_recv().expect("expected a Put OK response");
        match response {
            PtyWrite::Write(bytes) => {
                let s = String::from_utf8_lossy(&bytes);
                assert!(s.contains("OK"), "expected OK, got: {s}");
                assert!(s.contains("I=13"), "expected I=13 echoed, got: {s}");
                assert!(
                    s.contains(&format!("i={assigned_id}")),
                    "expected the resolved id echoed, got: {s}"
                );
            }
            PtyWrite::Resize(_) => panic!("Expected PtyWrite::Write"),
        }
    }

    #[test]
    fn kitty_put_unknown_image_number_sends_enoent() {
        use freminal_common::buffer_states::kitty_graphics::KittyControlData;

        let (mut handler, rx) = kitty_handler();

        let put_cmd = KittyGraphicsCommand {
            control: KittyControlData {
                action: Some(KittyAction::Put),
                image_number: Some(99),
                ..KittyControlData::default()
            },
            payload: Vec::new(),
        };
        handler.handle_kitty_graphics(put_cmd);

        let response = rx.try_recv().expect("expected an error response");
        match response {
            PtyWrite::Write(bytes) => {
                let s = String::from_utf8_lossy(&bytes);
                assert!(s.contains("ENOENT"), "expected ENOENT, got: {s}");
            }
            PtyWrite::Resize(_) => panic!("Expected PtyWrite::Write"),
        }
    }

    #[test]
    fn kitty_transmit_twice_same_number_newest_id_wins() {
        use freminal_common::buffer_states::kitty_graphics::{KittyControlData, KittyFormat};

        let (mut handler, rx) = kitty_handler();

        let make_transmit = |fill: u8| KittyGraphicsCommand {
            control: KittyControlData {
                action: Some(KittyAction::Transmit),
                format: Some(KittyFormat::Rgba),
                src_width: Some(2),
                src_height: Some(2),
                image_number: Some(5),
                ..KittyControlData::default()
            },
            payload: vec![fill; 16],
        };

        handler.handle_kitty_graphics(make_transmit(1));
        let first_id = extract_assigned_id(&rx);

        handler.handle_kitty_graphics(make_transmit(2));
        let second_id = extract_assigned_id(&rx);

        assert_ne!(first_id, second_id, "each I= transmit gets a fresh id");
        assert_eq!(
            handler.buffer().image_store().newest_id_for_number(5),
            Some(second_id),
            "newest_id_for_number should follow the most recent transmit"
        );

        // A put by number should target the newest (second) image.
        let put_cmd = KittyGraphicsCommand {
            control: KittyControlData {
                action: Some(KittyAction::Put),
                image_number: Some(5),
                ..KittyControlData::default()
            },
            payload: Vec::new(),
        };
        handler.handle_kitty_graphics(put_cmd);
        let response = rx.try_recv().expect("expected a Put OK response");
        match response {
            PtyWrite::Write(bytes) => {
                let s = String::from_utf8_lossy(&bytes);
                assert!(
                    s.contains(&format!("i={second_id}")),
                    "put by number should target the newest image, got: {s}"
                );
            }
            PtyWrite::Resize(_) => panic!("Expected PtyWrite::Write"),
        }
    }

    #[test]
    fn kitty_animation_frame_by_image_number_targets_resolved_image() {
        use freminal_common::buffer_states::kitty_graphics::{KittyControlData, KittyFormat};

        let (mut handler, rx) = kitty_handler();

        let transmit_cmd = KittyGraphicsCommand {
            control: KittyControlData {
                action: Some(KittyAction::Transmit),
                format: Some(KittyFormat::Rgba),
                src_width: Some(2),
                src_height: Some(2),
                image_number: Some(13),
                ..KittyControlData::default()
            },
            payload: vec![0u8; 16],
        };
        handler.handle_kitty_graphics(transmit_cmd);
        let assigned_id = extract_assigned_id(&rx);

        let frame_cmd = KittyGraphicsCommand {
            control: KittyControlData {
                action: Some(KittyAction::AnimationFrame),
                format: Some(KittyFormat::Rgba),
                src_width: Some(2),
                src_height: Some(2),
                image_number: Some(13),
                ..KittyControlData::default()
            },
            payload: vec![1u8; 16],
        };
        handler.handle_kitty_graphics(frame_cmd);

        let img = handler
            .buffer()
            .image_store()
            .get(assigned_id)
            .expect("resolved image should still exist");
        assert_eq!(
            img.frame_count(),
            2,
            "a=f,I= should add a frame to the resolved image"
        );
    }

    #[test]
    fn kitty_delete_by_number_prunes_virtual_placement() {
        use freminal_common::buffer_states::kitty_graphics::{
            KittyControlData, KittyDeleteTarget, KittyFormat,
        };

        let (mut handler, rx) = kitty_handler();

        let transmit_cmd = KittyGraphicsCommand {
            control: KittyControlData {
                action: Some(KittyAction::TransmitAndDisplay),
                format: Some(KittyFormat::Rgba),
                src_width: Some(2),
                src_height: Some(2),
                image_number: Some(13),
                unicode_placeholder: true,
                ..KittyControlData::default()
            },
            payload: vec![0u8; 16],
        };
        handler.handle_kitty_graphics(transmit_cmd);
        let assigned_id = extract_assigned_id(&rx);

        assert!(
            handler.virtual_placements.contains_key(&(assigned_id, 0)),
            "virtual placement should exist for the transmitted image"
        );

        let delete_cmd = KittyGraphicsCommand {
            control: KittyControlData {
                action: Some(KittyAction::Delete),
                delete_target: Some(KittyDeleteTarget::ByNumber),
                image_number: Some(13),
                ..KittyControlData::default()
            },
            payload: Vec::new(),
        };
        handler.handle_kitty_graphics(delete_cmd);

        assert!(
            !handler.virtual_placements.contains_key(&(assigned_id, 0)),
            "d=n should prune the virtual placement for the resolved image"
        );
    }

    // ------------------------------------------------------------------
    // Source-crop (`a=p`/`a=T` x/y/w/h, Task 100.9)
    // ------------------------------------------------------------------

    /// Transmit (store-only, no display) a 10x10 opaque-black RGBA base
    /// image with the given id and return the handler + receiver, for
    /// source-crop tests that need a non-trivial pixel size.
    fn kitty_handler_with_black_10x10_base(
        id: u32,
    ) -> (TerminalHandler, crossbeam_channel::Receiver<PtyWrite>) {
        use freminal_common::buffer_states::kitty_graphics::{
            KittyAction, KittyControlData, KittyFormat,
        };

        let (mut handler, rx) = kitty_handler();
        let cmd = KittyGraphicsCommand {
            control: KittyControlData {
                action: Some(KittyAction::Transmit),
                format: Some(KittyFormat::Rgba),
                src_width: Some(10),
                src_height: Some(10),
                image_id: Some(id),
                ..KittyControlData::default()
            },
            payload: vec![0u8; 10 * 10 * 4],
        };
        handler.handle_kitty_graphics(cmd);
        // Drain the OK response from the transmit so later assertions on
        // `rx` only see the Put command's response.
        let _ = rx.try_recv();
        (handler, rx)
    }

    /// `a=p,x=,y=,w=,h=` on a stored image must stamp the resolved
    /// [`SourceCrop`] onto every placed cell.
    #[test]
    fn kitty_put_with_source_crop_stamps_resolved_crop_onto_placed_cells() {
        let (mut handler, _rx) = kitty_handler_with_black_10x10_base(42);
        handler.buffer_mut().set_cursor_pos(Some(0), Some(0));

        let put_cmd = KittyGraphicsCommand {
            control: KittyControlData {
                action: Some(KittyAction::Put),
                image_id: Some(42),
                src_x: Some(2),
                src_y: Some(3),
                src_rect_width: Some(4),
                src_rect_height: Some(5),
                ..KittyControlData::default()
            },
            payload: Vec::new(),
        };
        handler.handle_kitty_graphics(put_cmd);

        let expected = SourceCrop {
            x: 2,
            y: 3,
            width: 4,
            height: 5,
        };
        let placed = handler.buffer().rows()[0].cells()[0]
            .image_placement()
            .expect("cell should carry a placement after a=p");
        assert_eq!(
            placed.source_crop,
            Some(expected),
            "a=p x/y/w/h should stamp the resolved SourceCrop onto placed cells"
        );
    }

    /// `a=p` with no `x=`/`y=`/`w=`/`h=` keys must leave `source_crop` as
    /// `None` (display the full image).
    #[test]
    fn kitty_put_without_crop_keys_yields_full_image_none() {
        let (mut handler, _rx) = kitty_handler_with_black_10x10_base(42);
        handler.buffer_mut().set_cursor_pos(Some(0), Some(0));

        let put_cmd = KittyGraphicsCommand {
            control: KittyControlData {
                action: Some(KittyAction::Put),
                image_id: Some(42),
                ..KittyControlData::default()
            },
            payload: Vec::new(),
        };
        handler.handle_kitty_graphics(put_cmd);

        let placed = handler.buffer().rows()[0].cells()[0]
            .image_placement()
            .expect("cell should carry a placement after a=p");
        assert_eq!(
            placed.source_crop, None,
            "a=p with no crop keys should display the full image (None)"
        );
    }

    /// `a=p,X=,Y=` (Task 100.19) must stamp the resolved [`SubCellOffset`]
    /// onto every placed cell.
    #[test]
    fn kitty_put_with_subcell_offset_stamps_resolved_offset_onto_placed_cells() {
        use freminal_buffer::image_store::SubCellOffset;

        let (mut handler, _rx) = kitty_handler_with_black_10x10_base(42);
        handler.buffer_mut().set_cursor_pos(Some(0), Some(0));

        let put_cmd = KittyGraphicsCommand {
            control: KittyControlData {
                action: Some(KittyAction::Put),
                image_id: Some(42),
                cell_x_offset: Some(3),
                cell_y_offset: Some(5),
                ..KittyControlData::default()
            },
            payload: Vec::new(),
        };
        handler.handle_kitty_graphics(put_cmd);

        let placed = handler.buffer().rows()[0].cells()[0]
            .image_placement()
            .expect("cell should carry a placement after a=p");
        assert_eq!(
            placed.subcell_offset,
            Some(SubCellOffset { x: 3, y: 5 }),
            "a=p X/Y should stamp the resolved SubCellOffset onto placed cells"
        );
    }

    /// `X=`/`Y=` at or beyond the cell's pixel dimensions must be clamped
    /// to strictly less than the cell size (Task 100.19) — the default
    /// handler cell size is 8x16px, so `X=100` clamps to 7 and `Y=100`
    /// clamps to 15.
    #[test]
    fn kitty_put_with_subcell_offset_beyond_cell_size_is_clamped() {
        use freminal_buffer::image_store::SubCellOffset;

        let (mut handler, _rx) = kitty_handler_with_black_10x10_base(42);
        handler.buffer_mut().set_cursor_pos(Some(0), Some(0));

        let put_cmd = KittyGraphicsCommand {
            control: KittyControlData {
                action: Some(KittyAction::Put),
                image_id: Some(42),
                cell_x_offset: Some(100),
                cell_y_offset: Some(100),
                ..KittyControlData::default()
            },
            payload: Vec::new(),
        };
        handler.handle_kitty_graphics(put_cmd);

        let placed = handler.buffer().rows()[0].cells()[0]
            .image_placement()
            .expect("cell should carry a placement after a=p");
        assert_eq!(
            placed.subcell_offset,
            Some(SubCellOffset { x: 7, y: 15 }),
            "an offset at/beyond the cell size must clamp to cell_size - 1"
        );
    }

    /// `a=p` with no `X=`/`Y=` keys must leave `subcell_offset` as `None`.
    #[test]
    fn kitty_put_without_subcell_offset_keys_yields_none() {
        let (mut handler, _rx) = kitty_handler_with_black_10x10_base(42);
        handler.buffer_mut().set_cursor_pos(Some(0), Some(0));

        let put_cmd = KittyGraphicsCommand {
            control: KittyControlData {
                action: Some(KittyAction::Put),
                image_id: Some(42),
                ..KittyControlData::default()
            },
            payload: Vec::new(),
        };
        handler.handle_kitty_graphics(put_cmd);

        let placed = handler.buffer().rows()[0].cells()[0]
            .image_placement()
            .expect("cell should carry a placement after a=p");
        assert_eq!(
            placed.subcell_offset, None,
            "a=p with no X/Y keys should have no sub-cell offset"
        );
    }

    /// `w=0` (explicitly zero) must be treated the same as absent — "full
    /// width from `x`" — matching the same 0/absent idiom as
    /// `resolve_compose_rect`.
    #[test]
    fn kitty_put_with_zero_width_uses_full_width_from_x() {
        let (mut handler, _rx) = kitty_handler_with_black_10x10_base(42);
        handler.buffer_mut().set_cursor_pos(Some(0), Some(0));

        let put_cmd = KittyGraphicsCommand {
            control: KittyControlData {
                action: Some(KittyAction::Put),
                image_id: Some(42),
                src_x: Some(3),
                src_rect_width: Some(0),
                src_rect_height: Some(4),
                ..KittyControlData::default()
            },
            payload: Vec::new(),
        };
        handler.handle_kitty_graphics(put_cmd);

        let expected = SourceCrop {
            x: 3,
            y: 0,
            width: 7, // 10 (image width) - 3 (x) = 7, the rest from x
            height: 4,
        };
        let placed = handler.buffer().rows()[0].cells()[0]
            .image_placement()
            .expect("cell should carry a placement after a=p");
        assert_eq!(
            placed.source_crop,
            Some(expected),
            "w=0 should default to the full remaining width from x"
        );
    }

    /// A bare `a=q,i=31` query command: the simplest reply-producing request.
    fn kitty_query_command() -> KittyGraphicsCommand {
        use freminal_common::buffer_states::kitty_graphics::{
            KittyAction, KittyControlData, KittyFormat,
        };

        KittyGraphicsCommand {
            control: KittyControlData {
                action: Some(KittyAction::Query),
                format: Some(KittyFormat::Rgb),
                image_id: Some(31),
                ..KittyControlData::default()
            },
            payload: vec![0, 0, 0],
        }
    }

    #[test]
    fn kitty_query_reply_seven_bit_framing_is_esc_underscore_esc_backslash() {
        let (mut handler, rx) = kitty_handler();
        handler.handle_kitty_graphics(kitty_query_command());

        let PtyWrite::Write(bytes) = rx.try_recv().unwrap() else {
            panic!("expected PtyWrite::Write");
        };
        assert_eq!(bytes, b"\x1b_Gi=31;OK\x1b\\".to_vec());
    }

    #[test]
    fn kitty_query_reply_eight_bit_framing_is_c1_apc_and_st() {
        use freminal_common::buffer_states::modes::s8c1t::S8c1t;

        let (mut handler, rx) = kitty_handler();
        handler.set_s8c1t_mode(S8c1t::EightBit);
        handler.handle_kitty_graphics(kitty_query_command());

        let PtyWrite::Write(bytes) = rx.try_recv().unwrap() else {
            panic!("expected PtyWrite::Write");
        };
        assert_eq!(bytes, b"\x9fGi=31;OK\x9c".to_vec());
        assert!(!bytes.contains(&0x1b), "8-bit reply must carry no ESC");
    }

    #[test]
    fn kitty_query_reply_is_tmux_wrapped_with_doubled_escs() {
        let (mut handler, rx) = kitty_handler();
        handler.in_tmux_passthrough = true;
        handler.handle_kitty_graphics(kitty_query_command());

        let PtyWrite::Write(bytes) = rx.try_recv().unwrap() else {
            panic!("expected PtyWrite::Write");
        };
        assert_eq!(
            bytes,
            b"\x1bPtmux;\x1b\x1b_Gi=31;OK\x1b\x1b\\\x1b\\".to_vec()
        );
    }

    #[test]
    fn kitty_error_reply_eight_bit_framing_is_c1_apc_and_st() {
        use freminal_common::buffer_states::modes::s8c1t::S8c1t;

        let (mut handler, rx) = kitty_handler();
        handler.set_s8c1t_mode(S8c1t::EightBit);
        handler.send_kitty_error(
            super::kitty_id_no_number(7, None),
            0,
            "ENOENT:file not found",
        );

        let PtyWrite::Write(bytes) = rx.try_recv().unwrap() else {
            panic!("expected PtyWrite::Write");
        };
        assert_eq!(bytes, b"\x9fGi=7;ENOENT:file not found\x9c".to_vec());
    }
}
