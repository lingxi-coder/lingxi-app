import { lazy, Suspense } from "react";
import { Navigate, Route, Routes } from "react-router-dom";
import { HomeScreen } from "@/app/screens/home-screen";

const ComponentLab = lazy(() => import("@/app/screens/component-lab"));

function RouteFallback() {
  return (
    <div className="grid min-h-svh place-items-center" role="status">
      <span className="size-5 animate-spin rounded-full border-2 border-muted border-t-foreground" />
      <span className="sr-only">正在载入</span>
    </div>
  );
}

export function App() {
  return (
    <Suspense fallback={<RouteFallback />}>
      <Routes>
        <Route path="/" element={<HomeScreen />} />
        <Route path="/_components" element={<ComponentLab />} />
        <Route path="*" element={<Navigate replace to="/" />} />
      </Routes>
    </Suspense>
  );
}
