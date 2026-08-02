import "./test/setup";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter } from "react-router-dom";
import { expect, it } from "vitest";
import { AppRoutes } from "./app/router";
import { createFakeClient } from "./test/fixtures";

function renderRoute(path: string) {
  return render(
    <MemoryRouter initialEntries={[path]}>
      <AppRoutes client={createFakeClient()} />
    </MemoryRouter>,
  );
}

it("redirects root and unknown routes to the System page", async () => {
  const { unmount } = renderRoute("/");
  expect(await screen.findByRole("heading", { level: 1, name: "System" })).toBeVisible();
  unmount();

  renderRoute("/not-a-route");
  expect(await screen.findByRole("heading", { level: 1, name: "System" })).toBeVisible();
});

it("provides keyboard-accessible System and Projects navigation with active state", async () => {
  const user = userEvent.setup();
  renderRoute("/system");
  const systemLink = screen.getByRole("link", { name: "System" });
  const projectsLink = screen.getByRole("link", { name: "Projects" });
  expect(systemLink).toHaveAttribute("aria-current", "page");
  expect(systemLink.tabIndex).toBe(0);
  expect(projectsLink.tabIndex).toBe(0);

  await user.click(projectsLink);
  expect(await screen.findByRole("heading", { level: 1, name: "Projects" })).toBeVisible();
  expect(projectsLink).toHaveAttribute("aria-current", "page");
  expect(screen.getByRole("main")).toBeVisible();
});
