//! Root-confined, no-follow filesystem primitives for security-sensitive state.
//!
//! These helpers are synchronous because callers hold an advisory lock across
//! an entire read-modify-write transaction. Unix uses directory file
//! descriptors and `*at` syscalls. Windows uses handle-relative NT file APIs.

use fs2::FileExt;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Component, Path, PathBuf};
use std::time::SystemTime;

use crate::{FlockGuard, FsError};

fn read_opened_to_string_limited(
    file: std::fs::File,
    relative: &Path,
    max_bytes: u64,
) -> Result<String, FsError> {
    let mut bytes = Vec::new();
    file.take(max_bytes.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|error| map_io(relative, error))?;
    if bytes.len() as u64 > max_bytes {
        return Err(FsError::TooLarge {
            actual: bytes.len() as u64,
            limit: max_bytes,
        });
    }
    String::from_utf8(bytes).map_err(|_| FsError::BinaryFile(relative.display().to_string()))
}

/// Owner-only defaults used for `.lingxi` state.
pub const PRIVATE_DIR_MODE: u32 = 0o700;
/// Owner-only defaults used for `.lingxi` state files.
pub const PRIVATE_FILE_MODE: u32 = 0o600;

/// Stable identity of an opened root directory. Values are derived from the
/// directory handle itself (device/inode on Unix, volume/file id on Windows),
/// never from a canonical pathname, so replacing a directory at the same path
/// is observable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RootIdentity {
    volume: u64,
    file: u64,
}

fn read_opened_tail_bytes(
    mut file: std::fs::File,
    relative: &Path,
    max_bytes: u64,
) -> Result<Vec<u8>, FsError> {
    let length = file
        .metadata()
        .map_err(|error| map_io(relative, error))?
        .len();
    let start = length.saturating_sub(max_bytes);
    file.seek(SeekFrom::Start(start))
        .map_err(|error| map_io(relative, error))?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)
        .map_err(|error| map_io(relative, error))?;
    Ok(bytes)
}

/// Options for an atomic root-confined write.
#[derive(Debug, Clone, Copy)]
pub struct AtomicWriteOptions {
    /// Replace an existing regular file when true; otherwise fail with
    /// [`FsError::AlreadyExists`].
    pub overwrite: bool,
    /// Create missing parent directories beneath the trusted root.
    pub create_parents: bool,
    /// Unix mode applied to newly-created directories.
    pub dir_mode: u32,
    /// Unix mode applied to the newly-created file.
    pub file_mode: u32,
}

/// Bytes and metadata read through the same no-follow handle that was opened
/// after the caller's permission check.
///
/// File tools must use this instead of reopening the canonical pathname.  The
/// pathname supplied by a model may contain a symlink, and a canonicalize-then-
/// read sequence otherwise leaves a retarget window between the two syscalls.
#[derive(Debug)]
pub struct RootedFileSnapshot {
    /// The bytes read from the opened regular-file handle.
    pub bytes: Vec<u8>,
    /// Size reported by that same handle.
    pub size: u64,
    /// Modification time reported by that same handle, when available.
    pub modified: Option<SystemTime>,
    /// Unix mode reported by that same handle, when the platform exposes one.
    pub mode: Option<u32>,
}

/// Metadata returned by a direct rooted write, without reopening its pathname.
#[derive(Debug, Default)]
pub struct RootedWriteResult {
    /// Modification time reported by the opened write handle, when available.
    pub modified: Option<SystemTime>,
}

/// Failures that need operation-specific model-facing messages in file tools.
#[derive(Debug, thiserror::Error)]
pub enum RootedFsError {
    /// A normal rooted filesystem failure.
    #[error(transparent)]
    Fs(#[from] FsError),
    /// The requested path no longer resolves to the approved target.
    #[error("requested symlink resolution changed")]
    SymlinkResolutionChanged,
    /// A requested parent no longer resolves to the approved directory.
    #[error("requested parent-directory symlink resolution changed")]
    ParentSymlinkResolutionChanged,
    /// The final component is a symlink and must be addressed by its target.
    #[error("requested final path component is a symbolic link")]
    LeafSymlink,
    /// The final component is not a regular file (for example, a FIFO).
    #[error("requested final path component is not a regular file")]
    NotRegularFile,
}

fn resolution_changed(requested: &Path, approved: &Path) -> RootedFsError {
    // A leaf link's target can have a different parent from the approved
    // target. Treat that as target-resolution drift, not parent-directory
    // drift; the caller can then preserve the Read-vs-Write error wording.
    if std::fs::symlink_metadata(requested)
        .map(|metadata| metadata.file_type().is_symlink())
        .unwrap_or(false)
    {
        return RootedFsError::SymlinkResolutionChanged;
    }
    let requested_parent = requested.parent().unwrap_or_else(|| Path::new("."));
    let approved_parent = approved.parent().unwrap_or_else(|| Path::new("."));
    match std::fs::canonicalize(requested_parent) {
        Ok(parent) if parent == approved_parent => RootedFsError::SymlinkResolutionChanged,
        _ => RootedFsError::ParentSymlinkResolutionChanged,
    }
}

fn verify_resolution(requested: &Path, approved: &Path) -> Result<(), RootedFsError> {
    match std::fs::canonicalize(requested) {
        Ok(current) if current == approved => Ok(()),
        Ok(_) | Err(_) => Err(resolution_changed(requested, approved)),
    }
}

fn verify_parent_resolution(requested: &Path, approved: &Path) -> Result<(), RootedFsError> {
    let requested_parent = requested.parent().unwrap_or_else(|| Path::new("."));
    let approved_parent = approved.parent().unwrap_or_else(|| Path::new("."));
    match std::fs::canonicalize(requested_parent) {
        Ok(current) if current == approved_parent => Ok(()),
        Ok(_) => Err(RootedFsError::ParentSymlinkResolutionChanged),
        Err(_) => {
            // A write may legitimately create missing parent components. In
            // that case the full parent cannot be canonicalized until the
            // rooted mkdirat/handle chain materializes it. Compare the nearest
            // existing ancestors instead, so a retargeted symlink is still
            // rejected before any directory is created.
            fn nearest_existing(path: &Path) -> Option<PathBuf> {
                let mut candidate = path;
                loop {
                    if let Ok(canonical) = std::fs::canonicalize(candidate) {
                        return Some(canonical);
                    }
                    candidate = candidate.parent()?;
                }
            }

            match (
                nearest_existing(requested_parent),
                nearest_existing(approved_parent),
            ) {
                (Some(current), Some(approved)) if current == approved => Ok(()),
                _ => Err(RootedFsError::ParentSymlinkResolutionChanged),
            }
        }
    }
}

fn verify_leaf_not_symlink(requested: &Path) -> Result<(), RootedFsError> {
    match std::fs::symlink_metadata(requested) {
        Ok(metadata) if metadata.file_type().is_symlink() => Err(RootedFsError::LeafSymlink),
        Ok(_) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(RootedFsError::Fs(FsError::Io(format!(
            "{}: {error}",
            requested.display()
        )))),
    }
}

/// Number of directory entries naming the file represented by an open handle.
/// The caller can open with `FILE_FLAG_OPEN_REPARSE_POINT` to avoid following
/// leaf links; querying the handle keeps this count tied to that same file.
///
/// # Errors
/// Returns the Windows error if handle information cannot be queried.
#[cfg(windows)]
pub fn file_link_count(file: &std::fs::File) -> std::io::Result<u32> {
    imp::file_link_count(file)
}

#[cfg(windows)]
mod imp {
    #[allow(clippy::wildcard_imports)]
    use super::*;
    use std::ffi::{c_void, OsStr, OsString};
    use std::os::windows::ffi::OsStrExt;
    use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
    use std::os::windows::io::{AsRawHandle, FromRawHandle, RawHandle};
    use std::sync::atomic::{AtomicU64, Ordering};

    type NtStatus = i32;
    type Handle = *mut c_void;

    #[repr(C)]
    struct UnicodeString {
        length: u16,
        maximum_length: u16,
        buffer: *mut u16,
    }

    #[repr(C)]
    struct ObjectAttributes {
        length: u32,
        root_directory: Handle,
        object_name: *mut UnicodeString,
        attributes: u32,
        security_descriptor: *mut c_void,
        security_quality_of_service: *mut c_void,
    }

    #[repr(C)]
    struct IoStatusBlock {
        status_or_pointer: usize,
        information: usize,
    }

    #[repr(C)]
    struct ByHandleFileInformation {
        file_attributes: u32,
        creation_time_low: u32,
        creation_time_high: u32,
        last_access_time_low: u32,
        last_access_time_high: u32,
        last_write_time_low: u32,
        last_write_time_high: u32,
        volume_serial_number: u32,
        file_size_high: u32,
        file_size_low: u32,
        number_of_links: u32,
        file_index_high: u32,
        file_index_low: u32,
    }

    #[link(name = "ntdll")]
    extern "system" {
        fn NtCreateFile(
            file_handle: *mut Handle,
            desired_access: u32,
            object_attributes: *mut ObjectAttributes,
            io_status_block: *mut IoStatusBlock,
            allocation_size: *mut i64,
            file_attributes: u32,
            share_access: u32,
            create_disposition: u32,
            create_options: u32,
            ea_buffer: *mut c_void,
            ea_length: u32,
        ) -> NtStatus;
        fn NtSetInformationFile(
            file_handle: Handle,
            io_status_block: *mut IoStatusBlock,
            file_information: *mut c_void,
            length: u32,
            file_information_class: u32,
        ) -> NtStatus;
        fn RtlNtStatusToDosError(status: NtStatus) -> u32;
    }

    #[link(name = "advapi32")]
    extern "system" {
        fn ConvertStringSecurityDescriptorToSecurityDescriptorW(
            string_security_descriptor: *const u16,
            string_sd_revision: u32,
            security_descriptor: *mut *mut c_void,
            security_descriptor_size: *mut u32,
        ) -> i32;
    }

    #[link(name = "kernel32")]
    extern "system" {
        fn GetFileInformationByHandle(
            file: Handle,
            information: *mut ByHandleFileInformation,
        ) -> i32;
        fn LocalFree(memory: *mut c_void) -> *mut c_void;
    }

    const DELETE: u32 = 0x0001_0000;
    const SYNCHRONIZE: u32 = 0x0010_0000;
    const GENERIC_READ: u32 = 0x8000_0000;
    const GENERIC_WRITE: u32 = 0x4000_0000;
    const FILE_TRAVERSE: u32 = 0x0000_0020;
    const FILE_READ_ATTRIBUTES: u32 = 0x0000_0080;
    const FILE_SHARE_READ: u32 = 0x0000_0001;
    const FILE_SHARE_WRITE: u32 = 0x0000_0002;
    const FILE_SHARE_DELETE: u32 = 0x0000_0004;
    const SHARE_ALL: u32 = FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE;
    const FILE_OPEN: u32 = 1;
    const FILE_CREATE: u32 = 2;
    const FILE_OPEN_IF: u32 = 3;
    const FILE_OVERWRITE_IF: u32 = 5;
    const FILE_DIRECTORY_FILE: u32 = 0x0000_0001;
    const FILE_SYNCHRONOUS_IO_NONALERT: u32 = 0x0000_0020;
    const FILE_NON_DIRECTORY_FILE: u32 = 0x0000_0040;
    const FILE_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
    const FILE_ATTRIBUTE_DIRECTORY: u32 = 0x0000_0010;
    const FILE_ATTRIBUTE_NORMAL: u32 = 0x0000_0080;
    const FILE_ATTRIBUTE_TEMPORARY: u32 = 0x0000_0100;
    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0000_0400;
    const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
    const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
    const OBJ_CASE_INSENSITIVE: u32 = 0x0000_0040;
    const OBJ_DONT_REPARSE: u32 = 0x0000_1000;
    const FILE_RENAME_INFORMATION: u32 = 10;
    const FILE_DISPOSITION_INFORMATION: u32 = 13;

    static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

    struct LocalSecurityDescriptor(*mut c_void);

    impl Drop for LocalSecurityDescriptor {
        fn drop(&mut self) {
            // SAFETY: the descriptor is allocated by LocalAlloc inside the
            // conversion API and remains uniquely owned by this guard.
            unsafe {
                LocalFree(self.0);
            }
        }
    }

    fn private_security_descriptor() -> Result<LocalSecurityDescriptor, FsError> {
        // Protected DACL: full access for the object owner and LocalSystem.
        let sddl: Vec<u16> = "D:P(A;;GA;;;OW)(A;;GA;;;SY)\0".encode_utf16().collect();
        let mut descriptor = std::ptr::null_mut();
        // SAFETY: the NUL-terminated SDDL and output pointer remain valid for
        // this synchronous call.
        let converted = unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                sddl.as_ptr(),
                1,
                &mut descriptor,
                std::ptr::null_mut(),
            )
        };
        if converted == 0 || descriptor.is_null() {
            Err(FsError::Io(format!(
                "could not create private Windows security descriptor: {}",
                std::io::Error::last_os_error()
            )))
        } else {
            Ok(LocalSecurityDescriptor(descriptor))
        }
    }

    fn nt_error(status: NtStatus) -> std::io::Error {
        // SAFETY: this conversion accepts every NTSTATUS and owns no resources.
        let code = unsafe { RtlNtStatusToDosError(status) };
        if code == u32::MAX {
            std::io::Error::new(
                std::io::ErrorKind::Other,
                format!(
                    "NTSTATUS 0x{:08x}",
                    u32::from_ne_bytes(status.to_ne_bytes())
                ),
            )
        } else {
            std::io::Error::from_raw_os_error(i32::from_ne_bytes(code.to_ne_bytes()))
        }
    }

    fn wide_component(name: &OsStr, path: &Path) -> Result<Vec<u16>, FsError> {
        let wide: Vec<u16> = name.encode_wide().collect();
        if wide.is_empty()
            || wide
                .iter()
                .any(|unit| *unit == 0 || *unit == u16::from(b':'))
            || wide.len() > u16::MAX as usize / 2
        {
            return Err(FsError::OutsideWorkspace(path.display().to_string()));
        }
        Ok(wide)
    }

    fn split(relative: &Path) -> Result<(Vec<OsString>, OsString), FsError> {
        validate_relative_path(relative)?;
        let mut parts: Vec<_> = relative
            .components()
            .filter_map(|component| match component {
                Component::Normal(value) => Some(value.to_os_string()),
                _ => None,
            })
            .collect();
        let file = parts
            .pop()
            .ok_or_else(|| FsError::OutsideWorkspace(relative.display().to_string()))?;
        for part in parts.iter().chain(std::iter::once(&file)) {
            let _ = wide_component(part, relative)?;
        }
        Ok((parts, file))
    }

    #[allow(clippy::too_many_arguments)]
    fn nt_create_relative(
        parent: &std::fs::File,
        name: &OsStr,
        path: &Path,
        desired_access: u32,
        share_access: u32,
        create_disposition: u32,
        create_options: u32,
        file_attributes: u32,
        security_descriptor: *mut c_void,
    ) -> Result<std::fs::File, FsError> {
        let mut wide = wide_component(name, path)?;
        let byte_len = u16::try_from(wide.len() * 2)
            .map_err(|_| FsError::OutsideWorkspace(path.display().to_string()))?;
        let mut unicode = UnicodeString {
            length: byte_len,
            maximum_length: byte_len,
            buffer: wide.as_mut_ptr(),
        };
        let object_attributes_length = u32::try_from(std::mem::size_of::<ObjectAttributes>())
            .map_err(|_| FsError::Io("OBJECT_ATTRIBUTES size exceeds ULONG".into()))?;
        let mut attributes = ObjectAttributes {
            length: object_attributes_length,
            root_directory: parent.as_raw_handle(),
            object_name: &mut unicode,
            attributes: OBJ_CASE_INSENSITIVE | OBJ_DONT_REPARSE,
            security_descriptor,
            security_quality_of_service: std::ptr::null_mut(),
        };
        let mut io_status = IoStatusBlock {
            status_or_pointer: 0,
            information: 0,
        };
        let mut handle = std::ptr::null_mut();
        // SAFETY: all pointers reference live, aligned C-layout values for this
        // synchronous call. A successful handle is transferred once to `File`.
        let status = unsafe {
            NtCreateFile(
                &mut handle,
                desired_access,
                &mut attributes,
                &mut io_status,
                std::ptr::null_mut(),
                file_attributes,
                share_access,
                create_disposition,
                create_options | FILE_OPEN_REPARSE_POINT | FILE_SYNCHRONOUS_IO_NONALERT,
                std::ptr::null_mut(),
                0,
            )
        };
        if status < 0 {
            return Err(map_io(path, nt_error(status)));
        }
        if handle.is_null() {
            return Err(FsError::Io(format!(
                "{}: NtCreateFile returned a null handle",
                path.display()
            )));
        }
        // SAFETY: NtCreateFile returned a uniquely owned HANDLE.
        Ok(unsafe { std::fs::File::from_raw_handle(handle as RawHandle) })
    }

    fn is_reparse(metadata: &std::fs::Metadata) -> bool {
        metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
    }

    fn ensure_directory(file: &std::fs::File, path: &Path) -> Result<(), FsError> {
        let metadata = file.metadata().map_err(|error| map_io(path, error))?;
        if metadata.is_dir() && !is_reparse(&metadata) {
            Ok(())
        } else {
            Err(FsError::OutsideWorkspace(path.display().to_string()))
        }
    }

    fn ensure_regular(file: &std::fs::File, path: &Path) -> Result<(), FsError> {
        let metadata = file.metadata().map_err(|error| map_io(path, error))?;
        if metadata.is_file() && !is_reparse(&metadata) {
            Ok(())
        } else {
            Err(FsError::OutsideWorkspace(path.display().to_string()))
        }
    }

    fn file_information(file: &std::fs::File) -> std::io::Result<ByHandleFileInformation> {
        let mut information = std::mem::MaybeUninit::<ByHandleFileInformation>::uninit();
        // SAFETY: `file` owns a live handle and the API initializes the output
        // structure before returning success.
        let ok =
            unsafe { GetFileInformationByHandle(file.as_raw_handle(), information.as_mut_ptr()) };
        if ok == 0 {
            return Err(std::io::Error::last_os_error());
        }
        // SAFETY: success above guarantees complete initialization.
        Ok(unsafe { information.assume_init() })
    }

    pub(super) fn file_link_count(file: &std::fs::File) -> std::io::Result<u32> {
        Ok(file_information(file)?.number_of_links)
    }

    fn root_identity_from_file(file: &std::fs::File, path: &Path) -> Result<RootIdentity, FsError> {
        let information = file_information(file).map_err(|error| map_io(path, error))?;
        Ok(RootIdentity {
            volume: u64::from(information.volume_serial_number),
            file: (u64::from(information.file_index_high) << 32)
                | u64::from(information.file_index_low),
        })
    }

    fn open_root_checked(
        root: &Path,
        expected: Option<&RootIdentity>,
    ) -> Result<std::fs::File, FsError> {
        let canonical = std::fs::canonicalize(root).map_err(|error| map_io(root, error))?;
        let mut options = std::fs::OpenOptions::new();
        options
            .read(true)
            .share_mode(SHARE_ALL)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT);
        let file = options
            .open(&canonical)
            .map_err(|error| map_io(&canonical, error))?;
        ensure_directory(&file, &canonical)?;
        if let Some(expected) = expected {
            if root_identity_from_file(&file, root)? != *expected {
                return Err(FsError::OutsideWorkspace(format!(
                    "root directory identity changed: {}",
                    root.display()
                )));
            }
        }
        Ok(file)
    }

    fn open_root(root: &Path) -> Result<std::fs::File, FsError> {
        open_root_checked(root, None)
    }

    pub(super) fn root_identity(root: &Path) -> Result<RootIdentity, FsError> {
        let metadata = std::fs::symlink_metadata(root).map_err(|error| map_io(root, error))?;
        if !metadata.is_dir() || is_reparse(&metadata) {
            return Err(FsError::OutsideWorkspace(root.display().to_string()));
        }
        let root_file = open_root(root)?;
        root_identity_from_file(&root_file, root)
    }

    pub(super) fn ensure_private_directory(
        root: &Path,
        relative: &Path,
        _dir_mode: u32,
    ) -> Result<RootIdentity, FsError> {
        let (parent, directory_name) = open_parent(root, relative, true)?;
        let descriptor = private_security_descriptor()?;
        let directory = nt_create_relative(
            &parent,
            &directory_name,
            relative,
            FILE_TRAVERSE | FILE_READ_ATTRIBUTES | SYNCHRONIZE,
            SHARE_ALL,
            FILE_OPEN_IF,
            FILE_DIRECTORY_FILE,
            FILE_ATTRIBUTE_DIRECTORY,
            descriptor.0,
        )?;
        ensure_directory(&directory, relative)?;
        root_identity_from_file(&directory, relative)
    }

    fn open_parent(
        root: &Path,
        relative: &Path,
        create: bool,
    ) -> Result<(std::fs::File, OsString), FsError> {
        open_parent_checked(root, relative, create, None)
    }

    fn open_parent_checked(
        root: &Path,
        relative: &Path,
        create: bool,
        expected: Option<&RootIdentity>,
    ) -> Result<(std::fs::File, OsString), FsError> {
        let (parents, file_name) = split(relative)?;
        let mut directory = open_root_checked(root, expected)?;
        for component in parents {
            let next = nt_create_relative(
                &directory,
                &component,
                relative,
                FILE_TRAVERSE | FILE_READ_ATTRIBUTES | SYNCHRONIZE,
                SHARE_ALL,
                if create { FILE_OPEN_IF } else { FILE_OPEN },
                FILE_DIRECTORY_FILE,
                FILE_ATTRIBUTE_DIRECTORY,
                std::ptr::null_mut(),
            )?;
            ensure_directory(&next, relative)?;
            directory = next;
        }
        Ok((directory, file_name))
    }

    #[allow(clippy::too_many_arguments)]
    fn open_regular(
        parent: &std::fs::File,
        file_name: &OsStr,
        relative: &Path,
        desired_access: u32,
        share_access: u32,
        disposition: u32,
        attributes: u32,
    ) -> Result<std::fs::File, FsError> {
        open_regular_with_security(
            parent,
            file_name,
            relative,
            desired_access,
            share_access,
            disposition,
            attributes,
            std::ptr::null_mut(),
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn open_regular_with_security(
        parent: &std::fs::File,
        file_name: &OsStr,
        relative: &Path,
        desired_access: u32,
        share_access: u32,
        disposition: u32,
        attributes: u32,
        security_descriptor: *mut c_void,
    ) -> Result<std::fs::File, FsError> {
        let file = nt_create_relative(
            parent,
            file_name,
            relative,
            desired_access,
            share_access,
            disposition,
            FILE_NON_DIRECTORY_FILE,
            attributes,
            security_descriptor,
        )?;
        ensure_regular(&file, relative)?;
        Ok(file)
    }

    fn validate_optional_regular(
        parent: &std::fs::File,
        file_name: &OsStr,
        relative: &Path,
    ) -> Result<(), FsError> {
        match open_regular(
            parent,
            file_name,
            relative,
            FILE_READ_ATTRIBUTES | SYNCHRONIZE,
            SHARE_ALL,
            FILE_OPEN,
            FILE_ATTRIBUTE_NORMAL,
        ) {
            Ok(file) => {
                drop(file);
                Ok(())
            }
            Err(FsError::NotFound(_)) => Ok(()),
            Err(error) => Err(error),
        }
    }

    fn align_up(value: usize, alignment: usize) -> usize {
        (value + alignment - 1) & !(alignment - 1)
    }

    fn rename_relative(
        source: &std::fs::File,
        parent: &std::fs::File,
        file_name: &OsStr,
        relative: &Path,
        overwrite: bool,
    ) -> Result<(), FsError> {
        let wide = wide_component(file_name, relative)?;
        let root_offset = align_up(1, std::mem::align_of::<Handle>());
        let length_offset = root_offset + std::mem::size_of::<Handle>();
        let name_offset = length_offset + std::mem::size_of::<u32>();
        let name_bytes = wide.len() * std::mem::size_of::<u16>();
        let total = name_offset + name_bytes;
        let name_bytes_u32 = u32::try_from(name_bytes)
            .map_err(|_| FsError::OutsideWorkspace(relative.display().to_string()))?;
        let total_u32 = u32::try_from(total)
            .map_err(|_| FsError::OutsideWorkspace(relative.display().to_string()))?;
        let word = std::mem::size_of::<usize>();
        let mut storage = vec![0usize; (total + word - 1) / word];
        let base = storage.as_mut_ptr().cast::<u8>();
        // SAFETY: native-word storage is aligned and large enough for the
        // documented variable-sized FILE_RENAME_INFORMATION layout.
        unsafe {
            base.write(u8::from(overwrite));
            let root_handle = parent.as_raw_handle();
            std::ptr::copy_nonoverlapping(
                std::ptr::from_ref(&root_handle).cast::<u8>(),
                base.add(root_offset),
                std::mem::size_of::<Handle>(),
            );
            let length_bytes = name_bytes_u32.to_ne_bytes();
            std::ptr::copy_nonoverlapping(
                length_bytes.as_ptr(),
                base.add(length_offset),
                length_bytes.len(),
            );
            std::ptr::copy_nonoverlapping(
                wide.as_ptr().cast::<u8>(),
                base.add(name_offset),
                name_bytes,
            );
        }
        let mut io_status = IoStatusBlock {
            status_or_pointer: 0,
            information: 0,
        };
        // SAFETY: the aligned rename buffer remains live during this call.
        let status = unsafe {
            NtSetInformationFile(
                source.as_raw_handle(),
                &mut io_status,
                storage.as_mut_ptr().cast(),
                total_u32,
                FILE_RENAME_INFORMATION,
            )
        };
        if status >= 0 {
            Ok(())
        } else {
            Err(map_io(relative, nt_error(status)))
        }
    }

    fn mark_delete(file: &std::fs::File, relative: &Path) -> Result<(), FsError> {
        let mut delete_file = 1u8;
        let mut io_status = IoStatusBlock {
            status_or_pointer: 0,
            information: 0,
        };
        // SAFETY: FILE_DISPOSITION_INFORMATION is one BOOLEAN byte.
        let status = unsafe {
            NtSetInformationFile(
                file.as_raw_handle(),
                &mut io_status,
                std::ptr::from_mut(&mut delete_file).cast(),
                1,
                FILE_DISPOSITION_INFORMATION,
            )
        };
        if status >= 0 {
            Ok(())
        } else {
            Err(map_io(relative, nt_error(status)))
        }
    }

    fn temp_name(final_name: &OsStr) -> OsString {
        let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let mut name = OsString::from(".");
        name.push(final_name);
        name.push(format!(".tmp-{}-{sequence}", std::process::id()));
        name
    }

    fn create_temp(
        parent: &std::fs::File,
        file_name: &OsStr,
        relative: &Path,
    ) -> Result<std::fs::File, FsError> {
        let descriptor = private_security_descriptor()?;
        for _ in 0..128 {
            let candidate = temp_name(file_name);
            match open_regular_with_security(
                parent,
                &candidate,
                relative,
                GENERIC_READ | GENERIC_WRITE | DELETE | SYNCHRONIZE,
                SHARE_ALL,
                FILE_CREATE,
                FILE_ATTRIBUTE_TEMPORARY,
                descriptor.0,
            ) {
                Ok(file) => return Ok(file),
                Err(FsError::AlreadyExists(_)) => continue,
                Err(error) => return Err(error),
            }
        }
        Err(FsError::Io(format!(
            "{}: unable to allocate a unique temporary file",
            relative.display()
        )))
    }

    pub(super) fn lock_exclusive(
        root: &Path,
        relative: &Path,
        _dir_mode: u32,
        _file_mode: u32,
    ) -> Result<RootedFileLock, FsError> {
        let (parent, file_name) = open_parent(root, relative, true)?;
        // Omitting FILE_SHARE_DELETE pins the lock directory entry for the
        // guard lifetime, so a racing rename cannot split lock ownership.
        let file = open_regular(
            &parent,
            &file_name,
            relative,
            GENERIC_READ | GENERIC_WRITE | SYNCHRONIZE,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            FILE_OPEN_IF,
            FILE_ATTRIBUTE_NORMAL,
        )?;
        file.lock_exclusive()
            .map_err(|error| map_io(relative, error))?;
        Ok(RootedFileLock {
            _file: file,
            path: root.join(relative).display().to_string(),
        })
    }

    pub(super) fn read_to_string(root: &Path, relative: &Path) -> Result<String, FsError> {
        let (parent, file_name) = open_parent(root, relative, false)?;
        let mut file = open_regular(
            &parent,
            &file_name,
            relative,
            GENERIC_READ | SYNCHRONIZE,
            SHARE_ALL,
            FILE_OPEN,
            FILE_ATTRIBUTE_NORMAL,
        )?;
        let mut body = String::new();
        file.read_to_string(&mut body)
            .map_err(|error| map_io(relative, error))?;
        Ok(body)
    }

    pub(super) fn read_to_string_limited(
        root: &Path,
        relative: &Path,
        max_bytes: u64,
    ) -> Result<String, FsError> {
        let (parent, file_name) = open_parent(root, relative, false)?;
        let file = open_regular(
            &parent,
            &file_name,
            relative,
            GENERIC_READ | SYNCHRONIZE,
            SHARE_ALL,
            FILE_OPEN,
            FILE_ATTRIBUTE_NORMAL,
        )?;
        read_opened_to_string_limited(file, relative, max_bytes)
    }

    pub(super) fn read_tail_bytes(
        root: &Path,
        relative: &Path,
        max_bytes: u64,
    ) -> Result<Vec<u8>, FsError> {
        let (parent, file_name) = open_parent(root, relative, false)?;
        let file = open_regular(
            &parent,
            &file_name,
            relative,
            GENERIC_READ | SYNCHRONIZE,
            SHARE_ALL,
            FILE_OPEN,
            FILE_ATTRIBUTE_NORMAL,
        )?;
        read_opened_tail_bytes(file, relative, max_bytes)
    }

    pub(super) fn create_new_file(root: &Path, relative: &Path) -> Result<(), FsError> {
        create_new_file_pinned(root, relative, None)
    }

    pub(super) fn create_new_file_pinned(
        root: &Path,
        relative: &Path,
        expected: Option<&RootIdentity>,
    ) -> Result<(), FsError> {
        let (parent, file_name) = open_parent_checked(root, relative, false, expected)?;
        let file = open_regular(
            &parent,
            &file_name,
            relative,
            GENERIC_WRITE | SYNCHRONIZE,
            SHARE_ALL,
            FILE_CREATE,
            FILE_ATTRIBUTE_NORMAL,
        )?;
        drop(file);
        Ok(())
    }

    pub(super) fn append_file(root: &Path, relative: &Path, content: &str) -> Result<(), FsError> {
        append_file_bytes_pinned(root, relative, content.as_bytes(), None)
    }

    pub(super) fn append_file_bytes(
        root: &Path,
        relative: &Path,
        content: &[u8],
    ) -> Result<(), FsError> {
        append_file_bytes_pinned(root, relative, content, None)
    }

    pub(super) fn append_file_pinned(
        root: &Path,
        relative: &Path,
        content: &str,
        expected: Option<&RootIdentity>,
    ) -> Result<(), FsError> {
        append_file_bytes_pinned(root, relative, content.as_bytes(), expected)
    }

    fn append_file_bytes_pinned(
        root: &Path,
        relative: &Path,
        content: &[u8],
        expected: Option<&RootIdentity>,
    ) -> Result<(), FsError> {
        // FILE_APPEND_DATA makes each write append at the filesystem level;
        // unlike a seek-to-end followed by a write, concurrent workers cannot
        // overwrite one another's chunks.
        const FILE_APPEND_DATA: u32 = 0x0004;
        let (parent, file_name) = open_parent_checked(root, relative, false, expected)?;
        let mut file = open_regular(
            &parent,
            &file_name,
            relative,
            FILE_APPEND_DATA | SYNCHRONIZE,
            SHARE_ALL,
            FILE_OPEN_IF,
            FILE_ATTRIBUTE_NORMAL,
        )?;
        file.write_all(content)
            .map_err(|error| map_io(relative, error))?;
        file.flush().map_err(|error| map_io(relative, error))
    }

    pub(super) fn open_append_file_pinned(
        root: &Path,
        relative: &Path,
        expected: Option<&RootIdentity>,
    ) -> Result<std::fs::File, FsError> {
        const FILE_APPEND_DATA: u32 = 0x0004;
        let (parent, file_name) = open_parent_checked(root, relative, false, expected)?;
        let descriptor = private_security_descriptor()?;
        open_regular_with_security(
            &parent,
            &file_name,
            relative,
            FILE_APPEND_DATA | SYNCHRONIZE,
            SHARE_ALL,
            FILE_OPEN_IF,
            FILE_ATTRIBUTE_NORMAL,
            descriptor.0,
        )
    }

    pub(super) fn read_to_string_pinned(
        root: &Path,
        relative: &Path,
        expected: Option<&RootIdentity>,
    ) -> Result<String, FsError> {
        let (parent, file_name) = open_parent_checked(root, relative, false, expected)?;
        let mut file = open_regular(
            &parent,
            &file_name,
            relative,
            GENERIC_READ | SYNCHRONIZE,
            SHARE_ALL,
            FILE_OPEN,
            FILE_ATTRIBUTE_NORMAL,
        )?;
        let mut body = String::new();
        file.read_to_string(&mut body)
            .map_err(|error| map_io(relative, error))?;
        Ok(body)
    }

    fn atomic_write_inner<F>(
        root: &Path,
        relative: &Path,
        bytes: &[u8],
        options: AtomicWriteOptions,
        after_parent_open: F,
    ) -> Result<(), FsError>
    where
        F: FnOnce(),
    {
        let (parent, file_name) = open_parent(root, relative, options.create_parents)?;
        after_parent_open();
        validate_optional_regular(&parent, &file_name, relative)?;
        let mut temp = create_temp(&parent, &file_name, relative)?;
        let result = (|| {
            temp.write_all(bytes)
                .map_err(|error| map_io(relative, error))?;
            temp.sync_all().map_err(|error| map_io(relative, error))?;
            validate_optional_regular(&parent, &file_name, relative)?;
            rename_relative(&temp, &parent, &file_name, relative, options.overwrite)
        })();
        if result.is_err() {
            let _ = mark_delete(&temp, relative);
        }
        result
    }

    pub(super) fn atomic_write(
        root: &Path,
        relative: &Path,
        bytes: &[u8],
        options: AtomicWriteOptions,
    ) -> Result<(), FsError> {
        atomic_write_inner(root, relative, bytes, options, || {})
    }

    fn read_file_after_permission_inner<F>(
        root: &Path,
        relative: &Path,
        requested: &Path,
        approved: &Path,
        after_permission: F,
    ) -> Result<RootedFileSnapshot, RootedFsError>
    where
        F: FnOnce(),
    {
        let target_exists = std::fs::symlink_metadata(requested).is_ok();
        if target_exists {
            verify_resolution(requested, approved)?;
        } else {
            verify_parent_resolution(requested, approved)?;
        }
        after_permission();
        // Re-check before opening the handle. The handle-relative NT open below
        // is what prevents a subsequent parent reparse-point swap from
        // redirecting the actual I/O.
        if target_exists {
            verify_resolution(requested, approved)?;
        } else {
            verify_parent_resolution(requested, approved)?;
        }
        let (parent, file_name) = open_parent(root, relative, false).map_err(RootedFsError::Fs)?;
        let mut file = open_regular(
            &parent,
            &file_name,
            relative,
            GENERIC_READ | SYNCHRONIZE,
            SHARE_ALL,
            FILE_OPEN,
            FILE_ATTRIBUTE_NORMAL,
        )
        .map_err(|error| match error {
            FsError::OutsideWorkspace(_) => RootedFsError::NotRegularFile,
            error => RootedFsError::Fs(error),
        })?;
        // Validate the requested path after opening, before consuming bytes.
        // A stable leaf symlink is intentionally supported for Read: `relative`
        // addresses its already-approved resolved target.
        if target_exists {
            verify_resolution(requested, approved)?;
        } else {
            verify_parent_resolution(requested, approved)?;
        }
        let metadata = file
            .metadata()
            .map_err(|error| RootedFsError::Fs(map_io(relative, error)))?;
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)
            .map_err(|error| RootedFsError::Fs(map_io(relative, error)))?;
        Ok(RootedFileSnapshot {
            size: metadata.len(),
            modified: metadata.modified().ok(),
            mode: None,
            bytes,
        })
    }

    pub(super) fn read_file_after_permission(
        root: &Path,
        relative: &Path,
        requested: &Path,
        approved: &Path,
    ) -> Result<RootedFileSnapshot, RootedFsError> {
        read_file_after_permission_inner(root, relative, requested, approved, || {})
    }

    pub(super) fn open_file_after_permission(
        root: &Path,
        relative: &Path,
        requested: &Path,
        approved: &Path,
    ) -> Result<std::fs::File, RootedFsError> {
        let target_exists = std::fs::symlink_metadata(requested).is_ok();
        if target_exists {
            verify_resolution(requested, approved)?;
        } else {
            verify_parent_resolution(requested, approved)?;
        }
        let (parent, file_name) = open_parent(root, relative, false).map_err(RootedFsError::Fs)?;
        let file = open_regular(
            &parent,
            &file_name,
            relative,
            GENERIC_READ | SYNCHRONIZE,
            SHARE_ALL,
            FILE_OPEN,
            FILE_ATTRIBUTE_NORMAL,
        )
        .map_err(|error| match error {
            FsError::OutsideWorkspace(_) => RootedFsError::NotRegularFile,
            error => RootedFsError::Fs(error),
        })?;
        if target_exists {
            verify_resolution(requested, approved)?;
        } else {
            verify_parent_resolution(requested, approved)?;
        }
        Ok(file)
    }

    #[cfg(test)]
    pub(super) fn read_file_after_permission_for_test<F>(
        root: &Path,
        relative: &Path,
        requested: &Path,
        approved: &Path,
        after_permission: F,
    ) -> Result<RootedFileSnapshot, RootedFsError>
    where
        F: FnOnce(),
    {
        read_file_after_permission_inner(root, relative, requested, approved, after_permission)
    }

    fn write_file_after_permission_inner<F>(
        root: &Path,
        relative: &Path,
        requested: &Path,
        approved: &Path,
        bytes: &[u8],
        after_permission: F,
    ) -> Result<RootedWriteResult, RootedFsError>
    where
        F: FnOnce(),
    {
        let target_exists = std::fs::symlink_metadata(requested).is_ok();
        if target_exists {
            verify_resolution(requested, approved)?;
            verify_leaf_not_symlink(requested)?;
        } else {
            verify_parent_resolution(requested, approved)?;
        }
        after_permission();
        // Missing parents are materialized by the fixed no-follow directory
        // handle chain. Never create them through the requested pathname:
        // doing so would reopen a retargetable ancestor before rooted I/O.
        let (parent, file_name) = open_parent(root, relative, true).map_err(RootedFsError::Fs)?;
        verify_parent_resolution(requested, approved)?;
        verify_leaf_not_symlink(requested)?;
        let mut file = open_regular(
            &parent,
            &file_name,
            relative,
            GENERIC_WRITE | SYNCHRONIZE,
            SHARE_ALL,
            FILE_OVERWRITE_IF,
            FILE_ATTRIBUTE_NORMAL,
        )
        .map_err(|error| {
            if matches!(error, FsError::OutsideWorkspace(_))
                && std::fs::symlink_metadata(requested)
                    .map(|metadata| metadata.file_type().is_symlink())
                    .unwrap_or(false)
            {
                RootedFsError::LeafSymlink
            } else {
                RootedFsError::Fs(error)
            }
        })?;
        if target_exists {
            verify_resolution(requested, approved)?;
        } else {
            verify_parent_resolution(requested, approved)?;
        }
        file.write_all(bytes)
            .map_err(|error| RootedFsError::Fs(map_io(relative, error)))?;
        verify_parent_resolution(requested, approved)?;
        if target_exists {
            verify_resolution(requested, approved)?;
        }
        Ok(RootedWriteResult {
            modified: file
                .metadata()
                .ok()
                .and_then(|metadata| metadata.modified().ok()),
        })
    }

    pub(super) fn write_file_after_permission(
        root: &Path,
        relative: &Path,
        requested: &Path,
        approved: &Path,
        bytes: &[u8],
    ) -> Result<RootedWriteResult, RootedFsError> {
        write_file_after_permission_inner(root, relative, requested, approved, bytes, || {})
    }

    #[cfg(test)]
    pub(super) fn write_file_after_permission_for_test<F>(
        root: &Path,
        relative: &Path,
        requested: &Path,
        approved: &Path,
        bytes: &[u8],
        after_permission: F,
    ) -> Result<RootedWriteResult, RootedFsError>
    where
        F: FnOnce(),
    {
        write_file_after_permission_inner(
            root,
            relative,
            requested,
            approved,
            bytes,
            after_permission,
        )
    }

    #[cfg(test)]
    pub(super) fn atomic_write_after_parent_open_for_test<F>(
        root: &Path,
        relative: &Path,
        bytes: &[u8],
        options: AtomicWriteOptions,
        after_parent_open: F,
    ) -> Result<(), FsError>
    where
        F: FnOnce(),
    {
        atomic_write_inner(root, relative, bytes, options, after_parent_open)
    }

    pub(super) fn remove_file(root: &Path, relative: &Path) -> Result<(), FsError> {
        let (parent, file_name) = open_parent(root, relative, false)?;
        let file = open_regular(
            &parent,
            &file_name,
            relative,
            DELETE | FILE_READ_ATTRIBUTES | SYNCHRONIZE,
            SHARE_ALL,
            FILE_OPEN,
            FILE_ATTRIBUTE_NORMAL,
        )?;
        mark_delete(&file, relative)
    }
}

impl Default for AtomicWriteOptions {
    fn default() -> Self {
        Self {
            overwrite: true,
            create_parents: true,
            dir_mode: PRIVATE_DIR_MODE,
            file_mode: PRIVATE_FILE_MODE,
        }
    }
}

/// OS advisory lock held on a root-confined file.
pub struct RootedFileLock {
    _file: std::fs::File,
    path: String,
}

impl FlockGuard for RootedFileLock {
    fn path(&self) -> &str {
        &self.path
    }
}

/// Reject absolute paths, parent traversal and platform prefixes.
pub fn validate_relative_path(relative: &Path) -> Result<(), FsError> {
    if relative.as_os_str().is_empty() || relative.is_absolute() {
        return Err(FsError::OutsideWorkspace(relative.display().to_string()));
    }
    for component in relative.components() {
        if !matches!(component, Component::Normal(_)) {
            return Err(FsError::OutsideWorkspace(relative.display().to_string()));
        }
    }
    Ok(())
}

/// Join a validated relative path for mock/default filesystem implementations.
pub fn checked_join(root: &Path, relative: &Path) -> Result<PathBuf, FsError> {
    validate_relative_path(relative)?;
    Ok(root.join(relative))
}

/// Capture the stable identity of `root` from an opened no-follow directory
/// handle.
pub fn root_identity(root: &Path) -> Result<RootIdentity, FsError> {
    imp::root_identity(root)
}

/// Create or open one directory below `root` without following symlinks and
/// return the identity of the opened directory handle. Newly-created Unix
/// directories use `dir_mode`; existing Unix directories are tightened to the
/// same mode before their identity is returned.
pub fn ensure_private_directory(
    root: &Path,
    relative: &Path,
    dir_mode: u32,
) -> Result<RootIdentity, FsError> {
    imp::ensure_private_directory(root, relative, dir_mode)
}

/// Acquire an exclusive lock without following any component below `root`.
pub fn lock_exclusive(
    root: &Path,
    relative: &Path,
    dir_mode: u32,
    file_mode: u32,
) -> Result<RootedFileLock, FsError> {
    imp::lock_exclusive(root, relative, dir_mode, file_mode)
}

/// Read a UTF-8 file without following any component below `root`.
pub fn read_to_string(root: &Path, relative: &Path) -> Result<String, FsError> {
    imp::read_to_string(root, relative)
}

/// Read at most `max_bytes` of UTF-8 without following any component below
/// `root`. The bound is enforced while reading, including when the file grows
/// concurrently.
pub fn read_to_string_limited(
    root: &Path,
    relative: &Path,
    max_bytes: u64,
) -> Result<String, FsError> {
    imp::read_to_string_limited(root, relative, max_bytes)
}

/// Read the final `max_bytes` from a regular file through a rooted no-follow
/// handle. This is intended for bounded log tails; unlike
/// [`read_to_string_limited`], a large file is not an error.
pub fn read_tail_bytes(root: &Path, relative: &Path, max_bytes: u64) -> Result<Vec<u8>, FsError> {
    imp::read_tail_bytes(root, relative, max_bytes)
}

/// Exclusively create a regular file without following any component below
/// `root`.
pub fn create_new_file(root: &Path, relative: &Path) -> Result<(), FsError> {
    imp::create_new_file(root, relative)
}

/// Exclusively create a file only if the opened root still has `expected`
/// identity.
pub fn create_new_file_pinned(
    root: &Path,
    relative: &Path,
    expected: Option<&RootIdentity>,
) -> Result<(), FsError> {
    imp::create_new_file_pinned(root, relative, expected)
}

/// Exclusively create and return a regular file through a pinned, no-follow
/// root handle. Unlike a create-then-reopen sequence, the returned handle is
/// the exact inode whose `O_EXCL` allocation succeeded.
#[cfg(unix)]
pub fn open_create_new_file_pinned(
    root: &Path,
    relative: &Path,
    expected: Option<&RootIdentity>,
) -> Result<std::fs::File, FsError> {
    imp::open_create_new_file_pinned(root, relative, expected)
}

/// Append to a regular file without following any component below `root`.
pub fn append_file(root: &Path, relative: &Path, content: &str) -> Result<(), FsError> {
    imp::append_file(root, relative, content)
}

/// Append raw bytes through a rooted no-follow handle.
pub fn append_file_bytes(root: &Path, relative: &Path, content: &[u8]) -> Result<(), FsError> {
    imp::append_file_bytes(root, relative, content)
}

/// Append UTF-8 only if the opened root still has `expected` identity.
pub fn append_file_pinned(
    root: &Path,
    relative: &Path,
    content: &str,
    expected: Option<&RootIdentity>,
) -> Result<(), FsError> {
    imp::append_file_pinned(root, relative, content, expected)
}

/// Open a regular file for filesystem-level append through a no-follow rooted
/// handle. The returned file remains confined even if a pathname is swapped
/// after this call, and `expected` rejects a root directory replacement before
/// the file is opened.
pub fn open_append_file_pinned(
    root: &Path,
    relative: &Path,
    expected: Option<&RootIdentity>,
) -> Result<std::fs::File, FsError> {
    imp::open_append_file_pinned(root, relative, expected)
}

/// Read UTF-8 only if the opened root still has `expected` identity.
pub fn read_to_string_pinned(
    root: &Path,
    relative: &Path,
    expected: Option<&RootIdentity>,
) -> Result<String, FsError> {
    imp::read_to_string_pinned(root, relative, expected)
}

/// Read bytes and metadata after checking that `requested` still resolves to
/// `approved`. The approved path is opened relative to a fixed, no-follow
/// directory-handle chain, so a concurrent parent swap cannot redirect I/O.
pub fn read_file_after_permission(
    root: &Path,
    relative: &Path,
    requested: &Path,
    approved: &Path,
) -> Result<RootedFileSnapshot, RootedFsError> {
    imp::read_file_after_permission(root, relative, requested, approved)
}

/// Open a regular file after checking that `requested` still resolves to
/// `approved`. The returned handle is opened through a fixed, no-follow
/// directory-handle chain, so callers can safely read it without reopening a
/// pathname after a concurrent parent or leaf swap.
pub fn open_file_after_permission(
    root: &Path,
    relative: &Path,
    requested: &Path,
    approved: &Path,
) -> Result<std::fs::File, RootedFsError> {
    imp::open_file_after_permission(root, relative, requested, approved)
}

/// Write bytes after checking the approved resolution and rejecting a final
/// symlink. The write is performed through a fixed parent directory handle;
/// unlike [`atomic_write`], this preserves direct truncate/write semantics.
pub fn write_file_after_permission(
    root: &Path,
    relative: &Path,
    requested: &Path,
    approved: &Path,
    bytes: &[u8],
) -> Result<RootedWriteResult, RootedFsError> {
    imp::write_file_after_permission(root, relative, requested, approved, bytes)
}

/// Atomically write bytes without following any component below `root`.
pub fn atomic_write(
    root: &Path,
    relative: &Path,
    bytes: &[u8],
    options: AtomicWriteOptions,
) -> Result<(), FsError> {
    imp::atomic_write(root, relative, bytes, options)
}

/// Remove a file without following any component below `root`.
pub fn remove_file(root: &Path, relative: &Path) -> Result<(), FsError> {
    imp::remove_file(root, relative)
}

#[cfg(test)]
fn read_file_after_permission_for_test<F>(
    root: &Path,
    relative: &Path,
    requested: &Path,
    approved: &Path,
    after_permission: F,
) -> Result<RootedFileSnapshot, RootedFsError>
where
    F: FnOnce(),
{
    imp::read_file_after_permission_for_test(root, relative, requested, approved, after_permission)
}

#[cfg(test)]
fn write_file_after_permission_for_test<F>(
    root: &Path,
    relative: &Path,
    requested: &Path,
    approved: &Path,
    bytes: &[u8],
    after_permission: F,
) -> Result<RootedWriteResult, RootedFsError>
where
    F: FnOnce(),
{
    imp::write_file_after_permission_for_test(
        root,
        relative,
        requested,
        approved,
        bytes,
        after_permission,
    )
}

#[allow(clippy::needless_pass_by_value)]
fn map_io(path: &Path, error: std::io::Error) -> FsError {
    match error.kind() {
        std::io::ErrorKind::NotFound => FsError::NotFound(path.display().to_string()),
        std::io::ErrorKind::PermissionDenied => {
            FsError::PermissionDenied(path.display().to_string())
        }
        std::io::ErrorKind::AlreadyExists => FsError::AlreadyExists(path.display().to_string()),
        _ => FsError::Io(format!("{}: {error}", path.display())),
    }
}

#[cfg(unix)]
mod imp {
    use super::*;
    use rustix::fs::{self, AtFlags, FileType, Mode, OFlags};
    use std::ffi::{OsStr, OsString};
    use std::os::fd::OwnedFd;
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

    fn map_unix_io(path: &Path, error: rustix::io::Errno) -> FsError {
        if matches!(error, rustix::io::Errno::LOOP | rustix::io::Errno::NOTDIR) {
            FsError::OutsideWorkspace(path.display().to_string())
        } else {
            map_io(path, error.into())
        }
    }

    fn mode(bits: u32, path: &Path) -> Result<Mode, FsError> {
        let bits = u16::try_from(bits)
            .map_err(|_| FsError::Io(format!("{}: invalid unix mode {bits:o}", path.display())))?;
        Ok(Mode::from_bits_retain(bits.into()))
    }

    fn identity_from_fd(fd: &OwnedFd, path: &Path) -> Result<RootIdentity, FsError> {
        let stat = fs::fstat(fd).map_err(|error| map_unix_io(path, error))?;
        Ok(RootIdentity {
            volume: u64::try_from(stat.st_dev).map_err(|_| {
                FsError::Io(format!("{}: invalid directory device id", path.display()))
            })?,
            file: u64::try_from(stat.st_ino)
                .map_err(|_| FsError::Io(format!("{}: invalid directory inode", path.display())))?,
        })
    }

    #[cfg(any(test, target_os = "ios"))]
    pub(super) fn open_direct_root_checked(
        root: &Path,
        expected: Option<&RootIdentity>,
    ) -> Result<OwnedFd, FsError> {
        // An iOS app may open its own container directly, but its sandbox
        // rejects opening protected ancestors such as `/private/var/mobile`.
        // Reject every symlink in the supplied path before and after the open,
        // then prove the opened handle still names the resolved directory.
        let canonical_before = std::fs::canonicalize(root).map_err(|error| map_io(root, error))?;
        if canonical_before != root {
            return Err(FsError::OutsideWorkspace(root.display().to_string()));
        }
        let directory = fs::open(
            root,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(|error| map_unix_io(root, error))?;
        let canonical_after = std::fs::canonicalize(root).map_err(|error| map_io(root, error))?;
        if canonical_after != root {
            return Err(FsError::OutsideWorkspace(root.display().to_string()));
        }
        let opened = fs::fstat(&directory).map_err(|error| map_unix_io(root, error))?;
        let resolved = fs::stat(root).map_err(|error| map_unix_io(root, error))?;
        if opened.st_dev != resolved.st_dev || opened.st_ino != resolved.st_ino {
            return Err(FsError::OutsideWorkspace(format!(
                "root directory identity changed: {}",
                root.display()
            )));
        }
        if let Some(expected) = expected {
            if identity_from_fd(&directory, root)? != *expected {
                return Err(FsError::OutsideWorkspace(format!(
                    "root directory identity changed: {}",
                    root.display()
                )));
            }
        }
        Ok(directory)
    }

    fn open_root_checked(root: &Path, expected: Option<&RootIdentity>) -> Result<OwnedFd, FsError> {
        // Never canonicalize the root before opening it. Canonicalization
        // follows a swapped task-output directory (or one of its parents),
        // turning a safe-looking rooted operation into a write to the swap
        // target. Start from a fixed directory handle and walk every root
        // component with `openat(..., NOFOLLOW)` instead; this pins the
        // parent-directory chain for the operation.
        // Darwin exposes `/tmp`, `/var`, and `/etc` as fixed OS aliases to
        // `/private/*`. Resolve only those kernel-provided aliases before the
        // no-follow walk; arbitrary user/task symlinks still fail closed at
        // the component where they occur.
        #[cfg(any(target_os = "ios", target_os = "macos"))]
        let root_alias_free = {
            let mut components = root.components();
            match (components.next(), components.next()) {
                (Some(Component::RootDir), Some(Component::Normal(first))) => {
                    let mapped = match first.to_str() {
                        Some("etc") | Some("tmp") | Some("var") => {
                            Some(first.to_string_lossy().into_owned())
                        }
                        _ => None,
                    };
                    if let Some(mapped) = mapped {
                        let mut normalized = PathBuf::from("/private");
                        normalized.push(mapped);
                        for component in components {
                            normalized.push(component.as_os_str());
                        }
                        normalized
                    } else {
                        root.to_path_buf()
                    }
                }
                _ => root.to_path_buf(),
            }
        };
        #[cfg(not(any(target_os = "ios", target_os = "macos")))]
        let root_alias_free = root.to_path_buf();

        // iOS grants access to the app container itself without granting
        // directory traversal over its system-owned ancestors. Directly pin
        // an absolute, alias-free root after the checks above instead of
        // starting the walk at `/`, which the sandbox rejects with EACCES.
        #[cfg(target_os = "ios")]
        if root_alias_free.is_absolute() {
            return open_direct_root_checked(&root_alias_free, expected);
        }

        let mut directory = if root_alias_free.is_absolute() {
            fs::open(
                Path::new("/"),
                OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::empty(),
            )
            .map_err(|error| map_unix_io(Path::new("/"), error))?
        } else {
            fs::open(
                Path::new("."),
                OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::empty(),
            )
            .map_err(|error| map_unix_io(Path::new("."), error))?
        };

        for component in root_alias_free.components() {
            let name = match component {
                Component::RootDir | Component::CurDir => continue,
                Component::Normal(name) => name,
                Component::ParentDir | Component::Prefix(_) => {
                    return Err(FsError::OutsideWorkspace(root.display().to_string()));
                }
            };
            directory = fs::openat(
                &directory,
                name,
                OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::empty(),
            )
            .map_err(|error| map_unix_io(root, error))?;
        }
        if let Some(expected) = expected {
            if identity_from_fd(&directory, root)? != *expected {
                return Err(FsError::OutsideWorkspace(format!(
                    "root directory identity changed: {}",
                    root.display()
                )));
            }
        }
        Ok(directory)
    }

    fn open_root(root: &Path) -> Result<OwnedFd, FsError> {
        open_root_checked(root, None)
    }

    pub(super) fn root_identity(root: &Path) -> Result<RootIdentity, FsError> {
        let directory = open_root(root)?;
        identity_from_fd(&directory, root)
    }

    pub(super) fn ensure_private_directory(
        root: &Path,
        relative: &Path,
        dir_mode: u32,
    ) -> Result<RootIdentity, FsError> {
        let (parent, directory_name) = open_parent(root, relative, true, dir_mode)?;
        match fs::mkdirat(&parent, &directory_name, mode(dir_mode, relative)?) {
            Ok(()) => {}
            Err(error) if error == rustix::io::Errno::EXIST => {}
            Err(error) => return Err(map_unix_io(relative, error)),
        }
        let directory = fs::openat(
            &parent,
            &directory_name,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(|error| map_unix_io(relative, error))?;
        fs::fchmod(&directory, mode(dir_mode, relative)?)
            .map_err(|error| map_unix_io(relative, error))?;
        identity_from_fd(&directory, relative)
    }

    fn split(relative: &Path) -> Result<(Vec<OsString>, OsString), FsError> {
        validate_relative_path(relative)?;
        let mut parts: Vec<_> = relative
            .components()
            .filter_map(|component| match component {
                Component::Normal(value) => Some(value.to_os_string()),
                _ => None,
            })
            .collect();
        let file = parts
            .pop()
            .ok_or_else(|| FsError::OutsideWorkspace(relative.display().to_string()))?;
        Ok((parts, file))
    }

    fn open_parent(
        root: &Path,
        relative: &Path,
        create: bool,
        dir_mode: u32,
    ) -> Result<(OwnedFd, OsString), FsError> {
        open_parent_checked(root, relative, create, dir_mode, None)
    }

    fn open_parent_checked(
        root: &Path,
        relative: &Path,
        create: bool,
        dir_mode: u32,
        expected: Option<&RootIdentity>,
    ) -> Result<(OwnedFd, OsString), FsError> {
        let (parents, file) = split(relative)?;
        let mut directory = open_root_checked(root, expected)?;
        for component in parents {
            if create {
                match fs::mkdirat(&directory, &component, mode(dir_mode, relative)?) {
                    Ok(()) => {}
                    Err(error) if error == rustix::io::Errno::EXIST => {}
                    Err(error) => return Err(map_unix_io(relative, error)),
                }
            }
            directory = fs::openat(
                &directory,
                &component,
                OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::empty(),
            )
            .map_err(|error| map_unix_io(relative, error))?;
        }
        Ok((directory, file))
    }

    fn ensure_opened_regular(fd: &OwnedFd, relative: &Path) -> Result<(), FsError> {
        let stat = fs::fstat(fd).map_err(|error| map_unix_io(relative, error))?;
        if FileType::from_raw_mode(stat.st_mode) == FileType::RegularFile {
            Ok(())
        } else {
            Err(FsError::OutsideWorkspace(relative.display().to_string()))
        }
    }

    fn validate_optional_regular(
        parent: &OwnedFd,
        file: &OsStr,
        relative: &Path,
    ) -> Result<(), FsError> {
        match fs::statat(parent, file, AtFlags::SYMLINK_NOFOLLOW) {
            Ok(stat) if FileType::from_raw_mode(stat.st_mode) == FileType::RegularFile => Ok(()),
            Ok(_) => Err(FsError::OutsideWorkspace(relative.display().to_string())),
            Err(rustix::io::Errno::NOENT) => Ok(()),
            Err(error) => Err(map_unix_io(relative, error)),
        }
    }

    pub(super) fn lock_exclusive(
        root: &Path,
        relative: &Path,
        dir_mode: u32,
        file_mode: u32,
    ) -> Result<RootedFileLock, FsError> {
        let (parent, file_name) = open_parent(root, relative, true, dir_mode)?;
        let fd = fs::openat(
            &parent,
            &file_name,
            OFlags::RDWR | OFlags::CREATE | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            mode(file_mode, relative)?,
        )
        .map_err(|error| map_unix_io(relative, error))?;
        ensure_opened_regular(&fd, relative)?;
        let file = std::fs::File::from(fd);
        file.lock_exclusive()
            .map_err(|error| map_io(relative, error))?;
        Ok(RootedFileLock {
            _file: file,
            path: root.join(relative).display().to_string(),
        })
    }

    pub(super) fn read_to_string(root: &Path, relative: &Path) -> Result<String, FsError> {
        let (parent, file_name) = open_parent(root, relative, false, PRIVATE_DIR_MODE)?;
        let fd = fs::openat(
            &parent,
            &file_name,
            OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(|error| map_unix_io(relative, error))?;
        ensure_opened_regular(&fd, relative)?;
        let mut file = std::fs::File::from(fd);
        let mut body = String::new();
        file.read_to_string(&mut body)
            .map_err(|error| map_io(relative, error))?;
        Ok(body)
    }

    pub(super) fn read_to_string_limited(
        root: &Path,
        relative: &Path,
        max_bytes: u64,
    ) -> Result<String, FsError> {
        let (parent, file_name) = open_parent(root, relative, false, PRIVATE_DIR_MODE)?;
        let fd = fs::openat(
            &parent,
            &file_name,
            OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(|error| map_unix_io(relative, error))?;
        ensure_opened_regular(&fd, relative)?;
        read_opened_to_string_limited(std::fs::File::from(fd), relative, max_bytes)
    }

    pub(super) fn read_tail_bytes(
        root: &Path,
        relative: &Path,
        max_bytes: u64,
    ) -> Result<Vec<u8>, FsError> {
        let (parent, file_name) = open_parent(root, relative, false, PRIVATE_DIR_MODE)?;
        let fd = fs::openat(
            &parent,
            &file_name,
            OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(|error| map_unix_io(relative, error))?;
        ensure_opened_regular(&fd, relative)?;
        read_opened_tail_bytes(std::fs::File::from(fd), relative, max_bytes)
    }

    pub(super) fn create_new_file(root: &Path, relative: &Path) -> Result<(), FsError> {
        create_new_file_pinned(root, relative, None)
    }

    pub(super) fn create_new_file_pinned(
        root: &Path,
        relative: &Path,
        expected: Option<&RootIdentity>,
    ) -> Result<(), FsError> {
        drop(open_create_new_file_pinned(root, relative, expected)?);
        Ok(())
    }

    pub(super) fn open_create_new_file_pinned(
        root: &Path,
        relative: &Path,
        expected: Option<&RootIdentity>,
    ) -> Result<std::fs::File, FsError> {
        let (parent, file_name) =
            open_parent_checked(root, relative, false, PRIVATE_DIR_MODE, expected)?;
        let fd = fs::openat(
            &parent,
            &file_name,
            OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            mode(PRIVATE_FILE_MODE, relative)?,
        )
        .map_err(|error| map_unix_io(relative, error))?;
        ensure_opened_regular(&fd, relative)?;
        fs::fchmod(&fd, mode(PRIVATE_FILE_MODE, relative)?)
            .map_err(|error| map_unix_io(relative, error))?;
        Ok(std::fs::File::from(fd))
    }

    pub(super) fn append_file(root: &Path, relative: &Path, content: &str) -> Result<(), FsError> {
        append_file_bytes_pinned(root, relative, content.as_bytes(), None)
    }

    pub(super) fn append_file_bytes(
        root: &Path,
        relative: &Path,
        content: &[u8],
    ) -> Result<(), FsError> {
        append_file_bytes_pinned(root, relative, content, None)
    }

    pub(super) fn append_file_pinned(
        root: &Path,
        relative: &Path,
        content: &str,
        expected: Option<&RootIdentity>,
    ) -> Result<(), FsError> {
        append_file_bytes_pinned(root, relative, content.as_bytes(), expected)
    }

    fn append_file_bytes_pinned(
        root: &Path,
        relative: &Path,
        content: &[u8],
        expected: Option<&RootIdentity>,
    ) -> Result<(), FsError> {
        let (parent, file_name) =
            open_parent_checked(root, relative, false, PRIVATE_DIR_MODE, expected)?;
        let fd = fs::openat(
            &parent,
            &file_name,
            OFlags::WRONLY | OFlags::APPEND | OFlags::CREATE | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            mode(PRIVATE_FILE_MODE, relative)?,
        )
        .map_err(|error| map_unix_io(relative, error))?;
        ensure_opened_regular(&fd, relative)?;
        let mut file = std::fs::File::from(fd);
        file.write_all(content)
            .map_err(|error| map_io(relative, error))?;
        file.flush().map_err(|error| map_io(relative, error))
    }

    pub(super) fn open_append_file_pinned(
        root: &Path,
        relative: &Path,
        expected: Option<&RootIdentity>,
    ) -> Result<std::fs::File, FsError> {
        let (parent, file_name) =
            open_parent_checked(root, relative, false, PRIVATE_DIR_MODE, expected)?;
        let fd = fs::openat(
            &parent,
            &file_name,
            OFlags::WRONLY | OFlags::APPEND | OFlags::CREATE | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            mode(PRIVATE_FILE_MODE, relative)?,
        )
        .map_err(|error| map_unix_io(relative, error))?;
        ensure_opened_regular(&fd, relative)?;
        fs::fchmod(&fd, mode(PRIVATE_FILE_MODE, relative)?)
            .map_err(|error| map_unix_io(relative, error))?;
        Ok(std::fs::File::from(fd))
    }

    pub(super) fn read_to_string_pinned(
        root: &Path,
        relative: &Path,
        expected: Option<&RootIdentity>,
    ) -> Result<String, FsError> {
        let (parent, file_name) =
            open_parent_checked(root, relative, false, PRIVATE_DIR_MODE, expected)?;
        let fd = fs::openat(
            &parent,
            &file_name,
            OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(|error| map_unix_io(relative, error))?;
        ensure_opened_regular(&fd, relative)?;
        let mut file = std::fs::File::from(fd);
        let mut body = String::new();
        file.read_to_string(&mut body)
            .map_err(|error| map_io(relative, error))?;
        Ok(body)
    }

    fn temp_name(final_name: &OsStr) -> OsString {
        let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let mut name = OsString::from(".");
        name.push(final_name);
        name.push(format!(".tmp-{}-{sequence}", std::process::id()));
        name
    }

    pub(super) fn atomic_write(
        root: &Path,
        relative: &Path,
        bytes: &[u8],
        options: AtomicWriteOptions,
    ) -> Result<(), FsError> {
        let (parent, file_name) =
            open_parent(root, relative, options.create_parents, options.dir_mode)?;
        validate_optional_regular(&parent, &file_name, relative)?;
        let temp_name = temp_name(&file_name);
        let temp_fd = fs::openat(
            &parent,
            &temp_name,
            OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            mode(options.file_mode, relative)?,
        )
        .map_err(|error| map_unix_io(relative, error))?;
        let mut temp = std::fs::File::from(temp_fd);
        let result = (|| {
            temp.write_all(bytes)
                .map_err(|error| map_io(relative, error))?;
            temp.sync_all().map_err(|error| map_io(relative, error))?;
            validate_optional_regular(&parent, &file_name, relative)?;
            if options.overwrite {
                fs::renameat(&parent, &temp_name, &parent, &file_name)
                    .map_err(|error| map_unix_io(relative, error))?;
            } else {
                fs::linkat(&parent, &temp_name, &parent, &file_name, AtFlags::empty())
                    .map_err(|error| map_unix_io(relative, error))?;
                let _ = fs::unlinkat(&parent, &temp_name, AtFlags::empty());
            }
            fs::fsync(&parent).map_err(|error| map_unix_io(relative, error))?;
            Ok(())
        })();
        if result.is_err() {
            let _ = fs::unlinkat(&parent, &temp_name, AtFlags::empty());
        }
        result
    }

    fn read_file_after_permission_inner<F>(
        root: &Path,
        relative: &Path,
        requested: &Path,
        approved: &Path,
        after_permission: F,
    ) -> Result<RootedFileSnapshot, RootedFsError>
    where
        F: FnOnce(),
    {
        let target_exists = std::fs::symlink_metadata(requested).is_ok();
        if target_exists {
            verify_resolution(requested, approved)?;
        } else {
            verify_parent_resolution(requested, approved)?;
        }
        after_permission();
        // Re-check before resolving the fixed parent fd, then again after the
        // leaf fd is open. The latter closes the permission-check → read gap.
        if target_exists {
            verify_resolution(requested, approved)?;
        } else {
            verify_parent_resolution(requested, approved)?;
        }
        let (parent, file_name) =
            open_parent(root, relative, false, PRIVATE_DIR_MODE).map_err(RootedFsError::Fs)?;
        // Stat without following the final component before opening. Besides
        // rejecting non-regular files, this keeps a FIFO from blocking the
        // synchronous `openat(O_RDONLY)` below while waiting for a writer.
        ensure_read_regular(&parent, &file_name, relative)?;
        let fd = fs::openat(
            &parent,
            &file_name,
            OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(|error| RootedFsError::Fs(map_unix_io(relative, error)))?;
        ensure_opened_regular(&fd, relative).map_err(RootedFsError::Fs)?;
        if target_exists {
            verify_resolution(requested, approved)?;
        } else {
            verify_parent_resolution(requested, approved)?;
        }
        let file = std::fs::File::from(fd);
        let metadata = file
            .metadata()
            .map_err(|error| RootedFsError::Fs(map_io(relative, error)))?;
        let mut bytes = Vec::new();
        let mut reader = file;
        reader
            .read_to_end(&mut bytes)
            .map_err(|error| RootedFsError::Fs(map_io(relative, error)))?;
        Ok(RootedFileSnapshot {
            size: metadata.len(),
            modified: metadata.modified().ok(),
            mode: Some({
                use std::os::unix::fs::MetadataExt;
                metadata.mode()
            }),
            bytes,
        })
    }

    pub(super) fn read_file_after_permission(
        root: &Path,
        relative: &Path,
        requested: &Path,
        approved: &Path,
    ) -> Result<RootedFileSnapshot, RootedFsError> {
        read_file_after_permission_inner(root, relative, requested, approved, || {})
    }

    pub(super) fn open_file_after_permission(
        root: &Path,
        relative: &Path,
        requested: &Path,
        approved: &Path,
    ) -> Result<std::fs::File, RootedFsError> {
        let target_exists = std::fs::symlink_metadata(requested).is_ok();
        if target_exists {
            verify_resolution(requested, approved)?;
        } else {
            verify_parent_resolution(requested, approved)?;
        }
        let (parent, file_name) =
            open_parent(root, relative, false, PRIVATE_DIR_MODE).map_err(RootedFsError::Fs)?;
        ensure_read_regular(&parent, &file_name, relative)?;
        let fd = fs::openat(
            &parent,
            &file_name,
            OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(|error| RootedFsError::Fs(map_unix_io(relative, error)))?;
        ensure_opened_regular(&fd, relative).map_err(RootedFsError::Fs)?;
        if target_exists {
            verify_resolution(requested, approved)?;
        } else {
            verify_parent_resolution(requested, approved)?;
        }
        Ok(std::fs::File::from(fd))
    }

    #[cfg(test)]
    pub(super) fn read_file_after_permission_for_test<F>(
        root: &Path,
        relative: &Path,
        requested: &Path,
        approved: &Path,
        after_permission: F,
    ) -> Result<RootedFileSnapshot, RootedFsError>
    where
        F: FnOnce(),
    {
        read_file_after_permission_inner(root, relative, requested, approved, after_permission)
    }

    fn verify_leaf_at(
        parent: &OwnedFd,
        file_name: &OsStr,
        relative: &Path,
    ) -> Result<(), RootedFsError> {
        match fs::statat(parent, file_name, AtFlags::SYMLINK_NOFOLLOW) {
            Ok(stat) if FileType::from_raw_mode(stat.st_mode) == FileType::Symlink => {
                Err(RootedFsError::LeafSymlink)
            }
            Ok(stat) if FileType::from_raw_mode(stat.st_mode) == FileType::RegularFile => Ok(()),
            Ok(_) => Err(RootedFsError::NotRegularFile),
            Err(rustix::io::Errno::NOENT) => Ok(()),
            Err(error) => Err(RootedFsError::Fs(map_unix_io(relative, error))),
        }
    }

    fn ensure_read_regular(
        parent: &OwnedFd,
        file_name: &OsStr,
        relative: &Path,
    ) -> Result<(), RootedFsError> {
        match fs::statat(parent, file_name, AtFlags::SYMLINK_NOFOLLOW) {
            Ok(stat) if FileType::from_raw_mode(stat.st_mode) == FileType::RegularFile => Ok(()),
            Ok(_) => Err(RootedFsError::NotRegularFile),
            Err(rustix::io::Errno::NOENT) => Err(RootedFsError::Fs(FsError::NotFound(
                relative.display().to_string(),
            ))),
            Err(error) => Err(RootedFsError::Fs(map_unix_io(relative, error))),
        }
    }

    fn write_file_after_permission_inner<F>(
        root: &Path,
        relative: &Path,
        requested: &Path,
        approved: &Path,
        bytes: &[u8],
        after_permission: F,
    ) -> Result<RootedWriteResult, RootedFsError>
    where
        F: FnOnce(),
    {
        let target_exists = std::fs::symlink_metadata(requested).is_ok();
        if target_exists {
            verify_resolution(requested, approved)?;
            verify_leaf_not_symlink(requested)?;
        } else {
            verify_parent_resolution(requested, approved)?;
        }
        after_permission();
        // Missing parents are materialized by the fixed no-follow directory
        // handle chain. Never create them through the requested pathname:
        // doing so would reopen a retargetable ancestor before rooted I/O.
        // `create_dir_all` used by FileWriteTool applies the process umask to
        // 0777, so retain that mode while changing only the resolution
        // primitive to rooted mkdirat.
        let (parent, file_name) =
            open_parent(root, relative, true, 0o777).map_err(RootedFsError::Fs)?;
        verify_parent_resolution(requested, approved)?;
        verify_leaf_at(&parent, &file_name, relative)?;
        let fd = fs::openat(
            &parent,
            &file_name,
            OFlags::WRONLY | OFlags::CREATE | OFlags::TRUNC | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::from_bits_retain(0o666),
        )
        .map_err(|error| {
            if error == rustix::io::Errno::LOOP {
                RootedFsError::LeafSymlink
            } else {
                RootedFsError::Fs(map_unix_io(relative, error))
            }
        })?;
        ensure_opened_regular(&fd, relative).map_err(RootedFsError::Fs)?;
        if target_exists {
            verify_resolution(requested, approved)?;
        } else {
            verify_parent_resolution(requested, approved)?;
        }
        let mut file = std::fs::File::from(fd);
        file.write_all(bytes)
            .map_err(|error| RootedFsError::Fs(map_io(relative, error)))?;
        // A parent swap after the fixed dirfd was opened cannot redirect this
        // write, but still fails closed so callers never report success for a
        // path whose approved resolution changed during the operation.
        verify_parent_resolution(requested, approved)?;
        if target_exists {
            verify_resolution(requested, approved)?;
        }
        Ok(RootedWriteResult {
            modified: file
                .metadata()
                .ok()
                .and_then(|metadata| metadata.modified().ok()),
        })
    }

    pub(super) fn write_file_after_permission(
        root: &Path,
        relative: &Path,
        requested: &Path,
        approved: &Path,
        bytes: &[u8],
    ) -> Result<RootedWriteResult, RootedFsError> {
        write_file_after_permission_inner(root, relative, requested, approved, bytes, || {})
    }

    #[cfg(test)]
    pub(super) fn write_file_after_permission_for_test<F>(
        root: &Path,
        relative: &Path,
        requested: &Path,
        approved: &Path,
        bytes: &[u8],
        after_permission: F,
    ) -> Result<RootedWriteResult, RootedFsError>
    where
        F: FnOnce(),
    {
        write_file_after_permission_inner(
            root,
            relative,
            requested,
            approved,
            bytes,
            after_permission,
        )
    }

    pub(super) fn remove_file(root: &Path, relative: &Path) -> Result<(), FsError> {
        let (parent, file_name) = open_parent(root, relative, false, PRIVATE_DIR_MODE)?;
        fs::unlinkat(&parent, &file_name, AtFlags::empty())
            .map_err(|error| map_unix_io(relative, error))
    }
}

#[cfg(all(not(unix), not(windows)))]
mod imp {
    use super::*;

    fn unsupported() -> FsError {
        FsError::Io("root-confined filesystem operations are unsupported on this platform".into())
    }

    pub(super) fn root_identity(_root: &Path) -> Result<RootIdentity, FsError> {
        Err(unsupported())
    }

    pub(super) fn ensure_private_directory(
        _root: &Path,
        _relative: &Path,
        _dir_mode: u32,
    ) -> Result<RootIdentity, FsError> {
        Err(unsupported())
    }

    pub(super) fn lock_exclusive(
        _root: &Path,
        _relative: &Path,
        _dir_mode: u32,
        _file_mode: u32,
    ) -> Result<RootedFileLock, FsError> {
        Err(unsupported())
    }

    pub(super) fn read_file_after_permission(
        _root: &Path,
        _relative: &Path,
        _requested: &Path,
        _approved: &Path,
    ) -> Result<RootedFileSnapshot, RootedFsError> {
        Err(RootedFsError::Fs(unsupported()))
    }

    pub(super) fn open_file_after_permission(
        _root: &Path,
        _relative: &Path,
        _requested: &Path,
        _approved: &Path,
    ) -> Result<std::fs::File, RootedFsError> {
        Err(RootedFsError::Fs(unsupported()))
    }

    #[cfg(test)]
    pub(super) fn read_file_after_permission_for_test<F>(
        _root: &Path,
        _relative: &Path,
        _requested: &Path,
        _approved: &Path,
        _after_permission: F,
    ) -> Result<RootedFileSnapshot, RootedFsError>
    where
        F: FnOnce(),
    {
        Err(RootedFsError::Fs(unsupported()))
    }

    pub(super) fn read_to_string(_root: &Path, _relative: &Path) -> Result<String, FsError> {
        Err(unsupported())
    }

    pub(super) fn read_to_string_limited(
        _root: &Path,
        _relative: &Path,
        _max_bytes: u64,
    ) -> Result<String, FsError> {
        Err(unsupported())
    }

    pub(super) fn read_tail_bytes(
        _root: &Path,
        _relative: &Path,
        _max_bytes: u64,
    ) -> Result<Vec<u8>, FsError> {
        Err(unsupported())
    }

    pub(super) fn atomic_write(
        _root: &Path,
        _relative: &Path,
        _bytes: &[u8],
        _options: AtomicWriteOptions,
    ) -> Result<(), FsError> {
        Err(unsupported())
    }

    pub(super) fn create_new_file(_root: &Path, _relative: &Path) -> Result<(), FsError> {
        Err(unsupported())
    }

    pub(super) fn create_new_file_pinned(
        _root: &Path,
        _relative: &Path,
        _expected: Option<&RootIdentity>,
    ) -> Result<(), FsError> {
        Err(unsupported())
    }

    pub(super) fn append_file(
        _root: &Path,
        _relative: &Path,
        _content: &str,
    ) -> Result<(), FsError> {
        Err(unsupported())
    }

    pub(super) fn append_file_bytes(
        _root: &Path,
        _relative: &Path,
        _content: &[u8],
    ) -> Result<(), FsError> {
        Err(unsupported())
    }

    pub(super) fn append_file_pinned(
        _root: &Path,
        _relative: &Path,
        _content: &str,
        _expected: Option<&RootIdentity>,
    ) -> Result<(), FsError> {
        Err(unsupported())
    }

    pub(super) fn open_append_file_pinned(
        _root: &Path,
        _relative: &Path,
        _expected: Option<&RootIdentity>,
    ) -> Result<std::fs::File, FsError> {
        Err(unsupported())
    }

    pub(super) fn read_to_string_pinned(
        _root: &Path,
        _relative: &Path,
        _expected: Option<&RootIdentity>,
    ) -> Result<String, FsError> {
        Err(unsupported())
    }

    pub(super) fn write_file_after_permission(
        _root: &Path,
        _relative: &Path,
        _requested: &Path,
        _approved: &Path,
        _bytes: &[u8],
    ) -> Result<RootedWriteResult, RootedFsError> {
        Err(RootedFsError::Fs(unsupported()))
    }

    #[cfg(test)]
    pub(super) fn write_file_after_permission_for_test<F>(
        _root: &Path,
        _relative: &Path,
        _requested: &Path,
        _approved: &Path,
        _bytes: &[u8],
        _after_permission: F,
    ) -> Result<RootedWriteResult, RootedFsError>
    where
        F: FnOnce(),
    {
        Err(RootedFsError::Fs(unsupported()))
    }

    pub(super) fn remove_file(_root: &Path, _relative: &Path) -> Result<(), FsError> {
        Err(unsupported())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_non_relative_paths() {
        assert!(validate_relative_path(Path::new("../escape")).is_err());
        assert!(validate_relative_path(Path::new("a/../escape")).is_err());
        assert!(validate_relative_path(Path::new("/absolute")).is_err());
        assert!(validate_relative_path(Path::new("a/b")).is_ok());
    }

    #[cfg(unix)]
    #[test]
    fn refuses_symlinked_parent_and_does_not_touch_victim() {
        let root = tempfile::tempdir().unwrap();
        let victim = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(victim.path(), root.path().join(".lingxi")).unwrap();
        assert!(atomic_write(
            root.path(),
            Path::new(".lingxi/state.json"),
            b"owned",
            AtomicWriteOptions::default(),
        )
        .is_err());
        assert!(!victim.path().join("state.json").exists());
    }

    #[cfg(unix)]
    #[test]
    fn rooted_task_output_rejects_a_swapped_root_symlink() {
        let parent = tempfile::tempdir().unwrap();
        let victim = tempfile::tempdir().unwrap();
        let task_dir = parent.path().join("tasks");
        std::os::unix::fs::symlink(victim.path(), &task_dir).unwrap();

        let create = create_new_file(&task_dir, Path::new("task.output"));
        assert!(
            create.is_err(),
            "a symlinked task directory must be refused"
        );
        assert!(!victim.path().join("task.output").exists());

        let append = append_file(&task_dir, Path::new("task.output"), "must not escape");
        assert!(append.is_err(), "append must use the same root pin");
        assert!(!victim.path().join("task.output").exists());
    }

    #[cfg(unix)]
    #[test]
    fn rooted_task_output_walk_rejects_a_symlinked_ancestor() {
        let parent = tempfile::tempdir().unwrap();
        let victim = tempfile::tempdir().unwrap();
        let swapped_parent = parent.path().join("runtime");
        std::os::unix::fs::symlink(victim.path(), &swapped_parent).unwrap();
        let task_dir = swapped_parent.join("tasks");

        let result = create_new_file(&task_dir, Path::new("task.output"));
        assert!(result.is_err(), "a symlinked ancestor must be refused");
        assert!(!victim.path().join("tasks/task.output").exists());
    }

    #[cfg(unix)]
    #[test]
    fn direct_root_open_accepts_a_real_root_and_rejects_a_symlinked_ancestor() {
        let real = tempfile::tempdir().unwrap();
        let canonical_real = std::fs::canonicalize(real.path()).unwrap();
        drop(imp::open_direct_root_checked(&canonical_real, None).unwrap());

        let parent = tempfile::tempdir().unwrap();
        let victim = tempfile::tempdir().unwrap();
        std::fs::create_dir(victim.path().join("nested")).unwrap();
        let linked_parent = parent.path().join("linked");
        std::os::unix::fs::symlink(victim.path(), &linked_parent).unwrap();
        let linked_root = linked_parent.join("nested");

        assert!(matches!(
            imp::open_direct_root_checked(&linked_root, None),
            Err(FsError::OutsideWorkspace(_))
        ));
    }

    #[test]
    fn pinned_root_rejects_same_path_directory_replacement() {
        let parent = tempfile::tempdir().unwrap();
        let task_dir = parent.path().join("tasks");
        std::fs::create_dir(&task_dir).unwrap();
        let identity = root_identity(&task_dir).unwrap();

        std::fs::rename(&task_dir, parent.path().join("tasks-old")).unwrap();
        std::fs::create_dir(&task_dir).unwrap();

        let result = create_new_file_pinned(&task_dir, Path::new("task.output"), Some(&identity));
        assert!(result.is_err(), "a replacement directory must fail the pin");
        assert!(!task_dir.join("task.output").exists());
    }

    #[test]
    fn private_directory_append_handle_rejects_root_replacement() {
        let parent = tempfile::tempdir().unwrap();
        let relative_dir = Path::new("tasks");
        let task_dir = parent.path().join(relative_dir);
        let identity =
            ensure_private_directory(parent.path(), relative_dir, PRIVATE_DIR_MODE).unwrap();
        let mut file =
            open_append_file_pinned(&task_dir, Path::new("task.output"), Some(&identity)).unwrap();
        file.write_all(b"first").unwrap();
        drop(file);

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            assert_eq!(
                std::fs::symlink_metadata(&task_dir)
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                PRIVATE_DIR_MODE
            );
            assert_eq!(
                std::fs::symlink_metadata(task_dir.join("task.output"))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                PRIVATE_FILE_MODE
            );
        }

        std::fs::rename(&task_dir, parent.path().join("tasks-old")).unwrap();
        std::fs::create_dir(&task_dir).unwrap();
        let result =
            open_append_file_pinned(&task_dir, Path::new("second.output"), Some(&identity));
        assert!(result.is_err(), "a replacement directory must fail the pin");
        assert!(!task_dir.join("second.output").exists());
    }

    #[cfg(unix)]
    #[test]
    fn private_directory_creation_refuses_symlink_leaf() {
        let parent = tempfile::tempdir().unwrap();
        let victim = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(victim.path(), parent.path().join("tasks")).unwrap();

        let result = ensure_private_directory(parent.path(), Path::new("tasks"), PRIVATE_DIR_MODE);
        assert!(result.is_err(), "a symlinked output root must be refused");
        assert!(victim.path().read_dir().unwrap().next().is_none());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn pinned_root_accepts_darwin_tmp_alias() {
        let task_dir = tempfile::tempdir_in("/tmp").unwrap();
        let identity = root_identity(task_dir.path()).unwrap();
        create_new_file_pinned(task_dir.path(), Path::new("task.output"), Some(&identity)).unwrap();
        assert!(task_dir.path().join("task.output").exists());
    }

    #[cfg(unix)]
    #[test]
    fn overwrite_rejects_final_symlink_without_touching_victim() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir(root.path().join(".lingxi")).unwrap();
        let victim = root.path().join("victim.json");
        std::fs::write(&victim, "victim").unwrap();
        let target = root.path().join(".lingxi/state.json");
        std::os::unix::fs::symlink(&victim, &target).unwrap();
        assert!(atomic_write(
            root.path(),
            Path::new(".lingxi/state.json"),
            b"new",
            AtomicWriteOptions::default(),
        )
        .is_err());
        assert_eq!(std::fs::read_to_string(&victim).unwrap(), "victim");
        assert!(std::fs::symlink_metadata(&target)
            .unwrap()
            .file_type()
            .is_symlink());
    }

    #[cfg(unix)]
    #[test]
    fn stable_leaf_symlink_read_uses_approved_target() {
        let root = tempfile::tempdir().unwrap();
        let root_path = std::fs::canonicalize(root.path()).unwrap();
        let real = root.path().join("real");
        std::fs::create_dir(&real).unwrap();
        let target = real.join("state.txt");
        std::fs::write(&target, "approved").unwrap();
        let requested = root.path().join("state-link.txt");
        std::os::unix::fs::symlink(&target, &requested).unwrap();
        let approved = std::fs::canonicalize(&requested).unwrap();
        let relative = approved.strip_prefix(&root_path).unwrap();

        let snapshot = read_file_after_permission(root.path(), relative, &requested, &approved)
            .expect("a stable, in-root leaf symlink remains readable");
        assert_eq!(snapshot.bytes, b"approved");
    }

    #[cfg(unix)]
    #[test]
    fn read_fails_closed_when_leaf_symlink_is_retargeted_after_permission() {
        let root = tempfile::tempdir().unwrap();
        let root_path = std::fs::canonicalize(root.path()).unwrap();
        let real = root.path().join("real");
        let victim = tempfile::tempdir().unwrap();
        std::fs::create_dir(&real).unwrap();
        std::fs::write(real.join("state.txt"), "approved").unwrap();
        std::fs::write(victim.path().join("state.txt"), "victim").unwrap();
        let requested = root.path().join("state-link.txt");
        std::os::unix::fs::symlink(real.join("state.txt"), &requested).unwrap();
        let approved = std::fs::canonicalize(&requested).unwrap();
        let relative = approved.strip_prefix(&root_path).unwrap();

        let result = read_file_after_permission_for_test(
            root.path(),
            relative,
            &requested,
            &approved,
            || {
                std::fs::remove_file(&requested).unwrap();
                std::os::unix::fs::symlink(victim.path().join("state.txt"), &requested).unwrap();
            },
        );
        assert!(matches!(
            result,
            Err(RootedFsError::SymlinkResolutionChanged)
        ));
        assert_eq!(
            std::fs::read_to_string(victim.path().join("state.txt")).unwrap(),
            "victim"
        );
    }

    #[cfg(unix)]
    #[test]
    fn write_fails_closed_when_parent_symlink_is_retargeted_after_permission() {
        let root = tempfile::tempdir().unwrap();
        let root_path = std::fs::canonicalize(root.path()).unwrap();
        let real = root.path().join("real");
        let victim = tempfile::tempdir().unwrap();
        std::fs::create_dir(&real).unwrap();
        std::fs::write(real.join("state.txt"), "approved").unwrap();
        let parent = root.path().join("active");
        std::os::unix::fs::symlink(&real, &parent).unwrap();
        let requested = parent.join("state.txt");
        let approved = std::fs::canonicalize(&requested).unwrap();
        let relative = approved.strip_prefix(&root_path).unwrap();

        let result = write_file_after_permission_for_test(
            root.path(),
            relative,
            &requested,
            &approved,
            b"attacker-controlled",
            || {
                std::fs::remove_file(&parent).unwrap();
                std::os::unix::fs::symlink(victim.path(), &parent).unwrap();
            },
        );
        assert!(matches!(
            result,
            Err(RootedFsError::ParentSymlinkResolutionChanged)
        ));
        assert_eq!(
            std::fs::read_to_string(real.join("state.txt")).unwrap(),
            "approved"
        );
        assert!(!victim.path().join("state.txt").exists());
    }

    #[cfg(unix)]
    #[test]
    fn write_missing_parent_swap_is_refused_before_rooted_mkdir() {
        let root = tempfile::tempdir().unwrap();
        let root_path = std::fs::canonicalize(root.path()).unwrap();
        let approved_dir = root.path().join("approved");
        let victim = tempfile::tempdir().unwrap();
        std::fs::create_dir(&approved_dir).unwrap();
        let parent = root.path().join("active");
        std::os::unix::fs::symlink(&approved_dir, &parent).unwrap();
        let requested = parent.join("new").join("state.txt");
        let approved = std::fs::canonicalize(&approved_dir)
            .unwrap()
            .join("new")
            .join("state.txt");
        let relative = approved.strip_prefix(&root_path).unwrap();

        let result = write_file_after_permission_for_test(
            root.path(),
            relative,
            &requested,
            &approved,
            b"attacker-controlled",
            || {
                std::fs::remove_file(&parent).unwrap();
                std::os::unix::fs::symlink(victim.path(), &parent).unwrap();
            },
        );
        assert!(matches!(
            result,
            Err(RootedFsError::ParentSymlinkResolutionChanged)
        ));
        assert!(!victim.path().join("new").join("state.txt").exists());
        assert!(!approved_dir.join("new").join("state.txt").exists());
    }

    #[cfg(unix)]
    #[test]
    fn write_missing_parent_through_stable_symlink_succeeds() {
        let root = tempfile::tempdir().unwrap();
        let root_path = std::fs::canonicalize(root.path()).unwrap();
        let approved_dir = root.path().join("approved");
        std::fs::create_dir(&approved_dir).unwrap();
        let parent = root.path().join("active");
        std::os::unix::fs::symlink(&approved_dir, &parent).unwrap();
        let requested = parent.join("new").join("state.txt");
        let approved = std::fs::canonicalize(&approved_dir)
            .unwrap()
            .join("new")
            .join("state.txt");
        let relative = approved.strip_prefix(&root_path).unwrap();

        write_file_after_permission(root.path(), relative, &requested, &approved, b"confined")
            .expect("stable parent symlink should preserve the approved target");
        assert_eq!(
            std::fs::read_to_string(approved_dir.join("new").join("state.txt")).unwrap(),
            "confined"
        );
    }

    #[cfg(unix)]
    #[test]
    fn write_rejects_stable_leaf_symlink_without_touching_victim() {
        let root = tempfile::tempdir().unwrap();
        let root_path = std::fs::canonicalize(root.path()).unwrap();
        let real = root.path().join("real.txt");
        let victim = root.path().join("victim.txt");
        std::fs::write(&real, "approved").unwrap();
        std::fs::write(&victim, "victim").unwrap();
        let requested = root.path().join("state-link.txt");
        std::os::unix::fs::symlink(&real, &requested).unwrap();
        let approved = std::fs::canonicalize(&requested).unwrap();
        let relative = approved.strip_prefix(&root_path).unwrap();

        let result =
            write_file_after_permission(root.path(), relative, &requested, &approved, b"new");
        assert!(matches!(result, Err(RootedFsError::LeafSymlink)));
        assert_eq!(std::fs::read_to_string(&real).unwrap(), "approved");
        assert_eq!(std::fs::read_to_string(&victim).unwrap(), "victim");
    }

    #[cfg(windows)]
    fn create_junction(link: &Path, target: &Path) -> bool {
        std::process::Command::new("cmd")
            .args(["/d", "/c", "mklink", "/j"])
            .arg(link)
            .arg(target)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|status| status.success())
            .unwrap_or(false)
    }

    #[cfg(windows)]
    #[test]
    fn refuses_junction_parent_for_write_and_lock() {
        let root = tempfile::tempdir().unwrap();
        let victim = tempfile::tempdir().unwrap();
        let junction = root.path().join(".lingxi");
        if !create_junction(&junction, victim.path()) {
            return;
        }
        assert!(atomic_write(
            root.path(),
            Path::new(".lingxi/state.json"),
            b"owned",
            AtomicWriteOptions::default(),
        )
        .is_err());
        assert!(lock_exclusive(
            root.path(),
            Path::new(".lingxi/state.lock"),
            PRIVATE_DIR_MODE,
            PRIVATE_FILE_MODE,
        )
        .is_err());
        assert!(!victim.path().join("state.json").exists());
        assert!(!victim.path().join("state.lock").exists());
    }

    #[cfg(windows)]
    #[test]
    fn parent_swap_cannot_redirect_handle_relative_atomic_write() {
        let root = tempfile::tempdir().unwrap();
        let victim = tempfile::tempdir().unwrap();
        let active = root.path().join(".lingxi");
        let opened = root.path().join(".lingxi-opened");
        std::fs::create_dir(&active).unwrap();
        let swapped = std::cell::Cell::new(false);
        let result = imp::atomic_write_after_parent_open_for_test(
            root.path(),
            Path::new(".lingxi/state.json"),
            b"confined",
            AtomicWriteOptions::default(),
            || {
                std::fs::rename(&active, &opened).unwrap();
                if create_junction(&active, victim.path()) {
                    swapped.set(true);
                } else {
                    std::fs::rename(&opened, &active).unwrap();
                }
            },
        );
        result.unwrap();
        if !swapped.get() {
            return;
        }
        assert_eq!(
            std::fs::read(opened.join("state.json")).unwrap(),
            b"confined"
        );
        assert!(!victim.path().join("state.json").exists());
        std::fs::remove_dir(&active).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn live_lock_handle_pins_its_directory_entry() {
        let root = tempfile::tempdir().unwrap();
        let relative = Path::new(".lingxi/state.lock");
        let guard =
            lock_exclusive(root.path(), relative, PRIVATE_DIR_MODE, PRIVATE_FILE_MODE).unwrap();
        let lock_path = root.path().join(relative);
        let moved_path = root.path().join(".lingxi/moved.lock");

        assert!(
            std::fs::rename(&lock_path, &moved_path).is_err(),
            "a live lock must deny FILE_SHARE_DELETE so the path cannot be swapped"
        );
        drop(guard);
        std::fs::rename(&lock_path, &moved_path).unwrap();
    }

    #[test]
    fn atomic_write_replaces_complete_contents() {
        let root = tempfile::tempdir().unwrap();
        let relative = Path::new(".lingxi/state.json");
        atomic_write(root.path(), relative, b"old", AtomicWriteOptions::default()).unwrap();
        atomic_write(root.path(), relative, b"new", AtomicWriteOptions::default()).unwrap();
        assert_eq!(read_to_string(root.path(), relative).unwrap(), "new");
    }

    #[test]
    fn limited_read_rejects_oversized_files_without_following_symlinks() {
        let root = tempfile::tempdir().unwrap();
        let relative = Path::new(".lingxi/state.json");
        atomic_write(
            root.path(),
            relative,
            b"12345",
            AtomicWriteOptions::default(),
        )
        .unwrap();
        assert_eq!(
            read_to_string_limited(root.path(), relative, 5).unwrap(),
            "12345"
        );
        assert!(matches!(
            read_to_string_limited(root.path(), relative, 4),
            Err(FsError::TooLarge {
                actual: 5,
                limit: 4
            })
        ));
    }

    #[test]
    fn no_clobber_is_atomic_and_preserves_existing_contents() {
        let root = tempfile::tempdir().unwrap();
        let relative = Path::new(".lingxi/state.json");
        atomic_write(root.path(), relative, b"old", AtomicWriteOptions::default()).unwrap();
        let options = AtomicWriteOptions {
            overwrite: false,
            ..AtomicWriteOptions::default()
        };
        assert!(matches!(
            atomic_write(root.path(), relative, b"new", options),
            Err(FsError::AlreadyExists(_))
        ));
        assert_eq!(read_to_string(root.path(), relative).unwrap(), "old");
    }

    #[test]
    fn exclusive_lock_serializes_contenders() {
        let root = tempfile::tempdir().unwrap();
        let relative = Path::new(".lingxi/state.lock");
        let first =
            lock_exclusive(root.path(), relative, PRIVATE_DIR_MODE, PRIVATE_FILE_MODE).unwrap();
        let root_path = root.path().to_path_buf();
        let (tx, rx) = std::sync::mpsc::channel();
        let contender = std::thread::spawn(move || {
            let second =
                lock_exclusive(&root_path, relative, PRIVATE_DIR_MODE, PRIVATE_FILE_MODE).unwrap();
            tx.send(()).unwrap();
            drop(second);
        });
        assert!(rx
            .recv_timeout(std::time::Duration::from_millis(50))
            .is_err());
        drop(first);
        rx.recv_timeout(std::time::Duration::from_secs(2)).unwrap();
        contender.join().unwrap();
    }
}
