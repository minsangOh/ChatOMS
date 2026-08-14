use serde::{Deserialize, Serialize};

/// The fixed vocabulary of Testing validation categories from
/// `PRODUCT_REQUIREMENTS.md` section 15 (포맷 검사/린트/타입 검사/단위+통합
/// 테스트/빌드). Provider-neutral and execution-neutral: this type carries no
/// information about which project, executable, or command a given kind
/// resolves to — see `chatoms_ports::validation::ValidationCommandCandidate`
/// and `chatoms_ports::repository::ValidationCommandApprovalRecord` for
/// that.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
pub enum ValidationCommandKind {
    Format,
    Lint,
    Typecheck,
    Test,
    Build,
}

impl ValidationCommandKind {
    pub const ALL: [Self; 5] = [
        Self::Format,
        Self::Lint,
        Self::Typecheck,
        Self::Test,
        Self::Build,
    ];
}
