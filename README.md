# ChatOMS

## Development toolchain

Use Node `22.18.0` (also recorded in `.node-version`) and the repository-pinned pnpm through Corepack. Do not rely on an ambient pnpm binary:

```powershell
corepack pnpm --version # 11.9.0
corepack pnpm typecheck
corepack pnpm test:run
corepack pnpm build
```

Tauri uses the same `corepack pnpm build` entrypoint before its Rust build.

ChatOMS는 공식 Claude Code CLI와 공식 Codex CLI를 로컬에서 조율하는 크로스플랫폼 데스크톱 AI 코딩 하네스다. 설계·구현·리뷰의 순차 워크플로를 GUI로 제공하며, 각 작업 종류의 실행 시 사용자가 capability와 실행 계약 조건을 만족하는 eligible provider를 선택한다.

## MVP 범위

- 여러 로컬 프로젝트를 목록에 등록할 수 있다.
- 전체 앱에서 활성 작업은 최대 하나이며, 한 작업은 하나의 프로젝트, 정확히 하나의 task branch와 최대 하나의 worktree만 사용한다.
- 작업 흐름은 설계 → 구현·테스트 → 리뷰 → 사용자 diff 승인 → 기본 브랜치 병합 순서다. 각 작업 종류에서 사용자가 eligible provider를 선택한다.
- 테스트 실패 자동 수정은 최대 2회, 중대 리뷰 문제 재수정은 최대 1회다.
- 로컬 기록, 민감정보 마스킹, 비정상 종료 복구와 보존 정책을 포함한다.
- 병렬 작업, 원격 Git 쓰기, 자동 push/PR/배포, 팀 기능, 클라우드 동기화와 중앙 서버는 제외한다.

상세 범위는 [제품 요구사항](docs/PRODUCT_REQUIREMENTS.md)과 [Phase 계획](docs/PHASE_PLAN.md)을 따른다.

## 기술 스택

- Desktop: Tauri 2
- Frontend: React, TypeScript strict
- Local orchestrator: Rust
- Metadata: SQLite
- Artifacts: 로컬 파일 시스템
- Isolation: Git branch + Git worktree
- Providers: 공식 Claude Code CLI, 공식 Codex CLI
- Platforms: Windows Native 우선, macOS 지원

## 현재 개발 상태

Phase 1~4가 완료되었다. Phase 2는 로컬 프로젝트 등록과 작업별 branch/worktree 격리를, Phase 3은 Claude 실행파일 trust와 capability preflight를, Phase 4는 Claude Planning·Implementation·Review와 승인된 Cargo-only validation의 사용자 시작·취소·복구 경로를 구현했다. Phase 4의 정상 종료 지점은 `AwaitingUserDiffApproval`이며, 이 상태에서는 마스킹된 review 결과만 읽기 전용으로 표시한다.

Git worktree는 `%LOCALAPPDATA%\ChatOMS\worktrees\<project-id>\<task-id>`에만 생성한다. Claude 실행은 사용자 동의와 capability gate를 통과한 뒤에만 시작한다. Codex는 executable trust와 실행 계약이 미승인이라 모든 작업 종류에서 `Unsupported`/`NotApproved`로 차단된다. 원격 Git 쓰기, 기본 브랜치 병합, `AutoFixing`/`ReviewFixing`, Context Package, 고위험 승인, 모델 추천·override·프로젝트별 모델 고정과 보존 정책 기반 정리는 Phase 5 이후 범위다. Windows build gate는 installer를 생성하지 않는 `node_modules\.bin\tauri.cmd build --debug --no-bundle -- --offline --locked -j 1`이다.

## 문서

- [제품 요구사항](docs/PRODUCT_REQUIREMENTS.md)
- [아키텍처](docs/ARCHITECTURE.md)
- [Phase 계획](docs/PHASE_PLAN.md)
- [보안 정책](docs/SECURITY_POLICY.md)
- [작업 상태 머신](docs/STATE_MACHINE.md)
- [설계 결정](docs/DECISIONS.md)
- [프로젝트 작업 규칙](AGENTS.md)
