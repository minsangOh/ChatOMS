# ChatOMS Design System

## 1. Product character

ChatOMS is a local developer-safety harness. Its interface should feel calm, explicit, and operational: important state is visible, irreversible choices require deliberate confirmation, and failures use fixed safe wording. The product is not a marketing surface, so clarity and trust take precedence over decorative novelty.

## 2. Color

- Page background: light blue-gray (`#eef1f6`); dark mode uses deep navy (`#101621`).
- Surface: white cards in light mode and blue-black cards (`#1a2230`) in dark mode.
- Primary text: near-black navy (`#172033`); muted text uses the established gray-blue palette.
- Accent and primary actions: restrained blue (`#2f5fad`); keyboard focus uses `#2f6fed`.
- Borders: cool gray (`#d6dce7`) with the existing subtle card shadow.
- Semantic state colors remain the existing positive, warning, negative, and neutral badge/notice colors in `src/styles.css`.

Do not introduce a new accent palette for individual features. Security and approval panels use the same surfaces and semantic notice colors as the rest of the application.

## 3. Typography

- Use the existing system UI font stack for headings, labels, body copy, and controls.
- Use the existing monospace stack, headed by Cascadia, only for identifiers and code-like values.
- Page titles remain the largest typographic element. Card headings and panel headings follow the current descending scale in `src/styles.css`.
- Body copy should be concise and literal. Confirmation copy must state scope, version binding, and immutability without relying on color alone.

## 4. Spacing and layout

- Preserve the current 4px-derived rhythm expressed by the established `0.25rem` through `3rem` spacing values.
- Pages use `page-stack`; major surfaces use `content-card` or `project-card`; task-specific controls stay inside the existing isolation/planning panels.
- Related approval rows use the established compact list rhythm. Confirmation actions use `form-actions`.
- Keep desktop density efficient and preserve the existing responsive breakpoints at 760px and 460px.

## 5. Components and interaction

- Primary actions use `button`; secondary or cancel actions use `button button--secondary`.
- Read-only status uses `muted`, `status-badge`, or existing semantic notice patterns.
- Recoverable loading and failure states never expose raw errors. Actions remain hidden or disabled until authoritative status is ready.
- Immutable approvals and declarations show recorded state without edit, replace, delete, or reset controls.
- Explicit confirmations use a checkbox or a dedicated confirmation step. Empty/default selections never imply consent.

## 6. Accessibility

- Every panel has a visible heading or `aria-label`; related controls use `fieldset` and `legend` where appropriate.
- Native buttons, radio buttons, and checkboxes are preferred over custom interactive elements.
- Disabled actions must be explained by adjacent text, not color alone.
- Preserve the existing keyboard focus treatment and reduced-motion behavior.
- Status changes that affect available actions should use an appropriate live region when the user would otherwise miss them.

## 7. Content and security boundaries

- UI status surfaces may display only fixed vocabulary and content-free metadata approved by the relevant contract.
- Raw paths, source, diff hunks, prompts, plans, provider output, process output, executable/environment details, authentication/session data, and persistence identifiers do not belong in generic policy or risk-assessment panels.
- Error copy is fixed and safe. Never interpolate backend error text into a security-sensitive panel.
- High-risk approvals, operation-risk declarations, provider consent, validation approval, diff approval, manual-resolution confirmation, and merge-abort approval remain visually and semantically distinct.

## 8. Current debt and extension rule

`src/styles.css` predates this document and contains literal values rather than centralized CSS custom properties. Consolidating those literals is accepted design-system debt and is outside narrowly scoped feature Units. New UI should first reuse existing classes; any necessary new selector must use the existing palette, spacing rhythm, typography, focus behavior, dark-mode treatment, and responsive conventions without broad stylesheet refactoring.
