import { useRef } from "react";
import { zodResolver } from "@hookform/resolvers/zod";
import { useVirtualizer } from "@tanstack/react-virtual";
import { ArrowLeftIcon, BellIcon, SendIcon } from "lucide-react";
import { useForm } from "react-hook-form";
import { Link } from "react-router-dom";
import { toast } from "sonner";
import { z } from "zod";
import {
  Accordion,
  AccordionContent,
  AccordionItem,
  AccordionTrigger,
} from "@/components/ui/accordion";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Checkbox } from "@/components/ui/checkbox";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
  DialogTrigger,
} from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Progress } from "@/components/ui/progress";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { Separator } from "@/components/ui/separator";
import { Slider } from "@/components/ui/slider";
import { Switch } from "@/components/ui/switch";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { Textarea } from "@/components/ui/textarea";
import { ToggleGroup, ToggleGroupItem } from "@/components/ui/toggle-group";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";

const demoSchema = z.object({
  title: z.string().trim().min(2, "至少输入 2 个字符"),
  note: z.string().trim().max(120, "最多 120 个字符"),
});

function FormDemo() {
  const form = useForm({
    resolver: zodResolver(demoSchema),
    defaultValues: { title: "", note: "" },
  });

  const submit = (value) => {
    toast.success(`已验证：${value.title}`);
    form.reset();
  };

  return (
    <form className="space-y-4" onSubmit={form.handleSubmit(submit)}>
      <div className="space-y-2">
        <Label htmlFor="demo-title">标题</Label>
        <Input id="demo-title" placeholder="输入一个标题" {...form.register("title")} />
        {form.formState.errors.title && (
          <p className="text-xs text-destructive">{form.formState.errors.title.message}</p>
        )}
      </div>
      <div className="space-y-2">
        <Label htmlFor="demo-note">备注</Label>
        <Textarea id="demo-note" placeholder="可选" {...form.register("note")} />
      </div>
      <Button type="submit">
        <SendIcon className="size-4" />
        验证表单
      </Button>
    </form>
  );
}

function VirtualListDemo() {
  const scrollElement = useRef(null);
  const rows = useVirtualizer({
    count: 250,
    getScrollElement: () => scrollElement.current,
    estimateSize: () => 42,
    overscan: 6,
  });

  return (
    <div ref={scrollElement} className="h-52 overflow-auto rounded-xl border">
      <div className="relative w-full" style={{ height: `${rows.getTotalSize()}px` }}>
        {rows.getVirtualItems().map((row) => (
          <div
            key={row.key}
            className="absolute left-0 top-0 flex w-full items-center border-b px-3 text-sm"
            style={{ height: `${row.size}px`, transform: `translateY(${row.start}px)` }}
          >
            虚拟列表项目 {row.index + 1}
          </div>
        ))}
      </div>
    </div>
  );
}

export default function ComponentLab() {
  return (
    <main className="min-h-svh bg-background px-4 py-6 sm:px-8">
      <div className="mx-auto max-w-5xl">
        <header className="mb-8 flex items-start justify-between gap-4">
          <div>
            <Badge variant="secondary">开发辅助页</Badge>
            <h1 className="mt-3 text-3xl font-semibold tracking-tight">基础控件实验室</h1>
            <p className="mt-2 text-sm text-muted-foreground">
              此路由按需加载，不进入正常首屏 bundle。访问地址：#/_components
            </p>
          </div>
          <Button asChild size="sm" variant="outline">
            <Link to="/">
              <ArrowLeftIcon className="size-4" />
              返回
            </Link>
          </Button>
        </header>

        <div className="grid gap-5 lg:grid-cols-2">
          <Card>
            <CardHeader><CardTitle>动作与反馈</CardTitle></CardHeader>
            <CardContent className="space-y-5">
              <div className="flex flex-wrap gap-2">
                <Button>主要按钮</Button>
                <Button variant="secondary">次要按钮</Button>
                <Button variant="outline">描边按钮</Button>
                <Tooltip>
                  <TooltipTrigger asChild>
                    <Button size="icon" variant="ghost" aria-label="通知">
                      <BellIcon className="size-4" />
                    </Button>
                  </TooltipTrigger>
                  <TooltipContent>查看通知</TooltipContent>
                </Tooltip>
              </div>
              <Alert>
                <BellIcon className="size-4" />
                <AlertTitle>Provider 已挂载</AlertTitle>
                <AlertDescription>Tooltip、主题、Query 与 Toast 可以直接使用。</AlertDescription>
              </Alert>
              <Progress value={68} />
              <Slider defaultValue={[42]} max={100} step={1} />
            </CardContent>
          </Card>

          <Card>
            <CardHeader><CardTitle>选择控件</CardTitle></CardHeader>
            <CardContent className="space-y-5">
              <div className="flex items-center justify-between rounded-xl border p-3">
                <Label htmlFor="notifications">通知</Label>
                <Switch id="notifications" defaultChecked />
              </div>
              <label className="flex items-center gap-3 rounded-xl border p-3 text-sm">
                <Checkbox defaultChecked />
                允许离线保存
              </label>
              <Select defaultValue="compact">
                <SelectTrigger className="w-full"><SelectValue placeholder="选择密度" /></SelectTrigger>
                <SelectContent>
                  <SelectItem value="compact">紧凑</SelectItem>
                  <SelectItem value="comfortable">舒适</SelectItem>
                </SelectContent>
              </Select>
              <ToggleGroup type="single" defaultValue="day" variant="outline">
                <ToggleGroupItem value="day">日</ToggleGroupItem>
                <ToggleGroupItem value="week">周</ToggleGroupItem>
                <ToggleGroupItem value="month">月</ToggleGroupItem>
              </ToggleGroup>
            </CardContent>
          </Card>

          <Card>
            <CardHeader><CardTitle>表单校验</CardTitle></CardHeader>
            <CardContent><FormDemo /></CardContent>
          </Card>

          <Card>
            <CardHeader><CardTitle>大量数据</CardTitle></CardHeader>
            <CardContent><VirtualListDemo /></CardContent>
          </Card>

          <Card className="lg:col-span-2">
            <CardHeader><CardTitle>组合与浮层</CardTitle></CardHeader>
            <CardContent>
              <Tabs defaultValue="details">
                <TabsList>
                  <TabsTrigger value="details">详情</TabsTrigger>
                  <TabsTrigger value="advanced">高级</TabsTrigger>
                </TabsList>
                <TabsContent value="details" className="pt-4">
                  <Accordion type="single" collapsible>
                    <AccordionItem value="one">
                      <AccordionTrigger>为什么基础组件直接放在源码中？</AccordionTrigger>
                      <AccordionContent>
                        新应用可以立即修改控件，不需要下载 registry 或新增依赖。
                      </AccordionContent>
                    </AccordionItem>
                  </Accordion>
                </TabsContent>
                <TabsContent value="advanced" className="pt-4">
                  <Dialog>
                    <DialogTrigger asChild><Button variant="outline">打开 Dialog</Button></DialogTrigger>
                    <DialogContent>
                      <DialogHeader>
                        <DialogTitle>本地浮层</DialogTitle>
                        <DialogDescription>Radix 行为和主题样式已经配置完成。</DialogDescription>
                      </DialogHeader>
                      <Separator />
                      <DialogFooter showCloseButton />
                    </DialogContent>
                  </Dialog>
                </TabsContent>
              </Tabs>
            </CardContent>
          </Card>
        </div>
      </div>
    </main>
  );
}
