import React from "react";
import { Button } from "@/components/ui/button";

export class ErrorBoundary extends React.Component {
  constructor(props) {
    super(props);
    this.state = { error: null };
  }

  static getDerivedStateFromError(error) {
    return { error };
  }

  render() {
    if (!this.state.error) return this.props.children;

    return (
      <main className="grid min-h-svh place-items-center p-6">
        <section className="w-full max-w-md rounded-2xl border bg-card p-6 shadow-sm">
          <p className="text-sm font-medium text-destructive">应用暂时无法显示</p>
          <h1 className="mt-2 text-2xl font-semibold tracking-tight">出现了意外错误</h1>
          <p className="mt-3 text-sm leading-6 text-muted-foreground">
            {this.state.error.message || "请重新载入本地应用。"}
          </p>
          <Button className="mt-6" onClick={() => window.location.reload()}>
            重新载入
          </Button>
        </section>
      </main>
    );
  }
}
