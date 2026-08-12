#![cfg(windows)]

use std::path::Path;

use chatoms_platform::{
    PlatformError, SecureAppPaths, path::WindowsPathResolver,
    permissions::WindowsPermissionManager, preflight::TrustedPreflightWorkingDirectory,
};
use chatoms_ports::path::AppPathResolver;

fn resolver_in(temp: &Path) -> WindowsPathResolver {
    let resolver = WindowsPathResolver::from_base_dir_for_test(temp.to_path_buf())
        .expect("absolute local base");
    SecureAppPaths::prepare(&resolver, &WindowsPermissionManager)
        .expect("app-owned layout including temp_dir prepares first");
    resolver
}

#[test]
fn prepare_creates_a_fixed_directory_under_temp_never_task_or_project_bound() {
    let temp = tempfile::tempdir().expect("independent test root");
    let resolver = resolver_in(temp.path());

    let trusted = TrustedPreflightWorkingDirectory::prepare(&resolver, &WindowsPermissionManager)
        .expect("first prepare");

    assert!(trusted.path().is_dir());
    assert_eq!(
        trusted.path(),
        resolver
            .temp_dir()
            .expect("temp dir")
            .join("provider-preflight")
    );
    assert!(
        trusted
            .path()
            .starts_with(resolver.temp_dir().expect("temp dir"))
    );
}

#[test]
fn prepare_is_idempotent_and_revalidate_succeeds_on_the_unchanged_directory() {
    let temp = tempfile::tempdir().expect("independent test root");
    let resolver = resolver_in(temp.path());

    let first = TrustedPreflightWorkingDirectory::prepare(&resolver, &WindowsPermissionManager)
        .expect("first prepare");
    let second = TrustedPreflightWorkingDirectory::prepare(&resolver, &WindowsPermissionManager)
        .expect("second prepare");

    assert_eq!(first.path(), second.path());
    first.revalidate().expect("unchanged directory revalidates");
    second
        .revalidate()
        .expect("unchanged directory revalidates");
}

#[test]
fn file_occupying_the_preflight_path_is_rejected_without_deletion() {
    let temp = tempfile::tempdir().expect("independent test root");
    let resolver = resolver_in(temp.path());
    let occupying = resolver
        .temp_dir()
        .expect("temp dir")
        .join("provider-preflight");
    std::fs::write(&occupying, b"not a directory").expect("occupying file");

    let error = TrustedPreflightWorkingDirectory::prepare(&resolver, &WindowsPermissionManager)
        .expect_err("file collision must fail closed");
    assert!(
        matches!(error, PlatformError::Path(_)),
        "expected a path failure, got {error:?}"
    );
    assert_eq!(
        std::fs::read(&occupying).expect("occupying file retained"),
        b"not a directory"
    );
}

#[test]
fn revalidate_fails_after_the_directory_is_removed() {
    let temp = tempfile::tempdir().expect("independent test root");
    let resolver = resolver_in(temp.path());
    let trusted = TrustedPreflightWorkingDirectory::prepare(&resolver, &WindowsPermissionManager)
        .expect("prepare");

    std::fs::remove_dir(trusted.path()).expect("remove prepared directory");

    trusted
        .revalidate()
        .expect_err("removed directory must fail revalidation, never a partial success");
}

#[test]
fn revalidate_fails_after_the_directory_is_rebound_to_a_different_object() {
    let temp = tempfile::tempdir().expect("independent test root");
    let resolver = resolver_in(temp.path());
    let trusted = TrustedPreflightWorkingDirectory::prepare(&resolver, &WindowsPermissionManager)
        .expect("prepare");

    let replacement = temp.path().join("replacement-target");
    std::fs::create_dir(&replacement).expect("replacement target");
    std::fs::remove_dir(trusted.path()).expect("clear prepared directory for rebinding");
    let output = std::process::Command::new("cmd.exe")
        .args(["/d", "/c", "mklink", "/J"])
        .arg(trusted.path())
        .arg(&replacement)
        .output()
        .expect("run mklink junction fixture");
    assert!(
        output.status.success(),
        "junction fixture is mandatory: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    trusted
        .revalidate()
        .expect_err("a rebound reparse point must fail revalidation, never a partial success");
}
