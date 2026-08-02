# ChatOMS 작업 상태 머신

## 불변조건

- 여러 프로젝트를 등록할 수 있지만 전체 앱에서 활성 작업은 최대 하나다.
- `Task.project_id`는 생성 후 변경할 수 없다.
- 한 `Task`는 정확히 하나의 task branch와 최대 하나의 worktree만 가진다.
- `Paused`, `RecoveryRequired`, `UnknownExternalEffect`를 포함한 모든 실행 비종료 상태는 `ActiveTaskLease`를 유지한다.
- `ActiveTaskLease`는 `Completed`, `Failed`, `Cancelled` 중 하나에 진입할 때만 해제한다.
- `CleanupPending`과 `Archived`는 종료 결과 이후의 보존 수명주기 상태이므로 lease를 다시 획득하지 않는다.
- 상태는 Rust enum으로 표현하고 domain 유스케이스를 통해서만 변경한다.
- `Paused`는 항상 검증 후 재개할 정상 업무 상태를 `resume_target_state`로 보유한다. `RecoveryRequired`는 복구 분석 전에는 target이 없으며 검증 후에만 target을 설정한다.

## 상태와 전이

`승인` 열의 “필요”는 해당 다음 상태로 이동하기 전에 사용자 결정이 필요하다는 의미다. 중간 단계를 건너뛴 전이는 허용하지 않는다.

| 상태 | 의미와 진입 조건 | 허용 다음 상태 | 승인 | 재개 |
|---|---|---|---|---|
| `Created` | 유효한 프로젝트 참조와 작업 입력으로 생성되고 `ActiveTaskLease`를 획득함 | `ProjectValidated`, `AwaitingGitInitApproval`, `Cancelled`, `Failed` | Git 초기화가 필요하면 다음 단계에서 필요 | 가능 |
| `ProjectValidated` | 경로와 프로젝트 기본 조건 검증 완료 | `WorktreeCreating`, `Cancelled`, `Failed` | 없음 | 가능 |
| `AwaitingGitInitApproval` | 대상 폴더가 Git 저장소가 아니며 초기화 설명이 준비됨 | `GitInitialized`, `Cancelled`, `Paused` | `GitInitialized` 전이 필요 | 가능 |
| `GitInitialized` | 승인된 Git 초기화와 초기 snapshot 완료 | `WorktreeCreating`, `Failed` | 완료된 승인 필요 | 가능 |
| `WorktreeCreating` | 기준 commit을 고정하고 예약된 task branch를 실제 생성한 뒤 worktree 생성 중 | `WorktreeReady`, `RecoveryRequired`, `Failed`, `Cancelled` | 없음 | 조건부 |
| `WorktreeReady` | 작업 전용 branch와 worktree 검증 완료 | `PlanningWithClaude`, `Paused`, `Cancelled` | 공급자 전송 동의 필요 | 가능 |
| `PlanningWithClaude` | Claude가 읽기 전용으로 요구사항·설계 분석 중 | `AwaitingDesignApproval`, `ImplementingWithCodex`, `Paused`, `Failed`, `RecoveryRequired` | 고위험 설계일 때만 필요 | 가능 |
| `AwaitingDesignApproval` | 고위험 설계와 영향 설명이 준비됨 | `ImplementingWithCodex`, `Paused`, `Cancelled` | 구현 전 필요 | 가능 |
| `ImplementingWithCodex` | Codex가 승인 범위 안에서 worktree를 수정 중 | `Testing`, `Paused`, `Failed`, `RecoveryRequired` | 범위 밖 동작은 별도 승인 | 가능 |
| `Testing` | 선택된 format/lint/type/test/build 검증 실행 중 | `AutoFixing`, `ReviewingWithClaude`, `Paused`, `Failed`, `RecoveryRequired` | 승인 대상 명령이면 필요 | 가능 |
| `AutoFixing` | 서로 다른 가설로 테스트 실패 수정 중, 최대 2회 | `Testing`, `Paused`, `Failed`, `RecoveryRequired` | 금지 범위 변경은 자동 전이 불가 | 가능 |
| `ReviewingWithClaude` | Claude가 실제 파일, diff와 검증 결과를 읽기 전용 리뷰 중 | `ReviewFixing`, `AwaitingUserDiffApproval`, `Paused`, `Failed`, `RecoveryRequired` | 없음 | 가능 |
| `ReviewFixing` | Codex가 Critical/Major 지적을 수정 중, 최대 1회 | `Testing`, `Paused`, `Failed`, `RecoveryRequired` | 승인 범위 확대 시 필요 | 가능 |
| `AwaitingUserDiffApproval` | 최종 diff, 테스트와 리뷰 결과가 준비됨 | `Merging`, `Paused`, `Cancelled` | `Merging` 전이 필요 | 가능 |
| `Merging` | 승인된 단일 작업 commit을 기본 브랜치에 `--no-ff` 병합 중 | `PostMergeTesting`, `MergeConflict`, `RecoveryRequired`, `Failed` | 사전 diff 승인 필요 | 조건부 |
| `MergeConflict` | Git이 자동 병합하지 못한 충돌이 존재함 | `Merging`, `Paused`, `Cancelled`, `Failed` | 의미 판단·수동 해결은 필요 | 가능 |
| `PostMergeTesting` | 병합 결과에서 build와 regression test 실행 중 | `Completed`, `Failed`, `RecoveryRequired` | 없음 | 조건부 |
| `Completed` | 병합 후 필수 검증까지 성공하고 같은 transaction에서 lease를 해제함 | `CleanupPending`, 조건부 `Archived` | 없음 | 불가 |
| `Paused` | 사용자가 보류했거나 안전하게 중단되었고 실행 중 process가 없음. 직전 정상 업무 상태를 resume target으로 보유하며 lease는 유지됨 | 저장된 resume target으로 contextual 재개, `Cancelled`, `RecoveryRequired`, `Failed` | 재개 시 필요 | 가능 |
| `Failed` | 재시도 소진 또는 복구 불가능한 실패로 종료하고 같은 transaction에서 lease를 해제함 | `CleanupPending`, 조건부 `Archived` | 정리 정책에 따름 | 불가 |
| `RecoveryRequired` | 비정상 종료 후 로컬 상태 확인이 필요하지만 외부 부작용의 불명확성이 확정되지는 않음. lease는 유지됨 | 검증된 target으로 contextual 재개, target이 있을 때 contextual `Paused`, `Cancelled`, `Failed` | target 설정과 재개 전 필요 | 가능 |
| `UnknownExternalEffect` | 중단된 외부 요청의 성공·실패를 판정할 수 없고 lease를 유지함. resume target을 보유하지 않음 | `RecoveryRequired`, `Cancelled`, `Failed` | 상태 판정과 후속 조치에 필요 | 직접 재개·보류 불가 |
| `Cancelled` | 사용자 취소 또는 승인 거부로 종료하고 같은 transaction에서 lease를 해제함 | `CleanupPending`, 조건부 `Archived` | 파괴적 정리는 별도 정책 적용 | 불가 |
| `CleanupPending` | 종료 결과가 확정된 뒤 보존 기간·안전 조건을 기다리거나 정리 재시도를 대기함 | `Archived` | 보존 표시가 있으면 자동 정리 금지 | 정리만 재시도 |
| `Archived` | 적용 가능한 보존·정리 조건을 완료하여 실행·복구 대상이 아님 | 없음 | 없음 | 불가 |

`Completed`, `Failed`, `Cancelled`에서 정리 대상이 하나라도 있으면 반드시 `CleanupPending`을 거친다. 정리 대상이 전혀 없을 때만 `Archived`로 직접 이동할 수 있다. Task 기록과 artifact에는 [90일 보존 정책](PRODUCT_REQUIREMENTS.md#185-보관-기간)을, 병합된 branch와 worktree에는 [최소 7일 및 안전 조건](PRODUCT_REQUIREMENTS.md#20-병합-후-worktree-정리)을 적용하며 조건 충족 전에는 `Archived`로 이동하지 않는다.

## Operation class

정의되지 않았거나 현재 상태에서 허용 목록에 없는 operation은 기본 차단한다. 상태 전이 자체도 domain 전이 규칙과 승인 정책을 별도로 통과해야 한다.

| Operation class | 의미 |
|---|---|
| `ReadOnly` | 로컬 상태, 파일, Git 상태·diff와 저장된 기록 조회 |
| `ProviderRead` | 코드 변경 권한이 없는 공급자 설계·리뷰 호출 |
| `ProviderWrite` | 승인 범위에서 worktree 변경을 수행하는 Codex 공급자 호출 |
| `GitIsolation` | 승인된 Git 초기화, task branch와 worktree 생성·검증 |
| `WorktreeWrite` | task worktree 내부 파일 변경과 허용된 충돌 정리 |
| `Validation` | format, lint, typecheck, test와 build 검증 |
| `ApprovalDecision` | 사용자 결정 기록과 승인된 전이 실행 |
| `Commit` | 승인된 단일 작업 commit 생성 |
| `Merge` | 승인된 기본 브랜치 `--no-ff` 병합 |
| `RecoveryDiagnostic` | checkpoint, worktree, provider session과 external effect의 읽기 전용 진단 |
| `Cleanup` | 보존 기간과 안전 조건을 통과한 branch, worktree와 artifact 정리 |

| 상태 | 허용 operation class |
|---|---|
| `Created` | `ReadOnly` |
| `ProjectValidated` | `ReadOnly`, `GitIsolation` |
| `AwaitingGitInitApproval` | `ReadOnly`, `ApprovalDecision` |
| `GitInitialized` | `ReadOnly`, `GitIsolation` |
| `WorktreeCreating` | `ReadOnly`, `GitIsolation` |
| `WorktreeReady` | `ReadOnly`, `ApprovalDecision` |
| `PlanningWithClaude` | `ReadOnly`, `ProviderRead` |
| `AwaitingDesignApproval` | `ReadOnly`, `ApprovalDecision` |
| `ImplementingWithCodex` | `ReadOnly`, `ProviderWrite`, `WorktreeWrite` |
| `Testing` | `ReadOnly`, `Validation` |
| `AutoFixing` | `ReadOnly`, `ProviderWrite`, `WorktreeWrite`, `Validation` |
| `ReviewingWithClaude` | `ReadOnly`, `ProviderRead` |
| `ReviewFixing` | `ReadOnly`, `ProviderWrite`, `WorktreeWrite`, `Validation` |
| `AwaitingUserDiffApproval` | `ReadOnly`, `ApprovalDecision` |
| `Merging` | `ReadOnly`, `Commit`, `Merge` |
| `MergeConflict` | `ReadOnly`, `ApprovalDecision`, 정책이 허용한 `WorktreeWrite` |
| `PostMergeTesting` | `ReadOnly`, `Validation` |
| `Completed` | `ReadOnly` |
| `Paused` | `ReadOnly`, `RecoveryDiagnostic`, `ApprovalDecision` |
| `Failed` | `ReadOnly` |
| `RecoveryRequired` | `ReadOnly`, `RecoveryDiagnostic`, `ApprovalDecision` |
| `UnknownExternalEffect` | `ReadOnly`, `RecoveryDiagnostic`, `ApprovalDecision` |
| `Cancelled` | `ReadOnly` |
| `CleanupPending` | `ReadOnly`, `Cleanup` |
| `Archived` | `ReadOnly` |

승인 대기 상태인 `AwaitingGitInitApproval`, `AwaitingDesignApproval`, `AwaitingUserDiffApproval`에서는 `ReadOnly`와 `ApprovalDecision`만 허용하고 자동 진행하지 않는다. 기본 브랜치 merge는 `Merging`에서만, 자동 cleanup은 `CleanupPending`에서만 허용한다.

`RecoveryRequired`에서는 읽기 전용 `RecoveryDiagnostic`만 자동 수행할 수 있고 명령을 자동 재실행하지 않는다. `UnknownExternalEffect`에서는 결과가 불명확한 외부 쓰기 요청을 재시도하지 않는다.

## 사용자 승인이 필요한 주요 전이

- `AwaitingGitInitApproval → GitInitialized`
- `WorktreeReady → PlanningWithClaude`: 작업별 공급자 전송 1회 동의
- `AwaitingDesignApproval → ImplementingWithCodex`
- 승인 범위를 확대하는 구현·테스트·리뷰 수정
- `AwaitingUserDiffApproval → Merging`
- 의미 판단이 필요한 `MergeConflict → Merging`
- `Paused` 또는 `RecoveryRequired`에서 실행 상태로 재개
- `UnknownExternalEffect`의 판정과 후속 조치

## 복구와 재개

### RecoveryRequired

로컬 process, DB, worktree 또는 checkpoint의 일관성을 확인해야 하는 상태다. 읽기 전용 진단으로 실제 상태를 확인한 뒤 checkpoint/version, worktree와 외부 효과 검증을 나타내는 domain validation과 함께 명시적인 resume target을 설정한다. 검증된 target이 있을 때만 `Paused`로 이동하거나 해당 target으로 재개할 수 있으며, 진단 결과만으로 명령이나 공급자 호출을 자동 재실행하지 않는다.

### UnknownExternalEffect

외부 요청이 중단되어 원격 부작용의 발생 여부를 알 수 없는 상태다. 성공이나 실패로 추정하지 않으며 동일 외부 쓰기 요청을 자동 재실행하지 않는다. resume target을 보유하거나 `Paused`로 직접 이동하지 않으며, 정상 복구는 사용자 또는 외부 시스템의 확인을 거쳐 `RecoveryRequired`로 이동한 뒤 명시적인 복구 target을 설정하는 순서로만 수행한다.

### 재개 검증

정적 상태 전이 API는 문맥이 필요 없는 edge만 검증한다. 정상 업무 상태에서 `Paused` 진입, `Paused`에서 저장된 target으로 재개, `RecoveryRequired`에서 검증된 target 설정·재개와 target을 유지한 `Paused` 전이는 Task aggregate의 contextual API로만 수행한다. `Paused`와 `RecoveryRequired`에서 재개할 때는 저장된 상태를 그대로 신뢰하지 않고 다음을 다시 검증한다.

1. 마지막 완료 checkpoint와 Task version
2. task branch와 worktree의 존재, 연결 관계와 현재 변경 상태
3. provider session 참조의 존재와 재사용 가능 여부
4. 미해결 external effect의 존재 여부
5. 목표 상태로의 domain 전이 규칙과 필요한 사용자 승인

검증이 불일치하면 실행 상태로 이동하지 않고 `RecoveryRequired` 또는 `UnknownExternalEffect`로 전이한다.

## Transaction과 checkpoint

상태 전이는 다음 작업을 하나의 SQLite transaction으로 처리한다.

1. 예상 현재 상태와 Task version 검증
2. domain 전이 규칙 검증
3. Task 현재 상태와 version 갱신
4. 이전·다음 상태, 원인, actor와 시각을 상태 전이 이력에 추가
5. 필요하면 checkpoint 참조 갱신
6. `Completed`, `Failed`, `Cancelled` 진입이면 같은 transaction에서 `ActiveTaskLease` 해제

최초 `Created` transition은 sequence `1`, `from_state = None`, `task_version = 0`으로 기록한다. 이후 sequence는 직전 값에서 정확히 1 증가해야 하며, 마지막 sequence 조회와 연속성·동시성 검증은 repository가 같은 transaction 안에서 수행한다.

하나라도 실패하면 전체를 rollback한다. 정리는 lease 해제와 별개이며 종료 결과가 확정된 뒤 `CleanupPending`에서 수행한다. 장시간 작업 직전과 완료 직후에는 checkpoint를 기록하며, 앱 재시작 시 SQLite 현재 상태와 마지막 완료 checkpoint를 함께 검증한다.
