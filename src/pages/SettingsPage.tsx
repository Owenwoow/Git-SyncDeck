import { useEffect, useId, useState, type KeyboardEvent, type ReactNode } from "react";
import { Check, ChevronDown, CircleAlert, Download, ExternalLink, FolderSearch, Loader2, Pause, Play, Power, RefreshCw, RotateCcw } from "lucide-react";
import { toast } from "sonner";
import * as api from "@/api";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Progress } from "@/components/ui/progress";
import { Switch } from "@/components/ui/switch";
import {
  DEFAULT_COMMIT_TEMPLATE, MAX_BIG_FILE_LIMIT_MB, MERGE_MESSAGE_TEMPLATE,
  NETWORK_TIMEOUT_CHOICES, renderCommitMessage,
} from "@/lib/status";
import { cn } from "@/lib/utils";
import { activePause, describePauseUntil, useSettings } from "@/state/settings";
import { useTasks } from "@/state/tasks";
import { useUpdates } from "@/state/updates";
import type { GitInfo, ProxyMode, Settings } from "@/types";

export type SettingsCategory = "general" | "sync" | "network" | "about";
const CATEGORIES: { id: SettingsCategory; label: string }[] = [
  { id: "general", label: "常规" }, { id: "sync", label: "同步" },
  { id: "network", label: "网络与诊断" }, { id: "about", label: "关于与更新" },
];
const errorMessage = (error: unknown) => error instanceof Error ? error.message : String(error);

function radioKeys(event: KeyboardEvent<HTMLDivElement>) {
  if (!["ArrowLeft", "ArrowRight", "Home", "End"].includes(event.key)) return;
  const choices = [...event.currentTarget.querySelectorAll<HTMLButtonElement>('button[role="radio"]:not(:disabled)')];
  if (!choices.length) return;
  const current = choices.indexOf(document.activeElement as HTMLButtonElement);
  const index = event.key === "Home" ? 0 : event.key === "End" ? choices.length - 1
    : (current + (event.key === "ArrowLeft" ? -1 : 1) + choices.length) % choices.length;
  event.preventDefault();
  choices[index].focus();
  choices[index].click();
}

function Group({ children }: { children: ReactNode }) {
  return <div className="divide-y rounded-lg border bg-card">{children}</div>;
}
function GroupHeading({ title, description }: { title: string; description?: string }) {
  return <div className="mb-2 px-0.5"><h3 className="text-sm font-medium">{title}</h3>
    {description && <p className="mt-1 text-xs leading-relaxed text-muted-foreground">{description}</p>}</div>;
}
function Row({ title, description, htmlFor, control, children }: {
  title: string; description?: ReactNode; htmlFor?: string; control?: ReactNode; children?: ReactNode;
}) {
  return <div className="px-4 py-3.5">
    <div className="flex flex-wrap items-start justify-between gap-x-4 gap-y-2">
      <div className="min-w-0 flex-1 basis-40">
        {htmlFor ? <label htmlFor={htmlFor} className="cursor-pointer text-sm font-medium">{title}</label>
          : <div className="text-sm font-medium">{title}</div>}
        {description && <div className="mt-1 text-xs leading-relaxed text-muted-foreground">{description}</div>}
      </div>
      {control && <div className="max-w-full shrink-0">{control}</div>}
    </div>
    {children && <div className="mt-3">{children}</div>}
  </div>;
}
function FieldFeedback({ field }: { field: keyof Settings }) {
  const { fieldStates } = useSettings();
  const state = fieldStates[field];
  if (!state) return null;
  return <span className={cn("mt-1 block text-xs", state === "error" ? "text-destructive" : "text-muted-foreground")}>
    {state === "saving" ? "正在保存…" : state === "saved" ? "已保存" : "保存失败，已恢复原值；可在上方重试"}
  </span>;
}
function SaveFeedback() {
  const { settings, loading, saveStatus, error, readError, reloadSettings, retryFailedSave, updateSettings } = useSettings();
  const loadFailure = readError || settings?.configReadError;
  return <div className="space-y-2 text-xs" aria-live="polite">
    {loadFailure ? <div role="alert" className="rounded-md border border-destructive/30 bg-destructive/5 px-3 py-2.5">
      <p className="font-medium text-destructive">无法读取已保存的设置</p>
      <p className="mt-1 break-words text-destructive">{loadFailure}</p>
      <p className="mt-1 text-muted-foreground">为保留已有数据，暂时不能保存设置。请处理原因后重新读取；配置文件读取错误可能需要重启应用。</p>
      <Button variant="outline" size="sm" className="mt-2" disabled={loading} onClick={() => void reloadSettings().catch(() => {})}>
        <RefreshCw className={loading ? "animate-spin" : ""} />重新读取
      </Button>
    </div> : loading ? <p role="status" className="flex items-center gap-1.5 text-muted-foreground"><Loader2 className="size-3 animate-spin" />正在读取设置…</p>
      : <p role="status" className="flex items-center gap-1.5 text-muted-foreground">
        {saveStatus === "saving" ? <Loader2 className="size-3 animate-spin" /> : <Check className="size-3" />}
        {saveStatus === "saving" ? "正在保存设置…" : saveStatus === "saved" ? "设置已保存" : "开关即时保存；文本失焦或回车保存；代理需点击“应用”"}
      </p>}
    {error && <div role="alert" className="rounded-md border border-destructive/30 px-3 py-2 text-destructive">
      <p className="break-words">保存失败：{error}</p><p className="mt-1">失败项已恢复为已保存值，其他修改仍会继续保存。</p>
      <Button variant="outline" size="sm" className="mt-2" disabled={saveStatus === "saving"}
        onClick={() => void retryFailedSave().catch(() => {})}>重试保存</Button>
    </div>}
    {settings?.runtimeWarning && <div role="alert" className="rounded-md border border-status-ahead/30 px-3 py-2">
      <p className="break-words text-status-ahead">开机启动状态需要处理：{settings.runtimeWarning}</p>
      <Button variant="outline" size="sm" className="mt-2" disabled={saveStatus === "saving" || !!loadFailure}
        onClick={() => void updateSettings({ launchAtLogin: settings.launchAtLogin }).catch(() => {})}>重试应用开机启动设置</Button>
    </div>}
  </div>;
}

export function SettingsPage({ category, onCategoryChange }: {
  category: SettingsCategory; onCategoryChange: (category: SettingsCategory) => void;
}) {
  const { settings, readError } = useSettings();
  const { update } = useUpdates();
  const disabled = !settings || !!readError || !!settings.configReadError;
  return <div className="mx-auto max-w-[1000px] space-y-5">
    <header><h1 className="text-xl font-semibold tracking-tight">设置</h1></header>
    <div className="flex items-start gap-4 wide:gap-6">
      <nav aria-label="设置分类" className="sticky top-0 w-28 shrink-0 space-y-0.5 wide:w-32">
        {CATEGORIES.map(({ id, label }) => <button key={id} type="button" onClick={() => onCategoryChange(id)}
          aria-current={category === id ? "page" : undefined}
          className={cn("flex min-h-9 w-full items-center gap-1.5 rounded-md px-2.5 py-2 text-left text-sm leading-5 transition-colors",
            category === id ? "bg-accent font-medium text-accent-foreground" : "text-muted-foreground hover:bg-accent/60 hover:text-foreground")}>
          <span>{label}</span>{id === "about" && update && <span className="size-1.5 shrink-0 rounded-full bg-status-ahead" aria-label="有新版本" />}
        </button>)}
      </nav>
      <div className="min-w-0 flex-1 space-y-5">
        <SaveFeedback />
        {category === "general" && <><fieldset disabled={disabled} className="min-w-0 space-y-5"><GeneralSection /></fieldset><Group><QuitRow /></Group></>}
        {category === "sync" && <fieldset disabled={disabled} className="min-w-0"><SyncSection /></fieldset>}
        {category === "network" && <><fieldset disabled={disabled} className="min-w-0 space-y-5"><Group><NetworkTimeoutCard /></Group><Group><ProxyCard /></Group></fieldset><DiagnosticsCard /></>}
        {category === "about" && <AboutSection />}
      </div>
    </div>
  </div>;
}

type SwitchKey = "launchAtLogin" | "closeToTray" | "autoSyncEnabled" | "syncOnLock" | "syncOnStartup" | "syncOnResume" | "syncOnUnlock" | "periodicCheck" | "notifyOnSuccess" | "notifyOnError" | "autoMerge" | "precommitCheck";
type SwitchItem = { key: SwitchKey; label: string; description?: ReactNode };
function SwitchSetting({ item }: { item: SwitchItem }) {
  const { settings, updateSettings, fieldStates } = useSettings();
  const id = useId();
  return <Row title={item.label} description={item.description} htmlFor={id} control={
    <Switch id={id} checked={settings?.[item.key] ?? false} disabled={!settings}
      onCheckedChange={(value) => void updateSettings({ [item.key]: value }).catch(() => {})} className="mt-0.5" />
  }>{fieldStates[item.key] && <FieldFeedback field={item.key} />}</Row>;
}
function SwitchGroup({ title, description, items }: { title: string; description?: string; items: SwitchItem[] }) {
  return <div><GroupHeading title={title} description={description} /><Group>{items.map((item) => <SwitchSetting key={item.key} item={item} />)}</Group></div>;
}

function GeneralSection() {
  const { settings, updateSettings } = useSettings();
  const [picking, setPicking] = useState(false);
  async function pickDir() {
    setPicking(true);
    try {
      const path = await api.pickDirectory(settings?.defaultCodeDir);
      if (path) await updateSettings({ defaultCodeDir: path });
    } catch (error) { toast.error("无法更改默认代码目录", { description: errorMessage(error) }); }
    finally { setPicking(false); }
  }
  return <>
    <Group>
      <SwitchSetting item={{ key: "launchAtLogin", label: "开机启动", description: "开机后在托盘运行，不弹出窗口。开机启动本身不授权自动提交、推送或拉取。" }} />
      <SwitchSetting item={{ key: "closeToTray", label: "关闭窗口后留在托盘", description: "关闭窗口时继续后台运行；是否自动操作仓库由“同步”里的规则控制。彻底退出请使用下方或托盘中的退出入口。" }} />
      <Row title="默认代码目录" description="“添加项目”页面默认扫描这里。">
        <div className="flex gap-2"><Input readOnly value={settings?.defaultCodeDir ?? ""} className="font-mono select-text" aria-label="默认代码目录" />
          <Button variant="outline" onClick={() => void pickDir()} disabled={picking || !settings}>{picking ? <Loader2 className="animate-spin" /> : <FolderSearch />}选择…</Button></div>
        <FieldFeedback field="defaultCodeDir" />
      </Row>
      <Row title="主题">
        <div className="grid max-w-md grid-cols-2 gap-3" role="radiogroup" aria-label="主题" onKeyDown={radioKeys}>
          {(["light", "dark"] as const).map((theme) => <button key={theme} type="button" role="radio" aria-checked={settings?.theme === theme}
            disabled={!settings} onClick={() => void updateSettings({ theme }).catch(() => {})}
            className={cn("rounded-lg border p-3 text-left text-sm transition-colors", settings?.theme === theme ? "border-foreground/40 ring-2 ring-foreground/15" : "hover:border-foreground/20")}>
            <div className={cn("mb-2 flex h-16 gap-2 overflow-hidden rounded border p-2", theme === "dark" ? "border-neutral-700 bg-neutral-900" : "border-neutral-200 bg-white")}>
              <div className={cn("w-1/4 rounded", theme === "dark" ? "bg-neutral-700" : "bg-neutral-200")} />
              <div className="flex-1 space-y-2 py-1">{[0, 1, 2].map((i) => <div key={i} className={cn("h-1.5 rounded", theme === "dark" ? "bg-neutral-600" : "bg-neutral-300")} />)}</div>
            </div><span className="flex items-center justify-between">{theme === "light" ? "浅色" : "深色"}{settings?.theme === theme && <Check className="size-4" />}</span>
          </button>)}
        </div><FieldFeedback field="theme" />
      </Row>
    </Group>
  </>;
}
function QuitRow() {
  const { requestQuit } = useTasks();
  return <Row title="退出应用" description="彻底退出后，托盘图标消失，仓库自动操作停止，直到下次启动。若有任务正在执行或等待，会先提示确认。"
    control={<Button variant="outline" onClick={() => void requestQuit()}><Power />退出 SyncDeck</Button>} />;
}

function SyncSection() {
  const { settings } = useSettings();
  const triggers = settings && (settings.syncOnLock || settings.syncOnStartup || settings.syncOnResume || settings.syncOnUnlock);
  return <div className="space-y-5">
    <Group><SwitchSetting item={{ key: "autoSyncEnabled", label: "自动同步", description: "只控制启动、锁屏、唤醒和解锁时自动发起的仓库同步。关闭后仍可手动刷新、一键同步、提交并推送和克隆；重新开启不会立即补跑。" }} /></Group>
    {settings?.autoSyncEnabled && <div className="space-y-5">
      {!triggers && <p role="status" className="rounded-md bg-muted px-3 py-2 text-sm">自动同步已开启，但未选择触发时机。</p>}
      <SwitchGroup title="离开这台电脑时同步" description="只处理本机待推送的提交，以及已开启自动提交的项目改动；没有要推送的内容时不联网。关机和睡眠前不同步。"
        items={[{ key: "syncOnLock", label: "锁屏" }]} />
      <SwitchGroup title="回到这台电脑时同步" description="按一键同步规则处理当前分支；同类触发间隔至少 10 分钟。启动或唤醒同步遇到联网失败时，15 秒后重试一次；暂停会阻止重试。"
        items={[{ key: "syncOnStartup", label: "启动 SyncDeck 时", description: "包括开机启动" }, { key: "syncOnResume", label: "从睡眠唤醒" }, { key: "syncOnUnlock", label: "解锁" }]} />
    </div>}
    {!settings?.autoSyncEnabled && <p className="text-xs leading-relaxed text-muted-foreground">触发时机已收起并保留原值。开机启动和托盘常驻可继续使用；下方只读检查仍可单独开启。</p>}
    <SwitchGroup title="只读状态检查" items={[{ key: "periodicCheck", label: "定时检查云端", description: "每 30 分钟仅 fetch/status，更新列表和托盘；不提交、不推送、不拉取工作区。与自动同步总开关独立，临时暂停期间不检查。" }]} />
    {!settings?.autoSyncEnabled && settings?.periodicCheck && <p role="status" className="text-sm text-status-ahead">自动同步已关闭；定时检查云端仍已开启。</p>}
    <PauseCard />
    <SwitchGroup title="通知" description="后台同步和托盘“立即同步”的结果通知；窗口内手动一键同步直接查看结果。"
      items={[{ key: "notifyOnError", label: "出错或冲突时通知", description: "每轮汇总一条通知" }, { key: "notifyOnSuccess", label: "同步成功时通知", description: "仅在确实提交、推送或拉取了内容时" }]} />
    <details className="group rounded-lg border bg-card">
      <summary className="flex cursor-pointer list-none items-center justify-between gap-2 rounded-lg px-4 py-3.5 text-sm font-medium focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring">
        提交与合并<ChevronDown className="size-4 transition-transform group-open:rotate-180" />
      </summary>
      <div className="space-y-5 border-t p-4">
        <p className="text-xs leading-relaxed text-muted-foreground">这些规则也用于手动一键同步。逐项目自动提交请在首页打开项目详情，启用“同步时自动提交全部改动”；手动一键同步和后台自动同步都适用，它不是独立的定时上传功能。</p>
        <CommitTemplateCard /><MergeAndCheckCard />
      </div>
    </details>
  </div>;
}
function PauseCard() {
  const { settings, pauseAutoSync, resumeAutoSync } = useSettings();
  const { tasks } = useTasks();
  const [now, setNow] = useState(Date.now());
  const [busy, setBusy] = useState(false);
  const until = activePause(settings, now);
  useEffect(() => {
    if (!until) return;
    const timer = window.setTimeout(() => setNow(Date.now()), Math.min(until.getTime() - Date.now() + 500, 2 ** 31 - 1));
    return () => window.clearTimeout(timer);
  }, [until?.getTime()]);
  const finishing = until && tasks.active?.automatic;
  async function run(action: () => Promise<void>) {
    setBusy(true);
    try { await action(); } catch { /* 全局保存反馈保留原因和重试入口。 */ }
    finally { setBusy(false); }
  }
  return <Group><Row title="临时暂停仓库自动操作" description={until
    ? <>{`自动同步和自动检查已暂停至 ${describePauseUntil(until)}。`}{finishing && "当前任务完成后暂停。"}手动操作仍可用；恢复仅恢复原有规则，不立即发起同步。</>
    : "暂停自动同步、定时检查及失败重试；已开始的一轮允许完成。使用本机本地时间并在重启后保留，应用更新检查不受影响。"}>
    <div className="flex flex-wrap gap-2">
      {until ? <Button variant="outline" disabled={busy || !settings} onClick={() => void run(resumeAutoSync)}><Play />恢复原有规则</Button>
        : <><Button variant="outline" disabled={busy || !settings} onClick={() => void run(() => pauseAutoSync("hour"))}><Pause />暂停 1 小时</Button>
          <Button variant="outline" disabled={busy || !settings} onClick={() => void run(() => pauseAutoSync("tomorrow"))}>到明天 00:00</Button></>}
      {busy && <Loader2 className="size-4 animate-spin self-center" />}
    </div><FieldFeedback field="autoSyncPausedUntil" />
  </Row></Group>;
}

function TextSetting({ field, title, description, placeholder, maxLength, number, disabled, preview }: {
  field: "commitTemplate" | "hostName" | "bigFileLimitMb"; title: string; description: ReactNode;
  placeholder?: string; maxLength?: number; number?: boolean; disabled?: boolean; preview?: (value: string) => ReactNode;
}) {
  const { settings, updateSettings } = useSettings();
  const id = useId();
  const saved = String(settings?.[field] ?? "");
  const [draft, setDraft] = useState<string | null>(null);
  const [validation, setValidation] = useState<string | null>(null);
  const value = draft ?? saved;
  async function save(submitted = value) {
    const next = submitted.trim();
    if (next === saved) { setDraft(null); return; }
    if (number && (!/^\d+$/.test(next) || Number(next) < 1 || Number(next) > MAX_BIG_FILE_LIMIT_MB)) {
      setValidation(`请输入 1～${MAX_BIG_FILE_LIMIT_MB} 之间的整数（MB）`); return;
    }
    setValidation(null);
    try { await updateSettings({ [field]: number ? Number(next) : next }); }
    catch { /* 状态层已回退并保留重试。 */ }
    finally { setDraft((current) => current === submitted ? null : current); }
  }
  return <Row title={title} description={description} htmlFor={id}>
    <div className="flex flex-wrap gap-2">
      <Input id={id} value={value} onChange={(event) => { setDraft(event.target.value); setValidation(null); }}
        onBlur={() => void save()} onKeyDown={(event) => event.key === "Enter" && event.currentTarget.blur()}
        disabled={!settings || disabled} placeholder={placeholder} maxLength={maxLength} inputMode={number ? "numeric" : undefined}
        aria-invalid={!!validation} aria-describedby={validation ? `${id}-validation` : undefined}
        className={cn(number ? "w-24 font-mono" : "min-w-40 flex-1", field === "commitTemplate" && "font-mono")} />
      {field === "commitTemplate" && <Button variant="outline" disabled={!settings || value === DEFAULT_COMMIT_TEMPLATE}
        onMouseDown={(event) => event.preventDefault()} onClick={() => { setDraft(DEFAULT_COMMIT_TEMPLATE); void save(DEFAULT_COMMIT_TEMPLATE); }}><RotateCcw />恢复默认</Button>}
    </div>
    {validation && <p id={`${id}-validation`} role="alert" className="mt-1 text-xs text-destructive">{validation}</p>}
    {draft !== null && value !== saved && !validation && <p className="mt-1 text-xs text-muted-foreground">尚未保存；失焦或回车保存</p>}
    {(draft === null || value === saved) && <FieldFeedback field={field} />}{preview?.(value)}
  </Row>;
}
function CommitTemplateCard() {
  const { settings } = useSettings();
  return <Group>
    <TextSetting field="hostName" title="本机名称" maxLength={40} placeholder={settings?.defaultHostName}
      description={`提交模板中的 {host} 使用此名称；留空使用系统电脑名${settings?.defaultHostName ? ` ${settings.defaultHostName}` : ""}。`} />
    <TextSetting field="commitTemplate" title="自动提交信息模板" placeholder={DEFAULT_COMMIT_TEMPLATE}
      description="{date} 表示同步时的本地时间，{host} 表示本机名称。留空恢复默认模板。"
      preview={(value) => <div className="mt-3 rounded-md bg-muted/50 px-3 py-2 text-xs break-words"><span className="text-muted-foreground">预览：</span><span className="font-mono select-text">{renderCommitMessage(value, new Date(), settings?.hostName || settings?.defaultHostName || "本机电脑名")}</span></div>} />
  </Group>;
}
function MergeAndCheckCard() {
  const { settings } = useSettings();
  return <Group>
    <SwitchSetting item={{ key: "autoMerge", label: "无冲突时自动合并", description: <>
      分叉时先预演，确认没有冲突才自动合并并推送；真正冲突交给外部工具。手动“提交并推送”不会自动合并。
      <span className="mt-1 block break-words">合并信息：<code>{MERGE_MESSAGE_TEMPLATE.split("{host}").join(settings?.hostName || settings?.defaultHostName || "本机电脑名")}</code></span>
    </> }} />
    <SwitchSetting item={{ key: "precommitCheck", label: "自动提交前检查", description: "自动提交前检查超过上限的大文件和疑似密钥文件（如 .env、*.pem、*.key、id_rsa）。手动“提交并推送”不做这两类检查。所有提交入口都会阻止未解决冲突及未完成的 Git 操作，此保护不可关闭。" }} />
    <TextSetting field="bigFileLimitMb" title="大文件上限（MB）" number disabled={!settings?.precommitCheck}
      description="只用于自动提交前检查；关闭检查后不能编辑。" />
  </Group>;
}

function NetworkTimeoutCard() {
  const { settings, updateSettings } = useSettings();
  return <Row title="单条 Git 操作执行上限" description="限制每条联网 Git 操作（fetch、push、pull、clone）的完整执行时间，包括传输和处理；不是仅连接超时，也不是整轮同步总时限。刷新状态的单条 fetch 最多 30 秒（或设置值，以较短者为准）。">
    <div role="radiogroup" aria-label="联网操作执行上限" onKeyDown={radioKeys} className="inline-flex max-w-full flex-wrap gap-0.5 rounded-md border p-0.5">
      {NETWORK_TIMEOUT_CHOICES.map((seconds) => <button key={seconds} type="button" role="radio" aria-checked={settings?.networkTimeoutSecs === seconds}
        disabled={!settings} onClick={() => void updateSettings({ networkTimeoutSecs: seconds }).catch(() => {})}
        className={cn("rounded px-3 py-1 text-sm", settings?.networkTimeoutSecs === seconds ? "bg-foreground text-background" : "text-muted-foreground hover:text-foreground")}>{seconds / 60} 分钟</button>)}
    </div><FieldFeedback field="networkTimeoutSecs" />
    <p className="mt-2 text-xs leading-relaxed text-muted-foreground">手动刷新失败时保留已知状态并显示原因，可再次手动刷新。启动或唤醒后的自动同步仅对联网失败项目在 15 秒后重试一次，临时暂停会阻止自动重试。</p>
  </Row>;
}
function describeProxy(settings: Pick<Settings, "proxySource" | "effectiveProxy">): string {
  const labels = { git: "Git 配置或环境变量", system: "Windows 系统代理", manual: "手动填写", none: "不使用代理" };
  return `${labels[settings.proxySource]}${settings.effectiveProxy ? ` · ${settings.effectiveProxy}` : ""}`;
}
function ProxyCard() {
  const { settings, updateSettings, discardFailedSave } = useSettings();
  const savedMode = settings?.proxyMode ?? "auto";
  const savedUrl = settings?.proxyUrl ?? "";
  const [draft, setDraft] = useState<{ mode: ProxyMode; url: string } | null>(null);
  const [saving, setSaving] = useState(false);
  const mode = draft?.mode ?? savedMode;
  const url = draft?.url ?? savedUrl;
  const dirty = mode !== savedMode || url !== savedUrl;
  async function apply() {
    const submitted = { mode, url };
    setSaving(true);
    try {
      await updateSettings({ proxyMode: mode, proxyUrl: url.trim() });
      setDraft((current) => current?.mode === submitted.mode && current.url === submitted.url ? null : current);
    } catch { /* 保留草稿供调整，运行时仍使用保存前的值。 */ }
    finally { setSaving(false); }
  }
  return <Row title="代理" description="自动模式优先使用 Git 配置或代理环境变量，其次使用 Windows 系统代理。模式和地址作为一组草稿，点击“应用”才生效；不会修改全局 Git 配置。">
    <div role="radiogroup" aria-label="代理模式" onKeyDown={radioKeys} className="inline-flex max-w-full flex-wrap gap-0.5 rounded-md border p-0.5">
      {([{ mode: "auto", label: "自动" }, { mode: "off", label: "不用代理" }, { mode: "manual", label: "手动填写" }] as const).map((option) =>
        <button key={option.mode} type="button" role="radio" aria-checked={mode === option.mode} disabled={saving || !settings}
          onClick={() => setDraft({ mode: option.mode, url })}
          className={cn("rounded px-3 py-1 text-sm", mode === option.mode ? "bg-foreground text-background" : "text-muted-foreground hover:text-foreground")}>{option.label}</button>)}
    </div>
    {mode === "manual" && <Input className="mt-3 font-mono" value={url} aria-label="代理地址" disabled={saving || !settings}
      placeholder="如 http://127.0.0.1:7890 或 socks5://127.0.0.1:1080"
      onChange={(event) => setDraft({ mode, url: event.target.value })} />}
    <div className="mt-3 flex flex-wrap items-center gap-2">
      <Button disabled={saving || !settings || !dirty} onClick={() => void apply()}>{saving && <Loader2 className="animate-spin" />}应用</Button>
      <Button variant="outline" disabled={saving || !dirty} onClick={() => {
        setDraft(null);
        discardFailedSave(["proxyMode", "proxyUrl"]);
      }}>放弃修改</Button>
      {dirty && <span className="text-xs text-muted-foreground">草稿尚未生效</span>}
    </div><FieldFeedback field="proxyMode" />
    <p className="mt-3 text-sm break-all">当前生效：{settings ? describeProxy(settings) : "正在检测…"}</p>
    <p className="mt-1 text-xs leading-relaxed text-muted-foreground">只影响 HTTPS 仓库；SSH 地址和仓库单独配置的 remote.&lt;远程名&gt;.proxy 不受此处影响。保存失败时继续使用此前已保存的设置。</p>
  </Row>;
}
function DiagnosticsCard() {
  const { settings, reloadSettings } = useSettings();
  const [git, setGit] = useState<GitInfo | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  async function inspect() {
    setBusy(true); setError(null);
    try { const info = await api.checkGit(); setGit(info); await reloadSettings(); }
    catch (cause) { setError(errorMessage(cause)); }
    finally { setBusy(false); }
  }
  useEffect(() => { void inspect(); }, []);
  return <Group><Row title="当前环境" control={<Button variant="outline" disabled={busy} onClick={() => void inspect()}><RefreshCw className={busy ? "animate-spin" : ""} />重新检测</Button>}>
    <dl className="space-y-2 text-sm"><div><dt className="text-xs text-muted-foreground">Git 版本</dt><dd className="mt-0.5 break-words select-text">{git ? git.installed ? git.version : "未检测到 Git，请安装后重启应用" : "正在检测…"}</dd></div>
      <div><dt className="text-xs text-muted-foreground">当前代理来源</dt><dd className="mt-0.5 break-all select-text">{settings ? describeProxy(settings) : "正在检测…"}</dd></div></dl>
    {error && <p role="alert" className="mt-2 text-xs break-words text-destructive">检测失败：{error}</p>}
    <p className="mt-3 text-xs leading-relaxed text-muted-foreground">具体项目的异常可在首页项目详情或同步结果中查看；“复制诊断文本”包含分支、上游、文件名和最近提交，不含文件内容。需要处理冲突时，可从同一处打开终端或编辑器。</p>
  </Row></Group>;
}

function formatBytes(bytes: number) {
  return bytes >= 1024 * 1024 ? `${(bytes / 1024 / 1024).toFixed(1)} MB` : `${Math.max(1, Math.round(bytes / 1024))} KB`;
}
function AboutSection() {
  const { version, update, phase, error, progress, downloaded, check, download, install } = useUpdates();
  const { tasks } = useTasks();
  const busy = phase === "checking" || phase === "downloading" || phase === "installing" || phase === "installed";
  const percent = progress?.total ? Math.min(100, Math.round(progress.downloaded / progress.total * 100)) : null;
  const repositoryBusy = !!tasks.active || tasks.waiting.length > 0 || tasks.installingUpdate;
  return <Group>
    <Row title="Git SyncDeck" description={`当前版本 ${version ? `v${version}` : "未知"}`} control={
      <Button variant="outline" disabled={busy || downloaded} onClick={() => void check()}>{phase === "checking" ? <Loader2 className="animate-spin" /> : <RefreshCw />}{phase === "checking" ? "检查中…" : "检查更新"}</Button>} />
    {phase === "latest" && <p role="status" className="flex items-center gap-1.5 px-4 py-3 text-sm text-muted-foreground"><Check className="size-4" />已是最新版本</p>}
    {error && <div role="alert" className="flex gap-1.5 px-4 py-3 text-sm text-destructive"><CircleAlert className="mt-0.5 size-4 shrink-0" /><span className="min-w-0 break-words">{error}</span></div>}
    {update && <Row title={`发现新版本 v${update.version}`} description={update.date ? `发布于 ${new Date(update.date).toLocaleDateString("zh-CN")}` : undefined}>
      {update.notes && <div className="mb-3"><div className="text-xs font-medium text-muted-foreground">更新说明</div><p className="mt-1 max-h-48 overflow-y-auto text-sm break-words whitespace-pre-wrap select-text">{update.notes}</p></div>}
      <div className="flex flex-wrap gap-2">
        {downloaded ? <Button disabled={busy || repositoryBusy} onClick={() => void install()}>{phase === "installing" && <Loader2 className="animate-spin" />}{phase === "installed" ? "已完成安装" : phase === "installing" ? "正在安装…" : "安装并重启"}</Button>
          : <Button disabled={busy} onClick={() => void download()}>{phase === "downloading" ? <Loader2 className="animate-spin" /> : <Download />}{phase === "downloading" ? "正在下载…" : "下载更新"}</Button>}
      </div>
      <p className="mt-2 text-xs leading-relaxed text-muted-foreground">下载不会安装或重启。安装需等待所有仓库任务结束，取得安装资格后会阻止新任务进入，并重启应用。</p>
      {downloaded && repositoryBusy && phase !== "installing" && <p role="status" className="mt-2 text-xs text-status-ahead">{tasks.active ? `请等待“${tasks.active.label}”结束后安装。` : "请等待排队中的仓库任务结束后安装。"}</p>}
      {(phase === "downloading" || (phase === "error" && progress && !downloaded)) && <div className="mt-3 space-y-1.5"><Progress value={percent ?? 0} aria-label="更新下载进度" /><p className="text-xs text-muted-foreground">{progress ? progress.total ? `已下载 ${formatBytes(progress.downloaded)} / ${formatBytes(progress.total)}（${percent}%）` : `已下载 ${formatBytes(progress.downloaded)}` : "正在连接…"}</p></div>}
      {phase === "downloaded" && <p role="status" className="mt-2 text-xs text-muted-foreground">更新已下载，等待你点击安装。</p>}
      {phase === "installed" && <p role="status" className="mt-2 text-xs text-muted-foreground">{api.isDemoMode ? "演示安装已完成，未更改实际应用。" : "已请求重启应用。"}</p>}
    </Row>}
    <Row title="项目主页" description="Owenwoow/Git-SyncDeck" control={<Button variant="outline" onClick={() => void api.openProjectHomepage().catch((cause) => toast.error("无法打开项目主页", { description: errorMessage(cause) }))}><ExternalLink />访问仓库</Button>} />
  </Group>;
}
