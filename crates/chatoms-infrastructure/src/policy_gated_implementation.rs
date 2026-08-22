use std::path::Path;

use chatoms_domain::TaskId;
use chatoms_ports::{
    context_package_implementation::{
        ContextPackageImplementationExecutor, PolicyGatedContextPackageImplementationExecutor,
    },
    error::PortFailure,
    filesystem::FilesystemIdentityPort,
    implementation::{
        ClaudeImplementationExecutor, ImplementationExecutionBrief,
        ImplementationExecutionStartOutcome, PolicyGatedClaudeImplementationExecutor,
        binding_matches_started_task,
    },
    process::CancellationSignal,
    provider_implementation_policy::ProviderImplementationPolicyBinding,
};

pub struct PolicyGatedImplementationExecutor<E, F, B> {
    inner: E,
    filesystem: F,
    binding: B,
}

impl<E, F, B> PolicyGatedImplementationExecutor<E, F, B> {
    pub const fn new(inner: E, filesystem: F, binding: B) -> Self {
        Self {
            inner,
            filesystem,
            binding,
        }
    }
}

impl<E, F, B> PolicyGatedImplementationExecutor<E, F, B>
where
    F: FilesystemIdentityPort,
    B: ProviderImplementationPolicyBinding,
{
    fn authorizes(&mut self, task_id: TaskId, started_task_version: u64, worktree: &Path) -> bool {
        if !binding_matches_started_task(&self.binding, task_id, started_task_version) {
            return false;
        }
        let Ok(live_identity) = self.filesystem.inspect_supported_directory(worktree) else {
            return false;
        };
        self.binding.matches_worktree_object_identity(
            &live_identity.volume_serial_hex,
            &live_identity.file_id_hex,
        )
    }
}

impl<E, F, B> PolicyGatedClaudeImplementationExecutor for PolicyGatedImplementationExecutor<E, F, B>
where
    E: ClaudeImplementationExecutor,
    F: FilesystemIdentityPort,
    B: ProviderImplementationPolicyBinding,
{
    fn start_implementation(
        &mut self,
        task_id: TaskId,
        started_task_version: u64,
        worktree: &Path,
        brief: ImplementationExecutionBrief<'_>,
        cancellation: &dyn CancellationSignal,
    ) -> Result<ImplementationExecutionStartOutcome, PortFailure> {
        if !self.authorizes(task_id, started_task_version, worktree) {
            return Ok(ImplementationExecutionStartOutcome::PreflightRejected);
        }
        self.inner
            .start_implementation(worktree, brief, cancellation)
    }
}

impl<E, F, B> PolicyGatedContextPackageImplementationExecutor
    for PolicyGatedImplementationExecutor<E, F, B>
where
    E: ContextPackageImplementationExecutor,
    F: FilesystemIdentityPort,
    B: ProviderImplementationPolicyBinding,
{
    fn start_implementation(
        &mut self,
        task_id: TaskId,
        started_task_version: u64,
        worktree: &Path,
        brief: ImplementationExecutionBrief<'_>,
        cancellation: &dyn CancellationSignal,
    ) -> Result<ImplementationExecutionStartOutcome, PortFailure> {
        if !self.authorizes(task_id, started_task_version, worktree) {
            return Ok(ImplementationExecutionStartOutcome::PreflightRejected);
        }
        self.inner
            .start_implementation(worktree, brief, cancellation)
    }
}

#[cfg(test)]
mod tests {
    use std::{
        path::{Path, PathBuf},
        sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        },
    };

    use chatoms_domain::{OperationRiskKind, TargetIdentityDigest, TaskId};
    use chatoms_ports::{
        context_package_implementation::{
            ContextPackageImplementationExecutor, PolicyGatedContextPackageImplementationExecutor,
        },
        error::{FailureCategory, PortFailure},
        filesystem::{DirectoryIdentity, DirectoryIdentityGuard, FilesystemIdentityPort},
        implementation::{
            ClaudeImplementationExecutor, ImplementationExecutionBrief,
            ImplementationExecutionStartOutcome, PolicyGatedClaudeImplementationExecutor,
        },
        process::{AtomicCancellationSignal, CancellationSignal},
        provider_implementation_policy::ProviderImplementationPolicyBinding,
    };

    use super::PolicyGatedImplementationExecutor;

    struct Binding {
        task_id: TaskId,
        version: u64,
        volume: &'static str,
        file: &'static str,
    }

    impl ProviderImplementationPolicyBinding for Binding {
        fn task_id(&self) -> TaskId {
            self.task_id
        }

        fn approved_task_version(&self) -> u64 {
            self.version
        }

        fn operation_kind(&self) -> OperationRiskKind {
            OperationRiskKind::ProviderImplementation
        }

        fn target_identity_digest(&self) -> TargetIdentityDigest {
            TargetIdentityDigest::from_digest_bytes([7; 32])
        }

        fn matches_worktree_object_identity(&self, volume: &str, file: &str) -> bool {
            self.volume == volume && self.file == file
        }
    }

    struct Filesystem(Result<DirectoryIdentity, PortFailure>);

    impl FilesystemIdentityPort for Filesystem {
        fn inspect_supported_directory(
            &mut self,
            _path: &Path,
        ) -> Result<DirectoryIdentity, PortFailure> {
            self.0.clone()
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

    struct Inner(Arc<AtomicUsize>);

    impl ClaudeImplementationExecutor for Inner {
        fn start_implementation(
            &mut self,
            _worktree: &Path,
            _brief: ImplementationExecutionBrief<'_>,
            _cancellation: &dyn CancellationSignal,
        ) -> Result<ImplementationExecutionStartOutcome, PortFailure> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok(ImplementationExecutionStartOutcome::PreflightRejected)
        }
    }

    impl ContextPackageImplementationExecutor for Inner {
        fn start_implementation(
            &mut self,
            _worktree: &Path,
            _brief: ImplementationExecutionBrief<'_>,
            _cancellation: &dyn CancellationSignal,
        ) -> Result<ImplementationExecutionStartOutcome, PortFailure> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok(ImplementationExecutionStartOutcome::PreflightRejected)
        }
    }

    fn identity(volume: &str, file: &str) -> DirectoryIdentity {
        DirectoryIdentity {
            canonical_path: PathBuf::from("C:/managed/task"),
            volume_serial_hex: volume.to_owned(),
            file_id_hex: file.to_owned(),
        }
    }

    fn brief() -> ImplementationExecutionBrief<'static> {
        ImplementationExecutionBrief {
            requirements: "requirements",
            completion_criteria: "criteria",
            prohibited_scope: "scope",
            plan_text: "plan",
        }
    }

    #[test]
    fn matching_identity_delegates_for_both_execution_modes() {
        let task_id = TaskId::new();
        for context_package in [false, true] {
            let calls = Arc::new(AtomicUsize::new(0));
            let mut executor = PolicyGatedImplementationExecutor::new(
                Inner(calls.clone()),
                Filesystem(Ok(identity("volume", "file"))),
                Binding {
                    task_id,
                    version: 3,
                    volume: "volume",
                    file: "file",
                },
            );
            let cancellation = AtomicCancellationSignal::new();
            let result = if context_package {
                PolicyGatedContextPackageImplementationExecutor::start_implementation(
                    &mut executor,
                    task_id,
                    4,
                    Path::new("C:/managed/task"),
                    brief(),
                    &cancellation,
                )
            } else {
                PolicyGatedClaudeImplementationExecutor::start_implementation(
                    &mut executor,
                    task_id,
                    4,
                    Path::new("C:/managed/task"),
                    brief(),
                    &cancellation,
                )
            };
            assert!(matches!(
                result,
                Ok(ImplementationExecutionStartOutcome::PreflightRejected)
            ));
            assert_eq!(calls.load(Ordering::SeqCst), 1);
        }
    }

    #[test]
    fn mismatched_or_uninspectable_identity_never_delegates() {
        let task_id = TaskId::new();
        for filesystem in [
            Filesystem(Ok(identity("other", "file"))),
            Filesystem(Ok(identity("volume", "other"))),
            Filesystem(Err(PortFailure::new(FailureCategory::PermissionDenied))),
        ] {
            let calls = Arc::new(AtomicUsize::new(0));
            let mut executor = PolicyGatedImplementationExecutor::new(
                Inner(calls.clone()),
                filesystem,
                Binding {
                    task_id,
                    version: 3,
                    volume: "volume",
                    file: "file",
                },
            );
            let result = PolicyGatedClaudeImplementationExecutor::start_implementation(
                &mut executor,
                task_id,
                4,
                Path::new("C:/managed/task"),
                brief(),
                &AtomicCancellationSignal::new(),
            );
            assert!(matches!(
                result,
                Ok(ImplementationExecutionStartOutcome::PreflightRejected)
            ));
            assert_eq!(calls.load(Ordering::SeqCst), 0);
        }
    }
}
