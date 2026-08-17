# ChatOMS 보안 정책

이 문서는 제품 요구사항의 보안·승인 규칙을 구현 가능한 정책 경계로 요약한다. 세부 요구사항은 [PRODUCT_REQUIREMENTS.md](PRODUCT_REQUIREMENTS.md), 상태별 실행 가능 여부는 [STATE_MACHINE.md](STATE_MACHINE.md)를 따른다.

## 기본 원칙

- 최소 권한, 기본 거부와 명시적 승인을 적용한다.
- 원본 프로젝트는 병합 승인 구간을 제외하고 읽기 전용으로 취급한다.
- AI 출력은 비신뢰 입력으로 취급한다.
- 프로그램과 인수 배열을 분리한 구조화 실행을 기본으로 하며 AI가 생성한 셸 문자열을 그대로 실행하지 않는다.
- 회사 장비에서는 조직의 보안, 프록시, 인증서, 패키지 저장소와 배포 정책이 앱 정책보다 우선한다.

## 인증정보 비보유

앱은 OAuth 토큰, API 키, 비밀번호, 쿠키, Authorization 헤더, 개인키 또는 Claude/Codex CLI 인증정보를 직접 읽거나 저장하지 않는다.

- 인증과 계정 전환은 공식 CLI 흐름에 위임한다.
- Codex 프로필은 공식 `CODEX_HOME` 경로 참조로 분리한다.
- `CLAUDE_CONFIG_DIR`은 설치 버전과 공식 문서에서 지원이 확인된 경우에만 사용하며, 확인 실패 시 장비별 단일 Claude 로그인으로 제한한다.
- 인증 파일 복사, 세션 조작, 브라우저 쿠키 또는 OS 자격증명 저장소 접근으로 계정 분리를 우회하지 않는다.

## 민감정보 마스킹

다음 값은 UI 표시뿐 아니라 로그, SQLite 필드와 artifact의 영구 저장 전에 마스킹한다.

- OAuth 토큰, API 키, 비밀번호와 쿠키
- Authorization 및 유사 인증 헤더
- `.env` 비밀값
- SSH·인증서 개인키
- Windows Credential Manager 및 macOS Keychain 반환값
- Claude/Codex 세션·인증 정보

원문은 가능한 한 메모리에 오래 유지하지 않으며 마스킹 실패가 감지되면 저장을 차단한다.

## 명령 분류

| 분류 | 대표 작업 | 정책 |
|---|---|---|
| 자동 허용 | 프로젝트/worktree 읽기, 코드 검색, Git 상태·diff 조회, worktree 내부 일반 수정, formatter/linter, 기존 테스트·빌드, 읽기 전용 분석 | 허용 범위·경로·현재 상태를 검증한 뒤 실행 |
| 사용자 승인 | 프로젝트 밖 변경, 대량·중요 파일 삭제, 관리자 권한, 시스템·영구 환경 설정, 새 패키지·버전·lockfile 변경, 설치 스크립트, 대상 프로젝트 schema·migration·데이터 변환, 외부 쓰기·업로드, push, 원격 변경, 기본 브랜치 쓰기, 기록 파괴 Git 명령 | 목적, 실제 명령, 대상, 부작용과 복구 가능성을 표시한 뒤 승인된 범위만 실행 |
| 항상 차단 | 인증정보 추출·출력, 쿠키·비밀번호 수집, 보안·계정 한도 우회, 승인 없는 회사 데이터 전송, 개인키 원문 노출, 악성 코드, AI에 의한 승인 정책 비활성화 | 승인을 요청하지 않고 차단·기록 |

Phase 1에서는 이 분류를 실제 Git, CLI 또는 네트워크 실행에 사용하지 않으며 관련 port와 정책 타입만 준비한다.

Phase 2에서는 의미 기반 Git allowlist만 실제 실행한다. 허용 대상은 repository/status/HEAD 조회, 승인된 local `git init`과 initial snapshot, task branch/worktree 생성·검증뿐이다. Shell 문자열, fetch, pull, push, remote 변경, 기본 브랜치 쓰기와 branch/worktree 삭제는 adapter 계약에서 제외한다.

Git 초기화 승인은 project ID, task ID와 expected task version에 결합하며 승인 전에는 `.git`, 파일 또는 commit mutation을 수행하지 않는다. `.gitignore`와 Git author 설정을 자동 생성·변경하지 않는다.

## 네트워크 정책

### 읽기 허용 목록

다음 범주의 승인된 도메인만 자동 읽기 후보가 된다.

- 공식 언어·프레임워크 문서
- 공식 패키지 저장소와 메타데이터
- GitHub 공개 저장소 읽기
- OpenAI 및 Anthropic 공식 문서
- 프로젝트에 등록된 사내 문서
- 사용자가 명시적으로 허용한 도메인

허용 목록 밖의 목적지와 목록 밖으로 이동하는 redirect는 중단하고 승인을 요청한다.

### 네트워크 쓰기

모든 네트워크 쓰기는 사용자 승인이 필요하다. 승인 화면에는 목적지, 방식, 목적, 전송 데이터 요약, 민감정보 가능성과 외부 부작용을 표시한다. 읽기로 시작한 요청이 쓰기로 바뀌면 새 승인이 필요하다.

### 공급자 전송

Claude/Codex에 Context Package와 코드 문맥을 보내는 행위도 외부 전송으로 간주한다. 작업 시작 전에 공급자, 전송 목적과 데이터 범위를 한 번 승인하고 그 작업의 설계·구현·수정·리뷰 호출에만 재사용한다. 공급자나 데이터 범위가 바뀌면 다시 승인한다.

Review 동의는 `Testing → Reviewing` 자동 전이 이후, 이미 `Reviewing` 상태인 task에서 기록·재사용된다. Planning/Implementation 동의와 달리 이 동의 자체는 어떤 상태 전이도 동반하지 않는다.

Review 결과 저장은 마스킹·크기 제한된 최종 review text와 outcome/exit code/turn count 같은 content-free metadata만 허용한다. Raw stdout/stderr, raw Git diff, transcript, tool I/O, prompt 원문, executable·environment 경로, login·session·cost 정보는 어떤 컬럼에도 저장하지 않는다.

**Review용 Git diff 읽기 경계 (Unit 4e-4a):** Claude Review가 참조할 task worktree의 현재 Git diff(staged+unstaged 변경 전체, worktree 자신의 HEAD 기준)는 향후 Review adapter의 stdin으로만 일시 전달되는 ephemeral 데이터다. `chatoms_ports::diff::WorktreeDiffPort`가 이를 읽고, `chatoms-application::review_diff::ReviewDiffReader`가 spawn 전에 기존 `GitService::verify_task_worktree`와 `FilesystemIdentityPort`로 worktree identity를 재검증한다. 읽기는 기존 trusted Git runtime(`GitCliAdapter`)만 사용하고 고정 argv(`diff --no-color --no-ext-diff --no-textconv HEAD -- .`)로 external diff driver·textconv·pager 실행을 차단하며, 호출자로부터 revision이나 path를 받지 않는다. 결과는 명시적 byte 상한(512KiB)과 wall-clock timeout(20초)으로 bound되고, 상한 초과·timeout·미확인 종료는 raw 내용 없는 안전한 outcome(`DiffTooLarge`/`TimedOut`/`Uncertain`)으로, Git 실행 실패와 non-UTF-8 출력은 raw 내용 없는 typed 오류로 분류한다. 이 diff는 SQLite, DTO, IPC, UI, 로그 어디에도 저장·노출하지 않는다.

**Claude Review adapter 입력·출력 경계 (Unit 4e-4b):** `chatoms-infrastructure::claude_review::ClaudeReviewAdapter`는 Unit 4e-4a가 읽은 ephemeral diff와 TaskBrief 세 필드만 stdin 고정 템플릿에 싣는다(저장된 Claude Planning 결과는 이번 첫 입력에서 제외). Diff는 stdin 섹션 헤더와 argv 고정 지시문 양쪽에서 "untrusted repository content"로 명시하고 그 안에 포함된 어떤 지시도 따르지 말라고 명시한다 — AI 출력뿐 아니라 리뷰 대상 repository 콘텐츠 자체도 비신뢰 입력으로 취급한다. argv에는 사용자 입력·TaskBrief 원문·diff 원문이 전혀 없으며, CWD는 app-owned trusted preflight directory이고 worktree는 `--add-dir` 읽기 전용 인자로만 전달된다(`--tools Read,Glob,Grep` + `--permission-mode plan`으로 Edit/Write/Bash가 세션에 아예 존재하지 않음). Claude Implementation이 확정한 hardening(`--strict-mcp-config`, `--setting-sources project,local`, `--disable-slash-commands`)을 그대로 적용한다. stdin 총 크기는 spawn 전 1MiB 상한을 넘으면 truncation 없이 typed `StdinTooLarge`로 fail-closed하고, spawn 직전마다 `ProviderCapabilityPort`를 재검증한다. stdout은 adapter 내부 bounded buffer에서만 처리되고 `--output-format json` envelope의 `subtype`/`is_error`/`result`/`num_turns`만 파싱하며, session id·cost·기타 필드는 파싱 대상이 아니다. 성공 text는 `SecretRedactor`를 거쳐 마스킹·크기 제한한 뒤에만 반환하고, nonzero exit·malformed JSON·stdout bound 초과는 `Failed`, 확인된 취소는 `Cancelled`, 미확인 종료는 `RecoveryRequired`로 분류한다. 이 Unit은 `task_review_results` 저장·상태 전이와 Tauri IPC/UI를 연결하지 않는다.

**Claude Review 실행 orchestration의 diff-검증-우선 순서 (Unit 4e-5):** `chatoms-application::review_execution::ReviewExecutionStarter::begin`은 Unit 4e-4a의 diff 읽기(worktree identity 재검증 포함)가 비어 있지 않은 in-bound 결과(`WorktreeDiffOutcome::Diff`)를 반환할 때만 Review 동의를 기록·재사용한다. Diff가 `NoChanges`/`DiffTooLarge`/`TimedOut`/`Uncertain`이거나 identity mismatch·Git 실행 실패로 오류를 반환하면, Claude 프로세스는 spawn되지 않고 Review 동의도 기록되지 않으며 task의 상태·버전·history·lease는 그대로 유지된다.

**Claude Review 결과 표시 경계 (Unit 4e-6):** `get_review_result` Tauri command와 ProjectsPage UI는 task가 현재 `AwaitingUserDiffApproval`일 때만 이미 저장된 masked `review_text`를 read-only로 표시한다. 그 외 상태와 결과 미기록은 동일하게 빈 상태로 취급하고, raw Git diff·raw stdout/stderr·provider executable 경로·session/login/cost 정보는 이 표면의 어떤 경로로도 반환·표시하지 않는다. `AwaitingUserDiffApproval`은 Phase 4의 현재 종료 지점이며, 최종 diff 승인 control, `approve_final_diff`류 command·use case, `AwaitingUserDiffApproval → Completed` 전이는 코드에도 이 문서에도 존재하지 않는다 — `docs/STATE_MACHINE.md`가 정의하는 `AwaitingUserDiffApproval`의 다음 상태는 `Merging`(기존 상태 머신의 vocabulary이며 별도 승인 없이는 구현·확장하지 않음), `Paused`, `Cancelled`뿐이다.

## 패키지 설치 정책

다음 조건을 모두 만족하는 복원 설치만 자동 허용 후보다.

- 기존 manifest와 lockfile에 선언·고정됨
- lockfile을 변경하지 않음
- 프로젝트/worktree 전용이며 전역 설치가 아님
- 설치 스크립트를 실행하지 않거나 별도 승인을 받음
- 회사 장비 정책을 준수함

새 패키지, 삭제·버전·출처 변경, lockfile 생성·재생성, 설치 스크립트, 전역 설치, 시스템 도구 체인과 환경 변경은 사전 승인이 필요하다.

## 데이터베이스 migration 정책

[제품 요구사항의 승인 정책](PRODUCT_REQUIREMENTS.md#9-설계-승인-정책)을 적용할 때 ChatOMS 자체 SQLite migration과 사용자가 등록한 대상 프로젝트의 migration을 구분한다.

### ChatOMS 내부 SQLite migration

- 앱 배포물에 포함되고 checksum이 검증된 migration만 실행한다.
- 앱 시작 시 필요한 forward migration을 자동 실행할 수 있다.
- 적용 순서와 checksum을 검증하고 각 migration은 하나의 transaction으로 실행한다.
- 이미 적용된 migration의 내용 변경, 중복 또는 역순 적용을 거부한다.
- 실패하면 앱 초기화를 중단하고 비밀정보와 내부 원문을 제외한 안전한 오류를 표시한다.
- 파괴적 migration은 사전 backup 또는 별도로 승인된 업데이트 정책 없이 자동 실행하지 않는다.

### 대상 프로젝트 migration

대상 프로젝트의 schema 파일 생성·변경, migration 생성·실행과 데이터 변환은 모두 사용자 별도 승인이 필요하다. ChatOMS 내부 migration의 자동 실행 정책을 대상 프로젝트에 적용하지 않는다.

## Phase 2 local Git mutation 경계

- Windows `DRIVE_FIXED` local directory만 지원한다. 프로젝트 입력은 첫 Git 호출 전에, app/control/worktree 경로는 첫 directory 생성 전에 nearest existing ancestor trust gate를 통과해야 한다. UNC, mapped·device network path, removable·RAM·CD-ROM·unknown drive, Cloud Files sync root·offline·recall·placeholder content와 identity를 확인할 수 없는 reparse root를 거부하며, deterministic 거부에서는 관리 directory를 만들지 않는다. 검증 뒤 ancestor replacement가 관찰되면 생성·Git 실행·완료를 중단하고 이미 생성된 앱 경로도 자동 삭제하지 않는다.
- canonical path 문자열만 신뢰하지 않고 volume serial, directory file ID, repository kind와 Git common-dir identity를 저장·재검증한다. Mutation 중에는 directory handle guard로 일반 rename/rebind 경쟁을 차단한다.
- Git executable은 부모 PATH에서 찾지 않는다. HKLM Git for Windows installer candidate를 final path, fixed drive, stable identity, pinned Authenticode signer와 runtime root·`cmd`/`mingw64/bin`·`libexec/git-core` boundary로 검증한다. 64-bit registry record가 있으면 그 record의 candidate 검증 실패를 32-bit record로 fallback하지 않으며, 64-bit record가 없을 때만 32-bit record를 검사한다. 서로 다른 두 valid root도 ambiguity로 거부한다. 실행 중 runtime/certificate 변경은 다른 후보로 fallback하지 않고 fail-closed다. 모든 명령은 `env_clear` 후 app-owned current directory, controlled PATH, controlled absolute `GIT_EXEC_PATH`, absolute `git -C <verified-root>`에서 실행한다.
- Active filter attribute와 Git LFS, repository local `filter.*`/`include.*`/`includeIf.*`, linked worktree, separate git-dir와 bare repository는 Phase 2에서 지원하지 않는다. 정적 repository와 preflight 시점의 effective attributes/config가 filter process를 실행하지 못하게 차단한다. 같은 사용자 권한의 악성 동시 프로세스가 최종 검증 직후 attributes/config를 교체하는 공격은 Phase 2 threat model 밖의 accepted residual risk이며, 사후 불일치가 관찰되면 완료 대신 `RecoveryRequired`로 전이한다.
- Git 실패·부분 성공·사후 검증 실패에서는 `worktree remove`, `--force`, `branch -D`, 자동 ref 삭제, 소유권 추정과 자동 재실행을 하지 않는다. Durable evidence와 `RecoveryRequired`를 기록하고 lease를 유지한다.
- Git capability는 asInvoker process에서만 제공한다. Windows token이 elevated, full-elevation 또는 high-integrity이면 ACL 판정을 완화하지 않고 unavailable로 처리한다. linked limited token을 자동 사용하거나 self-de-elevation하지 않으며, 사용자는 관리자 권한 없이 다시 실행해야 한다.
- Startup reconciliation은 읽기 전용 Git 진단과 DB 상태 확정만 수행한다. 정확한 성공 receipt가 없는 외부 효과를 성공으로 추정하지 않는다.

## Phase 3 provider(Claude/Codex) 실행파일 신뢰 경계

- Claude와 Codex 실행파일은 프로필별로 사용자가 지정한 절대경로만 사용한다. PATH 탐색과 고정 후보 경로의 자동 스캔은 수행하지 않는다.
- Claude 실행파일은 실행 직전마다 Windows Authenticode signer가 `Anthropic, PBC`인지 재검증한다. 서명을 확인할 수 없거나 검증에 실패하면 해당 provider capability를 `Unavailable`로 fail-closed 처리한다.
- Codex 실행파일의 서명 또는 동등한 신뢰 근거가 공식 문서로 확인되고 별도 구현계획이 사용자 승인을 받기 전까지 Codex capability는 항상 `Unsupported`로 보고된다. 사용자가 실행파일 경로를 직접 지정했더라도 이 조건에 예외를 두지 않으며, `codex app-server` 연결 실패 시 `codex exec`로 자동 전환하지 않는다.
- Claude와 Codex의 로그인 상태 확인 명령이 표준출력·표준오류로 반환하는 내용은 비신뢰 데이터로 취급한다. 로그인 여부 판정에는 검증된 종료 코드만 사용하며, 표준출력·표준오류 원문과 계정 이메일, 조직명, plan, 로그인 만료 시각, API key 일부 문자열은 UI, 로그, SQLite, artifact 어디에도 표시하거나 저장하지 않는다.
- Claude/Codex local preflight 및 향후 provider 실행은 project root, task worktree, 상속된 process current directory를 working directory로 쓰지 않는다.
- 검증·준비된 app-owned `temp` 하위 directory만 사용한다.
- 실행 직전 non-reparse, secure, stable identity를 재검증한다.
- 재검증 실패·불일치면 실행하지 않고 capability를 `Unavailable`로 fail-closed 처리한다.
- 이 directory는 profile·task와 결합하지 않는 시스템 수준 preflight 경계다.

## Cargo-only validation 실행 경계 (Unit 4d-2a)

- 검증 명령 실행은 승인된 Cargo 고정 서브커맨드(`fmt --all --check`/`clippy --workspace --all-targets --all-features`/`test --workspace`/`build --workspace`)로 제한한다. PATH 탐색은 하지 않으며 승인된 절대경로만 spawn한다.
- spawn 직전마다 승인된 executable·tool directory와(승인된 경우) `CARGO_HOME`/`RUSTUP_HOME`의 Windows stable NTFS 식별자를 다시 검증하고, identity 불일치·재검증 실패·reparse point/symlink·worktree 내부 executable은 spawn 없이 fail-closed 거부한다.
- 프로세스 환경은 `env_clear` 후 `PATH`(승인된 tool directory만), 앱 소유 `TEMP`/`TMP`, 현재 프로세스의 `SystemRoot`, 승인·재검증된 `CARGO_HOME`/`RUSTUP_HOME`(있는 경우)만 설정하며 그 외 부모 프로세스 환경 변수는 상속하지 않는다.
- `Format`/`Lint`/`Typecheck`는 10분, `Test`/`Build`는 30분 timeout을 적용하고 stdout은 2MiB로 bound한다. stdout/stderr 원문은 adapter 밖으로 노출·저장하지 않는다.
- **잔여 위험(기술적으로 차단하지 않음):** `cargo test`/`cargo build`가 실제로 컴파일·실행하는 worktree 자신의 Rust 코드(`build.rs`, proc macro, `#[test]` 본문)는 네트워크 접근, 임의 child process 생성과 Git mutation을 실행할 수 있다. 이는 사람이 같은 명령을 직접 실행할 때와 동일한 위험 수준이며 accepted residual risk로 취급한다. package-manager `run <script>` 실행은 이번 Unit의 범위 밖이며 별도 승인 없이는 구현하지 않는다.
- **검증 결과 영구 기록 (Unit 4d-2b):** `task_validation_command_results`는 승인 식별자(`task_id`/`approved_task_version`/`command_kind`)·`attempt_sequence`·outcome·exit code·시작/종료 시각과 마스킹·크기 상한(2000자)이 적용된 `safe_summary`만 저장하는 append-only immutable 테이블이다. Provider, session, transcript, executable path, environment path, 그리고 stdout/stderr 원문은 이 테이블 어떤 컬럼에도 저장하지 않는다. `safe_summary`는 이후 orchestration Unit이 기존 `SecretRedactor`와 자체 크기 상한을 거쳐 만든 안전 요약만 전달해야 하며, 저장 계층은 이를 재파싱·재마스킹하지 않고 그대로 신뢰한다.
- **Testing 검증 배치 완료 조건과 상태 매핑 (Unit 4d-2c):** `chatoms_application::testing_execution`은 현재 task version에 승인된 Cargo 검증 명령을 `Format → Lint → Typecheck → Test → Build` 고정 순서로 실행한다. `safe_summary`는 실제로 raw stdout/stderr를 전혀 전달받지 않으므로 이를 가공한 요약이 아니라, outcome별 고정 문구(success/exit failure/timeout/output bound/cancellation/uncertain)만 사용한다. 모든 승인 명령이 `Success`로 끝나면 마지막 결과와 함께 `Testing → Reviewing`으로 전이하고, `ExitFailure`/`TimedOut`/`StdoutBoundExceeded`/`Uncertain`은 첫 발생 즉시 그 결과를 남기고 `Testing → RecoveryRequired`로 전이하며 이후 승인된 명령은 실행하지 않는다. 확인된 `Cancelled`는 결과를 남기고 `Task::pause()`로 `Testing → Paused`(resume target `Testing`)로 전이한다. 이 마지막 결과 행과 상태 전이·history 기록은 하나의 SQLite transaction이며, 그 이전의 성공 결과들은 상태 변경 없이 append-only로만 기록된다. approval이 없거나 executable/tool directory/`CARGO_HOME`/`RUSTUP_HOME` identity 재검증이 실패하면 해당 명령은 spawn하지 않고 상태를 바꾸지 않는 typed 오류로 반환해, 승인 UI가 추가된 뒤 같은 `Testing` task에서 재시도할 수 있게 한다. 실행기 호출 중 발생한 panic은 payload를 검사·저장하지 않고 `Uncertain` 결과로 처리해 동일한 `RecoveryRequired` 경로로 fail-closed 처리한다.
- **Testing 실행 수명주기의 Tauri backend 배선 (Unit 4d-2d):** Testing batch 시작·취소는 Claude Planning/Implementation과 동일하게 memory-only `TestingRunRegistry`(task id 중복 등록 거부)로만 동시성을 제어하며, 별도 execution/session table을 두지 않는다. 시작 전 `TestingBatchStarter::begin`의 읽기 전용 검증(task state/version, isolation, 승인된 명령 존재)이 실패하면 subprocess·registry 등록·상태 전이 중 어느 것도 일어나지 않는다. 취소 요청은 registry의 cancellation signal에만 전달되고 상태를 직접 바꾸지 않으며, streaming runner가 확인한 결과만 기존 `TestingBatchRecorder`/`finalize_validation_command_batch` 경로(Unit 4d-2c)를 통해 `Paused`/`RecoveryRequired`로 반영된다. 앱 재시작 시 `PlanningRunRegistry`와 동일한 이유로(memory-only registry는 재사용·자동 재실행할 실행 핸들이 없음) leftover `Testing` task는 `TaskService::reconcile_startup_testing`이 기존 `mark_recovery_required` 경로로 `RecoveryRequired`로 되돌리고, 이 reconciliation이 실패하면 앱 부팅 자체를 `ManagedRuntime::unavailable`로 fail-closed한다. Tauri command DTO(`CancelTestingDto`)는 `{requested: bool}` 하나만 노출하며 raw path, stdout/stderr, panic payload, 내부 오류 원인은 어떤 경로로도 전달하지 않는다.
- **Testing 승인 UI의 경로 입력·비노출 경계 (Unit 4d-2e):** `approve_validation_command` IPC는 사용자로부터 Cargo executable 절대경로와 optional `CARGO_HOME`/`RUSTUP_HOME` 절대경로를 입력받지만, 어떤 IPC 응답(candidate 조회, 승인 상태 조회, 승인 성공 응답)도 이 경로나 그 identity를 되돌려주지 않는다 — 승인 성공 응답은 승인된 `kind` 목록만 담는다. 프런트엔드는 입력된 경로를 component state에만 보관하고, 승인 성공·실패·activeTaskId 변경 시 모두 그 값을 지워 화면에 다시 나타나지 않게 한다. 백엔드는 프런트가 보낸 executable 이름이나 argv를 신뢰하지 않고, 선택된 kind마다 그 자리에서 `ValidationCommandDiscovery`를 다시 호출해 얻은 현재 Cargo 후보로 `executable`/`arguments`를 재도출한 뒤 기존 `ValidationCommandService::approve_command`의 exact-candidate 검증에 넘긴다. 이 과정에서 모든 validation command 승인·재검증이 거치는 공유 wrapper `FilesystemIdentityHandle`이 `inspect_supported_file`을 감싼 adapter에 위임하지 않고 트레이트 기본값(`Unsupported`)으로 항상 fail-closed되던 버그를 함께 수정했다 — 수정 전에는 승인이 항상 거부되어 기능이 동작하지 않았을 뿐 안전 경계 자체가 약화된 적은 없다(fail-closed 방향의 버그).
- **Scoped validation approval (Phase 5d-4b):** `ValidationExecutionScope`는 `TaskWorktree`와 `ProjectRoot`만 허용하며 scope가 approval PK와 result FK의 일부다. forward migration은 기존 approval과 Testing result를 모두 `TaskWorktree`로 보존한다. `TaskWorktree` approval은 `ProjectRoot` lookup/request를 충족할 수 없고, merge 전 `ProjectRoot` approval은 diff approval과 독립된 immutable evidence다. `ProjectRoot` approval에는 task의 project id, confirmed project identity revision, root volume serial/file ID만 content-free snapshot으로 저장하며 raw diff나 raw process output은 저장하지 않는다.
- **ProjectRoot validation provenance and revalidation (Phase 5d-4b):** post-merge approval version은 현재 task version과 같다고 가정하지 않는다. starter는 transition history의 연속된 `AwaitingUserDiffApproval -> Merging -> PostMergeTesting` chain에서 source version을 찾고 exact-version `ProjectRoot` approval만 사용한다. 최신 approval fallback과 `current_version - N` 계산은 금지한다. 향후 executor는 spawn 직전에 target variant/scope, live root identity, approval snapshot/current confirmed project identity, executable/tool directory/approved environment directory가 target 밖인지, fixed Cargo argv와 controlled environment/timeout/output bound를 모두 다시 검증해야 한다. 이번 Unit은 ProjectRoot process 실행을 열지 않는다.
- **Post-merge result boundary (Phase 5d-4b):** post-merge validation은 Testing 결과 table/finalize 의미를 재사용하지 않고 별도 immutable result table을 사용한다. raw stdout/stderr, raw diff, executable/environment path, authentication/session 정보는 schema와 application result에 없다. 중간 성공만 append하고 마지막 성공은 result, `Completed`, history, lease release를 한 transaction으로 기록한다. `ExitFailure`, `TimedOut`, `StdoutBoundExceeded`, `BindingRejected`, `Cancelled`, `Uncertain` 및 panic/persistence uncertainty는 result와 `RecoveryRequired`, history를 한 transaction으로 기록하며 lease를 유지한다. primary write 실패는 성공으로 추정하지 않고 가능한 경우 결과 행 없이 `RecoveryRequired`로 fallback하며, fallback 실패도 `Completed`로 축소하지 않는다. `PostMergeTesting -> Failed`는 이번 Phase에서 사용하지 않는다.
- **`Implementing`에 대한 startup reconciliation 누락 수정 (Unit 4e-8a):** `Implementing`은 유일한 쓰기 권한 활성 상태인데도, Planning(`reconcile_startup_planning`)·Testing(`reconcile_startup_testing`)·Reviewing(`reconcile_startup_reviewing`)과 달리 대응하는 startup reconciliation이 없어, 앱이 `Implementing` 도중 재시작되면 memory-only `ImplementationRunRegistry`가 비워지고도 해당 task는 `RecoveryRequired`로 회수되지 못한 채 `ActiveTaskLease`를 쥐고 영구히 멈추는 fail-open 상태였다(Phase 4 마감 감사에서 발견). `TaskService::reconcile_startup_implementation`이 나머지 셋과 동일한 모양으로 활성 lease의 `Implementing` task를 기존 `mark_recovery_required` 경로(non-terminal이므로 lease 유지)로 되돌리고, `compose_runtime`이 이를 `reconcile_startup_planning` 직후·`reconcile_startup_testing` 이전에 호출하며, reconciliation 실패는 `ManagedRuntime::unavailable`로 fail-closed한다.

## ProjectRoot approval merge gate (Phase 5d-4c)

- `approve_user_diff_and_start_merge`는 동일 `(task_id, approved_task_version, ProjectRoot)` scope의 fixed Cargo `Test`와 `Build` approval이 모두 존재하는지 먼저 확인한다. 누락·stale version·state mismatch·approval lookup failure이면 diff approval, `Merging` 전이, Git adapter 준비와 background execution을 시작하지 않는다.
- readiness DTO는 두 boolean만 반환하며 raw diff, stdout/stderr, root/executable/environment path와 identity를 반환하지 않는다. executable identity는 native picker에서 승인 요청에 일회성으로 전달되고 일반 status/result 표면에는 재노출되지 않는다.
- 이번 Unit은 ProjectRoot Cargo process를 실행하지 않는다.

## 앱 데이터 경로와 Windows ACL

- 영구 SQLite, log와 artifact 저장 전에 `SecureAppPaths`의 경로·권한 검증을 통과해야 한다. `Degraded`, `Insecure`, `Unsupported`, `Unknown`은 모두 저장 차단 상태다.
- Windows app root는 `%LOCALAPPDATA%\ChatOMS`이며 current user와 `NT AUTHORITY\SYSTEM`에만 Full Control을 허용한다.
- DACL inheritance를 보호하고 `Everyone`, `BUILTIN\Users`, `Authenticated Users` 및 승인되지 않은 writable principal의 allow ACE를 허용하지 않는다.
- 권한 적용 후 DACL을 다시 읽어 current user·SYSTEM 권한, broad allow 부재와 directory inheritance flag를 검증한다. 적용 또는 검증 실패는 fail-closed 처리한다.
- app root, 필수 하위 directory와 task directory가 symlink 또는 reparse point이면 거부한다. 이 검사는 TOCTOU를 완전히 제거한다고 주장하지 않는다.
- Phase 1의 검증 범위는 local NTFS user profile이다. UNC, network share, redirected profile과 foreign filesystem은 지원·검증됐다고 간주하지 않는다.
- ACL 적용은 관리자 권한, owner 변경, deny ACE 추가, SACL 변경, privilege elevation 또는 외부 명령을 요구하지 않는다.

## 격리 보장 범위

MVP의 격리는 ChatOMS application policy와 공식 CLI가 제공하는 권한·sandbox 기능의 조합이다. 임의 자식 프로세스의 파일·네트워크 동작을 운영체제 전체 수준에서 완전히 격리한다고 보장하지 않는다.

- 지원되지 않는 정책이나 capability가 감지되면 권한을 완화하지 않고 실행을 차단한다.
- OS 수준 강제 격리가 필요한 회사 장비에서는 조직이 제공하는 별도 보안 통제를 우선 적용한다.

## Redaction과 로컬 진단 로그

- Raw secret, raw prompt, CLI 원문 출력과 인증정보를 로그 API에 직접 전달하거나 영구 저장하지 않는다.
- Authorization/Proxy-Authorization, Cookie, API key·token·password·session 계열 필드, private key, JWT·provider token 및 URL credential을 저장 전에 결정적으로 마스킹한다.
- Percent encoding 또는 JSON escape를 해석했을 때 민감정보가 발견되지만 안전한 부분 치환을 보장할 수 없으면 전체 값을 `[REDACTION_FAILED]`로 대체한다.
- `RedactedText`의 내부 값은 private이며 `Display`와 `Debug`는 검증된 redacted 결과만 노출한다. 검증 실패 시 raw logging fallback을 허용하지 않는다.
- 영구 로그는 권한 검증을 통과한 local logs directory에만 ANSI 없는 structured 형식으로 기록한다. Remote telemetry와 네트워크 전송은 Phase 1에서 금지한다.
- 사용자 오류에는 내부 경로, SID, SQL, source chain 또는 secret을 포함하지 않는다. Concrete source chain은 내부 adapter error에서만 유지하고 application 경계를 넘기지 않는다.
