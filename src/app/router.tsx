import { BrowserRouter, Navigate, Route, Routes } from "react-router-dom";
import type { IpcClient } from "../ipc/client";
import { ProjectsPage } from "../pages/ProjectsPage";
import { SystemPage } from "../pages/SystemPage";
import { AppShell } from "./AppShell";

interface RouterProps {
  client: IpcClient;
}

export function AppRouter({ client }: RouterProps) {
  return (
    <BrowserRouter>
      <AppRoutes client={client} />
    </BrowserRouter>
  );
}

export function AppRoutes({ client }: RouterProps) {
  return (
    <Routes>
      <Route element={<AppShell />}>
        <Route index element={<Navigate replace to="/system" />} />
        <Route path="system" element={<SystemPage client={client} />} />
        <Route path="projects" element={<ProjectsPage client={client} />} />
        <Route path="*" element={<Navigate replace to="/system" />} />
      </Route>
    </Routes>
  );
}
