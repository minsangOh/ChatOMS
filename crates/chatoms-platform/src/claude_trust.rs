//! Windows-only verification of a user-designated Claude Code executable.
//!
//! This module never searches `PATH` or a fixed candidate list. The caller
//! supplies a single profile-specific absolute path; this module only
//! confirms that path is safe to trust before anything executes it. Every
//! check is fail-closed: any error, ambiguity, or unverifiable state is
//! rejected rather than treated as a partial or degraded success.

use std::{
    ffi::OsStr,
    fs,
    mem::{size_of, zeroed},
    os::windows::{ffi::OsStrExt, io::AsRawHandle},
    path::{Path, PathBuf},
    ptr::null_mut,
};

use chatoms_ports::path::{PathError, PathErrorCode};
use windows_sys::Win32::{
    Foundation::HANDLE,
    Security::{
        Cryptography::{CERT_CONTEXT, CERT_NAME_SIMPLE_DISPLAY_TYPE, CertGetNameStringW},
        WinTrust::{
            WINTRUST_ACTION_GENERIC_VERIFY_V2, WINTRUST_DATA, WINTRUST_DATA_0, WINTRUST_FILE_INFO,
            WTD_CACHE_ONLY_URL_RETRIEVAL, WTD_CHOICE_FILE, WTD_REVOCATION_CHECK_CHAIN_EXCLUDE_ROOT,
            WTD_REVOKE_WHOLECHAIN, WTD_STATEACTION_CLOSE, WTD_STATEACTION_VERIFY, WTD_UI_NONE,
            WTHelperGetProvCertFromChain, WTHelperGetProvSignerFromChain,
            WTHelperProvDataFromStateData, WinVerifyTrust,
        },
    },
    Storage::FileSystem::{FILE_ID_INFO, FileIdInfo, GetFileInformationByHandleEx},
};

use crate::PlatformError;

/// Official Windows Authenticode signer for the Claude Code CLI, per
/// docs/DECISIONS.md's "Claude executable trust" decision.
const ANTHROPIC_SIGNER_NAME: &str = "Anthropic, PBC";

/// A Claude Code executable whose path and Authenticode signer have been
/// verified. Holding one is proof the checks passed at construction time;
/// call [`Self::revalidate`] again immediately before every use.
#[derive(Clone, Debug)]
pub struct TrustedClaudeExecutable {
    path: PathBuf,
    identity: String,
}

impl TrustedClaudeExecutable {
    /// Verifies a user-designated absolute path from scratch: it must be a
    /// regular file, not a reparse point or network path, and its
    /// Authenticode signer must be exactly [`ANTHROPIC_SIGNER_NAME`]. Any
    /// failure is fail-closed.
    pub fn verify(path: &Path) -> Result<Self, PlatformError> {
        let canonical = canonical_single_file(path)?;
        verify_authenticode_signer(&canonical, ANTHROPIC_SIGNER_NAME)?;
        let identity = file_identity(&canonical)?;
        Ok(Self {
            path: canonical,
            identity,
        })
    }

    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Re-checks the file at this trusted path still has the identity
    /// captured at [`Self::verify`] time and re-verifies its Authenticode
    /// signer. Callers must invoke this immediately before every execution;
    /// a path can be replaced after the initial check.
    pub fn revalidate(&self) -> Result<(), PlatformError> {
        if file_identity(&self.path)? != self.identity {
            return Err(invalid_executable());
        }
        verify_authenticode_signer(&self.path, ANTHROPIC_SIGNER_NAME)
    }
}

fn canonical_single_file(path: &Path) -> Result<PathBuf, PlatformError> {
    if !path.is_absolute() {
        return Err(invalid_executable());
    }
    let metadata = fs::symlink_metadata(path).map_err(path_error)?;
    if !metadata.is_file() || is_reparse(&metadata) || is_network_path(path) {
        return Err(invalid_executable());
    }
    let canonical = fs::canonicalize(path).map_err(path_error)?;
    if is_network_path(&canonical) {
        return Err(invalid_executable());
    }
    Ok(canonical)
}

fn verify_authenticode_signer(path: &Path, expected_signer: &str) -> Result<(), PlatformError> {
    let wide_path = wide_os(path.as_os_str());
    let mut file = WINTRUST_FILE_INFO {
        cbStruct: u32::try_from(size_of::<WINTRUST_FILE_INFO>())
            .map_err(|_| invalid_executable())?,
        pcwszFilePath: wide_path.as_ptr(),
        hFile: null_mut::<std::ffi::c_void>() as HANDLE,
        pgKnownSubject: null_mut(),
    };
    let mut data = WINTRUST_DATA {
        cbStruct: u32::try_from(size_of::<WINTRUST_DATA>()).map_err(|_| invalid_executable())?,
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
        return Err(invalid_executable());
    }
    let signer_check = (|| {
        // SAFETY: successful verification created this state data.
        let provider = unsafe { WTHelperProvDataFromStateData(data.hWVTStateData) };
        if provider.is_null() {
            return Err(invalid_executable());
        }
        // SAFETY: index zero requests the leaf signer of the active state.
        let signer = unsafe { WTHelperGetProvSignerFromChain(provider, 0, 0, 0) };
        if signer.is_null() {
            return Err(invalid_executable());
        }
        // SAFETY: index zero requests the leaf certificate of the active signer.
        let certificate = unsafe { WTHelperGetProvCertFromChain(signer, 0) };
        if certificate.is_null() {
            return Err(invalid_executable());
        }
        // SAFETY: certificate belongs to the state kept alive until the close call below.
        let context = unsafe { (*certificate).pCert };
        if context.is_null() {
            return Err(invalid_executable());
        }
        let name = certificate_simple_display_name(context)?;
        if name != expected_signer {
            return Err(invalid_executable());
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
    signer_check
}

/// Reads the certificate's simple display name — the same "signed by X"
/// name Windows Explorer's digital-signature tab and
/// `Get-AuthenticodeSignature` show — rather than a single RDN attribute, so
/// this matches what the official installation docs describe operators
/// verifying by hand.
fn certificate_simple_display_name(context: *const CERT_CONTEXT) -> Result<String, PlatformError> {
    // SAFETY: context is a valid, non-null certificate context borrowed from
    // the active WinVerifyTrust state; this call only reads from it and
    // reports the required buffer length because psznamestring is null.
    let required = unsafe {
        CertGetNameStringW(
            context,
            CERT_NAME_SIMPLE_DISPLAY_TYPE,
            0,
            null_mut(),
            null_mut(),
            0,
        )
    };
    if required <= 1 {
        return Err(invalid_executable());
    }
    let mut buffer = vec![0u16; required as usize];
    // SAFETY: buffer has exactly the capacity CertGetNameStringW reported above.
    let written = unsafe {
        CertGetNameStringW(
            context,
            CERT_NAME_SIMPLE_DISPLAY_TYPE,
            0,
            null_mut(),
            buffer.as_mut_ptr(),
            required,
        )
    };
    if written == 0 {
        return Err(invalid_executable());
    }
    if buffer.last() == Some(&0) {
        buffer.pop();
    }
    String::from_utf16(&buffer).map_err(|_| invalid_executable())
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
            u32::try_from(size_of::<FILE_ID_INFO>()).map_err(|_| invalid_executable())?,
        )
    };
    if ok == 0 {
        return Err(invalid_executable());
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

fn is_reparse(metadata: &fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    metadata.file_attributes() & 0x0000_0400 != 0
}

fn is_network_path(path: &Path) -> bool {
    use std::path::{Component, Prefix};
    matches!(
        path.components().next(),
        Some(Component::Prefix(prefix))
            if matches!(prefix.kind(), Prefix::UNC(..) | Prefix::VerbatimUNC(..) | Prefix::DeviceNS(_))
    )
}

fn wide_os(value: &OsStr) -> Vec<u16> {
    value.encode_wide().chain(std::iter::once(0)).collect()
}

fn path_error(error: std::io::Error) -> PlatformError {
    PlatformError::Path(PathError::with_source(
        PathErrorCode::InvalidBasePath,
        error,
    ))
}

fn invalid_executable() -> PlatformError {
    PlatformError::Path(PathError::new(PathErrorCode::InvalidBasePath))
}

#[cfg(test)]
mod tests {
    use super::TrustedClaudeExecutable;
    use std::path::Path;

    #[test]
    fn relative_path_is_fail_closed() {
        TrustedClaudeExecutable::verify(Path::new("claude.exe"))
            .expect_err("a relative path must never be trusted");
    }

    #[test]
    fn missing_path_is_fail_closed() {
        let missing = std::env::temp_dir().join("chatoms-claude-trust-missing-fixture.exe");
        TrustedClaudeExecutable::verify(&missing)
            .expect_err("a missing file must never be trusted");
    }

    #[test]
    fn directory_is_fail_closed() {
        let dir = tempfile::tempdir().expect("tempdir");
        TrustedClaudeExecutable::verify(dir.path())
            .expect_err("a directory is not an executable file");
    }

    #[test]
    fn unsigned_regular_file_is_fail_closed() {
        let dir = tempfile::tempdir().expect("tempdir");
        let candidate = dir.path().join("claude.exe");
        std::fs::write(&candidate, b"not a signed binary").expect("write fixture");
        TrustedClaudeExecutable::verify(&candidate)
            .expect_err("an unsigned file must never be trusted, even with the right file name");
    }

    #[test]
    fn reparse_point_is_rejected_before_signature_check() {
        let dir = tempfile::tempdir().expect("tempdir");
        let real = dir.path().join("real-claude.exe");
        std::fs::write(&real, b"not a signed binary").expect("write fixture");
        let link = dir.path().join("claude.exe");
        match std::os::windows::fs::symlink_file(&real, &link) {
            Ok(()) => {
                TrustedClaudeExecutable::verify(&link)
                    .expect_err("a symlink must never be trusted even when its target exists");
            }
            Err(_) => {
                // Creating a file symlink needs Developer Mode or an
                // elevated process on this Windows host. Skip rather than
                // fail the suite where that privilege is unavailable.
            }
        }
    }
}
