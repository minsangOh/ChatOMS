# ChatOMS 아키텍처

## 시스템 개요

ChatOMS는 별도 웹 서버 없이 한 장비에서 실행되는 Tauri 데스크톱 애플리케이션이다. UI는 시스템 자원을 직접 조작하지 않고, 모든 작업을 타입이 정의된 Tauri IPC를 통해 Rust application layer에 요청한다.

### Runtime 호출 흐름

아래 화살표는 실행 중 호출 방향을 나타낸다.

```mermaid
flowchart TB
    UI["React UI"] --> IPC["Typed Tauri IPC"]
    IPC --> APP["Rust application layer"]
    APP --> DOMAIN["Domain"]
    APP --> PORTS["Ports"]
    PORTS --> ADAPTERS["Adapters / infrastructure"]
    ADAPTERS --> PROVIDERS["Claude / Codex"]
    ADAPTERS --> SYSTEMS["Git / Process / SQLite / Artifacts"]
    ADAPTERS --> PLATFORM["Windows / macOS services"]
```

## 계층과 책임

### React UI

- 시스템 상태, 프로젝트, 작업 진행, 승인, 결과와 복구 정보를 표시한다.
- Git, CLI, SQLite, 파일 시스템 또는 네트워크를 직접 호출하지 않는다.
- Rust application service가 노출한 Tauri command를 호출하고 event stream을 구독한다.
- Phase 2 UI는 앱 셸, 시스템 상태, 프로젝트 등록·상태 조회와 task isolation 생성·복구 상태 표시를 제공한다.

### Tauri IPC

- 프런트엔드 DTO와 Rust application request/response 사이의 경계다.
- 입력을 검증하고 내부 오류를 안전한 사용자 오류로 변환한다.
- 내부 경로, 원문 프로세스 오류 또는 비밀정보를 UI에 그대로 전달하지 않는다.

### Rust application layer

- 프로젝트 등록, 작업 시작, 승인, 하네스 진행, 복구와 정리 유스케이스를 조정한다.
- 전체 앱의 단일 `ActiveTaskLease`와 작업별 단일 branch·최대 단일 worktree 불변조건을 트랜잭션으로 관리한다.
- domain port만 사용하며 구체적인 CLI, Git, SQLite와 OS 구현을 직접 포함하지 않는다.

### Domain core

- 프로젝트, 프로필, 작업, 상태 전이, 승인, Context Package와 artifact 참조를 모델링한다.
- Tauri, React, SQLite, Git 명령 및 Claude/Codex 프로토콜에 의존하지 않는다.
- 상태 전이는 [작업 상태 머신](STATE_MACHINE.md)에 정의된 도메인 유스케이스로만 수행한다.

### Ports, adapters, infrastructure

- Ports는 현재 Phase가 필요로 하는 의미 기반 공급자, Git, 저장소, 플랫폼과 업데이트 계약을 정의한다. 구현되지 않은 후속 Phase port를 선제 노출하지 않는다.
- Adapters는 Claude CLI, Codex app-server, Git/worktree와 구조화 프로세스 실행을 구현한다.
- Infrastructure는 SQLite repository, artifact 저장소, 로깅, 마스킹과 OS별 서비스를 구현한다.
- Phase 1~6의 UpdateService는 추상화만 존재하며 실제 배포 채널과 설치 동작은 Phase 7 전 결정한다.

| Port | 책임 | Adapter / infrastructure 경계 |
|---|---|---|
| `ProviderService` | 공급자 실행, event 변환, capability 검증 | Claude CLI adapter, Codex app-server adapter |
| `GitService` | 저장소 감지·상태 조회, 승인된 초기화, 기준 commit과 task branch/worktree 생성·검증 | `chatoms-infrastructure` Git adapter |
| `ProcessRunner` | 프로그램과 인수 배열 기반 실행, 출력·종료 제어 | Phase 3에서 공급자 실행과 함께 추가 |
| `ArtifactRepository` | 마스킹된 로그, diff, 결과와 첨부파일 저장·조회 | 로컬 artifact infrastructure |
| `FilePermissionService` | 앱 데이터 경로의 사용자 전용 권한 적용·검증 | Windows/macOS platform adapter |
| `UpdateService` | 업데이트 상태와 향후 업데이트 동작의 계약 | Phase 1~6에는 port만 정의하고 구현은 Phase 7 결정 이후 추가 |

이 표는 전체 목표 경계를 설명한다. 현재 source 기준으로 repository·path·permission·bootstrap·time port와 Phase 2 의미 기반 `GitService`가 있고, `ProviderService`와 범용 `ProcessRunner`는 Phase 3, 실제 `UpdateService`는 Phase 7에서 추가한다.

## 컴파일 의존 방향

아래 화살표는 런타임 호출이 아니라 소스 코드의 compile-time 의존을 나타낸다. Adapter와 infrastructure가 application layer에 정의된 port를 구현한다.

```text
React UI
  → Tauri IPC
    → application layer
      → domain
      → ports

adapters / infrastructure
  → ports
  → domain types (필요한 경우에만)
```

- 바깥 계층은 안쪽 계층을 의존할 수 있지만 domain core는 바깥 계층을 의존하지 않는다.
- 공급자 고유 메시지는 adapter 내부에서 공통 provider event로 변환한다.
- OS 조건문은 platform adapter에 한정하고 상위 유스케이스로 확산하지 않는다.
- model recommendation은 application layer의 정책 service가 담당하고 UI나 provider adapter가 임의로 결정하지 않는다.
- Gajae-Code에서 참고한 workflow 개념은 외부 adapter가 아니라 내부 Harness Orchestrator/Workflow Engine이 소유한다. 외부 Gajae-Code 저장소나 protocol에는 의존하지 않는다.

## Provider 실행 계약과 capability

설계·구현·리뷰는 작업 종류이며 특정 provider에 고정하지 않는다. 사용자가 각 작업 종류의 실행을 시작할 때 eligible provider를 선택한다. Eligible provider란 capability가 `Supported`이고 해당 작업 종류에 대한 승인된 실행 계약을 가진 provider다. 장기적으로 Claude Code와 Codex 모두 모든 작업 종류에 선택 가능해야 하지만, 실제 선택 가능 여부는 각 provider의 capability 상태와 승인된 실행 계약에 따라 제한된다. 실행 중 provider 자동 전환과 세션 handoff는 현재 범위에 포함하지 않는다.

### Claude — 현재 승인된 실행 계약

- 현재 승인된 계약은 읽기 전용 설계·리뷰 계약뿐이다. 설계 실행은 최대 12 turns, 리뷰 실행은 최대 8 turns로 제한한다.
- 읽기 전용 실행은 `--permission-mode plan`으로 파일 편집을 차단하고 `--tools "Read,Glob,Grep"` allowlist로 `Bash`를 포함한 나머지 도구를 차단하는 조합이다. 두 flag는 함께 필요하며 `--disallowedTools`가 아니라 이 allowlist를 우선 사용한다. 출력은 stream-json, 최종 결과는 JSON Schema로 검증한다.
- Claude provider capability의 `Supported`는 executable trust, 로컬 CLI compatibility, 로그인 상태를 모두 통과했을 때만 성립한다.
- Compatibility와 로그인 상태 검증은 모델 세션을 시작하거나 외부로 전송하지 않는 정적·로컬 preflight로 수행하며, 신뢰된 사용자 지정 executable에 대해서만 `--version`, 도움말 기반 필수 flag 확인과 `claude auth status` 종료 코드를 사용한다.
- `claude auth status`의 표준출력·표준오류는 절대 읽지 않으며 로그인 여부는 종료 코드만으로 판정한다.
- `--version`과 도움말 기반 필수 flag 확인의 표준출력은 compatibility 판정에 필요한 범위에서만 메모리 안에서 일시적으로 해석한 뒤 즉시 폐기한다. 표준오류는 이때도 해석·표시·저장하지 않으며, 원문은 UI, 로그, DB, artifact 어디에도 표시·저장하지 않는다.
- 실제 모델 호출을 통한 검증은 작업 시작 시 공급자 전송 승인 이후 단계로 미룬다.
- 신뢰·버전·필수 flag·로그인 상태 중 하나라도 실패하거나 판정이 모호하면 `Supported`가 아닌 `Unsupported`로 fail-closed 처리하며 권한을 완화하거나 비구조화 출력으로 전환하지 않는다.
- Claude 구현(write) 실행 계약은 장기 목표에 포함되나 아직 정의되지 않았다. 정의 전까지 Claude를 구현 작업 종류에 선택할 수 없다.

### Codex — 현재 capability 상태

- 구현 실행 계약이 정의되어 있으며 `codex app-server`의 stdio JSONL 프로토콜을 제어 방식으로 사용한다. 공식 문서 기준 `codex app-server`의 maturity는 Experimental이다.
- schema/capability 검증 절차는 공식 문서 근거가 확인되고 별도 구현계획이 승인되기 전까지 구현하지 않는다.
- Codex 실행파일의 서명 또는 동등한 신뢰 근거가 확인되기 전까지 Codex capability는 `Unsupported`로 보고되며, 호환성 검증이 실패해도 `codex exec`로 자동 전환하지 않는다.
- Codex의 설계·리뷰 실행 계약은 아직 정의되지 않았다.

## Context Package 흐름

전체 모델 대화를 그대로 전달하지 않고 작업별 Context Package를 사용한다.

```mermaid
sequenceDiagram
    participant User
    participant App as Application Layer
    participant DP as 설계 Provider
    participant Store as SQLite / Artifacts
    participant IP as 구현 Provider
    participant RP as 리뷰 Provider

    User->>App: 작업 요청·완료 조건·금지 범위
    User->>App: 각 작업 종류별 eligible provider 선택
    App->>DP: 마스킹된 설계 Context Package
    DP-->>App: 구조화된 설계·위험
    App->>Store: 결정·승인·Context revision 기록
    App->>IP: 승인 범위와 설계 Context Package
    IP-->>App: 변경·테스트·수정 결과
    App->>Store: diff·로그·결과 artifact 기록
    App->>RP: 실제 worktree 참조와 리뷰 package
    RP-->>App: 구조화된 리뷰 결과
```

- Package에는 작업 ID, 프로젝트/worktree, 기준 commit, 요구사항, 완료 조건, 결정, 금지 범위, 관련 파일, 테스트 결과, 수정 이력과 미해결 위험을 포함한다.
- 저장 및 공급자 전송 전에 민감정보를 마스킹한다.
- 각 모델은 Context Package만 신뢰하지 않고 허용된 범위에서 동일 worktree의 실제 파일을 확인한다.
- 공급자 전송은 [보안 정책](SECURITY_POLICY.md)의 작업별 1회 동의를 따른다.

## 저장소 역할

### SQLite

- 프로젝트, 프로필 참조, 작업, 현재 상태와 상태 전이 이력을 저장한다.
- 실행·승인·모델 선택·병합·복구 메타데이터와 artifact 참조를 저장한다.
- DB의 foreign key·unique·check 제약은 단일 `ActiveTaskLease`와 관계 cardinality 같은 영속 불변조건을 방어하고, domain은 상태 전이와 업무 의미를 검증한다.
- 모든 production file connection은 중앙 initializer에서 `foreign_keys=ON`, `journal_mode=WAL`, `synchronous=FULL`, `busy_timeout=5000`을 적용한 뒤 값을 다시 읽어 검증한다. `foreign_keys=OFF` raw connection은 schema가 방어할 수 없는 신뢰 경계 밖이다.
- migration은 application에 포함된 forward-only SQL과 원문 byte 기준 SHA-256 registry를 사용한다. 각 migration은 독립 transaction에서 SQL, `foreign_key_check`, metadata 기록을 함께 commit하며 실패한 현재 migration만 rollback한다.
- migration trigger는 immutable column과 불법 lease 직접 조작 방어에만 사용한다. Task·transition·lease lifecycle과 transition sequence 증가는 자동화하지 않으며 repository가 명시적인 transaction과 마지막 sequence 검증을 소유한다.
- Task 생성은 Task, 최초 `Created` transition과 lease를 하나의 `IMMEDIATE` transaction으로 기록한다. 종료 전이는 Task 갱신, transition 기록과 lease 삭제를 하나의 transaction으로 처리한다.
- 일반·contextual·post-terminal 전이는 expected version을 SQL `WHERE` 조건으로 검증하고, repository가 transition sequence 연속성과 DB 현재 상태 대비 aggregate 문맥을 재검증한다.
- Repository는 active lease를 자동 탈취·교체하거나 retry하지 않는다. 충돌은 typed error로 반환하며 recovery 정책은 후속 application use case가 결정한다.
- 읽기 경로는 UUID, 상태, branch identity, resume/terminal 불변조건과 transition history를 검증한 뒤 domain aggregate로 복원한다.

### Artifact 저장소

- 전체 로그, stdout/stderr, diff, 테스트·빌드 결과, 리뷰, 실패 보고서, 체크포인트와 첨부파일을 저장한다.
- 데이터는 Git 추적, 공유 폴더와 클라우드 동기화 폴더 밖의 앱 데이터 경로에 둔다.
- 원문을 영구 저장하기 전에 [보안 정책](SECURITY_POLICY.md)에 따라 마스킹한다.

## 플랫폼 추상화

Windows Native와 macOS 구현은 다음과 같은 port 뒤에 분리한다.

- `PlatformService`
- `PathResolver`
- `FilePermissionService`
- `ProcessRunner`
- `NotificationService`
- `DiskEncryptionStatusService`
- `UpdateService`

Windows를 우선 구현하며 WSL은 MVP 지원 범위에서 제외한다.

### 안전한 앱 데이터 경로

- `AppPathResolver` port는 경로 계산과 layout 검증만 담당하고, `FilesystemPermissionManager` port는 ACL 적용과 재검증만 담당한다.
- Windows 구현은 `%LOCALAPPDATA%\ChatOMS` 아래에 `data`, `logs`, `artifacts`, `temp`, `worktrees`를 분리하며 DB는 `data\chatoms.sqlite3`에 둔다.
- Phase 2 worktree는 검증된 `worktrees\<project-id>\<task-id>` 경로에만 생성하고 프로젝트 sibling이나 `temp`에는 생성하지 않는다.
- `SecureAppPaths`는 절대 local layout 확인, reparse point 거부, directory 생성, 권한 적용과 재검증 순서로 준비하며 `Secure`가 아닌 결과에는 validated path를 반환하지 않는다.
- Application layer는 `SecureAppPaths`가 반환한 경로만 영구 SQLite, log와 artifact 저장에 사용한다.
- macOS는 Phase 1에서 동일 port의 compile 구조와 `Unsupported` permission 결과만 제공하며 실제 경로·권한 검증은 macOS 환경에서 후속 수행한다.

## 오류 분류와 안전한 진단 경계

- `chatoms-ports`는 adapter 구현을 모르는 `FailureCategory`, `FailureSeverity`, `RetryDisposition`과 `CategorizedFailure` 계약을 소유한다.
- Infrastructure와 platform의 concrete error는 SQLite, OS, I/O 및 tracing source chain을 내부 진단용으로 유지하되 ports category로만 분류한다.
- Application은 domain error 또는 ports category만 stable `ApplicationError` code와 사용자 안전 메시지로 변환한다. `chatoms-infrastructure`와 `chatoms-platform`에 의존하지 않는다.
- 비신뢰 문자열은 infrastructure의 `SecretRedactor`를 거쳐 `RedactedText`가 된 후에만 영구 로그 경계로 전달한다.
- Production logging은 `SecureAppPaths` 결과와 `PermissionStatus::Secure`로 만든 validated logs directory만 받고, ANSI 없는 structured JSON을 non-blocking daily rolling file로 기록한다.
- 이 계약은 application의 stable `ApplicationError` mapping, Tauri IPC의 safe `IpcErrorDto`, frontend의 `FrontendError` safe field 표시까지 연결됐다. Read-only IPC와 `/system`, `/projects` UI가 구현됐으며 source, path, SID, SQL과 secret은 사용자 오류에 노출하지 않는다.

## Phase 1 application service (기반 이력)

- Application은 `BootstrapService`, `SystemService`, `ProjectService`, `TaskService`로 분리하며 각 서비스는 필요한 port만 빌려 사용한다. Global singleton이나 concrete adapter service locator는 두지 않는다.
- Bootstrap 순서는 secure storage 준비 → database bootstrap → logging bootstrap → active lease 조회다. Storage가 secure하지 않으면 뒤 단계를 호출하지 않고, database가 ready/upgraded가 아니면 logging과 lease 조회를 호출하지 않는다.
- Logging unavailable은 raw fallback 없이 degraded 상태로 처리한다. Secure storage와 database가 준비됐다면 application bootstrap 자체는 ready이며 system health는 `Degraded`다.
- System/project/task 결과는 Tauri 및 serde와 독립적인 application read model이다. IPC DTO는 Tauri 계층에서 별도로 정의하며 `ProjectDto`와 frontend에는 root path를 노출하지 않는다.
- Task 생성 시 application이 UUIDv7 Task ID와 branch identity, 최초 `Created` transition을 구성하고 repository의 원자적 create boundary를 호출한다. 이후 sequence 값은 repository port가 history를 캡슐화해 제공하고 concrete repository가 transaction 안에서 재검증한다.
- 정적 전이, pause 진입, `RecoveryRequired` 진입과 terminal 전이는 domain aggregate를 거쳐 repository transaction port로 저장한다. Resume/recovery target 확정은 실제 validation capability가 없으므로 현재 구현 API에서 `Unsupported`로 보류하며 validation token을 임의 생성하지 않는다.
- 시간은 `TimeProvider` port로 주입하며 production에서는 검증된 Unix epoch milliseconds를 반환하는 `SystemTimeProvider`를 Tauri composition root가 연결한다.

## Phase 1 production adapter와 Tauri IPC (기반 이력)

- Tauri composition root가 platform storage·time·capability adapter와 infrastructure database·logging·repository adapter를 구성하고 `BootstrapService`에 주입한다. Application service는 concrete adapter를 알지 못한다.
- Production startup은 한 번만 수행한다. Secure path, 단일 SQLite connection owner와 non-blocking logging guard는 managed runtime이 공유 소유하고, command마다 bootstrap이나 connection open을 반복하지 않는다.
- SQLite connection은 `SharedDatabase`의 `Mutex` 안에 보관하고 repository adapter만 접근한다. Global mutable state나 unsafe `Send`/`Sync` 구현은 사용하지 않는다.
- Managed runtime은 `Ready(AppRuntime)`와 `Unavailable`을 구분한다. Secure storage·database 실패는 unavailable이며 logging 실패는 raw fallback 없이 degraded ready다. Unavailable에서도 version과 health만 안전하게 조회할 수 있다.
- Tauri command는 managed state 획득, application service 호출, application read model의 IPC DTO 변환, safe IPC error 변환만 수행한다. SQL, filesystem, ACL, migration 또는 process를 직접 호출하지 않는다.
- IPC DTO는 camelCase와 안정적인 enum 문자열을 사용한다. UUID는 lowercase canonical 문자열, timestamp는 Unix epoch milliseconds이며 `ProjectDto`에는 전체 root path를 포함하지 않는다.
- Phase 1에서 도입한 read-only handler는 그대로 유지한다. Phase 2는 `inspect_project_candidate`, `register_project`, `get_project_git_status`, `create_isolation_task`, `get_task_isolation`, `approve_git_initialization`, `create_task_worktree` 목적별 handler만 추가하며 generic transition·provider·updater·installer command는 공개하지 않는다.

## Phase 1 frontend (기반 이력)

- React page는 Tauri `invoke`를 직접 호출하지 않는다. 중앙 typed IPC client가 승인된 command 문자열, camelCase payload, response guard와 safe frontend error 변환을 소유한다.
- Frontend type은 Rust IPC DTO의 camelCase 필드와 안정적 enum 문자열을 그대로 반영한다. Transport 결과는 `unknown`으로 받고 DTO별 runtime guard를 통과한 값만 page state에 전달한다.
- `/system`은 system·bootstrap status를 중심으로 version, health, storage, database, logging, active lease와 Phase 1 capability를 read-only로 표시한다. 핵심 호출 실패는 safe error state, 보조 호출 실패는 기존 application status를 변경하지 않는 partial notice로 처리한다.
- Phase 2 `/projects`는 project name, ID, timestamp와 안전한 display path를 표시하고 등록·Git 상태 조회·task isolation action을 제공한다. canonical root path는 frontend type과 render tree에 포함하지 않는다.
- Router와 page는 React local state만 사용한다. 별도 state manager, query cache, UI library 또는 CSS framework를 추가하지 않는다.
- 공통 loading, empty, error와 status badge는 semantic role, 텍스트 label, visible focus와 retry disposition을 적용하며 raw invoke error, source, path, SQL, SID, stack 또는 secret을 표시하지 않는다.

## Phase 2 프로젝트와 Git isolation

- 프로젝트 후보 검사는 첫 Git probe 전에 mutation 없이 존재하는 `DRIVE_FIXED` local directory의 final path와 Windows volume serial/directory file ID를 조회한다. Git 하위 경로는 enclosing repository root를 반환하고 사용자가 확인한 뒤 root와 Git common-dir stable identity를 저장한다. app/control/worktree path는 nearest existing ancestor를 검증한 뒤 한 component씩 생성하고 매 단계 final identity를 재검증한다. canonical 문자열은 표시 보조 정보일 뿐 mutation identity가 아니다. UNC, mapped·device network path, removable·RAM·CD-ROM·unknown drive, Cloud Files sync root·placeholder, linked worktree, separate git-dir, bare repository와 확인 불가능한 reparse root/common-dir를 거부한다.
- canonical root path는 repository와 Git adapter에만 전달한다. 일반 IPC는 사용자 홈을 `%USERPROFILE%`로 치환한 display path를 사용하고 오류·로그에는 원본 경로를 넣지 않는다.
- Application은 `register_project`, `approve_git_initialization`, `create_task_worktree`처럼 목적별 use case만 노출한다. generic task transition은 mutation IPC가 아니다.
- `GitService`는 raw process가 아닌 repository probe, status, init, initial snapshot과 branch/worktree 생성·검증 의미만 제공한다. 자동 보상·삭제 의미는 port에 노출하지 않는다. Adapter는 stable identity가 검증된 absolute executable과 argument 배열을 전달하고 `env_clear` 기반 최소 환경을 사용하며 shell 및 remote Git operation을 허용하지 않는다.
- task isolation 시작은 clean tracked/untracked status, attached current branch와 기존 HEAD commit을 모두 요구한다. Dirty, detached, unborn 상태는 안전한 원인·해결 안내와 함께 차단한다.
- 비-Git 프로젝트는 project·task·expected version·project identity revision에 묶인 일회용 승인 전에는 mutation하지 않는다. 승인 후에도 Git author가 없으면 initial snapshot을 만들지 않는다. `git add` 전 working tree attributes와 stage 후 index attributes에 active filter가 없어야 하며, snapshot receipt OID와 최종 clean HEAD가 정확히 일치할 때만 `GitInitialized`를 확정한다.
- 기존 Git 프로젝트는 base commit의 attributes 계층과 `$GIT_DIR/info/attributes`를 검사한다. system/global attributes를 차단하고 active filter·Git LFS를 지원하지 않는다. info attributes identity 또는 내용이 preflight 후 변경되면 완료하지 않는다.
- SQLite와 Git 효과는 원자적이지 않으므로 immutable operation intent와 command-start evidence를 먼저 commit한다. 외부 단계가 성공할 때마다 durable receipt를 추가하고 stable root/common-dir/worktree identity, 원본 checkout branch·HEAD, task branch·base commit과 clean 상태를 재검증한다. `CompletionRecorded` receipt, task transition과 isolation summary는 하나의 SQLite transaction으로 commit한다.
- Git command의 non-zero, receipt 저장 실패, 부분 성공, 경쟁 선점 또는 사후 검증 실패에서는 자동 삭제·재실행·소유권 추정을 하지 않고 `RecoveryRequired`로 전이한다. `worktree remove`, `--force`, `branch -D`와 자동 ref 삭제는 Phase 2 실행 경로에 없다.
- Startup reconciliation은 in-progress operation과 receipt를 읽기 전용으로 검사한다. 정확한 성공 receipt와 실제 상태가 모두 일치할 때만 완료 transaction을 재구성하고 그 밖에는 `RecoveryRequired`로 전이한다. lease는 유지하며 lease/task 불일치는 corruption으로 fail-closed한다.
- 0001 → 0002 migration은 legacy project root의 stable identity preflight를 SQL 적용 전에 수행한다. missing root, 지원하지 않는 storage, identity 불명확 또는 stable-ID 중복이면 전체 migration을 중단하며 자동 병합·삭제하지 않는다.
- Phase 2에는 cleanup operation이 없다. 보존 기간과 완료 작업의 일반 수명주기 정리는 Phase 6 책임이다.
