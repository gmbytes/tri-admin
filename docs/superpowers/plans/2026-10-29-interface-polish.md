# Tri Admin Interface Polish Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Polish the existing seven-module UI without adding product functionality.

**Architecture:** Keep the Vue/Element Plus application and all API/state logic. Replace shared styling with the user's blue-and-white palette, adjust presentation markup, and preserve existing semantic labels and test selectors.

**Tech Stack:** Vue 3, Element Plus, CSS, TypeScript, Playwright.

**Spec:** `docs/superpowers/specs/2026-10-29-interface-polish-design.md`

## Global Constraints

- Exactly the existing seven modules; no new business features or backend changes.
- No external font request or new package dependency.
- Use the current development branch and running Vite project for immediate review.
- Keep credentials, actual user data and service configuration untouched.

## Review Focus

- Narrow screens must not create page-level horizontal overflow; tables may scroll internally.
- Role dialogs and their save/cancel buttons must remain reachable on short screens.
- Icon-only navigation retains accessible names and an indication of the active page.
- Secondary text, borders and focus indicators remain readable in all preserved modules.
- Long filenames, API paths and permission codes must not break layouts.

### Task 1: Shared visual system and preserved page presentation

**Files:**
- Modify `apps/desktop/src/style.css` for tokens, shell, tables, forms, dialogs and responsive rules.
- Modify `apps/desktop/src/views/DashboardView.vue`, `LoginView.vue`, `ShellLayout.vue`, `UsersView.vue`, `RolesView.vue`, `MenusView.vue`, `PermissionsView.vue`, `LogsView.vue`, `FilesView.vue` for presentation only.
- Modify `apps/desktop/src/components/SideNavigation.vue` for matching icons and active semantics.
- Modify `apps/desktop/public/brand.svg` for palette consistency.
- Test `tests/e2e/core.spec.ts`.

**Interfaces:** Existing session store, navigation records, API requests and permissions are unchanged. CSS classes used by existing tests remain available.

- [x] Add a browser regression for 1440px, 900px and 390px widths, seven accessible navigation entries and role dialog bounds.
- [x] Run the targeted regression before implementation; expect the current 660px body minimum to fail at 390px.
- [x] Implement the spec, remove redundant decorative labels and unused banner styles, and keep actual counts/actions.
- [x] Run `npm test`, `npm --workspace @tri-admin/desktop run build`, formatting checks and `npm run test:e2e`; expect all to pass.
- [x] Inspect actual browser screenshots at desktop and narrow sizes; verify the preserved modules through E2E and open blank role/upload forms in IAB without saving.
- [x] Obtain one fresh final review, address material findings, and record validation results below.

## Execution record

- Working base: `dd33a5e`; clean existing development checkout. Visual implementation is authorized by the user's request; visual direction is the designer's call.
- CSS/presentation changes use existing behavior tests and visual verification. The added regression verifies user-facing viewport usability rather than mirroring CSS declarations.
- User refined the direction to Art Design Pro-inspired details with a fresh, elegant blue-and-white palette; the spec records this final direction.
- Regression RED: 390px viewport reported a 660px document. After removing the body minimum and refining responsive layout, desktop/narrow checks and the 390x500 role form pass.
- Viewport test synchronization: diagnostic evidence showed matchMedia updated before responsive CSS recalculated. One requestAnimationFrame after viewport changes makes the real-layout check deterministic; three consecutive runs passed.
- Final verification: 4 tool tests, 72 desktop tests, type checking, production build, formatting and all 18 E2E tests passed. Desktop IAB and 390px/390x500 fixture screenshots visually inspected.
- Fresh final review found no Critical/Important issue. Its one tab cascade polish finding was addressed by stable parent selectors so lazy Element Plus styles cannot replace the intended theme. No second review needed for this CSS-only correction.
- Review limits: the reviewer did not rerun the suite or control the browser; the implementing agent verified the complete suite and actual IAB screenshots. Native OS file pickers remain outside this visual task.
