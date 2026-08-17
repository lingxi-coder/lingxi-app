import { motion } from "motion/react";
import { toast } from "sonner";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Card, CardContent } from "@/components/ui/card";
import { useLingXi } from "@/lib/lingxi-provider";
import { cn } from "@/lib/utils";
import { useAppStore } from "@/src/stores/app-store";

const foundationItems = [
  {
    icon: "blocks",
    title: "常用控件已就绪",
    detail: "Radix + shadcn/ui 源码可直接修改",
  },
  {
    icon: "shield",
    title: "宿主能力已接入",
    detail: "数据、设备与网络统一经过 LingXi Bridge",
  },
  {
    icon: "sparkles",
    title: "应用能力已预装",
    detail: "Router、Query、表单、状态与动效无需安装",
  },
];

const foundationIconPaths = {
  blocks: <><rect x="3" y="3" width="7" height="7" rx="2" /><rect x="14" y="3" width="7" height="7" rx="2" /><rect x="3" y="14" width="7" height="7" rx="2" /><path d="M14 17.5h7M17.5 14v7" /></>,
  shield: <><path d="M12 3 4.5 6v5.3c0 4.5 3 8.4 7.5 9.7 4.5-1.3 7.5-5.2 7.5-9.7V6L12 3Z" /><path d="m8.8 12 2.1 2.1 4.5-4.5" /></>,
  sparkles: <><path d="m12 3 1.1 3.2L16 7.5l-2.9 1.3L12 12l-1.1-3.2L8 7.5l2.9-1.3L12 3Z" /><path d="m6 13 .8 2.2L9 16l-2.2.8L6 19l-.8-2.2L3 16l2.2-.8L6 13Z" /><path d="m18 13 .7 1.8 1.8.7-1.8.7L18 18l-.7-1.8-1.8-.7 1.8-.7L18 13Z" /></>,
  check: <path d="m5 12 4 4L19 6" />,
};

function FoundationIcon({ name, className = "size-4" }) {
  return (
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round" strokeLinejoin="round" className={className} aria-hidden="true">
      {foundationIconPaths[name]}
    </svg>
  );
}

export function HomeScreen() {
  const { adapter, bridgeReady, platformStyle } = useLingXi();
  const completedActions = useAppStore((state) => state.completedActions);
  const completeAction = useAppStore((state) => state.completeAction);

  const handleReady = () => {
    completeAction();
    toast.success("基础模板运行正常");
  };

  return (
    <main
      className="app-backdrop relative min-h-svh overflow-hidden"
      data-input-mode={adapter.context.inputMode}
      data-platform={adapter.key}
      style={platformStyle}
    >
      <div className="app-grid pointer-events-none absolute inset-0 opacity-45" />
      <div
        className="relative mx-auto flex min-h-svh w-full max-w-5xl flex-col px-5 pb-8 sm:px-8"
        style={{
          paddingTop: "max(1.25rem, var(--safe-area-top))",
          paddingLeft: "max(1.25rem, var(--safe-area-left))",
          paddingRight: "max(1.25rem, var(--safe-area-right))",
          paddingBottom: "max(2rem, var(--safe-area-bottom))",
        }}
      >
        <header className="flex items-center justify-between py-3">
          <div className="flex items-center gap-3">
            <div className="grid size-9 place-items-center rounded-xl border bg-card shadow-sm">
              <FoundationIcon name="sparkles" />
            </div>
            <div>
              <p className="text-sm font-semibold tracking-tight">LingXi Local App</p>
              <p className="text-xs text-muted-foreground">Vite foundation · offline ready</p>
            </div>
          </div>
          <Badge
            variant="outline"
            className={cn(
              "gap-1.5 bg-card/70 backdrop-blur",
              bridgeReady && "border-emerald-500/30 text-emerald-700 dark:text-emerald-300",
            )}
          >
            <span
              className={cn(
                "size-1.5 rounded-full bg-amber-500",
                bridgeReady && "bg-emerald-500",
              )}
            />
            {bridgeReady ? "Bridge 已连接" : "等待 Bridge"}
          </Badge>
        </header>

        <section className="flex flex-1 items-center py-10 sm:py-16">
          <div className="grid w-full items-center gap-10 lg:grid-cols-[1.08fr_0.92fr]">
            <motion.div
              initial={{ opacity: 0, y: 12 }}
              animate={{ opacity: 1, y: 0 }}
              transition={{ duration: 0.38, ease: "easeOut" }}
            >
              <Badge className="mb-5" variant="secondary">
                内置基础模板 v2
              </Badge>
              <h1 className="max-w-2xl text-balance text-4xl font-semibold tracking-[-0.04em] sm:text-6xl">
                直接开始构建，
                <span className="text-muted-foreground">不再等待初始化。</span>
              </h1>
              <p className="mt-5 max-w-xl text-pretty text-base leading-7 text-muted-foreground sm:text-lg">
                基础组件、主题、Providers 和依赖已经随 App 离线内置。生成任务只需修改业务源码，然后执行一次固定的 Vite build。
              </p>
              <div className="mt-8 flex flex-wrap items-center gap-3">
                <Button size="lg" onClick={handleReady}>
                  <FoundationIcon name="check" />
                  验证基础模板
                </Button>
                <p className="text-sm text-muted-foreground" aria-live="polite">
                  已完成 {completedActions} 次本地交互
                </p>
              </div>
            </motion.div>

            <Card className="border-border/70 bg-card/75 shadow-xl shadow-black/5 backdrop-blur-xl">
              <CardContent className="space-y-2 p-3 sm:p-4">
                {foundationItems.map((item, index) => (
                  <motion.article
                    key={item.title}
                    className="flex items-start gap-4 rounded-xl border border-transparent p-4 transition-colors hover:border-border hover:bg-background/60"
                    initial={{ opacity: 0, x: 12 }}
                    animate={{ opacity: 1, x: 0 }}
                    transition={{ delay: 0.08 + index * 0.06, duration: 0.3 }}
                  >
                    <div className="grid size-10 shrink-0 place-items-center rounded-xl bg-secondary text-secondary-foreground">
                      <FoundationIcon name={item.icon} />
                    </div>
                    <div>
                      <h2 className="text-sm font-semibold">{item.title}</h2>
                      <p className="mt-1 text-sm leading-6 text-muted-foreground">{item.detail}</p>
                    </div>
                  </motion.article>
                ))}
                <div className="flex items-center justify-between rounded-xl bg-foreground px-4 py-3 text-background">
                  <span className="text-sm font-medium">当前适配</span>
                  <code className="text-xs opacity-75">
                    {adapter.context.os}:{adapter.context.formFactor}
                  </code>
                </div>
              </CardContent>
            </Card>
          </div>
        </section>
      </div>
    </main>
  );
}
