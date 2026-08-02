use std::{
    ffi::c_void,
    io,
    mem::{size_of, zeroed},
    os::windows::ffi::OsStrExt,
    path::Path,
    ptr::{addr_of, addr_of_mut, null, null_mut},
};

use chatoms_ports::permissions::{
    FilesystemPermissionManager, PermissionError, PermissionErrorCode, PermissionStatus,
};
use windows_sys::Win32::{
    Foundation::{ERROR_INSUFFICIENT_BUFFER, ERROR_SUCCESS, GENERIC_ALL, LocalFree},
    Security::{
        ACCESS_ALLOWED_ACE, ACL, ACL_SIZE_INFORMATION, AclSizeInformation,
        Authorization::{
            EXPLICIT_ACCESS_W, GetNamedSecurityInfoW, NO_MULTIPLE_TRUSTEE, SE_FILE_OBJECT,
            SET_ACCESS, SetEntriesInAclW, SetNamedSecurityInfoW, TRUSTEE_IS_SID, TRUSTEE_IS_USER,
            TRUSTEE_IS_WELL_KNOWN_GROUP, TRUSTEE_W,
        },
        CONTAINER_INHERIT_ACE, CopySid, CreateWellKnownSid, DACL_SECURITY_INFORMATION, EqualSid,
        GetAce, GetAclInformation, GetLengthSid, GetSecurityDescriptorControl, GetTokenInformation,
        OBJECT_INHERIT_ACE, PROTECTED_DACL_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, PSID,
        SE_DACL_PROTECTED, SECURITY_MAX_SID_SIZE, SUB_CONTAINERS_AND_OBJECTS_INHERIT, TOKEN_USER,
        TokenUser, WinAuthenticatedUserSid, WinBuiltinUsersSid, WinLocalSystemSid, WinWorldSid,
    },
    Storage::FileSystem::FILE_ALL_ACCESS,
};

const CURRENT_PROCESS_TOKEN: isize = -4;
const ACCESS_ALLOWED_ACE_TYPE: u8 = 0;

#[derive(Clone, Copy, Debug, Default)]
pub struct WindowsPermissionManager;

impl FilesystemPermissionManager for WindowsPermissionManager {
    fn secure_directory(&self, path: &Path) -> Result<(), PermissionError> {
        require_directory(path)?;
        set_secure_acl(path, true)?;
        require_secure(self.verify_directory(path)?)
    }

    fn verify_directory(&self, path: &Path) -> Result<PermissionStatus, PermissionError> {
        require_directory(path)?;
        verify_acl(path, true)
    }

    fn secure_file(&self, path: &Path) -> Result<(), PermissionError> {
        require_file(path)?;
        set_secure_acl(path, false)?;
        require_secure(self.verify_file(path)?)
    }

    fn verify_file(&self, path: &Path) -> Result<PermissionStatus, PermissionError> {
        require_file(path)?;
        verify_acl(path, false)
    }
}

fn require_directory(path: &Path) -> Result<(), PermissionError> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() => Ok(()),
        Ok(_) => Err(PermissionError::new(
            PermissionErrorCode::InvariantViolation,
        )),
        Err(source) => Err(read_error(source)),
    }
}

fn require_file(path: &Path) -> Result<(), PermissionError> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_file() => Ok(()),
        Ok(_) => Err(PermissionError::new(
            PermissionErrorCode::InvariantViolation,
        )),
        Err(source) => Err(read_error(source)),
    }
}

fn require_secure(status: PermissionStatus) -> Result<(), PermissionError> {
    if status == PermissionStatus::Secure {
        Ok(())
    } else {
        Err(PermissionError::new(PermissionErrorCode::VerifyAclFailed))
    }
}

fn set_secure_acl(path: &Path, directory: bool) -> Result<(), PermissionError> {
    let current_user = current_user_sid()?;
    let system = well_known_sid(WinLocalSystemSid)?;
    let inheritance = if directory {
        SUB_CONTAINERS_AND_OBJECTS_INHERIT
    } else {
        0
    };
    let entries = [
        explicit_access(&current_user, TRUSTEE_IS_USER, inheritance),
        explicit_access(&system, TRUSTEE_IS_WELL_KNOWN_GROUP, inheritance),
    ];
    let mut acl = null_mut();
    // SAFETY: both entries contain valid SID pointers whose backing storage outlives this call;
    // the output pointer is initialized by the API and released with LocalFree below.
    let result = unsafe {
        SetEntriesInAclW(
            u32::try_from(entries.len()).expect("fixed ACL entry count fits u32"),
            entries.as_ptr(),
            null(),
            &mut acl,
        )
    };
    if result != ERROR_SUCCESS {
        return Err(win32_error(PermissionErrorCode::WriteAclFailed, result));
    }
    if acl.is_null() {
        return Err(PermissionError::new(
            PermissionErrorCode::InvariantViolation,
        ));
    }
    let _acl = LocalAllocation(acl.cast());
    let wide_path = wide_path(path)?;
    // SAFETY: wide_path is NUL-terminated; acl points to a valid ACL produced above. Owner,
    // group, and SACL are null because this operation changes only the protected DACL.
    let result = unsafe {
        SetNamedSecurityInfoW(
            wide_path.as_ptr(),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
            null_mut(),
            null_mut(),
            acl,
            null(),
        )
    };
    if result == ERROR_SUCCESS {
        Ok(())
    } else {
        Err(win32_error(PermissionErrorCode::WriteAclFailed, result))
    }
}

fn verify_acl(path: &Path, directory: bool) -> Result<PermissionStatus, PermissionError> {
    let current_user = current_user_sid()?;
    let system = well_known_sid(WinLocalSystemSid)?;
    let broad = [
        well_known_sid(WinWorldSid)?,
        well_known_sid(WinBuiltinUsersSid)?,
        well_known_sid(WinAuthenticatedUserSid)?,
    ];
    let security = read_security(path)?;
    if security.dacl.is_null() {
        return Ok(PermissionStatus::Insecure);
    }
    if !security.dacl_is_protected()? {
        return Ok(PermissionStatus::Insecure);
    }
    let ace_count = security.ace_count()?;
    let mut user_full = false;
    let mut system_full = false;

    for index in 0..ace_count {
        let ace = security.allowed_ace(index)?;
        let Some(ace) = ace else {
            return Ok(PermissionStatus::Insecure);
        };
        if broad.iter().any(|sid| equal_sid(ace.sid, sid)) {
            return Ok(PermissionStatus::Insecure);
        }
        let full_control =
            ace.mask & FILE_ALL_ACCESS == FILE_ALL_ACCESS || ace.mask & GENERIC_ALL == GENERIC_ALL;
        let inheritance_valid = !directory
            || (u32::from(ace.flags) & (OBJECT_INHERIT_ACE | CONTAINER_INHERIT_ACE)
                == OBJECT_INHERIT_ACE | CONTAINER_INHERIT_ACE);
        if !full_control || !inheritance_valid {
            return Ok(PermissionStatus::Insecure);
        }
        if equal_sid(ace.sid, &current_user) {
            user_full = true;
        } else if equal_sid(ace.sid, &system) {
            system_full = true;
        } else {
            return Ok(PermissionStatus::Insecure);
        }
    }

    if ace_count == 2 && user_full && system_full {
        Ok(PermissionStatus::Secure)
    } else {
        Ok(PermissionStatus::Insecure)
    }
}

fn explicit_access(sid: &Sid, trustee_type: i32, inheritance: u32) -> EXPLICIT_ACCESS_W {
    EXPLICIT_ACCESS_W {
        grfAccessPermissions: FILE_ALL_ACCESS,
        grfAccessMode: SET_ACCESS,
        grfInheritance: inheritance,
        Trustee: TRUSTEE_W {
            pMultipleTrustee: null_mut(),
            MultipleTrusteeOperation: NO_MULTIPLE_TRUSTEE,
            TrusteeForm: TRUSTEE_IS_SID,
            TrusteeType: trustee_type,
            ptstrName: sid.as_psid().cast(),
        },
    }
}

fn current_user_sid() -> Result<Sid, PermissionError> {
    let mut required = 0;
    // SAFETY: the process token pseudo handle is defined by the Windows SDK. A null buffer with
    // length zero is the documented size-query form and writes only to required.
    let first = unsafe {
        GetTokenInformation(
            CURRENT_PROCESS_TOKEN as *mut c_void,
            TokenUser,
            null_mut(),
            0,
            &mut required,
        )
    };
    if first != 0 || required == 0 {
        return Err(PermissionError::new(
            PermissionErrorCode::CurrentUserSidUnavailable,
        ));
    }
    let error = io::Error::last_os_error();
    if error.raw_os_error() != Some(ERROR_INSUFFICIENT_BUFFER as i32) {
        return Err(PermissionError::with_source(
            PermissionErrorCode::CurrentUserSidUnavailable,
            error,
        ));
    }
    let words = usize::try_from(required)
        .ok()
        .and_then(|bytes| bytes.checked_add(size_of::<usize>() - 1))
        .map(|bytes| bytes / size_of::<usize>())
        .ok_or_else(|| PermissionError::new(PermissionErrorCode::InvariantViolation))?;
    let mut buffer = vec![0usize; words];
    // SAFETY: buffer has at least required bytes and pointer alignment suitable for TOKEN_USER.
    let success = unsafe {
        GetTokenInformation(
            CURRENT_PROCESS_TOKEN as *mut c_void,
            TokenUser,
            buffer.as_mut_ptr().cast(),
            required,
            &mut required,
        )
    };
    if success == 0 {
        return Err(PermissionError::with_source(
            PermissionErrorCode::CurrentUserSidUnavailable,
            io::Error::last_os_error(),
        ));
    }
    // SAFETY: GetTokenInformation initialized TOKEN_USER at the start of the aligned buffer.
    let source = unsafe { (*(buffer.as_ptr().cast::<TOKEN_USER>())).User.Sid };
    copy_sid(source, PermissionErrorCode::CurrentUserSidUnavailable)
}

fn well_known_sid(kind: i32) -> Result<Sid, PermissionError> {
    let words = (SECURITY_MAX_SID_SIZE as usize).div_ceil(size_of::<usize>());
    let mut storage = vec![0usize; words];
    let mut size = SECURITY_MAX_SID_SIZE;
    // SAFETY: storage has SECURITY_MAX_SID_SIZE bytes and no domain SID is required for these
    // absolute well-known SID types.
    let success =
        unsafe { CreateWellKnownSid(kind, null_mut(), storage.as_mut_ptr().cast(), &mut size) };
    if success == 0 {
        Err(PermissionError::with_source(
            PermissionErrorCode::CurrentUserSidUnavailable,
            io::Error::last_os_error(),
        ))
    } else {
        Ok(Sid { storage })
    }
}

fn copy_sid(source: PSID, code: PermissionErrorCode) -> Result<Sid, PermissionError> {
    if source.is_null() {
        return Err(PermissionError::new(code));
    }
    // SAFETY: source is returned as a valid SID pointer by GetTokenInformation.
    let length = unsafe { GetLengthSid(source) };
    if length == 0 {
        return Err(PermissionError::with_source(
            code,
            io::Error::last_os_error(),
        ));
    }
    let words = (length as usize).div_ceil(size_of::<usize>());
    let mut storage = vec![0usize; words];
    // SAFETY: destination has length bytes and source remains valid for the duration of the call.
    let success = unsafe { CopySid(length, storage.as_mut_ptr().cast(), source) };
    if success == 0 {
        Err(PermissionError::with_source(
            code,
            io::Error::last_os_error(),
        ))
    } else {
        Ok(Sid { storage })
    }
}

fn equal_sid(left: PSID, right: &Sid) -> bool {
    // SAFETY: both pointers refer to validated SID storage that remains alive for this call.
    unsafe { EqualSid(left, right.as_psid()) != 0 }
}

fn wide_path(path: &Path) -> Result<Vec<u16>, PermissionError> {
    let mut wide: Vec<u16> = path.as_os_str().encode_wide().collect();
    if wide.is_empty() || wide.contains(&0) {
        return Err(PermissionError::new(
            PermissionErrorCode::InvariantViolation,
        ));
    }
    wide.push(0);
    Ok(wide)
}

fn read_security(path: &Path) -> Result<SecurityDescriptor, PermissionError> {
    let wide_path = wide_path(path)?;
    let mut dacl = null_mut();
    let mut descriptor: PSECURITY_DESCRIPTOR = null_mut();
    // SAFETY: wide_path is NUL-terminated and all output pointers are valid. Only DACL data is
    // requested; the returned descriptor owns the dacl and is released by SecurityDescriptor.
    let result = unsafe {
        GetNamedSecurityInfoW(
            wide_path.as_ptr(),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION,
            null_mut(),
            null_mut(),
            &mut dacl,
            null_mut(),
            &mut descriptor,
        )
    };
    if result != ERROR_SUCCESS {
        return Err(win32_error(PermissionErrorCode::ReadAclFailed, result));
    }
    if descriptor.is_null() {
        return Err(PermissionError::new(
            PermissionErrorCode::InvariantViolation,
        ));
    }
    Ok(SecurityDescriptor {
        allocation: LocalAllocation(descriptor),
        dacl,
    })
}

struct SecurityDescriptor {
    allocation: LocalAllocation,
    dacl: *mut ACL,
}

impl SecurityDescriptor {
    fn dacl_is_protected(&self) -> Result<bool, PermissionError> {
        let mut control = 0;
        let mut revision = 0;
        // SAFETY: allocation holds a valid security descriptor for the lifetime of self.
        let success =
            unsafe { GetSecurityDescriptorControl(self.allocation.0, &mut control, &mut revision) };
        if success == 0 {
            Err(PermissionError::with_source(
                PermissionErrorCode::VerifyAclFailed,
                io::Error::last_os_error(),
            ))
        } else {
            Ok(control & SE_DACL_PROTECTED != 0)
        }
    }

    fn ace_count(&self) -> Result<u32, PermissionError> {
        // SAFETY: zeroed is valid initialization for this plain C output structure.
        let mut information: ACL_SIZE_INFORMATION = unsafe { zeroed() };
        // SAFETY: dacl is non-null and belongs to the live security descriptor; output buffer has
        // the exact ACL_SIZE_INFORMATION size.
        let success = unsafe {
            GetAclInformation(
                self.dacl,
                addr_of_mut!(information).cast(),
                u32::try_from(size_of::<ACL_SIZE_INFORMATION>())
                    .expect("ACL information size fits u32"),
                AclSizeInformation,
            )
        };
        if success == 0 {
            Err(PermissionError::with_source(
                PermissionErrorCode::VerifyAclFailed,
                io::Error::last_os_error(),
            ))
        } else {
            Ok(information.AceCount)
        }
    }

    fn allowed_ace(&self, index: u32) -> Result<Option<AllowedAce>, PermissionError> {
        let mut raw = null_mut();
        // SAFETY: dacl is valid and index is bounded by ace_count in the caller.
        let success = unsafe { GetAce(self.dacl, index, &mut raw) };
        if success == 0 || raw.is_null() {
            return Err(PermissionError::with_source(
                PermissionErrorCode::VerifyAclFailed,
                io::Error::last_os_error(),
            ));
        }
        // SAFETY: GetAce returned a pointer to an ACE_HEADER inside the live ACL.
        let header = unsafe { &*(raw.cast::<windows_sys::Win32::Security::ACE_HEADER>()) };
        if header.AceType != ACCESS_ALLOWED_ACE_TYPE {
            return Ok(None);
        }
        // SAFETY: ACCESS_ALLOWED_ACE_TYPE guarantees the corresponding structure layout.
        let ace = unsafe { &*(raw.cast::<ACCESS_ALLOWED_ACE>()) };
        let sid = addr_of!(ace.SidStart).cast_mut().cast();
        Ok(Some(AllowedAce {
            mask: ace.Mask,
            flags: ace.Header.AceFlags,
            sid,
        }))
    }
}

struct AllowedAce {
    mask: u32,
    flags: u8,
    sid: PSID,
}

struct Sid {
    storage: Vec<usize>,
}

impl Sid {
    fn as_psid(&self) -> PSID {
        self.storage.as_ptr().cast_mut().cast()
    }
}

struct LocalAllocation(*mut c_void);

impl Drop for LocalAllocation {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: pointer was allocated by a Windows API documented to require LocalFree and
            // is released exactly once by this owner.
            unsafe {
                LocalFree(self.0);
            }
        }
    }
}

fn read_error(source: io::Error) -> PermissionError {
    if source.kind() == io::ErrorKind::PermissionDenied {
        PermissionError::with_source(PermissionErrorCode::PermissionDenied, source)
    } else {
        PermissionError::with_source(PermissionErrorCode::ReadAclFailed, source)
    }
}

fn win32_error(code: PermissionErrorCode, value: u32) -> PermissionError {
    let source = io::Error::from_raw_os_error(value as i32);
    if source.kind() == io::ErrorKind::PermissionDenied {
        PermissionError::with_source(PermissionErrorCode::PermissionDenied, source)
    } else {
        PermissionError::with_source(code, source)
    }
}
