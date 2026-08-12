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
