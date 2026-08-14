use std::{
    ffi::{OsStr, c_void},
    fs,
    mem::{size_of, zeroed},
    os::windows::{
        ffi::OsStrExt,
        fs::MetadataExt,
        io::{AsRawHandle, FromRawHandle, OwnedHandle},
    },
    path::{Component, Path, PathBuf, Prefix},
};

use chatoms_ports::{
    error::{FailureCategory, PortFailure},
    filesystem::{DirectoryIdentity, DirectoryIdentityGuard, FilesystemIdentityPort},
};
use windows_sys::Win32::{
    Foundation::INVALID_HANDLE_VALUE,
    Storage::{
        CloudFilters::{
            CF_PLACEHOLDER_STATE_INVALID, CF_PLACEHOLDER_STATE_PARTIAL,
            CF_PLACEHOLDER_STATE_PARTIALLY_ON_DISK, CF_PLACEHOLDER_STATE_PLACEHOLDER,
            CF_PLACEHOLDER_STATE_SYNC_ROOT, CfGetPlaceholderStateFromAttributeTag,
        },
        FileSystem::{
            CreateFileW, FILE_ATTRIBUTE_OFFLINE, FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS,
            FILE_ATTRIBUTE_RECALL_ON_OPEN, FILE_ATTRIBUTE_TAG_INFO, FILE_FLAG_BACKUP_SEMANTICS,
            FILE_ID_INFO, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
            FileAttributeTagInfo, FileIdInfo, GetDriveTypeW, GetFileInformationByHandleEx,
            GetFinalPathNameByHandleW, OPEN_EXISTING, VOLUME_NAME_DOS,
        },
    },
};

const DRIVE_FIXED: u32 = 3;

#[derive(Clone, Copy, Debug, Default)]
pub struct WindowsFilesystemIdentity;

struct WindowsDirectoryGuard {
    identity: DirectoryIdentity,
    _handles: Vec<OwnedHandle>,
}

impl DirectoryIdentityGuard for WindowsDirectoryGuard {
    fn identity(&self) -> &DirectoryIdentity {
        &self.identity
    }
}

impl FilesystemIdentityPort for WindowsFilesystemIdentity {
    fn inspect_supported_directory(
        &mut self,
        path: &Path,
    ) -> Result<DirectoryIdentity, PortFailure> {
        let canonical = fs::canonicalize(path).map_err(map_io)?;
        ensure_fixed_drive(&canonical)?;
        reject_cloud_ancestors(&canonical)?;
        let handle = open_directory(&canonical, true, false)?;
        let identity = identity_from_handle(&handle)?;
        ensure_fixed_drive(&identity.canonical_path)?;
        reject_cloud_ancestors(&identity.canonical_path)?;
        Ok(identity)
    }

    fn verify_local_tree(&mut self, root: &Path) -> Result<(), PortFailure> {
        let root_identity = self.inspect_supported_directory(root)?;
        let mut pending = vec![root_identity.canonical_path];
        while let Some(directory) = pending.pop() {
            for entry in fs::read_dir(&directory).map_err(map_io)? {
                let entry = entry.map_err(map_io)?;
                let metadata = fs::symlink_metadata(entry.path()).map_err(map_io)?;
                if metadata.file_type().is_symlink() {
                    return Err(PortFailure::new(FailureCategory::Unsupported));
                }
                if metadata.is_dir() {
                    let handle = open_directory(&entry.path(), true, false)?;
                    reject_cloud_handle(&handle)?;
                    pending.push(entry.path());
                } else {
                    let handle = open_file(&entry.path())?;
                    reject_cloud_handle(&handle)?;
                }
            }
        }
        Ok(())
    }

    fn inspect_supported_file(&mut self, path: &Path) -> Result<DirectoryIdentity, PortFailure> {
        let canonical = fs::canonicalize(path).map_err(map_io)?;
        ensure_fixed_drive(&canonical)?;
        reject_cloud_ancestors(&canonical)?;
        let handle = open_file(&canonical)?;
        let identity = identity_from_handle(&handle)?;
        ensure_fixed_drive(&identity.canonical_path)?;
        reject_cloud_ancestors(&identity.canonical_path)?;
        Ok(identity)
    }

    fn acquire_guard(
        &mut self,
        path: &Path,
        expected: &DirectoryIdentity,
    ) -> Result<Box<dyn DirectoryIdentityGuard>, PortFailure> {
        let actual = self.inspect_supported_directory(path)?;
        if !actual.same_object(expected) || actual.canonical_path != expected.canonical_path {
            return Err(PortFailure::new(FailureCategory::Conflict));
        }
        let handles = open_component_chain(&actual.canonical_path)?;
        let final_identity = identity_from_handle(
            handles
                .last()
                .ok_or_else(|| PortFailure::new(FailureCategory::InvalidInput))?,
        )?;
        if !final_identity.same_object(expected)
            || final_identity.canonical_path != expected.canonical_path
        {
            return Err(PortFailure::new(FailureCategory::Conflict));
        }
        Ok(Box::new(WindowsDirectoryGuard {
            identity: final_identity,
            _handles: handles,
        }))
    }
}

fn open_file(path: &Path) -> Result<OwnedHandle, PortFailure> {
    let metadata = fs::symlink_metadata(path).map_err(map_io)?;
    if metadata.file_attributes()
        & windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT
        != 0
    {
        return Err(PortFailure::new(FailureCategory::Unsupported));
    }
    if !metadata.is_file() {
        return Err(PortFailure::new(FailureCategory::Unsupported));
    }
    open_handle(path, 0, false)
}

fn open_component_chain(path: &Path) -> Result<Vec<OwnedHandle>, PortFailure> {
    let mut current = PathBuf::new();
    let mut handles = Vec::new();
    for component in path.components() {
        current.push(component.as_os_str());
        if matches!(component, Component::Prefix(_) | Component::RootDir) {
            continue;
        }
        handles.push(open_directory(&current, true, true)?);
    }
    Ok(handles)
}

fn open_directory(
    path: &Path,
    reject_reparse: bool,
    deny_delete_sharing: bool,
) -> Result<OwnedHandle, PortFailure> {
    if reject_reparse {
        let metadata = fs::symlink_metadata(path).map_err(map_io)?;
        if metadata.file_attributes()
            & windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT
            != 0
        {
            return Err(PortFailure::new(FailureCategory::Unsupported));
        }
        if !metadata.is_dir() {
            return Err(PortFailure::new(FailureCategory::InvalidInput));
        }
    }
    open_handle(path, FILE_FLAG_BACKUP_SEMANTICS, deny_delete_sharing)
}

fn open_handle(
    path: &Path,
    flags: u32,
    deny_delete_sharing: bool,
) -> Result<OwnedHandle, PortFailure> {
    let wide = wide_null(path.as_os_str());
    let raw = unsafe {
        CreateFileW(
            wide.as_ptr(),
            0,
            FILE_SHARE_READ
                | FILE_SHARE_WRITE
                | if deny_delete_sharing {
                    0
                } else {
                    FILE_SHARE_DELETE
                },
            std::ptr::null(),
            OPEN_EXISTING,
            flags,
            std::ptr::null_mut(),
        )
    };
    if raw == INVALID_HANDLE_VALUE {
        return Err(map_io(std::io::Error::last_os_error()));
    }
    let handle = unsafe { OwnedHandle::from_raw_handle(raw) };
    reject_cloud_handle(&handle)?;
    Ok(handle)
}

fn identity_from_handle(handle: &OwnedHandle) -> Result<DirectoryIdentity, PortFailure> {
    let raw = handle.as_raw_handle();
    let mut info: FILE_ID_INFO = unsafe { zeroed() };
    let ok = unsafe {
        GetFileInformationByHandleEx(
            raw,
            FileIdInfo,
            std::ptr::addr_of_mut!(info).cast::<c_void>(),
            u32::try_from(size_of::<FILE_ID_INFO>())
                .map_err(|_| PortFailure::new(FailureCategory::Internal))?,
        )
    };
    if ok == 0 {
        return Err(map_io(std::io::Error::last_os_error()));
    }
    let canonical_path = final_path(handle)?;
    Ok(DirectoryIdentity {
        canonical_path,
        volume_serial_hex: format!("{:016x}", info.VolumeSerialNumber),
        file_id_hex: info
            .FileId
            .Identifier
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect(),
    })
}

fn final_path(handle: &OwnedHandle) -> Result<PathBuf, PortFailure> {
    let raw = handle.as_raw_handle();
    let needed =
        unsafe { GetFinalPathNameByHandleW(raw, std::ptr::null_mut(), 0, VOLUME_NAME_DOS) };
    if needed == 0 {
        return Err(map_io(std::io::Error::last_os_error()));
    }
    let mut buffer =
        vec![
            0_u16;
            usize::try_from(needed).map_err(|_| PortFailure::new(FailureCategory::Internal))? + 1
        ];
    let written = unsafe {
        GetFinalPathNameByHandleW(
            raw,
            buffer.as_mut_ptr(),
            u32::try_from(buffer.len()).map_err(|_| PortFailure::new(FailureCategory::Internal))?,
            VOLUME_NAME_DOS,
        )
    };
    if written == 0 || usize::try_from(written).map_or(true, |value| value >= buffer.len()) {
        return Err(map_io(std::io::Error::last_os_error()));
    }
    buffer.truncate(
        usize::try_from(written).map_err(|_| PortFailure::new(FailureCategory::Internal))?,
    );
    let value =
        String::from_utf16(&buffer).map_err(|_| PortFailure::new(FailureCategory::InvalidInput))?;
    Ok(PathBuf::from(value.strip_prefix(r"\\?\").unwrap_or(&value)))
}

fn reject_cloud_ancestors(path: &Path) -> Result<(), PortFailure> {
    for ancestor in path.ancestors() {
        if ancestor.as_os_str().is_empty() {
            continue;
        }
        let metadata = fs::symlink_metadata(ancestor).map_err(map_io)?;
        reject_cloud_attributes(metadata.file_attributes(), 0)?;
        if metadata.file_attributes()
            & windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT
            != 0
        {
            return Err(PortFailure::new(FailureCategory::Unsupported));
        }
    }
    Ok(())
}

fn reject_cloud_handle(handle: &OwnedHandle) -> Result<(), PortFailure> {
    let mut info: FILE_ATTRIBUTE_TAG_INFO = unsafe { zeroed() };
    let ok = unsafe {
        GetFileInformationByHandleEx(
            handle.as_raw_handle(),
            FileAttributeTagInfo,
            std::ptr::addr_of_mut!(info).cast::<c_void>(),
            u32::try_from(size_of::<FILE_ATTRIBUTE_TAG_INFO>())
                .map_err(|_| PortFailure::new(FailureCategory::Internal))?,
        )
    };
    if ok == 0 {
        return Err(map_io(std::io::Error::last_os_error()));
    }
    reject_cloud_attributes(info.FileAttributes, info.ReparseTag)
}

fn reject_cloud_attributes(attributes: u32, reparse_tag: u32) -> Result<(), PortFailure> {
    if attributes
        & (FILE_ATTRIBUTE_OFFLINE
            | FILE_ATTRIBUTE_RECALL_ON_OPEN
            | FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS)
        != 0
    {
        return Err(PortFailure::new(FailureCategory::Unsupported));
    }
    let state = unsafe { CfGetPlaceholderStateFromAttributeTag(attributes, reparse_tag) };
    if state == CF_PLACEHOLDER_STATE_INVALID
        || state
            & (CF_PLACEHOLDER_STATE_PLACEHOLDER
                | CF_PLACEHOLDER_STATE_SYNC_ROOT
                | CF_PLACEHOLDER_STATE_PARTIAL
                | CF_PLACEHOLDER_STATE_PARTIALLY_ON_DISK)
            != 0
    {
        return Err(PortFailure::new(FailureCategory::Unsupported));
    }
    Ok(())
}

fn ensure_fixed_drive(path: &Path) -> Result<(), PortFailure> {
    let drive = match path.components().next() {
        Some(Component::Prefix(prefix)) => match prefix.kind() {
            Prefix::Disk(letter) | Prefix::VerbatimDisk(letter) => {
                PathBuf::from(format!("{}:\\", char::from(letter)))
            }
            _ => return Err(PortFailure::new(FailureCategory::Unsupported)),
        },
        _ => return Err(PortFailure::new(FailureCategory::InvalidInput)),
    };
    let wide = wide_null(drive.as_os_str());
    if unsafe { GetDriveTypeW(wide.as_ptr()) } != DRIVE_FIXED {
        return Err(PortFailure::new(FailureCategory::Unsupported));
    }
    Ok(())
}

fn wide_null(value: &OsStr) -> Vec<u16> {
    value.encode_wide().chain(std::iter::once(0)).collect()
}

fn map_io(error: std::io::Error) -> PortFailure {
    match error.kind() {
        std::io::ErrorKind::NotFound => PortFailure::new(FailureCategory::NotFound),
        std::io::ErrorKind::PermissionDenied => PortFailure::new(FailureCategory::PermissionDenied),
        _ => PortFailure::new(FailureCategory::InvalidInput),
    }
}
