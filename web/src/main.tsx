import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { App } from "./App";
import { HttpApiClient } from "./api/client";
import { sessionKeyStore } from "./state/keyStore";
import { SessionProvider, type ClientFactory } from "./state/session";
import { ToastProvider } from "./state/toasts";
import "./index.css";

const createClient: ClientFactory = (key) => new HttpApiClient(key);
const queryClient = new QueryClient({
  defaultOptions: { queries: { retry: 1, refetchOnWindowFocus: false } },
});

const root = document.getElementById("root");
if (root === null) throw new Error("index.html is missing #root");

createRoot(root).render(
  <StrictMode>
    <QueryClientProvider client={queryClient}>
      <SessionProvider createClient={createClient} keyStore={sessionKeyStore}>
        <ToastProvider>
          <App />
        </ToastProvider>
      </SessionProvider>
    </QueryClientProvider>
  </StrictMode>,
);
