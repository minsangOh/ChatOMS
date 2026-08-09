use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use chatoms_infrastructure::git::GitCliAdapter;
use chatoms_ports::{
    error::{CategorizedFailure, FailureCategory},
    git::{GitService, RepositoryKind, WorktreeCreationOutcome},
};

fn adapter() -> (tempfile::TempDir, GitCliAdapter) {
    let control = tempfile::tempdir().expect("Git control root");
    let adapter = GitCliAdapter::new(control.path().to_path_buf()).expect("controlled Git adapter");
    (control, adapter)
}

fn git(root: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .expect("run fixture git");
    assert!(
        output.status.success(),
        "git fixture command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout)
        .expect("git output")
        .trim()
        .to_owned()
}

fn committed_repository() -> tempfile::TempDir {
    let directory = tempfile::tempdir().expect("temporary repository");
    git(directory.path(), &["init"]);
    git(directory.path(), &["config", "user.name", "ChatOMS Test"]);
    git(
        directory.path(),
        &["config", "user.email", "chatoms@example.invalid"],
    );
    fs::write(directory.path().join("tracked.txt"), "baseline\n").expect("write fixture");
    git(directory.path(), &["add", "tracked.txt"]);
    git(directory.path(), &["commit", "-m", "test: baseline"]);
    directory
}

#[test]
fn nested_registration_detects_root_and_dirty_state_without_mutation() {
    let repository = committed_repository();
    let nested = repository.path().join("src").join("nested");
    fs::create_dir_all(&nested).expect("nested fixture");
    let (_control, mut adapter) = adapter();
    let inspection = adapter
        .inspect_project(&nested)
        .expect("inspect nested project");
    assert_eq!(inspection.repository_kind, RepositoryKind::Git);
    assert_eq!(
        inspection.canonical_root,
        fs::canonicalize(repository.path()).expect("canonical root")
    );
    assert!(
        inspection
            .repository_status
            .as_ref()
            .is_some_and(|status| status.clean)
    );
    #[cfg(windows)]
    {
        let case_alias = PathBuf::from(nested.to_string_lossy().to_uppercase());
        let alias = adapter
            .inspect_project(&case_alias)
            .expect("inspect case alias");
        assert_eq!(alias.canonical_key, inspection.canonical_key);
        assert_eq!(alias.confirmation_token, inspection.confirmation_token);
        let network = adapter
            .inspect_project(Path::new(r"\\server\share\project"))
            .expect_err("UNC paths are unsupported");
        assert_eq!(network.category(), FailureCategory::InvalidInput);
    }

    fs::write(repository.path().join("untracked.txt"), "dirty\n").expect("write untracked");
    let status = adapter
        .repository_status(repository.path())
        .expect("dirty status");
    assert!(!status.clean);
    assert!(status.current_branch.is_some());
    assert!(status.head_commit.is_some());

    let plain = tempfile::tempdir().expect("plain directory");
    let plain_inspection = adapter
        .inspect_project(plain.path())
        .expect("inspect non-Git directory");
    assert_eq!(plain_inspection.repository_kind, RepositoryKind::NonGit);
    assert!(!plain.path().join(".git").exists());
}

#[test]
fn initialization_uses_existing_ignore_and_creates_verified_snapshot() {
    let project = tempfile::tempdir().expect("plain project");
    fs::write(project.path().join(".gitignore"), "ignored.txt\n").expect("ignore fixture");
    fs::write(project.path().join("ignored.txt"), "ignored\n").expect("ignored fixture");
    fs::write(project.path().join("included.txt"), "included\n").expect("included fixture");
    let (_control, mut adapter) = adapter();
    adapter
        .initialize_repository(project.path())
        .expect("initialize repository");
    git(project.path(), &["config", "user.name", "ChatOMS Test"]);
    git(
        project.path(),
        &["config", "user.email", "chatoms@example.invalid"],
    );
    assert!(
        adapter
            .has_commit_author(project.path())
            .expect("author status")
    );
    let commit = adapter
        .create_initial_snapshot(project.path())
        .expect("snapshot commit");
    assert_eq!(
        adapter
            .repository_status(project.path())
            .expect("status")
            .head_commit
            .as_deref(),
        Some(commit.as_str())
    );
    assert!(
        !git(project.path(), &["ls-files"])
            .lines()
            .any(|path| path == "ignored.txt")
    );
}

#[test]
fn managed_worktree_preserves_base_checkout_and_never_deletes_on_collision() {
    let repository = committed_repository();
    let base_branch = git(repository.path(), &["branch", "--show-current"]);
    let base_commit = git(repository.path(), &["rev-parse", "HEAD"]);
    let worktree_parent = tempfile::tempdir().expect("worktree parent");
    let worktree = worktree_parent.path().join("managed");
    let branch = "ai-task/01900000-0000-7000-8000-000000000001";
    let (_control, mut adapter) = adapter();
    let safety = adapter
        .validate_repository_source(repository.path(), &base_commit)
        .expect("safe source");
    assert_eq!(
        adapter
            .create_task_worktree(repository.path(), branch, &base_commit, &worktree, &safety)
            .expect("create worktree"),
        WorktreeCreationOutcome::Created
    );
    assert!(
        adapter
            .verify_task_worktree(repository.path(), branch, &base_commit, &worktree)
            .expect("verify worktree")
    );
    assert_eq!(
        git(repository.path(), &["branch", "--show-current"]),
        base_branch
    );
    assert_eq!(git(repository.path(), &["rev-parse", "HEAD"]), base_commit);
    assert!(worktree.exists());
    let dirty_partial = worktree.join("user-untracked.txt");
    fs::write(&dirty_partial, "must survive\n").expect("dirty partial worktree");
    let retry = adapter
        .create_task_worktree(repository.path(), branch, &base_commit, &worktree, &safety)
        .expect_err("existing dirty worktree must fail without cleanup");
    assert_eq!(retry.category(), FailureCategory::AlreadyExists);
    assert_eq!(
        fs::read_to_string(&dirty_partial).expect("dirty file remains"),
        "must survive\n"
    );
    assert_eq!(
        git(
            repository.path(),
            &["rev-parse", &format!("refs/heads/{branch}")]
        ),
        base_commit
    );
    let second_branch = "ai-task/01900000-0000-7000-8000-000000000002";
    git(repository.path(), &["branch", second_branch, &base_commit]);
    let error = adapter
        .create_task_worktree(
            repository.path(),
            second_branch,
            &base_commit,
            &worktree_parent.path().join("collision"),
            &safety,
        )
        .expect_err("pre-existing branch must block");
    assert_eq!(error.category(), FailureCategory::AlreadyExists);
    assert_eq!(
        git(
            repository.path(),
            &["rev-parse", &format!("refs/heads/{second_branch}")]
        ),
        base_commit
    );
}

#[test]
fn non_git_filter_is_rejected_before_git_init_or_filter_execution() {
    let project = tempfile::tempdir().expect("plain project");
    let marker = project.path().join("filter-marker.txt");
    fs::write(project.path().join("payload.txt"), "payload\n").expect("payload");
    fs::write(
        project.path().join(".gitattributes"),
        "*.txt filter=external\n",
    )
    .expect("attributes");
    let (_control, mut adapter) = adapter();
    let error = adapter
        .initialize_repository(project.path())
        .expect_err("active filter must block before init");
    assert_eq!(error.category(), FailureCategory::Unsupported);
    assert!(!project.path().join(".git").exists());
    assert!(!marker.exists());
}

#[test]
fn effective_info_attributes_and_attribute_race_fail_without_deletion() {
    let repository = committed_repository();
    let base_commit = git(repository.path(), &["rev-parse", "HEAD"]);
    let worktree_parent = tempfile::tempdir().expect("worktree parent");
    let worktree = worktree_parent.path().join("managed");
    let branch = "ai-task/01900000-0000-7000-8000-000000000010";
    let (_control, mut adapter) = adapter();
    let safety = adapter
        .validate_repository_source(repository.path(), &base_commit)
        .expect("initial safety");
    let info = repository
        .path()
        .join(".git")
        .join("info")
        .join("attributes");
    fs::create_dir_all(info.parent().expect("info parent")).expect("info directory");
    fs::write(&info, "# identity changed\n").expect("change info attributes");
    assert_eq!(
        adapter
            .create_task_worktree(repository.path(), branch, &base_commit, &worktree, &safety)
            .expect("race is classified"),
        WorktreeCreationOutcome::Uncertain
    );
    assert!(!worktree.exists());
    assert!(git(repository.path(), &["branch", "--list", branch]).is_empty());

    fs::write(&info, "*.txt filter=external\n").expect("active info attributes");
    let error = adapter
        .validate_repository_source(repository.path(), &base_commit)
        .expect_err("info attributes override must be effective");
    assert_eq!(error.category(), FailureCategory::Unsupported);
}

#[test]
fn local_hooks_fsmonitor_and_filter_commands_cannot_create_markers() {
    let repository = committed_repository();
    let hooks = repository.path().join(".git").join("malicious-hooks");
    fs::create_dir(&hooks).expect("hooks directory");
    let checkout_marker = repository.path().join("post-checkout-marker.txt");
    let fsmonitor_marker = repository.path().join("fsmonitor-marker.txt");
    let filter_marker = repository.path().join("filter-marker.txt");
    let external_attributes = repository
        .path()
        .join(".git")
        .join("malicious-global.attributes");
    let shell_path = |path: &Path| path.to_string_lossy().replace('\\', "/");
    fs::write(
        hooks.join("post-checkout"),
        format!(
            "#!/bin/sh\nprintf marker > \"{}\"\n",
            shell_path(&checkout_marker)
        ),
    )
    .expect("checkout hook");
    let fsmonitor = repository.path().join(".git").join("malicious-fsmonitor");
    fs::write(
        &fsmonitor,
        format!(
            "#!/bin/sh\nprintf marker > \"{}\"\nprintf '\\n'\n",
            shell_path(&fsmonitor_marker)
        ),
    )
    .expect("fsmonitor hook");
    fs::write(&external_attributes, "*.txt filter=external\n").expect("external attributes");
    git(
        repository.path(),
        &[
            "config",
            "core.hooksPath",
            hooks.to_str().expect("hooks path"),
        ],
    );
    git(
        repository.path(),
        &[
            "config",
            "core.fsmonitor",
            fsmonitor.to_str().expect("monitor path"),
        ],
    );
    git(
        repository.path(),
        &[
            "config",
            "core.attributesFile",
            external_attributes.to_str().expect("attributes path"),
        ],
    );

    let (_control, mut adapter) = adapter();
    let base_commit = git(repository.path(), &["rev-parse", "HEAD"]);
    let worktree_parent = tempfile::tempdir().expect("worktree parent");
    let worktree = worktree_parent.path().join("managed");
    let branch = "ai-task/01900000-0000-7000-8000-000000000011";
    let safety = adapter
        .validate_repository_source(repository.path(), &base_commit)
        .expect("safe source");
    assert_eq!(
        adapter
            .create_task_worktree(repository.path(), branch, &base_commit, &worktree, &safety)
            .expect("controlled worktree"),
        WorktreeCreationOutcome::Created
    );
    adapter
        .repository_status(repository.path())
        .expect("controlled status");
    assert!(!checkout_marker.exists());
    assert!(!fsmonitor_marker.exists());

    fs::write(
        repository.path().join(".gitattributes"),
        "*.txt filter=external\n",
    )
    .expect("filter attributes");
    fs::write(repository.path().join("filter-input.txt"), "input\n").expect("filter input");
    git(repository.path(), &["add", ".gitattributes"]);
    git(
        repository.path(),
        &["commit", "-m", "test: filter attributes"],
    );
    git(
        repository.path(),
        &[
            "config",
            "filter.external.clean",
            &format!("cmd.exe /d /c echo marker>{}", filter_marker.display()),
        ],
    );
    let filtered_commit = git(repository.path(), &["rev-parse", "HEAD"]);
    let error = adapter
        .validate_repository_source(repository.path(), &filtered_commit)
        .expect_err("active base filter must be rejected");
    assert_eq!(error.category(), FailureCategory::Unsupported);
    assert!(!filter_marker.exists());
}

#[test]
fn inherited_git_environment_cannot_redirect_repository_or_config() {
    let repository = committed_repository();
    let attacker = tempfile::tempdir().expect("attacker directory");
    let attacker_config = attacker.path().join("attacker.config");
    let attacker_attributes = attacker.path().join("attacker.attributes");
    fs::write(
        &attacker_config,
        format!(
            "[core]\n\tbare = true\n\tattributesFile = {}\n",
            attacker_attributes.display()
        ),
    )
    .expect("attacker config");
    fs::write(&attacker_attributes, "*.txt filter=external\n").expect("attacker attributes");
    let marker_git = attacker.path().join("git.exe");
    fs::copy(
        std::env::var_os("ComSpec").expect("Windows command interpreter"),
        &marker_git,
    )
    .expect("PATH marker executable");
    let inherited_path = std::env::var_os("PATH").expect("PATH");
    let poisoned_path =
        std::env::join_paths([attacker.path(), Path::new(&inherited_path)]).expect("poisoned PATH");
    let output = Command::new(std::env::current_exe().expect("test executable"))
        .args(["--ignored", "--exact", "environment_attack_child"])
        .env("CHATOMS_TEST_ROOT", repository.path())
        .env("GIT_DIR", attacker.path())
        .env("GIT_WORK_TREE", attacker.path())
        .env("GIT_INDEX_FILE", attacker.path().join("index"))
        .env("GIT_CONFIG_COUNT", "1")
        .env("GIT_CONFIG_KEY_0", "core.bare")
        .env("GIT_CONFIG_VALUE_0", "true")
        .env("GIT_CONFIG_GLOBAL", &attacker_config)
        .env("GIT_CONFIG_SYSTEM", &attacker_config)
        .env("GIT_ATTR_NOSYSTEM", "0")
        .env("HOME", attacker.path())
        .env("PATH", poisoned_path)
        .output()
        .expect("run isolated environment attack test");
    assert!(
        output.status.success(),
        "environment attack child failed: {}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn local_filter_and_include_configuration_are_rejected_before_git_mutation() {
    let repository = committed_repository();
    let base = git(repository.path(), &["rev-parse", "HEAD"]);
    let (_control, mut adapter) = adapter();
    git(
        repository.path(),
        &["config", "filter.external.clean", "cmd.exe /d /c exit 0"],
    );
    assert_eq!(
        adapter
            .validate_repository_source(repository.path(), &base)
            .expect_err("local filter config is unsupported")
            .category(),
        FailureCategory::Unsupported
    );
    git(
        repository.path(),
        &["config", "--unset", "filter.external.clean"],
    );
    git(
        repository.path(),
        &["config", "include.path", "attacker.config"],
    );
    assert_eq!(
        adapter
            .validate_repository_source(repository.path(), &base)
            .expect_err("local include config is unsupported")
            .category(),
        FailureCategory::Unsupported
    );
}

#[test]
fn bare_separate_git_dir_and_linked_worktree_are_rejected() {
    let parent = tempfile::tempdir().expect("fixture parent");
    let bare = parent.path().join("bare.git");
    let output = Command::new("git")
        .args(["init", "--bare"])
        .arg(&bare)
        .output()
        .expect("create bare repository");
    assert!(output.status.success());
    let (_control, mut adapter) = adapter();
    assert_eq!(
        adapter
            .inspect_project(&bare)
            .expect_err("bare repository")
            .category(),
        FailureCategory::Unsupported
    );

    let separate_worktree = parent.path().join("separate-worktree");
    let separate_git = parent.path().join("separate-git");
    fs::create_dir(&separate_worktree).expect("separate worktree");
    let output = Command::new("git")
        .arg("init")
        .arg(format!("--separate-git-dir={}", separate_git.display()))
        .arg(&separate_worktree)
        .output()
        .expect("create separate git dir");
    assert!(output.status.success());
    assert!(adapter.inspect_project(&separate_worktree).is_err());

    let repository = committed_repository();
    let linked = parent.path().join("linked");
    git(
        repository.path(),
        &["worktree", "add", linked.to_str().expect("linked path")],
    );
    assert!(adapter.inspect_project(&linked).is_err());
}

#[test]
#[ignore = "invoked in an isolated child process by inherited_git_environment_cannot_redirect_repository_or_config"]
fn environment_attack_child() {
    let root = PathBuf::from(std::env::var_os("CHATOMS_TEST_ROOT").expect("fixture root"));
    let (_control, mut adapter) = adapter();
    let inspection = adapter
        .inspect_project(&root)
        .expect("controlled inspection");
    assert_eq!(inspection.repository_kind, RepositoryKind::Git);
    assert!(
        inspection
            .repository_status
            .is_some_and(|status| status.clean)
    );
    let base = adapter
        .repository_status(&root)
        .expect("controlled status")
        .head_commit
        .expect("HEAD commit");
    adapter
        .validate_repository_source(&root, &base)
        .expect("global and system attributes are isolated");
}
