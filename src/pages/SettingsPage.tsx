import { useEffect, useId, useRef, useState } from "react";
import { Check, CircleAlert, Download, FolderSearch, Loader2, Pause, Play, RefreshCw, RotateCcw } from "lucide-react";
import { toast } from "sonner";
import * as api from "@/api";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Progress } from "@/components/ui/progress";
import { Switch } from "@/components/ui/switch";
import { DEFAULT_COMMIT_TEMPLATE, NETWORK_TIMEOUT_CHOICES, renderCommitMessage } from "@/lib/status";
import { cn } from "@/lib/utils";
import { activePause, describePauseUntil, useSettings } from "@/state/settings";
import type { ProxyMode, Settings, Theme, UpdateInfo, UpdateProgress } from "@/types";

/** 设置：默认代码目录、联网超时、代理、自动同步、提交信息模板与本机名称、浅色/深色主题、关于与检查更新。pendingUpdate 是启动时已发现的新版本 */
export function SettingsPage({ pendingUpdate = null }: { pendingUpdate?: UpdateInfo | null }) {
  const { settings, updateSettings } = useSettings();
  const [picking, setPicking] = useState(false);

  async function pickDir() {
    setPicking(true);
    try {
      const dir = await api.pickDirectory(settings?.defaultCodeDir);
      if (dir) {
        await updateSettings({ defaultCodeDir: dir });
        toast.success("默认代码目录已保存", { description: dir });
      }
    } finally {
      setPicking(false);
    }
  }

  return (
    <div className="max-w-2xl space-y-8">
      <header>
        <h1 className="text-xl font-semibold tracking-tight">设置</h1>
      </header>

      <section className="space-y-3">
        <h2 className="text-sm font-medium text-muted-foreground">常规</h2>
        <div className="rounded-lg border bg-card p-4">
          <div className="text-sm font-medium">默认代码目录</div>
          <p className="mt-0.5 text-xs text-muted-foreground">"添加项目"页面会默认扫描这个目录。</p>
          <div className="mt-3 flex gap-2">
            <Input readOnly value={settings?.defaultCodeDir ?? ""} className="font-mono select-text" />
            <Button variant="outline" onClick={pickDir} disabled={picking || !settings}>
              {picking ? <Loader2 className="animate-spin" /> : <FolderSearch />}
              选择…
            </Button>
          </div>
        </div>
        <NetworkTimeoutCard />
        <ProxyCard />
      </section>

      <AutoSyncSection />

      <section className="space-y-3">
        <h2 className="text-sm font-medium text-muted-foreground">自动提交</h2>
        <CommitTemplateCard />
      </section>

      <section className="space-y-3">
        <h2 className="text-sm font-medium text-muted-foreground">外观</h2>
        <div className="rounded-lg border bg-card p-4">
          <div className="text-sm font-medium">主题</div>
          <div className="mt-3 grid grid-cols-2 gap-3" role="radiogroup" aria-label="主题">
            <ThemeCard
              theme="light"
              label="浅色"
              active={settings?.theme === "light"}
              onSelect={() => updateSettings({ theme: "light" })}
            />
            <ThemeCard
              theme="dark"
              label="深色"
              active={settings?.theme === "dark"}
              onSelect={() => updateSettings({ theme: "dark" })}
            />
          </div>
        </div>
      </section>

      <AboutSection pendingUpdate={pendingUpdate} />
    </div>
  );
}

/** 联网超时：fetch、push、pull 超过这个时间就终止，点选即保存 */
function NetworkTimeoutCard() {
  const { settings, updateSettings } = useSettings();

  async function choose(secs: number) {
    if (secs === settings?.networkTimeoutSecs) return;
    try {
      await updateSettings({ networkTimeoutSecs: secs });
      toast.success(`联网超时已改为 ${secs / 60} 分钟`);
    } catch (e) {
      toast.error("保存失败", { description: e instanceof Error ? e.message : String(e) });
    }
  }

  return (
    <div className="rounded-lg border bg-card p-4">
      <div className="text-sm font-medium">联网超时</div>
      <p className="mt-0.5 text-xs text-muted-foreground">
        一键同步、提交并推送时连接 GitHub 超过这个时间就停止（刷新状态只是看一眼，最多等 30 秒）。网络慢或者一次要拉取的内容很多时，可以调长一些。
      </p>
      <div className="mt-3 inline-flex rounded-md border p-0.5" role="radiogroup" aria-label="联网超时">
        {NETWORK_TIMEOUT_CHOICES.map((secs) => {
          const active = settings?.networkTimeoutSecs === secs;
          return (
            <button
              key={secs}
              type="button"
              role="radio"
              aria-checked={active}
              disabled={!settings}
              onClick={() => choose(secs)}
              className={cn(
                "rounded px-3 py-1 text-sm transition-colors",
                active ? "bg-foreground text-background" : "text-muted-foreground hover:text-foreground",
              )}
            >
              {secs / 60} 分钟
            </button>
          );
        })}
      </div>
    </div>
  );
}

const PROXY_MODE_OPTIONS: { mode: ProxyMode; label: string; saved: string }[] = [
  { mode: "auto", label: "自动", saved: "已改为自动选择代理" },
  { mode: "off", label: "不用代理", saved: "已改为不用代理" },
  { mode: "manual", label: "手动填写", saved: "已改为使用手动填写的代理" },
];

/** "当前：……"一行：SyncDeck 调用的 git 实际用的代理 */
function describeProxy(p: Pick<Settings, "effectiveProxy" | "proxySource">): string {
  switch (p.proxySource) {
    case "git":
      return `当前：使用 git 自己的代理 ${p.effectiveProxy ?? ""}`;
    case "system":
      return `当前：使用系统代理 ${p.effectiveProxy ?? ""}`;
    case "manual":
      return `当前：使用手动填写的代理 ${p.effectiveProxy ?? ""}`;
    default:
      return "当前：不使用代理";
  }
}

/** 代理：自动 / 不用代理 / 手动填写。点选即保存；手动地址失焦或回车保存 */
function ProxyCard() {
  const { settings, updateSettings } = useSettings();
  const savedMode = settings?.proxyMode ?? "auto";
  const savedUrl = settings?.proxyUrl ?? "";
  // 还没保存的选择和输入；null 表示跟随已保存的设置。保存失败时保留，方便改一下再保存
  const [pendingMode, setPendingMode] = useState<ProxyMode | null>(null);
  const [draft, setDraft] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);
  const inputRef = useRef<HTMLInputElement>(null);
  const mode = pendingMode ?? savedMode;
  const url = draft ?? savedUrl;

  // 当前实际情况：打开设置页时重新读一次（代理软件可能刚开或刚关），之后以每次保存的结果为准
  const [current, setCurrent] = useState<Pick<Settings, "effectiveProxy" | "proxySource"> | null>(null);
  useEffect(() => {
    if (settings) setCurrent({ effectiveProxy: settings.effectiveProxy, proxySource: settings.proxySource });
  }, [settings]);
  useEffect(() => {
    let alive = true;
    api.getSettings().then(
      (s) => alive && setCurrent({ effectiveProxy: s.effectiveProxy, proxySource: s.proxySource }),
      () => undefined,
    );
    return () => {
      alive = false;
    };
  }, []);

  async function save(patch: Partial<Settings>, success: string): Promise<boolean> {
    setSaving(true);
    try {
      await updateSettings(patch);
      toast.success(success);
      return true;
    } catch (e) {
      toast.error("保存失败", { description: e instanceof Error ? e.message : String(e) });
      // updateSettings 会先乐观更新，失败时重新读回已保存的值
      await updateSettings({}).catch(() => undefined);
      return false;
    } finally {
      setSaving(false);
    }
  }

  async function choose(next: ProxyMode) {
    if (next === mode) return;
    // 切换模式时丢掉没保存的地址草稿：切回"手动填写"时生效的是已保存的地址，输入框也要显示它
    setDraft(null);
    if (next === "manual" && !savedUrl) {
      // 还没填过地址：先填，失焦或回车时再保存
      setPendingMode("manual");
      setTimeout(() => inputRef.current?.focus(), 0);
      return;
    }
    if (next === savedMode) {
      // 取消还没保存的"手动填写"
      setPendingMode(null);
      return;
    }
    setPendingMode(next);
    const option = PROXY_MODE_OPTIONS.find((o) => o.mode === next);
    await save({ proxyMode: next }, option?.saved ?? "代理设置已保存");
    setPendingMode(null);
  }

  async function saveUrl() {
    const next = url.trim();
    if (!next && savedMode !== "manual") return; // 还没填，不打扰
    if (next === savedUrl && savedMode === "manual") {
      setDraft(null);
      return;
    }
    if (await save({ proxyMode: "manual", proxyUrl: next }, "代理地址已保存")) {
      setPendingMode(null);
      setDraft(null);
    }
  }

  return (
    <div className="rounded-lg border bg-card p-4">
      <div className="text-sm font-medium">代理</div>
      <p className="mt-0.5 text-xs text-muted-foreground">
        自动：git 自己配了代理就用 git 的；没配而 Windows 系统代理开着，就用系统代理。只对 SyncDeck 调用的 git 生效，不改全局 git
        配置。
      </p>
      <div className="mt-3 inline-flex rounded-md border p-0.5" role="radiogroup" aria-label="代理">
        {PROXY_MODE_OPTIONS.map(({ mode: m, label }) => {
          const active = mode === m;
          return (
            <button
              key={m}
              type="button"
              role="radio"
              aria-checked={active}
              disabled={!settings || saving}
              // 不让地址输入框先失焦保存一次草稿
              onMouseDown={(e) => e.preventDefault()}
              onClick={() => void choose(m)}
              className={cn(
                "rounded px-3 py-1 text-sm transition-colors",
                active ? "bg-foreground text-background" : "text-muted-foreground hover:text-foreground",
              )}
            >
              {label}
            </button>
          );
        })}
      </div>
      {mode === "manual" && (
        <Input
          ref={inputRef}
          value={url}
          onChange={(e) => setDraft(e.target.value)}
          onBlur={() => void saveUrl()}
          onKeyDown={(e) => e.key === "Enter" && e.currentTarget.blur()}
          placeholder="如 http://127.0.0.1:7890 或 socks5://127.0.0.1:1080"
          disabled={!settings || saving}
          className="mt-3 font-mono"
          aria-label="代理地址"
        />
      )}
      <p className="mt-3 text-sm break-all">{current ? describeProxy(current) : "当前：正在检测…"}</p>
      <p className="mt-1 text-xs text-muted-foreground">
        只影响 https 地址的仓库，SSH 地址的仓库不受影响；仓库里单独配置的 remote.&lt;远程名&gt;.proxy 也不受这里影响
      </p>
    </div>
  );
}

// ---------------- 自动同步 ----------------

type SwitchKey =
  | "launchAtLogin"
  | "closeToTray"
  | "syncOnShutdown"
  | "syncOnLock"
  | "syncOnSleep"
  | "syncOnStartup"
  | "syncOnResume"
  | "syncOnUnlock"
  | "periodicCheck"
  | "notifyOnSuccess"
  | "notifyOnError";

interface SwitchItem {
  key: SwitchKey;
  label: string;
  description?: string;
}

const BACKGROUND_SWITCHES: SwitchItem[] = [
  { key: "launchAtLogin", label: "开机自启", description: "开机后只在托盘里运行，不弹出窗口" },
  {
    key: "closeToTray",
    label: "关窗口时隐藏到托盘",
    description: "关掉窗口后继续在托盘里运行；要彻底退出，在托盘图标上右键选「退出」",
  },
];

const LEAVE_SWITCHES: SwitchItem[] = [
  { key: "syncOnShutdown", label: "关机、重启或注销", description: "最多等 30 秒，同步完或超时就继续关机" },
  { key: "syncOnLock", label: "锁屏" },
  { key: "syncOnSleep", label: "睡眠", description: "Windows 只给几秒，来不及推完的，下次唤醒或开机时补上" },
];

const ARRIVE_SWITCHES: SwitchItem[] = [
  { key: "syncOnStartup", label: "启动 SyncDeck 时", description: "包括开机自启" },
  { key: "syncOnResume", label: "从睡眠唤醒", description: "网络没连上时，等 15 秒再试一次" },
  { key: "syncOnUnlock", label: "解锁" },
];

const CHECK_SWITCHES: SwitchItem[] = [
  {
    key: "periodicCheck",
    label: "定时检查云端",
    description: "每 30 分钟看一次云端有没有新提交，只更新列表和托盘图标，不会自动拉取",
  },
];

const NOTIFY_SWITCHES: SwitchItem[] = [
  { key: "notifyOnError", label: "出错或冲突时", description: '如"2 个项目需要处理：A、B"，每轮只发一条' },
  { key: "notifyOnSuccess", label: "同步成功时", description: "只在确实推送或拉取了东西时发" },
];

/** 一行：左边文字，右边开关 */
function SwitchRow({
  item,
  checked,
  disabled,
  onChange,
}: {
  item: SwitchItem;
  checked: boolean;
  disabled: boolean;
  onChange: (value: boolean) => void;
}) {
  const id = useId();
  return (
    <div className="flex items-start justify-between gap-4 py-2">
      <label htmlFor={id} className="min-w-0 cursor-pointer">
        <div className="text-sm">{item.label}</div>
        {item.description && <p className="mt-0.5 text-xs text-muted-foreground">{item.description}</p>}
      </label>
      <Switch id={id} checked={checked} onCheckedChange={onChange} disabled={disabled} className="mt-0.5" />
    </div>
  );
}

/** 一张卡片：标题、说明和若干开关 */
function SwitchCard({ title, description, items }: { title: string; description?: string; items: SwitchItem[] }) {
  const { settings, updateSettings } = useSettings();

  async function toggle(key: SwitchKey, value: boolean) {
    try {
      await updateSettings({ [key]: value });
    } catch (e) {
      toast.error("保存失败", { description: e instanceof Error ? e.message : String(e) });
      // updateSettings 会先乐观更新，失败时重新读回已保存的值
      await updateSettings({}).catch(() => undefined);
    }
  }

  return (
    <div className="rounded-lg border bg-card p-4">
      <div className="text-sm font-medium">{title}</div>
      {description && <p className="mt-0.5 text-xs text-muted-foreground">{description}</p>}
      <div className="mt-2 divide-y">
        {items.map((item) => (
          <SwitchRow
            key={item.key}
            item={item}
            checked={settings?.[item.key] ?? false}
            disabled={!settings}
            onChange={(v) => void toggle(item.key, v)}
          />
        ))}
      </div>
    </div>
  );
}

/** 暂停自动同步到明天 00:00 / 恢复 */
function PauseCard() {
  const { settings, pauseAutoSync, resumeAutoSync } = useSettings();
  const [busy, setBusy] = useState(false);
  const until = activePause(settings);

  async function run(action: () => Promise<void>, done: string) {
    setBusy(true);
    try {
      await action();
      toast.success(done);
    } catch (e) {
      toast.error("操作失败", { description: e instanceof Error ? e.message : String(e) });
    } finally {
      setBusy(false);
    }
  }

  return (
    <div className="flex items-center justify-between gap-4 rounded-lg border bg-card p-4">
      <div className="min-w-0">
        <div className="text-sm font-medium">暂停自动同步</div>
        <p className="mt-0.5 text-xs text-muted-foreground">
          {until
            ? `已暂停到 ${describePauseUntil(until)}，期间离开和回到电脑时都不会自动同步（托盘里的「立即同步」照常可用）。`
            : "临时不想自动同步时，可以暂停到明天 00:00。托盘菜单里也能暂停和恢复。"}
        </p>
      </div>
      {until ? (
        <Button variant="outline" onClick={() => run(resumeAutoSync, "已恢复自动同步")} disabled={busy || !settings}>
          {busy ? <Loader2 className="animate-spin" /> : <Play />}
          恢复
        </Button>
      ) : (
        <Button variant="outline" onClick={() => run(pauseAutoSync, "自动同步已暂停到明天 00:00")} disabled={busy || !settings}>
          {busy ? <Loader2 className="animate-spin" /> : <Pause />}
          暂停到明天
        </Button>
      )}
    </div>
  );
}

/** 换电脑自动同步：后台运行、离开 / 到达时同步、定时检查、通知、暂停 */
function AutoSyncSection() {
  return (
    <section className="space-y-3">
      <h2 className="text-sm font-medium text-muted-foreground">自动同步</h2>
      <SwitchCard title="后台运行" items={BACKGROUND_SWITCHES} />
      <SwitchCard
        title="离开这台电脑时同步"
        description="只推送本机还没推上去的提交（打开了自动提交的项目先提交再推送）；没有要推的就完全不联网。"
        items={LEAVE_SWITCHES}
      />
      <SwitchCard
        title="回到这台电脑时同步"
        description="对所有项目跑一遍一键同步：拉取云端的新提交，补推上次没推完的。同一种自动同步 10 分钟内只做一次。"
        items={ARRIVE_SWITCHES}
      />
      <SwitchCard title="检查云端" items={CHECK_SWITCHES} />
      <SwitchCard
        title="通知"
        description="只在后台自动同步和托盘「立即同步」之后发；窗口里点的一键同步直接看结果。"
        items={NOTIFY_SWITCHES}
      />
      <PauseCard />
    </section>
  );
}

// ---------------- 自动提交 ----------------

/** 预览里电脑名的占位文字（还没读到设置时用） */
const HOST_PREVIEW = "本机电脑名";

/** 自动提交的提交信息模板：失焦或回车保存，空值恢复默认；下面是本机名称（模板里的 {host}） */
function CommitTemplateCard() {
  const { settings, updateSettings } = useSettings();
  const saved = settings?.commitTemplate ?? DEFAULT_COMMIT_TEMPLATE;
  const [draft, setDraft] = useState(saved);
  useEffect(() => setDraft(saved), [saved]);
  const savedHost = settings?.hostName ?? "";
  const [hostDraft, setHostDraft] = useState(savedHost);
  useEffect(() => setHostDraft(savedHost), [savedHost]);
  const hostPreview = hostDraft.trim() || settings?.defaultHostName || HOST_PREVIEW;

  async function saveHost(value: string) {
    const next = value.trim();
    if (next === savedHost) {
      setHostDraft(savedHost);
      return;
    }
    try {
      await updateSettings({ hostName: next });
      toast.success(next ? `本机名称已改为"${next}"` : "已改为使用电脑名");
    } catch (e) {
      setHostDraft(savedHost);
      toast.error("保存失败", { description: e instanceof Error ? e.message : String(e) });
      await updateSettings({}).catch(() => undefined);
    }
  }

  async function save(value: string) {
    const next = value.trim();
    if (next === saved) {
      setDraft(saved);
      return;
    }
    try {
      await updateSettings({ commitTemplate: next });
      toast.success(next && next !== DEFAULT_COMMIT_TEMPLATE ? "提交信息模板已保存" : "已恢复默认模板");
    } catch (e) {
      setDraft(saved);
      toast.error("保存失败", { description: e instanceof Error ? e.message : String(e) });
    }
  }

  return (
    <div className="rounded-lg border bg-card p-4">
      <div className="text-sm font-medium">提交信息模板</div>
      <p className="mt-0.5 text-xs text-muted-foreground">
        在项目详情里打开"一键同步时自动提交"的项目，一键同步时会用这个格式提交所有改动并推送。
      </p>
      <div className="mt-3 flex gap-2">
        <Input
          value={draft}
          onChange={(e) => setDraft(e.target.value)}
          onBlur={() => save(draft)}
          onKeyDown={(e) => e.key === "Enter" && e.currentTarget.blur()}
          placeholder={DEFAULT_COMMIT_TEMPLATE}
          disabled={!settings}
          className="font-mono"
          aria-label="提交信息模板"
        />
        <Button
          variant="outline"
          // 不让输入框先失焦保存一次草稿
          onMouseDown={(e) => e.preventDefault()}
          onClick={() => save(DEFAULT_COMMIT_TEMPLATE)}
          disabled={!settings || (saved === DEFAULT_COMMIT_TEMPLATE && draft === saved)}
        >
          <RotateCcw />
          恢复默认
        </Button>
      </div>
      <ul className="mt-3 space-y-1 text-xs text-muted-foreground">
        <li>
          <code className="rounded bg-muted px-1 py-px font-mono text-foreground/80">{"{date}"}</code>{" "}
          同步时的本地时间，如 2026-10-07 14:30
        </li>
        <li>
          <code className="rounded bg-muted px-1 py-px font-mono text-foreground/80">{"{host}"}</code>{" "}
          这台电脑的名字，就是下面的"本机名称"
        </li>
      </ul>
      <div className="mt-4 border-t pt-4">
        <label htmlFor="host-name" className="text-sm font-medium">
          本机名称
        </label>
        <p className="mt-0.5 text-xs text-muted-foreground">
          如"家里台式机""学校笔记本"，在 GitHub 上一眼看出是哪台电脑提交的；不填就用电脑名
          {settings?.defaultHostName ? ` ${settings.defaultHostName}` : ""}。
        </p>
        <Input
          id="host-name"
          value={hostDraft}
          onChange={(e) => setHostDraft(e.target.value)}
          onBlur={() => saveHost(hostDraft)}
          onKeyDown={(e) => e.key === "Enter" && e.currentTarget.blur()}
          placeholder={settings?.defaultHostName ?? ""}
          disabled={!settings}
          maxLength={40}
          className="mt-3 max-w-xs"
        />
      </div>
      <div className="mt-3 rounded-md bg-muted/50 px-3 py-2 text-xs">
        <span className="text-muted-foreground">预览：</span>
        <span className="font-mono break-all select-text">
          {renderCommitMessage(draft, new Date(), hostPreview)}
        </span>
      </div>
    </div>
  );
}

function ThemeCard({
  theme,
  label,
  active,
  onSelect,
}: {
  theme: Theme;
  label: string;
  active: boolean;
  onSelect: () => void;
}) {
  const dark = theme === "dark";
  return (
    <button
      type="button"
      role="radio"
      aria-checked={active}
      onClick={onSelect}
      className={cn(
        "group rounded-lg border p-2 text-left transition-colors",
        active ? "border-foreground/40 ring-2 ring-foreground/15" : "hover:border-foreground/20",
      )}
    >
      {/* 迷你预览：固定颜色，不随当前主题变化 */}
      <div
        className={cn(
          "flex h-20 overflow-hidden rounded-md border",
          dark ? "border-neutral-700 bg-neutral-900" : "border-neutral-200 bg-white",
        )}
      >
        <div className={cn("w-1/4 space-y-1 p-1.5", dark ? "bg-neutral-800" : "bg-neutral-100")}>
          {[0, 1, 2].map((i) => (
            <div key={i} className={cn("h-1.5 rounded-full", dark ? "bg-neutral-600" : "bg-neutral-300")} />
          ))}
        </div>
        <div className="flex-1 space-y-1.5 p-2">
          <div className={cn("h-2 w-1/2 rounded-full", dark ? "bg-neutral-500" : "bg-neutral-300")} />
          {["bg-emerald-500", "bg-blue-500", "bg-amber-500"].map((c) => (
            <div key={c} className="flex items-center gap-1.5">
              <div className={cn("h-1.5 flex-1 rounded-full", dark ? "bg-neutral-700" : "bg-neutral-200")} />
              <div className={cn("h-1.5 w-4 rounded-full opacity-70", c)} />
            </div>
          ))}
        </div>
      </div>
      <div className="mt-2 flex items-center justify-between px-0.5">
        <span className="text-sm">{label}</span>
        {active && <Check className="size-4" />}
      </div>
    </button>
  );
}


// ---------------- 关于与应用更新 ----------------

type UpdatePhase = "idle" | "checking" | "latest" | "available" | "installing" | "error";

function formatBytes(n: number): string {
  if (n >= 1024 * 1024) return `${(n / 1024 / 1024).toFixed(1)} MB`;
  return `${Math.max(1, Math.round(n / 1024))} KB`;
}

function AboutSection({ pendingUpdate }: { pendingUpdate: UpdateInfo | null }) {
  const [version, setVersion] = useState<string | null>(null);
  const [phase, setPhase] = useState<UpdatePhase>(pendingUpdate ? "available" : "idle");
  const [update, setUpdate] = useState<UpdateInfo | null>(pendingUpdate);
  const [error, setError] = useState<string | null>(null);
  const [progress, setProgress] = useState<UpdateProgress | null>(null);

  useEffect(() => {
    api.getAppVersion().then(setVersion, () => setVersion(null));
  }, []);

  // 启动时的静默检查可能在打开设置页之后才完成
  useEffect(() => {
    if (pendingUpdate) {
      setUpdate(pendingUpdate);
      setPhase((p) => (p === "idle" || p === "latest" ? "available" : p));
    }
  }, [pendingUpdate]);

  async function check() {
    setPhase("checking");
    setError(null);
    try {
      const info = await api.checkForUpdate();
      setUpdate(info);
      setPhase(info ? "available" : "latest");
    } catch (e) {
      setUpdate(null);
      setError(e instanceof Error ? e.message : String(e));
      setPhase("error");
    }
  }

  async function install() {
    setPhase("installing");
    setError(null);
    setProgress(null);
    try {
      await api.installUpdate(setProgress);
      // 真实环境下这里已经重启；演示模式不会真的更新，回到"有新版本"
      setPhase("available");
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
      setPhase("error");
    }
  }

  const percent = progress?.total ? Math.min(100, Math.round((progress.downloaded / progress.total) * 100)) : null;
  const busy = phase === "checking" || phase === "installing";
  const showUpdate = update !== null && (phase === "available" || phase === "installing" || phase === "error");

  return (
    <section className="space-y-3">
      <h2 className="text-sm font-medium text-muted-foreground">关于</h2>
      <div className="rounded-lg border bg-card p-4">
        <div className="flex items-center justify-between gap-4">
          <div>
            <div className="text-sm font-medium">Git SyncDeck</div>
            <p className="mt-0.5 text-xs text-muted-foreground">当前版本 {version ? `v${version}` : "未知"}</p>
          </div>
          <Button variant="outline" onClick={check} disabled={busy}>
            {phase === "checking" ? <Loader2 className="animate-spin" /> : <RefreshCw />}
            {phase === "checking" ? "检查中…" : "检查更新"}
          </Button>
        </div>

        {phase === "latest" && (
          <p className="mt-3 flex items-center gap-1.5 text-sm text-muted-foreground">
            <Check className="size-4" />
            已是最新版本
          </p>
        )}

        {phase === "error" && error && (
          <p className="mt-3 flex items-start gap-1.5 text-sm text-destructive">
            <CircleAlert className="mt-0.5 size-4 shrink-0" />
            <span className="min-w-0 break-words">检查或安装更新失败：{error}</span>
          </p>
        )}

        {showUpdate && update && (
          <div className="mt-4 space-y-3 border-t pt-4">
            <div className="flex items-center justify-between gap-4">
              <div>
                <div className="text-sm font-medium">发现新版本 v{update.version}</div>
                {update.date && (
                  <p className="mt-0.5 text-xs text-muted-foreground">
                    发布于 {new Date(update.date).toLocaleDateString("zh-CN")}
                  </p>
                )}
              </div>
              <Button onClick={install} disabled={busy}>
                {phase === "installing" ? <Loader2 className="animate-spin" /> : <Download />}
                {phase === "installing" ? "正在下载…" : "下载并安装"}
              </Button>
            </div>

            {update.notes && (
              <div>
                <div className="text-xs font-medium text-muted-foreground">更新说明</div>
                <p className="mt-1 max-h-40 overflow-y-auto text-sm whitespace-pre-wrap select-text">{update.notes}</p>
              </div>
            )}

            {phase === "installing" && (
              <div className="space-y-1.5">
                <Progress value={percent ?? 0} aria-label="下载进度" />
                <p className="text-xs text-muted-foreground">
                  {progress
                    ? progress.total
                      ? `已下载 ${formatBytes(progress.downloaded)} / ${formatBytes(progress.total)}（${percent}%），完成后会自动重启`
                      : `已下载 ${formatBytes(progress.downloaded)}，完成后会自动重启`
                    : "正在连接…"}
                </p>
              </div>
            )}
          </div>
        )}
      </div>
    </section>
  );
}
