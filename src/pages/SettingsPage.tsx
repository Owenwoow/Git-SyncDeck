import { useEffect, useId, useRef, useState } from "react";
import { Check, CircleAlert, Download, FolderSearch, Loader2, Pause, Play, Power, RefreshCw, RotateCcw } from "lucide-react";
import { toast } from "sonner";
import * as api from "@/api";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Progress } from "@/components/ui/progress";
import { Switch } from "@/components/ui/switch";
import {
  DEFAULT_BIG_FILE_LIMIT_MB,
  DEFAULT_COMMIT_TEMPLATE,
  MAX_BIG_FILE_LIMIT_MB,
  MERGE_MESSAGE_TEMPLATE,
  NETWORK_TIMEOUT_CHOICES,
  renderCommitMessage,
} from "@/lib/status";
import { cn } from "@/lib/utils";
import { useProjects, type SyncState } from "@/state/projects";
import { activePause, describePauseUntil, useSettings } from "@/state/settings";
import type { AutoSyncEvent, Project, ProxyMode, Settings, Theme, UpdateInfo, UpdateProgress } from "@/types";

// ---------------- 页面骨架：左边分类目录，右边只显示一类 ----------------

export type SettingsCategory = "general" | "network" | "autosync" | "commit" | "appearance" | "about";

const CATEGORIES: { id: SettingsCategory; label: string }[] = [
  { id: "general", label: "常规" },
  { id: "network", label: "网络" },
  { id: "autosync", label: "自动同步" },
  { id: "commit", label: "提交与合并" },
  { id: "appearance", label: "外观" },
  { id: "about", label: "关于" },
];

/** 一组设置项：圆角卡片，项与项之间有分隔线 */
function Group({ children }: { children: React.ReactNode }) {
  return <div className="divide-y rounded-lg border bg-card">{children}</div>;
}

/** 分组标题（可带一段说明），放在 Group 上面 */
function GroupHeading({ title, description }: { title: string; description?: string }) {
  return (
    <div className="mb-2 px-0.5">
      <h3 className="text-sm font-medium">{title}</h3>
      {description && <p className="mt-0.5 text-xs text-muted-foreground">{description}</p>}
    </div>
  );
}

/** 一行设置项：左边标题和说明，右边控件（control）；要填较长文字的放进 children，占满下一行 */
function Row({
  title,
  description,
  htmlFor,
  control,
  children,
}: {
  title: string;
  description?: React.ReactNode;
  htmlFor?: string;
  control?: React.ReactNode;
  children?: React.ReactNode;
}) {
  const heading = htmlFor ? (
    <label htmlFor={htmlFor} className="cursor-pointer text-sm font-medium">
      {title}
    </label>
  ) : (
    <div className="text-sm font-medium">{title}</div>
  );
  return (
    <div className="px-4 py-3.5">
      <div className="flex items-start justify-between gap-4">
        <div className="min-w-0 flex-1">
          {heading}
          {description && <div className="mt-0.5 text-xs text-muted-foreground">{description}</div>}
        </div>
        {control && <div className="shrink-0">{control}</div>}
      </div>
      {children && <div className="mt-3">{children}</div>}
    </div>
  );
}

/** 分段选择（联网超时、代理共用） */
function Segmented({ children, label }: { children: React.ReactNode; label: string }) {
  return (
    <div className="inline-flex rounded-md border p-0.5" role="radiogroup" aria-label={label}>
      {children}
    </div>
  );
}

/** 设置页：左边分类目录，点哪类右边只显示那一类。category 由 App 保存，切到别的页面再回来停在上次看的分类 */
export function SettingsPage({
  pendingUpdate = null,
  category,
  onCategoryChange,
}: {
  pendingUpdate?: UpdateInfo | null;
  category: SettingsCategory;
  onCategoryChange: (c: SettingsCategory) => void;
}) {
  return (
    <div className="mx-auto max-w-[1000px] space-y-6">
      <header>
        <h1 className="text-xl font-semibold tracking-tight">设置</h1>
      </header>

      <div className="flex items-start gap-6">
        <nav aria-label="设置分类" className="sticky top-0 w-32 shrink-0 space-y-0.5">
          {CATEGORIES.map(({ id, label }) => {
            const active = id === category;
            return (
              <button
                key={id}
                type="button"
                onClick={() => onCategoryChange(id)}
                aria-current={active ? "page" : undefined}
                className={cn(
                  "flex h-9 w-full items-center gap-2 rounded-md px-3 text-left text-sm transition-colors",
                  active
                    ? "bg-accent font-medium text-accent-foreground"
                    : "text-muted-foreground hover:bg-accent/60 hover:text-foreground",
                )}
              >
                <span>{label}</span>
                {id === "about" && pendingUpdate && (
                  <span className="size-2 rounded-full bg-status-ahead" aria-label="有新版本" />
                )}
              </button>
            );
          })}
        </nav>

        <div className="min-w-0 flex-1 space-y-6">
          {category === "general" && <GeneralSection />}
          {category === "network" && (
            <>
              <Group>
                <NetworkTimeoutCard />
              </Group>
              <Group>
                <ProxyCard />
              </Group>
            </>
          )}
          {category === "autosync" && <AutoSyncSection />}
          {category === "commit" && <CommitAndMergeSection />}
          {category === "appearance" && <AppearanceSection />}
          {category === "about" && <AboutSection pendingUpdate={pendingUpdate} />}
        </div>
      </div>
    </div>
  );
}

function GeneralSection() {
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
    <Group>
      <Row title="默认代码目录" description={'"添加项目"页面会默认扫描这个目录。'}>
        <div className="flex gap-2">
          <Input readOnly value={settings?.defaultCodeDir ?? ""} className="font-mono select-text" />
          <Button variant="outline" onClick={pickDir} disabled={picking || !settings}>
            {picking ? <Loader2 className="animate-spin" /> : <FolderSearch />}
            选择…
          </Button>
        </div>
      </Row>
    </Group>
  );
}

function AppearanceSection() {
  const { settings, updateSettings } = useSettings();
  return (
    <Group>
      <Row title="主题">
        <div className="grid max-w-md grid-cols-2 gap-3" role="radiogroup" aria-label="主题">
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
      </Row>
    </Group>
  );
}

// ---------------- 网络 ----------------

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
    <Row
      title="联网超时"
      description="一键同步、提交并推送时连接 GitHub 超过这个时间就停止（刷新状态只是看一眼，最多等 30 秒）。网络慢或者一次要拉取的内容很多时，可以调长一些。"
      control={
        <Segmented label="联网超时">
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
        </Segmented>
      }
    />
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
    <Row
      title="代理"
      description="自动：git 自己配了代理就用 git 的；没配而 Windows 系统代理开着，就用系统代理。只对 SyncDeck 调用的 git 生效，不改全局 git 配置。"
      control={
        <Segmented label="代理">
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
        </Segmented>
      }
    >
      {mode === "manual" && (
        <Input
          ref={inputRef}
          value={url}
          onChange={(e) => setDraft(e.target.value)}
          onBlur={() => void saveUrl()}
          onKeyDown={(e) => e.key === "Enter" && e.currentTarget.blur()}
          placeholder="如 http://127.0.0.1:7890 或 socks5://127.0.0.1:1080"
          disabled={!settings || saving}
          className="mb-3 font-mono"
          aria-label="代理地址"
        />
      )}
      <p className="text-sm break-all">{current ? describeProxy(current) : "当前：正在检测…"}</p>
      <p className="mt-1 text-xs text-muted-foreground">
        只影响 https 地址的仓库，SSH 地址的仓库不受影响；仓库里单独配置的 remote.&lt;远程名&gt;.proxy 也不受这里影响
      </p>
    </Row>
  );
}

// ---------------- 自动同步 ----------------

type SwitchKey =
  | "launchAtLogin"
  | "closeToTray"
  | "syncOnLock"
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
    description: "关掉窗口后继续在托盘里运行；要彻底退出，用下面的「退出 SyncDeck」，或在托盘图标上右键选「退出」",
  },
];

const LEAVE_SWITCHES: SwitchItem[] = [{ key: "syncOnLock", label: "锁屏" }];

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
    <Row
      title={item.label}
      description={item.description}
      htmlFor={id}
      control={<Switch id={id} checked={checked} onCheckedChange={onChange} disabled={disabled} className="mt-0.5" />}
    />
  );
}

/** 一组开关：上面是标题和说明，下面每个开关一行；extra 是接在最后的其他行 */
function SwitchGroup({
  title,
  description,
  items,
  extra,
}: {
  title: string;
  description?: string;
  items: SwitchItem[];
  extra?: React.ReactNode;
}) {
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
    <div>
      <GroupHeading title={title} description={description} />
      <Group>
        {items.map((item) => (
          <SwitchRow
            key={item.key}
            item={item}
            checked={settings?.[item.key] ?? false}
            disabled={!settings}
            onChange={(v) => void toggle(item.key, v)}
          />
        ))}
        {extra}
      </Group>
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
    <Group>
      <Row
        title="暂停自动同步"
        description={
          until
            ? `已暂停到 ${describePauseUntil(until)}，期间离开和回到电脑时都不会自动同步（托盘里的「立即同步」照常可用）。`
            : "临时不想自动同步时，可以暂停到明天 00:00。托盘菜单里也能暂停和恢复。"
        }
        control={
          until ? (
            <Button variant="outline" onClick={() => run(resumeAutoSync, "已恢复自动同步")} disabled={busy || !settings}>
              {busy ? <Loader2 className="animate-spin" /> : <Play />}
              恢复
            </Button>
          ) : (
            <Button
              variant="outline"
              onClick={() => run(pauseAutoSync, "自动同步已暂停到明天 00:00")}
              disabled={busy || !settings}
            >
              {busy ? <Loader2 className="animate-spin" /> : <Pause />}
              暂停到明天
            </Button>
          )
        }
      />
    </Group>
  );
}

/**
 * 现在是否在忙：一键同步进行中、后台自动同步进行中、有项目处于"同步中"、正在从 GitHub 克隆。
 */
function isAppBusy(sync: SyncState, autoSync: AutoSyncEvent | null, projects: Project[], cloning: boolean): boolean {
  return sync.phase === "running" || autoSync !== null || cloning || projects.some((p) => p.status === "syncing");
}

/** 「退出 SyncDeck」：没在做事直接退出，正在同步先确认；演示模式只弹提示 */
function QuitRow() {
  const { sync, autoSync, projects, cloning } = useProjects();
  const [confirming, setConfirming] = useState(false);

  async function quit() {
    setConfirming(false);
    try {
      await api.quitApp();
    } catch (e) {
      toast.error("退出失败", { description: e instanceof Error ? e.message : String(e) });
    }
  }

  function onClick() {
    if (api.isDemoMode) {
      toast.info("演示模式不会真的退出");
      return;
    }
    if (isAppBusy(sync, autoSync, projects, cloning)) setConfirming(true);
    else void quit();
  }

  return (
    <>
      <Row
        title="退出 SyncDeck"
        description="彻底退出程序，托盘图标也会消失，之后不再自动同步，直到下次启动。"
        control={
          <Button variant="outline" onClick={onClick}>
            <Power />
            退出 SyncDeck
          </Button>
        }
      />
      <Dialog open={confirming} onOpenChange={setConfirming}>
        <DialogContent showCloseButton={false}>
          <DialogHeader>
            <DialogTitle>退出 SyncDeck</DialogTitle>
            <DialogDescription>退出会中断正在进行的同步或克隆，确定退出吗？</DialogDescription>
          </DialogHeader>
          <DialogFooter>
            <Button variant="outline" onClick={() => setConfirming(false)}>
              取消
            </Button>
            <Button variant="destructive" onClick={() => void quit()}>
              退出
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </>
  );
}

/** 换电脑自动同步：后台运行、离开 / 到达时同步、检查云端、通知、暂停 */
function AutoSyncSection() {
  return (
    <div className="space-y-6">
      <SwitchGroup title="后台运行" items={BACKGROUND_SWITCHES} extra={<QuitRow />} />
      <SwitchGroup
        title="离开这台电脑时同步"
        description="只推送本机还没推上去的提交（打开了自动提交的项目先提交再推送）；没有要推的就完全不联网。关机和睡眠前不同步，没推上去的下次开机、唤醒或解锁时补上。"
        items={LEAVE_SWITCHES}
      />
      <SwitchGroup
        title="回到这台电脑时同步"
        description="对所有项目跑一遍一键同步：拉取云端的新提交，补推上次没推完的。同一种自动同步 10 分钟内只做一次。"
        items={ARRIVE_SWITCHES}
      />
      <SwitchGroup title="检查云端" items={CHECK_SWITCHES} />
      <SwitchGroup
        title="通知"
        description="只在后台自动同步和托盘「立即同步」之后发；窗口里点的一键同步直接看结果。"
        items={NOTIFY_SWITCHES}
      />
      <PauseCard />
    </div>
  );
}

// ---------------- 提交与合并 ----------------

/** 预览里电脑名的占位文字（还没读到设置时用） */
const HOST_PREVIEW = "本机电脑名";

function CommitAndMergeSection() {
  return (
    <div className="space-y-6">
      <CommitTemplateCard />
      <MergeAndCheckCard />
    </div>
  );
}

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
    <div>
      <GroupHeading title="自动提交" />
      <Group>
        <Row
          title="提交信息模板"
          description={'在项目详情里打开"一键同步时自动提交"的项目，一键同步时会用这个格式提交所有改动并推送。'}
        >
          <div className="flex gap-2">
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
          <div className="mt-3 rounded-md bg-muted/50 px-3 py-2 text-xs">
            <span className="text-muted-foreground">预览：</span>
            <span className="font-mono break-all select-text">
              {renderCommitMessage(draft, new Date(), hostPreview)}
            </span>
          </div>
        </Row>
        <Row
          title="本机名称"
          htmlFor="host-name"
          description={`如"家里台式机""学校笔记本"，在 GitHub 上一眼看出是哪台电脑提交的；不填就用电脑名${
            settings?.defaultHostName ? ` ${settings.defaultHostName}` : ""
          }。`}
        >
          <Input
            id="host-name"
            value={hostDraft}
            onChange={(e) => setHostDraft(e.target.value)}
            onBlur={() => saveHost(hostDraft)}
            onKeyDown={(e) => e.key === "Enter" && e.currentTarget.blur()}
            placeholder={settings?.defaultHostName ?? ""}
            disabled={!settings}
            maxLength={40}
          />
        </Row>
      </Group>
    </div>
  );
}

/** 冲突与提交检查：无冲突时自动合并、自动提交前检查、大文件上限。勾选即保存；上限失焦或回车保存 */
function MergeAndCheckCard() {
  const { settings, updateSettings } = useSettings();
  const savedLimit = settings?.bigFileLimitMb ?? DEFAULT_BIG_FILE_LIMIT_MB;
  const [limitDraft, setLimitDraft] = useState(String(savedLimit));
  useEffect(() => setLimitDraft(String(savedLimit)), [savedLimit]);

  async function save(patch: Partial<Settings>, success: string) {
    try {
      await updateSettings(patch);
      toast.success(success);
    } catch (e) {
      toast.error("保存失败", { description: e instanceof Error ? e.message : String(e) });
      // updateSettings 会先乐观更新，失败时重新读回已保存的值
      await updateSettings({}).catch(() => undefined);
    }
  }

  async function saveLimit() {
    const text = limitDraft.trim();
    const mb = Number(text);
    if (text === String(savedLimit)) return;
    if (!/^\d+$/.test(text) || mb < 1 || mb > MAX_BIG_FILE_LIMIT_MB) {
      toast.error("保存失败", { description: `大文件上限要填 1 ~ ${MAX_BIG_FILE_LIMIT_MB} 之间的整数（MB）` });
      setLimitDraft(String(savedLimit));
      return;
    }
    await save({ bigFileLimitMb: mb }, `大文件上限已改为 ${mb} MB`);
  }

  const autoMerge = settings?.autoMerge ?? true;
  const precommitCheck = settings?.precommitCheck ?? true;

  return (
    <div>
      <GroupHeading title="冲突与提交检查" />
      <Group>
        <Row
          title="无冲突时自动合并"
          htmlFor="setting-auto-merge"
          description={
            <div className="space-y-1">
              <p>
                本地和云端分叉时，一键同步先在内存里预演合并（不碰工作区）：没有冲突就自动合并并推送；有冲突就什么都不动，项目标红，可以在详情里「复制给
                AI」。关掉后分叉照旧记为异常。
              </p>
              <p>
                合并提交信息：
                <span className="font-mono text-foreground/80 select-text">
                  {MERGE_MESSAGE_TEMPLATE.split("{host}").join(HOST_PREVIEW)}
                </span>
              </p>
            </div>
          }
          control={
            <Checkbox
              id="setting-auto-merge"
              checked={autoMerge}
              disabled={!settings}
              onCheckedChange={(c) =>
                save({ autoMerge: c === true }, c === true ? "已打开无冲突时自动合并" : "已关闭自动合并")
              }
              className="mt-px"
            />
          }
        />
        <Row
          title="提交前检查"
          htmlFor="setting-precommit-check"
          description="自动提交前检查要提交的文件：有超过上限的大文件，或者没被 .gitignore 忽略的疑似密钥文件（.env、*.pem、*.key、id_rsa 等），这次就不提交，记为异常并列出文件。手动“提交并推送”不检查。"
          control={
            <Checkbox
              id="setting-precommit-check"
              checked={precommitCheck}
              disabled={!settings}
              onCheckedChange={(c) =>
                save({ precommitCheck: c === true }, c === true ? "已打开提交前检查" : "已关闭提交前检查")
              }
              className="mt-px"
            />
          }
        />
        <Row
          title="大文件上限（MB）"
          htmlFor="setting-big-file-limit"
          description="超过这个大小的文件不会被自动提交；需要先打开上面的“提交前检查”。"
          control={
            <Input
              id="setting-big-file-limit"
              inputMode="numeric"
              value={limitDraft}
              onChange={(e) => setLimitDraft(e.target.value)}
              onBlur={() => void saveLimit()}
              onKeyDown={(e) => e.key === "Enter" && e.currentTarget.blur()}
              disabled={!settings || !precommitCheck}
              className="h-8 w-24 font-mono"
            />
          }
        />
      </Group>
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
    <Group>
      <Row
        title="Git SyncDeck"
        description={`当前版本 ${version ? `v${version}` : "未知"}`}
        control={
          <Button variant="outline" onClick={check} disabled={busy}>
            {phase === "checking" ? <Loader2 className="animate-spin" /> : <RefreshCw />}
            {phase === "checking" ? "检查中…" : "检查更新"}
          </Button>
        }
      />

      {phase === "latest" && (
        <p className="flex items-center gap-1.5 px-4 py-3 text-sm text-muted-foreground">
          <Check className="size-4" />
          已是最新版本
        </p>
      )}

      {phase === "error" && error && (
        <p className="flex items-start gap-1.5 px-4 py-3 text-sm text-destructive">
          <CircleAlert className="mt-0.5 size-4 shrink-0" />
          <span className="min-w-0 break-words">检查或安装更新失败：{error}</span>
        </p>
      )}

      {showUpdate && update && (
        <Row
          title={`发现新版本 v${update.version}`}
          description={update.date ? `发布于 ${new Date(update.date).toLocaleDateString("zh-CN")}` : undefined}
          control={
            <Button onClick={install} disabled={busy}>
              {phase === "installing" ? <Loader2 className="animate-spin" /> : <Download />}
              {phase === "installing" ? "正在下载…" : "下载并安装"}
            </Button>
          }
        >
          <div className="space-y-3">
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
        </Row>
      )}
    </Group>
  );
}
