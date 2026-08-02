use std::{
    ffi::c_void,
    mem::size_of,
    os::windows::ffi::OsStrExt,
    path::Path,
    ptr::{null, null_mut},
};

use chatoms_platform::{
    SecureAppPaths, path::WindowsPathResolver, permissions::WindowsPermissionManager,
};
use chatoms_ports::{
    path::TaskId,
    permissions::{FilesystemPermissionManager, PermissionErrorCode, PermissionStatus},
};
use windows_sys::Win32::{
    Foundation::{ERROR_SUCCESS, GENERIC_ALL, LocalFree},
    Security::{
        ACL,
        Authorization::{
            EXPLICIT_ACCESS_W, GRANT_ACCESS, GetNamedSecurityInfoW, NO_MULTIPLE_TRUSTEE,
            SE_FILE_OBJECT, SetEntriesInAclW, SetNamedSecurityInfoW, TRUSTEE_IS_SID,
            TRUSTEE_IS_WELL_KNOWN_GROUP, TRUSTEE_W,
        },
        CreateWellKnownSid, DACL_SECURITY_INFORMATION, PROTECTED_DACL_SECURITY_INFORMATION,
        SECURITY_MAX_SID_SIZE, SUB_CONTAINERS_AND_OBJECTS_INHERIT, WinBuiltinAdministratorsSid,
        WinBuiltinUsersSid,
    },
};

#[test]
fn directory_acl_is_secure_and_repeated_application_is_idempotent() {
    let temp = tempfile::tempdir().expect("independent test root");
    let directory = temp.path().join("secured");
    std::fs::create_dir(&directory).expect("test child directory");
    let manager = WindowsPermissionManager;

    manager
        .secure_directory(&directory)
        .expect("apply user and SYSTEM ACL");
    assert_eq!(
        manager.verify_directory(&directory).expect("verify ACL"),
        PermissionStatus::Secure
    );
    manager
        .secure_directory(&directory)
        .expect("idempotent ACL application");
    assert_eq!(
        manager.verify_directory(&directory).expect("verify again"),
        PermissionStatus::Secure
    );
}

#[test]
fn file_acl_is_secure_and_directory_file_type_mismatches_are_typed() {
    let temp = tempfile::tempdir().expect("independent test root");
    let file = temp.path().join("database.sqlite3");
    std::fs::write(&file, b"fixture").expect("test file");
    let manager = WindowsPermissionManager;

    manager.secure_file(&file).expect("secure test file");
    assert_eq!(
        manager.verify_file(&file).expect("verify file ACL"),
        PermissionStatus::Secure
    );
    assert_eq!(
        manager
            .secure_directory(&file)
            .expect_err("file is not a directory")
            .code(),
        PermissionErrorCode::InvariantViolation
    );
}

#[test]
fn missing_acl_target_returns_typed_read_error() {
    let temp = tempfile::tempdir().expect("independent test root");
    let missing = temp.path().join("missing");
    let error = WindowsPermissionManager
        .verify_directory(&missing)
        .expect_err("missing target cannot be inspected");
    assert_eq!(error.code(), PermissionErrorCode::ReadAclFailed);
}

#[test]
fn broad_and_unapproved_explicit_writable_principals_are_not_secure() {
    let temp = tempfile::tempdir().expect("independent test root");
    let manager = WindowsPermissionManager;

    let broad = temp.path().join("broad");
    std::fs::create_dir(&broad).expect("broad fixture");
    manager.secure_directory(&broad).expect("secure baseline");
    add_well_known_allow(&broad, WinBuiltinUsersSid);
    assert_eq!(
        manager.verify_directory(&broad).expect("inspect broad ACE"),
        PermissionStatus::Insecure
    );
    manager
        .secure_directory(&broad)
        .expect("remove broad ACE by replacing protected DACL");
    assert_eq!(
        manager
            .verify_directory(&broad)
            .expect("verify repaired ACL"),
        PermissionStatus::Secure
    );

    let unapproved = temp.path().join("unapproved");
    std::fs::create_dir(&unapproved).expect("unknown fixture");
    manager
        .secure_directory(&unapproved)
        .expect("secure baseline");
    add_well_known_allow(&unapproved, WinBuiltinAdministratorsSid);
    assert_eq!(
        manager
            .verify_directory(&unapproved)
            .expect("inspect unapproved writable ACE"),
        PermissionStatus::Insecure
    );
}

#[test]
fn inherited_broad_access_is_removed_when_child_is_explicitly_secured() {
    let temp = tempfile::tempdir().expect("independent test root");
    let parent = temp.path().join("parent");
    std::fs::create_dir(&parent).expect("parent fixture");
    add_well_known_allow(&parent, WinBuiltinUsersSid);
    let child = parent.join("child");
    std::fs::create_dir(&child).expect("child inherits parent ACL");
    let manager = WindowsPermissionManager;

    assert_ne!(
        manager
            .verify_directory(&child)
            .expect("inherited ACL can be inspected"),
        PermissionStatus::Secure
    );
    manager
        .secure_directory(&child)
        .expect("protect child and remove inherited broad ACE");
    assert_eq!(
        manager.verify_directory(&child).expect("secure child"),
        PermissionStatus::Secure
    );
}

#[test]
fn secure_app_paths_applies_acl_only_below_independent_temp_root() {
    let temp = tempfile::tempdir().expect("independent test root");
    let resolver = WindowsPathResolver::from_base_dir_for_test(temp.path().to_path_buf())
        .expect("test resolver");
    let manager = WindowsPermissionManager;

    let paths = SecureAppPaths::prepare(&resolver, &manager).expect("secure app layout");
    for directory in [
        &paths.app_root,
        &paths.data_dir,
        &paths.logs_dir,
        &paths.artifacts_dir,
        &paths.temp_dir,
    ] {
        assert_eq!(
            manager
                .verify_directory(directory)
                .expect("secure directory"),
            PermissionStatus::Secure
        );
        assert!(directory.starts_with(temp.path()));
    }
    assert!(!paths.database_path.exists());

    let task_id = TaskId::new();
    let artifact = SecureAppPaths::prepare_task_artifact_dir(&resolver, &manager, task_id)
        .expect("secure task artifact directory");
    let temporary = SecureAppPaths::prepare_task_temp_dir(&resolver, &manager, task_id)
        .expect("secure task temp directory");
    for directory in [&artifact, &temporary] {
        assert_eq!(
            manager
                .verify_directory(directory)
                .expect("secure task path"),
            PermissionStatus::Secure
        );
        assert!(directory.starts_with(temp.path()));
    }
}

fn add_well_known_allow(path: &Path, kind: i32) {
    let mut sid_storage =
        vec![0usize; (SECURITY_MAX_SID_SIZE as usize).div_ceil(size_of::<usize>())];
    let mut sid_size = SECURITY_MAX_SID_SIZE;
    // SAFETY: storage has SECURITY_MAX_SID_SIZE bytes and kind is an absolute well-known SID.
    let success = unsafe {
        CreateWellKnownSid(
            kind,
            null_mut(),
            sid_storage.as_mut_ptr().cast(),
            &mut sid_size,
        )
    };
    assert_ne!(success, 0, "create well-known SID fixture");

    let wide_path: Vec<u16> = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let mut old_acl: *mut ACL = null_mut();
    let mut descriptor = null_mut();
    // SAFETY: path is NUL-terminated and output pointers remain valid until LocalFree.
    let result = unsafe {
        GetNamedSecurityInfoW(
            wide_path.as_ptr(),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION,
            null_mut(),
            null_mut(),
            &mut old_acl,
            null_mut(),
            &mut descriptor,
        )
    };
    assert_eq!(result, ERROR_SUCCESS, "read fixture ACL");
    assert!(!descriptor.is_null(), "security descriptor fixture");

    let entry = EXPLICIT_ACCESS_W {
        grfAccessPermissions: GENERIC_ALL,
        grfAccessMode: GRANT_ACCESS,
        grfInheritance: SUB_CONTAINERS_AND_OBJECTS_INHERIT,
        Trustee: TRUSTEE_W {
            pMultipleTrustee: null_mut(),
            MultipleTrusteeOperation: NO_MULTIPLE_TRUSTEE,
            TrusteeForm: TRUSTEE_IS_SID,
            TrusteeType: TRUSTEE_IS_WELL_KNOWN_GROUP,
            ptstrName: sid_storage.as_mut_ptr().cast(),
        },
    };
    let mut new_acl = null_mut();
    // SAFETY: entry and old_acl contain valid SID/ACL pointers for the duration of this call.
    let result = unsafe { SetEntriesInAclW(1, &entry, old_acl, &mut new_acl) };
    assert_eq!(result, ERROR_SUCCESS, "extend fixture ACL");
    assert!(!new_acl.is_null(), "new fixture ACL");

    // SAFETY: new_acl is valid and path remains NUL-terminated. Only the protected DACL changes.
    let result = unsafe {
        SetNamedSecurityInfoW(
            wide_path.as_ptr(),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
            null_mut(),
            null_mut(),
            new_acl,
            null(),
        )
    };
    // SAFETY: allocations came from Windows local allocation APIs and are each freed once.
    unsafe {
        LocalFree(new_acl.cast::<c_void>());
        LocalFree(descriptor);
    }
    assert_eq!(result, ERROR_SUCCESS, "apply fixture ACL");
}
