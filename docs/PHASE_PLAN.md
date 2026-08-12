# ChatOMS Phase 계획

## 범위 원칙과 의존관계

- 여러 프로젝트를 등록할 수 있지만 전체 앱에서 활성 작업은 최대 하나다.
- 한 작업은 하나의 프로젝트, 정확히 하나의 task branch와 최대 하나의 worktree만 사용한다.
- 각 Phase는 이전 Phase의 완료 조건을 통과한 뒤 시작한다.
- 후속 기능에 필요한 인터페이스는 정의할 수 있지만 동작을 선제 구현하지 않는다.

```text
Phase 1 기반
  ├─ Phase 2 Git 격리
  └─ Phase 3 프로세스·공급자
       Phase 2 + Phase 3
              ↓
         Phase 4 하네스
              ↓
         Phase 5 승인·병합
              ↓
         Phase 6 기록·복구
              ↓
         Phase 7 업데이트
```

## Phase 1 — 안전한 기반

**상태:** 완료 (기반 이력)

**선행조건:** 부트스트랩 문서 승인, 초기 프로젝트 생성과 신규 패키지에 대한 사전 보고.

**목적:** 이후 기능이 의존할 앱 셸, 계층 경계, 데이터 불변조건과 저장 전 보안 기반을 만든다.

**포함 범위:**

- Tauri 2 + React + TypeScript strict 앱 셸
- 기본 라우팅, 시스템 상태, 프로젝트 빈 화면, 공통 오류 표시
- domain/application/ports/adapters/infrastructure 모듈 경계
- 타입이 정의된 Tauri IPC
- SQLite migration과 repository transaction 경계
- `Project`, `AppProfile`, `ProviderBinding`, `Task`, `ActiveTaskLease` 기반 모델
- 여러 프로젝트, `Task.project_id` 불변, 앱 전체 단일 `ActiveTaskLease`, 작업별 정확히 하나의 task branch identity·최대 하나의 worktree 데이터 제약. Phase 1에서는 관계와 논리 식별자만 다루며 실제 Git branch/worktree는 생성하지 않는다.
- 앱 데이터 경로, 구조화 로깅과 저장 전 민감정보 마스킹
- Windows 구현 기반과 macOS 플랫폼 인터페이스
- Provider/Git/Process/Update port의 인터페이스

**제외 범위:**

- 프로젝트 등록 동작과 모든 실제 Git 명령
- branch/worktree 생성과 병합
- Claude/Codex 탐지·인증·실행
- 외부 네트워크 요청과 실제 업데이트 동작
- 하네스 진행, 승인·결과·복구 화면

**완료 조건:**

- Windows에서 앱 셸과 debug build가 실행된다.
- Windows build gate는 로컬 Tauri wrapper를 사용한 debug `--no-bundle` build이며 installer 생성 성공은 Phase 1 완료 조건이 아니다.
- MSI·NSIS 생성, WiX 공급망, installer 권한과 installer icon 검증은 배포·업데이트 단계로 연기한다.
- 시스템 상태와 프로젝트 빈 화면만 제공되며 범위 밖 화면은 구현되지 않는다.
- SQLite migration 재실행과 순서 검증이 통과한다.
- 단일 `ActiveTaskLease`, 불변 project 연결, 단일 task branch와 최대 단일 worktree 제약 테스트가 통과한다.
- 사용자 오류와 내부 오류가 분리되고 마스킹 테스트가 통과한다.
- Rust format/clippy/test, TypeScript typecheck와 프런트 테스트가 통과한다.
- 실행 경로에 Git, CLI, 네트워크와 업데이트 구현이 없다.

**현재 구현 상태:**

- Secure storage, database migration, structured logging과 system clock production adapter가 Tauri composition root에 연결됐다.
- Ready/Unavailable managed state와 system/project/task 최소 read-only Tauri IPC가 구현됐다.
- Typed frontend IPC client와 `/system`, `/projects` read-only 화면이 구현됐다. `/system`은 healthy·degraded·unavailable·partial error 상태를, `/projects`는 loading·empty·list·safe error 상태를 제공한다.
- Frontend typecheck, 17개 component/page/router/IPC client 테스트와 Vite build, workspace fmt/test/clippy/check를 완료했다.
- Tauri debug `--no-bundle -- --offline --locked -j 1` build와 executable metadata·PE·icon 검증을 완료했다. MSI·NSIS installer와 bundle 검증은 배포·업데이트 단계로 유지한다.
- Phase 1 구현과 로컬 커밋 정리는 완료됐으며, branch integration은 별도 승인 후 수행한다.

## Phase 2 — 프로젝트와 Git 격리

**상태:** 완료 (Phase 3 진입 전 감사·정리 대상)

**선행조건:** Phase 1 완료.

**목적:** 프로젝트 등록부터 작업별 branch/worktree 격리까지 안전하게 제공한다.

**포함 범위:** 프로젝트 등록·stable filesystem identity 검증, Git 감지, 비-Git 폴더 초기화 승인, 기준 commit 기록, 작업 branch/worktree 생성, 프로젝트·task isolation 상태 조회, controlled Git process, durable operation evidence, 부분 성공 검증·startup reconciliation과 `RecoveryRequired` 기록.

**안전 제한:** Windows production Git runtime은 HKLM installer candidate를 final path, fixed-drive identity, pinned Authenticode signer와 distribution boundary로 검증하며 PATH 탐색·runtime fallback을 하지 않는다. 프로젝트 Git probe와 app/control/worktree directory 생성 전에는 storage trust gate를 통과해야 한다. Network·Cloud Files·linked worktree·separate git-dir·bare repository·active external filter·local `filter.*`/`include.*`/`includeIf.*`와 Git LFS를 거부한다. 정적 repository filter execution은 차단하지만 동일 사용자 권한의 동시 악성 변조는 accepted residual risk다. Git 실패나 불명확한 결과에서 branch/worktree를 자동 삭제·재실행하지 않으며 `RecoveryRequired` task의 lease를 유지한다.

**제외 범위:** Claude/Codex 실행, 하네스 자동화, 기본 브랜치 병합, 원격 Git 쓰기, 보존 기간 기반 정리, 완료 작업의 일반 branch/worktree 수명주기 정리와 사용자 기존 Git 리소스 삭제.

**완료 조건:** 승인 없는 `git init`이 차단되고, clean 저장소에서 작업별 worktree가 생성되며 원본 작업 디렉터리에 AI 변경이 발생하지 않는다.

## Phase 3 — 프로세스와 공급자 연동

**상태:** 다음 승인된 구현 범위

**선행조건:** Phase 1 완료. 통합 검증에는 Phase 2의 worktree가 필요하다.

**목적:** 공식 CLI를 검증된 정책과 구조화 프로토콜로 실행한다.

**포함 범위:** 기본 profile에 귀속된 Claude 실행파일 절대경로 지정·저장과 최소 설정 UI, 실행 직전 Claude executable trust 재검증, CLI 탐지·버전·필수 flag/tool allowlist 지원 여부·로그인 상태에 대한 정적 capability probe, 구조화 `ProcessRunner`의 one-shot 실행 계약. Codex는 trust 근거가 별도 문서 근거로 확인되고 구현계획이 승인되기 전까지 capability를 `Unsupported`로 fail-closed 보고한다. 프로필별 `CODEX_HOME` binding, `ProcessRunner` stdout/stderr 스트리밍·중단·종료, Codex app-server JSON Schema 또는 TypeScript schema와 capability 검증, Claude 설계 12 turns·리뷰 8 turns 실측 강제는 실제 provider 실행 오케스트레이션에 속하므로 Phase 4로 이관한다.

**제외 범위:** 전체 하네스 상태 진행, 자동 수정, 사용자 병합, profile 삭제·이름 변경·다중 profile 생성·관리, 일반 settings UI, Codex 설정 UI, 실제 설계·구현·리뷰 세션 실행, Codex app-server 세션과 `CODEX_HOME` 연동, provider 실행 이벤트 스트리밍·중단 처리.

**완료 조건:** Claude 실행파일 trust 재검증과 capability probe(버전, 필수 flag/tool allowlist 지원 여부, 로그인 상태)가 지원 버전에서 fail-closed로 동작하고, 기본 profile의 Claude executable binding과 최소 설정 UI로 실행파일 경로를 지정·저장·새로고침할 수 있다. Claude 프로필 분리의 공식 지원을 확인하지 못하면 인증 파일 복사나 세션 조작 없이 장비별 단일 로그인을 사용한다. Codex executable trust 근거가 확인되기 전까지 Codex capability는 항상 `Unsupported`로 보고되며 `exec`로 자동 fallback하지 않는다. 실제 설계·리뷰 세션 실행, Claude turn 상한(설계 12/리뷰 8) 실측 강제, 프로필별 `CODEX_HOME`을 사용한 Codex app-server 세션과 호환성 실패·중단·오류의 공통 provider event 변환은 Phase 4에서 provider 선택과 실제 실행 시작 흐름 안에서 구현한다.

## Phase 4 — 하네스 워크플로

**선행조건:** Phase 2와 Phase 3 완료.

**목적:** 설계 → 구현·테스트 → 리뷰의 순차 워크플로를 완성한다. 각 작업 종류의 provider는 사용자가 실행 시 eligible provider 중에서 선택한다.

**포함 범위:** 상태 머신, Context Package revision, 설계 단계, 고위험 판별, 구현 단계, 검증 명령, 최대 2회 테스트 수정, 리뷰 단계, 최대 1회 중대 문제 재수정, 작업 종류·단계별 모델 추천, 추천 provider·모델·이유·예상 비용 등급 또는 호출 규모 표시, 사용자 override와 프로젝트별 모델 고정. provider 선택과 실제 실행 시작 흐름 안에서 실제 설계·리뷰 세션 실행과 Claude turn 상한(설계 12/리뷰 8) 실측 강제, 호환성 실패·중단·오류를 안전한 공통 provider event로 변환하는 처리, `ProcessRunner` stdout/stderr 스트리밍·중단·종료를 구현한다. 실제 Codex 활성화는 별도 trust 근거와 구현계획 승인 후에만 가능하며, 승인 이후 프로필별 `CODEX_HOME`을 사용한 Codex app-server 세션을 이 provider 실행 오케스트레이션에 포함한다.

**제외 범위:** 기본 브랜치 병합, 보관 기간 자동 정리, 업데이트.

**완료 조건:** 작업이 재시도 상한과 자동 수정 금지 조건을 지키며 `AwaitingUserDiffApproval` 또는 근거가 기록된 중단 상태에 도달한다. 모델 추천은 근거와 예상 비용 등급 또는 호출 규모를 표시하고 사용자 override와 프로젝트 고정을 우선하며, 승인 없이 더 비싼 모델로 자동 상향하지 않는다. Claude 설계·리뷰 세션은 turn 상한을 실측 강제하고, provider 호환성 실패·중단·오류는 공통 provider event로 안전하게 변환되며, Codex app-server 세션은 trust 승인 전까지 활성화되지 않는다.

## Phase 5 — 승인·검토·병합

**선행조건:** Phase 4 완료.

**목적:** 고위험 작업 승인, 최종 diff 검토와 안전한 로컬 병합을 제공한다.

**포함 범위:** 중앙 Policy Engine, 승인 화면, 결과 화면, 대상 프로젝트 schema·migration·데이터 변환 승인, 단일 작업 commit, `--no-ff` 병합, 보수적 충돌 분류와 병합 후 테스트. 자동 충돌 해결은 결정적으로 재생성 가능한 파일, 순수 formatting 차이와 명백한 import 정리에만 허용한다.

**제외 범위:** push, PR·이슈·릴리스, 배포, 프로그램 로직, 설정, DB schema·migration, 보안 정책, 의존성, 삭제 대 수정 또는 요구사항 의미 변경이 얽힌 충돌의 자동 해결.

**완료 조건:** 승인 없는 고위험 동작과 병합이 차단되고, 기준 브랜치 변경·미커밋 상태·위험 충돌에서 실패 폐쇄하며 병합 결과와 검증이 기록된다.

## Phase 6 — 기록·복구·보존

**선행조건:** Phase 2~5 완료.

**목적:** 감사 가능성, 비정상 종료 복구와 로컬 데이터 수명주기를 완성한다.

**포함 범위:** 전체 감사 기록, artifact 관리, 체크포인트, 복구 화면, `RecoveryRequired`, `UnknownExternalEffect`, 90일 작업 기록 정리, 병합 worktree 7일 보존·정리.

**제외 범위:** 장비 간 동기화, 원격 백업, 자동 업데이트.

**완료 조건:** 다음 조건을 모두 충족한다.

- 작업 기록은 완료·중단 시점부터 90일 보관한다. 진행 중 작업, 미병합 worktree, 사용자 보존 표시, 해결되지 않은 `RecoveryRequired`·`UnknownExternalEffect` 작업은 자동 삭제에서 제외한다.
- 삭제 시 작업에 연결된 로그, diff와 첨부파일도 함께 정리하고 고아 파일을 탐지·정리하되, 삭제 감사 기록에는 삭제 결과만 남긴다.
- 보존 정리는 앱 시작 시와 하루 한 번 실행한다.
- 병합된 worktree와 task branch는 최소 7일 보존하고, post-merge build·regression 성공, clean status, 사용자 보존 표시 없음과 미해결 외부 효과 없음이 모두 확인된 뒤에만 삭제한다.
- 비정상 종료 시 provider session 참조, task state, branch, worktree, 미커밋 변경과 checkpoint를 보존한다.
- 재시작 시 읽기 전용 진단만 자동 수행하며 명령이나 외부 요청을 자동 재실행하지 않는다.

## Phase 7 — 업데이트·롤백

**선행조건:** Phase 6 완료. 배포 채널, 릴리스 저장소, 서명키 보관 주체와 플랫폼별 롤백 방식에 대한 별도 승인.

**목적:** 서명된 앱 업데이트의 확인, 설치, 건강 검사와 실패 복구를 제공한다.

**포함 범위:** 승인된 배포 채널 조회, 다운로드 승인, 서명·해시 검증, 설정·DB·실행 파일 백업, 설치, 건강 검사와 롤백.

**제외 범위:** 확정되지 않은 배포 채널의 선제 구현, 진행 중 작업에서의 강제 업데이트, 회사 배포 정책 우회.

**완료 조건:** 서명·백업·상태 조건 실패 시 설치가 차단되고 Windows/macOS에서 실패한 업데이트가 검증된 이전 상태로 복구된다.
