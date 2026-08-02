# ChatOMS

ChatOMS는 공식 Claude Code CLI와 공식 Codex CLI를 로컬에서 조율하는 크로스플랫폼 데스크톱 AI 코딩 하네스다. Claude가 요구사항 분석·설계·최종 리뷰를 담당하고, Codex가 격리된 Git worktree에서 구현·테스트·수정을 수행하는 순차 워크플로를 GUI로 제공하는 것이 목표다.

## MVP 범위

- 여러 로컬 프로젝트를 목록에 등록할 수 있다.
- 전체 앱에서 활성 작업은 최대 하나이며, 한 작업은 하나의 프로젝트, 정확히 하나의 task branch와 최대 하나의 worktree만 사용한다.
- 작업 흐름은 Claude 설계 → Codex 구현·테스트 → Claude 리뷰 → 사용자 diff 승인 → 기본 브랜치 병합 순서다.
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

Phase 1의 앱 아이콘은 프로젝트 내부에서 결정적으로 생성한 임시 자산이며, 후속 UI 디자인 단계에서 교체한다.

Phase 1 수동 스캐폴드는 Windows에서 TypeScript typecheck, Vite build, Rust workspace check와 Tauri debug executable 생성을 통과했다. Phase 1 build gate는 installer를 생성하지 않는 `tauri build --debug --no-bundle -- --offline --locked -j 1`이다. Codex fallback pnpm 환경에서는 `node_modules\.bin\tauri.cmd`를 직접 실행한다.

## 문서

- [제품 요구사항](docs/PRODUCT_REQUIREMENTS.md)
- [아키텍처](docs/ARCHITECTURE.md)
- [Phase 계획](docs/PHASE_PLAN.md)
- [보안 정책](docs/SECURITY_POLICY.md)
- [작업 상태 머신](docs/STATE_MACHINE.md)
- [설계 결정](docs/DECISIONS.md)
- [프로젝트 작업 규칙](AGENTS.md)
