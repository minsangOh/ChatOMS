mod support;

use chatoms_infrastructure::database::{DatabaseConnection, SqliteFoundationRepository};
use chatoms_ports::{
    provider::ProviderKind,
    repository::{
        AppProfileRecord, FoundationRepository, ProviderBindingRecord, RepositoryErrorCode,
    },
};

use support::TestDatabase;

struct Fixture {
    database: TestDatabase,
}

impl Fixture {
    fn new() -> Self {
        let database = TestDatabase::migrated();
        Self { database }
    }

    fn open(&self) -> DatabaseConnection {
        DatabaseConnection::open(self.database.path()).expect("open repository connection")
    }
}

fn default_profile(id: &str) -> AppProfileRecord {
    AppProfileRecord {
        id: id.to_owned(),
        name: "Default".to_owned(),
        created_at_ms: 100,
        updated_at_ms: 100,
    }
}

fn claude_binding(id: &str, profile_id: &str) -> ProviderBindingRecord {
    ProviderBindingRecord {
        id: id.to_owned(),
        app_profile_id: profile_id.to_owned(),
        provider_kind: ProviderKind::Claude,
        display_name: "Claude Code".to_owned(),
        executable_path: None,
        created_at_ms: 100,
        updated_at_ms: 100,
    }
}

#[test]
fn ensure_default_profile_and_claude_binding_creates_records() {
    let fixture = Fixture::new();
    let mut connection = fixture.open();
    let mut repository = SqliteFoundationRepository::new(&mut connection);
    let profile = default_profile("profile-1");
    let binding = claude_binding("binding-1", "profile-1");
    let result = repository
        .ensure_default_profile_and_claude_binding(&profile, &binding)
        .expect("ensure binding");
    assert_eq!(result.display_name, "Claude Code");
    assert_eq!(result.provider_kind, ProviderKind::Claude);
    assert!(result.executable_path.is_none());
}

#[test]
fn ensure_default_profile_and_claude_binding_is_idempotent() {
    let fixture = Fixture::new();
    let mut connection = fixture.open();
    let mut repository = SqliteFoundationRepository::new(&mut connection);
    let profile = default_profile("profile-1");
    let binding = claude_binding("binding-1", "profile-1");
    let first = repository
        .ensure_default_profile_and_claude_binding(&profile, &binding)
        .expect("first ensure");

    let profile_2 = default_profile("profile-2");
    let binding_2 = claude_binding("binding-2", "profile-2");
    let second = repository
        .ensure_default_profile_and_claude_binding(&profile_2, &binding_2)
        .expect("second ensure");

    assert_eq!(first.id, second.id);
    assert_eq!(first.app_profile_id, second.app_profile_id);
}

#[test]
fn get_claude_binding_returns_none_when_no_profile_exists() {
    let fixture = Fixture::new();
    let mut connection = fixture.open();
    let mut repository = SqliteFoundationRepository::new(&mut connection);
    let result = repository
        .get_claude_binding("Default")
        .expect("get binding");
    assert!(result.is_none());
}

#[test]
fn get_claude_binding_returns_existing_binding() {
    let fixture = Fixture::new();
    let mut connection = fixture.open();
    let mut repository = SqliteFoundationRepository::new(&mut connection);
    let profile = default_profile("profile-1");
    let binding = claude_binding("binding-1", "profile-1");
    repository
        .ensure_default_profile_and_claude_binding(&profile, &binding)
        .expect("ensure binding");
    let result = repository
        .get_claude_binding("Default")
        .expect("get binding");
    assert!(result.is_some());
    let result = result.unwrap();
    assert_eq!(result.display_name, "Claude Code");
    assert_eq!(result.provider_kind, ProviderKind::Claude);
}

#[test]
fn update_claude_executable_path_round_trips() {
    let fixture = Fixture::new();
    let mut connection = fixture.open();
    let mut repository = SqliteFoundationRepository::new(&mut connection);
    let profile = default_profile("profile-1");
    let binding = claude_binding("binding-1", "profile-1");
    let created = repository
        .ensure_default_profile_and_claude_binding(&profile, &binding)
        .expect("ensure binding");

    repository
        .update_claude_executable_path(&created.id, Some("C:/claude.exe"), 200)
        .expect("set path");

    let loaded = repository
        .get_claude_binding("Default")
        .expect("get binding")
        .expect("binding must exist");
    assert_eq!(loaded.executable_path.as_deref(), Some("C:/claude.exe"));
    assert_eq!(loaded.updated_at_ms, 200);
}

#[test]
fn clear_claude_executable_path_sets_null() {
    let fixture = Fixture::new();
    let mut connection = fixture.open();
    let mut repository = SqliteFoundationRepository::new(&mut connection);
    let profile = default_profile("profile-1");
    let binding = claude_binding("binding-1", "profile-1");
    let created = repository
        .ensure_default_profile_and_claude_binding(&profile, &binding)
        .expect("ensure binding");
    repository
        .update_claude_executable_path(&created.id, Some("C:/claude.exe"), 200)
        .expect("set path");
    repository
        .update_claude_executable_path(&created.id, None, 300)
        .expect("clear path");

    let loaded = repository
        .get_claude_binding("Default")
        .expect("get binding")
        .expect("binding must exist");
    assert!(loaded.executable_path.is_none());
    assert_eq!(loaded.updated_at_ms, 300);
}

#[test]
fn update_claude_executable_path_rejects_nonexistent_binding() {
    let fixture = Fixture::new();
    let mut connection = fixture.open();
    let mut repository = SqliteFoundationRepository::new(&mut connection);
    let error = repository
        .update_claude_executable_path("nonexistent", Some("C:/claude.exe"), 100)
        .expect_err("nonexistent binding must fail");
    assert_eq!(error.code(), RepositoryErrorCode::BindingNotFound);
}

#[test]
fn update_claude_executable_path_rejects_codex_binding() {
    let fixture = Fixture::new();
    let mut connection = fixture.open();
    let raw = fixture.database.open_raw();
    raw.execute(
        "INSERT INTO app_profiles (id, name, created_at_ms, updated_at_ms)
         VALUES ('profile-1', 'Default', 100, 100)",
        [],
    )
    .expect("insert profile");
    raw.execute(
        "INSERT INTO provider_bindings (
            id, app_profile_id, provider_kind, display_name,
            created_at_ms, updated_at_ms
         ) VALUES ('codex-binding', 'profile-1', 'Codex', 'Codex CLI', 100, 100)",
        [],
    )
    .expect("insert Codex binding");
    drop(raw);

    let mut repository = SqliteFoundationRepository::new(&mut connection);
    let error = repository
        .update_claude_executable_path("codex-binding", Some("C:/codex.exe"), 200)
        .expect_err("Codex binding update must fail");
    assert_eq!(error.code(), RepositoryErrorCode::InvalidAggregate);
}

#[test]
fn ensure_rejects_codex_provider_kind() {
    let fixture = Fixture::new();
    let mut connection = fixture.open();
    let mut repository = SqliteFoundationRepository::new(&mut connection);
    let profile = default_profile("profile-1");
    let binding = ProviderBindingRecord {
        id: "binding-1".to_owned(),
        app_profile_id: "profile-1".to_owned(),
        provider_kind: ProviderKind::Codex,
        display_name: "Codex CLI".to_owned(),
        executable_path: None,
        created_at_ms: 100,
        updated_at_ms: 100,
    };
    let error = repository
        .ensure_default_profile_and_claude_binding(&profile, &binding)
        .expect_err("Codex ensure must fail");
    assert_eq!(error.code(), RepositoryErrorCode::InvalidAggregate);
}

#[test]
fn ensure_rejects_mismatched_profile_id() {
    let fixture = Fixture::new();
    let mut connection = fixture.open();
    let mut repository = SqliteFoundationRepository::new(&mut connection);
    let profile = default_profile("profile-1");
    let binding = claude_binding("binding-1", "different-profile");
    let error = repository
        .ensure_default_profile_and_claude_binding(&profile, &binding)
        .expect_err("mismatched profile must fail");
    assert_eq!(error.code(), RepositoryErrorCode::InvalidAggregate);
}
