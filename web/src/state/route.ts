import { useCallback, useEffect, useState } from "react";

/** The views, as URL paths under the mount, so a reload or a shared link lands on the same one. */
export type View = "list" | "review" | "map" | "projects" | "topics" | "categories" | "about";

const MOUNT = "/ui";

export const VIEW_PATHS: Record<View, string> = {
  list: `${MOUNT}/`,
  review: `${MOUNT}/review`,
  map: `${MOUNT}/map`,
  projects: `${MOUNT}/projects`,
  topics: `${MOUNT}/topics`,
  categories: `${MOUNT}/categories`,
  about: `${MOUNT}/about`,
};

function viewOf(pathname: string): View {
  const path = pathname.replace(/\/+$/, "");
  if (path.endsWith("/map")) return "map";
  if (path.endsWith("/review")) return "review";
  if (path.endsWith("/projects")) return "projects";
  if (path.endsWith("/topics")) return "topics";
  if (path.endsWith("/categories")) return "categories";
  if (path.endsWith("/about")) return "about";
  return "list";
}

export function useView(): [View, (view: View) => void] {
  const [view, setView] = useState<View>(() => viewOf(window.location.pathname));

  useEffect(() => {
    const onPop = () => setView(viewOf(window.location.pathname));
    window.addEventListener("popstate", onPop);
    return () => window.removeEventListener("popstate", onPop);
  }, []);

  const navigate = useCallback((next: View) => {
    const path = VIEW_PATHS[next];
    if (window.location.pathname !== path) window.history.pushState(null, "", path);
    setView(next);
  }, []);

  return [view, navigate];
}
