-- Immutable, content-free approval record for one high-risk effect category
-- (docs/PRODUCT_REQUIREMENTS.md section 9's 13-item taxonomy,
-- chatoms_domain::HighRiskCategory) on a specific task version. This is a
-- separate axis from provider-transmission consent
-- (task_provider_consents, 0006/0016) and from Context Package v1 manifests
-- (context_package_manifests, 0017): consent/manifest answer "may this data
-- be sent to this provider", while this table answers "is this class of
-- effect (schema change, data migration, difficult-to-recover change, etc.)
-- allowed to occur at all" -- a question that is provider- and
-- work-kind-neutral. Accordingly this table carries no provider, work_kind,
-- or data_scope column, and is not foreign-keyed to either of those tables:
-- a task may need a Context Package v1 consent/manifest pair and a
-- high-risk approval for the same change at the same time, entirely
-- independently.
--
-- No risk-category classification, approval UI/IPC, or execution gating is
-- implemented by this Unit -- this table only stores an already-approved
-- reference. It carries no free-text description of what was approved
-- (only the closed risk_category vocabulary), no diff/path/provider-output
-- content, and no auth/session/cost data.
CREATE TABLE task_high_risk_approvals (
    task_id TEXT NOT NULL,
    approved_task_version INTEGER NOT NULL CHECK (approved_task_version >= 0),
    risk_category TEXT NOT NULL CHECK (
        risk_category IN (
            'ArchitectureChange',
            'DatabaseSchemaChange',
            'AuthenticationOrAuthorizationChange',
            'SecurityPolicyChange',
            'ExternalNetworkBehaviorAddition',
            'ExternalDataTransmissionAddition',
            'LargeScaleFileMoveOrDeletion',
            'PublicApiOrStorageFormatChange',
            'OperatingSystemConfigurationChange',
            'AdministratorPrivilegesRequired',
            'BreakingCompatibilityChange',
            'DataMigration',
            'DifficultToRecoverChange'
        )
    ),
    approved_at_ms INTEGER NOT NULL CHECK (approved_at_ms >= 0),
    PRIMARY KEY (task_id, approved_task_version, risk_category),
    FOREIGN KEY (task_id) REFERENCES tasks (id)
);

CREATE TRIGGER task_high_risk_approvals_binding_insert
BEFORE INSERT ON task_high_risk_approvals
WHEN NOT EXISTS (
    SELECT 1 FROM tasks
    WHERE id = NEW.task_id AND version = NEW.approved_task_version
)
BEGIN
    SELECT RAISE(ABORT, 'high-risk approval task version binding mismatch');
END;

CREATE TRIGGER task_high_risk_approvals_immutable_update
BEFORE UPDATE ON task_high_risk_approvals
FOR EACH ROW
BEGIN
    SELECT RAISE(ABORT, 'task_high_risk_approvals is immutable');
END;

CREATE TRIGGER task_high_risk_approvals_immutable_delete
BEFORE DELETE ON task_high_risk_approvals
FOR EACH ROW
BEGIN
    SELECT RAISE(ABORT, 'task_high_risk_approvals is immutable');
END;

CREATE INDEX task_high_risk_approvals_task_id_idx ON task_high_risk_approvals (task_id);
