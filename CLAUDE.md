@AGENTS.md

# Claude Code 작업 규칙

- 작업 시작 전에 `README.md`와 `docs/*.md`를 확인한다.
- 프로젝트 요구사항, 설계 결정, 보안 정책, 상태 머신과 Phase 범위는 `AGENTS.md`에 정의된 문서 우선순위를 따른다.
- 현재 작업 worktree에서만 파일을 수정한다.
- 원본 checkout과 `main` 브랜치를 직접 수정하지 않는다.
- Phase 1과 Phase 2는 완료된 기반으로 취급한다.
- 현재 승인된 구현 범위는 Phase 3이다.
- Phase 4 이상 기능을 선제 구현하지 않는다.
- 구현 전에 현재 코드와 Phase 3 요구사항 사이의 gap을 먼저 분석한다.
- 기존 보안 불변조건, fail-closed 정책, recovery 정책을 테스트 통과 목적으로 완화하지 않는다.
- 동시성 제어는 필요한 resource 범위에만 적용하고, unrelated read-only operation까지 차단하는 광범위한 global lock을 도입하지 않는다.
- 새 package, dependency, manifest 또는 lockfile 변경이 필요하면 변경 전에 사용자에게 보고한다.
- 요구사항과 현재 구현이 충돌하거나 중요한 설계 판단이 필요한 경우 임의로 결정하지 말고 사용자에게 보고한다.