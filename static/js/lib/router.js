// Hash router hook. Pure parsing lives in route.js.
import { useState, useEffect } from "preact/hooks";
import { parseRoute } from "./route.js";

export { APP_TABS, parseRoute, pathFor } from "./route.js";

export function navigate(path) {
  if (location.hash !== path) location.hash = path;
}

export function useRoute() {
  const [route, setRoute] = useState(() => parseRoute(location.hash));
  useEffect(() => {
    const onChange = () => setRoute(parseRoute(location.hash));
    window.addEventListener("hashchange", onChange);
    return () => window.removeEventListener("hashchange", onChange);
  }, []);
  return route;
}
