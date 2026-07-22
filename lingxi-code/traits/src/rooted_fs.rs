//! Root-confined, no-follow filesystem primitives for security-sensitive state.
//!
//! These helpers are synchronous because callers hold an advisory lock across
//! an entire read-modify-write transaction. Unix uses directory file
//! descriptors and `*at` syscalls. Windows uses handle-relative NT file APIs.

use fs2::FileExt;
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};

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

    fn open_root(root: &Path) -> Result<std::fs::File, FsError> {
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
        Ok(file)
    }

    fn open_parent(
        root: &Path,
        relative: &Path,
        create: bool,
    ) -> Result<(std::fs::File, OsString), FsError> {
        let (parents, file_name) = split(relative)?;
        let mut directory = open_root(root)?;
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
        Ok(Mode::from_bits_retain(bits))
    }

    fn open_root(root: &Path) -> Result<OwnedFd, FsError> {
        let canonical = std::fs::canonicalize(root).map_err(|error| map_io(root, error))?;
        fs::open(
            &canonical,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(|error| map_unix_io(&canonical, error))
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
        let (parents, file) = split(relative)?;
        let mut directory = open_root(root)?;
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

    pub(super) fn lock_exclusive(
        _root: &Path,
        _relative: &Path,
        _dir_mode: u32,
        _file_mode: u32,
    ) -> Result<RootedFileLock, FsError> {
        Err(unsupported())
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

    pub(super) fn atomic_write(
        _root: &Path,
        _relative: &Path,
        _bytes: &[u8],
        _options: AtomicWriteOptions,
    ) -> Result<(), FsError> {
        Err(unsupported())
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
