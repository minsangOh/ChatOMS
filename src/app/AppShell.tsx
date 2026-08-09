import { NavLink, Outlet } from "react-router-dom";

export function AppShell() {
  return (
    <div className="app-layout">
      <aside className="sidebar" aria-label="Application navigation">
        <div className="brand-block">
          <span className="brand-mark" aria-hidden="true">
            CO
          </span>
          <div>
            <strong>ChatOMS</strong>
            <span>Phase 2</span>
          </div>
        </div>
        <nav className="navigation" aria-label="Primary">
          <NavLink to="/system" className={({ isActive }) => (isActive ? "active" : undefined)}>
            System
          </NavLink>
          <NavLink to="/projects" className={({ isActive }) => (isActive ? "active" : undefined)}>
            Projects
          </NavLink>
        </nav>
        <p className="sidebar-note">Local foundation</p>
      </aside>
      <main className="main-content" id="main-content">
        <Outlet />
      </main>
    </div>
  );
}
