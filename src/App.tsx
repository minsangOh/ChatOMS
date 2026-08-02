import { AppRouter } from "./app/router";
import { ipcClient, type IpcClient } from "./ipc/client";

interface AppProps {
  client?: IpcClient;
}

export function App({ client = ipcClient }: AppProps) {
  return <AppRouter client={client} />;
}
