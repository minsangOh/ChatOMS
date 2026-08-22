use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

use chatoms_domain::{Task, TaskState};
use chatoms_ports::{
    error::{FailureCategory, PortFailure},
    filesystem::{DirectoryIdentity, DirectoryIdentityGuard, FilesystemIdentityPort},
    git::RepositoryKind,
    repository::{
        GitIsolationStatus, ProjectFilesystemIdentityRecord, ProjectRecord, TaskGitIsolation,
    },
};

use crate::support::{FakeRepository, restored_task};

#[derive(Default)]
pub(crate) struct FakeFilesystem {
    pub(crate) identities: HashMap<PathBuf, DirectoryIdentity>,
    pub(crate) failures: HashMap<PathBuf, FailureCategory>,
}

impl FilesystemIdentityPort for FakeFilesystem {
    fn inspect_supported_directory(
        &mut self,
        path: &Path,
    ) -> Result<DirectoryIdentity, PortFailure> {
        if let Some(category) = self.failures.get(path) {
            return Err(PortFailure::new(*category));
        }
        self.identities
            .get(path)
            .cloned()
            .ok_or_else(|| PortFailure::new(FailureCategory::NotFound))
    }

    fn verify_local_tree(&mut self, _root: &Path) -> Result<(), PortFailure> {
        Err(PortFailure::new(FailureCategory::Unsupported))
    }

    fn acquire_guard(
        &mut self,
        _path: &Path,
        _expected: &DirectoryIdentity,
    ) -> Result<Box<dyn DirectoryIdentityGuard>, PortFailure> {
        Err(PortFailure::new(FailureCategory::Unsupported))
    }
}

pub(crate) fn fixture() -> (FakeRepository, FakeFilesystem, Task) {
    let (task, history) = restored_task(TaskState::AwaitingDesignApproval, 5, 50, None);
    let project_root = PathBuf::from("C:/project");
    let worktree = PathBuf::from("C:/managed/worktree");
    let mut repository = FakeRepository::default();
    repository.seed_task(task.clone(), history);
    repository.project_records.insert(
        task.project_id(),
        ProjectRecord {
            id: task.project_id(),
            name: "Project".to_owned(),
            root_path: project_root.to_string_lossy().into_owned(),
            canonical_path_key: "c:/project".to_owned(),
            display_path: "Project".to_owned(),
            created_at_ms: 10,
            updated_at_ms: 11,
        },
    );
    repository.project_identities.insert(
        task.project_id(),
        ProjectFilesystemIdentityRecord {
            project_id: task.project_id(),
            root_volume_serial_hex: "0000000000000001".to_owned(),
            root_file_id_hex: "11111111111111111111111111111111".to_owned(),
            repository_kind: RepositoryKind::Git,
            git_common_volume_serial_hex: None,
            git_common_file_id_hex: None,
            confirmed: true,
            revision: 3,
            verified_at_ms: 20,
        },
    );
    repository.isolations.insert(
        task.id(),
        TaskGitIsolation {
            task_id: task.id(),
            project_id: task.project_id(),
            status: GitIsolationStatus::WorktreeReady,
            operation_id: None,
            expected_task_version: 2,
            base_branch: Some("main".to_owned()),
            base_commit: Some("a".repeat(40)),
            worktree_path: Some(worktree.to_string_lossy().into_owned()),
            branch_created_by_app: true,
            worktree_created_by_app: true,
            created_at_ms: 20,
            updated_at_ms: 30,
        },
    );
    let filesystem = FakeFilesystem {
        identities: HashMap::from([
            (
                project_root.clone(),
                DirectoryIdentity {
                    canonical_path: project_root,
                    volume_serial_hex: "0000000000000001".to_owned(),
                    file_id_hex: "11111111111111111111111111111111".to_owned(),
                },
            ),
            (
                worktree.clone(),
                DirectoryIdentity {
                    canonical_path: worktree,
                    volume_serial_hex: "0000000000000002".to_owned(),
                    file_id_hex: "22222222222222222222222222222222".to_owned(),
                },
            ),
        ]),
        failures: HashMap::new(),
    };
    (repository, filesystem, task)
}
