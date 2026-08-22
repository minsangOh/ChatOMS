/// Fixed identity dimension distinguishing what data a provider-transmission
/// consent (`chatoms_ports::repository::ProviderConsent`) actually covers.
/// `task_id`/`provider`/`work_kind`/`approved_task_version` alone identify
/// *which task, provider, work kind, and task version* a consent was
/// granted for, but not *what data* was approved for transmission — the
/// same four values could, in principle, be re-approved for a materially
/// different transmitted payload shape. This type closes that gap as a
/// fifth identity component. It is owned entirely by application code: a
/// user string or AI output never determines its value.
///
/// `LegacyPhase4` is the fixed scope every Phase 4 Claude Planning,
/// Implementation, and Review consent uses today — the payload shape each
/// provider's own execution contract already defines (TaskBrief's three
/// fields, plus the prior plan text for Implementation or the current diff
/// for Review). `ContextPackageV1` is reserved storage vocabulary for a
/// future Context Package manifest (a later Unit); nothing in this Unit
/// constructs, stores, or reuses a consent with it.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ContextDataScope {
    LegacyPhase4,
    ContextPackageV1,
}

impl ContextDataScope {
    pub const ALL: [Self; 2] = [Self::LegacyPhase4, Self::ContextPackageV1];
}
