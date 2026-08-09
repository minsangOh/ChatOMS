//! Windows-only discovery and verification of the Git for Windows runtime.
//!
//! This module deliberately does not consult PATH.  The installer registry is
//! merely a candidate source; every candidate is subsequently checked through
//! WinVerifyTrust, a pinned signer certificate and a stable distribution
//! boundary before it is handed to the infrastructure Git adapter.

use std::{
    ffi::OsStr,
    fs,
    mem::{size_of, zeroed},
    os::windows::{
        ffi::{OsStrExt, OsStringExt},
        io::{AsRawHandle, FromRawHandle, OwnedHandle},
    },
    path::{Path, PathBuf},
    ptr::null_mut,
};

use chatoms_ports::{
    filesystem::FilesystemIdentityPort,
    path::{PathError, PathErrorCode},
};
use sha2::{Digest, Sha256};
use windows_sys::Win32::{
    Foundation::{ERROR_INSUFFICIENT_BUFFER, ERROR_SUCCESS, GetLastError, HANDLE, LocalFree},
    Security::Authorization::{GetNamedSecurityInfoW, SE_FILE_OBJECT},
    Security::{
        AccessCheck,
        Cryptography::{CERT_SHA1_HASH_PROP_ID, CertGetCertificateContextProperty},
        DACL_SECURITY_INFORMATION, DuplicateToken, GENERIC_MAPPING, GROUP_SECURITY_INFORMATION,
        GetLengthSid, GetSecurityDescriptorDacl, GetSecurityDescriptorGroup,
        GetSecurityDescriptorOwner, GetSidIdentifierAuthority, GetSidSubAuthority,
        GetSidSubAuthorityCount, GetTokenInformation, IsValidSecurityDescriptor, IsValidSid,
        OWNER_SECURITY_INFORMATION, PRIVILEGE_SET, SECURITY_MANDATORY_LABEL_AUTHORITY,
        SecurityImpersonation, TOKEN_DUPLICATE, TOKEN_MANDATORY_LABEL, TOKEN_QUERY, TokenElevation,
        TokenElevationType, TokenElevationTypeDefault, TokenElevationTypeLimited,
        TokenIntegrityLevel,
        WinTrust::{
            WINTRUST_ACTION_GENERIC_VERIFY_V2, WINTRUST_DATA, WINTRUST_DATA_0, WINTRUST_FILE_INFO,
            WTD_CACHE_ONLY_URL_RETRIEVAL, WTD_CHOICE_FILE, WTD_REVOCATION_CHECK_CHAIN_EXCLUDE_ROOT,
            WTD_REVOKE_WHOLECHAIN, WTD_STATEACTION_CLOSE, WTD_STATEACTION_VERIFY, WTD_UI_NONE,
            WTHelperGetProvCertFromChain, WTHelperGetProvSignerFromChain,
            WTHelperProvDataFromStateData, WinVerifyTrust,
        },
    },
    Storage::FileSystem::{
        DELETE, FILE_ADD_FILE, FILE_ADD_SUBDIRECTORY, FILE_APPEND_DATA, FILE_DELETE_CHILD,
        FILE_WRITE_ATTRIBUTES, FILE_WRITE_DATA, FILE_WRITE_EA, WRITE_DAC, WRITE_OWNER,
    },
    Storage::FileSystem::{FILE_ID_INFO, FileIdInfo, GetFileInformationByHandleEx},
    System::Com::CoTaskMemFree,
    System::Registry::{
        HKEY, HKEY_LOCAL_MACHINE, KEY_READ, KEY_WOW64_32KEY, KEY_WOW64_64KEY, REG_SZ, RegCloseKey,
        RegOpenKeyExW, RegQueryValueExW,
    },
    System::SystemInformation::GetSystemDirectoryW,
    System::SystemServices::MAXIMUM_ALLOWED,
    System::Threading::{GetCurrentProcess, OpenProcessToken},
    UI::Shell::{FOLDERID_Profile, SHGetKnownFolderPath},
};

use crate::PlatformError;

const GIT_UNINSTALL_KEY: &str = "SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\Git_is1";
// Git for Windows 2.48.1.windows.1 leaf code-signing certificate.  A rotation
// is intentionally fail-closed until this policy is explicitly updated.
const GIT_FOR_WINDOWS_SIGNER_SHA1: [u8; 20] = [
    0x3e, 0xb1, 0x4a, 0x3a, 0xef, 0x84, 0xb7, 0x15, 0x3e, 0x13, 0x93, 0x97, 0xf0, 0xa4, 0x9e, 0x2f,
    0xac, 0x66, 0x2b, 0x0e,
];

#[derive(Clone, Debug, Eq, PartialEq)]
enum InstallerRegistryRecord {
    Absent,
    Candidate(PathBuf),
    Invalid,
}

trait InstallerRegistry {
    fn record(&self, view: u32) -> Result<InstallerRegistryRecord, PlatformError>;
}

struct WindowsInstallerRegistry;

impl InstallerRegistry for WindowsInstallerRegistry {
    fn record(&self, view: u32) -> Result<InstallerRegistryRecord, PlatformError> {
        registry_install_location(view)
    }
}

#[derive(Clone, Debug)]
pub struct TrustedGitRuntime {
    root: PathBuf,
    executable: PathBuf,
    cmd: PathBuf,
    bin: PathBuf,
    exec_path: PathBuf,
    system_directory: PathBuf,
    system_root: PathBuf,
    identity: Vec<(PathBuf, String)>,
}

impl TrustedGitRuntime {
    pub fn discover() -> Result<Self, PlatformError> {
        reject_elevated_execution()?;
        discover_installer_runtime(&WindowsInstallerRegistry, Self::from_installer_root)
    }

    pub fn executable(&self) -> &Path {
        &self.executable
    }

    pub fn bin(&self) -> &Path {
        &self.bin
    }

    pub fn cmd(&self) -> &Path {
        &self.cmd
    }

    pub fn exec_path(&self) -> &Path {
        &self.exec_path
    }

    pub fn system_directory(&self) -> &Path {
        &self.system_directory
    }

    pub fn system_root(&self) -> &Path {
        &self.system_root
    }

    pub fn validate(&self) -> Result<(), PlatformError> {
        if capture_identity(self)? != self.identity {
            return Err(invalid_runtime());
        }
        verify_authenticode(&self.executable)
    }

    /// Canonical location of the direct user-level Git global config.  It is
    /// intentionally derived from the Windows profile API rather than HOME,
    /// USERPROFILE or XDG environment variables inherited by the app.
    pub fn user_global_config_path() -> Result<PathBuf, PlatformError> {
        let mut raw = null_mut();
        // SAFETY: FOLDERID_Profile is a valid constant; a null token requests
        // the current user profile and raw is an output pointer freed below.
        let result = unsafe { SHGetKnownFolderPath(&FOLDERID_Profile, 0, null_mut(), &mut raw) };
        if result < 0 || raw.is_null() {
            return Err(invalid_runtime());
        }
        // SAFETY: SHGetKnownFolderPath returned a NUL-terminated buffer owned
        // by the COM allocator.  It remains valid until CoTaskMemFree below.
        let length = unsafe { (0..).take_while(|index| *raw.add(*index) != 0).count() };
        // SAFETY: the length scan stopped at the buffer's NUL terminator.
        let profile = unsafe { std::slice::from_raw_parts(raw, length) };
        let profile = PathBuf::from(std::ffi::OsString::from_wide(profile));
        // SAFETY: raw was allocated by SHGetKnownFolderPath above.
        unsafe { CoTaskMemFree(raw.cast()) };
        Ok(profile.join(".gitconfig"))
    }

    fn from_installer_root(root: PathBuf) -> Result<Self, PlatformError> {
        let root = canonical_directory(&root)?;
        let mut filesystem = crate::filesystem::WindowsFilesystemIdentity;
        filesystem
            .verify_local_tree(&root)
            .map_err(|_| invalid_runtime())?;
        let cmd = canonical_directory(&root.join("cmd"))?;
        let executable = canonical_file(&cmd.join("git.exe"))?;
        let bin = canonical_directory(&root.join("mingw64").join("bin"))?;
        let exec_path =
            canonical_directory(&root.join("mingw64").join("libexec").join("git-core"))?;
        let system_directory = canonical_directory(&system_directory()?)?;
        let system_root = system_directory
            .parent()
            .map(Path::to_path_buf)
            .ok_or_else(invalid_runtime)?;
        if !executable.starts_with(&root)
            || !cmd.starts_with(&root)
            || !bin.starts_with(&root)
            || !exec_path.starts_with(&root)
        {
            return Err(invalid_runtime());
        }
        verify_runtime_acl(
            &root,
            &cmd,
            &bin,
            &exec_path,
            &executable,
            &system_directory,
        )?;
        verify_authenticode(&executable)?;
        let runtime = Self {
            root,
            executable,
            cmd,
            bin,
            exec_path,
            system_directory,
            system_root,
            identity: Vec::new(),
        };
        let identity = capture_identity(&runtime)?;
        Ok(Self {
            identity,
            ..runtime
        })
    }
}

fn discover_installer_runtime<R, V>(
    registry: &R,
    mut validate: V,
) -> Result<TrustedGitRuntime, PlatformError>
where
    R: InstallerRegistry,
    V: FnMut(PathBuf) -> Result<TrustedGitRuntime, PlatformError>,
{
    let mut selected: Option<TrustedGitRuntime> = None;
    for view in [KEY_WOW64_64KEY, KEY_WOW64_32KEY] {
        let root = match registry.record(view)? {
            InstallerRegistryRecord::Absent => continue,
            InstallerRegistryRecord::Candidate(root) => root,
            InstallerRegistryRecord::Invalid => return Err(invalid_runtime()),
        };
        let candidate = validate(root)?;
        if let Some(existing) = &selected {
            if existing.root != candidate.root {
                return Err(invalid_runtime());
            }
        } else {
            selected = Some(candidate);
        }
    }
    selected.ok_or_else(invalid_runtime)
}

fn reject_elevated_execution() -> Result<(), PlatformError> {
    let provider = WindowsTokenInformationProvider {
        token: primary_token()?,
    };
    reject_elevated_execution_with(&provider)
}

fn reject_elevated_execution_with(
    provider: &impl TokenInformationProvider,
) -> Result<(), PlatformError> {
    let mut elevation_type = [0u8; size_of::<i32>()];
    let type_result = provider.query(
        TokenInformationClass::ElevationType,
        TokenInformationOutput::Bytes(&mut elevation_type),
    );
    let elevation_type = i32::from_ne_bytes(elevation_type);
    if !type_result.succeeded
        || type_result.returned != size_of::<i32>() as u32
        || (elevation_type != TokenElevationTypeDefault
            && elevation_type != TokenElevationTypeLimited)
    {
        return Err(invalid_runtime());
    }

    let mut elevation = [0u8; size_of::<u32>()];
    let elevation_result = provider.query(
        TokenInformationClass::Elevation,
        TokenInformationOutput::Bytes(&mut elevation),
    );
    if !elevation_result.succeeded
        || elevation_result.returned != size_of::<u32>() as u32
        || u32::from_ne_bytes(elevation) != 0
    {
        return Err(invalid_runtime());
    }

    reject_integrity_rid(integrity_rid_with(provider)?)
}

fn integrity_rid_with(provider: &impl TokenInformationProvider) -> Result<u32, PlatformError> {
    let first = provider.query(
        TokenInformationClass::IntegrityLevel,
        TokenInformationOutput::None,
    );
    if first.succeeded || first.failure_error != Some(ERROR_INSUFFICIENT_BUFFER) {
        return Err(invalid_runtime());
    }
    let required = first.returned;
    validate_required_integrity_length(required)?;
    let word = size_of::<usize>();
    let bytes = usize::try_from(required).map_err(|_| invalid_runtime())?;
    let words = bytes
        .checked_add(word - 1)
        .and_then(|value| value.checked_div(word))
        .ok_or_else(invalid_runtime)?;
    let allocated = words.checked_mul(word).ok_or_else(invalid_runtime)?;
    let mut buffer = IntegrityBuffer::new(words, allocated);
    let second = provider.query(
        TokenInformationClass::IntegrityLevel,
        TokenInformationOutput::Integrity(&mut buffer),
    );
    if !second.succeeded {
        return Err(invalid_runtime());
    }
    let actual = usize::try_from(second.returned).map_err(|_| invalid_runtime())?;
    validate_actual_integrity_length(actual, allocated)?;
    integrity_rid_from_label(buffer.label(), buffer.start(), actual)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TokenInformationClass {
    ElevationType,
    Elevation,
    IntegrityLevel,
}

struct TokenInformationResult {
    succeeded: bool,
    returned: u32,
    /// Captured immediately after a failed Win32 call.  It is intentionally
    /// absent for success because GetLastError is undefined in that case.
    failure_error: Option<u32>,
}

enum TokenInformationOutput<'a> {
    None,
    Bytes(&'a mut [u8]),
    Integrity(&'a mut IntegrityBuffer),
}

trait TokenInformationProvider {
    fn query(
        &self,
        class: TokenInformationClass,
        output: TokenInformationOutput<'_>,
    ) -> TokenInformationResult;
}

struct WindowsTokenInformationProvider {
    token: OwnedHandle,
}

impl TokenInformationProvider for WindowsTokenInformationProvider {
    fn query(
        &self,
        class: TokenInformationClass,
        output: TokenInformationOutput<'_>,
    ) -> TokenInformationResult {
        let (information_class, buffer, length) = match output {
            TokenInformationOutput::None => (TokenIntegrityLevel, std::ptr::null_mut(), 0),
            TokenInformationOutput::Bytes(bytes) => match class {
                TokenInformationClass::ElevationType => (
                    TokenElevationType,
                    bytes.as_mut_ptr().cast(),
                    u32::try_from(bytes.len()).unwrap_or(0),
                ),
                TokenInformationClass::Elevation => (
                    TokenElevation,
                    bytes.as_mut_ptr().cast(),
                    u32::try_from(bytes.len()).unwrap_or(0),
                ),
                TokenInformationClass::IntegrityLevel => {
                    return TokenInformationResult {
                        succeeded: false,
                        returned: 0,
                        failure_error: Some(ERROR_SUCCESS),
                    };
                }
            },
            TokenInformationOutput::Integrity(buffer) => {
                if class != TokenInformationClass::IntegrityLevel {
                    return TokenInformationResult {
                        succeeded: false,
                        returned: 0,
                        failure_error: Some(ERROR_SUCCESS),
                    };
                }
                (
                    TokenIntegrityLevel,
                    buffer.output_ptr(),
                    buffer.output_length(),
                )
            }
        };
        let mut returned = 0u32;
        // SAFETY: the provider owns the queried token.  The selected output is
        // either null for the documented size query or points to live writable
        // storage whose length is passed unchanged to the Windows API.
        let succeeded = unsafe {
            GetTokenInformation(
                self.token.as_raw_handle(),
                information_class,
                buffer,
                length,
                &mut returned,
            ) != 0
        };
        TokenInformationResult {
            succeeded,
            returned,
            failure_error: (!succeeded).then(|| unsafe { GetLastError() }),
        }
    }
}

struct IntegrityBuffer {
    storage: Vec<usize>,
    allocated: usize,
}

impl IntegrityBuffer {
    fn new(words: usize, allocated: usize) -> Self {
        Self {
            storage: vec![0usize; words],
            allocated,
        }
    }

    fn output_ptr(&mut self) -> *mut core::ffi::c_void {
        self.storage.as_mut_ptr().cast()
    }

    fn output_length(&self) -> u32 {
        u32::try_from(self.allocated).unwrap_or(0)
    }

    fn start(&self) -> usize {
        self.storage.as_ptr() as usize
    }

    fn label(&self) -> &TOKEN_MANDATORY_LABEL {
        // SAFETY: storage is usize-aligned, has at least a mandatory-label
        // header after the validated allocation, and remains owned by self.
        unsafe { &*self.storage.as_ptr().cast::<TOKEN_MANDATORY_LABEL>() }
    }

    #[cfg(test)]
    fn bytes_mut(&mut self) -> &mut [u8] {
        // SAFETY: usize storage is contiguous and allocated bytes exactly span
        // the initialized backing allocation used for the test payload.
        unsafe { std::slice::from_raw_parts_mut(self.storage.as_mut_ptr().cast(), self.allocated) }
    }
}

fn validate_required_integrity_length(required: u32) -> Result<(), PlatformError> {
    if required == 0 || required < size_of::<TOKEN_MANDATORY_LABEL>() as u32 {
        Err(invalid_runtime())
    } else {
        Ok(())
    }
}

fn validate_actual_integrity_length(actual: usize, allocated: usize) -> Result<(), PlatformError> {
    if actual < size_of::<TOKEN_MANDATORY_LABEL>() || actual > allocated {
        Err(invalid_runtime())
    } else {
        Ok(())
    }
}

fn reject_integrity_rid(rid: u32) -> Result<(), PlatformError> {
    if rid >= 0x3000 {
        Err(invalid_runtime())
    } else {
        Ok(())
    }
}

fn integrity_rid_from_label(
    label: &TOKEN_MANDATORY_LABEL,
    buffer_start: usize,
    buffer_length: usize,
) -> Result<u32, PlatformError> {
    integrity_rid_from_label_with(label, buffer_start, buffer_length, &WindowsSidApi)
}

trait SidApi {
    fn is_valid(&self, sid: *mut core::ffi::c_void) -> bool;
    fn length(&self, sid: *mut core::ffi::c_void) -> Option<usize>;
    fn authority(&self, sid: *mut core::ffi::c_void) -> Option<[u8; 6]>;
    fn count(&self, sid: *mut core::ffi::c_void) -> Option<u8>;
    fn rid(&self, sid: *mut core::ffi::c_void, index: u32) -> Option<u32>;
}

struct WindowsSidApi;

impl SidApi for WindowsSidApi {
    fn is_valid(&self, sid: *mut core::ffi::c_void) -> bool {
        unsafe { IsValidSid(sid) != 0 }
    }
    fn length(&self, sid: *mut core::ffi::c_void) -> Option<usize> {
        usize::try_from(unsafe { GetLengthSid(sid) }).ok()
    }
    fn authority(&self, sid: *mut core::ffi::c_void) -> Option<[u8; 6]> {
        let value = unsafe { GetSidIdentifierAuthority(sid) };
        (!value.is_null()).then(|| unsafe { (*value).Value })
    }
    fn count(&self, sid: *mut core::ffi::c_void) -> Option<u8> {
        let value = unsafe { GetSidSubAuthorityCount(sid) };
        (!value.is_null()).then(|| unsafe { *value })
    }
    fn rid(&self, sid: *mut core::ffi::c_void, index: u32) -> Option<u32> {
        let value = unsafe { GetSidSubAuthority(sid, index) };
        (!value.is_null()).then(|| unsafe { *value })
    }
}

fn integrity_rid_from_label_with(
    label: &TOKEN_MANDATORY_LABEL,
    buffer_start: usize,
    buffer_length: usize,
    api: &impl SidApi,
) -> Result<u32, PlatformError> {
    const SID_HEADER_BYTES: usize = 8;
    const SID_REVISION: u8 = 1;
    const SID_SUBAUTHORITY_BYTES: usize = 4;
    let sid = label.Label.Sid;
    if sid.is_null() {
        return Err(invalid_runtime());
    }
    let sid_start = sid as usize;
    let buffer_end = buffer_start
        .checked_add(buffer_length)
        .ok_or_else(invalid_runtime)?;
    let header_end = sid_start
        .checked_add(SID_HEADER_BYTES)
        .ok_or_else(invalid_runtime)?;
    if sid_start < buffer_start || header_end > buffer_end {
        return Err(invalid_runtime());
    }
    // SAFETY: the fixed SID header lies entirely in the owned output buffer.
    let header = unsafe { std::slice::from_raw_parts(sid.cast::<u8>(), SID_HEADER_BYTES) };
    if header[0] != SID_REVISION || header[1] == 0 {
        return Err(invalid_runtime());
    }
    let calculated_length = SID_HEADER_BYTES
        .checked_add(
            usize::from(header[1])
                .checked_mul(SID_SUBAUTHORITY_BYTES)
                .ok_or_else(invalid_runtime)?,
        )
        .ok_or_else(invalid_runtime)?;
    let sid_end = sid_start
        .checked_add(calculated_length)
        .ok_or_else(invalid_runtime)?;
    if sid_end > buffer_end || !api.is_valid(sid) {
        return Err(invalid_runtime());
    }
    if api.length(sid) != Some(calculated_length) {
        return Err(invalid_runtime());
    }
    if api.authority(sid) != Some(SECURITY_MANDATORY_LABEL_AUTHORITY.Value) {
        return Err(invalid_runtime());
    }
    let Some(count) = api.count(sid).filter(|count| *count > 0) else {
        return Err(invalid_runtime());
    };
    api.rid(sid, u32::from(count - 1))
        .ok_or_else(invalid_runtime)
}

fn primary_token() -> Result<OwnedHandle, PlatformError> {
    let mut raw = null_mut();
    // SAFETY: current process is valid and raw is an output handle pointer.
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY | TOKEN_DUPLICATE, &mut raw) }
        == 0
    {
        return Err(invalid_runtime());
    }
    // SAFETY: OpenProcessToken returned an owned handle.
    Ok(unsafe { OwnedHandle::from_raw_handle(raw) })
}

fn capture_identity(runtime: &TrustedGitRuntime) -> Result<Vec<(PathBuf, String)>, PlatformError> {
    verify_runtime_acl(
        &runtime.root,
        &runtime.cmd,
        &runtime.bin,
        &runtime.exec_path,
        &runtime.executable,
        &runtime.system_directory,
    )?;
    let mut values = Vec::new();
    for directory in [
        &runtime.root,
        &runtime.cmd,
        &runtime.bin,
        &runtime.exec_path,
        &runtime.system_directory,
    ] {
        values.push((directory.clone(), directory_identity(directory)?));
    }
    let metadata = fs::symlink_metadata(&runtime.executable).map_err(path_error)?;
    if !metadata.is_file() || is_reparse(&metadata) || is_network_path(&runtime.executable) {
        return Err(invalid_runtime());
    }
    let digest = Sha256::digest(fs::read(&runtime.executable).map_err(path_error)?);
    let digest = digest
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    values.push((
        runtime.executable.clone(),
        format!(
            "{}:{}:{digest}",
            file_identity(&runtime.executable)?,
            metadata_identity(&metadata)
        ),
    ));
    Ok(values)
}

fn verify_runtime_acl(
    root: &Path,
    cmd: &Path,
    bin: &Path,
    exec_path: &Path,
    executable: &Path,
    system_directory: &Path,
) -> Result<(), PlatformError> {
    for directory in [root, cmd, bin, exec_path, system_directory] {
        reject_current_user_mutation(directory, true)?;
        reject_parent_replacement(directory)?;
    }
    reject_current_user_mutation(executable, false)?;
    reject_parent_replacement(executable)
}

fn reject_parent_replacement(path: &Path) -> Result<(), PlatformError> {
    let parent = path.parent().ok_or_else(invalid_runtime)?;
    let descriptor = security_descriptor(parent)?;
    let token = impersonation_token()?;
    let granted = maximum_allowed(descriptor.as_ptr(), token.as_raw_handle())?;
    if granted & (FILE_DELETE_CHILD | WRITE_DAC | WRITE_OWNER) != 0 {
        return Err(invalid_runtime());
    }
    Ok(())
}

fn reject_current_user_mutation(path: &Path, directory: bool) -> Result<(), PlatformError> {
    let descriptor = security_descriptor(path)?;
    let token = impersonation_token()?;
    let granted = maximum_allowed(descriptor.as_ptr(), token.as_raw_handle())?;
    let dangerous = if directory {
        FILE_ADD_FILE
            | FILE_ADD_SUBDIRECTORY
            | FILE_WRITE_EA
            | FILE_WRITE_ATTRIBUTES
            | FILE_DELETE_CHILD
            | DELETE
            | WRITE_DAC
            | WRITE_OWNER
    } else {
        FILE_WRITE_DATA
            | FILE_APPEND_DATA
            | FILE_WRITE_EA
            | FILE_WRITE_ATTRIBUTES
            | DELETE
            | WRITE_DAC
            | WRITE_OWNER
    };
    if granted & dangerous != 0 {
        return Err(invalid_runtime());
    }
    Ok(())
}

struct SecurityDescriptor(*mut core::ffi::c_void);

impl SecurityDescriptor {
    fn as_ptr(&self) -> *mut core::ffi::c_void {
        self.0
    }
}

impl Drop for SecurityDescriptor {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: GetNamedSecurityInfoW allocated this descriptor with LocalAlloc.
            unsafe { LocalFree(self.0.cast()) };
        }
    }
}

fn security_descriptor(path: &Path) -> Result<SecurityDescriptor, PlatformError> {
    let name = wide_os(path.as_os_str());
    let mut descriptor = std::ptr::null_mut();
    // SAFETY: name is NUL terminated and descriptor is a valid output pointer.
    let status = unsafe {
        GetNamedSecurityInfoW(
            name.as_ptr(),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION | OWNER_SECURITY_INFORMATION | GROUP_SECURITY_INFORMATION,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            &mut descriptor,
        )
    };
    if status != ERROR_SUCCESS || descriptor.is_null() {
        return Err(invalid_runtime());
    }
    let descriptor = SecurityDescriptor(descriptor.cast());
    let mut owner = std::ptr::null_mut();
    let mut group = std::ptr::null_mut();
    let mut owner_defaulted = 0;
    let mut group_defaulted = 0;
    let mut dacl_present = 0;
    let mut dacl = std::ptr::null_mut();
    let mut dacl_defaulted = 0;
    // SAFETY: descriptor is valid for these read-only descriptor queries.
    let valid = unsafe { IsValidSecurityDescriptor(descriptor.as_ptr().cast()) };
    let owner_ok = unsafe {
        GetSecurityDescriptorOwner(descriptor.as_ptr().cast(), &mut owner, &mut owner_defaulted)
    };
    let group_ok = unsafe {
        GetSecurityDescriptorGroup(descriptor.as_ptr().cast(), &mut group, &mut group_defaulted)
    };
    let dacl_ok = unsafe {
        GetSecurityDescriptorDacl(
            descriptor.as_ptr().cast(),
            &mut dacl_present,
            &mut dacl,
            &mut dacl_defaulted,
        )
    };
    if valid == 0
        || owner_ok == 0
        || group_ok == 0
        || dacl_ok == 0
        || owner.is_null()
        || group.is_null()
        || dacl_present == 0
        || dacl.is_null()
    {
        return Err(invalid_runtime());
    }
    Ok(descriptor)
}

fn impersonation_token() -> Result<std::os::windows::io::OwnedHandle, PlatformError> {
    let primary = primary_token()?;
    let mut impersonation = std::ptr::null_mut();
    // SAFETY: primary is valid and impersonation is an output handle pointer.
    if unsafe {
        DuplicateToken(
            primary.as_raw_handle(),
            SecurityImpersonation,
            &mut impersonation,
        )
    } == 0
    {
        return Err(invalid_runtime());
    }
    // SAFETY: DuplicateToken returned an owned impersonation token handle.
    Ok(unsafe { std::os::windows::io::OwnedHandle::from_raw_handle(impersonation) })
}

fn maximum_allowed(
    descriptor: *mut core::ffi::c_void,
    token: HANDLE,
) -> Result<u32, PlatformError> {
    let mapping = GENERIC_MAPPING {
        GenericRead: windows_sys::Win32::Storage::FileSystem::FILE_GENERIC_READ,
        GenericWrite: windows_sys::Win32::Storage::FileSystem::FILE_GENERIC_WRITE,
        GenericExecute: windows_sys::Win32::Storage::FileSystem::FILE_GENERIC_EXECUTE,
        GenericAll: windows_sys::Win32::Storage::FileSystem::FILE_ALL_ACCESS,
    };
    let mut privileges =
        u32::try_from(size_of::<PRIVILEGE_SET>()).map_err(|_| invalid_runtime())?;
    let mut granted = 0u32;
    let mut allowed = 0;
    let mut buffer = vec![0u8; usize::try_from(privileges).map_err(|_| invalid_runtime())?];
    // SAFETY: buffer provides the documented minimum PRIVILEGE_SET storage.
    let first = unsafe {
        AccessCheck(
            descriptor.cast(),
            token,
            MAXIMUM_ALLOWED,
            &mapping,
            buffer.as_mut_ptr().cast::<PRIVILEGE_SET>(),
            &mut privileges,
            &mut granted,
            &mut allowed,
        )
    };
    if first != 0 {
        return Ok(if allowed != 0 { granted } else { 0 });
    }
    if unsafe { GetLastError() } != ERROR_INSUFFICIENT_BUFFER {
        return Err(invalid_runtime());
    }
    buffer.resize(
        usize::try_from(privileges).map_err(|_| invalid_runtime())?,
        0,
    );
    // SAFETY: buffer was resized to AccessCheck's required privilege-set length.
    let second = unsafe {
        AccessCheck(
            descriptor.cast(),
            token,
            MAXIMUM_ALLOWED,
            &mapping,
            buffer.as_mut_ptr().cast::<PRIVILEGE_SET>(),
            &mut privileges,
            &mut granted,
            &mut allowed,
        )
    };
    if second == 0 {
        return Err(invalid_runtime());
    }
    Ok(if allowed != 0 { granted } else { 0 })
}

fn system_directory() -> Result<PathBuf, PlatformError> {
    let mut buffer = vec![0u16; 32_768];
    // SAFETY: buffer is valid writable UTF-16 storage for the documented API.
    let written = unsafe {
        GetSystemDirectoryW(
            buffer.as_mut_ptr(),
            u32::try_from(buffer.len()).map_err(|_| invalid_runtime())?,
        )
    };
    if written == 0 || usize::try_from(written).map_or(true, |size| size >= buffer.len()) {
        return Err(invalid_runtime());
    }
    buffer.truncate(usize::try_from(written).map_err(|_| invalid_runtime())?);
    Ok(PathBuf::from(std::ffi::OsString::from_wide(&buffer)))
}

fn directory_identity(path: &Path) -> Result<String, PlatformError> {
    let mut filesystem = crate::filesystem::WindowsFilesystemIdentity;
    let identity = filesystem
        .inspect_supported_directory(path)
        .map_err(|_| invalid_runtime())?;
    Ok(format!(
        "{}:{}:{}",
        identity.canonical_path.display(),
        identity.volume_serial_hex,
        identity.file_id_hex
    ))
}

fn file_identity(path: &Path) -> Result<String, PlatformError> {
    let file = fs::File::open(path).map_err(path_error)?;
    let mut info: FILE_ID_INFO = unsafe { zeroed() };
    // SAFETY: file owns a valid Windows handle and info is an appropriately
    // sized writable FILE_ID_INFO output buffer.
    let ok = unsafe {
        GetFileInformationByHandleEx(
            file.as_raw_handle(),
            FileIdInfo,
            std::ptr::addr_of_mut!(info).cast(),
            u32::try_from(size_of::<FILE_ID_INFO>()).map_err(|_| invalid_runtime())?,
        )
    };
    if ok == 0 {
        return Err(invalid_runtime());
    }
    Ok(format!(
        "{:016x}:{}",
        info.VolumeSerialNumber,
        info.FileId
            .Identifier
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
    ))
}

fn canonical_directory(path: &Path) -> Result<PathBuf, PlatformError> {
    let mut filesystem = crate::filesystem::WindowsFilesystemIdentity;
    filesystem
        .inspect_supported_directory(path)
        .map_err(|_| invalid_runtime())?;
    let metadata = fs::symlink_metadata(path).map_err(path_error)?;
    if !metadata.is_dir() || is_reparse(&metadata) || is_network_path(path) {
        return Err(invalid_runtime());
    }
    let canonical = fs::canonicalize(path).map_err(path_error)?;
    if is_network_path(&canonical) {
        return Err(invalid_runtime());
    }
    Ok(canonical)
}

fn canonical_file(path: &Path) -> Result<PathBuf, PlatformError> {
    let metadata = fs::symlink_metadata(path).map_err(path_error)?;
    if !metadata.is_file() || is_reparse(&metadata) || is_network_path(path) {
        return Err(invalid_runtime());
    }
    let canonical = fs::canonicalize(path).map_err(path_error)?;
    if is_network_path(&canonical) {
        return Err(invalid_runtime());
    }
    Ok(canonical)
}

fn registry_install_location(view: u32) -> Result<InstallerRegistryRecord, PlatformError> {
    let key_name = wide(GIT_UNINSTALL_KEY);
    let mut key: HKEY = null_mut();
    // SAFETY: key_name is NUL terminated and key is an output parameter.
    let opened = unsafe {
        RegOpenKeyExW(
            HKEY_LOCAL_MACHINE,
            key_name.as_ptr(),
            0,
            KEY_READ | view,
            &mut key,
        )
    };
    if opened != ERROR_SUCCESS {
        return Ok(InstallerRegistryRecord::Absent);
    }
    let result = (|| {
        for (name, expected) in [
            ("DisplayName", "Git"),
            ("Publisher", "The Git Development Community"),
        ] {
            if read_registry_string(key, name)?.as_deref() != Some(expected) {
                return Ok(InstallerRegistryRecord::Invalid);
            }
        }
        Ok(read_registry_string(key, "InstallLocation")?
            .map(PathBuf::from)
            .map_or(
                InstallerRegistryRecord::Invalid,
                InstallerRegistryRecord::Candidate,
            ))
    })();
    // SAFETY: key was returned by the successful RegOpenKeyExW call above.
    unsafe { RegCloseKey(key) };
    result
}

fn read_registry_string(key: HKEY, name: &str) -> Result<Option<String>, PlatformError> {
    let name = wide(name);
    let mut kind = 0;
    let mut bytes = 0;
    // SAFETY: this is the documented size-query call; all pointers are valid.
    let first = unsafe {
        RegQueryValueExW(
            key,
            name.as_ptr(),
            null_mut(),
            &mut kind,
            null_mut(),
            &mut bytes,
        )
    };
    if first != ERROR_SUCCESS || kind != REG_SZ || bytes == 0 || bytes % 2 != 0 {
        return Ok(None);
    }
    let mut value = vec![0u16; usize::try_from(bytes / 2).map_err(|_| invalid_runtime())?];
    // SAFETY: value has the exact byte capacity returned by the size query.
    let second = unsafe {
        RegQueryValueExW(
            key,
            name.as_ptr(),
            null_mut(),
            &mut kind,
            value.as_mut_ptr().cast(),
            &mut bytes,
        )
    };
    if second != ERROR_SUCCESS || kind != REG_SZ {
        return Ok(None);
    }
    if value.last() == Some(&0) {
        value.pop();
    }
    String::from_utf16(&value)
        .map(Some)
        .map_err(|_| invalid_runtime())
}

fn verify_authenticode(path: &Path) -> Result<(), PlatformError> {
    use std::mem::size_of;
    let path = wide_os(path.as_os_str());
    let mut file = WINTRUST_FILE_INFO {
        cbStruct: u32::try_from(size_of::<WINTRUST_FILE_INFO>()).map_err(|_| invalid_runtime())?,
        pcwszFilePath: path.as_ptr(),
        hFile: null_mut::<std::ffi::c_void>() as HANDLE,
        pgKnownSubject: null_mut(),
    };
    let mut data = WINTRUST_DATA {
        cbStruct: u32::try_from(size_of::<WINTRUST_DATA>()).map_err(|_| invalid_runtime())?,
        dwUIChoice: WTD_UI_NONE,
        fdwRevocationChecks: WTD_REVOKE_WHOLECHAIN,
        dwUnionChoice: WTD_CHOICE_FILE,
        Anonymous: WINTRUST_DATA_0 { pFile: &mut file },
        dwStateAction: WTD_STATEACTION_VERIFY,
        dwProvFlags: WTD_CACHE_ONLY_URL_RETRIEVAL | WTD_REVOCATION_CHECK_CHAIN_EXCLUDE_ROOT,
        ..Default::default()
    };
    let mut action = WINTRUST_ACTION_GENERIC_VERIFY_V2;
    // SAFETY: initialized structs remain alive for the complete verification call.
    let result = unsafe {
        WinVerifyTrust(
            null_mut(),
            &mut action,
            (&mut data as *mut WINTRUST_DATA).cast(),
        )
    };
    if result != 0 {
        return Err(invalid_runtime());
    }
    let signer = (|| {
        // SAFETY: successful verification created this state data.
        let provider = unsafe { WTHelperProvDataFromStateData(data.hWVTStateData) };
        if provider.is_null() {
            return Err(invalid_runtime());
        }
        // SAFETY: index zero requests the leaf signer of the active state.
        let signer = unsafe { WTHelperGetProvSignerFromChain(provider, 0, 0, 0) };
        if signer.is_null() {
            return Err(invalid_runtime());
        }
        // SAFETY: index zero requests the leaf certificate of the active signer.
        let certificate = unsafe { WTHelperGetProvCertFromChain(signer, 0) };
        if certificate.is_null() {
            return Err(invalid_runtime());
        }
        // SAFETY: certificate belongs to the state kept alive until the close call below.
        let context = unsafe { (*certificate).pCert };
        if context.is_null() {
            return Err(invalid_runtime());
        }
        let mut thumbprint = [0u8; 20];
        let mut length = 20;
        // SAFETY: context is valid and thumbprint is a 20 byte writable output buffer.
        let found = unsafe {
            CertGetCertificateContextProperty(
                context,
                CERT_SHA1_HASH_PROP_ID,
                thumbprint.as_mut_ptr().cast(),
                &mut length,
            )
        };
        if found == 0 || length != 20 || thumbprint != GIT_FOR_WINDOWS_SIGNER_SHA1 {
            return Err(invalid_runtime());
        }
        Ok(())
    })();
    data.dwStateAction = WTD_STATEACTION_CLOSE;
    // SAFETY: closes only the state allocated by the successful verification above.
    let _ = unsafe {
        WinVerifyTrust(
            null_mut(),
            &mut action,
            (&mut data as *mut WINTRUST_DATA).cast(),
        )
    };
    signer
}

fn wide(value: &str) -> Vec<u16> {
    OsStr::new(value)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}
fn wide_os(value: &OsStr) -> Vec<u16> {
    value.encode_wide().chain(std::iter::once(0)).collect()
}

fn metadata_identity(metadata: &fs::Metadata) -> String {
    use std::os::windows::fs::MetadataExt;
    format!(
        "{}:{}:{}:{}",
        metadata.creation_time(),
        metadata.last_write_time(),
        metadata.file_size(),
        metadata.file_attributes()
    )
}

fn is_reparse(metadata: &fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    metadata.file_attributes() & 0x0000_0400 != 0
}

fn is_network_path(path: &Path) -> bool {
    use std::path::{Component, Prefix};
    matches!(path.components().next(), Some(Component::Prefix(prefix)) if matches!(prefix.kind(), Prefix::UNC(..) | Prefix::VerbatimUNC(..) | Prefix::DeviceNS(_)))
}

fn path_error(error: std::io::Error) -> PlatformError {
    PlatformError::Path(PathError::with_source(
        PathErrorCode::InvalidBasePath,
        error,
    ))
}
fn invalid_runtime() -> PlatformError {
    PlatformError::Path(PathError::new(PathErrorCode::InvalidBasePath))
}

#[cfg(test)]
mod diagnostics {
    use std::{
        cell::{Cell, RefCell},
        collections::VecDeque,
    };

    use super::*;

    const LABEL_BYTES: usize = size_of::<TOKEN_MANDATORY_LABEL>();
    const SID_BYTES: usize = 12;

    struct RegistryFake {
        records: Vec<(u32, InstallerRegistryRecord)>,
    }

    impl InstallerRegistry for RegistryFake {
        fn record(&self, view: u32) -> Result<InstallerRegistryRecord, PlatformError> {
            Ok(self
                .records
                .iter()
                .find(|(candidate_view, _)| *candidate_view == view)
                .map_or(InstallerRegistryRecord::Absent, |(_, record)| {
                    record.clone()
                }))
        }
    }

    fn test_runtime(root: PathBuf) -> TrustedGitRuntime {
        TrustedGitRuntime {
            executable: root.join("cmd").join("git.exe"),
            cmd: root.join("cmd"),
            bin: root.join("mingw64").join("bin"),
            exec_path: root.join("mingw64").join("libexec").join("git-core"),
            system_directory: PathBuf::from("C:/Windows/System32"),
            system_root: PathBuf::from("C:/Windows"),
            root,
            identity: Vec::new(),
        }
    }

    #[test]
    fn official_git_runtime_discovery_accepts_a_valid_lower_priority_candidate_when_no_prior_record_exists()
     {
        let root = PathBuf::from("C:/Program Files/Git");
        let registry = RegistryFake {
            records: vec![(
                KEY_WOW64_32KEY,
                InstallerRegistryRecord::Candidate(root.clone()),
            )],
        };
        let discovered =
            discover_installer_runtime(&registry, |candidate| Ok(test_runtime(candidate)))
                .expect("a missing prior registry record permits the valid remaining record");
        assert_eq!(discovered.root, root);
    }

    #[test]
    fn official_git_runtime_discovery_does_not_fallback_after_invalid_prior_candidate() {
        let first = PathBuf::from("C:/Program Files/Git");
        let registry = RegistryFake {
            records: vec![
                (
                    KEY_WOW64_64KEY,
                    InstallerRegistryRecord::Candidate(first.clone()),
                ),
                (
                    KEY_WOW64_32KEY,
                    InstallerRegistryRecord::Candidate(PathBuf::from("C:/Git32")),
                ),
            ],
        };
        let validations = Cell::new(0);
        assert!(
            discover_installer_runtime(&registry, |candidate| {
                validations.set(validations.get() + 1);
                if candidate == first {
                    Err(invalid_runtime())
                } else {
                    Ok(test_runtime(candidate))
                }
            })
            .is_err()
        );
        assert_eq!(validations.get(), 1);
    }

    #[test]
    fn official_git_runtime_discovery_rejects_ambiguous_valid_roots() {
        let registry = RegistryFake {
            records: vec![
                (
                    KEY_WOW64_64KEY,
                    InstallerRegistryRecord::Candidate(PathBuf::from("C:/Git64")),
                ),
                (
                    KEY_WOW64_32KEY,
                    InstallerRegistryRecord::Candidate(PathBuf::from("C:/Git32")),
                ),
            ],
        };
        assert!(
            discover_installer_runtime(&registry, |candidate| Ok(test_runtime(candidate))).is_err()
        );
    }

    #[test]
    fn official_git_runtime_discovery_accepts_a_single_valid_prior_candidate() {
        let root = PathBuf::from("C:/Program Files/Git");
        let registry = RegistryFake {
            records: vec![(
                KEY_WOW64_64KEY,
                InstallerRegistryRecord::Candidate(root.clone()),
            )],
        };
        let discovered =
            discover_installer_runtime(&registry, |candidate| Ok(test_runtime(candidate)))
                .expect("single valid official candidate");
        assert_eq!(discovered.root, root);
    }

    #[test]
    fn official_git_runtime_discovery_rejects_invalid_record_and_missing_candidates() {
        let invalid = RegistryFake {
            records: vec![(KEY_WOW64_64KEY, InstallerRegistryRecord::Invalid)],
        };
        assert!(
            discover_installer_runtime(&invalid, |candidate| Ok(test_runtime(candidate))).is_err()
        );
        let missing = RegistryFake {
            records: Vec::new(),
        };
        assert!(
            discover_installer_runtime(&missing, |candidate| Ok(test_runtime(candidate))).is_err()
        );
    }

    #[test]
    fn official_git_runtime_discovery_does_not_fallback_after_signer_or_acl_validation_failure() {
        for failure in ["signer", "acl"] {
            let registry = RegistryFake {
                records: vec![
                    (
                        KEY_WOW64_64KEY,
                        InstallerRegistryRecord::Candidate(PathBuf::from("C:/Git64")),
                    ),
                    (
                        KEY_WOW64_32KEY,
                        InstallerRegistryRecord::Candidate(PathBuf::from("C:/Git32")),
                    ),
                ],
            };
            let validations = Cell::new(0);
            assert!(
                discover_installer_runtime(&registry, |candidate| {
                    validations.set(validations.get() + 1);
                    if candidate.ends_with("Git64") && (failure == "signer" || failure == "acl") {
                        Err(invalid_runtime())
                    } else {
                        Ok(test_runtime(candidate))
                    }
                })
                .is_err()
            );
            assert_eq!(validations.get(), 1);
        }
    }

    #[derive(Clone)]
    enum TokenPayload {
        None,
        Bytes(Vec<u8>),
        IntegrityLabel(u32),
    }

    #[derive(Clone)]
    struct TokenCallScript {
        class: TokenInformationClass,
        succeeded: bool,
        returned: u32,
        failure_error: u32,
        payload: TokenPayload,
    }

    struct TokenInformationFake {
        scripts: RefCell<VecDeque<TokenCallScript>>,
    }

    impl TokenInformationFake {
        fn new(scripts: impl IntoIterator<Item = TokenCallScript>) -> Self {
            Self {
                scripts: RefCell::new(scripts.into_iter().collect()),
            }
        }

        fn exhausted(&self) -> bool {
            self.scripts.borrow().is_empty()
        }
    }

    impl TokenInformationProvider for TokenInformationFake {
        fn query(
            &self,
            class: TokenInformationClass,
            output: TokenInformationOutput<'_>,
        ) -> TokenInformationResult {
            let Some(script) = self.scripts.borrow_mut().pop_front() else {
                return TokenInformationResult {
                    succeeded: false,
                    returned: 0,
                    failure_error: Some(ERROR_SUCCESS),
                };
            };
            if script.class != class
                || (script.succeeded && !write_token_payload(output, script.payload))
            {
                return TokenInformationResult {
                    succeeded: false,
                    returned: 0,
                    failure_error: Some(ERROR_SUCCESS),
                };
            }
            TokenInformationResult {
                succeeded: script.succeeded,
                returned: script.returned,
                failure_error: (!script.succeeded).then_some(script.failure_error),
            }
        }
    }

    fn write_token_payload(output: TokenInformationOutput<'_>, payload: TokenPayload) -> bool {
        match (output, payload) {
            (TokenInformationOutput::None, TokenPayload::None) => true,
            (TokenInformationOutput::Bytes(output), TokenPayload::Bytes(payload))
                if output.len() == payload.len() =>
            {
                output.copy_from_slice(&payload);
                true
            }
            (TokenInformationOutput::Integrity(buffer), TokenPayload::IntegrityLabel(rid)) => {
                let bytes = buffer.bytes_mut();
                if bytes.len() < LABEL_BYTES + SID_BYTES {
                    return false;
                }
                let base = bytes.as_mut_ptr();
                // SAFETY: bytes is backed by aligned IntegrityBuffer storage;
                // the label and 12-byte SID are fully inside its allocation.
                unsafe {
                    let label = base.cast::<TOKEN_MANDATORY_LABEL>();
                    let sid = base.add(LABEL_BYTES);
                    (*label).Label.Sid = sid.cast();
                    let sid_bytes = std::slice::from_raw_parts_mut(sid, SID_BYTES);
                    sid_bytes.copy_from_slice(&[1, 1, 0, 0, 0, 0, 0, 16, 0, 32, 0, 0]);
                    sid_bytes[8..12].copy_from_slice(&rid.to_le_bytes());
                }
                true
            }
            _ => false,
        }
    }

    fn token_success(
        class: TokenInformationClass,
        returned: u32,
        payload: TokenPayload,
    ) -> TokenCallScript {
        TokenCallScript {
            class,
            succeeded: true,
            returned,
            failure_error: ERROR_SUCCESS,
            payload,
        }
    }

    fn token_failure(
        class: TokenInformationClass,
        returned: u32,
        failure_error: u32,
    ) -> TokenCallScript {
        TokenCallScript {
            class,
            succeeded: false,
            returned,
            failure_error,
            payload: TokenPayload::None,
        }
    }

    fn normal_token_scripts(elevation_type: i32) -> Vec<TokenCallScript> {
        let integrity_length = u32::try_from(LABEL_BYTES + SID_BYTES).expect("test length fits");
        vec![
            token_success(
                TokenInformationClass::ElevationType,
                size_of::<i32>() as u32,
                TokenPayload::Bytes(elevation_type.to_ne_bytes().to_vec()),
            ),
            token_success(
                TokenInformationClass::Elevation,
                size_of::<u32>() as u32,
                TokenPayload::Bytes(0u32.to_ne_bytes().to_vec()),
            ),
            token_failure(
                TokenInformationClass::IntegrityLevel,
                integrity_length,
                ERROR_INSUFFICIENT_BUFFER,
            ),
            token_success(
                TokenInformationClass::IntegrityLevel,
                integrity_length,
                TokenPayload::IntegrityLabel(0x2000),
            ),
        ]
    }

    fn valid_integrity_buffer(rid: u32) -> (Vec<usize>, usize) {
        let mut storage = vec![0usize; 4];
        let base = storage.as_mut_ptr().cast::<u8>();
        // SAFETY: storage is aligned for the label and has room for a 12-byte SID.
        unsafe {
            let label = base.cast::<TOKEN_MANDATORY_LABEL>();
            let sid = base.add(LABEL_BYTES);
            (*label).Label.Sid = sid.cast();
            let bytes = std::slice::from_raw_parts_mut(sid, SID_BYTES);
            bytes.copy_from_slice(&[1, 1, 0, 0, 0, 0, 0, 16, 0, 32, 0, 0]);
            bytes[8..12].copy_from_slice(&rid.to_le_bytes());
        }
        (storage, LABEL_BYTES + SID_BYTES)
    }

    struct SidApiFake {
        valid: bool,
        length: Option<usize>,
        authority: Option<[u8; 6]>,
        count: Option<u8>,
        rid: Option<u32>,
        calls: Cell<u32>,
    }

    impl SidApiFake {
        fn valid() -> Self {
            Self {
                valid: true,
                length: Some(SID_BYTES),
                authority: Some(SECURITY_MANDATORY_LABEL_AUTHORITY.Value),
                count: Some(1),
                rid: Some(0x2000),
                calls: Cell::new(0),
            }
        }
    }

    impl SidApi for SidApiFake {
        fn is_valid(&self, _: *mut core::ffi::c_void) -> bool {
            self.calls.set(self.calls.get() + 1);
            self.valid
        }
        fn length(&self, _: *mut core::ffi::c_void) -> Option<usize> {
            self.length
        }
        fn authority(&self, _: *mut core::ffi::c_void) -> Option<[u8; 6]> {
            self.authority
        }
        fn count(&self, _: *mut core::ffi::c_void) -> Option<u8> {
            self.count
        }
        fn rid(&self, _: *mut core::ffi::c_void, _: u32) -> Option<u32> {
            self.rid
        }
    }

    fn parse_with(
        storage: &mut [usize],
        actual: usize,
        api: &impl SidApi,
    ) -> Result<u32, PlatformError> {
        let base = storage.as_mut_ptr().cast::<u8>();
        // SAFETY: test storage has TOKEN_MANDATORY_LABEL alignment.
        let label = unsafe { &*base.cast::<TOKEN_MANDATORY_LABEL>() };
        integrity_rid_from_label_with(label, base as usize, actual, api)
    }

    fn set_sid(storage: &mut [usize], sid: *mut core::ffi::c_void) {
        let base = storage.as_mut_ptr().cast::<u8>();
        // SAFETY: label is the first object in owned, aligned test storage.
        unsafe { (*base.cast::<TOKEN_MANDATORY_LABEL>()).Label.Sid = sid };
    }

    #[test]
    fn elevation_type_query_failure_is_rejected() {
        let provider = TokenInformationFake::new([token_failure(
            TokenInformationClass::ElevationType,
            0,
            ERROR_SUCCESS,
        )]);
        assert!(reject_elevated_execution_with(&provider).is_err());
    }

    #[test]
    fn full_elevation_type_is_rejected() {
        let provider = TokenInformationFake::new([token_success(
            TokenInformationClass::ElevationType,
            size_of::<i32>() as u32,
            TokenPayload::Bytes(
                windows_sys::Win32::Security::TokenElevationTypeFull
                    .to_ne_bytes()
                    .to_vec(),
            ),
        )]);
        assert!(reject_elevated_execution_with(&provider).is_err());
    }

    #[test]
    fn malformed_elevation_type_is_rejected() {
        let provider = TokenInformationFake::new([token_success(
            TokenInformationClass::ElevationType,
            size_of::<i32>() as u32,
            TokenPayload::Bytes(99i32.to_ne_bytes().to_vec()),
        )]);
        assert!(reject_elevated_execution_with(&provider).is_err());
    }

    #[test]
    fn elevation_flag_query_failure_is_rejected() {
        let provider = TokenInformationFake::new([
            token_success(
                TokenInformationClass::ElevationType,
                size_of::<i32>() as u32,
                TokenPayload::Bytes(1i32.to_ne_bytes().to_vec()),
            ),
            token_failure(TokenInformationClass::Elevation, 0, ERROR_SUCCESS),
        ]);
        assert!(reject_elevated_execution_with(&provider).is_err());
    }

    #[test]
    fn elevated_token_flag_is_rejected() {
        let provider = TokenInformationFake::new([
            token_success(
                TokenInformationClass::ElevationType,
                size_of::<i32>() as u32,
                TokenPayload::Bytes(1i32.to_ne_bytes().to_vec()),
            ),
            token_success(
                TokenInformationClass::Elevation,
                size_of::<u32>() as u32,
                TokenPayload::Bytes(1u32.to_ne_bytes().to_vec()),
            ),
        ]);
        assert!(reject_elevated_execution_with(&provider).is_err());
    }

    #[test]
    fn default_non_elevated_token_is_allowed() {
        let provider = TokenInformationFake::new(normal_token_scripts(TokenElevationTypeDefault));
        assert!(reject_elevated_execution_with(&provider).is_ok());
        assert!(provider.exhausted());
    }

    #[test]
    fn limited_non_elevated_token_is_allowed() {
        let provider = TokenInformationFake::new(normal_token_scripts(TokenElevationTypeLimited));
        assert!(reject_elevated_execution_with(&provider).is_ok());
        assert!(provider.exhausted());
    }

    #[test]
    fn integrity_size_query_true_is_rejected() {
        let mut scripts = normal_token_scripts(1);
        scripts[2] = token_success(
            TokenInformationClass::IntegrityLevel,
            28,
            TokenPayload::None,
        );
        scripts.truncate(3);
        let provider = TokenInformationFake::new(scripts);
        assert!(reject_elevated_execution_with(&provider).is_err());
    }

    #[test]
    fn integrity_size_query_wrong_error_is_rejected() {
        let mut scripts = normal_token_scripts(1);
        scripts[2] = token_failure(TokenInformationClass::IntegrityLevel, 28, ERROR_SUCCESS);
        scripts.truncate(3);
        let provider = TokenInformationFake::new(scripts);
        assert!(reject_elevated_execution_with(&provider).is_err());
    }

    #[test]
    fn integrity_required_length_zero_is_rejected() {
        let mut scripts = normal_token_scripts(1);
        scripts[2] = token_failure(
            TokenInformationClass::IntegrityLevel,
            0,
            ERROR_INSUFFICIENT_BUFFER,
        );
        scripts.truncate(3);
        let provider = TokenInformationFake::new(scripts);
        assert!(reject_elevated_execution_with(&provider).is_err());
    }

    #[test]
    fn integrity_required_length_short_is_rejected() {
        let mut scripts = normal_token_scripts(1);
        scripts[2] = token_failure(
            TokenInformationClass::IntegrityLevel,
            (LABEL_BYTES - 1) as u32,
            ERROR_INSUFFICIENT_BUFFER,
        );
        scripts.truncate(3);
        let provider = TokenInformationFake::new(scripts);
        assert!(reject_elevated_execution_with(&provider).is_err());
    }

    #[test]
    fn integrity_data_query_failure_is_rejected() {
        let mut scripts = normal_token_scripts(1);
        scripts[3] = token_failure(TokenInformationClass::IntegrityLevel, 0, ERROR_SUCCESS);
        let provider = TokenInformationFake::new(scripts);
        assert!(reject_elevated_execution_with(&provider).is_err());
    }

    #[test]
    fn integrity_actual_length_zero_is_rejected() {
        let mut scripts = normal_token_scripts(1);
        scripts[3] = token_success(
            TokenInformationClass::IntegrityLevel,
            0,
            TokenPayload::IntegrityLabel(0x2000),
        );
        let provider = TokenInformationFake::new(scripts);
        assert!(reject_elevated_execution_with(&provider).is_err());
    }

    #[test]
    fn integrity_actual_length_short_is_rejected() {
        let mut scripts = normal_token_scripts(1);
        scripts[3] = token_success(
            TokenInformationClass::IntegrityLevel,
            (LABEL_BYTES - 1) as u32,
            TokenPayload::IntegrityLabel(0x2000),
        );
        let provider = TokenInformationFake::new(scripts);
        assert!(reject_elevated_execution_with(&provider).is_err());
    }

    #[test]
    fn integrity_actual_length_exceeding_allocation_is_rejected() {
        let mut scripts = normal_token_scripts(1);
        scripts[3] = token_success(
            TokenInformationClass::IntegrityLevel,
            33,
            TokenPayload::IntegrityLabel(0x2000),
        );
        let provider = TokenInformationFake::new(scripts);
        assert!(reject_elevated_execution_with(&provider).is_err());
    }

    #[test]
    fn integrity_normal_payload_reaches_shared_sid_parser() {
        let provider = TokenInformationFake::new(normal_token_scripts(1));
        assert!(reject_elevated_execution_with(&provider).is_ok());
        assert!(provider.exhausted());
    }

    #[test]
    fn malformed_integrity_result_has_no_medium_rid_fallback() {
        let mut scripts = normal_token_scripts(1);
        scripts[3] = token_success(
            TokenInformationClass::IntegrityLevel,
            0,
            TokenPayload::IntegrityLabel(0x2000),
        );
        let provider = TokenInformationFake::new(scripts);
        assert!(reject_elevated_execution_with(&provider).is_err());
    }

    #[test]
    fn required_length_zero_is_rejected() {
        assert!(validate_required_integrity_length(0).is_err());
    }

    #[test]
    fn required_length_short_is_rejected() {
        assert!(validate_required_integrity_length((LABEL_BYTES - 1) as u32).is_err());
    }

    #[test]
    fn actual_length_short_is_rejected() {
        assert!(validate_actual_integrity_length(LABEL_BYTES - 1, 32).is_err());
    }

    #[test]
    fn actual_length_overflow_is_rejected() {
        assert!(validate_actual_integrity_length(33, 32).is_err());
    }

    #[test]
    fn null_sid_is_rejected_before_sid_api() {
        let (mut storage, actual) = valid_integrity_buffer(0x2000);
        set_sid(&mut storage, std::ptr::null_mut());
        let api = SidApiFake::valid();
        assert!(parse_with(&mut storage, actual, &api).is_err());
        assert_eq!(api.calls.get(), 0);
    }

    #[test]
    fn sid_pointer_before_buffer_is_rejected_before_sid_api() {
        let (mut storage, actual) = valid_integrity_buffer(0x2000);
        let base = storage.as_mut_ptr().cast::<u8>();
        set_sid(&mut storage, base.wrapping_sub(1).cast());
        let api = SidApiFake::valid();
        assert!(parse_with(&mut storage, actual, &api).is_err());
        assert_eq!(api.calls.get(), 0);
    }

    #[test]
    fn sid_pointer_at_actual_span_end_is_rejected_before_sid_api() {
        let (mut storage, actual) = valid_integrity_buffer(0x2000);
        let base = storage.as_mut_ptr().cast::<u8>();
        // SAFETY: actual points at the end of this owned allocation's actual span.
        set_sid(&mut storage, unsafe { base.add(actual).cast() });
        let api = SidApiFake::valid();
        assert!(parse_with(&mut storage, actual, &api).is_err());
        assert_eq!(api.calls.get(), 0);
    }

    #[test]
    fn sid_pointer_after_actual_span_is_rejected_before_sid_api() {
        let (mut storage, actual) = valid_integrity_buffer(0x2000);
        let base = storage.as_mut_ptr().cast::<u8>();
        let beyond_allocation = storage.len() * size_of::<usize>() + 1;
        set_sid(&mut storage, base.wrapping_add(beyond_allocation).cast());
        let api = SidApiFake::valid();
        assert!(parse_with(&mut storage, actual, &api).is_err());
        assert_eq!(api.calls.get(), 0);
    }

    #[test]
    fn sid_inside_allocation_outside_actual_span_is_rejected_before_sid_api() {
        let (mut storage, _) = valid_integrity_buffer(0x2000);
        let base = storage.as_mut_ptr().cast::<u8>();
        let actual = LABEL_BYTES + 8;
        // SAFETY: this pointer remains in the backing allocation but is after actual.
        set_sid(&mut storage, unsafe {
            base.add(LABEL_BYTES + SID_BYTES).cast()
        });
        let api = SidApiFake::valid();
        assert!(parse_with(&mut storage, actual, &api).is_err());
        assert_eq!(api.calls.get(), 0);
    }

    #[test]
    fn truncated_sid_fixed_header_is_rejected_before_sid_api() {
        let (mut storage, _) = valid_integrity_buffer(0x2000);
        let api = SidApiFake::valid();
        assert!(parse_with(&mut storage, LABEL_BYTES + 7, &api).is_err());
        assert_eq!(api.calls.get(), 0);
    }

    #[test]
    fn truncated_sid_subauthority_body_is_rejected_before_sid_api() {
        let (mut storage, actual) = valid_integrity_buffer(0x2000);
        let api = SidApiFake::valid();
        assert!(parse_with(&mut storage, actual - 1, &api).is_err());
        assert_eq!(api.calls.get(), 0);
    }

    #[test]
    fn zero_sid_subauthority_count_is_rejected() {
        let (mut storage, actual) = valid_integrity_buffer(0x2000);
        let base = storage.as_mut_ptr().cast::<u8>();
        // SAFETY: the count byte is inside the valid test SID.
        unsafe { *base.add(LABEL_BYTES + 1) = 0 };
        let api = SidApiFake::valid();
        assert!(parse_with(&mut storage, actual, &api).is_err());
    }

    #[test]
    fn bad_sid_revision_is_rejected() {
        let (mut storage, actual) = valid_integrity_buffer(0x2000);
        let base = storage.as_mut_ptr().cast::<u8>();
        // SAFETY: the revision byte is inside the valid test SID.
        unsafe { *base.add(LABEL_BYTES) = 2 };
        let api = SidApiFake::valid();
        assert!(parse_with(&mut storage, actual, &api).is_err());
    }

    #[test]
    fn bad_mandatory_authority_is_rejected() {
        let (mut storage, actual) = valid_integrity_buffer(0x2000);
        let mut api = SidApiFake::valid();
        api.authority = Some([0, 0, 0, 0, 0, 5]);
        assert!(parse_with(&mut storage, actual, &api).is_err());
    }

    #[test]
    fn medium_integrity_rid_is_allowed() {
        let (mut storage, actual) = valid_integrity_buffer(0x2000);
        let api = SidApiFake::valid();
        assert!(matches!(parse_with(&mut storage, actual, &api), Ok(0x2000)));
        assert!(reject_integrity_rid(0x2000).is_ok());
    }

    #[test]
    fn high_integrity_rid_is_rejected() {
        assert!(reject_integrity_rid(0x3000).is_err());
    }

    #[test]
    fn system_integrity_rid_is_rejected() {
        assert!(reject_integrity_rid(0x4000).is_err());
    }

    #[test]
    fn protected_integrity_rid_is_rejected() {
        assert!(reject_integrity_rid(0x5000).is_err());
    }

    #[test]
    fn is_valid_sid_false_is_rejected() {
        let (mut storage, actual) = valid_integrity_buffer(0x2000);
        let mut api = SidApiFake::valid();
        api.valid = false;
        assert!(parse_with(&mut storage, actual, &api).is_err());
    }

    #[test]
    fn get_length_sid_mismatch_is_rejected() {
        let (mut storage, actual) = valid_integrity_buffer(0x2000);
        let mut api = SidApiFake::valid();
        api.length = Some(16);
        assert!(parse_with(&mut storage, actual, &api).is_err());
    }

    #[test]
    fn official_git_runtime_discovery_emits_first_failure_trace() {
        let root = PathBuf::from(r"C:\Program Files\Git");
        let cmd = root.join("cmd");
        let bin = root.join("mingw64").join("bin");
        let git_core = root.join("mingw64").join("libexec").join("git-core");
        let executable = cmd.join("git.exe");
        let system = system_directory().expect("system directory");
        eprintln!("token_gate={:?}", reject_elevated_execution());
        for (role, path, directory) in [
            ("root", &root, true),
            ("cmd", &cmd, true),
            ("bin", &bin, true),
            ("git_core", &git_core, true),
            ("git_exe", &executable, false),
            ("system32", &system, true),
        ] {
            let descriptor = security_descriptor(path);
            let result = descriptor.and_then(|descriptor| {
                let token = impersonation_token()?;
                maximum_allowed(descriptor.as_ptr(), token.as_raw_handle())
            });
            eprintln!("role={role} directory={directory} maximum_allowed={result:?}");
            eprintln!(
                "role={role} acl_gate={:?}",
                reject_current_user_mutation(path, directory)
            );
            eprintln!(
                "role={role} parent_gate={:?}",
                reject_parent_replacement(path)
            );
        }
        eprintln!("authenticode={:?}", verify_authenticode(&executable));
        eprintln!("discovery={:?}", TrustedGitRuntime::discover());
    }
}
