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

- Phase 1과 Phase 2는 완료되었고, Phase 3이 다음 승인된 구현 범위다.
- Phase 4 이상 기능을 선제 구현하지 않는다.
- Phase 2의 목적별 local Git 격리는 유지한다. remote Git, 기본 브랜치 mutation, branch/worktree 삭제·정리와 병합은 여전히 구현 범위 밖이다.
- Provider 실행, ProcessRunner, provider session과 Claude/Codex 연동은 Phase 3 범위이며, 별도 요구사항 승인 없이는 구현하지 않는다.

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
