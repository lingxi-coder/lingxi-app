import { MotionConfig } from "motion/react";
import { ThemeProvider } from "next-themes";
import { HashRouter } from "react-router-dom";
import { QueryClientProvider } from "@tanstack/react-query";
import { ErrorBoundary } from "@/app/error-boundary";
import { Toaster } from "@/components/ui/sonner";
import { TooltipProvider } from "@/components/ui/tooltip";
import { LingXiBridgeProvider } from "@/lib/lingxi-provider";
import { queryClient } from "@/lib/query-client";

export function AppProviders({ children }) {
  return (
    <ErrorBoundary>
      <HashRouter>
        <QueryClientProvider client={queryClient}>
          <ThemeProvider attribute="class" defaultTheme="system" enableSystem>
            <MotionConfig reducedMotion="user">
              <TooltipProvider delayDuration={250}>
                <LingXiBridgeProvider>{children}</LingXiBridgeProvider>
                <Toaster position="top-center" richColors />
              </TooltipProvider>
            </MotionConfig>
          </ThemeProvider>
        </QueryClientProvider>
      </HashRouter>
    </ErrorBoundary>
  );
}
