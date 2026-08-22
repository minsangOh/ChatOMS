CREATE TABLE task_operation_risk_declarations (
    task_id TEXT NOT NULL,
    approved_task_version INTEGER NOT NULL CHECK (approved_task_version >= 0),
    operation_kind TEXT NOT NULL CHECK (operation_kind = 'ProviderImplementation'),
    target_identity_digest_hex TEXT NOT NULL CHECK (
        length(target_identity_digest_hex) = 64
        AND target_identity_digest_hex NOT GLOB '*[^0-9a-f]*'
    ),
    declared_at_ms INTEGER NOT NULL CHECK (declared_at_ms >= 0),
    PRIMARY KEY (task_id, approved_task_version, operation_kind),
    FOREIGN KEY (task_id) REFERENCES tasks(id) ON DELETE RESTRICT
) STRICT;

CREATE TABLE task_operation_risk_categories (
    task_id TEXT NOT NULL,
    approved_task_version INTEGER NOT NULL CHECK (approved_task_version >= 0),
    operation_kind TEXT NOT NULL CHECK (operation_kind = 'ProviderImplementation'),
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
    PRIMARY KEY (task_id, approved_task_version, operation_kind, risk_category),
    FOREIGN KEY (task_id, approved_task_version, operation_kind)
        REFERENCES task_operation_risk_declarations (
            task_id, approved_task_version, operation_kind
        ) ON DELETE RESTRICT,
    FOREIGN KEY (task_id, approved_task_version, risk_category)
        REFERENCES task_high_risk_approvals (
            task_id, approved_task_version, risk_category
        ) ON DELETE RESTRICT
) STRICT;

CREATE TRIGGER task_operation_risk_declarations_binding_insert
BEFORE INSERT ON task_operation_risk_declarations
FOR EACH ROW
WHEN NOT EXISTS (
    SELECT 1 FROM tasks
    WHERE id = NEW.task_id AND version = NEW.approved_task_version
)
BEGIN
    SELECT RAISE(ABORT, 'operation risk declaration task version binding mismatch');
END;

CREATE TRIGGER task_operation_risk_declarations_immutable_update
BEFORE UPDATE ON task_operation_risk_declarations
BEGIN
    SELECT RAISE(ABORT, 'task_operation_risk_declarations is immutable');
END;

CREATE TRIGGER task_operation_risk_declarations_immutable_delete
BEFORE DELETE ON task_operation_risk_declarations
BEGIN
    SELECT RAISE(ABORT, 'task_operation_risk_declarations is immutable');
END;

CREATE TRIGGER task_operation_risk_categories_immutable_update
BEFORE UPDATE ON task_operation_risk_categories
BEGIN
    SELECT RAISE(ABORT, 'task_operation_risk_categories is immutable');
END;

CREATE TRIGGER task_operation_risk_categories_immutable_delete
BEFORE DELETE ON task_operation_risk_categories
BEGIN
    SELECT RAISE(ABORT, 'task_operation_risk_categories is immutable');
END;

CREATE INDEX task_operation_risk_declarations_task_id_idx
    ON task_operation_risk_declarations (task_id);
