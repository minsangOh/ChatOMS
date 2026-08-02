# ChatOMS 설계 결정

확정된 결정과 의도적으로 보류한 결정을 기록한다. 상세 근거와 요구사항은 [PRODUCT_REQUIREMENTS.md](PRODUCT_REQUIREMENTS.md), 구현 순서는 [PHASE_PLAN.md](PHASE_PLAN.md)를 참조한다.

| 결정 | 상태 | 내용 |
|---|---|---|
| 기술 스택 | 확정 | Tauri 2 + React + TypeScript strict + Rust + SQLite를 사용한다. |
| 플랫폼 | 확정 | Windows Native를 우선 지원하고 macOS를 지원한다. WSL은 MVP에서 제외한다. |
| Codex 제어 | 확정 | 설치된 Codex에서 생성한 JSON Schema 또는 TypeScript schema와 capability를 검증한 뒤 `codex app-server`를 주 제어 인터페이스로 사용한다. |
| Codex fallback | 확정 | app-server 호환 실패 시 `codex exec`로 자동 전환하지 않는다. |
| 공급자 역할 | 확정 | Claude 설계 → Codex 구현·테스트 → Claude 리뷰 순서로 실행하며 Claude는 코드를 직접 수정하지 않는다. |
| 모델 간 문맥 | 확정 | 전체 대화 대신 작업별로 versioned Context Package를 생성·마스킹·기록해 전달한다. |
| 공급자 외부 전송 | 확정 | 작업 시작 시 공급자와 데이터 범위를 한 번 승인하고 해당 작업의 후속 호출에만 재사용한다. |
| 격리 수준 | 확정 | ChatOMS application policy와 공식 CLI 권한·sandbox를 조합하며 OS 전체 수준의 완전 격리를 보장하지 않는다. |
| 변경 격리 | 확정 | 향후 모든 AI 코드 변경은 작업별 branch와 worktree에서 수행하고 원본 디렉터리는 기본적으로 읽기 전용으로 취급한다. |
| 병합 이력 | 확정 | 사용자 diff 승인 후 단일 작업 commit을 생성하고 기본 브랜치에 `--no-ff` 병합한다. |
| 작업 격리 cardinality | 확정 | 한 `Task`는 하나의 프로젝트, 정확히 하나의 task branch와 최대 하나의 worktree만 가진다. |
| 동시성과 lease | 확정 | 여러 프로젝트를 등록할 수 있지만 앱 전체의 활성 작업은 하나다. `Paused`, `RecoveryRequired`, `UnknownExternalEffect`를 포함한 모든 실행 비종료 상태가 `ActiveTaskLease`를 유지하고 `Completed`, `Failed`, `Cancelled` 진입 시에만 해제한다. |
| Migration 경계 | 확정 | 검증된 ChatOMS 내부 SQLite forward migration과 사용자 대상 프로젝트의 schema·migration·데이터 변환 정책을 분리하며, 후자는 별도 사용자 승인이 필요하다. |
| Claude 읽기 전용 계약 | 확정 | 설계 최대 12 turns, 리뷰 최대 8 turns를 적용한다. 필수 flag·tool 제한·구조화 출력 지원은 Phase 3에서 설치 버전으로 검증하고, 지원 실패 시 권한 완화 없이 실행을 차단한다. |
| 프로필 연동 구현 시점 | 확정 | Phase 1은 `AppProfile`·`ProviderBinding` 모델만 정의한다. Phase 3에서 프로필 선택, 프로필별 `CODEX_HOME`과 Claude 프로필 분리의 공식 지원 여부를 검증한다. 미지원 시 Claude는 장비별 단일 로그인을 사용한다. |
| 모델 추천 구현 시점 | 확정 | model recommendation은 application layer 정책 service로 두고 Phase 4에서 추천 근거·비용 정보, 사용자 override와 프로젝트별 고정을 구현한다. 승인 없는 고비용 모델 자동 상향은 금지한다. |
| 보존 기간 | 확정 | 완료·중단 작업 기록은 90일, 병합된 worktree와 작업 branch는 안전 조건 충족 시까지 최소 7일 보존한다. |
| 업데이트 배포 | Phase 7 전 결정 | 배포 채널, 릴리스 저장소, 서명키 보관 주체와 플랫폼별 rollback 방식을 Phase 7 시작 전에 확정한다. GitHub Releases와 CI 서명키는 후보일 뿐 현재 결정이 아니다. |
