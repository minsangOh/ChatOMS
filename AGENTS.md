# ChatOMS 프로젝트 규칙

## 프로젝트 목적

- ChatOMS는 공식 Claude Code CLI와 공식 Codex CLI를 조율하는 로컬 데스크톱 AI 코딩 하네스다.
- Windows를 우선 지원하고 macOS를 지원한다.
- 기술 스택은 Tauri 2, React, TypeScript strict, Rust, SQLite다.

## 요구사항 기준

- 상세 제품 요구사항은 `docs/PRODUCT_REQUIREMENTS.md`를 기준으로 한다.
- 확정되지 않은 기능을 임의로 추가하지 않는다.
- 요구사항과 구현이 충돌하면 구현 전에 사용자에게 보고한다.

## 문서 우선순위

1. `docs/PRODUCT_REQUIREMENTS.md`의 제품 요구사항
2. `docs/DECISIONS.md`, `docs/SECURITY_POLICY.md`, `docs/STATE_MACHINE.md`의 확정 설계·보안·상태 불변조건
3. `docs/PHASE_PLAN.md`의 구현 단계와 범위
4. 이 파일의 현재 작업 규칙
5. `README.md`의 사용자·개발자 안내

같은 단계에서 충돌하거나 구현이 상위 문서와 다르면 임의로 해석하거나 구현하지 말고 사용자에게 보고한다. 이전 Phase 설명은 historical record이며 현재 작업 규칙이 아니다.

## 현재 구현 범위

- Phase 1~3은 완료되었고, 현재 승인된 구현 범위는 Phase 4 Unit 1의 provider-neutral `TaskState` rename과 SQLite forward migration, Unit 2의 `WorkKind` 및 provider eligibility 읽기 모델, Unit 3의 실제 provider CLI 실행 없이 fixture 기반으로 검증된 streaming/cancellation ProcessRunner 기반, Unit 4a-1의 task 생성 경계에 결합된 immutable `TaskBrief`(requirements/completionCriteria/prohibitedScope) 저장 기반, Unit 4a-2의 `TaskBrief` 입력 폼과 필수 검증 기반, Unit 4b-1의 Claude Planning 전송 동의 저장과 `WorktreeReady → Planning` 원자적 전이 기반, Unit 4b-2의 Claude Planning 실행 adapter 기반, Unit 4b-3의 Claude Planning 최종 결과 저장과 상태 전이 기반, Unit 4b-4의 Claude Planning 시작·취소 Tauri IPC/UI 배선과 `Planning → Cancelled` 직접 전이 기반, Unit 4b-5의 앱 재시작 시 leftover `Planning` task를 `RecoveryRequired`로 되돌리는 startup reconciliation 기반, Unit 4b-6의 Claude Planning background thread panic containment 기반, Unit 4b-7의 `AwaitingDesignApproval` task에 대한 저장된 Claude Planning 결과 read-only 표시 기반이다.
- Unit 4a-2의 구현: ProjectsPage에서 task 생성 전 세 TaskBrief 필드(requirements, completionCriteria, prohibitedScope)를 입력받고, 빈 값이면 요청을 보내지 않으며, IPC/Tauri boundary에서도 필수 검증으로 UI 우회를 방지한다.
- Unit 4b-1의 구현: `task_provider_consents` immutable 테이블과 `TaskService::start_planning` 유스케이스로 Claude Planning 1회 전송 동의를 기록·재사용하고, 동일 task version에 유효한 동의가 있으면 재사용하며, 동의 기록·`WorktreeReady → Planning` 상태 갱신·transition history 저장을 하나의 SQLite transaction으로 처리한다. 실제 Claude CLI 실행, Tauri IPC 노출과 동의 UI는 포함하지 않는다.
- Unit 4b-2의 구현: `crates/chatoms-infrastructure/src/claude_planning.rs`의 `ClaudePlanningAdapter`가 Unit 3 `StreamingProcessRunner`로 Claude Planning을 실행한다. spawn 직전 매번 `ProviderCapabilityPort`(기존 trust/compatibility/login/preflight 게이트)를 다시 호출해 캐시를 신뢰하지 않고, 실패 시 typed `PreflightRejected` 결과만 반환하며 subprocess를 시작하지 않는다. CWD는 app-owned trusted preflight directory이고 task worktree는 `--add-dir` 읽기 전용 인자로만 전달하며, TaskBrief 세 필드는 고정 템플릿으로 stdin에만 실린다. `ClaudePlanningObserver`는 raw stdout 콜백이 없는 content-free 이벤트만 노출한다.
- Unit 4b-3의 구현: 공식 문서로 `-p --permission-mode plan`이 헤드리스에서 승인 대화상자 없이 계획을 stdout에 출력하고 정상 종료함을 확인했고, argv에 `--output-format json`을 추가해 종료 시 단일 JSON envelope만 받는다. `ClaudePlanningAdapter`는 raw stdout을 호출부에 노출하지 않고 내부에서만 bounded 버퍼로 누적한 뒤 `subtype`/`is_error`/`result`/`num_turns` 필드만 신뢰하는 최소 스키마로 파싱하고, `SecretRedactor::redact_text`로 마스킹·크기 제한한 뒤 `ClaudePlanningResult`(content-free `PlanningResultOutcome` + 안전한 plan text)를 반환한다. session_id·비용·기타 필드는 파싱 대상이 아니라 애초에 포섭되지 않는다. `crates/chatoms-infrastructure/migrations/0007_task_planning_results.sql`이 `task_id` 1:1, provider/work_kind 고정, outcome CHECK, plan_text 존재 규칙을 가진 immutable 테이블을 추가한다. `TaskService::record_planning_result`가 결과 저장과 상태 전이(`Completed→AwaitingDesignApproval`, `Failed→Failed`, `RecoveryRequired→RecoveryRequired`)를 하나의 repository transaction으로 처리하고, 원자적 쓰기 자체가 실패하면 결과 행 없이 `Planning→RecoveryRequired`로 fallback한다. StreamingOutcome 매핑: `Completed`+`exit_code==0`+유효 스키마→`Completed`, `Completed`+nonzero 또는 `StdoutBoundExceeded`→`Failed`, `Cancelled`→`Cancelled`, `Uncertain`→`RecoveryRequired`(Planning의 `--tools Read,Glob,Grep` 계약상 외부 쓰기가 구조적으로 불가능하므로 `UnknownExternalEffect`는 이 Unit에서 도달 불가로 판단). 실제 Claude/Codex CLI, 로그인 probe와 configured executable은 테스트에서 실행하지 않는다.
- Unit 4b-4의 구현: 사용자 승인으로 `docs/STATE_MACHINE.md`의 `Planning` 행에 `Cancelled`를 직접 다음 상태로 추가했다(다른 active-execution 상태와 달리 `Paused`를 거치지 않음; 근거는 STATE_MACHINE.md 본문 참고). `chatoms-domain`의 `can_transition_to`에 이 edge를 반영했고, `TaskService::record_planning_result`는 이제 `PlanningResultOutcome::Cancelled`를 `TaskState::Cancelled`로 매핑해 결과 저장·상태 전이·lease 해제를 원자적으로 처리한다(과거의 `Unsupported` fallback은 제거됨). `chatoms-ports::planning::ClaudePlanningExecutor` port와 `chatoms-infrastructure`의 `ClaudePlanningAdapter` 구현체를 추가해 provider 실행을 port/adapter로 분리했고, `chatoms-application::planning_execution`의 `PlanningExecutionStarter::begin`(fresh capability 재확인 + `TaskService::start_planning` 원자 전이 + worktree/brief 조회, 거부 시 상태 불변)과 `PlanningExecutionRecorder::run_and_record`(provider 실행 + `TaskService::record_planning_result`, preflight 거부·executor 실패는 `RecoveryRequired`로 fallback)가 이를 연결한다. Tauri의 `start_claude_planning` 명령은 동의·전이를 동기적으로 수행해 즉시 `TaskDto`를 반환하고, 실제 provider 실행과 결과 기록은 detached background thread에서 진행하며, 메모리 전용 `PlanningRunRegistry`(task별 `AtomicCancellationSignal`)가 동시 실행 중인 `cancel_claude_planning` 요청이 그 thread에 도달하도록 한다. 취소 요청은 상태를 직접 바꾸지 않고, streaming runner가 확인한 결과만 `Cancelled`(확인된 취소) 또는 `RecoveryRequired`(불확실한 취소, lease 유지)로 기록된다. ProjectsPage는 `WorktreeReady`에서 `get_provider_eligibility` 결과로 Start 버튼을 게이팅하고 고정 blocker 문구만 표시하며, 전송 동의 확인 화면을 거친 뒤 시작하고, `Planning` 중에는 취소 버튼과 polling 기반 진행 상태를, 종료 상태에는 안전한 상태 문구만 표시한다. 이 과정에서 `RepositoryHandle`(src-tauri)이 `get_claude_binding`/`ensure_default_profile_and_claude_binding`/`update_claude_executable_path`를 위임하지 않고 있던 Phase 3의 기존 버그를 발견해 함께 수정했다(해당 세 메서드가 트레이트 기본값 `OperationFailed`로 빠져 프로덕션에서 항상 실패하고 있었음).
- Unit 4b-4는 Codex, Claude write, resume/handoff, `PlanReady`, Context Package 확장, planning 결과 상세 뷰, 고위험 승인 화면, 별도 execution/session table을 포함하지 않는다.
- Unit 4b-5의 구현: `TaskService::reconcile_startup_planning`(`crates/chatoms-application/src/tasks.rs`)이 `PlanningRunRegistry`가 메모리 전용이라 앱 재시작 뒤에는 재사용·자동 재실행할 실행 핸들이 없다는 근거로, 활성 lease의 task가 `Planning`이면 기존 `mark_recovery_required` 경로(정적 전이와 history 저장을 하나의 repository transaction으로 처리하며 non-terminal이므로 `ActiveTaskLease`를 유지)를 그대로 재사용해 `RecoveryRequired`로 전이한다. 활성 task가 없거나 `Planning`이 아니면 no-op이고, repository 오류는 그대로 전파해 성공으로 추정하지 않는다. `src-tauri/src/bootstrap.rs`의 `compose_runtime`이 기존 `GitIsolationService::reconcile_startup` 직후 이를 호출하며, 실패하면 기존 Git reconciliation과 동일하게 `ManagedRuntime::unavailable`로 fail-closed한다. `PlanningRunRegistry::register`(`src-tauri/src/state.rs`)는 이미 등록된 task id를 덮어쓰지 않고 `None`을 반환해 충돌을 거부하도록 수정했고, `handle_start_claude_planning`은 이 `None`을 `WorktreeReady → Planning` 전이가 이미 커밋된 뒤의 invariant 위반으로 취급해 `TaskService::record_planning_result`로 `RecoveryRequired`에 즉시 fallback한다. `cancel_claude_planning`이 registry에서 실행을 찾지 못하면(`requested: false`) ProjectsPage가 이를 오류로 취급해 상태 새로고침을 안내하는 안전한 복구 문구를 표시한다.
- Unit 4b-5는 provider 재실행, session resume/handoff, process 탐색·종료, 별도 execution/session table, Codex, Claude write와 planning 결과 상세 UI를 포함하지 않는다.
- Unit 4b-6의 구현: `PlanningExecutionRecorder::run_and_record_with_panic_containment`(`crates/chatoms-application/src/planning_execution.rs`)가 `run_and_record`(executor 호출) 전체를 `std::panic::catch_unwind` + `AssertUnwindSafe`로 감싸고, panic을 잡으면 payload를 검사·포맷·저장하지 않고 즉시 버린 뒤 기존 `TaskService::record_planning_result`(`PlanningResultOutcome::RecoveryRequired`) 경로를 그대로 재사용해 `Planning → RecoveryRequired` 전이·history 기록·lease 유지를 수행한다. 이 recovery 기록 자체가 실패하면(예: stale task version) 성공으로 추정하지 않고 그 오류를 그대로 반환하며, task는 `Planning`으로 남을 수 있고 Unit 4b-5의 `reconcile_startup_planning`이 다음 기동에서 회수한다. `src-tauri/src/commands/planning.rs`의 background thread는 이제 이 containment 경로를 호출하고, `PlanningRunRegistry` unregister는 명시적 호출 대신 `UnregisterOnDrop` RAII guard(스레드 진입 직후 생성)의 `Drop`으로 옮겨, panic 발생 위치·유무와 무관하게 unregister가 항상 실행되도록 보장한다(이 guard 자체는 순수 in-memory 정리이므로 panic 시에도 실패하지 않는다). 그 결과 `cancel_claude_planning`은 이미 죽은 thread를 가리키는 stale registry entry를 더 이상 "찾음"으로 잘못 보고하지 않는다. Rust의 기본 panic hook이 찍는 1줄 stderr 메시지 자체를 전역적으로 억제하는 process-wide `panic::set_hook` 변경은 이 Unit의 좁은 범위를 넘는 것으로 판단해 도입하지 않았으며, 이 앱 자신의 UI/DTO/DB/structured 로그 표면에는 panic payload가 어떤 경로로도 도달하지 않는다.
- Unit 4b-6은 실제 Claude/Codex CLI 실행, session/execution table, resume/handoff, provider 변경, UI 기능 확장과 전역 panic hook 변경을 포함하지 않는다.
- Unit 4b-7의 구현: `FoundationRepository::get_task_planning_result`(`crates/chatoms-ports/src/repository.rs`)와 그 SQLite 구현(`load_planning_result`, `crates/chatoms-infrastructure/src/database/repository.rs`)이 `task_planning_results`의 immutable 저장값을 그대로 읽어오고, `TaskService::get_planning_result`(`crates/chatoms-application/src/tasks.rs`)가 이를 `PlanningResultView`로 노출한다. Tauri의 `get_planning_result` 명령(`src-tauri/src/commands/planning.rs`)은 task를 조회해 현재 상태가 `AwaitingDesignApproval`일 때만 결과를 반환하고, 그 외 상태(및 결과 미기록)는 동일하게 `Ok(None)`으로 취급해 이 read-only 표면이 `record_planning_result`의 outcome-to-state 매핑에 의존하지 않게 한다. `PlanningResultDto`(`src-tauri/src/dto/planning_result.rs`)는 저장된 plan_text·outcome·exit_code·turn_count·시각만 그대로 직렬화하며 재마스킹·재파싱·재실행을 하지 않는다. 프런트엔드는 `src/ipc/planning_result.ts`의 런타임 가드로 unknown/malformed 응답을 거부하고, ProjectsPage는 `awaitingDesignApproval`에서만 결과를 조회해 읽기 전용으로 표시하며 결과 없음·조회 실패를 raw 오류 없는 안전한 빈 상태/오류 상태로 표시한다.
- Unit 4b-7은 "계속하기"·재계획·구현 시작·provider 재실행 UI, 고위험 승인 화면, planning 결과 수정·삭제, Codex, Claude write, resume/handoff, 별도 execution/session table을 포함하지 않는다.
- 승인된 Unit 4b-7을 넘는 Phase 4 후속 기능을 선제 구현하지 않는다.
- Phase 2의 목적별 local Git 격리는 유지한다. remote Git, 기본 브랜치 mutation, branch/worktree 삭제·정리와 병합은 여전히 구현 범위 밖이다.
- 실제 provider session 시작과 Claude/Codex CLI 실행은 별도 요구사항 승인 없이는 구현하지 않는다.

## 아키텍처

- `domain` 계층은 Tauri, SQLite, CLI 프로토콜에 의존하지 않는다.
- UI는 Git, CLI, 파일 시스템을 직접 조작하지 않는다.
- 프런트엔드는 Tauri IPC를 통해서만 Rust application service를 호출한다.
- 공급자, Git, 프로세스, 업데이트 기능은 port와 adapter로 분리한다.

## 보안

- OAuth 토큰, API 키, 비밀번호, 쿠키, 인증 헤더, 개인키와 CLI 인증정보를 읽거나 출력하거나 저장하지 않는다.
- 로그와 artifact는 영구 저장 전에 민감정보를 마스킹한다.
- AI 출력은 비신뢰 입력으로 취급한다.
- 셸 문자열 직접 실행을 기본 실행 방식으로 사용하지 않는다.
- 프로젝트 밖 파일 변경과 외부 전송은 사용자 승인 없이는 금지한다.

## Git 및 작업 격리

- 향후 모든 코드 변경은 작업별 branch와 worktree에서 수행한다.
- 한 `Task`는 정확히 하나의 task branch와 최대 하나의 worktree만 가진다.
- 기본 브랜치를 직접 수정하지 않는다.
- 현재 저장소 자체의 최초 프로젝트 초기화 절차는 사용자의 명시적 지시에 따른다.

## 상태와 데이터

- `Task.project_id`는 생성 후 불변이다.
- 전체 앱의 단일 활성 작업은 `ActiveTaskLease`와 DB 제약으로 함께 보장한다.
- `Paused`, `RecoveryRequired`, `UnknownExternalEffect`를 포함한 모든 실행 비종료 상태는 `ActiveTaskLease`를 유지한다.
- `ActiveTaskLease`는 `Completed`, `Failed`, `Cancelled` 중 하나로 진입할 때만 해제한다.
- `CleanupPending`과 `Archived`는 종료 결과 이후의 보존 수명주기이며 lease를 다시 획득하지 않는다.
- 상태 변경은 임의 문자열 갱신이 아니라 Rust 도메인 유스케이스를 통해 수행한다.
- 현재 상태 갱신, 상태 전이 이력 저장과 종료 전이의 `ActiveTaskLease` 해제는 하나의 트랜잭션으로 처리한다.

## 코드 품질

- TypeScript strict 모드를 유지한다.
- Rust에서 불필요한 `unwrap`, `expect`, `panic`을 사용하지 않는다.
- 사용자 오류 메시지와 내부 오류 원인을 분리한다.
- 새 불변조건에는 테스트를 추가한다.
- migration은 재실행 가능해야 하며 적용 순서를 검증한다.
- 변경 후 Rust format, clippy, test와 TypeScript typecheck, 프런트 테스트를 실행한다.

## 패키지

- 새 패키지를 추가하기 전에 패키지명, 버전, 출처, 목적, 변경될 manifest와 lockfile을 사용자에게 보고한다.
- 전역 설치와 시스템 환경 변경을 금지한다.

## 작업 완료 보고

다음 순서로 보고한다.

1. 변경 요약
2. 생성·수정 파일
3. 실행한 명령
4. 테스트·빌드 결과
5. 실패 또는 미해결 위험
6. 다음 Phase로 넘긴 항목
