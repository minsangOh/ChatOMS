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
- Phase 1 UI는 앱 셸, 기본 라우팅, 시스템 상태, 프로젝트 빈 화면과 공통 오류 표시에 한정한다.

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

- Ports는 공급자, Git, 프로세스, 저장소, 플랫폼과 업데이트 기능의 계약을 정의한다.
- Adapters는 Claude CLI, Codex app-server, Git/worktree와 구조화 프로세스 실행을 구현한다.
- Infrastructure는 SQLite repository, artifact 저장소, 로깅, 마스킹과 OS별 서비스를 구현한다.
- Phase 1~6의 UpdateService는 추상화만 존재하며 실제 배포 채널과 설치 동작은 Phase 7 전 결정한다.

| Port | 책임 | Adapter / infrastructure 경계 |
|---|---|---|
| `ProviderService` | 공급자 실행, event 변환, capability 검증 | Claude CLI adapter, Codex app-server adapter |
| `GitService` | 저장소 조회, task branch/worktree 생명주기, commit·merge | Git/worktree adapter |
| `ProcessRunner` | 프로그램과 인수 배열 기반 실행, 출력·종료 제어 | OS별 structured process adapter |
| `ArtifactRepository` | 마스킹된 로그, diff, 결과와 첨부파일 저장·조회 | 로컬 artifact infrastructure |
| `FilePermissionService` | 앱 데이터 경로의 사용자 전용 권한 적용·검증 | Windows/macOS platform adapter |
| `UpdateService` | 업데이트 상태와 향후 업데이트 동작의 계약 | Phase 1~6에는 port만 정의하고 구현은 Phase 7 결정 이후 추가 |

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

## Claude와 Codex의 역할

### Claude

- 요구사항 분석, 설계, 위험 식별과 최종 리뷰를 담당한다.
- 설계 실행은 최대 12 turns, 리뷰 실행은 최대 8 turns로 제한한다.
- 공통 기본 계약은 `--permission-mode plan`, `Read`·`Glob`·`Grep`만 허용, `Edit`·`Write`·`NotebookEdit`·`Bash`·web·MCP 도구 차단, stream-json 출력과 최종 JSON Schema 검증이다.
- Phase 3에서 설치된 Claude CLI 버전이 필요한 flag, tool 제한, 구조화 출력과 turn 상한을 지원하는지 런타임 검증한다.
- 필수 정책이 지원되지 않으면 권한을 완화하거나 비구조화 출력으로 전환하지 않고 실행을 차단한다.
- Claude는 코드를 직접 수정하지 않는다.

### Codex

- 구현, 리팩터링, 테스트 작성·실행과 승인된 수정 작업을 담당한다.
- `codex app-server`의 stdio JSONL 프로토콜을 주 제어 방식으로 사용한다.
- 설치된 Codex에서 JSON Schema 또는 TypeScript schema를 생성한 뒤 프로토콜과 필수 capability를 검증한다.
- 호환성 검증이 실패해도 `codex exec`로 자동 전환하지 않는다.

## Context Package 흐름

전체 모델 대화를 그대로 전달하지 않고 작업별 Context Package를 사용한다.

```mermaid
sequenceDiagram
    participant User
    participant App as Application Layer
    participant Claude
    participant Store as SQLite / Artifacts
    participant Codex

    User->>App: 작업 요청·완료 조건·금지 범위
    App->>Claude: 마스킹된 설계 Context Package
    Claude-->>App: 구조화된 설계·위험
    App->>Store: 결정·승인·Context revision 기록
    App->>Codex: 승인 범위와 설계 Context Package
    Codex-->>App: 변경·테스트·수정 결과
    App->>Store: diff·로그·결과 artifact 기록
    App->>Claude: 실제 worktree 참조와 리뷰 package
    Claude-->>App: 구조화된 리뷰 결과
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
- 상태 갱신과 상태 전이 이력은 하나의 트랜잭션으로 기록한다.

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
