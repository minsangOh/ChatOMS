mod support;

use support::{TestDatabase, is_constraint_error};

#[test]
fn provider_bindings_executable_path_column_exists_after_migration() {
    let database = TestDatabase::migrated();
    let connection = database.open_raw();
    connection
        .execute(
            "INSERT INTO app_profiles (id, name, created_at_ms, updated_at_ms)
             VALUES ('profile-1', 'Default', 100, 100)",
            [],
        )
        .expect("insert profile");
    connection
        .execute(
            "INSERT INTO provider_bindings (
                id, app_profile_id, provider_kind, display_name,
                executable_path, created_at_ms, updated_at_ms
             ) VALUES ('binding-1', 'profile-1', 'Claude', 'Claude Code',
                'C:/Users/test/claude.exe', 100, 100)",
            [],
        )
        .expect("insert binding with executable_path");
    let path: Option<String> = connection
        .query_row(
            "SELECT executable_path FROM provider_bindings WHERE id = 'binding-1'",
            [],
            |row| row.get(0),
        )
        .expect("read executable_path");
    assert_eq!(path.as_deref(), Some("C:/Users/test/claude.exe"));
}

#[test]
fn provider_bindings_executable_path_null_is_accepted() {
    let database = TestDatabase::migrated();
    let connection = database.open_raw();
    connection
        .execute(
            "INSERT INTO app_profiles (id, name, created_at_ms, updated_at_ms)
             VALUES ('profile-1', 'Default', 100, 100)",
            [],
        )
        .expect("insert profile");
    connection
        .execute(
            "INSERT INTO provider_bindings (
                id, app_profile_id, provider_kind, display_name,
                executable_path, created_at_ms, updated_at_ms
             ) VALUES ('binding-1', 'profile-1', 'Claude', 'Claude Code',
                NULL, 100, 100)",
            [],
        )
        .expect("insert binding with null executable_path");
    let path: Option<String> = connection
        .query_row(
            "SELECT executable_path FROM provider_bindings WHERE id = 'binding-1'",
            [],
            |row| row.get(0),
        )
        .expect("read executable_path");
    assert!(path.is_none());
}

#[test]
fn provider_bindings_empty_executable_path_is_rejected_on_insert() {
    let database = TestDatabase::migrated();
    let connection = database.open_raw();
    connection
        .execute(
            "INSERT INTO app_profiles (id, name, created_at_ms, updated_at_ms)
             VALUES ('profile-1', 'Default', 100, 100)",
            [],
        )
        .expect("insert profile");
    let error = connection
        .execute(
            "INSERT INTO provider_bindings (
                id, app_profile_id, provider_kind, display_name,
                executable_path, created_at_ms, updated_at_ms
             ) VALUES ('binding-1', 'profile-1', 'Claude', 'Claude Code',
                '', 100, 100)",
            [],
        )
        .expect_err("empty executable_path must be rejected");
    assert!(is_constraint_error(&error));
}

#[test]
fn provider_bindings_empty_executable_path_is_rejected_on_update() {
    let database = TestDatabase::migrated();
    let connection = database.open_raw();
    connection
        .execute(
            "INSERT INTO app_profiles (id, name, created_at_ms, updated_at_ms)
             VALUES ('profile-1', 'Default', 100, 100)",
            [],
        )
        .expect("insert profile");
    connection
        .execute(
            "INSERT INTO provider_bindings (
                id, app_profile_id, provider_kind, display_name,
                executable_path, created_at_ms, updated_at_ms
             ) VALUES ('binding-1', 'profile-1', 'Claude', 'Claude Code',
                'C:/claude.exe', 100, 100)",
            [],
        )
        .expect("insert binding");
    let error = connection
        .execute(
            "UPDATE provider_bindings SET executable_path = '' WHERE id = 'binding-1'",
            [],
        )
        .expect_err("empty path update must be rejected");
    assert!(is_constraint_error(&error));
}

#[test]
fn codex_binding_cannot_have_executable_path_on_insert() {
    let database = TestDatabase::migrated();
    let connection = database.open_raw();
    connection
        .execute(
            "INSERT INTO app_profiles (id, name, created_at_ms, updated_at_ms)
             VALUES ('profile-1', 'Default', 100, 100)",
            [],
        )
        .expect("insert profile");
    let error = connection
        .execute(
            "INSERT INTO provider_bindings (
                id, app_profile_id, provider_kind, display_name,
                executable_path, created_at_ms, updated_at_ms
             ) VALUES ('binding-1', 'profile-1', 'Codex', 'Codex CLI',
                'C:/codex.exe', 100, 100)",
            [],
        )
        .expect_err("Codex path must be rejected");
    assert!(is_constraint_error(&error));
}

#[test]
fn codex_binding_cannot_have_executable_path_on_update() {
    let database = TestDatabase::migrated();
    let connection = database.open_raw();
    connection
        .execute(
            "INSERT INTO app_profiles (id, name, created_at_ms, updated_at_ms)
             VALUES ('profile-1', 'Default', 100, 100)",
            [],
        )
        .expect("insert profile");
    connection
        .execute(
            "INSERT INTO provider_bindings (
                id, app_profile_id, provider_kind, display_name,
                executable_path, created_at_ms, updated_at_ms
             ) VALUES ('binding-1', 'profile-1', 'Codex', 'Codex CLI',
                NULL, 100, 100)",
            [],
        )
        .expect("insert Codex binding without path");
    let error = connection
        .execute(
            "UPDATE provider_bindings SET executable_path = 'C:/codex.exe' WHERE id = 'binding-1'",
            [],
        )
        .expect_err("Codex path update must be rejected");
    assert!(is_constraint_error(&error));
}

#[test]
fn codex_binding_with_null_path_is_accepted() {
    let database = TestDatabase::migrated();
    let connection = database.open_raw();
    connection
        .execute(
            "INSERT INTO app_profiles (id, name, created_at_ms, updated_at_ms)
             VALUES ('profile-1', 'Default', 100, 100)",
            [],
        )
        .expect("insert profile");
    connection
        .execute(
            "INSERT INTO provider_bindings (
                id, app_profile_id, provider_kind, display_name,
                executable_path, created_at_ms, updated_at_ms
             ) VALUES ('binding-1', 'profile-1', 'Codex', 'Codex CLI',
                NULL, 100, 100)",
            [],
        )
        .expect("Codex binding without path must be accepted");
}

#[test]
fn profile_kind_uniqueness_is_enforced() {
    let database = TestDatabase::migrated();
    let connection = database.open_raw();
    connection
        .execute(
            "INSERT INTO app_profiles (id, name, created_at_ms, updated_at_ms)
             VALUES ('profile-1', 'Default', 100, 100)",
            [],
        )
        .expect("insert profile");
    connection
        .execute(
            "INSERT INTO provider_bindings (
                id, app_profile_id, provider_kind, display_name,
                created_at_ms, updated_at_ms
             ) VALUES ('binding-1', 'profile-1', 'Claude', 'Claude Code', 100, 100)",
            [],
        )
        .expect("insert first Claude binding");
    let error = connection
        .execute(
            "INSERT INTO provider_bindings (
                id, app_profile_id, provider_kind, display_name,
                created_at_ms, updated_at_ms
             ) VALUES ('binding-2', 'profile-1', 'Claude', 'Another Claude', 100, 100)",
            [],
        )
        .expect_err("duplicate profile+kind must be rejected");
    assert!(is_constraint_error(&error));
}

#[test]
fn app_profile_name_uniqueness_is_enforced() {
    let database = TestDatabase::migrated();
    let connection = database.open_raw();
    connection
        .execute(
            "INSERT INTO app_profiles (id, name, created_at_ms, updated_at_ms)
             VALUES ('profile-1', 'Default', 100, 100)",
            [],
        )
        .expect("insert first profile");
    let error = connection
        .execute(
            "INSERT INTO app_profiles (id, name, created_at_ms, updated_at_ms)
             VALUES ('profile-2', 'Default', 200, 200)",
            [],
        )
        .expect_err("duplicate profile name must be rejected");
    assert!(is_constraint_error(&error));
}

#[test]
fn provider_binding_identity_columns_are_immutable() {
    let database = TestDatabase::migrated();
    let connection = database.open_raw();
    connection
        .execute(
            "INSERT INTO app_profiles (id, name, created_at_ms, updated_at_ms)
             VALUES ('profile-1', 'Default', 100, 100)",
            [],
        )
        .expect("insert profile");
    connection
        .execute(
            "INSERT INTO app_profiles (id, name, created_at_ms, updated_at_ms)
             VALUES ('profile-2', 'Other', 100, 100)",
            [],
        )
        .expect("insert second profile");
    connection
        .execute(
            "INSERT INTO provider_bindings (
                id, app_profile_id, provider_kind, display_name,
                created_at_ms, updated_at_ms
             ) VALUES ('binding-1', 'profile-1', 'Claude', 'Claude Code', 100, 100)",
            [],
        )
        .expect("insert binding");

    for sql in [
        "UPDATE provider_bindings SET id = 'binding-new' WHERE id = 'binding-1'",
        "UPDATE provider_bindings SET app_profile_id = 'profile-2' WHERE id = 'binding-1'",
        "UPDATE provider_bindings SET provider_kind = 'Codex' WHERE id = 'binding-1'",
        "UPDATE provider_bindings SET created_at_ms = 999 WHERE id = 'binding-1'",
    ] {
        let error = connection
            .execute(sql, [])
            .expect_err("immutable column update must fail");
        assert!(is_constraint_error(&error), "unexpected error for: {sql}");
    }
}

#[test]
fn provider_binding_mutable_columns_can_be_updated() {
    let database = TestDatabase::migrated();
    let connection = database.open_raw();
    connection
        .execute(
            "INSERT INTO app_profiles (id, name, created_at_ms, updated_at_ms)
             VALUES ('profile-1', 'Default', 100, 100)",
            [],
        )
        .expect("insert profile");
    connection
        .execute(
            "INSERT INTO provider_bindings (
                id, app_profile_id, provider_kind, display_name,
                executable_path, created_at_ms, updated_at_ms
             ) VALUES ('binding-1', 'profile-1', 'Claude', 'Claude Code',
                'C:/old.exe', 100, 100)",
            [],
        )
        .expect("insert binding");
    connection
        .execute(
            "UPDATE provider_bindings
             SET display_name = 'Updated Name', executable_path = 'C:/new.exe', updated_at_ms = 200
             WHERE id = 'binding-1'",
            [],
        )
        .expect("mutable column update must succeed");
    let (name, path, updated): (String, String, i64) = connection
        .query_row(
            "SELECT display_name, executable_path, updated_at_ms
             FROM provider_bindings WHERE id = 'binding-1'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .expect("read updated binding");
    assert_eq!(name, "Updated Name");
    assert_eq!(path, "C:/new.exe");
    assert_eq!(updated, 200);
}

#[test]
fn executable_path_can_be_set_to_null_via_update() {
    let database = TestDatabase::migrated();
    let connection = database.open_raw();
    connection
        .execute(
            "INSERT INTO app_profiles (id, name, created_at_ms, updated_at_ms)
             VALUES ('profile-1', 'Default', 100, 100)",
            [],
        )
        .expect("insert profile");
    connection
        .execute(
            "INSERT INTO provider_bindings (
                id, app_profile_id, provider_kind, display_name,
                executable_path, created_at_ms, updated_at_ms
             ) VALUES ('binding-1', 'profile-1', 'Claude', 'Claude Code',
                'C:/claude.exe', 100, 100)",
            [],
        )
        .expect("insert binding");
    connection
        .execute(
            "UPDATE provider_bindings SET executable_path = NULL WHERE id = 'binding-1'",
            [],
        )
        .expect("clear executable_path");
    let path: Option<String> = connection
        .query_row(
            "SELECT executable_path FROM provider_bindings WHERE id = 'binding-1'",
            [],
            |row| row.get(0),
        )
        .expect("read cleared path");
    assert!(path.is_none());
}
