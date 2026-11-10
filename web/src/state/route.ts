import { useCallback, useEffect, useState } from "react";

/** The two views, as URL paths under the mount, so a reload or a shared link lands on the same one. */
export type View = "list" | "map";

const MOUNT = "/ui";

function viewOf(pathname: string): View {
  return pathname.replace(/\/+$/, "").endsWith("/map") ? "map" : "list";
}

export function useView(): [View, (view: View) => void] {
  const [view, setView] = useState<View>(() => viewOf(window.location.pathname));

  useEffect(() => {
    const onPop = () => setView(viewOf(window.location.pathname));
    window.addEventListener("popstate", onPop);
    return () => window.removeEventListener("popstate", onPop);
  }, []);

  const navigate = useCallback((next: View) => {
    const path = next === "map" ? `${MOUNT}/map` : `${MOUNT}/`;
    if (window.location.pathname !== path) window.history.pushState(null, "", path);
    setView(next);
  }, []);

  return [view, navigate];
}
