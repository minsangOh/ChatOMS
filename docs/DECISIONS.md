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
| Phase 1 build gate | 확정 | Windows에서는 Codex fallback pnpm의 PATH 해석을 우회해 `node_modules\.bin\tauri.cmd`를 직접 실행하고, debug `--no-bundle -- --offline --locked -j 1`로 검증한다. |
| Installer 검증 시점 | 확정 | MSI·NSIS와 WiX 공급망은 배포·업데이트 단계에서 검증한다. Tauri는 MSI 생성 시 사용자 cache에 WiX를 외부 다운로드할 수 있으므로 Phase 1에서는 bundle을 생성하지 않는다. Installer icon은 최종 제품 icon 확정 후 검증한다. |
| 정적 전이와 contextual 전이 | 확정 | 정적 상태 행렬은 문맥 없는 전이만 다룬다. 정상 업무 상태의 pause, `Paused`에서 저장된 target으로의 재개, `RecoveryRequired`의 target 설정·pause·재개는 검증 token과 Task aggregate API로 수행한다. `Paused`는 항상 resume target을 보유하며 transition sequence는 1부터 시작하고 연속성 검증은 repository가 담당한다. |
| 불명 외부 효과 복구 | 확정 | `UnknownExternalEffect`는 resume target을 보유하거나 `Paused`로 직접 전이하지 않는다. `RecoveryRequired`로 이동해 복구 분석과 target 검증을 마친 뒤에만 재개하며, `RecoveryRequired → Paused`는 검증된 resume target이 있을 때만 target을 유지하며 허용한다. |
| SQLite connection과 migration | 확정 | Production file DB는 connection마다 `foreign_keys=ON`, WAL, `synchronous=FULL`, 5초 busy timeout을 적용·재검증한다. Migration은 embedded forward-only SQL, 원문 SHA-256 checksum과 독립 transaction을 사용하며 commit 전 `foreign_key_check`를 수행한다. Lifecycle 자동 trigger와 checksum 자동 갱신은 금지하고 sequence 연속성은 repository가 검증한다. |
| Repository transaction 경계 | 확정 | Repository가 Task lifecycle transaction을 소유한다. 생성은 Task·최초 `Created` transition·lease를 함께 commit하고, 종료는 Task 갱신·transition·lease 삭제를 함께 commit한다. expected version과 transition sequence를 검증하며 active lease 자동 탈취·교체와 retry를 금지한다. Persistence row는 domain aggregate와 transition으로 검증해 복원한다. |
| Windows 앱 데이터 경로 | 확정 | `%LOCALAPPDATA%\ChatOMS` 아래에 `data`, `logs`, `artifacts`, `temp`를 분리하고 DB는 `data\chatoms.sqlite3`를 사용한다. Identifier는 `io.github.minsangoh.chatoms`로 유지한다. |
| Windows 앱 데이터 ACL | 확정 | current user와 `NT AUTHORITY\SYSTEM`만 Full Control을 허용하는 protected DACL을 적용·재검증한다. Broad writable principal, reparse point 또는 비-Secure 결과에서는 영구 저장을 차단하며 관리자 권한을 요구하지 않는다. |
| macOS path·permission 검증 | 연기 | Phase 1은 port와 compile 가능한 stub 구조만 제공한다. 실제 Application Support 경로와 권한 적용·빌드 검증은 macOS 환경에서 별도로 수행한다. |
| Cross-layer failure category | 확정 | Platform-neutral `FailureCategory`, severity와 retry 계약은 `chatoms-ports`가 소유한다. Application은 이 계약만 stable error code로 변환하며 infrastructure/platform concrete error를 소유하거나 참조하지 않는다. |
| Source chain과 사용자 오류 분리 | 확정 | SQLite·I/O·OS·tracing source chain은 concrete adapter error에 유지한다. Application error는 category, severity, retry와 고정된 사용자 안전 메시지만 보유한다. |
| RedactedText 저장 경계 | 확정 | 비신뢰 텍스트는 bounded·deterministic redaction 및 잔여 민감정보 검증을 통과한 `RedactedText`로만 영구 진단 경계를 통과한다. 실패 시 raw fallback 없이 fail-closed marker를 사용한다. |
| 로컬 진단 로그 | 확정 | 권한 검증된 logs directory에 ANSI 없는 structured JSON을 non-blocking daily rolling file로 기록한다. Remote telemetry는 사용하지 않으며 application bootstrap 연결은 후속 단계에서 수행한다. |
| Application dependency inversion | 확정 | Application service는 domain과 ports만 사용하며 infrastructure/platform concrete adapter, filesystem, SQLite, process 또는 Tauri에 의존하지 않는다. |
| Bootstrap fail-fast 순서 | 확정 | Secure storage → database → logging → active lease 순서로 호출한다. Storage 또는 database가 준비되지 않으면 후속 port를 호출하지 않으며 logging failure는 raw fallback 없이 degraded health로 처리한다. |
| Application 시간 정책 | 확정 | Use case timestamp는 `TimeProvider` port에서 받는다. Application은 `SystemTime`이나 environment를 직접 읽지 않으며 production clock adapter는 후속 wiring 단계에서 제공한다. |
| Task identity 생성 책임 | 확정 | Task 생성 use case가 domain UUIDv7 ID를 만들고 canonical task branch identity와 최초 transition을 구성한다. 사용자 입력으로 task ID나 branch identity를 받지 않는다. |
| Contextual resume capability | 보류 | Pause와 `RecoveryRequired` 진입은 구현하지만 resume, recovery target 확정과 recovery pause는 실제 외부 상태 validation capability가 추가될 때까지 `Unsupported`다. Opaque validation token을 임의 생성하지 않는다. |
| Application read model | 확정 | `BootstrapStatus`, `SystemStatus`, `ProjectView`, `TaskView`, active task와 transition view는 Tauri/serde 독립 모델이다. IPC DTO와 사용자 경로 표시 정책은 IPC 단계에서 별도 결정한다. |
| Production adapter wiring | 확정 | Platform과 infrastructure의 production adapter 생성·수명 관리는 Tauri composition root 책임이다. Application은 ports만 사용하며 bootstrap은 startup에서 한 번 수행한다. |
| Startup critical/degraded 정책 | 확정 | Secure storage 또는 database 실패는 `Unavailable` runtime으로 제한하고 system health만 안전하게 제공한다. Logging 실패는 raw fallback 없이 `Degraded` ready로 처리하며 logging guard는 runtime 수명 동안 보존한다. |
| Phase 1 IPC surface | 확정 | System 4개, project 목록 1개, task read-only 3개 command만 등록한다. Task create·transition IPC는 Git/worktree/provider orchestration이 준비될 때까지 공개하지 않는다. |
| IPC DTO 및 경로 정책 | 확정 | Application read model과 serde DTO를 분리하고 camelCase를 사용한다. `ProjectDto`에는 전체 root path를 포함하지 않으며 IPC error에는 source, path, SQL, SID, stack 또는 secret을 포함하지 않는다. |
| Frontend IPC boundary | 확정 | Page는 `invoke`를 직접 호출하지 않고 중앙 typed client의 read-only API만 사용한다. Response는 `unknown`에서 DTO guard로 검증하고 오류는 승인된 code·message·severity·retry만 render한다. |
| Frontend state 관리 | 확정 | Phase 1 `/system`과 `/projects`는 React local component state를 사용한다. 별도 state management 또는 query/cache package를 추가하지 않는다. |
| Frontend mutation 범위 | 확정 | Project root path와 task mutation UI는 노출하지 않는다. Task 생성·전이, Git/provider, settings, updater와 installer UI는 해당 후속 Phase까지 구현하지 않는다. |
